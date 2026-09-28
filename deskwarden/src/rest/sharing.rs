//! **The sharing engine**: personal sharing on NodeWarden, run by the daemon.
//!
//! `docs/superpowers/specs/2026-09-27-personal-sharing-design.md` is the
//! design; its "Owner's decisions" block overrides the rest. In one line:
//! Share files a record into a per-person collection of a hidden organisation
//! the sharer owns, and the recipient gets it view-only.
//!
//! # Where it runs, and why
//!
//! Inside `rest::backend::RestBackend`, which holds the session, the vault
//! keys and -- through the profile -- the private key every step needs
//! (spec 11.2). `VaultBackend::sharing` hands it to the daemon's API as an
//! `Arc`; a `bw serve` backend has none, and says so by returning `None`.
//! It shares the backend's key cache rather than keeping a second one, and
//! holds the session by a WEAK handle, as the notifications hub does: an
//! engine cannot keep a switched-away account's session alive.
//!
//! # What it holds, and what it reads fresh
//!
//! It holds only what the last whole read already paid for (the sharing
//! org's roster and person collections, handed over by `synced` through
//! [`Sharing::note_directory`]), the dialect probe's answer, the recipient's
//! incoming invitations, and two sets the confirm pass learns (keys that
//! changed, accounts with no key yet). Every WRITE reads current server
//! state first and does only the missing steps (spec 13), so a Retry is the
//! same call and two PCs converge.
//!
//! # NodeWarden only, in this build
//!
//! The owner's decision: slices 2 and 3 target NodeWarden. The probe tells
//! the dialects apart (spec 7.1), and on anything but NodeWarden with the
//! slice-1 routes the view says `available: false` and every write refuses.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use serde::{Deserialize, Serialize};

use crate::rest::api::{Authenticated, RestClient, RestError};
use crate::rest::crypto::{CryptoError, EncString, SymmetricKey};
use crate::rest::organizations::{
    Directory, Member, MemberStatus, Roster, collection_ids_of, normalise_email, organization_of, sharer_of,
};
use crate::rest::sharing_api::{self, IncomingInvitation};
use crate::rest::sync::{Profile, VaultKeys};
use crate::vault_bridge::{VaultError, VaultItem};

// ---- what the daemon answers `GET /sharing` with --------------------------------

/// Why sharing is not available. Spec 11.3's reasons, plus `not-nodewarden`:
/// the owner's decision leaves official servers for a later slice, and the
/// spec's list had no word for that.
pub const REASON_BW_SERVE: &str = "bw-serve";
pub const REASON_NOT_NODEWARDEN: &str = "not-nodewarden";
pub const REASON_TOO_OLD: &str = "server-too-old";
pub const REASON_NOT_LOADED: &str = "not-loaded";

/// The one dialect this build shares on.
pub const DIALECT_NODEWARDEN: &str = "nodewarden";

/// Spec 6.1's per-person states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PersonState {
    /// Confirmed, with access to their collection: in their vault now.
    Shared,
    /// Accepted, confirm not run yet.
    WaitingAccept,
    /// NodeWarden: a pending invitation, no account for the email.
    WaitingAccount,
    /// Accepted, but the account has no public key yet.
    WaitingSignIn,
    /// The invitation's expiry has passed.
    Expired,
    /// Accepted, and the public key differs from the pin: not confirmed.
    KeyChanged,
    /// Their collection exists and no membership reaches it.
    Gone,
}

/// One person this account shares with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonEntry {
    pub email: String,
    pub state: PersonState,
    /// An invitation's expiry, ISO 8601.
    pub expires_at: Option<String>,
}

/// One record this account shares, and with whom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordEntry {
    pub item_id: String,
    pub people: Vec<String>,
}

/// A record someone shares with this account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedEntry {
    pub item_id: String,
    pub from: String,
}

/// An invitation addressed to this account, from a sharer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEntry {
    pub id: String,
    pub from: String,
    pub expires_at: Option<String>,
}

/// A sharing org this account accepted and its sharer has not confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingEntry {
    pub organization_id: String,
    pub from: String,
}

/// **Everything the window draws about sharing**, answered by `GET /sharing`
/// out of the engine's held state and the daemon's items -- no server round
/// trip (spec 11.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SharingView {
    pub available: bool,
    pub reason: Option<String>,
    pub dialect: Option<String>,
    /// Always false on NodeWarden (slice 1 makes rosters manager-only).
    pub roster_visible_to_recipients: bool,
    /// The account is NodeWarden's administrator and may mint invite codes.
    pub can_mint_invites: bool,
    pub people: Vec<PersonEntry>,
    pub records: Vec<RecordEntry>,
    pub received: Vec<ReceivedEntry>,
    pub incoming: Vec<IncomingEntry>,
    pub waiting: Vec<WaitingEntry>,
}

impl SharingView {
    /// Sharing is off, and why.
    #[must_use]
    pub fn unavailable(reason: &str) -> Self {
        Self { reason: Some(reason.to_string()), ..Self::default() }
    }

    /// One person, by email in any case.
    #[must_use]
    pub fn person(&self, email: &str) -> Option<&PersonEntry> {
        let email = normalise_email(email);
        self.people.iter().find(|p| p.email == email)
    }

    /// The people one record is shared with, in the record's order.
    #[must_use]
    pub fn people_of(&self, item_id: &str) -> Vec<&PersonEntry> {
        self.records
            .iter()
            .find(|r| r.item_id == item_id)
            .map(|r| r.people.iter().filter_map(|email| self.person(email)).collect())
            .unwrap_or_default()
    }

    /// **The pill's promise** (spec 6.1): at least one person has it now.
    #[must_use]
    pub fn shared_with_someone(&self, item_id: &str) -> bool {
        self.people_of(item_id).iter().any(|p| p.state == PersonState::Shared)
    }

    /// Who shared a received record.
    #[must_use]
    pub fn received_from(&self, item_id: &str) -> Option<&str> {
        self.received.iter().find(|r| r.item_id == item_id).map(|r| r.from.as_str())
    }
}

// ---- the engine's own types -----------------------------------------------------

/// What set off a pass (spec 11.5's triggers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A whole read: unlock, `POST /sync`, the hub's populate.
    WholeRead,
    /// An applied hub batch.
    Batch,
    /// The ten-minute timer.
    Timer,
    /// The engine's own write.
    Own,
}

/// Why a sharing call did not happen.
#[derive(Debug)]
pub enum SharingError {
    /// Sharing is not available here; the reason is one of the `REASON_*`s.
    Unavailable(&'static str),
    /// A step was refused, with the sentence to show.
    Refused(String),
    /// The vault backend's own failure (session gone, server unreachable).
    Vault(VaultError),
}

impl std::fmt::Display for SharingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "sharing is not available here ({reason})"),
            Self::Refused(sentence) => f.write_str(sentence),
            Self::Vault(e) => write!(f, "{e:?}"),
        }
    }
}

/// Items a sharing call changed on the server, for the daemon's cache.
#[derive(Debug, Clone, Default)]
pub struct SharingOutcome {
    /// Changed or new, as the server answered them, decrypted.
    pub upserted: Vec<VaultItem>,
    /// Ids that are gone.
    pub removed: Vec<String>,
}

impl SharingOutcome {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.upserted.is_empty() && self.removed.is_empty()
    }
}

/// What one pass did.
#[derive(Debug, Clone, Default)]
pub struct SharingPass {
    /// The view may read differently now.
    pub changed: bool,
    /// Items the pass moved (a finished move back).
    pub outcome: SharingOutcome,
}

/// Makes an organisation's RSA pair. `crypto::generate_org_key_pair` in
/// production; a fixture pair in tests, because RSA-2048 generation takes
/// seconds in a debug build and every first-share test would pay it.
pub type KeyPairMaker = fn(&SymmetricKey) -> Result<(String, EncString), CryptoError>;

/// Where the engine's files are, and how it makes a key pair.
#[derive(Clone)]
pub struct SharingSetup {
    pub state_path: Option<PathBuf>,
    pub sent_path: Option<PathBuf>,
    pub fill_stats_path: Option<PathBuf>,
    pub make_key_pair: KeyPairMaker,
}

impl SharingSetup {
    /// The real files, in the config directory the daemon uses for all three.
    #[must_use]
    pub fn production() -> Self {
        Self {
            state_path: crate::sharing_state::default_path(),
            sent_path: crate::sent_history::default_path(),
            fill_stats_path: crate::settings::config_dir().map(|dir| dir.join("fill-stats.json")),
            make_key_pair: crate::rest::crypto::generate_org_key_pair,
        }
    }

    /// No files at all: nothing is persisted. What a test starts from, and
    /// it fills in scratch paths where the test is about a file.
    #[must_use]
    pub fn none() -> Self {
        Self {
            state_path: None,
            sent_path: None,
            fill_stats_path: None,
            make_key_pair: crate::rest::crypto::generate_org_key_pair,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    NodeWarden,
    Official,
}

/// The probe's answer, once per engine (spec 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Probe {
    pub(crate) dialect: Dialect,
    /// NodeWarden answers `GET /api/users/{id}/public-key`: the slice-1 routes
    /// (read-only collections, `/share`, `/users`) are there.
    pub(crate) slice_one: bool,
}

/// The signed-in account, as the profile says.
#[derive(Debug, Clone, Default)]
pub(crate) struct Account {
    pub(crate) id: String,
    /// Task 9 (share) reads it first: the sharing org's name and billing email.
    #[allow(dead_code)]
    pub(crate) email: String,
    pub(crate) admin: bool,
}

/// This account's own sharing org, as the last read left it.
#[derive(Debug, Clone)]
pub(crate) struct MyOrg {
    pub(crate) id: String,
    /// `(collection id, normalised email)`, in server order.
    pub(crate) collections: Vec<(String, String)>,
    pub(crate) roster: Vec<Member>,
}

impl MyOrg {
    fn collection_for(&self, email: &str) -> Option<&str> {
        let email = normalise_email(email);
        self.collections.iter().find(|(_, e)| *e == email).map(|(id, _)| id.as_str())
    }

    fn email_of(&self, collection_id: &str) -> Option<&str> {
        self.collections.iter().find(|(id, _)| id == collection_id).map(|(_, e)| e.as_str())
    }

    fn member(&self, email: &str) -> Option<&Member> {
        let email = normalise_email(email);
        self.roster.iter().find(|m| normalise_email(&m.email) == email)
    }
}

/// What the engine holds between passes. See the module docs.
///
/// **A hand-written `Debug`, and it must have one** -- not for a caller, but
/// as `crate::debug_leak_guard`'s barrier. `incoming` holds invitation tokens
/// (`Zeroizing`), so to the guard `Held` reaches a secret; with no `Debug` of
/// its own it would pass that on to [`Sharing`]. The guard matches types by
/// bare name, so without this impl it flagged types that merely NAME a
/// `Sharing` as deriving `Debug` over a secret: the detail pane's `Section`
/// (its `Section::Sharing` variant) and, by further name matches,
/// `change_feed`'s `Change` and `FeedPage`. Refusing here, at the one type
/// that holds the tokens, un-flags all three in the right place; the impl
/// prints the invitations as a count and never a token.
#[derive(Clone, Default)]
pub(crate) struct Held {
    pub(crate) probe: Option<Probe>,
    pub(crate) account: Option<Account>,
    pub(crate) my_org: Option<MyOrg>,
    /// Other people's sharing orgs: `(org id, sharer)`.
    pub(crate) others: Vec<(String, String)>,
    /// Accepted, unconfirmed: `(org id, sharer)`.
    pub(crate) waiting: Vec<(String, String)>,
    pub(crate) incoming: Vec<IncomingInvitation>,
    /// Emails whose key differs from the pin.
    pub(crate) key_changed: BTreeSet<String>,
    /// Emails whose account has no public key yet.
    pub(crate) no_key: BTreeSet<String>,
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Held")
            .field("probe", &self.probe)
            .field("account", &self.account)
            .field("my_org", &self.my_org)
            .field("others", &self.others)
            .field("waiting", &self.waiting)
            .field("incoming", &format_args!("<{} invitations>", self.incoming.len()))
            .field("key_changed", &self.key_changed)
            .field("no_key", &self.no_key)
            .finish()
    }
}

/// **The engine.** See the module docs.
pub struct Sharing {
    pub(crate) client: RestClient,
    state: Weak<Mutex<Authenticated>>,
    /// Task 9 (share) is its first reader, through `keys_from`.
    #[allow(dead_code)]
    keys: Arc<Mutex<Option<Arc<VaultKeys>>>>,
    held: Mutex<Held>,
    setup: Mutex<SharingSetup>,
    /// One engine operation at a time: a share and a confirm pass must not
    /// interleave their reads and writes of the one org.
    ops: Mutex<()>,
}

impl Sharing {
    /// An engine over `RestBackend`'s client, session and key cache. Makes no
    /// request: `RestBackend::new`'s promise that constructing a backend
    /// touches no network holds.
    #[must_use]
    pub fn new(
        client: RestClient,
        state: Weak<Mutex<Authenticated>>,
        keys: Arc<Mutex<Option<Arc<VaultKeys>>>>,
    ) -> Self {
        Self {
            client,
            state,
            keys,
            held: Mutex::new(Held::default()),
            setup: Mutex::new(SharingSetup::production()),
            ops: Mutex::new(()),
        }
    }

    /// Where the files are. Production uses [`SharingSetup::production`]; a
    /// test points them into a scratch directory, or at nothing.
    pub fn set_up(&self, setup: SharingSetup) {
        *self.setup.lock().unwrap_or_else(PoisonError::into_inner) = setup;
    }

    pub(crate) fn setup(&self) -> SharingSetup {
        self.setup.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn one_at_a_time(&self) -> std::sync::MutexGuard<'_, ()> {
        self.ops.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One request, with the session locked for exactly its length -- the
    /// backend's own lock discipline. **Never called with `held` locked.**
    pub(crate) fn call<T>(
        &self,
        request: impl FnOnce(&RestClient, &mut Authenticated) -> Result<T, RestError>,
    ) -> Result<T, SharingError> {
        let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        request(&self.client, &mut guard).map_err(|e| SharingError::Vault(crate::rest::backend::rest_error(e)))
    }

    /// `GET /api/accounts/profile`, and what it says about the account.
    pub(crate) fn profile(&self) -> Result<Profile, SharingError> {
        let profile = self.call(|client, auth| client.profile(&mut auth.session))?;
        self.note_account(&profile);
        Ok(profile)
    }

    fn note_account(&self, profile: &Profile) {
        if let (Some(id), Some(email)) = (profile.id.clone(), profile.email.clone()) {
            self.held().account = Some(Account { id, email, admin: profile.is_server_admin() });
        }
    }

    /// The vault keys, unwrapped from `profile` with the session's master key
    /// and stored in the cache the backend reads too.
    ///
    /// `#[allow(dead_code)]` until Task 9 (share) gives it its first caller.
    #[allow(dead_code)]
    pub(crate) fn keys_from(&self, profile: &Profile) -> Result<Arc<VaultKeys>, SharingError> {
        let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
        let (keys, _) = {
            let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            VaultKeys::unwrap_from(&guard.master_key, profile)
                .map_err(|e| SharingError::Vault(crate::rest::backend::crypto_error(e)))?
        };
        let keys = Arc::new(keys);
        *self.keys.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&keys));
        Ok(keys)
    }

    /// **What the last whole read already paid for**, handed over by
    /// `RestBackend::synced` before it files the directory: this account's
    /// sharing org, its person collections (names already decrypted) and its
    /// roster, other people's sharing orgs, and the accepted-but-waiting ones.
    /// The confirm pass reads its roster here, so a whole read costs it no
    /// roster request of its own (spec 11.5).
    pub fn note_directory(&self, directory: &Directory, profile: Option<&Profile>) {
        if let Some(profile) = profile {
            self.note_account(profile);
        }
        let cached = self.cached_org_id();
        let mine = cached
            .as_deref()
            .and_then(|id| directory.sharing_orgs().into_iter().find(|o| o.id == id && o.sharing.as_ref().is_some_and(|s| s.mine)))
            .or_else(|| directory.my_sharing_org());
        let mut held = self.held();
        // The previous roster, kept for an org whose roster this read could
        // not fetch: a known roster is better than none for the confirm pass.
        let previous = held.my_org.take();
        held.my_org = mine.map(|org| MyOrg {
            id: org.id.clone(),
            collections: directory
                .collections_of(&org.id)
                .into_iter()
                .filter_map(|c| c.name.as_deref().map(|name| (c.id.clone(), normalise_email(name))))
                .collect(),
            roster: match directory.roster_of(&org.id) {
                Roster::Known(members) => members.clone(),
                Roster::Unknown => previous.as_ref().filter(|o| o.id == org.id).map(|o| o.roster.clone()).unwrap_or_default(),
            },
        });
        held.others = directory
            .sharing_orgs()
            .into_iter()
            .filter_map(|o| o.sharing.as_ref().filter(|s| !s.mine).map(|s| (o.id.clone(), s.sharer.clone())))
            .collect();
        held.waiting = directory
            .waiting()
            .iter()
            .filter_map(|o| o.sharing.as_ref().map(|s| (o.id.clone(), s.sharer.clone())))
            .collect();
    }

    /// The org id `sharing_state.json` remembers for this account.
    pub(crate) fn cached_org_id(&self) -> Option<String> {
        let path = self.setup().state_path?;
        let account = self.held().account.as_ref().map(|a| a.id.clone())?;
        crate::sharing_state::SharingState::load(&path).account(&account).sharing_org_id
    }

    /// The view -- pure over what is held and the daemon's items.
    #[must_use]
    pub fn view(&self, items: &[VaultItem]) -> SharingView {
        let held = self.held().clone();
        let Some(probe) = held.probe else {
            return SharingView::unavailable(REASON_NOT_LOADED);
        };
        if probe.dialect != Dialect::NodeWarden {
            return SharingView::unavailable(REASON_NOT_NODEWARDEN);
        }
        if !probe.slice_one {
            return SharingView::unavailable(REASON_TOO_OLD);
        }
        let now = now_millis();
        let mut view = SharingView {
            available: true,
            dialect: Some(DIALECT_NODEWARDEN.to_string()),
            can_mint_invites: held.account.as_ref().is_some_and(|a| a.admin),
            ..SharingView::default()
        };
        if let Some(org) = &held.my_org {
            for (collection_id, email) in &org.collections {
                if view.people.iter().any(|p| &p.email == email) {
                    continue;
                }
                let member = org.member(email);
                view.people.push(PersonEntry {
                    email: email.clone(),
                    state: person_state(member, collection_id, &held, now),
                    expires_at: member.and_then(|m| m.invitation_expires.clone()),
                });
            }
            for item in items {
                if organization_of(item) != Some(org.id.as_str()) {
                    continue;
                }
                let mut people: Vec<String> = Vec::new();
                for id in collection_ids_of(item) {
                    if let Some(email) = org.email_of(id) {
                        if !people.iter().any(|p| p == email) {
                            people.push(email.to_string());
                        }
                    }
                }
                view.records.push(RecordEntry { item_id: item.id.clone(), people });
            }
        }
        for item in items {
            if let Some(org) = organization_of(item) {
                if let Some((_, sharer)) = held.others.iter().find(|(id, _)| id == org) {
                    view.received.push(ReceivedEntry { item_id: item.id.clone(), from: sharer.clone() });
                }
            }
        }
        view.incoming = held
            .incoming
            .iter()
            .filter_map(|invitation| {
                let from = invitation.organization_name.as_deref().and_then(sharer_of)?;
                Some(IncomingEntry { id: invitation.id.clone(), from: from.to_string(), expires_at: invitation.expires_at.clone() })
            })
            .collect();
        view.waiting = held
            .waiting
            .iter()
            .map(|(organization_id, from)| WaitingEntry { organization_id: organization_id.clone(), from: from.clone() })
            .collect();
        view
    }

    /// **The probe**, once per engine: `GET /invitations/mine` tells
    /// NodeWarden from the rest, and the own-key route tells NodeWarden with
    /// slice 1 from an older one (spec 7.1). A transport failure is not
    /// cached: the next pass asks again.
    pub(crate) fn ensure_probe(&self) -> Result<Probe, SharingError> {
        if let Some(probe) = self.held().probe {
            return Ok(probe);
        }
        let invitations = self.call(|client, auth| sharing_api::my_invitations(client, &mut auth.session))?;
        let probe = match invitations {
            None => Probe { dialect: Dialect::Official, slice_one: false },
            Some(list) => {
                self.held().incoming = list;
                let own = self.account_id()?;
                let key = self.call(|client, auth| sharing_api::user_public_key(client, &mut auth.session, &own))?;
                Probe { dialect: Dialect::NodeWarden, slice_one: key.is_some() }
            }
        };
        self.held().probe = Some(probe);
        Ok(probe)
    }

    /// Refuses unless this is NodeWarden with slice 1.
    ///
    /// `#[allow(dead_code)]` until Task 9 (share) gives it its first caller.
    #[allow(dead_code)]
    pub(crate) fn require_available(&self) -> Result<(), SharingError> {
        let probe = self.ensure_probe()?;
        if probe.dialect != Dialect::NodeWarden {
            return Err(SharingError::Unavailable(REASON_NOT_NODEWARDEN));
        }
        if !probe.slice_one {
            return Err(SharingError::Unavailable(REASON_TOO_OLD));
        }
        Ok(())
    }

    /// The account's id, from the last profile, or read now.
    pub(crate) fn account_id(&self) -> Result<String, SharingError> {
        if let Some(account) = self.held().account.clone() {
            return Ok(account.id);
        }
        self.profile()?;
        self.held()
            .account
            .clone()
            .map(|a| a.id)
            .ok_or_else(|| SharingError::Refused("The server did not say which account this is.".to_string()))
    }

    /// **One pass** (spec 11.5): the probe, then the incoming invitations on a
    /// whole read or the timer. The confirm pass and unfinished moves join it
    /// in their own tasks.
    ///
    /// # Errors
    ///
    /// The probe could not be made; a later pass tries again.
    pub fn refresh(&self, trigger: Trigger) -> Result<SharingPass, SharingError> {
        let _one = self.one_at_a_time();
        let had_probe = self.held().probe.is_some();
        let probe = self.ensure_probe()?;
        let mut pass = SharingPass { changed: !had_probe, ..SharingPass::default() };
        if probe.dialect != Dialect::NodeWarden || !probe.slice_one {
            return Ok(pass);
        }
        if had_probe && matches!(trigger, Trigger::WholeRead | Trigger::Timer) {
            if let Some(list) = self.call(|client, auth| sharing_api::my_invitations(client, &mut auth.session))? {
                let mut held = self.held();
                let before: Vec<String> = held.incoming.iter().map(|i| i.id.clone()).collect();
                pass.changed |= before != list.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
                held.incoming = list;
            }
        }
        Ok(pass)
    }

    /// Whether the ten-minute timer has anything to do: someone in this
    /// account's org is invited or accepted and not yet confirmed.
    #[must_use]
    pub fn wants_timer(&self) -> bool {
        let held = self.held();
        held.my_org.as_ref().is_some_and(|org| {
            org.roster.iter().any(|m| m.status != MemberStatus::Confirmed && org.collection_for(&m.email).is_some())
        })
    }
}

/// Spec 6.1's table, as a function of the roster row, the collection and
/// what the confirm pass learned.
fn person_state(member: Option<&Member>, collection_id: &str, held: &Held, now: i64) -> PersonState {
    let Some(member) = member else {
        return PersonState::Gone;
    };
    let email = normalise_email(&member.email);
    match member.status {
        MemberStatus::Invited => {
            let expired = member
                .invitation_expires
                .as_deref()
                .and_then(crate::send::parse_iso_utc_millis)
                .is_some_and(|at| at < now);
            if expired { PersonState::Expired } else { PersonState::WaitingAccount }
        }
        MemberStatus::Accepted if held.key_changed.contains(&email) => PersonState::KeyChanged,
        MemberStatus::Accepted if held.no_key.contains(&email) => PersonState::WaitingSignIn,
        MemberStatus::Accepted => PersonState::WaitingAccept,
        MemberStatus::Confirmed if member.has_access(collection_id) => PersonState::Shared,
        MemberStatus::Confirmed => PersonState::Gone,
    }
}

/// Now, UTC milliseconds.
pub(crate) fn now_millis() -> i64 {
    use crate::send::SendClock;
    crate::send::SystemClock.now_unix_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::backend::tests::{
        FIXTURE_ORG, FIXTURE_USER_ID, fixture_key_pair, sharing_profile, signed_in_with_no_sync_route,
    };

    /// An engine with no server behind it, for the pure view.
    fn offline() -> Sharing {
        Sharing::new(crate::rest::api::RestClient::new("http://127.0.0.1:1"), Weak::new(), Arc::new(Mutex::new(None)))
    }

    fn member(email: &str, status: MemberStatus, collections: &[&str], expires: Option<&str>) -> Member {
        Member {
            id: format!("ou-{email}"),
            user_id: (status != MemberStatus::Invited).then(|| format!("u-{email}")),
            email: email.to_string(),
            name: None,
            role: "member".to_string(),
            status,
            collections: collections.iter().map(|c| (*c).to_string()).collect(),
            read_only: collections.iter().map(|c| (*c).to_string()).collect(),
            invitation_expires: expires.map(str::to_string),
        }
    }

    fn an_item(id: &str, org: &str, collections: &[&str]) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "organizationId": org, "collectionIds": collections
        }))
        .expect("an item")
    }

    /// **Every state of spec 6.1, read off the roster**, and every record's
    /// people read off its collections.
    #[test]
    fn every_person_state_is_read_off_the_roster_and_the_records_off_their_collections() {
        let engine = offline();
        {
            let mut held = engine.held();
            held.probe = Some(Probe { dialect: Dialect::NodeWarden, slice_one: true });
            held.account = Some(Account { id: "me".into(), email: "me@x.io".into(), admin: true });
            held.my_org = Some(MyOrg {
                id: "org1".into(),
                collections: ["anna", "bob", "carol", "dan", "eve", "fay", "gus", "hal"]
                    .iter()
                    .map(|n| (format!("c-{n}"), format!("{n}@x.io")))
                    .collect(),
                roster: vec![
                    member("anna@x.io", MemberStatus::Confirmed, &["c-anna"], None),
                    member("bob@x.io", MemberStatus::Accepted, &["c-bob"], None),
                    member("carol@x.io", MemberStatus::Invited, &["c-carol"], Some("2999-01-01T00:00:00Z")),
                    member("dan@x.io", MemberStatus::Invited, &["c-dan"], Some("2000-01-01T00:00:00Z")),
                    member("eve@x.io", MemberStatus::Accepted, &["c-eve"], None),
                    member("fay@x.io", MemberStatus::Accepted, &["c-fay"], None),
                    member("hal@x.io", MemberStatus::Confirmed, &[], None),
                ],
            });
            held.key_changed.insert("eve@x.io".into());
            held.no_key.insert("fay@x.io".into());
            held.others = vec![("org-anna".into(), "anna@x.io".into())];
        }
        let items = [an_item("r1", "org1", &["c-anna", "c-bob"]), an_item("r2", "org-anna", &[]), an_item("r3", "", &[])];
        let view = engine.view(&items);
        assert!(view.available && view.can_mint_invites);
        let state = |email: &str| view.person(email).map(|p| p.state);
        assert_eq!(state("anna@x.io"), Some(PersonState::Shared));
        assert_eq!(state("bob@x.io"), Some(PersonState::WaitingAccept));
        assert_eq!(state("carol@x.io"), Some(PersonState::WaitingAccount));
        assert_eq!(state("dan@x.io"), Some(PersonState::Expired));
        assert_eq!(state("eve@x.io"), Some(PersonState::KeyChanged));
        assert_eq!(state("fay@x.io"), Some(PersonState::WaitingSignIn));
        assert_eq!(state("gus@x.io"), Some(PersonState::Gone), "a collection and no member");
        assert_eq!(state("hal@x.io"), Some(PersonState::Gone), "a member without access to their collection");
        assert_eq!(view.records, vec![RecordEntry { item_id: "r1".into(), people: vec!["anna@x.io".into(), "bob@x.io".into()] }]);
        assert!(view.shared_with_someone("r1"), "anna is Shared, so the pill's promise holds");
        assert_eq!(view.received_from("r2"), Some("anna@x.io"));
        assert_eq!(view.received_from("r3"), None);
    }

    /// Before the probe has answered, sharing is "not loaded", never "on".
    #[test]
    fn an_engine_that_has_not_probed_says_not_loaded() {
        let view = offline().view(&[]);
        assert!(!view.available);
        assert_eq!(view.reason.as_deref(), Some(REASON_NOT_LOADED));
    }

    /// **The dialect probe is made once per session** (spec 7.1): the
    /// slice-one key route is asked once however many passes run. The
    /// invitation list doubles as the recipient's incoming list, so every
    /// whole read asks it again.
    #[test]
    fn the_dialect_probe_is_made_once_per_session() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup { make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).expect(1).create();
        let invitations = server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).expect(2).create();
        let key = server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "userId": FIXTURE_USER_ID, "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .expect(1)
            .create();
        engine.refresh(Trigger::WholeRead).expect("a first pass");
        engine.refresh(Trigger::WholeRead).expect("a second pass");
        engine.refresh(Trigger::Batch).expect("a batch pass asks nothing");
        invitations.assert();
        key.assert();
        assert!(engine.view(&[]).available);
        let _ = FIXTURE_ORG;
    }

    /// **An old NodeWarden without slice 1 turns sharing off** (spec 7.1):
    /// its own-key route answers 404, and without slice 1 a recipient could
    /// edit the sharer's records (spec 12.3).
    #[test]
    fn an_old_nodewarden_without_slice_one_turns_sharing_off() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str()).with_status(404).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        let view = engine.view(&[]);
        assert!(!view.available);
        assert_eq!(view.reason.as_deref(), Some(REASON_TOO_OLD));
    }

    /// An official server has no invitation list: sharing is not offered.
    #[test]
    fn an_official_server_is_told_sharing_is_nodewarden_only() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_status(404).create();
        let key = server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str()).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        key.assert();
        assert_eq!(engine.view(&[]).reason.as_deref(), Some(REASON_NOT_NODEWARDEN));
    }
}

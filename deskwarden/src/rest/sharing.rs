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
use crate::rest::sharing_api::{self, IncomingInvitation, KeyRoute};
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

// ---- the sentences a refused write says (spec 13) --------------------------------

pub const NOT_AN_EMAIL: &str = "That doesn't look like an email address.";
pub const YOURSELF: &str = "That's your own address.";
pub const IN_TRASH: &str = "This record is in the trash or the archive. Restore it before sharing it.";
pub const HAS_ATTACHMENTS: &str =
    "This record has attachments. Deskwarden can't share records with attachments yet.";
pub const UNREADABLE_PART: &str =
    "Part of this record could not be read, so it can't be moved. Open it in the Bitwarden web vault first.";
pub const OTHER_ORG: &str = "This record belongs to an organisation. Share it from there.";
pub const SHARE_FAILED: &str = "Couldn't share the record. It's still in your vault.";
pub const ORG_FAILED: &str = "Deskwarden couldn't start sharing on this server.";
/// **An account with no key pair** -- one made before its server kept keys,
/// and never opened since in an app that makes them. The probe leaves sharing
/// on for it (NodeWarden answers its own-key question with a 404 that says
/// nothing about the route; see [`Sharing::ensure_probe`]), so the refusal is
/// here, and says what is actually missing rather than blaming the server's
/// age or the organisation. Bitwarden's web vault makes the pair at sign-in.
pub const NO_KEY_PAIR: &str =
    "This account has no encryption keys for sharing yet. Sign in to the web vault once to create them, then try again.";

/// What a share ended as: the words the dialog chooses between (mock 6a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareResult {
    /// In their vault now.
    Shared,
    /// A member without the key yet: the confirm pass will finish it.
    WaitingAccept,
    /// No account: a pending invitation.
    WaitingAccount,
}

/// A share's result and what it changed.
#[derive(Debug, Clone)]
pub struct ShareOutcome {
    pub result: ShareResult,
    pub changed: SharingOutcome,
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

impl SharingError {
    /// A step's own sentence in place of a server failure. **A session that
    /// ended keeps its own error**, because the window answers that one
    /// differently (sign in again). A server sentence is kept after the
    /// step's, so "User is already a member" is not lost.
    #[must_use]
    pub(crate) fn or_say(self, sentence: &str) -> Self {
        match self {
            Self::Vault(VaultError::Unauthorized) | Self::Unavailable(_) => self,
            Self::Refused(server) if !server.is_empty() && server != sentence => {
                Self::Refused(format!("{sentence} The server said: {server}"))
            }
            _ => Self::Refused(sentence.to_string()),
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
    /// The sharing org's name and billing email, and what "that's your own
    /// address" is checked against.
    pub(crate) email: String,
    pub(crate) admin: bool,
    /// The profile carries a private key, so the account has a key pair and
    /// the server holds its public half. What lets the probe read a 404 from
    /// the own-key route as "no route" -- see [`Sharing::ensure_probe`].
    pub(crate) keyed: bool,
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
    /// The backend's key cache, which `keys_from` refreshes.
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
    ///
    /// A refusal that carries the server's own sentence becomes
    /// [`SharingError::Refused`] with it; every other failure is mapped as
    /// the backend maps it.
    pub(crate) fn call<T>(
        &self,
        request: impl FnOnce(&RestClient, &mut Authenticated) -> Result<T, RestError>,
    ) -> Result<T, SharingError> {
        let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        request(&self.client, &mut guard).map_err(|e| match e {
            RestError::Rejected { description, .. } if !description.is_empty() => SharingError::Refused(description),
            other => SharingError::Vault(crate::rest::backend::rest_error(other)),
        })
    }

    /// `GET /api/accounts/profile`, and what it says about the account.
    pub(crate) fn profile(&self) -> Result<Profile, SharingError> {
        let profile = self.call(|client, auth| client.profile(&mut auth.session))?;
        self.note_account(&profile);
        Ok(profile)
    }

    fn note_account(&self, profile: &Profile) {
        if let (Some(id), Some(email)) = (profile.id.clone(), profile.email.clone()) {
            self.held().account = Some(Account {
                id,
                email,
                admin: profile.is_server_admin(),
                keyed: profile.private_key.as_deref().is_some_and(|k| !k.is_empty()),
            });
        }
    }

    /// The vault keys, unwrapped from `profile` with the session's master key
    /// and stored in the cache the backend reads too.
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
    ///
    /// Built under the `held` lock rather than from a clone of [`Held`]: a
    /// clone would copy every incoming invitation's `Zeroizing` token only to
    /// read its id and drop it, and nothing here takes another lock or makes
    /// a request, so the guard is short.
    #[must_use]
    pub fn view(&self, items: &[VaultItem]) -> SharingView {
        let held = self.held();
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
    ///
    /// # Only a missing ROUTE is "too old"
    ///
    /// The answer is cached for the session, so misreading it turns sharing
    /// off until the next sign-in. A 200 is the route, with or without a key
    /// in it. A 405 or 501 is no route. A 404 is the hard one: NodeWarden's
    /// handler answers 404 for an account with no key pair too, and the status
    /// alone cannot tell the two apart. The profile can: an account whose
    /// profile carries a private key has its public half on the server, so
    /// its 404 is the route missing. A keyless account's 404 is read as the
    /// route present -- such an account can neither share nor be confirmed
    /// (both need a key pair), so the share flow refuses it when it asks for
    /// the keys, with a sentence about the keys rather than a wrong one about
    /// the server's age.
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
                let keyed = self.held().account.as_ref().is_some_and(|a| a.keyed);
                let route = self.call(|client, auth| sharing_api::key_route(client, &mut auth.session, &own))?;
                let slice_one = match route {
                    KeyRoute::Answered { .. } => true,
                    KeyRoute::Absent => false,
                    KeyRoute::NotFound => !keyed,
                };
                Probe { dialect: Dialect::NodeWarden, slice_one }
            }
        };
        self.held().probe = Some(probe);
        Ok(probe)
    }

    /// Refuses unless this is NodeWarden with slice 1.
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

/// Everything a write needs about the account and its sharing org, read
/// fresh at the start of the write (spec 13: every write reads current state
/// first).
///
/// **No `Debug`**: it holds the org key and the vault keys.
pub(crate) struct Ctx {
    pub(crate) account: Account,
    pub(crate) keys: Arc<VaultKeys>,
    pub(crate) org_id: String,
    pub(crate) org_key: SymmetricKey,
    /// `(collection id, normalised email)`.
    pub(crate) collections: Vec<(String, String)>,
}

impl Ctx {
    pub(crate) fn collection_for(&self, email: &str) -> Option<&str> {
        let email = normalise_email(email);
        self.collections.iter().find(|(_, e)| *e == email).map(|(id, _)| id.as_str())
    }
}

/// A crypto failure as a sharing error, mapped as the backend maps it.
pub(crate) fn crypto(e: CryptoError) -> SharingError {
    SharingError::Vault(crate::rest::backend::crypto_error(e))
}

/// Plausibly an email: one `@` with something either side, no whitespace.
fn plausible_email(email: &str) -> bool {
    let mut halves = email.split('@');
    matches!((halves.next(), halves.next(), halves.next()), (Some(l), Some(d), None) if !l.is_empty() && d.contains('.'))
        && !email.chars().any(char::is_whitespace)
}

/// A collection name read with the org key; a NodeWarden web-vault name
/// that is plaintext is read as it is.
fn collection_email(raw: &str, key: &SymmetricKey) -> Option<String> {
    match raw.parse::<EncString>() {
        Ok(enc) => {
            let bytes = crate::rest::crypto::decrypt(key, &enc).ok()?;
            std::str::from_utf8(&bytes).ok().map(normalise_email)
        }
        Err(_) => Some(normalise_email(raw)),
    }
}

/// **The refusals of spec 9.3**, before anything is sent. Decision 3 of this
/// build refuses every attachment, not only unkeyed ones: a record moved
/// back is re-created, and an attachment belongs to the old cipher id.
fn refuse_unmovable(
    record: &crate::rest::sync::DecryptedItem,
    failures: &[crate::rest::sync::DecryptFailure],
    my_org: Option<&str>,
) -> Result<(), SharingError> {
    let set = |key: &str| record.item.other.get(key).is_some_and(|v| !v.is_null());
    if set("deletedDate") || set("archivedDate") {
        return Err(SharingError::Refused(IN_TRASH.to_string()));
    }
    if record.item.other.get("attachments").and_then(serde_json::Value::as_array).is_some_and(|a| !a.is_empty()) {
        return Err(SharingError::Refused(HAS_ATTACHMENTS.to_string()));
    }
    if !failures.is_empty() {
        return Err(SharingError::Refused(UNREADABLE_PART.to_string()));
    }
    match organization_of(&record.item) {
        Some(org) if Some(org) != my_org => Err(SharingError::Refused(OTHER_ORG.to_string())),
        _ => Ok(()),
    }
}

/// The account the profile just named, refused when it has no key pair --
/// see [`NO_KEY_PAIR`]. Every owner's write asks this before its first key
/// is touched.
fn keyed_account(account: Option<Account>) -> Result<Account, SharingError> {
    let account = account.ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
    if !account.keyed {
        return Err(SharingError::Refused(NO_KEY_PAIR.to_string()));
    }
    Ok(account)
}

// ---- the owner's writes -----------------------------------------------------------

impl Sharing {
    /// This account's own sharing org among the profile's: the cached id while
    /// the account still owns it, else the first owned org named for it.
    pub(crate) fn my_org_id(&self, profile: &Profile, account: &Account) -> Option<String> {
        let owned: Vec<&crate::rest::sync::Organization> =
            profile.organizations.iter().filter(|o| o.is_owner()).collect();
        if let Some(cached) = self.cached_org_id() {
            if owned.iter().any(|o| o.id == cached) {
                return Some(cached);
            }
        }
        owned
            .iter()
            .find(|o| {
                o.name
                    .as_deref()
                    .and_then(sharer_of)
                    .is_some_and(|sharer| crate::rest::organizations::sharer_is(sharer, &account.email))
            })
            .map(|o| o.id.clone())
    }

    /// The org's person collections, decrypted: `(id, email)`.
    pub(crate) fn collections_of(&self, org: &str, key: &SymmetricKey) -> Result<Vec<(String, String)>, SharingError> {
        let listed = self.call(|client, auth| sharing_api::organization_collections(client, &mut auth.session, org))?;
        Ok(listed.into_iter().filter_map(|c| collection_email(&c.name, key).map(|email| (c.id, email))).collect())
    }

    /// The org's roster, fresh: `/users`, or `/members` on an older server.
    pub(crate) fn fetch_roster(&self, org: &str) -> Result<Vec<Member>, SharingError> {
        let answer = self.call(|client, auth| client.fetch_organization_members(&mut auth.session, org))?;
        Ok(crate::rest::organizations::map_members(&answer).unwrap_or_default())
    }

    /// One record, fresh from the server, decrypted with `keys`.
    pub(crate) fn fetch_record(
        &self,
        id: &str,
        keys: &VaultKeys,
    ) -> Result<(crate::rest::sync::DecryptedItem, Vec<crate::rest::sync::DecryptFailure>), SharingError> {
        let raw = self.call(|client, auth| client.fetch_cipher(&mut auth.session, id))?;
        crate::rest::sync::decrypt_cipher(&raw, keys)
            .ok_or_else(|| SharingError::Refused("This record could not be read.".to_string()))
    }

    /// The account and its sharing org, for a write that needs the org to
    /// exist already (stop, forget, reinvite, confirm).
    ///
    /// `#[allow(dead_code)]` until Task 10 (stop sharing) gives it its first
    /// caller; built here with the share flow whose steps it shares.
    #[allow(dead_code)]
    pub(crate) fn context(&self) -> Result<Ctx, SharingError> {
        self.require_available()?;
        let profile = self.profile()?;
        let account = keyed_account(self.held().account.clone())?;
        let keys = self.keys_from(&profile)?;
        let org_id = self
            .my_org_id(&profile, &account)
            .ok_or_else(|| SharingError::Refused("You are not sharing anything yet.".to_string()))?;
        let org_key = keys
            .organization(&org_id)
            .cloned()
            .ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        let collections = self.collections_of(&org_id, &org_key)?;
        Ok(Ctx { account, keys, org_id, org_key, collections })
    }

    /// **Creates the sharing org** (spec 9.1): a fresh org key wrapped to the
    /// account's own public key -- derived from its own private key, not
    /// asked of the server -- an RSA pair, and the first person's collection
    /// name. Returns the org's id and key.
    fn create_org(
        &self,
        profile: &Profile,
        account: &Account,
        keys: &VaultKeys,
        first_email: &str,
    ) -> Result<(String, SymmetricKey), SharingError> {
        let org_key = crate::rest::crypto::generate_symmetric_key().map_err(crypto)?;
        let private = crate::rest::sync::private_key_der(keys.user(), profile).map_err(crypto)?;
        let own_public = crate::rest::crypto::public_key_of(&private).map_err(crypto)?;
        let (public_key, encrypted_private_key) = (self.setup().make_key_pair)(&org_key).map_err(crypto)?;
        let body = sharing_api::NewOrganization {
            name: crate::rest::organizations::sharing_org_name(&account.email),
            billing_email: normalise_email(&account.email),
            key: crate::rest::crypto::wrap_key_to_public(&org_key, &own_public).map_err(crypto)?,
            public_key,
            encrypted_private_key,
            collection_name: crate::rest::crypto::encrypt(&org_key, first_email.as_bytes()).map_err(crypto)?,
        };
        let id = self
            .call(|client, auth| sharing_api::create_organization(client, &mut auth.session, &body))
            .map_err(|e| e.or_say(ORG_FAILED))?;
        Ok((id, org_key))
    }

    /// C(E), found by decrypted name or created with this account as its
    /// manager (spec 5.2). Returns its id.
    ///
    /// The manager is named by the account's USER id: on NodeWarden a
    /// member's org-user id is its user id (`organization_members` is keyed
    /// by `user_id`), which is what its collection create resolves a grant
    /// against.
    fn ensure_collection(&self, ctx: &mut Ctx, email: &str) -> Result<String, SharingError> {
        if let Some(id) = ctx.collection_for(email) {
            return Ok(id.to_string());
        }
        let name = crate::rest::crypto::encrypt(&ctx.org_key, email.as_bytes()).map_err(crypto)?;
        let id = self
            .call(|client, auth| sharing_api::create_collection(client, &mut auth.session, &ctx.org_id, &name, &ctx.account.id))
            .map_err(|e| e.or_say(&format!("Couldn't prepare sharing with {email}. Try again.")))?;
        ctx.collections.push((id.clone(), email.to_string()));
        Ok(id)
    }

    /// Whether `spki` may be trusted for `email`: no pin yet, or the same one.
    pub(crate) fn pin_allows(&self, account: &str, email: &str, spki: &[u8]) -> bool {
        let Some(path) = self.setup().state_path else {
            return true;
        };
        let state = crate::sharing_state::SharingState::load(&path).account(account);
        state.pin_for(email).is_none_or(|pin| pin.sha256 == crate::rest::crypto::public_key_sha256_hex(spki))
    }

    /// Pins a key (trust on first use, spec 11.5).
    pub(crate) fn pin(&self, account: &str, email: &str, key: &sharing_api::AccountKey) {
        let Some(path) = self.setup().state_path else {
            return;
        };
        let pin = crate::sharing_state::Pin {
            sha256: crate::rest::crypto::public_key_sha256_hex(&key.public_key),
            user_id: key.user_id.clone(),
            at: now_millis(),
        };
        if let Err(e) = crate::sharing_state::update(&path, |state| state.account_mut(account).set_pin(email, pin)) {
            log::warn!("a sharing pin could not be written: {e}");
        }
    }

    /// **The person's membership, with access to C(E)** (spec 7.2): nothing if
    /// they have it; otherwise the direct add with the key already wrapped
    /// (and pinned), or a pending invitation for an address with no account.
    /// A member without access to their own collection is removed and invited
    /// again (this build's decision 11).
    pub(crate) fn ensure_member(&self, ctx: &Ctx, email: &str, collection: &str) -> Result<ShareResult, SharingError> {
        let roster = self.fetch_roster(&ctx.org_id)?;
        if let Some(member) = roster.iter().find(|m| normalise_email(&m.email) == email) {
            if member.has_access(collection) {
                return Ok(match member.status {
                    MemberStatus::Confirmed => ShareResult::Shared,
                    MemberStatus::Accepted => ShareResult::WaitingAccept,
                    MemberStatus::Invited => ShareResult::WaitingAccount,
                });
            }
            let ou = member.id.clone();
            self.call(|client, auth| sharing_api::remove_user(client, &mut auth.session, &ctx.org_id, &ou))?;
        }
        let refused = format!("Couldn't invite {email}.");
        match self.call(|client, auth| sharing_api::member_key_by_email(client, &mut auth.session, &ctx.org_id, email))? {
            Some(account_key) => {
                // A changed key is not wrapped to (spec 12.1): they join
                // without the key and the card says their key changed.
                let trusted = self.pin_allows(&ctx.account.id, email, &account_key.public_key);
                let wrapped = if trusted {
                    Some(crate::rest::crypto::wrap_key_to_public(&ctx.org_key, &account_key.public_key).map_err(crypto)?)
                } else {
                    None
                };
                self.call(|client, auth| {
                    sharing_api::invite_member(client, &mut auth.session, &ctx.org_id, email, collection, wrapped.as_ref())
                })
                .map_err(|e| e.or_say(&refused))?;
                if trusted {
                    self.pin(&ctx.account.id, email, &account_key);
                    Ok(ShareResult::Shared)
                } else {
                    self.held().key_changed.insert(email.to_string());
                    Ok(ShareResult::WaitingAccept)
                }
            }
            None => {
                self.call(|client, auth| sharing_api::invite_member(client, &mut auth.session, &ctx.org_id, email, collection, None))
                    .map_err(|e| e.or_say(&refused))?;
                Ok(ShareResult::WaitingAccount)
            }
        }
    }

    /// **Files a record into C(E)** (spec 9.3). Already in the org: one
    /// `/collections` call with C(E) added. Personal: the whole record moves,
    /// with `organizationId` set and -- when it has its own key -- that key
    /// re-wrapped from the user key to the org key; `encrypt_item` then
    /// re-encrypts every field under whatever key the body now names. A stale
    /// revision is re-read and retried once (spec 13).
    fn file_into(
        &self,
        ctx: &Ctx,
        record: &crate::rest::sync::DecryptedItem,
        collection: &str,
    ) -> Result<VaultItem, SharingError> {
        let decrypted = |answer: serde_json::Value| -> Result<VaultItem, SharingError> {
            crate::rest::sync::decrypt_cipher(&answer, &ctx.keys)
                .map(|(written, _)| written.item)
                .ok_or_else(|| SharingError::Refused(SHARE_FAILED.to_string()))
        };
        if organization_of(&record.item) == Some(ctx.org_id.as_str()) {
            let mut ids: Vec<String> = collection_ids_of(&record.item).into_iter().map(str::to_string).collect();
            if ids.iter().any(|id| id == collection) {
                return Ok(record.item.clone());
            }
            ids.push(collection.to_string());
            let answer = self
                .call(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, &record.item.id, &ids))
                .map_err(|e| e.or_say(SHARE_FAILED))?;
            return decrypted(answer);
        }
        let attempt = |record: &crate::rest::sync::DecryptedItem| -> Result<serde_json::Value, SharingError> {
            let mut moved = crate::vault_bridge::with_revision_date_from(&record.item, &record.item);
            moved.other.insert("organizationId".into(), serde_json::Value::String(ctx.org_id.clone()));
            moved.other.remove("collectionIds");
            if let Some(wrapped) = moved.other.get("key").and_then(serde_json::Value::as_str).map(str::to_string) {
                let cipher_key = crate::rest::crypto::unwrap_key_under(&wrapped.parse::<EncString>().map_err(crypto)?, ctx.keys.user())
                    .map_err(crypto)?;
                let rewrapped = crate::rest::crypto::wrap_key_under(&cipher_key, &ctx.org_key).map_err(crypto)?;
                moved.other.insert("key".into(), serde_json::Value::String(rewrapped.to_string()));
            }
            let cipher = crate::rest::write::encrypt_item(&record.carrying(moved), &ctx.keys).map_err(crypto)?;
            let ids = vec![collection.to_string()];
            self.call(|client, auth| sharing_api::share_cipher(client, &mut auth.session, &record.item.id, &cipher, &ids))
        };
        match attempt(record) {
            Ok(answer) => decrypted(answer),
            Err(SharingError::Refused(said)) if said.to_lowercase().contains("out of date") => {
                let (fresh, _) = self.fetch_record(&record.item.id, &ctx.keys)?;
                decrypted(attempt(&fresh).map_err(|e| e.or_say(SHARE_FAILED))?)
            }
            Err(e) => Err(e.or_say(SHARE_FAILED)),
        }
    }

    /// Reads the org's roster and collections back into `held` after a write,
    /// so the next `GET /sharing` answers what the server now says.
    pub(crate) fn reload_org(&self, ctx: &Ctx) -> Result<(), SharingError> {
        let roster = self.fetch_roster(&ctx.org_id)?;
        let collections = self.collections_of(&ctx.org_id, &ctx.org_key)?;
        self.held().my_org = Some(MyOrg { id: ctx.org_id.clone(), collections, roster });
        Ok(())
    }

    /// **Share one record with one person** (spec 6.2-6.5). Ordered so a
    /// failure leaves the record untouched: nothing moves it until org,
    /// collection and membership exist. Every step reads what is there first,
    /// so a Retry is the same call.
    ///
    /// # Errors
    ///
    /// [`SharingError::Refused`] with the sentence to show, or the backend's
    /// own failure.
    pub fn share(&self, item_id: &str, email: &str) -> Result<ShareOutcome, SharingError> {
        let _one = self.one_at_a_time();
        self.require_available()?;
        let email = normalise_email(email);
        if !plausible_email(&email) {
            return Err(SharingError::Refused(NOT_AN_EMAIL.to_string()));
        }
        let profile = self.profile()?;
        let account = self.held().account.clone();
        if account.as_ref().is_some_and(|account| email == normalise_email(&account.email)) {
            return Err(SharingError::Refused(YOURSELF.to_string()));
        }
        let account = keyed_account(account)?;
        let mut keys = self.keys_from(&profile)?;
        let (record, failures) = self.fetch_record(item_id, &keys)?;
        let existing = self.my_org_id(&profile, &account);
        refuse_unmovable(&record, &failures, existing.as_deref())?;

        let (org_id, org_key, collections) = match existing {
            Some(org_id) => {
                let org_key = keys.organization(&org_id).cloned().ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
                let collections = self.collections_of(&org_id, &org_key)?;
                (org_id, org_key, collections)
            }
            None => {
                let (org_id, org_key) = self.create_org(&profile, &account, &keys, &email)?;
                keys = Arc::new(keys.with_organization(&org_id, org_key.clone()));
                // A server that made C(E) from `collectionName` (spec 9.5) is
                // adopted here by name, like any other collection.
                let collections = self.collections_of(&org_id, &org_key)?;
                (org_id, org_key, collections)
            }
        };
        if let Some(path) = self.setup().state_path {
            let (account_id, org) = (account.id.clone(), org_id.clone());
            let was_personal = organization_of(&record.item).is_none();
            let moved = crate::sharing_state::Move { folder_id: record.item.folder_id.clone(), favorite: record.item.favorite };
            let written = crate::sharing_state::update(&path, |state| {
                let me = state.account_mut(&account_id);
                me.sharing_org_id = Some(org);
                if was_personal {
                    me.moves.insert(record.item.id.clone(), moved);
                }
            });
            if let Err(e) = written {
                log::warn!("the sharing state could not be written before a share: {e}");
            }
        }
        let mut ctx = Ctx { account, keys, org_id, org_key, collections };
        let collection = self.ensure_collection(&mut ctx, &email)?;
        let result = self.ensure_member(&ctx, &email, &collection)?;
        let written = self.file_into(&ctx, &record, &collection)?;
        if let Some(path) = self.setup().sent_path {
            let entry = crate::sent_history::SentRecord {
                kind: crate::sent_history::SentKind::Share,
                sent_at_unix_millis: now_millis(),
                recipient: email.clone(),
                item_id: written.id.clone(),
                name: written.name.clone(),
                ..crate::sent_history::SentRecord::default()
            };
            if let Err(e) = crate::sent_history::update(&path, |history| history.record(entry)) {
                log::warn!("a share was made but could not be added to the sent history: {e}");
            }
        }
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a share: {e}");
        }
        Ok(ShareOutcome { result, changed: SharingOutcome { upserted: vec![written], removed: Vec::new() } })
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
        FIXTURE_EMAIL, FIXTURE_ORG, FIXTURE_USER_ID, fixture_key_pair, person_collection, personal_cipher, roster_row,
        shared_cipher, sharing_org_row, sharing_profile, signed_in_with_no_sync_route,
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
            held.account = Some(Account { id: "me".into(), email: "me@x.io".into(), admin: true, keyed: true });
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
        let profile = server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).expect(1).create();
        let invitations = server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).expect(2).create();
        let key = server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "userId": FIXTURE_USER_ID, "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .expect(1)
            .create();
        engine.refresh(Trigger::WholeRead).expect("a first pass");
        engine.refresh(Trigger::WholeRead).expect("a second pass");
        engine.refresh(Trigger::Batch).expect("a batch pass asks nothing");
        profile.assert();
        invitations.assert();
        key.assert();
        assert!(engine.view(&[]).available);
    }

    /// One probe against a NodeWarden whose own-key route answers `route`,
    /// for an account whose profile is `profile`; the view it leaves.
    fn probed(profile: serde_json::Value, route: impl FnOnce(crate::test_http::Mock) -> crate::test_http::Mock) -> SharingView {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        server.mock("GET", "/api/accounts/profile").with_body(profile.to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        route(server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        engine.view(&[])
    }

    /// The fixture profile with no key pair: `privateKey` null.
    fn keyless_profile() -> serde_json::Value {
        let mut profile = sharing_profile(serde_json::json!([]));
        profile["privateKey"] = serde_json::Value::Null;
        profile
    }

    /// **An old NodeWarden without slice 1 turns sharing off** (spec 7.1):
    /// its own-key route answers 404 for an account that has a key, or 405
    /// or 501, and without slice 1 a recipient could edit the sharer's
    /// records (spec 12.3).
    #[test]
    fn an_old_nodewarden_without_slice_one_turns_sharing_off() {
        for status in [404, 405, 501] {
            let view = probed(sharing_profile(serde_json::json!([])), |m| m.with_status(status));
            assert!(!view.available, "{status}");
            assert_eq!(view.reason.as_deref(), Some(REASON_TOO_OLD), "{status}");
        }
    }

    /// **A 200 without a key is the route answering**, so slice 1 is there.
    /// Read as "too old" it would turn sharing off for the whole session.
    #[test]
    fn an_own_key_answer_without_a_key_is_still_slice_one() {
        let view = probed(sharing_profile(serde_json::json!([])), |m| {
            m.with_body(serde_json::json!({ "userId": FIXTURE_USER_ID, "publicKey": null }).to_string())
        });
        assert!(view.available, "{view:?}");
    }

    /// **A keyless account's 404 is not taken for an old server.** NodeWarden
    /// answers 404 for an account with no key pair, so the status says
    /// nothing about the route; the profile's missing private key says why.
    /// Sharing stays available, and the share flow refuses on the keys.
    #[test]
    fn a_keyless_accounts_404_is_not_read_as_an_old_server() {
        let view = probed(keyless_profile(), |m| m.with_status(404));
        assert!(view.available, "{view:?}");
        let view = probed(keyless_profile(), |m| m.with_status(405));
        assert_eq!(view.reason.as_deref(), Some(REASON_TOO_OLD), "a 405 is no route whoever asks");
    }

    /// **`Held`'s `Debug` never prints an invitation token** -- the reason it
    /// is hand-written (see its doc).
    #[test]
    fn held_debug_never_prints_an_invitation_token() {
        let held = Held {
            incoming: vec![IncomingInvitation {
                id: "inv-1".into(),
                organization_id: "org-anna".into(),
                organization_name: Some("Shared by anna@x.io".into()),
                token: zeroize::Zeroizing::new("TOKEN-NEEDLE-7f3a".into()),
                expires_at: None,
            }],
            ..Held::default()
        };
        let printed = format!("{held:?}");
        assert!(!printed.contains("TOKEN-NEEDLE-7f3a"), "{printed}");
        assert!(printed.contains("<1 invitations>"), "{printed}");
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

    use crate::test_http::{Matcher, MockServer};

    /// A probed engine on a NodeWarden with slice 1, over the fixture
    /// account, with its files in a scratch directory.
    fn on_nodewarden(organizations: serde_json::Value) -> (MockServer, crate::rest::backend::RestBackend, Arc<Sharing>, crate::test_scratch::ScratchDir) {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let dir = crate::test_scratch::ScratchDir::new("sharing-flow");
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup {
            state_path: Some(dir.join("sharing_state.json")),
            sent_path: Some(dir.join("sent_history.json")),
            fill_stats_path: Some(dir.join("fill-stats.json")),
            make_key_pair: fixture_key_pair,
        });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(organizations).to_string()).expect_at_least(0).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).expect_at_least(0).create();
        server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .expect_at_least(0)
            .create();
        engine.refresh(Trigger::WholeRead).expect("probed");
        (server, backend, engine, dir)
    }

    fn roster(rows: &[serde_json::Value]) -> String {
        serde_json::json!({ "object": "list", "data": rows }).to_string()
    }

    /// The server's copy of a cipher it was sent: the request's own cipher,
    /// with the id and filing a server adds. What NodeWarden answers `/share`
    /// and `POST /api/ciphers` with, modulo fields this crate does not read.
    fn echo(id: &'static str, org: Option<&'static str>, collections: &'static [&'static str]) -> impl Fn(&crate::test_http::Request) -> Vec<u8> + Send + Sync + 'static {
        move |request| {
            let body: serde_json::Value = serde_json::from_slice(request.body()).expect("json");
            let mut cipher = body.get("cipher").cloned().unwrap_or(body);
            cipher["id"] = serde_json::json!(id);
            cipher["organizationId"] = serde_json::json!(org);
            cipher["collectionIds"] = serde_json::json!(collections);
            cipher["revisionDate"] = serde_json::json!("2026-09-27T00:00:00.000Z");
            cipher.to_string().into_bytes()
        }
    }

    /// **The first share ever** (spec 6.2): org, collection, direct add,
    /// move -- each once, in that order's dependencies, and no sync.
    #[test]
    fn a_first_share_creates_the_org_the_collection_the_invite_and_moves_once() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([]));
        let sync = server.mock("GET", "/api/sync?excludeDomains=true").expect(0).create();
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        let org = server
            .mock("POST", "/api/organizations")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "name": format!("Shared by {FIXTURE_EMAIL}"),
                "billingEmail": FIXTURE_EMAIL,
                "keys": { "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }
            })))
            .with_body(r#"{"id":"org-new","object":"organization"}"#)
            .expect(1)
            .create();
        // The org's collections: empty when the org is new, then the one the
        // share made. NodeWarden's web vault may leave a name in plaintext,
        // and the engine reads that too -- which is what lets this answer
        // name c-anna without the org key the test cannot know.
        server.mock("GET", "/api/organizations/org-new/collections").with_body(r#"{"data":[]}"#).expect(1).create();
        server
            .mock("GET", "/api/organizations/org-new/collections")
            .with_body(r#"{"data":[{"id":"c-anna","organizationId":"org-new","name":"anna@example.com"}]}"#)
            .create();
        let collection = server
            .mock("POST", "/api/organizations/org-new/collections")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "users": [{ "id": FIXTURE_USER_ID, "readOnly": false, "hidePasswords": false, "manage": true }]
            })))
            .with_body(r#"{"id":"c-anna","object":"collection"}"#)
            .expect(1)
            .create();
        server
            .mock("GET", "/api/organizations/org-new/users?includeCollections=true")
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(1)
            .create();
        server
            .mock("GET", "/api/organizations/org-new/users?includeCollections=true")
            .with_body(roster(&[
                roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]),
                roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"]),
            ]))
            .create();
        server
            .mock("GET", "/api/organizations/org-new/members/public-key?email=anna%40example.com")
            .with_body(serde_json::json!({ "userId": "anna-id", "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        let invite = server
            .mock("POST", "/api/organizations/org-new/members/invite")
            .match_request(|request| request.utf8_lossy_body().contains("\"encryptedOrgKey\":\"4."))
            .with_body(r#"{"id":"anna-id","status":1,"object":"organizationMember"}"#)
            .expect(1)
            .create();
        let share = server
            .mock("PUT", "/api/ciphers/p1/share")
            .match_body(Matcher::PartialJson(serde_json::json!({ "collectionIds": ["c-anna"], "cipher": { "organizationId": "org-new" } })))
            .with_body_from_request(echo("p1", Some("org-new"), &["c-anna"]))
            .expect(1)
            .create();

        let outcome = engine.share("p1", " Anna@Example.com ").expect("shared");
        org.assert();
        collection.assert();
        invite.assert();
        share.assert();
        sync.assert();
        assert_eq!(outcome.result, ShareResult::Shared);
        assert_eq!(outcome.changed.upserted.len(), 1);
        let moved = &outcome.changed.upserted[0];
        assert_eq!(moved.name, "Acme VPN", "the moved record decrypts under the new org key");
        assert_eq!(organization_of(moved), Some("org-new"));
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(state.sharing_org_id.as_deref(), Some("org-new"));
        assert_eq!(state.moves.get("p1").and_then(|m| m.folder_id.as_deref()), Some("f1"));
        assert!(state.pin_for("anna@example.com").is_some(), "a direct add pins the key at that moment");
        let sent = crate::sent_history::SentHistory::load(&dir.join("sent_history.json"));
        assert_eq!(sent.entries[0].kind, crate::sent_history::SentKind::Share);
        assert_eq!(sent.entries[0].recipient, "anna@example.com");
        assert_eq!(engine.view(std::slice::from_ref(moved)).person("anna@example.com").map(|p| p.state), Some(PersonState::Shared));
    }

    /// **An existing person, another record already in the org** (spec 6.3,
    /// 6.4): one collections call and nothing else written.
    #[test]
    fn a_share_to_an_existing_person_is_one_collections_call() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-bob"]).to_string()).create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": [person_collection("c-bob", "bob@example.com"), person_collection("c-anna", "anna@example.com")] }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[
                roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]),
                roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"]),
                roster_row("bob-id", Some("bob-id"), "bob@example.com", 2, &["c-bob"]),
            ]))
            .create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let collections = server
            .mock("PUT", "/api/ciphers/s1/collections")
            .match_body(Matcher::Json(serde_json::json!({ "collectionIds": ["c-bob", "c-anna"] })))
            .with_body(shared_cipher("s1", "Acme VPN", &["c-bob", "c-anna"]).to_string())
            .expect(1)
            .create();
        let outcome = engine.share("s1", "anna@example.com").expect("shared");
        collections.assert();
        writes.assert();
        assert_eq!(outcome.result, ShareResult::Shared);
    }

    /// **A person with no account** (spec 6.5): a pending invitation, no key
    /// wrapped, and the dialog is told so.
    #[test]
    fn a_share_to_someone_without_an_account_leaves_an_invitation() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": [person_collection("c-carol", "carol@example.com")] }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/members/public-key?email=carol%40example.com").as_str())
            .with_status(404)
            .create();
        let invite = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/members/invite").as_str())
            .match_request(|request| !request.utf8_lossy_body().contains("encryptedOrgKey"))
            .with_body(r#"{"id":"inv-1","status":0,"object":"organizationInvitation"}"#)
            .expect(1)
            .create();
        server
            .mock("PUT", "/api/ciphers/p1/share")
            .with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-carol"]))
            .create();
        let outcome = engine.share("p1", "carol@example.com").expect("shared");
        invite.assert();
        assert_eq!(outcome.result, ShareResult::WaitingAccount);
    }

    /// **A failure at the move leaves the record personal** (spec 6.2, 13),
    /// through the daemon: the item in its cache is untouched and the answer
    /// is the dialog's sentence.
    #[test]
    fn a_share_that_fails_at_the_move_leaves_the_record_personal() {
        let (mut server, cache, api, token) = crate::vault_api::tests::a_daemon();
        let engine = cache.bridge().sharing().expect("an engine");
        engine.set_up(SharingSetup { make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([sharing_org_row()])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": [person_collection("c-anna", "anna@example.com")] }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"])]))
            .create();
        server.mock("PUT", "/api/ciphers/live-1/share").with_status(500).create();
        let answer = crate::vault_api::tests::ask(&api, "POST", "/sharing/item/live-1", Some(&token), r#"{"email":"anna@example.com"}"#);
        assert_eq!(answer.status, 400);
        let body: serde_json::Value = serde_json::from_str(&answer.body).expect("json");
        assert_eq!(body["message"], SHARE_FAILED);
        let held = cache.get_by_id("live-1").expect("still in the daemon's vault");
        assert_eq!(organization_of(&held), None, "the record stayed personal");
    }

    /// **What cannot move is refused before anything is sent** (spec 9.3,
    /// and decision 3: any attachment at all).
    #[test]
    fn a_record_with_attachments_is_refused_before_anything_is_written() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let mut with_file = personal_cipher("p1", "Acme VPN");
        with_file["attachments"] = serde_json::json!([{ "id": "a1", "fileName": "2.x|y|z" }]);
        server.mock("GET", "/api/ciphers/p1").with_body(with_file.to_string()).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, HAS_ATTACHMENTS),
            other => panic!("expected the attachments refusal, got {other:?}"),
        }
        writes.assert();
        puts.assert();
        assert!(matches!(engine.share("p1", "not an email"), Err(SharingError::Refused(s)) if s == NOT_AN_EMAIL));
        assert!(matches!(engine.share("p1", FIXTURE_EMAIL), Err(SharingError::Refused(s)) if s == YOURSELF));
    }

    /// **A keyless account is refused on its keys, before anything is read
    /// or written.** The probe leaves sharing on for it -- NodeWarden's 404
    /// for its own key is no evidence of an old server (see
    /// `a_keyless_accounts_404_is_not_read_as_an_old_server`) -- so this is
    /// where it is stopped: with no private key there is no org key it could
    /// wrap to itself or unwrap, and a sentence about the server's age or a
    /// failed organisation would send the user looking in the wrong place.
    #[test]
    fn a_keyless_account_is_refused_with_a_sentence_about_keys() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup { make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(keyless_profile().to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str()).with_status(404).create();
        let record = server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).expect(0).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("probed");
        assert!(engine.view(&[]).available, "control: the probe left sharing on for a keyless account");
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, NO_KEY_PAIR),
            other => panic!("expected the keys refusal, got {other:?}"),
        }
        record.assert();
        writes.assert();
        puts.assert();
    }
}

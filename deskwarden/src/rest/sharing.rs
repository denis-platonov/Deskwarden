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
/// The server's list of the people in the sharing org was not one.
pub const ROSTER_UNREADABLE: &str = "Deskwarden couldn't read who you share with. Try again.";
/// A stop's refusal opens with this; the person who can still see the record
/// follows it.
pub const STOP_FAILED_PREFIX: &str = "Couldn't stop sharing.";
/// A share of an org copy whose move back to the personal vault is pending:
/// the personal copy is the record to share.
pub const COMING_HOME: &str = "This record is being moved back to your vault. Share it from there.";
/// The last person was stopped and the personal copy could not be made.
/// Nothing was changed.
pub const MOVE_BACK_FAILED: &str = "Couldn't move the record back to your vault. It is still shared.";
/// The last person was stopped and the record has an attachment, which the
/// delete that ends a move back would destroy (see [`Sharing::move_back`]).
pub const MOVE_BACK_ATTACHMENTS: &str =
    "This record has attachments, so Deskwarden can't move it back to your vault. It is still shared.";
/// The last person was stopped from a record in the trash or the archive: it
/// would come home as a live personal record.
pub const STOP_IN_TRASH: &str = "This record is in the trash or the archive. Restore it before you stop sharing it.";
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

/// What a stop did.
#[derive(Debug, Clone)]
pub struct StopOutcome {
    /// The record's new id, when the last person was stopped and it came home.
    pub moved_to: Option<String>,
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
    /// whole read or the timer, then every move back whose delete failed
    /// ([`Self::finish_unfinished`]) -- on every trigger, since each is one
    /// file read when nothing is pending. The confirm pass joins it in its
    /// own task.
    ///
    /// **A failed invitations read does not end the pass.** It used to, with
    /// `?`, and so a transient error there skipped the unfinished moves too:
    /// an org copy nobody was retrying, one pass at a time. It is logged, the
    /// incoming list keeps what it held, and the rest of the pass runs. It
    /// cannot run FIRST instead: a pass that finished a move and then failed
    /// on the invitations would return the error and drop the outcome, and
    /// the daemon would never be told the org copy is gone.
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
            match self.call(|client, auth| sharing_api::my_invitations(client, &mut auth.session)) {
                Ok(Some(list)) => {
                    let mut held = self.held();
                    let before: Vec<String> = held.incoming.iter().map(|i| i.id.clone()).collect();
                    pass.changed |= before != list.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
                    held.incoming = list;
                }
                Ok(None) => {}
                Err(e) => log::info!("the incoming invitations could not be read this pass: {e}"),
            }
        }
        pass.outcome = self.finish_unfinished();
        pass.changed |= !pass.outcome.is_empty();
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

/// How one try at filing a record failed: a stale revision, which is
/// re-read and retried, or anything else, already in the dialog's words.
enum Filing {
    Stale(SharingError),
    Failed(SharingError),
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
    if in_trash_or_archive(&record.item) {
        return Err(SharingError::Refused(IN_TRASH.to_string()));
    }
    if has_attachments(&record.item) {
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

/// The record carries an attachment. Asked by the share (decision 3) and by
/// the move back, for the same reason from either side: an attachment belongs
/// to one cipher id, and neither direction keeps the id.
fn has_attachments(item: &VaultItem) -> bool {
    item.other.get("attachments").and_then(serde_json::Value::as_array).is_some_and(|a| !a.is_empty())
}

/// The record is in the trash or the archive -- neither is a record to move.
fn in_trash_or_archive(item: &VaultItem) -> bool {
    let set = |key: &str| item.other.get(key).is_some_and(|v| !v.is_null());
    set("deletedDate") || set("archivedDate")
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
    ///
    /// **An answer that is not a roster is an error, never an empty one** --
    /// [`crate::rest::organizations::Roster`]'s own distinction. Read as
    /// nobody, it would have a share invite people who are already members,
    /// and [`Self::reload_org`] hold a roster that shows everyone as `Gone`.
    pub(crate) fn fetch_roster(&self, org: &str) -> Result<Vec<Member>, SharingError> {
        let answer = self.call(|client, auth| client.fetch_organization_members(&mut auth.session, org))?;
        crate::rest::organizations::map_members(&answer).ok_or_else(|| SharingError::Refused(ROSTER_UNREADABLE.to_string()))
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
        let refused = format!("Couldn't invite {email}.");
        if let Some(member) = roster.iter().find(|m| normalise_email(&m.email) == email) {
            if member.has_access(collection) {
                return Ok(match member.status {
                    MemberStatus::Confirmed => ShareResult::Shared,
                    MemberStatus::Accepted => ShareResult::WaitingAccept,
                    MemberStatus::Invited => ShareResult::WaitingAccount,
                });
            }
            let ou = member.id.clone();
            self.call(|client, auth| sharing_api::remove_user(client, &mut auth.session, &ctx.org_id, &ou))
                .map_err(|e| e.or_say(&refused))?;
        }
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
    /// `/collections` call with C(E) added. Personal: the whole record moves
    /// -- see [`Self::moved_body`].
    ///
    /// **A stale revision is re-read and retried once** (spec 13), and the
    /// fresh copy is judged again by [`refuse_unmovable`] before it is: what
    /// moved on the server in between may be a restore to the trash, an
    /// attachment, or another PC's share into the org -- which the retry then
    /// files with `/collections` rather than moving twice.
    fn file_into(
        &self,
        ctx: &Ctx,
        record: &crate::rest::sync::DecryptedItem,
        collection: &str,
    ) -> Result<VaultItem, SharingError> {
        match self.file_once(ctx, record, collection) {
            Ok(written) => Ok(written),
            Err(Filing::Failed(e)) => Err(e),
            Err(Filing::Stale(_)) => {
                let (fresh, failures) = self.fetch_record(&record.item.id, &ctx.keys)?;
                refuse_unmovable(&fresh, &failures, Some(ctx.org_id.as_str()))?;
                self.file_once(ctx, &fresh, collection).map_err(|filing| match filing {
                    Filing::Stale(e) => e.or_say(SHARE_FAILED),
                    Filing::Failed(e) => e,
                })
            }
        }
    }

    /// One try at [`Self::file_into`]. A stale-revision refusal comes back
    /// apart from every other failure, so the caller can re-read and retry.
    fn file_once(
        &self,
        ctx: &Ctx,
        record: &crate::rest::sync::DecryptedItem,
        collection: &str,
    ) -> Result<VaultItem, Filing> {
        let answered = |answer: Result<serde_json::Value, SharingError>| -> Result<VaultItem, Filing> {
            match answer {
                Ok(answer) => crate::rest::sync::decrypt_cipher(&answer, &ctx.keys)
                    .map(|(written, _)| written.item)
                    .ok_or_else(|| Filing::Failed(SharingError::Refused(SHARE_FAILED.to_string()))),
                Err(SharingError::Refused(said)) if said.to_lowercase().contains("out of date") => {
                    Err(Filing::Stale(SharingError::Refused(said)))
                }
                Err(e) => Err(Filing::Failed(e.or_say(SHARE_FAILED))),
            }
        };
        if organization_of(&record.item) == Some(ctx.org_id.as_str()) {
            let mut ids: Vec<String> = collection_ids_of(&record.item).into_iter().map(str::to_string).collect();
            if ids.iter().any(|id| id == collection) {
                return Ok(record.item.clone());
            }
            ids.push(collection.to_string());
            return answered(
                self.call(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, &record.item.id, &ids)),
            );
        }
        let cipher = self.moved_body(ctx, record).map_err(Filing::Failed)?;
        let ids = vec![collection.to_string()];
        answered(self.call(|client, auth| sharing_api::share_cipher(client, &mut auth.session, &record.item.id, &cipher, &ids)))
    }

    /// **The body that moves a personal record into the org**: the record
    /// with `organizationId` set, and **no ciphertext left under the user
    /// key anywhere in it**.
    ///
    /// A record with its own key keeps it, re-wrapped from the user key to
    /// the org key; its fields are under that key and do not change. A record
    /// without one -- every record Deskwarden itself makes -- has its fields
    /// under the user key, and `encrypt_item` re-encrypts only the ones this
    /// crate models under the org key. The rest ride through as they arrived
    /// (`uriChecksum`, passkeys, NodeWarden's types 6-8 and `data`), and
    /// NodeWarden merges the body over the stored row, so they would land in
    /// the org readable by nobody. [`crate::rest::write::MappedCipher::rekey_carried`]
    /// walks the whole body and moves each of them to the org key; for a
    /// keyed record the same walk checks that every leaf opens under its own
    /// key. Anything that opens under neither refuses the share whole.
    ///
    /// Giving the record a fresh key of its own during the move -- the
    /// official clients' way -- was the alternative. It needs the same walk
    /// (the unmodelled leaves are under the user key either way), and adds a
    /// per-item key Deskwarden never otherwise makes; so the smaller change
    /// was taken.
    ///
    /// **Nothing stored can keep the old ciphertext either.** NodeWarden
    /// spreads the body over the stored row, so a field the body left out
    /// would survive. None is left out: its cipher answer passes every stored
    /// field through (`cipherToResponse`'s `...passthrough`), so the record
    /// read here names each one, and the body -- built over that record --
    /// sends each back re-keyed. `data` included, which the answer gives as
    /// `null` when it is not a string, and a `null` overwrites.
    fn moved_body(
        &self,
        ctx: &Ctx,
        record: &crate::rest::sync::DecryptedItem,
    ) -> Result<crate::rest::write::MappedCipher, SharingError> {
        let unreadable = |_: CryptoError| SharingError::Refused(UNREADABLE_PART.to_string());
        let mut moved = crate::vault_bridge::with_revision_date_from(&record.item, &record.item);
        moved.other.insert("organizationId".into(), serde_json::Value::String(ctx.org_id.clone()));
        moved.other.remove("collectionIds");
        let own_key = match moved.other.get("key").and_then(serde_json::Value::as_str).map(str::to_string) {
            Some(wrapped) => {
                let wrapped = wrapped.parse::<EncString>().map_err(unreadable)?;
                let cipher_key = crate::rest::crypto::unwrap_key_under(&wrapped, ctx.keys.user()).map_err(unreadable)?;
                let rewrapped = crate::rest::crypto::wrap_key_under(&cipher_key, &ctx.org_key).map_err(crypto)?;
                moved.other.insert("key".into(), serde_json::Value::String(rewrapped.to_string()));
                Some(cipher_key)
            }
            None => None,
        };
        let mut cipher = crate::rest::write::encrypt_item(&record.carrying(moved), &ctx.keys)
            .map_err(|e| crypto(e).or_say(SHARE_FAILED))?;
        match &own_key {
            Some(key) => cipher.rekey_carried(key, key),
            None => cipher.rekey_carried(ctx.keys.user(), &ctx.org_key),
        }
        .map_err(unreadable)?;
        Ok(cipher)
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
        // **An org copy whose move back is pending is not shared again.** Its
        // personal copy exists, and the next pass unfiles everyone and
        // deletes it -- so a person added here would be told "shared" and
        // then silently lose it. Refused, not finished first: the record to
        // share is the personal copy, which the user shares from their vault.
        if self.pending_move(&account.id, item_id).is_some() {
            return Err(SharingError::Refused(COMING_HOME.to_string()));
        }
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
                // Into the backend's key cache too, which `keys_from` filled:
                // the moved record comes back under this org, and a read the
                // backend makes before its next whole read (a hub push's
                // one-record apply) needs the key the profile does not name
                // yet.
                *self.keys.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&keys));
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

// ---- stopping ---------------------------------------------------------------------

impl Sharing {
    /// **Stop sharing a record with one person** (spec 6.8).
    ///
    /// # Errors
    ///
    /// The sentence to show, or the backend's failure.
    pub fn stop(&self, item_id: &str, email: &str) -> Result<StopOutcome, SharingError> {
        let _one = self.one_at_a_time();
        let ctx = self.context()?;
        let outcome = self.stop_with(&ctx, item_id, email)?;
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a stop: {e}");
        }
        Ok(outcome)
    }

    /// [`Self::stop`] within a context already read -- `forget` stops every
    /// record in one context.
    ///
    /// **Only a record in THIS account's sharing org is stopped.** Anything
    /// else -- a personal record, another org's -- has nobody of ours to take
    /// off it, so the answer is "nothing changed", and a folder-and-star
    /// entry `sharing_state` still holds for it is stale: a share that wrote
    /// it and then failed before the move (the share records it first, so a
    /// move that lands is never without it). It is dropped here, the one
    /// place that reads the record and can tell. [`Self::move_back`] reads
    /// that entry, and is only reached past this check.
    ///
    /// **A record already on its way home is finished, never moved again.**
    /// A move back whose second half failed has its personal copy made and
    /// its `moves` entry spent; the org copy may still be filed for the
    /// person, so the view -- rightly -- still shows them on it, and a second
    /// Stop (or `forget`) will come. Moving it again would make a second
    /// personal copy, without the folder and star, and a second pending
    /// purge. So `unfinished` is read FIRST, whoever is named: that move was
    /// the last person's, so finishing it is the whole of any stop on it.
    pub(crate) fn stop_with(&self, ctx: &Ctx, item_id: &str, email: &str) -> Result<StopOutcome, SharingError> {
        let email = normalise_email(email);
        let refused = format!("{STOP_FAILED_PREFIX} {email} can still see it.");
        if let Some(personal_id) = self.pending_move(&ctx.account.id, item_id) {
            return self.finish_pending(ctx, item_id, &personal_id, &email);
        }
        let nothing = || StopOutcome { moved_to: None, changed: SharingOutcome::default() };
        let Some(collection) = ctx.collection_for(&email).map(str::to_string) else {
            return Ok(nothing());
        };
        let (record, failures) = self.fetch_record(item_id, &ctx.keys)?;
        if organization_of(&record.item) != Some(ctx.org_id.as_str()) {
            self.drop_stale_move(&ctx.account.id, &record.item.id);
            return Ok(nothing());
        }
        let current: Vec<String> = collection_ids_of(&record.item).into_iter().map(str::to_string).collect();
        if !current.contains(&collection) {
            return Ok(nothing());
        }
        let remaining: Vec<String> = current.into_iter().filter(|id| *id != collection).collect();
        if !remaining.is_empty() {
            let answer = self
                .call(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, item_id, &remaining))
                .map_err(|e| e.or_say(&refused))?;
            let (written, _) = crate::rest::sync::decrypt_cipher(&answer, &ctx.keys)
                .ok_or_else(|| SharingError::Refused(refused.clone()))?;
            return Ok(StopOutcome { moved_to: None, changed: SharingOutcome { upserted: vec![written.item], removed: Vec::new() } });
        }
        self.move_back(ctx, &record, &failures, &refused)
    }

    /// **The last person stopped: the record comes home** (spec 9.4).
    ///
    /// The personal copy is created FIRST, so a failure there changes nothing
    /// (the record is still shared, and the sentence says so). Then the org
    /// copy is filed in nothing -- invisible to everyone but the owner -- and
    /// deleted. A failed delete is remembered and retried every pass
    /// (`unfinished`), and the record is already home either way.
    ///
    /// # What the answer says when the second half fails
    ///
    /// Only what is true. Unfiled but not deleted: the person has lost it,
    /// so the stop succeeded -- and the unfiled org copy rides in `changed`
    /// beside the personal one, so the daemon does not keep showing it in
    /// the person's collection. Neither unfiled nor deleted: the person can
    /// still see it, so the stop is refused with the stop's own sentence, and
    /// the view keeps them on the record. The personal copy exists and the
    /// move is pending; the next pass finishes it, and so does a retry of the
    /// stop -- through [`Self::finish_pending`], which makes no second copy.
    ///
    /// # The create's answer is read in full
    ///
    /// A reply that does not decrypt cleanly, is not personal, or names no
    /// id is refused as the pre-send check refuses a record: nothing is
    /// unfiled or deleted, so the org copy stays exactly as it was. A copy
    /// the server did make (its reply names an id, and not the org copy's)
    /// is deleted before the refusal, so a failed move back leaves no
    /// duplicate behind.
    ///
    /// # Nothing comes home under the org key
    ///
    /// [`Self::moved_body`]'s guarantee, reversed, and by the same walk
    /// ([`crate::rest::write::MappedCipher::rekey_carried`]). A record without
    /// its own key has every field under the ORG key while it is shared;
    /// `encrypt_item` re-encrypts the modelled ones under the user key, and
    /// the walk moves every carried leaf (a URI checksum, a passkey,
    /// NodeWarden's `data`) from the org key to the user key. Left under the
    /// org key, those would be unreadable in the personal copy the moment the
    /// org copy is deleted -- the one moment nothing can undo. A record WITH
    /// its own key has it re-wrapped from the org key to the user key, and
    /// the walk checks every leaf opens under that key (`(K, K)`).
    ///
    /// # What cannot come home whole is not moved
    ///
    /// Each refused before anything is written, with its own sentence:
    ///
    /// * **The trash or the archive** ([`STOP_IN_TRASH`]): it would come home
    ///   live. Only here, at the move back -- taking one of several people
    ///   off a trashed record moves nothing, and `forget` must still be able
    ///   to.
    /// * **An attachment** ([`MOVE_BACK_ATTACHMENTS`]): it belongs to the org
    ///   cipher's id (decision 3's reason, from the other side), and the
    ///   delete that ends a move back would destroy it. The share refuses
    ///   attachments, but the web vault can add one to a record already
    ///   shared.
    /// * **A field that did not decrypt, or a leaf under neither key**
    ///   ([`UNREADABLE_PART`], the share's own sentence): it would arrive
    ///   unreadable.
    fn move_back(
        &self,
        ctx: &Ctx,
        record: &crate::rest::sync::DecryptedItem,
        failures: &[crate::rest::sync::DecryptFailure],
        refused: &str,
    ) -> Result<StopOutcome, SharingError> {
        let say = |sentence: &str| SharingError::Refused(sentence.to_string());
        let refuse = || say(MOVE_BACK_FAILED);
        let unreadable = || say(UNREADABLE_PART);
        if in_trash_or_archive(&record.item) {
            return Err(say(STOP_IN_TRASH));
        }
        if has_attachments(&record.item) {
            return Err(say(MOVE_BACK_ATTACHMENTS));
        }
        if !failures.is_empty() {
            return Err(unreadable());
        }
        let old = record.item.id.clone();
        let placed = self
            .setup()
            .state_path
            .map(|path| crate::sharing_state::SharingState::load(&path).account(&ctx.account.id))
            .and_then(|state| state.moves.get(&old).cloned())
            .unwrap_or_default();
        let mut home = record.item.clone();
        home.id = String::new();
        let own_key = match home.other.get("key").and_then(serde_json::Value::as_str).map(str::to_string) {
            Some(wrapped) => {
                let wrapped = wrapped.parse::<EncString>().map_err(|_| unreadable())?;
                let cipher_key = crate::rest::crypto::unwrap_key_under(&wrapped, &ctx.org_key).map_err(|_| unreadable())?;
                let rewrapped = crate::rest::crypto::wrap_key_under(&cipher_key, ctx.keys.user()).map_err(crypto)?;
                home.other.insert("key".into(), serde_json::Value::String(rewrapped.to_string()));
                Some(cipher_key)
            }
            None => None,
        };
        home.other.insert("organizationId".into(), serde_json::Value::Null);
        for field in ["collectionIds", "edit", "viewPassword", "permissions", "revisionDate", "lastKnownRevisionDate", "creationDate"] {
            home.other.remove(field);
        }
        home.folder_id = placed.folder_id;
        home.favorite = placed.favorite;
        let mut cipher = crate::rest::write::encrypt_item(&record.carrying(home), &ctx.keys)
            .map_err(|e| crypto(e).or_say(MOVE_BACK_FAILED))?;
        match &own_key {
            Some(key) => cipher.rekey_carried(key, key),
            None => cipher.rekey_carried(&ctx.org_key, ctx.keys.user()),
        }
        .map_err(|_| unreadable())?;
        let answer = self
            .call(|client, auth| client.create_cipher(&mut auth.session, &cipher))
            .map_err(|e| e.or_say(MOVE_BACK_FAILED))?;
        let created = match crate::rest::sync::decrypt_cipher(&answer, &ctx.keys) {
            Some((created, failures))
                if failures.is_empty() && !created.item.id.is_empty() && created.item.id != old && organization_of(&created.item).is_none() =>
            {
                created
            }
            _ => {
                // Never the org copy's id: a server echoing that one back is
                // not naming a copy it made.
                if let Some(made) = answer.get("id").and_then(serde_json::Value::as_str).filter(|id| !id.is_empty() && *id != old) {
                    if let Err(e) = self.call(|client, auth| client.hard_delete_cipher(&mut auth.session, made)) {
                        log::warn!("a personal copy whose answer could not be read was left behind: {e}");
                    }
                }
                return Err(refuse());
            }
        };
        let new = created.item.id.clone();

        let (unfiled, purged) = self.second_half(&old);
        self.rekey_local(&ctx.account.id, &old, &new, purged.is_err());
        let mut changed = SharingOutcome { upserted: vec![created.item], removed: Vec::new() };
        match (unfiled, purged) {
            (_, Ok(())) => changed.removed.push(old),
            (Ok(answer), Err(_)) => changed.upserted.extend(self.readable(&answer, &ctx.keys)),
            (Err(_), Err(_)) => return Err(SharingError::Refused(refused.to_string())),
        }
        Ok(StopOutcome { moved_to: Some(new), changed })
    }

    /// **A stop on a record whose move back is pending**: its second half
    /// again -- unfile, then delete -- and the personal copy it already has.
    /// See [`Self::stop_with`] on why this is never a second move. The answer
    /// follows [`Self::move_back`]'s rule: refused while the person can
    /// still see the org copy, and otherwise the new id, with the personal
    /// copy re-read so the daemon holds it even when the first try's refusal
    /// never told it.
    ///
    /// **The refusal names `email` only when it is true of them.** Any stop
    /// reaches this path, whoever it names, so "{email} can still see it" is
    /// said only when the org copy -- re-read for the purpose, on this rare
    /// branch alone -- is still filed in that person's collection. Otherwise
    /// the neutral [`STOP_FAILED_PREFIX`]: the stop did fail, and whoever can
    /// see the record, it is not someone this stop can name.
    fn finish_pending(&self, ctx: &Ctx, old: &str, personal_id: &str, email: &str) -> Result<StopOutcome, SharingError> {
        let (unfiled, purged) = self.second_half(old);
        let mut changed = SharingOutcome::default();
        match (unfiled, purged) {
            (_, Ok(())) => {
                self.strike_unfinished(&ctx.account.id, &[old.to_string()]);
                changed.removed.push(old.to_string());
            }
            (Ok(answer), Err(_)) => changed.upserted.extend(self.readable(&answer, &ctx.keys)),
            (Err(_), Err(_)) => {
                let theirs = ctx.collection_for(email);
                let still_theirs = theirs.is_some_and(|collection| {
                    self.fetch_record(old, &ctx.keys)
                        .is_ok_and(|(org_copy, _)| collection_ids_of(&org_copy.item).contains(&collection))
                });
                return Err(SharingError::Refused(if still_theirs {
                    format!("{STOP_FAILED_PREFIX} {email} can still see it.")
                } else {
                    STOP_FAILED_PREFIX.to_string()
                }));
            }
        }
        if let Ok((home, _)) = self.fetch_record(personal_id, &ctx.keys) {
            changed.upserted.push(home.item);
        }
        Ok(StopOutcome { moved_to: Some(personal_id.to_string()), changed })
    }

    /// **A move back's second half**: the org copy filed in nothing, then
    /// deleted, each tried whatever the other did. Filed in nothing first, so
    /// a failed delete leaves a copy nobody but the owner sees; a server that
    /// refuses the empty list still gets the delete. Both outcomes come back,
    /// because what the stop may say depends on both.
    fn second_half(&self, old: &str) -> (Result<serde_json::Value, SharingError>, Result<(), SharingError>) {
        let unfiled = self.call(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, old, &[]));
        if let Err(e) = &unfiled {
            log::info!("a record coming home could not be unfiled: {e}");
        }
        let purged = self.purge_org_copy(old);
        if let Err(e) = &purged {
            log::info!("a record came home and its org copy waits for the next pass: {e}");
        }
        (unfiled, purged)
    }

    /// A cipher answer as an item, when it reads cleanly; otherwise nothing,
    /// and the next whole read shows it.
    fn readable(&self, answer: &serde_json::Value, keys: &VaultKeys) -> Option<VaultItem> {
        crate::rest::sync::decrypt_cipher(answer, keys).filter(|(_, failures)| failures.is_empty()).map(|(read, _)| read.item)
    }

    /// The personal id of a move back still pending for `org_cipher_id`.
    fn pending_move(&self, account: &str, org_cipher_id: &str) -> Option<String> {
        let path = self.setup().state_path?;
        crate::sharing_state::SharingState::load(&path).account(account).unfinished.into_iter().find_map(|entry| {
            let crate::sharing_state::Unfinished::PurgeOrgCopy { org_cipher_id: id, personal_id } = entry;
            (id == org_cipher_id).then_some(personal_id)
        })
    }

    /// Strikes finished moves back from `unfinished`.
    fn strike_unfinished(&self, account: &str, org_cipher_ids: &[String]) {
        let Some(path) = self.setup().state_path else {
            return;
        };
        let written = crate::sharing_state::update(&path, |state| {
            state.account_mut(account).unfinished.retain(|entry| {
                let crate::sharing_state::Unfinished::PurgeOrgCopy { org_cipher_id, .. } = entry;
                !org_cipher_ids.contains(org_cipher_id)
            });
        });
        if let Err(e) = written {
            log::warn!("a finished move back could not be struck from the sharing state: {e}");
        }
    }

    /// `DELETE /api/ciphers/{id}/delete` -- NodeWarden's unconditional
    /// permanent delete; the bare route only trashes (see
    /// `RestClient::hard_delete_cipher`).
    ///
    /// **A 404 is done**: the org copy is already gone. Matched on the status
    /// itself, not on the [`VaultError::Http`] it maps to -- that one is every
    /// 4xx, and reading a 403 as "gone" would drop the retry and leave the
    /// copy in the org, filed nowhere, for good. A 5xx or a lost connection
    /// stays an error, and so pending.
    fn purge_org_copy(&self, id: &str) -> Result<(), SharingError> {
        self.call(|client, auth| match client.hard_delete_cipher(&mut auth.session, id) {
            Err(RestError::Status(404)) => Ok(()),
            other => other,
        })
    }

    /// Every local file that holds a record by id follows it to its new one
    /// (decision 14: the daemon owns them), and the move is either done or
    /// remembered as unfinished.
    fn rekey_local(&self, account: &str, old: &str, new: &str, unfinished: bool) {
        let setup = self.setup();
        if let Some(path) = setup.sent_path {
            if let Err(e) = crate::sent_history::update(&path, |history| history.rekey_item(old, new)) {
                log::warn!("a record came home and the sent history could not follow it: {e}");
            }
        }
        if let Some(path) = setup.fill_stats_path {
            crate::fill_stats::FillStats::new(path).rekey(old, new);
        }
        crate::fill_recall::rekey_production(old, new);
        if let Some(path) = setup.state_path {
            let written = crate::sharing_state::update(&path, |state| {
                let me = state.account_mut(account);
                me.moves.remove(old);
                if unfinished {
                    me.unfinished.push(crate::sharing_state::Unfinished::PurgeOrgCopy {
                        org_cipher_id: old.to_string(),
                        personal_id: new.to_string(),
                    });
                }
            });
            if let Err(e) = written {
                log::warn!("the sharing state could not be written after a record came home: {e}");
            }
        }
    }

    /// A folder-and-star entry for a record that is not in the sharing org
    /// -- see [`Self::stop_with`]. Read first, so a stop that finds nothing
    /// stale writes nothing.
    fn drop_stale_move(&self, account: &str, item_id: &str) {
        let Some(path) = self.setup().state_path else {
            return;
        };
        if !crate::sharing_state::SharingState::load(&path).account(account).moves.contains_key(item_id) {
            return;
        }
        if let Err(e) = crate::sharing_state::update(&path, |state| {
            state.account_mut(account).moves.remove(item_id);
        }) {
            log::warn!("a stale sharing move could not be dropped: {e}");
        }
    }

    /// **The second halves that failed**, retried on every pass (spec 5.4,
    /// 13): unfile, then delete ([`Self::second_half`]) -- the unfile too,
    /// since a first try that failed at both left the person able to see the
    /// org copy. An org copy already gone (404) counts as done -- see
    /// [`Self::purge_org_copy`]. A finished move announces the org copy gone
    /// and its personal copy, re-read with the keys the backend holds: the
    /// stop that made that copy may have been refused, and so never have
    /// told the daemon about it.
    pub(crate) fn finish_unfinished(&self) -> SharingOutcome {
        let mut outcome = SharingOutcome::default();
        let (Some(path), Some(account)) = (self.setup().state_path, self.held().account.clone()) else {
            return outcome;
        };
        let pending = crate::sharing_state::SharingState::load(&path).account(&account.id).unfinished;
        if pending.is_empty() {
            return outcome;
        }
        let keys = self.keys.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let mut done = Vec::new();
        for entry in pending {
            let crate::sharing_state::Unfinished::PurgeOrgCopy { org_cipher_id, personal_id } = entry;
            if self.second_half(&org_cipher_id).1.is_err() {
                continue;
            }
            if let Some(keys) = &keys {
                if let Ok((home, _)) = self.fetch_record(&personal_id, keys) {
                    outcome.upserted.push(home.item);
                }
            }
            outcome.removed.push(org_cipher_id.clone());
            done.push(org_cipher_id);
        }
        if !done.is_empty() {
            self.strike_unfinished(&account.id, &done);
        }
        outcome
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
        let moved = server.mock("PUT", "/api/ciphers/live-1/share").with_status(500).expect_at_least(1).create();
        let before = serde_json::to_value(cache.get_by_id("live-1").expect("in the daemon's vault")).expect("json");
        let answer = crate::vault_api::tests::ask(&api, "POST", "/sharing/item/live-1", Some(&token), r#"{"email":"anna@example.com"}"#);
        moved.assert();
        assert_eq!(answer.status, 400);
        let body: serde_json::Value = serde_json::from_str(&answer.body).expect("json");
        assert_eq!(body["message"], SHARE_FAILED);
        // The move WAS tried (above) and failed: the daemon's copy is the one
        // it held before, byte for byte, and no window was told otherwise.
        let after = serde_json::to_value(cache.get_by_id("live-1").expect("still in the daemon's vault")).expect("json");
        assert_eq!(after, before, "a failed move changed the daemon's copy");
        assert!(api.feed().wait_after(Some(0), false, std::time::Duration::ZERO).changes.is_empty(), "a failed move was announced");
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

    // ---- fix round 1: what a move carries, and the risks the review named ----

    fn user_sealed(plain: &str) -> String {
        crate::rest::crypto::tests::seal(&crate::rest::backend::tests::fixture_user_key(), plain.as_bytes())
    }

    /// The fixture org already has anna, confirmed, in her own collection: a
    /// share of a personal record is its read and its `/share`.
    fn anna_is_confirmed(server: &mut MockServer) {
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": [person_collection("c-anna", "anna@example.com")] }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[
                roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]),
                roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"]),
            ]))
            .create();
    }

    /// `/share` for `id`, answered with the echo, the cipher it was sent kept
    /// for the test.
    fn share_captured(server: &mut MockServer, id: &'static str) -> (Arc<Mutex<Option<serde_json::Value>>>, crate::test_http::Mock) {
        let captured = Arc::new(Mutex::new(None));
        let keep = Arc::clone(&captured);
        let reply = echo(id, Some(FIXTURE_ORG), &["c-anna"]);
        let mock = server
            .mock("PUT", format!("/api/ciphers/{id}/share").as_str())
            .with_body_from_request(move |request| {
                let body: serde_json::Value = serde_json::from_slice(request.body()).expect("json");
                *keep.lock().expect("unpoisoned") = Some(body["cipher"].clone());
                reply(request)
            })
            .expect(1)
            .create();
        (captured, mock)
    }

    /// Every ciphertext leaf of a cipher body but its own `key`, at any depth,
    /// and inside a string that is JSON (NodeWarden's `data`).
    fn ciphertext_leaves(cipher: &serde_json::Value) -> Vec<String> {
        fn walk(value: &serde_json::Value, out: &mut Vec<String>) {
            match value {
                serde_json::Value::String(text) => {
                    if text.parse::<EncString>().is_ok() {
                        out.push(text.clone());
                    } else if let Ok(nested @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) =
                        serde_json::from_str::<serde_json::Value>(text)
                    {
                        walk(&nested, out);
                    }
                }
                serde_json::Value::Array(list) => list.iter().for_each(|v| walk(v, out)),
                serde_json::Value::Object(map) => map.values().for_each(|v| walk(v, out)),
                _ => {}
            }
        }
        let mut body = cipher.clone();
        if let Some(object) = body.as_object_mut() {
            object.remove("key");
        }
        let mut out = Vec::new();
        walk(&body, &mut out);
        out
    }

    fn opens_under(key: &SymmetricKey, leaf: &str) -> Option<String> {
        let enc = leaf.parse::<EncString>().ok()?;
        crate::rest::crypto::decrypt(key, &enc).ok().map(|plain| String::from_utf8_lossy(&plain).into_owned())
    }

    /// **A record without its own key moves with nothing left under the user
    /// key** -- the Critical of the Task 9 review. `encrypt_item` re-encrypts
    /// only what this crate models; a URI checksum, a passkey and
    /// NodeWarden's `data` ride through, and NodeWarden merges the body over
    /// the stored row, so each would have landed in the org unreadable by
    /// everyone in it. Every leaf of what went on the wire opens under the
    /// ORG key, and the carried ones say what they said before.
    #[test]
    fn a_record_without_its_own_key_moves_with_every_leaf_under_the_org_key() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let mut record = personal_cipher("p1", "Acme VPN");
        record["login"]["uris"] =
            serde_json::json!([{ "uri": user_sealed("https://acme.example"), "uriChecksum": user_sealed("checksum-1"), "match": null }]);
        record["login"]["fido2Credentials"] = serde_json::json!([{
            "credentialId": user_sealed("cred-1"), "keyType": user_sealed("public-key"), "keyAlgorithm": user_sealed("ECDSA"),
            "keyCurve": user_sealed("P-256"), "keyValue": user_sealed("pkcs8-bytes"), "rpId": user_sealed("acme.example"),
            "userHandle": user_sealed("handle"), "counter": user_sealed("0"), "discoverable": user_sealed("true"),
            "creationDate": "2024-01-01T00:00:00.000Z"
        }]);
        record["passwordHistory"] =
            serde_json::json!([{ "password": user_sealed("old-password"), "lastUsedDate": "2023-01-01T00:00:00.000Z" }]);
        record["data"] =
            serde_json::Value::String(serde_json::json!({ "name": user_sealed("Acme VPN"), "extra": [user_sealed("data-leaf")] }).to_string());
        server.mock("GET", "/api/ciphers/p1").with_body(record.to_string()).create();
        anna_is_confirmed(&mut server);
        let (captured, share) = share_captured(&mut server, "p1");

        let outcome = engine.share("p1", "anna@example.com").expect("shared");
        share.assert();
        assert_eq!(outcome.result, ShareResult::Shared);
        let sent = captured.lock().expect("unpoisoned").clone().expect("a body went");
        assert!(sent["key"].is_null(), "no key of its own was invented");
        let org = crate::rest::backend::tests::fixture_org_key();
        let leaves = ciphertext_leaves(&sent);
        assert!(leaves.len() >= 18, "the walk saw only {} leaves", leaves.len());
        for leaf in &leaves {
            assert!(opens_under(&org, leaf).is_some(), "a leaf went to the org still under another key");
        }
        let open = |value: &serde_json::Value| value.as_str().and_then(|leaf| opens_under(&org, leaf));
        assert_eq!(open(&sent["login"]["uris"][0]["uriChecksum"]).as_deref(), Some("checksum-1"));
        assert_eq!(open(&sent["login"]["fido2Credentials"][0]["keyValue"]).as_deref(), Some("pkcs8-bytes"));
        assert_eq!(open(&sent["passwordHistory"][0]["password"]).as_deref(), Some("old-password"));
        let data: serde_json::Value = serde_json::from_str(sent["data"].as_str().expect("data stays a string")).expect("JSON");
        assert_eq!(open(&data["extra"][0]).as_deref(), Some("data-leaf"));
        assert_eq!(sent["login"]["fido2Credentials"][0]["creationDate"], "2024-01-01T00:00:00.000Z", "plaintext rides unchanged");
    }

    /// **NodeWarden's own types 6-8 move under the org key too.** This crate
    /// models none of them, so every field of a bank account, a driver's
    /// licence or a passport is carried -- and re-keyed by the walk.
    #[test]
    fn nodewardens_bank_account_licence_and_passport_move_under_the_org_key() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        anna_is_confirmed(&mut server);
        let org = crate::rest::backend::tests::fixture_org_key();
        for (id, kind, object) in [("b6", 6, "bankAccount"), ("b7", 7, "driversLicense"), ("b8", 8, "passport")] {
            let mut record = personal_cipher(id, "Papers");
            record["type"] = serde_json::json!(kind);
            record["login"] = serde_json::Value::Null;
            record[object] = serde_json::json!({ "number": user_sealed(&format!("{object}-number")), "holder": user_sealed("Denis") });
            server.mock("GET", format!("/api/ciphers/{id}").as_str()).with_body(record.to_string()).create();
            let (captured, share) = share_captured(&mut server, id);
            engine.share(id, "anna@example.com").expect("shared");
            share.assert();
            let sent = captured.lock().expect("unpoisoned").clone().expect("a body went");
            for leaf in ciphertext_leaves(&sent) {
                assert!(opens_under(&org, &leaf).is_some(), "type {kind}: a leaf is not under the org key");
            }
            let number = sent[object]["number"].as_str().and_then(|leaf| opens_under(&org, leaf));
            assert_eq!(number, Some(format!("{object}-number")), "type {kind}");
        }
    }

    /// **A record with its own key keeps it, re-wrapped under the org key**,
    /// and its fields -- carried ones included -- stay under that key.
    #[test]
    fn a_record_with_its_own_key_moves_that_key_rewrapped_under_the_org_key() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let own = crate::rest::crypto::generate_symmetric_key().expect("a key");
        let sealed = |plain: &str| crate::rest::crypto::tests::seal(&own, plain.as_bytes());
        let checksum = sealed("checksum-1");
        let mut record = personal_cipher("k1", "Acme VPN");
        record["key"] = serde_json::json!(
            crate::rest::crypto::wrap_key_under(&own, &crate::rest::backend::tests::fixture_user_key()).expect("wrapped").to_string()
        );
        record["name"] = serde_json::json!(sealed("Acme VPN"));
        record["login"] = serde_json::json!({
            "username": sealed("u@example.com"), "password": sealed("p4ssw0rd"),
            "uris": [{ "uri": sealed("https://acme.example"), "uriChecksum": checksum.clone(), "match": null }],
            "fido2Credentials": []
        });
        server.mock("GET", "/api/ciphers/k1").with_body(record.to_string()).create();
        anna_is_confirmed(&mut server);
        let (captured, share) = share_captured(&mut server, "k1");

        let outcome = engine.share("k1", "anna@example.com").expect("shared");
        share.assert();
        assert_eq!(outcome.changed.upserted[0].name, "Acme VPN");
        let sent = captured.lock().expect("unpoisoned").clone().expect("a body went");
        let rewrapped: EncString = sent["key"].as_str().expect("the key went").parse().expect("an EncString");
        let unwrapped = crate::rest::crypto::unwrap_key_under(&rewrapped, &crate::rest::backend::tests::fixture_org_key())
            .expect("the record's key opens under the org key");
        assert_eq!(opens_under(&unwrapped, sent["name"].as_str().expect("a name")).as_deref(), Some("Acme VPN"));
        for leaf in ciphertext_leaves(&sent) {
            assert!(opens_under(&unwrapped, &leaf).is_some(), "a leaf is not under the record's own key");
        }
        assert_eq!(
            sent["login"]["uris"][0]["uriChecksum"],
            serde_json::json!(checksum),
            "a carried leaf under the unchanged key is left alone"
        );
    }

    /// **A key that differs from the pin is not wrapped to** (spec 12.1): the
    /// person is added without the org key, the card will say their key
    /// changed, and the pin is not moved to the new key.
    #[test]
    fn a_key_that_differs_from_the_pin_is_not_wrapped_to() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let state = dir.join("sharing_state.json");
        let old = crate::sharing_state::Pin { sha256: "00".repeat(32), user_id: "anna-id".into(), at: 1 };
        crate::sharing_state::update(&state, |s| s.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", old.clone())).expect("pinned");
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": [person_collection("c-anna", "anna@example.com")] }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/members/public-key?email=anna%40example.com").as_str())
            .with_body(serde_json::json!({ "userId": "anna-id", "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        let invite = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/members/invite").as_str())
            .match_request(|request| !request.utf8_lossy_body().contains("encryptedOrgKey"))
            .with_body(r#"{"id":"anna-id","status":1,"object":"organizationMember"}"#)
            .expect(1)
            .create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-anna"])).create();
        let outcome = engine.share("p1", "anna@example.com").expect("shared");
        invite.assert();
        assert_eq!(outcome.result, ShareResult::WaitingAccept);
        assert!(engine.held().key_changed.contains("anna@example.com"));
        let pinned = crate::sharing_state::SharingState::load(&state).account(FIXTURE_USER_ID);
        assert_eq!(pinned.pin_for("anna@example.com"), Some(&old), "the pin moved to a key nobody confirmed");
    }

    /// **The trash, another organisation, and a field that did not decrypt**
    /// (spec 9.3): each refused with its own sentence, and nothing written.
    #[test]
    fn what_cannot_move_is_refused_with_its_own_sentence() {
        let mut team = sharing_org_row();
        team["id"] = serde_json::json!("org-team");
        team["name"] = serde_json::json!("Team");
        team["role"] = serde_json::json!("user");
        team["type"] = serde_json::json!(2);
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row(), team]));
        let mut trashed = personal_cipher("t1", "Old");
        trashed["deletedDate"] = serde_json::json!("2026-09-01T00:00:00.000Z");
        let mut theirs = shared_cipher("o1", "Team VPN", &[]);
        theirs["organizationId"] = serde_json::json!("org-team");
        let mut broken = personal_cipher("u1", "Half");
        broken["notes"] =
            serde_json::json!(crate::rest::crypto::tests::seal(&crate::rest::backend::tests::fixture_org_key(), b"not under this key"));
        server.mock("GET", "/api/ciphers/t1").with_body(trashed.to_string()).create();
        server.mock("GET", "/api/ciphers/o1").with_body(theirs.to_string()).create();
        server.mock("GET", "/api/ciphers/u1").with_body(broken.to_string()).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        for (id, sentence) in [("t1", IN_TRASH), ("o1", OTHER_ORG), ("u1", UNREADABLE_PART)] {
            match engine.share(id, "anna@example.com") {
                Err(SharingError::Refused(said)) => assert_eq!(said, sentence, "{id}"),
                other => panic!("{id}: expected a refusal, got {other:?}"),
            }
        }
        writes.assert();
        puts.assert();
    }

    fn stale_share(server: &mut MockServer, id: &str) -> crate::test_http::Mock {
        server
            .mock("PUT", format!("/api/ciphers/{id}/share").as_str())
            .with_status(400)
            .with_body(r#"{"message":"The client copy of this cipher is out of date. Resync the client and try again."}"#)
            .expect(1)
            .create()
    }

    /// **A stale revision is read again and retried once** (spec 13).
    #[test]
    fn a_stale_revision_is_read_again_and_retried_once() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let read = server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).expect(2).create();
        anna_is_confirmed(&mut server);
        let stale = stale_share(&mut server, "p1");
        let (_, retried) = share_captured(&mut server, "p1");
        let outcome = engine.share("p1", "anna@example.com").expect("shared on the retry");
        stale.assert();
        retried.assert();
        read.assert();
        assert_eq!(outcome.result, ShareResult::Shared);
    }

    /// **The retry judges the fresh copy again**: one that went to the trash
    /// in between is refused, not moved.
    #[test]
    fn a_stale_retry_refuses_a_fresh_copy_that_can_no_longer_move() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).expect(1).create();
        let mut trashed = personal_cipher("p1", "Acme VPN");
        trashed["deletedDate"] = serde_json::json!("2026-09-01T00:00:00.000Z");
        server.mock("GET", "/api/ciphers/p1").with_body(trashed.to_string()).expect(1).create();
        anna_is_confirmed(&mut server);
        let stale = stale_share(&mut server, "p1");
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::Refused(said)) => assert_eq!(said, IN_TRASH),
            other => panic!("expected the trash refusal, got {other:?}"),
        }
        stale.assert();
    }

    /// **An answer that is not a roster is an error, not nobody**: read as
    /// empty, the share would invite a member again, and a reload would show
    /// everyone as gone.
    #[test]
    fn an_unreadable_roster_is_an_error_and_nothing_is_invited() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": [person_collection("c-anna", "anna@example.com")] }).to_string())
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(r#"{"message":"Not found"}"#)
            .create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        assert!(matches!(engine.fetch_roster(FIXTURE_ORG), Err(SharingError::Refused(s)) if s == ROSTER_UNREADABLE));
        assert!(matches!(engine.share("p1", "anna@example.com"), Err(SharingError::Refused(s)) if s == ROSTER_UNREADABLE));
        writes.assert();
        puts.assert();
    }

    // ---- stopping, and the move back (Task 10) ----------------------------------

    fn with_collections(server: &mut MockServer, rows: &[serde_json::Value]) {
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(serde_json::json!({ "data": rows }).to_string())
            .expect_at_least(0)
            .create();
    }

    fn with_roster(server: &mut MockServer, rows: &[serde_json::Value]) {
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(rows))
            .expect_at_least(0)
            .create();
    }

    /// **Stopping one of two people is one collections call** (spec 6.8).
    #[test]
    fn stopping_one_of_two_people_takes_them_off_the_record_and_nothing_else() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com"), person_collection("c-bob", "bob@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna", "c-bob"]).to_string()).create();
        let off = server
            .mock("PUT", "/api/ciphers/s1/collections")
            .match_body(Matcher::Json(serde_json::json!({ "collectionIds": ["c-bob"] })))
            .with_body(shared_cipher("s1", "Acme VPN", &["c-bob"]).to_string())
            .expect(1)
            .create();
        let creates = server.mock("POST", "/api/ciphers").expect(0).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("stopped");
        off.assert();
        creates.assert();
        assert_eq!(outcome.moved_to, None);
        assert_eq!(outcome.changed.upserted.len(), 1);
    }

    /// **Stopping the last person moves the record home** (spec 6.8, 9.4):
    /// create the personal copy, unfile and delete the org copy, give back its
    /// folder and star, and re-key every local file that held the old id.
    #[test]
    fn stopping_the_last_person_creates_then_deletes_and_rekeys_local_files() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).moves.insert("s1".into(), crate::sharing_state::Move { folder_id: Some("f1".into()), favorite: true });
        }).expect("seeded");
        crate::sent_history::update(&dir.join("sent_history.json"), |h| h.record(crate::sent_history::SentRecord {
            kind: crate::sent_history::SentKind::Share, item_id: "s1".into(), recipient: "anna@example.com".into(), ..Default::default()
        })).expect("seeded");
        std::fs::write(dir.join("fill-stats.json"), r#"{"s1":3}"#).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        let create = server
            .mock("POST", "/api/ciphers")
            .match_request(|request| {
                let body: serde_json::Value = serde_json::from_slice(request.body()).expect("json");
                body["organizationId"].is_null() && body["folderId"] == "f1" && body["favorite"] == true && body.get("collectionIds").is_none()
            })
            .with_body_from_request(echo("p-new", None, &[]))
            .expect(1)
            .create();
        let unfile = server.mock("PUT", "/api/ciphers/s1/collections").with_body(shared_cipher("s1", "Acme VPN", &[]).to_string()).expect(1).create();
        let delete = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(204).expect(1).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("stopped");
        create.assert();
        unfile.assert();
        delete.assert();
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        assert_eq!(outcome.changed.removed, ["s1"]);
        assert_eq!(outcome.changed.upserted[0].name, "Acme VPN", "it decrypts under the user key again");
        assert_eq!(organization_of(&outcome.changed.upserted[0]), None);
        let sent = crate::sent_history::SentHistory::load(&dir.join("sent_history.json"));
        assert_eq!(sent.entries[0].item_id, "p-new");
        assert_eq!(std::fs::read_to_string(dir.join("fill-stats.json")).expect("reads"), r#"{"p-new":3}"#);
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.moves.is_empty() && state.unfinished.is_empty());
    }

    /// **A half-finished move back is finished on the next pass** (spec 13):
    /// the delete failed, the copy is filed nowhere, and the next pass
    /// deletes it and announces it gone.
    #[test]
    fn a_half_finished_move_back_is_finished_on_the_next_pass() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        server.mock("POST", "/api/ciphers").with_body_from_request(echo("p-new", None, &[])).create();
        server.mock("PUT", "/api/ciphers/s1/collections").with_body(shared_cipher("s1", "Acme VPN", &[]).to_string()).create();
        server.mock("DELETE", "/api/ciphers/s1/delete").with_status(500).expect(1).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("the personal copy is made; the delete waits");
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        assert!(outcome.changed.removed.is_empty(), "the org copy is still there");
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(state.unfinished.len(), 1);

        let retried = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(204).expect(1).create();
        let pass = engine.refresh(Trigger::Batch).expect("a pass");
        retried.assert();
        assert_eq!(pass.outcome.removed, ["s1"]);
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.unfinished.is_empty());
    }

    fn org_sealed(plain: &str) -> String {
        crate::rest::crypto::tests::seal(&crate::rest::backend::tests::fixture_org_key(), plain.as_bytes())
    }

    /// anna is the one person on `id`, and the unfile and the delete a move
    /// back ends with both answer.
    fn anna_is_the_last(server: &mut MockServer, id: &str) {
        with_collections(server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(server, &[]);
        server
            .mock("PUT", format!("/api/ciphers/{id}/collections").as_str())
            .with_body(shared_cipher(id, "Acme VPN", &[]).to_string())
            .expect_at_least(0)
            .create();
        server.mock("DELETE", format!("/api/ciphers/{id}/delete").as_str()).with_status(204).expect_at_least(0).create();
    }

    /// `POST /api/ciphers`, answered with the echo as `new_id`, the body it
    /// was sent kept for the test.
    fn create_captured(server: &mut MockServer, new_id: &'static str) -> (Arc<Mutex<Option<serde_json::Value>>>, crate::test_http::Mock) {
        let captured = Arc::new(Mutex::new(None));
        let keep = Arc::clone(&captured);
        let reply = echo(new_id, None, &[]);
        let mock = server
            .mock("POST", "/api/ciphers")
            .with_body_from_request(move |request| {
                let body: serde_json::Value = serde_json::from_slice(request.body()).expect("json");
                *keep.lock().expect("unpoisoned") = Some(body);
                reply(request)
            })
            .expect(1)
            .create();
        (captured, mock)
    }

    /// **A record without its own key comes home with nothing left under the
    /// org key** -- the share's guarantee, reversed. The carried leaves (a
    /// URI checksum, a passkey, NodeWarden's `data`) are under the ORG key
    /// while the record is shared; left there, the personal copy would be
    /// unreadable the moment the org copy is deleted. Every leaf of what
    /// went on the wire opens under the USER key and says what it said.
    #[test]
    fn a_record_without_its_own_key_comes_home_with_every_leaf_under_the_user_key() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let mut record = shared_cipher("s1", "Acme VPN", &["c-anna"]);
        record["login"]["uris"] =
            serde_json::json!([{ "uri": org_sealed("https://acme.example"), "uriChecksum": org_sealed("checksum-1"), "match": null }]);
        record["login"]["fido2Credentials"] = serde_json::json!([{
            "credentialId": org_sealed("cred-1"), "keyType": org_sealed("public-key"), "keyAlgorithm": org_sealed("ECDSA"),
            "keyCurve": org_sealed("P-256"), "keyValue": org_sealed("pkcs8-bytes"), "rpId": org_sealed("acme.example"),
            "userHandle": org_sealed("handle"), "counter": org_sealed("0"), "discoverable": org_sealed("true"),
            "creationDate": "2024-01-01T00:00:00.000Z"
        }]);
        record["passwordHistory"] =
            serde_json::json!([{ "password": org_sealed("old-password"), "lastUsedDate": "2023-01-01T00:00:00.000Z" }]);
        record["data"] =
            serde_json::Value::String(serde_json::json!({ "name": org_sealed("Acme VPN"), "extra": [org_sealed("data-leaf")] }).to_string());
        server.mock("GET", "/api/ciphers/s1").with_body(record.to_string()).create();
        anna_is_the_last(&mut server, "s1");
        let (captured, create) = create_captured(&mut server, "p-new");

        let outcome = engine.stop("s1", "anna@example.com").expect("stopped");
        create.assert();
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        assert_eq!(outcome.changed.upserted[0].name, "Acme VPN");
        let sent = captured.lock().expect("unpoisoned").clone().expect("a body went");
        assert!(sent["key"].is_null(), "no key of its own was invented");
        let user = crate::rest::backend::tests::fixture_user_key();
        let leaves = ciphertext_leaves(&sent);
        assert!(leaves.len() >= 17, "the walk saw only {} leaves", leaves.len());
        for leaf in &leaves {
            assert!(opens_under(&user, leaf).is_some(), "a leaf came home still under another key");
        }
        let open = |value: &serde_json::Value| value.as_str().and_then(|leaf| opens_under(&user, leaf));
        assert_eq!(open(&sent["login"]["uris"][0]["uriChecksum"]).as_deref(), Some("checksum-1"));
        assert_eq!(open(&sent["login"]["fido2Credentials"][0]["keyValue"]).as_deref(), Some("pkcs8-bytes"));
        assert_eq!(open(&sent["passwordHistory"][0]["password"]).as_deref(), Some("old-password"));
        let data: serde_json::Value = serde_json::from_str(sent["data"].as_str().expect("data stays a string")).expect("JSON");
        assert_eq!(open(&data["extra"][0]).as_deref(), Some("data-leaf"));
    }

    /// **A record with its own key comes home with that key re-wrapped under
    /// the user key**, and its fields -- carried ones included -- stay under
    /// the record's key, untouched.
    #[test]
    fn a_record_with_its_own_key_comes_home_with_that_key_rewrapped_under_the_user_key() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let own = crate::rest::crypto::generate_symmetric_key().expect("a key");
        let sealed = |plain: &str| crate::rest::crypto::tests::seal(&own, plain.as_bytes());
        let checksum = sealed("checksum-1");
        let mut record = shared_cipher("k1", "Acme VPN", &["c-anna"]);
        record["key"] = serde_json::json!(
            crate::rest::crypto::wrap_key_under(&own, &crate::rest::backend::tests::fixture_org_key()).expect("wrapped").to_string()
        );
        record["name"] = serde_json::json!(sealed("Acme VPN"));
        record["login"] = serde_json::json!({
            "username": sealed("u@example.com"), "password": sealed("p4ssw0rd"),
            "uris": [{ "uri": sealed("https://acme.example"), "uriChecksum": checksum.clone(), "match": null }],
            "fido2Credentials": []
        });
        server.mock("GET", "/api/ciphers/k1").with_body(record.to_string()).create();
        anna_is_the_last(&mut server, "k1");
        let (captured, create) = create_captured(&mut server, "p-new");

        let outcome = engine.stop("k1", "anna@example.com").expect("stopped");
        create.assert();
        assert_eq!(outcome.changed.upserted[0].name, "Acme VPN");
        let sent = captured.lock().expect("unpoisoned").clone().expect("a body went");
        let rewrapped: EncString = sent["key"].as_str().expect("the key went").parse().expect("an EncString");
        let unwrapped = crate::rest::crypto::unwrap_key_under(&rewrapped, &crate::rest::backend::tests::fixture_user_key())
            .expect("the record's key opens under the user key");
        for leaf in ciphertext_leaves(&sent) {
            assert!(opens_under(&unwrapped, &leaf).is_some(), "a leaf is not under the record's own key");
        }
        assert_eq!(sent["login"]["uris"][0]["uriChecksum"], serde_json::json!(checksum), "a carried leaf under the unchanged key is left alone");
    }

    /// `record`, whose last person is anna, stopped: refused with `sentence`
    /// and nothing written -- no create, no unfile, no delete.
    fn stays_shared_with_nothing_written(record: &serde_json::Value, sentence: &str) {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        let id = record["id"].as_str().expect("an id").to_string();
        server.mock("GET", format!("/api/ciphers/{id}").as_str()).with_body(record.to_string()).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        match engine.stop(&id, "anna@example.com") {
            Err(SharingError::Refused(said)) => assert_eq!(said, sentence),
            other => panic!("expected {sentence:?}, got {other:?}"),
        }
        writes.assert();
        puts.assert();
        deletes.assert();
    }

    /// **A leaf that opens under neither key does not come home**: it would
    /// be unreadable in the personal copy. The share's own sentence.
    #[test]
    fn a_leaf_under_neither_key_does_not_come_home() {
        let stranger = crate::rest::crypto::generate_symmetric_key().expect("a key");
        let mut record = shared_cipher("s1", "Acme VPN", &["c-anna"]);
        record["login"]["uris"] = serde_json::json!([{
            "uri": org_sealed("https://acme.example"),
            "uriChecksum": crate::rest::crypto::tests::seal(&stranger, b"checksum-1"),
            "match": null
        }]);
        stays_shared_with_nothing_written(&record, UNREADABLE_PART);
    }

    /// **A modelled field that did not decrypt does not come home either.**
    #[test]
    fn a_field_that_did_not_decrypt_does_not_come_home() {
        let mut record = shared_cipher("s1", "Acme VPN", &["c-anna"]);
        record["notes"] = serde_json::json!(user_sealed("under the wrong key for an org record"));
        stays_shared_with_nothing_written(&record, UNREADABLE_PART);
    }

    /// **An attachment keeps the record shared, and says why**: it belongs to
    /// the org cipher id, and the delete that ends a move back would destroy
    /// it.
    #[test]
    fn a_record_with_an_attachment_does_not_come_home() {
        let mut record = shared_cipher("s1", "Acme VPN", &["c-anna"]);
        record["attachments"] = serde_json::json!([{ "id": "a1", "fileName": "2.x|y|z" }]);
        stays_shared_with_nothing_written(&record, MOVE_BACK_ATTACHMENTS);
    }

    /// **A record in the trash or the archive does not come home live.**
    #[test]
    fn a_trashed_or_archived_record_does_not_come_home_live() {
        for field in ["deletedDate", "archivedDate"] {
            let mut record = shared_cipher("s1", "Acme VPN", &["c-anna"]);
            record[field] = serde_json::json!("2026-09-01T00:00:00.000Z");
            stays_shared_with_nothing_written(&record, STOP_IN_TRASH);
        }
    }

    /// **Taking one of two people off a trashed record still works**: only
    /// the move back is refused, so `forget` is never stuck on the trash.
    #[test]
    fn one_of_two_people_comes_off_a_trashed_record() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com"), person_collection("c-bob", "bob@example.com")]);
        with_roster(&mut server, &[]);
        let mut record = shared_cipher("s1", "Acme VPN", &["c-anna", "c-bob"]);
        record["deletedDate"] = serde_json::json!("2026-09-01T00:00:00.000Z");
        server.mock("GET", "/api/ciphers/s1").with_body(record.to_string()).create();
        let mut unfiled = shared_cipher("s1", "Acme VPN", &["c-bob"]);
        unfiled["deletedDate"] = serde_json::json!("2026-09-01T00:00:00.000Z");
        let off = server.mock("PUT", "/api/ciphers/s1/collections").with_body(unfiled.to_string()).expect(1).create();
        engine.stop("s1", "anna@example.com").expect("stopped");
        off.assert();
    }

    /// A move back of `s1` (folder and star remembered) whose create is
    /// answered by `reply`: refused with [`MOVE_BACK_FAILED`], the org copy
    /// neither unfiled nor deleted, its folder and star still remembered, and
    /// nothing pending. `orphan` names the copy the reply made, which must be
    /// deleted -- and no other DELETE may be sent.
    fn a_refused_create(reply: impl FnOnce(crate::test_http::Mock) -> crate::test_http::Mock, orphan: Option<&str>) {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let path = dir.join("sharing_state.json");
        crate::sharing_state::update(&path, |state| {
            state.account_mut(FIXTURE_USER_ID).moves.insert("s1".into(), crate::sharing_state::Move { folder_id: Some("f1".into()), favorite: true });
        }).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        let create = reply(server.mock("POST", "/api/ciphers")).expect(1).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        let cleaned = orphan.map(|id| server.mock("DELETE", format!("/api/ciphers/{id}/delete").as_str()).with_status(204).expect(1).create());
        match engine.stop("s1", "anna@example.com") {
            Err(SharingError::Refused(said)) => assert_eq!(said, MOVE_BACK_FAILED),
            other => panic!("expected the move-back refusal, got {other:?}"),
        }
        create.assert();
        puts.assert();
        deletes.assert();
        if let Some(cleaned) = cleaned {
            cleaned.assert();
        }
        let state = crate::sharing_state::SharingState::load(&path).account(FIXTURE_USER_ID);
        assert!(state.moves.contains_key("s1"), "the folder and star were spent on a move that did not happen");
        assert!(state.unfinished.is_empty());
    }

    /// **A failed create changes nothing**: no unfile, no delete.
    #[test]
    fn a_failed_create_leaves_the_org_copy_untouched() {
        a_refused_create(|m| m.with_status(500), None);
    }

    /// **A create reply that cannot be read in full is refused, and the copy
    /// it names is deleted** -- a field that does not decrypt, a copy filed
    /// in an org, no id at all -- and a reply naming the ORG copy's id is
    /// never taken for a copy to delete.
    #[test]
    fn an_unreadable_create_reply_leaves_the_org_copy_untouched_and_no_duplicate() {
        let stranger = crate::rest::crypto::generate_symmetric_key().expect("a key");
        let mut half = personal_cipher("p-new", "Acme VPN");
        half["name"] = serde_json::json!(crate::rest::crypto::tests::seal(&stranger, b"Acme VPN"));
        a_refused_create(move |m| m.with_body(half.to_string()), Some("p-new"));
        a_refused_create(|m| m.with_body_from_request(echo("p-new", Some("org-elsewhere"), &[])), Some("p-new"));
        a_refused_create(|m| m.with_body(r#"{"object":"cipher"}"#), None);
        a_refused_create(|m| m.with_body_from_request(echo("s1", None, &[])), None);
    }

    /// **A stop after a half-finished move makes no second copy** (review
    /// round 1). The unfile and the delete both fail: the person can still
    /// see the org copy, so the stop says so -- twice, and the second one
    /// creates nothing -- and the one pending move is finished by the next
    /// pass that can: unfiled, deleted, and the personal copy announced.
    #[test]
    fn a_stop_after_a_half_finished_move_makes_no_second_copy() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let path = dir.join("sharing_state.json");
        crate::sharing_state::update(&path, |state| {
            state.account_mut(FIXTURE_USER_ID).moves.insert("s1".into(), crate::sharing_state::Move { folder_id: Some("f1".into()), favorite: true });
        }).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        let create = server.mock("POST", "/api/ciphers").with_body_from_request(echo("p-new", None, &[])).expect(1).create();
        let unfile_fails = server.mock("PUT", "/api/ciphers/s1/collections").with_status(500).expect(2).create();
        let delete_fails = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(500).expect(2).create();
        let still_sees = format!("{STOP_FAILED_PREFIX} anna@example.com can still see it.");
        for attempt in ["first", "second"] {
            match engine.stop("s1", "anna@example.com") {
                Err(SharingError::Refused(said)) => assert_eq!(said, still_sees, "{attempt}"),
                other => panic!("{attempt}: expected the still-sees refusal, got {other:?}"),
            }
        }
        create.assert();
        unfile_fails.assert();
        delete_fails.assert();
        let pending = crate::sharing_state::SharingState::load(&path).account(FIXTURE_USER_ID).unfinished;
        assert_eq!(pending, [crate::sharing_state::Unfinished::PurgeOrgCopy { org_cipher_id: "s1".into(), personal_id: "p-new".into() }]);

        let unfiled = server
            .mock("PUT", "/api/ciphers/s1/collections")
            .match_body(Matcher::Json(serde_json::json!({ "collectionIds": [] })))
            .with_body(shared_cipher("s1", "Acme VPN", &[]).to_string())
            .expect(1)
            .create();
        let deleted = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(204).expect(1).create();
        server.mock("GET", "/api/ciphers/p-new").with_body(personal_cipher("p-new", "Acme VPN").to_string()).create();
        let pass = engine.refresh(Trigger::Batch).expect("a pass");
        unfiled.assert();
        deleted.assert();
        create.assert();
        assert_eq!(pass.outcome.removed, ["s1"]);
        assert_eq!(pass.outcome.upserted.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["p-new"], "the personal copy is announced");
        assert!(crate::sharing_state::SharingState::load(&path).account(FIXTURE_USER_ID).unfinished.is_empty());
    }

    /// **A retried stop finishes the pending move itself**, and answers the
    /// copy it already made -- one create in all.
    #[test]
    fn a_retried_stop_finishes_the_pending_move_with_the_copy_it_already_made() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        let create = server.mock("POST", "/api/ciphers").with_body_from_request(echo("p-new", None, &[])).expect(1).create();
        server.mock("PUT", "/api/ciphers/s1/collections").with_status(500).expect(1).create();
        server.mock("DELETE", "/api/ciphers/s1/delete").with_status(500).expect(1).create();
        assert!(engine.stop("s1", "anna@example.com").is_err(), "the person can still see it");
        server.mock("PUT", "/api/ciphers/s1/collections").with_body(shared_cipher("s1", "Acme VPN", &[]).to_string()).expect(1).create();
        let deleted = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(204).expect(1).create();
        server.mock("GET", "/api/ciphers/p-new").with_body(personal_cipher("p-new", "Acme VPN").to_string()).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("finished");
        deleted.assert();
        create.assert();
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        assert_eq!(outcome.changed.removed, ["s1"]);
        assert_eq!(outcome.changed.upserted.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["p-new"]);
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.unfinished.is_empty());
    }

    /// **Unfiled but not deleted is a stop that worked**: the person has lost
    /// it, and the org copy's new filing rides in the outcome, so the daemon
    /// stops showing it in their collection.
    #[test]
    fn an_org_copy_unfiled_but_not_deleted_is_announced_filed_nowhere() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        server.mock("POST", "/api/ciphers").with_body_from_request(echo("p-new", None, &[])).create();
        server.mock("PUT", "/api/ciphers/s1/collections").with_body(shared_cipher("s1", "Acme VPN", &[]).to_string()).create();
        server.mock("DELETE", "/api/ciphers/s1/delete").with_status(500).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("stopped");
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        let org_copy = outcome.changed.upserted.iter().find(|i| i.id == "s1").expect("the unfiled org copy is announced");
        assert!(collection_ids_of(org_copy).is_empty());
    }

    /// **A failed invitations read does not skip a half-finished move**: the
    /// pass logs it and runs on.
    #[test]
    fn a_failed_invitations_read_does_not_skip_a_half_finished_move() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).unfinished.push(crate::sharing_state::Unfinished::PurgeOrgCopy {
                org_cipher_id: "s1".into(),
                personal_id: "p1".into(),
            });
        }).expect("seeded");
        server.mock("GET", "/api/organizations/invitations/mine").with_status(500).create();
        server.mock("PUT", "/api/ciphers/s1/collections").with_body(shared_cipher("s1", "Acme VPN", &[]).to_string()).create();
        let deleted = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(204).expect(1).create();
        let pass = engine.refresh(Trigger::WholeRead).expect("the pass runs on");
        deleted.assert();
        assert_eq!(pass.outcome.removed, ["s1"]);
    }

    // ---- follow-up: a pending move back, from every side -------------------------

    /// `s1`'s move back is pending: its personal copy is `p-new`.
    fn pending_purge(dir: &crate::test_scratch::ScratchDir) -> std::path::PathBuf {
        let path = dir.join("sharing_state.json");
        crate::sharing_state::update(&path, |state| {
            state.account_mut(FIXTURE_USER_ID).unfinished.push(crate::sharing_state::Unfinished::PurgeOrgCopy {
                org_cipher_id: "s1".into(),
                personal_id: "p-new".into(),
            });
        }).expect("seeded");
        path
    }

    /// **An org copy on its way home is not shared again**: the next pass
    /// unfiles everyone and deletes it, so a person added now would be told
    /// "shared" and then lose it. Refused before the record is even read.
    #[test]
    fn an_org_copy_on_its_way_home_is_not_shared_again() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let path = pending_purge(&dir);
        let read = server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &[]).to_string()).expect(0).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        match engine.share("s1", "bob@example.com") {
            Err(SharingError::Refused(said)) => assert_eq!(said, COMING_HOME),
            other => panic!("expected the coming-home refusal, got {other:?}"),
        }
        read.assert();
        writes.assert();
        puts.assert();
        deletes.assert();
        assert_eq!(crate::sharing_state::SharingState::load(&path).account(FIXTURE_USER_ID).unfinished.len(), 1);
    }

    /// **A refused stop on a pending move names only someone it is true of.**
    /// Bob was never on the record: the neutral refusal. Anna's collection
    /// still holds the org copy: her name, as before.
    #[test]
    fn a_refused_stop_on_a_pending_move_names_only_someone_who_can_still_see_it() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        pending_purge(&dir);
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com"), person_collection("c-bob", "bob@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        server.mock("PUT", "/api/ciphers/s1/collections").with_status(500).create();
        server.mock("DELETE", "/api/ciphers/s1/delete").with_status(500).create();
        let creates = server.mock("POST", Matcher::Any).expect(0).create();
        for (email, sentence) in [
            ("bob@example.com", STOP_FAILED_PREFIX.to_string()),
            ("carol@example.com", STOP_FAILED_PREFIX.to_string()),
            ("anna@example.com", format!("{STOP_FAILED_PREFIX} anna@example.com can still see it.")),
        ] {
            match engine.stop("s1", email) {
                Err(SharingError::Refused(said)) => assert_eq!(said, sentence, "{email}"),
                other => panic!("{email}: expected a refusal, got {other:?}"),
            }
        }
        creates.assert();
    }

    /// **A pending move unfiled but not deleted is a stop that worked**
    /// ([`Sharing::finish_pending`]'s middle branch): the person has lost it,
    /// the org copy is announced filed nowhere beside the personal copy, and
    /// the delete stays pending. No second copy.
    #[test]
    fn a_pending_move_unfiled_but_not_deleted_is_a_stop_that_worked() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let path = pending_purge(&dir);
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        let unfiled = server
            .mock("PUT", "/api/ciphers/s1/collections")
            .match_body(Matcher::Json(serde_json::json!({ "collectionIds": [] })))
            .with_body(shared_cipher("s1", "Acme VPN", &[]).to_string())
            .expect(1)
            .create();
        let delete_fails = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(500).expect(1).create();
        server.mock("GET", "/api/ciphers/p-new").with_body(personal_cipher("p-new", "Acme VPN").to_string()).create();
        let creates = server.mock("POST", Matcher::Any).expect(0).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("the person has lost it");
        unfiled.assert();
        delete_fails.assert();
        creates.assert();
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        assert!(outcome.changed.removed.is_empty(), "the org copy is still there");
        let org_copy = outcome.changed.upserted.iter().find(|i| i.id == "s1").expect("the unfiled org copy is announced");
        assert!(collection_ids_of(org_copy).is_empty());
        assert!(outcome.changed.upserted.iter().any(|i| i.id == "p-new"), "the personal copy is announced");
        assert_eq!(crate::sharing_state::SharingState::load(&path).account(FIXTURE_USER_ID).unfinished.len(), 1, "the delete stays pending");
    }

    /// **A move remembered for a record that is still personal is stale** --
    /// a share that failed after writing it (Task 9's leftover). A stop for
    /// that record changes nothing on the server, and the stale entry goes.
    #[test]
    fn a_personal_record_is_not_moved_back_and_its_stale_move_is_dropped() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let path = dir.join("sharing_state.json");
        crate::sharing_state::update(&path, |state| {
            state.account_mut(FIXTURE_USER_ID).moves.insert("p1".into(), crate::sharing_state::Move { folder_id: Some("f1".into()), favorite: true });
        }).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        let outcome = engine.stop("p1", "anna@example.com").expect("nothing to stop");
        writes.assert();
        puts.assert();
        deletes.assert();
        assert_eq!(outcome.moved_to, None);
        assert!(outcome.changed.is_empty());
        assert!(crate::sharing_state::SharingState::load(&path).account(FIXTURE_USER_ID).moves.is_empty(), "the stale move stayed");
    }

    /// **Only a 404 is "already gone"**: an org copy the server no longer has
    /// finishes its move, and any other refusal (a 403 here) keeps it pending
    /// -- dropped, the copy would sit in the org, filed nowhere, for good.
    #[test]
    fn only_an_org_copy_already_gone_finishes_a_move_without_its_delete() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let path = dir.join("sharing_state.json");
        crate::sharing_state::update(&path, |state| {
            let me = state.account_mut(FIXTURE_USER_ID);
            for (org, personal) in [("s1", "p1"), ("s2", "p2")] {
                me.unfinished.push(crate::sharing_state::Unfinished::PurgeOrgCopy { org_cipher_id: org.into(), personal_id: personal.into() });
            }
        }).expect("seeded");
        let gone = server.mock("DELETE", "/api/ciphers/s1/delete").with_status(404).expect(1).create();
        let forbidden = server.mock("DELETE", "/api/ciphers/s2/delete").with_status(403).expect(1).create();
        let pass = engine.refresh(Trigger::Batch).expect("a pass");
        gone.assert();
        forbidden.assert();
        assert_eq!(pass.outcome.removed, ["s1"]);
        assert!(pass.changed);
        let left = crate::sharing_state::SharingState::load(&path).account(FIXTURE_USER_ID).unfinished;
        assert_eq!(left, [crate::sharing_state::Unfinished::PurgeOrgCopy { org_cipher_id: "s2".into(), personal_id: "p2".into() }]);
    }

    /// **Through the daemon, a move back is `itemGone(old)` then `item(new)`
    /// in one feed batch**, then `sharing` (spec 11.3), and the answer names
    /// the new id.
    #[test]
    fn a_move_back_through_the_daemon_is_announced_gone_then_new_in_one_batch() {
        let (mut server, cache, api, token) = crate::vault_api::tests::a_daemon();
        let engine = cache.bridge().sharing().expect("an engine");
        engine.set_up(SharingSetup { make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([sharing_org_row()])).to_string()).create();
        server.mock("GET", "/api/organizations/invitations/mine").with_body(r#"{"data":[]}"#).create();
        server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-anna"]).to_string()).create();
        anna_is_the_last(&mut server, "s1");
        server.mock("POST", "/api/ciphers").with_body_from_request(echo("p-new", None, &[])).expect(1).create();
        let answer = crate::vault_api::tests::ask(&api, "POST", "/sharing/item/s1/stop", Some(&token), r#"{"email":"anna@example.com"}"#);
        assert_eq!(answer.status, 200, "{}", answer.body);
        let body: serde_json::Value = serde_json::from_str(&answer.body).expect("json");
        assert_eq!(body["data"]["movedTo"], "p-new");
        let changes = api.feed().wait_after(Some(0), false, std::time::Duration::ZERO).changes;
        use crate::change_feed::Change;
        use crate::rest::notifications::Announcement;
        assert!(
            matches!(
                changes.as_slice(),
                [
                    Change::Vault(Announcement::ItemGone { id: gone, .. }),
                    Change::Vault(Announcement::Item { id: new, .. }),
                    Change::Sharing,
                ] if gone == "s1" && new == "p-new"
            ),
            "{changes:?}"
        );
        assert!(cache.get_by_id("p-new").is_some(), "the personal copy is in the daemon's vault");
    }
}

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
//! [`Sharing::note_directory`]), the [`Capabilities`] it has learned, the
//! sharers this account accepted and is waiting on, and two sets the confirm
//! pass learns (keys that changed, accounts with no key yet). Every WRITE reads current server
//! state first and does only the missing steps (spec 13), so a Retry is the
//! same call and two PCs converge.
//!
//! # One path, on every server
//!
//! `docs/superpowers/specs/2026-10-08-official-path-sharing-design.md`
//! amends the design: NodeWarden, Vaultwarden and official Bitwarden are all
//! shared on through Bitwarden's own routes and bodies, and nothing asks
//! which server this is. What differs between them is learned as
//! [`Capabilities`], each from an official route sharing needs anyway: a
//! route answering as missing turns sharing off (`server-too-old`), and a
//! roster read after an invite says whether the server accepted at once.
//! There is no invitation list -- no official server lets an invitee list
//! one -- and an invitation is accepted from its pasted link
//! ([`Sharing::accept_link`]) or in the web vault.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use serde::{Deserialize, Serialize};

use crate::rest::api::{Authenticated, RestClient, RestError};
use crate::rest::crypto::{CryptoError, EncString, SymmetricKey};
use crate::rest::organizations::{
    Directory, Member, MemberStatus, ReceivedRosters, Roster, collection_ids_of, normalise_email, organization_of, sharer_of,
};
use crate::rest::sharing_api;
use crate::rest::sync::{Profile, VaultKeys};
use crate::vault_bridge::{VaultError, VaultItem};

// ---- what the daemon answers `GET /sharing` with --------------------------------

/// Why sharing is not available (spec 11.3). `server-too-old` is a route
/// sharing needs answering as missing (official-path spec §2). There is no
/// "not NodeWarden" any more: every server shares on the one path.
pub const REASON_BW_SERVE: &str = "bw-serve";
pub const REASON_TOO_OLD: &str = "server-too-old";
pub const REASON_NOT_LOADED: &str = "not-loaded";

/// Spec 6.1's per-person states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PersonState {
    /// Confirmed, with access to their collection: in their vault now.
    Shared,
    /// Accepted, confirm not run yet.
    WaitingAccept,
    /// A pending invitation (official status 0). No server tells a client
    /// whether the address has an account, so nothing here claims either,
    /// and no expiry is claimed (official-path spec §3.4).
    Invited,
    /// Accepted, but the account has no public key yet.
    WaitingSignIn,
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
    /// NodeWarden's invitation expiry, ISO 8601, when the server names one.
    /// Not drawn: no expiry is claimed (official-path spec §3.4), because
    /// official servers do not show it. The ten-minute timer still reads it.
    pub expires_at: Option<String>,
    /// **Their collection's name decrypted under the org key** -- rule 2
    /// (see [`PersonCollection`]). `false` for a person known only from a
    /// plaintext name, which the server can write: the card shows them,
    /// marked unverified, and offers only Stop (a removal); Re-invite and
    /// Share anyway are grants and are not offered (final review, I1).
    ///
    /// Shown rather than hidden because the record really is filed in that
    /// collection -- the server says so -- and hiding the row would hide
    /// who may see it, along with the one control that takes it back.
    /// `serde(default)` reads an answer without the field as unverified,
    /// the safe side.
    #[serde(default)]
    pub authenticated: bool,
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
    /// The sharing org the record is in -- what Shared with me groups by,
    /// so two orgs that claim one sharer are never merged (official-path
    /// spec §4.3).
    #[serde(default)]
    pub organization_id: String,
    /// The sharer is the server's word, not only the org name's claim
    /// ([`crate::rest::organizations::SharingOrg::verified`]).
    /// `serde(default)` reads an answer without it as unverified.
    #[serde(default)]
    pub verified: bool,
}

/// A sharing org this account accepted and its sharer has not confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitingEntry {
    pub organization_id: String,
    pub from: String,
    /// As [`ReceivedEntry::verified`].
    #[serde(default)]
    pub verified: bool,
}

/// **Everything the window draws about sharing**, answered by `GET /sharing`
/// out of the engine's held state and the daemon's items -- no server round
/// trip (spec 11.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SharingView {
    pub available: bool,
    pub reason: Option<String>,
    /// People a record is shared with can list each other (official
    /// Bitwarden's `mini-details` is open to every member). Decided by
    /// `/config`'s server name (official-path spec §3.4): false only for
    /// Vaultwarden and NodeWarden, which keep the roster to managers; an
    /// unnamed server is warned about, the direction that discloses.
    pub roster_visible_to_recipients: bool,
    /// The account is NodeWarden's administrator and may mint invite codes.
    pub can_mint_invites: bool,
    pub people: Vec<PersonEntry>,
    pub records: Vec<RecordEntry>,
    pub received: Vec<ReceivedEntry>,
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

    /// Whether a received record's sharer is the server's word -- `false`
    /// for the org name's claim alone, and for a record not received at all.
    #[must_use]
    pub fn received_verified(&self, item_id: &str) -> bool {
        self.received.iter().any(|r| r.item_id == item_id && r.verified)
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
/// More than one organisation passes every test for this account's own
/// sharing org, and Deskwarden made none of them on this PC (plan fix round
/// 1, I1). Picking one would be a guess about which the user meant -- and a
/// planted one among them would win half the time -- so nothing is chosen.
///
/// **It says what is true and advises nothing** (fix round 2, m4). The
/// earlier "Delete or rename the extra ones" could have the user delete the
/// org their own shares live in. Offering a choice between them was weighed
/// and not built: every candidate carries the same "Shared by <you>" name, so
/// the only thing a picker could show to tell them apart is an id, and the
/// user's pick between a real org and a planted twin would be the same guess
/// this sentence refuses to make.
pub const SEVERAL_ORGS: &str = "You own more than one organisation named for you on this server, and Deskwarden can't tell \
     which of them is yours on this PC, so it hasn't shared anything.";
/// The sharing org's roster answered 404 and the profile no longer lists it
/// as this account's to own: deleted, or this account was moved out of the
/// owner role. Its id is dropped, so the next share makes a new one.
pub const ORG_GONE: &str = "Your sharing organisation is no longer on this server. Share again to start a new one.";
/// The user said No to an organisation Deskwarden did not create, and the
/// server would not let a new one be made.
pub const NO_FRESH_ORG: &str = "This server won't let Deskwarden create a sharing organisation, so it can't share \
     without the one you didn't choose.";
/// Yes to an organisation that no longer passes the roster rule, or is no
/// longer listed, by the time the answer came.
pub const ADOPT_REFUSED: &str = "Deskwarden can't use that organisation for sharing any more. Try sharing again.";
/// An adopt with no question pending, or for an org other than the one the
/// last share asked about (fix round 2, m2): only the user's answer to the
/// question Deskwarden asked can pin an org it did not create.
pub const ADOPT_NOT_ASKED: &str = "Deskwarden didn't ask about that organisation. Share again to choose.";
/// Yes was said, but the choice could not be saved, so it would be asked
/// again: said rather than pretended.
pub const ADOPT_NOT_SAVED: &str = "Deskwarden couldn't save that choice. Try again.";
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
/// and never opened since in an app that makes them. Sharing is on for it --
/// nothing about the server is wrong, and nothing asks the server about its
/// own key ([`Sharing::ensure_loaded`]) -- so the refusal is
/// here, and says what is actually missing rather than blaming the server's
/// age or the organisation. Bitwarden's web vault makes the pair at sign-in.
pub const NO_KEY_PAIR: &str =
    "This account has no encryption keys for sharing yet. Sign in to the web vault once to create them, then try again.";
/// An invite code asked for by an account that is not NodeWarden's
/// administrator -- refused here before any write, and the sentence the
/// route's own 403 is read as too.
pub const NOT_ADMIN: &str = "Only this server's administrator can make an invite code.";
/// An invite code the server would not make, for a reason with no sentence
/// of its own; the server's words follow it (`SharingError::or_say`).
pub const INVITE_CODE_FAILED: &str = "Couldn't make an invite code.";
/// The master password typed for an invite code was not this account's --
/// the route's "Invalid password".
pub const WRONG_PASSWORD: &str = "That master password isn't right.";

/// A Re-invite of someone whose collection name Deskwarden did not write --
/// see [`Sharing::reinvite`]. Says what to do instead: a share from the
/// dialog starts from an address the user typed.
#[must_use]
pub fn not_vouched_for(email: &str) -> String {
    format!(
        "Deskwarden didn't set up sharing with {email}, so it won't re-invite them. \
         Share a record with them from the Share dialog instead."
    )
}

/// The Share dialog's sentence when the server will not create the sharing
/// org (official-path spec §3.1) -- an official self-hosted server, where an
/// organisation comes only from a license. The user makes it in the web
/// vault under this name, and the next share finds it by name and ownership
/// and asks before using it ([`SharingError::NeedsAdoption`]): Deskwarden
/// did not create it.
#[must_use]
pub fn needs_org(email: &str) -> String {
    format!(
        "Your server needs an organisation for sharing. Create one named \"{}\" in the web vault, then try again.",
        crate::rest::organizations::sharing_org_name(email)
    )
}

/// What a share ended as: the words the dialog chooses between (mock 6a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareResult {
    /// In their vault now.
    Shared,
    /// A member without the key yet: the confirm pass will finish it.
    WaitingAccept,
    /// The server left a pending invitation: the person accepts, then the
    /// confirm pass finishes it.
    Invited,
}

/// **Show fingerprint's answer**: the five words, and the SHA-256 of the key
/// they were made from. Neither is secret -- both are of a PUBLIC key -- and
/// the hash is what binds "Share anyway" to the key the user compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub phrase: String,
    /// Lowercase hex, `crypto::public_key_sha256_hex`'s -- the pin's own form.
    pub key_sha256: String,
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

/// What forgetting a person did -- see [`Sharing::forget`].
///
/// **No `Clone`**, unlike its siblings: `failed` holds a [`SharingError`],
/// which has none (its [`VaultError`] has none).
#[derive(Debug, Default)]
pub struct ForgetOutcome {
    /// The records (old ids) that stopped being shared with them.
    pub stopped: Vec<String>,
    /// What those stops changed on the server, for the daemon's copy --
    /// **also when `failed` is set**: a stop that landed before a later step
    /// failed is still a change the daemon must file and announce.
    pub changed: SharingOutcome,
    /// Why the forget did not finish, when it did not: the sentence to show
    /// (or the session's own error). The person is NOT forgotten then --
    /// their membership, collection and pin are whatever the steps before
    /// the failure left -- and a Retry is the same call.
    pub failed: Option<SharingError>,
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
    /// **A share stopped to ask the user** (official-path spec §3.1, owner's
    /// fix round 1): one organisation on the server passes every roster test
    /// for this account's own sharing org, but this client did not create
    /// it and the user has not said to use it. Nothing was written. The
    /// window asks [`AdoptionQuestion::sentence`]; Yes is [`Sharing::adopt`]
    /// and the share again, No is [`Sharing::share_in_a_new_org`].
    NeedsAdoption(AdoptionQuestion),
}

/// **The organisation a share asks the user about** -- its id, and its name
/// as the server lists it. Plaintext on every server, and anyone who made the
/// org chose it, so it is shown in quotes beside a short id rather than
/// trusted as a description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionQuestion {
    pub organization_id: String,
    pub name: String,
}

impl AdoptionQuestion {
    /// The owner's wording: `Use the organisation "<name>" (<short id>) for
    /// sharing? Deskwarden did not create it.` The short id is the first
    /// eight characters, which tell two orgs of one name apart in the web
    /// vault's address bar.
    #[must_use]
    pub fn sentence(&self) -> String {
        let short: String = self.organization_id.chars().take(8).collect();
        format!("Use the organisation \u{201c}{}\u{201d} ({short}) for sharing? Deskwarden did not create it.", self.name)
    }
}

impl std::fmt::Display for SharingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "sharing is not available here ({reason})"),
            Self::Refused(sentence) => f.write_str(sentence),
            Self::Vault(e) => write!(f, "{e:?}"),
            Self::NeedsAdoption(_) => f.write_str("the share needs the user to choose an organisation"),
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
            Self::Vault(VaultError::Unauthorized) | Self::Unavailable(_) | Self::NeedsAdoption(_) => self,
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
    /// Where Show fingerprint looks for the EFF long list, in order --
    /// read, never written. Production: beside the executable
    /// (`eff_wordlist::eff_wordlist_paths`). A test names the repository's
    /// own `assets/` copy rather than trusting `build.rs`'s best-effort copy
    /// beside the test binary.
    pub eff_wordlist: Vec<PathBuf>,
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
            eff_wordlist: crate::rest::eff_wordlist::eff_wordlist_paths(),
        }
    }

    /// No files at all: nothing is persisted, and no word list is looked
    /// for. What a test starts from, and it fills in scratch paths where the
    /// test is about a file.
    #[must_use]
    pub fn none() -> Self {
        Self {
            state_path: None,
            sent_path: None,
            fill_stats_path: None,
            make_key_pair: crate::rest::crypto::generate_org_key_pair,
            eff_wordlist: Vec::new(),
        }
    }
}

/// **What this server does, as sharing has learned it this session**
/// (official-path spec §2) -- in place of a dialect probe. Each fact comes
/// from an official route sharing needs anyway; none is asked for its own
/// sake, and none names a server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Capabilities {
    /// The first pass has read the account: the view may say "available".
    pub(crate) loaded: bool,
    /// A route sharing needs answered as missing -- 405 or 501 from one of
    /// them ([`Sharing::route`]), or 404 from the sharer's own org roster
    /// ([`Sharing::fetch_roster`]). Sharing is off with [`REASON_TOO_OLD`]
    /// until the next sign-in.
    pub(crate) too_old: bool,
    /// What received orgs' `GET …/users/mini-details` have said this session:
    /// whether a member may read them here, and each answered org's one
    /// owner -- so "Shared by" is verified, and each org asked once
    /// (`organizations::Directory::attest_received`).
    pub(crate) received_rosters: ReceivedRosters,
    /// The roster read after the last invite showed the person Accepted
    /// (`Some(true)`, Vaultwarden and NodeWarden with mail off) or Invited
    /// (`Some(false)`). A share then goes straight to confirm, or waits.
    pub(crate) accepts_on_invite: Option<bool>,
    /// `POST /organizations` was refused (`Some(false)`, official
    /// self-hosted): the user creates the sharing org in the web vault.
    pub(crate) creates_orgs: Option<bool>,
    /// `GET /config`'s `server.name`, read once. Decides the
    /// roster-visibility warning.
    pub(crate) server_name: Option<String>,
    /// `/config` has answered, so [`Self::server_name`] is the server's own
    /// word -- a name, or none. Kept apart from [`Self::loaded`] (DW2 fix
    /// round 1): a `/config` that was unreachable for one pass must not
    /// leave the server unnamed for the whole session, so it is asked again
    /// on every later pass until it answers. Unknown warns, as unnamed does.
    pub(crate) name_known: bool,
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
    /// the server holds its public half. Every owner's write asks this before
    /// its first key is touched -- see [`NO_KEY_PAIR`].
    pub(crate) keyed: bool,
}

/// One person collection C(E): its id, the email its name holds, and
/// whether that name decrypted under the org key.
///
/// **`authenticated` is spec 11.5's rule 2.** Only a holder of the org key
/// can write a name that decrypts, so only such a name may put a person in
/// the confirm queue or let "Share anyway" hand them the org key. A plaintext
/// name -- NodeWarden's web vault may leave one, and the server can write one
/// -- is drawn on the card like any other and used to find a collection to
/// file into, stop or delete, but never to confirm. See
/// [`crate::rest::organizations::Collection::authenticated`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersonCollection {
    pub(crate) id: String,
    /// Normalised.
    pub(crate) email: String,
    pub(crate) authenticated: bool,
}

/// The collection for `email` in `collections`, authenticated or not as
/// `authenticated_only` asks -- [`MyOrg`]'s and [`Ctx`]'s one lookup.
///
/// **An authenticated match wins wherever it stands in server order.** Asked
/// for any match, the first one used to come back, so a plaintext name the
/// server listed ahead of the engine's own would be the collection a stop
/// looked in and the view read the person's state off (final review, I1).
/// The first plaintext match is the answer only when no authenticated one
/// exists.
fn person_collection_for<'a>(collections: &'a [PersonCollection], email: &str, authenticated_only: bool) -> Option<&'a str> {
    let email = normalise_email(email);
    let authentic = collections.iter().find(|c| c.email == email && c.authenticated);
    let any = || collections.iter().find(|c| c.email == email).filter(|_| !authenticated_only);
    authentic.or_else(any).map(|c| c.id.as_str())
}

/// **Every collection named for `email`**, authenticated first -- what a
/// REMOVAL reads (a stop, a forget), never a grant. A server can list a
/// plaintext twin beside the engine's own collection, and a record filed in
/// either is one the person may see; taking them off one and not the other
/// would report a stop that did not happen. Grants read
/// [`person_collection_for`] with `authenticated_only` instead.
fn person_collections_for<'a>(collections: &'a [PersonCollection], email: &str) -> Vec<&'a str> {
    let email = normalise_email(email);
    let named = || collections.iter().filter(|c| c.email == email);
    named().filter(|c| c.authenticated).chain(named().filter(|c| !c.authenticated)).map(|c| c.id.as_str()).collect()
}

/// This account's own sharing org, as the last read left it.
#[derive(Debug, Clone)]
pub(crate) struct MyOrg {
    pub(crate) id: String,
    /// In server order.
    pub(crate) collections: Vec<PersonCollection>,
    pub(crate) roster: Vec<Member>,
}

impl MyOrg {
    /// Rule 2's lookup: a collection whose name decrypted under the org key.
    fn authenticated_collection_for(&self, email: &str) -> Option<&str> {
        person_collection_for(&self.collections, email, true)
    }

    fn email_of(&self, collection_id: &str) -> Option<&str> {
        self.collections.iter().find(|c| c.id == collection_id).map(|c| c.email.as_str())
    }

    fn member(&self, email: &str) -> Option<&Member> {
        let email = normalise_email(email);
        self.roster.iter().find(|m| normalise_email(&m.email) == email)
    }
}

/// What the engine holds between passes. See the module docs.
///
/// **A hand-written `Debug`.** It was `crate::debug_leak_guard`'s barrier
/// while `Held` kept invitation tokens; it holds no secret now (there is no
/// invitation list), and the impl stays so that a field added later is
/// printed on purpose or not at all.
#[derive(Clone, Default)]
pub(crate) struct Held {
    pub(crate) capabilities: Capabilities,
    pub(crate) account: Option<Account>,
    pub(crate) my_org: Option<MyOrg>,
    /// Other people's sharing orgs: `(org id, sharer)`.
    pub(crate) others: Vec<(String, String)>,
    /// Accepted, unconfirmed: `(org id, sharer)`.
    pub(crate) waiting: Vec<(String, String)>,
    /// The ids of `others` and `waiting` whose sharer is only the org name's
    /// claim (official-path spec §4.3).
    pub(crate) unverified: BTreeSet<String>,
    /// Emails whose key differs from the pin.
    pub(crate) key_changed: BTreeSet<String>,
    /// Emails whose account has no public key yet.
    pub(crate) no_key: BTreeSet<String>,
    /// **The org the last share asked about** ([`SharingError::NeedsAdoption`])
    /// and the user has not answered yet (fix round 2, m2). [`Sharing::adopt`]
    /// takes only this id, once; No ([`Sharing::share_in_a_new_org`]) spends
    /// it too. So nothing but the user's own answer to the question
    /// Deskwarden asked -- not a caller of `POST /sharing/adopt` naming any
    /// org it likes -- can pin an org Deskwarden did not create.
    pub(crate) asked_adoption: Option<String>,
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Held")
            .field("capabilities", &self.capabilities)
            .field("account", &self.account)
            .field("my_org", &self.my_org)
            .field("others", &self.others)
            .field("waiting", &self.waiting)
            .field("unverified", &self.unverified)
            .field("key_changed", &self.key_changed)
            .field("no_key", &self.no_key)
            .field("asked_adoption", &self.asked_adoption)
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

    /// [`Self::call`], for a route sharing needs (official-path spec §2): a
    /// 405 or 501 is the server saying it has no such route, so sharing is
    /// off for the session with [`REASON_TOO_OLD`] and the answer is
    /// [`SharingError::Unavailable`]. Only the routes that are sharing's
    /// learn it this way -- a `/config` or a cipher read answering 405 says
    /// nothing about sharing, and goes through [`Self::call`].
    ///
    /// A 404 is not read here: from `/share` or `/confirm` it is a record or
    /// a person that has gone. The one 404 that means "no route" is the
    /// sharer's own roster's, which [`Self::fetch_roster`] reads.
    pub(crate) fn route<T>(
        &self,
        request: impl FnOnce(&RestClient, &mut Authenticated) -> Result<T, RestError>,
    ) -> Result<T, SharingError> {
        let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
        let answer = {
            let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            request(&self.client, &mut guard)
        };
        match answer {
            Ok(value) => Ok(value),
            Err(RestError::Status(405 | 501)) => {
                self.held().capabilities.too_old = true;
                Err(SharingError::Unavailable(REASON_TOO_OLD))
            }
            Err(RestError::Rejected { description, .. }) if !description.is_empty() => Err(SharingError::Refused(description)),
            Err(other) => Err(SharingError::Vault(crate::rest::backend::rest_error(other))),
        }
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
        // **The writes' own rule** ([`Self::my_org_id`]), so the confirm pass
        // never works on an org the writes would refuse: only an org this
        // client made or the user adopted is held, whatever its roster says
        // -- an org that merely passes the roster rule is not ours until the
        // user says so (fix round 1, C1). No roster is asked, so one this read
        // could not fetch changes nothing; and this is a READ, so it forgets
        // no cached id and writes no state (I2). No profile: nothing is held.
        let account = self.held().account.clone();
        // The id this PC made or the user adopted, whatever the profile now
        // says of it: never received from anyone (DW1 review, M3). Read here,
        // before `held` is locked, as `trusted_org` reads the same file.
        let pinned = account.as_ref().and_then(|account| self.pinned_org(&account.id));
        let chosen = profile.zip(account).and_then(|(profile, account)| self.trusted_org(profile, &account, false));
        let mine = chosen.and_then(|id| directory.sharing_orgs().into_iter().find(|o| o.id == id));
        let mut held = self.held();
        // The previous roster, kept for an org whose roster this read could
        // not fetch: a known roster is better than none for the confirm pass.
        let previous = held.my_org.take();
        held.my_org = mine.map(|org| MyOrg {
            id: org.id.clone(),
            collections: directory
                .collections_of(&org.id)
                .into_iter()
                .filter_map(|c| {
                    c.name.as_deref().map(|name| PersonCollection {
                        id: c.id.clone(),
                        email: normalise_email(name),
                        authenticated: c.authenticated,
                    })
                })
                .collect(),
            roster: match directory.roster_of(&org.id) {
                Roster::Known(members) => members.clone(),
                Roster::Unknown => previous.as_ref().filter(|o| o.id == org.id).map(|o| o.roster.clone()).unwrap_or_default(),
            },
        });
        // **Received: someone else's sharing orgs, and never this account's
        // own.** The directory marks an org this account owns and is named
        // for as its own; the pin covers the one it made or adopted even if
        // the profile now lists it otherwise -- renamed, or this account no
        // longer its owner -- so that org is never "Shared by" anyone
        // (`a_made_or_adopted_org_is_never_received_from_anyone`).
        let received = |o: &&crate::rest::organizations::Organisation| pinned.as_deref() != Some(o.id.as_str());
        held.others = directory
            .sharing_orgs()
            .into_iter()
            .filter(received)
            .filter_map(|o| o.sharing.as_ref().filter(|s| !s.mine).map(|s| (o.id.clone(), s.sharer.clone())))
            .collect();
        held.waiting = directory
            .waiting()
            .iter()
            .filter(received)
            .filter_map(|o| o.sharing.as_ref().map(|s| (o.id.clone(), s.sharer.clone())))
            .collect();
        // **Which of them name a sharer only by the org's own title**
        // (official-path spec §4.3): the view says "unverified" for these.
        held.unverified = directory
            .sharing_orgs()
            .into_iter()
            .chain(directory.waiting().iter())
            .filter(|o| o.sharing.as_ref().is_some_and(|s| !s.mine && !s.verified))
            .map(|o| o.id.clone())
            .collect();
    }

    /// The org id `sharing_state.json` pins as this account's own -- made
    /// here or adopted -- with no word from the profile on it. `None` with no
    /// file, no id, or an id cached by a build before either flag existed.
    fn pinned_org(&self, account_id: &str) -> Option<String> {
        let path = self.setup().state_path?;
        let me = crate::sharing_state::SharingState::load(&path).account(account_id);
        me.sharing_org_id.filter(|_| me.sharing_org_created || me.sharing_org_adopted)
    }

    /// [`Self::pinned_org`] for the account `profile` names -- what
    /// `RestBackend::synced` reads, before the directory is filed, so the
    /// directory itself never draws this account's own org as received
    /// (`organizations::Directory::not_received`, fix round 1, m11). It
    /// reads a file: never call it with the session locked.
    #[must_use]
    pub fn own_org_pin(&self, profile: &Profile) -> Option<String> {
        self.pinned_org(profile.id.as_deref()?)
    }

    /// What this session has learned about received orgs' rosters
    /// ([`Capabilities::received_rosters`]). `RestBackend::synced` takes a
    /// copy before the session lock, hands it to
    /// `organizations::Directory::attest_received`, and gives it back with
    /// [`Self::note_received_rosters`].
    #[must_use]
    pub fn received_rosters(&self) -> ReceivedRosters {
        self.held().capabilities.received_rosters.clone()
    }

    /// What a whole read learned, kept for the session.
    pub fn note_received_rosters(&self, rosters: ReceivedRosters) {
        self.held().capabilities.received_rosters = rosters;
    }

    /// The view -- pure over what is held and the daemon's items.
    ///
    /// Built under the `held` lock rather than from a clone of [`Held`]:
    /// nothing here takes another lock or makes a request, so the guard is
    /// short, and a clone would copy the roster to read it once.
    #[must_use]
    pub fn view(&self, items: &[VaultItem]) -> SharingView {
        let held = self.held();
        if !held.capabilities.loaded {
            return SharingView::unavailable(REASON_NOT_LOADED);
        }
        if held.capabilities.too_old {
            return SharingView::unavailable(REASON_TOO_OLD);
        }
        let hides_roster = held
            .capabilities
            .server_name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("vaultwarden") || name.eq_ignore_ascii_case("nodewarden"));
        let mut view = SharingView {
            available: true,
            roster_visible_to_recipients: !hides_roster,
            can_mint_invites: held.account.as_ref().is_some_and(|a| a.admin),
            ..SharingView::default()
        };
        if let Some(org) = &held.my_org {
            for PersonCollection { email, .. } in &org.collections {
                if view.people.iter().any(|p| &p.email == email) {
                    continue;
                }
                // The authenticated collection when there is one, wherever
                // it stands in server order (`person_collection_for`).
                let collection_id = person_collection_for(&org.collections, email, false).unwrap_or_default();
                let member = org.member(email);
                view.people.push(PersonEntry {
                    email: email.clone(),
                    state: person_state(member, collection_id, &held),
                    expires_at: member.and_then(|m| m.invitation_expires.clone()),
                    authenticated: org.authenticated_collection_for(email).is_some(),
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
                    view.received.push(ReceivedEntry {
                        item_id: item.id.clone(),
                        from: sharer.clone(),
                        organization_id: org.to_string(),
                        verified: !held.unverified.contains(org),
                    });
                }
            }
        }
        view.waiting = held
            .waiting
            .iter()
            .map(|(organization_id, from)| WaitingEntry {
                organization_id: organization_id.clone(),
                from: from.clone(),
                verified: !held.unverified.contains(organization_id),
            })
            .collect();
        view
    }

    /// **The first pass's two questions**, once per engine: who the account
    /// is, and what the server calls itself. Sharing is "not loaded" until
    /// the first has an answer, and is then available on every server --
    /// unless a route it needs answers as missing ([`Self::route`],
    /// [`Self::fetch_roster`]), which turns it off with [`REASON_TOO_OLD`] for
    /// the session.
    ///
    /// This replaces the dialect probe (official-path spec §2). The profile
    /// is usually already held -- the whole read that called
    /// [`Self::note_directory`] filed it -- so the account costs no request.
    /// The name is `/config`'s `server.name`, read for one sentence -- the
    /// roster-visibility warning ([`SharingView::roster_visible_to_recipients`])
    /// -- and nothing else branches on it. A failure to read the account is
    /// not remembered: the next pass asks again.
    ///
    /// **The name is asked until `/config` answers** (fix round 1), apart
    /// from `loaded`: an unreachable server, a 5xx or an ended session
    /// leaves it unknown, which warns, and every later pass asks again. Any
    /// answer -- a name, `null`, or a 4xx -- is the server's word and is
    /// kept: a server with no `/config` route would otherwise be asked on
    /// every write for the rest of the session, for a sentence that cannot
    /// change.
    pub(crate) fn ensure_loaded(&self) -> Result<(), SharingError> {
        if !self.held().capabilities.loaded {
            self.account_id()?;
            self.held().capabilities.loaded = true;
        }
        if self.held().capabilities.name_known {
            return Ok(());
        }
        let name = match self.call(|client, auth| sharing_api::server_name(client, &mut auth.session)) {
            Ok(name) => Some(name),
            Err(SharingError::Vault(VaultError::Unreachable(_) | VaultError::Unauthorized)) => {
                log::info!("the server's /config could not be reached for its name; the next pass asks again");
                None
            }
            Err(e) => {
                log::info!("the server's /config gave no name: {e}");
                Some(None)
            }
        };
        if let Some(name) = name {
            let mut held = self.held();
            held.capabilities.server_name = name;
            held.capabilities.name_known = true;
        }
        Ok(())
    }

    /// Refuses unless sharing is loaded and no route it needs has answered
    /// as missing.
    pub(crate) fn require_available(&self) -> Result<(), SharingError> {
        self.ensure_loaded()?;
        if self.held().capabilities.too_old {
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

    /// **One pass** (spec 11.5): the account, then every move back whose
    /// delete failed ([`Self::finish_unfinished`]) -- on every trigger, since
    /// each is one file read when nothing is pending -- and last the confirm
    /// pass ([`Self::confirm_pass`]), whose failure is logged: the finished
    /// moves above must still reach the daemon.
    ///
    /// There is no invitation list to read (official-path spec §4.1): no
    /// official server lets an invitee list their invitations, and
    /// NodeWarden's `/invitations/mine` is not called. An invitation is
    /// accepted from its link ([`Self::accept_link`]) or in the web vault.
    ///
    /// # Errors
    ///
    /// The account could not be read; a later pass tries again.
    pub fn refresh(&self, trigger: Trigger) -> Result<SharingPass, SharingError> {
        let _one = self.one_at_a_time();
        let was_loaded = self.held().capabilities.loaded;
        self.ensure_loaded()?;
        let mut pass = SharingPass { changed: !was_loaded, ..SharingPass::default() };
        if self.held().capabilities.too_old {
            return Ok(pass);
        }
        pass.outcome = self.finish_unfinished();
        pass.changed |= !pass.outcome.is_empty();
        match self.confirm_pass(trigger) {
            Ok(confirmed) => pass.changed |= confirmed,
            // A confirm that could not be made this time is tried again on the
            // next trigger; the pass itself did what it could, and its
            // finished moves above must still reach the daemon.
            Err(e) => log::info!("the confirm pass stopped early: {e}"),
        }
        Ok(pass)
    }

    /// Whether the ten-minute timer has anything to do: someone in this
    /// account's org is invited or accepted and not yet confirmed.
    ///
    /// **An invitation that has expired does not count** (final review): it
    /// can no longer be accepted, so nothing the timer reads can move it --
    /// only a Re-invite, which the user makes. Counted, one stale invitation
    /// kept the timer reading the roster every ten minutes for good.
    #[must_use]
    pub fn wants_timer(&self) -> bool {
        let held = self.held();
        let now = now_millis();
        held.my_org.as_ref().is_some_and(|org| {
            // Revoked waits for an admin, not for anything the timer reads.
            org.roster.iter().any(|m| {
                !matches!(m.status, MemberStatus::Confirmed | MemberStatus::Revoked)
                    && !invitation_ran_out(m, now)
                    && org.authenticated_collection_for(&m.email).is_some()
            })
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
    pub(crate) collections: Vec<PersonCollection>,
    /// The org's roster, read at the start of the write (spec 13) and again
    /// after an invite, which is how the write learns the new person's
    /// org-user id and status (the invite answers nothing).
    pub(crate) roster: Vec<Member>,
    /// **This account's own org-user id** in that roster ([`own_row`]): what
    /// a person collection names as its manager. On NodeWarden it is the
    /// user id; on an official server and on Vaultwarden it is not, which is
    /// why it is read and never assumed. `None` when the roster does not list
    /// the account -- such a write cannot create a collection.
    pub(crate) own_ou_id: Option<String>,
}

impl Ctx {
    /// One person's row in [`Self::roster`], by email in any case.
    pub(crate) fn member(&self, email: &str) -> Option<&Member> {
        let email = normalise_email(email);
        self.roster.iter().find(|m| normalise_email(&m.email) == email)
    }

    /// Every collection for `email`, authenticated first -- see
    /// [`person_collections_for`]. **Not for a grant**: nothing that creates
    /// a membership, wraps the org key or files a record for someone reads
    /// this (final review, I1). The old any-match `collection_for` went with
    /// that rule: its last grant reader was `ensure_collection`.
    pub(crate) fn collections_for(&self, email: &str) -> Vec<&str> {
        person_collections_for(&self.collections, email)
    }

    /// Rule 2's lookup -- see [`PersonCollection`].
    pub(crate) fn authenticated_collection_for(&self, email: &str) -> Option<&str> {
        person_collection_for(&self.collections, email, true)
    }
}

/// How one try at filing a record failed: a stale revision, which is
/// re-read and retried, or anything else, already in the dialog's words.
enum Filing {
    Stale(SharingError),
    Failed(SharingError),
}

/// A `sharing_state` read or write failure as the refusal to show: the
/// file's own plain sentence when it could not be read (see
/// [`crate::sharing_state::Unreadable`]), a generic one otherwise.
fn state_refused(e: &std::io::Error) -> SharingError {
    SharingError::Refused(
        crate::sharing_state::unreadable(e)
            .map_or(crate::sharing_state::UNREADABLE_NOW, crate::sharing_state::Unreadable::sentence)
            .to_string(),
    )
}

/// A crypto failure as a sharing error, mapped as the backend maps it --
/// except a person's key too short to wrap to, which is a refusal the user
/// can read (final review, item 6): it is a fact about THEIR key, not this
/// vault's cryptography failing.
pub(crate) fn crypto(e: CryptoError) -> SharingError {
    match e {
        CryptoError::RsaKeyTooShort { bits } => SharingError::Refused(key_too_short(bits)),
        e => SharingError::Vault(crate::rest::backend::crypto_error(e)),
    }
}

/// Their account's key is too short to hand the org key to.
#[must_use]
pub fn key_too_short(bits: usize) -> String {
    format!(
        "Their encryption key is only {bits} bits, too short to share with safely. \
         Deskwarden needs at least {}.",
        crate::rest::crypto::MIN_RSA_BITS
    )
}

/// Plausibly an email: one `@` with something either side, no whitespace.
fn plausible_email(email: &str) -> bool {
    let mut halves = email.split('@');
    matches!((halves.next(), halves.next(), halves.next()), (Some(l), Some(d), None) if !l.is_empty() && d.contains('.'))
        && !email.chars().any(char::is_whitespace)
}

/// A collection name read with the org key; a NodeWarden web-vault name
/// that is plaintext is read as it is. The `bool` is whether the name was
/// authenticated -- decrypted under the org key -- and is `true` in that
/// branch alone ([`PersonCollection::authenticated`]).
fn collection_email(raw: &str, key: &SymmetricKey) -> Option<(String, bool)> {
    match raw.parse::<EncString>() {
        Ok(enc) => {
            let bytes = crate::rest::crypto::decrypt(key, &enc).ok()?;
            std::str::from_utf8(&bytes).ok().map(|name| (normalise_email(name), true))
        }
        Err(_) => Some((normalise_email(raw), false)),
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

/// **Owned by this account and named for it** -- what makes an org a
/// CANDIDATE for this account's own sharing org (spec 5.1). Not enough on its
/// own: another account can create "Shared by <me>" and make this account an
/// owner of it. [`roster_says_mine`] and the user's own Yes decide.
fn owned_and_named(org: &crate::rest::sync::Organization, account_email: &str) -> bool {
    crate::rest::organizations::sharing_mark(org.name.as_deref().and_then(sharer_of), org.is_owner(), account_email)
        .is_some_and(|mark| mark.mine)
}

/// **The roster's word that a candidate org could be this account's own**
/// (official-path spec §3.1, owner's fix round 1, C1). All three must hold:
///
/// 1. This account's own row is an OWNER, and CONFIRMED -- status 2, not
///    official Bitwarden's revoked -1 (`MemberStatus::Revoked`, fix round 2).
/// 2. Every other row is a plain `member` -- in any status. An owner, admin,
///    manager or custom row reads every collection, so an attacker who made
///    "Shared by <me>", made this account an owner and then stepped down to
///    any of them could still read every share. An INVITED or revoked row in
///    such a role counts: accepted or restored later, it reads the same.
/// 3. Every collection a member is listed on is the ONE authenticated
///    collection named for that member's own email, held read-only and NOT
///    managed (a manager edits who reaches it; fix round 2, m3). A member
///    seat on anybody else's collection -- the attacker's other way to keep
///    reading -- fails, as does one on a plaintext-named collection, which the
///    server can write.
///
/// **This is not the whole answer, and cannot be.** Bitwarden and Vaultwarden
/// also grant collections through GROUPS, which `/users?includeCollections`
/// does not list per member: an attacker's group on a person collection is
/// invisible here. So passing makes an org only a candidate; it is used after
/// the user says Yes to "Use the organisation ... ? Deskwarden did not create
/// it." ([`Sharing::adopt`]), and that question is the backstop.
///
/// It replaces NodeWarden's `ownerEmail` attestation, which no other server
/// sends, and DW1's "no other owner", which an attacker satisfied by stepping
/// down.
fn roster_says_mine(roster: &[Member], collections: &[PersonCollection], account_id: &str) -> bool {
    let is_me = |m: &Member| m.user_id.as_deref() == Some(account_id);
    let confirmed_owner = roster.iter().any(|m| is_me(m) && m.role == "owner" && m.status == MemberStatus::Confirmed);
    confirmed_owner
        && roster.iter().filter(|m| !is_me(m)).all(|m| {
            let email = normalise_email(&m.email);
            let own: Vec<&str> = collections.iter().filter(|c| c.authenticated && c.email == email).map(|c| c.id.as_str()).collect();
            m.role == "member"
                && m.collections.iter().all(|c| {
                    own.as_slice() == [c.as_str()] && m.read_only.iter().any(|r| r == c) && !m.manage.iter().any(|x| x == c)
                })
        })
}

/// This account's own row in a roster -- see [`Ctx::own_ou_id`].
fn own_row<'a>(roster: &'a [Member], account_id: &str) -> Option<&'a Member> {
    roster.iter().find(|m| m.user_id.as_deref() == Some(account_id))
}

/// What one candidate org's roster said (fix round 1, I2): three answers, so
/// a roster that could not be READ is never taken for one that said no --
/// "no" drops a cached id, and "could not read" must not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attestation {
    /// Passes [`roster_says_mine`].
    Mine,
    /// Fails it, or the server would not show this account the roster (403
    /// or 404: no ManageUsers there, or no such org).
    NotMine,
    /// An answer that was not a roster.
    Unknown,
}

/// Which org a share files into ([`Sharing::org_for_a_share`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OrgChoice {
    /// Made by this client or adopted by the user: written to without asking.
    Ours(String),
    /// The one candidate that passes the roster rule, which the user has not
    /// chosen yet: the share stops and asks.
    Adoptable(AdoptionQuestion),
    /// No org of this account's: the share makes one.
    Nothing,
}

impl Sharing {
    /// **This account's own sharing org, for every write but a share's first
    /// step** -- the cached id, and only while this client MADE it or the
    /// user ADOPTED it, and the profile still lists it owned and named for
    /// this account (official-path spec §3.1, fix round 1). No roster is read
    /// and nothing is guessed: an org that is neither is not this account's
    /// to write to until the user says so, through a share
    /// ([`Self::org_for_a_share`]).
    ///
    /// **A cached id that fails is dropped** from `sharing_state.json`, with
    /// a plain warning naming nothing. **Only an id the profile LISTS can
    /// fail.** One it does not list is kept: the profile may be older than
    /// the id (`a_cached_id_the_profile_does_not_list_yet_is_kept`).
    ///
    /// # Errors
    ///
    /// None today; `Result` so a write's callers keep one shape with
    /// [`Self::org_for_a_share`].
    pub(crate) fn my_org_id(&self, profile: &Profile, account: &Account) -> Result<Option<String>, SharingError> {
        Ok(self.trusted_org(profile, account, true))
    }

    /// [`Self::my_org_id`]'s rule. `may_forget` is false on a READ path
    /// ([`Self::note_directory`]), which writes no state (fix round 1, I2).
    fn trusted_org(&self, profile: &Profile, account: &Account, may_forget: bool) -> Option<String> {
        let path = self.setup().state_path?;
        let me = crate::sharing_state::SharingState::load(&path).account(&account.id);
        let cached = me.sharing_org_id?;
        if !(me.sharing_org_created || me.sharing_org_adopted) {
            return None;
        }
        let listed = profile.organizations.iter().find(|o| o.id == cached)?;
        if owned_and_named(listed, &account.email) {
            return Some(cached);
        }
        if may_forget {
            self.forget_cached_org(&account.id, &cached);
        }
        None
    }

    /// **The org a share files into** (official-path spec §3.1, fix round
    /// 1): [`Self::my_org_id`]'s when there is one; otherwise every candidate
    /// ([`owned_and_named`]) has its roster read and judged
    /// ([`roster_says_mine`]), and
    ///
    /// - none passes: [`OrgChoice::Nothing`], and the share makes a new org;
    /// - one passes: [`OrgChoice::Adoptable`], and the share asks the user;
    /// - more than one: refused with [`SEVERAL_ORGS`] (I1) -- picking the
    ///   first was a guess, and a planted org among them would be it half the
    ///   time.
    ///
    /// A cached id that is neither made here nor adopted (a file from before
    /// adoption existed) is judged like any candidate, and dropped when its
    /// roster says no.
    ///
    /// # Errors
    ///
    /// **A roster that cannot be read is a refusal** ([`ROSTER_UNREADABLE`]),
    /// never a "no": read as no, a share would make a second org beside the
    /// real one, and drop its cached id. A failed request is the backend's.
    pub(crate) fn org_for_a_share(&self, profile: &Profile, account: &Account, keys: &VaultKeys) -> Result<OrgChoice, SharingError> {
        if let Some(id) = self.trusted_org(profile, account, true) {
            return Ok(OrgChoice::Ours(id));
        }
        let cached = self
            .setup()
            .state_path
            .and_then(|path| crate::sharing_state::SharingState::load(&path).account(&account.id).sharing_org_id);
        let mut passing = Vec::new();
        for org in profile.organizations.iter().filter(|o| owned_and_named(o, &account.email)) {
            match self.vouched(&org.id, &account.id, keys)? {
                Attestation::Mine => passing.push(org),
                Attestation::NotMine if cached.as_deref() == Some(org.id.as_str()) => self.forget_cached_org(&account.id, &org.id),
                Attestation::NotMine => {}
                Attestation::Unknown => return Err(SharingError::Refused(ROSTER_UNREADABLE.to_string())),
            }
        }
        match passing.as_slice() {
            [] => Ok(OrgChoice::Nothing),
            [org] => Ok(OrgChoice::Adoptable(AdoptionQuestion {
                organization_id: org.id.clone(),
                name: org.name.clone().unwrap_or_default(),
            })),
            _ => Err(SharingError::Refused(SEVERAL_ORGS.to_string())),
        }
    }

    /// One candidate's roster and person collections, read now, and their
    /// word ([`roster_says_mine`]). Through [`Self::call`], not
    /// [`Self::route`]: this is a question about an org this account may not
    /// even be the sharer of, so a 404 or 403 says nothing about the
    /// server's routes (I3) -- it is a no.
    fn vouched(&self, org: &str, account_id: &str, keys: &VaultKeys) -> Result<Attestation, SharingError> {
        let answer = self.call(|client, auth| match client.fetch_organization_members(&mut auth.session, org) {
            Err(RestError::Status(403 | 404)) => Ok(None),
            other => other.map(Some),
        })?;
        let Some(answer) = answer else {
            return Ok(Attestation::NotMine);
        };
        let Some(roster) = crate::rest::organizations::map_members(&answer) else {
            return Ok(Attestation::Unknown);
        };
        // No org key, no authenticated collection name: rule 3 cannot hold
        // for anybody with a seat, and the org could not be shared into.
        let Some(org_key) = keys.organization(org).cloned() else {
            return Ok(Attestation::NotMine);
        };
        let collections = self.collections_of(org, &org_key)?;
        Ok(if roster_says_mine(&roster, &collections, account_id) { Attestation::Mine } else { Attestation::NotMine })
    }

    /// **Yes to "Use the organisation ... for sharing?"** -- the user's own
    /// choice, pinned as `sharing_org_adopted` beside the id, after which the
    /// org is used as one made here is (official-path spec §3.1, fix round
    /// 1, C1 rule 4).
    ///
    /// The org is judged again first, as a share would judge it: the answer
    /// may come minutes after the question, and a roster that changed in
    /// between -- an admin added -- is refused ([`ADOPT_REFUSED`]) rather than
    /// pinned on the strength of what it said before.
    ///
    /// # Errors
    ///
    /// It is not the org the last share asked about, or that question was
    /// already answered ([`ADOPT_NOT_ASKED`]); the org is no longer a
    /// candidate or no longer passes; its roster could not be read; or the
    /// choice could not be saved ([`ADOPT_NOT_SAVED`]) -- said, since unsaved
    /// it would be asked again.
    pub fn adopt(&self, organization_id: &str) -> Result<(), SharingError> {
        let _one = self.one_at_a_time();
        // **Only the org the last share asked about, and only once** (fix
        // round 2, m2): any adopt spends the question, whatever it named and
        // however it ends, so a second Yes -- or a call naming another org --
        // needs a share to ask again.
        let asked = self.held().asked_adoption.take();
        if asked.as_deref() != Some(organization_id) {
            return Err(SharingError::Refused(ADOPT_NOT_ASKED.to_string()));
        }
        self.require_available()?;
        let profile = self.profile()?;
        let account = keyed_account(self.held().account.clone())?;
        if !profile.organizations.iter().any(|o| o.id == organization_id && owned_and_named(o, &account.email)) {
            return Err(SharingError::Refused(ADOPT_REFUSED.to_string()));
        }
        let keys = self.keys_from(&profile)?;
        match self.vouched(organization_id, &account.id, &keys)? {
            Attestation::Mine => {}
            Attestation::NotMine => return Err(SharingError::Refused(ADOPT_REFUSED.to_string())),
            Attestation::Unknown => return Err(SharingError::Refused(ROSTER_UNREADABLE.to_string())),
        }
        let Some(path) = self.setup().state_path else {
            return Err(SharingError::Refused(ADOPT_NOT_SAVED.to_string()));
        };
        let written = crate::sharing_state::update(&path, |state| {
            let me = state.account_mut(&account.id);
            me.sharing_org_id = Some(organization_id.to_string());
            me.sharing_org_created = false;
            me.sharing_org_adopted = true;
        });
        match written {
            Ok(_) => Ok(()),
            Err(e) if crate::sharing_state::unreadable(&e).is_some() => Err(state_refused(&e)),
            Err(e) => {
                log::warn!("an adopted sharing organisation could not be saved: {e}");
                Err(SharingError::Refused(ADOPT_NOT_SAVED.to_string()))
            }
        }
    }

    /// Drops a cached sharing org id that failed [`Self::my_org_id`] -- only
    /// if the file still holds that id, so a share that chose another in the
    /// meantime is not undone. Its made-here and adopted marks go with it.
    fn forget_cached_org(&self, account: &str, id: &str) {
        let Some(path) = self.setup().state_path else {
            return;
        };
        log::warn!("the cached sharing organisation is not this account's own, so it is no longer used");
        let dropped = crate::sharing_state::update(&path, |state| {
            let me = state.account_mut(account);
            if me.sharing_org_id.as_deref() == Some(id) {
                me.sharing_org_id = None;
                me.sharing_org_created = false;
                me.sharing_org_adopted = false;
            }
        });
        if let Err(e) = dropped {
            log::warn!("the cached sharing organisation could not be dropped: {e}");
        }
    }

    /// The org's person collections, decrypted where they are ciphertext.
    pub(crate) fn collections_of(&self, org: &str, key: &SymmetricKey) -> Result<Vec<PersonCollection>, SharingError> {
        let listed = self.call(|client, auth| sharing_api::organization_collections(client, &mut auth.session, org))?;
        Ok(listed
            .into_iter()
            .filter_map(|c| collection_email(&c.name, key).map(|(email, authenticated)| PersonCollection { id: c.id, email, authenticated }))
            .collect())
    }

    /// The org's roster, fresh, from `/users?includeCollections=true`.
    ///
    /// **An answer that is not a roster is an error, never an empty one** --
    /// [`crate::rest::organizations::Roster`]'s own distinction. Read as
    /// nobody, it would have a share invite people who are already members,
    /// and [`Self::reload_org`] hold a roster that shows everyone as `Gone`.
    ///
    /// **The sharer's own org only.** A 404 is read with the profile's help
    /// ([`Self::own_roster_missing`]): an org the profile still lists as this
    /// account's to own is a server without the route -- sharing goes off as
    /// [`Self::route`]'s 405 and 501 turn it off (official-path spec §2; an
    /// older NodeWarden had no `/users`) -- and any other is an org that has
    /// gone.
    pub(crate) fn fetch_roster(&self, org: &str) -> Result<Vec<Member>, SharingError> {
        let answer = self.route(|client, auth| match client.fetch_organization_members(&mut auth.session, org) {
            Err(RestError::Status(404)) => Ok(None),
            other => other.map(Some),
        })?;
        let Some(answer) = answer else {
            return Err(self.own_roster_missing(org));
        };
        crate::rest::organizations::map_members(&answer).ok_or_else(|| SharingError::Refused(ROSTER_UNREADABLE.to_string()))
    }

    /// **The sharer's own roster answered 404** (fix round 1, I3). Official
    /// Bitwarden answers 404 for an org that was deleted and for one this
    /// account may no longer manage -- neither is an old server. So the
    /// profile is read again: still listed with the owner role, it is the
    /// route that is missing, and sharing goes off ([`REASON_TOO_OLD`]).
    /// Otherwise the org is dropped -- from `held` and from the cached id,
    /// made-here or adopted -- so the next share makes a new one, and the
    /// write that asked is told so ([`ORG_GONE`]).
    fn own_roster_missing(&self, org: &str) -> SharingError {
        let profile = match self.profile() {
            Ok(profile) => profile,
            Err(e) => return e,
        };
        if profile.organizations.iter().any(|o| o.id == org && o.is_owner()) {
            self.held().capabilities.too_old = true;
            return SharingError::Unavailable(REASON_TOO_OLD);
        }
        let account = {
            let mut held = self.held();
            if held.my_org.as_ref().is_some_and(|mine| mine.id == org) {
                held.my_org = None;
            }
            held.account.clone()
        };
        if let Some(account) = account {
            self.forget_cached_org(&account.id, org);
        }
        SharingError::Refused(ORG_GONE.to_string())
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
        self.context_if_any()?.ok_or_else(|| SharingError::Refused("You are not sharing anything yet.".to_string()))
    }

    /// [`Self::context`], with "no sharing org" as `None` rather than a
    /// refusal -- for `forget`, where it means there is nothing to forget.
    ///
    /// The org is looked for BEFORE the account's key pair is required: an
    /// account with no org has shared nothing whether or not it has keys, so
    /// that answer is the true one for both.
    pub(crate) fn context_if_any(&self) -> Result<Option<Ctx>, SharingError> {
        self.require_available()?;
        let profile = self.profile()?;
        let account = self.held().account.clone().ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        let Some(org_id) = self.my_org_id(&profile, &account)? else {
            return Ok(None);
        };
        let account = keyed_account(Some(account))?;
        let keys = self.keys_from(&profile)?;
        let org_key = keys
            .organization(&org_id)
            .cloned()
            .ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        let collections = self.collections_of(&org_id, &org_key)?;
        let roster = self.fetch_roster(&org_id)?;
        let own_ou_id = own_row(&roster, &account.id).map(|row| row.id.clone());
        Ok(Some(Ctx { account, keys, org_id, org_key, collections, roster, own_ou_id }))
    }

    /// **Creates the sharing org** (spec 9.1): a fresh org key wrapped to the
    /// account's own public key -- derived from its own private key, not
    /// asked of the server -- an RSA pair, and the first person's collection
    /// name. Returns the org's id and key. `refused` is the sentence a
    /// failure is said with: [`ORG_FAILED`], or [`NO_FRESH_ORG`] when the
    /// user has just said No to an org they could have used. A server that
    /// answers the create with a refusal is, outside that No, told
    /// [`needs_org`] instead, and remembered as one that will not create.
    fn create_org(
        &self,
        profile: &Profile,
        account: &Account,
        keys: &VaultKeys,
        first_email: &str,
        refused: &str,
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
        // **Refused, not unreachable**, is the server saying it will not make
        // one: an official self-hosted server creates an organisation only
        // from a license ([SelfHosted(NotSelfHostedOnly = true)], base spec
        // §4.3). The user makes it in the web vault and the next share offers
        // it for adoption (official-path spec §3.1); the server's own words
        // follow. After a No to an org that could have been used, the
        // sentence stays [`NO_FRESH_ORG`]: making yet another org of the same
        // name would only be asked about again.
        //
        // **Only a 400 is that answer** (fix round 1): the attribute throws
        // `BadRequestException`, which `sharing_answer` reads as
        // `RestError::Rejected`. The raw answer is kept here, before
        // [`Self::call`] folds it into `SharingError`, because there a 400
        // with no sentence and a 403 or a 429 are the same
        // `VaultError::Http`. Any other status is no verdict on making
        // organisations -- a rate limit passes -- so it says `refused` and
        // learns nothing.
        let answer = self.call(|client, auth| Ok(sharing_api::create_organization(client, &mut auth.session, &body)))?;
        let id = match answer {
            Ok(id) => id,
            Err(RestError::Rejected { description, .. }) => {
                self.held().capabilities.creates_orgs = Some(false);
                let sentence = if refused == NO_FRESH_ORG { refused.to_string() } else { needs_org(&account.email) };
                return Err(SharingError::Refused(description).or_say(&sentence));
            }
            Err(other) => return Err(SharingError::Vault(crate::rest::backend::rest_error(other)).or_say(refused)),
        };
        self.held().capabilities.creates_orgs = Some(true);
        Ok((id, org_key))
    }

    /// C(E), found by decrypted name or created with this account as its
    /// manager (spec 5.2). Returns its id.
    ///
    /// **Only an AUTHENTICATED name is found** -- rule 2 for a grant (final
    /// review, I1). What comes back is the collection the person is invited
    /// to and the record is filed into, so a plaintext name -- which the
    /// server can write -- must not be it: a server that listed `anna@...` in
    /// plaintext over a collection someone else already reaches would be
    /// handed the record. Beside a plaintext twin, the engine makes its own.
    ///
    /// The manager is named by this account's own ORG-USER id
    /// ([`Ctx::own_ou_id`]), read off the roster. It used to be the user id,
    /// which is NodeWarden's org-user id and nobody else's: an official
    /// server or Vaultwarden would have answered "no such user".
    fn ensure_collection(&self, ctx: &mut Ctx, email: &str) -> Result<String, SharingError> {
        if let Some(id) = ctx.authenticated_collection_for(email) {
            return Ok(id.to_string());
        }
        let manager = ctx.own_ou_id.clone().ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
        let name = crate::rest::crypto::encrypt(&ctx.org_key, email.as_bytes()).map_err(crypto)?;
        let id = self
            .route(|client, auth| sharing_api::create_collection(client, &mut auth.session, &ctx.org_id, &name, &manager))
            .map_err(|e| e.or_say(&format!("Couldn't prepare sharing with {email}. Try again.")))?;
        // Authenticated: this engine wrote the name, under the org key.
        ctx.collections.push(PersonCollection { id: id.clone(), email: email.to_string(), authenticated: true });
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

    /// **The person's membership, with access to C(E)** (official-path spec
    /// §3.2): nothing if they have it; otherwise Bitwarden's invite, then a
    /// roster read -- the invite answers nothing, and the roster says the new
    /// org-user id and whether the server accepted at once
    /// ([`Capabilities::accepts_on_invite`]). A member without access to
    /// their own collection is removed and invited again (decision 11).
    ///
    /// No key is touched here. NodeWarden's direct add -- the org key wrapped
    /// to whatever key the server served for the address, at invite time --
    /// is gone with the route; a key is wrapped only by the confirm pass,
    /// under its pin rule (spec 11.5).
    pub(crate) fn ensure_member(&self, ctx: &mut Ctx, email: &str, collection: &str) -> Result<ShareResult, SharingError> {
        let refused = format!("Couldn't invite {email}.");
        if let Some(member) = ctx.member(email).cloned() {
            // A revoked member is listed on the collection and reaches none
            // of it (fix round 2, m1): invited again like one without access.
            if member.has_access(collection) && member.status != MemberStatus::Revoked {
                return Ok(share_result_of(&member));
            }
            let ou = member.id.clone();
            self.call(|client, auth| sharing_api::remove_user(client, &mut auth.session, &ctx.org_id, &ou))
                .map_err(|e| e.or_say(&refused))?;
        }
        self.route(|client, auth| sharing_api::invite_user(client, &mut auth.session, &ctx.org_id, email, collection))
            .map_err(|e| e.or_say(&refused))?;
        ctx.roster = self.fetch_roster(&ctx.org_id)?;
        let invited = ctx.member(email).cloned().ok_or_else(|| SharingError::Refused(refused.clone()))?;
        self.held().capabilities.accepts_on_invite = Some(invited.status != MemberStatus::Invited);
        Ok(share_result_of(&invited))
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
                self.route(|client, auth| sharing_api::set_cipher_collections(client, &mut auth.session, &record.item.id, &ids)),
            );
        }
        let cipher = self.moved_body(ctx, record).map_err(Filing::Failed)?;
        let ids = vec![collection.to_string()];
        answered(self.route(|client, auth| sharing_api::share_cipher(client, &mut auth.session, &record.item.id, &cipher, &ids)))
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

    /// **Share one record with one person** (spec 6.2-6.5, official-path
    /// spec §3). Ordered so a failure leaves the record untouched: nothing
    /// moves it until org, collection and membership exist. Every step reads
    /// what is there first, so a Retry is the same call.
    ///
    /// **Accepted at invite goes straight to confirm** (official-path spec
    /// §2, "Accepts on invite"). Vaultwarden, and NodeWarden with invitation
    /// email off, make an existing account Accepted the moment it is
    /// invited; the confirm pass then runs here, under its pin rule, so the
    /// dialog can say "Shared" rather than "waiting" for a person who will
    /// never be asked anything. A server that leaves an invitation waits for
    /// the person to accept, and the daemon confirms on a later pass.
    ///
    /// **An org this client did not make is never written to unasked**
    /// (official-path spec §3.1, fix round 1). When the only org that could
    /// be this account's is one the user has not chosen, the share stops
    /// before writing anything and answers [`SharingError::NeedsAdoption`].
    ///
    /// # Errors
    ///
    /// [`SharingError::Refused`] with the sentence to show,
    /// [`SharingError::NeedsAdoption`] with the question to ask, or the
    /// backend's own failure.
    pub fn share(&self, item_id: &str, email: &str) -> Result<ShareOutcome, SharingError> {
        self.share_with(item_id, email, false)
    }

    /// **The share after No** to "Use the organisation ... ?": the org that
    /// was offered is left alone and pinned as nothing, and this share makes
    /// a new org of its own -- or, on a server that will not let it, says so
    /// ([`NO_FRESH_ORG`]). An org this client made or the user adopted
    /// meanwhile is still used, rather than made twice.
    ///
    /// # Errors
    ///
    /// As [`Self::share`], never [`SharingError::NeedsAdoption`].
    pub fn share_in_a_new_org(&self, item_id: &str, email: &str) -> Result<ShareOutcome, SharingError> {
        self.share_with(item_id, email, true)
    }

    fn share_with(&self, item_id: &str, email: &str, new_org: bool) -> Result<ShareOutcome, SharingError> {
        let _one = self.one_at_a_time();
        if new_org {
            // No is an answer: the question is spent (m2).
            self.held().asked_adoption = None;
        }
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
        // **Pins that cannot be read end the share here**, before an org, a
        // collection or a membership is written: the confirm a share may run
        // straight after its invite is a pin decision, and the state write
        // below would be refused anyway. A file that did not parse is set
        // aside by the check, so the Retry the sentence asks for starts from
        // a fresh one.
        self.pins_for_a_write()?;
        let mut keys = self.keys_from(&profile)?;
        let (record, failures) = self.fetch_record(item_id, &keys)?;
        // The record's own refusals first -- the trash, an attachment, a part
        // that did not decrypt -- judged against its own org, so a record
        // that cannot move is refused before the sharing org is looked for
        // (which reads a roster). The org check follows, against ours.
        refuse_unmovable(&record, &failures, organization_of(&record.item))?;
        let existing = if new_org {
            self.my_org_id(&profile, &account)?
        } else {
            match self.org_for_a_share(&profile, &account, &keys)? {
                OrgChoice::Ours(id) => Some(id),
                OrgChoice::Adoptable(question) => {
                    // Recorded, so only this answer can pin it (m2).
                    self.held().asked_adoption = Some(question.organization_id.clone());
                    return Err(SharingError::NeedsAdoption(question));
                }
                OrgChoice::Nothing => None,
            }
        };
        refuse_unmovable(&record, &failures, existing.as_deref())?;

        let (org_id, org_key, collections, created) = match existing {
            Some(org_id) => {
                let org_key = keys.organization(&org_id).cloned().ok_or_else(|| SharingError::Refused(ORG_FAILED.to_string()))?;
                let collections = self.collections_of(&org_id, &org_key)?;
                (org_id, org_key, collections, false)
            }
            None => {
                let refused = if new_org { NO_FRESH_ORG } else { ORG_FAILED };
                let (org_id, org_key) = self.create_org(&profile, &account, &keys, &email, refused)?;
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
                (org_id, org_key, collections, true)
            }
        };
        if let Some(path) = self.setup().state_path {
            let (account_id, org) = (account.id.clone(), org_id.clone());
            let was_personal = organization_of(&record.item).is_none();
            let moved = crate::sharing_state::Move { folder_id: record.item.folder_id.clone(), favorite: record.item.favorite };
            let written = crate::sharing_state::update(&path, |state| {
                let me = state.account_mut(&account_id);
                // Made here now, or made here or adopted before under this
                // same id: such an id is this account's without asking again
                // (`Sharing::my_org_id`). Another id carries neither mark.
                let same = me.sharing_org_id.as_deref() == Some(org.as_str());
                me.sharing_org_created = created || (me.sharing_org_created && same);
                me.sharing_org_adopted = !created && me.sharing_org_adopted && same;
                me.sharing_org_id = Some(org);
                if was_personal {
                    me.moves.insert(record.item.id.clone(), moved);
                }
            });
            match written {
                Ok(_) => {}
                // Unreadable since the check above (another process wrote
                // it): refused as the check refuses, with nothing moved.
                Err(e) if crate::sharing_state::unreadable(&e).is_some() => return Err(state_refused(&e)),
                Err(e) => log::warn!("the sharing state could not be written before a share: {e}"),
            }
        }
        let roster = self.fetch_roster(&org_id)?;
        let own_ou_id = own_row(&roster, &account.id).map(|row| row.id.clone());
        let mut ctx = Ctx { account, keys, org_id, org_key, collections, roster, own_ou_id };
        let collection = self.ensure_collection(&mut ctx, &email)?;
        let mut result = self.ensure_member(&mut ctx, &email, &collection)?;
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
        if result == ShareResult::WaitingAccept {
            if let Err(e) = self.confirm_pass(Trigger::Own) {
                log::info!("the confirm after a share stopped early: {e}");
            }
            let confirmed = self
                .held()
                .my_org
                .as_ref()
                .and_then(|org| org.member(&email))
                .is_some_and(|member| member.status == MemberStatus::Confirmed);
            if confirmed {
                result = ShareResult::Shared;
            }
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
        // Every collection named for them, a plaintext twin included: a stop
        // is a removal, and one that left the record in the twin would say
        // "stopped" while they can still see it (`person_collections_for`).
        let theirs: Vec<String> = ctx.collections_for(&email).into_iter().map(str::to_string).collect();
        if theirs.is_empty() {
            return Ok(nothing());
        }
        let (record, failures) = self.fetch_record(item_id, &ctx.keys)?;
        if organization_of(&record.item) != Some(ctx.org_id.as_str()) {
            self.drop_stale_move(&ctx.account.id, &record.item.id);
            return Ok(nothing());
        }
        let current: Vec<String> = collection_ids_of(&record.item).into_iter().map(str::to_string).collect();
        if !current.iter().any(|id| theirs.contains(id)) {
            return Ok(nothing());
        }
        let remaining: Vec<String> = current.into_iter().filter(|id| !theirs.contains(id)).collect();
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
                    if let Err(e) = self.call(|client, auth| sharing_api::delete_cipher(client, &mut auth.session, made)) {
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
                let theirs = ctx.collections_for(email);
                let still_theirs = !theirs.is_empty()
                    && self
                        .fetch_record(old, &ctx.keys)
                        .is_ok_and(|(org_copy, _)| collection_ids_of(&org_copy.item).iter().any(|id| theirs.contains(id)));
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

    /// `DELETE /api/ciphers/{id}` -- Bitwarden's permanent delete, on every
    /// server ([`sharing_api::delete_cipher`] says why it is not NodeWarden's
    /// `/delete` suffix any more).
    ///
    /// **A 404 is done**: the org copy is already gone. `delete_cipher`
    /// matches the status itself, not the [`VaultError::Http`] it maps to --
    /// that one is every 4xx, and reading a 403 as "gone" would drop the
    /// retry and leave the copy in the org, filed nowhere, for good. A 5xx
    /// or a lost connection stays an error, and so pending.
    fn purge_org_copy(&self, id: &str) -> Result<(), SharingError> {
        self.call(|client, auth| sharing_api::delete_cipher(client, &mut auth.session, id))
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

// ---- people -----------------------------------------------------------------------

/// One record a forget could not stop: its name, when it could be read, and
/// why -- the stop's own sentence, or `None` for a failure that has none (a
/// server that did not answer).
struct Unstopped {
    name: Option<String>,
    reason: Option<String>,
}

/// The refusal of a forget some of whose stops failed.
///
/// **One record: its own reason, named**, so the user can act on it -- the
/// trash ([`STOP_IN_TRASH`]), an attachment ([`MOVE_BACK_ATTACHMENTS`]), a
/// part that did not decrypt ([`UNREADABLE_PART`]) each say what to do, and
/// "This record" in them needs the name beside it: a trashed record is not
/// in the daemon's copy, so the view cannot point at it. **Several: how
/// many** -- a list of sentences is not a toast. The count is not said to be
/// "still visible to them": a failed stop on a record whose move back is
/// pending may not be one they can still see (see
/// [`Sharing::finish_pending`]), and this sentence is true either way.
fn forget_refused(email: &str, unstopped: &[Unstopped]) -> String {
    match unstopped {
        [Unstopped { name, reason: Some(reason) }] => match name {
            Some(name) => format!("Couldn't forget {email}. Sharing \u{201c}{name}\u{201d} couldn't be stopped. {reason}"),
            None => format!("Couldn't forget {email}. {reason}"),
        },
        [_] => format!("Couldn't forget {email}. Sharing couldn't be stopped on one record."),
        several => format!("Couldn't forget {email}. Sharing couldn't be stopped on {} records.", several.len()),
    }
}

impl Sharing {
    /// **Forget a person** (spec 6.8, the ✕ on a suggestion): every record
    /// shared with them stopped, their membership or invitation removed,
    /// their collection deleted, their pin forgotten.
    ///
    /// # Which records
    ///
    /// **The server's list of what their collection holds**
    /// ([`sharing_api::collection_items`]), with any the daemon's `items`
    /// file there added. The daemon's copy alone is not enough: it holds no
    /// trash and no archive, so a trashed record filed only for them was
    /// never stopped, and the collection delete then left it in the org under
    /// a collection that no longer exists (review of Task 11). The server's
    /// list names it, its stop refuses ([`STOP_IN_TRASH`]), and so the
    /// forget stops short of the delete, below. A list that cannot be read
    /// ends the forget before anything is written.
    ///
    /// # Nobody to forget
    ///
    /// No sharing org, or a server without sharing: nothing was ever shared
    /// with anyone, so there is nothing to forget, and the answer is an
    /// empty outcome rather than a refusal. The window's ✕ on a suggestion
    /// names people the user only ever sent Sends to, and must not fail for
    /// them.
    ///
    /// The address book (`sent_history`) is the window's to prune, after it
    /// has deleted the person's Sends -- the existing forget question does
    /// both halves in one answer (spec 11.7).
    ///
    /// # A stop that fails part-way
    ///
    /// Every record is still tried: one that cannot stop (an attachment, the
    /// trash, a server that refused) must not keep the others shared. But
    /// **nothing after the stops runs while one failed**: [`Self::stop_with`]
    /// refuses only while the record is (or may be) still theirs, so removing
    /// the membership or deleting the collection then would report the person
    /// forgotten while they can still see something -- and would take away
    /// the collection the Retry finds that record by. Their pin stays too, so
    /// the key they joined with is still the one checked. Removing the
    /// membership FIRST, to cut them off whatever the stops did, was the
    /// alternative; it was not taken because the record it strands is still
    /// filed in their collection in an org they have left, shared with nobody
    /// and never coming home.
    ///
    /// A session that ended (or sharing gone) ends the loop at once: every
    /// later stop would fail the same way. Only a stop that changed something
    /// is reported in `stopped`; a record already off their collection is
    /// not news.
    ///
    /// # Why a failure rides in the outcome
    ///
    /// Once one stop has landed, the call's answer is `Ok` with
    /// [`ForgetOutcome::failed`] set, not `Err`: an `Err` would drop the
    /// stops already made -- a record moved home, another unfiled -- and the
    /// daemon would keep showing the person on them until its next whole
    /// read. `vault_api`'s route files and announces `changed`, then answers
    /// the refusal. Pinned by
    /// `a_refused_forget_through_the_daemon_still_announces_what_it_stopped`.
    ///
    /// # Errors
    ///
    /// Before anything was written -- the collection's list unread, say --
    /// the sentence to show, or the backend's failure. The account's own
    /// address is refused ([`YOURSELF`]) -- its
    /// membership is the org's owner.
    pub fn forget(&self, email: &str, items: &[VaultItem]) -> Result<ForgetOutcome, SharingError> {
        let _one = self.one_at_a_time();
        let ctx = match self.context_if_any() {
            Ok(Some(ctx)) => ctx,
            Ok(None) | Err(SharingError::Unavailable(_)) => return Ok(ForgetOutcome::default()),
            Err(e) => return Err(e),
        };
        let email = normalise_email(email);
        if email == normalise_email(&ctx.account.email) {
            return Err(SharingError::Refused(YOURSELF.to_string()));
        }
        let mut outcome = ForgetOutcome::default();
        // Every collection named for them, a plaintext twin included -- a
        // forget is a removal (`person_collections_for`), and a twin left
        // standing would keep whatever is filed in it theirs.
        let collections: Vec<String> = ctx.collections_for(&email).into_iter().map(str::to_string).collect();
        let mut records: Vec<String> = Vec::new();
        for collection in &collections {
            let listed = self
                .call(|client, auth| sharing_api::collection_items(client, &mut auth.session, &ctx.org_id, collection))
                .map_err(|e| e.or_say(&format!("Couldn't forget {email}. Deskwarden couldn't read what is shared with them. Try again.")))?;
            for id in listed {
                if !records.contains(&id) {
                    records.push(id);
                }
            }
        }
        for item in items {
            if organization_of(item) == Some(ctx.org_id.as_str())
                && collection_ids_of(item).iter().any(|id| collections.iter().any(|c| c == id))
                && !records.contains(&item.id)
            {
                records.push(item.id.clone());
            }
        }
        outcome.failed = 'forget: {
            let mut unstopped = Vec::new();
            for id in records {
                match self.stop_with(&ctx, &id, &email) {
                    Ok(stopped) => {
                        if stopped.moved_to.is_some() || !stopped.changed.is_empty() {
                            outcome.stopped.push(id);
                        }
                        outcome.changed.upserted.extend(stopped.changed.upserted);
                        outcome.changed.removed.extend(stopped.changed.removed);
                    }
                    Err(ended @ (SharingError::Vault(VaultError::Unauthorized) | SharingError::Unavailable(_))) => {
                        break 'forget Some(ended);
                    }
                    Err(e) => {
                        log::info!("a record could not be stopped while forgetting a person: {e}");
                        unstopped.push((id, e));
                    }
                }
            }
            if !unstopped.is_empty() {
                // The name is read only for the one-record sentence, and
                // from the server when the daemon's copy lacks the record
                // (the trash) -- one more read, on this branch alone.
                let named = |id: &str| {
                    items
                        .iter()
                        .find(|item| item.id == id)
                        .map(|item| item.name.clone())
                        .or_else(|| self.fetch_record(id, &ctx.keys).ok().map(|(record, _)| record.item.name))
                        .filter(|name| !name.is_empty())
                };
                let one = unstopped.len() == 1;
                let unstopped: Vec<Unstopped> = unstopped
                    .into_iter()
                    .map(|(id, e)| Unstopped {
                        name: if one { named(&id) } else { None },
                        reason: match e {
                            SharingError::Refused(sentence) => Some(sentence),
                            _ => None,
                        },
                    })
                    .collect();
                break 'forget Some(SharingError::Refused(forget_refused(&email, &unstopped)));
            }
            // Past the stops, the person can see nothing of ours: the
            // sentence for a later step says so, and the server's own words
            // follow it (`or_say`).
            let unfinished = format!("Sharing with {email} has stopped, but Deskwarden couldn't finish forgetting them. Try again.");
            let roster = match self.fetch_roster(&ctx.org_id) {
                Ok(roster) => roster,
                Err(e) => break 'forget Some(e),
            };
            if let Some(member) = roster.into_iter().find(|m| normalise_email(&m.email) == email) {
                if let Err(e) = self.call(|client, auth| sharing_api::remove_user(client, &mut auth.session, &ctx.org_id, &member.id)) {
                    break 'forget Some(e.or_say(&unfinished));
                }
            }
            for collection in &collections {
                if let Err(e) = self.call(|client, auth| sharing_api::delete_collection(client, &mut auth.session, &ctx.org_id, collection)) {
                    break 'forget Some(e.or_say(&unfinished));
                }
            }
            None
        };
        if outcome.failed.is_none() {
            if let Some(path) = self.setup().state_path {
                if let Err(e) = crate::sharing_state::update(&path, |state| state.account_mut(&ctx.account.id).forget_person(&email)) {
                    log::warn!("a forgotten person's pin could not be dropped: {e}");
                }
            }
            let mut held = self.held();
            held.key_changed.remove(&email);
            held.no_key.remove(&email);
        }
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a forget: {e}");
        }
        Ok(outcome)
    }

    /// **Re-invite** (spec 6.1's Re-invite action): a pending invitation is
    /// renewed -- `POST …/users/{ouId}/reinvite` on every server (official-path
    /// spec §3.4), a new token and a new lifetime, expired or not; anyone else
    /// without access to their collection is invited again the way a share
    /// would ([`Self::ensure_member`]). Someone who still has access is left
    /// as they are, and the answer says which of the three they are.
    ///
    /// The address is checked as a share checks it: the route takes any
    /// body, and the second branch can create a collection.
    ///
    /// # Only someone this engine named
    ///
    /// **Refused unless the person has an AUTHENTICATED collection** -- rule
    /// 2, for the one grant a card can start (final review, I1). A share
    /// starts from an address the user TYPED; a Re-invite starts from a name
    /// on the card, and a plaintext name is one the server can write. Its
    /// second branch invites the address into the org, and on a server that
    /// accepts at once the confirm pass would then hand her the org key, so
    /// a hostile server could list `mallory@...` in plaintext and have one
    /// click make her a recipient. Refused before anything is written: no
    /// key, no invite (`reinvite_refuses_a_plaintext_collection_name`).
    /// The window does not offer it on such a card either
    /// (`share_ui::PersonLine::reinvite`). Sharing a record with them from
    /// the Share dialog -- an address typed on purpose -- is the way on.
    ///
    /// # Errors
    ///
    /// The sentence to show, or the backend's failure.
    pub fn reinvite(&self, email: &str) -> Result<ShareResult, SharingError> {
        let _one = self.one_at_a_time();
        let mut ctx = self.context()?;
        let email = normalise_email(email);
        if !plausible_email(&email) {
            return Err(SharingError::Refused(NOT_AN_EMAIL.to_string()));
        }
        if email == normalise_email(&ctx.account.email) {
            return Err(SharingError::Refused(YOURSELF.to_string()));
        }
        if ctx.authenticated_collection_for(&email).is_none() {
            return Err(SharingError::Refused(not_vouched_for(&email)));
        }
        let refused = format!("Couldn't re-invite {email}.");
        let result = match ctx.member(&email).cloned() {
            Some(member) if member.status == MemberStatus::Invited => {
                let ou = member.id.clone();
                self.route(|client, auth| sharing_api::reinvite(client, &mut auth.session, &ctx.org_id, &ou))
                    .map_err(|e| e.or_say(&refused))?;
                ShareResult::Invited
            }
            _ => {
                let collection = self.ensure_collection(&mut ctx, &email)?;
                self.ensure_member(&mut ctx, &email, &collection)?
            }
        };
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a re-invite: {e}");
        }
        Ok(result)
    }
}

// ---- confirming -------------------------------------------------------------------

impl Sharing {
    /// **The confirm pass** (spec 11.5). For each accepted member of this
    /// account's sharing org whose email has an AUTHENTICATED person
    /// collection (rule 2: only a holder of the org key could have named it;
    /// a plaintext name the server could have written does not count -- see
    /// [`PersonCollection`]), fetch the key,
    /// compare it with the pin, and confirm -- wrapping the org key to it --
    /// unless the key changed. The roster is the one the last whole read
    /// fetched, so the whole-read and batch triggers cost no roster request;
    /// the timer reads it fresh, which is its one request of its own.
    ///
    /// # Which org, and which people
    ///
    /// `my_org` is only ever this account's OWN sharing org (`note_directory`
    /// and `reload_org` fill it with nothing else), so a member of someone
    /// else's org -- where this account is the recipient -- is never
    /// confirmed from here. Within it, only status 1 with an account id:
    /// an invitation has no key to wrap to, and a confirmed member has the
    /// org key already.
    ///
    /// # One person's failure is not the pass's
    ///
    /// A key read or confirm that fails for one person is logged and the
    /// next person is tried; the next trigger retries the first. Returning
    /// the error with `?` was the alternative, and it dropped the `changed`
    /// of every confirm that had already landed, so no `Sharing` change told
    /// a window about them (`one_refused_confirm_does_not_stop_the_next_person`).
    /// A session that ended, or a server that could not be reached, stops
    /// the loop at once: every later call would fail the same way.
    ///
    /// # Who is not asked again
    ///
    /// A person already `keyChanged` is re-checked by the timer alone. Their
    /// key does not come back to the pin often enough to spend a request on
    /// every hub batch; the timer (which runs while they wait) still sees a
    /// key that returned, and "Share anyway" is the user's way through before
    /// then.
    ///
    /// # Pins that cannot be read
    ///
    /// **Nobody is confirmed while `sharing_state.json` is there and does not
    /// parse.** Every confirm here is a pin decision -- compare, or trust on
    /// first use -- and a file that cannot be read would read as no pins at
    /// all, so a key already pinned and since swapped would be trusted as
    /// new. The pass logs it and waits; `state_path == None` (a test's setup)
    /// has no pins by design and is not this case
    /// (`unreadable_pins_confirm_nobody`).
    ///
    /// # The org key
    ///
    /// Read from the backend's key cache when it already holds it -- the
    /// whole read that filled `my_org` filled that cache too -- and from a
    /// fresh profile only when it does not. The cache is what every read
    /// decrypts this org's records with, so it is the same key; asking the
    /// profile every pass would add a third request to spec 11.5's two.
    fn confirm_pass(&self, trigger: Trigger) -> Result<bool, SharingError> {
        let (Some(org), Some(account)) = ({
            let held = self.held();
            (held.my_org.as_ref().map(|o| o.id.clone()), held.account.clone())
        }) else {
            return Ok(false);
        };
        if trigger == Trigger::Timer {
            let roster = self.fetch_roster(&org)?;
            if let Some(mine) = self.held().my_org.as_mut().filter(|mine| mine.id == org) {
                mine.roster = roster;
            }
        }
        let candidates: Vec<Member> = {
            let held = self.held();
            let Some(mine) = held.my_org.as_ref().filter(|mine| mine.id == org) else {
                return Ok(false);
            };
            mine.roster
                .iter()
                .filter(|m| m.status == MemberStatus::Accepted && m.user_id.as_deref().is_some_and(|id| !id.is_empty()))
                .filter(|m| mine.authenticated_collection_for(&m.email).is_some())
                .filter(|m| trigger == Trigger::Timer || !held.key_changed.contains(&normalise_email(&m.email)))
                .cloned()
                .collect()
        };
        if candidates.is_empty() {
            return Ok(false);
        }
        if !self.pins_readable() {
            log::warn!("the sharing pins could not be read, so nobody is confirmed until they can be");
            return Ok(false);
        }
        let cached = self.keys.lock().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(|keys| keys.organization(&org).cloned());
        let org_key = match cached {
            Some(key) => key,
            None => {
                let profile = self.profile()?;
                match self.keys_from(&profile)?.organization(&org).cloned() {
                    Some(key) => key,
                    None => return Ok(false),
                }
            }
        };
        let mut changed = false;
        for member in candidates {
            let email = normalise_email(&member.email);
            match self.confirm_one(&org, &org_key, &account.id, &member, &email) {
                Ok(moved) => changed |= moved,
                Err(ended @ SharingError::Vault(VaultError::Unauthorized | VaultError::Unreachable(_))) => {
                    log::info!("the confirm pass stopped: {ended}");
                    break;
                }
                Err(e) => log::info!("a person could not be confirmed this pass: {e}"),
            }
        }
        Ok(changed)
    }

    /// One candidate of [`Self::confirm_pass`]: their key, the pin check, the
    /// confirm. `Ok(true)` when the view reads differently afterwards.
    fn confirm_one(
        &self,
        org: &str,
        org_key: &SymmetricKey,
        account: &str,
        member: &Member,
        email: &str,
    ) -> Result<bool, SharingError> {
        let user_id = member.user_id.clone().unwrap_or_default();
        let Some(spki) = self.call(|client, auth| sharing_api::user_public_key(client, &mut auth.session, &user_id))? else {
            return Ok(self.held().no_key.insert(email.to_string()));
        };
        let had_no_key = self.held().no_key.remove(email);
        if !self.pin_allows(account, email, &spki) {
            return Ok(self.held().key_changed.insert(email.to_string()) || had_no_key);
        }
        self.confirm_to(org, org_key, account, member, email, sharing_api::AccountKey { user_id, public_key: spki })?;
        Ok(true)
    }

    /// Whether the pins can be read: no file configured (a test's setup),
    /// no file yet, or one that parses. See [`Self::confirm_pass`], which
    /// only asks -- it runs unasked, so it leaves a broken file where it is
    /// and waits.
    fn pins_readable(&self) -> bool {
        self.setup().state_path.is_none_or(|path| crate::sharing_state::SharingState::read(&path).is_some())
    }

    /// The same question for a write the USER asked for that must decide on
    /// a pin (a share's or re-invite's direct add): `Ok` when the pins can be
    /// read, else the refusal a state write would give -- the file set aside
    /// when it did not parse ([`crate::sharing_state::readable_for_a_write`]),
    /// so the Retry the sentence asks for can go ahead.
    fn pins_for_a_write(&self) -> Result<(), SharingError> {
        let Some(path) = self.setup().state_path else {
            return Ok(());
        };
        crate::sharing_state::readable_for_a_write(&path).map(|_| ()).map_err(|e| state_refused(&e))
    }

    /// Confirms one member to one key, pins it, and marks them confirmed in
    /// the held roster -- no roster re-read (spec 11.5's cost: two requests per
    /// person, the key and the confirm).
    ///
    /// **It does not check the pin**: its callers have, or -- "Share anyway"
    /// -- have been told by the user to trust this key over it. The pin is
    /// written only after the confirm landed, so a confirm that fails leaves
    /// the old pin standing and the person `keyChanged`.
    fn confirm_to(
        &self,
        org: &str,
        org_key: &SymmetricKey,
        account: &str,
        member: &Member,
        email: &str,
        key: sharing_api::AccountKey,
    ) -> Result<(), SharingError> {
        let wrapped = crate::rest::crypto::wrap_key_to_public(org_key, &key.public_key).map_err(crypto)?;
        let ou = member.id.clone();
        self.call(|client, auth| sharing_api::confirm_member(client, &mut auth.session, org, &ou, &wrapped))?;
        self.pin(account, email, &key);
        let mut held = self.held();
        held.key_changed.remove(email);
        held.no_key.remove(email);
        if let Some(mine) = held.my_org.as_mut().filter(|mine| mine.id == org) {
            if let Some(row) = mine.roster.iter_mut().find(|m| m.id == member.id) {
                row.status = MemberStatus::Confirmed;
            }
        }
        Ok(())
    }

    /// **"Share anyway"** (spec 6.1, `keyChanged`): the user has looked at the
    /// fingerprint and trusts the new key. Confirmed to it, and it becomes the
    /// pin.
    ///
    /// # Bound to the key the user checked
    ///
    /// `expected_sha256` is [`Fingerprint::key_sha256`] from the Show
    /// fingerprint the user compared. The key is fetched again here, and one
    /// that is not that key -- the server swapped it between the check and
    /// the click -- is refused, the person is marked `keyChanged` again, and
    /// neither the pin nor the membership changes
    /// (`share_anyway_refuses_a_key_other_than_the_one_fingerprinted`).
    /// Without it the click trusted whatever key the server served at that
    /// moment, which is the attack the fingerprint exists to catch.
    ///
    /// Held to the pass's rule 2 as well: only an accepted member whose email
    /// has an authenticated person collection -- someone this account shares
    /// with -- is confirmed here. The window offers the action only on such a
    /// card, but the route takes any address, and a confirm hands over the
    /// org key.
    ///
    /// The old pin is NOT dropped first. [`Self::confirm_to`] overwrites it
    /// once the confirm has landed; dropped first, a confirm that then
    /// failed would leave no pin at all, and the next pass would trust
    /// whatever key the server answered with then, on first use, unasked.
    ///
    /// # Errors
    ///
    /// The sentence to show, or the backend's failure.
    pub fn confirm_anyway(&self, email: &str, expected_sha256: &str) -> Result<(), SharingError> {
        let _one = self.one_at_a_time();
        let ctx = self.context()?;
        let email = normalise_email(email);
        let not_waiting = || SharingError::Refused(format!("{email} is not waiting to be confirmed."));
        if ctx.authenticated_collection_for(&email).is_none() {
            return Err(not_waiting());
        }
        let member = self
            .fetch_roster(&ctx.org_id)?
            .into_iter()
            .find(|m| {
                normalise_email(&m.email) == email
                    && m.status == MemberStatus::Accepted
                    && m.user_id.as_deref().is_some_and(|id| !id.is_empty())
            })
            .ok_or_else(not_waiting)?;
        let user_id = member.user_id.clone().unwrap_or_default();
        let spki = self
            .call(|client, auth| sharing_api::user_public_key(client, &mut auth.session, &user_id))?
            .ok_or_else(|| SharingError::Refused(format!("{email} has not signed in to a Bitwarden app yet.")))?;
        if !crate::rest::crypto::public_key_sha256_hex(&spki).eq_ignore_ascii_case(expected_sha256.trim()) {
            self.held().key_changed.insert(email.clone());
            return Err(SharingError::Refused(format!("{email}'s key changed again. Check the fingerprint before sharing.")));
        }
        self.confirm_to(&ctx.org_id, &ctx.org_key, &ctx.account.id, &member, &email, sharing_api::AccountKey { user_id, public_key: spki })?;
        if let Err(e) = self.reload_org(&ctx) {
            log::info!("the sharing view could not be re-read after a confirm: {e}");
        }
        Ok(())
    }

    /// **The fingerprint phrase** for a person (spec 9.6): their user id and
    /// the key the server holds for them, through Bitwarden's construction.
    /// The EFF list is read now and dropped when this returns, from where
    /// [`SharingSetup::eff_wordlist`] says -- beside the executable in
    /// production, the repository's own copy in a test.
    ///
    /// Any member of this account's org with an account: a confirmed one
    /// too, since the phrase is how the user checks a key they already trust.
    /// The answer carries the key's SHA-256 beside the phrase, which "Share
    /// anyway" sends back so it confirms the very key the user compared (see
    /// [`Self::confirm_anyway`]).
    ///
    /// # Errors
    ///
    /// No key yet, the word list missing, or the backend's failure.
    pub fn fingerprint(&self, email: &str) -> Result<Fingerprint, SharingError> {
        let ctx = self.context()?;
        let email = normalise_email(email);
        let member = self
            .fetch_roster(&ctx.org_id)?
            .into_iter()
            .find(|m| normalise_email(&m.email) == email && m.user_id.as_deref().is_some_and(|id| !id.is_empty()))
            .ok_or_else(|| SharingError::Refused(format!("{email} has not accepted yet, so there is no key to check.")))?;
        let user_id = member.user_id.unwrap_or_default();
        let spki = self
            .call(|client, auth| sharing_api::user_public_key(client, &mut auth.session, &user_id))?
            .ok_or_else(|| SharingError::Refused(format!("{email} has not signed in to a Bitwarden app yet.")))?;
        let words = crate::rest::eff_wordlist::load_from(&self.setup().eff_wordlist)
            .map_err(|e| SharingError::Refused(e.to_string()))?;
        let phrase = crate::rest::crypto::fingerprint_phrase(&user_id, &spki, &words).map_err(crypto)?;
        Ok(Fingerprint { phrase, key_sha256: crate::rest::crypto::public_key_sha256_hex(&spki) })
    }
}

// ---- the recipient -----------------------------------------------------------------

/// A leave asked of an organisation that is not someone else's sharing org --
/// this account's own (whose owner NodeWarden will not let leave anyway), a
/// real organisation, or one this account has already left.
pub const NOT_A_SHARER: &str = "That is not someone sharing with you.";
/// A pasted invite link the server would not accept; its own words follow
/// (`SharingError::or_say`).
pub const LINK_ACCEPT_FAILED: &str = "Couldn't accept the invitation.";

/// A pasted invite link addressed to another account (official-path spec
/// §4.1). The server would refuse it too; this says so in words, before a
/// request is made.
#[must_use]
pub fn link_for_someone_else(link_email: &str, account_email: &str) -> String {
    format!("This invite link is for {link_email}. You're signed in as {account_email}.")
}

impl Sharing {
    /// **Accept a pasted invite link** (official-path spec §4.1): read it
    /// ([`crate::invite_link::parse`]), refuse it in words when its email is
    /// not this account's, and otherwise send its token to Bitwarden's own
    /// accept route. The sharer's daemon confirms, and the records arrive on
    /// the push or the poll that confirm produces.
    ///
    /// **Nothing is filed from the link** (official-path spec §4.3: every
    /// place that needs an attested owner keeps needing one). The link's
    /// `organizationName` is plaintext anyone can write -- "Shared by
    /// boss@corp" names nobody -- so the org is not put in [`Held::waiting`]
    /// or [`Held::others`] here. It reaches Shared with me when a whole read
    /// lists it, as every other sharing org does -- its sharer the roster's
    /// one owner where a member may read it, else the name's claim marked
    /// unverified ([`Held::unverified`]); until then the window's toast
    /// (`share_ui::LINK_ACCEPTED`) is the answer.
    /// `a_links_org_name_files_no_sharer_until_the_server_attests_one` holds
    /// this.
    ///
    /// The link is the caller's; nothing here logs it or keeps it, and the
    /// token leaves this process only in the accept's body.
    ///
    /// # Errors
    ///
    /// The link is not one ([`crate::invite_link::LinkError::sentence`]), it
    /// is for someone else ([`link_for_someone_else`]), or the server refused
    /// -- its sentence after [`LINK_ACCEPT_FAILED`].
    pub fn accept_link(&self, link: &str) -> Result<(), SharingError> {
        let _one = self.one_at_a_time();
        self.require_available()?;
        let invite = crate::invite_link::parse(link).map_err(|e| SharingError::Refused(e.sentence().to_string()))?;
        let account = self.held().account.clone().ok_or_else(|| SharingError::Refused(LINK_ACCEPT_FAILED.to_string()))?;
        let mine = normalise_email(&account.email);
        if invite.email != mine {
            return Err(SharingError::Refused(link_for_someone_else(&invite.email, &mine)));
        }
        self.call(|client, auth| {
            sharing_api::accept_invite(client, &mut auth.session, &invite.organization_id, &invite.organization_user_id, &invite.token)
        })
        .map_err(|e| e.or_say(LINK_ACCEPT_FAILED))?;
        Ok(())
    }

    /// **Stop receiving from a sharer** (spec 6.9): leave their sharing org,
    /// `POST /api/organizations/{id}/leave`. Every record from them is gone
    /// from this vault at once, so the outcome names each item of `items`
    /// filed in that org, and the daemon takes them out of its copy and
    /// announces them gone -- no whole read.
    ///
    /// **Only someone ELSE's sharing org**, as the last whole read listed them
    /// ([`Held::others`]): the kebab that asks is a received record's, and a
    /// leave of anything else -- this account's own sharing org, a real
    /// organisation -- is not what "Stop receiving" means. NodeWarden refuses
    /// its owner a leave in any case.
    ///
    /// # Errors
    ///
    /// Not someone else's sharing org ([`NOT_A_SHARER`]), or the server
    /// refused.
    pub fn leave(&self, organization_id: &str, items: &[VaultItem]) -> Result<SharingOutcome, SharingError> {
        let _one = self.one_at_a_time();
        self.require_available()?;
        if !self.held().others.iter().any(|(id, _)| id == organization_id) {
            return Err(SharingError::Refused(NOT_A_SHARER.to_string()));
        }
        self.call(|client, auth| sharing_api::leave_organization(client, &mut auth.session, organization_id))
            .map_err(|e| e.or_say("Couldn't stop receiving."))?;
        {
            let mut held = self.held();
            held.others.retain(|(id, _)| id != organization_id);
            held.waiting.retain(|(id, _)| id != organization_id);
            held.unverified.remove(organization_id);
            held.capabilities.received_rosters.owners.remove(organization_id);
        }
        Ok(SharingOutcome {
            upserted: Vec::new(),
            removed: items
                .iter()
                .filter(|item| organization_of(item) == Some(organization_id))
                .map(|item| item.id.clone())
                .collect(),
        })
    }
}

// ---- invite codes -----------------------------------------------------------------

impl Sharing {
    /// **A NodeWarden registration code for one pending invitation** (owner
    /// decision Q3 (b)). Registration on NodeWarden takes an admin's invite
    /// code, so a person invited before they have an account can do nothing
    /// until someone gives them one; when the sharer IS the admin, this is
    /// that code.
    ///
    /// # Which invitation
    ///
    /// `email`'s row in a FRESH roster of this account's sharing org (spec
    /// 13: every write reads current state first) -- its id is the ouId the
    /// route is keyed by. Only a pending invitation qualifies, and anything
    /// else is refused here, before the password is even hashed: an account
    /// already there needs no code, an expired invitation has to be renewed
    /// first (the server would refuse it, and the code would expire with
    /// it), and an address nobody invited has nothing to attach a code to.
    ///
    /// # The password
    ///
    /// The route asks for the master password again, as `/api/admin/invites`
    /// does. The typed password is hashed here with the session's own master
    /// key -- exactly the hash a login sends -- into a `Zeroizing`, and only
    /// that hash is sent. The master key's lock is held for the hash alone
    /// and let go before the request, which [`Self::call`] locks again.
    ///
    /// **Why this route and not `/api/admin/invites`** (the brief's): see
    /// [`sharing_api::registration_invite`] -- a code tied to one invitation
    /// and expiring with it, refused by the server for anyone who already has
    /// an account, rather than a free-standing code that outlives the reason
    /// it was made.
    ///
    /// # Errors
    ///
    /// Not the administrator ([`NOT_ADMIN`]), no pending invitation for the
    /// address, a wrong password ([`WRONG_PASSWORD`]), another server refusal
    /// after [`INVITE_CODE_FAILED`], or the backend's own failure.
    pub fn mint_invite_code(&self, email: &str, password: &[u8]) -> Result<sharing_api::InviteCode, SharingError> {
        let _one = self.one_at_a_time();
        self.require_available()?;
        let profile = self.profile()?;
        if !profile.is_server_admin() {
            return Err(SharingError::Refused(NOT_ADMIN.to_string()));
        }
        let email = normalise_email(email);
        let no_invitation = || SharingError::Refused(format!("There is no invitation waiting for {email}."));
        let account = self.held().account.clone().ok_or_else(|| SharingError::Refused(INVITE_CODE_FAILED.to_string()))?;
        let Some(org) = self.my_org_id(&profile, &account)? else {
            return Err(no_invitation());
        };
        let member = self.fetch_roster(&org)?.into_iter().find(|m| normalise_email(&m.email) == email).ok_or_else(no_invitation)?;
        if member.status != MemberStatus::Invited {
            return Err(SharingError::Refused(has_an_account(&email)));
        }
        let expired = member
            .invitation_expires
            .as_deref()
            .and_then(crate::send::parse_iso_utc_millis)
            .is_some_and(|at| at < now_millis());
        if expired {
            return Err(SharingError::Refused(invitation_expired(&email)));
        }
        let hash = {
            let state = self.state.upgrade().ok_or(SharingError::Vault(VaultError::Unauthorized))?;
            let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.master_key.password_hash(password)
        };
        self.call(|client, auth| {
            sharing_api::registration_invite(client, &mut auth.session, &org, &member.id, &hash).map_err(|e| match e {
                // NodeWarden's 403 here is the admin check alone (a
                // non-manager is a 404): the role was taken away since the
                // profile read above.
                RestError::Status(403) => RestError::Rejected { error: String::new(), description: NOT_ADMIN.to_string() },
                other => other,
            })
        })
        .map_err(|e| invite_code_refused(e, &email))
    }
}

/// An address whose row is a member, not an invitation.
fn has_an_account(email: &str) -> String {
    format!("{email} already has an account, so they don't need an invite code.")
}

fn invitation_expired(email: &str) -> String {
    format!("The invitation to {email} has expired. Re-invite them, then make the code.")
}

/// **The registration-invite route's refusals, as plain sentences.** The
/// server's own words are matched lower-cased on a phrase, as
/// `sharing_api`'s races are, since they are the only thing that tells a
/// wrong password from a person who registered meanwhile; anything else
/// keeps them after [`INVITE_CODE_FAILED`] (`or_say`), and a session that
/// ended keeps its own error.
fn invite_code_refused(error: SharingError, email: &str) -> SharingError {
    let SharingError::Refused(said) = &error else {
        return error.or_say(INVITE_CODE_FAILED);
    };
    let lowered = said.to_lowercase();
    if said == NOT_ADMIN {
        error
    } else if lowered.contains("invalid password") {
        SharingError::Refused(WRONG_PASSWORD.to_string())
    } else if lowered.contains("already has an account") {
        SharingError::Refused(has_an_account(email))
    } else if lowered.contains("expired") {
        SharingError::Refused(invitation_expired(email))
    } else {
        error.or_say(INVITE_CODE_FAILED)
    }
}

/// What a share or a re-invite ends as, read off the person's roster row.
/// A revoked row never gets here from [`Sharing::ensure_member`], which
/// invites such a person again; read anyway, it is not "Shared" -- they wait
/// for an admin, as an invitation waits for its person.
fn share_result_of(member: &Member) -> ShareResult {
    match member.status {
        MemberStatus::Confirmed => ShareResult::Shared,
        MemberStatus::Accepted => ShareResult::WaitingAccept,
        MemberStatus::Invited | MemberStatus::Revoked => ShareResult::Invited,
    }
}

/// Spec 6.1's table, as a function of the roster row, the collection and
/// what the confirm pass learned.
fn person_state(member: Option<&Member>, collection_id: &str, held: &Held) -> PersonState {
    let Some(member) = member else {
        return PersonState::Gone;
    };
    let email = normalise_email(&member.email);
    match member.status {
        MemberStatus::Invited => PersonState::Invited,
        MemberStatus::Accepted if held.key_changed.contains(&email) => PersonState::KeyChanged,
        MemberStatus::Accepted if held.no_key.contains(&email) => PersonState::WaitingSignIn,
        MemberStatus::Accepted => PersonState::WaitingAccept,
        MemberStatus::Confirmed if member.has_access(collection_id) => PersonState::Shared,
        // Revoked reaches nothing until restored: not Shared, and a share or
        // Re-invite brings them back as for any member who lost access.
        MemberStatus::Confirmed | MemberStatus::Revoked => PersonState::Gone,
    }
}

/// A pending invitation whose expiry has passed -- what [`Sharing::wants_timer`]
/// leaves out, since nothing it reads can move it. Not shown on the card (no
/// expiry is claimed). A member past the invitation, or one with no expiry
/// the server named, is not expired.
fn invitation_ran_out(member: &Member, now: i64) -> bool {
    member.status == MemberStatus::Invited
        && member.invitation_expires.as_deref().and_then(crate::send::parse_iso_utc_millis).is_some_and(|at| at < now)
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

    /// A person collection whose name decrypted under the org key -- what
    /// a whole read holds for one this engine made.
    fn authentic(id: &str, email: &str) -> PersonCollection {
        PersonCollection { id: id.to_string(), email: email.to_string(), authenticated: true }
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
            manage: Vec::new(),
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
            held.capabilities.loaded = true;
            held.account = Some(Account { id: "me".into(), email: "me@x.io".into(), admin: true, keyed: true });
            held.my_org = Some(MyOrg {
                id: "org1".into(),
                collections: ["anna", "bob", "carol", "dan", "eve", "fay", "gus", "hal"]
                    .iter()
                    .map(|n| authentic(&format!("c-{n}"), &format!("{n}@x.io")))
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
        assert_eq!(state("carol@x.io"), Some(PersonState::Invited));
        assert_eq!(state("dan@x.io"), Some(PersonState::Invited), "no expiry is claimed (official-path spec §3.4)");
        assert_eq!(state("eve@x.io"), Some(PersonState::KeyChanged));
        assert_eq!(state("fay@x.io"), Some(PersonState::WaitingSignIn));
        assert_eq!(state("gus@x.io"), Some(PersonState::Gone), "a collection and no member");
        assert_eq!(state("hal@x.io"), Some(PersonState::Gone), "a member without access to their collection");
        assert_eq!(view.records, vec![RecordEntry { item_id: "r1".into(), people: vec!["anna@x.io".into(), "bob@x.io".into()] }]);
        assert!(view.shared_with_someone("r1"), "anna is Shared, so the pill's promise holds");
        assert_eq!(view.received_from("r2"), Some("anna@x.io"));
        assert_eq!(view.received_from("r3"), None);
    }

    /// Before the first pass has read the account, sharing is "not loaded",
    /// never "on".
    #[test]
    fn an_engine_that_has_not_probed_says_not_loaded() {
        let view = offline().view(&[]);
        assert!(!view.available);
        assert_eq!(view.reason.as_deref(), Some(REASON_NOT_LOADED));
    }

    /// **The first pass reads the account, once, and turns sharing on** --
    /// on any server: nothing asks which one it is (official-path spec §2).
    /// A batch pass asks nothing at all.
    #[test]
    fn the_first_pass_reads_the_account_once_and_turns_sharing_on() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup { make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        let profile = server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).expect(1).create();
        let key = server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str()).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("a first pass");
        engine.refresh(Trigger::WholeRead).expect("a second pass");
        engine.refresh(Trigger::Batch).expect("a batch pass");
        profile.assert();
        key.assert();
        assert!(engine.view(&[]).available);
        assert!(engine.held().capabilities.loaded);
    }

    /// **A route sharing needs that answers as missing turns sharing off for
    /// the session** (official-path spec §2): the sharer's own roster
    /// answering 404 -- every server gives the owner ManageUsers, so that is
    /// no route -- or any of sharing's routes answering 405 or 501.
    #[test]
    fn a_missing_route_turns_sharing_off_for_the_session() {
        for status in [404, 405, 501] {
            let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
            server.mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str()).with_status(status).create();
            assert!(engine.view(&[]).available, "{status}: available before");
            assert!(matches!(engine.fetch_roster(FIXTURE_ORG), Err(SharingError::Unavailable(REASON_TOO_OLD))), "{status}");
            let view = engine.view(&[]);
            assert!(!view.available, "{status}");
            assert_eq!(view.reason.as_deref(), Some(REASON_TOO_OLD), "{status}");
            assert!(matches!(engine.share("p1", "anna@example.com"), Err(SharingError::Unavailable(REASON_TOO_OLD))), "{status}: refused");
        }
    }

    /// The fixture profile with no key pair: `privateKey` null.
    fn keyless_profile() -> serde_json::Value {
        let mut profile = sharing_profile(serde_json::json!([]));
        profile["privateKey"] = serde_json::Value::Null;
        profile
    }

    /// **An official server shares on the same path**: one that has no
    /// NodeWarden invitation list (404) is not told sharing is off, and its
    /// view is available after the first pass.
    #[test]
    fn an_official_server_shares_on_the_same_routes() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        let view = engine.view(&[]);
        assert!(view.available, "{view:?}");
        assert_eq!(view.reason, None);
    }

    use crate::test_http::{Matcher, MockServer};

    /// **A server that will not create the sharing org** (official
    /// self-hosted, official-path spec §3.1): the share asks the user to make
    /// it in the web vault, by its name, keeps the server's own words after
    /// that, remembers the answer, and writes nothing else.
    #[test]
    fn a_server_that_will_not_create_the_org_asks_the_user_to_make_it() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        server
            .mock("POST", "/api/organizations")
            .with_status(400)
            .with_body(r#"{"message":"Only allowed when not self hosted."}"#)
            .expect(1)
            .create();
        let invites = server.mock("POST", Matcher::Regex("/users/invite$".into())).expect(0).create();
        let moved = server.mock("PUT", Matcher::Any).expect(0).create();
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::Refused(sentence)) => {
                assert!(sentence.starts_with(&needs_org(FIXTURE_EMAIL)), "{sentence}");
                assert!(sentence.ends_with("Only allowed when not self hosted."), "{sentence}");
            }
            other => panic!("expected the create-it-yourself sentence, got {other:?}"),
        }
        invites.assert();
        moved.assert();
        assert_eq!(engine.held().capabilities.creates_orgs, Some(false));
        assert_eq!(
            needs_org("Denis@Example.com"),
            "Your server needs an organisation for sharing. Create one named \"Shared by denis@example.com\" in the web vault, then try again."
        );
    }

    /// **The roster-visibility warning follows the server's own name**
    /// (official-path spec §3.4): Vaultwarden and NodeWarden keep the roster
    /// to managers and get none; official Bitwarden, any other name, and a
    /// server that names itself nothing are warned about -- the direction
    /// that discloses rather than hides.
    #[test]
    fn the_roster_warning_follows_the_servers_own_name() {
        let engine = offline();
        let visible = |name: Option<&str>| {
            {
                let mut held = engine.held();
                held.capabilities.loaded = true;
                held.capabilities.server_name = name.map(str::to_string);
            }
            engine.view(&[]).roster_visible_to_recipients
        };
        assert!(!visible(Some("Vaultwarden")));
        assert!(!visible(Some("NodeWarden")));
        assert!(!visible(Some("nodewarden")), "case does not matter");
        assert!(visible(Some("Bitwarden")));
        assert!(visible(None), "an unnamed server warns");
    }

    /// **The first pass reads the server's name once**, from `/config`.
    #[test]
    fn the_first_pass_reads_the_servers_name_once() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        let config = server
            .mock("GET", "/api/config")
            .with_body(r#"{"object":"config","server":{"name":"NodeWarden","url":"https://github.com/shuaiplus/NodeWarden"}}"#)
            .expect(1)
            .create();
        engine.refresh(Trigger::WholeRead).expect("a first pass");
        engine.refresh(Trigger::WholeRead).expect("a second pass");
        config.assert();
        assert_eq!(engine.held().capabilities.server_name.as_deref(), Some("NodeWarden"));
        assert!(!engine.view(&[]).roster_visible_to_recipients);
    }

    /// **A `/config` that fails for one pass is asked again** (fix round 1):
    /// a 500 leaves the name unknown -- warned about, as an unnamed server
    /// is -- without costing the session its answer, and the next pass that
    /// reaches it learns the name and stops asking.
    #[test]
    fn a_config_that_failed_is_asked_again_until_it_answers() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        let failing = server.mock("GET", "/api/config").with_status(500).expect(1).create();
        engine.ensure_loaded().expect("loaded, whatever /config said");
        failing.assert();
        failing.remove();
        assert!(engine.view(&[]).available);
        assert!(engine.view(&[]).roster_visible_to_recipients, "an unknown name warns");
        assert!(!engine.held().capabilities.name_known);
        let config = server
            .mock("GET", "/api/config")
            .with_body(r#"{"object":"config","server":{"name":"Vaultwarden","url":"https://github.com/dani-garcia/vaultwarden"}}"#)
            .expect(1)
            .create();
        engine.ensure_loaded().expect("a second pass");
        engine.ensure_loaded().expect("a third pass, which does not ask");
        config.assert();
        assert!(engine.held().capabilities.name_known);
        assert!(!engine.view(&[]).roster_visible_to_recipients, "Vaultwarden keeps the roster to managers");
    }

    /// **Only a 400 says the server will not make the org** (fix round 1): a
    /// rate limit or a 403 is no verdict on creating organisations, so it is
    /// the plain failure, and nothing is learned about the server.
    #[test]
    fn a_create_refused_with_another_status_is_the_plain_failure() {
        for status in [429, 403] {
            let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
            server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
            server.mock("POST", "/api/organizations").with_status(status).expect(1).create();
            match engine.share("p1", "anna@example.com") {
                Err(SharingError::Refused(sentence)) => assert_eq!(sentence, ORG_FAILED, "{status}"),
                other => panic!("{status}: expected the plain failure, got {other:?}"),
            }
            assert_eq!(engine.held().capabilities.creates_orgs, None, "{status}");
        }
    }

    /// A loaded engine (its first pass made) on a NodeWarden, over the
    /// fixture account, with its files in a scratch directory -- and, when
    /// the profile lists the fixture's own sharing org, that org cached as
    /// MADE HERE: the state of the PC that shares from it. Only such an org
    /// (or one the user adopted) is ever written to (official-path spec
    /// §3.1, fix round 1); [`on_nodewarden_unpinned`] is the PC that has not
    /// chosen it.
    fn on_nodewarden(organizations: serde_json::Value) -> (MockServer, crate::rest::backend::RestBackend, Arc<Sharing>, crate::test_scratch::ScratchDir) {
        let pinned = organizations.as_array().is_some_and(|orgs| orgs.iter().any(|org| org["id"] == FIXTURE_ORG));
        let (server, backend, engine, dir) = on_nodewarden_unpinned(organizations);
        if pinned {
            cache_org(&dir, true);
        }
        (server, backend, engine, dir)
    }

    /// A scratch directory whose `sharing_state.json` caches the fixture's
    /// own sharing org as made here, for an engine set up by hand: only a
    /// made or adopted org is written to (fix round 1).
    fn pinned_dir(tag: &str) -> crate::test_scratch::ScratchDir {
        let dir = crate::test_scratch::ScratchDir::new(tag);
        cache_org(&dir, true);
        dir
    }

    /// [`on_nodewarden`] with nothing cached: a second PC, a fresh install,
    /// or an org this client did not make.
    fn on_nodewarden_unpinned(organizations: serde_json::Value) -> (MockServer, crate::rest::backend::RestBackend, Arc<Sharing>, crate::test_scratch::ScratchDir) {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let dir = crate::test_scratch::ScratchDir::new("sharing-flow");
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup {
            state_path: Some(dir.join("sharing_state.json")),
            sent_path: Some(dir.join("sent_history.json")),
            fill_stats_path: Some(dir.join("fill-stats.json")),
            make_key_pair: fixture_key_pair,
            eff_wordlist: vec![PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/eff_large_wordlist.txt"))],
        });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(organizations).to_string()).expect_at_least(0).create();
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

    /// **The first share ever** (spec 6.2, official-path spec §3): org,
    /// collection, Bitwarden's invite, move -- and, because this server
    /// accepted the person at invite, the confirm at once. The collection
    /// names the sharer by its own ORG-USER id off the roster, which is not
    /// its user id here, as on an official server.
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
        // share made -- with the name the share sent, so it decrypts under the
        // new org key (which the test cannot know) and is authenticated, as
        // the confirm pass's rule 2 requires.
        let made_name = Arc::new(Mutex::new(String::new()));
        server.mock("GET", "/api/organizations/org-new/collections").with_body(r#"{"data":[]}"#).expect(1).create();
        let read_back = Arc::clone(&made_name);
        server
            .mock("GET", "/api/organizations/org-new/collections")
            .with_body_from_request(move |_| {
                let name = read_back.lock().expect("unpoisoned").clone();
                serde_json::json!({ "data": [{ "id": "c-anna", "organizationId": "org-new", "name": name }] }).to_string().into_bytes()
            })
            .create();
        let owner = || roster_row("owner-ou", Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]);
        server
            .mock("GET", "/api/organizations/org-new/users?includeCollections=true")
            .with_body(roster(&[owner()]))
            .expect(1)
            .create();
        server
            .mock("GET", "/api/organizations/org-new/users?includeCollections=true")
            .with_body(roster(&[owner(), roster_row("anna-ou", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]))
            .create();
        let keep = Arc::clone(&made_name);
        let collection = server
            .mock("POST", "/api/organizations/org-new/collections")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "users": [{ "id": "owner-ou", "readOnly": false, "hidePasswords": false, "manage": true }]
            })))
            .with_body_from_request(move |request| {
                let body: serde_json::Value = serde_json::from_slice(request.body()).expect("json");
                *keep.lock().expect("unpoisoned") = body["name"].as_str().unwrap_or_default().to_string();
                br#"{"id":"c-anna","object":"collection"}"#.to_vec()
            })
            .expect(1)
            .create();
        let invite = server
            .mock("POST", "/api/organizations/org-new/users/invite")
            .match_body(Matcher::PartialJson(serde_json::json!({ "emails": ["anna@example.com"], "type": 2 })))
            .with_body("")
            .expect(1)
            .create();
        let share = server
            .mock("PUT", "/api/ciphers/p1/share")
            .match_body(Matcher::PartialJson(serde_json::json!({ "collectionIds": ["c-anna"], "cipher": { "organizationId": "org-new" } })))
            .with_body_from_request(echo("p1", Some("org-new"), &["c-anna"]))
            .expect(1)
            .create();
        let key = server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).expect(1).create();
        let confirm = server
            .mock("POST", "/api/organizations/org-new/users/anna-ou/confirm")
            .match_request(|request| request.utf8_lossy_body().contains("\"key\":\"4."))
            .with_body("")
            .expect(1)
            .create();
        let direct = server.mock("POST", "/api/organizations/org-new/members/invite").expect(0).create();
        let lookup = server.mock("GET", Matcher::Regex("members/public-key".into())).expect(0).create();

        let outcome = engine.share("p1", " Anna@Example.com ").expect("shared");
        org.assert();
        collection.assert();
        invite.assert();
        share.assert();
        key.assert();
        confirm.assert();
        direct.assert();
        lookup.assert();
        sync.assert();
        assert_eq!(outcome.result, ShareResult::Shared, "accepted at invite, confirmed at once");
        assert_eq!(engine.held().capabilities.accepts_on_invite, Some(true));
        assert_eq!(outcome.changed.upserted.len(), 1);
        let moved = &outcome.changed.upserted[0];
        assert_eq!(moved.name, "Acme VPN", "the moved record decrypts under the new org key");
        assert_eq!(organization_of(moved), Some("org-new"));
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(state.sharing_org_id.as_deref(), Some("org-new"));
        assert_eq!(state.moves.get("p1").and_then(|m| m.folder_id.as_deref()), Some("f1"));
        assert!(state.pin_for("anna@example.com").is_some(), "the confirm pins the key it wrapped to");
        let sent = crate::sent_history::SentHistory::load(&dir.join("sent_history.json"));
        assert_eq!(sent.entries[0].kind, crate::sent_history::SentKind::Share);
        assert_eq!(sent.entries[0].recipient, "anna@example.com");
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

    /// **A server that leaves an invitation** (official Bitwarden, or any
    /// server with invitation email): the share files the record and says
    /// so, and nothing is confirmed until the person accepts.
    #[test]
    fn a_share_the_server_leaves_invited_waits_for_the_accept() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-carol", "carol@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(1)
            .create();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[
                roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]),
                roster_row("inv-1", None, "carol@example.com", 0, &["c-carol"]),
            ]))
            .create();
        let invite = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").expect(1).create();
        let confirms = server.mock("POST", Matcher::Regex("/confirm$".into())).expect(0).create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-carol"])).create();
        let outcome = engine.share("p1", "carol@example.com").expect("shared");
        invite.assert();
        confirms.assert();
        assert_eq!(outcome.result, ShareResult::Invited);
        assert_eq!(engine.held().capabilities.accepts_on_invite, Some(false));
    }

    /// **A failure at the move leaves the record personal** (spec 6.2, 13),
    /// through the daemon: the item in its cache is untouched and the answer
    /// is the dialog's sentence.
    #[test]
    fn a_share_that_fails_at_the_move_leaves_the_record_personal() {
        let (mut server, cache, api, token) = crate::vault_api::tests::a_daemon();
        let engine = cache.bridge().sharing().expect("an engine");
        let dir = pinned_dir("sharing-move-fails-daemon");
        engine.set_up(SharingSetup { state_path: Some(dir.join("sharing_state.json")), make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([sharing_org_row()])).to_string()).create();
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

    /// **Through the daemon, the question is an answer, No is the share
    /// again, and Yes is its own route** (fix round 1): `POST
    /// /sharing/item/{id}` answers `{adopt: {organizationId, name}}` with
    /// nothing written or announced; `newOrganization: true` makes a new org
    /// (here refused by the server, which is said); `POST /sharing/adopt`
    /// pins the org.
    #[test]
    fn the_adoption_question_goes_over_the_daemons_api() {
        let (mut server, cache, api, token) = crate::vault_api::tests::a_daemon();
        let engine = cache.bridge().sharing().expect("an engine");
        let dir = crate::test_scratch::ScratchDir::new("sharing-adopt-daemon");
        engine.set_up(SharingSetup { state_path: Some(dir.join("sharing_state.json")), make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([sharing_org_row()])).to_string()).create();
        with_collections(&mut server, &[]);
        with_roster(&mut server, &[me_row()]);
        let writes = server.mock("POST", Matcher::Regex("^/api/".into())).expect(0).create();
        let answer = crate::vault_api::tests::ask(&api, "POST", "/sharing/item/live-1", Some(&token), r#"{"email":"anna@example.com"}"#);
        assert_eq!(answer.status, 200, "{}", answer.body);
        let body: serde_json::Value = serde_json::from_str(&answer.body).expect("json");
        assert_eq!(body["data"]["adopt"]["organizationId"], FIXTURE_ORG);
        assert_eq!(body["data"]["adopt"]["name"], format!("Shared by {FIXTURE_EMAIL}"));
        assert!(body["data"].get("result").is_none(), "a question is not a result: {body}");
        writes.assert();
        assert!(api.feed().wait_after(Some(0), false, std::time::Duration::ZERO).changes.is_empty(), "a question was announced");

        server.mock("POST", "/api/organizations").with_status(400).with_body(r#"{"message":"You cannot create an organization."}"#).create();
        let answer = crate::vault_api::tests::ask(&api, "POST", "/sharing/item/live-1", Some(&token), r#"{"email":"anna@example.com","newOrganization":true}"#);
        assert_eq!(answer.status, 400, "{}", answer.body);
        let body: serde_json::Value = serde_json::from_str(&answer.body).expect("json");
        assert!(body["message"].as_str().is_some_and(|m| m.starts_with(NO_FRESH_ORG)), "{body}");
        assert_eq!(cached(&dir), (None, false), "No pinned nothing");

        // No spent the question (m2): Yes needs the share to ask again, and
        // then takes only the org it asked about.
        let adopt = |org: &str| {
            crate::vault_api::tests::ask(&api, "POST", "/sharing/adopt", Some(&token), &serde_json::json!({ "organizationId": org }).to_string())
        };
        assert_eq!(adopt(FIXTURE_ORG).status, 400, "an answer to a question already answered");
        let asked = crate::vault_api::tests::ask(&api, "POST", "/sharing/item/live-1", Some(&token), r#"{"email":"anna@example.com"}"#);
        assert!(asked.body.contains("\"adopt\""), "{}", asked.body);
        let wrong = adopt("org-other");
        assert_eq!(wrong.status, 400, "{}", wrong.body);
        assert!(wrong.body.contains(ADOPT_NOT_ASKED), "{}", wrong.body);
        crate::vault_api::tests::ask(&api, "POST", "/sharing/item/live-1", Some(&token), r#"{"email":"anna@example.com"}"#);
        let answer = adopt(FIXTURE_ORG);
        assert_eq!(answer.status, 200, "{}", answer.body);
        assert_eq!(cached(&dir).0.as_deref(), Some(FIXTURE_ORG));
        assert!(adopted(&dir), "Yes pinned the org as adopted");
        assert_eq!(crate::vault_api::tests::ask(&api, "POST", "/sharing/adopt", Some(&token), "{}").status, 400, "no organisation named");
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
    /// or written.** Sharing is on for it -- nothing about the server is
    /// wrong -- so this is where it is stopped: with no private key there is
    /// no org key it could wrap to itself or unwrap, and a sentence about the
    /// server's age or a failed organisation would send the user looking in
    /// the wrong place.
    #[test]
    fn a_keyless_account_is_refused_with_a_sentence_about_keys() {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup { make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(keyless_profile().to_string()).create();
        server.mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str()).with_status(404).create();
        let record = server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).expect(0).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("a first pass");
        assert!(engine.view(&[]).available, "control: sharing is on for a keyless account");
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
    /// person is accepted at invite, the confirm at once finds their key
    /// changed and does not confirm, the card will say so, and the pin is not
    /// moved to the new key.
    #[test]
    fn a_key_that_differs_from_the_pin_is_not_wrapped_to() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let state = dir.join("sharing_state.json");
        let old = crate::sharing_state::Pin { sha256: "00".repeat(32), user_id: "anna-id".into(), at: 1 };
        crate::sharing_state::update(&state, |s| s.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", old.clone())).expect("pinned");
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(1)
            .create();
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]);
        let invite = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").expect(1).create();
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        let confirm = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str()).expect(0).create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-anna"])).create();
        let outcome = engine.share("p1", "anna@example.com").expect("shared");
        invite.assert();
        confirm.assert();
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
        with_roster(&mut server, &[]);
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
        let delete = server.mock("DELETE", "/api/ciphers/s1").with_status(204).expect(1).create();
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
        server.mock("DELETE", "/api/ciphers/s1").with_status(500).expect(1).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("the personal copy is made; the delete waits");
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        assert!(outcome.changed.removed.is_empty(), "the org copy is still there");
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(state.unfinished.len(), 1);

        let retried = server.mock("DELETE", "/api/ciphers/s1").with_status(204).expect(1).create();
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
        server.mock("DELETE", format!("/api/ciphers/{id}").as_str()).with_status(204).expect_at_least(0).create();
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
        let cleaned = orphan.map(|id| server.mock("DELETE", format!("/api/ciphers/{id}").as_str()).with_status(204).expect(1).create());
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
        let delete_fails = server.mock("DELETE", "/api/ciphers/s1").with_status(500).expect(2).create();
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
        let deleted = server.mock("DELETE", "/api/ciphers/s1").with_status(204).expect(1).create();
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
        server.mock("DELETE", "/api/ciphers/s1").with_status(500).expect(1).create();
        assert!(engine.stop("s1", "anna@example.com").is_err(), "the person can still see it");
        server.mock("PUT", "/api/ciphers/s1/collections").with_body(shared_cipher("s1", "Acme VPN", &[]).to_string()).expect(1).create();
        let deleted = server.mock("DELETE", "/api/ciphers/s1").with_status(204).expect(1).create();
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
        server.mock("DELETE", "/api/ciphers/s1").with_status(500).create();
        let outcome = engine.stop("s1", "anna@example.com").expect("stopped");
        assert_eq!(outcome.moved_to.as_deref(), Some("p-new"));
        let org_copy = outcome.changed.upserted.iter().find(|i| i.id == "s1").expect("the unfiled org copy is announced");
        assert!(collection_ids_of(org_copy).is_empty());
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
        server.mock("DELETE", "/api/ciphers/s1").with_status(500).create();
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
        let delete_fails = server.mock("DELETE", "/api/ciphers/s1").with_status(500).expect(1).create();
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
        let gone = server.mock("DELETE", "/api/ciphers/s1").with_status(404).expect(1).create();
        let forbidden = server.mock("DELETE", "/api/ciphers/s2").with_status(403).expect(1).create();
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
        let dir = pinned_dir("sharing-move-back-daemon");
        engine.set_up(SharingSetup { state_path: Some(dir.join("sharing_state.json")), make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([sharing_org_row()])).to_string()).create();
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

    // ---- forgetting a person, and re-inviting one (Task 11) ----------------------

    /// **Forget X** (spec 6.8): every share with X stopped -- a record shared
    /// with others loses X, a record shared only with X comes home -- then
    /// X's membership removed, X's collection deleted, and X's pin forgotten.
    #[test]
    fn forgetting_a_person_stops_every_share_removes_them_and_deletes_their_collection() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", crate::sharing_state::Pin { sha256: "ab".into(), user_id: "anna-id".into(), at: 1 });
        }).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com"), person_collection("c-bob", "bob@example.com")]);
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"])]);
        with_collection_items(&mut server, "c-anna", &["s1", "s2"]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Both", &["c-anna", "c-bob"]).to_string()).create();
        server.mock("GET", "/api/ciphers/s2").with_body(shared_cipher("s2", "Only anna", &["c-anna"]).to_string()).create();
        let off = server.mock("PUT", "/api/ciphers/s1/collections").match_body(Matcher::Json(serde_json::json!({ "collectionIds": ["c-bob"] })))
            .with_body(shared_cipher("s1", "Both", &["c-bob"]).to_string()).expect(1).create();
        server.mock("POST", "/api/ciphers").with_body_from_request(echo("p2", None, &[])).expect(1).create();
        server.mock("PUT", "/api/ciphers/s2/collections").with_body(shared_cipher("s2", "Only anna", &[]).to_string()).create();
        server.mock("DELETE", "/api/ciphers/s2").with_status(204).create();
        let member = server.mock("DELETE", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id").as_str()).with_status(204).expect(1).create();
        let collection = server.mock("DELETE", format!("/api/organizations/{FIXTURE_ORG}/collections/c-anna").as_str()).with_status(204).expect(1).create();
        let items: Vec<VaultItem> = [shared_cipher("s1", "Both", &["c-anna", "c-bob"]), shared_cipher("s2", "Only anna", &["c-anna"])]
            .iter()
            .map(|raw| serde_json::from_value(serde_json::json!({ "id": raw["id"], "name": "x", "organizationId": raw["organizationId"], "collectionIds": raw["collectionIds"] })).expect("an item"))
            .collect();
        let outcome = engine.forget("anna@example.com", &items).expect("forgotten");
        off.assert();
        member.assert();
        collection.assert();
        assert_eq!(outcome.stopped, ["s1", "s2"]);
        assert!(outcome.changed.removed.contains(&"s2".to_string()));
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.pin_for("anna@example.com").is_none());
    }

    /// **Re-invite** (official-path spec §3.4): a pending invitation is
    /// renewed with `POST …/users/{ouId}/reinvite`; a person who is gone is
    /// invited again with Bitwarden's invite.
    #[test]
    fn reinviting_renews_an_invitation_and_invites_a_gone_person_again() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-carol", "carol@example.com"), person_collection("c-gus", "gus@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"])]))
            .expect(3)
            .create();
        with_roster(
            &mut server,
            &[roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"]), roster_row("inv-gus", None, "gus@example.com", 0, &["c-gus"])],
        );
        let renewed = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/inv-carol/reinvite").as_str()).with_body("").expect(1).create();
        assert_eq!(engine.reinvite("carol@example.com").expect("renewed"), ShareResult::Invited);
        renewed.assert();

        let invited = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str())
            .match_body(Matcher::PartialJson(serde_json::json!({ "emails": ["gus@example.com"] })))
            .with_body("")
            .expect(1)
            .create();
        assert_eq!(engine.reinvite("gus@example.com").expect("invited"), ShareResult::Invited);
        invited.assert();
    }

    /// `GET .../collections/{collection}/items` answers `ids`: what the
    /// server says the collection holds, the trash included.
    fn with_collection_items(server: &mut MockServer, collection: &str, ids: &[&str]) {
        let rows: Vec<serde_json::Value> =
            ids.iter().map(|id| serde_json::json!({ "id": id, "collectionId": collection, "object": "collectionItem" })).collect();
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/collections/{collection}/items").as_str())
            .with_body(serde_json::json!({ "object": "list", "data": rows, "continuationToken": null }).to_string())
            .expect_at_least(0)
            .create();
    }

    /// The daemon's items for these server records, each named by its id:
    /// what `forget` adds to the server's list of a person's collection.
    fn items_of(records: &[serde_json::Value]) -> Vec<VaultItem> {
        records
            .iter()
            .map(|raw| {
                serde_json::from_value(serde_json::json!({
                    "id": raw["id"], "name": raw["id"], "organizationId": raw["organizationId"], "collectionIds": raw["collectionIds"]
                }))
                .expect("an item")
            })
            .collect()
    }

    /// Anna, pinned and confirmed, with her own collection and bob's beside
    /// it; `s1` (an attachment, filed for anna alone -- it cannot come home)
    /// and `s2` (anna and bob) on the server; `s2`'s unfile for anna answers.
    /// Returns `s1` and `s2` as the server has them, and the unfile.
    fn anna_on_a_stuck_record_and_a_shared_one(server: &mut MockServer, dir: &crate::test_scratch::ScratchDir) -> ([serde_json::Value; 2], crate::test_http::Mock) {
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", crate::sharing_state::Pin { sha256: "ab".into(), user_id: "anna-id".into(), at: 1 });
        }).expect("seeded");
        with_collections(server, &[person_collection("c-anna", "anna@example.com"), person_collection("c-bob", "bob@example.com")]);
        with_roster(server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"])]);
        with_collection_items(server, "c-anna", &["s1", "s2"]);
        let mut stuck = shared_cipher("s1", "Has a file", &["c-anna"]);
        stuck["attachments"] = serde_json::json!([{ "id": "a1", "fileName": "2.x|y|z" }]);
        let both = shared_cipher("s2", "Both", &["c-anna", "c-bob"]);
        server.mock("GET", "/api/ciphers/s1").with_body(stuck.to_string()).create();
        server.mock("GET", "/api/ciphers/s2").with_body(both.to_string()).create();
        let off = server
            .mock("PUT", "/api/ciphers/s2/collections")
            .match_body(Matcher::Json(serde_json::json!({ "collectionIds": ["c-bob"] })))
            .with_body(shared_cipher("s2", "Both", &["c-bob"]).to_string())
            .expect(1)
            .create();
        ([stuck, both], off)
    }

    /// **A forget that cannot stop every record keeps the person** -- this
    /// build's answer to a stop failing part-way (spec 6.8 leaves it open).
    /// Every record is tried: the one that will not stop (an attachment
    /// cannot come home) does not keep the others shared. But while anna can
    /// still see it, nothing makes the forget look done: no membership
    /// removed, no collection deleted, the pin kept, and a refusal that names
    /// the one record and carries its own reason, so the user can act on it.
    /// What DID stop is in the outcome.
    #[test]
    fn a_forget_that_cannot_stop_one_record_keeps_the_person_and_says_so() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let (records, off) = anna_on_a_stuck_record_and_a_shared_one(&mut server, &dir);
        let creates = server.mock("POST", Matcher::Any).expect(0).create();
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        let outcome = engine.forget("anna@example.com", &items_of(&records)).expect("the stops were tried");
        off.assert();
        creates.assert();
        deletes.assert();
        assert_eq!(outcome.stopped, ["s2"], "the stuck record is not reported stopped");
        assert_eq!(outcome.changed.upserted.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["s2"]);
        match &outcome.failed {
            Some(SharingError::Refused(said)) => {
                assert_eq!(said, &format!("Couldn't forget anna@example.com. Sharing \u{201c}s1\u{201d} couldn't be stopped. {MOVE_BACK_ATTACHMENTS}"));
            }
            other => panic!("expected the forget's refusal, got {other:?}"),
        }
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.pin_for("anna@example.com").is_some(), "a person who can still see a record keeps their pin");
    }

    /// **A membership the server will not remove leaves the forget
    /// unfinished, in words that are true**: every record has stopped, so
    /// the sentence says sharing has. The collection and the pin wait for the
    /// Retry, which is the same call and finishes it.
    #[test]
    fn a_membership_that_will_not_go_says_sharing_stopped_and_a_retry_finishes() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let state_path = dir.join("sharing_state.json");
        crate::sharing_state::update(&state_path, |state| {
            state.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", crate::sharing_state::Pin { sha256: "ab".into(), user_id: "anna-id".into(), at: 1 });
        }).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"])]);
        with_collection_items(&mut server, "c-anna", &[]);
        let member_path = format!("/api/organizations/{FIXTURE_ORG}/users/anna-id");
        let collection_path = format!("/api/organizations/{FIXTURE_ORG}/collections/c-anna");
        let refused = server.mock("DELETE", member_path.as_str()).with_status(500).expect(1).create();
        let kept = server.mock("DELETE", collection_path.as_str()).with_status(204).expect(0).create();
        let outcome = engine.forget("anna@example.com", &[]).expect("tried");
        refused.assert();
        kept.assert();
        match &outcome.failed {
            Some(SharingError::Refused(said)) => assert_eq!(
                said,
                "Sharing with anna@example.com has stopped, but Deskwarden couldn't finish forgetting them. Try again."
            ),
            other => panic!("expected the unfinished sentence, got {other:?}"),
        }
        assert!(crate::sharing_state::SharingState::load(&state_path).account(FIXTURE_USER_ID).pin_for("anna@example.com").is_some());

        let removed = server.mock("DELETE", member_path.as_str()).with_status(204).expect(1).create();
        let deleted = server.mock("DELETE", collection_path.as_str()).with_status(204).expect(1).create();
        let outcome = engine.forget("anna@example.com", &[]).expect("finished");
        removed.assert();
        deleted.assert();
        assert!(outcome.failed.is_none(), "{:?}", outcome.failed);
        assert!(crate::sharing_state::SharingState::load(&state_path).account(FIXTURE_USER_ID).pin_for("anna@example.com").is_none());
    }

    /// **The account's own address is never forgotten**: its membership is
    /// the org's owner. Refused before anything is written.
    #[test]
    fn forgetting_yourself_is_refused_and_nothing_is_written() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[]);
        with_roster(&mut server, &[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]);
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        assert!(matches!(engine.forget(&FIXTURE_EMAIL.to_uppercase(), &[]), Err(SharingError::Refused(s)) if s == YOURSELF));
        deletes.assert();
    }

    /// **Through the daemon, what a refused forget DID stop is still filed
    /// and announced** -- why `ForgetOutcome` carries its refusal instead of
    /// the call returning it: an early `Err` would drop the stops that had
    /// landed, and the daemon would go on showing anna on a record she has
    /// lost. The route still answers the refusal.
    #[test]
    fn a_refused_forget_through_the_daemon_still_announces_what_it_stopped() {
        let (mut server, cache, api, token) = crate::vault_api::tests::a_daemon();
        let engine = cache.bridge().sharing().expect("an engine");
        let dir = pinned_dir("sharing-forget-daemon");
        engine.set_up(SharingSetup { state_path: Some(dir.join("sharing_state.json")), make_key_pair: fixture_key_pair, ..SharingSetup::none() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([sharing_org_row()])).to_string()).create();
        server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .create();
        let (records, off) = anna_on_a_stuck_record_and_a_shared_one(&mut server, &dir);
        assert!(cache.apply_written_elsewhere(&items_of(&records), &[], cache.epoch().era()), "seeded");
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        let answer = crate::vault_api::tests::ask(&api, "POST", "/sharing/person/forget", Some(&token), r#"{"email":"anna@example.com"}"#);
        off.assert();
        deletes.assert();
        assert_eq!(answer.status, 400, "{}", answer.body);
        assert!(answer.body.contains(MOVE_BACK_ATTACHMENTS), "{}", answer.body);
        let changes = api.feed().wait_after(Some(0), false, std::time::Duration::ZERO).changes;
        use crate::change_feed::Change;
        use crate::rest::notifications::Announcement;
        assert!(
            matches!(changes.as_slice(), [Change::Vault(Announcement::Item { id, .. }), Change::Sharing] if id == "s2"),
            "{changes:?}"
        );
        let s2 = cache.get_by_id("s2").expect("s2 is held");
        assert_eq!(collection_ids_of(&s2), ["c-bob"], "the daemon no longer shows anna on s2");
    }

    // ---- review round 1: what the server says the collection holds --------------

    /// Anna, confirmed, alone in her collection, which the server says holds
    /// `ids`; bob's collection beside it.
    fn anna_holds(server: &mut MockServer, ids: &[&str]) {
        with_collections(server, &[person_collection("c-anna", "anna@example.com"), person_collection("c-bob", "bob@example.com")]);
        with_roster(server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 2, &["c-anna"])]);
        with_collection_items(server, "c-anna", ids);
    }

    /// **A trashed record the daemon never holds is still found, and keeps
    /// the person** (review of Task 11). The daemon's copy has no trash, so
    /// the record is known only from the server's list of anna's collection.
    /// Its stop refuses -- it would come home live -- and so nothing is
    /// deleted: not her membership, not her collection (which would leave
    /// the record in the org under a collection that no longer exists). The
    /// refusal names it, from the server, and says what to do.
    #[test]
    fn a_trashed_record_the_daemon_never_holds_keeps_the_person_and_deletes_nothing() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        anna_holds(&mut server, &["s1"]);
        let mut trashed = shared_cipher("s1", "Acme VPN", &["c-anna"]);
        trashed["deletedDate"] = serde_json::json!("2026-09-01T00:00:00.000Z");
        server.mock("GET", "/api/ciphers/s1").with_body(trashed.to_string()).expect_at_least(1).create();
        let posts = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        let outcome = engine.forget("anna@example.com", &[]).expect("the stop was tried");
        posts.assert();
        puts.assert();
        deletes.assert();
        assert!(outcome.stopped.is_empty());
        match &outcome.failed {
            Some(SharingError::Refused(said)) => assert_eq!(
                said,
                &format!("Couldn't forget anna@example.com. Sharing \u{201c}Acme VPN\u{201d} couldn't be stopped. {STOP_IN_TRASH}")
            ),
            other => panic!("expected the trash refusal, got {other:?}"),
        }
    }

    /// **A live record the daemon does not hold still comes home**, from the
    /// server's list alone; and a record the daemon's stale copy files for
    /// anna that the server no longer does (`s9`, personal now) is tried,
    /// changes nothing, and is not reported stopped.
    #[test]
    fn a_record_the_daemon_does_not_hold_still_comes_home() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        anna_holds(&mut server, &["s2"]);
        server.mock("GET", "/api/ciphers/s2").with_body(shared_cipher("s2", "Only anna", &["c-anna"]).to_string()).create();
        server.mock("GET", "/api/ciphers/s9").with_body(personal_cipher("s9", "Went home").to_string()).expect(1).create();
        let home = server.mock("POST", "/api/ciphers").with_body_from_request(echo("p2", None, &[])).expect(1).create();
        server.mock("PUT", "/api/ciphers/s2/collections").with_body(shared_cipher("s2", "Only anna", &[]).to_string()).create();
        let purged = server.mock("DELETE", "/api/ciphers/s2").with_status(204).expect(1).create();
        let member = server.mock("DELETE", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id").as_str()).with_status(204).expect(1).create();
        let collection = server.mock("DELETE", format!("/api/organizations/{FIXTURE_ORG}/collections/c-anna").as_str()).with_status(204).expect(1).create();
        let stale = items_of(&[shared_cipher("s9", "Went home", &["c-anna"])]);
        let outcome = engine.forget("anna@example.com", &stale).expect("forgotten");
        home.assert();
        purged.assert();
        member.assert();
        collection.assert();
        assert!(outcome.failed.is_none(), "{:?}", outcome.failed);
        assert_eq!(outcome.stopped, ["s2"], "s9 changed nothing");
        assert_eq!(outcome.changed.removed, ["s2"]);
        assert_eq!(outcome.changed.upserted.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["p2"]);
    }

    /// **Several records that will not stop are counted**, not listed.
    #[test]
    fn several_records_that_will_not_stop_are_counted() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        anna_holds(&mut server, &["s1", "s3"]);
        let mut trashed = shared_cipher("s1", "Acme VPN", &["c-anna"]);
        trashed["deletedDate"] = serde_json::json!("2026-09-01T00:00:00.000Z");
        let mut filed = shared_cipher("s3", "Has a file", &["c-anna"]);
        filed["attachments"] = serde_json::json!([{ "id": "a1", "fileName": "2.x|y|z" }]);
        server.mock("GET", "/api/ciphers/s1").with_body(trashed.to_string()).create();
        server.mock("GET", "/api/ciphers/s3").with_body(filed.to_string()).create();
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        let outcome = engine.forget("anna@example.com", &[]).expect("the stops were tried");
        deletes.assert();
        match &outcome.failed {
            Some(SharingError::Refused(said)) => {
                assert_eq!(said, "Couldn't forget anna@example.com. Sharing couldn't be stopped on 2 records.");
            }
            other => panic!("expected the counted refusal, got {other:?}"),
        }
    }

    /// **A session that ends mid-forget ends the loop at once**: its own
    /// error comes back (the window answers it with sign-in, not a retry),
    /// no later record is even read, and nothing is deleted.
    #[test]
    fn a_session_that_ends_mid_forget_stops_the_loop_at_once() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        anna_holds(&mut server, &["s1", "s2"]);
        server.mock("GET", "/api/ciphers/s1").with_status(401).expect_at_least(1).create();
        let later = server.mock("GET", "/api/ciphers/s2").expect(0).create();
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        let outcome = engine.forget("anna@example.com", &[]).expect("the loop ran");
        later.assert();
        deletes.assert();
        assert!(outcome.stopped.is_empty());
        assert!(matches!(outcome.failed, Some(SharingError::Vault(VaultError::Unauthorized))), "{:?}", outcome.failed);
    }

    /// **Nobody to forget is not a failure**: an account that never shared
    /// has no sharing org, and a server without sharing has none either --
    /// the window's ✕ on a Sends-only contact must not fail. Nothing is
    /// written, and the outcome is empty.
    #[test]
    fn forgetting_with_no_sharing_org_or_no_sharing_is_nothing_to_do() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        let deletes = server.mock("DELETE", Matcher::Any).expect(0).create();
        let outcome = engine.forget("anna@example.com", &[]).expect("nothing to forget");
        deletes.assert();
        assert!(outcome.stopped.is_empty() && outcome.changed.is_empty() && outcome.failed.is_none());

        let (mut official, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        engine.set_up(SharingSetup::none());
        official.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        let deletes = official.mock("DELETE", Matcher::Any).expect(0).create();
        let outcome = engine.forget("anna@example.com", &[]).expect("nothing to forget off NodeWarden");
        deletes.assert();
        assert!(outcome.stopped.is_empty() && outcome.failed.is_none());
    }

    // ---- confirming, Share anyway, Show fingerprint (Task 12) -------------------

    fn anna_key() -> String {
        serde_json::json!({ "userId": "anna-id", "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string()
    }

    /// **An accepted person is confirmed by the timer, with no sync** (spec
    /// 11.5 trigger 4): the timer reads the roster itself -- its one request
    /// of its own -- then two requests per person: the key, the confirm.
    #[test]
    fn an_accepted_person_is_confirmed_by_the_timer_without_a_sync() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let sync = server.mock("GET", "/api/sync?excludeDomains=true").expect(0).create();
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        engine.held().my_org = Some(MyOrg { id: FIXTURE_ORG.into(), collections: vec![authentic("c-anna", "anna@example.com")], roster: Vec::new() });
        let roster_read = server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]))
            .expect(1)
            .create();
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).expect(1).create();
        let confirm = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str())
            .match_request(|request| request.utf8_lossy_body().contains("\"key\":\"4."))
            .with_body("")
            .expect(1)
            .create();
        assert!(!engine.wants_timer(), "control: nobody is known to be waiting before the roster is read");
        let pass = engine.refresh(Trigger::Timer).expect("a pass");
        roster_read.assert();
        confirm.assert();
        sync.assert();
        assert!(pass.changed);
        let pin = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(pin.pin_for("anna@example.com").is_some(), "the first confirm pins the key");
        assert_eq!(engine.view(&[]).person("anna@example.com").map(|p| p.state), Some(PersonState::Shared));
        assert!(!engine.wants_timer(), "a confirmed person gives the timer nothing to do");
    }

    /// **The confirm pass reuses the roster the whole read fetched** (spec
    /// 11.5): after `note_directory`, no roster request of its own.
    #[test]
    fn the_confirm_pass_reuses_the_roster_the_sync_fetched() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-anna", "anna@example.com")],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        let roster_read = server.mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str()).expect(0).create();
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).expect(1).create();
        let confirm = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str()).with_body("").expect(1).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        roster_read.assert();
        confirm.assert();
    }

    /// **A changed public key is not confirmed** (spec 11.5 rule 4, 12.1):
    /// the pin differs, so no key is wrapped to it and the card says so.
    #[test]
    fn a_changed_public_key_is_not_confirmed() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", crate::sharing_state::Pin { sha256: "00".repeat(32), user_id: "anna-id".into(), at: 1 });
        }).expect("seeded");
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-anna", "anna@example.com")],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        let confirm = server.mock("POST", Matcher::Any).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        confirm.assert();
        assert_eq!(engine.view(&[]).person("anna@example.com").map(|p| p.state), Some(PersonState::KeyChanged));
    }

    /// **A member whose email has no collection is never confirmed** (spec
    /// 11.5 rule 2): a server-injected accept. Only a holder of the org key
    /// can write a collection name that decrypts, so the server alone cannot
    /// put a stranger in the confirm queue.
    #[test]
    fn a_member_whose_email_has_no_collection_is_never_confirmed() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-anna", "anna@example.com")],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("mal-id", Some("mal-id"), "mallory@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        let key = server.mock("GET", "/api/users/mal-id/public-key").expect(0).create();
        let confirm = server.mock("POST", Matcher::Any).expect(0).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        key.assert();
        confirm.assert();
    }

    /// An accepted account with no key yet waits for its first sign-in.
    #[test]
    fn an_account_without_a_key_waits_for_a_first_sign_in() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-anna", "anna@example.com")],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        server.mock("GET", "/api/users/anna-id/public-key").with_status(404).create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        assert_eq!(engine.view(&[]).person("anna@example.com").map(|p| p.state), Some(PersonState::WaitingSignIn));
    }

    /// **"Share anyway"** after a key change: confirmed to the new key, and
    /// the pin follows it.
    #[test]
    fn share_anyway_confirms_the_new_key_and_repins_it() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", crate::sharing_state::Pin { sha256: "00".repeat(32), user_id: "anna-id".into(), at: 1 });
        }).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]);
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        let confirm = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str()).with_body("").expect(1).create();
        let fingerprinted = crate::rest::crypto::public_key_sha256_hex(&crate::rest::crypto::tests::fixture_public_spki());
        engine.confirm_anyway("anna@example.com", &fingerprinted).expect("confirmed");
        confirm.assert();
        let pin = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(
            pin.pin_for("anna@example.com").map(|p| p.sha256.clone()),
            Some(crate::rest::crypto::public_key_sha256_hex(&crate::rest::crypto::tests::fixture_public_spki()))
        );
    }

    /// **Show fingerprint** (spec 9.6): the phrase the person's own app shows,
    /// from their user id and the key the server holds for them -- pinned to
    /// Bitwarden's SDK vector by serving that user and key.
    #[test]
    fn a_persons_fingerprint_is_the_phrase_their_own_app_shows() {
        use crate::rest::crypto::tests::{SDK_FINGERPRINT_KEY, SDK_FINGERPRINT_PHRASE, SDK_FINGERPRINT_USER};
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[roster_row("anna-ou", Some(SDK_FINGERPRINT_USER), "anna@example.com", 2, &["c-anna"])]);
        server
            .mock("GET", format!("/api/users/{SDK_FINGERPRINT_USER}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(SDK_FINGERPRINT_KEY) }).to_string())
            .create();
        let fingerprint = engine.fingerprint("anna@example.com").expect("a phrase");
        assert_eq!(fingerprint.phrase, SDK_FINGERPRINT_PHRASE);
        assert_eq!(fingerprint.key_sha256, crate::rest::crypto::public_key_sha256_hex(SDK_FINGERPRINT_KEY));
    }

    /// **One person's refused confirm does not hold up the next**: the pass
    /// logs it and goes on, and the one that landed is still announced --
    /// with `?` in the loop it was dropped, and no `Sharing` change told the
    /// window that bob is in.
    #[test]
    fn one_refused_confirm_does_not_stop_the_next_person() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-anna", "anna@example.com"), authentic("c-bob", "bob@example.com")],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [
                roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"]),
                roster_row("bob-id", Some("bob-id"), "bob@example.com", 1, &["c-bob"]),
            ] }))
            .expect("a roster"),
        });
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        server.mock("GET", "/api/users/bob-id/public-key").with_body(anna_key()).create();
        server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str())
            .with_status(400)
            .with_body(r#"{"message":"Something else went wrong"}"#)
            .expect(1)
            .create();
        let bob = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/bob-id/confirm").as_str()).with_body("").expect(1).create();
        let pass = engine.refresh(Trigger::WholeRead).expect("a pass");
        bob.assert();
        assert!(pass.changed, "bob's confirm landed and was not announced");
        let view = engine.view(&[]);
        assert_eq!(view.person("anna@example.com").map(|p| p.state), Some(PersonState::WaitingAccept));
        assert_eq!(view.person("bob@example.com").map(|p| p.state), Some(PersonState::Shared));
    }

    /// **"Share anyway" confirms only someone this account shares with**: an
    /// accepted member with no person collection is refused, as the pass
    /// skips them (spec 11.5 rule 2), and nothing is wrapped to their key.
    #[test]
    fn share_anyway_refuses_a_member_with_no_collection() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[roster_row("mal-id", Some("mal-id"), "mallory@example.com", 1, &["c-anna"])]);
        let key = server.mock("GET", "/api/users/mal-id/public-key").expect(0).create();
        let confirm = server.mock("POST", Matcher::Any).expect(0).create();
        assert!(matches!(engine.confirm_anyway("mallory@example.com", &"11".repeat(32)), Err(SharingError::Refused(_))));
        key.assert();
        confirm.assert();
    }

    // ---- fix round 1: authenticated names, the checked key, unreadable pins ----

    /// **A plaintext collection name is not rule 2** (review of Task 12): the
    /// server can write `mallory@example.com` in plaintext itself, beside an
    /// accepted row carrying its own key. Only a name that decrypted under
    /// the org key lets "Share anyway" hand the org key over. The pass's half
    /// is `vault_host`'s `a_plaintext_collection_name_never_confirms_anyone`,
    /// which reads the name through a real whole read.
    #[test]
    fn share_anyway_refuses_a_plaintext_collection_name() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[serde_json::json!({ "id": "c-mal", "organizationId": FIXTURE_ORG, "name": "mallory@example.com" })]);
        with_roster(&mut server, &[roster_row("mal-id", Some("mal-id"), "mallory@example.com", 1, &["c-mal"])]);
        let key = server.mock("GET", "/api/users/mal-id/public-key").expect(0).create();
        let confirm = server.mock("POST", Matcher::Any).expect(0).create();
        let fingerprinted = crate::rest::crypto::public_key_sha256_hex(&crate::rest::crypto::tests::fixture_public_spki());
        assert!(matches!(engine.confirm_anyway("mallory@example.com", &fingerprinted), Err(SharingError::Refused(_))));
        key.assert();
        confirm.assert();
        // And the pass, over the same names read into `held` by a reload.
        let ctx = engine.context().expect("a context");
        assert!(!ctx.collections_for("mallory@example.com").is_empty() && ctx.authenticated_collection_for("mallory@example.com").is_none());
        engine.reload_org(&ctx).expect("reloaded");
        let pass = engine.refresh(Trigger::Timer).expect("a pass");
        key.assert();
        confirm.assert();
        assert!(!pass.changed);
    }

    /// **A key under 2048 bits is never wrapped to** (final review, item 6):
    /// the server chooses which key an account answers with, and the org key
    /// wrapped to a short one is not a secret. The person is invited --
    /// nothing about the address is wrong -- but the confirm at once refuses
    /// the key, so nobody is confirmed and nothing is pinned.
    #[test]
    fn a_share_to_a_short_key_confirms_nobody() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(1)
            .create();
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]);
        let short = {
            use rsa::pkcs8::EncodePublicKey;
            let private = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 1024).expect("a 1024-bit key");
            private.to_public_key().to_public_key_der().expect("spki").as_bytes().to_vec()
        };
        server
            .mock("GET", "/api/users/anna-id/public-key")
            .with_body(serde_json::json!({ "userId": "anna-id", "publicKey": crate::rest::crypto::tests::base64(&short) }).to_string())
            .create();
        server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-anna"])).create();
        let confirm = server.mock("POST", Matcher::Regex("/confirm$".into())).expect(0).create();
        let outcome = engine.share("p1", "anna@example.com").expect("shared, not confirmed");
        confirm.assert();
        assert_eq!(outcome.result, ShareResult::WaitingAccept);
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.pin_for("anna@example.com").is_none(), "a refused key is not pinned");
    }

    /// **An expired invitation gives the timer nothing to do** (final
    /// review): nobody can accept it, so only a Re-invite moves it. A live
    /// one still does.
    #[test]
    fn an_expired_invitation_does_not_keep_the_timer_running() {
        let engine = offline();
        let set = |expires: &str| {
            engine.held().my_org = Some(MyOrg {
                id: "org1".into(),
                collections: vec![authentic("c-dan", "dan@x.io")],
                roster: vec![member("dan@x.io", MemberStatus::Invited, &["c-dan"], Some(expires))],
            });
        };
        set("2000-01-01T00:00:00Z");
        assert!(!engine.wants_timer(), "an expired invitation");
        set("2999-01-01T00:00:00Z");
        assert!(engine.wants_timer(), "control: a live invitation still wants the timer");
    }

    /// **An account with no key yet is waiting to accept, not to create an
    /// account**: the server accepted it at invite, the confirm at once finds
    /// no key, and the view says it waits for a first sign-in.
    #[test]
    fn an_accepted_account_with_no_key_waits_for_a_first_sign_in() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-kim", "kim@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(1)
            .create();
        with_roster(&mut server, &[roster_row("kim-ou", Some("kim-id"), "kim@example.com", 1, &["c-kim"])]);
        server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").expect(1).create();
        server.mock("GET", "/api/users/kim-id/public-key").with_status(404).create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-kim"])).create();
        let outcome = engine.share("p1", "kim@example.com").expect("shared");
        assert_eq!(outcome.result, ShareResult::WaitingAccept, "an existing account is never told to create one");
        assert_eq!(engine.view(&[]).person("kim@example.com").map(|p| p.state), Some(PersonState::WaitingSignIn));
    }

    // ---- an org is mine only when made here or adopted (official-path spec §3.1, fix round 1) ----

    fn seed_org(dir: &crate::test_scratch::ScratchDir, created: bool, adopted: bool) {
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            let me = state.account_mut(FIXTURE_USER_ID);
            me.sharing_org_id = Some(FIXTURE_ORG.into());
            me.sharing_org_created = created;
            me.sharing_org_adopted = adopted;
        })
        .expect("seeded");
    }

    fn cache_org(dir: &crate::test_scratch::ScratchDir, created: bool) {
        seed_org(dir, created, false);
    }

    fn cached(dir: &crate::test_scratch::ScratchDir) -> (Option<String>, bool) {
        let me = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        (me.sharing_org_id, me.sharing_org_created)
    }

    fn adopted(dir: &crate::test_scratch::ScratchDir) -> bool {
        crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID).sharing_org_adopted
    }

    /// The fixture account's own row: owner, confirmed.
    fn me_row() -> serde_json::Value {
        roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])
    }

    /// `row` in another role, as an official server writes it: `type` 0
    /// owner, 1 admin, 3 manager, 4 custom.
    fn in_role(mut row: serde_json::Value, kind: i64) -> serde_json::Value {
        row["type"] = serde_json::json!(kind);
        row
    }

    /// An org named for and owned by the fixture account that this client
    /// did NOT make, with `rows` on its roster and `collections` in it -- and
    /// a personal record `p1` to share.
    fn unpinned_org(rows: &[serde_json::Value], collections: &[serde_json::Value]) -> (MockServer, crate::rest::backend::RestBackend, Arc<Sharing>, crate::test_scratch::ScratchDir) {
        let (mut server, backend, engine, dir) = on_nodewarden_unpinned(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, collections);
        with_roster(&mut server, rows);
        (server, backend, engine, dir)
    }

    /// **The share goes to a fresh org of its own**: the unpinned org is not
    /// offered, nothing is written to it, and nothing is filed into it.
    fn shares_into_a_fresh_org(server: &mut MockServer, engine: &Sharing, dir: &crate::test_scratch::ScratchDir, case: &str) {
        let planted_writes = server.mock("POST", Matcher::Regex(format!("^/api/organizations/{FIXTURE_ORG}/"))).expect(0).create();
        let filed = server.mock("PUT", "/api/ciphers/p1/share").match_request(|r| r.utf8_lossy_body().contains(FIXTURE_ORG)).expect(0).create();
        let fresh = server.mock("POST", "/api/organizations").with_body(r#"{"id":"org-new","object":"organization"}"#).expect(1).create();
        server.mock("GET", "/api/organizations/org-new/collections").with_body(r#"{"data":[]}"#).create();
        // What follows the create is not this test's business; the create is.
        let answer = engine.share("p1", "anna@example.com");
        assert!(!matches!(answer, Err(SharingError::NeedsAdoption(_))), "{case}: the user was offered an org that fails the roster rule");
        fresh.assert();
        planted_writes.assert();
        filed.assert();
        assert_eq!(cached(dir), (Some("org-new".into()), true), "{case}: the fresh org is remembered as made here");
    }

    /// **C1: another account in the owner, admin, manager or custom role
    /// makes an org not this account's** -- whoever holds one of those reads
    /// every collection, and an attacker who made "Shared by <me>" can step
    /// down from owner to any of them and still read every share.
    #[test]
    fn an_unpinned_org_with_another_privileged_member_is_refused() {
        for kind in [0, 1, 3, 4] {
            let mallory = in_role(roster_row("mal-ou", Some("mal-id"), "mallory@evil.example", 2, &[]), kind);
            let (mut server, _backend, engine, dir) = unpinned_org(&[me_row(), mallory], &[]);
            shares_into_a_fresh_org(&mut server, &engine, &dir, &format!("type {kind}"));
        }
    }

    /// **C1: a member who reaches any collection but their own** -- the
    /// attacker kept a plain member seat on a person collection they made --
    /// is refused; so is a member with edit rights on their own, and one on
    /// a collection whose name did not decrypt under the org key.
    #[test]
    fn an_unpinned_org_with_a_member_beyond_their_own_read_only_collection_is_refused() {
        let anna = person_collection("c-anna", "anna@example.com");
        let on_annas = roster_row("mal-ou", Some("mal-id"), "mallory@evil.example", 2, &["c-anna"]);
        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row(), on_annas], std::slice::from_ref(&anna));
        shares_into_a_fresh_org(&mut server, &engine, &dir, "a member on someone else's collection");

        let mut writable = roster_row("anna-ou", Some("anna-id"), "anna@example.com", 2, &["c-anna"]);
        writable["collections"][0]["readOnly"] = serde_json::json!(false);
        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row(), writable], &[anna]);
        shares_into_a_fresh_org(&mut server, &engine, &dir, "a member who can edit their own collection");

        let plain = roster_row("mal-ou", Some("mal-id"), "mallory@evil.example", 2, &["c-mal"]);
        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row(), plain], &[plaintext_collection("c-mal", "mallory@evil.example")]);
        shares_into_a_fresh_org(&mut server, &engine, &dir, "a member on a collection named in plaintext");

        // Fix round 2, m3: read-only but MANAGE is not "held read-only" -- a
        // manager of a collection edits who reaches it.
        let mut managing = roster_row("anna-ou", Some("anna-id"), "anna@example.com", 2, &["c-anna"]);
        managing["collections"][0]["manage"] = serde_json::json!(true);
        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row(), managing], &[person_collection("c-anna", "anna@example.com")]);
        shares_into_a_fresh_org(&mut server, &engine, &dir, "a member who manages their own collection");
    }

    /// **C1: an invitation in the admin role is an admin** -- accepted
    /// later, it reads everything -- and so is one revoked or in any status.
    /// And an org where this account is not itself a confirmed owner on the
    /// roster is not its own, whatever the profile says.
    #[test]
    fn an_unpinned_org_with_an_invited_admin_or_an_unconfirmed_me_is_refused() {
        let invited = in_role(roster_row("inv-x", None, "mallory@evil.example", 0, &[]), 1);
        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row(), invited], &[]);
        shares_into_a_fresh_org(&mut server, &engine, &dir, "an invited admin");

        let accepted_me = roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 1, &[]);
        let (mut server, _backend, engine, dir) = unpinned_org(&[accepted_me], &[]);
        shares_into_a_fresh_org(&mut server, &engine, &dir, "this account only accepted");

        let admin_me = in_role(me_row(), 1);
        let (mut server, _backend, engine, dir) = unpinned_org(&[admin_me], &[]);
        shares_into_a_fresh_org(&mut server, &engine, &dir, "this account an admin");

        // Fix round 2, m1: official Bitwarden's revoked is status -1, which
        // is not "confirmed" -- for this account's own row, and an admin row
        // stays an admin row revoked or not.
        let revoked_me = roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, -1, &[]);
        let (mut server, _backend, engine, dir) = unpinned_org(&[revoked_me], &[]);
        shares_into_a_fresh_org(&mut server, &engine, &dir, "this account revoked");

        let revoked_admin = in_role(roster_row("mal-ou", Some("mal-id"), "mallory@evil.example", -1, &[]), 1);
        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row(), revoked_admin], &[]);
        shares_into_a_fresh_org(&mut server, &engine, &dir, "a revoked admin");
    }

    /// **m1: a revoked recipient is not Shared on the card**: revoked, they
    /// reach nothing until restored, so the card says Gone, and they do not
    /// keep the ten-minute timer reading the roster.
    #[test]
    fn a_revoked_recipient_is_not_shown_as_shared() {
        let engine = offline();
        let ivy = |status: i64| {
            crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("ivy-ou", Some("ivy-id"), "ivy@x.io", status, &["c-ivy"])] }))
                .expect("a roster")
        };
        {
            let mut held = engine.held();
            held.capabilities.loaded = true;
            held.account = Some(Account { id: "me".into(), email: "me@x.io".into(), admin: false, keyed: true });
            held.my_org = Some(MyOrg { id: "org1".into(), collections: vec![authentic("c-ivy", "ivy@x.io")], roster: ivy(-1) });
        }
        assert_eq!(engine.view(&[]).person("ivy@x.io").map(|p| p.state), Some(PersonState::Gone));
        assert!(!engine.wants_timer(), "a revoked member kept the timer running");
        engine.held().my_org.as_mut().expect("held").roster = ivy(2);
        assert_eq!(engine.view(&[]).person("ivy@x.io").map(|p| p.state), Some(PersonState::Shared), "control");
    }

    /// **m1: a share to a revoked person invites them again** -- as for a
    /// member without access to their collection (decision 11): removed,
    /// then Bitwarden's invite, never "already shared".
    #[test]
    fn a_share_to_a_revoked_person_removes_and_invites_them_again() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[me_row(), roster_row("anna-ou", Some("anna-id"), "anna@example.com", -1, &["c-anna"])]))
            .expect(1)
            .create();
        with_roster(&mut server, &[me_row(), roster_row("inv-anna", None, "anna@example.com", 0, &["c-anna"])]);
        let removed = server.mock("DELETE", format!("/api/organizations/{FIXTURE_ORG}/users/anna-ou").as_str()).with_body("").expect(1).create();
        let invited = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").expect(1).create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-anna"])).create();
        let outcome = engine.share("p1", "anna@example.com").expect("shared");
        removed.assert();
        invited.assert();
        assert_eq!(outcome.result, ShareResult::Invited);
    }

    /// **m2: adopt takes only the org the last share asked about, once.** No
    /// question pending, or another id, is refused and pins nothing; Yes
    /// spends the question; No spends it too.
    #[test]
    fn adopt_takes_only_the_org_the_share_asked_about_and_only_once() {
        let not_asked = |answer: Result<(), SharingError>| matches!(answer, Err(SharingError::Refused(s)) if s == ADOPT_NOT_ASKED);
        let (_server, _backend, engine, dir) = unpinned_org(&[me_row()], &[]);
        assert!(not_asked(engine.adopt(FIXTURE_ORG)), "no question pending");
        assert!(matches!(engine.share("p1", "anna@example.com"), Err(SharingError::NeedsAdoption(_))));
        assert!(not_asked(engine.adopt("org-other")), "an org that was not asked about");
        assert_eq!(cached(&dir), (None, false));
        assert!(matches!(engine.share("p1", "anna@example.com"), Err(SharingError::NeedsAdoption(_))));
        engine.adopt(FIXTURE_ORG).expect("the org asked about");
        assert!(adopted(&dir));
        assert!(not_asked(engine.adopt(FIXTURE_ORG)), "the question is spent");

        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row()], &[]);
        assert!(matches!(engine.share("p1", "anna@example.com"), Err(SharingError::NeedsAdoption(_))));
        server.mock("POST", "/api/organizations").with_status(400).with_body(r#"{"message":"No."}"#).create();
        let _ = engine.share_in_a_new_org("p1", "anna@example.com");
        assert!(not_asked(engine.adopt(FIXTURE_ORG)), "No answered the question");
        assert_eq!(cached(&dir), (None, false));
    }

    /// **A second PC's roster -- only members, each on their own read-only
    /// collection, in any status -- asks the user, writes nothing, and Yes
    /// pins the org** (fix round 1, C1 rule 4).
    #[test]
    fn a_second_pcs_org_is_asked_about_and_yes_pins_it() {
        let rows = [
            me_row(),
            roster_row("anna-ou", Some("anna-id"), "anna@example.com", 2, &["c-anna"]),
            roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"]),
            roster_row("bob-ou", Some("bob-id"), "bob@example.com", 1, &[]),
        ];
        let collections = [person_collection("c-anna", "anna@example.com"), person_collection("c-carol", "carol@example.com")];
        let (mut server, _backend, engine, dir) = unpinned_org(&rows, &collections);
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::NeedsAdoption(question)) => {
                assert_eq!(question.organization_id, FIXTURE_ORG);
                assert_eq!(question.name, format!("Shared by {FIXTURE_EMAIL}"));
                let sentence = question.sentence();
                assert!(sentence.contains(FIXTURE_ORG) && sentence.contains(&question.name), "{sentence}");
                assert!(sentence.ends_with("Deskwarden did not create it."), "{sentence}");
            }
            other => panic!("expected the question, got {other:?}"),
        }
        writes.assert();
        puts.assert();
        assert_eq!(cached(&dir), (None, false), "asking pins nothing");
        assert!(engine.held().my_org.is_none(), "an org the user has not chosen is not held");

        engine.adopt(FIXTURE_ORG).expect("adopted");
        assert_eq!(cached(&dir), (Some(FIXTURE_ORG.into()), false), "pinned, and not as made here");
        assert!(adopted(&dir), "pinned as adopted");
    }

    /// **An org made in the web vault -- only this account on it -- is asked
    /// about too**: nothing on the roster says who made it.
    #[test]
    fn an_org_made_in_the_web_vault_is_asked_about() {
        let (_server, _backend, engine, dir) = unpinned_org(&[me_row()], &[]);
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::NeedsAdoption(question)) => assert_eq!(question.organization_id, FIXTURE_ORG),
            other => panic!("expected the question, got {other:?}"),
        }
        assert_eq!(cached(&dir), (None, false));
    }

    /// **Once adopted, an org is used as a made one is**: no question, no
    /// roster test, and a whole read holds it for the confirm pass.
    #[test]
    fn an_adopted_org_is_used_without_asking_again() {
        let (mut server, _backend, engine, dir) = on_nodewarden_unpinned(serde_json::json!([sharing_org_row()]));
        seed_org(&dir, false, true);
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        anna_is_confirmed(&mut server);
        let (_, shared) = share_captured(&mut server, "p1");
        let outcome = engine.share("p1", "anna@example.com").expect("shared, not asked");
        shared.assert();
        assert_eq!(outcome.result, ShareResult::Shared);

        engine.held().my_org = None;
        let profile: Profile = serde_json::from_value(sharing_profile(serde_json::json!([sharing_org_row()]))).expect("profile");
        let directory = crate::rest::organizations::Directory::from_sync(
            &serde_json::from_value(serde_json::json!({ "profile": sharing_profile(serde_json::json!([sharing_org_row()])), "ciphers": [], "folders": [], "collections": [] })).expect("sync"),
            &engine.keys_from(&profile).expect("keys"),
        );
        engine.note_directory(&directory, Some(&profile));
        assert_eq!(engine.held().my_org.as_ref().map(|o| o.id.as_str()), Some(FIXTURE_ORG));
    }

    /// **No pins nothing, and the share goes on in a new org**; on a server
    /// that will not make one, the refusal says so and still pins nothing.
    #[test]
    fn no_pins_nothing_and_shares_in_a_new_org() {
        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row()], &[]);
        assert!(matches!(engine.share("p1", "anna@example.com"), Err(SharingError::NeedsAdoption(_))));
        let planted_writes = server.mock("POST", Matcher::Regex(format!("^/api/organizations/{FIXTURE_ORG}/"))).expect(0).create();
        let fresh = server.mock("POST", "/api/organizations").with_body(r#"{"id":"org-new","object":"organization"}"#).expect(1).create();
        server.mock("GET", "/api/organizations/org-new/collections").with_body(r#"{"data":[]}"#).create();
        // What follows the create is not this test's business; the create is.
        let _ = engine.share_in_a_new_org("p1", "anna@example.com");
        fresh.assert();
        planted_writes.assert();
        assert_eq!(cached(&dir), (Some("org-new".into()), true));
        assert!(!adopted(&dir), "No adopted nothing");

        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row()], &[]);
        server
            .mock("POST", "/api/organizations")
            .with_status(400)
            .with_body(r#"{"message":"You cannot create an organization."}"#)
            .create();
        match engine.share_in_a_new_org("p1", "anna@example.com") {
            Err(SharingError::Refused(sentence)) => assert!(sentence.starts_with(NO_FRESH_ORG), "{sentence}"),
            other => panic!("expected the refusal, got {other:?}"),
        }
        assert_eq!(cached(&dir), (None, false), "nothing pinned");
    }

    /// **I1: two orgs that both pass are refused, not guessed between** --
    /// unless one of them is pinned, which is then the answer.
    #[test]
    fn two_orgs_that_both_pass_are_refused_not_guessed_between() {
        let mut two = sharing_org_row();
        two["id"] = serde_json::json!("org-two");
        let orgs = serde_json::json!([sharing_org_row(), two]);
        let (mut server, _backend, engine, dir) = on_nodewarden_unpinned(orgs.clone());
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[]);
        with_roster(&mut server, &[me_row()]);
        server.mock("GET", "/api/organizations/org-two/collections").with_body(r#"{"data":[]}"#).create();
        server.mock("GET", "/api/organizations/org-two/users?includeCollections=true").with_body(roster(&[me_row()])).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, SEVERAL_ORGS),
            other => panic!("expected the refusal, got {other:?}"),
        }
        writes.assert();
        assert_eq!(cached(&dir), (None, false));

        seed_org(&dir, false, true);
        let profile: Profile = serde_json::from_value(sharing_profile(orgs)).expect("profile");
        let account = engine.held().account.clone().expect("an account");
        assert_eq!(engine.my_org_id(&profile, &account).expect("an answer").as_deref(), Some(FIXTURE_ORG), "the pinned one");
    }

    /// **I2: a roster that cannot be read forgets nothing.** A pinned org is
    /// used and held whatever its roster read did, and a whole read writes no
    /// state; a cached id that is not pinned, whose roster is unreadable, is
    /// kept and the share refuses rather than choosing.
    #[test]
    fn a_failed_roster_read_keeps_the_pinned_org() {
        let (mut server, _backend, engine, dir) = on_nodewarden_unpinned(serde_json::json!([sharing_org_row()]));
        seed_org(&dir, false, true);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(r#"{"message":"Not found"}"#)
            .create();
        let profile: Profile = serde_json::from_value(sharing_profile(serde_json::json!([sharing_org_row()]))).expect("profile");
        let account = engine.held().account.clone().expect("an account");
        assert_eq!(engine.my_org_id(&profile, &account).expect("an answer").as_deref(), Some(FIXTURE_ORG));

        let before = std::fs::read(dir.join("sharing_state.json")).expect("the state");
        let directory = crate::rest::organizations::Directory::from_sync(
            &serde_json::from_value(serde_json::json!({ "profile": sharing_profile(serde_json::json!([sharing_org_row()])), "ciphers": [], "folders": [], "collections": [] })).expect("sync"),
            &engine.keys_from(&profile).expect("keys"),
        );
        assert_eq!(directory.roster_of(FIXTURE_ORG), &Roster::Unknown, "control: this read has no roster");
        engine.note_directory(&directory, Some(&profile));
        assert_eq!(engine.held().my_org.as_ref().map(|o| o.id.as_str()), Some(FIXTURE_ORG), "held");
        assert_eq!(std::fs::read(dir.join("sharing_state.json")).expect("the state"), before, "a whole read wrote the sharing state");

        cache_org(&dir, false);
        engine.held().my_org = None;
        engine.note_directory(&directory, Some(&profile));
        assert!(engine.held().my_org.is_none(), "an unpinned cached id is not held");
        assert_eq!(cached(&dir), (Some(FIXTURE_ORG.into()), false), "and a whole read does not forget it");
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[]);
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, ROSTER_UNREADABLE),
            other => panic!("expected the refusal, got {other:?}"),
        }
        assert_eq!(cached(&dir), (Some(FIXTURE_ORG.into()), false), "an unreadable roster drops nothing");
    }

    /// **A cached id that is neither made here nor adopted, whose roster
    /// fails the rule, is dropped by a write** -- and one made here is used
    /// without asking the roster at all.
    #[test]
    fn a_cached_id_the_roster_disowns_is_dropped_unless_made_here() {
        let mallory = in_role(roster_row("mal-ou", Some("mal-id"), "mallory@evil.example", 2, &[]), 0);
        let (mut server, _backend, engine, dir) = unpinned_org(&[me_row(), mallory], &[]);
        cache_org(&dir, false);
        shares_into_a_fresh_org(&mut server, &engine, &dir, "a cached id another owner holds");

        let (_server, _backend, engine, dir) = on_nodewarden_unpinned(serde_json::json!([sharing_org_row()]));
        cache_org(&dir, true);
        let profile: Profile = serde_json::from_value(sharing_profile(serde_json::json!([sharing_org_row()]))).expect("profile");
        let account = engine.held().account.clone().expect("an account");
        assert_eq!(engine.my_org_id(&profile, &account).expect("an answer").as_deref(), Some(FIXTURE_ORG), "the id this client made");
    }

    /// **I3: a 404 from the sharer's own roster is not "too old" when the
    /// org has gone** -- deleted, or this account no longer its owner. The
    /// profile is asked; the org and its cached id are dropped, and sharing
    /// stays on so the next share makes a new one.
    #[test]
    fn a_deleted_org_is_dropped_and_sharing_stays_on() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg { id: FIXTURE_ORG.into(), collections: Vec::new(), roster: Vec::new() });
        server.mock("GET", "/api/accounts/profile").with_body(sharing_profile(serde_json::json!([])).to_string()).create();
        server.mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str()).with_status(404).create();
        match engine.fetch_roster(FIXTURE_ORG) {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, ORG_GONE),
            other => panic!("expected the org to be gone, got {other:?}"),
        }
        assert!(!engine.held().capabilities.too_old, "a deleted org is not an old server");
        assert!(engine.view(&[]).available);
        assert!(engine.held().my_org.is_none(), "the gone org is not held");
        assert_eq!(cached(&dir), (None, false), "and not cached: the next share makes a new one");
    }

    /// **I3: a 404 from anything but the sharer's own roster leaves sharing
    /// on** -- from `/share` (a record gone), the invite, a confirm (a person
    /// gone), or a candidate org's roster (no ManageUsers there).
    #[test]
    fn a_404_from_any_other_route_leaves_sharing_on() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        anna_is_confirmed(&mut server);
        server.mock("PUT", "/api/ciphers/p1/share").with_status(404).create();
        assert!(engine.share("p1", "anna@example.com").is_err());
        assert!(!engine.held().capabilities.too_old, "/share");

        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-kim", "kim@example.com")]);
        with_roster(&mut server, &[me_row()]);
        server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_status(404).create();
        assert!(engine.share("p1", "kim@example.com").is_err());
        assert!(!engine.held().capabilities.too_old, "the invite");

        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[me_row()]))
            .expect(1)
            .create();
        with_roster(&mut server, &[me_row(), roster_row("anna-ou", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]);
        server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str()).with_body("").create();
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-ou/confirm").as_str()).with_status(404).create();
        server.mock("PUT", "/api/ciphers/p1/share").with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-anna"])).create();
        let outcome = engine.share("p1", "anna@example.com").expect("shared; the confirm did not land");
        assert_eq!(outcome.result, ShareResult::WaitingAccept);
        assert!(!engine.held().capabilities.too_old, "/confirm");

        let (mut server, _backend, engine, _dir) = unpinned_org(&[], &[]);
        server.mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str()).with_status(404).create();
        server.mock("POST", "/api/organizations").with_status(400).with_body(r#"{"message":"No."}"#).create();
        assert!(engine.share("p1", "anna@example.com").is_err());
        assert!(!engine.held().capabilities.too_old, "a candidate org's roster");
    }

    /// **A cached id the profile does not list YET is kept** (final
    /// re-review, minor 2): a whole read that began before a share's
    /// `create_org` and ended after its state write carries a profile
    /// without the new org. Through `note_directory` -- the race's own path
    /// -- and `my_org_id`, the id and its `sharing_org_created` flag stay.
    #[test]
    fn a_cached_id_the_profile_does_not_list_yet_is_kept() {
        let (_server, _backend, engine, dir) = on_nodewarden(serde_json::json!([]));
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            let me = state.account_mut(FIXTURE_USER_ID);
            me.sharing_org_id = Some("org-new".into());
            me.sharing_org_created = true;
        })
        .expect("seeded");
        let stale: Profile = serde_json::from_value(sharing_profile(serde_json::json!([]))).expect("profile");
        let directory = crate::rest::organizations::Directory::from_sync(
            &serde_json::from_value(serde_json::json!({ "profile": sharing_profile(serde_json::json!([])), "ciphers": [], "folders": [], "collections": [] })).expect("sync"),
            &engine.keys_from(&stale).expect("keys"),
        );
        engine.note_directory(&directory, Some(&stale));
        let account = engine.held().account.clone().expect("an account");
        assert_eq!(engine.my_org_id(&stale, &account).expect("an answer"), None, "not the answer this time");
        let me = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!((me.sharing_org_id.as_deref(), me.sharing_org_created), (Some("org-new"), true), "kept, flag and all");
    }

    // ---- final review, I1: a plaintext name never starts a grant ----

    /// A person collection whose name is PLAINTEXT -- one the server could
    /// have written itself.
    fn plaintext_collection(id: &str, email: &str) -> serde_json::Value {
        serde_json::json!({ "id": id, "organizationId": FIXTURE_ORG, "name": email, "object": "collection" })
    }

    /// **Re-invite is refused for a person known only from a plaintext
    /// name, before anything is written** (final review, I1). Its second
    /// branch invites the address, and on a server that accepts at once the
    /// confirm pass would hand her the org key; a hostile server that listed
    /// `mallory@...` in plaintext would have one click make her a recipient.
    /// So: no invite, no key looked up, no pin.
    #[test]
    fn reinvite_refuses_a_plaintext_collection_name() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(&mut server, &[plaintext_collection("c-mal", "mallory@example.com")]);
        with_roster(&mut server, &[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]);
        let key = server.mock("GET", Matcher::Regex("/public-key$".into())).expect(0).create();
        let writes = server.mock("POST", Matcher::Any).expect(0).create();
        match engine.reinvite("Mallory@example.com") {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, not_vouched_for("mallory@example.com")),
            other => panic!("a plaintext name was re-invited: {other:?}"),
        }
        key.assert();
        writes.assert();
        let state = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert!(state.pin_for("mallory@example.com").is_none(), "nothing was pinned");
    }

    /// **A share beside a plaintext twin files into a collection the engine
    /// names itself** (final review, I1): the plaintext `c-plain` is neither
    /// the collection anna is invited to nor the one the record lands in.
    #[test]
    fn a_share_beside_a_plaintext_twin_makes_its_own_collection() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[plaintext_collection("c-plain", "anna@example.com")]);
        server
            .mock("GET", format!("/api/organizations/{FIXTURE_ORG}/users?includeCollections=true").as_str())
            .with_body(roster(&[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]))
            .expect(1)
            .create();
        with_roster(
            &mut server,
            &[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]), roster_row("inv-1", None, "anna@example.com", 0, &["c-anna"])],
        );
        let made = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/collections").as_str())
            .with_body(r#"{"id":"c-anna","object":"collection"}"#)
            .expect(1)
            .create();
        let invite = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/invite").as_str())
            .match_request(|request| request.utf8_lossy_body().contains("c-anna") && !request.utf8_lossy_body().contains("c-plain"))
            .with_body("")
            .expect(1)
            .create();
        let moved = server
            .mock("PUT", "/api/ciphers/p1/share")
            .match_request(|request| request.utf8_lossy_body().contains("c-anna") && !request.utf8_lossy_body().contains("c-plain"))
            .with_body_from_request(echo("p1", Some(FIXTURE_ORG), &["c-anna"]))
            .expect(1)
            .create();
        engine.share("p1", "anna@example.com").expect("shared");
        made.assert();
        invite.assert();
        moved.assert();
    }

    /// **A stop takes the person off every collection named for them**, a
    /// plaintext twin included: a removal that left the record in the twin
    /// would say "stopped" while they can still see it.
    #[test]
    fn a_stop_takes_the_person_off_a_plaintext_twin_too() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        with_collections(
            &mut server,
            &[
                plaintext_collection("c-plain", "anna@example.com"),
                person_collection("c-anna", "anna@example.com"),
                person_collection("c-bob", "bob@example.com"),
            ],
        );
        with_roster(&mut server, &[]);
        server.mock("GET", "/api/ciphers/s1").with_body(shared_cipher("s1", "Acme VPN", &["c-plain", "c-anna", "c-bob"]).to_string()).create();
        let off = server
            .mock("PUT", "/api/ciphers/s1/collections")
            .match_body(Matcher::Json(serde_json::json!({ "collectionIds": ["c-bob"] })))
            .with_body(shared_cipher("s1", "Acme VPN", &["c-bob"]).to_string())
            .expect(1)
            .create();
        engine.stop("s1", "anna@example.com").expect("stopped");
        off.assert();
    }

    /// **The view marks a person known only from a plaintext name**, and
    /// reads a person with both kinds off the authenticated collection
    /// wherever it stands in server order (final review, I1).
    #[test]
    fn the_view_marks_a_plaintext_person_and_prefers_the_authenticated_collection() {
        let engine = offline();
        {
            let mut held = engine.held();
            held.capabilities.loaded = true;
            held.my_org = Some(MyOrg {
                id: "org1".into(),
                collections: vec![
                    PersonCollection { id: "c-plain".into(), email: "anna@x.io".into(), authenticated: false },
                    authentic("c-anna", "anna@x.io"),
                    PersonCollection { id: "c-mal".into(), email: "mal@x.io".into(), authenticated: false },
                ],
                roster: vec![
                    member("anna@x.io", MemberStatus::Confirmed, &["c-anna"], None),
                    member("mal@x.io", MemberStatus::Confirmed, &["c-mal"], None),
                ],
            });
        }
        let view = engine.view(&[]);
        let anna = view.person("anna@x.io").expect("anna");
        assert!(anna.authenticated);
        assert_eq!(anna.state, PersonState::Shared, "read off c-anna, which she reaches, not the plaintext c-plain");
        let mal = view.person("mal@x.io").expect("mal is shown");
        assert!(!mal.authenticated, "a plaintext name is unverified");
        assert_eq!(view.people.len(), 2, "one entry per person");
        let json = serde_json::to_value(&view).expect("json");
        let back: SharingView = serde_json::from_value(json).expect("round trip");
        assert_eq!(back, view);
        let mut bare = serde_json::to_value(anna).expect("json");
        bare.as_object_mut().expect("an object").remove("authenticated");
        let read: PersonEntry = serde_json::from_value(bare).expect("an entry without the field");
        assert!(!read.authenticated, "an answer without the field is unverified");
    }

    /// **"Share anyway" confirms the key the user fingerprinted, or nothing**:
    /// a key swapped between Show fingerprint and the click is refused, the
    /// person is `keyChanged` again, and neither the pin nor the membership
    /// moves.
    #[test]
    fn share_anyway_refuses_a_key_other_than_the_one_fingerprinted() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        let old = crate::sharing_state::Pin { sha256: "00".repeat(32), user_id: "anna-id".into(), at: 1 };
        crate::sharing_state::update(&dir.join("sharing_state.json"), |state| {
            state.account_mut(FIXTURE_USER_ID).set_pin("anna@example.com", old.clone());
        }).expect("seeded");
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]);
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        let confirm = server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str()).expect(0).create();
        match engine.confirm_anyway("anna@example.com", &"11".repeat(32)) {
            Err(SharingError::Refused(said)) => {
                assert_eq!(said, "anna@example.com's key changed again. Check the fingerprint before sharing.");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        confirm.assert();
        let pinned = crate::sharing_state::SharingState::load(&dir.join("sharing_state.json")).account(FIXTURE_USER_ID);
        assert_eq!(pinned.pin_for("anna@example.com"), Some(&old), "the pin moved to a key nobody checked");
        assert!(engine.held().key_changed.contains("anna@example.com"));
    }

    /// **Pins that cannot be read confirm nobody**: read as empty, a corrupt
    /// `sharing_state.json` would be "no pins", and a swapped key would be
    /// trusted on first use. The pass waits instead -- no key read, no
    /// confirm.
    #[test]
    fn unreadable_pins_confirm_nobody() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        std::fs::write(dir.join("sharing_state.json"), b"{ not the state").expect("a corrupt file");
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-anna", "anna@example.com")],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        let key = server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).expect(0).create();
        let confirm = server.mock("POST", Matcher::Any).expect(0).create();
        let pass = engine.refresh(Trigger::WholeRead).expect("a pass");
        key.assert();
        confirm.assert();
        assert!(!pass.changed);
        // Control: it is the corrupt file that is refused, not a missing one.
        std::fs::remove_file(dir.join("sharing_state.json")).expect("removed");
        assert!(crate::sharing_state::SharingState::read(&dir.join("sharing_state.json")).is_some());
    }

    /// **A `keyChanged` person is not asked about on every batch**: a hub
    /// batch skips them, and the timer still looks.
    #[test]
    fn a_changed_key_is_rechecked_by_the_timer_alone() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-anna", "anna@example.com")],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        engine.held().key_changed.insert("anna@example.com".into());
        with_roster(&mut server, &[roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])]);
        let key = server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).expect(1).create();
        server.mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str()).with_body("").create();
        engine.refresh(Trigger::Batch).expect("a batch pass");
        engine.refresh(Trigger::WholeRead).expect("a whole-read pass");
        engine.refresh(Trigger::Timer).expect("the timer");
        key.assert();
    }

    /// **The confirm wraps THE org key to THEIR key**: the posted `key`
    /// opens under the fixture private key -- the half of the key the
    /// server served for anna -- to exactly the org key, not merely to
    /// something shaped like a type-4 string.
    #[test]
    fn the_posted_confirm_key_opens_to_the_org_key_under_their_private_key() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-anna", "anna@example.com")],
            roster: crate::rest::organizations::map_members(&serde_json::json!({ "data": [roster_row("anna-id", Some("anna-id"), "anna@example.com", 1, &["c-anna"])] })).expect("a roster"),
        });
        server.mock("GET", "/api/users/anna-id/public-key").with_body(anna_key()).create();
        let posted = Arc::new(Mutex::new(None::<String>));
        let keep = Arc::clone(&posted);
        let confirm = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/anna-id/confirm").as_str())
            .with_body_from_request(move |request| {
                let body: serde_json::Value = serde_json::from_slice(request.body()).expect("json");
                *keep.lock().expect("unpoisoned") = body["key"].as_str().map(str::to_string);
                Vec::new()
            })
            .expect(1)
            .create();
        engine.refresh(Trigger::WholeRead).expect("a pass");
        confirm.assert();
        let wrapped: EncString = posted.lock().expect("unpoisoned").clone().expect("a key was posted").parse().expect("an EncString");
        let opened = crate::rest::crypto::unwrap_org_key(
            &crate::rest::crypto::tests::hex(crate::rest::crypto::tests::ORG_KEY_PRIVATE_PKCS8_DER),
            &wrapped,
        )
        .expect("opens under anna's private key");
        assert!(opened == crate::rest::backend::tests::fixture_org_key(), "the confirm wrapped some other key");
    }

    // ---- fix round 2: a direct add over pins that cannot be read ----------------

    /// The files in `dir` that a broken state file was set aside as.
    fn set_aside_in(dir: &crate::test_scratch::ScratchDir) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(dir.path())
            .expect("the directory")
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("sharing_state.json.unreadable-")))
            .collect()
    }

    /// **A share over pins that cannot be read writes nothing**: refused
    /// before the org, the collection, the membership or the move, with the
    /// same sentence, and the broken file kept aside for a Retry to start
    /// fresh.
    #[test]
    fn a_share_over_unreadable_pins_is_refused_before_anything_is_written() {
        let (mut server, _backend, engine, dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        server.mock("GET", "/api/ciphers/p1").with_body(personal_cipher("p1", "Acme VPN").to_string()).create();
        with_collections(&mut server, &[person_collection("c-anna", "anna@example.com")]);
        with_roster(&mut server, &[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[])]);
        let posts = server.mock("POST", Matcher::Any).expect(0).create();
        let puts = server.mock("PUT", Matcher::Any).expect(0).create();
        std::fs::write(dir.join("sharing_state.json"), b"{ torn").expect("a corrupt file");
        match engine.share("p1", "anna@example.com") {
            Err(SharingError::Refused(said)) => assert_eq!(said, crate::sharing_state::UNREADABLE_SET_ASIDE),
            other => panic!("expected the unreadable-pins refusal, got {other:?}"),
        }
        posts.assert();
        puts.assert();
        assert_eq!(set_aside_in(&dir).len(), 1);
    }

    /// **A pending invitation is Invited whatever its expiry** (official-path
    /// spec §3.4): official servers do not show a client the expiry, so no
    /// card claims one -- and a NodeWarden that does name one is read the same.
    #[test]
    fn a_pending_invitation_is_invited_whatever_its_expiry() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([sharing_org_row()]));
        engine.held().my_org = Some(MyOrg {
            id: FIXTURE_ORG.into(),
            collections: vec![authentic("c-carol", "carol@example.com"), authentic("c-dan", "dan@example.com")],
            roster: Vec::new(),
        });
        let mut carol = roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"]);
        carol["invitationExpirationDate"] = serde_json::json!("2999-01-01T00:00:00.000Z");
        let mut dan = roster_row("inv-dan", None, "dan@example.com", 0, &["c-dan"]);
        dan["invitationExpirationDate"] = serde_json::json!("2001-01-01T00:00:00.000Z");
        with_roster(&mut server, &[carol, dan]);
        engine.refresh(Trigger::Timer).expect("a pass");
        let view = engine.view(&[]);
        assert_eq!(view.person("carol@example.com").map(|p| p.state), Some(PersonState::Invited));
        assert_eq!(view.person("dan@example.com").map(|p| p.state), Some(PersonState::Invited));
    }

    /// The fixture account on NodeWarden, owning the sharing org, as the
    /// server's administrator or not. One file, the sharing state, with the
    /// org cached as made here ([`pinned_dir`]), and not loaded yet: the
    /// mint loads, as every write does.
    fn on_nodewarden_as(admin: bool) -> (MockServer, crate::rest::backend::RestBackend, Arc<Sharing>, crate::test_scratch::ScratchDir) {
        let (mut server, backend) = signed_in_with_no_sync_route();
        let engine = backend.sharing_engine();
        let dir = pinned_dir("sharing-mint");
        engine.set_up(SharingSetup { state_path: Some(dir.join("sharing_state.json")), ..SharingSetup::none() });
        let mut profile = sharing_profile(serde_json::json!([sharing_org_row()]));
        if admin {
            profile["role"] = serde_json::json!("admin");
        }
        server.mock("GET", "/api/accounts/profile").with_body(profile.to_string()).expect_at_least(0).create();
        server
            .mock("GET", format!("/api/users/{FIXTURE_USER_ID}/public-key").as_str())
            .with_body(serde_json::json!({ "publicKey": crate::rest::crypto::tests::base64(&crate::rest::crypto::tests::fixture_public_spki()) }).to_string())
            .expect_at_least(0)
            .create();
        (server, backend, engine, dir)
    }

    /// Carol's pending invitation, expiring far in the future.
    fn carol_invited() -> serde_json::Value {
        let mut carol = roster_row("inv-carol", None, "carol@example.com", 0, &["c-carol"]);
        carol["invitationExpirationDate"] = serde_json::json!("2999-01-01T00:00:00.000Z");
        carol
    }

    /// NodeWarden's `toInviteResponse`, as the registration-invite route
    /// answers with it.
    const MINTED: &str = r#"{"code":"abc","status":"active","createdBy":"user-1","usedBy":null,"createdAt":"2026-09-28T00:00:00.000Z","updatedAt":"2026-09-28T00:00:00.000Z","expiresAt":"2999-01-01T00:00:00.000Z","inviteLink":"https://nw.example/#/register?invite=abc","object":"invite"}"#;

    /// **An administrator mints a registration code for one pending
    /// invitation** (owner decision Q3 (b)): the invitation is found by email
    /// in a fresh roster read, the typed password is turned into the server
    /// hash with the session's own master key, and only the hash leaves the
    /// process -- to that invitation's own route. Anyone else is refused
    /// before any write.
    #[test]
    fn an_admin_mints_an_invite_code_with_the_password_hash_and_nobody_else_can() {
        let (mut server, _backend, engine, _dir) = on_nodewarden_as(true);
        with_roster(&mut server, &[roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]), carol_invited()]);
        // The fixture account's master key is PBKDF2 of "master" with one
        // iteration; its server hash is one more PBKDF2 round keyed by it.
        let expected = crate::rest::crypto::master_key(b"master", FIXTURE_EMAIL, crate::rest::crypto::Kdf::Pbkdf2 { iterations: 1 })
            .expect("the fixture master key")
            .password_hash(b"master");
        let minted = server
            .mock("POST", format!("/api/organizations/{FIXTURE_ORG}/users/inv-carol/registration-invite").as_str())
            .match_body(Matcher::Json(serde_json::json!({ "masterPasswordHash": expected.as_str() })))
            .with_status(201)
            .with_body(MINTED)
            .expect(1)
            .create();
        let code = engine.mint_invite_code("Carol@Example.com", b"master").expect("minted");
        minted.assert();
        assert_eq!(code.code, "abc");
        assert_eq!(code.link.as_deref(), Some("https://nw.example/#/register?invite=abc"));
        assert_eq!(code.expires_at.as_deref(), Some("2999-01-01T00:00:00.000Z"));
        assert!(engine.view(&[]).can_mint_invites);

        let (mut other, _backend, engine, _dir) = on_nodewarden_as(false);
        with_roster(&mut other, &[carol_invited()]);
        let never = other.mock("POST", Matcher::Any).expect(0).create();
        assert!(matches!(engine.mint_invite_code("carol@example.com", b"master"), Err(SharingError::Refused(s)) if s == NOT_ADMIN));
        never.assert();
        assert!(!engine.view(&[]).can_mint_invites);
    }

    /// **Only a pending invitation gets a code, and the server's refusals
    /// read as plain sentences.** Someone with an account, an invitation
    /// that has expired, and an address nobody invited are refused before
    /// any write; a wrong password, "already has an account", a lost admin
    /// role and anything else the route answers each say what happened.
    #[test]
    fn only_a_pending_invitation_gets_a_code_and_the_servers_refusals_are_plain() {
        let (mut server, _backend, engine, _dir) = on_nodewarden_as(true);
        let mut dan = roster_row("inv-dan", None, "dan@example.com", 0, &["c-dan"]);
        dan["invitationExpirationDate"] = serde_json::json!("2001-01-01T00:00:00.000Z");
        with_roster(
            &mut server,
            &[
                roster_row(FIXTURE_USER_ID, Some(FIXTURE_USER_ID), FIXTURE_EMAIL, 2, &[]),
                roster_row("ou-anna", Some("anna-id"), "anna@example.com", 2, &["c-anna"]),
                dan,
                carol_invited(),
            ],
        );
        let route = |ou: &str| format!("/api/organizations/{FIXTURE_ORG}/users/{ou}/registration-invite");
        let not_anna = server.mock("POST", route("ou-anna").as_str()).expect(0).create();
        let not_dan = server.mock("POST", route("inv-dan").as_str()).expect(0).create();
        let refused = |engine: &Sharing, email: &str| match engine.mint_invite_code(email, b"master") {
            Err(SharingError::Refused(said)) => said,
            other => panic!("expected a refusal for {email}, got {:?}", other.map(|_| "a code")),
        };
        assert_eq!(refused(&engine, "anna@example.com"), "anna@example.com already has an account, so they don't need an invite code.");
        assert_eq!(refused(&engine, "dan@example.com"), "The invitation to dan@example.com has expired. Re-invite them, then make the code.");
        assert_eq!(refused(&engine, "zed@example.com"), "There is no invitation waiting for zed@example.com.");
        not_anna.assert();
        not_dan.assert();

        let said = |sentence: &str| {
            serde_json::json!({ "error": sentence, "error_description": sentence, "ErrorModel": { "Message": sentence, "Object": "error" } }).to_string()
        };
        let carol = route("inv-carol");
        server.mock("POST", carol.as_str()).with_status(400).with_body(said("Invalid password")).expect(1).create();
        server.mock("POST", carol.as_str()).with_status(400).with_body(said("This person already has an account")).expect(1).create();
        server.mock("POST", carol.as_str()).with_status(400).with_body(said("Invitation has expired")).expect(1).create();
        server
            .mock("POST", carol.as_str())
            .with_status(403)
            .with_body(said("Only a server administrator can create registration invites"))
            .expect(1)
            .create();
        server.mock("POST", carol.as_str()).with_status(400).with_body(said("Something else")).expect(1).create();
        assert_eq!(refused(&engine, "carol@example.com"), WRONG_PASSWORD);
        assert_eq!(refused(&engine, "carol@example.com"), "carol@example.com already has an account, so they don't need an invite code.");
        assert_eq!(refused(&engine, "carol@example.com"), "The invitation to carol@example.com has expired. Re-invite them, then make the code.");
        assert_eq!(refused(&engine, "carol@example.com"), NOT_ADMIN);
        assert_eq!(refused(&engine, "carol@example.com"), format!("{INVITE_CODE_FAILED} The server said: Something else"));
    }

    /// A Bitwarden invite link for `email`, to the fixture-free org `org-guid`.
    fn invite_link_for(email: &str) -> String {
        format!(
            "https://vault.example/#/accept-organization?organizationId=0f0e0d0c-0b0a-4009-8807-060504030201\
             &organizationUserId=a1b2c3d4-e5f6-4789-8abc-def012345678&email={}\
             &organizationName=Shared+by+denis%40example.com&token=tok-1&initOrganization=False&orgUserHasExistingUser=True",
            email.replace('@', "%40")
        )
    }

    /// **A pasted link is accepted on Bitwarden's own route** (official-path
    /// spec §4.1): the link's ids in the path, its token in the body, and no
    /// invitation list read.
    #[test]
    fn a_pasted_link_is_accepted_on_the_official_route() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        let accepted = server
            .mock("POST", "/api/organizations/0f0e0d0c-0b0a-4009-8807-060504030201/users/a1b2c3d4-e5f6-4789-8abc-def012345678/accept")
            .match_body(Matcher::Json(serde_json::json!({ "token": "tok-1" })))
            .with_body("")
            .expect(1)
            .create();
        let listed = server.mock("GET", Matcher::Regex("invitations".into())).expect(0).create();
        engine.accept_link(&invite_link_for(FIXTURE_EMAIL)).expect("accepted");
        accepted.assert();
        listed.assert();
    }

    /// **A link's org name files no sharer until the server attests one**
    /// (official-path spec §4.3, fix round 1 I1). The name is plaintext
    /// anyone can write: a "Shared by boss@corp.example" link, accepted, puts
    /// nothing in waiting and nothing in Shared with me -- not even a record
    /// of that org the vault already holds -- and a later pass whose
    /// directory does not list the org adds nothing either. Only the server's
    /// own listing of the org files it, and then marked unverified unless a
    /// roster names its owner (`a_whole_read_files_the_unverified_sharers`).
    #[test]
    fn a_links_org_name_files_no_sharer_until_the_server_attests_one() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        server.mock("POST", Matcher::Regex("/accept$".into())).with_body("").expect(1).create();
        let org = "0f0e0d0c-0b0a-4009-8807-060504030201";
        let link = invite_link_for(FIXTURE_EMAIL).replace("Shared+by+denis%40example.com", "Shared+by+boss%40corp.example");
        engine.accept_link(&link).expect("accepted");
        let theirs: VaultItem =
            serde_json::from_value(serde_json::json!({ "id": "theirs-1", "name": "Payroll", "organizationId": org })).expect("item");
        let view = engine.view(std::slice::from_ref(&theirs));
        assert!(view.waiting.is_empty(), "a sharer named by the link alone: {view:?}");
        assert!(view.received.is_empty(), "a record filed under the link's claim: {view:?}");
        engine.refresh(Trigger::Timer).expect("a pass");
        let view = engine.view(std::slice::from_ref(&theirs));
        assert!(view.waiting.is_empty() && view.received.is_empty(), "{view:?}");
    }

    /// **A link for another account is refused in words, before any
    /// request** -- the server would refuse it too, in its own.
    #[test]
    fn a_link_for_someone_else_is_refused_before_any_request() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        let posts = server.mock("POST", Matcher::Any).expect(0).create();
        match engine.accept_link(&invite_link_for("someone@else.example")) {
            Err(SharingError::Refused(sentence)) => {
                assert_eq!(sentence, link_for_someone_else("someone@else.example", &FIXTURE_EMAIL.to_lowercase()));
            }
            other => panic!("a link for someone else was accepted: {other:?}"),
        }
        posts.assert();
        assert!(engine.view(&[]).waiting.is_empty());
    }

    /// **What is not a link is refused with the parser's sentence**, and a
    /// server's refusal keeps its own words after the engine's.
    #[test]
    fn a_bad_link_and_a_refused_accept_say_why() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        match engine.accept_link("https://vault.example/#/vault") {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, crate::invite_link::LinkError::NotAnInviteLink.sentence()),
            other => panic!("not a link was accepted: {other:?}"),
        }
        server
            .mock("POST", Matcher::Regex("/accept$".into()))
            .with_status(400)
            .with_body(r#"{"message":"Invalid token."}"#)
            .create();
        match engine.accept_link(&invite_link_for(FIXTURE_EMAIL)) {
            Err(SharingError::Refused(sentence)) => assert_eq!(sentence, format!("{LINK_ACCEPT_FAILED} The server said: Invalid token.")),
            other => panic!("a refused accept succeeded: {other:?}"),
        }
    }

    /// **A received record says whether its sharer is the server's word**
    /// (official-path spec §4.3), and carries its organisation, which Shared
    /// with me groups by; a sharer being waited on is marked the same way.
    #[test]
    fn received_records_and_waiting_sharers_say_whether_the_sharer_is_verified() {
        let engine = offline();
        {
            let mut held = engine.held();
            held.capabilities.loaded = true;
            held.others = vec![("org-anna".into(), "anna@x.io".into()), ("org-boss".into(), "boss@x.io".into())];
            held.waiting = vec![("org-carol".into(), "carol@x.io".into())];
            held.unverified = ["org-boss".to_string(), "org-carol".to_string()].into_iter().collect();
        }
        let items = [an_item("r1", "org-anna", &[]), an_item("r2", "org-boss", &[])];
        let view = engine.view(&items);
        assert_eq!(
            view.received,
            vec![
                ReceivedEntry { item_id: "r1".into(), from: "anna@x.io".into(), organization_id: "org-anna".into(), verified: true },
                ReceivedEntry { item_id: "r2".into(), from: "boss@x.io".into(), organization_id: "org-boss".into(), verified: false },
            ]
        );
        assert!(view.received_verified("r1"));
        assert!(!view.received_verified("r2"));
        assert!(!view.received_verified("nothing"));
        assert_eq!(view.waiting, vec![WaitingEntry { organization_id: "org-carol".into(), from: "carol@x.io".into(), verified: false }]);
    }

    /// **A whole read files which received orgs are unverified** -- the
    /// directory's own mark -- for the view to say so.
    #[test]
    fn a_whole_read_files_the_unverified_sharers() {
        let (_server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        // `sharing_profile` (backend.rs's fixture) lists it in both
        // `organizations` and `organizationsNew`, as a confirmed member.
        let body = sharing_profile(serde_json::json!([
            { "id": "org-boss", "name": "Shared by boss@x.io", "type": 2, "status": 2, "enabled": true }
        ]));
        let profile: Profile = serde_json::from_value(body.clone()).expect("profile");
        let response: crate::rest::sync::SyncResponse =
            serde_json::from_value(serde_json::json!({ "profile": body, "ciphers": [], "folders": [], "collections": [] })).expect("sync");
        let directory = crate::rest::organizations::Directory::from_sync(&response, &engine.keys_from(&profile).expect("keys"));
        engine.note_directory(&directory, Some(&profile));
        let held = engine.held();
        assert_eq!(held.others, vec![("org-boss".to_string(), "boss@x.io".to_string())]);
        assert!(held.unverified.contains("org-boss"));
    }

    /// **This account's own sharing org is never received from anyone**
    /// (DW1 review, M3): an org this client made, or the user adopted, is
    /// not listed in Shared with me whatever the profile now says of it --
    /// here, no longer owned and renamed for someone else.
    #[test]
    fn a_made_or_adopted_org_is_never_received_from_anyone() {
        let renamed = serde_json::json!([
            { "id": FIXTURE_ORG, "name": "Shared by boss@x.io", "type": 2, "status": 2, "enabled": true }
        ]);
        let (_server, _backend, engine, _dir) = on_nodewarden(renamed.clone());
        let body = sharing_profile(renamed);
        let profile: Profile = serde_json::from_value(body.clone()).expect("profile");
        let response: crate::rest::sync::SyncResponse =
            serde_json::from_value(serde_json::json!({ "profile": body, "ciphers": [], "folders": [], "collections": [] })).expect("sync");
        let directory = crate::rest::organizations::Directory::from_sync(&response, &engine.keys_from(&profile).expect("keys"));
        assert!(directory.sharing_orgs().iter().any(|o| o.id == FIXTURE_ORG), "the directory alone reads it as received");
        engine.note_directory(&directory, Some(&profile));
        let held = engine.held();
        assert!(held.others.is_empty(), "{:?}", held.others);
        assert!(held.waiting.is_empty(), "{:?}", held.waiting);
    }

    /// **What a whole read learns about rosters is kept for the session.**
    #[test]
    fn what_a_whole_read_learns_about_rosters_is_kept_for_the_session() {
        let engine = offline();
        assert_eq!(engine.received_rosters(), ReceivedRosters::default());
        let learned = ReceivedRosters {
            readable: Some(true),
            owners: [("org-boss".to_string(), Some("boss@x.io".to_string()))].into_iter().collect(),
        };
        engine.note_received_rosters(learned.clone());
        assert_eq!(engine.received_rosters(), learned);
    }

    /// A whole sync on [`on_nodewarden`]'s server, its profile listing
    /// `organizations` -- what `RestBackend::synced` reads. Asked `reads`
    /// times exactly: no revision route is mocked, so every list is a whole
    /// read.
    fn whole_reads(server: &mut MockServer, organizations: serde_json::Value, reads: usize) -> crate::test_http::Mock {
        let body = serde_json::json!({
            "object": "sync",
            "profile": sharing_profile(organizations),
            "folders": [], "ciphers": [], "collections": []
        });
        server.mock("GET", "/api/sync?excludeDomains=true").with_body(body.to_string()).expect(reads).create()
    }

    fn boss_org() -> serde_json::Value {
        serde_json::json!([{ "id": "org-boss", "name": "Shared by boss@x.io", "type": 2, "status": 2, "enabled": true }])
    }

    /// **A member refused once is asked nothing more that session** (fix
    /// round 1, I2): NodeWarden answers a member's `mini-details` 404, and
    /// the second whole read makes no request for it at all.
    #[test]
    fn a_refused_roster_is_not_asked_again_on_a_later_whole_read() {
        let (mut server, backend, engine, _dir) = on_nodewarden(boss_org());
        let syncs = whole_reads(&mut server, boss_org(), 2);
        let asked = server.mock("GET", Matcher::Regex("/users/mini-details$".into())).with_status(404).expect(1).create();
        crate::vault_backend::VaultBackend::list_items(&backend).expect("first read");
        crate::vault_backend::VaultBackend::list_items(&backend).expect("second read");
        syncs.assert();
        asked.assert();
        assert_eq!(engine.received_rosters().readable, Some(false));
        assert!(engine.held().unverified.contains("org-boss"));
    }

    /// **An answered roster costs one request a session** (fix round 1, m12):
    /// on a server that lets a member read it, the second whole read applies
    /// the kept owner -- verified -- without asking again.
    #[test]
    fn an_answered_roster_is_asked_once_across_whole_reads() {
        let (mut server, backend, engine, _dir) = on_nodewarden(boss_org());
        let syncs = whole_reads(&mut server, boss_org(), 2);
        let rows = serde_json::json!({ "data": [
            { "id": "ou-b", "userId": "u-b", "email": "Boss@X.io", "type": 0, "status": 2 },
            { "id": "ou-me", "userId": FIXTURE_USER_ID, "email": FIXTURE_EMAIL, "type": 2, "status": 2 }
        ]});
        let asked = server
            .mock("GET", "/api/organizations/org-boss/users/mini-details")
            .with_body(rows.to_string())
            .expect(1)
            .create();
        for read in ["first", "second"] {
            crate::vault_backend::VaultBackend::list_items(&backend).expect("a read");
            let held = engine.held();
            assert_eq!(held.others, vec![("org-boss".to_string(), "boss@x.io".to_string())], "{read}");
            assert!(!held.unverified.contains("org-boss"), "{read} read: the kept owner is applied");
        }
        syncs.assert();
        asked.assert();
    }

    /// **This PC's own org, listed in a received org's shape, is asked
    /// about by nobody** (fix round 1, m11): the backend takes it out of the
    /// directory before any roster is asked, so no `mini-details` goes out
    /// and nothing names a sharer for it.
    #[test]
    fn a_whole_read_asks_no_roster_of_this_pcs_own_org() {
        let renamed = serde_json::json!([
            { "id": FIXTURE_ORG, "name": "Shared by boss@x.io", "type": 2, "status": 2, "enabled": true }
        ]);
        let (mut server, backend, engine, _dir) = on_nodewarden(renamed.clone());
        let syncs = whole_reads(&mut server, renamed, 1);
        let asked = server.mock("GET", Matcher::Regex("/users/mini-details$".into())).expect(0).create();
        crate::vault_backend::VaultBackend::list_items(&backend).expect("a read");
        syncs.assert();
        asked.assert();
        assert!(engine.held().others.is_empty());
    }

    /// **Stop receiving from X** (spec 6.9): leave the sharer's org, and every
    /// record from them leaves the vault.
    #[test]
    fn stopping_receiving_leaves_the_org_and_removes_its_records() {
        let (mut server, _backend, engine, _dir) = on_nodewarden(serde_json::json!([]));
        engine.held().others = vec![("org-denis".into(), "denis@example.com".into())];
        let left = server.mock("POST", "/api/organizations/org-denis/leave").with_body("").expect(1).create();
        let items: Vec<VaultItem> = ["a", "b"].iter().map(|id| serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "organizationId": if *id == "a" { "org-denis" } else { "org-other" }
        })).expect("item")).collect();
        let outcome = engine.leave("org-denis", &items).expect("left");
        left.assert();
        assert_eq!(outcome.removed, ["a"]);
        assert!(matches!(engine.leave("org-mine", &items), Err(SharingError::Refused(_))), "only someone else's sharing org can be left");
    }
}

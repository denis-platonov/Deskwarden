//! **Who else can see this item.**
//!
//! One sentence, and the whole module is an answer to it. Everything below
//! exists so that a surface -- design 8a's `Owner` row, design 5a's "Also
//! visible to 14 people in Engineering", this rail's own ORGANISATIONS
//! section -- can ask that question about a [`VaultItem`] and get a true
//! answer, or an honest "this app does not know".
//!
//! # What was already here, and what was not
//!
//! The key hierarchy was done before this module existed and is not touched
//! by it. [`crate::rest::sync`]'s `CipherKeys::owner_of` resolves a cipher's
//! `organizationId` to that organisation's key and falls back to the user
//! key, and [`VaultKeys::unwrap_from`] has held the org keys since the REST
//! backend was written. **An organisation-owned item already opens.** That is
//! normally the risky half of organisation support, and it was not the half
//! that was missing.
//!
//! What was missing is everything above it: nothing knew an item was shared,
//! with whom, or under what name. This module is that layer and nothing else.
//!
//! # Read only, and the line is drawn at the key
//!
//! Nothing here writes. There is no create, no invite, no policy edit and --
//! most deliberately -- **no move of an item into a collection**. That last
//! one is not an omission of convenience: moving a personal item into an
//! organisation re-encrypts it from the user key to the organisation key,
//! which is the one operation in this feature that changes what a cipher is
//! encrypted under. Everything in this file reads values that are already
//! wrapped under a key this account holds. See [`Directory`]'s closing
//! section for what the move would actually take.
//!
//! # Three round trips become zero, and then one per organisation
//!
//! The obvious build is a fetch per noun: `GET /api/organizations`,
//! `GET /api/collections`, and a members call. Two of those three are already
//! paid for. `GET /api/sync` carries `profile.organizations[]` -- the array
//! [`VaultKeys`] takes the org keys out of, so it is load-bearing already and
//! cannot be dropped -- and a top-level `collections[]` beside `ciphers` and
//! `folders`, which [`crate::rest::sync::SyncResponse`] now parses instead of
//! discarding. Both arrive at the same instant as the items they describe, so
//! a collection row and its item count cannot come from two different reads
//! of the account.
//!
//! Only the **roster** costs a request, because no sync carries one:
//! `profile.organizations[].users` is `[]` on this deployment and absent on
//! others. So [`Directory::load`] makes one `GET` per organisation --
//! **and zero for an account with none**, which is this owner's own
//! situation and the case the whole design is arranged not to charge for.
//!
//! # Every failure is the same failure, on purpose
//!
//! A server that has never heard of `/api/organizations/{id}/members` -- a
//! stock Bitwarden, which spells it `/users`; an older NodeWarden, which had
//! no route at all -- answers 404. A member who may not read the roster gets
//! 403. A transport error is a transport error. All three, plus a body that
//! is not a list, become [`Roster::Unknown`], which is the *same* state as
//! "not fetched yet" and which every surface already has to draw. There is no
//! error screen anywhere in this feature: an account whose server implements
//! none of these routes gets an empty [`Directory`], the ORGANISATIONS
//! section draws nothing at all, and the app behaves exactly as it did before
//! this module existed. That is asserted, not hoped for; see
//! `a_server_that_implements_none_of_this_degrades_to_an_empty_directory`.
//!
//! # Where the directory is held
//!
//! In this module, in a process-wide slot ([`current`], [`remember`]), filled
//! by the sync [`crate::rest::backend::RestBackend::synced`] already makes.
//!
//! **That is a second cache, and `rest::backend`'s module docs argue against
//! second caches.** The argument there is precise and it does not reach this
//! one: two caches disagree about a *vault*, because each decides for itself
//! what the vault is. This holds no vault. It holds a directory of
//! organisations and collections, it is written only by the sync that
//! produced the items the window is drawing, and it is replaced wholesale on
//! every such sync. The one disagreement it *can* have is stated rather than
//! denied: [`crate::vault_cache::VaultCache`] may still be showing an older
//! snapshot than the last sync, in which case a collection created since then
//! has a row and a count of zero. It corrects itself on the next populate,
//! and a row that is briefly empty is a much smaller untruth than the
//! alternative -- threading a new value through `VaultBackend`,
//! `VaultSnapshot` and every forwarding backend so that four impls that have
//! nothing to say about organisations each have to say something.

use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::rest::api::{RestClient, Session};
use crate::rest::crypto::{EncString, SymmetricKey, decrypt};
use crate::rest::sync::{SyncResponse, VaultKeys};
use crate::vault_bridge::VaultItem;

/// What a collection row says when its name is ciphertext this account
/// cannot open.
///
/// Its own constant and not an empty string, for [`crate::rest::sync`]'s rule
/// about a nameless item: a blank row is indistinguishable from a collection
/// somebody really did call nothing, and the whole point of recording a
/// decryption failure is that the two are different.
pub const UNREADABLE_NAME: &str = "(name unreadable)";

// ---- sharing organisations ---------------------------------------------------
//
// Personal sharing (spec 5.1) puts one hidden organisation behind each sharer,
// named `Shared by <sharer email>` in plaintext. The name is the one place
// every client can read -- the recipient's Deskwarden must recognise SOMEONE
// ELSE'S sharing org to hide it and to say "from ...", and so must the
// sharer's second PC -- so recognition is by name, and "mine" is the name plus
// ownership.

/// The exact prefix a sharing organisation's name starts with.
pub const SHARING_ORG_PREFIX: &str = "Shared by ";

/// Official servers probably cap org names at 50 (spec 4.3); names are kept
/// within it everywhere, which leaves 40 characters for the email.
const SHARING_ORG_NAME_LIMIT: usize = 50;

/// An email as sharing compares it: trimmed and lower-cased.
#[must_use]
pub fn normalise_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// The name a sharer's organisation is created with. An email longer than 40
/// characters is cut to 39 and `…`, so the label reads
/// `from denis.platonov@a-very-long-dom…` -- which is honest.
#[must_use]
pub fn sharing_org_name(email: &str) -> String {
    let email = normalise_email(email);
    let room = SHARING_ORG_NAME_LIMIT - SHARING_ORG_PREFIX.len();
    let shown = if email.chars().count() > room {
        let cut: String = email.chars().take(room - 1).collect();
        format!("{cut}\u{2026}")
    } else {
        email
    };
    format!("{SHARING_ORG_PREFIX}{shown}")
}

/// The sharer a name claims, when it is a sharing org's: the prefix, then
/// something that parses as an email (one `@`, no whitespace), optionally cut
/// with `…`.
#[must_use]
pub fn sharer_of(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(SHARING_ORG_PREFIX)?;
    let bare = rest.strip_suffix('\u{2026}').unwrap_or(rest);
    let mut halves = bare.split('@');
    let (Some(local), Some(domain), None) = (halves.next(), halves.next(), halves.next()) else {
        return None;
    };
    let plausible = !local.is_empty()
        && !domain.is_empty()
        && !bare.chars().any(char::is_whitespace);
    plausible.then_some(rest)
}

/// Whether `claimed` -- what [`sharer_of`] read out of a name -- is `email`.
/// A cut name matches every address it is a prefix of and that was long
/// enough to have been cut.
#[must_use]
pub fn sharer_is(claimed: &str, email: &str) -> bool {
    let email = normalise_email(email);
    match claimed.strip_suffix('\u{2026}') {
        Some(cut) => email.starts_with(&normalise_email(cut)) && sharing_org_name(&email).ends_with('\u{2026}'),
        None => normalise_email(claimed) == email,
    }
}

/// **Whether an organisation is a sharing org, and whose** -- the name's
/// claim checked against the server's attestation (controller decision,
/// final review).
///
/// Anyone can create an organisation named "Shared by boss@..." and add
/// anyone to it; read by name alone, its records showed "Shared by boss@...
/// · view-only". So a name-only claim is never shown as "Shared by":
///
/// - **This account's own**: it owns the org and the name is its own
///   address -- nothing to attest, the account made it. Only refused when
///   the server attests a DIFFERENT owner.
/// - **Someone else's**, where `owner_email` (NodeWarden's `ownerEmail`)
///   matches the name: the sharer is the name's, as before.
/// - `owner_email` present and NOT the name: a forged name. The org is a
///   sharing org received from its REAL owner -- they are sharing with this
///   account, whatever they called it -- and the sharer shown is theirs.
///   One whose attested owner is this account itself is not a sharing org
///   at all (its name claims someone else; it is shown as the org it is).
/// - No `owner_email` (a server that does not attest): not a sharing org.
///   It is drawn as the ordinary organisation it provably is.
///
/// `claimed` is [`sharer_of`] of the name; `None` is no sharing name at all.
#[must_use]
pub fn sharing_mark(claimed: Option<&str>, owner_email: Option<&str>, is_owner: bool, account_email: &str) -> Option<SharingOrg> {
    let claimed = claimed?;
    let owner = owner_email.map(normalise_email).filter(|o| !o.is_empty());
    let account = normalise_email(account_email);
    let names_me = !account.is_empty() && sharer_is(claimed, &account);
    // **The attested owner is the sharer shown** wherever there is one
    // (final re-review, Minor 3): the name is cut to fit the org name's 50
    // characters (`sharing_org_name`), the attestation is the whole address.
    if is_owner && names_me && owner.as_deref().is_none_or(|o| o == account) {
        return Some(SharingOrg { sharer: owner.unwrap_or_else(|| claimed.to_string()), mine: true });
    }
    match owner {
        Some(owner) if sharer_is(claimed, &owner) => Some(SharingOrg { sharer: owner, mine: false }),
        Some(owner) if owner == account => None,
        Some(owner) => Some(SharingOrg { sharer: owner, mine: false }),
        None => None,
    }
}

/// A sharing organisation, as the directory marks one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharingOrg {
    /// The sharer's email as the name carries it (possibly cut with `…`).
    pub sharer: String,
    /// This account owns it and is the sharer it names.
    pub mine: bool,
}

/// One organisation this account is a member of.
///
/// `name` is plaintext on **every** implementation and that is not an
/// oversight -- see [`crate::rest::sync::Organization::name`], which carries
/// the reason (an invitee has no org key, and the name is on the invitation).
/// The collection names underneath are the ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organisation {
    pub id: String,
    /// `None` when the server sent no name. The row falls back to the id,
    /// which is ugly and true, rather than to a placeholder that would make
    /// two different organisations look like the same one.
    pub name: Option<String>,
    /// Whether the organisation is switched on. A server that did not say is
    /// read as enabled -- see [`crate::rest::sync::Organization::enabled`].
    pub enabled: bool,
    /// `Some` for a personal-sharing organisation -- hidden from every
    /// organisations surface, see [`Directory::visible_organisations`].
    pub sharing: Option<SharingOrg>,
}

impl Organisation {
    /// What to print for this organisation. Never empty.
    #[must_use]
    pub fn label(&self) -> &str {
        match self.name.as_deref() {
            Some(name) if !name.trim().is_empty() => name,
            _ => &self.id,
        }
    }
}

/// One collection inside one organisation.
///
/// **The name is the only ciphertext in this whole directory**, and it is
/// wrapped under the *organisation's* key rather than the user's -- which is
/// why [`Directory::from_sync`] needs a [`VaultKeys`] at all and why
/// [`VaultKeys::organization`] exists.
///
/// A plain `String` and not a `Zeroizing<String>`, matching
/// [`crate::vault_bridge::Folder::name`] exactly: a folder name is encrypted
/// under the user key and is modelled as a plain `String` for the same
/// reason, which is that the value's destination is an egui label and a
/// `Zeroizing<String>` handed to a painter is wiped no more thoroughly than a
/// `String` is. Consistency with the neighbouring concept is worth more here
/// than a wipe that does not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    pub id: String,
    pub organization_id: String,
    /// `None` for a name that arrived as ciphertext this account could not
    /// open -- an organisation whose key failed to unwrap, most likely. See
    /// [`UNREADABLE_NAME`], which is what gets drawn.
    pub name: Option<String>,
    /// **The name decrypted under the org key**, so only a holder of that
    /// key can have written it. `false` for a plaintext name -- which
    /// NodeWarden's web vault may leave, and which the SERVER can write too.
    /// A plaintext name is drawn like any other, but nothing that grants or
    /// confirms trusts it: the sharing engine's rule 2 (spec 11.5) reads a
    /// person collection's email off an authenticated name only, or a hostile
    /// server could list `mallory@...` in plaintext beside an accepted row
    /// and have the org key wrapped to its own key
    /// (`a_plaintext_collection_name_never_confirms_anyone`).
    pub authenticated: bool,
}

impl Collection {
    /// What to print for this collection. Never empty.
    #[must_use]
    pub fn label(&self) -> &str {
        match self.name.as_deref() {
            Some(name) if !name.trim().is_empty() => name,
            _ => UNREADABLE_NAME,
        }
    }
}

/// Where one membership stands. Bitwarden's three steps (spec 2): invited
/// (a row, no account attached yet on NodeWarden), accepted (no org key yet),
/// confirmed (holds the org key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemberStatus {
    Invited,
    Accepted,
    Confirmed,
}

/// One member of one organisation. See the module docs for what it carries
/// and why it is thin; personal sharing added the org-user id, the account id,
/// the three-state status, which collections are read-only, and an
/// invitation's expiry -- each the answer to a question the confirm pass or
/// the SHARING card asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The org-user id (`ouId`) the membership routes take. On NodeWarden's
    /// older `/members` rows it is the account id, which is what NodeWarden's
    /// membership routes take.
    pub id: String,
    /// The account behind the membership; `None` for an invitation.
    pub user_id: Option<String>,
    pub email: String,
    /// The display name, when the server sent one. Not used for counting;
    /// kept so a surface that wants to *name* the other viewers can.
    pub name: Option<String>,
    /// Lower-cased, as NodeWarden's `normalizeRole` writes it: `owner`,
    /// `admin`, `manager`, `member`, `custom`.
    pub role: String,
    pub status: MemberStatus,
    /// The collection ids listed against this member.
    pub collections: Vec<String>,
    /// The subset of `collections` held read-only.
    pub read_only: Vec<String>,
    /// NodeWarden's `invitationExpirationDate`, for an invitation.
    pub invitation_expires: Option<String>,
}

impl Member {
    /// Holds the org key: Bitwarden's status 2.
    #[must_use]
    pub fn confirmed(&self) -> bool {
        self.status == MemberStatus::Confirmed
    }

    /// Listed against `collection_id`.
    #[must_use]
    pub fn has_access(&self, collection_id: &str) -> bool {
        self.collections.iter().any(|c| c == collection_id)
    }

    /// Whether this member reaches every collection in the organisation
    /// without being listed against any of them.
    ///
    /// An owner or an admin does. This is the reason the roster cannot be
    /// counted by intersecting collection lists alone: an organisation's
    /// owner is frequently listed against **no** collections and can still
    /// read every item in it, so a count that only intersected would report
    /// "0 other people" on an item the owner is reading right now.
    #[must_use]
    pub fn reaches_everything(&self) -> bool {
        matches!(self.role.as_str(), "owner" | "admin")
    }

    /// Whether this member can see an item filed in `collections`.
    ///
    /// An item with **no** collections at all is reachable only by the
    /// members who reach everything. That is the honest reading of the wire:
    /// an organisation cipher filed into nothing is visible to the
    /// organisation's administrators and to nobody else, which is what a
    /// server's own access check computes.
    #[must_use]
    pub fn reaches(&self, collections: &[&str]) -> bool {
        self.reaches_everything()
            || collections.iter().any(|id| self.collections.iter().any(|mine| mine == id))
    }
}

/// What is known about one organisation's members.
///
/// **Two states and not an `Option<Vec<Member>>`**, because the empty vector
/// would then mean two opposite things -- "this organisation has no other
/// members" and "the list came back as something this app could not read" --
/// and every surface would have to remember which. `Unknown` is the state a
/// server without the route produces, the state a 403 produces, and the state
/// before the fetch happens; `Known(vec![])` is a real, and very unusual,
/// organisation of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Roster {
    Unknown,
    Known(Vec<Member>),
}

/// **The answer** -- what [`Directory::audience_of`] hands back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Audience {
    /// This item belongs to the account and to nobody else.
    Personal,
    /// This item is owned by an organisation.
    Shared {
        /// The organisation's own label, ready to print.
        organisation: String,
        /// The labels of the collections this item is filed in, in the order
        /// the directory holds them. Empty for an organisation item filed in
        /// nothing, which is a real state -- see [`Member::reaches`].
        collections: Vec<String>,
        /// How many people **other than the account holder** can see it, or
        /// `None` when the roster is [`Roster::Unknown`].
        ///
        /// The viewer is subtracted because they are necessarily in the set:
        /// this count is only ever asked about an item that is on the
        /// viewer's screen, which means the server let them read it, which
        /// means they reach it. `saturating_sub` and not `- 1` because a
        /// server that returned a roster not containing the viewer would
        /// otherwise underflow, and "0 other people" is the right answer to
        /// a roster this app cannot find itself in.
        others: Option<usize>,
    },
    /// In this account's own sharing org: the people it is shared with, by
    /// their person collections' names. Never names the organisation.
    SharedWithPeople { people: Vec<String> },
    /// In someone else's sharing org: who shared it.
    SharedWithMe { from: String },
}

impl Audience {
    /// Whether this item is owned by an organisation at all.
    #[must_use]
    pub fn is_shared(&self) -> bool {
        !matches!(self, Audience::Personal)
    }
}

/// Every organisation and collection this account can reach, and what is
/// known about who is in them.
///
/// # What moving an item into a collection would take
///
/// Recorded here because it is the obvious next slice and because this
/// module's shape is what it would be built on. It is **not** implemented and
/// nothing here half-implements it.
///
/// The write itself is `POST /api/ciphers/{id}/share`, whose body is the
/// whole cipher plus a `collectionIds` array. The hard part is not the route:
///
/// 1. **Every field is re-encrypted.** A personal cipher's fields are wrapped
///    under the user key (or under the cipher's own key, which is wrapped
///    under the user key -- [`crate::rest::sync`]'s step 5). An organisation
///    cipher's are wrapped under the org key. So the move is decrypt-all,
///    re-encrypt-all, which means the plaintext of every field of the item
///    exists in this process for the duration, and `rest::write`'s rule -- a
///    value that never decrypted must not be re-encrypted, because doing so
///    buries it -- becomes load-bearing for a whole item at once rather than
///    for one edited field.
/// 2. **`attachments` cannot come with it.** Each attachment has its own key,
///    wrapped under the cipher's, and this crate does not decrypt attachments
///    at all (`rest::sync`'s own doc says so). A share that left them behind
///    would silently lose files; a share that refused an item with
///    attachments is honest and is probably the right first version.
/// 3. **It is not reversible from here.** There is no "move back out" route:
///    the web vault does it by cloning into a personal item and deleting the
///    org one. So the confirmation in front of it has to say so.
/// 4. **The failure mode is the worst in the app.** If the `POST` half-lands
///    -- server takes the cipher, rejects the collection ids -- the item is
///    now encrypted under a key the *organisation* holds and filed nowhere.
///    It is still readable (the account has the org key), but it has left the
///    personal vault and no local state records that it was ever there.
///
/// None of that is a reason not to build it. It is a reason it is its own
/// slice with its own confirmation, its own refusal for attachment-carrying
/// items, and its own test for the half-landed write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directory {
    organisations: Vec<Organisation>,
    collections: Vec<Collection>,
    /// `(organisation id, what is known about its members)`. A `Vec` and not
    /// a map for [`VaultKeys`]'s own reason, one level up: a user is in a
    /// handful of organisations at most, and a linear scan over four entries
    /// is not worth a hash.
    rosters: Vec<(String, Roster)>,
    /// Accepted-but-unconfirmed sharing orgs from `profile.organizationsNew`
    /// -- the recipient's "waiting for X to finish sharing".
    waiting: Vec<Organisation>,
}

/// **Nothing shared at all, as a `static`** -- so a caller that needs a
/// `&Directory` and has none can have one without allocating.
///
/// `sidebar::VaultLists` holds a borrow rather than an owned directory so
/// that it stays `Copy`, and its `live_only` constructor -- the one every
/// caller starts from, and the one every test in three files uses -- has no
/// directory to borrow. This is what it borrows. A `static` and not a `const`
/// because `&SOME_CONST` of a type that owns a `Vec` cannot be promoted to
/// `'static`; the `Vec::new()`s below are const, so the `static` itself costs
/// no initialiser and no allocation.
pub static NOTHING_SHARED: Directory = Directory {
    organisations: Vec::new(),
    collections: Vec::new(),
    rosters: Vec::new(),
    waiting: Vec::new(),
};

impl Directory {
    /// **Nothing shared at all** -- the whole answer for an account with no
    /// organisations, for a `bw serve` backend, and for a server that
    /// implements none of these routes.
    ///
    /// All three produce this, and they must: the rail asks only
    /// [`Self::is_empty`], so a section that is dead weight for an account
    /// with no organisations is structurally impossible rather than a rule
    /// the drawing code has to keep.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether there is anything at all to show.
    ///
    /// **Organisations, and deliberately not collections.** An organisation
    /// with no collections is a real thing and its row is still worth drawing
    /// -- items can be organisation-owned and filed in nothing. A collection
    /// with no organisation cannot happen, so the other direction is not a
    /// state to test for.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.organisations.is_empty()
    }

    /// Every organisation, in the order the server listed them.
    #[must_use]
    pub fn organisations(&self) -> &[Organisation] {
        &self.organisations
    }

    /// The organisations an organisations surface may show: every one but the
    /// sharing orgs, which are plumbing (spec 6.9).
    #[must_use]
    pub fn visible_organisations(&self) -> Vec<&Organisation> {
        self.organisations.iter().filter(|o| o.sharing.is_none()).collect()
    }

    /// Every sharing organisation this account is in, its own and others'.
    #[must_use]
    pub fn sharing_orgs(&self) -> Vec<&Organisation> {
        self.organisations.iter().filter(|o| o.sharing.is_some()).collect()
    }

    /// This account's own sharing org: the first in server order, which on
    /// NodeWarden is creation order. The sharing engine prefers the id it
    /// cached (`sharing_state.json`) over this.
    #[must_use]
    pub fn my_sharing_org(&self) -> Option<&Organisation> {
        self.organisations.iter().find(|o| o.sharing.as_ref().is_some_and(|s| s.mine))
    }

    /// Sharing orgs this account has accepted and the sharer has not confirmed.
    #[must_use]
    pub fn waiting(&self) -> &[Organisation] {
        &self.waiting
    }

    /// The person collection for `email` in `org`, by decrypted name, case
    /// folded. Where two decrypt to one email, the first in server order.
    #[must_use]
    pub fn collection_named(&self, org: &str, email: &str) -> Option<&Collection> {
        let email = normalise_email(email);
        self.collections
            .iter()
            .find(|c| c.organization_id == org && c.name.as_deref().map(normalise_email) == Some(email.clone()))
    }

    /// The collections belonging to one organisation, in server order.
    #[must_use]
    pub fn collections_of(&self, organization_id: &str) -> Vec<&Collection> {
        self.collections.iter().filter(|c| c.organization_id == organization_id).collect()
    }

    /// What is known about one organisation's members.
    #[must_use]
    pub fn roster_of(&self, organization_id: &str) -> &Roster {
        self.rosters
            .iter()
            .find(|(id, _)| id == organization_id)
            .map_or(&Roster::Unknown, |(_, roster)| roster)
    }

    /// **The question this module exists for.**
    ///
    /// Pure, and over the item the window already holds: no request, no lock,
    /// no clock. A caller may ask it per row of a list without thinking about
    /// the cost.
    ///
    /// An item whose `organizationId` names an organisation this directory
    /// does not know is still [`Audience::Shared`] -- the item says it is
    /// owned by somebody, and answering `Personal` because the directory is
    /// incomplete would be the one wrong answer that matters. The
    /// organisation's label falls back to its id and the roster to `None`.
    #[must_use]
    pub fn audience_of(&self, item: &VaultItem) -> Audience {
        let Some(org_id) = organization_of(item) else {
            return Audience::Personal;
        };
        let filed_in = collection_ids_of(item);
        let organisation = self
            .organisations
            .iter()
            .find(|o| o.id == org_id)
            .map_or_else(|| org_id.to_string(), |o| o.label().to_string());
        if let Some(sharing) = self.organisations.iter().find(|o| o.id == org_id).and_then(|o| o.sharing.as_ref()) {
            return if sharing.mine {
                Audience::SharedWithPeople {
                    people: filed_in
                        .iter()
                        .filter_map(|id| self.collections.iter().find(|c| c.id == *id))
                        .filter_map(|c| c.name.clone())
                        .collect(),
                }
            } else {
                Audience::SharedWithMe { from: sharing.sharer.clone() }
            };
        }
        let collections = filed_in
            .iter()
            .filter_map(|id| self.collections.iter().find(|c| c.id == *id))
            .map(|c| c.label().to_string())
            .collect();
        let others = match self.roster_of(org_id) {
            Roster::Unknown => None,
            // Anyone past the invitation counts as a viewer -- the rule this
            // count always had (status "not zero"); a member still waiting
            // for the key is about to see it.
            Roster::Known(members) => Some(
                members
                    .iter()
                    .filter(|m| m.status != MemberStatus::Invited && m.reaches(&filed_in))
                    .count()
                    // The viewer. See `Audience::Shared::others`.
                    .saturating_sub(1),
            ),
        };
        Audience::Shared { organisation, collections, others }
    }

    /// The directory a sync carries, with every roster still
    /// [`Roster::Unknown`].
    ///
    /// `keys` is needed for exactly one thing -- a collection's name, which
    /// is wrapped under its organisation's key. A collection whose
    /// organisation has no usable key (it failed to unwrap, and
    /// [`VaultKeys::unwrap_from`] skipped it rather than failing the sync)
    /// keeps a `None` name and is still listed: the row is worth having even
    /// unnamed, because the *items* behind it are the thing the user came
    /// for and they may well decrypt.
    #[must_use]
    pub fn from_sync(response: &SyncResponse, keys: &VaultKeys) -> Self {
        let account_email = response.profile.as_ref().and_then(|p| p.email.clone()).unwrap_or_default();
        // Whether `org` is a sharing organisation, and whose: its name's
        // claim checked against the server's `ownerEmail` (`sharing_mark`).
        // Shared between the confirmed organisations below and
        // `organizationsNew`'s not-yet-confirmed ones, since both carry both.
        let mark = |org: &crate::rest::sync::Organization| {
            sharing_mark(org.name.as_deref().and_then(sharer_of), org.owner_email.as_deref(), org.is_owner(), &account_email)
        };

        let organisations: Vec<Organisation> = response
            .profile
            .as_ref()
            .map(|profile| {
                profile
                    .organizations
                    .iter()
                    .map(|org| Organisation {
                        id: org.id.clone(),
                        name: org.name.clone(),
                        // A server that did not say is read as enabled. See
                        // `rest::sync::Organization::enabled`.
                        enabled: org.enabled.unwrap_or(true),
                        sharing: mark(org),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let collections = response
            .collections
            .iter()
            .filter_map(|raw| map_collection(raw, keys))
            .collect();

        let rosters = organisations.iter().map(|o| (o.id.clone(), Roster::Unknown)).collect();

        // `organizationsNew` carries every accepted-or-confirmed membership;
        // `organizations` above is confirmed only. What is in the first and
        // not the second is a sharing org this account has accepted and the
        // sharer has not yet confirmed -- "waiting for X to finish sharing".
        // Only ever somebody else's: this account's own sharing org is
        // created already confirmed, so it can never appear only here.
        let waiting: Vec<Organisation> = response
            .profile
            .as_ref()
            .map(|profile| {
                profile
                    .organizations_new
                    .iter()
                    .filter(|org| !profile.organizations.iter().any(|confirmed| confirmed.id == org.id))
                    .filter_map(|org| {
                        let sharing = mark(org)?;
                        (!sharing.mine).then(|| Organisation {
                            id: org.id.clone(),
                            name: org.name.clone(),
                            enabled: org.enabled.unwrap_or(true),
                            sharing: Some(sharing),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self { organisations, collections, rosters, waiting }
    }

    /// Replaces what is known about one organisation's members.
    ///
    /// Takes the raw JSON rather than a parsed list, so that
    /// [`crate::rest::api`] holds no opinion about what a member is and this
    /// module holds all of it -- the same split `fetch_cipher` and
    /// `decrypt_cipher` already have.
    ///
    /// An answer this cannot read leaves the roster [`Roster::Unknown`]. It
    /// does **not** become `Known(vec![])`: see [`Roster`] for why that
    /// difference is the whole reason the type has two variants.
    pub fn set_roster(&mut self, organization_id: &str, answer: &Value) {
        let Some(members) = map_members(answer) else {
            return;
        };
        match self.rosters.iter_mut().find(|(id, _)| id == organization_id) {
            Some((_, slot)) => *slot = Roster::Known(members),
            None => self.rosters.push((organization_id.to_string(), Roster::Known(members))),
        }
    }

    /// **The whole fetch**: a sync's directory, plus one roster request per
    /// organisation.
    ///
    /// The sync is the caller's, already made and already parsed -- this
    /// costs no round trip of its own. **An account with no organisations
    /// makes no request at all**, which is the case this is arranged for; the
    /// loop simply does not run.
    ///
    /// Every roster failure is swallowed to [`Roster::Unknown`] by
    /// [`Self::set_roster`] not being reached. Nothing here can fail, and
    /// that is the signature: a server that implements none of this hands
    /// back a directory with organisations and collections read out of the
    /// sync (or an empty one, if it sends neither), and the app is exactly
    /// where it was.
    #[must_use]
    pub fn load(
        client: &RestClient,
        session: &mut Session,
        response: &SyncResponse,
        keys: &VaultKeys,
    ) -> Self {
        let mut directory = Self::from_sync(response, keys);
        // **Not other people's sharing orgs.** A recipient is a plain member,
        // and NodeWarden answers a plain member's roster request with 404 on
        // both routes -- two requests per org per sync for nothing. Their
        // rosters are also none of this account's business (spec 10.2).
        let wanted: Vec<String> = directory
            .organisations
            .iter()
            .filter(|o| o.sharing.as_ref().is_none_or(|s| s.mine))
            .map(|o| o.id.clone())
            .collect();
        for id in wanted {
            if let Ok(answer) = client.fetch_organization_members(session, &id) {
                directory.set_roster(&id, &answer);
            }
        }
        directory
    }
}

/// The organisation that owns this item, or `None` for a personal one.
///
/// **Read off [`VaultItem::other`], where it already is.** `organizationId`
/// is not a modelled field and does not need to be: [`crate::rest::sync`]'s
/// mapper carries every key it does not understand through `other` verbatim,
/// so this fact has been in every item this app has held since the REST
/// backend landed. Modelling it would mean a new field on `VaultItem`, a new
/// `skip_serializing_if`, and a new thing for `rest::write` to get right on
/// every `PUT` -- for a value nothing writes.
///
/// A `null` or an empty string is `None`. NodeWarden's `normalizeOptionalId`
/// sends `null`; a server that sent `""` means the same thing, and an item
/// owned by an organisation with no id is not an item this app can say
/// anything true about.
#[must_use]
pub fn organization_of(item: &VaultItem) -> Option<&str> {
    item.other
        .get("organizationId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

/// The collections this item is filed in.
///
/// [`organization_of`]'s reasoning applied to the other half of the fact:
/// `collectionIds` rides `VaultItem::other` already. A missing key, a `null`
/// and an empty array are all the empty slice -- an organisation item filed
/// in nothing is a real state (see [`Member::reaches`]) and is not
/// distinguishable from a server that omitted the key, so they are not
/// distinguished.
#[must_use]
pub fn collection_ids_of(item: &VaultItem) -> Vec<&str> {
    item.other
        .get("collectionIds")
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_str).filter(|id| !id.is_empty()).collect())
        .unwrap_or_default()
}

/// One collection off the wire.
///
/// Returns `None` only for a value that is not an object, has no `id`, or
/// names no organisation -- a collection belonging to nothing cannot be filed
/// under an organisation row and would be a row with no parent.
fn map_collection(raw: &Value, keys: &VaultKeys) -> Option<Collection> {
    let object = raw.as_object()?;
    let id = object.get("id").and_then(Value::as_str)?.to_string();
    let organization_id = object
        .get("organizationId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?
        .to_string();
    let read = object
        .get("name")
        .and_then(Value::as_str)
        .and_then(|raw| collection_name(raw, keys.organization(&organization_id)));
    let (name, authenticated) = match read {
        Some((name, authenticated)) => (Some(name), authenticated),
        None => (None, false),
    };
    Some(Collection { id, organization_id, name, authenticated })
}

/// A collection's `name`, which is **ciphertext on Bitwarden and plaintext on
/// NodeWarden**, read without the caller having to know which server answered.
///
/// The rule is the parse, and it is a sound discriminator rather than a
/// guess: an [`EncString`] is `type.b64|b64|b64`, and a name a human typed
/// parses as one only if it begins `2.` or `4.` and every part is valid
/// base64 of exactly the right length. "Engineering" does not. So:
///
/// * does not parse -> it is a plaintext name, taken verbatim;
/// * parses and decrypts -> the plaintext inside;
/// * parses and does **not** decrypt -> `None`, which draws as
///   [`UNREADABLE_NAME`].
///
/// The third case is the one that matters and it is why this is not
/// `unwrap_or(raw)`: falling back to the raw text there would paint
/// `2.aB9d...|...|...` into the rail as though it were a collection's name.
///
/// The `bool` is [`Collection::authenticated`]: `true` in the decrypt branch
/// alone.
fn collection_name(raw: &str, key: Option<&SymmetricKey>) -> Option<(String, bool)> {
    let Ok(enc) = raw.parse::<EncString>() else {
        return Some((raw.to_string(), false));
    };
    let key = key?;
    let bytes = decrypt(key, &enc).ok()?;
    std::str::from_utf8(&bytes).ok().map(|name| (name.to_string(), true))
}

/// A members answer, read out of whichever envelope arrived.
///
/// `{"object":"list","data":[...]}` is what both NodeWarden and Bitwarden
/// send; a bare array is read too, because the envelope is presentation and
/// nothing about who is in an organisation turns on which one arrived. This
/// is [`crate::rest::api`]'s own "tolerant about the envelope, strict about
/// the item" rule, applied to the one route that module does not map.
///
/// `None` -- rather than an empty list -- for anything else, including a
/// `{"message":"Not found"}` error body that arrived with a 200. See
/// [`Roster`] on why that distinction is load-bearing.
///
/// `pub(crate)` for the sharing engine, which reads a fresh roster through the same reader.
pub(crate) fn map_members(answer: &Value) -> Option<Vec<Member>> {
    let rows = match answer.get("data") {
        Some(Value::Array(rows)) => rows,
        _ => answer.as_array()?,
    };
    Some(rows.iter().filter_map(map_member).collect())
}

/// One member.
///
/// `None` for a row that is not an object or carries no id: a member this app
/// cannot name is a member it cannot count without double-counting, since two
/// idless rows are indistinguishable.
fn map_member(raw: &Value) -> Option<Member> {
    let object = raw.as_object()?;
    let id = object.get("id").and_then(Value::as_str)?.to_string();
    // **Two row shapes, one reader.** `/users` rows (official, NodeWarden from
    // slice 1) name the account in `userId` and say 0/1/2 in `status`.
    // NodeWarden's older `/members` rows ARE the account -- `id` is the user
    // id -- and say "accepted" as status 1 with `encryptedOrgKey: null`, which
    // the old reader counted as confirmed. That was right for counting viewers
    // and wrong for confirming (spec 3.2).
    let legacy = object.contains_key("encryptedOrgKey");
    let status = match object.get("status").and_then(Value::as_i64) {
        // A server that sent no status is read as confirmed: the row is in
        // the roster, and discounting it would under-count who can see an
        // item.
        None => MemberStatus::Confirmed,
        Some(0) => MemberStatus::Invited,
        Some(1) if legacy => {
            if object.get("encryptedOrgKey").and_then(Value::as_str).is_some_and(|k| !k.is_empty()) {
                MemberStatus::Confirmed
            } else {
                MemberStatus::Accepted
            }
        }
        Some(1) => MemberStatus::Accepted,
        Some(_) => MemberStatus::Confirmed,
    };
    let user_id = match object.get("userId") {
        Some(value) => value.as_str().filter(|s| !s.is_empty()).map(str::to_string),
        None if legacy => Some(id.clone()),
        None => None,
    };
    // NodeWarden writes the role as a word and a constant `type: 0` beside it;
    // official rows carry only the number. The word wins.
    let role = match object.get("role").and_then(Value::as_str) {
        Some(role) => role.to_ascii_lowercase(),
        None => match object.get("type").and_then(Value::as_i64) {
            Some(0) => "owner",
            Some(1) => "admin",
            Some(3) => "manager",
            Some(4) => "custom",
            _ => "member",
        }
        .to_string(),
    };
    let mut collections = Vec::new();
    let mut read_only = Vec::new();
    for entry in object.get("collections").and_then(Value::as_array).into_iter().flatten() {
        // Bare id strings (NodeWarden's older rows) or `{id, readOnly, ..}`.
        let Some(cid) = entry.as_str().or_else(|| entry.get("id").and_then(Value::as_str)).filter(|id| !id.is_empty())
        else {
            continue;
        };
        if entry.get("readOnly").and_then(Value::as_bool) == Some(true) {
            read_only.push(cid.to_string());
        }
        collections.push(cid.to_string());
    }
    Some(Member {
        id,
        user_id,
        email: object.get("email").and_then(Value::as_str).unwrap_or_default().to_string(),
        name: object.get("name").and_then(Value::as_str).map(str::to_string),
        role,
        status,
        collections,
        read_only,
        invitation_expires: object
            .get("invitationExpirationDate")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

// ---- the directory on the wire ----------------------------------------------
//
// **The daemon owns the vault, and so it owns this too.** A window reading the
// daemon's vault (`crate::daemon_link`) makes no sync of its own, so nothing
// in its process ever builds a directory -- and its rail drew no
// organisations at all. The daemon serves its own as `GET /organizations`
// (`crate::vault_api`), in this one shape, and the window holds what it is
// handed exactly as it would hold one it had built.
//
// Everything a directory holds crosses: the collection names already
// decrypted, and the rosters -- member emails and roles -- because `audience_
// of` is the question a client asks of it, and a directory without them
// answers "unknown" where an unlinked window answers a count. Nothing here is
// a key or a vault item. A client-agnostic shape: organisations, collections,
// rosters, and nothing about which surface draws them.

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireDirectory {
    organisations: Vec<WireOrganisation>,
    collections: Vec<WireCollection>,
    rosters: Vec<WireRoster>,
    #[serde(default)]
    waiting: Vec<WireOrganisation>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireOrganisation {
    id: String,
    name: Option<String>,
    enabled: bool,
    #[serde(default)]
    sharer: Option<String>,
    #[serde(default)]
    mine: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCollection {
    id: String,
    organization_id: String,
    name: Option<String>,
    /// [`Collection::authenticated`]. Absent in a body from before it
    /// existed, and read as `false`: unauthenticated is the safe default.
    #[serde(default)]
    authenticated: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRoster {
    organization_id: String,
    /// `None` is [`Roster::Unknown`] -- not an empty list, for that type's
    /// whole reason.
    members: Option<Vec<WireMember>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireMember {
    id: String,
    #[serde(default)]
    user_id: Option<String>,
    email: String,
    name: Option<String>,
    role: String,
    #[serde(default = "confirmed_status")]
    status: MemberStatus,
    collections: Vec<String>,
    #[serde(default)]
    read_only: Vec<String>,
    #[serde(default)]
    invitation_expires: Option<String>,
}

/// [`WireMember::status`]'s default -- a wire row from before this field
/// existed (the daemon's own cache, or `daemon_link`'s fixed test JSON) said
/// nothing about status and meant confirmed: the old wire's `confirmed: bool`
/// had no other value.
fn confirmed_status() -> MemberStatus {
    MemberStatus::Confirmed
}

/// One [`Organisation`], to the wire. Shared by `organisations` and `waiting`,
/// which are the same shape wearing two different hats.
fn organisation_to_wire(o: &Organisation) -> WireOrganisation {
    WireOrganisation {
        id: o.id.clone(),
        name: o.name.clone(),
        enabled: o.enabled,
        sharer: o.sharing.as_ref().map(|s| s.sharer.clone()),
        mine: o.sharing.as_ref().is_some_and(|s| s.mine),
    }
}

/// The inverse of [`organisation_to_wire`].
fn organisation_from_wire(o: WireOrganisation) -> Organisation {
    Organisation {
        id: o.id,
        name: o.name,
        enabled: o.enabled,
        sharing: o.sharer.map(|sharer| SharingOrg { sharer, mine: o.mine }),
    }
}

impl Directory {
    /// This directory as the JSON `GET /organizations` answers with.
    #[must_use]
    pub fn to_body(&self) -> Value {
        let wire = WireDirectory {
            organisations: self.organisations.iter().map(organisation_to_wire).collect(),
            collections: self
                .collections
                .iter()
                .map(|c| WireCollection {
                    id: c.id.clone(),
                    organization_id: c.organization_id.clone(),
                    name: c.name.clone(),
                    authenticated: c.authenticated,
                })
                .collect(),
            rosters: self
                .rosters
                .iter()
                .map(|(organization_id, roster)| WireRoster {
                    organization_id: organization_id.clone(),
                    members: match roster {
                        Roster::Unknown => None,
                        Roster::Known(members) => Some(
                            members
                                .iter()
                                .map(|m| WireMember {
                                    id: m.id.clone(),
                                    user_id: m.user_id.clone(),
                                    email: m.email.clone(),
                                    name: m.name.clone(),
                                    role: m.role.clone(),
                                    status: m.status,
                                    collections: m.collections.clone(),
                                    read_only: m.read_only.clone(),
                                    invitation_expires: m.invitation_expires.clone(),
                                })
                                .collect(),
                        ),
                    },
                })
                .collect(),
            waiting: self.waiting.iter().map(organisation_to_wire).collect(),
        };
        serde_json::to_value(wire).unwrap_or(Value::Null)
    }

    /// A directory read back out of [`Self::to_body`]'s JSON.
    ///
    /// # Errors
    ///
    /// A body that is not a directory.
    pub fn from_body(body: Value) -> Result<Self, String> {
        let wire: WireDirectory =
            serde_json::from_value(body).map_err(|e| format!("not a sharing directory: {e}"))?;
        Ok(Self {
            organisations: wire.organisations.into_iter().map(organisation_from_wire).collect(),
            collections: wire
                .collections
                .into_iter()
                .map(|c| Collection { id: c.id, organization_id: c.organization_id, name: c.name, authenticated: c.authenticated })
                .collect(),
            rosters: wire
                .rosters
                .into_iter()
                .map(|r| {
                    let roster = match r.members {
                        None => Roster::Unknown,
                        Some(members) => Roster::Known(
                            members
                                .into_iter()
                                .map(|m| Member {
                                    id: m.id,
                                    user_id: m.user_id,
                                    email: m.email,
                                    name: m.name,
                                    role: m.role,
                                    status: m.status,
                                    collections: m.collections,
                                    read_only: m.read_only,
                                    invitation_expires: m.invitation_expires,
                                })
                                .collect(),
                        ),
                    };
                    (r.organization_id, roster)
                })
                .collect(),
            waiting: wire.waiting.into_iter().map(organisation_from_wire).collect(),
        })
    }
}

// ---- where the directory is held --------------------------------------------

/// The one held directory. See the module docs for why it is here rather than
/// threaded through `VaultBackend`.
static HELD: Mutex<Option<Arc<Directory>>> = Mutex::new(None);

/// Stores `directory` as this process's current one.
///
/// Called by [`crate::rest::backend::RestBackend::synced`], which builds it,
/// and by `crate::daemon_link`'s backend, which is handed the daemon's own
/// alongside the vault it loads -- so in both kinds of process "the directory
/// describes the vault the window is drawing" is true by construction rather
/// than by scheduling.
pub fn remember(directory: Directory) {
    let mut held = HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *held = Some(Arc::new(directory));
}

/// The current directory, or an empty one.
///
/// **Never an `Option`**, and that is the whole ergonomics of the feature: a
/// `bw serve` account, a REST account that has not synced yet, and a REST
/// account on a server with no organisation routes are three different
/// situations that want exactly one behaviour -- draw nothing -- and an
/// `Option` would make three call sites each decide that again.
///
/// An `Arc` clone, so a frame closure holds the lock for the length of one
/// pointer copy and never across a paint.
#[must_use]
pub fn current() -> Arc<Directory> {
    let held = HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    held.as_ref().map_or_else(|| Arc::new(Directory::empty()), Arc::clone)
}

/// Puts the held directory back to nothing.
///
/// **For a lock and for an account switch**, on exactly
/// [`crate::rest::send`]'s `active_account` reasoning: a directory taken for
/// one account is a statement about somebody else's sharing the moment the
/// account changes, and a rail that kept drawing `Engineering` after a switch
/// would be naming an organisation the signed-in user may not be in.
///
/// Also `pub` for the tests in this file and in `vault_window`, which is the
/// honest reason it is not `pub(crate)`: a process-wide slot that tests
/// cannot reset is a slot whose tests depend on their own order.
pub fn forget() {
    let mut held = HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *held = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest::api::{Device, RestClient, RestError};
    use crate::rest::crypto::encrypt;
    use crate::rest::sync::tests::keys_with_organisation;
    use serde_json::json;

    const ORG: &str = "11111111-1111-1111-1111-111111111111";
    const COLLECTION: &str = "22222222-2222-2222-2222-222222222222";

    /// Sixty-four bytes of a user key, and sixty-four of an organisation's.
    /// Invented; nothing here reaches a real server or a real vault.
    fn keys() -> VaultKeys {
        keys_with_organisation(&[7u8; 64], ORG, &[9u8; 64])
    }

    /// One item as the window holds it: the two facts this module reads ride
    /// `other`, which is where `rest::sync`'s mapper leaves them.
    fn item(organization_id: Option<&str>, collections: &[&str]) -> VaultItem {
        let mut other = serde_json::Map::new();
        other.insert(
            "organizationId".to_string(),
            organization_id.map_or(Value::Null, |id| Value::String(id.to_string())),
        );
        other.insert(
            "collectionIds".to_string(),
            Value::Array(collections.iter().map(|id| Value::String((*id).to_string())).collect()),
        );
        VaultItem {
            id: "item-1".to_string(),
            name: "Shared login".to_string(),
            fields: Vec::new(),
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other,
        }
    }

    /// An item carrying neither key at all -- the shape a server that sends
    /// no organisation fields produces.
    fn bare_item() -> VaultItem {
        let mut bare = item(None, &[]);
        bare.other.clear();
        bare
    }

    /// A sync payload with one organisation and one collection in it. `name`
    /// on the collection is passed through verbatim so a caller can hand
    /// either ciphertext or plaintext.
    fn sync_payload(collection_name: &str) -> SyncResponse {
        serde_json::from_value(json!({
            "profile": {
                "key": "2.aaa|bbb|ccc",
                "organizations": [
                    { "id": ORG, "name": "Engineering", "enabled": true, "key": "4.zzz" }
                ]
            },
            "ciphers": [],
            "folders": [],
            "collections": [
                { "id": COLLECTION, "organizationId": ORG, "name": collection_name,
                  "object": "collection" }
            ]
        }))
        .expect("the fixture parses")
    }

    /// **The zero-organisation case, which is this owner's own.**
    ///
    /// A sync from an account that has never touched organisations carries no
    /// `collections` key at all and an empty `organizations` array. Three
    /// things are asserted about it and the third is the one the rail depends
    /// on: the parse survives, the directory is empty, and `is_empty` says so
    /// -- which is the single condition `sidebar::draw_organisations` returns
    /// on, so an empty directory is a rail with no section in it.
    #[test]
    fn an_account_with_no_organisations_has_an_empty_directory() {
        let response: SyncResponse = serde_json::from_value(json!({
            "profile": { "key": "2.aaa|bbb|ccc", "organizations": [] },
            "ciphers": [],
            "folders": []
        }))
        .expect("a sync with no collections key at all still parses");
        let directory = Directory::from_sync(&response, &keys());

        assert!(directory.is_empty(), "an account with no organisations has a directory: {directory:?}");
        assert!(directory.organisations().is_empty());
        assert!(directory.collections_of(ORG).is_empty());
        // And the question every surface asks is answered without one.
        assert_eq!(directory.audience_of(&item(None, &[])), Audience::Personal);
    }

    /// **`collections: null` must not fail the whole sync**, which is the
    /// vault. `SyncResponse`'s own doc has always said a server without
    /// organisations "sends no `collections`, or sends `null`"; until this
    /// field existed that sentence cost nothing, and a plain
    /// `#[serde(default)]` would have turned the second half of it into a
    /// vault that will not load.
    #[test]
    fn a_null_collections_array_does_not_fail_the_sync() {
        let response: SyncResponse = serde_json::from_value(json!({
            "profile": { "key": "2.aaa|bbb|ccc", "organizations": [] },
            "ciphers": [],
            "folders": [],
            "collections": Value::Null
        }))
        .expect("`collections: null` fails the whole sync, which is the vault");
        assert!(response.collections.is_empty());
    }

    /// **A collection name is ciphertext on Bitwarden and plaintext on
    /// NodeWarden, and both are read.** The third case is the one that
    /// matters: ciphertext this account cannot open must NOT be painted into
    /// the rail as though it were a name.
    #[test]
    fn a_collection_name_is_read_whether_it_is_ciphertext_or_plaintext() {
        let keys = keys();
        let org_key = keys.organization(ORG).expect("the fixture holds an org key").clone();
        let sealed = encrypt(&org_key, b"Shared credentials").expect("the seal").to_string();

        // Ciphertext under the organisation's key.
        let directory = Directory::from_sync(&sync_payload(&sealed), &keys);
        assert_eq!(directory.collections_of(ORG)[0].label(), "Shared credentials");

        // Plaintext, which is what this owner's own server stores.
        let directory = Directory::from_sync(&sync_payload("Shared credentials"), &keys);
        assert_eq!(directory.collections_of(ORG)[0].label(), "Shared credentials");

        // Ciphertext under somebody else's key: the row survives, and it does
        // not paint `2.aB9d...|...|...` as a collection's name.
        let stranger = keys_with_organisation(&[7u8; 64], "other-org", &[3u8; 64]);
        let sealed_elsewhere = encrypt(
            stranger.organization("other-org").expect("the stranger's key"),
            b"Shared credentials",
        )
        .expect("the seal")
        .to_string();
        let directory = Directory::from_sync(&sync_payload(&sealed_elsewhere), &keys);
        let collection = directory.collections_of(ORG)[0];
        assert_eq!(collection.name, None);
        assert_eq!(collection.label(), UNREADABLE_NAME);
        assert!(
            !collection.label().contains('|'),
            "a ciphertext reached the rail as a collection name: {:?}",
            collection.label()
        );
    }

    /// **The whole answer: who else can see this item.**
    ///
    /// Four members, and every one of them is here to make a different point.
    /// The owner is listed against NO collections and must still be counted,
    /// which is the reason `Member::reaches_everything` exists at all: a
    /// count that only intersected collection lists would report zero other
    /// viewers on an item the organisation's owner is reading right now.
    #[test]
    fn the_audience_counts_everyone_who_reaches_the_item_and_not_the_viewer() {
        let mut directory = Directory::from_sync(&sync_payload("Shared credentials"), &keys());
        directory.set_roster(
            ORG,
            &json!({ "object": "list", "data": [
                // Reaches everything, listed against nothing.
                { "id": "u-owner", "email": "owner@example.invalid", "role": "owner",
                  "status": 1, "collections": [] },
                // Reaches it by being listed against the collection.
                { "id": "u-in", "email": "in@example.invalid", "role": "member",
                  "status": 1, "collections": [COLLECTION] },
                // In the organisation, not in this collection.
                { "id": "u-out", "email": "out@example.invalid", "role": "member",
                  "status": 1, "collections": ["some-other-collection"] },
                // Invited and never accepted: cannot see anything yet.
                { "id": "u-pending", "email": "pending@example.invalid", "role": "member",
                  "status": 0, "collections": [COLLECTION] }
            ]}),
        );

        let Audience::Shared { organisation, collections, others } =
            directory.audience_of(&item(Some(ORG), &[COLLECTION]))
        else {
            panic!("an organisation-owned item read as personal");
        };
        assert_eq!(organisation, "Engineering");
        assert_eq!(collections, vec!["Shared credentials".to_string()]);
        // Owner + the listed member = 2 reachers, minus the viewer = 1.
        assert_eq!(others, Some(1), "the count is wrong about who can see this item");

        // An organisation item filed in NO collection is reachable only by the
        // members who reach everything -- which is one, and that one is the
        // viewer. "0 other people" is the true answer, not a bug.
        let Audience::Shared { collections, others, .. } =
            directory.audience_of(&item(Some(ORG), &[]))
        else {
            panic!("an organisation-owned item read as personal");
        };
        assert!(collections.is_empty());
        assert_eq!(others, Some(0));
    }

    /// **The directory survives the wire whole** -- organisations, decrypted
    /// collection names, a known roster and an unknown one -- so a window
    /// reading the daemon's vault answers `audience_of` exactly as a window
    /// that synced for itself does.
    #[test]
    fn a_directory_read_back_off_the_wire_is_the_same_directory() {
        let mut directory = Directory::from_sync(&sync_payload("Shared credentials"), &keys());
        directory.rosters.push(("org-unknown".to_string(), Roster::Unknown));
        directory.set_roster(
            ORG,
            &json!({ "object": "list", "data": [
                { "id": "u-owner", "email": "owner@example.invalid", "name": "Owner",
                  "role": "owner", "status": 1, "collections": [] },
                { "id": "u-in", "email": "in@example.invalid", "role": "member",
                  "status": 1, "collections": [COLLECTION] }
            ]}),
        );
        let back = Directory::from_body(directory.to_body()).expect("reads back");
        assert_eq!(back, directory);
        assert_eq!(back.roster_of("org-unknown"), &Roster::Unknown, "unknown became known");
        assert_eq!(
            back.audience_of(&item(Some(ORG), &[COLLECTION])),
            directory.audience_of(&item(Some(ORG), &[COLLECTION]))
        );
        // And an empty one is empty, which is the owner's own account.
        assert!(Directory::from_body(Directory::empty().to_body()).expect("reads back").is_empty());
        assert!(Directory::from_body(json!({ "nope": 1 })).is_err());
    }

    /// **An unknown roster is not a roster of nobody**, and this is the whole
    /// reason [`Roster`] has two variants rather than being an
    /// `Option<Vec<Member>>` whose empty case means two opposite things.
    ///
    /// A server that will not answer the members route leaves `others` as
    /// `None` -- "this app does not know" -- which is what a surface draws as
    /// nothing at all. `Some(0)` would be a claim, and the wrong one.
    #[test]
    fn an_unknown_roster_is_not_a_roster_of_nobody() {
        let mut directory = Directory::from_sync(&sync_payload("Shared credentials"), &keys());
        assert_eq!(directory.roster_of(ORG), &Roster::Unknown);
        let Audience::Shared { others, .. } = directory.audience_of(&item(Some(ORG), &[COLLECTION]))
        else {
            panic!("shared item read as personal");
        };
        assert_eq!(others, None, "an unfetched roster claimed a number");

        // A body that is not a list -- an error object that arrived with a
        // 200, say -- leaves it unknown too, rather than becoming `Known([])`.
        directory.set_roster(ORG, &json!({ "message": "Not found" }));
        assert_eq!(directory.roster_of(ORG), &Roster::Unknown);

        // And a real organisation of one is a DIFFERENT state, which is the
        // half that makes the distinction worth having.
        directory.set_roster(ORG, &json!({ "object": "list", "data": [] }));
        assert_eq!(directory.roster_of(ORG), &Roster::Known(Vec::new()));
    }

    /// An item is personal, or it is shared. **An item naming an organisation
    /// this directory has never heard of is still shared** -- answering
    /// `Personal` because the directory is incomplete is the one wrong answer
    /// that matters, because it is the one that tells a user a shared secret
    /// is theirs alone.
    #[test]
    fn an_unknown_organisation_is_still_shared() {
        let directory = Directory::from_sync(&sync_payload("Shared credentials"), &keys());
        assert_eq!(directory.audience_of(&item(None, &[])), Audience::Personal);
        assert!(!directory.audience_of(&item(None, &[])).is_shared());

        let stranger = directory.audience_of(&item(Some("an-organisation-nobody-listed"), &[]));
        assert!(stranger.is_shared(), "an org-owned item read as personal: {stranger:?}");
        let Audience::Shared { organisation, others, .. } = stranger else { unreachable!() };
        // The id, which is ugly and true, rather than a label that would make
        // two unknown organisations look like one.
        assert_eq!(organisation, "an-organisation-nobody-listed");
        assert_eq!(others, None);
    }

    /// **Both member shapes and both envelopes**, because which one a server
    /// sends says nothing about who can see what.
    ///
    /// NodeWarden's member row carries bare collection id strings and wraps
    /// the list in `{"object":"list","data":[..]}`; Bitwarden's carries
    /// `{"id":..,"readOnly":..}` objects. A bare array is read too.
    #[test]
    fn both_member_shapes_and_both_envelopes_are_read() {
        let nodewarden = map_members(&json!({ "object": "list", "data": [
            { "id": "u-1", "email": "a@example.invalid", "role": "member",
              "collections": [COLLECTION] }
        ]}))
        .expect("the list envelope");
        let bitwarden = map_members(&json!([
            { "id": "u-1", "email": "a@example.invalid", "type": 2,
              "collections": [{ "id": COLLECTION, "readOnly": false }] }
        ]))
        .expect("a bare array");

        for (which, members) in [("NodeWarden", nodewarden), ("Bitwarden", bitwarden)] {
            assert_eq!(members.len(), 1, "{which}: the row was dropped");
            assert!(
                members[0].reaches(&[COLLECTION]),
                "{which}: the collection did not survive the read: {:?}",
                members[0]
            );
            // A server that sent no status is read as confirmed -- under-counting
            // the people who can see an item is the direction that misleads.
            assert!(members[0].confirmed(), "{which}: a member with no status was discounted");
        }

        // Anything that is not a list at all is `None`, which stays
        // `Roster::Unknown`. See that type.
        assert!(map_members(&json!({ "message": "Not found" })).is_none());
        assert!(map_members(&Value::Null).is_none());
    }

    /// **A stock Bitwarden, or an older NodeWarden, degrades to exactly
    /// today's behaviour.**
    ///
    /// The members route does not exist -- `404` on every spelling of it --
    /// and the account still gets its organisations and collections out of
    /// the sync, still lists items, and reports the roster as unknown rather
    /// than raising anything. **Nothing in this path can return an error**;
    /// `Directory::load` has no `Result` in its signature, which is what
    /// makes "no error screen" structural rather than a rule.
    #[test]
    fn a_server_without_the_members_route_degrades_to_an_unknown_roster() {
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(
                r#"{"access_token":"AT-1","expires_in":3600,"token_type":"Bearer",
                    "refresh_token":"RT-1","scope":"api offline_access"}"#,
            )
            .create();
        let members = server
            .mock("GET", format!("/api/organizations/{ORG}/users?includeCollections=true").as_str())
            .with_status(404)
            .with_body(r#"{"message":"Not found"}"#)
            .expect(1)
            .create();

        let client = RestClient::new(server.url());
        let mut session = client
            .password_grant(
                "a@b.c",
                "HASH",
                &Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC"),
            )
            .expect("the grant");

        let directory =
            Directory::load(&client, &mut session, &sync_payload("Shared credentials"), &keys());

        members.assert();
        assert!(!directory.is_empty(), "the sync's own half of the directory was lost");
        assert_eq!(directory.organisations()[0].label(), "Engineering");
        assert_eq!(directory.collections_of(ORG).len(), 1);
        assert_eq!(directory.roster_of(ORG), &Roster::Unknown);
    }

    /// **An account with no organisations makes no request at all**, which is
    /// the case the whole design is arranged not to charge for -- and the
    /// owner's own.
    ///
    /// A mock with `expect(0)`: the route is registered so that a request
    /// reaching it is *seen*, and asserted never hit. Without the
    /// registration a request would be unmatched and the test would pass on a
    /// server that was in fact contacted.
    #[test]
    fn an_account_with_no_organisations_makes_no_request_at_all() {
        let mut server = crate::test_http::server();
        server
            .mock("POST", "/identity/connect/token")
            .with_body(
                r#"{"access_token":"AT-1","expires_in":3600,"token_type":"Bearer",
                    "refresh_token":"RT-1","scope":"api offline_access"}"#,
            )
            .create();
        let nothing = server.mock("GET", crate::test_http::Matcher::Any).expect(0).create();

        let client = RestClient::new(server.url());
        let mut session = client
            .password_grant(
                "a@b.c",
                "HASH",
                &Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC"),
            )
            .expect("the grant");
        let response: SyncResponse = serde_json::from_value(json!({
            "profile": { "key": "2.aaa|bbb|ccc", "organizations": [] },
            "ciphers": [], "folders": []
        }))
        .expect("the fixture parses");

        let directory = Directory::load(&client, &mut session, &response, &keys());

        nothing.assert();
        assert!(directory.is_empty());
    }

    /// The held slot: replaced wholesale, emptied on demand, and **never an
    /// `Option` at the reading end**. A caller that has never stored one gets
    /// an empty directory rather than a `None` it has to decide about.
    ///
    /// Serialised with the other slot test by being the only two that touch
    /// it, and each one leaves it empty.
    #[test]
    fn the_held_directory_is_replaced_wholesale_and_can_be_emptied() {
        forget();
        assert!(current().is_empty(), "the slot was not empty at the start of this test");

        remember(Directory::from_sync(&sync_payload("Shared credentials"), &keys()));
        assert_eq!(current().organisations()[0].label(), "Engineering");

        // An account switch. `RestBackend::new` calls this, which is what
        // stops a rail drawing somebody else's organisation.
        forget();
        assert!(current().is_empty(), "a forgotten directory came back");
    }

    /// `organizationId` and `collectionIds` ride `VaultItem::other`, and the
    /// three empty spellings a server can use for "none" all read as none.
    ///
    /// This is the one place those two keys are read in the whole crate --
    /// `sidebar::SidebarFilter::scope_contains` delegates here rather than
    /// re-reading them -- so this is the only test that has to hold the
    /// spelling.
    #[test]
    fn the_two_facts_are_read_off_the_item_the_window_already_holds() {
        assert_eq!(organization_of(&item(Some(ORG), &[])), Some(ORG));
        assert_eq!(organization_of(&item(None, &[])), None);
        // An empty string is not an organisation; nothing produces it, and an
        // item owned by one with no id is not something this app can say
        // anything true about.
        assert_eq!(organization_of(&item(Some(""), &[])), None);

        assert_eq!(collection_ids_of(&item(Some(ORG), &[COLLECTION])), vec![COLLECTION]);
        assert!(collection_ids_of(&item(Some(ORG), &[])).is_empty());
        // Keys the server did not send at all.
        assert!(collection_ids_of(&bare_item()).is_empty());
        assert_eq!(organization_of(&bare_item()), None);
    }

    // ---- personal sharing -------------------------------------------------

    /// `/users` rows (official shape, NodeWarden from slice 1) say the three
    /// states in `status`; NodeWarden's older `/members` rows ARE the account
    /// and say "accepted" as status 1 with no key. One roster reader serves
    /// both, and `status 1` is never read as confirmed from either.
    #[test]
    fn a_roster_row_says_invited_accepted_or_confirmed_whichever_route_answered() {
        let users = json!({ "data": [
            { "object": "organizationUserUserDetails", "id": "ou-1", "userId": "u-1", "email": "a@x.io",
              "type": 2, "status": 2, "collections": [{ "id": "c1", "readOnly": true, "hidePasswords": false, "manage": false }] },
            { "object": "organizationUserUserDetails", "id": "ou-2", "userId": "u-2", "email": "b@x.io", "type": 2, "status": 1 },
            { "object": "organizationUserUserDetails", "id": "inv-3", "userId": null, "email": "c@x.io", "type": 2, "status": 0,
              "invitationExpirationDate": "2026-10-04T00:00:00Z" }
        ]});
        let members = map_members(&users).expect("a roster");
        assert_eq!(members[0].status, MemberStatus::Confirmed);
        assert_eq!(members[0].user_id.as_deref(), Some("u-1"));
        assert!(members[0].has_access("c1"));
        assert_eq!(members[0].read_only, ["c1"]);
        assert_eq!(members[1].status, MemberStatus::Accepted);
        assert!(!members[1].confirmed());
        assert_eq!(members[2].status, MemberStatus::Invited);
        assert_eq!(members[2].user_id, None);
        assert_eq!(members[2].invitation_expires.as_deref(), Some("2026-10-04T00:00:00Z"));

        let legacy = json!({ "data": [
            { "object": "organizationMember", "id": "u-9", "email": "d@x.io", "role": "member", "type": 0, "status": 1, "encryptedOrgKey": null, "collections": ["c1"] },
            { "object": "organizationMember", "id": "u-8", "email": "e@x.io", "role": "owner", "type": 0, "status": 1, "encryptedOrgKey": "4.k", "collections": [] }
        ]});
        let members = map_members(&legacy).expect("a roster");
        assert_eq!(members[0].status, MemberStatus::Accepted, "status 1 with no key is accepted, not confirmed");
        assert_eq!(members[0].user_id.as_deref(), Some("u-9"));
        assert_eq!(members[0].role, "member", "NodeWarden's constant `type: 0` is not an owner");
        assert_eq!(members[1].status, MemberStatus::Confirmed);
        assert_eq!(members[1].role, "owner");
    }

    #[test]
    fn a_sharing_org_is_named_after_the_sharer_and_a_long_address_is_cut_honestly() {
        assert_eq!(sharing_org_name(" Denis@Example.com "), "Shared by denis@example.com");
        let long = "denis.platonov@a-very-long-domain-name-indeed.example";
        let name = sharing_org_name(long);
        assert_eq!(name.chars().count(), 50);
        assert!(name.ends_with('…'));
        // `sharer_of` returns a literal substring of `name` after the 10-byte
        // prefix, so it is exactly `name`'s 50 chars minus those 10 -- 40,
        // ellipsis included -- matching `SHARING_ORG_NAME_LIMIT`'s own doc
        // ("leaves 40 characters for the email").
        assert_eq!(sharer_of(&name), Some("denis.platonov@a-very-long-domain-name-…"));
        assert!(sharer_is(sharer_of(&name).expect("a sharer"), long));
        assert!(!sharer_is(sharer_of(&name).expect("a sharer"), "someone@else.example"));
        assert_eq!(sharer_of("Engineering"), None);
        assert_eq!(sharer_of("Shared by nobody"), None, "no `@`, not an email");
    }

    /// **The rail never shows a sharing org, and an audience never names one.**
    #[test]
    fn sharing_orgs_are_left_out_of_the_visible_organisations_and_answer_as_people() {
        let response: SyncResponse = serde_json::from_value(json!({
            "profile": {
                "key": "2.aaa|bbb|ccc",
                "email": "denis@example.com",
                "organizations": [
                    { "id": ORG, "name": "Engineering", "enabled": true, "key": "4.zzz" },
                    { "id": "mine", "name": "Shared by denis@example.com", "role": "owner", "key": "4.zzz" },
                    { "id": "theirs", "name": "Shared by anna@example.com", "role": "member", "key": "4.zzz", "ownerEmail": "anna@example.com" }
                ],
                "organizationsNew": [
                    { "id": "pending", "name": "Shared by carol@example.com", "role": "member", "status": 1, "ownerEmail": "carol@example.com" }
                ]
            },
            "ciphers": [], "folders": [],
            "collections": [ { "id": "c-bob", "organizationId": "mine", "name": "bob@example.com" } ]
        })).expect("parses");
        let directory = Directory::from_sync(&response, &keys());
        let visible: Vec<&str> = directory.visible_organisations().iter().map(|o| o.id.as_str()).collect();
        assert_eq!(visible, [ORG]);
        assert_eq!(directory.my_sharing_org().map(|o| o.id.as_str()), Some("mine"));
        assert_eq!(directory.waiting().iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["pending"]);
        assert_eq!(directory.collection_named("mine", "BOB@example.com").map(|c| c.id.as_str()), Some("c-bob"));

        assert_eq!(
            directory.audience_of(&item(Some("mine"), &["c-bob"])),
            Audience::SharedWithPeople { people: vec!["bob@example.com".to_string()] }
        );
        assert_eq!(
            directory.audience_of(&item(Some("theirs"), &[])),
            Audience::SharedWithMe { from: "anna@example.com".to_string() }
        );
        assert!(directory.audience_of(&item(Some("theirs"), &[])).is_shared());

        // The wire keeps all of it, for a window reading the daemon's copy.
        let round = Directory::from_body(directory.to_body()).expect("reads back");
        assert_eq!(round, directory);
    }

    /// **A "Shared by" name is believed only when the server attests it**
    /// (controller decision, final review): anyone can create an org named
    /// "Shared by boss@..." and add anyone. A forged name is shown as its
    /// REAL owner's share; a name with no attestation is no sharing org at
    /// all; a name matching the attested owner is a share as before; and
    /// this account's own org needs no attestation, unless it names another
    /// owner.
    #[test]
    fn a_shared_by_name_is_believed_only_when_the_server_attests_its_owner() {
        let me = "denis@example.com";
        let mark = |name: &str, owner: Option<&str>, is_owner: bool| sharing_mark(sharer_of(name), owner, is_owner, me);
        assert_eq!(
            mark("Shared by boss@corp.example", Some("mallory@evil.example"), false),
            Some(SharingOrg { sharer: "mallory@evil.example".into(), mine: false }),
            "a forged name: received from the attested owner"
        );
        assert_eq!(mark("Shared by boss@corp.example", None, false), None, "a name-only claim is no sharing org");
        assert_eq!(
            mark("Shared by boss@corp.example", Some(" Boss@Corp.example "), false),
            Some(SharingOrg { sharer: "boss@corp.example".into(), mine: false }),
            "attested: a share from the name's sharer"
        );
        assert_eq!(mark("Shared by denis@example.com", None, true), Some(SharingOrg { sharer: me.into(), mine: true }), "my own");
        assert_eq!(mark("Shared by denis@example.com", Some(me), true), Some(SharingOrg { sharer: me.into(), mine: true }));
        assert_eq!(
            mark("Shared by denis@example.com", Some("mallory@evil.example"), false),
            Some(SharingOrg { sharer: "mallory@evil.example".into(), mine: false }),
            "someone else's org named for me is theirs"
        );
        assert_eq!(mark("Shared by anna@example.com", Some(me), true), None, "my org named for someone else is no share");
        assert_eq!(mark("Engineering", Some("boss@corp.example"), false), None, "no sharing name, no sharing org");
        // A long address is cut in the name; the attested owner is whole.
        let long = "denis.platonov@a-very-long-domain-name-for-testing.example";
        let cut = sharing_org_name(long);
        assert!(cut.ends_with('\u{2026}'), "{cut}");
        assert_eq!(mark(&cut, Some(long), false).map(|s| s.sharer), Some(long.to_string()), "the whole address, not the cut name");

        // Through a whole read: the forged org's records say who really
        // owns it, and a name-only org is an ordinary organisation.
        let response: SyncResponse = serde_json::from_value(json!({
            "profile": {
                "key": "2.aaa|bbb|ccc",
                "email": me,
                "organizations": [
                    { "id": "forged", "name": "Shared by boss@corp.example", "role": "member", "key": "4.zzz", "ownerEmail": "mallory@evil.example" },
                    { "id": "bare", "name": "Shared by boss@corp.example", "role": "member", "key": "4.zzz" }
                ],
                "organizationsNew": [
                    { "id": "waiting-forged", "name": "Shared by boss@corp.example", "role": "member", "status": 1, "ownerEmail": "mallory@evil.example" }
                ]
            },
            "ciphers": [], "folders": [], "collections": []
        })).expect("parses");
        let directory = Directory::from_sync(&response, &keys());
        assert_eq!(
            directory.audience_of(&item(Some("forged"), &[])),
            Audience::SharedWithMe { from: "mallory@evil.example".to_string() }
        );
        assert!(directory.sharing_orgs().iter().all(|o| o.id != "bare"), "a name-only claim is not a sharing org");
        assert!(directory.visible_organisations().iter().any(|o| o.id == "bare"), "it is shown as the organisation it is");
        assert_eq!(
            directory.waiting().iter().map(|o| o.sharing.as_ref().map(|s| s.sharer.as_str())).collect::<Vec<_>>(),
            [Some("mallory@evil.example")]
        );
    }

    /// **The roster route is `/users` and nothing else**: NodeWarden's older
    /// `/members` is never asked, even when `/users` is missing -- sharing is
    /// on Bitwarden's routes only, and that server is one it calls too old.
    #[test]
    fn the_roster_reads_users_and_never_the_old_members_route() {
        let mut server = crate::test_http::server();
        server.mock("POST", "/identity/connect/token")
            .with_body(r#"{"access_token":"AT-1","expires_in":3600,"refresh_token":"RT-1","token_type":"Bearer"}"#)
            .create();
        let client = RestClient::new(server.url());
        let device = Device::windows_desktop("11111111-2222-3333-4444-555555555555", "TEST-PC");
        let mut session = client.password_grant("a@b.c", "HASH", &device).expect("grant");
        let users = server
            .mock("GET", format!("/api/organizations/{ORG}/users?includeCollections=true").as_str())
            .with_status(404)
            .expect(1)
            .create();
        let members = server.mock("GET", format!("/api/organizations/{ORG}/members").as_str()).expect(0).create();
        assert_eq!(client.fetch_organization_members(&mut session, ORG), Err(RestError::Status(404)));
        users.assert();
        members.assert();
    }
}

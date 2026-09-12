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

/// One member of one organisation.
///
/// **This is the membership model, and it is deliberately thin.** It carries
/// exactly what answering "who else can see this item" needs and not one
/// field more: a role (because an owner or an admin reaches every collection
/// regardless of what is listed against them), the collections listed against
/// them, and whether the membership is confirmed. No permissions map, no
/// avatar colour, no `type` -- those are the web vault's business, and this
/// app does not write memberships at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub id: String,
    pub email: String,
    /// The display name, when the server sent one. Not used for counting;
    /// kept so a surface that wants to *name* the other viewers can.
    pub name: Option<String>,
    /// Lower-cased, as NodeWarden's `normalizeRole` writes it: `owner`,
    /// `admin`, `manager`, `member`. Read by [`Self::reaches_everything`] and
    /// compared nowhere else.
    pub role: String,
    /// Whether this membership is accepted. Bitwarden's `status` is `2` for
    /// confirmed and NodeWarden's is `1`; both use `0` for staged or revoked,
    /// so the test is "not zero" rather than a number, and a server that sent
    /// no status at all is read as confirmed.
    pub confirmed: bool,
    /// The collection ids listed against this member.
    pub collections: Vec<String>,
}

impl Member {
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
}

impl Audience {
    /// Whether this item is owned by an organisation at all.
    #[must_use]
    pub fn is_shared(&self) -> bool {
        matches!(self, Audience::Shared { .. })
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
        let collections = filed_in
            .iter()
            .filter_map(|id| self.collections.iter().find(|c| c.id == *id))
            .map(|c| c.label().to_string())
            .collect();
        let others = match self.roster_of(org_id) {
            Roster::Unknown => None,
            Roster::Known(members) => Some(
                members
                    .iter()
                    .filter(|m| m.confirmed && m.reaches(&filed_in))
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
        Self { organisations, collections, rosters }
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
        for id in directory.organisations.iter().map(|o| o.id.clone()).collect::<Vec<_>>() {
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
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .and_then(|raw| collection_name(raw, keys.organization(&organization_id)));
    Some(Collection { id, organization_id, name })
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
fn collection_name(raw: &str, key: Option<&SymmetricKey>) -> Option<String> {
    let Ok(enc) = raw.parse::<EncString>() else {
        return Some(raw.to_string());
    };
    let key = key?;
    let bytes = decrypt(key, &enc).ok()?;
    std::str::from_utf8(&bytes).ok().map(str::to_string)
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
fn map_members(answer: &Value) -> Option<Vec<Member>> {
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
    Some(Member {
        id,
        email: object.get("email").and_then(Value::as_str).unwrap_or_default().to_string(),
        name: object.get("name").and_then(Value::as_str).map(str::to_string),
        role: object
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("member")
            .to_ascii_lowercase(),
        // A server that sent no status is read as confirmed: the row is in
        // the roster, and inventing a reason to discount it would under-count
        // the people who can see an item, which is the direction that
        // misleads.
        confirmed: object.get("status").and_then(Value::as_i64).is_none_or(|s| s != 0),
        collections: object
            .get("collections")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|entry| {
                        // Two shapes on the wire: NodeWarden's member row
                        // carries bare id strings, and Bitwarden's carries
                        // `{"id":..,"readOnly":..}` objects. Both are read,
                        // because which one arrived says nothing about who
                        // can see what.
                        entry
                            .as_str()
                            .or_else(|| entry.get("id").and_then(Value::as_str))
                            .filter(|id| !id.is_empty())
                    })
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

// ---- where the directory is held --------------------------------------------

/// The one held directory. See the module docs for why it is here rather than
/// threaded through `VaultBackend`.
static HELD: Mutex<Option<Arc<Directory>>> = Mutex::new(None);

/// Stores `directory` as this process's current one.
///
/// Called by [`crate::rest::backend::RestBackend::synced`] and by nothing
/// else, which is what makes "the directory describes the vault the window is
/// drawing" true by construction rather than by scheduling.
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
    use crate::rest::api::{Device, RestClient};
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
            assert!(members[0].confirmed, "{which}: a member with no status was discounted");
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
            .mock("GET", format!("/api/organizations/{ORG}/members").as_str())
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
}

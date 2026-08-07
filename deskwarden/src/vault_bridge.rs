use crate::app_match::{AppMatch, APP_MATCH_FIELD_NAME};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zeroize::Zeroizing;

/// One entry of an item's `fields` array -- a Bitwarden custom field.
///
/// It needs its own `#[serde(flatten)] other` for exactly the reason
/// [`UriEntry`] does, and this one has teeth: a custom field carries more than
/// name and value. At minimum `type` (0=text, 1=hidden, 2=boolean, 3=linked)
/// and, for linked fields, `linkedId`. `VaultField` is serialized on every
/// state-replacing write -- [`with_app_match`] rebuilds the whole `fields`
/// vector and the item is PUT as its complete new state -- so a key dropped on
/// deserialize is a key *deleted from the user's vault* on the next write.
///
/// The concrete harm, before this field existed: attaching an app match (via
/// "Add app...") to an item that already had custom fields rewrote every one
/// of them without its type. A **hidden field silently became an ordinary
/// visible one**, and a linked field lost what it was linked to. Nothing
/// failed, nothing was logged, and it happened on a real 1656-item vault.
///
/// Note what is deliberately *not* here: `type` and `linkedId` as typed
/// fields. Preserving unknown keys is the fix; naming them is a separate
/// feature, and modelling a wire shape from memory is how a modelled field and
/// its `other` copy start disagreeing.
///
/// Note what these two modelled fields deliberately do *not* carry, unlike
/// [`LoginData`]'s: `#[serde(default, skip_serializing_if = "Option::is_none")]`.
/// That attribute would honour the "don't inject a key the source never had"
/// rule for a field arriving as `{"name":"PIN","type":1}` -- but it would
/// break the *observed* shape in exchange, because `Option<String>` cannot
/// tell an absent key from an explicit `null` once deserialized, and a LINKED
/// custom field really does arrive as `{"value":null,"type":3,"linkedId":100}`.
/// Adding it drops that `null` and fails
/// `unknown_keys_on_a_custom_field_survive_a_round_trip`. Doing this properly
/// needs `Option<Option<String>>` (outer = key present, inner = null vs
/// value), which changes the type every caller reads. RECORDED, NOT ACTIONED.
///
/// `other` is `pub` and unguarded, so inserting `"name"` or `"value"` into it
/// would emit a duplicate JSON key and `bw` would take the last one --
/// silently renaming the field. Not reachable today (deserialize routes those
/// two keys into the typed fields, and nothing in the crate writes to `other`),
/// and [`UriEntry`]/[`LoginData`] share the shape; it is why `other` should
/// stay a map only serde fills.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VaultField {
    pub name: Option<String>,
    pub value: Option<String>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// The `login` object `bw serve` returns for login-type items.
///
/// Only the two fields we type are modelled; everything else `bw` sends inside
/// `login` (TOTP, URIs, password history) is preserved through `other` so a
/// round-trip write doesn't drop it.
///
/// Both modelled fields carry `skip_serializing_if`, for the same reason
/// [`VaultItem::login`] does: `bw serve`'s edit endpoint takes the payload as
/// the item's new state, so a login object that arrived as `{"password":"p"}`
/// must not be written back as `{"username":null,"password":"p"}` -- that
/// injects a key the source never had. Unmodelled keys already round-trip
/// exactly through `other`; these two now do too.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LoginData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// `Zeroizing<String>` rather than a plain `String`: `VaultCache::items`
    /// hands callers a clone of the whole snapshot (the vault window keeps
    /// one open, `app::fill_from_vault`/`handle_match` make short-lived
    /// ones), so a plaintext password wrapped this way wipes itself on
    /// *every* one of those clones' drops, not just `VaultCache::clear`'s
    /// own copy -- zeroizing only `clear`'s copy would be a false sense of
    /// security while the others are still resident. `Zeroizing<Z>`
    /// (de)serializes exactly like `Z` (the `zeroize` crate's `serde`
    /// feature, enabled in `Cargo.toml`), so this changes nothing about the
    /// wire format `bw serve` sends or receives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Zeroizing<String>>,
    /// `Zeroizing<String>` for the same reason [`Self::password`] is: the
    /// TOTP seed is a long-lived 2FA secret, not a one-time code (`bw
    /// serve`'s `/object/totp/{id}` endpoint derives the current code from
    /// this on every call), so it deserves the same wipe-on-drop guarantee
    /// as the password sitting right next to it rather than lingering in
    /// freed memory as a plain `String`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<Zeroizing<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uris: Vec<UriEntry>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// One entry of `login.uris`. `bw`'s match-strategy field (`match`) on each
/// entry is not modelled here, so `UriEntry` needs its own `other` flatten
/// for the same reason `LoginData.other` and `VaultItem.other` exist:
/// `#[serde(flatten)]` only captures unknown keys at the level of the struct
/// it's declared on, so `VaultItem`'s or `LoginData`'s flatten cannot reach
/// into the elements of this nested `Vec` -- without its own flatten field,
/// unmodelled keys on a URI entry would be silently dropped on write.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UriEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// A payment card (`type: 3`).
///
/// Field names and value types are transcribed from a live capture of
/// `GET /object/template/item.card` (recorded in
/// `.superpowers/sdd/item-shapes-capture.md`), not from memory.
///
/// **All six values are strings, including the expiry.** Bitwarden's own
/// template sends `expMonth: "04"` -- zero-padded -- so modelling either half
/// as a number would rewrite `"04"` as `4` on the next full-state PUT and
/// fail to parse an empty string at all.
///
/// Its own `#[serde(flatten)] other` for the same reason [`UriEntry`] has
/// one: [`VaultItem`]'s catch-all cannot reach inside a nested object, so
/// without this any key Bitwarden adds here would be silently dropped on the
/// next full-state PUT.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CardData {
    #[serde(rename = "cardholderName", default, skip_serializing_if = "Option::is_none")]
    pub cardholder_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    /// `Zeroizing` for the same reason [`LoginData::password`] is: a card
    /// number is a long-lived secret, and `VaultCache::items` hands out
    /// clones, so wiping only one copy would be a false sense of security.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<Zeroizing<String>>,
    #[serde(rename = "expMonth", default, skip_serializing_if = "Option::is_none")]
    pub exp_month: Option<String>,
    #[serde(rename = "expYear", default, skip_serializing_if = "Option::is_none")]
    pub exp_year: Option<String>,
    /// The security code (CVV/CVC). `Zeroizing`, as above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Zeroizing<String>>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// An identity (`type: 4`). Eighteen fields, of which a real item populates a
/// handful.
///
/// Seventeen of them are the exact keys of `GET
/// /object/template/item.identity`, captured live (see
/// `.superpowers/sdd/item-shapes-capture.md`).
///
/// **`address3` is the eighteenth, and it is deliberate, not a stray.** It is
/// absent from the template but present in Bitwarden's documented item
/// schema. Because every field here carries
/// `skip_serializing_if = "Option::is_none"`, modelling a key that does not
/// exist costs exactly nothing: it never appears on write. If real items do
/// carry it, modelling it means the detail pane shows it instead of hiding it
/// in `other`. Do not "clean this up" -- deleting it can only lose data.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IdentityData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "firstName", default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    #[serde(rename = "middleName", default, skip_serializing_if = "Option::is_none")]
    pub middle_name: Option<String>,
    #[serde(rename = "lastName", default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address2: Option<String>,
    /// See the struct doc: modelled on purpose despite being absent from the
    /// captured template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(rename = "postalCode", default, skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(rename = "passportNumber", default, skip_serializing_if = "Option::is_none")]
    pub passport_number: Option<String>,
    #[serde(rename = "licenseNumber", default, skip_serializing_if = "Option::is_none")]
    pub license_number: Option<String>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// An SSH key (`type: 5`).
///
/// Field names are transcribed from a live capture (recorded in
/// `.superpowers/sdd/item-shapes-capture.md`, "SSH key (type 5) -- VERIFIED
/// 2026-08-01"), not from memory. This type was deliberately left unmodelled
/// until that capture existed: `GET /object/template/item.sshKey` returns 400
/// on this CLI (2026.7.0) and the user's vault held no type-5 item, so the
/// only way to get the shape was to create one. `POST /object/item` with
/// `{"type": 5, ...}` was accepted, so this CLI does support the type despite
/// having no template for it.
///
/// Its own `#[serde(flatten)] other` for the same reason [`UriEntry`] and
/// [`CardData`] have one: [`VaultItem`]'s catch-all cannot reach inside a
/// nested object, so without this any key Bitwarden adds here would be
/// silently dropped on the next full-state PUT.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SshKeyData {
    /// `Zeroizing`: this is the secret the whole item exists to hold, so it
    /// gets exactly the treatment [`CardData::number`], [`CardData::code`],
    /// [`LoginData::password`], `LoginData::totp` and [`VaultItem::notes`]
    /// have.
    #[serde(rename = "privateKey", default, skip_serializing_if = "Option::is_none")]
    pub private_key: Option<Zeroizing<String>>,
    /// Plain `String`: a public key is public by construction. Wrapping it
    /// would widen the zeroize guarantee for nothing (see `deskwarden`'s
    /// README on what that guarantee does and does not cover).
    #[serde(rename = "publicKey", default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// Plain `String`, as [`Self::public_key`]: a fingerprint is a digest of
    /// the public half and is designed to be shown.
    #[serde(rename = "keyFingerprint", default, skip_serializing_if = "Option::is_none")]
    pub key_fingerprint: Option<String>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VaultItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub fields: Vec<VaultField>,
    /// `skip_serializing_if` so items with no login object (secure notes,
    /// cards) don't gain a `"login": null` on write, which `bw serve`'s edit
    /// endpoint would treat as new state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<LoginData>,
    /// The `card` object on a `type: 3` item. `skip_serializing_if` for the
    /// same reason [`Self::login`] has it: an item that gains `"card": null`
    /// on a full-state PUT has been told its card is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<CardData>,
    /// The `identity` object on a `type: 4` item. See [`Self::card`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<IdentityData>,
    /// The `sshKey` object on a `type: 5` item. See [`Self::card`].
    ///
    /// This was the last unmodelled type, and it was unmodelled on purpose
    /// rather than by omission: its wire shape could not be verified until a
    /// throwaway key was created in the user's vault. See [`SshKeyData`].
    #[serde(rename = "sshKey", default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<SshKeyData>,
    /// Item-level free text. A secure note's entire body lives here, which is
    /// why that type needs no struct of its own -- its `secureNote` object
    /// carries only a `{"type": 0}` discriminator, which rides
    /// [`Self::other`] untouched -- and why notes on an ordinary login were
    /// invisible until this field existed.
    ///
    /// `Zeroizing` because a secure note *is* the secret, exactly as
    /// [`LoginData::password`] is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Zeroizing<String>>,
    /// Raw `bw` item type: 1=Login, 2=SecureNote, 3=Card, 4=Identity,
    /// 5=SshKey.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub item_type: Option<i64>,
    #[serde(rename = "folderId", default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// What kind of thing an item is, derived from `bw`'s numeric `type`.
///
/// `Unknown` is not defensive padding: Bitwarden can ship a type 6, and an
/// unrecognised item must render as unsupported rather than fall through to
/// a login-shaped pane over data that is not a login. Collapsing a distinct
/// situation into a representation that means something else is the failure
/// mode behind most of the findings recorded in this repo's progress ledger.
///
/// Match it **exhaustively, with no catch-all arm**, everywhere behaviour
/// differs by kind. A `_ =>` would silently give `Unknown` whatever the
/// neighbouring arm does, which is precisely what this variant exists to
/// prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Login,
    SecureNote,
    Card,
    Identity,
    SshKey,
    Unknown(i64),
}

impl ItemKind {
    /// The one place a type number becomes a kind. An absent `type`
    /// preserves today's behaviour, in which every item was a login.
    pub fn of(item: &VaultItem) -> Self {
        match item.item_type {
            None | Some(1) => ItemKind::Login,
            Some(2) => ItemKind::SecureNote,
            Some(3) => ItemKind::Card,
            Some(4) => ItemKind::Identity,
            Some(5) => ItemKind::SshKey,
            Some(other) => ItemKind::Unknown(other),
        }
    }

    pub fn label(self) -> String {
        match self {
            ItemKind::Login => "Login".to_string(),
            ItemKind::SecureNote => "Secure note".to_string(),
            ItemKind::Card => "Card".to_string(),
            ItemKind::Identity => "Identity".to_string(),
            ItemKind::SshKey => "SSH key".to_string(),
            ItemKind::Unknown(_) => "Unsupported item".to_string(),
        }
    }
}

/// Pure helper: returns a copy of `item` with its `deskwarden:app-match`
/// custom field replaced (or added) to encode `m`. All other fields —
/// including anything not modeled by `VaultItem` and captured in `other` —
/// are preserved unchanged, since `bw serve`'s edit endpoint expects the
/// full item as the new state rather than a server-merged patch.
pub fn with_app_match(item: &VaultItem, m: &AppMatch) -> VaultItem {
    let is_app_match = |f: &VaultField| f.name.as_deref() == Some(APP_MATCH_FIELD_NAME);
    // Every *other* field is cloned wholesale, so anything unmodelled on it
    // (`type`, `linkedId`, ...) rides `VaultField::other` untouched.
    let mut fields: Vec<VaultField> = item.fields.clone();
    let existing = fields.iter().position(|f| is_app_match(f));

    // The app-match field is the one field rebuilt rather than copied, because
    // its `value` is the thing being changed. Its extra keys are still the
    // server's to keep: `bw` normalises what this app writes (a field created
    // here with no `type` comes back carrying one), so rebuilding from
    // name/value alone would drop them on the next write -- the same bug this
    // struct's `other` exists to prevent, one field further in. When there is
    // no such field to replace, an empty map is correct: inventing keys nobody
    // observed would be modelling from memory.
    let other = match existing {
        Some(i) => fields[i].other.clone(),
        None => serde_json::Map::new(),
    };
    let rebuilt = VaultField {
        name: Some(APP_MATCH_FIELD_NAME.to_string()),
        value: Some(m.to_field_value()),
        other,
    };
    // Replaced IN PLACE, not removed and re-appended: Bitwarden preserves and
    // displays custom-field order, so appending would visibly reshuffle the
    // user's own fields every time an app match is saved.
    match existing {
        Some(i) => fields[i] = rebuilt,
        None => fields.push(rebuilt),
    }

    let mut updated = item.clone();
    updated.fields = fields;
    updated
}

/// Pure helper: returns a copy of `item` filed under `folder_id` (or filed
/// nowhere, when that is `None`). Everything else -- including anything
/// unmodelled riding [`VaultItem::other`] -- is cloned untouched, exactly as
/// [`with_app_match`] does for the field it changes.
///
/// This is the *snapshot* half of a move. The *wire* half is
/// [`folder_move_body`], and the two are deliberately separate: the wire body
/// cannot be derived from this value alone, because serializing it drops the
/// `folderId` key entirely when the item is unfiled -- see that function.
pub fn with_folder(item: &VaultItem, folder_id: Option<&str>) -> VaultItem {
    let mut moved = item.clone();
    moved.folder_id = folder_id.map(str::to_string);
    moved
}

/// Pure helper: a copy of `item` marked (or unmarked) as a favourite, built
/// exactly the way [`with_folder`] and [`with_app_match`] build theirs --
/// clone, change one field, leave everything else including the catch-all
/// untouched.
///
/// **This deliberately does not go through the edit form.**
/// `detail_edit::EditDraft::apply_to` clones the item and overwrites the
/// fields the form owns; `favorite` is not one of them, so routing a
/// favourite through a draft would either need the draft to carry a field no
/// box on the form edits, or would silently discard the flag. It is a direct
/// one-field write on the item, and this is that write.
///
/// No `folderId`-style wire trap here: [`VaultItem::favorite`] carries no
/// `skip_serializing_if`, so it is stated on **every** write this app makes
/// and an ordinary `update_item` of the value below really does send
/// `"favorite": false` rather than omitting the key for the server to merge.
/// `un_favouriting_an_item_states_favorite_false_on_the_wire` pins that,
/// because the omitted-key failure is the one that silently does nothing --
/// it is precisely what `folder_move_body` exists to prevent one field over.
pub fn with_favorite(item: &VaultItem, favorite: bool) -> VaultItem {
    let mut updated = item.clone();
    updated.favorite = favorite;
    updated
}

/// The key `bw serve` puts an item's previous passwords under.
///
/// **Deliberately not a field on [`VaultItem`]**, for exactly the reason
/// [`DELETED_DATE_KEY`] is not: a typed field is a compile error at nineteen
/// struct literals across nine files. The precedent set there is followed
/// here -- an accessor over the catch-all, which buys the UI everything a
/// typed field would.
///
/// Riding `other` also means the array is preserved verbatim on every write
/// this app makes, which is the property that matters most: password history
/// is data the *server* maintains, and a client that dropped it on a PUT
/// would destroy it.
const PASSWORD_HISTORY_KEY: &str = "passwordHistory";

/// One previous password, with when it stopped being the current one.
///
/// `password` is `Zeroizing<String>` because a previous password **is a
/// password** -- the same reason [`LoginData::password`] is one. The
/// materialised copy this accessor hands back therefore wipes itself on
/// drop; the JSON value it was read *from* still sits in
/// [`VaultItem::other`] un-wiped, which is the existing, recorded escape
/// route rather than a new one (see the `>>>` zeroize block in
/// `.superpowers/sdd/progress.md`).
///
/// `last_used_date` is the raw ISO-8601 string `bw` sent, for the same reason
/// [`deleted_date`] returns one: this crate has no date type and inventing a
/// parse here would be modelling from memory. It is `Option` because the
/// entry is a JSON object whose keys this crate does not control -- an entry
/// with a password and no date is showable, and dropping the whole entry
/// because its timestamp was missing would hide a secret the user has.
#[derive(Clone)]
pub struct PasswordHistoryEntry {
    pub password: Zeroizing<String>,
    pub last_used_date: Option<String>,
}

/// Hand-written so `{:?}` cannot print a previous password. Every other
/// secret-carrying struct in this file derives `Debug` and does leak into a
/// format string, which is the recorded state of play -- this one is new, so
/// it starts without the escape route rather than adding one more.
impl std::fmt::Debug for PasswordHistoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordHistoryEntry")
            .field("password", &"<redacted>")
            .field("last_used_date", &self.last_used_date)
            .finish()
    }
}

/// The item's previous passwords, newest first as `bw` orders them.
///
/// Reads [`VaultItem::other`] rather than a modelled field -- see
/// [`PASSWORD_HISTORY_KEY`]. The wire shape is
/// `[{"lastUsedDate": "<iso>", "password": "<string>"}, ...]`, taken from the
/// CLI's own `PasswordHistoryResponse`
/// (`apps/cli/src/vault/models/password-history.response.ts`), which is the
/// class that literally builds this JSON, not a recollection of it.
///
/// **Every malformed shape yields an empty vector or a shorter one, never an
/// error**, and that is a decision rather than laziness. This is a read for
/// *display*: an item whose history is absent, `null`, an empty array or --
/// if Bitwarden ever changes it -- something else entirely has no previous
/// passwords worth showing, and a `Result` here would put an error banner on
/// a detail pane over a field the user never asked about. Absent and empty
/// are both real and both common: the CLI normalises an empty history to
/// `null` when saving (`adjustPasswordHistoryLength`), while the list
/// endpoint sends `[]` -- measured as `[]` on all 1654 items of the user's
/// live vault on 2026-08-01.
///
/// An entry with no `password` string is skipped rather than shown blank: the
/// row exists to show a secret, and a row of bullets over nothing claims the
/// user has a previous password that this build failed to load.
pub fn password_history(item: &VaultItem) -> Vec<PasswordHistoryEntry> {
    let Some(entries) = item.other.get(PASSWORD_HISTORY_KEY).and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let password = entry.get("password")?.as_str()?;
            Some(PasswordHistoryEntry {
                password: Zeroizing::new(password.to_string()),
                last_used_date: entry
                    .get("lastUsedDate")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        })
        .collect()
}

/// The key `bw serve` puts on an item that is in the trash, and on no other
/// item.
///
/// It is **deliberately not a field on [`VaultItem`]** -- it rides
/// [`VaultItem::other`] like `attachments` and `reprompt` do, and the two
/// functions below are the whole of this crate's typed access to it. The
/// tradeoff is recorded in full in `.superpowers/sdd/progress.md`; the short
/// version is that adding a field to `VaultItem` is a compile error at
/// nineteen struct literals across nine files, five of them in files the trash
/// backend does not own, and an accessor over the catch-all buys the UI
/// everything a typed field would (read it, sort by it, show "deleted N days
/// ago") without that blast radius.
const DELETED_DATE_KEY: &str = "deletedDate";


/// When `bw serve` says this item was trashed, or `None` for a live item.
///
/// Verified against the live backend (`.superpowers/sdd/item-shapes-capture.md`):
/// `GET /list/object/items?trash=true` returns ONLY trashed items and every
/// one of them carries `deletedDate`, while not one of the 1654 items in the
/// default list does. So presence of the key is exactly "this item is in the
/// trash", and this function is also the trashed-ness predicate.
///
/// Returns the raw ISO-8601 string `bw` sent rather than a parsed timestamp:
/// this crate has no date type, no date dependency, and nothing to check a
/// parse against, so inventing one here would be modelling from memory. A UI
/// that wants "deleted 3 days ago" parses this.
pub fn deleted_date(item: &VaultItem) -> Option<&str> {
    item.other.get(DELETED_DATE_KEY).and_then(|v| v.as_str())
}

/// A copy of `item` with the trash marker removed -- what a restored item
/// looks like, built the same way [`with_folder`] builds a moved one.
///
/// **This is not cosmetic.** A trashed item arrives carrying `deletedDate`,
/// that key rides [`VaultItem::other`], and `other` is serialized on every
/// write this app makes. Putting the item back into the live snapshot verbatim
/// would leave the snapshot's copy claiming a deletion date the server no
/// longer holds, and the next ordinary edit of that item would PUT the stale
/// key straight back at `bw serve`. What that backend does with a `deletedDate`
/// on an item PUT is **unverified** -- it was not probed, and this crate does
/// not guess -- which is exactly why the key is dropped here rather than sent
/// and hoped about.
///
/// Every other key is cloned untouched, including everything else riding the
/// catch-all.
pub fn without_deleted_date(item: &VaultItem) -> VaultItem {
    let mut restored = item.clone();
    restored.other.remove(DELETED_DATE_KEY);
    restored
}

/// The request body for a move: the item's ordinary write shape, with
/// `folderId` **stated explicitly** -- present, and `null` when the item is
/// being un-filed.
///
/// The explicit statement is the whole point of this function, and of the move
/// path existing separately from [`VaultBridge::update_item`] at all.
/// `.superpowers/sdd/put-semantics-capture.md` records a live experiment
/// against the user's `bw serve`: **omitted keys are MERGED, not cleared.**
/// Three keys (`notes`, `login.uris`, `fields`) were each dropped from a PUT
/// and all three survived it. `VaultItem::folder_id` carries
/// `skip_serializing_if = "Option::is_none"`, so the obvious implementation --
/// set `folder_id = None` and hand the item to `update_item` -- produces a body
/// with **no `folderId` key**, which that server merges: the item stays in its
/// old folder while the app believes it moved. That is the recorded
/// `login.uris` empty-vec defect, except reachable.
///
/// The key is inserted UNCONDITIONALLY rather than only when un-filing, so
/// there is no branch that can be got wrong, and so a move to a folder and a
/// move out of one produce the same shape with different values. An item that
/// was already unfiled and stays unfiled therefore gains a `"folderId": null`
/// where an ordinary write would have had no key at all. That is deliberate,
/// and it is harmless on this backend for a reason already shipped and tested:
/// [`VaultBridge::create_item`] has always POSTed `"folderId": null` for an
/// item with no folder (see
/// `create_item_omits_blank_username_and_password_instead_of_sending_empty_strings`),
/// so `bw serve` accepting explicit null as "no folder" is not an inference.
/// The alternative -- skipping the write when the item's local `folder_id`
/// already equals the target -- was rejected: that local copy comes from the
/// vault window's own `Vec`, which can be behind the server, so a stale field
/// would turn a real move into a silent no-op. That is the same class of
/// failure this function exists to prevent, moved one layer up.
///
/// Nothing else about the body is touched, so every OTHER write in this app is
/// byte-identical to what it sent before this path existed -- pinned by
/// `an_ordinary_update_of_an_unfiled_item_still_omits_folder_id_entirely`.
fn folder_move_body(
    item: &VaultItem,
    folder_id: Option<&str>,
) -> Result<serde_json::Value, VaultError> {
    let moved = with_folder(item, folder_id);
    let value = serde_json::to_value(&moved).map_err(|e| VaultError::Parse(e.to_string()))?;
    match value {
        serde_json::Value::Object(mut map) => {
            map.insert(
                "folderId".to_string(),
                match folder_id {
                    Some(id) => serde_json::Value::String(id.to_string()),
                    None => serde_json::Value::Null,
                },
            );
            Ok(serde_json::Value::Object(map))
        }
        // Unreachable: `VaultItem` is a struct, so `to_value` yields an
        // object. It is an error rather than a `expect`/`unwrap` because the
        // alternative to panicking here is NOT "send it anyway" -- a body
        // without the key is precisely the silent no-op above, so there is
        // nothing safe to fall through to. No test covers this arm; it cannot
        // be reached without changing `VaultItem` into something that is not a
        // struct.
        other => Err(VaultError::Parse(format!(
            "a vault item serialized to {other} rather than a JSON object, so its folderId \
             could not be stated explicitly"
        ))),
    }
}

/// The key Bitwarden uses as an item's **optimistic-concurrency token**, and
/// the reason every write on this type answers with the server's copy.
///
/// It rides [`VaultItem::other`] like [`DELETED_DATE_KEY`], so it is echoed
/// back verbatim on every full-state PUT this app makes. That is not inert.
/// Measured against the user's live `bw serve` 2026.7.0 on a throwaway item:
///
/// | PUT | body's `revisionDate` | result |
/// |---|---|---|
/// | 1st after a fetch | what the fetch reported | 200, server answers with a NEWER one |
/// | 2nd from the same fetched value | the fetch's, now stale | **400** |
///
/// The 400's message is `The client copy of this cipher is out of date.
/// Resync the client and try again.` -- so a caller that keeps the value it
/// SENT (rather than the one it got back) is holding a token the server has
/// already superseded, and its next write of that item is refused.
///
/// This is why [`VaultBridge::update_item`] and
/// [`VaultBridge::move_item_to_folder`] return `VaultItem` rather than `()`.
/// Stripping the key instead was tried and rejected: `bw serve` answers 200
/// and reports the new state, but a `GET` immediately afterwards still shows
/// the OLD one -- the write is accepted and then not visible, which is worse
/// than a refusal because nothing surfaces it.
///
/// **No longer `#[cfg(test)]`.** It was, because the only thing that named it
/// was the mock below -- production code never touched the key, it just let it
/// ride `other`. [`with_revision_date_from`] is the first production reader,
/// and it exists because one write cannot let the key ride: see its doc.
const REVISION_DATE_KEY: &str = "revisionDate";

/// A copy of `item` carrying `source`'s revision token instead of its own.
///
/// **For the two writes that cannot return the server's copy.**
/// `POST /restore/item/{id}` -- which both un-trashes and un-archives -- has
/// no body this crate has verified the shape of, so
/// [`crate::vault_cache::VaultCache::restore_item`] and
/// [`crate::vault_cache::VaultCache::unarchive_item`] cannot adopt an answer
/// the way [`VaultBridge::update_item`] and [`VaultBridge::move_item_to_folder`]
/// do. They read the item back with [`VaultBridge::get_item`] and adopt this
/// one key off it instead.
///
/// **One key and not the whole item**, deliberately. A `GET` taken right after
/// a restore may race the ~1.5s settle this backend was measured to have
/// (see [`VaultBridge::archive_item`]), so its copy can still be the
/// pre-restore one -- carrying `deletedDate`, which
/// [`without_deleted_date`] exists to keep out of the live snapshot.
/// Swapping the whole item in would reinstate exactly that. The revision
/// token is the one field where the server's value is right *whatever* the
/// settle is doing, because a token is only ever "the one the next write must
/// quote".
///
/// An absent key on `source` REMOVES the key rather than leaving `item`'s in
/// place: "the server reports no token" and "the server reports the token I
/// already had" are different claims, and keeping a superseded token because
/// the answer did not mention one is the defect this function exists for.
pub fn with_revision_date_from(item: &VaultItem, source: &VaultItem) -> VaultItem {
    let mut adopted = item.clone();
    match source.other.get(REVISION_DATE_KEY) {
        Some(revision) => {
            adopted.other.insert(REVISION_DATE_KEY.to_string(), revision.clone());
        }
        None => {
            adopted.other.remove(REVISION_DATE_KEY);
        }
    }
    adopted
}

/// A `mockito` PUT mock that answers the way `bw serve` really does: with the
/// item the request carried, wrapped in the success envelope, and with
/// `REVISION_DATE_KEY` advanced to `new_revision`.
///
/// **Every write mock in this crate used to answer `.with_status(200)` and an
/// EMPTY body.** That is not this backend, and the gap is exactly where the
/// stale-revision defect lived: a suite whose PUTs answer nothing can never
/// notice that the app is throwing the answer away, nor that the token it
/// keeps instead is one the server has already superseded. Reach for this
/// rather than a bare `with_status(200)` for any write that succeeds.
#[cfg(test)]
pub fn echoing_item_put(
    server: &mut mockito::Server,
    path: &str,
    new_revision: &'static str,
) -> mockito::Mock {
    server
        .mock("PUT", path)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body_from_request(move |req| {
            let sent = req.body().expect("a PUT this app makes always carries a body");
            let mut item: serde_json::Value =
                serde_json::from_slice(sent).expect("this app's write bodies are JSON");
            if let Some(map) = item.as_object_mut() {
                map.insert(REVISION_DATE_KEY.to_string(), serde_json::json!(new_revision));
            }
            serde_json::json!({ "success": true, "data": item }).to_string().into_bytes()
        })
}

/// Whether `item` carries a `deskwarden:app-match` custom field **at all**,
/// answered on the field's NAME alone and therefore independently of whether
/// its value parses.
///
/// [`extract_app_match`] cannot answer this: it ends in `.ok()`, so a field
/// whose JSON is malformed, or whose `trigger` is a string this build does not
/// know, is indistinguishable from no field at all. That field is a visible
/// custom field in every Bitwarden client and is hand-editable in all of them,
/// so the corrupted shape is reachable without this app ever being involved --
/// and the detail pane's `MATCHED APP` card used to answer it with "No app is
/// matched to this item yet" and no Remove, which left the field unclearable
/// from that pane by any sequence of clicks.
///
/// The card asks this and [`extract_app_match`] both, and the pair of answers
/// is what separates "unbound" from "bound to something unreadable". The
/// clear-up path is [`without_app_match`], which already filters on the same
/// name and so removes the field whether or not it parses.
pub fn has_app_match_field(item: &VaultItem) -> bool {
    item.fields
        .iter()
        .any(|f| f.name.as_deref() == Some(APP_MATCH_FIELD_NAME))
}

pub fn extract_app_match(item: &VaultItem) -> Option<AppMatch> {
    item.fields
        .iter()
        .find(|f| f.name.as_deref() == Some(APP_MATCH_FIELD_NAME))
        .and_then(|f| f.value.as_deref())
        .and_then(|v| AppMatch::from_field_value(v).ok())
}

/// A copy of `item` with the `deskwarden:app-match` custom field **gone** --
/// the inverse of [`with_app_match`], and the only way an app binding can be
/// undone.
///
/// It exists because [`with_app_match`] cannot express "no match": its whole
/// body is "rebuild that one field", and there is no [`AppMatch`] value that
/// means unbound. The detail pane's `MATCHED APP` card offers Remove, and
/// without this the only honest thing it could offer was nothing.
///
/// **The field is removed, not blanked.** A field left in place with an empty
/// value would still be a `deskwarden:app-match` row in every other Bitwarden
/// client, and [`extract_app_match`] would file it as a match that fails to
/// parse -- which is indistinguishable from a corrupted one. Removing it puts
/// the item back into exactly the shape it had before any match was saved.
///
/// Every other field is cloned wholesale, and so is everything unmodelled
/// riding [`VaultItem::other`], for the reason [`with_app_match`] records: the
/// PUT is state-replacing, so anything this function drops is dropped from the
/// user's vault.
///
/// An item that carries no such field comes back unchanged -- so a Remove that
/// races a Remove is a no-op rather than an error.
pub fn without_app_match(item: &VaultItem) -> VaultItem {
    let mut updated = item.clone();
    updated
        .fields
        .retain(|f| f.name.as_deref() != Some(APP_MATCH_FIELD_NAME));
    updated
}

#[derive(Debug)]
pub enum VaultError {
    Http(String),
    Parse(String),
    /// `bw serve` answered with `401 Unauthorized`: the session token it was
    /// started with (or handed per-request) is no longer valid. Distinct
    /// from the catch-all `Http` because it is the one HTTP failure that
    /// means "re-authenticate", not "retry" -- `bw serve` can stay alive and
    /// keep answering every other request normally while every one of these
    /// keeps failing, which a plain `Http(String)` gives callers no clean way
    /// to detect without parsing the message.
    Unauthorized,
}

/// Turns a failed `ureq` call into a [`VaultError`], distinguishing a
/// `401 Unauthorized` response (see `VaultError::Unauthorized`'s doc) from
/// every other transport/status failure.
fn map_http_err(e: ureq::Error) -> VaultError {
    match e {
        ureq::Error::Status(401, _) => VaultError::Unauthorized,
        e => VaultError::Http(e.to_string()),
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[allow(dead_code)]
    success: bool,
    data: T,
}

#[derive(Deserialize)]
struct ItemList {
    data: Vec<VaultItem>,
}

/// A vault folder as `bw serve` lists it.
///
/// The `#[serde(flatten)] other` is the same rule as [`UriEntry`]'s and
/// [`VaultField`]'s -- keep every key the server sent, so nothing this app
/// writes back can be a truncated copy of what it read. Unlike those two it is
/// currently belt-and-braces rather than load-bearing: *nothing PUTs a folder
/// today*. `create_folder`/`update_folder` build their payload as
/// `json!({ "name": name })` and never serialize a `Folder`, `delete_folder`
/// sends no body, and `VaultCache`'s folder methods only mutate the in-memory
/// snapshot. The path that makes it load-bearing is the encrypted vault disk
/// cache, which serializes `Vec<Folder>` to disk and reads it back: without
/// this, every key beyond id/name (`organizationId`, `revisionDate`, ...)
/// would be lost across the round trip through the file.
///
/// One shared caveat with the other catch-alls: `other` is `pub` and
/// unguarded, so inserting `"id"` or `"name"` into it would emit a duplicate
/// JSON key and `bw` would take the last one -- unreachable today, since
/// deserialize routes those two into the typed fields and no code writes to
/// `other`, but it is why `other` should stay something only serde fills.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct FolderList {
    data: Vec<Folder>,
}

/// The payload to create one new item, of any kind `bw serve` understands.
///
/// `bw serve`'s create endpoint wants a full item shape (like the edit
/// endpoint), but a brand new item has nothing else to preserve, unlike
/// `update_item`.
///
/// **An enum, not a struct with four optional sub-objects**, because a create
/// payload must carry EXACTLY ONE type object. A struct of optionals makes "a
/// card that also posts an empty `login: {}`" representable, and that is
/// precisely how an item ends up with two type objects -- the same defect
/// class as `EditDraft::apply_to`'s unconditional `login.unwrap_or_default()`,
/// which was a live bug. Here it is not a rule to remember; it is unsayable.
///
/// Every variant carries `name` and `folder_id` because those are item-level,
/// not part of any type object. The kind-specific payload reuses the SAME
/// structs the read path deserializes into ([`CardData`], [`IdentityData`]),
/// so the wire key names have exactly one definition and the create and edit
/// paths cannot drift apart.
#[derive(Debug, Clone)]
pub enum NewItem {
    Login {
        name: String,
        folder_id: Option<String>,
        username: String,
        password: String,
    },
    /// A secure note's body is **item-level `notes`**, not a field of its type
    /// object: `secureNote` carries only a `{"type": 0}` discriminator
    /// (verified, `.superpowers/sdd/item-shapes-capture.md`).
    SecureNote {
        name: String,
        folder_id: Option<String>,
        body: String,
    },
    Card {
        name: String,
        folder_id: Option<String>,
        card: CardData,
    },
    Identity {
        name: String,
        folder_id: Option<String>,
        identity: IdentityData,
    },
    /// Spelled out field by field rather than carrying an `SshKeyData`,
    /// because that struct does not exist in this file yet -- `type: 5`'s
    /// shape was verified separately and the struct lands on its own branch.
    /// The three names below are the captured wire keys, so switching this
    /// variant to `ssh_key: SshKeyData` once that branch merges is a local
    /// change with no change to the bytes.
    SshKey {
        name: String,
        folder_id: Option<String>,
        private_key: Zeroizing<String>,
        public_key: String,
        key_fingerprint: String,
    },
}

impl NewItem {
    pub fn login(
        name: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
        folder_id: Option<String>,
    ) -> Self {
        NewItem::Login {
            name: name.into(),
            folder_id,
            username: username.into(),
            password: password.into(),
        }
    }

    pub fn secure_note(
        name: impl Into<String>,
        body: impl Into<String>,
        folder_id: Option<String>,
    ) -> Self {
        NewItem::SecureNote { name: name.into(), folder_id, body: body.into() }
    }

    pub fn card(name: impl Into<String>, card: CardData, folder_id: Option<String>) -> Self {
        NewItem::Card { name: name.into(), folder_id, card }
    }

    pub fn identity(
        name: impl Into<String>,
        identity: IdentityData,
        folder_id: Option<String>,
    ) -> Self {
        NewItem::Identity { name: name.into(), folder_id, identity }
    }

    pub fn ssh_key(
        name: impl Into<String>,
        private_key: impl Into<String>,
        public_key: impl Into<String>,
        key_fingerprint: impl Into<String>,
        folder_id: Option<String>,
    ) -> Self {
        NewItem::SshKey {
            name: name.into(),
            folder_id,
            private_key: Zeroizing::new(private_key.into()),
            public_key: public_key.into(),
            key_fingerprint: key_fingerprint.into(),
        }
    }

    /// `pub` because a refused create has to name what it refused, and the
    /// only name that exists at that point is this one -- there is no vault
    /// item yet. See `vault_window`'s `ItemWrite::Create`.
    pub fn name(&self) -> &str {
        match self {
            NewItem::Login { name, .. }
            | NewItem::SecureNote { name, .. }
            | NewItem::Card { name, .. }
            | NewItem::Identity { name, .. }
            | NewItem::SshKey { name, .. } => name,
        }
    }

    fn folder_id(&self) -> Option<&str> {
        match self {
            NewItem::Login { folder_id, .. }
            | NewItem::SecureNote { folder_id, .. }
            | NewItem::Card { folder_id, .. }
            | NewItem::Identity { folder_id, .. }
            | NewItem::SshKey { folder_id, .. } => folder_id.as_deref(),
        }
    }

    /// The exact JSON `create_item` POSTs.
    ///
    /// Pure, so the wire shape is asserted directly by unit tests rather than
    /// inferred from a mock's return value.
    ///
    /// **Blank means absent, not an empty string** -- the convention
    /// [`VaultBridge::create_item`] has always followed, now applied to every
    /// kind by one shared rule (see [`without_blank_values`]) instead of one
    /// hand-written `if` per field. The edit path maps blank to an absent key,
    /// so a create that sent `""` would make the item silently change shape
    /// server-side between its first and second save.
    ///
    /// **`folderId` is stated explicitly, `null` when the item is unfiled.**
    /// This is deliberately NOT what the update path does, and the two must
    /// not be "tidied" into agreement: `.superpowers/sdd/put-semantics-capture.md`
    /// records that on a PUT this backend silently IGNORES a null `folderId`,
    /// so a null there means "keep the old folder" and cannot clear one. On a
    /// CREATE there is no previous value to preserve, so the null is
    /// unambiguous -- and it is the shape this app has always POSTed and
    /// `bw serve` has always accepted, so keeping it is also the option that
    /// changes nothing about a request the server is known to like.
    pub fn to_payload(&self) -> serde_json::Value {
        let (type_number, type_key, type_object, notes) = match self {
            NewItem::Login { username, password, .. } => {
                let mut login = serde_json::Map::new();
                login.insert("username".to_string(), serde_json::json!(username));
                login.insert("password".to_string(), serde_json::json!(password));
                (1, "login", login, None)
            }
            NewItem::SecureNote { body, .. } => (
                2,
                "secureNote",
                // Not pruned: `{"type": 0}` is the discriminator that makes
                // this a secure note, and 0 is its real value, not a blank.
                serde_json::Map::from_iter([("type".to_string(), serde_json::json!(0))]),
                Some(body.as_str()),
            ),
            NewItem::Card { card, .. } => (3, "card", as_object(card), None),
            NewItem::Identity { identity, .. } => (4, "identity", as_object(identity), None),
            NewItem::SshKey { private_key, public_key, key_fingerprint, .. } => {
                let mut ssh = serde_json::Map::new();
                ssh.insert("privateKey".to_string(), serde_json::json!(private_key.as_str()));
                ssh.insert("publicKey".to_string(), serde_json::json!(public_key));
                ssh.insert("keyFingerprint".to_string(), serde_json::json!(key_fingerprint));
                (5, "sshKey", ssh, None)
            }
        };

        let mut payload = serde_json::Map::new();
        payload.insert("name".to_string(), serde_json::json!(self.name()));
        payload.insert("type".to_string(), serde_json::json!(type_number));
        payload.insert(
            "folderId".to_string(),
            match self.folder_id() {
                Some(id) => serde_json::json!(id),
                None => serde_json::Value::Null,
            },
        );
        if let Some(body) = notes.filter(|b| !b.is_empty()) {
            payload.insert("notes".to_string(), serde_json::json!(body));
        }
        payload.insert(
            type_key.to_string(),
            serde_json::Value::Object(without_blank_values(type_object)),
        );
        serde_json::Value::Object(payload)
    }
}

/// Serializes one of the typed item sub-objects into a JSON map.
///
/// The `unwrap_or_default` arm is unreachable: every caller passes a struct,
/// so `to_value` yields an object and cannot fail. An empty map is the right
/// fallback anyway -- it produces a payload with a present-but-empty type
/// object, which is exactly what a create with no fields filled in sends.
fn as_object<T: Serialize>(value: &T) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

/// Drops every key whose value is the empty string -- the one place the
/// "blank means absent" convention is implemented for a create payload.
///
/// Applied to the whole type object rather than per field, so a field added to
/// [`CardData`] or [`IdentityData`] later inherits the convention instead of
/// needing a new `if` that someone has to remember to write. `None` fields
/// never reach here at all: every field on those structs carries
/// `skip_serializing_if = "Option::is_none"`.
///
/// Shallow on purpose. These objects are flat maps of strings; recursing would
/// mean guessing at a nesting that does not exist.
fn without_blank_values(
    object: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    object.into_iter().filter(|(_, v)| v.as_str() != Some("")).collect()
}

/// `GET /object/totp/{id}` wraps its answer the same way `list_items` and
/// `list_folders` do: the envelope's `data` is itself an object carrying a
/// nested `data` field, not the bare value. `Envelope<Option<String>>` would
/// try to deserialize that nested object directly as an `Option<String>` and
/// fail on the very success case it's meant to handle -- so it needs this
/// wrapper, same as `ItemList`/`FolderList`.
#[derive(Deserialize)]
struct TotpData {
    data: Option<String>,
}

/// `GET /generate` wraps its answer the same double-nested way
/// `/object/totp/{id}` does -- the CLI builds a `StringResponse`
/// (`{object: "string", data: "<secret>"}`) and hands it to
/// `Response.success`, which puts *that* in the envelope's `data`. Same
/// wrapper rule as [`TotpData`], and it is asserted rather than assumed by
/// `a_generator_response_that_is_not_the_expected_envelope_is_a_parse_error`.
#[derive(Deserialize)]
struct GeneratedString {
    data: String,
}

/// What to ask `GET /generate` for: a character password, or a word
/// passphrase.
///
/// **The two are different requests, not one request with a flag**, because
/// they take disjoint options: `length`/`minNumber`/`minSpecial`/the four
/// character classes belong to one and `words`/`separator`/`capitalize`/
/// `includeNumber` to the other. `GenerateCommand` reads *every* key on
/// *every* request and simply ignores the ones its chosen type has no use
/// for, so a single flat struct would happily send a character length with a
/// passphrase request and appear to work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerateRequest {
    Password(PasswordRecipe),
    Passphrase(PassphraseRecipe),
}

/// A character password's options, named for the query keys the **serve
/// route** accepts.
///
/// VERIFIED, not transcribed from the CLI's flag list: `bw serve`'s
/// `/generate` handler passes `ctx.request.query` straight to
/// `GenerateCommand.run`, which reads `uppercase`, `lowercase`, `number`,
/// `special`, `length`, `passphrase`, `separator`, `words`, `capitalize`,
/// `includeNumber`, `minNumber`, `minSpecial` and `ambiguous` off that object
/// (`apps/cli/src/oss-serve-configurator.ts`,
/// `apps/cli/src/tools/generate.command.ts`). The CLI's SHORT flags -- `-u`,
/// `-l`, `-n`, `-s`, `-p`, `-c` -- are commander aliases on the argv parser
/// and do not exist as query keys; `?u=true` is read by nothing and silently
/// drops the class. Cross-checked against the user's live `bw serve`
/// (2026.7.0) on 2026-08-01, which answered both spellings of a real request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordRecipe {
    /// `length`. The route clamps anything below 5 up to 5.
    pub length: u32,
    pub uppercase: bool,
    pub lowercase: bool,
    pub number: bool,
    pub special: bool,
    /// `minNumber`.
    pub min_number: u32,
    /// `minSpecial`.
    pub min_special: u32,
    /// `ambiguous` -- and the name here is the opposite way round from the
    /// wire key ON PURPOSE. The CLI's own help for the flag is "Avoid
    /// ambiguous characters", and `GenerateCommand` passes
    /// `ambiguous: !normalizedOptions.ambiguous` into the generator, whose
    /// `ambiguous: true` means *allow* them. So `ambiguous=true` on the query
    /// string means "avoid", which is what this field is called.
    pub avoid_ambiguous: bool,
}

/// Deskwarden's own default recipe, and it is deliberately **not** the CLI's
/// (`-uln --length 14`): this is what a user gets for clicking Generate
/// beside a password box, so it turns on all four classes and asks for 20
/// characters rather than shipping a weaker default because a command-line
/// tool from another decade had one.
impl Default for PasswordRecipe {
    fn default() -> Self {
        Self {
            length: 20,
            uppercase: true,
            lowercase: true,
            number: true,
            special: true,
            min_number: 1,
            min_special: 1,
            avoid_ambiguous: true,
        }
    }
}

/// A word passphrase's options. See [`PasswordRecipe`] for how the key
/// spellings were verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassphraseRecipe {
    /// `words`. The route clamps anything below 3 up to 3.
    pub words: u32,
    /// `separator`. The route takes only the FIRST character of anything
    /// longer than one, and treats the literal words `space` and `empty` as
    /// `" "` and `""`.
    pub separator: String,
    pub capitalize: bool,
    /// `includeNumber`.
    pub include_number: bool,
}

impl Default for PassphraseRecipe {
    fn default() -> Self {
        Self {
            words: 4,
            separator: "-".to_string(),
            capitalize: true,
            include_number: true,
        }
    }
}

impl GenerateRequest {
    /// The query parameters, in a fixed order so a test can assert the whole
    /// string exactly.
    ///
    /// **Every boolean is stated, `false` included**, rather than omitted when
    /// off. Two reasons, both measured from the route's own code. First,
    /// `convertBooleanOption` reads a missing key as `false` *and* reads any
    /// value that is not `""` or `"true"` as `false`, so an omitted `special`
    /// and a `special=false` are the same thing to the server -- stating it
    /// costs nothing and makes the request self-describing on the wire.
    /// Second, and the reason it matters: when all four character classes come
    /// out false the command **silently substitutes `uppercase + lowercase +
    /// number`**. A recipe that asked for lowercase only would therefore be
    /// honoured, but one that asked for nothing at all comes back as a
    /// three-class password with no error -- so a caller must not let all four
    /// be off, and this function does not hide the fact that it can happen.
    fn query(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::Password(p) => vec![
                ("length", p.length.to_string()),
                ("uppercase", bool_param(p.uppercase)),
                ("lowercase", bool_param(p.lowercase)),
                ("number", bool_param(p.number)),
                ("special", bool_param(p.special)),
                ("minNumber", p.min_number.to_string()),
                ("minSpecial", p.min_special.to_string()),
                ("ambiguous", bool_param(p.avoid_ambiguous)),
            ],
            // `passphrase` FIRST, so the switch that decides which kind of
            // secret this is reads at the front of the string.
            Self::Passphrase(p) => vec![
                ("passphrase", bool_param(true)),
                ("words", p.words.to_string()),
                ("separator", p.separator.clone()),
                ("capitalize", bool_param(p.capitalize)),
                ("includeNumber", bool_param(p.include_number)),
            ],
        }
    }

    /// The query as one `key=value&...` string. Test-facing: it is what makes
    /// "a passphrase request states no character length" assertable without a
    /// server. The real call builds the request from [`Self::query`] pair by
    /// pair so `ureq` does the percent-encoding.
    #[cfg(test)]
    fn query_string(&self) -> String {
        self.query()
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&")
    }
}

/// A boolean as the route spells it. Not `to_string()` inline at each call
/// site: `convertBooleanOption` accepts exactly `""` and `"true"` and reads
/// `"1"`, `"yes"` and `"on"` as *off*, so which spelling is used is a wire
/// fact worth having in one place.
fn bool_param(value: bool) -> String {
    if value { "true".to_string() } else { "false".to_string() }
}

#[derive(Clone)]
pub struct VaultBridge {
    base_url: String,
    /// For the routes `bw serve` answers out of its own local vault. See
    /// [`READ_DEADLINE`].
    read_agent: crate::http_agent::TotalBounded,
    /// For the routes that push to `vault.bitwarden.com` before answering.
    /// See [`WRITE_DEADLINE`].
    write_agent: crate::http_agent::TotalBounded,
}

/// Connect timeout for `agent` below. `bw serve` is a local process on
/// `localhost`, not a remote host, so even this is generous for a plain TCP
/// handshake -- the point is only to fail fast if the port stops accepting
/// connections at all (the process died, or never started), not to give a
/// slow network the benefit of the doubt.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Total-time bound for [`VaultBridge::read_agent`]: the longest a single
/// *read* may take before it is treated as failed.
///
/// **This is a freeze budget, not a patience budget.** Nine bridge calls are
/// still made synchronously on the eframe UI thread, including the
/// once-per-second TOTP poll, so this number is measured in "how long the
/// whole app is unresponsive", and every second of it is paid by the user
/// watching a frozen window.
///
/// **It applies only to the routes `bw serve` answers by itself.** `GET
/// /list/object/items`, `/list/object/folders`, `/object/item/{id}`,
/// `/object/totp/{id}` and `/generate` are all served out of the local vault
/// file (or computed locally, for a TOTP code and a generated password) with
/// no network hop of any kind. That is what makes a 10s cap on them fair: the
/// only thing on the other end is a Node process on loopback that is either
/// answering or broken. The writes do *not* have that property and do not use
/// this constant -- see [`WRITE_DEADLINE`].
///
/// 10s. Derived from what the call actually costs, not from another
/// constant: `/list/object/items` -- the slowest read this bridge makes --
/// measures 1.1s cold and 1.2s warm against this app's largest observed vault
/// (1654 items), so this is roughly 8x headroom for a bigger vault on a loaded
/// machine.
///
/// It is deliberately **not** [`crate::bw_serve::READINESS_DEADLINE`], which
/// an earlier version of this comment cited. That 30s is the total budget for
/// a whole *retry schedule* (`bw_serve::readiness_schedule`) covering a Node
/// cold start -- a different quantity from a single call's bound. And this
/// codebase has already decided 30s is the wrong length for a legitimate
/// single backend operation, in both directions: `bw_serve::BACKEND_OP_TIMEOUT`
/// exists precisely because 30s is too *short* for a backend start or sync.
/// Neither of those runs through this agent (`bw sync` is a separate CLI
/// process), so borrowing either number would import reasoning that does not
/// apply.
///
/// One caveat this budget does not cover, and it is the caller's to know:
/// [`crate::vault_cache::VaultCache::populate`] makes **two** bounded calls in
/// sequence (`list_items` then `list_folders`, documented as two round-trips
/// on that method), so `vault_window`'s populate path can freeze for 2 x this,
/// not this. No single call exceeds the budget; the sequence is not one call.
///
/// Whole-request rather than per-read, and that is what closes the v0.3.0
/// hang: a per-read timeout does not survive connection reuse, and this agent
/// pools aggressively (the TOTP poll hits the same socket every second). See
/// [`crate::http_agent`] for the full trace. Bounding the body along with the
/// head costs nothing here -- every response is JSON from a process on
/// loopback, not a bulk transfer.
///
/// `pub(crate)` -- the only constant in this module that is, and for one named
/// reason. [`crate::app_window::WORKING_DEADLINE`] is a **sum over the startup
/// worker's phases**, and one of those phases is `bw_serve::wait_for_vault_ready`
/// making 11 [`VaultBridge::list_items`] calls, each bounded by this. That sum
/// used to carry a hand-copied `BRIDGE_READ_BUDGET = 10s` mirror of this value,
/// because this was private -- so raising this number silently left the window's
/// watchdog too short and it would have started killing healthy startups.
/// Deriving the sum from the source is what removed the mirror. Exported to that
/// one reader only; the rest of this module's constants
/// ([`CONNECT_TIMEOUT`], [`WRITE_DEADLINE`]) bound nothing outside it and stay
/// private.
pub(crate) const READ_DEADLINE: Duration = Duration::from_secs(10);

/// Total-time bound for [`VaultBridge::write_agent`]: the longest a single
/// *write* may take before it is treated as failed.
///
/// **Separate from [`READ_DEADLINE`] because the two are not the same
/// situation, however similar the code looks.** `bw serve`'s POST/PUT/DELETE
/// routes re-encrypt the item and push it to `vault.bitwarden.com`
/// synchronously before answering -- this crate already states that at
/// `vault_cache.rs`'s `set_app_match` doc ("the user has been told the save
/// succeeded. It had -- server-side"). So a write is bounded by the user's
/// link to Bitwarden's API, not by loopback, and the 1.2s measurement that
/// justifies the read budget is no evidence about it whatsoever. Sharing one
/// constant between them would be the same mistake [`crate::http_agent`]
/// exists to undo, one level down: two distinct situations averaged into one
/// number, with a doc comment describing only the friendlier of them.
///
/// What a wrong value costs here is also asymmetric, and it is why this is the
/// larger number. A read that times out is a stale list and a retry. A write
/// that times out is a **lie**: the push may well have completed at the server
/// and the user is shown a failure for a save that happened. The cache's own
/// write-through reasoning is built on that asymmetry.
///
/// 30s. Not derived from [`READ_DEADLINE`] and not from `bw_serve`'s
/// lifecycle numbers; it is the same length this crate already gives its other
/// call over the public internet to a third-party API,
/// `updater::API_DEADLINE`, which is the closest comparable situation it has
/// (small request, external host, unknown link). It is a real cost: a write
/// still runs on the UI thread, so 30s is 30s of frozen window in the worst
/// case. That is accepted rather than hidden, on the grounds that a write is
/// user-initiated and expected to take a moment, whereas the TOTP poll that
/// dominates the read path is neither.
///
/// **Not bounded by anything here**: how long Bitwarden's API itself takes.
/// If a save legitimately needs more than 30s the user still sees a failure
/// for a write that may land later. No time-based rule can tell that apart
/// from a dead link, and this says so rather than implying otherwise.
const WRITE_DEADLINE: Duration = Duration::from_secs(30);

impl VaultBridge {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            read_agent: crate::http_agent::bounded_total(CONNECT_TIMEOUT, READ_DEADLINE),
            write_agent: crate::http_agent::bounded_total(CONNECT_TIMEOUT, WRITE_DEADLINE),
        }
    }

    pub fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        let url = format!("{}/list/object/items", self.base_url);
        let body: Envelope<ItemList> = self
            .read_agent
            .get(&url)
            .call()
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data.data)
    }

    /// Fetches a single item by id via `GET /object/item/{id}`.
    ///
    /// Used by the fill path, which previously pulled the *entire* vault and
    /// linear-scanned it every time a single item's credentials were needed.
    pub fn get_item(&self, id: &str) -> Result<VaultItem, VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, id);
        let body: Envelope<VaultItem> = self
            .read_agent
            .get(&url)
            .call()
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data)
    }

    pub fn list_folders(&self) -> Result<Vec<Folder>, VaultError> {
        let url = format!("{}/list/object/folders", self.base_url);
        let body: Envelope<FolderList> = self
            .read_agent
            .get(&url)
            .call()
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data.data)
    }

    /// Answers with the server's copy, for `REVISION_DATE_KEY`'s reason:
    /// this is [`Self::update_item`] with the body built for it.
    pub fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<VaultItem, VaultError> {
        self.update_item(&with_app_match(item, m))
    }

    pub fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        let url = format!("{}/object/folder", self.base_url);
        let body: Envelope<Folder> = self
            .write_agent
            .post(&url)
            .send_json(serde_json::json!({ "name": name }))
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data)
    }

    /// Renames a folder via `bw serve`'s `PUT /object/folder/{id}` -- the
    /// same endpoint shape as [`Self::update_item`], scoped to folders.
    pub fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        let url = format!("{}/object/folder/{}", self.base_url, id);
        let body: Envelope<Folder> = self
            .write_agent
            .put(&url)
            .send_json(serde_json::json!({ "name": name }))
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data)
    }

    pub fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/object/folder/{}", self.base_url, id);
        self.write_agent
            .delete(&url)
            .call()
            .map_err(map_http_err)?;
        Ok(())
    }

    /// Creates an item of any kind. The body is [`NewItem::to_payload`]'s,
    /// which is where the "blank means absent" and explicit-`folderId`
    /// conventions live and where they are tested.
    pub fn create_item(&self, new_item: &NewItem) -> Result<VaultItem, VaultError> {
        let url = format!("{}/object/item", self.base_url);
        let body: Envelope<VaultItem> = self
            .write_agent
            .post(&url)
            .send_json(new_item.to_payload())
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data)
    }

    /// Writes `item` back as its own new state -- the same PUT `set_app_match`
    /// already used, generalized so the vault window's edit flow doesn't need
    /// its own copy of it.
    ///
    /// **Returns the item the SERVER answered with, not `Ok(())`, and every
    /// caller must store that rather than the value it sent.** See
    /// `REVISION_DATE_KEY`: the body carries `revisionDate` out of
    /// [`VaultItem::other`], Bitwarden reads it as an optimistic-concurrency
    /// token, and the write bumps it. A caller that keeps its own pre-write
    /// copy is holding a stale token from the moment this returns, and its
    /// NEXT write of the same item is rejected with 400 "The client copy of
    /// this cipher is out of date."
    pub fn update_item(&self, item: &VaultItem) -> Result<VaultItem, VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, item.id);
        let body: Envelope<VaultItem> = self
            .write_agent
            .put(&url)
            .send_json(item)
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data)
    }

    /// Files `item` under `folder_id`, or un-files it when that is `None`.
    ///
    /// A dedicated path rather than `update_item(&with_folder(..))`, and the
    /// difference is one key: `VaultItem::folder_id` is skipped when absent,
    /// so the `update_item` spelling PUTs a body with no `folderId` at all,
    /// which `bw serve` MERGES -- the item keeps its old folder and the
    /// un-file silently does nothing. [`folder_move_body`] states the key
    /// explicitly and carries the full reasoning; the failure it prevents was
    /// watched, as a 501 from a mockito matcher that rejects the omitting
    /// body.
    ///
    /// Callers should reach this through [`crate::vault_cache::VaultCache`],
    /// not here, so the snapshot moves with the server.
    ///
    /// Answers with the server's copy for the reason [`Self::update_item`]
    /// does -- this is the same PUT, so it invalidates the caller's
    /// `revisionDate` in exactly the same way. See `REVISION_DATE_KEY`.
    pub fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<VaultItem, VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, item.id);
        let body: Envelope<VaultItem> = self
            .write_agent
            .put(&url)
            .send_json(folder_move_body(item, folder_id)?)
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data)
    }

    pub fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, id);
        self.write_agent
            .delete(&url)
            .call()
            .map_err(map_http_err)?;
        Ok(())
    }

    /// The items in the vault's trash, and only those.
    ///
    /// Same path as [`Self::list_items`] with one query parameter, which is
    /// the entire difference and is measured, not guessed
    /// (`.superpowers/sdd/item-shapes-capture.md`, verified against the user's
    /// live `bw serve` 2026.7.0):
    ///
    /// | query | items returned | carrying `deletedDate` |
    /// |---|---|---|
    /// | none | 1654 | 0 |
    /// | `trash=true` | 14 | 14 |
    /// | `deleted=true` | 1654 | 0 -- **silently ignored** |
    /// | `includeDeleted=true` | 1654 | 0 -- **silently ignored** |
    ///
    /// Two things follow, and both are why this is not a filter over
    /// [`Self::list_items`]. `trash=true` returns a DISJOINT set, not a
    /// superset, so there is nothing in the default list to filter. And the
    /// two plausible-looking spellings do not fail -- they answer 200 with the
    /// whole live vault -- so a typo here does not surface as an error; it
    /// surfaces as a Trash view showing the user's entire vault.
    /// `list_trash_asks_for_only_the_trashed_items` therefore asserts the
    /// REQUEST's query string, not the parsed response.
    ///
    /// Deliberately NOT cached: see
    /// [`crate::vault_cache::VaultCache::list_trash_unless_superseded`].
    pub fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        let url = format!("{}/list/object/items", self.base_url);
        let body: Envelope<ItemList> = self
            .read_agent
            .get(&url)
            .query("trash", "true")
            .call()
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data.data)
    }

    /// The items in the vault's archive, and only those.
    ///
    /// Same shape as [`Self::list_trash`], and the same trap in a nastier
    /// form. VERIFIED against the live backend
    /// (`.superpowers/sdd/item-shapes-capture.md`): `?archived=true` returns
    /// only archived items, while **`?archive=true` -- the same word without
    /// the "d" -- is silently ignored and answers 200 with the entire live
    /// vault.** A typo here does not fail; it fills the Archive row with all
    /// 1654 of the user's items. `list_archive_asks_for_only_the_archived_items`
    /// therefore asserts the REQUEST's query string, not the parsed response,
    /// because a test written against a mock that answers regardless of query
    /// passes for both spellings.
    ///
    /// Deliberately NOT cached, for
    /// [`crate::vault_cache::VaultCache::list_trash_unless_superseded`]'s reasons
    /// exactly.
    pub fn list_archive(&self) -> Result<Vec<VaultItem>, VaultError> {
        let url = format!("{}/list/object/items", self.base_url);
        let body: Envelope<ItemList> = self
            .read_agent
            .get(&url)
            .query("archived", "true")
            .call()
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data.data)
    }

    /// Puts an item into the archive: `POST /archive/item/{id}`.
    ///
    /// **A 200 here does not prove the state changed.** Measured: an item
    /// archived immediately after being created answered 200, stayed in the
    /// default list, and never appeared under `?archived=true`; the same
    /// sequence with a ~1.5s settle worked. Nothing in this crate may
    /// therefore read a list back to "confirm" an archive -- a read that
    /// races the settle reports a failure that did not happen, which is worse
    /// than not checking. The window instead moves the item between its own
    /// lists and lets the next ordinary refresh reconcile; see
    /// [`crate::vault_cache::VaultCache::archive_item`].
    pub fn archive_item(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/archive/item/{}", self.base_url, id);
        self.write_agent.post(&url).call().map_err(map_http_err)?;
        Ok(())
    }

    /// Takes an item back OUT of the archive -- and the route is
    /// `POST /restore/item/{id}`, the same one [`Self::restore_item`] uses.
    ///
    /// **There is no "unarchive" endpoint.** `POST /unarchive/item/{id}` is
    /// 404 and `DELETE /archive/item/{id}` is 405, and an earlier pass
    /// concluded from those two that archiving was a one-way door. It is not:
    /// the CLI wires unarchiving into its *restore* command
    /// (`restore.command.ts` calls `unarchiveWithServer`), so `bw serve`
    /// exposes one `POST /restore/:object/:id` that both un-trashes and
    /// un-archives, selected by the item's current state. VERIFIED against
    /// the live backend with a control that asserted the item really was
    /// archived first -- without that control the test passes while proving
    /// nothing, which is how it went wrong the first time.
    ///
    /// A separate function from `restore_item` rather than a call site
    /// reusing it, even though the request is byte-identical: the two are
    /// different operations to the user and to the caller, and a shared name
    /// would make "restore" mean two things at every call site instead of at
    /// this one. `unarchiving_and_untrashing_hit_the_same_route` pins that
    /// they stay identical.
    pub fn unarchive_item(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/restore/item/{}", self.base_url, id);
        self.write_agent.post(&url).call().map_err(map_http_err)?;
        Ok(())
    }

    /// Takes a trashed item out of the trash: `POST /restore/item/{id}`.
    ///
    /// A different path shape from every other item call in this file
    /// (`/restore/item/{id}`, not `/object/item/{id}`), which is why the test
    /// asserts the path and the method rather than only the outcome.
    /// VERIFIED against the live backend: the item returns to the default
    /// `/list/object/items` afterwards.
    ///
    /// Callers should reach this through
    /// [`crate::vault_cache::VaultCache`], not here, so the live snapshot
    /// gains the item the server just un-deleted.
    pub fn restore_item(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/restore/item/{}", self.base_url, id);
        self.write_agent
            .post(&url)
            .call()
            .map_err(map_http_err)?;
        Ok(())
    }

    /// Deletes a trashed item for good: `DELETE /object/item/{id}?permanent=true`.
    ///
    /// **The query parameter is the whole operation.** The same path without
    /// it is [`Self::delete_item`], which SOFT-deletes -- so dropping
    /// `permanent=true` does not fail, it silently re-trashes an
    /// already-trashed item and the user's "delete forever" does nothing
    /// whatever while reporting success. `purging_an_item_states_permanent_true`
    /// asserts the query is on the wire for exactly that reason, and
    /// `an_ordinary_delete_is_still_a_soft_delete_and_states_no_query`
    /// guards the mirror-image mistake.
    ///
    /// VERIFIED against the live backend: the item leaves the trash list.
    pub fn purge_item(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, id);
        self.write_agent
            .delete(&url)
            .query("permanent", "true")
            .call()
            .map_err(map_http_err)?;
        Ok(())
    }

    /// `None` when the item has no TOTP secret configured -- `bw serve`
    /// answers that with `400 Bad Request` rather than a null payload (see
    /// the `get_totp_returns_none_when_the_item_has_no_totp` test), so *that
    /// specific* status is treated as "no code" rather than propagated as
    /// `VaultError`. A *parse* failure on an actual 2xx response still is
    /// one: that would mean `bw serve` changed shape under us, worth
    /// surfacing.
    ///
    /// A `401` is the other non-2xx handled specially: every other call site
    /// routes a `401` through `map_http_err` to `VaultError::Unauthorized`
    /// so a stale/invalidated session (`bw lock` elsewhere, a server-side
    /// timeout, a password change on another device) triggers re-
    /// authentication. This used to be the one call site that skipped that --
    /// a blanket `Err(ureq::Error::Status(_, _)) => Ok(None)` swallowed a
    /// `401` the exact same way it swallowed a genuine "no TOTP configured"
    /// `400`, so a stale session read as "this item has no TOTP secret":
    /// codes went silently blank with no re-auth prompt until some unrelated
    /// write happened to hit the same `401` on a call site that *did* check.
    ///
    /// Every *other* status (500, 503, a proxy timeout, ...) is a genuine
    /// failure, not "no code" (review Minor 3, independent review of
    /// a7b33cb): the old blanket mapping to `Ok(None)` made a 500 from a
    /// struggling `bw serve` indistinguishable, at the poll site, from an
    /// item that was never TOTP-enabled -- the row silently vanished, no
    /// line was logged, and a failure-streak flag elsewhere reset as if the
    /// poll had actually succeeded, so a backend flapping between refused
    /// and 5xx could log a false "recovered".
    pub fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        let url = format!("{}/object/totp/{}", self.base_url, id);
        match self.read_agent.get(&url).call() {
            Ok(response) => {
                let body: Envelope<TotpData> = response
                    .into_json()
                    .map_err(|e| VaultError::Parse(e.to_string()))?;
                Ok(body.data.data)
            }
            Err(ureq::Error::Status(401, _)) => Err(VaultError::Unauthorized),
            Err(ureq::Error::Status(400, _)) => Ok(None),
            Err(ureq::Error::Status(status, _)) => {
                Err(VaultError::Http(format!("bw serve returned {status} fetching a TOTP code")))
            }
            Err(e) => Err(VaultError::Http(e.to_string())),
        }
    }

    /// Asks `bw serve` for a fresh password or passphrase: `GET /generate`.
    ///
    /// Returns `Zeroizing<String>` rather than a `String` for the reason
    /// [`LoginData::password`] is one: what comes back IS a password from the
    /// moment it exists, so it wipes itself on drop wherever a caller took a
    /// copy. That guarantee is partial and this crate says so rather than
    /// implying more -- `into_json`'s own buffer holds the plaintext for the
    /// length of the call and is not wiped, exactly as it is not for every
    /// other secret this file reads.
    ///
    /// Every non-2xx goes through [`map_http_err`], so a `401` from a locked
    /// vault reaches the re-auth path as [`VaultError::Unauthorized`] instead
    /// of a generic failure beside a password box. Unlike [`Self::get_totp`]
    /// there is **no special-cased status**: this route has no equivalent of
    /// "this item has no TOTP secret", so every failure really is one.
    pub fn generate(&self, request: &GenerateRequest) -> Result<Zeroizing<String>, VaultError> {
        let url = format!("{}/generate", self.base_url);
        let mut call = self.read_agent.get(&url);
        for (key, value) in request.query() {
            call = call.query(key, &value);
        }
        let body: Envelope<GeneratedString> = call
            .call()
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(Zeroizing::new(body.data.data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `revisionDate` `echoing_item_put` reports back, distinct from any
    /// value a fixture starts with so "the app kept what it sent" and "the app
    /// took what the server answered" cannot look alike.
    const NEXT_REVISION: &str = "2026-08-03T02:33:03.427Z";
    use crate::app_match::TriggerMode;

    /// A minimal item with no type, for tests that only care about one field.
    fn a_bare_item() -> VaultItem {
        VaultItem {
            id: "1".into(),
            name: "x".into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn item_kind_covers_every_type_number() {
        let kind = |t: Option<i64>| {
            let mut item = a_bare_item();
            item.item_type = t;
            ItemKind::of(&item)
        };
        assert_eq!(kind(Some(1)), ItemKind::Login);
        assert_eq!(kind(Some(2)), ItemKind::SecureNote);
        assert_eq!(kind(Some(3)), ItemKind::Card);
        assert_eq!(kind(Some(4)), ItemKind::Identity);
        assert_eq!(kind(Some(5)), ItemKind::SshKey);
        // A type Bitwarden has not shipped yet must be representable as
        // itself, not collapsed into a login -- otherwise a future item
        // renders a login-shaped pane over data that is not a login.
        assert_eq!(kind(Some(6)), ItemKind::Unknown(6));
        // An absent type preserves today's behaviour.
        assert_eq!(kind(None), ItemKind::Login);
    }

    #[test]
    fn every_kind_has_its_own_label() {
        // A label shared by two kinds would make the read pane's subtitle
        // lie about one of them.
        let labels = [
            ItemKind::Login,
            ItemKind::SecureNote,
            ItemKind::Card,
            ItemKind::Identity,
            ItemKind::SshKey,
            ItemKind::Unknown(6),
        ]
        .map(ItemKind::label);
        for (i, label) in labels.iter().enumerate() {
            assert!(!label.is_empty());
            for other in &labels[i + 1..] {
                assert_ne!(label, other, "two kinds share the label {label:?}");
            }
        }
    }

    #[test]
    fn a_card_round_trips_with_absent_fields_still_absent() {
        // The property that has broken twice in this file already: a key the
        // server never sent must not appear on write.
        let raw = r#"{"id":"1","name":"Visa","type":3,"favorite":false,"fields":[],
            "card":{"number":"4111111111111111","brand":"Visa"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert_eq!(item.card.as_ref().unwrap().brand.as_deref(), Some("Visa"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        let after = serde_json::to_value(&item).unwrap();
        assert_eq!(before, after, "a card round trip changed the item's shape");
    }

    #[test]
    fn a_card_round_trips_with_empty_strings_still_empty() {
        // Empty is not absent. Collapsing the two is the mirror of the bug
        // above and just as silent.
        let raw = r#"{"id":"1","name":"Visa","type":3,"favorite":false,"fields":[],
            "card":{"number":"","brand":"","code":""}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        let after = serde_json::to_value(&item).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn unknown_keys_inside_a_card_survive_a_round_trip() {
        // VaultItem's own flatten cannot reach inside a nested object --
        // this is why UriEntry exists. Same rule, two more structs.
        let raw = r#"{"id":"1","name":"Visa","type":3,"favorite":false,"fields":[],
            "card":{"number":"4111","somethingNew":{"deep":true}}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        let after = serde_json::to_value(&item).unwrap();
        assert_eq!(before, after, "an unmodelled key inside `card` was dropped");
    }

    #[test]
    fn a_cards_expiry_stays_a_string_including_its_zero_padding() {
        // Bitwarden's own `item.card` template sends `expMonth: "04"` -- a
        // zero-padded *string*. Modelling either half as a number would turn
        // "04" into 4 on write and "" into a parse error.
        let raw = r#"{"id":"1","name":"Visa","type":3,"favorite":false,"fields":[],
            "card":{"cardholderName":"John Doe","brand":"visa",
                    "number":"4242424242424242","expMonth":"04","expYear":"2023",
                    "code":"123"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let card = item.card.as_ref().unwrap();
        assert_eq!(card.exp_month.as_deref(), Some("04"));
        assert_eq!(card.exp_year.as_deref(), Some("2023"));
        assert_eq!(card.number.as_deref().map(|n| n.as_str()), Some("4242424242424242"));
        assert_eq!(card.code.as_deref().map(|c| c.as_str()), Some("123"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn every_key_of_bitwardens_card_template_is_modelled() {
        // The six keys `GET /object/template/item.card` returns, transcribed
        // from `.superpowers/sdd/item-shapes-capture.md` -- deliberately NOT
        // from `CardData`'s own `rename` attributes, which would reproduce any
        // typo in the struct and defeat the entire point of this test.
        //
        // `other.is_empty()` is the whole mechanism. A misspelt rename (say
        // `cardHolderName` for `cardholderName`) deserializes to `None`, the
        // real key rides the catch-all, the item still round-trips
        // byte-identically, and the field is silently missing from the UI
        // forever. Nothing else in the suite would fail -- verified by doing
        // exactly that misspelling and watching this test, and only this test,
        // go red. The per-field assertions below then name each key, so
        // `cardholder_name` in particular -- asserted nowhere else -- cannot
        // rot unnoticed.
        let raw = r#"{"id":"1","name":"Visa","type":3,"favorite":false,"fields":[],
            "card":{"cardholderName":"John Doe","brand":"visa",
                    "number":"4242424242424242","expMonth":"04","expYear":"2023",
                    "code":"123"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let card = item.card.as_ref().unwrap();
        assert!(
            card.other.is_empty(),
            "a template key fell through to the catch-all: {:?}",
            card.other
        );
        assert_eq!(card.cardholder_name.as_deref(), Some("John Doe"));
        assert_eq!(card.brand.as_deref(), Some("visa"));
        assert_eq!(card.number.as_deref().map(|n| n.as_str()), Some("4242424242424242"));
        assert_eq!(card.exp_month.as_deref(), Some("04"));
        assert_eq!(card.exp_year.as_deref(), Some("2023"));
        assert_eq!(card.code.as_deref().map(|c| c.as_str()), Some("123"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn every_observed_login_key_is_either_modelled_or_rides_the_catch_all() {
        // The same teeth as the card/identity tests, for the one struct whose
        // key list the capture took from REAL items rather than a template:
        // `login` on the user's 1656-item vault carries exactly
        // fido2Credentials, password, passwordRevisionDate, totp, uris and
        // username.
        //
        // `LoginData` models four of those and leaves two on purpose, so a
        // bare `other.is_empty()` would be wrong here. Asserting the catch-all
        // holds EXACTLY the two unmodelled keys is the equivalent check: a
        // misspelt rename on any of the four would push a third key into
        // `other` and fail this, while still round-tripping byte-identically
        // everywhere else.
        let raw = r#"{"id":"1","name":"Site","type":1,"favorite":false,"fields":[],
            "login":{"username":"u","password":"p","totp":"seed",
                     "uris":[{"uri":"https://example.com"}],
                     "fido2Credentials":[],"passwordRevisionDate":null}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let login = item.login.as_ref().unwrap();
        assert_eq!(login.username.as_deref(), Some("u"));
        assert_eq!(login.password.as_deref().map(|p| p.as_str()), Some("p"));
        assert_eq!(login.totp.as_deref().map(|t| t.as_str()), Some("seed"));
        assert_eq!(login.uris.len(), 1);
        assert_eq!(login.uris[0].uri.as_deref(), Some("https://example.com"));
        let mut unmodelled: Vec<&str> = login.other.keys().map(|k| k.as_str()).collect();
        unmodelled.sort_unstable();
        assert_eq!(
            unmodelled,
            ["fido2Credentials", "passwordRevisionDate"],
            "a modelled login key fell through to the catch-all: {:?}",
            login.other
        );
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn an_identity_round_trips_including_unmodelled_keys() {
        let raw = r#"{"id":"1","name":"Me","type":4,"favorite":false,"fields":[],
            "identity":{"firstName":"A","lastName":"B","futureField":7}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert_eq!(item.identity.as_ref().unwrap().first_name.as_deref(), Some("A"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn every_key_of_bitwardens_identity_template_is_modelled() {
        // The seventeen keys `GET /object/template/item.identity` returns.
        // If one is missing from `IdentityData` it rides `other` instead, the
        // detail pane never shows it, and nothing fails loudly.
        let raw = r#"{"id":"1","name":"Me","type":4,"favorite":false,"fields":[],
            "identity":{"title":"Mr","firstName":"A","middleName":"B","lastName":"C",
                "address1":"1","address2":"2","city":"D","state":"E","postalCode":"F",
                "country":"G","company":"H","email":"I","phone":"J","ssn":"K",
                "username":"L","passportNumber":"M","licenseNumber":"N"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let identity = item.identity.as_ref().unwrap();
        assert!(
            identity.other.is_empty(),
            "a template key fell through to the catch-all: {:?}",
            identity.other
        );
        assert_eq!(identity.postal_code.as_deref(), Some("F"));
        assert_eq!(identity.license_number.as_deref(), Some("N"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn a_secure_note_round_trips_with_its_body_in_item_level_notes() {
        let raw = r#"{"id":"1","name":"Wifi","type":2,"favorite":false,"fields":[],
            "notes":"the passphrase","secureNote":{"type":0}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert_eq!(item.notes.as_deref().map(|n| n.as_str()), Some("the passphrase"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn notes_on_a_login_are_now_modelled_and_still_round_trip() {
        // Regression guard for the existing type: `notes` used to ride the
        // `other` catch-all, so moving it into a typed field must not change
        // any login's wire shape.
        let raw = r#"{"id":"1","name":"Site","type":1,"favorite":false,"fields":[],
            "notes":"a note","login":{"username":"u"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn an_item_with_no_notes_does_not_gain_a_null_notes_key() {
        let raw = r#"{"id":"1","name":"Site","type":1,"fields":[]}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let after = serde_json::to_value(&item).unwrap();
        assert!(after.get("notes").is_none(), "an absent notes key became null");
        assert!(after.get("card").is_none(), "an absent card key became null");
        assert!(after.get("identity").is_none(), "an absent identity key became null");
    }

    #[test]
    fn an_explicitly_null_notes_key_is_dropped_rather_than_echoed() {
        // Real items from `bw serve` carry `"notes": null` when they have no
        // note -- the capture shows `notes` present on every one of a
        // 1656-item vault. Modelling it as Option + skip_serializing_if means
        // that key now VANISHES on write instead of round-tripping as null.
        // Deliberate and consistent with how `folderId` and every optional
        // login field already behave (see
        // `an_explicitly_null_login_field_is_still_dropped_rather_than_echoed`);
        // pinned here because it silently changed the write shape of nearly
        // every item in a real vault.
        //
        // WHETHER THAT IS HARMLESS IS AN OPEN QUESTION, and this test does not
        // answer it: it depends on whether `bw serve`'s state-replacing PUT
        // reads an absent key as "no change" or as "clear", which needs an
        // experiment against a live backend. This pins the current behaviour
        // so that answer can be acted on deliberately rather than discovered.
        let raw = r#"{"id":"1","name":"Site","type":1,"fields":[],"notes":null}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert!(item.notes.is_none());
        let after = serde_json::to_value(&item).unwrap();
        assert!(after.get("notes").is_none(), "an explicitly null notes key was echoed back");
    }

    #[test]
    fn an_ssh_key_round_trips_including_unmodelled_keys() {
        // Was `an_ssh_key_object_rides_the_catch_all_untouched`, back when
        // `type: 5` was the one shape the capture could not verify and the
        // whole `sshKey` object rode `VaultItem::other`. The shape is now
        // captured (see `.superpowers/sdd/item-shapes-capture.md`) and
        // modelled, so the object travels through `SshKeyData` -- and the
        // guarantee the assertions state is unchanged and is now the harder
        // one: a modelled struct has to reassemble byte-identically, which
        // the catch-all got for free. `x` is the key nothing models, and it
        // is what proves `SshKeyData` carries its own flatten.
        let raw = r#"{"id":"1","name":"Deploy key","type":5,"favorite":false,"fields":[],
            "sshKey":{"privateKey":"PRIV","publicKey":"PUB","keyFingerprint":"FP","x":1}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert_eq!(ItemKind::of(&item), ItemKind::SshKey);
        let ssh = item.ssh_key.as_ref().unwrap();
        assert_eq!(ssh.private_key.as_deref().map(|k| k.as_str()), Some("PRIV"));
        assert_eq!(ssh.public_key.as_deref(), Some("PUB"));
        assert_eq!(ssh.key_fingerprint.as_deref(), Some("FP"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap(), "an sshKey object was altered");
    }

    #[test]
    fn an_ssh_key_round_trips_with_absent_fields_still_absent() {
        // The property that has broken twice in this file already: a key the
        // server never sent must not appear on write. A `"publicKey": null`
        // arriving at `bw serve`'s full-state PUT says the public key is
        // gone.
        let raw = r#"{"id":"1","name":"Deploy key","type":5,"favorite":false,"fields":[],
            "sshKey":{"privateKey":"PRIV"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let ssh = item.ssh_key.as_ref().unwrap();
        assert!(ssh.public_key.is_none());
        assert!(ssh.key_fingerprint.is_none());
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap(), "an ssh key round trip changed the item's shape");
    }

    #[test]
    fn an_ssh_key_round_trips_with_empty_strings_still_empty() {
        // Empty is not absent. Collapsing the two is the mirror of the bug
        // above and just as silent.
        let raw = r#"{"id":"1","name":"Deploy key","type":5,"favorite":false,"fields":[],
            "sshKey":{"privateKey":"","publicKey":"","keyFingerprint":""}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn unknown_keys_inside_an_ssh_key_survive_a_round_trip() {
        // `VaultItem`'s own flatten cannot reach inside a nested object --
        // this is why `UriEntry` exists, and this repo has shipped that bug
        // three times.
        let raw = r#"{"id":"1","name":"Deploy key","type":5,"favorite":false,"fields":[],
            "sshKey":{"privateKey":"PRIV","somethingNew":{"deep":true}}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap(), "an unmodelled key inside `sshKey` was dropped");
    }

    /// **The only test in this file that catches a misspelled `rename`.**
    ///
    /// A round-trip test structurally cannot: the catch-all absorbs the
    /// difference, so a field renamed `"publickey"` deserializes to `None`,
    /// the real `publicKey` rides `other`, and the item is written back
    /// looking correct while the pane shows nothing. This was demonstrated
    /// live earlier in this project -- misspelling `cardholderName` failed
    /// exactly one test out of 330, the identity/card equivalent of this one.
    ///
    /// The fixture is built from the CAPTURE FILE
    /// (`.superpowers/sdd/item-shapes-capture.md`, "SSH key (type 5) --
    /// VERIFIED 2026-08-01"), not from `SshKeyData`, so it is an independent
    /// statement of the wire shape rather than a restatement of the struct.
    #[test]
    fn every_key_of_the_captured_ssh_shape_is_modelled() {
        // The three keys the create response returned, in the capture's own
        // alphabetical order.
        let raw = r#"{"id":"1","name":"SSH test key (deskwarden)","type":5,"favorite":false,"fields":[],
            "sshKey":{"keyFingerprint":"SHA256:AAAA","privateKey":"-----BEGIN OPENSSH PRIVATE KEY-----",
                "publicKey":"ssh-ed25519 AAAAC3Nz deskwarden-ssh-test"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let ssh = item.ssh_key.as_ref().unwrap();
        assert!(
            ssh.other.is_empty(),
            "a captured key fell through to the catch-all, so it is misspelled or unmodelled \
             in SshKeyData and the pane will never show it: {:?}",
            ssh.other
        );
        assert_eq!(ssh.key_fingerprint.as_deref(), Some("SHA256:AAAA"));
        assert_eq!(
            ssh.private_key.as_deref().map(|k| k.as_str()),
            Some("-----BEGIN OPENSSH PRIVATE KEY-----")
        );
        assert_eq!(
            ssh.public_key.as_deref(),
            Some("ssh-ed25519 AAAAC3Nz deskwarden-ssh-test")
        );
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn an_item_with_no_ssh_key_does_not_gain_a_null_ssh_key_key() {
        // `skip_serializing_if` on the field itself: every login in the
        // user's 1656-item vault goes through this path, and a
        // `"sshKey": null` on each of them is what the two shipped
        // instances of this bug looked like.
        let raw = r#"{"id":"1","name":"Site","type":1,"favorite":false,"fields":[]}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert!(item.ssh_key.is_none());
        let after = serde_json::to_value(&item).unwrap();
        assert!(after.get("sshKey").is_none(), "an absent sshKey key became null");
    }

    #[test]
    fn a_present_but_null_key_survives_a_round_trip() {
        // Real items from `bw serve` carry `login.passwordRevisionDate: null`
        // -- a key that is PRESENT with a null value. It rides LoginData's
        // catch-all, and a careless skip_serializing_if change would drop it
        // silently.
        let raw = r#"{"id":"1","name":"Site","type":1,"favorite":false,"fields":[],
            "login":{"username":"u","passwordRevisionDate":null}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn a_real_shaped_item_round_trips_with_every_observed_key() {
        // Every item-level key observed on the user's real vault. All but
        // the modelled ones must ride `other` untouched.
        let raw = r#"{"id":"1","object":"item","type":1,"name":"Site",
            "notes":"a note","favorite":false,"fields":[],"folderId":null,
            "collectionIds":[],"attachments":[],"key":"K","reprompt":0,
            "passwordHistory":[{"password":"old","lastUsedDate":"2020-01-01T00:00:00.000Z"}],
            "creationDate":"2020-01-01T00:00:00.000Z",
            "revisionDate":"2021-01-01T00:00:00.000Z",
            "login":{"username":"u","password":"p","totp":"seed","uris":[],
                     "fido2Credentials":[],"passwordRevisionDate":null}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        let after = serde_json::to_value(&item).unwrap();

        // A real item round-trips through exactly TWO normalisations, both of
        // which predate this task, are already pinned by their own tests, and
        // are named here rather than papered over by a looser assertion:
        //
        //   * `folderId: null` is dropped, because null and absent mean the
        //     same thing to `bw` -- the rule
        //     `an_explicitly_null_login_field_is_still_dropped_rather_than_echoed`
        //     records for `login`, applied by `folder_id`'s own
        //     `skip_serializing_if`;
        //   * an empty `login.uris` is dropped, by `LoginData::uris`'s
        //     `skip_serializing_if = "Vec::is_empty"`.
        //
        // Removing them from `before` and demanding exact equality afterwards
        // is a stronger check than skipping those keys would be: it asserts
        // that these two, and nothing else, differ. Every unmodelled
        // item-level key the capture found -- attachments, collectionIds,
        // creationDate, key, object, passwordHistory, reprompt, revisionDate
        // -- must survive byte-identically.
        let mut expected = before;
        let root = expected.as_object_mut().unwrap();
        assert_eq!(root.remove("folderId"), Some(serde_json::Value::Null));
        assert_eq!(
            root["login"].as_object_mut().unwrap().remove("uris"),
            Some(serde_json::json!([]))
        );

        assert_eq!(expected, after, "a real-shaped item changed shape across a round trip");
    }

    #[test]
    fn extract_app_match_finds_matching_field() {
        let item = VaultItem {
            id: "1".into(),
            name: "Rockstar".into(),
            fields: vec![VaultField {
                name: Some(APP_MATCH_FIELD_NAME_FOR_TEST.into()),
                value: Some(r#"{"process":"RockstarGamesLauncher.exe","trigger":"prompt"}"#.into()),
                other: serde_json::Map::new(),
            }],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };
        let m = extract_app_match(&item).unwrap();
        assert_eq!(m.process, "RockstarGamesLauncher.exe");
        assert_eq!(m.trigger, TriggerMode::Prompt);
    }

    #[test]
    fn extract_app_match_returns_none_without_field() {
        let item = VaultItem {
            id: "1".into(),
            name: "Other".into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };
        assert!(extract_app_match(&item).is_none());
    }

    #[test]
    fn extract_app_match_returns_none_on_malformed_value() {
        let item = VaultItem {
            id: "1".into(),
            name: "Broken".into(),
            fields: vec![VaultField {
                name: Some(APP_MATCH_FIELD_NAME_FOR_TEST.into()),
                value: Some("not json".into()),
                other: serde_json::Map::new(),
            }],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };
        assert!(extract_app_match(&item).is_none());
        // The pair of answers the detail pane's card needs: the field IS
        // there, it just will not parse. `extract_app_match` alone cannot
        // tell this from the item above, which carries no such field at all.
        assert!(
            has_app_match_field(&item),
            "a field that fails to parse is reported as no field at all"
        );
    }

    /// [`has_app_match_field`] answers on the NAME, so it must say yes for
    /// every value -- including ones nothing can parse -- and no when the
    /// field is genuinely absent.
    #[test]
    fn has_app_match_field_reports_presence_and_not_parseability() {
        let bare = VaultItem {
            id: "1".into(),
            name: "Bare".into(),
            fields: vec![VaultField {
                name: Some("Some other field".into()),
                value: Some("x".into()),
                other: serde_json::Map::new(),
            }],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };
        assert!(!has_app_match_field(&bare), "a field with another name counted");

        for value in [
            // A real saved match.
            r#"{"process":"a.exe","trigger":"prompt"}"#,
            // Malformed JSON.
            "{not json",
            // Well-formed JSON, unknown `trigger` -- the hand-edit that is
            // easiest to make in another client's UI.
            r#"{"process":"a.exe","trigger":"telepathy"}"#,
            // Emptied by hand.
            "",
        ] {
            let mut item = bare.clone();
            item.fields.push(VaultField {
                name: Some(APP_MATCH_FIELD_NAME_FOR_TEST.into()),
                value: Some(value.to_string()),
                other: serde_json::Map::new(),
            });
            assert!(
                has_app_match_field(&item),
                "the field is present but unreported for value {value:?}"
            );
            // And `without_app_match` really clears each of them -- the
            // card's Remove is only an offer if this holds.
            assert!(
                !has_app_match_field(&without_app_match(&item)),
                "Remove would leave the field behind for value {value:?}"
            );
        }
        // The control: at least one of those values genuinely does NOT parse,
        // so the loop is not four copies of the happy case.
        let mut broken = bare.clone();
        broken.fields.push(VaultField {
            name: Some(APP_MATCH_FIELD_NAME_FOR_TEST.into()),
            value: Some("{not json".into()),
            other: serde_json::Map::new(),
        });
        assert!(has_app_match_field(&broken) && extract_app_match(&broken).is_none());
    }

    #[test]
    fn with_app_match_preserves_unknown_fields() {
        let json = r#"{
            "id": "1",
            "name": "Rockstar",
            "type": 1,
            "fields": [],
            "login": {"username": "a", "password": "b"},
            "notes": "secret",
            "folderId": null,
            "favorite": true
        }"#;
        let item: VaultItem = serde_json::from_str(json).unwrap();

        let m = AppMatch::for_process("RockstarGamesLauncher.exe", TriggerMode::Prompt);
        let updated = with_app_match(&item, &m);

        let value = serde_json::to_value(&updated).unwrap();
        assert_eq!(value["type"], serde_json::json!(1));
        assert_eq!(
            value["login"],
            serde_json::json!({"username": "a", "password": "b"})
        );
        assert_eq!(value["notes"], serde_json::json!("secret"));
        assert_eq!(value["favorite"], serde_json::json!(true));

        let m_back = extract_app_match(&updated).unwrap();
        assert_eq!(m_back.process, "RockstarGamesLauncher.exe");
        assert_eq!(m_back.trigger, TriggerMode::Prompt);
    }

    /// The inverse of the test above, and the whole of what the detail pane's
    /// Remove does.
    #[test]
    fn without_app_match_removes_the_field_and_keeps_every_other_one() {
        let raw = r#"{
            "id": "1",
            "name": "Rockstar",
            "type": 1,
            "favorite": true,
            "notes": "secret",
            "fields": [
                {"name": "PIN", "value": "1234", "type": 1},
                {"name": "Recovery", "value": "abc", "type": 0}
            ],
            "login": {"username": "a", "password": "b"}
        }"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let bound = with_app_match(
            &item,
            &AppMatch::for_process("RockstarGamesLauncher.exe", TriggerMode::Prompt),
        );
        // The premise. Without it a `without_app_match` that did nothing at
        // all would pass everything below.
        assert!(extract_app_match(&bound).is_some(), "nothing was bound to begin with");

        let cleared = without_app_match(&bound);
        assert!(
            extract_app_match(&cleared).is_none(),
            "the item is still bound to an app"
        );
        // REMOVED, not blanked: a field left in place with an empty value is
        // still a `deskwarden:app-match` row in every other Bitwarden client.
        assert!(
            !cleared
                .fields
                .iter()
                .any(|f| f.name.as_deref() == Some(APP_MATCH_FIELD_NAME_FOR_TEST)),
            "the field is still there, emptied rather than removed: {:?}",
            cleared.fields
        );
        // The user's own custom fields, in their own order -- the PUT is
        // state-replacing, so anything dropped here is dropped from the vault.
        let names: Vec<&str> = cleared.fields.iter().filter_map(|f| f.name.as_deref()).collect();
        assert_eq!(names, vec!["PIN", "Recovery"]);
        assert_eq!(cleared.fields[0].value.as_deref(), Some("1234"));
        // ...and the `type` key riding `VaultField::other`, which no model
        // here names.
        assert_eq!(
            cleared.fields[0].other.get("type"),
            Some(&serde_json::json!(1)),
            "an unmodelled key on a surviving field was dropped"
        );

        let value = serde_json::to_value(&cleared).unwrap();
        assert_eq!(value["favorite"], serde_json::json!(true));
        assert_eq!(value["notes"], serde_json::json!("secret"));
        assert_eq!(
            value["login"],
            serde_json::json!({"username": "a", "password": "b"})
        );
    }

    /// A Remove that races a Remove, and every item that never had a match:
    /// nothing to do, and nothing broken by doing it.
    #[test]
    fn without_app_match_on_an_unbound_item_changes_nothing() {
        let item: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"Site","type":1,"favorite":false,
                "fields":[{"name":"PIN","value":"1234","type":1}]}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(without_app_match(&item)).unwrap(),
            serde_json::to_value(&item).unwrap()
        );
    }

    #[test]
    fn unknown_keys_on_a_custom_field_survive_a_round_trip() {
        // `VaultField` is serialized on every state-replacing PUT, and a real
        // Bitwarden custom field carries more than name/value: `type`
        // (0=text, 1=hidden, 2=boolean, 3=linked) and `linkedId`. Without
        // `VaultField::other` those keys are dropped on deserialize and
        // therefore absent on the next write. Same rule as `UriEntry`:
        // `VaultItem`'s flatten cannot reach into a struct nested inside a
        // `Vec`.
        let raw = r#"{"id":"1","name":"Site","type":1,"favorite":false,
            "fields":[{"name":"PIN","value":"1234","type":1,"linkedId":null},
                      {"name":"Which user","value":null,"type":3,"linkedId":100}]}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            before,
            serde_json::to_value(&item).unwrap(),
            "a key on a custom field was dropped across a round trip"
        );
    }

    #[test]
    fn a_custom_field_arriving_without_a_value_gains_a_null_value_on_write() {
        // PINS A KNOWN DEFECT, it does not bless it. `VaultField`'s two
        // modelled fields have no `skip_serializing_if`, so a field that
        // arrives as `{"name":"PIN","type":1}` is written back as
        // `{"name":"PIN","value":null,"type":1}` -- the very thing
        // `LoginData`'s doc comment forbids.
        //
        // It is NOT fixed by adding the attribute: `Option<String>` cannot
        // tell an absent key from an explicit `null`, and a linked custom
        // field genuinely arrives carrying `"value": null` (see
        // `unknown_keys_on_a_custom_field_survive_a_round_trip`), which the
        // attribute would then drop. Trading an unobserved shape for an
        // observed one is not a fix. The real fix is
        // `Option<Option<String>>`, which changes the type every caller
        // reads. This test exists so that change has something to flip.
        let raw = r#"{"id":"1","name":"Site","type":1,"favorite":false,
            "fields":[{"name":"PIN","type":1}]}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let value = serde_json::to_value(&item).unwrap();
        assert_eq!(
            value["fields"][0],
            serde_json::json!({"name": "PIN", "value": null, "type": 1}),
            "the injected-null behaviour changed; if it was fixed, flip this test"
        );
    }

    #[test]
    fn with_app_match_replaces_the_existing_field_in_place() {
        // Bitwarden preserves and displays custom-field order. Filtering the
        // old app-match field out and appending the new one reshuffles the
        // user's fields in the Bitwarden UI on every save; replacing in place
        // does not.
        let raw = format!(
            r#"{{"id":"1","name":"Site","type":1,"favorite":false,
                "fields":[{{"name":"First","value":"a","type":0}},
                          {{"name":"{}","value":"old.exe|prompt","type":0}},
                          {{"name":"Last","value":"z","type":0}}]}}"#,
            APP_MATCH_FIELD_NAME_FOR_TEST
        );
        let item: VaultItem = serde_json::from_str(&raw).unwrap();

        let updated = with_app_match(
            &item,
            &AppMatch::for_process("new.exe", TriggerMode::Prompt),
        );
        let names: Vec<&str> = updated
            .fields
            .iter()
            .filter_map(|f| f.name.as_deref())
            .collect();
        assert_eq!(
            names,
            vec!["First", APP_MATCH_FIELD_NAME_FOR_TEST, "Last"],
            "saving an app match reordered the user's custom fields"
        );
        assert_eq!(extract_app_match(&updated).unwrap().process, "new.exe");
    }

    #[test]
    fn with_app_match_keeps_a_hidden_field_hidden() {
        // The user-visible harm, stated as a test: attaching an app match to
        // an item that already has a HIDDEN custom field (`"type": 1`)
        // rewrote every one of its fields without their type, so the hidden
        // field came back as an ordinary visible text field. Silent, on a
        // real vault.
        let raw = r#"{"id":"1","name":"Site","type":1,"favorite":false,
            "fields":[{"name":"Recovery code","value":"s3cret","type":1,"linkedId":null}]}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();

        let updated = with_app_match(
            &item,
            &AppMatch::for_process("game.exe", TriggerMode::Prompt),
        );
        let value = serde_json::to_value(&updated).unwrap();
        let fields = value["fields"].as_array().unwrap();

        let hidden = fields
            .iter()
            .find(|f| f["name"] == serde_json::json!("Recovery code"))
            .expect("the pre-existing custom field vanished");
        assert_eq!(
            hidden,
            &serde_json::json!({
                "name": "Recovery code", "value": "s3cret", "type": 1, "linkedId": null
            }),
            "a hidden custom field lost its type when an app match was attached"
        );
    }

    #[test]
    fn with_app_match_keeps_the_extra_keys_of_the_field_it_replaces() {
        // The app-match field is the one field `with_app_match` rebuilds
        // rather than copies. `bw` normalises what this app writes -- a field
        // created here with no `type` comes back carrying one -- so rebuilding
        // it from name/value alone would drop those keys on the *next* write.
        // Replacing it must therefore carry over whatever else it arrived
        // with.
        let raw = format!(
            r#"{{"id":"1","name":"Site","type":1,"favorite":false,
                "fields":[{{"name":"{}","value":"old.exe|prompt","type":0,"linkedId":null}}]}}"#,
            APP_MATCH_FIELD_NAME_FOR_TEST
        );
        let item: VaultItem = serde_json::from_str(&raw).unwrap();

        let updated = with_app_match(
            &item,
            &AppMatch::for_process("new.exe", TriggerMode::Prompt),
        );
        let value = serde_json::to_value(&updated).unwrap();
        let fields = value["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 1, "the app-match field was duplicated");
        assert_eq!(fields[0]["type"], serde_json::json!(0));
        assert_eq!(fields[0]["linkedId"], serde_json::Value::Null);
        assert_eq!(extract_app_match(&updated).unwrap().process, "new.exe");
    }

    #[test]
    fn an_app_match_field_added_to_a_fresh_item_carries_no_extra_keys() {
        // The other half of the rule above: when there is nothing to replace,
        // the new field must be exactly name+value. Inventing a `type` we
        // never observed would be modelling from memory.
        let item = a_bare_item();
        let updated = with_app_match(
            &item,
            &AppMatch::for_process("game.exe", TriggerMode::Prompt),
        );
        let value = serde_json::to_value(&updated).unwrap();
        let fields = value["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(
            fields[0].as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["name", "value"],
            "a freshly added app-match field gained keys nobody observed"
        );
    }

    #[test]
    fn list_items_parses_bw_serve_envelope() {
        let mut server = mockito::Server::new();
        let body = r#"{"success":true,"data":{"data":[
            {"id":"1","name":"Rockstar","fields":[]},
            {"id":"2","name":"Mabl","fields":[]}
        ]}}"#;
        let _m = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create();

        let bridge = VaultBridge::new(server.url());
        let items = bridge.list_items().unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "1");
        assert_eq!(items[1].name, "Mabl");
    }

    /// Pins [`READ_DEADLINE`] as a UI-thread freeze budget derived from
    /// measured call cost, and specifically *not* re-derived from
    /// `bw_serve`'s constants -- which is how it drifted to 30s and tripled
    /// the worst-case freeze on a fresh connection.
    ///
    /// That the deadline is applied *at all*, and that it survives connection
    /// reuse, is pinned separately and by timing in
    /// `http_agent::a_total_bounded_agent_bounds_a_pooled_connection_that_never_answers`.
    /// That `VaultBridge` cannot hold an agent built any other way is now the
    /// type's job: both fields are `http_agent::TotalBounded`, whose only
    /// constructor is `bounded_total`. The previous version of this comment
    /// claimed that property for a bare `ureq::Agent` field, and a reviewer
    /// disproved it by replacing the constructor call with a bare, unbounded
    /// ureq agent -- which compiled, and left all 747 tests green.
    #[test]
    fn the_ui_thread_read_deadline_is_a_single_call_budget_not_a_backend_lifecycle_one() {
        // The slowest read this bridge makes, measured: /list/object/items
        // over 1654 items, 1.1s cold / 1.2s warm.
        const SLOWEST_MEASURED_CALL: Duration = Duration::from_millis(1_200);

        assert!(
            READ_DEADLINE >= SLOWEST_MEASURED_CALL * 5,
            "not enough headroom over the slowest real call"
        );
        // Both of `bw_serve`'s numbers describe backend *lifecycle* waits --
        // a retry schedule over a Node cold start, and a start-or-sync
        // operation -- neither of which runs through this agent. Borrowing
        // either as this bound is what made the UI freeze longer than it had
        // to; the assertion is that this stays strictly the smaller quantity.
        assert!(READ_DEADLINE < crate::bw_serve::READINESS_DEADLINE);
        assert!(READ_DEADLINE < crate::bw_serve::BACKEND_OP_TIMEOUT);
    }

    /// The read budget is measured over loopback; the write budget is not, and
    /// this fails if the two are ever collapsed back into one number.
    ///
    /// `bw serve`'s POST/PUT/DELETE routes push to `vault.bitwarden.com`
    /// before answering, so a write is bounded by the user's internet link
    /// while a read is bounded by a local Node process. The 1.2s
    /// `/list/object/items` measurement that justifies [`READ_DEADLINE`] is no
    /// evidence at all about a write, and a single shared constant is exactly
    /// how that gets forgotten.
    #[test]
    fn a_write_gets_a_longer_budget_than_a_loopback_read() {
        assert!(
            WRITE_DEADLINE > READ_DEADLINE,
            "a write traverses the internet and a read does not; giving them the same budget \
             means the write budget was set by loopback evidence that does not apply to it"
        );
        // Not `> READ_DEADLINE` alone: 10s and 11s would satisfy that while
        // being, in substance, the one averaged number this split exists to
        // undo. The gap has to be big enough to represent a different
        // situation.
        assert!(
            WRITE_DEADLINE >= READ_DEADLINE * 2,
            "the gap is too small to be a different situation rather than a nudge"
        );
        // And still bounded: a write is on the UI thread too, so this is a
        // frozen-window budget however justified it is.
        assert!(
            WRITE_DEADLINE <= Duration::from_secs(60),
            "a save must not be able to freeze the window for a minute"
        );
    }

    /// The hole finding 1 found, closed for this file: a constants test proves
    /// nothing about which agent the production methods actually reach for.
    ///
    /// So this reads the source of every method above the test module and
    /// checks the pairing structurally, by HTTP verb rather than by a list of
    /// method names -- a method that mutates (`post`/`put`/`delete`) must use
    /// `write_agent`, a method that reads (`get`) must use `read_agent`. A
    /// tenth write added later, wired to the read agent out of habit, fails
    /// here without anyone having to remember to extend a list.
    #[test]
    fn every_mutating_route_uses_the_write_agent_and_every_read_the_read_one() {
        // SPLIT ACROSS `concat!` ARGUMENTS, DELIBERATELY: `include_str!` pulls
        // this test module in as well, so a needle written as one literal
        // would match its own declaration here. Splitting the production text
        // rather than the field name is what keeps that from happening; the
        // `impl_source` slice below is the belt to this test's braces.
        const READ_FIELD: &str = concat!("read_", "agent");
        const WRITE_FIELD: &str = concat!("write_", "agent");
        // `(&url)` is part of every needle on purpose: `.get(` alone also
        // matches `HashMap::get` and `serde_json::Value::get`, which several
        // free functions in this file use and which have nothing to do with
        // HTTP. Every route on this type builds a `url` local first, so this
        // is the shape that means "issues a request". A future route that
        // names the local something else escapes the needle -- which is what
        // the two count assertions at the bottom are for.
        const MUTATING_VERBS: [&str; 3] = [".post(&url)", ".put(&url)", ".delete(&url)"];

        let source = include_str!("vault_bridge.rs");
        // Everything before the test module -- production code only, so a
        // mockito `.post(` in a test fixture cannot be mistaken for a route.
        // Split on the module header rather than on `#[cfg(test)]`, which also
        // sits on a test-facing helper *inside* the production half and would
        // cut the impl block in two (found by this test failing with 0 routes,
        // not reasoned about in advance).
        let impl_source = source
            .split(BELOW_CUT_MARKER)
            .next()
            .expect("split always yields at least one part");
        assert!(
            impl_source.len() < source.len(),
            "the test-module marker was not found, so this guard is scanning its own fixtures"
        );

        // One chunk per method: `pub fn` is how every route on this type is
        // declared, and each chunk runs to the start of the next one.
        let mut checked_writes = 0;
        let mut checked_reads = 0;
        for chunk in impl_source.split("pub fn ").skip(1) {
            let name = chunk.split('(').next().unwrap_or("<unnamed>").trim();
            let mutates = MUTATING_VERBS.iter().any(|verb| chunk.contains(verb));
            if mutates {
                assert!(
                    chunk.contains(WRITE_FIELD),
                    "`{name}` issues a mutating request but does not use `{WRITE_FIELD}`. \
                     That route pushes to vault.bitwarden.com synchronously, so bounding it \
                     with the loopback read budget hard-fails saves on a slow link"
                );
                checked_writes += 1;
            } else if chunk.contains(".get(&url)") {
                assert!(
                    chunk.contains(READ_FIELD),
                    "`{name}` issues a read but does not use `{READ_FIELD}`"
                );
                checked_reads += 1;
            }
        }

        // Positive control: without these, a rename that made every chunk
        // match neither branch would leave this test passing vacuously.
        // 11 = the 9 this test was written against, plus `archive_item` and
        // `unarchive_item`; 7 = the 6 plus `list_archive`. Raised deliberately
        // when the Archive sidebar row merged, having checked each of the
        // three against the rule above rather than to make the test go green:
        // the two POSTs mutate and take the write agent, the archived-items
        // query is a read and takes the read one.
        assert_eq!(
            checked_writes, 11,
            "expected 11 mutating routes on VaultBridge, found {checked_writes} -- if a route \
             was genuinely added or removed, update this number deliberately"
        );
        assert_eq!(
            checked_reads, 7,
            "expected 7 read routes on VaultBridge, found {checked_reads}"
        );
    }

    #[test]
    fn get_item_parses_a_single_item_envelope() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/object/item/abc")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"success":true,"data":{"id":"abc","name":"Rockstar","fields":[],
                    "login":{"username":"u","password":"p","totp":null}}}"#,
            )
            .create();

        let bridge = VaultBridge::new(server.url());
        let item = bridge.get_item("abc").unwrap();

        assert_eq!(item.id, "abc");
        let login = item.login.unwrap();
        assert_eq!(login.username.as_deref(), Some("u"));
        assert_eq!(login.password.as_deref().map(|p| p.as_str()), Some("p"));
    }

    #[test]
    fn get_item_reports_http_failure() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/object/item/missing")
            .with_status(404)
            .create();

        let bridge = VaultBridge::new(server.url());
        assert!(bridge.get_item("missing").is_err());
    }

    #[test]
    fn a_401_response_maps_to_vault_error_unauthorized() {
        // Regression test: a stale/invalidated session leaves `bw serve`
        // alive but 401-ing, and callers need to be able to tell that apart
        // from an ordinary transport/status failure in order to trigger
        // re-authentication rather than just retrying. Checked against
        // `list_items` (the call `VaultCache::populate` makes) as a
        // representative read and `update_item` as a representative write --
        // every other call site routes through the same `map_http_err`.
        let mut server = mockito::Server::new();
        let _items = server.mock("GET", "/list/object/items").with_status(401).create();
        let _update = server.mock("PUT", "/object/item/1").with_status(401).create();

        let bridge = VaultBridge::new(server.url());

        assert!(matches!(bridge.list_items(), Err(VaultError::Unauthorized)));

        let item = VaultItem {
            id: "1".into(),
            name: "A".into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };
        assert!(matches!(bridge.update_item(&item), Err(VaultError::Unauthorized)));
    }

    #[test]
    fn a_non_401_status_stays_a_plain_http_error() {
        // Only 401 means "re-authenticate"; every other status (a 500, a
        // 404, ...) must keep surfacing as the catch-all `Http` variant so
        // callers don't mistake an unrelated server error for a stale
        // session.
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/list/object/items").with_status(500).create();

        let bridge = VaultBridge::new(server.url());
        assert!(matches!(bridge.list_items(), Err(VaultError::Http(_))));
    }

    #[test]
    fn items_without_a_login_object_do_not_gain_a_null_login_on_write() {
        // `bw serve`'s edit endpoint takes the payload as the item's new
        // state, so emitting `"login": null` for a secure note would be a
        // destructive round-trip.
        let item: VaultItem =
            serde_json::from_str(r#"{"id":"1","name":"Note","fields":[],"notes":"n"}"#).unwrap();
        let m = AppMatch::for_process("a.exe", TriggerMode::Auto);
        let value = serde_json::to_value(with_app_match(&item, &m)).unwrap();
        assert!(value.get("login").is_none(), "got: {value}");
    }

    #[test]
    fn login_object_extras_survive_a_round_trip() {
        let item: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"A","fields":[],
                "login":{"username":"u","password":"p","totp":"seed","uris":[{"uri":"x"}]}}"#,
        )
        .unwrap();
        let m = AppMatch::for_process("a.exe", TriggerMode::Auto);
        let value = serde_json::to_value(with_app_match(&item, &m)).unwrap();
        assert_eq!(value["login"]["totp"], serde_json::json!("seed"));
        assert_eq!(value["login"]["uris"], serde_json::json!([{"uri":"x"}]));
        assert_eq!(value["login"]["username"], serde_json::json!("u"));
    }

    #[test]
    fn uri_entry_extras_survive_a_round_trip() {
        // `bw serve`'s login URI entries carry a `match` key (the per-URI
        // match-detection strategy) alongside `uri`. Without its own
        // `#[serde(flatten)] other` field, `UriEntry` would silently drop
        // `match` on every write -- including the app-match-saving path
        // exercised here via `with_app_match`.
        let item: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"A","fields":[],
                "login":{"username":"u","uris":[{"uri":"https://x.com","match":2}]}}"#,
        )
        .unwrap();
        let m = AppMatch::for_process("a.exe", TriggerMode::Auto);
        let value = serde_json::to_value(with_app_match(&item, &m)).unwrap();
        assert_eq!(
            value["login"]["uris"],
            serde_json::json!([{"uri": "https://x.com", "match": 2}])
        );
    }

    #[test]
    fn a_partial_login_object_does_not_gain_the_keys_it_lacked() {
        // Asymmetric serialisation would turn `{"password":"p"}` into
        // `{"username":null,"password":"p"}`, inventing a key the source item
        // never had -- and `bw serve`'s edit endpoint treats the payload as
        // the new state, so the invented key sticks.
        let item: VaultItem =
            serde_json::from_str(r#"{"id":"1","name":"A","fields":[],"login":{"password":"p"}}"#)
                .unwrap();
        let m = AppMatch::for_process("a.exe", TriggerMode::Auto);
        let value = serde_json::to_value(with_app_match(&item, &m)).unwrap();

        assert!(
            value["login"].get("username").is_none(),
            "login gained a username key: {value}"
        );
        assert_eq!(value["login"]["password"], serde_json::json!("p"));
    }

    #[test]
    fn an_empty_login_object_stays_empty_through_a_round_trip() {
        let item: VaultItem =
            serde_json::from_str(r#"{"id":"1","name":"A","fields":[],"login":{}}"#).unwrap();
        let m = AppMatch::for_process("a.exe", TriggerMode::Auto);
        let value = serde_json::to_value(with_app_match(&item, &m)).unwrap();
        assert_eq!(value["login"], serde_json::json!({}), "got: {value}");
    }

    #[test]
    fn an_explicitly_null_login_field_is_still_dropped_rather_than_echoed() {
        // `null` and "absent" are the same thing to `bw`, and dropping is the
        // consistent choice: it matches what `VaultItem::login` already does.
        let item: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"A","fields":[],"login":{"username":null,"password":"p"}}"#,
        )
        .unwrap();
        let m = AppMatch::for_process("a.exe", TriggerMode::Auto);
        let value = serde_json::to_value(with_app_match(&item, &m)).unwrap();
        assert!(value["login"].get("username").is_none(), "got: {value}");
    }

    const APP_MATCH_FIELD_NAME_FOR_TEST: &str = crate::app_match::APP_MATCH_FIELD_NAME;

    #[test]
    fn typed_fields_round_trip_through_real_bw_shapes() {
        let json = r#"{
            "id": "1", "name": "Ledgerline", "type": 1, "favorite": true,
            "folderId": "f1", "fields": [],
            "login": {"username": "a", "password": "b", "totp": "SEED123",
                       "uris": [{"uri": "https://app.ledgerline.com"}]}
        }"#;
        let item: VaultItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.item_type, Some(1));
        assert_eq!(item.folder_id.as_deref(), Some("f1"));
        assert!(item.favorite);
        let login = item.login.unwrap();
        assert_eq!(login.totp.as_deref().map(|t| t.as_str()), Some("SEED123"));
        assert_eq!(login.uris[0].uri.as_deref(), Some("https://app.ledgerline.com"));
    }

    #[test]
    fn typed_fields_default_sanely_when_absent() {
        let item: VaultItem = serde_json::from_str(r#"{"id":"1","name":"A","fields":[]}"#).unwrap();
        assert_eq!(item.item_type, None);
        assert_eq!(item.folder_id, None);
        assert!(!item.favorite);
    }

    #[test]
    fn typed_fields_do_not_break_existing_app_match_round_trip() {
        // with_app_match must still preserve type/folderId/favorite exactly
        // as extract_app_match's existing tests already check for `other` --
        // this locks the same guarantee for the newly-typed fields.
        let item: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"A","type":3,"favorite":true,"folderId":"f9","fields":[]}"#,
        )
        .unwrap();
        let m = crate::app_match::AppMatch::for_process("a.exe", crate::app_match::TriggerMode::Auto);
        let value = serde_json::to_value(with_app_match(&item, &m)).unwrap();
        assert_eq!(value["type"], serde_json::json!(3));
        assert_eq!(value["favorite"], serde_json::json!(true));
        assert_eq!(value["folderId"], serde_json::json!("f9"));
    }

    #[test]
    fn list_folders_parses_bw_serve_envelope() {
        let mut server = mockito::Server::new();
        let body = r#"{"success":true,"data":{"data":[
            {"id":"f1","name":"Engineering"},
            {"id":"f2","name":"Personal"}
        ]}}"#;
        let _m = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create();

        let bridge = VaultBridge::new(server.url());
        let folders = bridge.list_folders().unwrap();

        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].name, "Engineering");
    }

    #[test]
    fn unknown_keys_on_a_folder_survive_a_round_trip() {
        // `Folder` is the fourth struct in this crate to need a catch-all.
        // `bw` sends more on a folder than id/name (`organizationId`,
        // `revisionDate`, ...), and the encrypted-disk-cache plan serializes
        // `Vec<Folder>` to disk, which makes the loss real rather than
        // theoretical.
        let raw = r#"{"id":"f1","name":"Engineering",
            "organizationId":null,"revisionDate":"2026-01-02T03:04:05.000Z"}"#;
        let folder: Folder = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            before,
            serde_json::to_value(&folder).unwrap(),
            "a key on a folder was dropped across a round trip"
        );
    }

    #[test]
    fn create_folder_posts_and_parses_the_new_folder() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/object/folder")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"id":"f3","name":"Shared"}}"#)
            .create();

        let bridge = VaultBridge::new(server.url());
        let folder = bridge.create_folder("Shared").unwrap();
        assert_eq!(folder.id, "f3");
    }

    #[test]
    fn update_folder_puts_the_new_name_and_parses_the_result() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("PUT", "/object/folder/f3")
            .match_body(mockito::Matcher::Json(serde_json::json!({ "name": "Renamed" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"id":"f3","name":"Renamed"}}"#)
            .create();

        let bridge = VaultBridge::new(server.url());
        let folder = bridge.update_folder("f3", "Renamed").unwrap();
        assert_eq!(folder.id, "f3");
        assert_eq!(folder.name, "Renamed");
    }

    #[test]
    fn delete_folder_calls_the_delete_endpoint() {
        let mut server = mockito::Server::new();
        let _m = server.mock("DELETE", "/object/folder/f3").with_status(200).create();
        let bridge = VaultBridge::new(server.url());
        assert!(bridge.delete_folder("f3").is_ok());
    }

    /// The five type keys `bw serve` recognises. A create payload must carry
    /// EXACTLY ONE of them; the tests below assert on the four that must be
    /// absent, not merely on the one that must be present, because an item
    /// with two type objects is what the enum shape exists to prevent.
    const TYPE_KEYS: [&str; 5] = ["login", "secureNote", "card", "identity", "sshKey"];

    /// A filled card, so a payload test can tell "pruned because blank" from
    /// "never modelled".
    fn a_filled_card() -> CardData {
        CardData {
            cardholder_name: Some("A Holder".into()),
            brand: Some("Visa".into()),
            number: Some(Zeroizing::new("4111111111111111".into())),
            exp_month: Some("04".into()),
            exp_year: Some("2031".into()),
            code: Some(Zeroizing::new("123".into())),
            other: serde_json::Map::new(),
        }
    }

    fn a_filled_identity() -> IdentityData {
        IdentityData {
            first_name: Some("Ada".into()),
            last_name: Some("Lovelace".into()),
            email: Some("ada@example.com".into()),
            ..IdentityData::default()
        }
    }

    /// Every kind, each built with real values, for the shared structural
    /// assertions. Blank-field behaviour is pinned separately below.
    fn one_of_each_kind() -> Vec<(NewItem, i64, &'static str)> {
        vec![
            (NewItem::login("n", "u", "p", None), 1, "login"),
            (NewItem::secure_note("n", "body", None), 2, "secureNote"),
            (NewItem::card("n", a_filled_card(), None), 3, "card"),
            (NewItem::identity("n", a_filled_identity(), None), 4, "identity"),
            (NewItem::ssh_key("n", "PRIV", "PUB", "FP", None), 5, "sshKey"),
        ]
    }

    #[test]
    fn each_create_payload_carries_exactly_one_type_object() {
        for (new_item, expected_type, expected_key) in one_of_each_kind() {
            let payload = new_item.to_payload();
            let map = payload.as_object().expect("a create payload is a JSON object");
            assert_eq!(map.get("type"), Some(&serde_json::json!(expected_type)));
            assert!(map.get(expected_key).is_some(), "{expected_key} missing");
            for other in TYPE_KEYS {
                if other != expected_key {
                    // `.get`, not `payload[other]`: indexing a Value with a
                    // MISSING key yields Value::Null, so `== Null` passes for
                    // an absent key and proves nothing.
                    assert!(
                        map.get(other).is_none(),
                        "a {expected_key} payload also carried {other}"
                    );
                }
            }
        }
    }

    #[test]
    fn each_create_payload_states_folder_id_explicitly() {
        // Deliberately different from the UPDATE path, which must never send
        // a null folderId (`bw serve` ignores it -- see
        // `.superpowers/sdd/put-semantics-capture.md`). On a CREATE there is
        // no previous value to preserve, so an explicit null is unambiguous
        // and this is the shape already shipped and accepted.
        for (new_item, _, _) in one_of_each_kind() {
            let payload = new_item.to_payload();
            let map = payload.as_object().unwrap();
            assert_eq!(
                map.get("folderId"),
                Some(&serde_json::Value::Null),
                "an unfiled create payload dropped its explicit null folderId"
            );
        }
        let filed = NewItem::card("n", a_filled_card(), Some("f1".to_string())).to_payload();
        assert_eq!(filed["folderId"], serde_json::json!("f1"));
    }

    #[test]
    fn a_secure_note_posts_its_body_as_item_level_notes() {
        // The body is item-level `notes`; `secureNote` is only a `{"type":0}`
        // discriminator (verified, `.superpowers/sdd/item-shapes-capture.md`).
        let payload = NewItem::secure_note("Wifi", "the passphrase", None).to_payload();
        assert_eq!(payload["notes"], serde_json::json!("the passphrase"));
        assert_eq!(payload["secureNote"], serde_json::json!({ "type": 0 }));
    }

    #[test]
    fn a_blank_field_is_absent_rather_than_an_empty_string_for_every_kind() {
        // The convention `create_item` has always followed, now pinned per
        // kind: blank means ABSENT. The edit path maps blank to an absent
        // key, so a create that sent `""` would make the item silently change
        // shape between its first and second save.
        // A secure note is the one kind whose type object is NOT empty when
        // everything is blank: `{"type": 0}` is the discriminator that makes
        // it a secure note at all, and 0 is a real value, not a blank. Its
        // body is item-level `notes`, asserted below with the others.
        let blank_of_each = [
            (NewItem::login("n", "", "", None), "login", serde_json::json!({})),
            (
                NewItem::secure_note("n", "", None),
                "secureNote",
                serde_json::json!({ "type": 0 }),
            ),
            (
                NewItem::card(
                    "n",
                    CardData {
                        cardholder_name: Some(String::new()),
                        brand: Some(String::new()),
                        number: Some(Zeroizing::new(String::new())),
                        exp_month: Some(String::new()),
                        exp_year: Some(String::new()),
                        code: Some(Zeroizing::new(String::new())),
                        other: serde_json::Map::new(),
                    },
                    None,
                ),
                "card",
                serde_json::json!({}),
            ),
            (
                NewItem::identity(
                    "n",
                    IdentityData {
                        first_name: Some(String::new()),
                        last_name: Some(String::new()),
                        email: Some(String::new()),
                        ..IdentityData::default()
                    },
                    None,
                ),
                "identity",
                serde_json::json!({}),
            ),
            (NewItem::ssh_key("n", "", "", "", None), "sshKey", serde_json::json!({})),
        ];
        for (new_item, key, expected_object) in blank_of_each {
            let payload = new_item.to_payload();
            assert_eq!(
                payload[key], expected_object,
                "a blank {key} payload sent empty strings instead of omitting keys"
            );
            // And the secure note's body, which is item-level rather than
            // inside the type object.
            assert!(
                payload.as_object().unwrap().get("notes").is_none(),
                "a blank body became an empty `notes` string on a {key} payload"
            );
        }
    }

    #[test]
    fn a_partly_filled_payload_keeps_what_was_filled_in() {
        // The mirror of the test above: pruning blanks must not prune values.
        let payload = NewItem::ssh_key("n", "PRIV", "", "FP", None).to_payload();
        assert_eq!(
            payload["sshKey"],
            serde_json::json!({ "privateKey": "PRIV", "keyFingerprint": "FP" })
        );
    }

    /// Answers a create with a minimal item so the tests below can assert on
    /// the REQUEST body -- which is the whole point -- rather than on what the
    /// mock was told to return.
    const CREATED_BODY: &str = r#"{"success":true,"data":{"id":"9","name":"New","fields":[]}}"#;

    #[test]
    fn create_item_posts_a_login_shaped_payload() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/object/item")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "name": "New",
                "type": 1,
                "folderId": null,
                "login": { "username": "u", "password": "p" },
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"id":"9","name":"New","type":1,"fields":[],
                "login":{"username":"u","password":"p"}}}"#)
            .create();

        let bridge = VaultBridge::new(server.url());
        let created = bridge.create_item(&NewItem::login("New", "u", "p", None)).unwrap();
        assert_eq!(created.id, "9");
        assert_eq!(created.login.unwrap().username.as_deref(), Some("u"));
    }

    #[test]
    fn create_item_omits_blank_username_and_password_instead_of_sending_empty_strings() {
        // Matches `EditDraft::apply_to`'s "blank means absent" convention:
        // the mock only matches a request body whose `login` object has no
        // `username`/`password` keys at all (not `""`), so if `create_item`
        // regresses to sending them as empty strings this test fails with a
        // 501 from the unmatched mock rather than silently passing.
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/object/item")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "name": "New",
                "type": 1,
                "folderId": null,
                "login": {},
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"id":"9","name":"New","type":1,"fields":[]}}"#)
            .create();

        let bridge = VaultBridge::new(server.url());
        let created = bridge.create_item(&NewItem::login("New", "", "", None)).unwrap();
        assert_eq!(created.id, "9");
    }

    #[test]
    fn create_item_posts_a_secure_note_shaped_payload() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/object/item")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "name": "Wifi",
                "type": 2,
                "folderId": null,
                "notes": "the passphrase",
                "secureNote": { "type": 0 },
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(CREATED_BODY)
            .create();

        let bridge = VaultBridge::new(server.url());
        let new_item = NewItem::secure_note("Wifi", "the passphrase", None);
        assert_eq!(bridge.create_item(&new_item).unwrap().id, "9");
    }

    #[test]
    fn create_item_posts_a_card_shaped_payload() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/object/item")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "name": "Visa",
                "type": 3,
                "folderId": "f1",
                "card": {
                    "cardholderName": "A Holder",
                    "brand": "Visa",
                    "number": "4111111111111111",
                    "expMonth": "04",
                    "expYear": "2031",
                    "code": "123",
                },
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(CREATED_BODY)
            .create();

        let bridge = VaultBridge::new(server.url());
        let new_item = NewItem::card("Visa", a_filled_card(), Some("f1".to_string()));
        assert_eq!(bridge.create_item(&new_item).unwrap().id, "9");
    }

    #[test]
    fn create_item_posts_an_identity_shaped_payload() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/object/item")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "name": "Me",
                "type": 4,
                "folderId": null,
                "identity": {
                    "firstName": "Ada",
                    "lastName": "Lovelace",
                    "email": "ada@example.com",
                },
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(CREATED_BODY)
            .create();

        let bridge = VaultBridge::new(server.url());
        let new_item = NewItem::identity("Me", a_filled_identity(), None);
        assert_eq!(bridge.create_item(&new_item).unwrap().id, "9");
    }

    #[test]
    fn create_item_posts_an_ssh_key_shaped_payload() {
        // The one kind whose shape came from a live POST rather than a
        // template endpoint: `{"type":5,"name":...,"sshKey":{privateKey,
        // publicKey, keyFingerprint}}` returned 200 (see
        // `.superpowers/sdd/item-shapes-capture.md`).
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/object/item")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "name": "deploy",
                "type": 5,
                "folderId": null,
                "sshKey": {
                    "privateKey": "PRIV",
                    "publicKey": "PUB",
                    "keyFingerprint": "FP",
                },
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(CREATED_BODY)
            .create();

        let bridge = VaultBridge::new(server.url());
        let new_item = NewItem::ssh_key("deploy", "PRIV", "PUB", "FP", None);
        assert_eq!(bridge.create_item(&new_item).unwrap().id, "9");
    }

    #[test]
    fn update_item_puts_the_full_item_state() {
        let mut server = mockito::Server::new();
        let _m = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION).create();
        let bridge = VaultBridge::new(server.url());
        let item: VaultItem = serde_json::from_str(r#"{"id":"1","name":"A","fields":[]}"#).unwrap();
        assert!(bridge.update_item(&item).is_ok());
    }

    /// The item every move test below starts from, unfiled unless the test
    /// files it. Parsed from JSON rather than built with a struct literal so
    /// the expected bodies further down can be read against a real wire shape.
    fn an_item_in_folder(folder: Option<&str>) -> VaultItem {
        let raw = match folder {
            Some(f) => format!(r#"{{"id":"1","name":"A","fields":[],"folderId":"{f}"}}"#),
            None => r#"{"id":"1","name":"A","fields":[]}"#.to_string(),
        };
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn moving_an_item_into_a_folder_puts_that_folders_id() {
        // `Matcher::Json` compares parsed `serde_json::Value`s, so this is an
        // assertion on the ACTUAL request body and not on the returned value:
        // a body that differs in any key makes mockito answer 501, which
        // `unwrap` turns into a failure, and `assert` then reports the miss.
        let mut server = mockito::Server::new();
        let m = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION)
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "1",
                "name": "A",
                "fields": [],
                "favorite": false,
                "folderId": "f1",
            })))
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        bridge.move_item_to_folder(&an_item_in_folder(None), Some("f1")).unwrap();
        m.assert();
    }

    #[test]
    fn unfiling_an_item_puts_an_explicit_null_folder_id_rather_than_omitting_the_key() {
        // THE TRAP THIS PATH EXISTS FOR. `bw serve` MERGES omitted keys (see
        // `.superpowers/sdd/put-semantics-capture.md`), so a body without a
        // `folderId` key leaves the item in `f1` while the app believes it was
        // un-filed. `serde_json::Value` equality distinguishes an absent key
        // from a null one -- `{"a":1}` != `{"a":1,"folderId":null}` -- so this
        // matcher rejects the omitting body rather than accepting it as
        // equivalent. `the_unfile_body_carries_a_folder_id_key_that_is_present_and_null`
        // asserts the same property structurally, with a message that says
        // which of the two failed.
        let mut server = mockito::Server::new();
        let m = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION)
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "1",
                "name": "A",
                "fields": [],
                "favorite": false,
                "folderId": null,
            })))
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        bridge.move_item_to_folder(&an_item_in_folder(Some("f1")), None).unwrap();
        m.assert();
    }

    #[test]
    fn the_unfile_body_carries_a_folder_id_key_that_is_present_and_null() {
        // The structural half of the test above, kept separate because the
        // two failures a reader needs to tell apart -- "the key is absent"
        // and "the key is present with the wrong value" -- are one 501 from
        // mockito and two different messages here.
        let body = folder_move_body(&an_item_in_folder(Some("f1")), None).unwrap();
        let map = body.as_object().expect("an item body is a JSON object");
        assert!(
            map.contains_key("folderId"),
            "the un-file body has NO folderId key at all: {body}. `bw serve` merges omitted \
             keys, so this body leaves the item in its old folder and the un-file silently \
             does nothing"
        );
        assert_eq!(
            map["folderId"],
            serde_json::Value::Null,
            "the un-file body states a folderId that is not null: {body}"
        );
    }

    #[test]
    fn an_already_unfiled_item_still_states_folder_id_null_when_moved_to_no_folder() {
        // DELIBERATE, and the one case where the move body carries a key an
        // ordinary write of the same item would not have. `folder_move_body`'s
        // doc records why: the key is inserted unconditionally so there is no
        // branch to get wrong, `create_item` already POSTs `"folderId": null`
        // for a folderless item so the value is known-good on this backend,
        // and the alternative (skip the write when the local `folder_id`
        // already matches) trusts a possibly-stale local copy to decide
        // whether a real move happens.
        //
        // `get`, NOT `body["folderId"]`: indexing a `serde_json::Value` with a
        // missing key yields `Value::Null`, so the obvious spelling
        // `assert_eq!(body["folderId"], Value::Null)` passes just as happily
        // when the key is ABSENT -- which is the one thing this whole path
        // exists to rule out. That spelling was written here first and caught
        // by a bite check; it is recorded so nobody reintroduces it.
        let body = folder_move_body(&an_item_in_folder(None), None).unwrap();
        assert_eq!(
            body.as_object().expect("an item body is a JSON object").get("folderId"),
            Some(&serde_json::Value::Null),
            "an already-unfiled item's move body does not state folderId as present-and-null: \
             {body}"
        );
        assert_eq!(
            serde_json::to_value(&an_item_in_folder(None)).unwrap().get("folderId"),
            None,
            "the premise: an ordinary write of this same item omits the key entirely"
        );
    }

    #[test]
    fn a_move_states_the_folder_and_changes_nothing_else_about_a_real_shaped_item() {
        // The same real item as
        // `a_real_shaped_item_round_trips_with_every_observed_key`, so a move
        // is held to that test's standard: every unmodelled key observed on
        // the user's vault -- attachments, collectionIds, creationDate, key,
        // object, passwordHistory, reprompt, revisionDate -- must survive the
        // move byte-identically.
        let raw = r#"{"id":"1","object":"item","type":1,"name":"Site",
            "notes":"a note","favorite":false,"fields":[],"folderId":null,
            "collectionIds":[],"attachments":[],"key":"K","reprompt":0,
            "passwordHistory":[{"password":"old","lastUsedDate":"2020-01-01T00:00:00.000Z"}],
            "creationDate":"2020-01-01T00:00:00.000Z",
            "revisionDate":"2021-01-01T00:00:00.000Z",
            "login":{"username":"u","password":"p","totp":"seed","uris":[],
                     "fido2Credentials":[],"passwordRevisionDate":null}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let body = folder_move_body(&item, Some("f9")).unwrap();

        // Exactly TWO differences from the item as it arrived, both named
        // rather than papered over by a looser assertion: the folder the move
        // is setting, and `LoginData::uris`'s pre-existing empty-vec
        // omission. Demanding equality after applying only those two is a
        // stronger check than skipping the keys would be.
        let mut expected: serde_json::Value = serde_json::from_str(raw).unwrap();
        let root = expected.as_object_mut().unwrap();
        root.insert("folderId".to_string(), serde_json::json!("f9"));
        assert_eq!(
            root["login"].as_object_mut().unwrap().remove("uris"),
            Some(serde_json::json!([]))
        );
        assert_eq!(expected, body, "a move altered something other than the item's folder");
    }

    #[test]
    fn an_ordinary_update_of_an_unfiled_item_still_omits_folder_id_entirely() {
        // THE GUARD ON THE CHOICE, not a test of the move path. The move is a
        // dedicated path precisely so no other write's bytes change; removing
        // `skip_serializing_if` from `VaultItem::folder_id` instead would put
        // `"folderId": null` on EVERY item PUT this app makes. This test fails
        // if anyone does that, and so does
        // `a_real_shaped_item_round_trips_with_every_observed_key`.
        let mut server = mockito::Server::new();
        let m = echoing_item_put(&mut server, "/object/item/1", NEXT_REVISION)
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "1",
                "name": "A",
                "fields": [],
                "favorite": false,
            })))
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        bridge.update_item(&an_item_in_folder(None)).unwrap();
        m.assert();
    }

    #[test]
    fn a_failed_move_is_reported_rather_than_swallowed() {
        // The vault window reverts the dragged row on `Err`, so a move that
        // the server rejected must not come back `Ok`.
        let mut server = mockito::Server::new();
        let _m = server.mock("PUT", "/object/item/1").with_status(500).create();
        let bridge = VaultBridge::new(server.url());
        assert!(bridge.move_item_to_folder(&an_item_in_folder(Some("f1")), None).is_err());
    }

    #[test]
    fn with_folder_changes_the_folder_and_leaves_the_rest_of_the_item_alone() {
        let raw = r#"{"id":"1","name":"A","type":1,"favorite":true,"fields":[],
            "folderId":"f1","reprompt":0,"login":{"username":"u"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();

        let moved = with_folder(&item, Some("f2"));
        assert_eq!(moved.folder_id.as_deref(), Some("f2"));
        assert_eq!(with_folder(&item, None).folder_id, None);

        // Everything but `folderId` is untouched, including the unmodelled
        // `reprompt` riding `other`.
        let mut before = serde_json::to_value(&item).unwrap();
        let mut after = serde_json::to_value(&moved).unwrap();
        before.as_object_mut().unwrap().remove("folderId");
        after.as_object_mut().unwrap().remove("folderId");
        assert_eq!(before, after);
    }

    #[test]
    fn delete_item_calls_the_delete_endpoint() {
        let mut server = mockito::Server::new();
        let _m = server.mock("DELETE", "/object/item/1").with_status(200).create();
        let bridge = VaultBridge::new(server.url());
        assert!(bridge.delete_item("1").is_ok());
    }

    #[test]
    fn get_totp_returns_the_current_code() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/object/totp/1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":"482913"}}"#)
            .create();
        let bridge = VaultBridge::new(server.url());
        assert_eq!(bridge.get_totp("1").unwrap(), Some("482913".to_string()));
    }

    #[test]
    fn get_totp_returns_none_when_the_item_has_no_totp() {
        // bw serve answers a 400 for an item with no TOTP secret configured --
        // that's an expected "no code", not a real error.
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/object/totp/2").with_status(400).create();
        let bridge = VaultBridge::new(server.url());
        assert_eq!(bridge.get_totp("2").unwrap(), None);
    }

    #[test]
    fn get_totp_reports_unauthorized_on_a_401_instead_of_no_totp() {
        // Regression test (final review Minor): a stale session must surface
        // as `VaultError::Unauthorized` here too, not be folded into the same
        // "no TOTP configured" bucket as a genuine 400 -- otherwise a code
        // that should be re-authenticated for instead reads as an item that
        // simply has no TOTP secret, and goes silently blank.
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/object/totp/3").with_status(401).create();
        let bridge = VaultBridge::new(server.url());
        assert!(matches!(bridge.get_totp("3"), Err(VaultError::Unauthorized)));
    }

    #[test]
    fn get_totp_reports_an_error_on_a_500_instead_of_no_totp() {
        // Regression test for review Minor 3 (independent review of
        // a7b33cb): only a 400 means "no TOTP configured". A 500/503 from a
        // struggling bw serve must surface as a real error -- folding it
        // into `Ok(None)` made it indistinguishable, at the poll site, from
        // an item that was never TOTP-enabled: the row silently vanished,
        // nothing was logged, and a failure-streak flag elsewhere reset as
        // though the poll had succeeded.
        let mut server = mockito::Server::new();
        let _m = server.mock("GET", "/object/totp/4").with_status(500).create();
        let bridge = VaultBridge::new(server.url());
        assert!(matches!(bridge.get_totp("4"), Err(VaultError::Http(_))));
    }

    // --- Trash -------------------------------------------------------------

    /// One trashed item with EVERY key the live capture found on one
    /// (`.superpowers/sdd/item-shapes-capture.md`), transcribed from that
    /// document and not from `VaultItem` -- so this is an independent
    /// statement of the wire shape rather than a restatement of the struct.
    /// Note `folderId` and `notes` are absent, exactly as the capture records.
    fn a_trashed_item_body() -> &'static str {
        r#"{"success":true,"data":{"data":[
            {"id":"t1","object":"item","type":1,"name":"Old thing",
             "deletedDate":"2026-07-30T09:15:00.000Z",
             "creationDate":"2020-01-01T00:00:00.000Z",
             "revisionDate":"2021-01-01T00:00:00.000Z",
             "favorite":false,"fields":[],"collectionIds":[],"attachments":[],
             "key":"K","reprompt":0,"passwordHistory":[],
             "login":{"username":"u","password":"p"}}
        ]}}"#
    }

    #[test]
    fn list_trash_asks_for_only_the_trashed_items() {
        // THE ASSERTION IS ON THE REQUEST, and it has to be. `?deleted=true`
        // and `?includeDeleted=true` were both measured against the live
        // backend and both answer 200 with the ENTIRE LIVE VAULT -- they are
        // ignored, not rejected. So a wrong or missing parameter here cannot
        // be caught by looking at what comes back: the response parses fine
        // and the Trash view shows 1654 live items. `Matcher::Exact` on the
        // query string is what makes this bite: it rejects an absent query, a
        // renamed parameter and an extra one, each as a mockito 501 that turns
        // the `unwrap` below into a failure.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_trashed_item_body())
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        let trashed = bridge.list_trash().unwrap();
        m.assert();

        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].id, "t1");
        assert_eq!(deleted_date(&trashed[0]), Some("2026-07-30T09:15:00.000Z"));
    }

    #[test]
    fn the_live_item_list_still_states_no_query_at_all() {
        // The mirror of the test above, and not redundant with it: the two
        // calls share a path, so the failure this rules out is `list_items`
        // acquiring `trash=true` (every item in the app becoming a trashed
        // one) rather than `list_trash` losing it. `Matcher::Missing` matches
        // only a request with no query string.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[{"id":"1","name":"A","fields":[]}]}}"#)
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        assert_eq!(bridge.list_items().unwrap().len(), 1);
        m.assert();
    }

    #[test]
    fn list_archive_asks_for_only_the_archived_items() {
        // THE ASSERTION IS ON THE REQUEST, and the reason is worse here than
        // it is for the trash: `?archive=true` -- the same word without the
        // "d" -- was measured against the live backend and is SILENTLY
        // IGNORED, answering 200 with the entire live vault. So the wrong
        // spelling parses fine and fills the Archive row with every item the
        // user has. `Matcher::Exact` rejects an absent query, that spelling,
        // and an extra parameter, each as a mockito 501 that fails the
        // `unwrap` below.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("archived=true".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"success":true,"data":{"data":[
                    {"id":"a1","name":"Archived","object":"item","type":1,"fields":[],
                     "login":{"username":"u","password":"p"}}
                ]}}"#,
            )
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        let archived = bridge.list_archive().unwrap();
        m.assert();

        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, "a1");
    }

    #[test]
    fn archiving_an_item_posts_to_the_archive_endpoint() {
        // Another path shape of its own (`/archive/item/{id}`, not
        // `/object/item/{id}`), so mockito answering only that exact POST is
        // the test.
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/archive/item/a1").with_status(200).expect(1).create();

        let bridge = VaultBridge::new(server.url());
        bridge.archive_item("a1").unwrap();
        m.assert();
    }

    #[test]
    fn unarchiving_and_untrashing_hit_the_same_route() {
        // THE CORRECTED FACT. `POST /unarchive/item/{id}` is a 404 and
        // `DELETE /archive/item/{id}` a 405, and an earlier pass read those
        // as "archiving is a one-way door". The CLI wires unarchiving into
        // its *restore* command, so one `POST /restore/item/{id}` serves both
        // meanings, selected by the item's state.
        //
        // The mock deliberately allows ONLY that path: a `unarchive_item`
        // that guessed `/unarchive/item/a1` gets a 501 here and fails, which
        // is the regression this exists to catch. The two calls are then
        // asserted to be indistinguishable on the wire -- that is the claim
        // the doc makes, and it would otherwise be only a comment.
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/restore/item/a1").with_status(200).expect(2).create();

        let bridge = VaultBridge::new(server.url());
        bridge.unarchive_item("a1").unwrap();
        bridge.restore_item("a1").unwrap();
        m.assert();
    }

    #[test]
    fn a_401_on_any_archive_call_maps_to_unauthorized() {
        // Same rule as the trash calls: the window's re-auth path keys off
        // `Unauthorized`, so a locked vault must not reach the Archive row as
        // a generic failure.
        let mut server = mockito::Server::new();
        let _list = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("archived=true".into()))
            .with_status(401)
            .create();
        let _archive = server.mock("POST", "/archive/item/a1").with_status(401).create();
        let _unarchive = server.mock("POST", "/restore/item/a1").with_status(401).create();

        let bridge = VaultBridge::new(server.url());
        assert!(matches!(bridge.list_archive(), Err(VaultError::Unauthorized)));
        assert!(matches!(bridge.archive_item("a1"), Err(VaultError::Unauthorized)));
        assert!(matches!(bridge.unarchive_item("a1"), Err(VaultError::Unauthorized)));
    }

    #[test]
    fn a_rejected_archive_is_an_error_not_a_silent_success() {
        // Re-POSTing `/archive` on an already-archived item is a 400 on the
        // live backend. If that arrived as `Ok(())` the window would move the
        // item out of its live list on a write that never happened.
        let mut server = mockito::Server::new();
        let _archive = server.mock("POST", "/archive/item/a1").with_status(400).create();

        let bridge = VaultBridge::new(server.url());
        assert!(matches!(bridge.archive_item("a1"), Err(VaultError::Http(_))));
    }

    #[test]
    fn restore_item_posts_to_the_restore_endpoint() {
        // A different path shape from every other item call in this file, so
        // the path itself is the thing under test: mockito only answers a POST
        // to exactly `/restore/item/t1`, and anything else is a 501.
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/restore/item/t1")
            .with_status(200)
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        bridge.restore_item("t1").unwrap();
        m.assert();
    }

    #[test]
    fn purging_an_item_states_permanent_true() {
        // THE SILENT-FAILURE GUARD. `DELETE /object/item/{id}` without this
        // parameter is a SOFT delete -- the same call `delete_item` makes. Run
        // against an item that is already in the trash it succeeds, changes
        // nothing, and the user's "delete forever" silently does not happen.
        // Nothing about the response distinguishes the two, so the query
        // string is asserted directly.
        let mut server = mockito::Server::new();
        let m = server
            .mock("DELETE", "/object/item/t1")
            .match_query(mockito::Matcher::Exact("permanent=true".into()))
            .with_status(200)
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        bridge.purge_item("t1").unwrap();
        m.assert();
    }

    #[test]
    fn an_ordinary_delete_is_still_a_soft_delete_and_states_no_query() {
        // The mirror-image mistake, and the more destructive of the two:
        // `permanent=true` leaking onto the ordinary delete path would turn
        // every "Delete" in the app into an unrecoverable purge, with the item
        // never reaching the trash the user expects to find it in.
        let mut server = mockito::Server::new();
        let m = server
            .mock("DELETE", "/object/item/1")
            .match_query(mockito::Matcher::Missing)
            .with_status(200)
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        bridge.delete_item("1").unwrap();
        m.assert();
    }

    #[test]
    fn a_401_on_any_trash_call_maps_to_unauthorized() {
        // The vault window's re-auth path keys off `Unauthorized`, so a stale
        // session on any of these three must not arrive as a generic `Http`
        // failure -- a Trash view that reports "something went wrong" for a
        // locked vault is the `get_totp` defect (recorded above) in a new
        // place.
        let mut server = mockito::Server::new();
        let _list = server
            .mock("GET", "/list/object/items")
            .match_query(mockito::Matcher::Exact("trash=true".into()))
            .with_status(401)
            .create();
        let _restore = server.mock("POST", "/restore/item/t1").with_status(401).create();
        let _purge = server
            .mock("DELETE", "/object/item/t1")
            .match_query(mockito::Matcher::Exact("permanent=true".into()))
            .with_status(401)
            .create();

        let bridge = VaultBridge::new(server.url());
        assert!(matches!(bridge.list_trash(), Err(VaultError::Unauthorized)));
        assert!(matches!(bridge.restore_item("t1"), Err(VaultError::Unauthorized)));
        assert!(matches!(bridge.purge_item("t1"), Err(VaultError::Unauthorized)));
    }

    #[test]
    fn a_non_401_failure_on_a_trash_call_stays_a_plain_http_error() {
        let mut server = mockito::Server::new();
        let _restore = server.mock("POST", "/restore/item/t1").with_status(500).create();
        let bridge = VaultBridge::new(server.url());
        assert!(matches!(bridge.restore_item("t1"), Err(VaultError::Http(_))));
    }

    #[test]
    fn a_trashed_item_round_trips_with_every_captured_key_including_deleted_date() {
        // The capture's trashed-item key list, verbatim. `deletedDate` is
        // UNMODELLED on purpose, so what this pins is that riding the
        // catch-all is lossless: a trashed item written back (the restore path
        // does not PUT, but the vault window's ordinary edit can reach an item
        // this app has held) must not be a truncated copy of what arrived.
        let raw = r#"{"id":"t1","object":"item","type":1,"name":"Old thing",
            "deletedDate":"2026-07-30T09:15:00.000Z",
            "creationDate":"2020-01-01T00:00:00.000Z",
            "revisionDate":"2021-01-01T00:00:00.000Z",
            "favorite":false,"fields":[],"collectionIds":[],"attachments":[],
            "key":"K","reprompt":0,"passwordHistory":[],
            "login":{"username":"u","password":"p"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            before,
            serde_json::to_value(&item).unwrap(),
            "a trashed item changed shape across a round trip"
        );
    }

    #[test]
    fn deleted_date_is_present_on_a_trashed_item_and_absent_on_a_live_one() {
        // The accessor IS the trashed-ness predicate, which is only true
        // because the capture measured zero `deletedDate` keys across 1654
        // live items. Both directions are asserted so the property cannot rot
        // into "always Some" or "always None".
        let trashed: VaultItem = serde_json::from_str(
            r#"{"id":"t1","name":"A","fields":[],"deletedDate":"2026-07-30T09:15:00.000Z"}"#,
        )
        .unwrap();
        let live: VaultItem =
            serde_json::from_str(r#"{"id":"1","name":"A","fields":[]}"#).unwrap();
        assert_eq!(deleted_date(&trashed), Some("2026-07-30T09:15:00.000Z"));
        assert_eq!(deleted_date(&live), None);
    }

    #[test]
    fn without_deleted_date_drops_that_key_and_nothing_else() {
        // The one key a restore must not carry into the live snapshot, and
        // every other key -- including the rest of the catch-all -- untouched.
        let raw = r#"{"id":"t1","object":"item","type":1,"name":"Old thing",
            "deletedDate":"2026-07-30T09:15:00.000Z","favorite":true,"fields":[],
            "reprompt":0,"key":"K","login":{"username":"u"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let restored = without_deleted_date(&item);

        assert_eq!(deleted_date(&restored), None, "a restored item still claims a deletion date");

        let mut expected: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            expected.as_object_mut().unwrap().remove("deletedDate"),
            Some(serde_json::json!("2026-07-30T09:15:00.000Z")),
            "the premise: the source item carried the key"
        );
        assert_eq!(
            expected,
            serde_json::to_value(&restored).unwrap(),
            "restoring an item changed something other than its deletion date"
        );
    }

    #[test]
    fn without_deleted_date_on_a_live_item_is_a_faithful_copy() {
        // Idempotent, so the restore path cannot be got wrong by being run
        // twice or on an item that was never trashed.
        let raw = r#"{"id":"1","name":"A","type":1,"favorite":false,"fields":[],
            "reprompt":0,"login":{"username":"u"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert_eq!(
            serde_json::to_value(&item).unwrap(),
            serde_json::to_value(without_deleted_date(&item)).unwrap()
        );
    }

    // -----------------------------------------------------------------------
    // Revision tokens
    // -----------------------------------------------------------------------

    #[test]
    fn with_revision_date_from_takes_that_one_key_and_leaves_the_rest() {
        // The whole of what `VaultCache::current_revision_of` is allowed to do
        // with a read-back. A `GET` taken right after a restore may still be
        // showing the pre-restore state -- trashed, and under whatever name
        // and fields the server last committed -- so anything this took
        // besides the token would put that state back into the live snapshot.
        //
        // Returning `source.clone()` gives
        //     the source's OTHER keys came across too
        //     left: "the server's stale copy"  right: "Mine"
        let mine: VaultItem = serde_json::from_str(
            r#"{"id":"1","object":"item","type":1,"name":"Mine","favorite":true,"fields":[],
                "reprompt":0,"key":"K","revisionDate":"2026-07-30T09:15:00.000Z"}"#,
        )
        .unwrap();
        let source: VaultItem = serde_json::from_str(
            r#"{"id":"1","object":"item","type":1,"name":"the server's stale copy",
                "favorite":false,"fields":[],"deletedDate":"2026-07-30T09:15:00.000Z",
                "revisionDate":"2026-08-03T11:47:19.101Z"}"#,
        )
        .unwrap();

        let adopted = with_revision_date_from(&mine, &source);

        assert_eq!(
            adopted.other.get("revisionDate").and_then(|v| v.as_str()),
            Some("2026-08-03T11:47:19.101Z"),
            "the stale token survived the adoption"
        );
        assert_eq!(adopted.name, "Mine", "the source's OTHER keys came across too");
        assert_eq!(deleted_date(&adopted), None, "the source's deletion date came across too");

        // POSITIVE CONTROL: everything else really is byte-identical to
        // `mine`, so this cannot pass against a function that quietly dropped
        // a key the way an early `without_deleted_date` bug would have.
        let mut expected = serde_json::to_value(&mine).unwrap();
        expected.as_object_mut().unwrap().insert(
            "revisionDate".to_string(),
            serde_json::json!("2026-08-03T11:47:19.101Z"),
        );
        assert_eq!(expected, serde_json::to_value(&adopted).unwrap());
    }

    #[test]
    fn a_source_with_no_token_removes_the_one_the_item_had() {
        // "The server reports no token" and "the server reports the token I
        // already had" are different claims, and keeping a superseded token
        // because the answer did not mention one is the defect this function
        // exists for. Leaving `item`'s key in place on the `None` arm is the
        // mutation; it gives
        //     an unmentioned token was kept rather than dropped
        let mine: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"Mine","type":1,"fields":[],
                "revisionDate":"2026-07-30T09:15:00.000Z"}"#,
        )
        .unwrap();
        let tokenless: VaultItem =
            serde_json::from_str(r#"{"id":"1","name":"Mine","type":1,"fields":[]}"#).unwrap();

        let adopted = with_revision_date_from(&mine, &tokenless);

        assert_eq!(
            adopted.other.get("revisionDate"),
            None,
            "an unmentioned token was kept rather than dropped"
        );
        // POSITIVE CONTROL for the assertion above: a function that returned
        // an empty item, or `tokenless`, would satisfy it for free.
        assert_eq!(adopted.name, "Mine");
    }

    // -----------------------------------------------------------------------
    // Favourites
    // -----------------------------------------------------------------------

    #[test]
    fn with_favorite_changes_that_one_field_and_nothing_else() {
        // The same property `without_deleted_date` is held to: one field
        // moves, and everything else -- including every key riding the
        // catch-all -- is byte-identical. An item's favourite flag is written
        // by a full-state PUT, so anything this helper dropped would be
        // dropped from the user's vault.
        let raw = r#"{"id":"1","object":"item","type":1,"name":"A","favorite":false,
            "fields":[],"reprompt":0,"key":"K","passwordHistory":[],
            "collectionIds":[],"attachments":[],
            "login":{"username":"u","password":"p","passwordRevisionDate":null}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let favourited = with_favorite(&item, true);

        assert!(favourited.favorite, "with_favorite(.., true) did not set the flag");

        let mut expected: serde_json::Value = serde_json::from_str(raw).unwrap();
        expected.as_object_mut().unwrap().insert("favorite".into(), serde_json::json!(true));
        assert_eq!(
            expected,
            serde_json::to_value(&favourited).unwrap(),
            "favouriting an item changed something other than its favourite flag"
        );
    }

    #[test]
    fn un_favouriting_an_item_states_favorite_false_on_the_wire() {
        // THE SILENT-NO-OP GUARD, and it is the `folderId` trap one field
        // over: `.superpowers/sdd/put-semantics-capture.md` records that this
        // backend MERGES omitted keys. If `VaultItem::favorite` ever gained a
        // `skip_serializing_if`, un-favouriting would send a body with no
        // `favorite` key at all, the server would keep the old value, and the
        // star would come back on the next sync while the app reported
        // success. `favorite` carries no such attribute today and this asserts
        // the consequence rather than the attribute.
        let item: VaultItem =
            serde_json::from_str(r#"{"id":"1","name":"A","fields":[],"favorite":true}"#).unwrap();
        let body = serde_json::to_value(with_favorite(&item, false)).unwrap();
        assert_eq!(
            body.get("favorite"),
            Some(&serde_json::json!(false)),
            "un-favouriting produced a body that does not state favorite=false: {body}"
        );
    }

    // -----------------------------------------------------------------------
    // `passwordHistory`
    // -----------------------------------------------------------------------

    #[test]
    fn password_history_reads_the_captured_wire_shape_newest_first() {
        // The shape is the CLI's own `PasswordHistoryResponse`
        // (`lastUsedDate`, `password`), which is the class that builds this
        // JSON -- not a recollection of it.
        let item: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"A","fields":[],"passwordHistory":[
                {"lastUsedDate":"2026-07-30T09:15:00.000Z","password":"newer-old"},
                {"lastUsedDate":"2024-01-02T03:04:05.000Z","password":"older-old"}
            ]}"#,
        )
        .unwrap();
        let history = password_history(&item);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].password.as_str(), "newer-old");
        assert_eq!(history[0].last_used_date.as_deref(), Some("2026-07-30T09:15:00.000Z"));
        assert_eq!(history[1].password.as_str(), "older-old");
        assert_eq!(history[1].last_used_date.as_deref(), Some("2024-01-02T03:04:05.000Z"));
    }

    #[test]
    fn an_item_with_no_history_reads_as_empty_rather_than_failing() {
        // All four ways "no history" actually arrives. `[]` is not a
        // hypothetical: it is what all 1654 items of the user's live vault
        // carried when this was measured on 2026-08-01. `null` is what the
        // CLI writes when a history is emptied
        // (`adjustPasswordHistoryLength`), and an absent key is what a freshly
        // created item has. The fourth is a shape change this build has never
        // seen, and the rule for it is the same: no rows, no error banner.
        for raw in [
            r#"{"id":"1","name":"A","fields":[],"passwordHistory":[]}"#,
            r#"{"id":"1","name":"A","fields":[],"passwordHistory":null}"#,
            r#"{"id":"1","name":"A","fields":[]}"#,
            r#"{"id":"1","name":"A","fields":[],"passwordHistory":"nonsense"}"#,
        ] {
            let item: VaultItem = serde_json::from_str(raw).unwrap();
            assert!(
                password_history(&item).is_empty(),
                "expected no history entries from {raw}"
            );
        }
    }

    #[test]
    fn a_history_entry_without_a_password_is_skipped_and_one_without_a_date_is_kept() {
        // Asymmetric on purpose. The password is what the row exists to show,
        // so an entry without one is not a row; the date is decoration, so an
        // entry without one still shows the secret the user has.
        let item: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"A","fields":[],"passwordHistory":[
                {"lastUsedDate":"2026-07-30T09:15:00.000Z"},
                {"password":"dateless"},
                {"lastUsedDate":"2026-07-30T09:15:00.000Z","password":null},
                "not-an-object"
            ]}"#,
        )
        .unwrap();
        let history = password_history(&item);
        assert_eq!(history.len(), 1, "{history:?}");
        assert_eq!(history[0].password.as_str(), "dateless");
        assert_eq!(history[0].last_used_date, None);
    }

    #[test]
    fn an_items_password_history_survives_a_round_trip_untouched() {
        // The property that matters most, and the reason this is an accessor
        // over the catch-all rather than a modelled field: password history is
        // data the SERVER maintains, every write this app makes is a
        // full-state PUT, and a client that dropped the array would delete the
        // user's previous passwords. Two entries, so an off-by-one truncation
        // would show.
        let raw = r#"{"id":"1","object":"item","type":1,"name":"A","favorite":false,
            "fields":[],"reprompt":0,"key":"K","collectionIds":[],"attachments":[],
            "passwordHistory":[
                {"lastUsedDate":"2026-07-30T09:15:00.000Z","password":"p1"},
                {"lastUsedDate":"2024-01-02T03:04:05.000Z","password":"p2"}],
            "login":{"username":"u","password":"p"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(
            before,
            serde_json::to_value(&item).unwrap(),
            "an item's password history did not survive a round trip"
        );
        // And it survives the two helpers that rebuild an item, since both
        // are on the write path.
        for rebuilt in [with_favorite(&item, true), with_folder(&item, Some("f1"))] {
            assert_eq!(
                password_history(&rebuilt).len(),
                2,
                "a rebuilt item lost its password history"
            );
        }
    }

    #[test]
    fn a_history_password_is_not_printed_by_its_own_debug() {
        // `{:?}` on a secret is how plaintext reaches a log line. Every
        // pre-existing secret-carrying struct here derives `Debug` and does
        // leak -- recorded, not re-litigated -- so this new one starts
        // without the escape route.
        let item: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"A","fields":[],
                "passwordHistory":[{"lastUsedDate":"2026-07-30T09:15:00.000Z",
                                    "password":"correct-horse"}]}"#,
        )
        .unwrap();
        let printed = format!("{:?}", password_history(&item));
        assert!(!printed.contains("correct-horse"), "a previous password was printed: {printed}");
        assert!(
            printed.contains("2026-07-30T09:15:00.000Z"),
            "the redaction also swallowed the date, so this test could pass on an \
             empty vector: {printed}"
        );
    }

    // -----------------------------------------------------------------------
    // `GET /generate`
    //
    // EVERY assertion here is on the REQUEST, and that is the whole point.
    // `bw serve` hands `ctx.request.query` to `GenerateCommand` verbatim and
    // that command reads each key with `CliUtils.convertBooleanOption` /
    // `convertNumberOption` / `convertStringOption`, every one of which
    // FALLS BACK SILENTLY: an unrecognised key is not an error, it is an
    // absent option, and the command then substitutes a default. So a
    // misspelled parameter answers 200 with a perfectly good password
    // generated to the WRONG recipe -- a 20-character 4-class password
    // silently becomes the CLI's default 14-character `uln` one, and nothing
    // about the response says so. Asserting the query string is the only
    // thing that can catch it.
    // -----------------------------------------------------------------------

    fn a_generated_body(value: &str) -> String {
        // The shape `StringResponse` + `Response.success` produce, which is
        // the same double-nested envelope `/object/totp/{id}` uses.
        format!(r#"{{"success":true,"data":{{"object":"string","data":"{value}"}}}}"#)
    }

    #[test]
    fn generating_a_password_asks_for_every_option_by_its_serve_route_spelling() {
        // The spellings are the LONG option names only. The CLI's short flags
        // (`-u`, `-l`, `-n`, `-s`, `-p`, `-c`) are commander aliases that
        // exist only on the argv parser; the serve route never sees them, so
        // `?u=true` would be ignored and the class silently dropped.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/generate")
            .match_query(mockito::Matcher::Exact(
                "length=20&uppercase=true&lowercase=true&number=true&special=true\
                 &minNumber=1&minSpecial=1&ambiguous=true"
                    .into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_generated_body("XJl2s6xNXpa1fpTiC2pKWzoA"))
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        let generated = bridge
            .generate(&GenerateRequest::Password(PasswordRecipe::default()))
            .unwrap();
        m.assert();
        assert_eq!(generated.as_str(), "XJl2s6xNXpa1fpTiC2pKWzoA");
    }

    #[test]
    fn generating_a_passphrase_states_passphrase_true_and_its_own_options() {
        // Without `passphrase=true` the route generates a PASSWORD and
        // answers 200: `GenerateCommand` picks its type from exactly this key
        // (`convertBooleanOption(passedOptions?.passphrase) ? "passphrase" :
        // "password"`), so the omission is not an error anywhere, it is a
        // different kind of secret than the user asked for.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/generate")
            .match_query(mockito::Matcher::Exact(
                "passphrase=true&words=4&separator=-&capitalize=true&includeNumber=true".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_generated_body("Haiku-Gaffe-Sliding-Broker"))
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        let generated = bridge
            .generate(&GenerateRequest::Passphrase(PassphraseRecipe::default()))
            .unwrap();
        m.assert();
        assert_eq!(generated.as_str(), "Haiku-Gaffe-Sliding-Broker");
    }

    #[test]
    fn a_password_request_never_states_passphrase_and_a_passphrase_never_states_length() {
        // The mirror-image mistakes, and neither would fail against the live
        // backend: `passphrase=false` leaking onto a password request is
        // harmless today but would become a passphrase the moment the key were
        // built from a variable, and `length` on a passphrase request is read
        // by the command and then ignored, so a UI offering a length control
        // for a passphrase would appear to work.
        let password = GenerateRequest::Password(PasswordRecipe::default()).query_string();
        let passphrase = GenerateRequest::Passphrase(PassphraseRecipe::default()).query_string();
        assert!(
            !password.contains("passphrase"),
            "a password request stated the passphrase switch: {password}"
        );
        assert!(
            !passphrase.contains("length"),
            "a passphrase request stated a character length: {passphrase}"
        );
        assert!(
            passphrase.starts_with("passphrase=true"),
            "a passphrase request did not state passphrase=true: {passphrase}"
        );
    }

    #[test]
    fn booleans_are_stated_as_the_literal_true_and_false_the_route_understands() {
        // `CliUtils.convertBooleanOption` accepts a value of `""` or `"true"`
        // (case-insensitively) and treats EVERYTHING ELSE as false -- `"1"`,
        // `"yes"` and `"on"` all read as off. So `false` has to be spelled,
        // and it has to be spelled as something that is not `true`.
        let off = PasswordRecipe {
            uppercase: false,
            special: false,
            avoid_ambiguous: false,
            ..PasswordRecipe::default()
        };
        let query = GenerateRequest::Password(off).query_string();
        assert!(query.contains("uppercase=false"), "{query}");
        assert!(query.contains("special=false"), "{query}");
        assert!(query.contains("ambiguous=false"), "{query}");
        assert!(query.contains("lowercase=true"), "{query}");
    }

    #[test]
    fn a_401_from_the_generator_maps_to_unauthorized() {
        // Same rule as every other call in this file: a locked vault must
        // reach the re-auth path, not a generic "something went wrong" beside
        // a password box.
        let mut server = mockito::Server::new();
        // `Matcher::Any` is stated rather than left to the default, and it
        // has to be: mockito 1.x defaults a mock's query matcher to
        // "no query string at all", so a mock declared without this answers
        // **501** to the query-carrying request under test -- which arrives
        // as `Http`, and would have made this test claim the 401 mapping was
        // broken when it was not. Watched, not assumed.
        let _m = server
            .mock("GET", "/generate")
            .match_query(mockito::Matcher::Any)
            .with_status(401)
            .create();
        let bridge = VaultBridge::new(server.url());
        assert!(matches!(
            bridge.generate(&GenerateRequest::Password(PasswordRecipe::default())),
            Err(VaultError::Unauthorized)
        ));
    }

    #[test]
    fn a_non_401_generator_failure_stays_a_plain_http_error() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/generate")
            .match_query(mockito::Matcher::Any)
            .with_status(500)
            .create();
        let bridge = VaultBridge::new(server.url());
        assert!(matches!(
            bridge.generate(&GenerateRequest::Password(PasswordRecipe::default())),
            Err(VaultError::Http(_))
        ));
    }

    #[test]
    fn a_generator_response_that_is_not_the_expected_envelope_is_a_parse_error() {
        // `/generate` wraps its answer the same double-nested way
        // `/object/totp/{id}` does. A bare `{"success":true,"data":"pw"}` is
        // what a naive reading of the route would expect, and taking it would
        // mean this call had quietly stopped matching the backend.
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/generate")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":"pw"}"#)
            .create();
        let bridge = VaultBridge::new(server.url());
        assert!(matches!(
            bridge.generate(&GenerateRequest::Password(PasswordRecipe::default())),
            Err(VaultError::Parse(_))
        ));
    }

    #[test]
    fn a_passphrase_separator_reaches_the_wire_percent_encoded() {
        // The one free-text option. A separator of `&` or `=` written into the
        // query unescaped would split into extra parameters, and the route
        // would read the fragments as options rather than fail -- so what is
        // pinned is that the value survives as ONE parameter.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/generate")
            .match_query(mockito::Matcher::UrlEncoded("separator".into(), "&".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(a_generated_body("a&b&c&d"))
            .expect(1)
            .create();

        let bridge = VaultBridge::new(server.url());
        bridge
            .generate(&GenerateRequest::Passphrase(PassphraseRecipe {
                separator: "&".to_string(),
                ..PassphraseRecipe::default()
            }))
            .unwrap();
        m.assert();
    }
    // -----------------------------------------------------------------
    // The region BELOW the cut -- the half no source guard here reads.
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and cannot be found by a guard looking for
    /// the real attribute.
    const BELOW_CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// The literal every source guard in this file cuts the file at. Split so
    /// it is not itself an occurrence of the thing it names -- unsplit it
    /// would be a SECOND occurrence in this file, and the uniqueness control
    /// below could not be written at all.
    const BELOW_CUT_MARKER: &str = concat!("mod ", "tests {");

    /// Column-0 lines below the cut that are the CONTENTS OF A STRING LITERAL
    /// rather than source. Each is controlled below: it must still occur in
    /// this file exactly once, so a stale entry cannot quietly widen the hole
    /// the walk exists to close.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Deliberately exact rather than a `starts_with`:
    /// a whole module written on one line is not a module opener as far as
    /// this walk is concerned, and must fail it.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// The two-state walk of everything from the cut to EOF, over whatever
    /// text it is handed. Returns `(visited, modules, closes, depth)` so the
    /// caller can control it for non-vacuity.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison here is against the line's real
    /// text on a CRLF working tree and on an LF one alike. The blobs this
    /// repository stores are LF and only `core.autocrlf=true` makes a working
    /// tree CRLF, so a needle written with a carriage return in it would match
    /// nothing on a plain checkout -- green, and reading nothing.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = source
            .find(BELOW_CUT_MARKER)
            .expect("the cut marker is controlled by the caller");
        let mut depth = 0usize;
        // The module the cut lands ON is gated by the attribute immediately
        // above the cut, which is outside the region walked here. The test
        // below asserts that attribute is there; this `true` is that
        // assertion's other half.
        let mut gated = true;
        let mut modules = 0usize;
        let mut closes = 0usize;
        let mut visited = 0usize;
        for line in source[cut..].lines() {
            visited += 1;
            if depth == 0 {
                // Between modules NOTHING is allowed but blanks, comments, the
                // gate and a module opener -- at ANY indentation, because an
                // indented `fn` at file scope is still a top-level item and a
                // column-0-only filter would miss it.
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                if trimmed == BELOW_CUT_GATE {
                    gated = true;
                    continue;
                }
                assert!(
                    !line.starts_with(char::is_whitespace) && below_cut_is_module_opener(trimmed),
                    "top-level source below the cut: {line:?}. Every source guard in this file \
                     slices at the test-module opener and reads only what is ABOVE it, so an \
                     item down here is read by none of them: a tenth route added down here is never checked for the verb-to-agent pairing the guard above enforces -- \
                     and the suite stays green. Move it above the test module."
                );
                assert!(
                    gated,
                    "the module {line:?} below the cut is not test-gated, so it SHIPS -- and it \
                     ships in the half of the file no source guard here reads"
                );
                gated = false;
                depth = 1;
                modules += 1;
            } else if !line.is_empty() && !line.starts_with(char::is_whitespace) {
                // Inside a test module every item is indented, so the only
                // column-0 line is the module's own closing brace.
                if line == "}" {
                    depth = 0;
                    closes += 1;
                    continue;
                }
                assert!(
                    BELOW_CUT_STRING_LINES.contains(&line),
                    "a column-0 line inside a test module below the cut: {line:?}. Either a \
                     top-level item escaped the brace count, or this is the contents of a \
                     string literal and belongs in BELOW_CUT_STRING_LINES"
                );
            }
        }
        (visited, modules, closes, depth)
    }

    /// **Below the cut there is nothing but gated test modules, and the cut is
    /// where every guard in this file believes it is.**
    ///
    /// Two things can silently empty every source guard in this file, and
    /// neither changes a single guard's own text:
    ///
    /// 1. **Anything appended below the test module is invisible to all of
    ///    them.** They read the half above the cut and nothing else. Measured
    ///    on the commit before this test existed, a one-line tuple struct
    ///    appended at EOF gives 1772 lib + 169 bin, 0 failed, 0 warnings.
    /// 2. **The cut can move UP.** The slice takes the FIRST occurrence of the
    ///    marker, so the marker appearing in a comment or a string above the
    ///    real test module truncates the production half and vacates every
    ///    guard downstream of the truncation -- silently, because the guards
    ///    whose needles still fall inside go on passing.
    ///
    /// The walk closes the first; the uniqueness and anchor controls close the
    /// second.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() {
        let source: &str = include_str!("vault_bridge.rs");

        // 1. The cut lands where the guards think it does, and there is only
        //    one place it could land.
        let seen = source.matches(BELOW_CUT_MARKER).count();
        assert_eq!(
            seen, 1,
            "the cut marker occurs {seen} times in this file. Every guard here takes the FIRST \
             one, so a second occurrence -- in a comment, in a string, in a doc example -- is \
             a cut that can move up and truncate the production half all of them read"
        );
        let cut = source
            .find(BELOW_CUT_MARKER)
            .expect("counted exactly one just above");
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the marker was matched inside a \
             comment or a string literal rather than at a real module opener"
        );
        assert!(
            source[..cut].trim_end().ends_with(BELOW_CUT_GATE),
            "the module the cut lands on is not preceded by the test gate, so the region below \
             the cut opens with a module that SHIPS"
        );

        // 2. Positive control on WHERE the cut is: the production half must
        //    still reach the last production item in the file. Were the marker
        //    matched above the real test module, this anchor would fall below
        //    the cut instead of just above it.
        const LAST_PRODUCTION_ITEM: &str = concat!("Ok(Zeroizing::new(body.", "data.data))");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: the anchor (the last route's return) is not in this file exactly once, \
             so it no longer pins anything -- repoint it at the last production item above the \
             test module"
        );
        let anchor = source
            .find(LAST_PRODUCTION_ITEM)
            .expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, which means \
             the cut moved up and the production half every guard in this file reads is \
             truncated"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk, run over an LF copy of this file and a CRLF copy of the
        //    same text, which must agree. Built BOTH ways rather than compared
        //    against the bytes on disk on purpose: this repository stores LF
        //    blobs and only `core.autocrlf=true` makes a working tree CRLF, so
        //    a control that asserted "this file is CRLF" would itself be a
        //    check that passes on this machine and fails on Linux CI -- which
        //    is the defect being closed here, wearing the other hat.
        let lf = source.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        let as_lf = walk_below_the_cut(&lf);
        let as_crlf = walk_below_the_cut(&crlf);
        assert_eq!(
            as_lf, as_crlf,
            "the walk gives a different answer on an LF copy of this file than on a CRLF one, \
             so something in it is sensitive to line endings"
        );
        // And the file as it really is on disk, whichever of the two that is.
        let as_on_disk = walk_below_the_cut(source);
        assert!(
            as_on_disk == as_lf || as_on_disk == as_crlf,
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) = as_on_disk;
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a test \
             module's worth -- the slice is empty or nearly so and this test proves nothing"
        );
        assert_eq!(
            depth, 0,
            "a test module below the cut is never closed by a column-0 brace, so the walk ran \
             off the end of the file inside it and stopped inspecting top-level lines"
        );
        assert_eq!(
            modules, 1,
            "the number of top-level test modules below the cut changed. That is fine -- but \
             this count is the control that proves the walk really visited them, so update it \
             deliberately rather than loosening it"
        );
        assert_eq!(
            closes, modules,
            "control: every module the walk opened must also have been closed at column 0"
        );
        for known in BELOW_CUT_STRING_LINES {
            assert_eq!(
                source.matches(known).count(),
                1,
                "control: the string-literal exception {known:?} is not in this file exactly \
                 once, so it is stale and is widening this check for nothing"
            );
        }
    }
}

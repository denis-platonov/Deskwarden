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
    /// Item-level free text. A secure note's entire body lives here, which is
    /// why that type needs no struct of its own -- its `secureNote` object
    /// carries only a `{"type": 0}` discriminator, which rides
    /// [`Self::other`] untouched -- and why notes on an ordinary login were
    /// invisible until this field existed.
    ///
    /// `Zeroizing` because a secure note *is* the secret, exactly as
    /// [`LoginData::password`] is.
    ///
    /// There is deliberately **no `ssh_key` field yet**: `type: 5`'s wire
    /// shape is the one this repo could not verify (the CLI's
    /// `item.sshKey` template endpoint 400s and no real sample exists), and
    /// modelling it from memory is how a modelled field and its `other` copy
    /// start disagreeing. A type-5 item's `sshKey` object rides
    /// [`Self::other`] intact in the meantime.
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

pub fn extract_app_match(item: &VaultItem) -> Option<AppMatch> {
    item.fields
        .iter()
        .find(|f| f.name.as_deref() == Some(APP_MATCH_FIELD_NAME))
        .and_then(|f| f.value.as_deref())
        .and_then(|v| AppMatch::from_field_value(v).ok())
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

/// The minimal payload to create a new Login item. `bw serve`'s create
/// endpoint wants a full item shape (like the edit endpoint), but a brand
/// new item has nothing else to preserve, unlike `update_item`.
#[derive(Debug, Clone)]
pub struct NewLoginItem {
    pub name: String,
    pub username: String,
    pub password: String,
    pub folder_id: Option<String>,
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

#[derive(Clone)]
pub struct VaultBridge {
    base_url: String,
    agent: ureq::Agent,
}

/// Connect timeout for `agent` below. `bw serve` is a local process on
/// `localhost`, not a remote host, so even this is generous for a plain TCP
/// handshake -- the point is only to fail fast if the port stops accepting
/// connections at all (the process died, or never started), not to give a
/// slow network the benefit of the doubt.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Read timeout for `agent` below (time to receive the response once the
/// request has been sent, not overall request time). `list_items` -- the
/// slowest normal call, pulling the entire vault in one response -- was
/// measured at ~1.1s for 1657 items against a cold `bw serve`; this leaves
/// generous headroom above that for a larger vault or a loaded machine while
/// still bounding the worst case. Notably: this agent is shared by every
/// vault call in this app, including the once-per-second TOTP poll on the
/// vault window's UI thread (review Minor 4, independent review of a7b33cb)
/// -- with no timeout at all, a `bw serve` that accepted the connection but
/// then hung (not crashed -- a crash fails fast on its own) blocked that
/// poll, and the whole UI thread with it, indefinitely.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

impl VaultBridge {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            agent: ureq::AgentBuilder::new()
                .timeout_connect(CONNECT_TIMEOUT)
                .timeout_read(READ_TIMEOUT)
                .build(),
        }
    }

    pub fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        let url = format!("{}/list/object/items", self.base_url);
        let body: Envelope<ItemList> = self
            .agent
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
            .agent
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
            .agent
            .get(&url)
            .call()
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data.data)
    }

    pub fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<(), VaultError> {
        self.update_item(&with_app_match(item, m))
    }

    pub fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        let url = format!("{}/object/folder", self.base_url);
        let body: Envelope<Folder> = self
            .agent
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
            .agent
            .put(&url)
            .send_json(serde_json::json!({ "name": name }))
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data)
    }

    pub fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/object/folder/{}", self.base_url, id);
        self.agent
            .delete(&url)
            .call()
            .map_err(map_http_err)?;
        Ok(())
    }

    pub fn create_item(&self, new_item: &NewLoginItem) -> Result<VaultItem, VaultError> {
        let url = format!("{}/object/item", self.base_url);
        // Blank means absent, not an empty string -- matching
        // `EditDraft::apply_to`'s convention (and `LoginData`'s own
        // `skip_serializing_if = "Option::is_none"`) for the edit path. A
        // newly-created item with a blank username, saved verbatim here as
        // `"username": ""` and then immediately re-saved once through the
        // edit form (which maps blank to an absent key), would otherwise
        // silently change shape server-side between the two saves.
        let mut login = serde_json::Map::new();
        if !new_item.username.is_empty() {
            login.insert("username".to_string(), serde_json::json!(new_item.username));
        }
        if !new_item.password.is_empty() {
            login.insert("password".to_string(), serde_json::json!(new_item.password));
        }
        let payload = serde_json::json!({
            "name": new_item.name,
            "type": 1,
            "folderId": new_item.folder_id,
            "login": login,
        });
        let body: Envelope<VaultItem> = self
            .agent
            .post(&url)
            .send_json(payload)
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data)
    }

    /// Writes `item` back as its own new state -- the same PUT `set_app_match`
    /// already used, generalized so the vault window's edit flow doesn't need
    /// its own copy of it.
    pub fn update_item(&self, item: &VaultItem) -> Result<(), VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, item.id);
        self.agent
            .put(&url)
            .send_json(item)
            .map_err(map_http_err)?;
        Ok(())
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
    pub fn move_item_to_folder(
        &self,
        item: &VaultItem,
        folder_id: Option<&str>,
    ) -> Result<(), VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, item.id);
        self.agent
            .put(&url)
            .send_json(folder_move_body(item, folder_id)?)
            .map_err(map_http_err)?;
        Ok(())
    }

    pub fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, id);
        self.agent
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
    /// Deliberately NOT cached: see [`crate::vault_cache::VaultCache::list_trash`].
    pub fn list_trash(&self) -> Result<Vec<VaultItem>, VaultError> {
        let url = format!("{}/list/object/items", self.base_url);
        let body: Envelope<ItemList> = self
            .agent
            .get(&url)
            .query("trash", "true")
            .call()
            .map_err(map_http_err)?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data.data)
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
        self.agent
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
        self.agent
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
        match self.agent.get(&url).call() {
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn an_ssh_key_object_rides_the_catch_all_untouched() {
        // `type: 5` is the one shape the capture could not verify, so
        // `SshKeyData` deliberately does not exist and the whole `sshKey`
        // object rides `VaultItem::other`. That deferral is only safe if the
        // object survives a full-state PUT byte-identically -- which follows
        // by isomorphism from the secure note's unmodelled `secureNote`, but
        // is worth stating outright since it is the guarantee the deferral
        // rests on. Field names here are illustrative and NOT a claim about
        // the real shape; the test asserts only that whatever arrives is
        // echoed back unchanged.
        let raw = r#"{"id":"1","name":"Deploy key","type":5,"favorite":false,"fields":[],
            "sshKey":{"privateKey":"PRIV","publicKey":"PUB","keyFingerprint":"FP"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert_eq!(ItemKind::of(&item), ItemKind::SshKey);
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap(), "an sshKey object was altered");
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
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        };
        assert!(extract_app_match(&item).is_none());
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

        let m = AppMatch {
            process: "RockstarGamesLauncher.exe".into(),
            trigger: TriggerMode::Prompt,
        };
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
            &AppMatch {
                process: "new.exe".into(),
                trigger: TriggerMode::Prompt,
            },
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
            &AppMatch {
                process: "game.exe".into(),
                trigger: TriggerMode::Prompt,
            },
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
            &AppMatch {
                process: "new.exe".into(),
                trigger: TriggerMode::Prompt,
            },
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
            &AppMatch {
                process: "game.exe".into(),
                trigger: TriggerMode::Prompt,
            },
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
        let m = AppMatch {
            process: "a.exe".into(),
            trigger: TriggerMode::Auto,
        };
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
        let m = AppMatch {
            process: "a.exe".into(),
            trigger: TriggerMode::Auto,
        };
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
        let m = AppMatch {
            process: "a.exe".into(),
            trigger: TriggerMode::Auto,
        };
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
        let m = AppMatch {
            process: "a.exe".into(),
            trigger: TriggerMode::Auto,
        };
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
        let m = AppMatch {
            process: "a.exe".into(),
            trigger: TriggerMode::Auto,
        };
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
        let m = AppMatch {
            process: "a.exe".into(),
            trigger: TriggerMode::Auto,
        };
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
        let m = crate::app_match::AppMatch {
            process: "a.exe".into(),
            trigger: crate::app_match::TriggerMode::Auto,
        };
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

    #[test]
    fn create_item_posts_a_login_shaped_payload() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/object/item")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"id":"9","name":"New","type":1,"fields":[],
                "login":{"username":"u","password":"p"}}}"#)
            .create();

        let bridge = VaultBridge::new(server.url());
        let new_item = NewLoginItem {
            name: "New".into(),
            username: "u".into(),
            password: "p".into(),
            folder_id: None,
        };
        let created = bridge.create_item(&new_item).unwrap();
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
        let new_item = NewLoginItem {
            name: "New".into(),
            username: "".into(),
            password: "".into(),
            folder_id: None,
        };
        let created = bridge.create_item(&new_item).unwrap();
        assert_eq!(created.id, "9");
    }

    #[test]
    fn update_item_puts_the_full_item_state() {
        let mut server = mockito::Server::new();
        let _m = server.mock("PUT", "/object/item/1").with_status(200).create();
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
        let m = server
            .mock("PUT", "/object/item/1")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "1",
                "name": "A",
                "fields": [],
                "favorite": false,
                "folderId": "f1",
            })))
            .with_status(200)
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
        let m = server
            .mock("PUT", "/object/item/1")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "1",
                "name": "A",
                "fields": [],
                "favorite": false,
                "folderId": null,
            })))
            .with_status(200)
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
        let m = server
            .mock("PUT", "/object/item/1")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "id": "1",
                "name": "A",
                "fields": [],
                "favorite": false,
            })))
            .with_status(200)
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
}

use crate::app_match::{AppMatch, APP_MATCH_FIELD_NAME};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VaultField {
    pub name: Option<String>,
    pub value: Option<String>,
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
    /// Raw `bw` item type: 1=Login, 2=SecureNote, 3=Card, 4=Identity.
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
    let mut fields: Vec<VaultField> = item
        .fields
        .iter()
        .filter(|f| f.name.as_deref() != Some(APP_MATCH_FIELD_NAME))
        .cloned()
        .collect();
    fields.push(VaultField {
        name: Some(APP_MATCH_FIELD_NAME.to_string()),
        value: Some(m.to_field_value()),
    });

    let mut updated = item.clone();
    updated.fields = fields;
    updated
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
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

    pub fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, id);
        self.agent
            .delete(&url)
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
    fn extract_app_match_finds_matching_field() {
        let item = VaultItem {
            id: "1".into(),
            name: "Rockstar".into(),
            fields: vec![VaultField {
                name: Some(APP_MATCH_FIELD_NAME_FOR_TEST.into()),
                value: Some(r#"{"process":"RockstarGamesLauncher.exe","trigger":"prompt"}"#.into()),
            }],
            login: None,
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
            }],
            login: None,
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
}

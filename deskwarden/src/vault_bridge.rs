use crate::app_match::{AppMatch, APP_MATCH_FIELD_NAME};
use serde::{Deserialize, Serialize};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,
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

impl VaultBridge {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            agent: ureq::Agent::new(),
        }
    }

    pub fn list_items(&self) -> Result<Vec<VaultItem>, VaultError> {
        let url = format!("{}/list/object/items", self.base_url);
        let body: Envelope<ItemList> = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| VaultError::Http(e.to_string()))?
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
            .map_err(|e| VaultError::Http(e.to_string()))?
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
            .map_err(|e| VaultError::Http(e.to_string()))?
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
            .map_err(|e| VaultError::Http(e.to_string()))?
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
            .map_err(|e| VaultError::Http(e.to_string()))?
            .into_json()
            .map_err(|e| VaultError::Parse(e.to_string()))?;
        Ok(body.data)
    }

    pub fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/object/folder/{}", self.base_url, id);
        self.agent
            .delete(&url)
            .call()
            .map_err(|e| VaultError::Http(e.to_string()))?;
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
            .map_err(|e| VaultError::Http(e.to_string()))?
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
            .map_err(|e| VaultError::Http(e.to_string()))?;
        Ok(())
    }

    pub fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        let url = format!("{}/object/item/{}", self.base_url, id);
        self.agent
            .delete(&url)
            .call()
            .map_err(|e| VaultError::Http(e.to_string()))?;
        Ok(())
    }

    /// `None` when the item has no TOTP secret configured -- `bw serve`
    /// answers that with a non-2xx rather than a null payload, so any HTTP
    /// failure here is treated as "no code" rather than propagated as
    /// `VaultError`. A *parse* failure on an actual 2xx response still is one:
    /// that would mean `bw serve` changed shape under us, worth surfacing.
    pub fn get_totp(&self, id: &str) -> Result<Option<String>, VaultError> {
        let url = format!("{}/object/totp/{}", self.base_url, id);
        match self.agent.get(&url).call() {
            Ok(response) => {
                let body: Envelope<TotpData> = response
                    .into_json()
                    .map_err(|e| VaultError::Parse(e.to_string()))?;
                Ok(body.data.data)
            }
            Err(ureq::Error::Status(_, _)) => Ok(None),
            Err(e) => Err(VaultError::Http(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_match::TriggerMode;

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
        assert_eq!(login.password.as_deref(), Some("p"));
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
        assert_eq!(login.totp.as_deref(), Some("SEED123"));
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
}

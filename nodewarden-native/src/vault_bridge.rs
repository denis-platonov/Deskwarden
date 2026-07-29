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
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// Pure helper: returns a copy of `item` with its `nodewarden:app-match`
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

    pub fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<(), VaultError> {
        let updated = with_app_match(item, m);

        let url = format!("{}/object/item/{}", self.base_url, item.id);
        self.agent
            .put(&url)
            .send_json(&updated)
            .map_err(|e| VaultError::Http(e.to_string()))?;
        Ok(())
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
}

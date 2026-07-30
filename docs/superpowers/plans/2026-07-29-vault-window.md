# Vault Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the "2b" vault window from `docs/design/deskwarden-design-spec.md` (section 4.8): a three-pane browser (folders sidebar · item list · detail pane) reachable from the tray, with full read/edit/create for login items and folders, live TOTP, password strength, local fill-count analytics, an auto-lock countdown, and real website favicons per item.

**Architecture:** A new `vault_window` module (own directory, one file per pane) opened as a blocking `eframe` window from the tray, following the exact pattern already used by `login_ui::run_login_flow` and `picker_ui::run_picker` (frameless custom-chrome window, `Rc<RefCell<_>>` result handoff, `styled` first-frame font-timing guard). `vault_bridge.rs` gains the typed fields and CRUD methods the window needs; three small new leaf modules (`fill_stats.rs`, `password_strength.rs`, `favicon.rs`) hold logic that has nothing to do with drawing and needs to be unit-tested on its own.

**Tech Stack:** Same as the rest of the crate — `eframe`/`egui` 0.35, `ureq` for HTTP (favicon fetch, reusing the same `bw serve` HTTP pattern), `serde`/`serde_json`, and now `png` promoted from a dev-only to a real dependency (favicon decoding).

## Global Constraints

- Window size: fixed **1240×740**, frameless with the same custom-chrome pattern as `login_ui::draw_window_chrome` (mark + title left, ▢✕— controls right, draggable bar). Not resizable — matches every other window in this app.
- Pane widths: sidebar **212px**, item list **390px**, detail pane gets the remainder (both fixed dividers are hairline-separated, matching `theme::HAIRLINE`).
- Auto-lock timeout is a hardcoded constant, **15 minutes**, until the (not-yet-built) 3e preferences window makes it configurable. Name the constant `AUTO_LOCK_TIMEOUT` in `vault_window/mod.rs` so it's a one-line change later.
- Fill-count analytics are **local-only** — a JSON file in the config dir (`fill_stats.json`, next to `session.bin`), never written back to the vault. `app::fill_from_vault` gets one new call to `fill_stats::record_fill` after a successful fill; it must not gain a vault write.
- Password strength is a **local heuristic**, no new crate: four tiers, `Weak` / `Fair` / `Good` / `Strong`.
- Favicons: fetched over HTTP from `{icon_base}/{domain}/icon.png`, `icon_base` being `https://icons.bitwarden.net` for the default cloud server or `{server_url}/icons` for a configured self-hosted one (see Task 4). Any failure — no URI on the item, fetch error, non-200, decode error — falls back to the existing colored-initials monogram (`theme::avatar`). Never blocks the UI thread; loads on a background thread, drained via a channel, exactly like `loading_ui::show_while`'s pattern.
- Scope boundary on item types: the design's own detail-pane examples (Ledgerline, Vantage VPN) are both **Login** items, and autofill only ever fills Logins. Cards / Secure notes / Identities appear in the sidebar counts, list rows, and type badges (per spec), but selecting one shows a minimal read-only "not editable yet" panel in the detail pane rather than a bespoke card/note editor. Full create/edit/delete is Login-only and folder-only.
- Folder scope: **create and delete only**, no rename — the user asked for "folder creation" specifically; rename is not in the design spec's folder rows either.
- Sync: this app has nothing that auto-syncs with Bitwarden's server on a timer -- `main()` runs `bw sync` exactly once, at startup; everything after that (including this window's own item/folder lists) only re-reads whatever `bw serve` already has cached locally. Task 9 adds a manual "Sync" toolbar button (`bw_serve::run_bw_sync`, reloading `items`/`folders` on success) as the escape hatch for "I changed something on another device" -- this is a deliberate, minimal addition (a button, not a background poller) requested mid-plan; do not expand it into automatic periodic syncing, which is a materially different (and more failure-prone, given `bw sync` is a real network+CLI round-trip) feature than what was asked for.
- Toolbar scope cut: the design's toolbar also shows a live "● Synced 1 min ago" pill and an account-initials avatar circle. The avatar is cheap (Task 9 adds it: `theme::avatar` fed from `login_ui::check_bw_status_details().user_email`, same source the login window already uses). The sync pill is **not built** — this app has nowhere that records a last-sync timestamp today, and adding one is a separate, real feature (tracking `bw sync` completion times) rather than a vault-window detail; flagged here as a deliberate cut, not an oversight, so it isn't quietly expected of Task 9's implementer.
- Keyboard shortcuts from spec section 5 that apply to this window (`Ctrl+K` focus search, `Ctrl+L` lock, `Ctrl+N` new item, `Ctrl+Shift+F` fill in app) are real requirements, not nice-to-haves — Task 9 wires all four alongside the window's own logic, not left as a "click the button" fallback.
- Every new Win32/`egui` window follows the established font-timing guard (`styled` bool, `theme::apply` on frame 1, real UI from frame 2) — this is a confirmed real crash source elsewhere in this codebase (see `login_ui.rs`, `picker_ui.rs`).
- Reuse `theme.rs` primitives everywhere one exists (`avatar`, `primary_button`, `secondary_button`, `card_header`, `field_label`, `text_field`, `password_field`, `hairline`, `kbd_chip`, `toggle_pill`) rather than hand-rolling new widgets that already have a themed equivalent.

---

### Task 1: VaultBridge — typed item fields + Folder model

**Files:**
- Modify: `deskwarden/src/vault_bridge.rs`
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `VaultItem::item_type: Option<i64>` (raw bw type: 1=Login, 2=SecureNote, 3=Card, 4=Identity), `VaultItem::folder_id: Option<String>`, `VaultItem::favorite: bool`, `LoginData::totp: Option<String>`, `LoginData::uris: Vec<UriEntry>` where `pub struct UriEntry { pub uri: Option<String> }`. Produces `pub struct Folder { pub id: String, pub name: String }` and `VaultBridge::list_folders(&self) -> Result<Vec<Folder>, VaultError>`.
- Consumes: nothing new — extends the existing `Envelope<T>`/`ItemList`-style deserialization already in this file.

- [ ] **Step 1: Write the failing tests**

Add to the existing `#[cfg(test)] mod tests` block in `deskwarden/src/vault_bridge.rs`:

```rust
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
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test --lib vault_bridge:: -- --include-ignored` from `deskwarden/`
Expected: FAIL to compile (`item_type`, `folder_id`, `totp`, `uris`, `Folder`, `list_folders` don't exist yet).

- [ ] **Step 3: Add the typed fields and `Folder` model**

In `deskwarden/src/vault_bridge.rs`, extend `LoginData`:

```rust
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

/// One entry of `login.uris`. Only the URI itself is modelled -- `bw`'s
/// match-strategy field on each entry is preserved through `VaultItem.other`
/// via the top-level flatten, same as everything else this struct doesn't
/// name.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UriEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}
```

Extend `VaultItem`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VaultItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub fields: Vec<VaultField>,
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
```

Add near the top of the file, after the existing structs:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct FolderList {
    data: Vec<Folder>,
}
```

Add to `impl VaultBridge`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib vault_bridge::` from `deskwarden/`
Expected: PASS, all tests including the pre-existing ones (this step must not regress `with_app_match`'s existing round-trip guarantees).

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/vault_bridge.rs
git commit -m "feat: add typed item fields and a Folder model to VaultBridge"
```

---

### Task 2: VaultBridge — folder/item CRUD + TOTP

**Files:**
- Modify: `deskwarden/src/vault_bridge.rs`

**Interfaces:**
- Consumes: `Folder`, `VaultItem`, `Envelope<T>` from Task 1.
- Produces: `VaultBridge::create_folder(&self, name: &str) -> Result<Folder, VaultError>`, `delete_folder(&self, id: &str) -> Result<(), VaultError>`, `create_item(&self, new_item: &NewLoginItem) -> Result<VaultItem, VaultError>` where `pub struct NewLoginItem { pub name: String, pub username: String, pub password: String, pub folder_id: Option<String> }`, `update_item(&self, item: &VaultItem) -> Result<(), VaultError>` (generalizes the PUT that `set_app_match` already does), `delete_item(&self, id: &str) -> Result<(), VaultError>`, `get_totp(&self, id: &str) -> Result<Option<String>, VaultError>`.

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib vault_bridge::` from `deskwarden/`
Expected: FAIL to compile (`NewLoginItem`, `create_folder`, etc. don't exist).

- [ ] **Step 3: Implement**

Add near `Folder`:

```rust
/// The minimal payload to create a new Login item. `bw serve`'s create
/// endpoint wants a full item shape (like the edit endpoint), but a brand
/// new item has nothing else to preserve, unlike `update_item`.
pub struct NewLoginItem {
    pub name: String,
    pub username: String,
    pub password: String,
    pub folder_id: Option<String>,
}
```

Add to `impl VaultBridge`:

```rust
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
        let payload = serde_json::json!({
            "name": new_item.name,
            "type": 1,
            "folderId": new_item.folder_id,
            "login": {
                "username": new_item.username,
                "password": new_item.password,
            },
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
                let body: Envelope<Option<String>> = response
                    .into_json()
                    .map_err(|e| VaultError::Parse(e.to_string()))?;
                Ok(body.data)
            }
            Err(ureq::Error::Status(_, _)) => Ok(None),
            Err(e) => Err(VaultError::Http(e.to_string())),
        }
    }
```

Now update `set_app_match` to call the new `update_item` instead of duplicating the PUT:

```rust
    pub fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<(), VaultError> {
        self.update_item(&with_app_match(item, m))
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib vault_bridge::` from `deskwarden/`
Expected: PASS, including the pre-existing `set_app_match`-adjacent tests (its behavior is unchanged, only its implementation now shares code with `update_item`).

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/vault_bridge.rs
git commit -m "feat: add folder/item CRUD and TOTP lookup to VaultBridge"
```

---

### Task 3: fill_stats.rs + password_strength.rs

**Files:**
- Create: `deskwarden/src/fill_stats.rs`
- Create: `deskwarden/src/password_strength.rs`
- Modify: `deskwarden/src/lib.rs` (declare both modules)
- Modify: `deskwarden/src/app.rs` (call `fill_stats::record_fill` after a successful fill)
- Modify: `deskwarden/src/main.rs` (construct the `FillStats` store next to `SessionStore`, thread it into `app::fill_from_vault`)

**Interfaces:**
- Produces: `fill_stats::FillStats::new(path: PathBuf) -> Self`, `.record_fill(&self, item_id: &str)`, `.count(&self, item_id: &str) -> u32`. Produces `password_strength::rate(password: &str) -> Strength` where `enum Strength { Weak, Fair, Good, Strong }` with `.label(&self) -> &'static str` ("Weak"/"Fair"/"Good"/"Strong").
- Consumes: nothing beyond `std`.

- [ ] **Step 1: Write the failing tests**

`deskwarden/src/fill_stats.rs`:

```rust
//! Local-only fill-count analytics for the vault window's detail pane
//! ("Filled 41 times"). Deliberately never touches the vault: this is
//! per-device usage trivia, not data worth a sync round-trip or a write on
//! every single autofill.

use std::collections::HashMap;
use std::path::PathBuf;

pub struct FillStats {
    path: PathBuf,
}

impl FillStats {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Increments `item_id`'s count and persists immediately. Best-effort:
    /// a failure to read or write the file is not the caller's problem --
    /// analytics that silently don't update this one time is a much smaller
    /// deal than a failed autofill.
    pub fn record_fill(&self, item_id: &str) {
        let mut counts = self.load();
        *counts.entry(item_id.to_string()).or_insert(0) += 1;
        let _ = self.save(&counts);
    }

    pub fn count(&self, item_id: &str) -> u32 {
        self.load().get(item_id).copied().unwrap_or(0)
    }

    fn load(&self) -> HashMap<String, u32> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, counts: &HashMap<String, u32>) -> std::io::Result<()> {
        let json = serde_json::to_string(counts)?;
        std::fs::write(&self.path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn unique_path(label: &str) -> PathBuf {
        temp_dir().join(format!("deskwarden-test-fill-stats-{label}-{}.json", std::process::id()))
    }

    #[test]
    fn a_fresh_item_has_zero_fills() {
        let stats = FillStats::new(unique_path("fresh"));
        assert_eq!(stats.count("item-1"), 0);
    }

    #[test]
    fn recording_a_fill_increments_and_persists() {
        let path = unique_path("increment");
        let stats = FillStats::new(path.clone());
        stats.record_fill("item-1");
        stats.record_fill("item-1");
        stats.record_fill("item-2");

        assert_eq!(stats.count("item-1"), 2);
        assert_eq!(stats.count("item-2"), 1);

        // A fresh handle to the same path sees the persisted counts.
        let reopened = FillStats::new(path.clone());
        assert_eq!(reopened.count("item-1"), 2);

        std::fs::remove_file(&path).ok();
    }
}
```

`deskwarden/src/password_strength.rs`:

```rust
//! A local, dependency-free password strength heuristic for the vault
//! window's detail-pane metadata strip ("Strength: strong"). Not a real
//! entropy estimator (that's what `zxcvbn` is for) -- just enough signal to
//! flag the two things a saved password can obviously get wrong: too short,
//! or drawn from only one character class.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    Weak,
    Fair,
    Good,
    Strong,
}

impl Strength {
    pub fn label(self) -> &'static str {
        match self {
            Strength::Weak => "Weak",
            Strength::Fair => "Fair",
            Strength::Good => "Good",
            Strength::Strong => "Strong",
        }
    }
}

/// Scores on length plus character-class diversity (lower/upper/digit/
/// symbol, up to 4 classes). Short passwords are capped low regardless of
/// diversity -- a 6-character password with all four classes is still weak.
pub fn rate(password: &str) -> Strength {
    let len = password.chars().count();
    if len == 0 {
        return Strength::Weak;
    }

    let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_symbol = password.chars().any(|c| !c.is_ascii_alphanumeric());
    let classes = [has_lower, has_upper, has_digit, has_symbol]
        .iter()
        .filter(|b| **b)
        .count();

    if len < 8 {
        return Strength::Weak;
    }
    if len < 12 {
        return if classes >= 3 { Strength::Fair } else { Strength::Weak };
    }
    if len < 16 {
        return if classes >= 3 { Strength::Good } else { Strength::Fair };
    }
    if classes >= 3 {
        Strength::Strong
    } else {
        Strength::Good
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_password_is_weak() {
        assert_eq!(rate(""), Strength::Weak);
    }

    #[test]
    fn short_password_is_weak_regardless_of_diversity() {
        assert_eq!(rate("Ab1!"), Strength::Weak);
    }

    #[test]
    fn long_single_class_password_is_not_strong() {
        assert_eq!(rate("aaaaaaaaaaaaaaaaaaaa"), Strength::Good);
    }

    #[test]
    fn long_diverse_password_is_strong() {
        assert_eq!(rate("Tr0ub4dor&3xtraLong!"), Strength::Strong);
    }

    #[test]
    fn medium_diverse_password_is_good() {
        assert_eq!(rate("Correct#Horse1"), Strength::Good);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib fill_stats:: password_strength::` from `deskwarden/`
Expected: FAIL (modules not declared in `lib.rs` yet, won't compile).

- [ ] **Step 3: Wire the modules in and call `record_fill`**

In `deskwarden/src/lib.rs`, add in alphabetical position:

```rust
pub mod fill_stats;
```
and
```rust
pub mod password_strength;
```

In `deskwarden/src/app.rs`, thread a `&FillStats` into `fill_from_vault` and record on success:

```rust
pub fn fill_from_vault<A: UiAutomationFiller, B: SendInputFiller>(
    vault: &VaultBridge,
    injector: &Injector<A, B>,
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
    hwnd: isize,
) {
    match vault.get_item(item_id) {
        Ok(item) => {
            let (username, password) = credentials_for(&item);
            if username.is_empty() && password.is_empty() {
                log::warn!("vault item {item_id} has no login credentials; nothing to fill");
                return;
            }
            match injector.fill(hwnd, &username, &password) {
                Ok(()) => fill_stats.record_fill(item_id),
                Err(e) => log::error!("fill failed for item {item_id} into hwnd {hwnd}: {e}"),
            }
        }
        Err(e) => log::error!("could not read vault item {item_id} to fill it: {e:?}"),
    }
}
```

`handle_match` and every other caller of `fill_from_vault` (both call sites already in `app.rs`) need the new parameter threaded through the same way `vault`/`injector` already are -- add `fill_stats: &crate::fill_stats::FillStats` to `handle_match`'s signature too, and pass it down. Update `app.rs`'s existing tests only if any call `fill_from_vault`/`handle_match` directly (check before editing; if none do, no test changes needed here).

In `deskwarden/src/main.rs`, next to the existing `SessionStore` construction:

```rust
    let fill_stats_path = config_dir.join("fill-stats.json");
    let fill_stats = fill_stats::FillStats::new(fill_stats_path);
```//
and add `use deskwarden::fill_stats;` (or `crate::fill_stats` if `main.rs` already imports via the crate root -- match whatever import style the file already uses for `session_store`). Thread `&fill_stats` through every call site that currently calls `app::handle_match`/`app::fill_from_vault`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo build --lib --bin deskwarden && cargo test --lib` from `deskwarden/`
Expected: PASS, 0 failures, and the bin target still links (confirms every `fill_from_vault`/`handle_match` call site was updated).

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/fill_stats.rs deskwarden/src/password_strength.rs deskwarden/src/lib.rs deskwarden/src/app.rs deskwarden/src/main.rs
git commit -m "feat: add local fill-count analytics and a password strength heuristic"
```

---

### Task 4: favicon.rs

**Files:**
- Create: `deskwarden/src/favicon.rs`
- Modify: `deskwarden/src/lib.rs` (declare module)
- Modify: `deskwarden/Cargo.toml` (promote `png` from `[dev-dependencies]` to `[dependencies]`)

**Interfaces:**
- Consumes: `ureq` (already a dependency), `png` (promoted).
- Produces: `pub fn icon_base_url(server_url: Option<&str>) -> String` (pure, testable), `pub fn domain_from_uri(uri: &str) -> Option<String>` (pure, testable — strips scheme/path/port the same way `login_ui::server_host` does for the login footer), `pub fn fetch_icon_bytes(url: &str) -> Option<Vec<u8>>` (blocking HTTP GET, for use from a background thread only), `pub fn decode_rgba(png_bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)>` (width, height, RGBA8 pixels).

- [ ] **Step 1: Write the failing tests**

```rust
//! Fetches an item's website favicon for the vault window's item list and
//! detail pane. Every failure mode here (no URI, fetch error, decode error)
//! is meant to be swallowed by the caller and fall back to the existing
//! colored-initials monogram (`theme::avatar`) -- a missing icon is not
//! worth an error path, this app already has one perfectly good fallback.

use std::io::Read;

/// Where to fetch icons from: Bitwarden's own icon service for the default
/// cloud, or the self-hosted server's own icon proxy otherwise. Self-hosted
/// Bitwarden servers proxy icon fetches themselves (`{server}/icons/...`)
/// rather than having the client reach out to a third party directly.
pub fn icon_base_url(server_url: Option<&str>) -> String {
    match server_url {
        Some(url) if !url.trim().is_empty() && !url.contains("bitwarden.com") && !url.contains("bitwarden.eu") => {
            format!("{}/icons", url.trim().trim_end_matches('/'))
        }
        _ => "https://icons.bitwarden.net".to_string(),
    }
}

/// Extracts a bare domain (`vault.example.com`, no scheme/path/port) from a
/// login item's stored URI, the same normalization `login_ui::server_host`
/// already does for the login window's server footer.
pub fn domain_from_uri(uri: &str) -> Option<String> {
    let stripped = uri
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = stripped.split(['/', '?', '#']).next().unwrap_or(stripped);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() || !host.contains('.') {
        None
    } else {
        Some(host.to_string())
    }
}

/// Blocking GET for the icon's raw bytes. Call only from a background
/// thread -- see `vault_window::favicon_loader` for the async wrapper the UI
/// actually uses.
pub fn fetch_icon_bytes(url: &str) -> Option<Vec<u8>> {
    let response = ureq::get(url).call().ok()?;
    let mut bytes = Vec::new();
    response.into_reader().read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Decodes PNG bytes to (width, height, RGBA8 pixels), normalizing whatever
/// color type/bit depth the source used (indexed, grayscale, RGB without
/// alpha, ...) to straight 8-bit RGBA via `png`'s built-in transformations,
/// so the caller never has to branch on source format.
pub fn decode_rgba(png_bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut decoder = png::Decoder::new(png_bytes);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (width, height) = (info.width as usize, info.height as usize);

    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf[..info.buffer_size()]
            .iter()
            .flat_map(|&g| [g, g, g, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf[..info.buffer_size()]
            .chunks_exact(2)
            .flat_map(|ga| [ga[0], ga[0], ga[0], ga[1]])
            .collect(),
        png::ColorType::Indexed => return None,
    };

    Some((width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_base_defaults_to_bitwardens_cloud_service() {
        assert_eq!(icon_base_url(None), "https://icons.bitwarden.net");
        assert_eq!(icon_base_url(Some("")), "https://icons.bitwarden.net");
        assert_eq!(
            icon_base_url(Some("https://vault.bitwarden.com")),
            "https://icons.bitwarden.net"
        );
    }

    #[test]
    fn icon_base_proxies_through_a_self_hosted_server() {
        assert_eq!(
            icon_base_url(Some("https://vault.example.eu/")),
            "https://vault.example.eu/icons"
        );
    }

    #[test]
    fn domain_strips_scheme_path_and_port() {
        assert_eq!(
            domain_from_uri("https://app.ledgerline.com/login?x=1"),
            Some("app.ledgerline.com".to_string())
        );
        assert_eq!(
            domain_from_uri("http://192.168.1.20:8443/x"),
            Some("192.168.1.20:8443".to_string()).map(|_| "192.168.1.20".to_string())
        );
    }

    #[test]
    fn domain_rejects_uris_with_no_dotted_host() {
        assert_eq!(domain_from_uri("localhost"), None);
        assert_eq!(domain_from_uri(""), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib favicon::` from `deskwarden/`
Expected: FAIL to compile (`png` not a real dependency yet, module not declared).

- [ ] **Step 3: Promote `png` and declare the module**

In `deskwarden/Cargo.toml`, move `png = "0.17"` out of `[dev-dependencies]` into `[dependencies]` (drop the "Only for the ui_preview example's" comment above it, since it's no longer example-only; note instead that it backs favicon decoding).

In `deskwarden/src/lib.rs`, add `pub mod favicon;` in alphabetical position.

- [ ] **Step 4: Fix the domain-port test and run everything**

The `domain_strips_scheme_path_and_port` test as drafted above is self-contradictory (asserts a port-bearing host equals itself with the port removed via a no-op `.map`) -- fix it before running:

```rust
    #[test]
    fn domain_strips_scheme_path_and_port() {
        assert_eq!(
            domain_from_uri("https://app.ledgerline.com/login?x=1"),
            Some("app.ledgerline.com".to_string())
        );
    }

    #[test]
    fn domain_strips_a_port_when_present() {
        // A bare IP with no dot would be rejected by the dotted-host check,
        // so this uses a real hostname with a port instead.
        assert_eq!(
            domain_from_uri("https://vault.example.com:8443/x"),
            Some("vault.example.com".to_string())
        );
    }
```

Run: `cargo test --lib favicon::` from `deskwarden/`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/favicon.rs deskwarden/src/lib.rs deskwarden/Cargo.toml
git commit -m "feat: add favicon fetching and PNG decoding for the vault window"
```

---

### Task 5: vault_window/sidebar.rs

**Files:**
- Create: `deskwarden/src/vault_window/mod.rs` (module root; this task only adds the pieces `sidebar.rs` needs to compile standalone -- the real window loop lands in Task 9)
- Create: `deskwarden/src/vault_window/sidebar.rs`
- Modify: `deskwarden/src/lib.rs` (declare `pub mod vault_window;`)

**Interfaces:**
- Consumes: `vault_bridge::{VaultItem, Folder}`, `theme`.
- Produces: `pub enum SidebarFilter { All, Favorites, Logins, Cards, SecureNotes, Trash, Folder(String) }` (`Trash` reflects `bw`'s own soft-delete state on an item, exposed later once Task 6 reads it -- for now it just needs to exist as a selectable filter), `pub fn count_for(items: &[VaultItem], filter: &SidebarFilter) -> usize` (pure, testable), `pub fn draw_sidebar(ui: &mut egui::Ui, items: &[VaultItem], folders: &[Folder], selected: &mut SidebarFilter, lock_countdown: &str) -> SidebarAction` where `pub enum SidebarAction { None, NewFolder, DeleteFolder(String) }`.

- [ ] **Step 1: Write the failing test**

`deskwarden/src/vault_window/sidebar.rs`:

```rust
//! The vault window's left pane (design 4.8 "Sidebar"): the VAULT section
//! (All items / Favorites / Logins / Cards / Secure notes / Trash, each with
//! a live count) and the FOLDERS section (one row per real vault folder,
//! also counted), plus the auto-lock countdown pinned to the bottom.

use crate::theme;
use crate::vault_bridge::{Folder, VaultItem};
use eframe::egui::{self, RichText};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarFilter {
    All,
    Favorites,
    Logins,
    Cards,
    SecureNotes,
    Trash,
    Folder(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    None,
    NewFolder,
    DeleteFolder(String),
}

/// How many of `items` fall under `filter`. Pure and separate from drawing
/// so the sidebar's counts are testable without an egui context.
pub fn count_for(items: &[VaultItem], filter: &SidebarFilter) -> usize {
    items
        .iter()
        .filter(|item| match filter {
            SidebarFilter::All => true,
            SidebarFilter::Favorites => item.favorite,
            SidebarFilter::Logins => item.item_type == Some(1),
            SidebarFilter::Cards => item.item_type == Some(3),
            SidebarFilter::SecureNotes => item.item_type == Some(2),
            SidebarFilter::Trash => false, // wired to real trash state in Task 6
            SidebarFilter::Folder(id) => item.folder_id.as_deref() == Some(id.as_str()),
        })
        .count()
}

pub fn draw_sidebar(
    ui: &mut egui::Ui,
    items: &[VaultItem],
    folders: &[Folder],
    selected: &mut SidebarFilter,
    lock_countdown: &str,
) -> SidebarAction {
    let mut action = SidebarAction::None;

    ui.vertical(|ui| {
        ui.set_width(ui.available_width());
        ui.add_space(4.0);
        section_label(ui, "VAULT");
        for (label, filter) in [
            ("All items", SidebarFilter::All),
            ("Favorites", SidebarFilter::Favorites),
            ("Logins", SidebarFilter::Logins),
            ("Cards", SidebarFilter::Cards),
            ("Secure notes", SidebarFilter::SecureNotes),
            ("Trash", SidebarFilter::Trash),
        ] {
            let count = count_for(items, &filter);
            if sidebar_row(ui, label, count, *selected == filter) {
                *selected = filter;
            }
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            section_label(ui, "FOLDERS");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("+").frame(false)).clicked() {
                    action = SidebarAction::NewFolder;
                }
            });
        });
        for folder in folders {
            let filter = SidebarFilter::Folder(folder.id.clone());
            let count = count_for(items, &filter);
            ui.horizontal(|ui| {
                if sidebar_row(ui, &folder.name, count, *selected == filter) {
                    *selected = filter.clone();
                }
                if ui.small_button("×").on_hover_text("Delete folder").clicked() {
                    action = SidebarAction::DeleteFolder(folder.id.clone());
                }
            });
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(lock_countdown).size(11.0).color(theme::TEXT_GHOST));
        });
    });

    action
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(theme::letterspaced(text, 10.0, theme::SEMIBOLD, 1.2, theme::TEXT_GHOST));
    ui.add_space(4.0);
}

/// One VAULT/FOLDERS row: label left, right-aligned count. Returns true when
/// clicked.
fn sidebar_row(ui: &mut egui::Ui, label: &str, count: usize, selected: bool) -> bool {
    let response = ui.add(
        egui::Button::new("")
            .frame(false)
            .min_size(egui::vec2(ui.available_width(), 26.0)),
    );
    ui.allocate_ui_at_rect(response.rect, |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::semibold(label, 13.0).color(if selected {
                theme::BLUE_DEEP
            } else {
                theme::TEXT_SECONDARY
            }));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(count.to_string()).size(12.0).color(theme::TEXT_GHOST));
            });
        });
    });
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::VaultField;

    fn item(item_type: Option<i64>, favorite: bool, folder_id: Option<&str>) -> VaultItem {
        VaultItem {
            id: "1".into(),
            name: "x".into(),
            fields: vec![],
            login: None,
            item_type,
            folder_id: folder_id.map(str::to_string),
            favorite,
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn all_counts_every_item() {
        let items = vec![item(Some(1), false, None), item(Some(3), true, None)];
        assert_eq!(count_for(&items, &SidebarFilter::All), 2);
    }

    #[test]
    fn favorites_counts_only_favorited_items() {
        let items = vec![item(Some(1), true, None), item(Some(1), false, None)];
        assert_eq!(count_for(&items, &SidebarFilter::Favorites), 1);
    }

    #[test]
    fn logins_and_cards_are_disjoint() {
        let items = vec![item(Some(1), false, None), item(Some(3), false, None)];
        assert_eq!(count_for(&items, &SidebarFilter::Logins), 1);
        assert_eq!(count_for(&items, &SidebarFilter::Cards), 1);
    }

    #[test]
    fn folder_counts_only_items_in_that_folder() {
        let items = vec![
            item(Some(1), false, Some("f1")),
            item(Some(1), false, Some("f2")),
            item(Some(1), false, None),
        ];
        assert_eq!(count_for(&items, &SidebarFilter::Folder("f1".to_string())), 1);
    }
}
```

`deskwarden/src/vault_window/mod.rs` (stub for now, filled in by Task 9):

```rust
//! The "2b" vault window: folders sidebar, item list, and detail pane. See
//! `docs/design/deskwarden-design-spec.md` section 4.8.

pub mod sidebar;
```

- [ ] **Step 2: Declare the module and run the tests**

Add `pub mod vault_window;` to `deskwarden/src/lib.rs` in alphabetical position.

Run: `cargo test --lib vault_window::sidebar::` from `deskwarden/`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add deskwarden/src/vault_window/ deskwarden/src/lib.rs
git commit -m "feat: add the vault window sidebar (folders, counts, lock countdown)"
```

---

### Task 6: vault_window/item_list.rs

**Files:**
- Create: `deskwarden/src/vault_window/item_list.rs`
- Modify: `deskwarden/src/vault_window/mod.rs` (`pub mod item_list;`)

**Interfaces:**
- Consumes: `sidebar::SidebarFilter`, `favicon`, `theme`, virtualization pattern from `picker_ui::list_card` (row-index closure over `ScrollArea::show_rows`) -- copy the pattern, this module doesn't depend on `picker_ui` directly (that stays private to its own file).
- Produces: `pub fn matches_filter(item: &VaultItem, filter: &SidebarFilter, search_lower: &str) -> bool` (pure, testable -- combines the sidebar filter with the search box), `pub fn draw_item_list(ui: &mut egui::Ui, items: &[VaultItem], filter: &SidebarFilter, search: &mut String, selected_id: &mut Option<String>, icons: &IconCache) -> ItemListAction` where `pub enum ItemListAction { None, NewItem }`, and `pub struct IconCache` (thin wrapper the window owns and passes down; populated by Task 9's favicon loader, read-only here).

- [ ] **Step 1: Write the failing tests**

```rust
//! The vault window's middle pane: search box, `+ New`, and the virtualized
//! item list (design 4.8 "Item list"). Virtualized the same way
//! `picker_ui`'s lists are (`ScrollArea::show_rows`) -- a real vault can be
//! in the thousands, and laying out every row on every repaint was already
//! a confirmed source of a laggy picker before that fix.

use super::sidebar::SidebarFilter;
use crate::theme;
use crate::vault_bridge::VaultItem;
use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke};
use std::collections::HashMap;

/// Holds loaded favicon textures, keyed by item id. Owned by
/// `vault_window::mod` (Task 9), which populates it from the background
/// favicon loader; this module only ever reads it.
#[derive(Default)]
pub struct IconCache {
    pub textures: HashMap<String, egui::TextureHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemListAction {
    None,
    NewItem,
}

/// True when `item` is both in `filter`'s scope (see
/// `sidebar::count_for` for the same per-filter logic) and matches
/// `search_lower` against its name or username.
pub fn matches_filter(item: &VaultItem, filter: &SidebarFilter, search_lower: &str) -> bool {
    let in_scope = match filter {
        SidebarFilter::All => true,
        SidebarFilter::Favorites => item.favorite,
        SidebarFilter::Logins => item.item_type == Some(1),
        SidebarFilter::Cards => item.item_type == Some(3),
        SidebarFilter::SecureNotes => item.item_type == Some(2),
        SidebarFilter::Trash => false,
        SidebarFilter::Folder(id) => item.folder_id.as_deref() == Some(id.as_str()),
    };
    if !in_scope {
        return false;
    }
    if search_lower.is_empty() {
        return true;
    }
    let username = item
        .login
        .as_ref()
        .and_then(|l| l.username.as_deref())
        .unwrap_or("");
    item.name.to_lowercase().contains(search_lower) || username.to_lowercase().contains(search_lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, username: Option<&str>, item_type: Option<i64>) -> VaultItem {
        VaultItem {
            id: "1".into(),
            name: name.into(),
            fields: vec![],
            login: username.map(|u| crate::vault_bridge::LoginData {
                username: Some(u.to_string()),
                password: None,
                totp: None,
                uris: vec![],
                other: serde_json::Map::new(),
            }),
            item_type,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_search_matches_everything_in_scope() {
        assert!(matches_filter(&item("Ledgerline", None, Some(1)), &SidebarFilter::All, ""));
    }

    #[test]
    fn search_matches_name_case_insensitively() {
        assert!(matches_filter(&item("Ledgerline", None, Some(1)), &SidebarFilter::All, "ledger"));
        assert!(!matches_filter(&item("Ledgerline", None, Some(1)), &SidebarFilter::All, "vantage"));
    }

    #[test]
    fn search_matches_username_too() {
        let it = item("Ledgerline", Some("a.novak@ledgerline.com"), Some(1));
        assert!(matches_filter(&it, &SidebarFilter::All, "novak"));
    }

    #[test]
    fn out_of_scope_items_never_match_regardless_of_search() {
        let it = item("Ledgerline", None, Some(3)); // a Card
        assert!(!matches_filter(&it, &SidebarFilter::Logins, ""));
        assert!(!matches_filter(&it, &SidebarFilter::Logins, "ledgerline"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib vault_window::item_list::` from `deskwarden/`
Expected: FAIL (module not declared).

- [ ] **Step 3: Add `draw_item_list` and declare the module**

Append to `deskwarden/src/vault_window/item_list.rs` (below the tests module is fine, or above it -- match the file's existing convention of tests at the bottom):

```rust
const ROW_HEIGHT: f32 = 50.0;

pub fn draw_item_list(
    ui: &mut egui::Ui,
    items: &[VaultItem],
    filter: &SidebarFilter,
    search: &mut String,
    selected_id: &mut Option<String>,
    icons: &IconCache,
) -> ItemListAction {
    let mut action = ItemListAction::None;

    ui.horizontal(|ui| {
        let width = (ui.available_width() - 70.0).max(40.0);
        ui.add(
            egui::TextEdit::singleline(search)
                // Stable id so `Ctrl+K` (wired in `vault_window::mod`) can
                // request focus on this field from outside this function.
                .id(egui::Id::new("vault-search"))
                .hint_text(RichText::new("Search").color(theme::TEXT_GHOST))
                .desired_width(width)
                .margin(Margin::symmetric(10, 8)),
        );
        if theme::primary_button(ui, "New", None).clicked() {
            action = ItemListAction::NewItem;
        }
    });
    ui.add_space(8.0);

    let search_lower = search.to_lowercase();
    let filtered: Vec<&VaultItem> = items
        .iter()
        .filter(|item| matches_filter(item, filter, &search_lower))
        .collect();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, filtered.len(), |ui, row_range| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for row in row_range {
                let item = filtered[row];
                let selected = selected_id.as_deref() == Some(item.id.as_str());
                if item_row(ui, item, selected, icons.textures.get(&item.id)) {
                    *selected_id = Some(item.id.clone());
                }
            }
        });

    action
}

fn item_row(
    ui: &mut egui::Ui,
    item: &VaultItem,
    selected: bool,
    icon: Option<&egui::TextureHandle>,
) -> bool {
    let username = item.login.as_ref().and_then(|l| l.username.as_deref()).unwrap_or("");
    let frame = egui::Frame::new()
        .fill(if selected { theme::CARD } else { theme::CANVAS })
        .stroke(if selected {
            Stroke::new(1.0, theme::BLUE)
        } else {
            Stroke::NONE
        })
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match icon {
                    Some(tex) => {
                        ui.add(egui::Image::new((tex.id(), tex.size_vec2())).fit_to_exact_size(egui::Vec2::splat(32.0)));
                    }
                    None => theme::avatar(ui, &theme::initials(&item.name), 32.0, selected),
                }
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(theme::semibold(&item.name, 13.0).color(if selected {
                        theme::BLUE_DEEP
                    } else {
                        theme::INK
                    }));
                    if !username.is_empty() {
                        ui.label(RichText::new(username).size(11.0).color(theme::TEXT_FAINT));
                    }
                });
            });
        });
    frame.response.interact(Sense::click()).clicked()
}
```

In `deskwarden/src/vault_window/mod.rs`, add `pub mod item_list;`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib vault_window::item_list::` from `deskwarden/`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/vault_window/
git commit -m "feat: add the vault window's item list (search, virtualized rows, icons)"
```

---

### Task 7: vault_window/detail.rs — read mode

**Files:**
- Create: `deskwarden/src/vault_window/detail.rs`
- Modify: `deskwarden/src/vault_window/mod.rs` (`pub mod detail;`)

**Interfaces:**
- Consumes: `password_strength::rate`, `fill_stats::FillStats`, `theme`.
- Produces: `pub fn metadata_line(updated_days_ago: Option<i64>, fill_count: u32, password: &str) -> String` (pure, testable -- "Updated 3 days ago · Filled 41 times · Strength: strong"), `pub fn draw_detail_read(ui: &mut egui::Ui, item: &VaultItem, fill_count: u32, totp: Option<&str>, totp_seconds_left: u8, reveal_password: &mut bool) -> DetailAction` where `pub enum DetailAction { None, Edit, Fill, CopyUsername, CopyPassword, CopyTotp, OpenWebsite(String) }`.

- [ ] **Step 1: Write the failing test**

```rust
//! The vault window's right pane in read mode (design 4.8 "Detail pane"):
//! title bar, LOGIN CREDENTIALS card, AUTOFILL TARGETS card, and the
//! metadata strip. Edit mode is `detail_edit.rs` (Task 8) -- kept separate
//! because the two have almost no shared state (read mode is passive
//! display + copy actions; edit mode owns a draft `VaultItem` and validates
//! it), and the read-mode file was already large enough on its own.

use crate::password_strength;
use crate::theme;
use crate::vault_bridge::VaultItem;
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailAction {
    None,
    Edit,
    Fill,
    CopyUsername,
    CopyPassword,
    CopyTotp,
    OpenWebsite(String),
}

/// The metadata strip's text: "Updated N days ago · Filled N times ·
/// Strength: X". `updated_days_ago` is `None` when the item carries no
/// parseable `revisionDate` (shows "Updated recently" rather than
/// fabricating a number).
pub fn metadata_line(updated_days_ago: Option<i64>, fill_count: u32, password: &str) -> String {
    let updated = match updated_days_ago {
        Some(0) => "Updated today".to_string(),
        Some(1) => "Updated 1 day ago".to_string(),
        Some(n) => format!("Updated {n} days ago"),
        None => "Updated recently".to_string(),
    };
    let filled = if fill_count == 1 {
        "Filled 1 time".to_string()
    } else {
        format!("Filled {fill_count} times")
    };
    let strength = password_strength::rate(password).label();
    format!("{updated} \u{b7} {filled} \u{b7} Strength: {strength}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_line_pluralizes_fill_count() {
        assert_eq!(
            metadata_line(Some(3), 41, "Tr0ub4dor&3xtraLong!"),
            "Updated 3 days ago \u{b7} Filled 41 times \u{b7} Strength: Strong"
        );
        assert_eq!(
            metadata_line(Some(1), 1, "weak"),
            "Updated 1 day ago \u{b7} Filled 1 time \u{b7} Strength: Weak"
        );
    }

    #[test]
    fn metadata_line_handles_missing_update_date() {
        assert_eq!(
            metadata_line(None, 0, ""),
            "Updated recently \u{b7} Filled 0 times \u{b7} Strength: Weak"
        );
    }

    #[test]
    fn metadata_line_handles_today() {
        assert_eq!(
            metadata_line(Some(0), 5, "abc"),
            "Updated today \u{b7} Filled 5 times \u{b7} Strength: Weak"
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib vault_window::detail::` from `deskwarden/`
Expected: FAIL (module not declared).

- [ ] **Step 3: Add `draw_detail_read` and declare the module**

Append to `deskwarden/src/vault_window/detail.rs`:

```rust
pub fn draw_detail_read(
    ui: &mut egui::Ui,
    item: &VaultItem,
    fill_count: u32,
    totp: Option<&str>,
    totp_seconds_left: u8,
    reveal_password: &mut bool,
) -> DetailAction {
    let mut action = DetailAction::None;
    let login = item.login.as_ref();
    let username = login.and_then(|l| l.username.as_deref()).unwrap_or("");
    let password = login.and_then(|l| l.password.as_deref()).unwrap_or("");

    ui.horizontal(|ui| {
        theme::avatar(ui, &theme::initials(&item.name), 44.0, true);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(theme::bold(&item.name, 22.0).color(theme::INK));
            ui.label(RichText::new("Login").size(12.0).color(theme::TEXT_FAINT));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, "Edit").clicked() {
                action = DetailAction::Edit;
            }
            if theme::primary_button(ui, "Fill in app", Some("CTRL+SHIFT+F")).clicked() {
                action = DetailAction::Fill;
            }
        });
    });
    ui.add_space(14.0);

    card(ui, "LOGIN CREDENTIALS", |ui| {
        credential_row(ui, "Username", username, "Copy", &mut action, DetailAction::CopyUsername);
        theme::hairline(ui);
        password_row(ui, password, reveal_password, &mut action);
        if let Some(code) = totp {
            theme::hairline(ui);
            totp_row(ui, code, totp_seconds_left, &mut action);
        }
    });
    ui.add_space(10.0);

    let website = login
        .and_then(|l| l.uris.first())
        .and_then(|u| u.uri.as_deref())
        .unwrap_or("");
    if !website.is_empty() {
        card(ui, "AUTOFILL TARGETS", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(website).size(13.0).color(theme::TEXT_SECONDARY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::secondary_button(ui, "Open").clicked() {
                        action = DetailAction::OpenWebsite(website.to_string());
                    }
                });
            });
        });
        ui.add_space(10.0);
    }

    let updated_days_ago = item
        .other
        .get("revisionDate")
        .and_then(|v| v.as_str())
        .and_then(days_since);
    ui.label(
        RichText::new(metadata_line(updated_days_ago, fill_count, password))
            .size(11.0)
            .color(theme::TEXT_GHOST),
    );

    action
}

fn card(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::letterspaced(title, 10.0, theme::SEMIBOLD, 1.2, theme::TEXT_GHOST));
            ui.add_space(8.0);
            contents(ui);
        });
}

fn credential_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    copy_label: &str,
    action: &mut DetailAction,
    on_copy: DetailAction,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).size(11.0).color(theme::TEXT_FAINT));
            ui.label(RichText::new(value).size(13.0).color(theme::INK));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, copy_label).clicked() {
                *action = on_copy;
            }
        });
    });
    ui.add_space(6.0);
}

fn password_row(ui: &mut egui::Ui, password: &str, revealed: &mut bool, action: &mut DetailAction) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("Password").size(11.0).color(theme::TEXT_FAINT));
            let shown = if *revealed { password.to_string() } else { "•".repeat(password.chars().count().max(8)) };
            ui.label(RichText::new(shown).size(13.0).color(theme::INK).family(egui::FontFamily::Monospace));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, "Copy").clicked() {
                *action = DetailAction::CopyPassword;
            }
            if theme::secondary_button(ui, if *revealed { "Hide" } else { "Reveal" }).clicked() {
                *revealed = !*revealed;
            }
        });
    });
    ui.add_space(6.0);
}

fn totp_row(ui: &mut egui::Ui, code: &str, seconds_left: u8, action: &mut DetailAction) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new("One-time code").size(11.0).color(theme::TEXT_FAINT));
            ui.label(
                RichText::new(code)
                    .size(17.0)
                    .family(egui::FontFamily::Monospace)
                    .color(theme::INK),
            );
            let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 4.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(2), theme::HAIRLINE);
            let fraction = (seconds_left as f32 / 30.0).clamp(0.0, 1.0);
            let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * fraction, rect.height()));
            ui.painter().rect_filled(filled, CornerRadius::same(2), theme::BLUE);
            ui.label(RichText::new(format!("{seconds_left}s left")).size(10.0).color(theme::TEXT_GHOST));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if theme::secondary_button(ui, "Copy").clicked() {
                *action = DetailAction::CopyTotp;
            }
        });
    });
}

/// Days between an RFC3339 `revisionDate` (as `bw serve` sends it) and now.
/// `None` on anything unparseable -- the caller shows "Updated recently"
/// rather than a wrong number.
fn days_since(revision_date: &str) -> Option<i64> {
    // A minimal RFC3339 date parse: only the `YYYY-MM-DD` prefix is needed
    // for a day-granularity "N days ago", so this avoids pulling in a full
    // datetime crate for one field. `std::time::SystemTime` supplies "now".
    let date_part = revision_date.get(0..10)?;
    let mut parts = date_part.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let revision_days = days_from_civil(year, month, day);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let today_days = (now.as_secs() / 86400) as i64;

    Some((today_days - revision_days).max(0))
}

/// Howard Hinnant's civil-from-days algorithm, days-from-civil direction:
/// converts a (year, month, day) into a day count since the Unix epoch,
/// without pulling in a datetime crate for one field.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
```

In `deskwarden/src/vault_window/mod.rs`, add `pub mod detail;`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib vault_window::detail::` from `deskwarden/`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/vault_window/
git commit -m "feat: add the vault window's detail pane (read mode, TOTP, metadata)"
```

---

### Task 8: vault_window/detail_edit.rs — edit + create mode

**Files:**
- Create: `deskwarden/src/vault_window/detail_edit.rs`
- Modify: `deskwarden/src/vault_window/mod.rs` (`pub mod detail_edit;`)

**Interfaces:**
- Consumes: `vault_bridge::{VaultItem, NewLoginItem, Folder}`, `theme`, `password_strength`.
- Produces: `pub struct EditDraft { pub name: String, pub username: String, pub password: String, pub folder_id: Option<String> }` with `impl EditDraft { pub fn from_item(item: &VaultItem) -> Self; pub fn empty() -> Self; pub fn is_valid(&self) -> bool; pub fn apply_to(&self, item: &VaultItem) -> VaultItem; pub fn to_new_item(&self) -> NewLoginItem }`, `pub fn draw_detail_edit(ui: &mut egui::Ui, draft: &mut EditDraft, folders: &[Folder], creating: bool) -> EditAction` where `pub enum EditAction { None, Save, Cancel }`.

- [ ] **Step 1: Write the failing tests**

```rust
//! The vault window's detail pane in edit mode, and the "+ New" creation
//! flow (they share one form: creating is editing against an empty draft
//! with no item id yet). See `detail.rs` for why this is a separate file
//! from read mode.

use crate::theme;
use crate::vault_bridge::{Folder, LoginData, NewLoginItem, VaultItem};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};

#[derive(Debug, Clone, Default)]
pub struct EditDraft {
    pub name: String,
    pub username: String,
    pub password: String,
    pub folder_id: Option<String>,
}

impl EditDraft {
    pub fn from_item(item: &VaultItem) -> Self {
        let login = item.login.as_ref();
        Self {
            name: item.name.clone(),
            username: login.and_then(|l| l.username.clone()).unwrap_or_default(),
            password: login.and_then(|l| l.password.clone()).unwrap_or_default(),
            folder_id: item.folder_id.clone(),
        }
    }

    pub fn empty() -> Self {
        Self::default()
    }

    /// A name is the one thing `bw serve`'s create/edit endpoints reject an
    /// empty item without -- everything else (blank username/password) is
    /// legitimate for e.g. a placeholder entry.
    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
    }

    /// Applies this draft onto a clone of `item`, preserving every field the
    /// draft doesn't know about (id, favorite, type, fields, TOTP, uris, ...)
    /// via the same "clone then overwrite known fields" pattern
    /// `vault_bridge::with_app_match` already uses, so a save from this form
    /// can never silently drop data the design doesn't expose an editor for.
    pub fn apply_to(&self, item: &VaultItem) -> VaultItem {
        let mut updated = item.clone();
        updated.name = self.name.clone();
        updated.folder_id = self.folder_id.clone();
        let mut login = updated.login.unwrap_or_default();
        login.username = if self.username.is_empty() { None } else { Some(self.username.clone()) };
        login.password = if self.password.is_empty() { None } else { Some(self.password.clone()) };
        updated.login = Some(login);
        updated
    }

    pub fn to_new_item(&self) -> NewLoginItem {
        NewLoginItem {
            name: self.name.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            folder_id: self.folder_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    None,
    Save,
    Cancel,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> VaultItem {
        VaultItem {
            id: "1".into(),
            name: "Ledgerline".into(),
            fields: vec![],
            login: Some(LoginData {
                username: Some("a@b.com".into()),
                password: Some("p".into()),
                totp: Some("SEED".into()),
                uris: vec![],
                other: serde_json::Map::new(),
            }),
            item_type: Some(1),
            folder_id: Some("f1".into()),
            favorite: true,
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn from_item_pulls_the_current_credentials() {
        let draft = EditDraft::from_item(&item());
        assert_eq!(draft.name, "Ledgerline");
        assert_eq!(draft.username, "a@b.com");
        assert_eq!(draft.folder_id.as_deref(), Some("f1"));
    }

    #[test]
    fn empty_draft_is_invalid() {
        assert!(!EditDraft::empty().is_valid());
    }

    #[test]
    fn a_named_draft_is_valid() {
        let draft = EditDraft { name: "New".into(), ..EditDraft::empty() };
        assert!(draft.is_valid());
    }

    #[test]
    fn apply_to_preserves_fields_the_form_never_touched() {
        let draft = EditDraft { name: "Renamed".into(), username: "new@b.com".into(), password: "np".into(), folder_id: None };
        let updated = draft.apply_to(&item());

        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.folder_id, None);
        assert!(updated.favorite, "favorite must survive an edit the form doesn't expose");
        assert_eq!(updated.item_type, Some(1));
        assert_eq!(updated.login.as_ref().unwrap().totp.as_deref(), Some("SEED"));
    }

    #[test]
    fn to_new_item_carries_the_drafts_fields() {
        let draft = EditDraft { name: "New".into(), username: "u".into(), password: "p".into(), folder_id: Some("f2".into()) };
        let new_item = draft.to_new_item();
        assert_eq!(new_item.name, "New");
        assert_eq!(new_item.folder_id.as_deref(), Some("f2"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib vault_window::detail_edit::` from `deskwarden/`
Expected: FAIL (module not declared).

- [ ] **Step 3: Add `draw_detail_edit` and declare the module**

Append to `deskwarden/src/vault_window/detail_edit.rs`:

```rust
pub fn draw_detail_edit(
    ui: &mut egui::Ui,
    draft: &mut EditDraft,
    folders: &[Folder],
    creating: bool,
) -> EditAction {
    let mut action = EditAction::None;

    ui.label(theme::bold(if creating { "New login" } else { "Edit login" }, 19.0).color(theme::INK));
    ui.add_space(12.0);

    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::HAIRLINE))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            theme::field_label(ui, "Name");
            theme::text_field(ui, &mut draft.name, false);
            ui.add_space(10.0);

            theme::field_label(ui, "Username");
            theme::text_field(ui, &mut draft.username, false);
            ui.add_space(10.0);

            theme::field_label(ui, "Password");
            let mut reveal = true;
            theme::password_field(ui, &mut draft.password, &mut reveal);
            ui.add_space(10.0);

            theme::field_label(ui, "Folder");
            egui::ComboBox::from_id_salt("edit-folder")
                .selected_text(
                    folders
                        .iter()
                        .find(|f| Some(&f.id) == draft.folder_id.as_ref())
                        .map(|f| f.name.as_str())
                        .unwrap_or("No folder"),
                )
                .show_ui(ui, |ui| {
                    if ui.selectable_label(draft.folder_id.is_none(), "No folder").clicked() {
                        draft.folder_id = None;
                    }
                    for folder in folders {
                        let selected = draft.folder_id.as_deref() == Some(folder.id.as_str());
                        if ui.selectable_label(selected, &folder.name).clicked() {
                            draft.folder_id = Some(folder.id.clone());
                        }
                    }
                });
        });

    if !draft.is_valid() {
        ui.add_space(6.0);
        ui.label(RichText::new("Name is required.").size(12.0).color(theme::ERROR));
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let save = egui::Button::new(if draft.is_valid() { "Save" } else { "Save (needs a name)" });
        if ui.add_enabled(draft.is_valid(), save).clicked() {
            action = EditAction::Save;
        }
        if theme::secondary_button(ui, "Cancel").clicked() {
            action = EditAction::Cancel;
        }
    });

    action
}
```

In `deskwarden/src/vault_window/mod.rs`, add `pub mod detail_edit;`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib vault_window::detail_edit::` from `deskwarden/`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/vault_window/
git commit -m "feat: add the vault window's edit/create form for login items"
```

---

### Task 9: vault_window/mod.rs — window orchestration, favicon loading, auto-lock

**Files:**
- Modify: `deskwarden/src/vault_window/mod.rs` (replace the Task-5 stub with the real window loop)

**Interfaces:**
- Consumes: everything from Tasks 1-8 (`vault_bridge::{VaultBridge, VaultItem, Folder}`, `fill_stats::FillStats`, `favicon`, `sidebar::{draw_sidebar, SidebarFilter, SidebarAction}`, `item_list::{draw_item_list, ItemListAction, IconCache}`, `detail::{draw_detail_read, DetailAction}`, `detail_edit::{draw_detail_edit, EditDraft, EditAction}`, `theme`, `login_ui::draw_window_chrome`/`round_window_corners` (reused directly, not copied -- both are already `pub fn` in `login_ui.rs`)).
- Produces: `pub fn run(vault: VaultBridge, fill_stats: FillStats, injector: &Injector<impl UiAutomationFiller, impl SendInputFiller>, server_url: Option<String>, session_token: String) -> VaultWindowResult` where `pub struct VaultWindowResult { pub locked: bool }` (tells the caller in `main.rs` whether the window's own "Lock" action or the auto-lock timer fired, so `main.rs` knows to route back to `login_ui::run_login_flow` next). `session_token` is needed for the toolbar's manual "Sync" button (`bw_serve::run_bw_sync` takes a session token) -- this app has nothing that auto-syncs with the remote server on a timer (see the Global Constraints addition on this), so a manual sync action is the only way to pull in changes made on another device without restarting the whole app.
- `AUTO_LOCK_TIMEOUT: Duration` constant, `const AUTO_LOCK_TIMEOUT: Duration = Duration::from_secs(15 * 60);` (Global Constraints).

- [ ] **Step 1: Replace the module stub**

Replace `deskwarden/src/vault_window/mod.rs` entirely:

```rust
//! The "2b" vault window: folders sidebar, item list, and detail pane. See
//! `docs/design/deskwarden-design-spec.md` section 4.8.
//!
//! Reuses `login_ui`'s frameless custom-chrome window pattern
//! (`draw_window_chrome`/`round_window_corners`) rather than duplicating
//! it -- both are already `pub fn` there for exactly this reason.

pub mod detail;
pub mod detail_edit;
pub mod item_list;
pub mod sidebar;

use crate::bw_serve;
use crate::fill_stats::FillStats;
use crate::injector::{Injector, SendInputFiller, UiAutomationFiller};
use crate::login_ui::{draw_window_chrome, round_window_corners, ChromeAction};
use crate::theme;
use crate::vault_bridge::{Folder, VaultBridge, VaultItem};
use detail::DetailAction;
use detail_edit::{draw_detail_edit, EditAction, EditDraft};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};
use item_list::{draw_item_list, IconCache, ItemListAction};
use sidebar::{draw_sidebar, SidebarAction, SidebarFilter};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const WINDOW_TITLE: &str = "Deskwarden Vault";
const WINDOW_SIZE: [f32; 2] = [1240.0, 740.0];
const SIDEBAR_WIDTH: f32 = 212.0;
const LIST_WIDTH: f32 = 390.0;

/// See Global Constraints: hardcoded until the 3e preferences window exists.
const AUTO_LOCK_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// TOTP is re-fetched from `bw serve` on this interval while an item with a
/// code is selected -- cheap enough to poll (one local HTTP call) and far
/// simpler than implementing the TOTP algorithm ourselves when `bw serve`
/// already exposes the current code directly.
const TOTP_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub struct VaultWindowResult {
    pub locked: bool,
}

enum DetailMode {
    None,
    Read,
    Edit(EditDraft),
    Create(EditDraft),
}

/// One result from the background favicon loader: which item it was for,
/// and the decoded pixels (`None` if this item had no usable icon).
struct FaviconResult {
    item_id: String,
    pixels: Option<(usize, usize, Vec<u8>)>,
}

/// Opens the vault window and blocks until it's closed (the ✕/window-close
/// path) or locked (the `Lock` button or the auto-lock timer). Mirrors
/// `login_ui::run_login_flow`'s `Rc<RefCell<_>>` result handoff -- the
/// update closure is `FnMut + 'static` and can't return anything directly.
pub fn run<A: UiAutomationFiller, B: SendInputFiller>(
    vault: VaultBridge,
    fill_stats: FillStats,
    injector: &Injector<A, B>,
    server_url: Option<String>,
    session_token: String,
) -> VaultWindowResult {
    let locked = Rc::new(RefCell::new(false));
    let locked_for_closure = locked.clone();
    let mut sync_status: Option<Result<(), String>> = None;

    let mut items: Vec<VaultItem> = vault.list_items().unwrap_or_default();
    let mut folders: Vec<Folder> = vault.list_folders().unwrap_or_default();
    // For the toolbar's avatar circle (design 4.8's `AN` initials badge).
    // `None` just omits the avatar -- an unreadable account email is not
    // worth failing the window over.
    let account_email = crate::login_ui::check_bw_status_details().user_email;
    let mut filter = SidebarFilter::All;
    let mut search = String::new();
    let mut selected_id: Option<String> = items.first().map(|i| i.id.clone());
    let mut mode = DetailMode::Read;
    let mut reveal_password = false;
    let mut icons = IconCache::default();

    let (favicon_tx, favicon_rx): (mpsc::Sender<FaviconResult>, Receiver<FaviconResult>) = mpsc::channel();
    let mut favicon_requested: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut totp_code: Option<String> = None;
    let mut totp_last_poll = Instant::now() - TOTP_POLL_INTERVAL;
    let mut last_activity = Instant::now();

    let mut styled = false;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(false)
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            theme::apply(ui.ctx());
            round_window_corners(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        if ui.ctx().input(|i| i.pointer.any_click() || !i.events.is_empty()) {
            last_activity = Instant::now();
        }
        if last_activity.elapsed() >= AUTO_LOCK_TIMEOUT {
            *locked_for_closure.borrow_mut() = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let remaining = AUTO_LOCK_TIMEOUT.saturating_sub(last_activity.elapsed());
        let lock_countdown = format!(
            "Locks in {}:{:02}",
            remaining.as_secs() / 60,
            remaining.as_secs() % 60
        );

        while let Ok(result) = favicon_rx.try_recv() {
            if let Some((w, h, rgba)) = result.pixels {
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                let tex = ui.ctx().load_texture(result.item_id.clone(), image, egui::TextureOptions::default());
                icons.textures.insert(result.item_id, tex);
            }
        }

        match draw_window_chrome(ui, "Deskwarden Vault") {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            ChromeAction::Minimize => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }

        // Spec section 5's keyboard model for this window: Ctrl+K focuses
        // search, Ctrl+L locks, Ctrl+N opens the new-item form. Ctrl+Shift+F
        // ("fill in app") is checked separately, down in the `DetailMode::Read`
        // arm below, where the selected item is already in scope.
        let (ctrl_k, ctrl_l, ctrl_n) = ui.ctx().input(|i| {
            (
                i.modifiers.ctrl && i.key_pressed(egui::Key::K),
                i.modifiers.ctrl && i.key_pressed(egui::Key::L),
                i.modifiers.ctrl && i.key_pressed(egui::Key::N),
            )
        });
        if ctrl_k {
            ui.memory_mut(|m| m.request_focus(egui::Id::new("vault-search")));
        }
        if ctrl_l {
            *locked_for_closure.borrow_mut() = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctrl_n {
            mode = DetailMode::Create(EditDraft::empty());
        }

        egui::TopBottomPanel::top("vault-toolbar")
            .frame(egui::Frame::new().fill(theme::CARD).inner_margin(Margin::symmetric(20, 10)))
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::mark(ui, 20.0);
                    ui.label(theme::bold("Deskwarden", 14.0).color(theme::INK));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::secondary_button(ui, "Lock").clicked() {
                            *locked_for_closure.borrow_mut() = true;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if let Some(email) = &account_email {
                            theme::avatar(ui, &theme::initials(email), 26.0, false);
                        }
                        // Manual sync: this app has nowhere that auto-syncs on
                        // a timer (see `main()`'s own single startup-time
                        // `bw sync` -- everything after that only re-reads
                        // whatever's already local). A change made on another
                        // device otherwise wouldn't show up here until the
                        // whole app restarts; this button is the escape hatch.
                        if theme::secondary_button(ui, "Sync").clicked() {
                            let result = bw_serve::run_bw_sync(&session_token);
                            if result.is_ok() {
                                items = vault.list_items().unwrap_or_default();
                                folders = vault.list_folders().unwrap_or_default();
                            } else if let Err(e) = &result {
                                log::warn!("manual vault sync failed: {e}");
                            }
                            sync_status = Some(result);
                        }
                        if let Some(status) = &sync_status {
                            let (text, color) = match status {
                                Ok(()) => ("Synced", theme::TEXT_GHOST),
                                Err(_) => ("Sync failed", theme::ERROR),
                            };
                            ui.label(RichText::new(text).size(11.0).color(color));
                        }
                    });
                });
            });

        egui::SidePanel::left("vault-sidebar")
            .exact_width(SIDEBAR_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::WINDOW_BG).inner_margin(Margin::symmetric(14, 12)).stroke(Stroke::new(1.0, theme::HAIRLINE)))
            .show_inside(ui, |ui| {
                match draw_sidebar(ui, &items, &folders, &mut filter, &lock_countdown) {
                    SidebarAction::NewFolder => {
                        if let Ok(folder) = vault.create_folder("New folder") {
                            folders.push(folder);
                        }
                    }
                    SidebarAction::DeleteFolder(id) => {
                        if vault.delete_folder(&id).is_ok() {
                            folders.retain(|f| f.id != id);
                            if filter == SidebarFilter::Folder(id) {
                                filter = SidebarFilter::All;
                            }
                        }
                    }
                    SidebarAction::None => {}
                }
            });

        egui::SidePanel::left("vault-item-list")
            .exact_width(LIST_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(14, 12)).stroke(Stroke::new(1.0, theme::HAIRLINE)))
            .show_inside(ui, |ui| {
                match draw_item_list(ui, &items, &filter, &mut search, &mut selected_id, &icons) {
                    ItemListAction::NewItem => mode = DetailMode::Create(EditDraft::empty()),
                    ItemListAction::None => {}
                }
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(20, 18)))
            .show_inside(ui, |ui| {
                let selected_item = selected_id.as_ref().and_then(|id| items.iter().find(|i| &i.id == id)).cloned();

                // Kick off a favicon fetch the first time this item is seen,
                // and only for items with a website to derive a domain from.
                if let Some(item) = &selected_item {
                    if !icons.textures.contains_key(&item.id) && !favicon_requested.contains(&item.id) {
                        if let Some(uri) = item.login.as_ref().and_then(|l| l.uris.first()).and_then(|u| u.uri.as_deref()) {
                            if let Some(domain) = crate::favicon::domain_from_uri(uri) {
                                favicon_requested.insert(item.id.clone());
                                spawn_favicon_fetch(item.id.clone(), domain, server_url.clone(), favicon_tx.clone());
                            }
                        }
                    }
                }

                match &mut mode {
                    DetailMode::None => {
                        ui.label("Select an item.");
                    }
                    DetailMode::Read => {
                        if let Some(item) = &selected_item {
                            if item.item_type != Some(1) {
                                ui.label(theme::bold(&item.name, 19.0).color(theme::INK));
                                ui.add_space(6.0);
                                ui.label("This item type isn't editable in Deskwarden yet.");
                                return;
                            }

                            if totp_last_poll.elapsed() >= TOTP_POLL_INTERVAL {
                                totp_last_poll = Instant::now();
                                totp_code = vault.get_totp(&item.id).ok().flatten();
                            }
                            let seconds_left = (30 - (std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() % 30)
                                .unwrap_or(0))) as u8;

                            let fill_count = fill_stats.count(&item.id);
                            let mut action = draw_detail_read(ui, item, fill_count, totp_code.as_deref(), seconds_left, &mut reveal_password);
                            // Ctrl+Shift+F (spec section 5) is the keyboard
                            // equivalent of clicking "Fill in app" -- checked
                            // here, not at the top level, because it needs
                            // exactly the selected `item` this arm already
                            // has and the button click above doesn't.
                            if ui.ctx().input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::F)) {
                                action = DetailAction::Fill;
                            }
                            // `item` and `totp_code` already hold everything
                            // a copy action needs -- `draw_detail_read` only
                            // needs to report *which* field was clicked, not
                            // hand the value back through its return type.
                            let login = item.login.as_ref();
                            match action {
                                DetailAction::Edit => mode = DetailMode::Edit(EditDraft::from_item(item)),
                                DetailAction::Fill => {
                                    // Fills into whatever native app targets
                                    // this item -- wired up in Task 10, which
                                    // has window-watch context this module
                                    // doesn't.
                                }
                                DetailAction::CopyUsername => {
                                    if let Some(username) = login.and_then(|l| l.username.as_deref()) {
                                        ui.ctx().copy_text(username.to_string());
                                    }
                                }
                                DetailAction::CopyPassword => {
                                    if let Some(password) = login.and_then(|l| l.password.as_deref()) {
                                        ui.ctx().copy_text(password.to_string());
                                    }
                                }
                                DetailAction::CopyTotp => {
                                    if let Some(code) = &totp_code {
                                        ui.ctx().copy_text(code.clone());
                                    }
                                }
                                DetailAction::OpenWebsite(url) => {
                                    let _ = webbrowser_open(&url);
                                }
                                DetailAction::None => {}
                            }
                        } else {
                            ui.label("Select an item.");
                        }
                    }
                    DetailMode::Edit(draft) => {
                        match draw_detail_edit(ui, draft, &folders, false) {
                            EditAction::Save => {
                                if let Some(item) = &selected_item {
                                    let updated = draft.apply_to(item);
                                    if vault.update_item(&updated).is_ok() {
                                        if let Some(pos) = items.iter().position(|i| i.id == item.id) {
                                            items[pos] = updated;
                                        }
                                        mode = DetailMode::Read;
                                    }
                                }
                            }
                            EditAction::Cancel => mode = DetailMode::Read,
                            EditAction::None => {}
                        }
                    }
                    DetailMode::Create(draft) => {
                        match draw_detail_edit(ui, draft, &folders, true) {
                            EditAction::Save => {
                                if let Ok(created) = vault.create_item(&draft.to_new_item()) {
                                    selected_id = Some(created.id.clone());
                                    items.push(created);
                                    mode = DetailMode::Read;
                                }
                            }
                            EditAction::Cancel => mode = DetailMode::Read,
                            EditAction::None => {}
                        }
                    }
                }
            });

        ui.ctx().request_repaint_after(Duration::from_millis(500));
    });

    VaultWindowResult { locked: *locked.borrow() }
}

/// Spawns a one-shot background thread to fetch and decode `domain`'s
/// favicon, sending the result back over `tx`. A bare `thread::spawn` (not
/// `thread::scope`) because the result travels back over an owned channel
/// with no borrowed data -- there's nothing here that needs the caller's
/// stack to stay alive, unlike `loading_ui::show_while`'s worker.
fn spawn_favicon_fetch(item_id: String, domain: String, server_url: Option<String>, tx: mpsc::Sender<FaviconResult>) {
    std::thread::spawn(move || {
        let base = crate::favicon::icon_base_url(server_url.as_deref());
        let url = format!("{base}/{domain}/icon.png");
        let pixels = crate::favicon::fetch_icon_bytes(&url).and_then(|bytes| crate::favicon::decode_rgba(&bytes));
        let _ = tx.send(FaviconResult { item_id, pixels });
    });
}

fn webbrowser_open(url: &str) -> std::io::Result<()> {
    std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()?;
    Ok(())
}
```

- [ ] **Step 2: Build**

Run: `cargo build --lib --bin deskwarden` from `deskwarden/`
Expected: compiles clean after the clipboard-copy fix above. `injector` is accepted as a parameter but only actually invoked by the `Fill` action, which Task 10 wires up — an unused-parameter warning until then is expected and resolved by that task, not this one.

- [ ] **Step 3: Commit**

```bash
git add deskwarden/src/vault_window/mod.rs
git commit -m "feat: wire up the vault window (layout, favicon loading, auto-lock, TOTP polling)"
```

---

### Task 10: Tray + main.rs wiring, "Fill in app" from the detail pane

**Files:**
- Modify: `deskwarden/src/tray.rs` (add an `open_vault_id: MenuId` + "Open Vault" menu item, same pattern as `add_app_id`)
- Modify: `deskwarden/src/main.rs` (handle the new menu event; resolve which `hwnd` "Fill in app" targets)

**Interfaces:**
- Consumes: `vault_window::run`, `tray::AppTray`.
- Produces: nothing new consumed by later tasks (this is the integration task).

- [ ] **Step 1: Add the tray menu item**

In `deskwarden/src/tray.rs`, in `AppTray`, add a field:

```rust
    pub open_vault_id: MenuId,
```

In `build_tray()`, alongside the existing `add_app`/`quit` construction:

```rust
    let open_vault = MenuItem::new("Open Vault", true, None);
```

Add `open_vault` to the menu (append it before `Add app...` in whatever `Menu::append`/`append_items` call already builds the menu order — check the existing code for the exact call before editing), and add `open_vault_id: open_vault.id().clone(),` to the returned `AppTray`.

- [ ] **Step 2: Make `FillStats` cloneable**

`main()` needs to hand a second `FillStats` handle into the vault window without moving the one its own fill path holds. In `deskwarden/src/fill_stats.rs`, add `Clone` to the derive on the struct:

```rust
#[derive(Clone)]
pub struct FillStats {
```

(`PathBuf: Clone`, so this needs no hand-written impl.)

- [ ] **Step 3: Add `app::find_window_for_process` + test**

In `deskwarden/src/app.rs`, near `match_entries`:

```rust
/// Finds a currently-open window whose exe name matches `process` -- for
/// "Fill in app" (the vault window's detail pane), which has no
/// window-watch context of its own and needs to resolve a target hwnd from
/// just an item's `deskwarden:app-match` process name.
pub fn find_window_for_process<'a>(
    windows: &'a [crate::window_list::WindowInfo],
    process: &str,
) -> Option<&'a crate::window_list::WindowInfo> {
    windows.iter().find(|w| w.exe_name.eq_ignore_ascii_case(process))
}
```

In `app.rs`'s existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn find_window_for_process_matches_case_insensitively() {
        let windows = vec![
            crate::window_list::WindowInfo {
                hwnd: 1,
                pid: 100,
                exe_path: r"C:\Games\EpicGamesLauncher.exe".into(),
                exe_name: "EpicGamesLauncher.exe".into(),
                title: "Epic Games Launcher".into(),
            },
            crate::window_list::WindowInfo {
                hwnd: 2,
                pid: 200,
                exe_path: r"C:\Windows\notepad.exe".into(),
                exe_name: "notepad.exe".into(),
                title: "Untitled - Notepad".into(),
            },
        ];
        let found = find_window_for_process(&windows, "epicgameslauncher.exe").unwrap();
        assert_eq!(found.hwnd, 1);
        assert!(find_window_for_process(&windows, "steam.exe").is_none());
    }
```

Run: `cargo test --lib app::` from `deskwarden/`
Expected: PASS.

- [ ] **Step 4: Wire `DetailAction::Fill` in `vault_window/mod.rs`**

`extract_app_match(item)` needs no external context (Task 1 already has it in scope via `vault_bridge`), and `injector` is already a `vault_window::run` parameter -- so this resolves entirely inside the `DetailMode::Read` arm Task 9 wrote, no new parameters needed on `run` itself. Replace the `DetailAction::Fill` arm from Task 9's `mod.rs`:

```rust
                                DetailAction::Fill => {
                                    match crate::vault_bridge::extract_app_match(item) {
                                        Some(app_match) => {
                                            let windows = crate::window_list::list_windows(std::process::id());
                                            match crate::app::find_window_for_process(&windows, &app_match.process) {
                                                // fill_from_vault does its own credential lookup
                                                // and the fill in one call -- nothing else here
                                                // needs to touch `injector` directly.
                                                Some(target) => crate::app::fill_from_vault(
                                                    &vault,
                                                    injector,
                                                    &fill_stats,
                                                    &item.id,
                                                    target.hwnd,
                                                ),
                                                None => log::info!(
                                                    "\"Fill in app\" for {}: {} isn't currently open",
                                                    item.name, app_match.process
                                                ),
                                            }
                                        }
                                        None => log::info!(
                                            "\"Fill in app\" for {}: no app is matched to this item yet",
                                            item.name
                                        ),
                                    }
                                }
```

Add `use crate::window_list;` and `use crate::app;` to `vault_window/mod.rs`'s imports if not already present from earlier tasks.

- [ ] **Step 5: Handle the menu event in `main.rs`**

This block lives inside the same `loop { ... }` in `main()` that already handles `tray.add_app_id`, so it has every piece of state the startup sequence needs already in local scope: `store`, `job`, `session_token`, `bw_serve_child`, `vault`, `engine`, `schedule`. On a lock, this reruns exactly the same restart sequence `main()`'s own startup retry path already does (`reauthenticate` → `stop_bw_serve` on the *old* child → `try_start_backend` → `wait_for_vault_ready_with_spinner` → rebuild the match engine) rather than inventing a new one — see the existing block starting at the `wait_for_vault_ready_with_spinner` call partway through `main()` for the pattern this mirrors.

In `deskwarden/src/main.rs`, alongside the existing `if event.id == tray.add_app_id { ... }` block:

```rust
            if event.id == tray.open_vault_id {
                let server_url = login_ui::check_bw_status_details().server_url;
                let result = vault_window::run(vault.clone(), fill_stats.clone(), &injector, server_url, session_token.clone());

                if result.locked {
                    log::info!("vault window locked itself; re-authenticating");
                    bw_serve::stop_bw_serve(&mut bw_serve_child);
                    session_token = reauthenticate(&store);
                    bw_serve_child = match try_start_backend(
                        &session_token,
                        job.as_ref(),
                        bw_serve::PORT_RELEASE_GRACE_RESTART,
                    ) {
                        Ok(child) => child,
                        Err(e) => {
                            log::error!("{e}");
                            fatal_startup_error(&format!(
                                "Deskwarden could not restart its Bitwarden backend after the \
                                 vault window locked.\n\n{e}\n\nFull details are in:\n{}",
                                logging::log_file_path(&config_dir).display()
                            ));
                        }
                    };
                    match wait_for_vault_ready_with_spinner(&vault, &schedule) {
                        Ok(items) => {
                            match refresh_match_engine(&vault, &mut engine) {
                                Ok(count) => log::info!("match engine refreshed after unlock: {count} app match(es)"),
                                Err(e) => log::warn!("match engine refresh after unlock failed: {e:?}"),
                            }
                            let _ = items; // refresh_match_engine already re-read the vault; this is just the readiness signal
                        }
                        Err(e) => {
                            log::error!("{e}");
                            bw_serve::stop_bw_serve(&mut bw_serve_child);
                            fatal_startup_error(&format!(
                                "Deskwarden's Bitwarden backend did not come back up after the \
                                 vault window locked.\n\n{e}\n\nFull details are in:\n{}",
                                logging::log_file_path(&config_dir).display()
                            ));
                        }
                    }
                }
                last_dispatched_hwnd = None;
            }
```

`fatal_startup_error` is already `!`-returning (checked: every existing call site in `main.rs` uses it the same way, inside a `match ... Err(e) => { ...; fatal_startup_error(...) }` arm with no trailing value needed) -- reusing it here for the same "cannot recover, nothing left to do but tell the user and exit" situations the startup path already uses it for is consistent with the rest of this function, not a new failure mode this task invents.

- [ ] **Step 6: Build and run the full suite**

Run: `cargo build --lib --bin deskwarden && cargo test --lib` from `deskwarden/`
Expected: PASS, 0 failures, bin still links (confirms every `handle_match`/`fill_from_vault` call site picked up the `fill_stats` parameter from Task 3, and the new `open_vault_id` branch compiles against real `main()` state).

- [ ] **Step 7: Commit**

```bash
git add deskwarden/src/tray.rs deskwarden/src/main.rs deskwarden/src/app.rs deskwarden/src/fill_stats.rs
git commit -m "feat: wire the vault window into the tray, resolve Fill in app targets"
```

---

### Task 11: Final whole-branch review

Use `superpowers:requesting-code-review`'s `code-reviewer.md` template, dispatched on the most capable available model per `subagent-driven-development`'s Model Selection section. Package the diff with this skill's `scripts/review-package MERGE_BASE HEAD` (`MERGE_BASE` = the commit this work branched from).

Point the reviewer at this plan's Global Constraints section verbatim, plus:
- Confirm `app::fill_from_vault` never gained a vault-write call (fill-count analytics must stay local-only — grep the diff for any new `update_item`/`set_app_match` call inside the fill path).
- Confirm every new window (`vault_window::run`) has the font-timing `styled` guard on its first frame.
- Confirm the favicon loader never blocks the UI thread (the fetch+decode must happen inside `spawn_favicon_fetch`'s thread, not inline in the update closure).
- Confirm `EditDraft::apply_to` really does preserve every field the edit form doesn't expose (favorite, item_type, TOTP, uris, custom fields) — this is exactly the kind of silent-data-loss bug the existing `with_app_match` tests already guard against elsewhere in this codebase; the review should hold this new code to the same bar.
- Confirm Task 10's backend-restart block (on `result.locked`) actually mirrors the existing startup retry path's sequence (`stop_bw_serve` → `reauthenticate` → `try_start_backend` → `wait_for_vault_ready_with_spinner` → refresh the match engine) rather than a subset of it — a partial copy here would mean the vault window's Lock button leaves `bw serve` running against a session `main()`'s own state no longer agrees is current.

After a clean review, use `superpowers:finishing-a-development-branch` to offer merge/PR/keep/discard.

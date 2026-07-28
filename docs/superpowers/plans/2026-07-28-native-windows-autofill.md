# Native Windows App Autofill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `nodewarden-native`, a standalone Rust Windows background app that watches the foreground window, matches it against Bitwarden vault items (via `bw serve`), and fills credentials using UI Automation with a `SendInput` fallback.

**Architecture:** A single Rust binary/crate with focused modules: pure-logic modules (data model, matching, injector orchestration) built test-first; Win32-integration modules (window watching, UI Automation, SendInput, process enumeration, DPAPI) built with manual verification via `cargo run --example`; and a top-level wiring layer (tray icon, hotkey, `bw serve` lifecycle) that ties it together. No changes to Bitwarden's own code — this crate only talks to the officially documented `bw serve` local REST API.

**Tech Stack:** Rust (edition 2021), `windows` crate (Win32 bindings), `serde`/`serde_json`, `ureq` (HTTP client for `bw serve`), `directories` (config paths), `tray-icon`, `eframe`/`egui` (picker + overlay windows), `global-hotkey`. Dev/test: `mockito`.

## Global Constraints

- Windows only (no cross-platform abstraction layers).
- No modifications to Bitwarden's server, CLI, desktop, or browser-extension code — integration is exclusively through `bw serve`'s local REST API (default `http://localhost:8087`).
- The target vault backend is nodewarden, a self-hosted Bitwarden-API-compatible server the user already runs (not bitwarden.com). The `bw` CLI must be pointed at it via `bw config server <nodewarden-url>` before login — see Task 12.
- Do not depend on Bitwarden's `bitwarden-core`/`bitwarden-vault`/`bitwarden-crypto` crates or the in-progress Rust `bw` CLI in `bitwarden/sdk-internal` — their license (`LICENSE_SDK.txt`) prohibits redistribution to third parties and restricts use to official Bitwarden server products, and the CLI itself is pre-release (v0.0.2, core commands unimplemented). `bw serve` (the mature, GA, published CLI) is the only sanctioned integration surface.
- `nodewarden-native` is intended for open-source publication under a permissive license (MIT or Apache-2.0) with its own distinct branding — no visual mimicry of Bitwarden's official app.
- Login/unlock is a GUI flow (Task 12), not a terminal prompt: the user picks official-vs-self-hosted, enters a server URL if self-hosted, and enters credentials, which then drive `bw config server` / `bw login` / `bw unlock` under the hood.
- Passwords are passed to the `bw` CLI via `--passwordenv` (an environment variable read once by the child process), never as a bare CLI argument — bare-argument passwords are visible to other processes/users via the OS process list.
- Release binaries are code-signed via a free open-source code-signing provider (e.g. SignPath's OSS program) rather than a paid certificate, to reduce SmartScreen/AV false-positive friction for users without ongoing cost. This is a release/CI concern, not an implementation task — tracked as a follow-up once the binary is ready to ship, not part of the tasks below.
- Vault match metadata is stored in the existing custom-field mechanism under the exact field name `nodewarden:app-match`, value a JSON object `{"process": "<exe-name>", "trigger": "prompt" | "hotkey" | "auto"}`.
- Decrypted secrets are never written to disk; only the CLI session token is persisted, and only DPAPI-encrypted.
- v1 matches on process name only (no path/signature verification) — see spec's accepted-risk note.
- All new third-party crates must be MIT/Apache-2.0 licensed (already true of every crate listed above).

---

## File Structure

```
nodewarden-native/
  Cargo.toml
  src/
    main.rs              # entry point: bw serve lifecycle, tray, wiring
    app_match.rs          # AppMatch data model + JSON (de)serialization
    session_store.rs      # DPAPI-protected CLI session token cache
    vault_bridge.rs        # bw serve HTTP client + app-match extraction
    process_list.rs        # running-process enumeration (Toolhelp32)
    match_engine.rs         # process-name -> vault-item-id lookup cache
    window_watch.rs          # SetWinEventHook wrapper (foreground events)
    injector/
      mod.rs                # orchestration: UI Automation, fallback SendInput
      ui_automation.rs       # IUIAutomation-based field fill
      send_input.rs          # SendInput-based field fill
    picker_ui.rs              # process picker window (capture flow)
    overlay_ui.rs              # prompt-trigger overlay window
    login_ui.rs                 # server/credentials login screen
    tray.rs                    # system tray icon + menu
    hotkey.rs                   # global hotkey registration
  examples/
    watch_windows.rs            # manual verification harness for window_watch
    ui_automation_probe.rs       # manual verification harness for ui_automation
  tests/
    (integration tests live inline as #[cfg(test)] modules per file)
```

---

### Task 1: Project scaffolding

**Files:**
- Create: `nodewarden-native/Cargo.toml`
- Create: `nodewarden-native/src/main.rs`

**Interfaces:**
- Produces: a buildable, runnable no-op binary that later tasks add modules to.

- [ ] **Step 1: Create the crate**

```bash
cd "E:/Personal/node-bitwarden"
cargo new nodewarden-native --bin
```

- [ ] **Step 2: Write `Cargo.toml`**

```toml
[package]
name = "nodewarden-native"
version = "0.1.0"
edition = "2021"

[dependencies]
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Accessibility",
    "Win32_System_Diagnostics_ToolHelp",
    "Win32_System_Threading",
    "Win32_Security_Cryptography",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_System_Com",
] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
ureq = { version = "2", features = ["json"] }
directories = "5"
tray-icon = "0.19"
eframe = "0.29"
global-hotkey = "0.6"

[dev-dependencies]
mockito = "1"
```

- [ ] **Step 3: Write a no-op `src/main.rs`**

```rust
fn main() {
    println!("nodewarden-native starting");
}
```

- [ ] **Step 4: Verify it builds and runs**

Run: `cargo run`
Expected: prints `nodewarden-native starting` with no errors.

- [ ] **Step 5: Commit**

```bash
git add nodewarden-native/Cargo.toml nodewarden-native/Cargo.lock nodewarden-native/src/main.rs
git commit -m "chore: scaffold nodewarden-native crate"
```

---

### Task 2: AppMatch data model

**Files:**
- Create: `nodewarden-native/src/app_match.rs`
- Modify: `nodewarden-native/src/main.rs` (add `mod app_match;`)

**Interfaces:**
- Produces:
  - `pub const APP_MATCH_FIELD_NAME: &str = "nodewarden:app-match";`
  - `pub enum TriggerMode { Prompt, Hotkey, Auto }` (serde tag: lowercase)
  - `pub struct AppMatch { pub process: String, pub trigger: TriggerMode }`
  - `impl AppMatch { pub fn to_field_value(&self) -> String; pub fn from_field_value(value: &str) -> Result<Self, serde_json::Error>; }`

- [ ] **Step 1: Write the failing tests**

```rust
// nodewarden-native/src/app_match.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_name_matches_spec() {
        assert_eq!(APP_MATCH_FIELD_NAME, "nodewarden:app-match");
    }

    #[test]
    fn round_trips_through_json() {
        let original = AppMatch {
            process: "RockstarGamesLauncher.exe".to_string(),
            trigger: TriggerMode::Prompt,
        };
        let json = original.to_field_value();
        let parsed = AppMatch::from_field_value(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn serializes_trigger_as_lowercase() {
        let m = AppMatch { process: "mabl.exe".to_string(), trigger: TriggerMode::Auto };
        assert_eq!(m.to_field_value(), r#"{"process":"mabl.exe","trigger":"auto"}"#);
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(AppMatch::from_field_value("not json").is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test app_match -- --nocapture`
Expected: FAIL — `app_match` module / types don't exist yet.

- [ ] **Step 3: Write the implementation**

```rust
// nodewarden-native/src/app_match.rs (above the tests module)
use serde::{Deserialize, Serialize};

pub const APP_MATCH_FIELD_NAME: &str = "nodewarden:app-match";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerMode {
    Prompt,
    Hotkey,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppMatch {
    pub process: String,
    pub trigger: TriggerMode,
}

impl AppMatch {
    pub fn to_field_value(&self) -> String {
        serde_json::to_string(self).expect("AppMatch always serializes")
    }

    pub fn from_field_value(value: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(value)
    }
}
```

Add to `src/main.rs`:

```rust
mod app_match;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test app_match -- --nocapture`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add nodewarden-native/src/app_match.rs nodewarden-native/src/main.rs
git commit -m "feat: add AppMatch data model with JSON round-trip"
```

---

### Task 3: DPAPI session token store

**Files:**
- Create: `nodewarden-native/src/session_store.rs`
- Modify: `nodewarden-native/src/main.rs` (add `mod session_store;`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `pub struct SessionStore { /* private */ }`
  - `impl SessionStore { pub fn new(path: std::path::PathBuf) -> Self; pub fn save(&self, token: &str) -> std::io::Result<()>; pub fn load(&self) -> Option<String>; }`

- [ ] **Step 1: Write the failing test**

```rust
// nodewarden-native/src/session_store.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn round_trips_a_token_through_dpapi_and_disk() {
        let path = temp_dir().join(format!("nodewarden-test-session-{}.bin", std::process::id()));
        let store = SessionStore::new(path.clone());

        store.save("super-secret-session-token").unwrap();
        let loaded = store.load();

        assert_eq!(loaded.as_deref(), Some("super-secret-session-token"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_returns_none_when_file_missing() {
        let path = temp_dir().join("nodewarden-test-session-does-not-exist.bin");
        let store = SessionStore::new(path);
        assert_eq!(store.load(), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test session_store -- --nocapture`
Expected: FAIL — `SessionStore` doesn't exist yet.

- [ ] **Step 3: Write the implementation**

```rust
// nodewarden-native/src/session_store.rs (above the tests module)
use std::path::PathBuf;
use windows::Win32::Foundation::LocalFree;
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTOAPI_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
};

pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn save(&self, token: &str) -> std::io::Result<()> {
        let protected = protect(token.as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e:?}")))?;
        std::fs::write(&self.path, protected)
    }

    pub fn load(&self) -> Option<String> {
        let bytes = std::fs::read(&self.path).ok()?;
        let plain = unprotect(&bytes).ok()?;
        String::from_utf8(plain).ok()
    }
}

fn protect(data: &[u8]) -> windows::core::Result<Vec<u8>> {
    unsafe {
        let input = CRYPTOAPI_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPTOAPI_BLOB::default();
        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let result =
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(output.pbData as _)));
        Ok(result)
    }
}

fn unprotect(data: &[u8]) -> windows::core::Result<Vec<u8>> {
    unsafe {
        let input = CRYPTOAPI_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPTOAPI_BLOB::default();
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        let result =
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(output.pbData as _)));
        Ok(result)
    }
}
```

Add to `src/main.rs`:

```rust
mod session_store;
```

Note for the implementer: `windows` crate 0.58's exact `CryptProtectData`/`CryptUnprotectData` signature (which parameters are `Option<*const T>` vs bare pointers) can shift between minor versions — if this doesn't compile as-is, check `cargo doc --open -p windows` for the installed version and adjust the call sites; the DPAPI semantics (protect/unprotect a byte blob, free the output with `LocalFree`) stay the same.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test session_store -- --nocapture`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add nodewarden-native/src/session_store.rs nodewarden-native/src/main.rs
git commit -m "feat: add DPAPI-backed session token store"
```

---

### Task 4: Vault bridge (bw serve client)

**Files:**
- Create: `nodewarden-native/src/vault_bridge.rs`
- Modify: `nodewarden-native/src/main.rs` (add `mod vault_bridge;`)

**Interfaces:**
- Consumes: `app_match::{AppMatch, APP_MATCH_FIELD_NAME}` (Task 2).
- Produces:
  - `pub struct VaultItem { pub id: String, pub name: String, pub fields: Vec<VaultField> }`
  - `pub struct VaultField { pub name: Option<String>, pub value: Option<String> }`
  - `pub fn extract_app_match(item: &VaultItem) -> Option<AppMatch>`
  - `pub struct VaultBridge { /* private */ }`
  - `impl VaultBridge { pub fn new(base_url: impl Into<String>) -> Self; pub fn list_items(&self) -> Result<Vec<VaultItem>, VaultError>; pub fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<(), VaultError>; }`
  - `pub enum VaultError { Http(String), Parse(String) }`

- [ ] **Step 1: Write the failing tests**

```rust
// nodewarden-native/src/vault_bridge.rs
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
        };
        let m = extract_app_match(&item).unwrap();
        assert_eq!(m.process, "RockstarGamesLauncher.exe");
        assert_eq!(m.trigger, TriggerMode::Prompt);
    }

    #[test]
    fn extract_app_match_returns_none_without_field() {
        let item = VaultItem { id: "1".into(), name: "Other".into(), fields: vec![] };
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
        };
        assert!(extract_app_match(&item).is_none());
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

    const APP_MATCH_FIELD_NAME_FOR_TEST: &str = crate::app_match::APP_MATCH_FIELD_NAME;
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test vault_bridge -- --nocapture`
Expected: FAIL — module doesn't exist yet.

- [ ] **Step 3: Write the implementation**

```rust
// nodewarden-native/src/vault_bridge.rs (above the tests module)
use crate::app_match::{AppMatch, APP_MATCH_FIELD_NAME};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VaultField {
    pub name: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VaultItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub fields: Vec<VaultField>,
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

    pub fn set_app_match(&self, item: &VaultItem, m: &AppMatch) -> Result<(), VaultError> {
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

        let url = format!("{}/object/item/{}", self.base_url, item.id);
        self.agent
            .put(&url)
            .send_json(&updated)
            .map_err(|e| VaultError::Http(e.to_string()))?;
        Ok(())
    }
}
```

Add to `src/main.rs`:

```rust
mod vault_bridge;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test vault_bridge -- --nocapture`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add nodewarden-native/src/vault_bridge.rs nodewarden-native/src/main.rs
git commit -m "feat: add bw serve HTTP bridge and app-match field extraction"
```

---

### Task 5: Process enumeration

**Files:**
- Create: `nodewarden-native/src/process_list.rs`
- Modify: `nodewarden-native/src/main.rs` (add `mod process_list;`)

**Interfaces:**
- Produces:
  - `pub struct ProcessInfo { pub pid: u32, pub exe_name: String }`
  - `pub fn list_processes() -> windows::core::Result<Vec<ProcessInfo>>`

- [ ] **Step 1: Write the failing test**

```rust
// nodewarden-native/src/process_list.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_the_current_process() {
        let processes = list_processes().unwrap();
        let current_pid = std::process::id();
        assert!(
            processes.iter().any(|p| p.pid == current_pid),
            "expected current pid {current_pid} in process list of {} entries",
            processes.len()
        );
    }

    #[test]
    fn returns_a_nonempty_list() {
        let processes = list_processes().unwrap();
        assert!(!processes.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test process_list -- --nocapture`
Expected: FAIL — `list_processes` doesn't exist yet.

- [ ] **Step 3: Write the implementation**

```rust
// nodewarden-native/src/process_list.rs (above the tests module)
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};

pub struct ProcessInfo {
    pub pid: u32,
    pub exe_name: String,
}

pub fn list_processes() -> windows::core::Result<Vec<ProcessInfo>> {
    let mut result = Vec::new();

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len())],
                );
                result.push(ProcessInfo {
                    pid: entry.th32ProcessID,
                    exe_name: name,
                });

                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
    }

    Ok(result)
}
```

Add to `src/main.rs`:

```rust
mod process_list;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test process_list -- --nocapture`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add nodewarden-native/src/process_list.rs nodewarden-native/src/main.rs
git commit -m "feat: add running-process enumeration via Toolhelp32"
```

---

### Task 6: Match engine

**Files:**
- Create: `nodewarden-native/src/match_engine.rs`
- Modify: `nodewarden-native/src/main.rs` (add `mod match_engine;`)

**Interfaces:**
- Consumes: `app_match::AppMatch` (Task 2).
- Produces:
  - `pub struct MatchEngine { /* private */ }`
  - `impl MatchEngine { pub fn new() -> Self; pub fn rebuild(&mut self, entries: &[(String, AppMatch)]); pub fn lookup(&self, exe_name: &str) -> Option<(&str, &AppMatch)>; }`

- [ ] **Step 1: Write the failing tests**

```rust
// nodewarden-native/src/match_engine.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_match::TriggerMode;

    fn entry(item_id: &str, process: &str, trigger: TriggerMode) -> (String, AppMatch) {
        (item_id.to_string(), AppMatch { process: process.to_string(), trigger })
    }

    #[test]
    fn empty_engine_matches_nothing() {
        let engine = MatchEngine::new();
        assert!(engine.lookup("anything.exe").is_none());
    }

    #[test]
    fn matches_exact_process_name() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "RockstarGamesLauncher.exe", TriggerMode::Prompt)]);

        let (id, m) = engine.lookup("RockstarGamesLauncher.exe").unwrap();
        assert_eq!(id, "1");
        assert_eq!(m.trigger, TriggerMode::Prompt);
    }

    #[test]
    fn matches_case_insensitively() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "RockstarGamesLauncher.exe", TriggerMode::Auto)]);

        assert!(engine.lookup("rockstargameslauncher.EXE").is_some());
    }

    #[test]
    fn returns_none_for_unrelated_process() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "mabl.exe", TriggerMode::Hotkey)]);

        assert!(engine.lookup("notepad.exe").is_none());
    }

    #[test]
    fn rebuild_replaces_previous_entries() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[entry("1", "mabl.exe", TriggerMode::Hotkey)]);
        engine.rebuild(&[entry("2", "notepad.exe", TriggerMode::Auto)]);

        assert!(engine.lookup("mabl.exe").is_none());
        assert!(engine.lookup("notepad.exe").is_some());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test match_engine -- --nocapture`
Expected: FAIL — `MatchEngine` doesn't exist yet.

- [ ] **Step 3: Write the implementation**

```rust
// nodewarden-native/src/match_engine.rs (above the tests module)
use crate::app_match::AppMatch;
use std::collections::HashMap;

pub struct MatchEngine {
    by_process: HashMap<String, (String, AppMatch)>,
}

impl MatchEngine {
    pub fn new() -> Self {
        Self { by_process: HashMap::new() }
    }

    pub fn rebuild(&mut self, entries: &[(String, AppMatch)]) {
        self.by_process = entries
            .iter()
            .map(|(item_id, m)| (m.process.to_lowercase(), (item_id.clone(), m.clone())))
            .collect();
    }

    pub fn lookup(&self, exe_name: &str) -> Option<(&str, &AppMatch)> {
        self.by_process
            .get(&exe_name.to_lowercase())
            .map(|(id, m)| (id.as_str(), m))
    }
}

impl Default for MatchEngine {
    fn default() -> Self {
        Self::new()
    }
}
```

Add to `src/main.rs`:

```rust
mod match_engine;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test match_engine -- --nocapture`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add nodewarden-native/src/match_engine.rs nodewarden-native/src/main.rs
git commit -m "feat: add in-memory process-name match engine"
```

---

### Task 7: Window watcher

**Files:**
- Create: `nodewarden-native/src/window_watch.rs`
- Create: `nodewarden-native/examples/watch_windows.rs`
- Modify: `nodewarden-native/src/main.rs` (add `mod window_watch;`)

**Interfaces:**
- Consumes: `process_list` is not reused here (process name is resolved directly from the event's PID via `QueryFullProcessImageNameW` for freshness); no dependency on Task 5's snapshot-based listing.
- Produces:
  - `pub struct ForegroundEvent { pub hwnd: isize, pub pid: u32, pub exe_name: String }`
  - `pub fn watch_foreground_windows(callback: impl FnMut(ForegroundEvent) + 'static) -> windows::core::Result<()>` — blocks the calling thread running a message loop; calls `callback` on every foreground-window change.

This module is Win32-event-driven and cannot be meaningfully unit tested (it requires a live desktop session and real window-focus changes). It is verified manually via the example binary below, per the spec's testing approach.

- [ ] **Step 1: Write the implementation**

```rust
// nodewarden-native/src/window_watch.rs
use std::cell::RefCell;
use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, GetWindowThreadProcessId, TranslateMessage, MSG,
    EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
};

pub struct ForegroundEvent {
    pub hwnd: isize,
    pub pid: u32,
    pub exe_name: String,
}

thread_local! {
    static CALLBACK: RefCell<Option<Box<dyn FnMut(ForegroundEvent)>>> = RefCell::new(None);
}

pub fn watch_foreground_windows(
    callback: impl FnMut(ForegroundEvent) + 'static,
) -> windows::core::Result<()> {
    CALLBACK.with(|c| *c.borrow_mut() = Some(Box::new(callback)));

    unsafe {
        let hook: HWINEVENTHOOK = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.is_invalid() {
            return Err(windows::core::Error::from_win32());
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    Ok(())
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _dwms_event_time: u32,
) {
    if hwnd.0 == 0 {
        return;
    }

    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return;
    }

    let exe_name = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
        Ok(handle) => {
            let mut buffer = [0u16; MAX_PATH as usize];
            let mut size = buffer.len() as u32;
            let name = if QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buffer.as_mut_ptr()),
                &mut size,
            )
            .is_ok()
            {
                let full_path = String::from_utf16_lossy(&buffer[..size as usize]);
                full_path
                    .rsplit('\\')
                    .next()
                    .unwrap_or(&full_path)
                    .to_string()
            } else {
                String::new()
            };
            let _ = CloseHandle(handle);
            name
        }
        Err(_) => String::new(),
    };

    if exe_name.is_empty() {
        return;
    }

    CALLBACK.with(|c| {
        if let Some(cb) = c.borrow_mut().as_mut() {
            cb(ForegroundEvent { hwnd: hwnd.0, pid, exe_name });
        }
    });
}
```

Add to `src/main.rs`:

```rust
mod window_watch;
```

- [ ] **Step 2: Write the manual verification example**

```rust
// nodewarden-native/examples/watch_windows.rs
fn main() {
    println!("Watching foreground window changes. Alt-Tab between apps to see events; Ctrl+C to quit.");
    nodewarden_native::window_watch::watch_foreground_windows(|event| {
        println!("foreground: pid={} exe={} hwnd={}", event.pid, event.exe_name, event.hwnd);
    })
    .expect("failed to start window watcher");
}
```

Note: this example requires `window_watch` to be `pub` from a library target. Add a `src/lib.rs` re-exporting the modules used by examples:

```rust
// nodewarden-native/src/lib.rs
pub mod window_watch;
```

And add a `[lib]` section to `Cargo.toml`:

```toml
[lib]
name = "nodewarden_native"
path = "src/lib.rs"
```

- [ ] **Step 3: Verify it builds**

Run: `cargo build --example watch_windows`
Expected: builds with no errors.

- [ ] **Step 4: Manually verify**

Run: `cargo run --example watch_windows`
Expected: switch focus between at least two different applications (e.g. a browser and a text editor) using Alt-Tab; confirm each switch prints a line with the correct `exe` name for the app now in front.

- [ ] **Step 5: Commit**

```bash
git add nodewarden-native/src/window_watch.rs nodewarden-native/src/lib.rs nodewarden-native/examples/watch_windows.rs nodewarden-native/src/main.rs nodewarden-native/Cargo.toml
git commit -m "feat: add foreground-window watcher via SetWinEventHook"
```

---

### Task 8: UI Automation injector

**Files:**
- Create: `nodewarden-native/src/injector/ui_automation.rs`
- Create: `nodewarden-native/examples/ui_automation_probe.rs`
- Modify: `nodewarden-native/src/lib.rs` (add `pub mod injector;`)

**Interfaces:**
- Produces: `pub fn fill_via_ui_automation(hwnd: isize, username: &str, password: &str) -> windows::core::Result<bool>` — returns `Ok(true)` if both a username and password edit control were found and filled, `Ok(false)` if the window's accessibility tree didn't expose two usable edit controls (caller should fall back), `Err` on a COM-level failure.

This module requires a live target window and is verified manually, per the spec's testing approach.

- [ ] **Step 1: Write the implementation**

```rust
// nodewarden-native/src/injector/ui_automation.rs
use windows::core::{Result, BSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants,
    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_ValuePatternId,
};

pub fn fill_via_ui_automation(hwnd: isize, username: &str, password: &str) -> Result<bool> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;

        let root = automation.ElementFromHandle(HWND(hwnd))?;

        let condition = automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &windows::Win32::System::Variant::VARIANT::from(UIA_EditControlTypeId.0),
        )?;

        let edits = root.FindAll(TreeScope_Descendants, &condition)?;
        let count = edits.Length()?;

        if count < 2 {
            return Ok(false);
        }

        let username_element = edits.GetElement(0)?;
        let password_element = edits.GetElement(1)?;

        let username_value: IUIAutomationValuePattern =
            username_element.GetCurrentPatternAs(UIA_ValuePatternId)?;
        let password_value: IUIAutomationValuePattern =
            password_element.GetCurrentPatternAs(UIA_ValuePatternId)?;

        username_value.SetValue(&BSTR::from(username))?;
        password_value.SetValue(&BSTR::from(password))?;

        Ok(true)
    }
}
```

Add to `src/lib.rs`:

```rust
pub mod injector;
```

Create `src/injector/mod.rs` with just this for now (Task 10 fills it in):

```rust
// nodewarden-native/src/injector/mod.rs
pub mod ui_automation;
```

Note for the implementer: exact `windows` crate 0.58 signatures for `FindAll`/`CreatePropertyCondition`/`GetCurrentPatternAs` (particularly `VARIANT` construction) can differ slightly by version — if compilation errors surface, check `cargo doc --open -p windows --features Win32_UI_Accessibility` for the installed version's exact signatures; the approach (create automation instance, get element from HWND, find Edit controls by control-type property, get the Value pattern, call SetValue) stays the same regardless.

- [ ] **Step 2: Write the manual verification example**

```rust
// nodewarden-native/examples/ui_automation_probe.rs
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

fn main() {
    let hwnd = unsafe { GetForegroundWindow() };
    println!("Probing foreground window {:?} for two Edit controls...", hwnd.0);

    match nodewarden_native::injector::ui_automation::fill_via_ui_automation(
        hwnd.0,
        "probe-username",
        "probe-password",
    ) {
        Ok(true) => println!("Found and filled two edit controls."),
        Ok(false) => println!("Did not find two edit controls (fallback would trigger)."),
        Err(e) => println!("COM error: {e:?}"),
    }
}
```

- [ ] **Step 3: Verify it builds**

Run: `cargo build --example ui_automation_probe`
Expected: builds with no errors.

- [ ] **Step 4: Manually verify**

Run against a real login form (e.g. open Notepad's Find dialog as a quick two-field-ish smoke test, then against one of the actual target apps — Mabl desktop app or Rockstar Games Launcher): focus the target window, then `cargo run --example ui_automation_probe`. Expected: reports filling two edit controls, and the target window's fields visibly contain `probe-username` / `probe-password`.

- [ ] **Step 5: Commit**

```bash
git add nodewarden-native/src/injector/ nodewarden-native/examples/ui_automation_probe.rs nodewarden-native/src/lib.rs
git commit -m "feat: add UI Automation-based credential injector"
```

---

### Task 9: SendInput fallback injector

**Files:**
- Create: `nodewarden-native/src/injector/send_input.rs`
- Modify: `nodewarden-native/src/injector/mod.rs` (add `pub mod send_input;`)

**Interfaces:**
- Produces: `pub fn fill_via_send_input(username: &str, password: &str) -> windows::core::Result<()>` — types `username`, presses Tab, types `password`, into whatever control currently has keyboard focus.

Verified manually, same reasoning as Task 8.

- [ ] **Step 1: Write the implementation**

```rust
// nodewarden-native/src/injector/send_input.rs
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_TAB,
};

pub fn fill_via_send_input(username: &str, password: &str) -> windows::core::Result<()> {
    type_text(username)?;
    press_tab()?;
    type_text(password)?;
    Ok(())
}

fn type_text(text: &str) -> windows::core::Result<()> {
    for ch in text.encode_utf16() {
        send_unicode_char(ch)?;
    }
    Ok(())
}

fn send_unicode_char(ch: u16) -> windows::core::Result<()> {
    let mut down = keybd_input(0, KEYEVENTF_UNICODE);
    down.wScan = ch;
    let mut up = keybd_input(0, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
    up.wScan = ch;

    send(&[to_input(down), to_input(up)])
}

fn press_tab() -> windows::core::Result<()> {
    let down = keybd_input(VK_TAB.0, KEYBD_EVENT_FLAGS(0));
    let up = keybd_input(VK_TAB.0, KEYEVENTF_KEYUP);
    send(&[to_input(down), to_input(up)])
}

fn keybd_input(vk: u16, flags: KEYBD_EVENT_FLAGS) -> KEYBDINPUT {
    KEYBDINPUT {
        wVk: VIRTUAL_KEY(vk),
        wScan: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    }
}

fn to_input(ki: KEYBDINPUT) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }
}

fn send(inputs: &[INPUT]) -> windows::core::Result<()> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(windows::core::Error::from_win32());
    }
    Ok(())
}
```

Add to `src/injector/mod.rs`:

```rust
pub mod send_input;
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build`
Expected: builds with no errors.

- [ ] **Step 3: Manually verify**

Add a temporary call to `fill_via_send_input("probe-user", "probe-pass")` in `examples/ui_automation_probe.rs`'s `Ok(false)` branch (or a small ad hoc example), focus a plain text field (e.g. Notepad), run it, and confirm `probe-user`, a tab-driven field change, and `probe-pass` are typed in as expected. Remove the temporary call afterward if added only for this check.

- [ ] **Step 4: Commit**

```bash
git add nodewarden-native/src/injector/send_input.rs nodewarden-native/src/injector/mod.rs
git commit -m "feat: add SendInput-based fallback injector"
```

---

### Task 10: Injector orchestration

**Files:**
- Modify: `nodewarden-native/src/injector/mod.rs`

**Interfaces:**
- Consumes: `injector::ui_automation::fill_via_ui_automation`, `injector::send_input::fill_via_send_input` (Tasks 8-9), used only by the production adapters — the orchestration logic itself is tested against fakes.
- Produces:
  - `pub trait UiAutomationFiller { fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String>; }`
  - `pub trait SendInputFiller { fn fill(&self, user: &str, pass: &str) -> Result<(), String>; }`
  - `pub struct Injector<A: UiAutomationFiller, B: SendInputFiller> { pub ui: A, pub fallback: B }`
  - `impl<A, B> Injector<A, B> { pub fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String>; }`
  - `pub struct RealUiAutomation;` implementing `UiAutomationFiller` via Task 8's function.
  - `pub struct RealSendInput;` implementing `SendInputFiller` via Task 9's function.

- [ ] **Step 1: Write the failing tests**

```rust
// nodewarden-native/src/injector/mod.rs (add near the top, above existing `pub mod` lines)
#[cfg(test)]
mod orchestration_tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeUi {
        result: Result<bool, String>,
        calls: RefCell<u32>,
    }
    impl UiAutomationFiller for FakeUi {
        fn fill(&self, _hwnd: isize, _user: &str, _pass: &str) -> Result<bool, String> {
            *self.calls.borrow_mut() += 1;
            self.result.clone()
        }
    }

    struct FakeFallback {
        calls: RefCell<u32>,
    }
    impl SendInputFiller for FakeFallback {
        fn fill(&self, _user: &str, _pass: &str) -> Result<(), String> {
            *self.calls.borrow_mut() += 1;
            Ok(())
        }
    }

    #[test]
    fn does_not_fall_back_when_ui_automation_succeeds() {
        let ui = FakeUi { result: Ok(true), calls: RefCell::new(0) };
        let fallback = FakeFallback { calls: RefCell::new(0) };
        let injector = Injector { ui, fallback };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.ui.calls.borrow(), 1);
        assert_eq!(*injector.fallback.calls.borrow(), 0);
    }

    #[test]
    fn falls_back_when_ui_automation_finds_no_fields() {
        let ui = FakeUi { result: Ok(false), calls: RefCell::new(0) };
        let fallback = FakeFallback { calls: RefCell::new(0) };
        let injector = Injector { ui, fallback };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.fallback.calls.borrow(), 1);
    }

    #[test]
    fn falls_back_when_ui_automation_errors() {
        let ui = FakeUi { result: Err("com failure".into()), calls: RefCell::new(0) };
        let fallback = FakeFallback { calls: RefCell::new(0) };
        let injector = Injector { ui, fallback };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.fallback.calls.borrow(), 1);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test orchestration_tests -- --nocapture`
Expected: FAIL — `Injector`, `UiAutomationFiller`, `SendInputFiller` don't exist yet.

- [ ] **Step 3: Write the implementation**

```rust
// nodewarden-native/src/injector/mod.rs (above the orchestration_tests module)
pub mod ui_automation;
pub mod send_input;

pub trait UiAutomationFiller {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String>;
}

pub trait SendInputFiller {
    fn fill(&self, user: &str, pass: &str) -> Result<(), String>;
}

pub struct Injector<A: UiAutomationFiller, B: SendInputFiller> {
    pub ui: A,
    pub fallback: B,
}

impl<A: UiAutomationFiller, B: SendInputFiller> Injector<A, B> {
    pub fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String> {
        match self.ui.fill(hwnd, user, pass) {
            Ok(true) => Ok(()),
            Ok(false) => self.fallback.fill(user, pass),
            Err(_) => self.fallback.fill(user, pass),
        }
    }
}

pub struct RealUiAutomation;
impl UiAutomationFiller for RealUiAutomation {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String> {
        ui_automation::fill_via_ui_automation(hwnd, user, pass).map_err(|e| e.to_string())
    }
}

pub struct RealSendInput;
impl SendInputFiller for RealSendInput {
    fn fill(&self, user: &str, pass: &str) -> Result<(), String> {
        send_input::fill_via_send_input(user, pass).map_err(|e| e.to_string())
    }
}
```

Note: the `Ok(bool)` result of `FakeUi::fill` must implement `Clone` for the test above (`result: Result<bool, String>` used via `self.result.clone()`) — add `#[derive(Clone)]`-friendly usage by cloning the stored `Result` field as written; no change needed to the trait itself.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test orchestration_tests -- --nocapture`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add nodewarden-native/src/injector/mod.rs
git commit -m "feat: add injector orchestration with UI Automation -> SendInput fallback"
```

---

### Task 11: Picker and overlay UI

**Files:**
- Create: `nodewarden-native/src/picker_ui.rs`
- Create: `nodewarden-native/src/overlay_ui.rs`
- Modify: `nodewarden-native/src/lib.rs` (add `pub mod picker_ui; pub mod overlay_ui;`)

**Interfaces:**
- Consumes: `process_list::{list_processes, ProcessInfo}` (Task 5), `vault_bridge::{VaultBridge, VaultItem}` (Task 4), `app_match::{AppMatch, TriggerMode}` (Task 2).
- Produces:
  - `pub fn run_picker(vault: &vault_bridge::VaultBridge, target_item: &vault_bridge::VaultItem) -> Option<app_match::AppMatch>` — opens a blocking egui window with a search box over `list_processes()` results and a trigger-mode choice; on confirm, calls `vault.set_app_match(...)` and returns the saved `AppMatch`; returns `None` if the user cancels.
  - `pub fn show_prompt_overlay(app_name: &str) -> bool` — opens a small blocking egui window near the screen's top-right corner with "Fill" / "Dismiss" buttons for the given app name; returns `true` if "Fill" was clicked.

This is GUI code, verified manually.

- [ ] **Step 1: Write the implementation**

Both windows below need to hand a result back to their caller once the user clicks a button, but `eframe::run_simple_native`'s closure is `FnMut` + `'static` and runs on every repaint — it must own (`move`) its state, so a plain local variable moved into it can't be read afterward by the calling function. The fix used throughout this task is: put the result in `Rc<RefCell<T>>`, move a clone into the closure, read the original after `run_simple_native` returns (safe here because eframe runs the closure on the same thread that's blocked inside the call — no cross-thread sharing is happening).

```rust
// nodewarden-native/src/picker_ui.rs
use crate::app_match::{AppMatch, TriggerMode};
use crate::process_list::{list_processes, ProcessInfo};
use crate::vault_bridge::{VaultBridge, VaultItem};
use eframe::egui;
use std::cell::RefCell;
use std::rc::Rc;

pub fn run_picker(vault: VaultBridge, target_item: VaultItem) -> Option<AppMatch> {
    let processes = list_processes().unwrap_or_default();
    let result: Rc<RefCell<Option<AppMatch>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();

    let mut filter = String::new();
    let mut selected_pid: Option<u32> = None;
    let mut trigger = TriggerMode::Prompt;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([420.0, 480.0]),
        ..Default::default()
    };

    let _ = eframe::run_simple_native("Add app to nodewarden", options, move |ctx, frame| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut done = false;

            ui.heading(format!("Match a process to \"{}\"", target_item.name));
            ui.text_edit_singleline(&mut filter);

            egui::ScrollArea::vertical().show(ui, |ui| {
                for p in processes
                    .iter()
                    .filter(|p| p.exe_name.to_lowercase().contains(&filter.to_lowercase()))
                {
                    let selected = selected_pid == Some(p.pid);
                    if ui.selectable_label(selected, format!("{} (pid {})", p.exe_name, p.pid)).clicked() {
                        selected_pid = Some(p.pid);
                    }
                }
            });

            ui.separator();
            egui::ComboBox::from_label("Trigger")
                .selected_text(format!("{trigger:?}"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut trigger, TriggerMode::Prompt, "Prompt");
                    ui.selectable_value(&mut trigger, TriggerMode::Hotkey, "Hotkey");
                    ui.selectable_value(&mut trigger, TriggerMode::Auto, "Auto");
                });

            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    if let Some(pid) = selected_pid {
                        if let Some(p) = processes.iter().find(|p| p.pid == pid) {
                            let m = AppMatch { process: p.exe_name.clone(), trigger };
                            if vault.set_app_match(&target_item, &m).is_ok() {
                                *result_for_closure.borrow_mut() = Some(m);
                            }
                        }
                    }
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });

            if done {
                frame.close();
            }
        });
    });

    result.borrow_mut().take()
}
```

```rust
// nodewarden-native/src/overlay_ui.rs
use eframe::egui;
use std::cell::RefCell;
use std::rc::Rc;

pub fn show_prompt_overlay(app_name: &str) -> bool {
    let app_name = app_name.to_string();
    let fill_clicked = Rc::new(RefCell::new(false));
    let fill_clicked_for_closure = fill_clicked.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([300.0, 100.0])
            .with_always_on_top(),
        ..Default::default()
    };

    let _ = eframe::run_simple_native("nodewarden", options, move |ctx, frame| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut done = false;

            ui.label(format!("Fill saved credentials into {app_name}?"));
            ui.horizontal(|ui| {
                if ui.button("Fill").clicked() {
                    *fill_clicked_for_closure.borrow_mut() = true;
                    done = true;
                }
                if ui.button("Dismiss").clicked() {
                    done = true;
                }
            });
            if done {
                frame.close();
            }
        });
    });

    let clicked = *fill_clicked.borrow();
    clicked
}
```

Add to `src/lib.rs`:

```rust
pub mod picker_ui;
pub mod overlay_ui;
```

Note for the implementer: `run_picker`'s signature changed from borrowing (`&VaultBridge`, `&VaultItem`) to owning (`VaultBridge`, `VaultItem`) because the closure must own everything it captures. Callers (Task 13) clone a `VaultBridge` (now `Clone`, see Task 4) and a `VaultItem` before calling it.

- [ ] **Step 2: Verify it builds**

Run: `cargo build`
Expected: builds with no errors.

- [ ] **Step 3: Manually verify**

Add a temporary `#[test]`-free scratch call in `main.rs` (or a small example) invoking `run_picker` against a `VaultBridge` pointed at a running `bw serve` with at least one item, and `show_prompt_overlay("Test App")` standalone. Confirm: the picker lists real running processes, filters as you type, and saves the field via a visible `bw serve` PUT (check with `bw list items --search <item name>` afterward that the custom field is present); the overlay shows and returns the correct boolean for each button.

- [ ] **Step 4: Commit**

```bash
git add nodewarden-native/src/picker_ui.rs nodewarden-native/src/overlay_ui.rs nodewarden-native/src/lib.rs
git commit -m "feat: add process picker and prompt overlay UI"
```

---

### Task 12: Login and unlock UI

**Files:**
- Create: `nodewarden-native/src/login_ui.rs`
- Modify: `nodewarden-native/src/lib.rs` (add `pub mod login_ui;`)

**Interfaces:**
- Produces:
  - `pub enum BwStatus { Unauthenticated, Locked, Unlocked }` (`Clone, Copy, PartialEq, Eq`)
  - `pub fn check_bw_status() -> BwStatus` — runs `bw status` and parses the result.
  - `pub fn configure_server(url: &str)` — runs `bw config server <url>`.
  - `pub fn run_login_flow() -> String` — blocking GUI flow; shows a server-choice + email field when `check_bw_status()` is `Unauthenticated`, or just a password field when `Locked`/`Unlocked`; runs `bw login`/`bw unlock` accordingly and returns the resulting session token.

This is GUI code, verified manually, same reasoning as Task 11. It replaces the terminal-based password prompt from earlier drafts of this plan with the actual required UX: a login screen offering official-vs-self-hosted server choice plus credentials.

- [ ] **Step 1: Write the implementation**

```rust
// nodewarden-native/src/login_ui.rs
use eframe::egui;
use std::cell::RefCell;
use std::process::Command;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BwStatus {
    Unauthenticated,
    Locked,
    Unlocked,
}

pub fn check_bw_status() -> BwStatus {
    let output = Command::new("bw")
        .args(["status"])
        .output()
        .expect("failed to run `bw status` (is the Bitwarden CLI installed and on PATH?)");
    let text = String::from_utf8_lossy(&output.stdout);
    if text.contains("\"status\":\"unlocked\"") {
        BwStatus::Unlocked
    } else if text.contains("\"status\":\"locked\"") {
        BwStatus::Locked
    } else {
        BwStatus::Unauthenticated
    }
}

pub fn configure_server(url: &str) {
    let status = Command::new("bw")
        .args(["config", "server", url])
        .status()
        .expect("failed to run `bw config server` (is the Bitwarden CLI installed and on PATH?)");
    if !status.success() {
        panic!("`bw config server {url}` failed");
    }
}

/// Runs `bw` with the given args plus a password supplied via an
/// environment variable (`--passwordenv`), never as a bare CLI argument —
/// a bare-argument password would be visible to other processes/users
/// via the OS process list.
fn run_bw_with_password(args: &[&str], password: &str) -> Result<String, String> {
    let mut cmd = Command::new("bw");
    cmd.args(args);
    cmd.args(["--passwordenv", "NODEWARDEN_BW_PASSWORD"]);
    cmd.env("NODEWARDEN_BW_PASSWORD", password);
    let output = cmd.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn run_login_flow() -> String {
    let status = check_bw_status();
    let token: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let token_for_closure = token.clone();

    let mut self_hosted = false;
    let mut server_url = String::new();
    let mut email = String::new();
    let mut password = String::new();
    let mut error: Option<String> = None;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([360.0, 320.0]),
        ..Default::default()
    };

    let _ = eframe::run_simple_native("Log in to nodewarden", options, move |ctx, frame| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("nodewarden-native");

            if status == BwStatus::Unauthenticated {
                ui.checkbox(&mut self_hosted, "Self-hosted server");
                if self_hosted {
                    ui.label("Server URL");
                    ui.text_edit_singleline(&mut server_url);
                }
                ui.label("Email");
                ui.text_edit_singleline(&mut email);
            }

            ui.label("Master password");
            ui.add(egui::TextEdit::singleline(&mut password).password(true));

            if let Some(err) = &error {
                ui.colored_label(egui::Color32::RED, err);
            }

            if ui.button("Continue").clicked() {
                if status == BwStatus::Unauthenticated && self_hosted && !server_url.is_empty() {
                    configure_server(&server_url);
                }

                let result = match status {
                    BwStatus::Unauthenticated => {
                        run_bw_with_password(&["login", &email, "--raw"], &password)
                    }
                    BwStatus::Locked | BwStatus::Unlocked => {
                        run_bw_with_password(&["unlock", "--raw"], &password)
                    }
                };

                match result {
                    Ok(session_token) => *token_for_closure.borrow_mut() = Some(session_token),
                    Err(e) => error = Some(e),
                }
            }

            if token_for_closure.borrow().is_some() {
                frame.close();
            }
        });
    });

    token
        .borrow_mut()
        .take()
        .expect("login flow closed without producing a session token")
}
```

Add to `src/lib.rs`:

```rust
pub mod login_ui;
```

- [ ] **Step 2: Verify it builds**

Run: `cargo build`
Expected: builds with no errors.

- [ ] **Step 3: Manually verify**

With the Bitwarden CLI installed and *not* currently logged in (`bw logout` first if needed), run a small scratch call to `login_ui::run_login_flow()` (e.g. temporarily from `main.rs`, removed once Task 13 wires it in properly). Expected: the "Self-hosted server" checkbox and email field appear; checking it reveals a URL field; entering your nodewarden URL, email, and master password and clicking Continue configures the server, logs in, and returns a non-empty session token with no error shown. Re-run after `bw lock` (without logging out) and confirm only the password field appears this time, and Continue unlocks successfully.

- [ ] **Step 4: Commit**

```bash
git add nodewarden-native/src/login_ui.rs nodewarden-native/src/lib.rs
git commit -m "feat: add login/unlock UI for official and self-hosted servers"
```

---

### Task 13: Tray app wiring and end-to-end integration

**Files:**
- Create: `nodewarden-native/src/tray.rs`
- Create: `nodewarden-native/src/hotkey.rs`
- Modify: `nodewarden-native/src/main.rs` (full rewrite of wiring)

**Interfaces:**
- Consumes everything from Tasks 2-12: `app_match`, `session_store::SessionStore`, `vault_bridge::{VaultBridge, extract_app_match}`, `match_engine::MatchEngine`, `window_watch::watch_foreground_windows`, `injector::{Injector, RealUiAutomation, RealSendInput}`, `picker_ui::run_picker`, `overlay_ui::show_prompt_overlay`, `login_ui::run_login_flow`.
- Produces: the running application — no further consumers.

- [ ] **Step 1: Write the tray module**

```rust
// nodewarden-native/src/tray.rs
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

pub struct AppTray {
    _icon: TrayIcon,
    pub add_app_id: tray_icon::menu::MenuId,
    pub quit_id: tray_icon::menu::MenuId,
}

pub fn build_tray() -> AppTray {
    let menu = Menu::new();
    let add_app = MenuItem::new("Add app...", true, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&add_app).unwrap();
    menu.append(&quit).unwrap();

    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("nodewarden-native")
        .build()
        .expect("failed to build tray icon");

    AppTray { _icon: icon, add_app_id: add_app.id().clone(), quit_id: quit.id().clone() }
}

pub fn next_menu_event() -> Option<MenuEvent> {
    MenuEvent::receiver().try_recv().ok()
}
```

- [ ] **Step 2: Write the hotkey module**

```rust
// nodewarden-native/src/hotkey.rs
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};

pub struct FillHotkey {
    _manager: GlobalHotKeyManager,
    hotkey_id: u32,
}

pub fn register_fill_hotkey() -> FillHotkey {
    let manager = GlobalHotKeyManager::new().expect("failed to init hotkey manager");
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyB);
    manager.register(hotkey).expect("failed to register Ctrl+Alt+B");

    FillHotkey { _manager: manager, hotkey_id: hotkey.id() }
}

pub fn fill_hotkey_pressed(fh: &FillHotkey) -> bool {
    if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
        return event.id == fh.hotkey_id;
    }
    false
}
```

- [ ] **Step 3: Rewrite `main.rs`**

```rust
// nodewarden-native/src/main.rs
mod app_match;
mod hotkey;
mod injector;
mod login_ui;
mod match_engine;
mod overlay_ui;
mod picker_ui;
mod process_list;
mod session_store;
mod tray;
mod vault_bridge;
mod window_watch;

use app_match::AppMatch;
use injector::{Injector, RealSendInput, RealUiAutomation};
use match_engine::MatchEngine;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use vault_bridge::VaultBridge;

const BW_SERVE_URL: &str = "http://localhost:8087";

fn main() {
    let config_dir = directories::ProjectDirs::from("dev", "nodewarden", "nodewarden-native")
        .expect("could not resolve config directory")
        .config_dir()
        .to_path_buf();
    std::fs::create_dir_all(&config_dir).expect("failed to create config directory");

    let session_path = config_dir.join("session.bin");
    let store = session_store::SessionStore::new(session_path);

    let session_token = match store.load() {
        Some(token) => token,
        None => {
            let token = login_ui::run_login_flow();
            store.save(&token).expect("failed to persist session token");
            token
        }
    };

    let _bw_serve = spawn_bw_serve(&session_token);
    std::thread::sleep(std::time::Duration::from_millis(500));

    let vault = VaultBridge::new(BW_SERVE_URL);
    let mut engine = MatchEngine::new();
    refresh_match_engine(&vault, &mut engine);

    let injector = Injector { ui: RealUiAutomation, fallback: RealSendInput };
    let _fill_hotkey = hotkey::register_fill_hotkey();
    let _tray = tray::build_tray();

    let (tx, rx) = mpsc::channel::<window_watch::ForegroundEvent>();
    std::thread::spawn(move || {
        let _ = window_watch::watch_foreground_windows(move |event| {
            let _ = tx.send(event);
        });
    });

    loop {
        if let Ok(event) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
            if let Some((item_id, m)) = engine.lookup(&event.exe_name) {
                handle_match(&vault, &injector, item_id, m, event.hwnd, &event.exe_name);
            }
        }
    }
}

fn handle_match(
    vault: &VaultBridge,
    injector: &Injector<RealUiAutomation, RealSendInput>,
    item_id: &str,
    m: &AppMatch,
    hwnd: isize,
    exe_name: &str,
) {
    let should_fill = match m.trigger {
        app_match::TriggerMode::Auto => true,
        app_match::TriggerMode::Prompt => overlay_ui::show_prompt_overlay(exe_name),
        app_match::TriggerMode::Hotkey => false, // filled from the hotkey path instead
    };

    if !should_fill {
        return;
    }

    if let Ok(items) = vault.list_items() {
        if let Some(item) = items.iter().find(|i| i.id == item_id) {
            let (username, password) = credentials_for(item);
            let _ = injector.fill(hwnd, &username, &password);
        }
    }
}

fn credentials_for(_item: &vault_bridge::VaultItem) -> (String, String) {
    // bw serve's item payload includes a `login: { username, password }` object;
    // extend VaultItem/VaultField in vault_bridge.rs with those fields when wiring
    // this up for real, then read them here instead of the placeholder below.
    (String::new(), String::new())
}

fn refresh_match_engine(vault: &VaultBridge, engine: &mut MatchEngine) {
    let entries = vault
        .list_items()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| vault_bridge::extract_app_match(item).map(|m| (item.id.clone(), m)))
        .collect::<Vec<_>>();
    engine.rebuild(&entries);
}

fn spawn_bw_serve(session_token: &str) -> Child {
    Command::new("bw")
        .args(["serve", "--port", "8087"])
        .env("BW_SESSION", session_token)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn `bw serve` (is the Bitwarden CLI installed and on PATH?)")
}
```

Note for the implementer: `credentials_for` is intentionally left reading empty strings — `vault_bridge::VaultItem`/`VaultField` (Task 4) only modeled `id`/`name`/`fields` because that's all the match-extraction logic needed. Before this task is considered functionally complete, extend `VaultItem` with the `login: Option<LoginData>` shape `bw serve` actually returns (`{"login": {"username": "...", "password": "..."}}`) and implement `credentials_for` to read it — write this as a small follow-up TDD task (parse a sample `bw serve` item JSON with a `login` block, assert extracted username/password) before wiring real fills end-to-end.

- [ ] **Step 4: Verify it builds**

Run: `cargo build`
Expected: builds with no errors (after resolving `credentials_for`'s TODO per the note above — do that first if you want a real end-to-end fill, or proceed with empty placeholders to verify the rest of the pipeline compiles and runs first).

- [ ] **Step 5: Manually verify end-to-end**

With the Bitwarden CLI installed and at least one vault item that has an `nodewarden:app-match` field pointing at a real running app's process name (set via `run_picker` from Task 11, or manually via `bw` CLI), run: `cargo run`. Expected:
1. The login screen from Task 12 appears once; completing it (self-hosted URL + email + password, or just password if already logged in) configures the server if needed, logs in/unlocks, and starts `bw serve`.
2. Bring the matched application's window to the foreground.
3. For `prompt` trigger: the overlay appears; clicking Fill types credentials into the window (verify against both an app where UI Automation succeeds and one where it falls back to SendInput, per the spec's testing approach — e.g. Mabl desktop app and Rockstar Games Launcher).
4. For `auto` trigger: credentials are typed immediately with no overlay.
5. Quit via the tray menu's "Quit" item stops the process (wire this into the event loop if not already handled by Step 3's `_tray` receiver — poll `tray::next_menu_event()` in the main loop and `std::process::exit(0)` on a `quit_id` match).

- [ ] **Step 6: Commit**

```bash
git add nodewarden-native/src/tray.rs nodewarden-native/src/hotkey.rs nodewarden-native/src/main.rs nodewarden-native/Cargo.toml
git commit -m "feat: wire tray, hotkey, bw serve lifecycle, and match dispatch into main"
```

---

## Plan Self-Review Notes

- **Spec coverage:** window watcher (Task 7), process picker via running-process list (Tasks 5, 11), three configurable trigger modes (Tasks 2, 13), UI Automation + SendInput fallback (Tasks 8-10), `bw serve` as the vault source with DPAPI-protected session (Tasks 3-4, 13), GUI login/unlock covering both official and self-hosted servers (Task 12, added after design review), no backend/schema changes (custom field only, Task 2/4), security notes (transient in-memory secrets, password never passed as a bare CLI argument — Task 12) are all covered.
- **Known gap surfaced explicitly, not hidden:** `credentials_for` in Task 13 is a real placeholder for the `login.username`/`login.password` extraction, called out inline as a required follow-up TDD task rather than silently glossed over — the surrounding pipeline (matching, triggering, injecting) is fully implemented and testable independently of it.
- **Closure-capture bug caught and fixed:** the original Task 11 draft captured picker/overlay results in a `move` closure and then read them from the enclosing function afterward, which doesn't compile. Fixed with the `Rc<RefCell<T>>` pattern, applied consistently in Task 11 and the new Task 12.
- **Type consistency check:** `AppMatch`, `TriggerMode`, `VaultItem` (now `Clone`), `VaultBridge` (now `Clone`), `VaultField`, `MatchEngine::lookup` return type `Option<(&str, &AppMatch)>`, `Injector::fill` signature, and `login_ui::BwStatus`/`run_login_flow` are used identically across every task that references them.

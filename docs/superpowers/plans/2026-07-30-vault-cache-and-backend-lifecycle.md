# Vault Cache and Backend Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Serve vault reads from an in-memory cache so `bw serve` is only needed for sync, writes and TOTP, and let one user setting decide whether it stays running at idle.

**Architecture:** A `VaultCache` holds items and folders in memory, populated once per unlock through the existing `VaultBridge` and dropped on lock. All writes route through the cache so exactly one place can leave it stale. A `Settings` struct persisted as `settings.json` picks the backend lifecycle policy, which is a pure function of (setting, window open) tested as a table. A General preferences window exposes the setting.

**Tech Stack:** Rust, `serde`/`serde_json`, `eframe`/`egui` 0.35, existing `VaultBridge` HTTP client.

**Spec:** `docs/superpowers/specs/2026-07-30-vault-cache-and-backend-lifecycle-design.md`

## Global Constraints

- **Memory only.** The cache is never written to disk. Decrypted vault data at rest would contradict the README's "never touches encryption, key derivation, or sync logic itself".
- **Idle holds no vault contents.** The cache must be empty after lock, preserving the property `main()` already maintains by `drop()`ping items after building match entries.
- **All writes route through `VaultCache`.** No call site may call `VaultBridge`'s write methods directly and separately update the cache.
- **Settings never fail startup.** A missing, partial, or malformed `settings.json` yields defaults.
- **Default preserves today's behaviour:** `keep_backend_running: true`.
- Windows-only crate. Tests run with `cargo test` from `deskwarden/`.
- Every commit message ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/settings.rs` (create) | `Settings` struct, load/save, defaults |
| `deskwarden/src/vault_cache.rs` (create) | In-memory snapshot, read-through, write-through, invalidation |
| `deskwarden/src/backend_policy.rs` (create) | Pure `should_run` decision |
| `deskwarden/src/prefs_ui.rs` (create) | General preferences window |
| `deskwarden/src/lib.rs` (modify) | Register the four new modules |
| `deskwarden/src/tray.rs` (modify) | Add a Preferences menu item |
| `deskwarden/src/main.rs` (modify) | Own settings + cache, wire lifecycle and the menu item |
| `deskwarden/src/vault_window/mod.rs` (modify) | Read and write through the cache |

---

### Task 1: Settings persistence

**Files:**
- Create: `deskwarden/src/settings.rs`
- Modify: `deskwarden/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `Settings { keep_backend_running: bool, auto_lock_minutes: u64 }`, `Settings::default()`, `Settings::load(&Path) -> Settings`, `Settings::save(&self, &Path) -> std::io::Result<()>`, `Settings::auto_lock_timeout(&self) -> Duration`.

- [ ] **Step 1: Write the failing tests**

Create `deskwarden/src/settings.rs`:

```rust
//! User preferences, persisted as `settings.json` in the config directory.
//!
//! Follows `fill_stats`'s pattern: plain serde over a small struct, with
//! every read falling back to defaults. A settings file is never a reason
//! the app cannot start, so a missing, partial, or corrupt file is a
//! silent fall-back rather than an error.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Auto-lock timeout used when the stored value is absent. Matches the
/// constant this replaces in `vault_window`, which was marked "hardcoded
/// until the 3e preferences window exists".
const DEFAULT_AUTO_LOCK_MINUTES: u64 = 15;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether `bw serve` stays running while the vault is unlocked.
    ///
    /// `true` (the default) is today's behaviour: everything is instant and
    /// the backend holds ~111 MB at idle. `false` runs it only while the
    /// vault window is open; reads come from `VaultCache` either way, so
    /// autofill is unaffected.
    pub keep_backend_running: bool,
    /// Idle minutes before the vault window locks itself.
    pub auto_lock_minutes: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            keep_backend_running: true,
            auto_lock_minutes: DEFAULT_AUTO_LOCK_MINUTES,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    pub fn auto_lock_timeout(&self) -> Duration {
        Duration::from_secs(self.auto_lock_minutes * 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let p = temp_dir().join(format!("deskwarden-settings-test-{name}.json"));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn the_default_preserves_todays_behaviour() {
        let s = Settings::default();
        assert!(s.keep_backend_running);
        assert_eq!(s.auto_lock_timeout(), Duration::from_secs(15 * 60));
    }

    #[test]
    fn settings_round_trip_through_disk() {
        let path = temp_path("round-trip");
        let written = Settings {
            keep_backend_running: false,
            auto_lock_minutes: 5,
        };
        written.save(&path).unwrap();
        assert_eq!(Settings::load(&path), written);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_yields_defaults() {
        assert_eq!(Settings::load(&temp_path("absent")), Settings::default());
    }

    #[test]
    fn a_partial_file_keeps_defaults_for_absent_fields() {
        // `#[serde(default)]` on the struct is what makes this work: a file
        // written by an older build must not fail to parse once a field is
        // added.
        let path = temp_path("partial");
        std::fs::write(&path, r#"{"keep_backend_running": false}"#).unwrap();
        let loaded = Settings::load(&path);
        assert!(!loaded.keep_backend_running);
        assert_eq!(loaded.auto_lock_minutes, DEFAULT_AUTO_LOCK_MINUTES);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_malformed_file_yields_defaults_rather_than_failing() {
        let path = temp_path("malformed");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        let _ = std::fs::remove_file(&path);
    }
}
```

- [ ] **Step 2: Register the module**

In `deskwarden/src/lib.rs`, add alongside the existing `pub mod` lines:

```rust
pub mod settings;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --manifest-path deskwarden/Cargo.toml settings`
Expected: 5 passed.

- [ ] **Step 4: Commit**

```bash
git add deskwarden/src/settings.rs deskwarden/src/lib.rs
git commit -m "feat: persist user settings with default-on-failure loading

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Backend lifecycle policy

**Files:**
- Create: `deskwarden/src/backend_policy.rs`
- Modify: `deskwarden/src/lib.rs`

**Interfaces:**
- Consumes: `settings::Settings` from Task 1.
- Produces: `should_run(keep_backend_running: bool, vault_window_open: bool) -> bool`.

- [ ] **Step 1: Write the failing tests**

Create `deskwarden/src/backend_policy.rs`:

```rust
//! When `bw serve` should be running.
//!
//! A pure function so the policy is table-testable rather than something
//! that has to be verified by opening windows. The lifecycle itself lives
//! in `main`; this only decides.

/// Whether the backend should be running right now.
///
/// With `keep_backend_running` the answer is always yes -- today's
/// behaviour, everything instant, ~111 MB held at idle.
///
/// Without it the backend runs only while the vault window is open. That is
/// deliberately *not* "per operation": TOTP polls once a second and writes
/// are frequent while the window is open, so tearing down between them
/// would be pathological. Reads are served by `VaultCache`, so autofill is
/// unaffected either way and idle -- the state that lasts hours -- costs
/// nothing.
pub fn should_run(keep_backend_running: bool, vault_window_open: bool) -> bool {
    keep_backend_running || vault_window_open
}

#[cfg(test)]
mod tests {
    use super::should_run;

    #[test]
    fn keeping_it_running_ignores_whether_a_window_is_open() {
        assert!(should_run(true, false));
        assert!(should_run(true, true));
    }

    #[test]
    fn saving_memory_ties_the_backend_to_the_vault_window() {
        assert!(should_run(false, true));
        assert!(!should_run(false, false));
    }

    #[test]
    fn the_only_state_that_stops_the_backend_is_idle_while_saving_memory() {
        // Spelled out as a table so a future change that accidentally makes
        // the default mode shut down is a failing test, not a surprise.
        let cases = [
            ((true, true), true),
            ((true, false), true),
            ((false, true), true),
            ((false, false), false),
        ];
        for ((keep, open), expected) in cases {
            assert_eq!(
                should_run(keep, open),
                expected,
                "keep_backend_running={keep}, vault_window_open={open}"
            );
        }
    }
}
```

- [ ] **Step 2: Register the module**

In `deskwarden/src/lib.rs`:

```rust
pub mod backend_policy;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --manifest-path deskwarden/Cargo.toml backend_policy`
Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add deskwarden/src/backend_policy.rs deskwarden/src/lib.rs
git commit -m "feat: pure policy for when bw serve should run

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: The vault cache

**Files:**
- Create: `deskwarden/src/vault_cache.rs`
- Modify: `deskwarden/src/lib.rs`

**Interfaces:**
- Consumes: `vault_bridge::{VaultBridge, VaultItem, Folder, NewLoginItem, VaultError}`.
- Produces: `VaultCache::new(VaultBridge)`, `populate(&self) -> Result<(), VaultError>`, `items(&self) -> Vec<VaultItem>`, `folders(&self) -> Vec<Folder>`, `is_populated(&self) -> bool`, `clear(&self)`, `create_item`, `update_item`, `delete_item`, `create_folder`, `update_folder`, `delete_folder`, `bridge(&self) -> &VaultBridge`.

**Note for the implementer:** `VaultBridge` already has all the HTTP methods with these exact names (see `deskwarden/src/vault_bridge.rs`). This task wraps them; it does not reimplement them. Interior mutability via `Mutex` so the cache can be shared as `&self` — callers hold it behind an `Arc`.

- [ ] **Step 1: Write the failing tests**

Create `deskwarden/src/vault_cache.rs`:

```rust
//! An in-memory snapshot of the vault, in front of `VaultBridge`.
//!
//! Every read in the app used to be an HTTP call to `bw serve`, which is
//! why it had to run permanently. Holding items here means reads -- the
//! vault window's list and the autofill match path -- never touch it, so
//! the backend is only needed for sync, writes and TOTP.
//!
//! **Memory only, by design.** Nothing here is written to disk: decrypted
//! vault data at rest would contradict the README's claim that deskwarden
//! never touches encryption or storage. `clear` drops everything, and
//! `main` calls it on lock so idle holds no vault contents.
//!
//! **All writes go through here.** Each write updates the snapshot on
//! success, so there is exactly one place that can leave the cache stale
//! rather than one per call site.

use crate::vault_bridge::{Folder, NewLoginItem, VaultBridge, VaultError, VaultItem};
use std::sync::Mutex;

#[derive(Default)]
struct Snapshot {
    items: Vec<VaultItem>,
    folders: Vec<Folder>,
    populated: bool,
}

pub struct VaultCache {
    bridge: VaultBridge,
    snapshot: Mutex<Snapshot>,
}

impl VaultCache {
    pub fn new(bridge: VaultBridge) -> Self {
        Self {
            bridge,
            snapshot: Mutex::new(Snapshot::default()),
        }
    }

    /// The underlying bridge, for the operations that genuinely need the
    /// backend and are not cached: TOTP and `bw sync`.
    pub fn bridge(&self) -> &VaultBridge {
        &self.bridge
    }

    /// Fills the snapshot from the backend. Called once per unlock, and
    /// again after a sync.
    pub fn populate(&self) -> Result<(), VaultError> {
        let items = self.bridge.list_items()?;
        let folders = self.bridge.list_folders()?;
        let mut snapshot = self.lock();
        snapshot.items = items;
        snapshot.folders = folders;
        snapshot.populated = true;
        Ok(())
    }

    pub fn is_populated(&self) -> bool {
        self.lock().populated
    }

    pub fn items(&self) -> Vec<VaultItem> {
        self.lock().items.clone()
    }

    pub fn folders(&self) -> Vec<Folder> {
        self.lock().folders.clone()
    }

    /// Drops everything. Called on lock and on quit.
    pub fn clear(&self) {
        let mut snapshot = self.lock();
        snapshot.items.clear();
        snapshot.items.shrink_to_fit();
        snapshot.folders.clear();
        snapshot.folders.shrink_to_fit();
        snapshot.populated = false;
    }

    pub fn create_item(&self, new_item: &NewLoginItem) -> Result<VaultItem, VaultError> {
        let created = self.bridge.create_item(new_item)?;
        self.lock().items.push(created.clone());
        Ok(created)
    }

    pub fn update_item(&self, item: &VaultItem) -> Result<(), VaultError> {
        self.bridge.update_item(item)?;
        let mut snapshot = self.lock();
        if let Some(existing) = snapshot.items.iter_mut().find(|i| i.id == item.id) {
            *existing = item.clone();
        }
        Ok(())
    }

    pub fn delete_item(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_item(id)?;
        self.lock().items.retain(|i| i.id != id);
        Ok(())
    }

    pub fn create_folder(&self, name: &str) -> Result<Folder, VaultError> {
        let created = self.bridge.create_folder(name)?;
        self.lock().folders.push(created.clone());
        Ok(created)
    }

    pub fn update_folder(&self, id: &str, name: &str) -> Result<Folder, VaultError> {
        let updated = self.bridge.update_folder(id, name)?;
        let mut snapshot = self.lock();
        if let Some(existing) = snapshot.folders.iter_mut().find(|f| f.id == updated.id) {
            existing.name = updated.name.clone();
        }
        Ok(updated)
    }

    pub fn delete_folder(&self, id: &str) -> Result<(), VaultError> {
        self.bridge.delete_folder(id)?;
        self.lock().folders.retain(|f| f.id != id);
        Ok(())
    }

    /// A poisoned lock means another thread panicked mid-update, which
    /// cannot corrupt anything here worse than a stale snapshot -- recover
    /// rather than propagating a panic into the UI thread.
    fn lock(&self) -> std::sync::MutexGuard<'_, Snapshot> {
        self.snapshot.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_for(url: String) -> VaultCache {
        VaultCache::new(VaultBridge::new(url))
    }

    fn items_body() -> &'static str {
        r#"{"success":true,"data":{"data":[
            {"id":"1","name":"Alpha","fields":[],"type":1},
            {"id":"2","name":"Beta","fields":[],"type":1}
        ]}}"#
    }

    fn folders_body() -> &'static str {
        r#"{"success":true,"data":{"data":[{"id":"f1","name":"Work"}]}}"#
    }

    #[test]
    fn populate_fills_the_snapshot_and_reads_come_from_it() {
        let mut server = mockito::Server::new();
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .expect(1)
            .create();
        let folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .expect(1)
            .create();

        let cache = cache_for(server.url());
        assert!(!cache.is_populated());
        cache.populate().unwrap();

        // Many reads, one fetch: `expect(1)` fails if a read hits HTTP.
        for _ in 0..5 {
            assert_eq!(cache.items().len(), 2);
            assert_eq!(cache.folders().len(), 1);
        }
        items.assert();
        folders.assert();
    }

    #[test]
    fn clear_empties_the_snapshot_so_idle_holds_no_vault_contents() {
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();

        let cache = cache_for(server.url());
        cache.populate().unwrap();
        assert_eq!(cache.items().len(), 2);

        cache.clear();
        assert!(cache.items().is_empty());
        assert!(cache.folders().is_empty());
        assert!(!cache.is_populated());
    }

    #[test]
    fn a_deleted_item_leaves_the_cache_immediately() {
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _d = server.mock("DELETE", "/object/item/1").with_status(200).create();

        let cache = cache_for(server.url());
        cache.populate().unwrap();
        cache.delete_item("1").unwrap();

        let ids: Vec<String> = cache.items().into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec!["2".to_string()]);
    }

    #[test]
    fn a_failed_write_leaves_the_cache_untouched() {
        // The cache must reflect the server, never an optimistic guess.
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _d = server.mock("DELETE", "/object/item/1").with_status(500).create();

        let cache = cache_for(server.url());
        cache.populate().unwrap();
        assert!(cache.delete_item("1").is_err());
        assert_eq!(cache.items().len(), 2, "a failed delete removed it anyway");
    }

    #[test]
    fn a_renamed_folder_updates_in_place() {
        let mut server = mockito::Server::new();
        let _i = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _f = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let _u = server
            .mock("PUT", "/object/folder/f1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"id":"f1","name":"Renamed"}}"#)
            .create();

        let cache = cache_for(server.url());
        cache.populate().unwrap();
        cache.update_folder("f1", "Renamed").unwrap();

        assert_eq!(cache.folders()[0].name, "Renamed");
    }
}
```

- [ ] **Step 2: Register the module**

In `deskwarden/src/lib.rs`:

```rust
pub mod vault_cache;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --manifest-path deskwarden/Cargo.toml vault_cache`
Expected: 5 passed.

- [ ] **Step 4: Commit**

```bash
git add deskwarden/src/vault_cache.rs deskwarden/src/lib.rs
git commit -m "feat: in-memory vault cache with write-through and clear-on-lock

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Read and write the vault window through the cache

**Files:**
- Modify: `deskwarden/src/vault_window/mod.rs`
- Modify: `deskwarden/src/main.rs`

**Interfaces:**
- Consumes: `VaultCache` (Task 3), `Settings` (Task 1).
- Produces: `vault_window::run` takes `cache: Arc<VaultCache>` in place of its current `vault: VaultBridge` parameter, and `auto_lock: Duration` in place of the `AUTO_LOCK_TIMEOUT` constant.

**Note for the implementer:** `vault_window::run` currently calls `vault.list_items()`, `vault.list_folders()`, `vault.create_item`, `vault.update_item`, `vault.delete_item`, `vault.create_folder`, `vault.update_folder`, `vault.delete_folder`, and `vault.get_totp`. The first eight move to the identically-named `VaultCache` methods. `get_totp` stays on the bridge via `cache.bridge().get_totp(...)`, because TOTP is generated by the CLI and is not cached.

- [ ] **Step 1: Change the signature and the reads**

In `deskwarden/src/vault_window/mod.rs`, change the `vault: VaultBridge` parameter of `run` to `cache: std::sync::Arc<crate::vault_cache::VaultCache>`, delete the `AUTO_LOCK_TIMEOUT` constant, and add an `auto_lock: Duration` parameter used everywhere that constant was.

Replace the body of `spawn_vault_load` so it reads the cache rather than the bridge:

```rust
fn spawn_vault_load(
    cache: std::sync::Arc<crate::vault_cache::VaultCache>,
    tx: mpsc::Sender<(Vec<VaultItem>, Vec<Folder>)>,
    // `true` after a sync, which changes the vault underneath us: the
    // snapshot is still marked populated but is now stale, so the
    // `is_populated` short-circuit below would serve pre-sync data and the
    // sync would appear to do nothing. `false` on window open, where the
    // snapshot from unlock is current and re-fetching would throw away the
    // whole point of the cache.
    force_refresh: bool,
) {
    std::thread::spawn(move || {
        if force_refresh || !cache.is_populated() {
            if let Err(e) = cache.populate() {
                log::warn!("could not populate the vault cache: {e:?}");
            }
        }
        let _ = tx.send((cache.items(), cache.folders()));
    });
}
```

Call it with `false` from the initial load, and `true` from the sync-completion handler.

- [ ] **Step 2: Point every write at the cache**

In the same file, apply exactly these replacements. The method names and signatures on `VaultCache` are identical to `VaultBridge`'s, so each is a receiver swap:

| Current call | Becomes |
| --- | --- |
| `vault.list_items()` | `cache.items()` (no `Result`) |
| `vault.list_folders()` | `cache.folders()` (no `Result`) |
| `vault.create_item(&draft.to_new_item())` | `cache.create_item(&draft.to_new_item())` |
| `vault.update_item(&updated)` | `cache.update_item(&updated)` |
| `vault.delete_item(&item.id)` | `cache.delete_item(&item.id)` |
| `vault.create_folder("New folder")` | `cache.create_folder("New folder")` |
| `vault.update_folder(&state.folder_id, &state.name)` | `cache.update_folder(&state.folder_id, &state.name)` |
| `vault.delete_folder(&state.folder_id)` | `cache.delete_folder(&state.folder_id)` |
| `vault.get_totp(&item.id)` | `cache.bridge().get_totp(&item.id)` |

`get_totp` is the one that stays on the bridge: codes are generated by the CLI per request and are not cacheable.

Note that `items()`/`folders()` return `Vec<_>` rather than `Result<Vec<_>, VaultError>`, so the `.unwrap_or_default()` at those two call sites is removed.

- [ ] **Step 3: Update the caller**

In `deskwarden/src/main.rs`, `open_vault_window` currently passes `vault.clone()`. Pass the `Arc<VaultCache>` and `settings.auto_lock_timeout()` instead.

- [ ] **Step 4: Verify it builds and all tests still pass**

Run: `cargo test --manifest-path deskwarden/Cargo.toml`
Expected: all tests pass, no warnings.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/vault_window/mod.rs deskwarden/src/main.rs
git commit -m "refactor: serve vault window reads and writes through the cache

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: Autofill reads from the cache, and the backend lifecycle

**Files:**
- Modify: `deskwarden/src/main.rs`
- Modify: `deskwarden/src/app.rs`

**Interfaces:**
- Consumes: `VaultCache` (Task 3), `backend_policy::should_run` (Task 2), `Settings` (Task 1).
- Produces: nothing new.

- [ ] **Step 1: Own the settings and cache in `main`**

In `deskwarden/src/main.rs`, after `config_dir` is created:

```rust
let settings_path = config_dir.join("settings.json");
let settings = settings::Settings::load(&settings_path);
```

and after `VaultBridge::new(BW_SERVE_URL)`:

```rust
let cache = std::sync::Arc::new(vault_cache::VaultCache::new(vault.clone()));
```

- [ ] **Step 2: Populate once at unlock instead of fetching per read**

Replace the startup `wait_for_vault_ready_with_spinner` result handling so the items it returns also populate the cache:

```rust
// One fetch per unlock. Everything downstream -- the match engine, the
// vault window, autofill -- reads this snapshot rather than re-fetching.
if let Err(e) = cache.populate() {
    log::warn!("could not populate the vault cache at startup: {e:?}");
}
```

- [ ] **Step 3: Fill from the cache**

In `deskwarden/src/app.rs`, `fill_from_vault` calls `vault.get_item(item_id)`. Change it to take `&VaultCache` and resolve from the snapshot, falling back to the bridge if the cache is empty:

```rust
let item = cache
    .items()
    .into_iter()
    .find(|i| i.id == item_id)
    .map(Ok)
    // Empty cache while unlocked should not happen; fall back rather than
    // failing the fill, and log it as a bug signal.
    .unwrap_or_else(|| {
        log::warn!("cache miss for item {item_id} during a fill; falling back to bw serve");
        cache.bridge().get_item(item_id)
    });
```

- [ ] **Step 4: Make the backend handle optional**

`bw_serve_child` is currently a plain `Child`, which cannot express "not running". Change its declaration in `main` to `Option<Child>`:

```rust
let mut bw_serve_child: Option<Child> = Some(start_backend(&session_token, job.as_ref()));
```

Every existing use must be updated to match — `bw_serve::stop_bw_serve(&mut bw_serve_child)` becomes `if let Some(child) = bw_serve_child.as_mut() { bw_serve::stop_bw_serve(child); }`, and the two reassignments in the retry and lock-recovery paths wrap their result in `Some(...)`. The compiler finds all of them; there is no silent failure mode here.

- [ ] **Step 5: Apply the lifecycle policy**

In the main loop, reconcile the backend against the policy on either side of the vault window opening:

```rust
// `vault_window_open` is true only while `open_vault_window` is running,
// so this is evaluated before and after that call.
let wanted = backend_policy::should_run(settings.keep_backend_running, vault_window_open);
if wanted && bw_serve_child.is_none() {
    bw_serve_child = try_start_backend(
        &session_token,
        job.as_ref(),
        bw_serve::PORT_RELEASE_GRACE_RESTART,
    )
    .ok();
} else if !wanted {
    if let Some(child) = bw_serve_child.as_mut() {
        bw_serve::stop_bw_serve(child);
    }
    bw_serve_child = None;
}
```

When the vault window opens in save-memory mode, start the backend on a background thread rather than awaiting it, so the window paints from cache immediately and the ~8s spin-up overlaps with the user searching and navigating.

- [ ] **Step 6: Clear the cache on lock and quit**

Wherever `result.locked` is handled, and in the tray Quit handler before `process::exit`, add:

```rust
cache.clear();
```

- [ ] **Step 7: Verify**

Run: `cargo test --manifest-path deskwarden/Cargo.toml`
Expected: all tests pass, no warnings.

- [ ] **Step 8: Commit**

```bash
git add deskwarden/src/main.rs deskwarden/src/app.rs
git commit -m "feat: fill from cache and run bw serve per the lifecycle policy

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: General preferences window

**Files:**
- Create: `deskwarden/src/prefs_ui.rs`
- Modify: `deskwarden/src/lib.rs`, `deskwarden/src/tray.rs`, `deskwarden/src/main.rs`

**Interfaces:**
- Consumes: `Settings` (Task 1), `theme::toggle_pill`, `login_ui::{draw_window_chrome, ChromeAction, round_window_corners}`.
- Produces: `prefs_ui::run(settings: Settings) -> Settings` — opens the window, returns the (possibly edited) settings for the caller to save.

- [ ] **Step 1: Write the window**

Create `deskwarden/src/prefs_ui.rs`. It mirrors the frameless-chrome pattern the other windows use, and uses `theme::toggle_pill`, which already exists for exactly these rows (design 3e). Only the General section is built; 3e's other six sections have no settings behind them yet.

```rust
//! The 3e preferences window, General section.
//!
//! Scoped to the settings that exist. `AUTO_LOCK_TIMEOUT` was marked in
//! `vault_window` as "hardcoded until the 3e preferences window exists" --
//! this is that window, so it lives here now.

use crate::login_ui::{draw_window_chrome, round_window_corners, ChromeAction};
use crate::settings::Settings;
use crate::theme;
use eframe::egui::{self, Margin, RichText};
use std::cell::RefCell;
use std::rc::Rc;

const WINDOW_TITLE: &str = "Deskwarden Preferences";

/// One settings row: label, description, trailing toggle. Returns the new
/// value. The whole row is the hit target, matching the design.
fn toggle_row(ui: &mut egui::Ui, label: &str, description: &str, value: bool) -> bool {
    let mut next = value;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::semibold(label, 13.0).color(theme::INK));
            ui.label(RichText::new(description).size(11.0).color(theme::TEXT_FAINT));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(40.0, 22.0), egui::Sense::click());
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                theme::toggle_pill(ui, value);
            });
            if response.clicked() {
                next = !value;
            }
        });
    });
    next
}

/// Opens the preferences window and blocks until it closes, returning the
/// edited settings. The caller persists them.
pub fn run(settings: Settings) -> Settings {
    let result = Rc::new(RefCell::new(settings.clone()));
    let result_for_closure = result.clone();
    let mut styled = false;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 300.0])
            .with_resizable(false)
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            round_window_corners(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        if draw_window_chrome(ui, WINDOW_TITLE) == ChromeAction::Close {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(Margin::symmetric(26, 22)))
            .show(ui, |ui| {
                let mut current = result_for_closure.borrow_mut();
                ui.label(theme::bold("General", 15.0).color(theme::INK));
                ui.add_space(16.0);

                current.keep_backend_running = toggle_row(
                    ui,
                    "Keep the Bitwarden backend running",
                    "Faster, and uses about 110 MB while idle. Off runs it only \
                     while this window is open; autofill is unaffected either way.",
                    current.keep_backend_running,
                );
            });
    });

    let edited = result.borrow().clone();
    edited
}
```

- [ ] **Step 2: Register the module**

In `deskwarden/src/lib.rs`:

```rust
pub mod prefs_ui;
```

- [ ] **Step 3: Add the tray menu item**

In `deskwarden/src/tray.rs`, add a `pub preferences_id: MenuId` field to the tray struct alongside `open_vault_id`, and a `MenuItem::new("Preferences...", true, None)` appended before the Quit separator, storing its id.

- [ ] **Step 4: Handle the menu item**

In `deskwarden/src/main.rs`, next to the `open_vault_id` handler:

```rust
if event.id == tray.preferences_id {
    let edited = prefs_ui::run(settings.clone());
    if edited != settings {
        settings = edited;
        if let Err(e) = settings.save(&settings_path) {
            log::warn!("could not save settings: {e}");
        }
    }
    last_dispatched_hwnd = None;
}
```

- [ ] **Step 5: Verify**

Run: `cargo test --manifest-path deskwarden/Cargo.toml` then `cargo build --manifest-path deskwarden/Cargo.toml`
Expected: all tests pass, builds with no warnings.

- [ ] **Step 6: Manual check**

Launch the app, open Preferences from the tray, toggle the setting, close the window, reopen it, and confirm the toggle kept its value. Then confirm `%APPDATA%\Deskwarden\Deskwarden\config\settings.json` contains `"keep_backend_running": false`.

- [ ] **Step 7: Commit**

```bash
git add deskwarden/src/prefs_ui.rs deskwarden/src/lib.rs deskwarden/src/tray.rs deskwarden/src/main.rs
git commit -m "feat: General preferences window with the backend lifecycle toggle

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Correct the README's backend memory figure

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Fix the figure**

The Size section reports `bw serve` at 117–162 MB tray-only from a cold measurement. Steady state after serving a real 1657-item vault is ~111 MB and it grows as it serves. Update the RAM table's `bw serve` row to a steady-state figure and note that the backend can now be shut down at idle via Preferences.

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: use a steady-state bw serve figure and note the new setting

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

# Keeping the UI Loaded Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `keep_ui_loaded` setting that makes the vault window's process hide instead of exiting on a plain close, so every reopen after the first is instant.

**Architecture:** The child already runs in its own process and already reads `settings.json` itself. On a plain close it hides its viewport rather than sending `ViewportCommand::Close`, releases its vault-service attachment slot, and waits on a named Windows event that the daemon sets when the user asks for the vault again. Every outcome the daemon must act on -- lock, re-auth, account switch/add/remove, and a preferences edit -- still exits through today's result-file-and-reap path, so the daemon's session machinery is untouched.

**Tech Stack:** Rust 2021, `windows` crate 0.58 (`CreateEventW`/`OpenEventW`/`SetEvent`/`WaitForSingleObject`), `eframe`/`egui` (`ViewportCommand::Visible`, `CancelClose`), `serde`.

## Global Constraints

- **`cfg(test)` seams are banned crate-wide.** Test seams are `fn`-pointer structs in production code, as `ServiceEnv`, `StartEnv` and `DiskCacheEnv` are.
- **`SYNCHRONIZATION_SYNCHRONIZE` comes from the `windows` crate, never a literal.** `vault_service` shipped `0x0010` for a right that is `0x0010_0000`; every `OpenMutexW` returned ACCESS_DENIED and all 23 tests passed because the fake kernel never reached the call.
- **Default is `false`**, and a `settings.json` without the field parses as `false`.
- **Build and test with `CARGO_TARGET_DIR=/e/_dw_agent/run`.** Do not create a second target directory -- the disk has under 2 GB free.
- **`RUSTFLAGS="-D warnings"` for builds.** Read the whole build output, not just the last line.
- **The `tests::` sync/switch family is flaky on this machine** (the unmodified tree fails 7-9 single-threaded, membership shifting per run). Judge your change by the tests named in each task, and compare a full-suite run against a `git stash` baseline before blaming yourself.

---

### Task 1: The setting

**Files:**
- Modify: `deskwarden/src/settings.rs` (struct at :764, `Default` at :1241, `persist_preferences` at :1377)

**Interfaces:**
- Produces: `Settings { pub keep_ui_loaded: bool, .. }`, default `false`, persisted by `persist_preferences`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `deskwarden/src/settings.rs`:

```rust
    /// **Off unless asked for.** This setting spends ~100 MB for speed, and
    /// a memory cost nobody chose is the complaint this whole split came
    /// from ("tray again is 50Mb").
    #[test]
    fn keeping_the_ui_loaded_is_off_by_default() {
        assert!(!Settings::default().keep_ui_loaded);
    }

    /// An older file predates the field. Absent must read as off, for the
    /// same reason the default is off -- an upgrade must not start holding
    /// a process the user never asked for.
    #[test]
    fn a_settings_file_without_the_field_keeps_the_ui_unloaded() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"keep_backend_running": false}"#).expect("write");
        assert!(!Settings::load(&path).keep_ui_loaded);
    }

    /// It survives a save. `persist_preferences` re-reads the file and
    /// copies field by field, so a field omitted from that list is silently
    /// discarded on every save -- which is a setting that will not stay on.
    #[test]
    fn keeping_the_ui_loaded_survives_persist_preferences() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("settings.json");
        let mut s = Settings::default();
        s.keep_ui_loaded = true;
        s.persist_preferences(&path).expect("persist");
        assert!(
            Settings::load(&path).keep_ui_loaded,
            "the setting was dropped on save, so turning it on in Preferences would not stick"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib settings:: 2>&1 | tail -20
```

Expected: compile error, `no field keep_ui_loaded on type Settings`.

- [ ] **Step 3: Add the field**

In `deskwarden/src/settings.rs`, immediately after the `keep_backend_running` field (:771):

```rust
    /// Whether the vault window's process stays resident, hidden, after a
    /// plain close.
    ///
    /// `false` (the default) is today's behaviour: closing the window ends
    /// its process, and the next open is a cold start -- measured at 263 ms
    /// to the first frame and 5.65 s to 1668 items on screen. `true` keeps
    /// that process alive with its viewport hidden, so every reopen is
    /// immediate, at the cost of roughly 100 MB held while the vault is
    /// unlocked.
    ///
    /// **The sibling of [`Settings::keep_backend_running`], and the same
    /// shape of trade**, with one difference worth knowing: this memory is
    /// held in a process that can be killed, not in the tray for the life
    /// of the session. That is only true because the window runs in a
    /// process of its own; before that split, "keep the UI loaded" would
    /// have meant keeping the OpenGL driver in the daemon until sign-out,
    /// which is the defect the split was for.
    ///
    /// **Off by default, and an older `settings.json` without this field
    /// parses as off.** A memory cost nobody chose is exactly the report
    /// this work came from.
    pub keep_ui_loaded: bool,
```

In `impl Default for Settings` (:1241), after `keep_backend_running: true,`:

```rust
            keep_ui_loaded: false,
```

In `persist_preferences` (:1377), add `keep_ui_loaded,` to the destructuring list after `keep_backend_running,`, and the copy after the `keep_backend_running` copy:

```rust
        on_disk.keep_ui_loaded = *keep_ui_loaded;
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib settings:: 2>&1 | grep -E "^test result"
```

Expected: `test result: ok.` The destructure in `persist_preferences` is exhaustive, so the compiler fails the build if you miss it -- that is the point of writing it that way.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/settings.rs && git commit -m "Add keep_ui_loaded, off by default"
```

---

### Task 2: The decision, as a pure function

**Files:**
- Modify: `deskwarden/src/ui_process.rs` (beside `UiVaultResult` at :276)
- Modify: `deskwarden/src/main.rs` (a pin in the test module beside `vault_follow_up`'s tests)

**Interfaces:**
- Consumes: `ui_process::UiVaultResult` (existing, six fields: `locked`, `needs_reauth`, `edited_settings`, `switch_to`, `add_account`, `remove_account`).
- Produces: `ui_process::OnClose::{Hide, Exit}` and `ui_process::on_close(keep_loaded: bool, result: &UiVaultResult) -> OnClose`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `deskwarden/src/ui_process.rs`:

```rust
    fn plain() -> UiVaultResult {
        UiVaultResult {
            locked: false,
            needs_reauth: false,
            edited_settings: None,
            switch_to: None,
            add_account: false,
            remove_account: false,
        }
    }

    /// The one case that hides: the setting is on and the user just closed
    /// the window.
    #[test]
    fn a_plain_close_hides_when_the_setting_is_on() {
        assert_eq!(on_close(true, &plain()), OnClose::Hide);
    }

    /// **Off means today's behaviour, exactly.** With the setting off no
    /// result hides, or the setting would not be a setting.
    #[test]
    fn nothing_hides_when_the_setting_is_off() {
        assert_eq!(on_close(false, &plain()), OnClose::Exit);
    }

    /// **Every outcome the daemon acts on exits**, so that the result file,
    /// the reap and the resettle all keep working unchanged. Each is
    /// asserted by name rather than in a loop: a loop that built the wrong
    /// value would pass six times.
    #[test]
    fn every_outcome_the_daemon_acts_on_exits() {
        let locked = UiVaultResult { locked: true, ..plain() };
        assert_eq!(on_close(true, &locked), OnClose::Exit, "a lock must reach the daemon");

        let reauth = UiVaultResult { needs_reauth: true, ..plain() };
        assert_eq!(on_close(true, &reauth), OnClose::Exit, "a re-auth must reach the daemon");

        let switch = UiVaultResult { switch_to: Some("other".into()), ..plain() };
        assert_eq!(on_close(true, &switch), OnClose::Exit, "a switch must reach the daemon");

        let add = UiVaultResult { add_account: true, ..plain() };
        assert_eq!(on_close(true, &add), OnClose::Exit, "an add must reach the daemon");

        let remove = UiVaultResult { remove_account: true, ..plain() };
        assert_eq!(on_close(true, &remove), OnClose::Exit, "a remove must reach the daemon");
    }

    /// **A preferences edit exits too, and this is the subtle one.**
    /// `vault_follow_up` returns `Done` for it -- editing preferences is
    /// not a reason a window closed -- but the daemon reads
    /// `edited_settings` ABOVE that match (`main.rs:7121`): it copies the
    /// edited settings into its own estate and runs
    /// `apply_disk_cache_change`. A window that hid after a visit to the
    /// gear would withhold both, and the daemon would go on running against
    /// settings the user had changed.
    #[test]
    fn a_preferences_edit_exits_even_though_its_follow_up_is_done() {
        let geared = UiVaultResult {
            edited_settings: Some(crate::settings::Settings::default()),
            ..plain()
        };
        assert_eq!(
            on_close(true, &geared),
            OnClose::Exit,
            "the window hid holding edited settings, so the daemon never applied them"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib ui_process:: 2>&1 | tail -15
```

Expected: `cannot find function on_close in this scope`.

- [ ] **Step 3: Write the function**

Add to `deskwarden/src/ui_process.rs`, after `UiVaultResult`:

```rust
/// What a closing vault window does with its process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnClose {
    /// Hide the viewport and stay resident, ready to be shown again.
    Hide,
    /// End the process, which is how every result gets home.
    Exit,
}

/// **Whether this close hides the window or ends its process.**
///
/// Hiding is safe for exactly one result: the empty one. Every field of
/// [`UiVaultResult`] is something the daemon acts on, and the only way any
/// of them reaches the daemon is this process exiting -- the result file is
/// named by pid and read when the child is reaped. A process that hid while
/// holding a set field would be a lock, a switch or a settings edit that
/// silently never happened.
///
/// **This deliberately does not mirror `vault_follow_up`'s `Done`.** That
/// function answers a different question -- what the daemon does next --
/// and it does not read `edited_settings`, because the daemon has already
/// applied it by then (`main.rs:7121`). Hiding on `Done` alone would
/// swallow a preferences edit. `the_hide_rule_is_stricter_than_done` in
/// `main.rs` holds the two together.
#[must_use]
pub fn on_close(keep_loaded: bool, result: &UiVaultResult) -> OnClose {
    let nothing_to_report = !result.locked
        && !result.needs_reauth
        && !result.add_account
        && !result.remove_account
        && result.switch_to.is_none()
        && result.edited_settings.is_none();
    if keep_loaded && nothing_to_report {
        OnClose::Hide
    } else {
        OnClose::Exit
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib ui_process:: 2>&1 | grep -E "^test result"
```

Expected: `test result: ok.`

- [ ] **Step 5: Add the cross-module pin**

The two rules live in different crates halves and must not drift. Add to `mod tests` in `deskwarden/src/main.rs`:

```rust
    /// **Hiding is stricter than `Done`, and must stay stricter.**
    ///
    /// If a future result made `vault_follow_up` return `Done` while
    /// `ui_process::on_close` said `Hide`, that outcome would be
    /// swallowed by a window that hid instead of reporting it. This walks
    /// every combination of the six fields and asserts the implication in
    /// the direction that matters: Hide implies Done, never the reverse.
    #[test]
    fn the_hide_rule_is_stricter_than_done() {
        let mut hides = 0;
        for bits in 0u8..64 {
            let result = deskwarden::ui_process::UiVaultResult {
                locked: bits & 1 != 0,
                needs_reauth: bits & 2 != 0,
                add_account: bits & 4 != 0,
                remove_account: bits & 8 != 0,
                switch_to: (bits & 16 != 0).then(|| "other".to_string()),
                edited_settings: (bits & 32 != 0)
                    .then(deskwarden::settings::Settings::default),
            };
            let window = vault_window::VaultWindowResult {
                locked: result.locked,
                needs_reauth: result.needs_reauth,
                edited_settings: result.edited_settings.clone(),
                switch_to: result.switch_to.clone(),
                add_account: result.add_account,
                remove_account: result.remove_account,
                account_details: None,
            };
            if deskwarden::ui_process::on_close(true, &result)
                == deskwarden::ui_process::OnClose::Hide
            {
                hides += 1;
                assert_eq!(
                    vault_follow_up(&window),
                    VaultFollowUp::Done,
                    "a result that hides does not land on `Done`, so hiding it loses \
                     whatever the daemon would have done about it: {result:?}"
                );
            }
        }
        assert_eq!(
            hides, 1,
            "control: exactly one of the 64 combinations should hide -- the empty one. \
             {hides} did, so this test is not pinning what it says"
        );
    }
```

- [ ] **Step 6: Run it**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --bin deskwarden the_hide_rule_is_stricter 2>&1 | grep -E "^test result"
```

Expected: `test result: ok. 1 passed`. If `UiVaultResult` does not derive `Debug`, add it -- it holds no secret material, only flags and a settings snapshot.

- [ ] **Step 7: Commit**

```bash
git add deskwarden/src/ui_process.rs deskwarden/src/main.rs && git commit -m "Decide hide-or-exit from the result the child already reports"
```

---

### Task 3: The show signal

**Files:**
- Create: `deskwarden/src/ui_show.rs`
- Modify: `deskwarden/src/lib.rs` (add `pub mod ui_show;`)

**Interfaces:**
- Produces: `ui_show::signal_name(pid: u32) -> String`, `ui_show::ShowEnv { pub create: fn(&str) -> Option<Signal>, pub set: fn(&str) -> bool }`, `ui_show::ShowEnv::production()`, `ui_show::Signal` (owns the handle, closes on drop, `wait(&self, timeout_ms: u32) -> bool`), `ui_show::ask_to_show(env: &ShowEnv, pid: u32) -> bool`.

- [ ] **Step 1: Write the failing tests**

Create `deskwarden/src/ui_show.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// **Per-process, and under `Local\`.** Two vault windows never exist
    /// at once today, but a name without the pid would make a stale event
    /// from a dead process wake the wrong window -- and `Local\` keeps it
    /// inside the logon session, which is the same scope the vault-service
    /// attachment slots use.
    #[test]
    fn the_name_is_scoped_to_the_logon_session_and_the_process() {
        let name = signal_name(1234);
        assert!(name.starts_with(r"Local\"), "not logon-session scoped: {name}");
        assert!(name.contains("1234"), "not per-process: {name}");
        assert_ne!(signal_name(1234), signal_name(1235));
    }

    /// **The right is a standard right, and this is the pin.**
    /// `vault_service` shipped `0x0010` for `SYNCHRONIZE`, which is
    /// `0x0010_0000`; every open returned ACCESS_DENIED and all 23 tests
    /// passed because the fake kernel never reached the call. A literal
    /// here would repeat it exactly.
    #[test]
    fn the_rights_come_from_the_windows_crate_not_a_literal() {
        let source = include_str!("ui_show.rs");
        let production = source.split("mod tests").next().expect("a production half");
        assert!(
            !production.contains("0x0010"),
            "a raw access-right literal is back in `ui_show`; use the `windows` crate's \
             constants, as `vault_service` now does after shipping this exact bug"
        );
        assert!(
            production.contains("SYNCHRONIZE"),
            "control: this module no longer names a synchronisation right at all, so the \
             assertion above is guarding nothing"
        );
    }

    /// The whole point, over the real kernel: a signal set by "the daemon"
    /// is seen by "the child". A fake would prove only that the fake works,
    /// which is the defect class this crate keeps finding.
    #[test]
    fn a_signal_set_from_outside_wakes_the_waiter() {
        let pid = std::process::id();
        let env = ShowEnv::production();
        let signal = (env.create)(&signal_name(pid)).expect("create the event");
        assert!(!signal.wait(0), "the event was signalled before anybody set it");
        assert!(ask_to_show(&env, pid), "setting the event failed");
        assert!(signal.wait(1_000), "the waiter never saw the signal");
        assert!(
            !signal.wait(0),
            "the event did not auto-reset, so one ask would show the window for ever"
        );
    }

    /// Asking a pid that has no window is a `false`, not a panic and not a
    /// hang. The daemon's fallback reads this to mean "spawn a fresh one".
    #[test]
    fn asking_a_process_that_has_no_signal_fails_cleanly() {
        assert!(!ask_to_show(&ShowEnv::production(), 0xFFFF_FFF0));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib ui_show:: 2>&1 | tail -10
```

Expected: `file not found for module ui_show` until `lib.rs` declares it, then unresolved names.

- [ ] **Step 3: Write the module**

Put this ABOVE the test module in `deskwarden/src/ui_show.rs`:

```rust
//! **Waking a hidden vault window.**
//!
//! When `keep_ui_loaded` is on, a plain close hides the window's viewport
//! and its process stays alive. `foreground::raise_process` cannot bring
//! that back -- a hidden viewport has no window to raise -- so the daemon
//! needs a way to say *show yourself* to a process it does not share memory
//! with.
//!
//! A named auto-reset event is that way, and it is the idiom this crate
//! already uses: `vault_service`'s attachment slots are named mutexes under
//! `Local\`, created by one process and opened by another.
//!
//! **Auto-reset, not manual.** A manual-reset event stays signalled until
//! somebody clears it, so a window shown once would be shown for ever after
//! -- and the reset would have to happen in the child, on a path that must
//! not be forgotten. Auto-reset makes one set mean one show.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    SYNCHRONIZATION_SYNCHRONIZE,
};

/// The event a UI process waits on to be shown.
///
/// Named by pid because that is the daemon's whole record that a window
/// exists -- it is what the spawn returned, what the result file is named
/// by, and what the show is aimed at. There is no second registry to
/// disagree with it.
#[must_use]
pub fn signal_name(pid: u32) -> String {
    format!(r"Local\Deskwarden-UI-Show-{pid}")
}

/// A live handle to the event, closed when this is dropped.
pub struct Signal(HANDLE);

impl Signal {
    /// Waits up to `timeout_ms` for somebody to set this event.
    ///
    /// `true` means it was signalled; `false` means the timeout passed or
    /// the wait failed. A caller must treat `false` as "carry on waiting",
    /// never as "show the window".
    #[must_use]
    pub fn wait(&self, timeout_ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.0, timeout_ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for Signal {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// The kernel calls this module makes, behind `fn` pointers so a test can
/// watch the decisions without the calls. `cfg(test)` seams are banned
/// crate-wide; this is the `ServiceEnv` idiom.
pub struct ShowEnv {
    /// Creates (or opens) the named event and keeps it alive until dropped.
    pub create: fn(&str) -> Option<Signal>,
    /// Sets the named event. `false` if no process is waiting on that name.
    pub set: fn(&str) -> bool,
}

impl ShowEnv {
    #[must_use]
    pub fn production() -> Self {
        Self { create: create_event, set: set_event }
    }
}

fn create_event(name: &str) -> Option<Signal> {
    let wide = HSTRING::from(name);
    // Auto-reset (`bManualReset` false), initially unsignalled.
    let handle = unsafe { CreateEventW(None, false, false, PCWSTR(wide.as_ptr())) }.ok()?;
    Some(Signal(handle))
}

fn set_event(name: &str) -> bool {
    let wide = HSTRING::from(name);
    // **`SYNCHRONIZATION_SYNCHRONIZE` from the crate, plus the right to
    // set.** See this module's test for why a literal is forbidden here.
    let opened =
        unsafe { OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(wide.as_ptr())) };
    match opened {
        Ok(handle) => {
            let set = unsafe { SetEvent(handle) }.is_ok();
            unsafe {
                let _ = CloseHandle(handle);
            }
            set
        }
        Err(_) => false,
    }
}

/// **Ask the UI process `pid` to show itself.**
///
/// `false` means no process is listening on that name -- it died, or it
/// never created the event. The caller's answer to `false` is to spawn a
/// fresh window, never to give up: under the one-window rule a refusal that
/// opens nothing is an *Open Vault* that never opens again.
#[must_use]
pub fn ask_to_show(env: &ShowEnv, pid: u32) -> bool {
    (env.set)(&signal_name(pid))
}
```

Add to `deskwarden/src/lib.rs`, in alphabetical position among the `pub mod` lines:

```rust
pub mod ui_show;
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib ui_show:: 2>&1 | grep -E "^test result|panicked"
```

Expected: `test result: ok. 4 passed`. If `SYNCHRONIZATION_SYNCHRONIZE` does not resolve, find its real path with:

```bash
grep -rn "SYNCHRONIZATION_SYNCHRONIZE" deskwarden/src/vault_service.rs
```

and use the same import that module uses -- do not substitute a literal.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/ui_show.rs deskwarden/src/lib.rs && git commit -m "A named auto-reset event for showing a hidden window"
```

---

### Task 4: The daemon learns to show

**Files:**
- Modify: `deskwarden/src/ui_process.rs` (`UiOpenDecision` at :189, `open_decision` at :209)
- Modify: `deskwarden/src/main.rs` (`OpenUiWindow` at :6039, `ask_for_the_vault_window` at :6087)

**Interfaces:**
- Consumes: `ui_show::ask_to_show`, `ui_show::ShowEnv::production()` from Task 3.
- Produces: `UiOpenDecision::ShowTheHiddenOne { pid }`; `open_decision(already_open: Option<u32>, hidden: bool)`; `OpenUiWindow { hidden: bool, .. }`.

- [ ] **Step 1: Write the failing tests**

In `mod tests` in `deskwarden/src/ui_process.rs`:

```rust
    /// A hidden window is shown, not spawned and not raised. Raising is
    /// what `FocusTheOpenOne` does and it cannot work here: a hidden
    /// viewport has no window for `raise_process` to bring forward.
    #[test]
    fn a_hidden_window_is_shown() {
        assert_eq!(
            open_decision(Some(77), true),
            UiOpenDecision::ShowTheHiddenOne { pid: 77 }
        );
    }

    /// A visible window is still focused, and nothing is spawned. Two vault
    /// windows on the same vault is two editors of the same records.
    #[test]
    fn a_visible_window_is_still_focused() {
        assert_eq!(open_decision(Some(77), false), UiOpenDecision::FocusTheOpenOne { pid: 77 });
    }

    /// No window at all is still a spawn, whatever `hidden` says -- there
    /// is nothing to hide.
    #[test]
    fn no_window_is_still_a_spawn() {
        assert_eq!(open_decision(None, false), UiOpenDecision::Spawn);
        assert_eq!(open_decision(None, true), UiOpenDecision::Spawn);
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib ui_process:: 2>&1 | tail -10
```

Expected: `this function takes 1 argument but 2 arguments were supplied`.

- [ ] **Step 3: Extend the decision**

In `deskwarden/src/ui_process.rs`, add the variant to `UiOpenDecision`:

```rust
    /// One is open but hidden, because `keep_ui_loaded` kept it resident
    /// after a plain close. Show *that* one.
    ShowTheHiddenOne { pid: u32 },
```

and replace `open_decision`'s body, keeping its existing doc and adding to it:

```rust
/// `hidden` is whether the open window has hidden itself after a plain
/// close. It is a separate argument rather than a third state of
/// `already_open` because the pid means the same thing either way -- the
/// process exists -- and only what to DO with it differs.
pub fn open_decision(already_open: Option<u32>, hidden: bool) -> UiOpenDecision {
    match (already_open, hidden) {
        (Some(pid), true) => UiOpenDecision::ShowTheHiddenOne { pid },
        (Some(pid), false) => UiOpenDecision::FocusTheOpenOne { pid },
        (None, _) => UiOpenDecision::Spawn,
    }
}
```

- [ ] **Step 4: Run to verify the unit tests pass**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib ui_process:: 2>&1 | grep -E "^test result"
```

Expected: `test result: ok.`

- [ ] **Step 5: Wire the daemon**

In `deskwarden/src/main.rs`, add the field to `OpenUiWindow` (:6039):

```rust
    /// Whether this window has hidden itself after a plain close, which it
    /// does only when `keep_ui_loaded` is on. A hidden window is still an
    /// open window for the one-window rule -- the next *Open Vault* shows
    /// it rather than starting a second.
    hidden: bool,
```

Set it at the one construction site, in `spawn_the_vault_window_in_its_own_process`:

```rust
    Some(OpenUiWindow { child, pid, opened_at: Instant::now(), hidden: false })
```

Replace the body of `ask_for_the_vault_window` (:6087) with:

```rust
    fn ask_for_the_vault_window(&mut self) -> bool {
        let hidden = self.vault.as_ref().is_some_and(|open| open.hidden);
        match deskwarden::ui_process::open_decision(self.vault_pid(), hidden) {
            deskwarden::ui_process::UiOpenDecision::FocusTheOpenOne { pid } => {
                let raised = deskwarden::foreground::raise_process(pid);
                log::info!(
                    "the vault window is already open as process {pid}; brought it forward \
                     ({raised:?}) rather than opening a second one"
                );
                true
            }
            deskwarden::ui_process::UiOpenDecision::ShowTheHiddenOne { pid } => {
                // **A failure here is not a refusal.** If the process died
                // between hiding and now, or never created its event, the
                // answer is a fresh window -- under the one-window rule a
                // refusal that opens nothing is an Open Vault that never
                // opens again.
                if deskwarden::ui_show::ask_to_show(
                    &deskwarden::ui_show::ShowEnv::production(),
                    pid,
                ) {
                    log::info!("asked the hidden vault window (process {pid}) to show itself");
                    if let Some(open) = self.vault.as_mut() {
                        open.hidden = false;
                    }
                    return true;
                }
                log::warn!(
                    "the hidden vault window (process {pid}) did not answer; forgetting it and \
                     starting a fresh one"
                );
                if let Some(mut open) = self.vault.take() {
                    let _ = open.child.kill();
                }
                match spawn_the_vault_window_in_its_own_process() {
                    Some(open) => {
                        self.vault = Some(open);
                        true
                    }
                    None => false,
                }
            }
            deskwarden::ui_process::UiOpenDecision::Spawn => {
                match spawn_the_vault_window_in_its_own_process() {
                    Some(open) => {
                        self.vault = Some(open);
                        true
                    }
                    None => false,
                }
            }
        }
    }
```

- [ ] **Step 6: Build and run the whole binary suite**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run RUSTFLAGS="-D warnings" cargo build --manifest-path deskwarden/Cargo.toml --bin deskwarden 2>&1 | grep -E "^(error|warning)" -A5
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --bin deskwarden 2>&1 | grep -E "^test result|^    [a-z]"
```

Expected: the build is clean. The pin `the_loop_polls_the_ui_child_and_waits_for_nothing` counts `ask_for_the_vault_window` occurrences and expects 3; this task adds no new call, so it should still pass. If a pin fails, **read its message before touching it** -- each names the mutation it caught.

- [ ] **Step 7: Commit**

```bash
git add deskwarden/src/ui_process.rs deskwarden/src/main.rs && git commit -m "The daemon shows a hidden window instead of spawning a second"
```

---

### Task 5: The Preferences row

**Files:**
- Modify: `deskwarden/src/prefs_ui.rs` (label constants near :132, the row near :3096)

**Interfaces:**
- Consumes: `Settings::keep_ui_loaded` from Task 1; `toggle_row(ui, label, description, value) -> bool` (existing).

- [ ] **Step 1: Write the failing test**

In `mod tests` in `deskwarden/src/prefs_ui.rs`:

```rust
    /// **The label says what it costs.** `keep_backend_running`'s row says
    /// what holding the backend costs, and this trade is the same shape:
    /// somebody turning it on should not discover the memory afterwards.
    #[test]
    fn the_ui_loaded_row_names_both_halves_of_the_trade() {
        let description = UI_LOADED_DESCRIPTION;
        assert!(
            description.contains("MB"),
            "the row does not say what it costs, so the memory is a surprise: {description}"
        );
        assert!(
            UI_LOADED_LABEL.to_lowercase().contains("open"),
            "the row does not say what it buys: {UI_LOADED_LABEL}"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib prefs_ui:: 2>&1 | tail -8
```

Expected: `cannot find value UI_LOADED_DESCRIPTION in this scope`.

- [ ] **Step 3: Add the constants and the row**

Next to `BACKEND_LABEL` (:132) in `deskwarden/src/prefs_ui.rs`:

```rust
/// The label for [`crate::settings::Settings::keep_ui_loaded`].
const UI_LOADED_LABEL: &str = "Open the vault instantly";

/// The description under [`UI_LOADED_LABEL`].
///
/// Names the cost as well as the benefit, as [`BACKEND_LABEL`]'s does: this
/// is the same trade in a different process, and a user who turns it on
/// should not meet the memory afterwards in Task Manager.
const UI_LOADED_DESCRIPTION: &str =
    "Keeps the vault window loaded and hidden after you close it, so it opens \
     immediately next time. Holds about 100 MB while the vault is unlocked. \
     Locking, switching account or changing these settings closes it fully.";
```

Immediately after the `keep_backend_running` row (:3096), add:

```rust
        state.settings.keep_ui_loaded = toggle_row(
            ui,
            UI_LOADED_LABEL,
            UI_LOADED_DESCRIPTION,
            state.settings.keep_ui_loaded,
        );
```

- [ ] **Step 4: Run the tests**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib prefs_ui:: 2>&1 | grep -E "^test result"
```

Expected: `test result: ok.` Several `prefs_ui` tests count rows or pin page contents; if one fails, read its message -- a row genuinely was added, so a count that names the page is re-pinned with a commit message saying what moved.

- [ ] **Step 5: See it**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo run --manifest-path deskwarden/Cargo.toml --example ui_preview -- --all 2>&1 | tail -3
```

Expected: the example writes its PNGs without panicking. Open the preferences one and confirm the new row reads correctly under the backend row.

- [ ] **Step 6: Commit**

```bash
git add deskwarden/src/prefs_ui.rs && git commit -m "A Preferences row for keeping the vault window loaded"
```

---

### Task 6: The child hides and comes back

**Files:**
- Modify: `deskwarden/src/vault_window/mod.rs` (`ChromeAction::Close` at :2080, `run` at :5193)
- Modify: `deskwarden/src/main.rs` (`run_as_a_ui_process` at :10039)

**Interfaces:**
- Consumes: `ui_process::on_close`/`OnClose` (Task 2), `ui_show::{ShowEnv, Signal, signal_name}` (Task 3), `Settings::keep_ui_loaded` (Task 1), `vault_service::{attach, Attachment, ServiceEnv}` (existing -- `Attachment` releases its slot when dropped).
- Produces: `vault_window::run` gains a final parameter `hide_instead_of_closing: Option<HideHooks>`, where `pub struct HideHooks { pub should_hide: fn(&VaultWindowResult) -> bool, pub on_hidden: Box<dyn Fn()>, pub on_shown: Box<dyn Fn()>, pub wait_for_show: Box<dyn Fn() -> bool> }`.

**This is the task with the real risk in it.** `eframe::run_native` cannot be called twice in one process -- winit's event loop is consumed -- so hiding must happen *inside* the running app, never by returning and re-running.

- [ ] **Step 1: Write the failing test**

In `mod tests` in `deskwarden/src/vault_window/mod.rs`:

```rust
    /// **Hiding happens inside the running app, and this pin says so.**
    ///
    /// `eframe::run_native` consumes winit's event loop; a second call in
    /// the same process does not start a second window, it fails. So a
    /// "hide" implemented by returning from `run` and calling it again
    /// would be a window that never comes back -- and it would look right
    /// in review. The hide must be a viewport command sent from inside the
    /// update closure.
    #[test]
    fn the_hide_is_a_viewport_command_not_a_second_run() {
        let source = include_str!("mod.rs");
        let production = source.split("mod tests").next().expect("a production half");
        assert!(
            production.contains(concat!("ViewportCommand::", "Visible(false)")),
            "nothing in this module hides the viewport, so `keep_ui_loaded` has no way to \
             keep a window that can come back"
        );
        assert_eq!(
            production.matches("run_native").count(),
            1,
            "`run_native` is called more than once, which winit does not support: the second \
             window never opens and the user's Open Vault does nothing"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml --lib vault_window::tests::the_hide_is_a_viewport 2>&1 | tail -8
```

Expected: FAIL, "nothing in this module hides the viewport".

- [ ] **Step 3: Add the hooks type and the hide path**

In `deskwarden/src/vault_window/mod.rs`, above `run`:

```rust
/// **What a window does instead of closing, when it may stay resident.**
///
/// Boxed closures rather than `fn` pointers because the child needs to
/// capture its attachment slot and its event handle. This is not a
/// `cfg(test)` seam -- production passes `Some(..)` when
/// `keep_ui_loaded` is on and `None` when it is off, and a test passes its
/// own hooks to watch the sequence without a window.
pub struct HideHooks {
    /// Whether this outcome may hide. Production hands in
    /// `ui_process::on_close(true, ..) == Hide`.
    pub should_hide: fn(&VaultWindowResult) -> bool,
    /// Called after the viewport is hidden: release the attachment slot, so
    /// save-memory can still stop `bw serve` behind a hidden window.
    pub on_hidden: Box<dyn Fn()>,
    /// Blocks until the daemon asks for the window. `false` means the wait
    /// failed and the process should exit rather than hang hidden for ever.
    pub wait_for_show: Box<dyn Fn() -> bool>,
    /// Called after the viewport is shown again: retake the attachment slot.
    pub on_shown: Box<dyn Fn()>,
}
```

In the `ChromeAction::Close` arm (:2080), replace the single send with a call to a helper that either hides or closes. Add the helper as a method on the app struct that owns `ChromeAction` handling, and give it the app's current outcome:

```rust
    /// Close, or hide and wait to be shown again.
    ///
    /// **The wait runs on a worker, never on this thread.** The update
    /// closure is the frame thread; blocking it would freeze the hidden
    /// window's process so that the show it is waiting for could never be
    /// painted. The worker wakes the UI with `request_repaint`.
    fn close_or_hide(&mut self, ctx: &egui::Context) {
        let Some(hooks) = self.hide_hooks.as_ref() else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        if !(hooks.should_hide)(&self.outcome()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        (hooks.on_hidden)();
        log::info!("the vault window hid itself; this process stays loaded for the next open");
        self.hidden = true;
        ctx.request_repaint();
    }
```

Drive the wait from the update closure, once per hide, on a worker:

```rust
        // **Hidden: wait off-thread for the daemon to ask.**
        if self.hidden && !self.waiting_for_show {
            self.waiting_for_show = true;
            let ctx_for_wait = ctx.clone();
            let woken = Arc::clone(&self.woken);
            if let Some(hooks) = self.hide_hooks.as_ref() {
                let wait = hooks.wait_for_show.clone_box();
                std::thread::spawn(move || {
                    let shown = wait();
                    woken.store(if shown { 1 } else { 2 }, Ordering::SeqCst);
                    ctx_for_wait.request_repaint();
                });
            }
        }
        match self.woken.swap(0, Ordering::SeqCst) {
            1 => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                if let Some(hooks) = self.hide_hooks.as_ref() {
                    (hooks.on_shown)();
                }
                self.hidden = false;
                self.waiting_for_show = false;
                log::info!("the vault window was asked to show itself and did");
            }
            2 => {
                log::warn!("the hidden vault window's wait failed; closing rather than \
                            staying hidden with nothing able to show it");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            _ => {}
        }
```

Add the three fields to the app struct (`hide_hooks: Option<HideHooks>`, `hidden: bool`, `waiting_for_show: bool`, `woken: Arc<AtomicU8>`) and initialise them in `build_frame`. Since `wait_for_show` is a `Box<dyn Fn>` that must move to a thread, declare it as `Arc<dyn Fn() -> bool + Send + Sync>` instead of `Box` and clone the `Arc` -- adjust `HideHooks` accordingly and drop the `clone_box` above.

Also handle Alt+F4, which does not go through `ChromeAction`:

```rust
        if ctx.input(|i| i.viewport().close_requested()) && self.hide_hooks.is_some() && !self.hidden {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_or_hide(ctx);
        }
```

- [ ] **Step 4: Wire the child**

In `deskwarden/src/main.rs`, in `run_as_a_ui_process`, before the `vault_window::run` call:

```rust
    // **The slot this process holds while it is USING the vault**, released
    // while hidden so that save-memory can still stop `bw serve` behind a
    // hidden window. Without this, `keep_ui_loaded` would silently pin
    // ~111 MB of backend against a user who turned `keep_backend_running`
    // off for exactly that reason.
    let service_env = deskwarden::vault_service::ServiceEnv::production();
    let attachment = std::sync::Mutex::new(deskwarden::vault_service::attach(&service_env));

    let hide_hooks = settings.keep_ui_loaded.then(|| {
        let signal = (deskwarden::ui_show::ShowEnv::production().create)(
            &deskwarden::ui_show::signal_name(std::process::id()),
        );
        vault_window::HideHooks {
            should_hide: |result| {
                deskwarden::ui_process::on_close(
                    true,
                    &deskwarden::ui_process::UiVaultResult {
                        locked: result.locked,
                        needs_reauth: result.needs_reauth,
                        edited_settings: result.edited_settings.clone(),
                        switch_to: result.switch_to.clone(),
                        add_account: result.add_account,
                        remove_account: result.remove_account,
                    },
                ) == deskwarden::ui_process::OnClose::Hide
            },
            on_hidden: Box::new(move || { /* drop the attachment */ }),
            wait_for_show: std::sync::Arc::new(move || {
                signal.as_ref().is_some_and(|s| s.wait(windows::Win32::System::Threading::INFINITE))
            }),
            on_shown: Box::new(move || { /* retake the attachment */ }),
        }
    });
```

Capture `attachment` in `on_hidden`/`on_shown` by `Arc<Mutex<Option<Attachment>>>`: `on_hidden` does `*guard = None`, `on_shown` does `*guard = vault_service::attach(&service_env)`. Pass `hide_hooks` as the new final argument to `vault_window::run`.

If `should_hide`'s `fn`-pointer form cannot borrow, change the field to `Arc<dyn Fn(&VaultWindowResult) -> bool + Send + Sync>` -- it takes no captures in production, so either form works; pick whichever compiles without a clone in the hot path.

- [ ] **Step 5: Build and run the suite**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run RUSTFLAGS="-D warnings" cargo build --manifest-path deskwarden/Cargo.toml --bin deskwarden 2>&1 | grep -E "^(error|warning)" -A6
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml 2>&1 | grep -E "^test result|^    [a-z]"
```

Expected: clean build; `the_hide_is_a_viewport_command_not_a_second_run` passes. Compare any `tests::` failures against a `git stash` baseline before assuming they are yours.

- [ ] **Step 6: Commit**

```bash
git add deskwarden/src/vault_window/mod.rs deskwarden/src/main.rs && git commit -m "The vault window hides instead of exiting when it may stay loaded"
```

---

### Task 7: Prove it on the running app

**Files:** none -- this task changes nothing and exists because none of the above observes a real process.

- [ ] **Step 1: Build release and start clean**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo build --manifest-path deskwarden/Cargo.toml --bin deskwarden --release 2>&1 | tail -2
powershell -NoProfile -Command "Get-Process deskwarden -ErrorAction SilentlyContinue | Stop-Process"
```

- [ ] **Step 2: Turn the setting on, with save-memory on too**

Edit `%APPDATA%\Deskwarden\Deskwarden\config\settings.json` so that `"keep_ui_loaded": true` and `"keep_backend_running": false`. Both together are the combination the attachment-slot release exists for.

- [ ] **Step 3: Launch, close the window, and confirm it hid**

```bash
powershell -NoProfile -Command "Start-Process 'E:\_dw_agent\run\release\deskwarden.exe'"
```

Close the vault window with its titlebar X, then:

```bash
powershell -NoProfile -Command "Get-Process deskwarden | Select-Object Id,@{n='MB';e={[math]::Round(\$_.WorkingSet64/1MB,1)}},MainWindowTitle | Format-Table -AutoSize | Out-String"
```

Expected: **two** processes still, the UI one with an empty `MainWindowTitle`. The log says `the vault window hid itself`.

- [ ] **Step 4: Confirm the backend stopped behind it**

```bash
powershell -NoProfile -Command "Get-Process bw -ErrorAction SilentlyContinue | Measure-Object | Select-Object -Expand Count"
```

Expected: `0`. A non-zero count means the attachment slot was not released and this setting is quietly pinning ~111 MB of backend -- the defect Task 6's `on_hidden` exists to prevent.

- [ ] **Step 5: Reopen from the tray and confirm it is the same process**

Click *Open Vault*. Then:

```bash
LOG="C:/Users/plato/AppData/Roaming/Deskwarden/Deskwarden/config/deskwarden.log"
grep -E "hid itself|asked to show|is process" "$LOG" | tail -5
```

Expected: `asked the hidden vault window (process N) to show itself` with **the same N** as the original spawn, and no new `the vault window is process` line. The window should appear with no perceptible delay.

- [ ] **Step 6: Confirm the daemon never gained the driver**

```bash
powershell -NoProfile -Command "\$d = Get-Process deskwarden | Sort-Object StartTime | Select-Object -First 1; \$d.Modules | Where-Object { \$_.ModuleName -match 'nvoglv' } | Measure-Object | Select-Object -Expand Count"
```

Expected: `0`. The daemon must still be the ~33 MB process it became when the window moved out.

- [ ] **Step 7: Confirm a lock still exits**

Lock the vault from inside the window. Expected: the UI process is **gone** from `Get-Process`, the daemon logs `UI process N came home with`, and the session resettles. This is the rule from Task 2 observed end to end.

- [ ] **Step 8: Confirm a preferences edit still exits**

Reopen the vault, open the gear, change any setting, close the window. Expected: the UI process is gone and the daemon applied the change -- this is the case `on_close` treats more strictly than `vault_follow_up` does.

- [ ] **Step 9: Restore the machine's settings and commit nothing**

Set `keep_ui_loaded` and `keep_backend_running` back to whatever the owner had. This task has no commit.

---

## Self-review

**Spec coverage.** The setting (Task 1), hide-or-exit including the `edited_settings` refinement (Task 2), the named event (Task 3), `hidden` plus the third `open_decision` answer (Task 4), the Preferences row (Task 5), the child's hide/show and the attachment-slot release (Task 6), and every bullet of "How it will be known to work" (Task 7). The spec's "adjacent, not in scope" Win+L item has deliberately no task.

**Known soft spots**, flagged rather than hidden:

1. **Task 6 is the only task whose code is written against structure this plan did not read in full** -- the vault window app's struct and its `ChromeAction` dispatch. The field names and the exact place to hang `close_or_hide` will need adjusting to what is actually there. The pin in Step 1 and the two hard constraints (one `run_native`; the wait never on the frame thread) are what must survive that adjustment.
2. **`should_hide` may need to be an `Arc<dyn Fn>`** rather than a `fn` pointer; Step 4 says so and either satisfies the tests.
3. **`INFINITE` waits are a hang if the daemon dies.** A daemon that exits without killing its hidden child leaves a process waiting for ever. `close_on_quit` already kills the open window on a user quit, and `farewell_to_an_open_window` decides that; confirm during Task 6 that it reaches a hidden window too, and if it does not, that is a real gap to fix inside Task 6 rather than a later one.

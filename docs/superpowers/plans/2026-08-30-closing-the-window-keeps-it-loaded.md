# Closing the Window Keeps It Loaded Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** With *Open the vault instantly* on, a user visits Preferences, closes the vault window, and the next open is instant — no new process, no re-unlock, no Windows Hello. The preference they changed still reaches the daemon, still runs `apply_disk_cache_change`, and now reaches it *sooner* than before: the moment the modal closes rather than the moment the window does.

**Architecture:** This implements `docs/superpowers/specs/2026-08-30-closing-the-window-keeps-it-loaded-design.md`, which chose option 2 — a live channel — over "the child persists and the daemon re-reads" (loses `apply_disk_cache_change`'s refusal, which only works because the daemon writes last) and over "make the exit cheap" (the exit costs ~50 MB of GL arenas and a 1–3 s cold start whatever the unlock costs).

One sentence governs everything below: **`edited_settings` gets a second way home, so `ui_process::on_close` stops having to be the thing that guarantees it gets home at all.** The child hands the settings to the daemon over a file plus a doorbell event while it is still alive; the daemon applies them through the same `apply_edited_settings` the tray and the vault loop use; and only *then* may a close hide instead of exit. `SetEvent` returning `false` — nobody listening — is the whole failure story, and it degrades to exactly today's behaviour.

**Tech Stack:** Rust, egui/eframe, `windows` crate named events (the `ui_show::Signal`/`ShowEnv` types, reused verbatim), `serde_json`, `std::fs` temp-then-rename.

## Global Constraints

- `cfg(test)` seams are banned crate-wide; seams are `fn`-pointer structs or boxed hooks in production code (`ShowEnv`, `HideHooks`, `VaultFrameEnv`).
- Build with `RUSTFLAGS="-D warnings"` and run `cargo test --no-run` under the same flags; zero warnings on both.
- `export CARGO_TARGET_DIR=/e/_dw_agent/run` — **never create a second target directory.** ~20 GB free, that one is already 14 GB.
- Tests must not pass vacuously: every negative assertion carries a positive control. The house defect is a test that passes because it never reached the thing it names.
- **Judge a failing test by reading it, never by its name prefix.** A `the_hide_rule_*` failure in this branch is expected in Task 6 and is a finding anywhere else.
- **Do not disturb the three `password_lifetime_tests` allocator guards.** Nothing in this plan touches the sign-in path; if a step appears to, stop and report.
- No test may touch the network, a real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, or spawn `bw`. Named-kernel-object tests are the documented exception (`ui_show`'s existing tests) and must use a pid this process does not have.
- Commit with explicit paths and `-F` a message file. Never `git add -A`, `--amend`, `reset`, or `rebase`.
- Branch: `closing-the-window-keeps-it-loaded`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/ui_show.rs` (modify) | `settings_name(pid)` — the doorbell's name, beside the show signal's and the visibility mutex's. |
| `deskwarden/src/ui_process.rs` (modify) | `edited_settings_path`, `write_edited_settings`, `read_edited_settings`, `forget_edited_settings`; `on_close`'s third parameter. |
| `deskwarden/src/vault_window/mod.rs` (modify) | `HideHooks::deliver_settings`, `deliver_if_needed`, `effective_keep_ui_loaded`, `close_or_hide`'s new decision, the modal-close delivery, minimize's doc and pin. |
| `deskwarden/src/main.rs` (modify) | `apply_edited_settings` (the de-duplication), `OpenUiWindow::settings_doorbell`, `UiWindows::take_edited_settings`, the loop's poll, the child's production `deliver_settings` hook, the re-pinned 64-combination test. |

**No new file.** The channel is two functions in a module whose entire job is already "the child's answer crossing to the daemon" (`ui_process`) and one name in the module whose entire job is already "named objects between the two" (`ui_show`). A third module would split a boundary that reads better whole.

---

### Task 1: One write-back, not three

**Files:** modify `deskwarden/src/main.rs`

The vault loop's write-back carries the comment *"Wiring it into one shell only is this file's house defect."* There are two shells today. Task 8 adds a third. Extract first, so that the third caller is a line rather than a copy.

**Interfaces**

- *Produces:* `fn apply_edited_settings(cache: &VaultCache, settings: &mut settings::Settings, settings_path: &Path, edited: settings::Settings)`.
- *Consumes:* `apply_disk_cache_change`, `Settings::persist_preferences`, `deskwarden::clipboard::configure`.
- *Callers after this task:* the tray's `preferences_id` handler (~`main.rs:2585`), the vault loop's `if let Some(edited)` block (~`main.rs:7343`).

The two existing bodies are identical in effect. The tray writes `if !apply(..) { false } else { edited.cache_vault_to_disk }`; the vault loop writes `apply(..) && edited.cache_vault_to_disk`. Enumerate the four inputs and they agree, so this is a de-duplication and not a generalisation over a difference. **If they turn out to disagree on any input, stop and report** — that is a live bug, not a refactor.

- [ ] **Step 1: Write the failing test**

In `main.rs`'s test module, beside the existing settings tests:

```rust
/// **The one place a preference edit lands, and it runs the disk-cache
/// side effect.**
///
/// Asserted at the point of effect -- the file on disk -- rather than over
/// a boolean, because `apply_disk_cache_change` is a side effect that can
/// REFUSE, and a test that read the flag back would pass against a
/// function that never called it.
#[test]
fn applying_an_edit_turns_the_disk_cache_on_and_off_for_real() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let settings_path = dir.path().join("settings.json");
    let cache = VaultCache::new_in(dir.path());
    let mut settings = settings::Settings::default();
    settings.cache_vault_to_disk = false;

    // Control: nothing on disk before anybody asks for it.
    assert!(
        cache.disk_cache_path().is_none_or(|p| !p.exists()),
        "control: the disk cache existed before the edit that asks for it"
    );

    apply_edited_settings(
        &cache,
        &mut settings,
        &settings_path,
        settings::Settings { cache_vault_to_disk: true, ..settings.clone() },
    );
    assert!(settings.cache_vault_to_disk, "the edit did not reach the estate");
    assert!(
        settings::Settings::load(&settings_path).cache_vault_to_disk,
        "the edit was not persisted"
    );

    apply_edited_settings(
        &cache,
        &mut settings,
        &settings_path,
        settings::Settings { cache_vault_to_disk: false, ..settings.clone() },
    );
    assert!(!settings.cache_vault_to_disk);
    assert!(
        cache.disk_cache_path().is_none_or(|p| !p.exists()),
        "turning the disk cache off left the encrypted copy on disk; \
         `apply_disk_cache_change` was not run from the one write-back"
    );
}

/// An edit that changes nothing writes nothing -- the `edited != *settings`
/// guard both existing shells have, kept in the extraction.
#[test]
fn an_edit_that_changes_nothing_does_not_touch_the_file() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let settings_path = dir.path().join("settings.json");
    let cache = VaultCache::new_in(dir.path());
    let mut settings = settings::Settings::default();

    apply_edited_settings(&cache, &mut settings, &settings_path, settings.clone());
    assert!(
        !settings_path.exists(),
        "a visit to the gear that changed nothing still wrote settings.json"
    );

    // Control: the same call with a real change DOES write, so the
    // assertion above is not passing because nothing works.
    apply_edited_settings(
        &cache,
        &mut settings,
        &settings_path,
        settings::Settings { check_breaches: !settings.check_breaches, ..settings.clone() },
    );
    assert!(settings_path.exists(), "control: a real change did not write either");
}
```

**`VaultCache::new_in` and `Settings::load`'s exact spellings must be read out of the source before writing this** — use whatever the neighbouring disk-cache tests in `main.rs` already use to build a cache in a temp directory, and copy that idiom rather than inventing one. If no such neighbour exists, stop and report.

Run: fails to resolve `apply_edited_settings`.

- [ ] **Step 2: Write the function**

```rust
/// **Apply one edited `Settings` to the daemon's estate.**
///
/// The disk-cache side effect (which can refuse), the assignment, the
/// preferences-only save, and the clipboard re-install -- in that order,
/// because `apply_disk_cache_change` must decide before the value it may
/// veto is written anywhere.
///
/// **Three shells reach this and it is deliberately one function.** The
/// tray's Preferences item, the gear inside the vault window, and the live
/// channel a resident window delivers over. Two copies of this already
/// existed and the vault loop's carried a comment calling the duplication
/// "this file's house defect"; a third copy is what this replaces.
///
/// `persist_preferences`, never a whole-struct `save`: `settings.vault_window`
/// is whatever was on disk at startup, and `vault_window::run` writes fresh
/// geometry straight to the same file on its way out. A whole-struct write
/// here silently reverts the size and position the user just left.
fn apply_edited_settings(
    cache: &VaultCache,
    settings: &mut settings::Settings,
    settings_path: &Path,
    edited: settings::Settings,
) {
    if edited == *settings {
        return;
    }
    let keep_disk_cache = apply_disk_cache_change(
        cache,
        settings.cache_vault_to_disk,
        edited.cache_vault_to_disk,
    ) && edited.cache_vault_to_disk;
    *settings = settings::Settings { cache_vault_to_disk: keep_disk_cache, ..edited };
    if let Err(e) = settings.persist_preferences(settings_path) {
        log::warn!("could not save settings: {e}");
    }
    deskwarden::clipboard::configure(settings.clipboard_clearing());
}
```

Run: green.

- [ ] **Step 3: Move the tray handler onto it**

Replace the body of the `if event.id == tray.preferences_id` block's `if edited != estate.settings { .. }` with a single call, keeping every surrounding comment that is about the *tray* (the blocking note, the `drain_requests_queued_behind_a_window` sweep). Move the comments that are about the *write-back* — the `persist_preferences`-not-`save` paragraph and the `apply_disk_cache_change` ordering note — onto `apply_edited_settings`'s doc rather than deleting them.

```rust
let edited = prefs_ui::run(estate.settings.clone());
apply_edited_settings(&estate.cache, &mut estate.settings, &settings_path, edited);
```

Run the whole suite. Green.

- [ ] **Step 4: Move the vault loop's write-back onto it**

**Read `main.rs`'s loop-tail source pins before touching this.** `the_vault_loop_tail_reads_edited_settings_once` (~`main.rs:20285`) asserts `result.edited_settings.is_some()` appears **zero** times and counts occurrences of the word `edited_settings` in the tail; `nothing_outside_the_two_branch_bodies_may_jump` scans the same span for control flow. The replacement must keep the field read exactly once and introduce no `continue`/`return`/`?`:

```rust
if let Some(edited) = result.edited_settings.clone() {
    apply_edited_settings(&est.cache, &mut est.settings, deps.settings_path, edited);
}
```

If a source pin fails, **read it and satisfy what it actually asserts** — do not relax it. These pins are the last defence against the v0.5.0 defect (a `continue` here made Lock silently not lock for the rest of a window's life).

Run the whole suite. Green, including `nothing_between_the_window_closing_and_the_branches_may_jump`.

---

### Task 2: The doorbell's name

**Files:** modify `deskwarden/src/ui_show.rs`

**Interfaces**

- *Produces:* `pub fn settings_name(pid: u32) -> String`.
- *Consumes:* nothing new. `Signal`, `ShowEnv`, `create_show_event`, `set_show_event` are reused unchanged — `Signal::wait(0)` is already the non-blocking read the daemon needs.

The direction is the mirror of `signal_name`'s: the **daemon** creates and waits, the **child** sets. Auto-reset for the reason this module already gives — a token consumed once, with the kernel doing the reset.

- [ ] **Step 1: Write the failing test**

```rust
/// A THIRD name, and the third one is the daemon's ear rather than the
/// child's. Sharing a name with either of the other two would have the
/// daemon consuming the child's show signal, or the child's visibility
/// mutex answering a question about settings.
#[test]
fn the_settings_doorbell_is_its_own_name() {
    let name = settings_name(1234);
    assert!(name.starts_with(r"Local\"), "not logon-session scoped: {name}");
    assert!(name.contains("1234"), "not per-process: {name}");
    assert_ne!(name, signal_name(1234), "the doorbell shares a name with the show signal");
    assert_ne!(name, visible_name(1234), "the doorbell shares a name with the visibility mutex");
    assert_ne!(settings_name(1234), settings_name(1235));
}

/// **Over the real kernel, in the direction production uses it**: the
/// daemon creates and polls with a zero timeout, the child sets, and one
/// set is consumed by one read.
///
/// The zero-timeout poll is the whole point -- `main`'s loop is what
/// answers the hotkey and can never block -- so it is what is asserted,
/// not a convenient long wait.
#[test]
fn the_daemon_can_poll_the_doorbell_without_blocking() {
    let pid = std::process::id() ^ 0x3333;
    let env = ShowEnv::production();
    let ear = (env.create)(&settings_name(pid)).expect("the event should be creatable");

    assert!(!ear.wait(0), "the doorbell rang before anybody pressed it");
    assert!((env.set)(&settings_name(pid)), "pressing the doorbell failed");
    assert!(ear.wait(0), "a rung doorbell did not read as rung on a zero-timeout poll");
    assert!(!ear.wait(0), "the doorbell did not auto-reset; one edit would be applied forever");
}

/// Nobody listening is a clean `false`. The child reads that as "keep the
/// old transport": do not hide, exit and carry the settings home in the
/// result file, exactly as before this feature.
#[test]
fn pressing_a_doorbell_nobody_holds_fails_cleanly() {
    assert!(!(ShowEnv::production().set)(&settings_name(0xFFFF_FFE0)));
}
```

Run: fails to resolve `settings_name`.

- [ ] **Step 2: Add the name**

```rust
/// **The name a UI process presses to say "I have edited settings for
/// you".**
///
/// The mirror of [`signal_name`]: that one is the daemon asking the child
/// to show itself, this one is the child asking the daemon to read a file.
/// Same `Local\` scope and same per-pid keying for the same two reasons --
/// a global name would cross between users, and a name without the pid
/// would let a dead process's doorbell be answered on behalf of a live one.
///
/// **Created by the DAEMON**, right after the spawn that produces the pid,
/// and polled with `Signal::wait(0)` once per pass of `main`'s loop. A
/// blocking wait is impossible there: that loop is what drains the hotkey
/// and answers the tray.
///
/// Auto-reset, like [`signal_name`]'s and unlike `single_instance`'s: this
/// means *there is something to read now*, which is a token to be consumed.
#[must_use]
pub fn settings_name(pid: u32) -> String {
    format!(r"Local\Deskwarden-UI-Settings-{pid}")
}
```

Run: green. Note that `no_access_right_is_written_as_a_literal` still passes — this adds no kernel call.

---

### Task 3: The payload file, written so a reader can never see half of it

**Files:** modify `deskwarden/src/ui_process.rs`

**Interfaces**

- *Produces:* `edited_settings_path(config_dir, pid)`, `write_edited_settings(path, &Settings) -> io::Result<()>`, `read_edited_settings(path) -> Option<Settings>`, `forget_edited_settings(path)`.
- *Consumes:* `crate::settings::Settings`, `serde_json`, `std::fs`.

**Deliberately not the result file.** `ui-result-<pid>.json` is the child's last act, written once on the way out and `forget_result`ed by the daemon after a reap. Writing it early would put a truncating write and a delete on one path from two processes.

- [ ] **Step 1: Write the failing test**

```rust
/// **A different file from the result**, and that is the point: the result
/// file is written once on the way out and deleted by the daemon after the
/// reap. A live channel sharing it would be a truncating write racing a
/// delete.
#[test]
fn the_live_settings_file_is_not_the_result_file() {
    let dir = Path::new(r"C:\config");
    assert_ne!(edited_settings_path(dir, 77), result_path(dir, 77));
    assert_ne!(edited_settings_path(dir, 77), edited_settings_path(dir, 78));
    assert!(
        edited_settings_path(dir, 77).to_string_lossy().contains("77"),
        "not named by pid, so a dead window's file could be read as a live one's"
    );
}

/// **A reader never sees half a file**, because the write lands by rename.
///
/// Asserted by writing a SECOND, different settings over the first and
/// reading back: a truncate-in-place implementation passes the round trip
/// but leaves a window in which the file is empty. The temp file is
/// asserted gone, which is the observable consequence of the rename
/// actually being the landing.
#[test]
fn an_edited_settings_file_lands_whole_and_leaves_no_temp_behind() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = edited_settings_path(dir.path(), 77);

    assert!(read_edited_settings(&path).is_none(), "control: read something before anything was written");

    let first = Settings::default();
    write_edited_settings(&path, &first).expect("the write should succeed");
    assert_eq!(read_edited_settings(&path).as_ref(), Some(&first));

    let second = Settings { check_breaches: !first.check_breaches, ..first.clone() };
    write_edited_settings(&path, &second).expect("the second write should succeed");
    assert_eq!(
        read_edited_settings(&path).as_ref(),
        Some(&second),
        "the second delivery did not replace the first"
    );

    let strays: Vec<_> = std::fs::read_dir(dir.path())
        .expect("readable")
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(strays.is_empty(), "the write left temp files behind: {strays:?}");

    forget_edited_settings(&path);
    assert!(
        read_edited_settings(&path).is_none(),
        "the daemon deleted the file and can still read it"
    );
}

/// Unparseable is `None` and a log line, never a panic -- the same answer
/// `read_result` gives, and for the same reason: the daemon's response to
/// a file it cannot read is to act on nothing, and the user's response is
/// to change the setting again.
#[test]
fn an_unparseable_delivery_is_ignored_rather_than_fatal() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = edited_settings_path(dir.path(), 77);
    std::fs::write(&path, "{ not json").expect("writable");
    assert!(read_edited_settings(&path).is_none());

    // Control: the same path with real content reads back, so the
    // assertion above is not passing because the path is wrong.
    write_edited_settings(&path, &Settings::default()).expect("writable");
    assert!(read_edited_settings(&path).is_some(), "control: a valid file did not read either");
}
```

Run: fails to resolve.

- [ ] **Step 2: Write the four functions**

```rust
/// **Where a resident UI process leaves a preferences edit for the daemon
/// to pick up, without exiting to deliver it.**
///
/// Named by pid for [`result_path`]'s reasons, and a DIFFERENT name from
/// it: that file is the child's last act and is deleted by the daemon
/// after the reap, so sharing it would be a truncating write racing a
/// delete across two processes.
pub fn edited_settings_path(config_dir: &Path, pid: u32) -> PathBuf {
    config_dir.join(format!("ui-settings-{pid}.json"))
}

/// Write a preferences edit for the daemon, **atomically**.
///
/// Temp-then-rename rather than `fs::write`, because unlike the result
/// file this one is read while both processes are alive. `fs::write`
/// truncates before it writes, so a daemon that polled on a timer could
/// read an empty file. Production never polls on a timer -- it reads only
/// after the doorbell, which is set after this returns -- and the rename
/// is the belt to that braces: a second delivery over the first cannot be
/// observed half-applied either.
pub fn write_edited_settings(path: &Path, settings: &Settings) -> io::Result<()> {
    let json = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)?;
    // `fs::rename` is `MoveFileEx` with `REPLACE_EXISTING` on Windows, so
    // this lands over an earlier delivery rather than failing on it.
    std::fs::rename(&temp, path)
}

/// Read a preferences edit, from the daemon, after the doorbell rang.
///
/// `None` for every failure -- absent, unreadable, unparseable -- and each
/// is the same thing from the daemon's side: nothing to apply. Logged
/// rather than silent, for [`read_result`]'s reason: a consistently
/// unparseable file presents to the user as "settings changed in the vault
/// window sometimes do not stick".
pub fn read_edited_settings(path: &Path) -> Option<Settings> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Settings>(&text) {
            Ok(settings) => Some(settings),
            Err(e) => {
                log::warn!("a UI process's settings delivery at {} did not parse ({e})", path.display());
                None
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("could not read a UI process's settings delivery at {}: {e}", path.display());
            None
        }
    }
}

/// Delete a delivery once it has been applied. Best effort, for
/// [`forget_result`]'s reason.
pub fn forget_edited_settings(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != io::ErrorKind::NotFound {
            log::warn!("could not delete {} after applying it: {e}", path.display());
        }
    }
}
```

Run: green.

---

### Task 4: `effective_keep_ui_loaded`

**Files:** modify `deskwarden/src/vault_window/mod.rs`

A window whose user has just turned `keep_ui_loaded` **off** must not hide. Today that is unreachable — the edit forces an exit — and Task 6 makes it reachable, so it is handled first.

**Interfaces**

- *Produces:* `fn effective_keep_ui_loaded(edited: Option<&crate::settings::Settings>, started_with: bool) -> bool`.
- *Precedent:* `effective_auto_lock` (~`vault_window/mod.rs:1506`), which solves the identical problem for the auto-lock timer and whose comment records the same reported defect ("turning auto-lock off in Settings changed nothing until the vault was closed and reopened"). **Read that function and mirror its shape exactly**; if it takes its arguments in another order or spells the fallback differently, follow it rather than this snippet.

- [ ] **Step 1: Write the failing test**

```rust
/// **The modal's live value wins, in BOTH directions**, the way
/// `effective_auto_lock`'s does.
///
/// The off-direction is the one this feature makes reachable: before it,
/// an edited `keep_ui_loaded` forced the window to exit, so the stale
/// parameter could never be consulted. After it, a window started with the
/// setting on whose user has just turned it off would otherwise hide --
/// and a user who just asked for the process not to stay loaded would
/// watch it stay loaded.
#[test]
fn the_modal_decides_whether_this_window_may_stay_loaded() {
    let off = crate::settings::Settings { keep_ui_loaded: false, ..Default::default() };
    let on = crate::settings::Settings { keep_ui_loaded: true, ..Default::default() };

    assert!(
        !effective_keep_ui_loaded(Some(&off), true),
        "the window was started with the setting on and the user has just turned it off; \
         it must not stay loaded"
    );
    assert!(
        effective_keep_ui_loaded(Some(&on), false),
        "the user turned it on in this window's own modal and it did not take effect"
    );
    assert!(
        effective_keep_ui_loaded(None, true),
        "control: with no modal ever opened the startup value must still decide"
    );
    assert!(!effective_keep_ui_loaded(None, false));
}
```

Run: fails to resolve.

- [ ] **Step 2: Write it**

```rust
/// **Whether THIS window may stay loaded, re-read rather than remembered.**
///
/// `started_with` is what `settings.json` said when this process started,
/// which is what decided whether it has [`HideHooks`] at all.
/// `edited` is what the gear's modal has said since.
///
/// The same shape as [`effective_auto_lock`] and for the same reason: a
/// preference the user changed in this window must bind in this window.
/// The off-direction is the sharp one -- a user who has just turned *Open
/// the vault instantly* off and then closed the window would otherwise
/// watch the process stay resident anyway, which is the setting appearing
/// not to work at the exact moment it is being tested.
fn effective_keep_ui_loaded(
    edited: Option<&crate::settings::Settings>,
    started_with: bool,
) -> bool {
    edited.map_or(started_with, |s| s.keep_ui_loaded)
}
```

Run: green.

---

### Task 5: `HideHooks` learns to deliver

**Files:** modify `deskwarden/src/vault_window/mod.rs`

**Interfaces**

- *Produces:* `HideHooks::deliver_settings: Box<dyn Fn(&crate::settings::Settings) -> bool>`; `fn deliver_if_needed(hooks, edited_settings, delivered) -> bool`.
- *Consumes:* nothing outside this module. The file write and the `SetEvent` live in `main.rs`'s `run_as_a_ui_process` (Task 8), so this module still reaches no kernel and no disk — which is what keeps it testable.
- *Note:* `delivered` is a **new cell**, `Rc<Cell<bool>>`, alongside `hidden` and `waiting_for_show` at ~`vault_window/mod.rs:1330`. `edited_settings` itself is **never cleared** — its "`Some` for the rest of the window's life" contract is load-bearing in five places and this plan does not touch it.

- [ ] **Step 1: Write the failing test**

```rust
/// **Delivery is attempted once, and its ANSWER is believed.**
///
/// `false` means no daemon is holding the doorbell, and the window must
/// then keep the old transport: exit, and carry the settings home in the
/// result file. `true` means a live daemon has the payload and the window
/// is free to hide.
///
/// The two halves are each other's control: an implementation that always
/// returned `true` fails the first assertion, and one that always returned
/// `false` fails the second.
#[test]
fn a_refused_delivery_is_not_recorded_as_delivered() {
    let attempts = std::rc::Rc::new(std::cell::Cell::new(0));
    let answer = std::rc::Rc::new(std::cell::Cell::new(false));

    let hooks = {
        let attempts = std::rc::Rc::clone(&attempts);
        let answer = std::rc::Rc::clone(&answer);
        HideHooks {
            wait_for_show: std::sync::Arc::new(|| false),
            on_hidden: Box::new(|| {}),
            on_shown: Box::new(|| {}),
            deliver_settings: Box::new(move |_| {
                attempts.set(attempts.get() + 1);
                answer.get()
            }),
        }
    };

    let edited: Rc<RefCell<Option<crate::settings::Settings>>> =
        Rc::new(RefCell::new(Some(crate::settings::Settings::default())));
    let delivered = std::rc::Rc::new(std::cell::Cell::new(false));

    assert!(!deliver_if_needed(&hooks, &edited, &delivered), "a refused delivery reported success");
    assert!(!delivered.get(), "a refused delivery was recorded as delivered");
    assert_eq!(attempts.get(), 1);

    answer.set(true);
    assert!(deliver_if_needed(&hooks, &edited, &delivered), "an accepted delivery reported failure");
    assert!(delivered.get());
    assert_eq!(attempts.get(), 2, "control: the retry after a refusal never reached the hook");

    // Already delivered: no second attempt, and still `true`.
    assert!(deliver_if_needed(&hooks, &edited, &delivered));
    assert_eq!(attempts.get(), 2, "an already-delivered edit was sent a second time");
}

/// **Nothing to deliver is a delivered state, not a refusal.** A window
/// that never opened the gear has nothing outstanding, so `close_or_hide`
/// must not read "no delivery happened" as "an edit is being withheld".
#[test]
fn a_window_that_never_opened_the_gear_has_nothing_outstanding() {
    let attempts = std::rc::Rc::new(std::cell::Cell::new(0));
    let hooks = {
        let attempts = std::rc::Rc::clone(&attempts);
        HideHooks {
            wait_for_show: std::sync::Arc::new(|| false),
            on_hidden: Box::new(|| {}),
            on_shown: Box::new(|| {}),
            deliver_settings: Box::new(move |_| { attempts.set(attempts.get() + 1); true }),
        }
    };
    let edited: Rc<RefCell<Option<crate::settings::Settings>>> = Rc::new(RefCell::new(None));
    let delivered = std::rc::Rc::new(std::cell::Cell::new(false));

    assert!(deliver_if_needed(&hooks, &edited, &delivered));
    assert_eq!(attempts.get(), 0, "an empty cell was delivered to the daemon anyway");
}
```

Run: fails to compile — `HideHooks` has no `deliver_settings`, and `deliver_if_needed` does not exist. **Fix every existing `HideHooks` literal in the test module in the same step**, giving each `deliver_settings: Box::new(|_| true)` unless the test is about delivery.

- [ ] **Step 2: Add the field and the helper**

On `HideHooks`, after `on_shown`:

```rust
    /// **Hand a preferences edit to the daemon while this window stays
    /// alive**, returning whether it landed.
    ///
    /// This is the second transport `ui_process::on_close`'s relaxed rule
    /// depends on. Production writes `ui_process::edited_settings_path`
    /// and presses `ui_show::settings_name`; `false` means nobody is
    /// holding that doorbell, and the window must then fall back to the
    /// only transport there has ever been -- exiting, with the settings in
    /// the result file.
    ///
    /// Boxed and taking `&Settings` rather than owning it, because the
    /// cell it comes from is NOT emptied by a delivery: `edited_settings`
    /// stays `Some` for the rest of the window's life (five readers depend
    /// on that) and re-delivery on exit is harmless -- the daemon's
    /// write-back is guarded by `edited != settings`.
    pub deliver_settings: Box<dyn Fn(&crate::settings::Settings) -> bool>,
```

And, beside `close_or_hide`:

```rust
/// **Whether there is nothing left for the daemon to hear about the
/// settings**, delivering them first if there is.
///
/// `true` means the daemon either has this window's preferences edit or
/// there was never one to have. `false` means an edit exists and could not
/// be handed over, and the caller must therefore end this process so the
/// result file can carry it.
///
/// Idempotent: once `delivered` is set, this neither writes nor presses
/// anything, so the two call sites (the modal's dismissal and the close)
/// cost one delivery between them rather than one each.
fn deliver_if_needed(
    hooks: &HideHooks,
    edited_settings: &Rc<RefCell<Option<crate::settings::Settings>>>,
    delivered: &std::rc::Rc<std::cell::Cell<bool>>,
) -> bool {
    if delivered.get() {
        return true;
    }
    let borrowed = edited_settings.borrow();
    let Some(settings) = borrowed.as_ref() else {
        // Nothing outstanding is not a refusal; the gear was never opened.
        return true;
    };
    if (hooks.deliver_settings)(settings) {
        delivered.set(true);
        true
    } else {
        log::info!(
            "no daemon answered this window's settings delivery; it will close rather than \
             hide, so the edit reaches the daemon in the result file"
        );
        false
    }
}
```

Run: green.

---

### Task 6: `on_close` grows a third input, and the 64-combination pin is re-pinned

**Files:** modify `deskwarden/src/ui_process.rs`, `deskwarden/src/main.rs`

**This is the task the branch exists for**, and the one where a failing test is expected. `the_hide_rule_is_stricter_than_done` in `main.rs` will fail to compile at Step 1 (the arity change) and is re-pinned in Step 3.

**Interfaces**

- *Changes:* `pub fn on_close(keep_loaded: bool, result: &UiVaultResult, settings_delivered: bool) -> OnClose`.
- *Callers:* `vault_window::close_or_hide` (Task 7), the pin in `main.rs`, and `ui_process`'s own tests.

- [ ] **Step 1: Write the failing test**

In `ui_process`'s test module, replacing `a_preferences_edit_exits_even_though_its_follow_up_is_done` with the pair below and giving every other `on_close` call in the module a third argument of `true` (they carry no `edited_settings`, so the flag cannot change their answer — and `every_outcome_the_daemon_acts_on_exits` passing with `true` is itself the proof that the flag relaxes *only* the settings field):

```rust
/// **An UNDELIVERED preferences edit still exits, which is the old rule
/// exactly.**
///
/// The only route `edited_settings` ever had to the daemon was this
/// process ending. A window that hid holding an edit nobody had taken
/// would withhold the estate copy, `apply_disk_cache_change`,
/// `persist_preferences` and the clipboard re-install -- all four.
#[test]
fn an_undelivered_preferences_edit_still_exits() {
    let geared =
        UiVaultResult { edited_settings: Some(Settings::default()), ..Default::default() };
    assert_eq!(
        on_close(true, &geared, false),
        OnClose::Exit,
        "the window hid holding an edit no daemon had taken, so it never arrived"
    );
}

/// **A DELIVERED preferences edit hides, and this is the whole defect.**
///
/// `edited_settings` stays `Some` for the rest of a window's life, so
/// under the old rule one visit to the gear made every later close an
/// exit -- and the gear is where *Open the vault instantly* lives. The
/// user turned the setting on and was rewarded with a cold start and a
/// Windows Hello prompt on the very next open.
///
/// It may hide now because the edit is no longer being withheld: the
/// daemon took it over the live channel while this window was still up.
#[test]
fn a_delivered_preferences_edit_hides() {
    let geared =
        UiVaultResult { edited_settings: Some(Settings::default()), ..Default::default() };
    assert_eq!(
        on_close(true, &geared, true),
        OnClose::Hide,
        "a visit to Preferences still ends the process, which is the reported defect"
    );
}

/// **Delivery relaxes ONE field and no other.** A locked window exits
/// however delivered its settings are: a lock is a reason the window
/// closed and the daemon's whole response to it is a teardown this
/// process has to be gone for.
#[test]
fn delivering_settings_does_not_excuse_any_other_outcome() {
    for (what, result) in [
        ("a lock", UiVaultResult { locked: true, ..Default::default() }),
        ("a re-auth", UiVaultResult { needs_reauth: true, ..Default::default() }),
        ("an add", UiVaultResult { add_account: true, ..Default::default() }),
        ("a remove", UiVaultResult { remove_account: true, ..Default::default() }),
        (
            "a switch",
            UiVaultResult { switch_to: Some(AccountId::generate()), ..Default::default() },
        ),
    ] {
        let with_settings = UiVaultResult {
            edited_settings: Some(Settings::default()),
            ..result.clone()
        };
        assert_eq!(on_close(true, &result, true), OnClose::Exit, "{what} must reach the daemon");
        assert_eq!(
            on_close(true, &with_settings, true),
            OnClose::Exit,
            "{what} hid because the settings beside it had been delivered"
        );
    }
    // Control: the empty result DOES hide under the same flag, so the
    // loop above is not passing because nothing ever hides.
    assert_eq!(on_close(true, &UiVaultResult::default(), true), OnClose::Hide);
}
```

Run: fails to compile (arity), in `ui_process` and in `main.rs`'s pin.

- [ ] **Step 2: Change the rule**

```rust
/// **Whether this close hides the window or ends its process.**
///
/// Every field of [`UiVaultResult`] is something the daemon acts on, and a
/// window that hid while holding one it had not delivered would be a lock,
/// a switch or a settings edit that silently never happened.
///
/// **Five of the six can only travel by this process exiting.** They are
/// each a *reason the window closed* -- the daemon's answer to every one
/// is `resettle_session` or an account settle, and it needs the window
/// gone to run either. So they still force an exit, whatever else is true.
///
/// **`edited_settings` is the exception, and it always was the odd one
/// out**: its own doc says it is "not a reason the window closed -- the
/// window closed for whatever it closed for, and this rides along". It now
/// has a second route home -- `HideHooks::deliver_settings`, a file and a
/// doorbell the daemon reads while this process is still alive -- and
/// `settings_delivered` says whether that route was taken. It is a
/// parameter rather than a field of the result because it is a fact about
/// the CHANNEL, not about the window's outcome: the result crossing the
/// process boundary is unchanged, and the daemon never sees this flag.
///
/// **This still does not mirror `vault_follow_up`'s `Done`.** That
/// function does not read `edited_settings` at all, because by the time it
/// is consulted the daemon has applied it. Hiding on `Done` alone would
/// hide an UNDELIVERED edit, which is the defect this whole rule exists to
/// prevent. `the_hide_rule_is_stricter_than_done` in `main.rs` holds the
/// two together over every combination of the six fields, at both values
/// of this flag.
#[must_use]
pub fn on_close(
    keep_loaded: bool,
    result: &UiVaultResult,
    settings_delivered: bool,
) -> OnClose {
    let nothing_to_report = !result.locked
        && !result.needs_reauth
        && !result.add_account
        && !result.remove_account
        && result.switch_to.is_none()
        && (result.edited_settings.is_none() || settings_delivered);
    if keep_loaded && nothing_to_report {
        OnClose::Hide
    } else {
        OnClose::Exit
    }
}
```

Run `ui_process`'s tests: green. `main.rs` still fails to compile.

- [ ] **Step 3: Re-pin the 64 combinations**

Replace `the_hide_rule_is_stricter_than_done`'s body. **This is a re-pin, not a loosening**: the property asserted — *a result that hides loses nothing the daemon would have acted on* — is identical. What moved is one of its inputs, because a delivered `edited_settings` is not lost by hiding. The stated reason goes in the doc comment so the next reader does not have to reconstruct it.

```rust
/// **Hiding is stricter than `Done`, and must stay stricter.**
///
/// Two rules in two crate halves decide overlapping things.
/// `vault_follow_up` says what the DAEMON does next;
/// `ui_process::on_close` says whether the CHILD may stay alive instead of
/// reporting. If a result ever hid while its follow-up was not `Done`,
/// that outcome would be swallowed by a window that never came home to
/// deliver it.
///
/// Walked over all 64 combinations of the six fields rather than a few
/// chosen ones, because the interesting case is the one nobody thought to
/// write down.
///
/// **Re-pinned 2026-08-30, and deliberately not relaxed.** `on_close` grew
/// a `settings_delivered` flag, because `edited_settings` gained a second
/// route to the daemon that does not require exiting -- see
/// `docs/superpowers/specs/2026-08-30-closing-the-window-keeps-it-loaded-design.md`.
/// The property here is unchanged; one of its inputs moved. So the walk
/// runs TWICE:
///
/// - **undelivered** must count exactly 1 -- byte-for-byte the old rule,
///   which is the control proving the flag is load-bearing rather than
///   ignored;
/// - **delivered** must count exactly 2 -- the empty result and the
///   `edited_settings`-only one, which is the control proving the
///   relaxation is ONE combination and not a widening. A rule that had
///   dropped the `locked` conjunct would count 4 here and fail.
///
/// Hide implies `Done` in both passes, and it holds for the newly-hiding
/// combination for the reason this test was written: a result carrying
/// only `edited_settings` lands on `Done`, because editing preferences is
/// not a reason a window closed.
#[test]
fn the_hide_rule_is_stricter_than_done() {
    for (delivered, expected_hides) in [(false, 1), (true, 2)] {
        let mut hides = 0;
        for bits in 0u8..64 {
            let crossing = deskwarden::ui_process::UiVaultResult {
                locked: bits & 1 != 0,
                needs_reauth: bits & 2 != 0,
                add_account: bits & 4 != 0,
                remove_account: bits & 8 != 0,
                switch_to: (bits & 16 != 0).then(deskwarden::accounts::AccountId::generate),
                edited_settings: (bits & 32 != 0)
                    .then(deskwarden::settings::Settings::default),
            };
            let window = VaultWindowResult {
                locked: crossing.locked,
                needs_reauth: crossing.needs_reauth,
                edited_settings: crossing.edited_settings.clone(),
                switch_to: crossing.switch_to.clone(),
                add_account: crossing.add_account,
                remove_account: crossing.remove_account,
                account_details: None,
            };
            if deskwarden::ui_process::on_close(true, &crossing, delivered)
                == deskwarden::ui_process::OnClose::Hide
            {
                hides += 1;
                assert_eq!(
                    vault_follow_up(&window),
                    VaultFollowUp::Done,
                    "a result that hides does not land on `Done`, so hiding it loses \
                     whatever the daemon would have done about it \
                     (settings_delivered={delivered}): {crossing:?}"
                );
            }
        }
        assert_eq!(
            hides, expected_hides,
            "control: with settings_delivered={delivered}, exactly {expected_hides} of the \
             64 combinations should hide. {hides} did, so this test is not pinning what it \
             says it pins"
        );
    }
}
```

Run the whole suite. Green.

---

### Task 7: The window decides, and the modal delivers

**Files:** modify `deskwarden/src/vault_window/mod.rs`

**Interfaces**

- *Consumes:* `deliver_if_needed`, `effective_keep_ui_loaded`, `ui_process::on_close/3`.
- *Changes:* `close_or_hide`'s signature gains `delivered: &Rc<Cell<bool>>` and `started_keep_ui_loaded: bool`; the modal arm at ~`:4947` delivers on `PrefsAction::Close`.

`close_or_hide` already has nine parameters and a `#[allow(clippy::too_many_arguments)]`. **Do not fold the cells into a struct in this task** — that is a refactor with its own blast radius across the frame closure's clones, and it is not what this branch is for. Add the two, keep the allow.

- [ ] **Step 1: Write the failing test**

Extend Task 5's test module:

```rust
/// **A window whose user has just turned the setting OFF does not hide**,
/// even with its edit delivered.
///
/// This case is only reachable because of this branch: before it, an
/// edited `keep_ui_loaded` forced an exit, so the stale startup value was
/// never consulted. The user's very first act after turning *Open the
/// vault instantly* off is to close the window and see whether it worked.
#[test]
fn turning_the_setting_off_in_the_modal_stops_this_window_hiding() {
    let turned_off =
        crate::settings::Settings { keep_ui_loaded: false, ..Default::default() };
    let crossing = crate::ui_process::UiVaultResult {
        edited_settings: Some(turned_off.clone()),
        ..Default::default()
    };
    assert_eq!(
        crate::ui_process::on_close(
            effective_keep_ui_loaded(Some(&turned_off), true),
            &crossing,
            true,
        ),
        crate::ui_process::OnClose::Exit,
        "the user turned the setting off and the process stayed loaded anyway"
    );

    // Control: the same delivered edit with the setting left ON does hide,
    // so the assertion above is not passing because nothing ever hides.
    let left_on = crate::settings::Settings { keep_ui_loaded: true, ..Default::default() };
    assert_eq!(
        crate::ui_process::on_close(
            effective_keep_ui_loaded(Some(&left_on), true),
            &crate::ui_process::UiVaultResult {
                edited_settings: Some(left_on),
                ..Default::default()
            },
            true,
        ),
        crate::ui_process::OnClose::Hide
    );
}

/// **Minimize is not a hide, and now says so.**
///
/// The reported defect's second sentence -- "only goes on minimize" -- was
/// a minimized window keeping its process by never running
/// `close_or_hide` at all. That is CORRECT and stays: a minimized window
/// has a taskbar button, is restored without the daemon, and is still
/// using the vault, so it must keep the visibility name and the
/// attachment. This pins that the `Minimize` arm neither consults the hide
/// rule nor touches the hooks, so a future "unify close and minimize"
/// cannot quietly make a minimized window drop its attachment and strand
/// `bw serve`.
#[test]
fn the_minimize_arm_does_not_go_through_the_hide_rule() {
    let source = include_str!("mod.rs");
    let arm = source
        .split("ChromeAction::Minimize =>")
        .nth(1)
        .expect("the Minimize arm")
        .split("ChromeAction::None")
        .next()
        .expect("the arm ends before the None arm");
    assert!(
        arm.contains("Minimized(true)"),
        "control: this is not the Minimize arm any more, so the assertions below \
         are reading the wrong text"
    );
    for forbidden in ["close_or_hide", "on_close", "hide_hooks", "deliver_if_needed"] {
        assert!(
            !arm.contains(forbidden),
            "the Minimize arm now mentions `{forbidden}`. A minimized window is still IN \
             USE -- taskbar button, one-click restore, decrypted vault on a machine its \
             owner is at -- so it must keep the visibility name and the vault attachment \
             that a hide deliberately drops"
        );
    }
}
```

Run: fails (`effective_keep_ui_loaded` is Task 4's, so this compiles; the minimize pin passes immediately and the first test fails only if Task 4/6 were skipped). **If the minimize pin passes on the first run that is expected** — it is pinning existing correct behaviour, and its control assertion is what stops it being vacuous.

- [ ] **Step 2: Change `close_or_hide`**

```rust
    hidden: &std::rc::Rc<std::cell::Cell<bool>>,
    delivered: &std::rc::Rc<std::cell::Cell<bool>>,
    started_keep_ui_loaded: bool,
```

and its body, replacing the `on_close` call:

```rust
    // **Deliver before deciding.** This covers Alt+F4 with the modal still
    // up -- the case the every-frame write into `edited_settings` exists
    // for -- and is a no-op when the modal's own dismissal already
    // delivered. A refusal here means no daemon is holding the doorbell,
    // and the window then closes so the result file can carry the edit,
    // which is exactly what it did before this feature.
    let settings_delivered = deliver_if_needed(hooks, edited_settings, delivered);
    // **The setting is re-read, not remembered.** A user who has just
    // turned *Open the vault instantly* off in this window's own modal
    // must not watch this process stay loaded. See
    // `effective_keep_ui_loaded`, and `effective_auto_lock` for the
    // precedent in the same file.
    let keep_loaded = effective_keep_ui_loaded(crossing.edited_settings.as_ref(), started_keep_ui_loaded);
    if crate::ui_process::on_close(keep_loaded, &crossing, settings_delivered)
        == crate::ui_process::OnClose::Exit
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        return;
    }
```

Note the `hooks` binding already exists above (the `let Some(hooks) = hooks else { .. }` early return), and `crossing` is built above the call — **move the `crossing` construction above the delivery** if it is not already, so both read the same borrow of `edited_settings`.

Update both call sites (~`:1387` and ~`:2172`) and `build_frame_with_search`'s plumbing of `started_keep_ui_loaded`. **The startup value is not in this function today** — it is `settings.keep_ui_loaded` in `main.rs`'s `run_as_a_ui_process`, and it is exactly "the hooks are `Some`". So pass `true` at the call sites when `hooks.is_some()`: `hooks` being `Some` **is** the startup setting, which this module's own `HideHooks` doc already states. Prefer that over threading a new parameter through `run`; if the reviewer disagrees, threading it is a mechanical change.

- [ ] **Step 3: Deliver when the modal is dismissed**

At ~`:4947`:

```rust
        if let Some(state) = prefs.as_mut() {
            let action = crate::prefs_ui::draw_prefs_modal(ui.ctx(), state);
            *edited_settings_for_closure.borrow_mut() = Some(state.settings.clone());
            if action == crate::prefs_ui::PrefsAction::Close {
                prefs = None;
                // **Hand it to the daemon now, not at the close.** The
                // window is staying open, and a preference the user just
                // changed should bind now -- which is what the tray's
                // Preferences item has always done. Before this, a
                // `keep_backend_running` change made here did nothing
                // until the window closed.
                //
                // A `None` hook set is a window with `keep_ui_loaded`
                // off, which has no channel and does not need one: it
                // will exit and the result file will carry this.
                if let Some(hooks) = hide_hooks.as_ref() {
                    deliver_if_needed(hooks, &edited_settings_for_closure, &delivered);
                }
            }
        }
```

Add `let delivered = std::rc::Rc::new(std::cell::Cell::new(false));` beside `hidden` at ~`:1330`, with clones for the closure exactly as `hidden` has.

**`delivered` must be reset to `false` if the modal is opened again and changes something.** Simplest correct rule, and the one to implement: reset `delivered` to `false` on every frame the modal is up (right beside the every-frame write into `edited_settings_for_closure` at `:4946`), so a second edit is always re-delivered on dismissal and on close. Re-delivery is idempotent at the daemon (`edited != settings`), so over-delivering costs one file write and one `SetEvent`; under-delivering loses a setting.

Run the whole suite. Green.

---

### Task 8: The daemon holds the doorbell

**Files:** modify `deskwarden/src/main.rs`

**Interfaces**

- *Consumes:* `ui_show::settings_name`, `ui_show::ShowEnv::create`, `ui_process::{edited_settings_path, read_edited_settings, forget_edited_settings, write_edited_settings}`, `apply_edited_settings` (Task 1).
- *Produces:* `OpenUiWindow::settings_doorbell: Option<deskwarden::ui_show::Signal>`; `UiWindows::take_edited_settings(&self, config_dir) -> Option<settings::Settings>`; the child's production `deliver_settings` hook.

- [ ] **Step 1: The daemon end — create at spawn, poll in the loop**

In `spawn_the_vault_window_in_its_own_process`, after the pid is known:

```rust
    // **Created by the daemon, pressed by the child.** The mirror of the
    // show signal, which the child creates and the daemon presses. Made
    // here rather than lazily because "the daemon is listening" is what
    // the child's `SetEvent` answer MEANS: a doorbell created only when
    // somebody thought to would have the child concluding no daemon
    // exists and exiting to deliver a setting the daemon was ready to
    // take.
    //
    // `None` is survivable and is not a failure to spawn: the child's
    // press then returns `false`, and it falls back to exiting with the
    // edit in the result file -- exactly the behaviour before this
    // feature.
    let settings_doorbell =
        (deskwarden::ui_show::ShowEnv::production().create)(
            &deskwarden::ui_show::settings_name(pid),
        );
    if settings_doorbell.is_none() {
        log::warn!(
            "could not create the settings doorbell for UI process {pid}; a preferences \
             edit made in that window will close it rather than being delivered live"
        );
    }
```

and store it on `OpenUiWindow`. On `UiWindows`:

```rust
    /// **Take a preferences edit the open window delivered while staying
    /// alive**, if it rang for one.
    ///
    /// Polled with a ZERO timeout, once per pass of `main`'s loop, beside
    /// the `try_wait` on the child and the `is_held` on the visibility
    /// name. A blocking wait is impossible here: this loop is what drains
    /// the hotkey, watches the foreground and answers the tray.
    ///
    /// Read only after the doorbell, never on a timer, which is what makes
    /// the file whole when it is read: the child writes it and then
    /// presses. The file is deleted once taken, so a second pass cannot
    /// apply it twice -- though applying it twice would be harmless, since
    /// `apply_edited_settings` is guarded by `edited != *settings`.
    fn take_edited_settings(&self, config_dir: &Path) -> Option<settings::Settings> {
        let open = self.vault.as_ref()?;
        if !open.settings_doorbell.as_ref()?.wait(0) {
            return None;
        }
        let path = deskwarden::ui_process::edited_settings_path(config_dir, open.pid);
        let edited = deskwarden::ui_process::read_edited_settings(&path);
        deskwarden::ui_process::forget_edited_settings(&path);
        if edited.is_some() {
            log::info!(
                "the vault window (process {}) delivered a preferences edit without closing",
                open.pid
            );
        }
        edited
    }
```

In `main`'s loop, immediately **above** the `poll_the_vault_window` call:

```rust
        // **A preferences edit from a window that is still open.** Above
        // the reap deliberately: if this pass both takes a delivery and
        // finds the window gone, the live edit is applied first and the
        // result file's copy of it is then a no-op at
        // `apply_edited_settings`'s `edited == *settings` guard. The other
        // order would apply the same edit twice, which is harmless but
        // would log the disk-cache change twice.
        if let Some(edited) = ui_windows.take_edited_settings(&config_dir) {
            apply_edited_settings(
                &estate.cache,
                &mut estate.settings,
                &settings_path,
                edited,
            );
        }
```

- [ ] **Step 2: The child end — the production hook**

In `run_as_a_ui_process`, inside the `settings.keep_ui_loaded.then(|| { .. })` block, add to the `HideHooks` literal:

```rust
            deliver_settings: {
                let config_dir = config_dir.clone();
                Box::new(move |edited: &deskwarden::settings::Settings| {
                    // **The file first, the doorbell second**, and the
                    // daemon reads only on the doorbell -- so it can never
                    // see a file that is not finished. The write itself
                    // lands by rename for the second delivery's sake.
                    let path = deskwarden::ui_process::edited_settings_path(
                        &config_dir,
                        std::process::id(),
                    );
                    if let Err(e) = deskwarden::ui_process::write_edited_settings(&path, edited) {
                        log::warn!("could not write this window's settings delivery ({e})");
                        return false;
                    }
                    // `false` means nobody holds the doorbell: no daemon,
                    // or one that could not create it. The caller reads
                    // that as "close rather than hide", so the edit goes
                    // home in the result file instead.
                    (deskwarden::ui_show::ShowEnv::production().set)(
                        &deskwarden::ui_show::settings_name(std::process::id()),
                    )
                })
            },
```

- [ ] **Step 3: Pin the loop's ordering and the doorbell's ownership**

```rust
/// **The live settings channel is read ABOVE the reap**, so that a pass
/// which both takes a delivery and finds the window gone applies the live
/// copy first and lets the result file's identical copy fall out at
/// `apply_edited_settings`'s equality guard.
#[test]
fn the_loop_takes_a_live_settings_delivery_before_it_reaps_the_window() {
    let source = include_str!("main.rs");
    let take = source.find("ui_windows.take_edited_settings(").expect("the live take");
    let reap = source.find("ui_windows.poll_the_vault_window(").expect("the reap");
    assert!(
        take < reap,
        "the reap comes first, so a settings edit is applied twice and the disk-cache \
         change is logged twice"
    );
}

/// **The daemon creates the doorbell, and it does so at the spawn.**
///
/// The child's `SetEvent` answer is the whole failure signal -- `false`
/// means "no daemon, exit and use the result file". A doorbell created
/// later than the spawn would make that answer say "no daemon" during a
/// window in which there certainly is one.
#[test]
fn the_doorbell_is_created_beside_the_spawn_that_names_it() {
    let source = include_str!("main.rs");
    let spawn = source
        .split("fn spawn_the_vault_window_in_its_own_process")
        .nth(1)
        .expect("the spawn function");
    let body = spawn.split("\nfn ").next().expect("the function body");
    assert!(
        body.contains("ui_show::settings_name"),
        "the settings doorbell is not created where the window is spawned"
    );
    assert!(
        body.contains("ShowEnv::production().create"),
        "control: the doorbell is named here but not created here"
    );
}
```

Run the whole suite, then `cargo test --no-run` and a build, both under `RUSTFLAGS="-D warnings"`. Green, zero warnings.

---

### Task 9: Minimize, made deliberate

**Files:** modify `deskwarden/src/vault_window/mod.rs`

**No behavioural change.** The design's answer is that today's minimize is correct and was correct by omission; this writes the reason down beside it. The pin is already in place from Task 7 Step 1.

- [ ] **Step 1: Document the arm**

Replace the bare arm with:

```rust
            // **Minimize is not a hide, and must not become one.**
            //
            // It sends the viewport command and nothing else: no
            // `close_or_hide`, no `on_close`, no hook. A minimized window
            // is still IN USE -- it has a taskbar button, the user
            // restores it with one click and no daemon involvement, and it
            // is holding a decrypted vault on a machine its owner is
            // sitting at. So it keeps the visibility name (`vault_is_in_use`
            // must go on answering `true`, or save-memory would stop
            // `bw serve` under a window one click from the foreground) and
            // it keeps its vault-service attachment.
            //
            // A hide is the opposite on every count: no taskbar button, no
            // way back except the daemon's named event, attachment dropped
            // precisely so the backend may stop.
            //
            // **And it is deliberately NOT gated on `keep_ui_loaded`.**
            // Gating it would make that setting change what the minimize
            // button does, and would leave a window with the setting off
            // unminimizable. The reported defect's "only goes on minimize"
            // was this path being the one that happened to keep the
            // process -- the fix is that a close keeps it too, not that a
            // minimize stops.
            //
            // `the_minimize_arm_does_not_go_through_the_hide_rule` pins it.
            ChromeAction::Minimize => {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true))
            }
```

Run the suite. Green.

---

### Task 10: The live check

**Files:** none

**No unit test in this crate observes a real window hiding.** `vault_window::run` consumes winit's event loop and cannot be called from a test; the hide is a viewport command inside a frame closure. Everything above pins the decisions; this is the only thing that settles the behaviour.

- [ ] **Step 1:** Fresh launch with `keep_ui_loaded` **off**. Open the vault, close it. Confirm the process ends — the baseline, and the control for every step below.
- [ ] **Step 2:** Open the vault, gear → turn *Open the vault instantly* on, dismiss the modal. **Before closing anything**, read the log and confirm a line saying the window delivered a preferences edit without closing. This is the fix's mechanism, observed directly.
- [ ] **Step 3:** Close the vault window. Confirm in Task Manager that `deskwarden.exe` is still running with the window's memory footprint, and that the log says the window hid itself. **This is the acceptance criterion for the whole branch.**
- [ ] **Step 4:** Open the vault again. Confirm: **no Windows Hello prompt**, no re-unlock, the window appears immediately. This is the reported defect, gone.
- [ ] **Step 5:** Repeat steps 2–4 a second time in the same session — gear, dismiss, close, open. The second cycle is what would fail if `delivered` were not reset while the modal is up.
- [ ] **Step 6:** With the window hidden, confirm `bw serve` stops if `keep_backend_running` is off (the attachment really was released), and that the tray's *Open Vault* brings the same process back rather than starting a second.
- [ ] **Step 7:** **The disk-cache side effect, live.** Gear → turn *Keep an encrypted copy on disk* on, dismiss. Confirm the encrypted file appears **while the window is still open** — the daemon ran `apply_disk_cache_change` from the live delivery. Turn it off, dismiss, confirm the file is gone. Then close the window and confirm it hides.
- [ ] **Step 8:** **Turning the setting off.** With the window resident, gear → turn *Open the vault instantly* off, dismiss, close. Confirm the process **ends**. This is Task 4's whole purpose and the case the fix made reachable.
- [ ] **Step 9:** Gear → dismiss → press **Lock**. Confirm the window closes, the vault locks, and the master password is asked for. The v0.5.0 defect was exactly "settings visited, then Lock did nothing"; Task 1 rewrote that block and this is the manual check on it.
- [ ] **Step 10:** With a hidden window resident, quit from the tray. Confirm the hidden process is gone (`close_on_quit` kills by pid regardless of visibility) and that no `ui-settings-*.json` is left in the config directory.
- [ ] **Step 11:** With a hidden window resident, press Win+L and come back. Confirm the vault locked and the hidden process was closed (`close_because_the_user_walked_away`), and confirm no `ui-settings-*.json` is left behind.

---

## Self-review

### Spec coverage

| Spec requirement | Where |
| --- | --- |
| Option 2 chosen; child delivers live | Tasks 3, 5, 8 |
| Doorbell is a named auto-reset event, daemon-created | Task 2, Task 8 Step 1 |
| Payload is its own file, written temp-then-rename | Task 3 |
| `SetEvent` returning `false` degrades to today's behaviour | Task 5 Step 1 (`a_refused_delivery_is_not_recorded_as_delivered`), Task 6 (`an_undelivered_preferences_edit_still_exits`) |
| `edited_settings` never cleared; re-delivery idempotent | Task 5 Step 2 doc; Task 1's `edited == *settings` guard |
| `apply_disk_cache_change` still runs, from one place | Task 1, asserted at the point of effect; Task 10 Step 7 live |
| Delivery on modal dismissal *and* on close | Task 7 Steps 2–3 |
| `effective_keep_ui_loaded` | Task 4, Task 7 Step 1 |
| 64-combination pin re-pinned, two passes, 1 and 2 | Task 6 Step 3 |
| Minimize unchanged, made deliberate | Task 7 Step 1 (pin), Task 9 (doc) |
| Live verification | Task 10 |

### Placeholder scan

Searched for `TBD`, `appropriate`, `similar to`, `etc.`, `and so on`, `handle errors`: no hits. Every step that changes code carries the code. Three places say **stop and report** rather than improvise: Task 1's "if the two write-backs disagree on any input", Task 1 Step 1's `VaultCache::new_in` spelling, and Task 4's "if `effective_auto_lock` takes its arguments in another order". Those are boundaries with code I read but did not re-verify line by line, not gaps.

### Soft spots I am flagging rather than hiding

1. **The refused disk cache diverges in a resident window.** If `enable_disk_persistence` fails, the daemon corrects itself and the file to `false` and shows a message box, but the child's `edited_settings` still says `true` — so a second gear click seeds a checkbox that is on while the file says off, until the window is genuinely closed. Today the window always closes in that scenario, so this is newly *visible* rather than newly *wrong*. The fix is an acknowledgement carrying the corrected settings back down; the design deliberately does not build it. **Recorded, not overlooked.**

2. **`delivered` is reset every frame the modal is up (Task 7 Step 3).** That means dragging a slider in Preferences and dismissing costs one file write and one `SetEvent`, not one per frame — the reset is per-frame but the *delivery* is on dismissal and on close. If a future change moves delivery into the frame body, this becomes a write per frame and must be debounced. Flagged because the reset and the delivery are in adjacent code and the relationship is easy to break.

3. **`close_or_hide` now takes eleven arguments.** It already carried `#[allow(clippy::too_many_arguments)]`. Folding the seven `Rc<RefCell<_>>` cells into one struct is the right cleanup and is **deliberately not in this branch**: it touches every clone in the frame closure and several source pins that split on that closure's text, and mixing it with a behaviour fix would make both harder to review.

4. **Task 1 Step 4 edits the span two source pins watch.** `the_vault_loop_tail_reads_edited_settings_once` and `nothing_outside_the_two_branch_bodies_may_jump` are the last defence against the v0.5.0 defect. The replacement is a strict simplification and should satisfy both, but **if either fails, read it and satisfy what it asserts — do not relax it.** A `?` or an early return smuggled into that span is the exact shape that made Lock silently not lock for a whole release.

5. **The daemon polls `wait(0)` once per loop pass, so delivery latency is one iteration.** That is milliseconds in practice and it is the same latency the reap already has. But it means a `keep_backend_running` change is applied *just after* the modal closes rather than synchronously with it, and a test that asserted synchrony would be asserting something the design does not promise.

6. **Nothing here proves two UI processes cannot both hold doorbells.** The one-window rule means there is only ever one, and the names are per-pid, so a second would be harmless. I did not add a test for a situation the surrounding invariant already forbids — but if a second surface is ever added, `UiWindows` gains a second field and this channel needs a second doorbell with it.

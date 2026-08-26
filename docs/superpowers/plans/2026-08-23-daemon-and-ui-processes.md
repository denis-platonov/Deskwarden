# Daemon and UI Processes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the tray daemon and each rich window as separate processes of the same binary, so closing a window returns the OpenGL driver's memory — which only process exit can return.

**Architecture:** One executable, two modes. `deskwarden.exe --autostart` is the daemon: tray, hotkey, match engine, `bw serve` ownership, and the bare-Win32 cards. `deskwarden.exe --ui <surface>` is spawned per window, talks HTTP to the daemon's `bw serve`, and exits when its window closes. No IPC: the UI reads the DPAPI session and the active account from disk and knows the port as a constant.

**Tech Stack:** Rust, `windows` crate 0.58, the existing `app_mutex`, `session_store`, `settings`, `bw_serve` and `job_object` modules.

**Spec:** `docs/superpowers/specs/2026-08-23-daemon-and-ui-processes-design.md`. Read it before Task 1; the reasoning there is the specification and this plan does not repeat it.

## Global Constraints

- **No secret on a command line.** Command lines are readable by other processes. The mode and surface name are not secrets; a session token or password would be.
- **The UI must never own `bw serve`.** The daemon is the orchestrator; the UI is a client of the backend it runs.
- **The UI is NOT assigned to the daemon's `KillOnCloseJob`.** A daemon restart must not close open windows.
- **`bw_serve::BW_SERVE_PORT` is a compile-time constant (8087)** — do not add port discovery, a rendezvous file, or a pipe.
- **Every surface lives in exactly one renderer.** A card drawn in both the daemon's Win32 and the UI's egui is the "two things that must agree" defect on the surface that types passwords.
- **No `cfg(test)` seams.** Banned crate-wide. Use the `fn`-pointer seam idiom (`PromptCalls`, `VaultFrameEnv`, `TakeoverEnv`).
- **No test may touch** the network, the real vault, the real clipboard, the real screen, `%APPDATA%\Deskwarden`, a real dialog, or spawn `bw`.
- **Never build into `deskwarden/target`.** Use `CARGO_TARGET_DIR=/e/_dw_agent/run`, `-j 2`.
- **Never write scratch files under `deskwarden/src/**`.**
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.
- Branch: `daemon-ui-split`, off `main`.
- **Machine note:** this machine's Windows dynamic port range starts at 1024, overlapping mockito's server ports, so a rotating 25–90 tests fail with `os error 10054` in `vault_cache`, `vault_bridge`, `updater`, `picker_ui`, `breach`, `bw_serve`, `vault_window` and `app::fill_dispatch_tests`. **CI is green on the same commits.** Re-run anything suspicious in isolation before concluding.

---

## Why Task 0 exists

`deskwarden/src/main.rs` is ~23,000 lines and its startup interleaves daemon concerns (tray, hotkey, `bw serve`, match engine, job object) with UI concerns (the vault window, the account picker's egui surfaces, first-surface choice) across roughly 1,200 lines. **No honest task list can be written over code nobody has mapped**, and a plan that guessed at those boundaries would send an implementer to rewrite a function whose second half belongs to the other process.

So Task 0 produces the map, and Tasks 2–3 are written against it rather than against a guess. If Task 0's findings contradict this plan, **the findings win** and the plan is amended before Task 2 starts.

---

### Task 0: Map the startup, and find where the two roles already separate

**Files:**
- Create: `docs/superpowers/notes/2026-08-23-startup-role-map.md`
- Read only: `deskwarden/src/main.rs`, `deskwarden/src/app_window.rs`, `deskwarden/src/bw_serve.rs`, `deskwarden/src/job_object.rs`, `deskwarden/src/app_mutex.rs`, `deskwarden/src/single_instance.rs`

**Produces:** the note below, which Tasks 2 and 3 are written against.

- [ ] **Step 1: Trace `main` from entry to the event loop**

Record, with line numbers, every step between process entry and the tray event loop, and label each **daemon**, **UI**, or **both**. `LaunchIntent` (`main.rs:8517`) and `FirstSurface` (`main.rs:8578`) already encode part of this distinction — say how much.

- [ ] **Step 2: Find every site that opens an egui window**

`vault_window::run`, `login_ui::run_login_flow`, `overlay_ui`'s save-login and generator, `preflight_host`, `prefs_ui`, `scratch_window`, `loading_ui`. For each: who calls it, on which thread, and **whether it blocks the caller**. A comment at `main.rs:4758` says `vault_window::run` "blocked the tray" — confirm whether that is still true, because if it is, this split also fixes a responsiveness bug and that belongs in the record.

- [ ] **Step 3: Establish what the daemon must keep**

List what cannot move to a UI process: the tray icon, the global hotkey registration (`RegisterHotKey` binds to the calling thread), `bw serve` ownership and its job object, the match engine, the foreground watcher, and the Win32 cards (`unlock_prompt`, `picker_prompt`).

- [ ] **Step 4: Establish what a UI process needs at startup**

For each egui surface, what state does it need before it can draw? The spec claims settings, the DPAPI session and the constant port are sufficient. **Verify that claim per surface** and record any that needs something else — that is the finding most likely to change this plan.

- [ ] **Step 5: Write the note and commit**

```bash
git add docs/superpowers/notes/2026-08-23-startup-role-map.md && git commit -F msg.txt
```

State plainly in the note anything that contradicts the spec or this plan.

---

### Task 1: The `--ui` mode flag

**Files:**
- Modify: `deskwarden/src/main.rs` (near `AUTOSTART_FLAG`, `main.rs:8507`, and `LaunchIntent`, `main.rs:8517`)

**Interfaces:**
- Produces: `UI_FLAG: &str = "--ui"`, a `Surface` enum naming the windows a UI process can be asked for, and `LaunchIntent::Ui(Surface)`.

Pure argument parsing. No behaviour changes yet — a `--ui` launch may do exactly what a normal launch does at the end of this task.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_ui_flag_names_the_surface_it_was_asked_for() {
    assert_eq!(launch_intent_from(&["deskwarden.exe", "--ui", "vault"]), LaunchIntent::Ui(Surface::Vault));
}

#[test]
fn a_ui_flag_with_no_surface_is_refused_rather_than_guessed() {
    assert_ne!(
        launch_intent_from(&["deskwarden.exe", "--ui"]),
        LaunchIntent::Ui(Surface::Vault),
        "guessing a surface would open a window the user did not ask for; refuse instead"
    );
}

#[test]
fn an_unknown_surface_is_refused_rather_than_defaulted() {
    assert_ne!(
        launch_intent_from(&["deskwarden.exe", "--ui", "nonsense"]),
        LaunchIntent::Ui(Surface::Vault)
    );
}

#[test]
fn autostart_is_unchanged_by_the_new_flag() {
    assert_eq!(launch_intent_from(&["deskwarden.exe", "--autostart"]), LaunchIntent::Autostart);
    assert_eq!(launch_intent_from(&["deskwarden.exe"]), LaunchIntent::UserLaunch);
}
```

Use the real names Task 0 records for `LaunchIntent`'s existing variants; the assertions above are about behaviour, not spelling.

- [ ] **Step 2: Run and watch them fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2 launch_intent
```

- [ ] **Step 3: Implement, run, and check the installer guard still holds**

`the_installers_run_entry_passes_the_flag_the_app_reads` reconciles `AUTOSTART_FLAG` against `installer/deskwarden.iss`. The new flag is **not** in the installer and must not break that guard.

- [ ] **Step 4: Commit**

---

### Task 2: The daemon spawns a UI process for the vault window

**Files:**
- Modify: `deskwarden/src/main.rs` (the call site Task 0 records for `vault_window::run`)

The first behavioural change, and deliberately the smallest one that proves the design: the tray's *Open Vault* stops calling `vault_window::run` in-process and spawns `deskwarden.exe --ui vault` instead.

**The spawned process must NOT be assigned to the `KillOnCloseJob`** — see the spec. This is the single most important line in this task and the easiest to add by reflex, because every other child this app spawns *is* assigned.

- [ ] **Step 1: Write the failing test**

The spawn decision must be a pure function so a test can watch it without starting a process — the `fn`-pointer seam idiom, as `unlock_prompt`'s `PromptCalls` does. Assert: the command is the current executable, the arguments are the UI flag and the surface, **no secret appears in any argument**, and the child is not job-assigned.

- [ ] **Step 2: Run and watch it fail**
- [ ] **Step 3: Implement**

`std::env::current_exe()` names the binary; the daemon keeps running its loop rather than blocking on the child.

- [ ] **Step 4: Run the full suite, then look at it**

Launch the daemon with `--autostart`, open the vault from the tray, and confirm: a second `deskwarden.exe` appears, the tray stays responsive while it is open, and **closing the window returns the daemon to ~11 MB** while the UI process disappears.

```bash
powershell -NoProfile -Command "Get-Process deskwarden | Select-Object Id,@{n='MB';e={[math]::Round($_.PrivateMemorySize64/1MB,1)}}"
```

**This measurement is the point of the whole plan.** If the daemon's number rises when the window opens, something in the daemon is still creating a GL context and the split has bought nothing — stop and find it before continuing.

- [ ] **Step 5: Commit**

---

### Task 3: The UI-mode startup path

**Files:**
- Modify: `deskwarden/src/main.rs`, guided by Task 0's map

A `--ui` launch must skip everything on Task 0's daemon list — no tray icon, no hotkey registration, no `bw serve` spawn, no match engine, no foreground watcher — and instead load settings and the DPAPI session, then run the requested surface and exit with its result.

- [ ] **Step 1: Write the failing tests**

Pin the exclusions, since they are what makes the UI cheap and each is invisible once working: a UI-mode startup registers no tray icon, no hotkey, and spawns no `bw serve`. Use the existing source-pin idiom where a runtime assertion is impossible (see `job_object.rs` and `picker_prompt.rs`'s `no_thread_quit_pin`), **normalising line endings first** — this is a CRLF checkout and a pin slicing on `"\n}"` is vacuous here. Include control assertions.

- [ ] **Step 2: Run and watch them fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Verify the UI process's cost**

With the vault window open, the UI process should be ~90–115 MB and the daemon unchanged at ~11 MB. Record both numbers in the commit message — this plan's justification is a memory claim and it should be checkable later.

- [ ] **Step 5: Commit**

---

### Task 4: A UI with no daemon exits by itself

**Files:**
- Create: `deskwarden/src/daemon_watch.rs`
- Modify: `deskwarden/src/main.rs`, `deskwarden/src/lib.rs`

**Interfaces:**
- Produces: a pure decision function over (daemon present?, how long absent) answering keep-running / exit, plus a `fn`-pointer seam for the presence check so tests need no mutex.

**Read the spec's "A UI with no daemon exits by itself" section before writing anything.** Three rules there are each a defect if inverted.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_ui_whose_daemon_is_present_keeps_running() { /* present -> KeepRunning */ }

#[test]
fn a_brief_absence_is_a_restart_and_is_waited_out() {
    // A self-update stops the old daemon before starting the new one. Exiting
    // on the first miss would close every window the user had open, mid-update.
}

#[test]
fn a_sustained_absence_is_a_quit_and_the_ui_exits() { }

#[test]
fn a_ui_that_never_saw_a_daemon_does_not_exit_on_that_basis() {
    // A double-click finds no mutex because no daemon exists YET, and must
    // start one. "A UI whose daemon WENT AWAY exits" is the rule; inverted,
    // the app cannot be started by double-clicking at all and the failure
    // looks like a broken binary.
}
```

- [ ] **Step 2: Run and watch them fail**
- [ ] **Step 3: Measure the update gap, then choose the grace period**

Time a real self-update's daemon-restart gap and pick a period comfortably above it. **Do not guess a round number** — the spec deliberately leaves this unfixed because guessing short means the updater closes the user's windows. Record the measurement beside the constant.

- [ ] **Step 4: Implement, using `app_mutex::APP_MUTEX_NAME`**

Open the mutex to *ask*, never to claim — a UI that acquired it would make itself look like the daemon to the next launch.

- [ ] **Step 5: Run the suite and commit**

---

### Task 5: One window per surface

**Files:**
- Modify: `deskwarden/src/main.rs`

Asking for the vault window while one is open must focus the existing window, not spawn a second process. Follow the existing single-instance machinery rather than inventing a second scheme.

- [ ] **Step 1: Write the failing test** — a second request for an open surface spawns nothing.
- [ ] **Step 2: Run and watch it fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Verify by hand** — open the vault, ask again from the tray, confirm one process and the window comes forward.
- [ ] **Step 5: Commit**

---

### Task 6: The daemon re-syncs when a UI exits

**Files:**
- Modify: `deskwarden/src/main.rs`

**This is the spec's default answer to the staleness question, chosen because it adds nothing that can disagree.** If the user has since chosen option 2 or 3, implement that instead and amend the spec.

An app-match added in the vault window is invisible to the daemon's match engine until it re-reads. On the child process exiting, the daemon re-syncs and rebuilds the engine.

- [ ] **Step 1: Write the failing test** — a UI exit triggers a re-sync and an engine rebuild.
- [ ] **Step 2: Run and watch it fail**
- [ ] **Step 3: Implement**
- [ ] **Step 4: Verify by hand** — add an app-match in the vault window, close it, and confirm `CTRL+ALT+B` on that app now matches.
- [ ] **Step 5: Commit**

---

### Task 7: Move the remaining egui surfaces, and record the result

**Files:**
- Modify: `deskwarden/src/main.rs` and each surface's call site

Every remaining in-process egui window — Preferences, the save-login form, the generator, the sequence editor, rehearsal, the login flow — becomes a `Surface` the daemon spawns.

**Verify the daemon creates no GL context on any path.** A single missed call site keeps the driver loaded in the daemon and undoes the entire plan; a source pin over the daemon's own module set is the cheap way to hold this, since a runtime assertion cannot see a path nobody took.

- [ ] **Step 1: Enumerate** every remaining site from Task 0's map, and pin the daemon against creating a GL context.
- [ ] **Step 2: Move them one at a time**, running the suite between each.
- [ ] **Step 3: Measure the finished thing**

Daemon idle, daemon with a UI open, UI alone, and daemon after the UI closes. Update the spec's table with the real numbers, including any that disappoint.

- [ ] **Step 4: Update the docs** — the spec's status line, and `2026-08-21`'s pointer to it.
- [ ] **Step 5: Commit**

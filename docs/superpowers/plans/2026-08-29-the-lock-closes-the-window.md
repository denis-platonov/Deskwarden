# The Lock Closes the Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A user presses Win+L with the vault window open, comes back, unlocks
Windows, and the vault window's process is gone — along with the daemon's
cache, `bw serve` and the clipboard, which it already took. Today that process
survives, showing a decrypted vault on a machine its owner has walked away
from, because `lock_after_walking_away` has no way to reach it.

**Architecture:** This implements
`docs/superpowers/specs/2026-08-29-the-lock-closes-the-window-design.md`.

The shape has one governing constraint and everything follows from it: **the
decision is already written, is pure, and is not being reopened.**
`away_lock::locks_the_vault(event, auto_lock, has_token)` answers whether a
departure locks the vault, `lock_after_walking_away` already returns early
when it says no, and the window close is added strictly *after* that early
return. No new preference, no second opinion about auto-lock, no fourth
parameter to `locks_the_vault`.

The impure half goes where its precedent already lives:
`UiWindows::close_on_quit` kills the child, takes the slot and forgets the
result file, and its decision is `ui_process::farewell_to_an_open_window`.
This work gives that function a third reason — `WhyClose::TheUserWalkedAway` —
and `UiWindows` a second killer beside `close_on_quit`.

The one non-obvious hazard, and the reason two of the tasks below exist:
`Child::kill` on Windows is `TerminateProcess` with **exit code 1**, and
`UiVaultResult::EXIT_LOCKED` is **1**. An away-lock kill that left the
`UiWindows` slot occupied would be reaped on the next loop pass, decoded as
`locked: true`, and produce a second `resettle_session` — a second
master-password prompt for a window the daemon killed itself. `close_on_quit`
is immune only because `process::exit(0)` is on the line after it. Here the
loop keeps running.

**Tech Stack:** Rust, Win32 via the `windows` crate (untouched by this work),
`std::process::Child::kill`, the existing `UiWindows` registry, source-text
pin tests in `main.rs` (the `bw_serve_gate` idiom).

## Global Constraints

- `cfg(test)` seams are banned crate-wide; seams are `fn`-pointer structs in production code.
- Build with `RUSTFLAGS="-D warnings"`; zero warnings.
- `export CARGO_TARGET_DIR=/e/_dw_agent/run` — never create a second target directory; the disk has ~23 GB free and that one is already 14 GB.
- Tests must not pass vacuously: every negative assertion carries a positive control. The house defect is "a test that passes because it never reached the thing it names".
- The `rest::`/`vault_cache::`/`picker_ui::`/`bw_serve::` mock-HTTP test family is flaky (~74 failures per full run, shifting membership); compare against a `git stash` baseline before believing you broke it. Another worker is fixing it — do not attempt it.

Additionally, and specific to this branch:

- **Do not edit `deskwarden/src/rest/`.** Another worker owns it.
- **No test may touch** the network, a real vault, the clipboard, the screen,
  `%APPDATA%\Deskwarden`, or spawn `bw` — or spawn `deskwarden.exe --ui vault`.
  Task 6 is the live check and is run by a human, by hand, not by `cargo test`.
- Commit with explicit paths and `-F` a message file. Never `git add -A`,
  `--amend`, `reset`, `rebase`, or `git stash` (the flaky-suite baseline check
  above is read-only, on a clean tree, restored immediately).
- Branch: `two-factor-without-the-cli`.

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/ui_process.rs` (modify) | `WhyClose`, the extended `farewell_to_an_open_window`, and their tests. The whole pure half. |
| `deskwarden/src/main.rs` (modify) | `UiWindows::close_because_the_user_walked_away`, the new parameter and call in `lock_after_walking_away`, the call site in `main`'s loop, the two stale doc comments, and the source pins. |
| `deskwarden/src/away_lock.rs` (modify) | One stale paragraph in the module doc. No code. |

---

### Task 1: A third reason for closing the window

**Files:** Modify `deskwarden/src/ui_process.rs`

**Interfaces**

- *Consumes:* `DaemonExit`, `Farewell` (both already here).
- *Produces:* `ui_process::WhyClose`, `impl From<DaemonExit> for WhyClose`, and
  `farewell_to_an_open_window(WhyClose, Option<u32>) -> Farewell`.

`DaemonExit` is deliberately **not** given a third variant: the daemon is not
exiting when the user presses Win+L, and a variant reading
`DaemonExit::TheUserWalkedAway` would be a lie in the type name at the one
place a reader goes to check what closing means. `WhyClose` is what the
decision is actually over; `DaemonExit` stays and converts into it, so the
quit path keeps asking its own lifecycle question.

- [ ] **Step 1: Write the failing test**

Append inside `mod tests` in `deskwarden/src/ui_process.rs`:

```rust
    /// **A workstation lock closes the window, and it is the same decision
    /// function that says so.**
    ///
    /// This arm is only ever reached downstream of
    /// `away_lock::locks_the_vault`, which is what makes it correct for this
    /// function to have no opinion about `auto_lock`: by the time a
    /// `TheUserWalkedAway` exists, the gate has already been passed.
    #[test]
    fn walking_away_closes_the_vault_window() {
        assert_eq!(
            farewell_to_an_open_window(WhyClose::TheUserWalkedAway, Some(4242)),
            Farewell::CloseIt { pid: 4242 },
            "a decrypted vault rendered on screen must not survive the moment its owner \
             locked the workstation and left; the daemon's own cache and bw serve are torn \
             down in the same breath and this process is the only thing left holding one"
        );
        assert_eq!(
            farewell_to_an_open_window(WhyClose::TheUserWalkedAway, None),
            Farewell::NothingOpen,
            "no window open is nothing to close -- the ordinary tray-only state this whole \
             feature was originally written for"
        );
    }

    /// The positive control on the test above: adding the third arm must not
    /// have flattened the distinction the first two arms exist to draw. A
    /// `match` that had degenerated into `Some(pid) => CloseIt` would pass
    /// every assertion above and fail exactly this one.
    #[test]
    fn a_restart_still_leaves_the_window_alone_now_that_a_third_reason_exists() {
        assert_eq!(
            farewell_to_an_open_window(WhyClose::DaemonIsRestarting, Some(4242)),
            Farewell::NothingOpen,
            "an update or a crash must still not close the user's window mid-edit"
        );
        assert_eq!(
            farewell_to_an_open_window(WhyClose::DaemonIsQuitting, Some(4242)),
            Farewell::CloseIt { pid: 4242 },
            "control: the quit arm the two callers below rely on is unchanged"
        );
    }

    /// The two existing call sites pass a `DaemonExit`. The conversion must be
    /// the identity they were relying on, or this refactor silently changes
    /// what quitting does.
    #[test]
    fn the_daemon_exit_reasons_convert_to_the_same_decisions_they_made_before() {
        assert_eq!(
            farewell_to_an_open_window(DaemonExit::UserQuit.into(), Some(7)),
            Farewell::CloseIt { pid: 7 }
        );
        assert_eq!(
            farewell_to_an_open_window(DaemonExit::Restart.into(), Some(7)),
            Farewell::NothingOpen
        );
        assert_eq!(WhyClose::from(DaemonExit::UserQuit), WhyClose::DaemonIsQuitting);
        assert_eq!(WhyClose::from(DaemonExit::Restart), WhyClose::DaemonIsRestarting);
    }
```

Then **delete** the three superseded tests in the same module —
`quitting_closes_the_vault_window_rather_than_leaving_it_showing_the_vault`,
`a_restart_leaves_the_window_alone_because_the_daemon_is_coming_back`, and
`a_quit_with_no_window_open_has_nothing_to_close` — whose assertions are
carried, `DaemonExit`-spelled, by
`the_daemon_exit_reasons_convert_to_the_same_decisions_they_made_before` and
`a_restart_still_leaves_the_window_alone_now_that_a_third_reason_exists`. Move
the first one's reasoning comment onto the `WhyClose::DaemonIsQuitting`
variant, where it now belongs.

- [ ] **Step 2: Run it and watch it fail**

```bash
export CARGO_TARGET_DIR=/e/_dw_agent/run
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- ui_process
```

Expected: **compile error**, `cannot find type 'WhyClose' in this scope`.

- [ ] **Step 3: Implement**

Replace `farewell_to_an_open_window` and add `WhyClose` beside `DaemonExit` in
`deskwarden/src/ui_process.rs`:

```rust
/// **Why the open UI window is being closed**, which is not the same question
/// as why the daemon is going away.
///
/// [`DaemonExit`] answers the second and converts into this; it keeps its own
/// name because the quit path genuinely is asking a daemon-lifecycle question.
/// The third reason is not a daemon lifecycle event at all -- the daemon is
/// running, will keep running, and has just been told by Windows that the
/// person using it left the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhyClose {
    /// The tray's *Quit*: the user asked for the app to be gone. The quit
    /// handler has just killed `bw serve` and cleared the vault cache, the
    /// breach results and the clipboard, precisely so that nothing decrypted
    /// outlives the moment the user said to go away -- and a vault window left
    /// running is a process still showing that user's entire decrypted vault,
    /// on screen, with no app behind it and no auto-lock timer that means
    /// anything any more.
    DaemonIsQuitting,
    /// The process is ending and expects to be back -- an update swapping the
    /// binary, or a crash. **The window stays up.** This is the same
    /// distinction [`UiSpawnPlan::joins_the_daemons_job`] is about, drawn one
    /// level up: the daemon comes back, brings `bw serve` up on the same port,
    /// and the window's next request succeeds.
    DaemonIsRestarting,
    /// Windows reported that the user walked away -- Win+L, a session switch,
    /// or a suspend -- **and `away_lock::locks_the_vault` said that locks this
    /// vault**. That gate is why this arm carries no preference of its own: a
    /// value of this variant cannot exist unless the user's own auto-lock
    /// setting already answered yes.
    ///
    /// The daemon is not going anywhere. What is going away is the decrypted
    /// vault, and the largest piece of it is in another process.
    TheUserWalkedAway,
}

impl From<DaemonExit> for WhyClose {
    fn from(exit: DaemonExit) -> Self {
        match exit {
            DaemonExit::UserQuit => WhyClose::DaemonIsQuitting,
            DaemonExit::Restart => WhyClose::DaemonIsRestarting,
        }
    }
}

/// Whether the daemon closes the UI window it has open.
///
/// **Two of the three reasons close it and one does not**, and that
/// distinction is the whole content of this function. A daemon *restart* must
/// not close the user's window, because the daemon comes back and recovery is
/// a retry rather than a handshake. A **Quit** is not a restart: nothing comes
/// back. A **workstation lock** is not a restart either, and is the one reason
/// here that leaves the daemon alive -- what ends is the decrypted vault, and
/// this process is holding the visible copy of it.
///
/// The cost of both closing arms is an edit in progress in that window. It is
/// the same cost the window's own idle auto-lock already charges, which closes
/// the viewport without asking (`vault_window::idle_frame` -> `IdleFrame::Lock`).
pub fn farewell_to_an_open_window(reason: WhyClose, open: Option<u32>) -> Farewell {
    match (reason, open) {
        (WhyClose::DaemonIsQuitting | WhyClose::TheUserWalkedAway, Some(pid)) => {
            Farewell::CloseIt { pid }
        }
        // Matched rather than caught by a wildcard so that a fourth reason is
        // a compile error here -- the one place that has to weigh it -- rather
        // than a silent inheritance of "leave it alone".
        (WhyClose::DaemonIsRestarting, _) | (_, None) => Farewell::NothingOpen,
    }
}
```

Then update `close_on_quit`'s call in `main.rs` so the crate still builds:

```rust
        let reason = deskwarden::ui_process::WhyClose::from(
            deskwarden::ui_process::DaemonExit::UserQuit,
        );
```

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 -- ui_process
```

Expected: three tests pass, zero warnings, and the binary still compiles.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/ui_process.rs deskwarden/src/main.rs
git commit -F <message file>
```

Message: a third reason for closing the window, and why it is a new enum
rather than a third `DaemonExit`.

---

### Task 2: The kill, beside `close_on_quit`

**Files:** Modify `deskwarden/src/main.rs`

**Interfaces**

- *Consumes:* `ui_process::{WhyClose, Farewell, farewell_to_an_open_window, result_path, forget_result}`.
- *Produces:* `UiWindows::close_because_the_user_walked_away(&mut self, config_dir: &Path)`.

This is the impure half and it is a near-twin of `close_on_quit`. The one
difference between them is the one that matters, and it is why this is a
second method rather than a parameter on the first: **the daemon keeps
running after this one**, so the slot it empties and the file it deletes are
not about tidiness. See the collision Task 3 pins.

- [ ] **Step 1: Write the failing test**

In `main.rs`'s existing source-pin test module (the one containing
`both_shutdown_paths_clear_the_clipboard_as_a_quit`, around line 12440), add:

```rust
    /// **The away-lock kill empties the registry slot**, and that is not
    /// tidiness.
    ///
    /// `Child::kill` on Windows is `TerminateProcess` with exit code 1, and
    /// `UiVaultResult::EXIT_LOCKED` is 1. Left in the slot, the killed child
    /// would be reaped on the next pass of a loop that is still running,
    /// decoded as `locked: true`, and handed to `run_vault_loop` as a lock to
    /// act on -- a second `resettle_session` and a second master-password
    /// prompt for a window the daemon killed itself. `close_on_quit` gets away
    /// without caring because `process::exit(0)` is the next line; this one
    /// does not.
    #[test]
    fn the_walked_away_kill_takes_the_slot_so_the_kill_is_not_reaped_as_a_lock() {
        let source = include_str!("main.rs");
        let body = source
            .split_once(concat!("fn close_because_the_user_walked_", "away("))
            .expect("control: the away-lock close must be defined in this file")
            .1
            .split_once("\n    }")
            .expect("the method must be brace-terminated at method indentation")
            .0;
        assert!(
            body.contains("self.vault.take()"),
            "the slot must be emptied in the same breath as the kill; got {body:?}"
        );
        assert!(
            body.contains("forget_result"),
            "the child will never have written a result worth reading, and a file nobody \
             reads is one the user's config directory should not keep; got {body:?}"
        );
        assert!(
            !body.contains("read_result"),
            "reading the killed child's result file is the exact mistake this test exists \
             to prevent: exit code 1 is EXIT_LOCKED; got {body:?}"
        );
    }

    /// The exit-code collision the test above is about, asserted rather than
    /// asserted-about. If Windows or this crate ever stopped colliding here,
    /// the reasoning above would be stale and this test says so first.
    #[test]
    fn a_terminated_process_status_collides_with_the_locked_exit_code() {
        assert_eq!(
            deskwarden::ui_process::UiVaultResult::EXIT_LOCKED,
            1,
            "TerminateProcess (which is what Child::kill is on Windows) sets exit code 1. \
             These being equal is why the away-lock kill must take the registry slot"
        );
        assert!(
            deskwarden::ui_process::UiVaultResult::from_exit_code(1).locked,
            "control: status 1 really does decode as a lock, so leaving a killed child in \
             the slot really would produce a spurious one"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --bin deskwarden -- the_walked_away_kill a_terminated_process_status
```

Expected: `the_walked_away_kill_takes_the_slot_so_the_kill_is_not_reaped_as_a_lock`
panics with *"control: the away-lock close must be defined in this file"*, and
`a_terminated_process_status_collides_with_the_locked_exit_code` **passes** —
it is a statement about code that already exists, and its passing now is what
makes the first test's reasoning true rather than decorative.

- [ ] **Step 3: Implement**

In `deskwarden/src/main.rs`, in `impl UiWindows`, immediately after
`close_on_quit`:

```rust
    /// **Close the open window, because Windows says the user walked away.**
    ///
    /// The decision is [`deskwarden::ui_process::farewell_to_an_open_window`]
    /// under [`deskwarden::ui_process::WhyClose::TheUserWalkedAway`], and the
    /// gate in front of *that* is `away_lock::locks_the_vault` -- already
    /// asked, and already answered yes, by the only caller
    /// ([`lock_after_walking_away`]). Nothing here re-reads a setting.
    ///
    /// **A near-twin of [`UiWindows::close_on_quit`], and the difference is
    /// the whole reason it is a second method.** That one runs on the way to
    /// `process::exit(0)`; this one runs in the middle of a loop that keeps
    /// going. So emptying the slot is load-bearing rather than tidy:
    /// `Child::kill` is `TerminateProcess` with exit code 1,
    /// `UiVaultResult::EXIT_LOCKED` is 1, and a killed child left in the slot
    /// would be reaped on the next pass and read back as a lock the user never
    /// asked for -- a second `resettle_session` and a second master-password
    /// prompt for a window this function killed itself. See
    /// `the_walked_away_kill_takes_the_slot_so_the_kill_is_not_reaped_as_a_lock`.
    ///
    /// **Nothing is waited on.** The caller is about to block on a
    /// master-password prompt that can stand there for hours; a window that
    /// will not die must not be able to hold that up, and there is nothing to
    /// learn from the wait anyway. The result file is deleted rather than
    /// read, for the reason `close_on_quit` gives and one more: its exit code
    /// is ours.
    ///
    /// **Visibility is never asked about**, and that is deliberate. This works
    /// over the process id the spawn returned, so a UI process left resident
    /// and hidden -- `keep_ui_loaded`, on another branch -- is killed by this
    /// same line with no change here, provided it is still recorded in this
    /// slot. A hidden process holding a decrypted vault across a workstation
    /// lock is strictly worse than a visible one: nothing on screen reminds
    /// the user it is there.
    fn close_because_the_user_walked_away(&mut self, config_dir: &Path) {
        let reason = deskwarden::ui_process::WhyClose::TheUserWalkedAway;
        if let deskwarden::ui_process::Farewell::CloseIt { pid } =
            deskwarden::ui_process::farewell_to_an_open_window(reason, self.vault_pid())
        {
            let mut open = self.vault.take().expect("a pid means the slot is occupied");
            log::info!(
                "Windows reported the user walked away with the vault window (process {pid}) \
                 open; closing it rather than leaving a decrypted vault on a machine its \
                 owner has left"
            );
            if let Err(e) = open.child.kill() {
                log::warn!(
                    "could not close the vault window's process {pid} ({e}); the vault is \
                     still being locked behind it, but that process may still be showing it"
                );
            }
            deskwarden::ui_process::forget_result(&deskwarden::ui_process::result_path(
                config_dir, pid,
            ));
        }
    }
```

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --bin deskwarden -- the_walked_away_kill a_terminated_process_status
```

Expected: both pass. A `dead_code` warning on the new method is expected to
**fail the build** under `-D warnings` until Task 3 calls it — that is the
correct failure and Task 3 is next. If you must see this task green in
isolation, run Tasks 2 and 3 back to back and commit at the end of Task 3;
do not add an `#[allow(dead_code)]` to buy an intermediate green.

- [ ] **Step 5: Commit**

Commit together with Task 3 (see above). Paths: `deskwarden/src/main.rs`.

---

### Task 3: `lock_after_walking_away` can reach the window

**Files:** Modify `deskwarden/src/main.rs`

**Interfaces**

- *Consumes:* `UiWindows::close_because_the_user_walked_away`.
- *Produces:* `lock_after_walking_away`'s new `ui: &mut UiWindows` parameter,
  and the call in `main`'s loop that passes it.

`main` already owns `ui_windows` (declared around line 2279) and already has
it in scope at the `pump_windows_messages()` call site around line 2447, so
this is a parameter and an argument and nothing more.

**The order inside the function is the substance of this task.** After the
`locks_the_vault` gate returns early:

1. `clipboard::clear_if_still_ours_for(ClearTrigger::Lock)` — unchanged, still
   first, for the reason already written there.
2. **the window** — before the resettle, because `resettle_session` blocks on a
   master-password prompt that can stand there for hours. A window closed
   after that prompt is answered is a window that survived the whole absence.
3. `resettle_session` — unchanged.

- [ ] **Step 1: Write the failing test**

In the same source-pin module as Task 2's tests:

```rust
    /// **The window is closed before the resettle, not after.**
    ///
    /// `resettle_session` blocks on a master-password prompt, and that prompt
    /// can stand on screen for as long as the user is away -- which is the
    /// entire duration this feature exists to cover. A close sequenced after
    /// it is a vault window that survives the whole absence and is torn down
    /// at the moment the user comes back and types their password, which is
    /// precisely when it no longer matters.
    ///
    /// A source pin because `lock_after_walking_away` takes an `AppTray` and a
    /// `SessionEstate` and cannot be called from any test in this crate -- the
    /// same reason `both_shutdown_paths_clear_the_clipboard_as_a_quit` reads
    /// source text one screen above.
    #[test]
    fn the_walked_away_lock_closes_the_window_before_it_blocks_on_the_password_prompt() {
        let source = include_str!("main.rs");
        let body = source
            .split_once(concat!("fn lock_after_walking_", "away("))
            .expect("control: the away-lock effect must be defined in this file")
            .1
            .split_once("\nfn ")
            .expect("the function must be followed by another item")
            .0;

        let close = body
            .find(concat!("close_because_the_user_walked_", "away("))
            .expect(
                "the away-lock path must close the vault window; without this the user \
                 presses Win+L and leaves a decrypted vault rendered in a second process",
            );
        let clipboard = body
            .find(concat!("clear_if_still_ours_", "for("))
            .expect("control: the clipboard clear is still on this path");
        let resettle = body
            .find(concat!("resettle_", "session("))
            .expect("control: the resettle is still on this path");

        assert!(
            clipboard < close,
            "the clipboard clear stays first: it is microseconds and it is the one thing \
             that outlives this process entirely"
        );
        assert!(
            close < resettle,
            "the window must be closed BEFORE resettle_session, which blocks on a \
             master-password prompt for as long as the user is away"
        );
    }

    /// The away-lock close has exactly one caller, and it is the gated one.
    ///
    /// A second call site would be a second place that could reach the kill
    /// without passing `away_lock::locks_the_vault` first -- which is the only
    /// thing making `WhyClose::TheUserWalkedAway` free of a preference of its
    /// own.
    #[test]
    fn nothing_but_the_gated_away_lock_path_closes_the_window_for_walking_away() {
        let source = include_str!("main.rs");
        assert_eq!(
            source.matches(concat!("close_because_the_user_walked_", "away(")).count(),
            2,
            "expected exactly the definition and its one call inside \
             lock_after_walking_away, and no others"
        );
        assert_eq!(
            source.matches(concat!("close_on_", "quit(")).count(),
            2,
            "control: the quit path's own killer still has exactly its definition and its \
             one call, so the count above is measuring what it claims to"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --bin deskwarden -- walked_away
```

Expected: the first test panics on its `.expect(...)` for
`close_because_the_user_walked_away(`; the second fails with `left: 1, right: 2`.

- [ ] **Step 3: Implement**

In `deskwarden/src/main.rs`, add the parameter to `lock_after_walking_away`
(it already carries `#[allow(clippy::too_many_arguments)]`):

```rust
fn lock_after_walking_away(
    est: &mut SessionEstate,
    away: deskwarden::away_lock::AwayEvent,
    job: &Arc<Option<job_object::KillOnCloseJob>>,
    schedule: &[Duration],
    tray: &tray::AppTray,
    backend_op_rx: &Arc<Mutex<mpsc::Receiver<BackendOp>>>,
    config_dir: &Path,
    first_run_account: Option<&accounts::AccountId>,
    // **The registry, so that this path can reach the second process.** Its
    // absence from this list was the defect: the vault window has run in a
    // process of its own since the daemon/UI split, the daemon's loop pumps
    // messages the whole time it is up, and this function was locking
    // everything except the largest decrypted thing there was.
    ui: &mut UiWindows,
) {
```

Immediately after the existing
`deskwarden::clipboard::clear_if_still_ours_for(deskwarden::clipboard::ClearTrigger::Lock);`
line, and **before** the `let SessionEstate { .. }` destructuring that leads
into the resettle:

```rust
    // **Before the resettle, and that ordering is the point.**
    // `resettle_session` below blocks on a master-password prompt which can
    // stand on screen for as long as the user is away -- the whole duration
    // this feature exists to cover. A window closed after it is a window that
    // survived the entire absence. See
    // `the_walked_away_lock_closes_the_window_before_it_blocks_on_the_password_prompt`.
    ui.close_because_the_user_walked_away(config_dir);
```

And in `main`'s loop, at the existing call:

```rust
        if let Some(away) = pump_windows_messages() {
            lock_after_walking_away(
                &mut estate,
                away,
                &job,
                &schedule,
                &tray,
                &backend_op_rx,
                &config_dir,
                first_run_account.as_ref(),
                &mut ui_windows,
            );
        }
```

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --bin deskwarden -- walked_away
RUSTFLAGS="-D warnings" cargo build --manifest-path deskwarden/Cargo.toml -j 2
```

Expected: both tests pass, the binary builds, zero warnings (Task 2's
`dead_code` warning is gone now that the method has a caller).

If the build fails on a borrow conflict — `ui_windows` borrowed mutably while
something else in the loop holds it — **stop and report** rather than
restructuring the loop; the call site chosen above is inside the `if let` on
`pump_windows_messages()`, where nothing else is live.

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/main.rs
git commit -F <message file>
```

Message: the away-lock path reaches the vault window's process, the ordering
against the password prompt, and why the registry slot is emptied.

---

### Task 4: The two doc comments that assert the opposite

**Files:** Modify `deskwarden/src/away_lock.rs`, `deskwarden/src/main.rs`

**Interfaces**

- *Consumes:* nothing. *Produces:* nothing. Doc text only, plus one test.

Two doc comments in the tree say this defect cannot happen. They were true
when written and stopped being true at the daemon/UI process split. Left in
place they are worse than no comment: the next person to look at
`away_lock` reads that the vault-window case is covered and stops.

- [ ] **Step 1: Write the failing test**

In `deskwarden/src/away_lock.rs`'s `mod tests`:

```rust
    /// **The module doc must not still claim the vault-window case is
    /// somebody else's.**
    ///
    /// It said so, correctly, when the vault window ran a nested `eframe` loop
    /// inside this process. Since the daemon/UI split the window is a separate
    /// process, the daemon's loop pumps throughout, and this module's decision
    /// governs that window too -- via
    /// `main::lock_after_walking_away` -> `UiWindows::close_because_the_user_walked_away`.
    /// A stale reassurance here is how the defect survived: a reader checking
    /// whether the window was covered found a paragraph saying it was.
    #[test]
    fn the_module_doc_does_not_claim_the_pump_is_asleep_while_a_window_is_up() {
        let source = include_str!("away_lock.rs");
        assert!(
            source.contains("its own process"),
            "control: the module doc must actually describe the arrangement it has -- the \
             vault window is a separate process and this module's decision reaches it"
        );
        assert!(
            !source.contains("the pump does not run while a vault window is up"),
            "this sentence was true before the daemon/UI process split and is now the \
             reassurance that hid a security defect"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- away_lock
```

Expected: fails on the first assertion (`its own process` is not in the file
yet), which is the control — proving the second assertion is being evaluated
against a file this test actually found.

- [ ] **Step 3: Implement**

In `deskwarden/src/away_lock.rs`, replace the module-doc paragraph beginning
*"One consequence, stated rather than hidden:"* (currently lines ~39–45) with:

```rust
//! One consequence, restated after the daemon/UI split changed it: the vault
//! window runs in **its own process** (`deskwarden.exe --ui vault`), the
//! daemon does not block on it, and this pump therefore runs the whole time
//! that window is up. So [`locks_the_vault`]'s answer governs two things and
//! not one -- the daemon's own session, and the second process holding a
//! decrypted vault on screen. `main::lock_after_walking_away` acts on both,
//! the second through `UiWindows::close_because_the_user_walked_away`.
//!
//! This paragraph used to say the opposite, and it was right at the time: the
//! window was a nested `eframe` loop inside this process, nothing pumped
//! while it ran, and the window's own idle auto-lock was all that covered the
//! user. It is recorded here because a stale reassurance in this spot is
//! exactly how a decrypted vault came to survive Win+L -- see
//! `docs/superpowers/specs/2026-08-29-the-lock-closes-the-window-design.md`.
//!
//! The one state where the old paragraph still holds is the in-daemon
//! fallback window, opened when `spawn_the_vault_window_in_its_own_process`
//! could not start a process at all. That window is a nested loop, this pump
//! does not run while it is up, and its own idle auto-lock is what covers the
//! user there.
```

In `deskwarden/src/main.rs`, in `lock_after_walking_away`'s doc, replace the
final paragraph's stale sentence — *"A vault WINDOW being open cannot reach
here at all: that window runs its own nested loop and this one is not being
pumped while it is up -- and its own idle auto-lock is what covers the user
there."* — with:

```rust
/// **A vault window being open is now the case this function most needs to
/// handle**, and until this change it was the one it could not. The window
/// runs in a process of its own, the daemon's loop pumps throughout, and
/// `ui.close_because_the_user_walked_away` above is what reaches it. The
/// in-daemon fallback window -- opened only when no UI process could be
/// started -- is still a nested loop that this pump does not run beneath, and
/// its own idle auto-lock is still what covers the user there.
```

- [ ] **Step 4: Run it and watch it pass**

```bash
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 -- away_lock
```

- [ ] **Step 5: Commit**

```bash
git add deskwarden/src/away_lock.rs deskwarden/src/main.rs
git commit -F <message file>
```

Message: the two comments that said this could not happen, and why they are
corrected rather than deleted.

---

### Task 5: The `keep_ui_loaded` rendezvous

**Files:** Modify `docs/superpowers/specs/2026-08-29-the-lock-closes-the-window-design.md` only.

**Interfaces**

- *Consumes:* whatever `keep_ui_loaded`'s branch actually does, **read, not
  assumed**. *Produces:* a recorded answer in the design's Status section.

`keep_ui_loaded` is on another branch. This task writes **no Rust** and makes
**no guess** about which state it is in. It exists so that the question is
answered by reading rather than by either branch assuming the other handled
it.

- [ ] **Step 1: Find out whether the branch exists here yet**

```bash
git branch -a --list '*keep*ui*' '*keep_ui_loaded*'
git log --all --oneline -20 --grep='keep_ui_loaded'
git grep -n 'keep_ui_loaded' $(git rev-list --all --max-count=200) -- deskwarden/src | head -20
```

- [ ] **Step 2: If it is not reachable, record that and stop**

Append to the design's **Status** section:

> `keep_ui_loaded` was not reachable from this checkout on 2026-08-29 (`git
> branch -a` and an all-refs grep found nothing). The dependency stands as
> written in *The hidden window*: whichever branch lands second must confirm
> that a resident-hidden UI process is recorded in `UiWindows.vault`, because
> that slot is the sole input to
> `UiWindows::close_because_the_user_walked_away`.

- [ ] **Step 3: If it *is* reachable, read exactly one thing and record it**

The only question is: **when the window is hidden rather than closed, is the
child still recorded in `UiWindows.vault`?**

```bash
git show <branch>:deskwarden/src/main.rs | grep -n 'keep_ui_loaded' -A 20 -B 5
```

Append to the design's **Status** section, one of:

> `keep_ui_loaded` (`<branch>`, `<sha>`) leaves the resident process recorded
> in `UiWindows.vault`, so this work covers the hidden window with no change.
> Verified by reading `<file>:<line>`.

or:

> `keep_ui_loaded` (`<branch>`, `<sha>`) records the resident process in
> `<where>`, which `UiWindows::close_because_the_user_walked_away` does not
> read. A hidden window therefore survives Win+L on the merge of these two
> branches. Closing that is `keep_ui_loaded`'s obligation and is filed as
> `<issue/note>`; the fix is one line, because the close is over a pid.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-08-29-the-lock-closes-the-window-design.md
git commit -F <message file>
```

Message: what the other branch actually does, read rather than assumed.

---

### Task 6: The live check — a real process, a real Win+L

**Files:** none. This is a human running a build, by hand, on a real desktop.

**Interfaces**

- *Consumes:* a `deskwarden.exe` built from this branch, a real vault, a real
  Windows session. *Produces:* a paragraph in the design's Status section
  recording the two observations and their dates.

**Why this task exists at all.** This is a security property about a real
process surviving a real workstation lock, and **no test in this crate
observes that.** Every test above is a pure function over values or a string
found in a source file. They are the right tests for the decisions they cover
and they prove nothing about `TerminateProcess` reaching a `--ui vault` child
whose window is up. `away_lock`'s own module doc already says the manual check
is the check: *"press Win+L, come back, unlock Windows, and confirm Deskwarden
is asking for the master password."* This extends it by one process.

**The negative control is not optional.** A live check that only ever watched
processes die would pass identically on a build that killed the vault window
unconditionally — ignoring `locks_the_vault`, ignoring the user's setting.
Half of this task is watching a process **survive**.

- [ ] **Step 1: Build and start it**

```bash
export CARGO_TARGET_DIR=/e/_dw_agent/run
RUSTFLAGS="-D warnings" cargo build --manifest-path deskwarden/Cargo.toml -j 2
/e/_dw_agent/run/debug/deskwarden.exe
```

Sign in and reach an unlocked vault. In Preferences, confirm auto-lock is
**on** with a long timeout (30 minutes) — long enough that nothing observed
below can be the idle timer.

- [ ] **Step 2: Open the window and record its process id**

Tray → *Open Vault*. Then, in a second shell:

```bash
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='deskwarden.exe'\" | Select-Object ProcessId, CommandLine | Format-List"
```

Two processes are expected: the daemon (no `--ui`) and the window
(`--ui vault`). **Write down the `--ui vault` pid.** If only one appears, the
spawn fell back to the in-daemon window and this check is measuring the wrong
thing — restart and try again before continuing.

- [ ] **Step 3: Press Win+L. Wait ten seconds. Unlock Windows.**

Ten seconds so that nothing observed can be blamed on the check being faster
than the daemon's loop.

- [ ] **Step 4: The positive observation**

```bash
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='deskwarden.exe'\" | Select-Object ProcessId, CommandLine | Format-List"
```

Required: **the `--ui vault` pid from Step 2 is gone.** The daemon's pid is
still there. Deskwarden is asking for the master password. Also confirm, from
`away_lock`'s own wording, that pasting yields nothing you copied out of the
vault.

Then check the daemon did not talk itself into a second lock:

```bash
grep -iE 'walked away|resettle|locking the vault' "$LOCALAPPDATA/Deskwarden/deskwarden.log" | tail -20
```

Required: **one** "locking the vault" line and **one** close line for that
pid. Two would be the `EXIT_LOCKED` collision Task 2 exists to prevent, and
would mean the registry slot was not taken.

- [ ] **Step 5: The negative control — auto-lock OFF**

Restart, sign in, and in Preferences turn auto-lock **off**. Open the vault
window. Record the `--ui vault` pid. Press Win+L, wait ten seconds, unlock.

Required: **that pid is still running**, and Deskwarden is **not** asking for
the master password. That is the design's answer to question 3 — the setting
means what it says, and the window is not a second lock policy hiding behind
one that is switched off.

If the process is gone here, the gate is being bypassed: something is
constructing `WhyClose::TheUserWalkedAway` without passing
`away_lock::locks_the_vault`, and
`nothing_but_the_gated_away_lock_path_closes_the_window_for_walking_away`
should have caught it. Stop and report.

- [ ] **Step 6: The suspend arm, if a machine is available**

Repeat Step 2 with auto-lock on, then sleep the machine
(`rundll32.exe powrprof.dll,SetSuspendState 0,1,0`) rather than pressing
Win+L. Wake it. Required: the same observation as Step 4.

Record if the observation differs — the design flags this arm as the one where
`PBT_APMSUSPEND`'s short and unguaranteed window may not leave time for the
kill to land, and a differing result is information, not a failure of this
plan.

- [ ] **Step 7: Record it**

Append to the design's **Status** section the two (or three) observations,
each with its date, the pid observed, and whether it was gone or still there.
An undated "verified manually" is not a record.

- [ ] **Step 8: Commit**

```bash
git add docs/superpowers/specs/2026-08-29-the-lock-closes-the-window-design.md
git commit -F <message file>
```

Message: the live check, both observations, and why the negative control is
half of it.

---

### Task 7: The whole suite, against a baseline

**Files:** none.

- [ ] **Step 1: Run it**

```bash
export CARGO_TARGET_DIR=/e/_dw_agent/run
RUSTFLAGS="-D warnings" cargo test --manifest-path deskwarden/Cargo.toml -j 2 2>&1 | tail -40
```

- [ ] **Step 2: Judge the failures**

The `rest::`/`vault_cache::`/`picker_ui::`/`bw_serve::` mock-HTTP family fails
~74 times per full run with shifting membership and is being fixed by somebody
else. Do not investigate a failure in that family, and do not report it as
caused by this work, without first comparing against a baseline on a clean
tree.

**Any failure outside those four families is this work's**, in particular
anything in `ui_process`, `away_lock`, or the `main.rs` pin modules. Those are
the ones this change actually touches, and none of them does any I/O.

- [ ] **Step 3: Report**

Zero warnings, the count and family membership of every failure, and the
baseline comparison if one was needed.

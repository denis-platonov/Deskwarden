# The Startup Window in Its Own Process Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the daemon drawing the vault window when Deskwarden is launched by hand, so a double-click no longer costs ~50 MB of OpenGL driver for the life of the process.

**Architecture:** The tray door already spawns `deskwarden.exe --ui vault` and reaps its result. The startup door does not. This points the startup door at the same machinery — no new mechanism, one door moved onto an existing one.

**Tech Stack:** Rust, the existing `ui_process` / `UiSpawnPlan` / `spawn_out_of_any_job` seam.

## Why this is a plan and not an edit

Measured on 2026-08-26, from the running installed build's own log: **3 launches opened the window as its own process, 32 drew it in the daemon.** The 32 are every hand-launch. `first_surface` maps `LaunchIntent::UserLaunch` to `FirstSurface::ShowTheWindow` and `LaunchIntent::LoginAutostart` to `StayInTheTray`, so autostart never hits it — which is why the 0.10.0 measurements looked right and nothing caught this.

The code is `main.rs:1341`, and it is the riskiest path in the app:

- `startup_vault` is declared at `main.rs:996` and dispatched a thousand lines later at `main.rs:2252`.
- **Four source pins read this file's own text and assert the shape of this region**: `main.rs:28004`, `28009`, `28129`, `28555`. They exist because this path has been broken by well-meaning edits before. Any change here fails them, and re-pinning without understanding what each one guards would be defeating the guard rather than satisfying it.
- The current shape answers a real user report — *"On start there is another window Setting up your vault and then actual window loads"* — which is why it is one window with two stages rather than a spinner and then a window. **A fix that reintroduces two windows has traded one complaint for another.**

## Global Constraints

- **No test may touch** the network, the real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, a real dialog, or spawn `bw`.
- **No `cfg(test)` seams.** Banned crate-wide.
- **Never build into `deskwarden/target`.** Use `CARGO_TARGET_DIR=/e/_dw_agent/run`.
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.
- **Run the FULL suite after every task**, not a filter. This crate's local runs carry 40–140 loopback failures from the machine's TCP dynamic port range starting at 1024; the check that works is "did a module with no `mockito` in it fail", and CI is the arbiter. A filtered read of this suite is what let three real failures reach CI on 2026-08-26 and killed the 0.11.0 release.
- Branch: `startup-ui-process`.

## The shape of the change

**The UI process is spawned FIRST, and the daemon does its startup work behind it.**

Not "probe, then spawn": the readiness probe can take ~8 s on a cold `bw serve`, and a user who double-clicked would stare at nothing for all of it. The UI process is the same binary, already shows a frame in ~470 ms (measured), and already does its own readiness wait and vault load — that is exactly what the tray door relies on today.

So the daemon:

1. spawns the UI process (window appears, owned by that process),
2. runs `wait_for_vault_ready` itself — it needs the items to arm the match engine, and this is unchanged work,
3. `arm_autofill_and_seed_cache` as now,
4. reaps the UI process's result through the same path the tray door uses.

Step 2 is *not* removable: the daemon's match engine needs the item list whether or not a window is open. It is the same call the `StayInTheTray` arm already makes.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/main.rs` (modify) | The `ShowTheWindow` branch, and whichever of the four pins genuinely describe a shape that changed. |

---

### Task 1: Prove the two doors differ, before changing either

**Files:** Modify `deskwarden/src/main.rs` (test module below the cut)

The claim this whole plan rests on is that one door spawns and the other does not. That is currently asserted by nothing — it was found by reading a log. A test that reads this file's own source, in `bw_serve_gate`'s idiom, makes the difference visible and makes Task 2's fix provable.

- [ ] **Step 1: Write the failing test**

```rust
    /// **The startup door must open the window the same way the tray door
    /// does**: by spawning a UI process, never by building a frame in this
    /// process.
    ///
    /// Read over this file's own source, in `bw_serve_gate`'s idiom, because
    /// the two doors are a thousand lines apart and nothing else compares
    /// them. Found by reading a running build's log -- 3 launches spawned, 32
    /// drew in the daemon -- and asserted here so it cannot come back.
    #[test]
    fn the_startup_door_spawns_a_ui_process_like_the_tray_door_does() {
        let source = code(include_str!("main.rs"));
        let opener = concat!("if surface == FirstSurface::Show", "TheWindow {");
        let region = source
            .split_once(opener)
            .expect("the startup branch must still exist")
            .1
            .split_once(concat!("match engine loaded with ", "{} app match(es)"))
            .expect("the startup branch must still be followed by the engine log")
            .0;
        assert!(region.len() > 200, "control: the region is empty, so this proves nothing");
        assert!(
            !region.contains(concat!("app_window::run_from_", "working")),
            "the startup door still builds a vault frame in the daemon, which loads the \
             OpenGL driver into the process that holds the tray for the rest of its life"
        );
        assert!(
            region.contains(concat!("spawn_the_vault_window_in_its_own_", "process")),
            "the startup door does not spawn a UI process"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- the_startup_door_spawns
```

Expected: FAIL on the first assertion — the region still contains `run_from_working`. **This failure is the bug, reproduced.** Do not proceed until it fails for that reason and not because the region came back empty.

- [ ] **Step 3: Commit the failing test**

Committed red, on its own, so the defect is in the history as something that was demonstrated before it was fixed.

---

### Task 2: Point the startup door at the spawn

> **ATTEMPTED 2026-08-26 AND REVERTED. Read this before writing any code.**
>
> Task 2 as written below is **not achievable**, and two existing pins are why.
> Both were found by making the mistake, not by reading ahead. The attempt is
> reverted; Task 1's red test stands.
>
> **First wrong turn: spawning in the startup branch.** The obvious edit is to
> call `spawn_the_vault_window_in_its_own_process()` where `run_from_working`
> was. It compiles, and it leads straight to `child.wait()` — because that
> block has nothing to poll a child with. `the_daemon_never_blocks_on_a_ui_
> process_pin` forbids `child.wait()` **by name**, and its doc says why: the
> owner reported *"Ctrl Alt B should work with main window open"* twice, and
> blocking there leaves the hotkey queued until the window closes. The pin
> caught it immediately.
>
> **Second wrong turn: asking for the window at the dispatcher.** With
> blocking ruled out, the window has to be requested where `ui_windows`
> exists — the `if startup_vault.is_some()` call at roughly `main.rs:2160`.
> That works, satisfies the no-blocking pin, and breaks a *different* one:
>
> > the launch that already has a session shows no window of its own any more,
> > so the user watches nothing at all for the eight seconds `bw serve` takes
>
> Because the dispatcher runs **after** `wait_for_vault_ready`. The plan's own
> "spawn first, probe behind it" ordering is right and cannot be had there.
>
> **Third wrong turn, and this one only Task 3 could find.** Moving
> `ui_windows` above the startup block *does* work: the window is asked for
> before the probe, the loop polls it, all four re-pinned guards pass, clippy
> is clean, and the daemon comes up with **`nvoglv64.dll` absent** — 35.7 MB
> against the 99.3 MB the same build showed when it drew its own window. The
> mechanism is right.
>
> It still fails in the user's hands:
>
> > Your vault could not be loaded — the vault backend did not become ready
> > within the deadline; last error: `http://localhost:8087/list/object/items:
> > connection timed out`
>
> **The UI process started before `bw serve` did.** The old shape probed for
> readiness *first* and drew the window afterwards, so the window always found
> a live backend and the spinner covered the wait. Spawning first removes that
> guarantee — and with `keep_backend_running: false` (save-memory mode) there
> was no backend running at all, so the child's own deadline expired against
> nothing.
>
> The tray door does not have this problem because `open_vault_window` starts
> the backend **before** it asks for the window. The startup door asked
> without starting.
>
> So the ordering is not "ask, then probe" but **"start the backend, ask,
> then probe"** — start being cheap and non-blocking, readiness being the slow
> part that both processes can wait through in parallel. That is a fourth
> constraint on top of the three above, and it is the reason Task 3 is in this
> plan and cannot be skipped: every automated check passed on a change that
> did not work.
>
> **Reverted in `3721d22`.** Task 1's red test stands.
>
> **What this actually needs.** `ui_windows` is created at `main.rs:2168`,
> about a thousand lines below the startup block. Every route to a correct fix
> goes through **moving the registry's creation above the startup block** so
> the window can be requested before the probe runs, and the loop can poll the
> child the whole time. That is a change to `main`'s startup ordering, not to
> this branch, and it deserves its own plan with the estate/tray dependencies
> mapped first — `ui_windows` is declared where it is because the comment
> above it says the tray must exist, and the tray is built just above it.
>
> Until then the defect stands and is understood: **32 hand-launches drew in
> the daemon, 3 spawned.**

**Files:** Modify `deskwarden/src/main.rs`

- [ ] **Step 1: Replace the branch body**

The `ShowTheWindow` branch becomes, in order: spawn the UI process; run `wait_for_vault_ready`; `arm_autofill_and_seed_cache`; reap. Keep `recover_from_failed_vault_wait` on the probe-failure arm — it is unchanged and still the answer to a backend that never came up.

`startup_vault` is filled from the reaped `UiVaultResult` rather than from `warm.vault`. `ui_process::UiVaultResult` already carries all six daemon-actionable outcomes, so nothing new crosses the boundary.

- [ ] **Step 2: Run the failing test and watch it pass**

- [ ] **Step 3: Run the FULL suite**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2
```

Four pins will fail: `main.rs:28004`, `28009`, `28129`, `28555`. **Read each one before touching it.** Each names the specific mutation it caught; a pin whose subject genuinely moved is re-pinned with a commit message saying what moved and why, and a pin whose subject did not move is a real failure telling you the change went further than intended.

- [ ] **Step 4: Commit**

---

### Task 3: Prove it on the running app, not only in tests

**Files:** none

The assertion in Task 1 is about source text. The claim is about a process's memory, and only a running build can answer it.

- [ ] **Step 1: Build release and launch by double-click**

- [ ] **Step 2: Check the daemon's loaded modules**

```powershell
(Get-Process deskwarden).Modules | Where-Object { $_.ModuleName -match 'nvoglv|opengl32' }
```

Expected: **empty for the daemon**, non-empty for the `--ui vault` child. Before this change the daemon had both and sat at 99.3 MB.

- [ ] **Step 3: Close the window and re-measure the daemon**

Expected: it returns to roughly its pre-window figure and stays there. Before this change it held ~40–60 MB for the life of the process.

- [ ] **Step 4: Check the log says the right thing**

`the vault window is process N (… --ui vault)` on a hand-launch, and no `the warm launch window showed`.

- [ ] **Step 5: Confirm the user report has not been reopened**

One window, not a spinner followed by a separate window at a different size. This is the acceptance criterion the current code exists to satisfy, and it is the one a test cannot check.

---

## What this does NOT do

- **It does not split the binary.** `deskwarden-tray.exe` and the linker-level guarantee are `2026-08-26-two-binaries-design.md`, and that work is gated on the updater being able to swap a set of files atomically. This plan closes the 32 launches; it does not stop a 33rd door being written.
- **It does not touch autostart**, which never reached this branch.
- **It does not change what the window looks like.**

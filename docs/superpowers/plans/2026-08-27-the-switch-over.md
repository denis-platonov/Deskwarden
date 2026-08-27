# The Switch-Over Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `vault_service`'s counting actually govern `bw serve`, so the backend outlives the daemon exactly as long as another app is using it -- and no longer.

**Architecture:** Replace the kill-on-close job object's guarantee for the `bw serve` child with the attachment count landed in `vault_service`. Nothing else moves.

**Tech Stack:** Rust, `windows` 0.58, the `vault_service` module merged in PR #4.

---

## Read this before writing any code: the benefit is narrower than it looks

`2026-08-27-the-vault-service-outlives-both-apps.md` was written as though the
vault window dies with the daemon. **It does not.** `spawn_vault_ui_process`
deliberately does not call `job_object::spawn_in_job`, and its doc says why:

> A daemon restart is routine [...] Assigned to the job, every one of those
> would close the vault window the user had open, mid-edit. [...] when the
> daemon comes back it brings `bw serve` up on the same constant port and the
> window's next request succeeds. **Recovery is a retry, not a handshake.**

So the window already survives. What the switch-over adds is narrow and
specific:

**Today:** the daemon exits, the job kills `bw serve`, and an open vault
window's next request fails until some daemon returns and restarts the
backend.

**After:** the daemon exits while a window is attached, `bw serve` keeps
running, and the window keeps working with no gap.

That is a real improvement to a real user-visible stall. It is **not** "the
window can now run alone", which is already true today.

## What it costs, stated plainly

The job object guarantees `bw serve` **cannot** outlive the app, by any exit
route, enforced by the kernel. `main.rs` says what that buys: without it "an
orphan survives the app and holds `BW_SERVE_PORT` against every later
launch", and worse, it orphans "an unlocked-vault server".

After this change that guarantee is gone, replaced by:

- a five-second supervisor tick (`ORPHAN_CHECK_INTERVAL`), and
- the correctness of `win_is_held`, which was **wrong on its first writing**
  and reported "nobody attached" while two processes were attached. Under
  this change, that same bug becomes "an unlocked vault stays on localhost
  with nobody watching it".

**This is the decision the plan exists to surface.** It is a security trade
made to remove a stall, and it should be taken deliberately or not at all.

## The cheaper alternative, to be rejected explicitly or taken

The stall is "the window's backend is gone until a daemon returns". That can
also be fixed by **letting the window start `bw serve` itself** when it finds
the port dead -- `ensure_running` used by the UI process, with the job object
left exactly as it is. The orphan guarantee survives, because whoever starts
a backend still owns it in their own job.

This is strictly less powerful (a backend restart costs a cold start and, on
the direct backend, a Windows Hello prompt) and strictly safer. It should be
compared against the full switch-over before either is built.

---

## Global Constraints

- **No test may touch** the network, the real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, a real dialog, or spawn `bw`. Kernel objects go behind `fn` pointers.
- **No `cfg(test)` seams.** Banned crate-wide.
- **One target directory**, `CARGO_TARGET_DIR=/e/_dw_agent/run`.
- **`--lib` does not run `main.rs`'s tests.** Use `--bin deskwarden` too.
- **CI is the arbiter.** The local suite yields a different failing set on each run, and CI only triggers on `main` and on PRs to `main`.
- **`job_object` keeps a ledger** of every `.rs` outside `src/`, and `foreground` keeps one of every module. Both will catch new files, as designed.
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.
- Branch: `the-switch-over`.

---

### Task 0: The decision

**No code.** Answer, in writing, in this file:

- [x] Is the stall worth trading the kernel's orphan guarantee for? **YES -- the owner chose the full switch-over on 2026-08-27**, with the cost above stated in front of them. Task 1a is therefore NOT built.
- [x] If **yes**: does the daemon keep the job object for its *other* children? **Yes. Only `bw serve` moves.** Every other `spawn_in_job` caller is untouched, and the vault window was never in the job to begin with.

---

### Task 1a (if the alternative wins): the window starts its own backend

**Files:** Modify `deskwarden/src/main.rs` (the `--ui` entry path)

The UI process calls `vault_service::ensure_running` before its first vault
request, and spawns `bw serve` **into its own kill-on-close job** if nothing
is there. The daemon is unchanged, and nothing is ever orphaned, because
every backend is in the job of whoever started it.

- [ ] **Step 1: Write the failing test** -- the UI path adopts a verified running service and does not spawn; and spawns into a job when the port is silent.
- [ ] **Steps 2-5:** red, implement, full suite, commit.

---

### Task 1b (if the switch-over wins): `win_stop` becomes real

**Files:** Modify `deskwarden/src/vault_service.rs`, `deskwarden/src/main.rs`

`win_stop` currently logs that it is not wired. It must actually end the
`bw serve` on that port. The daemon owns the child handle, so the handle has
to reach the supervisor -- which is the whole reason `bw serve` moves out of
the job.

- [x] **Step 1: Write the failing test** -- stopping goes through the owned child handle and NOT through "kill whatever is on the port", which would end a process this app cannot identify. Task 2 of the previous plan refuses to do exactly that, and this must not reintroduce it by the back door.
- [x] **Steps 2-5:** red first, implemented, committed.

---

## BLOCKER found while wiring Task 2: the last app cannot stop the service

After Task 3 takes `bw serve` out of the job:

1. the daemon starts it, the vault window attaches;
2. the daemon is killed -- `bw serve` survives, which is the feature;
3. the window closes, sees nobody attached, tries to stop the service;
4. **it cannot.** It never started that child, so `stop_action(false)` says leave it alone.

The child is out of the job, its parent is gone, and the only code that could end it has no handle. `bw serve` holds an unlocked vault on localhost indefinitely -- the exact failure the job object exists to prevent, reached by following this plan.

Task 2 assumed the supervisor always lives in the process that spawned the backend. Once the backend outlives that process, it does not -- and that is precisely the scenario the switch-over exists for.

**Tasks 2 and 3 are blocked until this is resolved.** Shipping Task 3 without it creates, silently, the security failure the whole design was built to avoid.

### The options

**A. A verified service may be stopped by any of our apps.** `verify` already proves a service is ours (it holds `Local\Deskwarden-Vault-<our fingerprint>`). It refuses to kill the *unidentifiable*; a verified one is ours by definition. Needs a pid, which the current source pin forbids -- so the pin would gain a narrow, named exception, and the service would have to publish its pid somewhere only it can write.

**B. The service supervises itself.** `bw serve` is not ours to modify, so this means a small supervisor process of ours that owns the child, starts with the first attachment and exits with the last. It is the honest shape -- the thing that owns the backend is the thing that outlives the apps -- and it is a new process, which is real work.

**C. Keep the job, take the cheaper alternative** (Task 1a): the window starts its own `bw serve` into its own job when it finds the port dead. No orphan is ever possible. Costs a cold start and, on the direct backend, a Hello prompt.

- [ ] **Decide between A, B and C before writing any more code.**

---

### Task 2 (switch-over only, BLOCKED): the supervisor runs

**Files:** Modify `deskwarden/src/main.rs`

A thread calling `supervise` on `ORPHAN_CHECK_INTERVAL`, and `release` on
clean shutdown so the common case does not wait five seconds for something
that is already known.

- [ ] **Step 1: Write the failing test** -- shutdown releases the slot before the process ends, and the supervisor stops the backend when the last slot clears.
- [ ] **Steps 2-5:** red, implement, full suite, commit.

---

### Task 3 (switch-over only): `bw serve` leaves the job

**Files:** Modify `deskwarden/src/main.rs`

The one line that gives up the guarantee. Last, deliberately: everything that
replaces it is already running and observed by the time this flips.

- [ ] **Step 1:** flip it, and rewrite the comment that currently explains why `bw serve` is in the job so that it explains why it no longer is.
- [ ] **Steps 2-4:** full suite, commit.

---

## Verification

- [ ] Full suite `--lib` and `--bin deskwarden`; CI as arbiter.
- [ ] `cargo clippy --all-targets` and `cargo deny check licenses advisories` clean.
- [ ] **A running build, and this one cannot be skipped.** Start the tray, open the vault window, close the tray, confirm the window still works. Kill both from Task Manager and confirm nothing is left holding `BW_SERVE_PORT`. Every previous attempt in this area passed every automated check and failed in the user's hands.
- [ ] **Ask before launching Deskwarden.** Starting a build trips `single_instance`'s takeover and kills the user's running app; that has already happened once in this project.
- [ ] State plainly how long an orphan can hold the port, and say the number.

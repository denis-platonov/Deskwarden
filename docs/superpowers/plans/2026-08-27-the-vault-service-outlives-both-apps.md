# The Vault Service Outlives Both Apps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the vault service something both apps start, reconnect to, and let go of — so neither app owns the vault and either can run alone.

**Architecture:** Liveness is a kernel fact, not a count this app keeps. Each app holds a named handle while it needs the vault; the service exits when the last one is released. An app that finds a service already running **reconnects after proving it is ours**, and only restarts one it cannot verify.

**Tech Stack:** Rust, `windows` 0.58 — named mutexes (`app_mutex`'s idiom), job objects (`job_object`), and the existing `bw_serve` spawn.

## What this changes, and what it gives up

Today `bw serve` is spawned suspended into a `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` job before it runs one instruction, so **the kernel guarantees it cannot outlive the app**. `main.rs`'s own comment says what that buys: without it "an orphan survives the app and holds `BW_SERVE_PORT` against every later launch", and worse, orphans "an unlocked-vault server".

Reference-counting gives that guarantee up. `2026-08-27-one-door-to-the-vault.md` accepts the trade — one rule beats one guarantee — but the acceptance is only honest if the two consequences are handled rather than noted:

**The held port stops being a hazard and becomes the mechanism.** An orphan holding `BW_SERVE_PORT` is exactly what the next app reconnects to. That inverts the old failure into the new feature — **provided the app can tell the difference between our service and something else on that port.**

**That verification is the piece the job object made unnecessary.** It has to be built, and it is the security-critical part of this plan: a process that answers on a loopback port is not, by itself, ours. See Task 2.

## Global Constraints

- **No test may touch** the network, the real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, a real dialog, or spawn `bw`. Kernel objects go behind `fn` pointers, as `single_instance::TakeoverEnv` and `DiskCacheEnv` already do.
- **No `cfg(test)` seams.** Banned crate-wide.
- **One target directory**, `CARGO_TARGET_DIR=/e/_dw_agent/run`. A second cost 5.5 GB and filled the disk; `run/debug/incremental` reaching 19 GB is safe to delete.
- **`--lib` does not run `main.rs`'s tests.** Use `--bin deskwarden` too.
- **CI builds with `-D warnings`**, and pins `cargo-deny ^0.20`. Validate against what CI installs.
- **The local suite is not trustworthy**: 40–140 loopback failures, cause unknown — port range and concurrency both investigated and disproven. The check that works is *did a module with no `mockito` in it fail*. CI is the arbiter.
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.
- Branch: `service-outlives-apps`.

---

### Task 1: The attachment handle

**Files:** Create `deskwarden/src/vault_service.rs`; modify `deskwarden/src/lib.rs`

**Interfaces:**
```rust
pub struct Attachment(/* private */);      // held while this app needs the vault
pub fn attach(env: &ServiceEnv) -> Attachment;
pub fn anyone_attached(env: &ServiceEnv) -> bool;
```

**The rule:** an `Attachment` is a *kernel object*, released by the OS on process death — clean exit, crash, or kill alike. Nothing decrements a number.

### Which primitive, and the trap that rules most of them out

**Added 2026-08-27 before implementing, after reading `app_mutex`.** That
module documents the thing that breaks the obvious design:

> a `CreateMutexW` that finds the name already there still *opens a handle to
> it*, and a named object lives as long as any handle does

So **one shared named object cannot answer "is anyone attached"**: the
process asking keeps the name alive by asking. `take_if_free` exists precisely
because an incoming instance polling with `acquire` "would wait out its whole
timeout against itself". Any design where the supervisor tests a shared name
has that bug.

Nor can the attachment be a mutex *owned* by each app: a mutex has one owner
at a time, and two apps must be attached at once.

**Nor is the connection itself enough**, which is the other obvious answer.
It works for a persistent pipe, where a client vanishing is a kernel-visible
disconnect -- but `bw serve` is HTTP on a port, with no persistent per-client
connection to count. A mechanism that only works for one of the two services
reintroduces the split this design exists to remove.

**What does work: one named object per app, and a list of names the
supervisor tries to open.**

- Each app creates `Local\Deskwarden-Attach-<uuid>` and holds it for as long
  as it needs the vault. The OS releases it on death, however the death
  happened.
- The app records that name where the supervisor can find it.
- The supervisor **opens each name and drops the handle immediately**. A name
  that fails to open has no live holder. Transient handles do not keep a name
  alive past the drop, which is what makes this safe where testing a shared
  name is not.
- No app is attached when no recorded name opens.

**The list of names is a hint, not a count.** A stale entry costs one failed
`OpenMutexW`; a missing entry costs an app that is attached but unseen, which
is why the entry is written *before* the vault is used and not after. This is
the one place bookkeeping appears, and it is deliberately the kind that
degrades into a wrong answer nobody acts on rather than a leak.

`app_mutex` is the precedent and not a second scheme: it already answers "is Deskwarden running?" for the installer, per logon session, with `Local\` scoping. This asks the same question about a different subject.

- [x] **Step 1: Write the failing tests** — that two attachments both report attached, that dropping one leaves `anyone_attached` true and dropping both makes it false, and — **the test the design exists for** — that an attachment abandoned *without* being dropped (the crash case, simulated through the `ServiceEnv` seam) still reports detached.

- [x] **Steps 2–5:** implemented and committed. **Red-first was not honoured** -- tests and implementation were written together. What stood in for it: `asking_never_makes_a_name_live` failed on its first run and its own control is what caught the reason (it was asking about a cleanly released name, which is removed from the register, so `is_held` was never consulted).

---

### Task 2: Proving a running service is ours

**Files:** Modify `deskwarden/src/vault_service.rs`

This is the piece the job object made unnecessary and reference-counting makes essential.

**The question:** something is listening on `BW_SERVE_PORT`. Is it our `bw serve`, for this account, or is it another process — another user's, a different app's, or something hostile that grabbed the port first?

**What is not sufficient, and why each fails:**

- *It answered on the port.* Any process can bind a loopback port. Loopback is not an authentication boundary; on a shared machine another logon session can listen too.
- *It answered our shape of JSON.* So can anything that has read this repository.
- *The pid matches something we remember.* Pids are reused, and the thing this must survive is precisely the case where the app that remembered it is gone.

**What the plan requires instead:** a **named kernel object the service creates and only the service can hold**, checked before the port is trusted, plus the account fingerprint the service is serving. Both are already idioms here — the mutex from `app_mutex`, the fingerprint from `vault_disk_cache`'s header (`SHA-256(email ‖ 0x1F ‖ server)`), which exists so a cache file cannot be read for the wrong account.

**An unverifiable service is not adopted and not killed.** It is left alone and the app reports that it could not start its backend. Killing a process this app cannot identify is worse than refusing to use it.

- [ ] **Step 1: Write the failing tests** — a service that holds the object and matches the fingerprint is adopted; one that holds it but serves a *different* account is refused; one that answers the port while holding nothing is refused and **not killed**.

- [ ] **Steps 2–5:** red, implement, full suite, commit.

---

### Task 3: Start, reconnect, exit

**Files:** Modify `deskwarden/src/vault_service.rs`, `deskwarden/src/main.rs`

- **First attachment starts the service.** Two apps launching together must not start two: the winner is decided by the same named-object race `single_instance` already resolves, not a second scheme.
- **Losing the race is not an error.** The loser waits briefly and attaches to the winner's service.
- **The last attachment released exits the service.**
- **Reconnect precedes restart**, always. Restarting costs a cold start and, on the direct backend, another Hello prompt.

**The job object moves rather than disappearing.** `bw serve` is still spawned suspended into a job — but the job is held by whatever supervises the count, so the kernel still kills it when *nothing* is attached, rather than when one particular app dies. That keeps most of the old guarantee: the orphan window is between the last release and the supervisor noticing, not unbounded.

- [ ] **Step 1: Write the failing tests** — two concurrent starts produce one service; the loser attaches rather than failing; releasing the last attachment stops it; releasing one of two does not.

- [ ] **Steps 2–5:** red, implement, full suite, commit.

---

### Task 4: The window that is not eliminated

**Files:** Modify `deskwarden/src/vault_service.rs`

The spec is explicit that the orphan window is **bounded, not eliminated**, and that "a test that only drives clean exits does not show that it does".

- [ ] **Step 1: Write the failing test** — every app crashes (attachments abandoned, not released) and the service is observed to stop. Drive it through the seam; do not kill a real process in a test.
- [ ] **Step 2:** implement whatever closes it — a supervisor that re-checks `anyone_attached` on an interval is the cheapest thing that works, and its interval is the window's size and should be named as such.
- [ ] **Steps 3–5:** full suite, commit.

---

## What this plan does NOT do

- **The apps still share one binary.** `2026-08-26-two-binaries-design.md` is separate and still gated on the updater swapping a set atomically.
- **The daemon still holds passwords.** That needs the snapshot to narrow, which needs the vault windows, which this plan makes possible but does not do.
- **No UI.** A service running with no window and no tray is a state a user can see in Task Manager; naming it for them is real work and is not here.

## Verification

- [ ] Full suite, `--lib` and `--bin deskwarden`, with the no-mockito check; CI as arbiter.
- [ ] `cargo clippy --all-targets` and `cargo deny check licenses advisories` clean.
- [ ] **A running build, and this one cannot be skipped.** Every previous attempt in this area passed every automated check and failed in the user's hands. Start the tray, open the window, close the tray, confirm the window still works; kill both from Task Manager and confirm nothing is left holding the port.
- [ ] Say plainly whether an orphan can still hold `BW_SERVE_PORT`, and for how long. The spec promises bounded, not zero, and the number should be stated rather than implied.

# The Cache Moves Behind the Door Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every vault read go through one door, with the encrypted cache *inside* it, so no consumer reads a cache file and no consumer knows whether one exists.

**Architecture:** `VaultBackend` is already the door — `VaultBridge` (`bw serve`) and `RestBackend` both implement it. What is wrong is which side of it the cache sits on. A `CachingBackend` wraps a `VaultBackend` and consults the version 2 file first; consumers hold a `VaultBackend` and stop being able to tell.

**Tech Stack:** Rust. The v2 cache format and `open_item` already exist; this moves the caller, it does not build a mechanism.

## Why this first, and not the process split

`2026-08-27-one-door-to-the-vault.md` describes a service both apps start, stop and reconnect to. That is three moving parts at once — a boundary, a process, and a lifecycle — and only the boundary can be established without the other two.

So this plan does the boundary **in-process**. Nothing gains or loses a process; what changes is that "does a read consult the cache" stops being a question any consumer can ask. When the service becomes its own process, the door it exposes is the one this plan draws, and the move is mechanical rather than a redesign.

**It also repays a debt made hours earlier.** `VaultCache::item_from_disk` landed on 2026-08-27 as a consumer-facing direct file read, which the one-door design forbids. It is the right mechanism in the wrong place; this is where it moves.

## Global Constraints

- **No test may touch** the network, the real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, a real dialog, or spawn `bw`. `DiskCacheEnv`'s `fn` pointers already stand in for Hello and DPAPI.
- **No `cfg(test)` seams.** Banned crate-wide.
- **One target directory**, `CARGO_TARGET_DIR=/e/_dw_agent/run`. A second cost 5.5 GB and filled the disk; `run/debug/incremental` reached 19 GB and is safe to delete.
- **`--lib` does not run `main.rs`'s tests.** Use `--bin deskwarden` too.
- **CI builds with `-D warnings`.** A discarded `#[must_use]` is an error there and a warning here.
- **CI pins `cargo-deny ^0.20`.** Validate config against the version CI installs, not whatever is local.
- **The local suite is not trustworthy.** 40–140 loopback failures, cause unknown — the port range and concurrency were both investigated and disproven. The check that works is *did a module with no `mockito` in it fail*, and CI is the arbiter.
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.
- Branch: `cache-behind-the-door`.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/vault_backend.rs` (modify) | `CachingBackend`: a `VaultBackend` that consults the cache first. |
| `deskwarden/src/vault_cache.rs` (modify) | `item_from_disk` stops being public; the disk cache is reached only from the caching backend. |
| `deskwarden/src/app.rs` (modify) | The fill asks its backend, not a file. |
| `deskwarden/src/backend_policy.rs` (modify) | The read-path choice, beside the existing service choice. |

---

### Task 1: The door is asserted before it exists

**Files:** Modify `deskwarden/src/vault_cache.rs`

- [ ] **Step 1: Write the failing test** — a source pin, in `bw_serve_gate`'s idiom, that the disk cache's per-item read is reachable from exactly one module.

```rust
    /// **One door.** `vault_disk_cache`'s per-item read exists so the vault
    /// service can answer "one item"; a consumer calling it directly is a
    /// second way to reach the vault, and the whole point of
    /// `2026-08-27-one-door-to-the-vault.md` is that there is not one.
    ///
    /// Read over source rather than enforced by visibility because the two
    /// live in the same crate: `pub(crate)` would still let `app.rs` call it,
    /// and moving the module is a bigger change than this rule needs.
    #[test]
    fn only_the_caching_backend_reaches_the_cache_file() {
        let callers = crate_files_mentioning(concat!("item_from_", "disk"));
        assert_eq!(
            callers,
            vec!["vault_backend.rs", "vault_cache.rs"],
            "something outside the caching backend reads the cache file directly, so a read \
             can now bypass the vault service: {callers:?}"
        );
    }
```

Expected to fail listing `app.rs`, which is exactly the debt being repaid.

- [ ] **Step 2–3:** run it red, commit it red.

---

### Task 2: `CachingBackend`

**Files:** Modify `deskwarden/src/vault_backend.rs`

**Interfaces:**
```rust
pub struct CachingBackend { inner: Arc<dyn VaultBackend>, disk: Arc<DiskCache> }
impl VaultBackend for CachingBackend { /* reads consult the file, writes do not */ }
```

**The rule that makes it safe:** **reads may be answered from the cache; writes never are.** A write goes to `inner` and its result refreshes what the cache holds. A caching layer that answered a write from a file would be a vault that disagrees with the server, which is the defect class this project has already paid for once in `rest::write`.

`get_item` is the one that matters: it consults `open_item` and falls through. `list_items` does not — a caller asking for the whole vault is asking for the vault, and the snapshot already serves that.

- [ ] **Step 1: Write the failing tests** — that `get_item` answers from the file without touching `inner` (counting fake), that a miss falls through, that **every write reaches `inner`** (drive all of them; a new write method that silently cached would be the bug), and that after `clear()` a `get_item` falls through rather than answering.

- [ ] **Steps 2–5:** red, implement, full suite, commit.

---

### Task 3: The read-path setting

**Files:** Modify `deskwarden/src/backend_policy.rs`, `deskwarden/src/prefs_ui.rs`

`backend_policy::choose` already answers *which service*. This adds *which read path*, as a second pure function beside it, so both decisions are made in the module that owns decisions and neither is inferred at a call site.

**Walk the combinations before drawing the row.** The spec names one that looks wrong — `bw serve` with a cache-first read, where the cache is refreshed by a subprocess the user asked not to keep running. Decide what that does, and write the answer in the module doc rather than letting the code imply it.

- [ ] Steps as above, ending with the full suite and a commit.

---

### Task 4: The fill asks its backend

**Files:** Modify `deskwarden/src/app.rs`, `deskwarden/src/vault_cache.rs`

- [ ] **Step 1:** `item_from_disk` stops being `pub`, and `app.rs`'s fill path asks the backend. Task 1's pin goes green.

- [ ] **Step 2:** `autofill_really_fills_from_a_restored_snapshot` must still pass — a fill with `bw serve` stopped, now served by the caching backend rather than by a resident password. **That test is the acceptance criterion and must not be weakened.**

- [ ] **Steps 3–5:** full suite, `--bin deskwarden` as well, commit.

---

## What this plan does NOT do

- **No process moves.** The service is still in-process; `2026-08-27-one-door-to-the-vault.md`'s lifecycle is a later plan.
- **The daemon still holds passwords in its snapshot.** That is `nothing_the_daemon_can_reach_hands_back_a_password`, still `#[ignore]`d, and it needs the snapshot to narrow — which needs the vault windows, which needs the service to be a process.
- **No `bw serve` ownership change.** Decided (same lifecycle as REST) and not implemented here.

## Verification

- [ ] Full suite, `--lib` and `--bin deskwarden`, with the no-mockito check; CI as arbiter.
- [ ] `cargo clippy --all-targets` and `cargo deny check licenses advisories` clean.
- [ ] **A running build**: turn the disk cache on, stop the backend, and fill. The three previous attempts in this area all passed every automated check and failed in the user's hands.
- [ ] Say plainly that the daemon still holds the passwords. It will still be true.

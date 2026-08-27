# The Tray Stops Holding Passwords Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the daemon keeping every password in the vault decrypted in memory for the whole session.

**Architecture:** The daemon's three uses of vault data are separated. Two of them — the match engine and the account picker — need identifiers and display text and nothing else. The third, a fill, needs one secret at the moment it types it, and already knows how to fetch one. So the cached *items* become cached *metadata*, and the secret is fetched by id when it is used.

**Tech Stack:** Rust. No new dependencies; this removes data rather than adding machinery.

## Why this is worth doing

`VaultCache::snapshot` is a `Vec<VaultItem>`, and `LoginData::password` is a `Zeroizing<String>`. On the owner's vault that is **1,666 decrypted passwords resident in the process that owns the tray, the hotkey and the match engine**, for as long as the vault is unlocked. `clear()` empties it on lock — but the owner's settings are `auto_lock_enabled: false` with a 999-minute timeout, so in practice the exposure is "until you quit", on a process designed to run for days.

Nothing needs them there. Established by reading the code, not assumed:

- **`MatchEngine::rebuild(&mut self, entries: &[(String, AppMatch)])`** — the fill path's index is *already* a projection. It never sees a `VaultItem`.
- **`app::SEARCH_CORPUS` holds no secrets today** and says so in its own comment.
- **The fill already fetches by id.** `app.rs:444` calls `cache.bridge().get_item(item_id)`. Today that is the *miss* arm, reached when `get_by_id` returns `None`; this plan makes it the only arm.

So the only reason the passwords are there is that `picker_offers_for` calls `cache.items()`, which clones the whole vault, and `get_by_id` hands a whole item to the fill.

## Scope, and the one thing this plan does NOT finish

This plan makes the daemon **stop using** cached passwords, and removes them from every path it can. It does **not** delete the password field from the daemon's snapshot outright, and the reason is a real dependency rather than caution:

**The sign-in path still draws the vault window inside the daemon.** The owner's log shows `single window: showing SignIn` → `showing Working` → `showing Vault` on a first-run or re-auth launch, and that window reads the daemon's cache directly. Emptying the snapshot's passwords breaks it.

So the final step — the snapshot itself carrying no secret — is **gated on that window becoming a UI process**, which is `2026-08-26-startup-window-in-its-own-process.md` and its four known constraints. Task 4 below writes the assertion and marks it `#[ignore]` with the reason, so the finish line is in the code rather than in someone's memory.

What Tasks 1–3 deliver on their own: the tray no longer *reads* a cached password on any path a user exercises, and a test proves it. That is worth having before the gate opens.

## Global Constraints

- **No test may touch** the network, the real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, a real dialog, or spawn `bw`.
- **No `cfg(test)` seams.** Banned crate-wide.
- **Never build into `deskwarden/target`.** Use `CARGO_TARGET_DIR=/e/_dw_agent/run`.
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.
- **Run the full suite after every task**, and remember `main.rs`'s tests need `--bin deskwarden` — `--lib` silently reports "0 filtered out" and runs none of them.
- **This machine's runs carry 40–140 loopback failures** from a TCP dynamic port range starting at 1024. The check that works is *did a module with no `mockito` in it fail*; CI is the arbiter. A filtered read of this suite is what let three real failures reach CI and killed the 0.11.0 release.
- Branch: `tray-holds-no-passwords`.

## The decision this plan is built on

**Machines without Windows Hello keep today's in-memory path.** That is the owner's decision, taken 2026-08-27 against the alternative of sealing under DPAPI alone. It means the encrypted disk cache stays optional and two storage shapes stay alive.

**It does not affect this plan**, and that is worth stating because it looks like it should: the projection is about what the daemon *holds*, not about where the vault is *stored*. A tray that fetches secrets by id holds no passwords whether they came from a disk cache, `bw serve`, or a direct REST sync. The Hello question belongs to the follow-on plan that makes the cache the vault's home.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/vault_cache.rs` (modify) | A projection type, and the accessor that hands one out. |
| `deskwarden/src/app_candidates.rs` (modify) | Take the projection instead of `&[VaultItem]`. |
| `deskwarden/src/app.rs` (modify) | `picker_offers_for` uses the projection; the fill fetches by id. |

---

### Task 1: Prove the tray holds passwords

**Files:** Modify `deskwarden/src/vault_cache.rs`

The claim is the whole reason for the plan and nothing asserts it. Written first, committed **red**, so the defect is demonstrated rather than described.

**Interfaces:** Consumes `VaultCache`, `VaultSnapshot`, the existing test fixtures in `vault_cache::tests`.

- [ ] **Step 1: Write the failing test**

```rust
    /// **The daemon must not hold a decrypted password.**
    ///
    /// It owns the tray, the hotkey and the match engine, and it runs for
    /// days. `clear()` empties the snapshot on lock, but auto-lock is a
    /// setting a user can turn off -- the owner's is off -- so "until the
    /// vault locks" is in practice "until the process exits".
    ///
    /// Driven through the public accessors a caller actually has, not by
    /// reaching into the field: what matters is what the daemon can *reach*,
    /// and a private field it cannot read is not an exposure this test is
    /// about.
    #[test]
    fn nothing_the_daemon_can_reach_hands_back_a_password() {
        let cache = a_cache_holding(vec![an_item_with_password("i1", "hunter2")]);
        let reachable: Vec<String> = cache
            .items()
            .into_iter()
            .filter_map(|item| item.login.and_then(|l| l.password).map(|p| p.to_string()))
            .collect();
        assert!(
            reachable.is_empty(),
            "the daemon can read {} cached password(s) out of its own snapshot",
            reachable.len()
        );
    }
```

Use the module's existing fixture helpers rather than new ones; if `a_cache_holding` and `an_item_with_password` do not exist under those names, use whatever the neighbouring tests use and keep the assertion identical.

- [ ] **Step 2: Run it and watch it fail**

```bash
CARGO_TARGET_DIR=/e/_dw_agent/run cargo test --manifest-path deskwarden/Cargo.toml -j 2 --lib -- nothing_the_daemon_can_reach
```

Expected: FAIL, reporting one reachable password. **That failure is the bug.** If it passes, stop — the premise is wrong and the rest of this plan is unnecessary.

- [ ] **Step 3: Commit it red**

---

### Task 2: A projection, and the picker on it

**Files:** Modify `deskwarden/src/vault_cache.rs`, `deskwarden/src/app_candidates.rs`, `deskwarden/src/app.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct ItemFacts { pub id: String, pub name: String, pub username: String, pub uris: Vec<String>, pub item_type: Option<i64> }
  impl VaultCache { pub fn facts(&self) -> Vec<ItemFacts>; }
  ```
- Changes: `app_candidates::candidates(exe_name: &str, title: &str, items: &[ItemFacts]) -> Vec<Candidate>`.

**Why a new type and not `VaultItem` with the password blanked:** a type that *could* carry a secret and happens not to is one edit away from carrying one again, and nothing would fail. `ItemFacts` has no field a secret fits in, which is the assertion made structural.

- [ ] **Step 1: Write the failing test**

```rust
    /// `ItemFacts` is the projection, and it is a projection by CONSTRUCTION:
    /// there is no field on it a password, TOTP seed, note or card number
    /// could be put in. Asserted over the type's own debug shape so that
    /// adding such a field fails here rather than being noticed later.
    #[test]
    fn the_projection_has_nowhere_to_put_a_secret() {
        let facts = ItemFacts {
            id: "i1".to_string(),
            name: "Example".to_string(),
            username: "me@example.com".to_string(),
            uris: vec!["https://example.com".to_string()],
            item_type: Some(1),
        };
        let rendered = format!("{facts:?}").to_lowercase();
        for forbidden in ["password", "totp", "notes", "card", "sshkey", "identity"] {
            assert!(
                !rendered.contains(forbidden),
                "`ItemFacts` grew a {forbidden} field, so the daemon's projection can carry a \
                 secret again"
            );
        }
    }
```

- [ ] **Step 2: Run it and watch it fail** — `cannot find struct ItemFacts`.

- [ ] **Step 3: Write the implementation**

Add `ItemFacts` and `VaultCache::facts()`, which maps the snapshot without cloning secrets. Repoint `app_candidates::candidates` at `&[ItemFacts]` and `picker_offers_for` at `cache.facts()` instead of `cache.items()`.

`picker_offers` and `picker_corpus` take the same slice. `Candidate` already carries `id`, `name` and `username` only, so nothing downstream of the matcher changes.

- [ ] **Step 4: Run the full suite** (`--lib` and `--bin deskwarden`). `app_candidates`'s own tests build `VaultItem` fixtures and will need their helper repointed at `ItemFacts`; the assertions should not change, and if one has to, that is a signal the projection dropped something the matcher used.

- [ ] **Step 5: Commit**

---

### Task 3: A fill fetches its secret

> **BLOCKED, found 2026-08-27 while writing the test. Do not implement as
> written.**
>
> Making the fill always call `get_item` breaks a shipped feature, and the test
> one screen below the one this task was modelled on says so:
>
> > **Autofill really fills from a snapshot restored off the encrypted disk
> > file, with `bw serve` genuinely absent** … the path that makes autofill
> > work with the backend fully stopped.
>
> The cached password is not a convenience. It is what lets a fill work with no
> backend running at all — the whole point of `cache_vault_to_disk`, and what
> makes a cache-first launch possible. This plan's cost section called that a
> *latency* trade ("a fill may pay a round trip"). It is a **capability**
> trade, and the plan was wrong.
>
> **What unblocks it is the spec, not a workaround**: with the encrypted cache
> as the vault's home, the daemon reads **one item** out of the file on demand
> and decrypts it, instead of either holding 1,666 in RAM or needing a live
> backend. `vault_disk_cache` is whole-snapshot today
> (`load(fingerprint) -> DiskCacheLoad`), so **per-item read is the piece to
> build**, and it belongs to
> `2026-08-27-the-vault-lives-in-a-place-not-a-process.md`.
>
> **So this branch delivers Task 2 and stops.** That is a real reduction — the
> account picker and *Search vault* no longer clone the vault, and
> `app_candidates` cannot see a password at all — but it is not the headline,
> and the headline needs the cache work.

**Files:** Modify `deskwarden/src/app.rs`

**Interfaces:** Consumes `VaultCache::backend_handle` / `bridge().get_item`.

- [ ] **Step 1: Write the failing test**

```rust
    /// **The fill asks the backend for the item, always.** It used to prefer
    /// a cached copy and fall back to the backend on a miss, which is why the
    /// daemon held every password: the cache existed to answer this.
    ///
    /// Asserted through the backend seam rather than by timing: the fake
    /// records that `get_item` was called, and a fill that answered from the
    /// snapshot would not call it at all.
    #[test]
    fn a_fill_asks_the_backend_for_the_item_even_when_the_snapshot_has_one() {
        let backend = a_recording_backend_holding(an_item_with_password("i1", "hunter2"));
        let cache = a_cache_on(backend.clone());
        cache.populate_with(a_snapshot_holding("i1"), cache.epoch());
        fill_from_item(&cache, "i1", /* the module's usual arguments */);
        assert_eq!(
            backend.get_item_calls(),
            1,
            "the fill answered from the cached item, so the cached password is load-bearing \
             and the daemon still has to hold it"
        );
    }
```

Use the module's existing recording-backend fixture. If there is none, the smallest honest version is a `VaultBackend` impl in the test module counting `get_item` calls.

- [ ] **Step 2: Run it and watch it fail**

- [ ] **Step 3: Write the implementation**

Delete the `get_by_id(...).map(Ok).unwrap_or_else(...)` preference at `app.rs:438-445` and call `cache.bridge().get_item(item_id)` directly. **Keep the comment's history**: it records that cloning the whole vault cost 5.66 MB and 46,494 allocations, which is why `get_by_id` replaced `items()` — that lesson stands and the new shape is its conclusion, not its reversal.

- [ ] **Step 4: Run the full suite**

Watch for the latency claim: a fill now always makes a backend call. On direct REST that is an HTTPS round trip; on `bw serve` it is a local one, and with `keep_backend_running` off it may start the backend. **The default for that setting is on**, so the common case is a warm local call — but say so in the commit rather than letting it be discovered.

- [ ] **Step 5: Commit**

---

### Task 4: The assertion for the gate that is not open yet

> **Also blocked, and now by two gates rather than one.** Task 3 above is the
> second: the snapshot cannot drop its passwords while a fill reads them, and a
> fill must read them while it has to work with the backend stopped.
>
> **Task 1's test is `#[ignore]`d, with both gates named in the attribute.** An
> earlier revision of this note said to leave it red, on the grounds that a red
> test is the more honest marker. That was right about honesty and wrong about
> consequences: a red test on `main` is a red CI on `main`, and a red CI is
> what stops the next release being verifiable. Ignored-with-a-reason is honest
> *and* lands — and because the reason names the two plans that unblock it, it
> reads as a finish line rather than as a test somebody switched off.

**Files:** Modify `deskwarden/src/vault_cache.rs`

Task 1's test still fails after Tasks 2 and 3: the daemon no longer *uses* cached passwords, but the snapshot still holds them, because the in-process sign-in vault window reads them.

- [ ] **Step 1: Mark Task 1's test ignored, with the reason in the attribute**

```rust
    #[test]
    #[ignore = "blocked on the sign-in path's vault window becoming a UI process; see \
                docs/superpowers/plans/2026-08-26-startup-window-in-its-own-process.md. \
                Tasks 2 and 3 removed every daemon path that READS a cached password; the \
                snapshot still carries them because that window reads the cache directly."]
```

**Ignored, not deleted, and not weakened to pass.** An ignored test with a reason is a finish line; a deleted one is a forgotten intention, and a weakened one is worse than either.

- [ ] **Step 2: Run the full suite and confirm the ignore count went up by exactly one**

- [ ] **Step 3: Commit**

---

## Verification before this branch is finished

- [ ] The full suite, `--lib` and `--bin deskwarden`, with every failure accounted for by the module-has-no-mockito check.
- [ ] `cargo clippy --all-targets` clean.
- [ ] **A running build**: press `CTRL+ALT+B` on a matched app and confirm the fill still works, and that the account picker still lists candidates for an unmatched one. Tasks 2 and 3 change the data both paths run on, and no test in this crate reaches either.
- [ ] Say plainly in the branch summary that the daemon **still holds the passwords** and that Tasks 2–3 removed the paths that read them. The headline is not yet true, and claiming it would be the defect this project keeps finding.

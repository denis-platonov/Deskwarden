# Per-Item Read From the Encrypted Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the daemon open **one** vault item from the encrypted disk cache without decrypting the other 1,665, so it can stop holding every password in memory and still fill with the backend stopped.

**Architecture:** The cache file gains two sections under one content key: a secret-free *facts* section the daemon loads whole, and a *secrets* section of independently-sealed items it opens one at a time. One Hello unseal yields the content key; per-item opens cost no prompt and no I/O beyond a seek.

**Tech Stack:** Rust, the existing `aes-gcm`, `hello.rs` and DPAPI machinery in `vault_disk_cache`. No new dependencies.

## Why this is a format change and not a refactor

`vault_disk_cache` encrypts one `DiskSnapshot { items, folders }` as **a single AES-256-GCM ciphertext**, with the plaintext header as additional authenticated data. There is no way to open one item out of that: GCM authenticates the whole message, so decrypting anything means decrypting everything.

That is why three separate pieces of work are stalled behind this one:

- **The tray still holds 1,666 plaintext passwords** — `nothing_the_daemon_can_reach_hands_back_a_password` is `#[ignore]`d against exactly this.
- **A fill cannot fetch by id**, because `autofill_really_fills_from_a_restored_snapshot` requires filling with `bw serve` stopped, and the only local copy is the in-memory one.
- **The startup window cannot be a UI process**, because it would have nothing to read while the backend comes up.

## What is NOT changing, and must not

`vault_disk_cache`'s security properties are the reason this design is worth building on. All of them survive:

- a random 32-byte content key, **AES-256-GCM**;
- that key sealed under a **Windows Hello** signature, private half in the **TPM**;
- the whole file **DPAPI-wrapped**;
- the header plaintext *inside* the DPAPI envelope, so an expired or foreign-account file is deleted **without a Hello prompt**;
- deleted on lock, logout, re-auth, and after seven days.

**One Hello unseal per process, still.** The content key is obtained once; every per-item open uses it. A design that prompted per item would be unusable, and a design that prompted twice on one launch has failed — `2026-08-27-the-vault-lives-in-a-place-not-a-process.md` says so.

## Global Constraints

- **No test may touch** the network, the real vault, the clipboard, the screen, `%APPDATA%\Deskwarden`, a real dialog, or spawn `bw`. Hello and DPAPI are already behind `DiskCacheEnv`'s `fn` pointers; use them.
- **No `cfg(test)` seams.** Banned crate-wide.
- **One target directory.** `CARGO_TARGET_DIR=/e/_dw_agent/run`. A second one cost 5.5 GB and filled the disk on 2026-08-27; `run/debug/incremental` reached 19 GB and is safe to delete when space runs short.
- **`--lib` does not run `main.rs`'s tests.** Use `--bin deskwarden` as well; `--lib` reports "0 filtered out" and runs none of them.
- **CI builds with `-D warnings` and this machine does not.** A `#[must_use]` discarded locally is an error there.
- **Commit with explicit paths and `-F` a message file.** Never `git add -A`, `--amend`, `reset`, `rebase`, or `git stash`.
- Branch: `per-item-cache-read`.

## The format

**Version 2.** One content key, two sections:

```
header (plaintext, inside the DPAPI envelope, as today)
  format_version: 2
  written_at, account_fingerprint, item_count      -- unchanged
  facts_len: u32                                   -- new
  index: [(id, offset, len)]                        -- new, see below

facts   : AES-256-GCM( ItemFacts[] + folders )      -- AAD = header
secrets : per item, AES-256-GCM( VaultItem )        -- AAD = header ‖ item id
```

Three decisions in that, each with a reason:

**The index is in the header, so it is plaintext.** It carries item **ids** and offsets — ids are GUIDs the server assigned and already appear in URLs, so this leaks nothing a header's `item_count` did not. Encrypting it would mean opening the facts section to find where an item is, which is the cost this design exists to avoid.

**The AAD binds each secret to its id.** `header ‖ id`, not just `header`. Without the id, two ciphertexts in the same file are interchangeable: an attacker who can write the file could swap the entry for `bank.example` with the one for a site they control, and the daemon would type the wrong password into the right box and never know. GCM authenticates *that this ciphertext was sealed under this key*, not *that it belongs here*.

**Facts and secrets are separate ciphertexts, not one with a skip.** A single message would have to be opened whole to authenticate, which is exactly the property being removed.

**Version 1 files are deleted, not migrated.** The cache is a rebuildable optimisation with a seven-day life, `RejectReason::UnknownVersion` already exists for precisely this, and a migration path is code that runs once and is then wrong forever.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `deskwarden/src/vault_disk_cache.rs` (modify) | The v2 format: header, index, two sections, `load_facts`, `open_item`. |
| `deskwarden/src/vault_cache.rs` (modify) | Hold facts rather than items when the disk cache is the source; ask it for one item on a fill. |
| `deskwarden/src/app.rs` (modify) | The fill asks the cache for one item instead of reading a whole one out of the snapshot. |

---

### Task 1: The v2 file, written and read whole

**Files:** Modify `deskwarden/src/vault_disk_cache.rs`

Format first, with the existing whole-file read still the only reader. Nothing behaves differently at the end of this task, which is the point: the format change and the behaviour change are separately reviewable.

- [ ] **Step 1: Write the failing tests**

```rust
    /// **A secret is bound to its id, not merely to the file.**
    ///
    /// Without the id in the AAD, two entries in one file are
    /// interchangeable: somebody who can write the file swaps the ciphertext
    /// for `bank.example` with one for a site they control, and the daemon
    /// types the wrong password into the right box. GCM proves *this key
    /// sealed this message*; only the AAD can prove *this message belongs to
    /// this item*.
    #[test]
    fn a_secret_moved_to_another_items_slot_will_not_open() {
        let key = [7u8; 32];
        let header = a_header(2);
        let sealed = seal_item(&key, &header, "item-a", &an_item("item-a", "hunter2"));
        assert!(
            open_item(&key, &header, "item-b", &sealed).is_err(),
            "a ciphertext sealed for item-a opened as item-b, so entries in this file are \
             interchangeable"
        );
        assert!(open_item(&key, &header, "item-a", &sealed).is_ok(), "control: it opens as itself");
    }

    /// The facts section carries no secret. This is the assertion the whole
    /// design exists for and it must not be a comment.
    #[test]
    fn the_facts_section_has_no_password_in_it() {
        let key = [7u8; 32];
        let header = a_header(2);
        let sealed = seal_facts(&key, &header, &[facts_of(&an_item("item-a", "hunter2"))], &[]);
        let raw = format!("{sealed:?}");
        assert!(!raw.contains("hunter2"), "the sealed bytes are the plaintext");
        let opened = open_facts(&key, &header, &sealed).expect("the facts");
        let rendered = format!("{opened:?}");
        assert!(
            !rendered.to_lowercase().contains("hunter2"),
            "a password reached the facts section, which the daemon loads whole"
        );
    }

    /// A v1 file is refused by the header, unread, with no Hello prompt --
    /// `RejectReason::UnknownVersion`'s existing contract. Deleted rather
    /// than migrated: this is a rebuildable seven-day cache, and a migration
    /// is code that runs once and is wrong forever after.
    #[test]
    fn a_version_one_file_is_rejected_without_being_opened() {
        let env = an_env_that_fails_if_hello_is_called();
        let cache = a_disk_cache(&env);
        write_raw_file_with_version(1);
        assert_eq!(cache.load("fingerprint"), DiskCacheLoad::Rejected(RejectReason::UnknownVersion));
    }
```

- [ ] **Step 2: Run them and watch them fail** (`cannot find function seal_item`)

- [ ] **Step 3: Implement the v2 format** — header fields, index, `seal_facts`/`open_facts`, `seal_item`/`open_item`. Keep `load()` reading everything, by opening the facts and then every secret, so existing callers are untouched.

- [ ] **Step 4: Run the full suite** — `--lib` and `--bin deskwarden`.

- [ ] **Step 5: Commit**

---

### Task 2: `load_facts` and `open_item`

**Files:** Modify `deskwarden/src/vault_disk_cache.rs`

**Interfaces produced:**
```rust
pub fn load_facts(&self, fingerprint: &str) -> DiskFactsLoad;   // facts + folders + written_at
pub fn open_item(&self, id: &str) -> Option<VaultItem>;          // one secret, by id
```

`open_item` needs the content key without a second Hello prompt, so the key obtained by `load_facts` is held for the process's lifetime — `Zeroizing`, and dropped by the same `clear()` that empties the snapshot on lock.

- [ ] **Step 1: Write the failing tests** — that `load_facts` never opens a secrets entry (assert through a counting `DiskCacheEnv`), that `open_item` returns the right item, that an unknown id is `None` rather than a panic, and that **`open_item` after `clear()` returns `None`** rather than reaching for a key that should be gone.

- [ ] **Step 2–5:** run red, implement, run the full suite, commit.

---

### Task 3: The cache holds facts

> **PARTLY DONE, and the rest is gated. Read before starting.**
> 
> What landed: `VaultCache::item_from_disk` reaches one secret out of the v2
> file, and `clear()` closes it so a lock takes the content key away. Those
> are the pieces that do not depend on the question below, and they are what
> Task 4 needs.
> 
> What did not: **the snapshot still holds `VaultItem`s.** Two in-process
> vault windows read them straight out of it --
> `main.rs:1371` (`run_from_working`, the hand-launch path) and
> `main.rs:6463` (`run_from_vault`, after sign-in). Both take the cache and
> expect whole items, so narrowing the snapshot breaks the window rather than
> the daemon.
> 
> This is the same gate as
> `2026-08-26-startup-window-in-its-own-process.md`, which has four
> documented walls -- the last of them found only by running the app. It is
> not a second problem and must not get a second solution: **either both
> windows become UI processes, or the window fetches items itself.** Whichever
> is chosen goes in `vault_cache`'s module doc, because an undocumented split
> here is how two vault shapes come to exist.

**Files:** Modify `deskwarden/src/vault_cache.rs`

The snapshot stops carrying `VaultItem`s when the disk cache is the source. `VaultCache::project` already exists and callers already take `ItemFacts`, so this is where those two meet.

**The open question this task must answer explicitly, not silently:** the vault window's in-process path (sign-in) reads items out of this cache. Either it moves to `open_item` per row it displays, or that path keeps a full snapshot and this task narrows to the daemon's own uses. **Decide it in the code review, and write the decision into the module doc** — an undocumented split here is how two vault shapes come to exist.

- [ ] Steps as above, ending with the full suite and a commit.

---

### Task 4: A fill opens one secret

**Files:** Modify `deskwarden/src/app.rs`

- [ ] **Step 1: The test that was blocked.** `autofill_really_fills_from_a_restored_snapshot` must still pass with `bw serve` absent — now filling through `open_item` rather than from a resident password. That test already exists and is the acceptance criterion; do not weaken it.

- [ ] **Step 2: Un-ignore `nothing_the_daemon_can_reach_hands_back_a_password`** and make it pass. That is this plan's finish line.

- [ ] **Steps 3–5:** implement, full suite, commit.

---

## Verification before this branch is finished

- [ ] Full suite, `--lib` **and** `--bin deskwarden`, every failure named and accounted for. The local suite has flaky loopback tests; re-run a failure in isolation before believing it, and CI is the arbiter.
- [ ] `cargo clippy --all-targets` clean, and `cargo deny check licenses advisories` clean.
- [ ] **A running build**, and this cannot be skipped — the last three attempts in this area passed every automated check and failed in the user's hands. Turn the disk cache on, restart, stop the backend, and fill. Then check the daemon's memory holds no vault: with `open_item` in place the tray should sit near its pristine figure rather than ~35 MB.
- [ ] **Say plainly** whether the vault window's in-process path still holds a full snapshot. If it does, the headline is "the daemon holds no passwords *on the autofill path*", and claiming more would be the defect this project keeps finding.

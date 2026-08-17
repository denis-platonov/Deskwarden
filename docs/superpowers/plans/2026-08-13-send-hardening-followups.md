# Send and Seam Hardening Follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Four small, independent corrections found during the 2026-08-13 review and flake work, each cheap and each closing a real hole rather than a hypothetical one.

**Architecture:** No new subsystems. Each task is a contained change to an existing file with a test that fails before it and passes after.

**Tech Stack:** Rust, egui 0.35, the `bw` CLI via `send.rs`.

## Global Constraints

- **Windows only.** Light theme only.
- **Zero warnings**, including a shipping `cargo build`.
- **Never build into `deskwarden/target`.** Fresh `CARGO_TARGET_DIR` outside the repo per run; confirm each prints its own `Compiling deskwarden`.
- **Commit with explicit paths** and `-F` a message file, never a PowerShell here-string. Never `git stash` (two pre-existing stash entries must survive), `git add -A`, `--amend`, `reset`, `rebase`.
- **No test may** touch the network, the real vault, `%APPDATA%\Deskwarden`, real dialogs, or spawn `bw`.
- Files here carry below-the-cut guards with a byte-offset close check and a derived `column_zero_module_openers` cross-check. Adding or removing a gated test module changes a derived count.
- **Tasks are independent.** Do them in any order, or singly.

---

## Task 1: Revoke should disable, not delete — **CANCELLED 2026-08-17. DO NOT DO THIS.**

**Decision: revoke keeps deleting.** The reasoning is recorded in
`docs/superpowers/specs/2026-08-13-send-record-import-decision.md` under "The
decisions": a disabled Send is still ciphertext on a server, and for a payload
carrying a TOTP seed "switched off but still there" is a worse resting state
than "gone". The renderability argument below is correct and was outweighed.

**What follows from that, and is still to be done elsewhere:** the Revoked row
is cut from `2026-08-13-send-a-record.md` Task 4 rather than reinterpreted, and
the pre-revoke confirmation must say *permanently removed*, not *paused*.

The original analysis is kept below because it is the record of why the
alternative was considered — **it is not an instruction.**

---

**Why:** `send.rs:542` runs `["send", "delete", id]`. A deleted Send is gone from `bw send list`, so a **Revoked** row can never be displayed — the state exists in the design and is unrenderable in the product. Bitwarden supports `disabled`, and we already write `"disabled":false` in the create JSON at `send.rs:492` without ever using it. Disabling kills the link just as hard, keeps the record so its dates and "Send again" remain visible, and is reversible after a mis-click.

**Files:**
- Modify: `deskwarden/src/send.rs` (the delete invocation at ~line 542; the doc at ~line 884)

**Interfaces:**
- Produces: `pub fn revoke_send(runner: &CliSendRunner, id: &str) -> Result<(), SendError>` — edits the Send to `disabled: true`.
- Keeps: the existing delete path, renamed to `delete_send_permanently`, for a separate "remove from Shared" action.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn revoking_disables_the_send_rather_than_deleting_it() {
    // Measured consequence, not a style preference: a deleted Send does not
    // come back from `bw send list`, so the Shared screen cannot show that it
    // was revoked -- the row simply vanishes, which reads as "never existed".
    let (args, _envs) = the_one_spawn(|| revoke_send(&runner, ID));
    assert!(
        !args.contains(&"delete".to_string()),
        "revoke must not delete: the record is what makes the Revoked state renderable"
    );
    assert!(args.contains(&"edit".to_string()));
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `CARGO_TARGET_DIR=<outside-repo> cargo test --lib send::`
Expected: FAIL — `cannot find function revoke_send`.

- [ ] **Step 3: Implement `revoke_send`** using `bw send edit` with `disabled: true`, JSON on **stdin, never argv**, spawned into the job like every other Send command. Keep `delete_send_permanently` for permanent removal.

- [ ] **Step 4: Run it and watch it pass.**

- [ ] **Step 5: Keep the session assertion.** The existing test proving `BW_SESSION` reaches the child exactly once must cover the new path too — a `bw send edit` that answers "locked" leaves the link live, which is the failure that matters.

- [ ] **Step 6:** Update `SEND_PUBLIC_SURFACE` and the needle counts in `vault_window/send_ui.rs::source_pins` deliberately, as `1fe2bae` did when it added `cli_send_create`.

- [ ] **Step 7: Commit.**

---

## Task 2: `accessUrl` must not be renderable into a log

**Why:** `CreatedSend` (`send.rs:557`) and `SendSummary` (`send.rs:566`) both `#[derive(Debug)]` and both carry `access_url`. That URL contains the **decryption key** after `#` — it is key material, not an identifier. Nothing logs them today (verified 2026-08-13), so this is not a live leak; it is one careless `log::debug!("{summary:?}")` away from writing a working key into a plaintext file, and the type gives no hint that would be bad. `SendPlan` already has a hand-written redacting `Debug` at ~line 153 for exactly this reason.

**Files:**
- Modify: `deskwarden/src/send.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_debug_render_never_carries_the_decryption_key() {
    let summary = SendSummary {
        id: "abc".into(),
        name: "SAP Production".into(),
        access_url: "https://send.bitwarden.com/#abcdefghijklmnop/somekeyhere".into(),
        deletion_date: "2026-08-17T14:20:00.000Z".into(),
        is_file: false,
    };
    let rendered = format!("{summary:?}");
    assert!(
        !rendered.contains("somekeyhere"),
        "the access URL carries the Send's decryption key after `#`; a Debug that prints it \
         turns any stray log line into a full disclosure of the Send's contents"
    );
    assert!(rendered.contains("SAP Production"), "control: the Debug still identifies the Send");
}
```

- [ ] **Step 2: Run it and watch it fail.** Expected: the derived `Debug` prints the whole URL.

- [ ] **Step 3: Hand-write `Debug` for both `CreatedSend` and `SendSummary`**, printing id and name and eliding the URL, in the style of `SendPlan`'s existing impl.

- [ ] **Step 4: Run it and watch it pass. Commit.**

---

## Task 3: `vault_window::run` bypasses the `spawn_load` seam

**Why:** Found while fixing the `frame_promptness` flake. `vault_window/mod.rs:1438` calls `spawn_vault_load` **directly** rather than through the `VaultFrameEnv::load` seam, so that path is not substitutable and not pinned. `VaultFrameEnv` exists precisely so every route to production is substitutable; a seam that production walks around is a seam that proves nothing. This is the same defect class the `seam!` / width-pin / whole-body-pin work was built to prevent, and it went unnoticed because nothing was watching *that* seam.

**Files:**
- Modify: `deskwarden/src/vault_window/mod.rs` (~line 1438)

- [ ] **Step 1: Write the failing test.** Stub `VaultFrameEnv::load` and assert that a successful auto-sync's forced reload goes through the stub. Before the fix it does not, because production calls `spawn_vault_load` directly.

- [ ] **Step 2: Run it and watch it fail.**

- [ ] **Step 3: Route the call through the seam.**

- [ ] **Step 4: Run it and watch it pass.**

- [ ] **Step 5: Audit for siblings.** Grep for other direct `spawn_*` calls in `run` that have a seam field, and record what you find — the value here is knowing whether this was the only one.

- [ ] **Step 6:** Re-check that the sync-**success** pill path is covered. The `frame_promptness` fix made its test exercise the failure path, leaving success-path pill rendering uncovered; that gap is recorded in that commit's message. If routing through the seam makes a success-path test feasible again, add it.

- [ ] **Step 7: Commit.**

---

## Task 4: Six local squashers in `vault_window/mod.rs`

**Why:** `c067ce1` fixed `spawn_export`'s pin to use `code_squashed` instead of a private `split_whitespace` squash, because the private one kept whole-line comments and so redded on a plain explanatory comment — with a message accusing the author of hoisting a blocking call. The agent deliberately left **six other local squashers** in the same file alone, since they sit in modules `code_squashed` is not in scope for. Whether they carry the same false-red is unmeasured.

**Files:**
- Modify: `deskwarden/src/vault_window/mod.rs`

- [ ] **Step 1: Locate all six** and record, for each, whether it drops comment-only lines.

- [ ] **Step 2: For each that does not, measure the false red.** Insert a whole-line comment inside the pinned function and run its test. A squasher that reds is a defect; one that does not is fine as-is.

- [ ] **Step 3:** For each measured red, either bring `code_squashed` into scope or give the local squasher the same comment-dropping behaviour. **Do not change any that did not red** — an unmeasured "consistency" edit to a pin is how pins get weakened.

- [ ] **Step 4: Verify both directions per changed pin:** a whole-line comment is now green, a **trailing** comment on a code line still reds (the documented, accepted boundary), and a genuine body change still reds.

- [ ] **Step 5: Commit.**

---

---

## Deferred: modifier-plus-letter chords (`^A`) — NOT scheduled

**Decided 2026-08-13: not doing this now.** Recorded so the reasoning survives.

`app_match.rs:116` describes the stored sequence as "in KeePass's notation", and
KeePass supports modifier-plus-**letter** chords: `^A` is Ctrl+A, `^C`, `^V`.
`^A{PASSWORD}` — replace whatever is prefilled, then type — is one of the
commonest real auto-type sequences, and it is the design's own headline example
in section 4a.

**We support the modifiers but not letters.** `key_sequence.rs:193-195` maps
`+`/`^`/`%` to Shift/Ctrl/Alt, so `^{TAB}` parses and compiles. `KEYS`
(`key_sequence.rs:100`) contains only *named* keys — Enter, Tab, Esc, arrows,
F-keys. There is no `A` in it, or any other letter.

**Why deferring is safe:** the failure is loud. A modifier with nothing bindable
after it becomes `Refusal::DanglingModifier` at plan time
(`injector/sequence.rs:706` and `:780`), so a pasted KeePass sequence containing
`^A` is refused with a named reason rather than silently typing the wrong keys.
There is no data-corruption risk in leaving it unsupported — only a gap.

**Why it is not a small job.** A letter's virtual-key code is **keyboard-layout
dependent**: `^A` on AZERTY is a different physical key than on QWERTY. Doing it
properly means `VkKeyScanEx` or scan codes, not a naive VK table — and a naive
table would silently send the wrong chord for international users, which is a
worse failure than the current refusal.

**Revisit when** a real target application needs it — a prefilling login dialog
is the likely trigger. At that point it is its own plan, with layout handling as
its first task rather than an afterthought.

---

## Notes for the implementer

- Tasks 1 and 2 are both small `send.rs` edits in the same area and can share a commit if you prefer, though separate is cleaner for review.
- Task 1 is **CANCELLED** (2026-08-17) and is no longer a prerequisite for anything. `2026-08-13-send-a-record.md` Task 4 keeps its Revoked arm, but reaches it only via a Send disabled elsewhere — see that plan's implementer notes.
- **Tasks 2, 3 and 4 are unaffected and remain the ready work here.** Task 2 (the redacting `Debug`) is the most valuable: `access_url` carries the decryption key after `#`, and nothing today stops a stray log line from writing a working key to a plaintext file.
- Task 3 touches `vault_window/mod.rs`, which is frequently held by other work. Coordinate before starting.

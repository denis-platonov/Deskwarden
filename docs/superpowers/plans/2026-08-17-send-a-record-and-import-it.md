# Sending a Record and Importing It Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user send a whole credential record — username, password, and
optionally the TOTP seed — through a Bitwarden Send, and let the recipient
import it into their own vault so their client computes live one-time codes.

**Architecture:** A versioned JSON payload travels inside an ordinary Bitwarden
Send. Export builds the payload from a vault item and hands it to the existing
`send.rs` CLI path. Import fetches the Send with `bw send receive`, validates
the payload strictly as untrusted data, and creates a vault item through the
bridge's existing HTTP write to `bw serve` (`POST /object/item`). The TOTP seed
— and only the seed — is additionally sealed under a passphrase that must
travel out-of-band, because a Send's own key is in the link.

**Tech Stack:** Rust, egui 0.35, `bw` CLI via `send.rs`, `bw serve` HTTP via
`vault_bridge.rs`, `aes-gcm` and `sha2` (both already dependencies), `zeroize`.

**Source spec:** `docs/superpowers/specs/2026-08-13-send-record-import-decision.md`
— read it first. Its two decisions (2026-08-17) are settled and must not be
relitigated: **the imported record goes into the recipient's own vault**, and
**revoke deletes**.

## Global Constraints

- **Windows only.** Light theme only.
- **Zero warnings**, including a shipping `cargo build`.
- **Never build into `deskwarden/target`.** Fresh `CARGO_TARGET_DIR` outside
  the repo per run; confirm each prints its own `Compiling deskwarden`.
- **Commit with explicit paths** and `-F` a message file, never a PowerShell
  here-string. Never `git stash` (two pre-existing stash entries must
  survive), `git add -A`, `--amend`, `reset`, `rebase`.
- **No test may** touch the network, the real vault, `%APPDATA%\Deskwarden`,
  real dialogs, or spawn `bw`.
- **Pair every claim.** Every new test must be shown failing when the property
  it guards is broken. A control that cannot fail is a defect, not a test —
  this crate has shipped vacuous controls at least twice.
- Below-the-cut guards with a byte-offset close check and a derived
  `column_zero_module_openers` cross-check apply crate-wide; adding a gated
  test module changes a derived count.
- **`Zeroizing` end to end** for the payload, the seed, the passphrase, and
  every intermediate buffer. `SendPlan.text` is already `Zeroizing<String>`.
- **Baseline at time of writing: 2336 lib / 217 bin / 6 ignored / 0 failed.**

## What is deliberately NOT in this plan

- **File Sends.** The spec preferred a `.json` file Send so a browser offers a
  download rather than rendering a seed on screen. `send.rs` hardcodes
  `"file":null` and has no file path at all; adding one is its own plan. This
  plan uses a **text Send with `hidden: true`**, which `SendPlan` already
  supports — the viewer masks the content until deliberately revealed. That is
  weaker than a download and is called out in Task 9's copy.
- **A Revoked row.** Revoke deletes, by decision. See the spec.
- **Honouring `not_after` by deleting the imported item later.** The decision
  accepted that a vault item does not expire. `not_after` is advisory and must
  be *presented* as advisory (Task 8).

---

## File Structure

| File | Responsibility |
|---|---|
| `deskwarden/src/record/mod.rs` (new) | Module root; re-exports. |
| `deskwarden/src/record/payload.rs` (new) | The `Record` type, its versioned JSON writer, and the strict reader. Pure; no I/O. |
| `deskwarden/src/record/seal.rs` (new) | Passphrase sealing of the seed field. Pure; wraps `aes-gcm`. |
| `deskwarden/src/record/import.rs` (new) | Turning a validated `Record` into a `VaultItem` to POST, plus the collision decision. Pure. |
| `deskwarden/src/send.rs` | One new invocation builder: `bw send receive`. |
| `deskwarden/src/vault_bridge.rs` | One new method: create an item via `POST /object/item`. |
| `deskwarden/src/vault_window/record_ui.rs` (new) | The "Send a record" and "Import from Send" surfaces. |
| `deskwarden/src/lib.rs` | Declare `record`. |

**Ship point:** Tasks 1–6 make export whole and shippable on their own. Tasks
7–10 add import. If the work is split across sessions, split there.

---

## Task 1: The payload type and its versioned writer

**Files:**
- Create: `deskwarden/src/record/mod.rs`, `deskwarden/src/record/payload.rs`
- Modify: `deskwarden/src/lib.rs`

**Interfaces:**
- Produces:
  ```rust
  pub const RECORD_FORMAT: &str = "deskwarden.record";
  pub const RECORD_VERSION: u32 = 1;

  pub struct Record {
      pub name: String,
      pub username: Option<String>,
      pub password: Option<Zeroizing<String>>,
      pub uri: Option<String>,
      pub notes: Option<String>,
      /// Sealed by `seal.rs`, never the bare seed. `None` when not sent.
      pub totp_sealed: Option<SealedSeed>,
      /// Advisory only. RFC 3339. See the spec.
      pub not_after: Option<String>,
  }

  pub fn write_json(record: &Record) -> Zeroizing<String>;
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn absence_is_meaningful_and_is_written_as_absence() {
    // The spec: "an unticked field is absent, not empty". An empty string
    // would import as a username of "", silently overwriting nothing with
    // nothing and rendering as a blank the user did not choose.
    let record = Record {
        name: "SAP Production".to_string(),
        username: Some("dplatonov".to_string()),
        password: None,
        uri: None,
        notes: None,
        totp_sealed: None,
        not_after: None,
    };
    let json = write_json(&record);
    assert!(json.contains("\"username\":\"dplatonov\""));
    assert!(
        !json.contains("\"password\""),
        "an unsent field must be ABSENT from the JSON, not present-and-empty: {json}"
    );
    // Control: the writer really does emit a password when there is one, so
    // the assertion above is about absence and not about a writer that never
    // writes passwords at all.
    let with_password = Record { password: Some(Zeroizing::new("hunter2".into())), ..record };
    assert!(write_json(&with_password).contains("\"password\":\"hunter2\""));
}

#[test]
fn the_envelope_names_the_format_and_the_version() {
    // A payload that does not say what it is cannot be refused politely by a
    // future reader; it can only fail to parse.
    let json = write_json(&Record {
        name: "x".to_string(),
        username: None, password: None, uri: None, notes: None,
        totp_sealed: None, not_after: None,
    });
    assert!(json.contains("\"format\":\"deskwarden.record\""));
    assert!(json.contains("\"version\":1"));
}
```

- [ ] **Step 2: Run it and watch it fail.** Expected: `cannot find type Record`.

- [ ] **Step 3: Implement `Record` and `write_json`.** Build the JSON with the
  same hand-rolled `push_json_string` approach `send.rs` already uses (see
  `send.rs:~472`) rather than adding a serde derive — the payload must be
  written into a `Zeroizing<String>` whose intermediate buffers are also
  zeroizing, and `serde_json::to_string` allocates one that is not.

- [ ] **Step 4: Run it and watch it pass.**

- [ ] **Step 5: Commit.**

---

## Task 2: The strict reader

**Files:**
- Modify: `deskwarden/src/record/payload.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum RecordRefusal {
      NotOurFormat,
      UnsupportedVersion(u32),
      UnknownField(String),
      MissingName,
      Malformed(&'static str),
      TooLarge,
  }
  pub fn read_json(text: &str) -> Result<Record, RecordRefusal>;
  ```

**Why strict:** the JSON is written by someone else and arrives over the
network. The spec is explicit — unknown fields are **rejected, not ignored**,
nothing is interpreted as an instruction, nothing is executed.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn an_unknown_field_is_refused_rather_than_ignored() {
    // Ignoring unknown keys is how a payload written for a LATER version
    // imports silently and wrongly here: a future `"totp_plain"` would be
    // dropped on the floor and the user would be told the import succeeded.
    let json = r#"{"format":"deskwarden.record","version":1,"name":"x","surprise":true}"#;
    assert_eq!(read_json(json), Err(RecordRefusal::UnknownField("surprise".to_string())));
}

#[test]
fn a_payload_from_somewhere_else_is_refused_by_name() {
    assert_eq!(read_json(r#"{"format":"something.else","version":1,"name":"x"}"#),
               Err(RecordRefusal::NotOurFormat));
    assert_eq!(read_json(r#"{"format":"deskwarden.record","version":99,"name":"x"}"#),
               Err(RecordRefusal::UnsupportedVersion(99)));
}

#[test]
fn a_record_survives_a_round_trip_with_every_field_set() {
    // The control that makes the refusal tests meaningful: a reader that
    // refused EVERYTHING would pass all of them.
    let original = Record {
        name: "SAP Production".to_string(),
        username: Some("dplatonov".to_string()),
        password: Some(Zeroizing::new("hunter2".to_string())),
        uri: Some("https://sap.example".to_string()),
        notes: Some("line one\nline two \"quoted\"".to_string()),
        totp_sealed: None,
        not_after: Some("2026-09-01T00:00:00Z".to_string()),
    };
    let back = read_json(&write_json(&original)).expect("round trip");
    assert_eq!(back.name, original.name);
    assert_eq!(back.username, original.username);
    assert_eq!(back.password.as_deref(), original.password.as_deref());
    assert_eq!(back.notes, original.notes);
    assert_eq!(back.not_after, original.not_after);
}

#[test]
fn an_oversized_payload_is_refused_before_it_is_parsed() {
    // A Send is fetched into memory. A multi-megabyte "record" is not a
    // record; refusing early keeps a hostile payload from being walked at all.
    let huge = format!(r#"{{"format":"deskwarden.record","version":1,"name":"{}"}}"#,
                       "x".repeat(200_000));
    assert_eq!(read_json(&huge), Err(RecordRefusal::TooLarge));
}
```

- [ ] **Step 2: Run it and watch it fail.**

- [ ] **Step 3: Implement `read_json`.** Reuse the crate's existing JSON
  reading approach in `send.rs` (`string_field` and friends) rather than
  introducing a second style. Enforce a size cap (`MAX_PAYLOAD_BYTES`, 64 KiB)
  **before** parsing. Reject unknown keys by enumerating the known ones and
  failing on anything else.

- [ ] **Step 4: Run it and watch it pass.**

- [ ] **Step 5: Prove the notes field is data.** Add a test that a `notes`
  value containing `{PASSWORD}` and `{TOTP}` round-trips as literal text and
  is **not** interpreted by `key_sequence`. This is the "no field is an
  instruction" rule made checkable.

- [ ] **Step 6: Commit.**

---

## Task 3: Sealing the seed under a passphrase

**Files:**
- Create: `deskwarden/src/record/seal.rs`

**Why:** a Send's content is protected by the fragment key, which is **in the
link**. Whoever has the link has the content. For a username and password that
is the bargain already accepted by sending them. A TOTP seed is unrotatable and
permanent, so "whoever has the link" is too weak. A passphrase layer makes the
link alone insufficient — **but only if the passphrase travels out of band.**

**Interfaces:**
- Produces:
  ```rust
  pub struct SealedSeed { pub salt: [u8; 16], pub nonce: [u8; 12], pub ciphertext: Vec<u8> }
  pub fn seal(seed: &str, passphrase: &str) -> SealedSeed;
  pub fn unseal(sealed: &SealedSeed, passphrase: &str) -> Result<Zeroizing<String>, SealFailed>;
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_sealed_seed_needs_the_passphrase_and_the_link_is_not_enough() {
    let sealed = seal("JBSWY3DPEHPK3PXP", "correct horse battery staple");
    assert!(
        !sealed.ciphertext.windows(4).any(|w| w == b"JBSW"),
        "the seed is recognisable in the ciphertext, so it was not encrypted"
    );
    assert_eq!(
        unseal(&sealed, "correct horse battery staple").unwrap().as_str(),
        "JBSWY3DPEHPK3PXP"
    );
    assert!(matches!(unseal(&sealed, "wrong"), Err(SealFailed::WrongPassphrase)));
}

#[test]
fn two_seals_of_the_same_seed_differ() {
    // A fixed nonce or salt would make identical seeds produce identical
    // ciphertext, so an observer holding one link could recognise the same
    // seed in another.
    let a = seal("JBSWY3DPEHPK3PXP", "pw");
    let b = seal("JBSWY3DPEHPK3PXP", "pw");
    assert_ne!(a.ciphertext, b.ciphertext);
    assert_ne!(a.salt, b.salt);
}

#[test]
fn a_tampered_ciphertext_is_refused_rather_than_decrypted_to_garbage() {
    let mut sealed = seal("JBSWY3DPEHPK3PXP", "pw");
    sealed.ciphertext[0] ^= 0xff;
    assert!(matches!(unseal(&sealed, "pw"), Err(SealFailed::WrongPassphrase)));
}
```

- [ ] **Step 2: Run it and watch it fail.**

- [ ] **Step 3: Implement using `aes-gcm` (already a dependency).** Derive the
  key from the passphrase and a **random 16-byte salt**; use a random 12-byte
  nonce. Salt and nonce come from `getrandom`, already a dependency.
  **Do not hand-roll anything.** Version the struct's JSON representation so a
  future KDF change is a refusal rather than a misparse.

  **Note for the implementer:** the crate currently derives the Windows Hello
  key with a single SHA-256 (`hello.rs`). That is appropriate there — the
  input is a Hello signature with full entropy. It is **not** appropriate here,
  where the input is a human-chosen passphrase and the ciphertext may sit on a
  server for days. Use a deliberately slow KDF. If adding one means a new
  dependency, propose it in the commit message with the measured cost rather
  than quietly reusing SHA-256.

- [ ] **Step 4: Run it and watch it pass.**

- [ ] **Step 5: Prove the passphrase and seed are zeroized.** Follow the
  existing allocator-probe pattern in `login_ui.rs` (`PROBE_LOCK`). **Check the
  probe can fail** before trusting it — a test guarding zeroization has already
  shipped here that could not fail, because the probe zeroes every freed block.

- [ ] **Step 6: Commit.**

---

## Task 4: Building a record from a vault item

**Files:**
- Create: `deskwarden/src/record/mod.rs` additions (pure selection logic)

**Interfaces:**
- Produces:
  ```rust
  pub struct RecordSelection {
      pub username: bool, pub password: bool, pub uri: bool,
      pub notes: bool, pub totp: bool,
  }
  pub fn record_from(item: &VaultItem, sel: &RecordSelection, seed: Option<&str>,
                     passphrase: Option<&str>, not_after: Option<String>) -> Record;
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn an_unticked_field_never_reaches_the_record() {
    let item = item_with(("dplatonov", "hunter2", "https://sap.example"));
    let sel = RecordSelection { username: true, password: false, uri: false,
                                notes: false, totp: false };
    let record = record_from(&item, &sel, None, None, None);
    assert_eq!(record.username.as_deref(), Some("dplatonov"));
    assert!(record.password.is_none(), "an unticked password reached the record");
    assert!(record.uri.is_none());
    // Control: the item really does carry a password, so `is_none` above is
    // about the selection and not about an empty fixture.
    assert_eq!(item.login_password().as_deref(), Some("hunter2"));
}

#[test]
fn ticking_totp_without_a_passphrase_cannot_produce_a_bare_seed() {
    // The load-bearing one. A seed must never be written unsealed, and the
    // failure mode to prevent is "user ticked TOTP, no passphrase set, we
    // sent it anyway".
    let item = item_with(("dplatonov", "hunter2", "https://sap.example"));
    let sel = RecordSelection { totp: true, ..RecordSelection::default() };
    let record = record_from(&item, &sel, Some("JBSWY3DPEHPK3PXP"), None, None);
    assert!(record.totp_sealed.is_none(),
            "a seed was included with no passphrase to seal it under");
    let json = write_json(&record);
    assert!(!json.contains("JBSWY3DPEHPK3PXP"),
            "the bare seed reached the payload: {json}");
}
```

- [ ] **Step 2: Run it and watch it fail.**

- [ ] **Step 3: Implement `record_from`.** A seed is included **only** when
  `sel.totp` and a passphrase are both present; otherwise `totp_sealed` is
  `None`. Make this a property of the function's shape, not a branch a caller
  can skip — take the passphrase and seed together so "seed without passphrase"
  is expressible only as the `None` arm.

- [ ] **Step 4: Run it and watch it pass. Commit.**

---

## Task 5: Sending the record

**Files:**
- Modify: `deskwarden/src/vault_window/record_ui.rs` (new file, first half)

- [ ] **Step 1:** Wire `write_json` into a `SendPlan`: `text` is the payload,
  **`hidden: true`** (see "What is deliberately NOT in this plan"), `name` is
  the record's name.

- [ ] **Step 2: Write the test** that the created Send's `text` parses back as
  a `Record` and that `hidden` is true. Pair it: flipping `hidden` to false
  must red.

- [ ] **Step 3:** Reuse `cli_send_create` unchanged. **Do not add a second
  Send-creating path** — `spawn_send_create` was measured able to have its body
  emptied with every static guard still green, and it fell only to a
  behavioural test. One path, already guarded.

- [ ] **Step 4: Run and commit.**

---

## Task 6: The export surface

**Files:**
- Modify: `deskwarden/src/vault_window/record_ui.rs`

- [ ] **Step 1:** Field checkboxes, defaulting to **username and password
  ticked, TOTP unticked.** A seed is not a default.

- [ ] **Step 2:** When TOTP is ticked, a passphrase field appears and the
  send button is **disabled until it is non-empty**. Test the disabled state
  directly as a pure function of the draft, not by driving the UI.

- [ ] **Step 3: The warning copy, verbatim from the spec's reasoning.** When
  TOTP is ticked, show: *"Sending a seed is not sharing a code — it is cloning
  the second factor, permanently. Anyone who opens this can generate valid
  codes indefinitely. Revoking stops new recipients; it cannot retract what
  was already fetched."*

- [ ] **Step 4:** Pin that copy by content, the way this crate pins refusal
  messages across file boundaries — the sentence is the safety control, and a
  reworded one must be a deliberate edit.

- [ ] **Step 5: Commit.** **Export is now shippable.**

---

## Task 7: Fetching a Send — **DONE (`96ec238`)**

> **This task's original instruction was wrong and is corrected here**, because
> a later reader would otherwise follow it.
>
> It said the password must go on **stdin, never argv**. `bw send receive`
> has **no stdin route for a password at all** — verified against the CLI. It
> offers exactly three: the inline flag (secret in argv), `--passwordfile`
> (secret written to disk, outliving the run), and `--passwordenv`, which
> names an **environment variable**, so argv carries the variable's name and
> never its value. The third is what shipped — the same channel `BW_SESSION`
> already travels on. `create` can pipe because all its secrets ride in one
> JSON body; `receive` has no such body.
>
> Two things fell out of it that were not in the plan:
> - **`SendInvocation`'s `Debug` was leaking.** It printed `args` in full,
>   justified on the premise that arguments never carry a secret. That premise
>   died when a command took an access URL **positionally**, so `5eab0ff`'s
>   redaction work was incomplete. Now elided whole.
> - The file's ban on the string `--password` also refused `--passwordenv`,
>   since one is a prefix of the other. It was replaced with a **sharper**
>   rule — every occurrence must be part of `--passwordenv` — which still
>   refuses the inline flag and now also refuses `--passwordfile`.
>
> A receive carries **no session token**: fetching a Send is anonymous, so the
> vault key is not handed to a child with no use for it.

**Files:**
- Modify: `deskwarden/src/send.rs`

**Interfaces:**
- Produces: `pub fn receive_invocation(url: &str, password: Option<&str>) -> SendInvocation`

- [ ] **Step 1: Verify the CLI surface first.** This plan assumes
  `bw send receive <url>`. **Confirm against `bw send receive --help` before
  writing code** and record what you found — the module doc at `send.rs:18`
  documents `send create` because someone checked, and this deserves the same.

- [ ] **Step 2: Write the failing test** that the URL reaches argv and any
  password reaches **stdin, never argv** — the same rule `cli_send_create`
  already follows, for the same reason: argv is visible to other processes.

- [ ] **Step 3: Implement. Step 4: Run it and watch it pass. Commit.**

---

## Task 8: Turning a validated record into a vault item

**Files:**
- Create: `deskwarden/src/record/import.rs`
- Modify: `deskwarden/src/vault_bridge.rs`

**Why HTTP and not the CLI:** the bridge already writes to `bw serve` — see
`vault_bridge.rs:232` (`POST /object/item`) and the `PUT` edit path that
`with_app_match` feeds. Sends use the CLI; the vault does not. **Follow the
bridge.**

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_seed_lands_in_the_items_own_totp_field() {
    // So the VAULT computes the code. Deskwarden must not become a TOTP
    // implementation, and a seed in a note is a seed no client can use.
    let record = record_with_sealed_seed("JBSWY3DPEHPK3PXP", "pw");
    let item = item_from(&record, Some("pw")).unwrap();
    assert_eq!(item.login_totp().as_deref(), Some("JBSWY3DPEHPK3PXP"));
    assert!(!item.notes.as_deref().unwrap_or_default().contains("JBSWY3DPEHPK3PXP"),
            "the seed was also written somewhere it cannot be used from");
}

#[test]
fn a_record_whose_seal_will_not_open_produces_no_item_at_all() {
    // Not a partial item with the seed missing: the user asked to import a
    // record, and half of one imported silently is worse than a refusal.
    let record = record_with_sealed_seed("JBSWY3DPEHPK3PXP", "pw");
    assert!(matches!(item_from(&record, Some("wrong")), Err(ImportRefusal::WrongPassphrase)));
}
```

- [ ] **Step 2: Run it and watch it fail.**

- [ ] **Step 3: Implement `item_from`**, and a `create_item` on the bridge
  using `POST /object/item`, shaped the way `with_app_match` shapes an edit.

- [ ] **Step 4:** `not_after` in the past → import still proceeds but the UI
  says the record is stale (see Task 10). **It is staleness information, not
  enforcement** — the decision accepted this. Test that a past `not_after`
  does **not** block the import, so nobody later "fixes" it into a hard gate
  that the spec says it cannot be.

- [ ] **Step 5: Run and commit.**

---

## Task 9: The collision policy

**Files:**
- Modify: `deskwarden/src/record/import.rs`

**Why this task exists:** the vault decision made it mandatory. Import creates
a **real item**; importing the same record twice must not silently produce two.

**Interfaces:**
- Produces:
  ```rust
  pub enum Collision { Fresh, SameName { existing_id: String } }
  pub fn collides(record: &Record, existing: &[VaultItem]) -> Collision;
  ```

- [ ] **Step 1: Write the failing test** that a record whose name matches an
  existing item reports `SameName` with that item's id, and that a differing
  name reports `Fresh`. Include a control that the fixture vault is non-empty,
  so `Fresh` is not the answer to an empty list.

- [ ] **Step 2: Run it and watch it fail.**

- [ ] **Step 3: Implement.** Match on **name only**. Do not match on username
  or URI: two genuinely different accounts on one service share both, and a
  false collision that silently overwrites a credential is far worse than a
  duplicate item.

- [ ] **Step 4:** The UI **asks**; it does not decide. On `SameName`, offer
  "Create a second item" and "Replace the existing one" with no default
  preselected. **Never overwrite without asking** — this is the only step in
  the feature that can destroy data the user already had.

- [ ] **Step 5: Commit.**

---

## Task 10: The import surface

**Files:**
- Modify: `deskwarden/src/vault_window/record_ui.rs`

- [ ] **Step 1: "Import from Send" takes the link**, not a pasted blob. The
  spec is explicit: the clipboard is exactly the leak the fill path's password
  step already refuses to touch.

- [ ] **Step 2: Show what will be imported before creating anything** — the
  field names present in the payload, never their values.

- [ ] **Step 3:** A passphrase prompt appears only when the payload carries a
  sealed seed.

- [ ] **Step 4:** When `not_after` has passed, show *"This record was marked
  stale on <date>. It will still import."* — advisory, per the decision. Pin
  this sentence too: copy that implies the record expires on its own is wrong
  and must not ship.

- [ ] **Step 5:** Every `RecordRefusal` renders as a sentence naming the
  reason. A refusal that renders as a generic failure teaches the user to
  retry, which is the opposite of what a rejected payload should teach.

- [ ] **Step 6:** Update `foreground.rs`'s module classification if this
  surface opens a window — the crate requires **every** module to be
  classified, and this has caught out three agents.

- [ ] **Step 7: Commit.**

---

## Notes for the implementer

- **The payload is data, always.** No field is a command, a path, a URL to
  fetch, or a sequence to run. `notes` is text to store. Nothing auto-opens.
- **`SendSummary` and `CreatedSend` have hand-written redacting `Debug`s**
  (`5eab0ff`), as does `RawOutput`. Any new type holding a Send URL or `bw`
  output needs the same — that commit found a third carrier the plan missed.
- The `hidden: true` text Send is a **compromise**, not the spec's preference.
  If file Sends land later, revisit Task 5 first.
- `MAX_PAYLOAD_BYTES` is a refusal, not a truncation. Never import a prefix.

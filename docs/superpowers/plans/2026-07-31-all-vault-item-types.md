# All Vault Item Types Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the vault window full create, read, update and delete for every Bitwarden item type — secure notes, cards, identities and SSH keys — instead of treating every item as a login.

**Architecture:** Four sibling optional fields on `VaultItem` (`card`, `identity`, `sshKey`, `notes`), each modelled struct carrying its own `#[serde(flatten)] other` catch-all, exactly as `LoginData` and `UriEntry` already do. One `ItemKind` enum, derived from `item_type` in a single place and matched exhaustively, drives the read pane, the edit form and the create payload. Autofill is untouched.

**Tech Stack:** Rust, `serde`/`serde_json`, `zeroize`, `eframe`/`egui` 0.35, `mockito` for bridge tests.

**Spec:** `docs/superpowers/specs/2026-07-31-all-vault-item-types-design.md` (commit 0b9da7d). Every design decision there is settled. **Do not re-litigate them** — in particular, autofill stays login-only, item type is fixed at creation, and a generic renderer and a tagged `ItemData` union were both considered and rejected with reasons.

---

## Global Constraints

- **Build and test with `-j 2` only.** Higher parallelism hits a page-file limit on this machine.
  - `cargo test --manifest-path deskwarden/Cargo.toml -j 2`
  - `cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2`
- **Zero warnings.** The tree is warning-free and must stay that way.
- **Every write is a full-state PUT.** `bw serve`'s edit endpoint replaces the item; it does not merge. So a field this app does not model must survive untouched, and a modelled-but-empty field must not become `null` or `""` where the server had nothing. This repository has shipped that bug **twice** (`LoginData`/`VaultItem`, then `UriEntry`); the round-trip tests in `vault_bridge.rs` are what caught the second one.
- **`#[serde(flatten)]` only captures unknown keys at the level of the struct it is declared on.** Every new nested struct therefore needs its own `other`. `VaultItem`'s cannot reach inside them.
- **Every new `Option<T>` field on `VaultItem` needs `#[serde(default, skip_serializing_if = "Option::is_none")]`.** Without it an item gains `"card": null` on write, which the server reads as "the card is gone".
- **Compare `serde_json::Value`, never JSON strings, in round-trip assertions.** Key order is not part of the contract and `flatten` does not preserve it.
- **Secrets are `Zeroizing<String>`:** `card.number`, `card.code`, `ssh_key.private_key`, and item-level `notes`. Same treatment `LoginData.password` and `login.totp` already have. `Zeroizing<String>` (de)serializes exactly like `String` — the `zeroize` crate's `serde` feature is already enabled.
- **Do not widen the zeroize guarantee.** The documented escape routes (egui's galley cache, the clipboard, serde's buffers, `passwordHistory` riding the flattened `other` map) are a recorded decision, stated honestly in `deskwarden/README.md`. See the `>>>` block in `.superpowers/sdd/progress.md`. Wrapping more fields is out of scope.
- **Autofill is not touched.** No change to `app.rs`, `match_engine.rs`, `window_watch.rs`, the injectors, or `credentials_for`.
- **Extract decisions into pure functions and unit-test them directly.** Logic inside an eframe closure cannot be tested. Every one of the sixteen defects the independent reviews found on this codebase lived in an untested seam.
- **Record each task and each review in `.superpowers/sdd/progress.md`** (gitignored by repo convention — a local working file), including what you deliberately did *not* do and why.
- Line numbers below are as of commit `e83ef03` plus the review-16 fix; locate by function name, not by line.

---

## File Structure

| File | Responsibility |
|---|---|
| `deskwarden/src/vault_bridge.rs` | `CardData`, `IdentityData`, `SshKeyData`, `VaultItem::notes`, `ItemKind`, and the per-kind create payloads. |
| `deskwarden/src/vault_window/detail.rs` | Read pane: kind-aware header and chrome, per-kind body cards, notes card, unsupported pane. |
| `deskwarden/src/vault_window/detail_edit.rs` | `EditDraft` becomes kind-aware; the create form gains a type selector. |
| `deskwarden/src/vault_window/mod.rs` | Threads the kind through where the two panes are called; the Ctrl+N path. |
| `deskwarden/src/picker_ui.rs` | `load_items_for_picker` filters to logins. |
| `deskwarden/src/vault_cache.rs` | `create_item` signature follows `vault_bridge`'s. |

---

## Task 1: Verify the wire shapes (BLOCKING — no struct is written before this)

**This task produces no code.** It produces facts, recorded in the ledger, that every later task depends on.

**Why it is a task and not an assumption:** a wrong field name here does not fail loudly. `serde` deserializes it to `None`, the real key rides the `other` catch-all, and the item is written back looking correct — until the modelled copy and the `other` copy disagree and one silently wins. `sidebar.rs::scope_contains` leaves Trash unimplemented for exactly this reason: "no confirmed knowledge of `bw serve`'s trash/deletedDate JSON shape, so rather than guess (and risk silently misclassifying real data)". Follow that norm.

**Files:**
- Modify: `.superpowers/sdd/progress.md` (record the captured shapes)

- [ ] **Step 1: Ask the user to capture one item of each type**

This needs an unlocked vault, so it needs the master password, which an agent must not handle. Give the user this command to run themselves, and wait for the output:

```bash
bw list items --pretty | jq '[.[] | {t: .type} + (.type as $t | if $t == 2 then {secureNote} elif $t == 3 then {card} elif $t == 4 then {identity} elif $t == 5 then {sshKey} else {} end)] | group_by(.t) | map(.[0])'
```

If `jq` is unavailable, `bw list items > items.json` and inspect it directly. What is needed is the **exact key names** of the `card`, `identity`, `sshKey` and `secureNote` objects, plus confirmation of where a secure note's body lives (expected: the item-level `notes` field).

If the vault has no item of some type, create a throwaway one in the web vault, capture it, then delete it.

- [ ] **Step 2: Record the captured shapes in the ledger**

Write the real key names into `.superpowers/sdd/progress.md` under a heading for this plan. The expectations to confirm or correct:

- `card` — `cardholderName`, `brand`, `number`, `expMonth`, `expYear`, `code`
- `identity` — `title`, `firstName`, `middleName`, `lastName`, `address1`, `address2`, `address3`, `city`, `state`, `postalCode`, `country`, `company`, `email`, `phone`, `ssn`, `username`, `passportNumber`, `licenseNumber`
- `sshKey` — `privateKey`, `publicKey`, `keyFingerprint`
- `secureNote` — a type discriminator only; body in item-level `notes`
- Whether `expMonth`/`expYear` arrive as strings or numbers. **This one changes the struct**, so do not guess it.

- [ ] **Step 3: Note any divergence loudly**

If a captured name differs from the expectation above, say so explicitly in your report and correct every later task that names it. A silent correction is how the two copies start disagreeing.

---

## Task 2: The data model, `ItemKind`, and the picker filter

**Files:**
- Modify: `deskwarden/src/vault_bridge.rs`
- Modify: `deskwarden/src/picker_ui.rs`
- Test: inline `#[cfg(test)]` modules in both (this crate tests inline everywhere)

**Interfaces:**
- Produces, for every later task:
  - `pub struct CardData { cardholder_name, brand, number, exp_month, exp_year, code, other }`
  - `pub struct IdentityData { ..18 fields.., other }`
  - `pub struct SshKeyData { private_key, public_key, key_fingerprint, other }`
  - `VaultItem::notes: Option<Zeroizing<String>>`, `VaultItem::card`, `::identity`, `::ssh_key`
  - `pub enum ItemKind { Login, SecureNote, Card, Identity, SshKey, Unknown(i64) }`
  - `pub fn ItemKind::of(item: &VaultItem) -> ItemKind`
  - `pub fn ItemKind::label(self) -> String`

- [ ] **Step 1: Write the failing tests**

Append to `vault_bridge.rs`'s test module. Use the field names **confirmed in Task 1**; the ones below are the expectation, not the authority.

```rust
    #[test]
    fn item_kind_covers_every_type_number() {
        let kind = |t: Option<i64>| {
            let mut item = a_bare_item();
            item.item_type = t;
            ItemKind::of(&item)
        };
        assert_eq!(kind(Some(1)), ItemKind::Login);
        assert_eq!(kind(Some(2)), ItemKind::SecureNote);
        assert_eq!(kind(Some(3)), ItemKind::Card);
        assert_eq!(kind(Some(4)), ItemKind::Identity);
        assert_eq!(kind(Some(5)), ItemKind::SshKey);
        // A type Bitwarden has not shipped yet must be representable as
        // itself, not collapsed into a login -- otherwise a future item
        // renders a login-shaped pane over data that is not a login.
        assert_eq!(kind(Some(6)), ItemKind::Unknown(6));
        // An absent type preserves today's behaviour.
        assert_eq!(kind(None), ItemKind::Login);
    }

    #[test]
    fn a_card_round_trips_with_absent_fields_still_absent() {
        // The property that has broken twice in this file already: a key the
        // server never sent must not appear on write.
        let raw = r#"{"id":"1","name":"Visa","type":3,"fields":[],
            "card":{"number":"4111111111111111","brand":"Visa"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert_eq!(item.card.as_ref().unwrap().brand.as_deref(), Some("Visa"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        let after = serde_json::to_value(&item).unwrap();
        assert_eq!(before, after, "a card round trip changed the item's shape");
    }

    #[test]
    fn a_card_round_trips_with_empty_strings_still_empty() {
        // Empty is not absent. Collapsing the two is the mirror of the bug
        // above and just as silent.
        let raw = r#"{"id":"1","name":"Visa","type":3,"fields":[],
            "card":{"number":"","brand":"","code":""}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        let after = serde_json::to_value(&item).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn unknown_keys_inside_a_card_survive_a_round_trip() {
        // VaultItem's own flatten cannot reach inside a nested object --
        // this is why UriEntry exists. Same rule, three more structs.
        let raw = r#"{"id":"1","name":"Visa","type":3,"fields":[],
            "card":{"number":"4111","somethingNew":{"deep":true}}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        let after = serde_json::to_value(&item).unwrap();
        assert_eq!(before, after, "an unmodelled key inside `card` was dropped");
    }

    #[test]
    fn an_identity_round_trips_including_unmodelled_keys() {
        let raw = r#"{"id":"1","name":"Me","type":4,"fields":[],
            "identity":{"firstName":"A","lastName":"B","futureField":7}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert_eq!(item.identity.as_ref().unwrap().first_name.as_deref(), Some("A"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn an_ssh_key_round_trips_including_unmodelled_keys() {
        let raw = r#"{"id":"1","name":"deploy","type":5,"fields":[],
            "sshKey":{"privateKey":"PRIV","publicKey":"PUB","keyFingerprint":"FP","x":1}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let ssh = item.ssh_key.as_ref().unwrap();
        assert_eq!(ssh.private_key.as_deref().map(|k| k.as_str()), Some("PRIV"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn a_secure_note_round_trips_with_its_body_in_item_level_notes() {
        let raw = r#"{"id":"1","name":"Wifi","type":2,"fields":[],
            "notes":"the passphrase","secureNote":{"type":0}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        assert_eq!(item.notes.as_deref().map(|n| n.as_str()), Some("the passphrase"));
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn notes_on_a_login_are_now_modelled_and_still_round_trip() {
        // Regression guard for the existing type: `notes` used to ride the
        // `other` catch-all, so moving it into a typed field must not change
        // any login's wire shape.
        let raw = r#"{"id":"1","name":"Site","type":1,"fields":[],
            "notes":"a note","login":{"username":"u"}}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let before: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(before, serde_json::to_value(&item).unwrap());
    }

    #[test]
    fn an_item_with_no_notes_does_not_gain_a_null_notes_key() {
        let raw = r#"{"id":"1","name":"Site","type":1,"fields":[]}"#;
        let item: VaultItem = serde_json::from_str(raw).unwrap();
        let after = serde_json::to_value(&item).unwrap();
        assert!(after.get("notes").is_none(), "an absent notes key became null");
    }
```

In `picker_ui.rs`'s test module:

```rust
    #[test]
    fn the_picker_lists_only_logins() {
        // Attaching an app match to a secure note is meaningless: the fill
        // would type two empty strings into the matched application.
        let items = vec![
            item_of_type("Site", Some(1)),
            item_of_type("Wifi", Some(2)),
            item_of_type("Visa", Some(3)),
            item_of_type("Legacy", None),
        ];
        let listed: Vec<String> = logins_only(items).into_iter().map(|i| i.name).collect();
        assert_eq!(listed, vec!["Site".to_string(), "Legacy".to_string()]);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 vault_bridge picker_ui`
Expected: compilation errors — `ItemKind` not found, no field `card` on `VaultItem`, `logins_only` not found.

- [ ] **Step 3: Add the structs**

In `vault_bridge.rs`, beside `LoginData`:

```rust
/// A payment card (`type: 3`).
///
/// Its own `#[serde(flatten)] other` for the same reason `UriEntry` has one:
/// `VaultItem`'s catch-all cannot reach inside a nested object, so without
/// this any key Bitwarden adds here would be silently dropped on the next
/// full-state PUT.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CardData {
    #[serde(rename = "cardholderName", default, skip_serializing_if = "Option::is_none")]
    pub cardholder_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    /// `Zeroizing` for the same reason `LoginData::password` is: a card
    /// number is a long-lived secret, and `items()` hands out clones, so
    /// wiping only one copy would be a false sense of security.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<Zeroizing<String>>,
    #[serde(rename = "expMonth", default, skip_serializing_if = "Option::is_none")]
    pub exp_month: Option<String>,
    #[serde(rename = "expYear", default, skip_serializing_if = "Option::is_none")]
    pub exp_year: Option<String>,
    /// The security code (CVV/CVC). `Zeroizing`, as above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Zeroizing<String>>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// An identity (`type: 4`). Eighteen fields, of which a real item populates
/// a handful -- see `detail.rs`'s empty-field suppression.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IdentityData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "firstName", default, skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    #[serde(rename = "middleName", default, skip_serializing_if = "Option::is_none")]
    pub middle_name: Option<String>,
    #[serde(rename = "lastName", default, skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(rename = "postalCode", default, skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(rename = "passportNumber", default, skip_serializing_if = "Option::is_none")]
    pub passport_number: Option<String>,
    #[serde(rename = "licenseNumber", default, skip_serializing_if = "Option::is_none")]
    pub license_number: Option<String>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// An SSH key (`type: 5`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SshKeyData {
    /// `Zeroizing`: this is the secret the whole item exists to hold.
    #[serde(rename = "privateKey", default, skip_serializing_if = "Option::is_none")]
    pub private_key: Option<Zeroizing<String>>,
    #[serde(rename = "publicKey", default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(rename = "keyFingerprint", default, skip_serializing_if = "Option::is_none")]
    pub key_fingerprint: Option<String>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}
```

Add to `VaultItem`, beside `login`:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<CardData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<IdentityData>,
    #[serde(rename = "sshKey", default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<SshKeyData>,
    /// Item-level free text. A secure note's entire body lives here, which
    /// is why that type needs no struct of its own -- and why notes on an
    /// ordinary login were invisible until this field existed.
    ///
    /// `Zeroizing` because a secure note *is* the secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Zeroizing<String>>,
```

**Every existing `VaultItem { .. }` literal in the crate now fails to compile.** There are several in tests and a few in production (`app.rs`, `picker_ui.rs`, `vault_cache.rs`, `vault_bridge.rs`, `item_list.rs`, `sidebar.rs`). Fix them by adding the four fields as `None` — do **not** add `..Default::default()` to production literals, because that hides the next field addition from exactly the review that should catch it.

- [ ] **Step 4: Add `ItemKind`**

```rust
/// What kind of thing an item is, derived from `bw`'s numeric `type`.
///
/// `Unknown` is not defensive padding: Bitwarden can ship a type 6, and an
/// unrecognised item must render as unsupported rather than fall through to
/// a login-shaped pane over data that is not a login. Collapsing a distinct
/// situation into a representation that means something else is the failure
/// mode behind most of the findings recorded in this repo's progress ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Login,
    SecureNote,
    Card,
    Identity,
    SshKey,
    Unknown(i64),
}

impl ItemKind {
    /// The one place a type number becomes a kind. An absent `type`
    /// preserves today's behaviour, in which every item was a login.
    pub fn of(item: &VaultItem) -> Self {
        match item.item_type {
            None | Some(1) => ItemKind::Login,
            Some(2) => ItemKind::SecureNote,
            Some(3) => ItemKind::Card,
            Some(4) => ItemKind::Identity,
            Some(5) => ItemKind::SshKey,
            Some(other) => ItemKind::Unknown(other),
        }
    }

    pub fn label(self) -> String {
        match self {
            ItemKind::Login => "Login".to_string(),
            ItemKind::SecureNote => "Secure note".to_string(),
            ItemKind::Card => "Card".to_string(),
            ItemKind::Identity => "Identity".to_string(),
            ItemKind::SshKey => "SSH key".to_string(),
            ItemKind::Unknown(_) => "Unsupported item".to_string(),
        }
    }
}
```

- [ ] **Step 5: Filter the picker to logins**

In `picker_ui.rs`, add the pure function and apply it where `load_items_for_picker` returns its list:

```rust
/// The picker offers only logins.
///
/// An app match on a secure note or a card is meaningless: `credentials_for`
/// would resolve an empty username and password, and the injector would type
/// two empty strings into the matched application. Filtering here rather
/// than at fill time means the user is never offered the choice.
fn logins_only(items: Vec<VaultItem>) -> Vec<VaultItem> {
    items
        .into_iter()
        .filter(|i| ItemKind::of(i) == ItemKind::Login)
        .collect()
}
```

**Check what this does to the empty-vault distinction.** `PickerItemsResult` distinguishes `EmptyVault` from `BackendUnreachable` (a review found the misdiagnosis and a redesign fixed it). A vault holding only secure notes now filters to zero logins — decide deliberately whether that is `EmptyVault` and make the message honest. "Your vault has no items" would be a new instance of the exact misdiagnosis that redesign removed. Prefer wording that says there are no *logins* to attach.

- [ ] **Step 6: Run the tests**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2`
Expected: all pass, including every pre-existing round-trip test **unchanged** — that is the hard constraint. If a pre-existing assertion had to be edited, stop: it means the wire shape moved, which is the bug this task exists to avoid.

- [ ] **Step 7: Commit**

```bash
cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2
git add deskwarden/src/vault_bridge.rs deskwarden/src/picker_ui.rs
git commit -m "feat: model cards, identities, SSH keys and item notes

Four sibling fields on VaultItem, each modelled struct carrying its own
serde(flatten) catch-all -- VaultItem's own cannot reach inside a nested
object, which is why UriEntry exists. Card number, security code, SSH
private key and notes are Zeroizing, matching password and totp.

ItemKind derives the kind in one place and carries an Unknown variant, so
a future Bitwarden type cannot render as a login.

The picker now lists only logins: an app match on a secure note would
have the injector type two empty strings into the matched application."
```

---

## Task 3: The read pane frame

The header, the chrome, the dispatch, the notes card, and the unsupported pane. The three data-bearing panes come in Task 4, so this task's deliverable is "a card no longer claims to be a login" — reviewable on its own.

**Files:**
- Modify: `deskwarden/src/vault_window/detail.rs`
- Modify: `deskwarden/src/vault_window/mod.rs` (only if the call site needs the kind threaded)
- Test: inline in `detail.rs`

**Interfaces:**
- Consumes: `ItemKind`.
- Produces: `fn draw_kind_body(ui, item, kind, ..) -> DetailAction` dispatch, and `pub fn notes_text(item: &VaultItem) -> Option<&str>`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn only_logins_offer_to_fill() {
        // A card has no username or password, so a Fill button on it would
        // type two empty strings into whatever window is focused.
        assert!(kind_offers_fill(ItemKind::Login));
        assert!(!kind_offers_fill(ItemKind::Card));
        assert!(!kind_offers_fill(ItemKind::Identity));
        assert!(!kind_offers_fill(ItemKind::SecureNote));
        assert!(!kind_offers_fill(ItemKind::SshKey));
        assert!(!kind_offers_fill(ItemKind::Unknown(6)));
    }

    #[test]
    fn every_kind_has_a_label_and_none_of_them_say_login() {
        for kind in [ItemKind::SecureNote, ItemKind::Card, ItemKind::Identity,
                     ItemKind::SshKey, ItemKind::Unknown(9)] {
            assert_ne!(kind.label(), "Login");
        }
    }

    #[test]
    fn notes_are_surfaced_only_when_there_is_something_to_show() {
        assert_eq!(notes_text(&item_with_notes(None)), None);
        // An empty or whitespace-only note is not content: rendering the
        // card for it produces an empty box under a heading.
        assert_eq!(notes_text(&item_with_notes(Some(""))), None);
        assert_eq!(notes_text(&item_with_notes(Some("   "))), None);
        assert_eq!(notes_text(&item_with_notes(Some("hi"))), Some("hi"));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 detail`
Expected: `cannot find function kind_offers_fill`, `cannot find function notes_text`.

- [ ] **Step 3: Implement**

```rust
/// Whether this kind can be filled into an application.
///
/// Login-only by explicit design decision (see the spec): the fill path
/// resolves exactly a username and a password, so every other kind would
/// type two empty strings. This gates both the "Fill in app" button and the
/// autofill-targets card.
fn kind_offers_fill(kind: ItemKind) -> bool {
    matches!(kind, ItemKind::Login)
}

/// The item's note body, or `None` when there is nothing worth a card.
fn notes_text(item: &VaultItem) -> Option<&str> {
    item.notes
        .as_deref()
        .map(|n| n.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
}
```

In `draw_detail_read`:

- Replace the hardcoded `"Login"` subtitle with `ItemKind::of(item).label()`.
- Wrap the **"Fill in app"** button and the **AUTOFILL TARGETS** card in `if kind_offers_fill(kind)`. Delete stays for every kind.
- Replace the unconditional **"LOGIN CREDENTIALS"** card with an exhaustive dispatch, no catch-all arm:

```rust
    match kind {
        ItemKind::Login => { /* today's credentials card, unchanged */ }
        ItemKind::Card => { /* Task 4 */ }
        ItemKind::Identity => { /* Task 4 */ }
        ItemKind::SshKey => { /* Task 4 */ }
        ItemKind::SecureNote => { /* body renders via the notes card below */ }
        ItemKind::Unknown(t) => unsupported_card(ui, t),
    }
```

- After the dispatch, for every kind:

```rust
    if let Some(notes) = notes_text(item) {
        card(ui, "NOTES", |ui| {
            ui.label(RichText::new(notes).size(13.0).color(theme::INK));
        });
    }
```

- `unsupported_card` states the fact and nothing more:

```rust
/// An item type this build does not know. States that plainly rather than
/// rendering fabricated fields -- the item's real data is intact in the
/// `other` catch-all and a newer build will show it.
fn unsupported_card(ui: &mut egui::Ui, item_type: i64) {
    card(ui, "UNSUPPORTED ITEM", |ui| {
        ui.label(
            RichText::new(format!(
                "Deskwarden does not know how to show item type {item_type} yet. \
                 Its contents are unchanged and safe -- open it in the Bitwarden \
                 web vault or app to view or edit it."
            ))
            .size(12.0)
            .color(theme::TEXT_FAINT),
        );
    });
}
```

For Task 4's three arms, leave a `card(ui, "…", |_| {})` placeholder that renders the heading and nothing else, so this task compiles and is independently reviewable. Do not leave `todo!()` — it panics.

- [ ] **Step 4: Run the tests, then commit**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2
git add deskwarden/src/vault_window/detail.rs deskwarden/src/vault_window/mod.rs
git commit -m "feat: the read pane stops claiming every item is a login

Kind-aware subtitle, an exhaustive per-kind dispatch with no catch-all,
and a notes card for every kind -- notes were invisible for all of them,
logins included. Fill in app and the autofill targets card now render only
for logins, so a card no longer offers to type two empty strings into the
focused window. An unknown type says so instead of rendering as a login."
```

---

## Task 4: The card, identity and SSH key panes

**Files:**
- Modify: `deskwarden/src/vault_window/detail.rs`
- Test: inline

**Interfaces:**
- Produces: `pub fn card_expiry_text(month: Option<&str>, year: Option<&str>) -> Option<String>`, `pub fn identity_groups(identity: &IdentityData) -> Vec<(&'static str, Vec<(&'static str, String)>)>`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn card_expiry_renders_whatever_half_is_present() {
        assert_eq!(card_expiry_text(Some("3"), Some("2028")), Some("03/2028".to_string()));
        assert_eq!(card_expiry_text(Some("11"), Some("2028")), Some("11/2028".to_string()));
        // Either half may be absent, and a half-formed "03/" reads as data
        // loss rather than as a partially-filled item.
        assert_eq!(card_expiry_text(Some("3"), None), Some("03".to_string()));
        assert_eq!(card_expiry_text(None, Some("2028")), Some("2028".to_string()));
        assert_eq!(card_expiry_text(None, None), None);
        assert_eq!(card_expiry_text(Some(""), Some("")), None);
        // Not a number: show it rather than swallowing it.
        assert_eq!(card_expiry_text(Some("xx"), Some("2028")), Some("xx/2028".to_string()));
    }

    #[test]
    fn identity_groups_hide_empty_fields_and_empty_groups() {
        // Eighteen fields, a handful populated. Without suppression the pane
        // is mostly blank labels.
        let mut identity = IdentityData::default();
        identity.first_name = Some("Ada".to_string());
        identity.last_name = Some("Lovelace".to_string());
        identity.email = Some("ada@example.com".to_string());

        let groups = identity_groups(&identity);
        let names: Vec<&str> = groups.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["Name", "Contact"], "an empty group was rendered");

        let name_fields: Vec<&str> = groups[0].1.iter().map(|(l, _)| *l).collect();
        assert_eq!(name_fields, vec!["First name", "Last name"]);
    }

    #[test]
    fn an_entirely_empty_identity_renders_no_groups() {
        assert!(identity_groups(&IdentityData::default()).is_empty());
    }

    #[test]
    fn whitespace_only_identity_fields_count_as_empty() {
        let mut identity = IdentityData::default();
        identity.company = Some("   ".to_string());
        assert!(identity_groups(&identity).is_empty());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --manifest-path deskwarden/Cargo.toml -j 2 detail`
Expected: `cannot find function card_expiry_text`, `cannot find function identity_groups`.

- [ ] **Step 3: Implement the pure functions**

```rust
/// `MM/YYYY`, with either half allowed to be missing.
///
/// `expMonth` arrives unpadded (`"3"`), so it is zero-padded when it parses
/// as a month and passed through untouched when it does not -- showing an
/// unexpected value beats silently dropping it.
fn card_expiry_text(month: Option<&str>, year: Option<&str>) -> Option<String> {
    let clean = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty());
    let month = clean(month).map(|m| match m.parse::<u8>() {
        Ok(n) if (1..=12).contains(&n) => format!("{n:02}"),
        _ => m.to_string(),
    });
    let year = clean(year).map(str::to_string);
    match (month, year) {
        (Some(m), Some(y)) => Some(format!("{m}/{y}")),
        (Some(m), None) => Some(m),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// The identity pane's rows, grouped, with empty fields and empty groups
/// removed. Pure so the suppression rule is tested directly rather than
/// inferred from a screenshot.
fn identity_groups(identity: &IdentityData) -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    let f = |label: &'static str, value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| (label, v.to_string()))
    };
    let groups: Vec<(&'static str, Vec<(&'static str, String)>)> = vec![
        (
            "Name",
            vec![
                f("Title", &identity.title),
                f("First name", &identity.first_name),
                f("Middle name", &identity.middle_name),
                f("Last name", &identity.last_name),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        (
            "Contact",
            vec![
                f("Email", &identity.email),
                f("Phone", &identity.phone),
                f("Username", &identity.username),
                f("Company", &identity.company),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        (
            "Address",
            vec![
                f("Address", &identity.address1),
                f("Address 2", &identity.address2),
                f("Address 3", &identity.address3),
                f("City", &identity.city),
                f("State", &identity.state),
                f("Postal code", &identity.postal_code),
                f("Country", &identity.country),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        (
            "Government IDs",
            vec![
                f("SSN", &identity.ssn),
                f("Passport number", &identity.passport_number),
                f("Licence number", &identity.license_number),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
    ];
    groups.into_iter().filter(|(_, rows)| !rows.is_empty()).collect()
}
```

- [ ] **Step 4: Render the three panes**

Fill in Task 3's placeholder arms, reusing the existing helpers rather than inventing widgets: `card(..)` for the container, `credential_row(..)` for label/value/copy, and the same masked-reveal treatment `password_row` uses for anything secret.

- **Card** — `CARD DETAILS`: Cardholder name, Brand, Number *(masked, reveal, copy)*, Expiry via `card_expiry_text`, Security code *(masked, reveal, copy)*. Skip empty rows, as the identity pane does.
- **Identity** — `IDENTITY`: iterate `identity_groups`, a small heading per group, `credential_row` per field.
- **SSH key** — `SSH KEY`: Public key *(copy)*, Fingerprint *(copy)*, Private key *(masked, reveal, copy)*.

Reveal state must live where the pane's other reveal state lives — the same place `reveal_password` does. A fresh `let mut revealed = false` inside the closure resets every frame and produces a toggle that does nothing; that exact bug has already been found and fixed once in `detail_edit.rs`, so do not reintroduce it.

- [ ] **Step 5: Run the tests, then commit**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
git add deskwarden/src/vault_window/detail.rs
git commit -m "feat: card, identity and SSH key read panes

Expiry formatting and identity grouping/suppression are pure functions
with tests -- an identity has eighteen fields and real ones populate a
handful, so without suppression the pane is mostly blank labels. Card
number, security code and SSH private key are masked with reveal and
copy, matching the password row."
```

---

## Task 5: Creating each kind

**Files:**
- Modify: `deskwarden/src/vault_bridge.rs` (create payloads)
- Modify: `deskwarden/src/vault_cache.rs` (signature follows)
- Modify: `deskwarden/src/vault_window/detail_edit.rs` (type selector)
- Test: inline in `vault_bridge.rs` and `detail_edit.rs`

**Interfaces:**
- Produces: `pub enum NewItem { Login {..}, SecureNote {..}, Card {..}, Identity {..}, SshKey {..} }` replacing `NewLoginItem`, and `VaultBridge::create_item(&self, new_item: &NewItem)`.

`NewItem` is an enum, not a struct with optional everything, because **a create payload must carry exactly one type object.** A struct with four optional sub-objects makes "a card that also posts an empty `login: {}`" representable, and that is precisely how an item ends up with two type objects.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn each_create_payload_carries_exactly_one_type_object() {
        for (new_item, expected_type, expected_key) in [
            (NewItem::login("n", "u", "p", None), 1, "login"),
            (NewItem::secure_note("n", "body", None), 2, "secureNote"),
            (NewItem::card("n", None), 3, "card"),
            (NewItem::identity("n", None), 4, "identity"),
            (NewItem::ssh_key("n", None), 5, "sshKey"),
        ] {
            let payload = new_item.to_payload();
            assert_eq!(payload["type"], expected_type);
            assert!(payload.get(expected_key).is_some(), "{expected_key} missing");
            for other in ["login", "secureNote", "card", "identity", "sshKey"] {
                if other != expected_key {
                    assert!(
                        payload.get(other).is_none(),
                        "a {expected_key} payload also carried {other}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_secure_note_posts_its_body_as_item_level_notes() {
        let payload = NewItem::secure_note("Wifi", "the passphrase", None).to_payload();
        assert_eq!(payload["notes"], "the passphrase");
    }

    #[test]
    fn a_blank_note_body_is_absent_rather_than_an_empty_string() {
        // Same convention the login payload already follows: blank means
        // absent, so a create and a subsequent edit do not disagree about
        // the item's shape.
        let payload = NewItem::secure_note("Wifi", "", None).to_payload();
        assert!(payload.get("notes").is_none());
    }
```

- [ ] **Step 2: Run to verify they fail; then implement**

Replace `NewLoginItem` with `NewItem`, keeping `create_item`'s existing blank-means-absent convention (already documented in that function) for every kind. Update `vault_cache::create_item` and its call sites.

The create form gains a **type selector inline in the form** — a row of five choices, defaulting to Login. Not a modal: this app has exactly one modal pattern and a five-item choice does not justify a second. The selector renders **only when creating**; editing shows the kind as static text, because type is fixed at creation.

- [ ] **Step 3: Run the tests, then commit**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
git add deskwarden/src/vault_bridge.rs deskwarden/src/vault_cache.rs deskwarden/src/vault_window/detail_edit.rs
git commit -m "feat: create any item kind

NewItem is an enum rather than a struct of optionals, so a payload
carrying two type objects -- a card that also posts an empty login -- is
not representable. Blank means absent for every kind, matching the
existing login payload convention. The type selector renders only when
creating; type is fixed thereafter."
```

---

## Task 6: Editing each kind

**Files:**
- Modify: `deskwarden/src/vault_window/detail_edit.rs`
- Test: inline

**There is a live bug to fix here, and it is the reason this task is not just "add more fields".** `EditDraft::apply_to` does `updated.login.unwrap_or_default()` unconditionally, so **saving any non-login item from the edit form gives it a `login` object it never had** — an item with two type objects, exactly the risk the spec names. This must be fixed as part of this task, and it needs its own regression test.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn editing_a_card_does_not_give_it_a_login_object() {
        // The live bug: apply_to unconditionally did
        // `updated.login.unwrap_or_default()`, so saving a card from the
        // edit form added an empty login to it.
        let card = a_card_item();
        assert!(card.login.is_none());
        let draft = EditDraft::from_item(&card);
        let saved = draft.apply_to(&card);
        assert!(saved.login.is_none(), "editing a card gave it a login object");
        assert!(saved.card.is_some(), "editing a card dropped its card object");
    }

    #[test]
    fn editing_each_kind_preserves_unmodelled_fields() {
        // The fixture carries real values in `other`, not empty ones -- a
        // previous review closed exactly this coverage gap for logins and
        // it must not reopen per kind.
        for raw in [CARD_WITH_EXTRAS, IDENTITY_WITH_EXTRAS, SSH_WITH_EXTRAS, NOTE_WITH_EXTRAS] {
            let item: VaultItem = serde_json::from_str(raw).unwrap();
            let draft = EditDraft::from_item(&item);
            let saved = draft.apply_to(&item);
            let before: serde_json::Value = serde_json::from_str(raw).unwrap();
            let after = serde_json::to_value(&saved).unwrap();
            assert_eq!(before, after, "an unchanged edit altered the item");
        }
    }
```

The second test is the strongest one in the plan: an edit that changes nothing must produce a byte-identical item, for every kind.

- [ ] **Step 2: Run to verify they fail; then implement**

`EditDraft` becomes kind-aware — one variant's worth of fields per kind, with `from_item` populating from the matching object and `apply_to` writing back **only** that kind's object, on a clone of the item so everything else survives. Keep the existing "clone then overwrite known fields" pattern; do not reimplement it per kind.

- [ ] **Step 3: Run the tests, then commit**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2
git add deskwarden/src/vault_window/detail_edit.rs
git commit -m "fix+feat: edit any item kind without corrupting it

EditDraft::apply_to unconditionally did login.unwrap_or_default(), so
saving a card, note, identity or SSH key from the edit form gave it an
empty login object it never had -- an item with two type objects. The
draft is now kind-aware and writes back only its own kind's object.

Pinned by a test asserting that an edit which changes nothing produces a
byte-identical item, for every kind, with real values in the unmodelled
catch-all rather than empty ones."
```

---

## Task 7: Final whole-branch review

**Not a formality.** Sixteen independent reviews of this repository have found sixteen real defects, several introduced *by* a fix. Dispatch a fresh subagent with no prior context.

- [ ] **Step 1: Verify**

```bash
cargo test --manifest-path deskwarden/Cargo.toml -j 2
cargo check --manifest-path deskwarden/Cargo.toml --all-targets -j 2
```

Both clean, zero warnings. Record the test count.

- [ ] **Step 2: Dispatch the review**

Give the reviewer the spec, this plan, the commit range, and this hunt list:

- **The recurring shape:** a change correct in isolation that does not reach the behaviour it claims. For every claim, find the real call site and trace what a user sees.
- **Can any edit or create produce an item with two type objects?** Enumerate every write path.
- **Does an unchanged edit of each kind produce a byte-identical item?** Verify the tests are non-vacuous and that the fixtures carry real values in `other`, not empty ones.
- **Did any pre-existing round-trip test have to change?** If so, the wire shape moved and something is wrong.
- **Is `ItemKind` matched exhaustively everywhere, with no catch-all** that would swallow `Unknown`?
- **Is autofill genuinely untouched** — `app.rs`, `match_engine.rs`, the injectors, `credentials_for`?
- **Does the picker's login filter interact correctly with `PickerItemsResult`'s `EmptyVault` vs `BackendUnreachable`** distinction, or does a notes-only vault now get misdiagnosed?
- **Are the new reveal toggles persistent**, or do they reset every frame like the bug already fixed once in `detail_edit.rs`?
- **Is any new secret field missing `Zeroizing`?**
- **Out of scope, do not flag:** the documented zeroize escape routes, `ureq`'s per-syscall read timeout, review-6's backend-op generation Minors.

- [ ] **Step 3: Triage, fix, record**

Fix Criticals and Importants; triage Minors explicitly. Append the verdict, every finding, every fix and every deferral to `.superpowers/sdd/progress.md`.

---

## Self-Review

**Spec coverage.** Data model → Task 2. `ItemKind` with `Unknown` → Task 2. Read pane (kind label, login-only fill/autofill chrome, per-kind bodies, notes card, unsupported pane, empty suppression, expiry) → Tasks 3 and 4. Create with type selector and one type object → Task 5. Edit with preservation → Task 6. Picker filter → Task 2. Wire-shape verification → Task 1. Testing requirements → distributed across Tasks 2, 4, 5, 6 with the three round-trip properties named per struct. Risks → Task 7's hunt list.

**One thing the spec did not name and this plan adds:** the live `apply_to` login-injection bug, found while reading the code to write Task 6. It is in scope because it is the same defect the spec's "an item carrying two type objects" risk describes, and because Task 6 cannot be implemented correctly without fixing it.

**Deliberate non-goal, so an implementer does not invent it:** the item list's subtitle stays the username. Showing a card's brand and last four digits there would be a nice touch and is not specified; adding it silently would put card data in a new place with no test and no decision behind it.

**Type consistency.** `ItemKind::of(&VaultItem) -> ItemKind` and `ItemKind::label(self) -> String` are used identically in Tasks 2, 3 and 4. `NewItem` replaces `NewLoginItem` in Task 5 and `vault_cache::create_item` follows in the same task, so no task references a type that does not yet exist. `notes_text`, `card_expiry_text` and `identity_groups` are defined in the task that first uses them.

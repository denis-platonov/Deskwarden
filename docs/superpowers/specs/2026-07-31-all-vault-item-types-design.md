# All vault item types — design

**Status:** approved, ready for planning
**Date:** 2026-07-31
**Follows:** the vault window plan (`docs/superpowers/plans/2026-07-29-vault-window.md`)

## Goal

Make Deskwarden's vault window handle every Bitwarden item type — secure
notes, cards, identities and SSH keys — with full create, read, update and
delete, instead of treating every item as a login.

## Current state, stated precisely

The sidebar already filters and counts all five types
(`sidebar.rs::scope_contains` matches `item_type` 1/2/3/4/5), and the item
list already shows them. Everything past that point assumes a login:

- `draw_detail_read` (`vault_window/detail.rs`) hardcodes the subtitle
  **"Login"**, a **"Fill in app"** primary button, a **"LOGIN CREDENTIALS"**
  card and an **"AUTOFILL TARGETS"** card. A credit card therefore renders as
  a login with empty rows and a Fill button that would type two empty strings
  into whatever application is focused.
- `VaultItem` models only `login`. There is no `card`, `identity` or `sshKey`
  field, so those objects ride the `#[serde(flatten)] other` catch-all —
  preserved on write, but invisible.
- **`notes` is not modelled either.** It is an item-level field, so a secure
  note renders as a name and nothing else, and notes attached to an ordinary
  login are invisible too.
- `detail_edit.rs` and `create_item` produce `type: 1` only.
- `load_items_for_picker` does not filter by type, so "Add app…" will attach
  an app-match to a secure note, which then fills two empty strings.

## Decisions

Each of these was settled during brainstorming. Record them so they are not
re-derived.

**Full CRUD for every type**, not read-only. The user chose this over the
smaller "view now, write later" option.

**Autofill stays login-only.** Cards and identities are not fillable, the
match engine keeps ignoring non-logins, and `credentials_for` is unchanged.
The fill hot path is the most heavily reviewed code in this repository and
this feature does not touch it. Filling a six-field card through an injector
that types exactly a username and a password is a separate design, cleanly
separable later.

**Secrets are treated exactly as passwords are.** Card number, card security
code and SSH private key are masked by default with a reveal toggle and a
copy button, and are `Zeroizing<String>` so every clone self-wipes — the same
treatment `LoginData.password` and `login.totp` already get. Item-level
`notes` is `Zeroizing<String>` too: a secure note *is* the secret.

This inherits the honest caveat already documented in
`deskwarden/README.md`: `Zeroizing` covers the value and its clones, not the
copies that egui's galley cache, the clipboard, and serde's serialization
buffers make. That caveat is unchanged by this feature and must not be
re-litigated here — see the `>>>` block in `.superpowers/sdd/progress.md`.

**Typed structs per type**, not a generic renderer and not a tagged union.
Considered and rejected:

- *A generic renderer* that draws whatever keys the JSON carries. Zero
  round-trip risk, one renderer for everything — but no control over field
  order, labels or masking (you cannot mask a security code you have not
  named), and the edit form degrades to something close to raw JSON.
- *A tagged `ItemData` enum* replacing the sibling `Option` fields, so a card
  structurally cannot carry login data. Cleaner in principle, but Bitwarden's
  wire format has sibling optional objects rather than a tagged union, so it
  needs hand-written `Serialize`/`Deserialize` on the exact seam that has
  already produced two silent data-loss bugs in this repository. Worst
  risk/reward of the three.

**`ItemKind` has an explicit `Unknown(i64)` variant.** Bitwarden can add a
type 6. An unrecognised item must render as an unsupported item, not fall
through to a login-shaped pane. Collapsing a distinct situation into a
representation that means something else is the failure mode behind most of
the fifteen review findings recorded in the progress ledger.

**Item type is fixed at creation.** The edit form displays the kind and will
not change it. Bitwarden has no notion of converting a card into a login, and
writing an item under a new type would orphan the old type's object inside
`other`.

## Architecture

### 1. Data model

`VaultItem` gains four sibling fields, mirroring how `login` already sits
there, each with `#[serde(default, skip_serializing_if = "Option::is_none")]`:

```rust
pub card: Option<CardData>,
pub identity: Option<IdentityData>,
#[serde(rename = "sshKey")]
pub ssh_key: Option<SshKeyData>,
pub notes: Option<Zeroizing<String>>,
```

`skip_serializing_if` is not optional here. `bw serve`'s edit endpoint is
state-replacing, so an item that gains `"card": null` on write has been told
its card is gone. This is the same reason `login` already carries it.

**Every new struct carries its own `#[serde(flatten)] other`.**
`#[serde(flatten)]` only captures unknown keys at the level of the struct it
is declared on, so `VaultItem`'s catch-all cannot reach inside a nested
object. `UriEntry` exists in this codebase precisely because that was
discovered the hard way, twice.

Expected field sets, **to be verified against the real vault before
implementation** (see §5):

- `CardData` — `cardholderName`, `brand`, `number` *(Zeroizing)*,
  `expMonth`, `expYear`, `code` *(Zeroizing)*.
- `IdentityData` — `title`, `firstName`, `middleName`, `lastName`,
  `address1`, `address2`, `address3`, `city`, `state`, `postalCode`,
  `country`, `company`, `email`, `phone`, `ssn`, `username`,
  `passportNumber`, `licenseNumber`.
- `SshKeyData` — `privateKey` *(Zeroizing)*, `publicKey`, `keyFingerprint`.
- Secure notes need **no struct**: the body is item-level `notes`, and
  Bitwarden's `secureNote` object carries only a type discriminator, which
  rides `other` untouched.

`ItemKind`, derived from `item_type` in exactly one place:

```rust
pub enum ItemKind { Login, SecureNote, Card, Identity, SshKey, Unknown(i64) }
```

Matched exhaustively, with no catch-all arm, everywhere behaviour differs.
An absent `item_type` maps to `Login`, preserving today's behaviour for items
whose type the server omitted.

### 2. The read pane

Header subtitle becomes the kind label: "Login", "Secure note", "Card",
"Identity", "SSH key", "Unsupported item".

**"Fill in app" and the autofill-targets card render only for logins.** Every
other kind gets Delete and nothing else in that row. This is what stops a
card offering to type empty strings into a focused application.

Body, one card per kind:

| Kind | Contents |
|---|---|
| Login | Unchanged. |
| Card | Cardholder name, Brand, Number *(masked/reveal/copy)*, Expiry as `MM/YYYY`, Security code *(masked/reveal/copy)*. |
| Identity | Grouped: Name, Contact, Address, Government IDs. |
| SSH key | Public key *(copy)*, Fingerprint *(copy)*, Private key *(masked/reveal/copy)*. |
| Secure note | The note body. |
| Unsupported | Name, kind, notes, and a line stating Deskwarden does not know this type yet. No fabricated fields. |

Then a **Notes** card at the bottom for every kind whose `notes` is
non-empty, logins included.

**Empty fields are hidden, and an empty group does not render.** An identity
has eighteen fields and real ones populate a handful; without this rule the
pane is mostly blank labels. The suppression rule is a pure function, tested
directly.

The expiry rendering is a small pure function too: `expMonth`/`expYear` are
separate string fields and either may be absent, so "what does a card with a
month but no year display" is a decision, not an accident. It renders
whatever half is present rather than a half-formed `MM/`.

### 3. Create and edit

`EditDraft` becomes kind-aware, and the create form gains a **type selector
inline in the form** — not a new modal step. This app has exactly one modal
pattern and a five-item choice does not justify a second.

`NewLoginItem` gains siblings, one per kind. A create payload must contain
**exactly one type object and no siblings**: posting a card with an empty
`login: {}` alongside it is how an item ends up with both.

Editing is a full-state PUT, so the preservation property already established
for logins applies unchanged to every new kind: the draft is applied onto a
**clone of the existing item**, so unmodelled fields survive. This is
`with_app_match`'s and `EditDraft::apply_to`'s existing pattern and must not
be re-invented per kind.

### 4. Picker

`load_items_for_picker` filters to logins. Attaching an app-match to a secure
note is meaningless — the fill would type two empty strings — and the filter
is one predicate.

### 5. Verifying the wire shape

**The exact JSON field names are verified against the real vault before any
struct is written, not transcribed from memory.** One
`GET /list/object/items` against an unlocked `bw serve`, reduced to one item
of each type, with the raw JSON recorded in the implementation plan.

This is a norm in this codebase, not caution for its own sake:
`sidebar.rs::scope_contains` leaves Trash explicitly unimplemented because
there is "no confirmed knowledge of `bw serve`'s trash/deletedDate JSON
shape, so rather than guess (and risk silently misclassifying real data)" it
does nothing. A wrong field name here does not fail loudly. It deserializes
to `None`, rides `other`, and is silently written back — until the day the
modelled field and the `other` copy disagree.

If the real vault has no item of some type, create a throwaway one in the web
vault to capture its shape, then delete it.

## Error handling

Nothing here introduces a new failure mode. Reads come from `VaultCache` as
they do today; writes go through the same `VaultCache` methods, which already
surface `VaultError::Unauthorized` through the vault window's re-auth path.

The only new decision: an item whose `item_type` is known but whose
corresponding object is **absent** (a `type: 3` with no `card`) renders as
that kind with every field empty, not as an unsupported item. The server said
what it is; a missing object is an empty card, not an unknown type.

## Testing

Round-trip fidelity, per new struct and for item-level `notes`, following the
tests already in `vault_bridge.rs`:

- **absent stays absent** — a card with no `brand` key does not gain
  `"brand": null` on write.
- **empty stays empty** — an empty string is not an absent key.
- **unknown keys survive** — deserialize a real item carrying unmodelled
  keys, re-serialize, assert the JSON is byte-equal. This is the test that
  caught the `UriEntry` bug and would have caught the `LoginData` one.

Plus:

- `ItemKind` as a table test over every type number, including an unknown one
  and an absent one.
- Empty-field suppression and the expiry formatter, as pure functions.
- The picker lists only logins.
- Each kind's create payload carries exactly one type object and no siblings.
- An edit of each kind preserves unmodelled fields, using a fixture with real
  values in `other` rather than empty ones — a coverage gap a previous review
  closed for `EditDraft` and which must not reopen per kind.

Decisions are extracted into pure functions and tested directly rather than
reasoned about inside an eframe closure. On this codebase that is not a
stylistic preference: every one of the fifteen defects the independent
reviews found lived in an untested seam.

## Risks

- **Silent data loss on write.** The dominant risk, and the one this
  repository has already realised twice in two different structs. Mitigated
  by the per-struct `other` catch-all, the three round-trip tests per struct,
  and verifying field names against the real vault rather than memory.
- **An item carrying two type objects.** A create payload that includes an
  empty sibling, or an edit that writes a kind's object onto an item of
  another kind. Mitigated by building create payloads per kind and by type
  being immutable after creation.
- **The identity pane being unreadable.** Eighteen fields, mostly empty.
  Mitigated by grouping and empty suppression, both tested.
- **Scope creep into autofill.** Cards and identities look fillable. They are
  out of scope by an explicit decision, and the fill path is not touched.

## Out of scope

- Autofill for non-login types, including the app-match field, the match
  engine, the hotkey picker and the prompt overlay.
- Passkey management. Bitwarden stores passkeys as `fido2Credentials` on
  login items, the sidebar already filters them, and creating or editing them
  is a different feature.
- Attachments.
- Trash, which stays unimplemented for the reason documented in
  `sidebar.rs::scope_contains`.
- Organization and collection ownership.
- The `Zeroizing` escape routes already documented in
  `deskwarden/README.md`. New secret fields get the same treatment the
  existing ones have; widening that guarantee is a separate decision.

## Sequencing note

This feature and the planned encrypted disk cache
(`docs/superpowers/specs/2026-07-30-encrypted-vault-disk-cache-design.md`)
both touch `VaultItem`'s serde shape. The disk cache serializes the whole
snapshot to disk, so its round-trip tests must cover whatever fields exist
when it ships. **Doing this feature first is the cheaper order**: the disk
cache then inherits the new fields for free, rather than needing its tests
extended afterwards.

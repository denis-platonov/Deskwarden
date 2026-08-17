# Card Art — Bank Icon, Network Badge, and Brand Detection

**Date:** 2026-08-17
**Status:** Design, approved in outline. Ready for a plan.

A card in the vault currently renders like every other non-login item: a
generic tile. This gives cards the two things that actually identify one to a
person — **the bank it is with**, and **which network it runs on** — laid out
the way a physical card is: bank large, network small in the lower right.

---

## The pieces

They are one design because they share the card detail view and the item list
tile, and because **four of them are consequences of the same function.**

1. **Network badge** — from `CardData.brand`, which the vault already stores.
2. **Bank icon** — from a namespaced custom field naming the bank's domain,
   fetched through the existing favicon machinery.
3. **Brand detection** — suggest the network from the card number while the
   user has not chosen one.
4. **Masked number** — the right number of dots, grouped the way that brand
   groups, with the last four shown.
5. **Masked security code** — three dots or four, depending on the brand.
6. **Billing ZIP**, and later a link to a full address.

### Why 3, 4 and 5 are one thing

A card's digit count, its grouping, and its security-code length are all
properties of the network:

| | Visa / Mastercard | American Express | Diners Club |
|---|---|---|---|
| Digits | 16 | **15** | 14 |
| Grouping | 4-4-4-4 | **4-6-5** | 4-6-4 |
| Security code | 3 | **4** | 3 |

So `•••• •••• •••• 4242` is correct for a Visa and **wrong for an Amex**, which
must read `•••• •••••• •4321`. A mask that always draws sixteen dots in fours
is not a cosmetic approximation — it tells the user their card is a different
card from the one they hold.

This makes `brand_for_number` load-bearing for the badge, the mask, the
grouping and the code length at once, which is the argument for putting all of
it in one spec rather than four.

**The mask is derived from the stored number's own length where there is one.**
The brand table is the authority for *grouping* and for the code length; the
dot count follows the digits actually stored, so a card the table does not
recognise still masks to its true length rather than to a guess.

## What already exists

Almost all of it, which is why this is worth doing now.

- **`CardData.brand`** (`vault_bridge.rs:150`) is modelled, deserialized and
  tested. Bitwarden stores `"Visa"` on the item. **No BIN lookup, no network
  request, nothing leaves the machine.** Fixtures show both `"Visa"` and
  `"visa"`, so every read of it is case-insensitive.
- **`favicon.rs`** is a complete pipeline: `domain_from_uri`, `fetch_icon_bytes`,
  `decode_rgba`, `read_cached_icon`, `write_cached_icon`, all keyed on a domain
  string.
- **`IconCache`** (`item_list.rs:15`) holds loaded textures **keyed by item
  id**, populated by a loader elsewhere; `item_list` only ever reads it.
- **`deskwarden:app-match`** (`vault_bridge.rs:362`) is an established
  convention for a namespaced custom field this app owns, with
  `with_app_match` showing how to write one back without destroying the
  server's other keys.
- **The 32px avatar tile with a monogram fallback** (`item_list.rs:567`,
  `:1070`) is where a login favicon already renders.

## The seam that makes this small

`IconCache` is keyed by **item id**. `favicon.rs` is keyed by **domain**. The
only thing standing between them is a question — *which domain does this item
use?* — that is currently answered inline for logins.

Lift it into one pure function:

```rust
/// The domain whose icon represents this item, or `None` for an item that
/// has no icon of its own and should fall back to a monogram.
pub fn icon_domain_for(item: &VaultItem) -> Option<String>
```

- A login answers with `domain_from_uri` of its first URI, exactly as today.
- A card answers with its `deskwarden:bank-domain` field, if set.
- Everything else answers `None`.

**Consequence:** the icon loader and `item_list` need no knowledge of cards at
all. They keep asking "what domain?" and start getting an answer for a kind of
item that previously had none. Cards get fetching, disk caching, the on-screen
prefetch window and the monogram fallback for free, because those are
properties of the machinery rather than of logins.

This is the whole design. The rest is drawing.

---

## 1. The network badge

**Source:** `CardData.brand`, matched case-insensitively against a fixed set.

**Drawn, not shipped.** The marks are generated the way
`assets/generate-icon.py` already generates the application icon, rather than
committing the official logos. Two reasons: it keeps the repository free of
opaque binary brand assets, and it sidesteps each network's brand guidelines on
colour, clear space and minimum size — rules that a 12-pixel corner badge would
break by existing. The cost is honest: a drawn mark is less immediately
recognisable than the real one.

**An unrecognised brand renders no badge.** Not a placeholder, not a question
mark — the tile simply looks as it does today. A badge that says "unknown" is
noise on an item the user already knows the identity of.

## 2. The bank icon

**Source:** a `deskwarden:bank-domain` custom field, set through a picker in
the card's detail view. Never free text — a typo produces a silent fetch
failure and an unexplained blank tile.

**Written with the `with_app_match` pattern**, rebuilding only that one field
and preserving every other key the server round-trips, for the reason that
function records: `bw` normalises what this app writes, so rebuilding a field
from name and value alone drops keys on the next write.

**No field means no bank icon**, and the tile falls back to the network badge
drawn at full size, then to the existing monogram. Each fallback is a step down
in specificity, never a blank.

**Privacy is unchanged in kind but new in degree.** Fetching a bank's favicon
tells whoever serves it that someone looked up that bank — the same exposure
logins already have, now applying to an item type where it did not before.
Because the user sets the domain explicitly through a picker, this is a choice
rather than a surprise. The card number is never involved in any request.

## 3. Brand detection from the number

A pure function over the leading digits:

```rust
pub fn brand_for_number(digits: &str) -> Option<CardBrand>
```

**It returns Bitwarden's canonical spellings** — `Visa`, `Mastercard`,
`American Express`, `Discover`, `JCB`, `Diners Club`, `UnionPay` — and the
dropdown offers exactly the same set. This is interoperability, not tidiness:
`brand` is shared with every other Bitwarden client, and the web vault renders
its own card art from that string. `"MasterCard"` or `"MC"` yields a card that
looks correct in Deskwarden and blank everywhere else.

**The suggestion never overwrites a choice.** The rule is *suggest only while
untouched*: once the user picks from the dropdown, their pick stands
permanently, including when the number is edited afterwards. Without this, the
sequence "type number → we set Visa → user corrects to Mastercard → user fixes
a typo in the number → we silently restore Visa" is the default behaviour, and
it is a data-loss bug wearing a convenience costume.

`detail_edit.rs` already has this exact mechanism in `template_touched`. Use
the same shape rather than inventing a second one.

**An unrecognised prefix leaves the field alone** rather than clearing it. A
partially typed number passes through prefixes matching nothing, and a brand
field that flickers empty while typing reads as broken.

**No Luhn validation.** It answers a different question — whether the number is
well-formed — and a card the user is midway through typing is always invalid.
Refusing to suggest until the number is complete would make the feature appear
only after it has stopped being useful.

## 4. The masked fields and the expiry label

**The number.** Masked to its true length, grouped by the brand's own
convention, with the **last four digits shown**: `•••• •••• •••• 4242` for a
Visa, `•••• •••••• •4321` for an American Express. The last four are the
digits printed on every receipt and asked for by every support line; they are
what identifies the card to its owner, and they are not enough to use it.

Fewer than four digits stored → mask everything and reveal nothing. A partial
number is a data-entry state, not a card, and revealing "the last four" of a
six-digit fragment discloses a larger fraction of it.

**The security code.** Three dots or four, per the table above. This one is
never revealed by the mask — unlike the last four, a CVV has no
identification use to trade against its risk. Revealing it stays an explicit,
deliberate act behind the existing reveal affordance.

**The expiry label is `MM/YY`.** A card expires in a month and a year; there
is no day. `CardData` stores `exp_month` and `exp_year`, and
`detail.rs::card_expiry_text` already composes them — this is a label, not a
new field.

## 5. Billing ZIP, and the address link

An online card form usually wants three things: the number, the security code,
and the **billing postcode**. The postcode is the one the vault has nowhere to
put today.

**Do the ZIP first, on the card itself.** A `deskwarden:billing-zip` custom
field, in the same namespaced convention as `deskwarden:app-match` and
`deskwarden:bank-domain`. No lookup, no second item, no linkage — and it
covers the common case on its own.

**The full address already exists and is not a new record type.**
`ItemKind::Identity` models `address1`/`address2`/`address3`, `city`, `state`,
`postal_code`, `country`, plus name, company, email and phone
(`vault_bridge.rs:182`). Identities are creatable, and editable as of the
2026-08-17 fix that turned on Card and Identity together. **Do not add an
Address kind**; Bitwarden has no such type, and inventing one would produce
items no other client can read.

**What is genuinely missing is a link**, and Bitwarden has no native
card-to-identity relation. When it is wanted, use the same convention again:
`deskwarden:billing-identity` holding the identity's item id, resolved at
render time, degrading to the ZIP and then to nothing.

**Ordering matters here.** The ZIP field is a few lines and covers most real
forms; the identity link needs a picker, a resolution path, and a decision
about what to show when the linked item is deleted. Shipping the ZIP first
means the second half is optional rather than load-bearing.

## 6. The card pane's layout

The detail pane currently lists a card's fields as five stacked label/value
rows, in the order the struct happens to have them. **Lay it out like the
object it describes instead**, so the eye finds things where the physical card
puts them.

```
[brand]  ••••  ••••  ••••  4242
Expires  08/29        Code  •••
──────────────────────────────────
A. Novak                    (or the linked identity)
```

**One line: brand then number.** The brand becomes the icon from §1 rather
than the word "Visa", sitting immediately before the digits — which is where
it sits on the card, and which removes a whole row.

**One line: expiry and security code.** They are short, they are always read
together when filling a form, and stacking them wastes two rows on eight
characters.

**The cardholder goes to the bottom**, below a divider, because it is the
least-consulted field and because that slot is where the **linked identity**
(§5) will render when one is set. Name or identity, never both — the identity
supersedes the bare name, since it contains it.

### Consequences to check rather than assume

- **The rows above are being rebuilt anyway** for masking (§4) and for the
  typography fix that made the expiry match the digits around it. Do this
  after that lands, not concurrently — they are the same three rows.
- **Two values on one line changes the copy affordance.** Every value in this
  pane is click-to-copy with its own hit area; two on a line means two hit
  areas in one row, and the existing row-level click must not become
  ambiguous. `copy_row` is built around one value per row — check whether it
  generalises or needs a sibling.
- The pane's layout guards measure painted ink and row heights. Removing two
  rows will move them **legitimately**; re-pin deliberately rather than
  loosening a tolerance.

## 7. Shortcuts for card fields

Every value in this pane should be reachable from the keyboard, as a login's
already are.

**The existing table is keyed by field ROLE, not by item kind**
(`detail.rs:1104`): `Password → CTRL+B`, `Username → CTRL+U`,
`Totp → CTRL+T`, `Website → CTRL+SHIFT+U`. There is also a **drift guard that
refuses the same chord appearing twice**, so card chords must be added to that
table rather than beside it — a parallel mechanism would be the "two
enumerations that must agree" defect this project keeps losing to.

**Two ways to fit cards into it, and the choice is real:**

**A — reuse the roles.** A card's number is its sensitive value and its
cardholder is its identifying one, so `CTRL+B` copies the number and `CTRL+U`
the cardholder, exactly as they do on a login. Only the security code and
expiry need new chords. Muscle memory transfers; two new chords instead of
four.

**B — give cards their own chords.** Unambiguous, self-documenting, and every
chord means exactly one thing everywhere. Costs four new bindings in an
already-crowded space, and asks the user to learn a second set.

**Recommendation: A.** The table is already role-keyed rather than
kind-keyed, which is the design saying roles are the unit. "Copy the secret"
and "copy who it belongs to" are the same intent on both kinds.

**Whichever is chosen, the chord-uniqueness guard decides whether it is even
expressible.** Check that guard before writing code: if it forbids one chord
resolving differently per item kind, option A is not available and the
question is settled by the constraint rather than by taste.

Expiry and security code need chords in either case. **Do not pick them from
this document** — pick them by reading which chords the crate already spends,
including the global hotkeys in `hotkey.rs`, and say what you found.

## 9. Tile composition

**Detail view:** bank icon large, network badge in the lower-right corner,
overlapping the icon's edge — the arrangement of a physical card, which is why
it reads without a legend.

**Item list:** the same composition inside the existing 32px tile. The badge is
small enough at that size to be a colour-and-shape cue rather than a readable
mark, which is its job there: it distinguishes two cards from the same bank.

**The list tile geometry does not change.** Cards render into the tile that
already exists, so the layout guards over it stay true and the change cannot
push a control out of the row.

---

## 10. Testing

The pure parts carry the weight, as usual here:

- `icon_domain_for` — a login yields its URI domain, a card with the field
  yields that domain, a card without yields `None`, and a **control** that the
  card fixture really does carry a number, so `None` is about the missing field
  and not an empty item.
- `brand_for_number` — one case per network, asserted **positively** against
  the canonical string; an unrecognised prefix yields `None`; a prefix of a
  valid number yields either the right brand or `None`, never a wrong one.
- **The touched rule** — a test that edits the number after a manual pick and
  asserts the pick survives. This is the test that matters most; write it
  first.
- `mask_for` — a Visa masks to sixteen dots in fours with the last four
  shown, an Amex to fifteen in 4-6-5, and a stored number of fewer than four
  digits reveals **nothing**. Assert the rendered string, not a length.
- The security-code mask is four dots for Amex and three otherwise, and is
  never revealed by the mask itself.
- Badge and tile composition through the existing headless `Context::run_ui`
  harness, measuring painted ink the way the overlay row tests do.

## 11. Deliberately not in scope

- **Reading the bank from the number.** BIN-to-issuer requires a lookup
  service, and sending even a prefix discloses the issuer of a card in the
  user's vault. The explicit domain field exists to avoid this.
- **Luhn checking, expiry validation, or any other correctness UI** on card
  fields. Different feature, different argument.
- **Official network logos.** Revisit only if the drawn marks read poorly at
  32px, which is the size that decides it.

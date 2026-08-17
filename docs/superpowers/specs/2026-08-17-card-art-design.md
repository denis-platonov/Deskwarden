# Card Art — Bank Icon, Network Badge, and Brand Detection

**Date:** 2026-08-17
**Status:** Design, approved in outline. Ready for a plan.

A card in the vault currently renders like every other non-login item: a
generic tile. This gives cards the two things that actually identify one to a
person — **the bank it is with**, and **which network it runs on** — laid out
the way a physical card is: bank large, network small in the lower right.

---

## The three pieces

They are one design because they share the card detail view and the item list
tile. Split into separate specs, each would touch the same two files.

1. **Network badge** — from `CardData.brand`, which the vault already stores.
2. **Bank icon** — from a namespaced custom field naming the bank's domain,
   fetched through the existing favicon machinery.
3. **Brand detection** — suggest the network from the card number while the
   user has not chosen one.

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

## 4. Layout

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

## Testing

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
- Badge and tile composition through the existing headless `Context::run_ui`
  harness, measuring painted ink the way the overlay row tests do.

## Deliberately not in scope

- **Reading the bank from the number.** BIN-to-issuer requires a lookup
  service, and sending even a prefix discloses the issuer of a card in the
  user's vault. The explicit domain field exists to avoid this.
- **Luhn checking, expiry validation, or any other correctness UI** on card
  fields. Different feature, different argument.
- **Official network logos.** Revisit only if the drawn marks read poorly at
  32px, which is the size that decides it.

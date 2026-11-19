# Slice 13 - Card and bank logos (design 1e)

Spec: `docs/design/deskwarden-v2-design-spec.md` Part D section 4, decision D1 (Part A 0.2).

## Tasks

1. **Marks and tables** (`kit/logos.rs`, `kit/logo_paths.rs`). `Net` (nine design networks + RuPay), `Bank` (ten), `Mark`; colours, short and long labels, rule text, Simple Icons path data (CC0, release 13.21.0) as Rust consts, a bank-from-domain table. Tests: every path parses and stays in its viewBox; tables match the design; domain matching is by dot boundary.
2. **Offline drawing.** `rasterise` (non-zero winding, exact horizontal coverage, four sub-scanlines) turns a glyph into a white-on-alpha mask texture at exactly the device pixels it covers, cached per context; `paint` / `paint_aligned` draw the glyph contained in the design's image box, or the text label when there is no glyph. Tests: holes stay clear, partial coverage, glyph inside its box, text fallback, anchoring.
3. **Mir.** `card_brand::is_mir_number` (2200-2204) and `Net::for_number` testing it before Mastercard. Mir is not a Bitwarden brand, so no `CardBrand` variant is added and nothing is stored.
4. **Badge composition.** `avatar::Look::Brand { mark, chip }`; `item_list::{card_net, item_bank}`; `row_look` makes a bank the main badge with the network as a 30x20 / 42x28 chip, else the network, else favicon, else initials. The list, the detail header and the command palette all go through `row_look`.
5. **Card face.** Gradient base is the bank's colour, else the network's, else navy; bank mark top-left (62x24, else its name 18/900); network mark bottom-right (the user's `brand-marks` picture if present, else the bundled mark, else the long label 20/900).
6. **Editor.** The card number row shows the live network badge from the digits typed (else the chosen brand).
7. **Preview** surfaces in `examples/ui_preview.rs` (not run).

## Rulings

- Ruling: Only 13 of the design's 19 marks are bundled; Mir, Maestro, UnionPay, Citi, ING and Santander are drawn as the design's text tiles -- Simple Icons no longer carries those six (the CDN 404s for them, so the design itself shows its fallback) and no other CC0 source was available offline -- cost if wrong: six names show text instead of a glyph; adding a path const later is a one-line change.
- Ruling: Glyphs are cropped to their ink and contained in the design's image box, not the whole 24x24 viewBox -- CSS `contain` on the viewBox would draw a wordmark a few points wide, far under the sizes the design lists -- cost if wrong: marks look larger than the mock.
- Ruling: Glyphs are rasterised by a small in-crate scanline filler rather than `kit::icon::fill` -- `fill` is convex-only and Simple Icons are concave with holes; no dependency added -- cost if wrong: ~100 lines to maintain.
- Ruling: Mir is a `Net` and not a `CardBrand` -- `CardBrand` is Bitwarden's ten brands one for one and a stored "Mir" would render blank in other clients. A card with no brand, or the brand "Other", and a 2200-2204 number badges as Mir; it is never written -- cost if wrong: a Mir card's brand box stays empty until the user picks Other.
- Ruling: A bank is derived from the domain the item already carries (a card's `deskwarden:bank-domain`, a login's first website) through a built-in domain table; nothing new is stored -- the design's `bank` key does not exist in Bitwarden -- cost if wrong: a bank outside the ten keeps its favicon/initials.
- Ruling: A bank's mark wins over the card network and over a favicon for logins on that bank's domain, as the design's `mainKey` does -- cost if wrong: a Chase login shows the Chase mark instead of its favicon.
- Ruling: The linked-login pill on the card face (design 4.5) is not built -- it needs the window to select another item from the detail pane, which this slice does not add; the card is linked to its bank by domain and the bank row in the field card still names the bank -- cost if wrong: no one-click jump from a card to its login.
- Ruling: The tray gets no logos -- design section 5 note 3 says its rows draw initials or favicons only -- cost if wrong: none (as designed).
- Ruling: The `brand-marks` override stays where it already applied (the card number row marks) and now also wins on the card face's network slot; the small avatar badges always use the bundled marks -- cost if wrong: a user's own Visa picture does not show on 44pt tiles.

# Autofill picker popup to the v2 design

The owner said the account picker (`src/picker_prompt.rs`, painted through
`src/panel_draw.rs::draw_account_row`) is "not exact match" to design 1b's Fill
panel (`docs/design/deskwarden-v2-design-spec.md` Part D 1.3).

## Tasks

Round 1 (merged as f027750):

1. Row geometry: 64 px rows, 2 px apart, list padded 8 px each side.
2. Row text: username bold over the item name, centred on the tile.
3. Hover selects (D 1.3 `ovSel`): wash and `Enter` badge follow the pointer, and focus with them.

Round 2 (owner: "as close to the design as possible, keeping ONE extra: the Search the vault row"):

4. Remove from the card: the "Fill from vault" heading pair (and the second step's and search mode's), the
   close button, every digit / S / N / ESC key badge, the New login / Cancel / Edit binding footer.
   Layout is header + rows + 8 px padding (`layout`, `layout_for`, `layout_empty`).
   Tests: `nothing_the_card_lays_out_falls_off_the_bottom_of_it`,
   `the_card_draws_no_footer_no_close_glyph_and_no_key_badges`.
5. The card is sized to its content: header + n rows (+ the Search row), or the second step's fields.
   Test: `the_card_is_sized_to_its_content`.
6. The Search the vault row is styled as an account row: a magnifier in the tile slot, no badge.
7. Keyboard kept: arrows and hover select, Enter fills, Esc cancels, 1-5 pick a row (undrawn),
   typing a letter starts the search with that letter in the box, Ctrl+N opens New login
   (list, empty card, search mode), B edits the binding in the second step.
8. The No-match (empty) card is the design's 1.5 card: header, heading pair, two 46 px buttons 8 px apart;
   it loses its Cancel footer, its close button and its N / S badges (N and S still work there).
9. Locales: tooltip lines "S search the vault" and "N new login" dropped (they would lie), the "Esc close"
   line renumbered, and the keys for the removed headings and buttons deleted from all 35 locales and the
   text-fit table in theme.rs.

## Rulings

Ruling: the owner's reversal supersedes round 1's "keep the chrome" ruling -- the card is the header (shield, "Deskwarden", `?`) and the rows, plus one Search the vault row -- the owner said so -- cost if wrong: none; mouse users lose the New login and Cancel buttons (Ctrl+N and Esc remain; the empty card still offers New login as a button).
Ruling: the empty (No-match) card keeps its heading pair and its two buttons, because design 1.5 draws exactly those -- cost if wrong: one heading on one mode.
Ruling: the second step (which field to type) and search mode lose their headings too; the search field keeps its border, because it is the one container the owner asked for earlier ("both inside of same field") -- cost if wrong: a step with no title, whose rows say what they type.
Ruling: search mode stays at its full six-row height (its result count changes per keystroke and a card that resized under the typing hand would jump); the first two steps are sized to their rows -- cost if wrong: the card resizes once when moving from the list to the second step.
Ruling: typing a letter A-Z starts the search; bare N and S are no longer shortcuts in the list (they are letters), Ctrl+N is New login. Digits 1-5 still pick a row, so a digit does not start a search; non-Latin layouts start a search only through their Latin-position letters as GDI's MapVirtualKey reports them -- cost if wrong: the first character from an IME is not seeded (the box is still one click away on the Search row).
Ruling: the `?` tooltip no longer lists the S and N keys -- translating two new lines into 34 languages was out of scope and the old ones would be false -- cost if wrong: Ctrl+N is undiscoverable on the list card.
Ruling: the window corners stay DWM's (Win11 ~8 px) rather than the design's 18 px, and the soft 0 24px 60px shadow is the system's -- GDI has no antialiased region -- cost if wrong: corners 10 px tighter than the mock.
Ruling: hover moves focus only in the list and palette steps, not in search (the text box owns the keyboard) and not on the empty card -- cost if wrong: search results keep the grey hover.
Ruling: an item with no username leads with its name and leaves the faint line empty -- cost if wrong: one blank line on such rows.
Ruling: the title's -0.01em letter-spacing is not applied -- GDI has no tracking -- cost if wrong: ~1 px.

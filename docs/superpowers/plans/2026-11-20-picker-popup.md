# Autofill picker popup to the v2 design

The owner said the account picker (`src/picker_prompt.rs`, painted through
`src/panel_draw.rs::draw_account_row`) is "not exact match" to design 1b's Fill
panel (`docs/design/deskwarden-v2-design-spec.md` Part D 1.3).

## Tasks

1. Row geometry: 64 px rows, 2 px apart, list padded 8 px each side (the card had
   58 px pitch and a 16 px inset). Test: `the_rows_are_the_designs_64_with_a_2_gap_in_a_list_padded_8`,
   `panel_draw::a_row_paints_the_designs_64_wash_and_centres_its_text_on_the_tile`.
2. Row text: username bold over the item name (the card led with the name), the
   two lines centred on the tile as a 19 + 2 + 17 block. Test: `a_row_leads_with_the_username_and_names_the_item_under_it`.
3. Hover selects (D 1.3 `ovSel`): the blue wash and `Enter` badge follow the pointer,
   and focus follows with it so Enter fills the row it shows.
4. Heading pair at the design's 19/800 over 15 (D 1.5), 4 px apart; theme.rs
   text-fit budgets updated. Test: `the_heading_pair_is_the_designs_title_over_body`.

## Rulings

Ruling: the picker keeps its "Fill from vault" heading pair, the ✕, the digit/S/N chips, the Search-the-vault row and the New login / Cancel footer, none of which design 1b's Fill panel draws -- the card also hosts the second step, search and the empty mode, and nothing 0.15 does may be dropped -- cost if wrong: the list step carries ~150 px of chrome the mock does not.
Ruling: the window corners stay DWM's (Win11 ~8 px) rather than the design's 18 px, and the soft 0 24px 60px shadow is the system's -- GDI has no antialiased region and a hard 18 px `SetWindowRgn` corner is uglier than a smaller smooth one -- cost if wrong: corners 10 px tighter than the mock.
Ruling: the card is 575 px tall at six slots (was 524) because rows are now the design's 66 pitch; it is still laid out at six slots in both steps so it never changes height under the pointer -- cost if wrong: a tall card on a small work area (placement already clamps).
Ruling: hover moves focus only in the list and palette steps, not in search (the text box owns the keyboard) and not on the empty card (its rows are buttons that would grow a focus ring) -- cost if wrong: search results keep the grey hover instead of the blue selection.
Ruling: an item with no username leads with its name and leaves the faint line empty -- the design always has a username -- cost if wrong: one blank line on such rows.
Ruling: the title's -0.01em letter-spacing is not applied -- GDI draws no tracking on a proportional face -- cost if wrong: ~1 px on a 20-character title.

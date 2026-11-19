# Slice 15d: Sends and the last 0.x frames

**Why:** slice 15c left the Sends screen as the two-pane list and detail of slice 3 (only its empty states restyled) and five modals on the 0.x `theme::modal_card` / `movable_modal` frames; the owner's complaint ("many small screens are not updated") stands for those. This slice builds the Shared card grid and the Send detail modal, moves the five frames onto `kit::modal::Dialog`, restyles the edit view's 2FA row buttons, and sweeps the previews for anything else that still reads 0.x.

**Source of truth:** `docs/design/deskwarden-v2-design-spec.md` Part A 4 (Shared view), Part B 2 (modal frame), 7 (Send detail), 8 (2FA); `docs/design/Deskwarden v2.dc.html`.

**Method:** every screen is looked at in its `ui_preview` PNG before and after; the v2 references are `vault_shell_light.png`, `modal_new_item.png`, `edit_login.png`. Before-PNGs are kept in `$CARGO_TARGET_DIR/before/`.

## Tasks

1. **Sends.** `send_ui.rs` keeps its model (rows, states, wording, the verdict and every `SendUiAction`) and loses its two columns; `send_ui/shared_page.rs` draws the page and the modals.
   - The page: 32/800 "Shared" with the 20 px `?` tip (status bubble, opens below), the sub-line, a two-column grid (one column under 560 px) of cards: 46 px avatar, name over recipient, status pill, chevron, a meta row (clock + when it ends, eye + opens), Copy link and Revoke along the foot of a live card. Ended cards are drawn at .62.
   - The modal (600): header (56 px tile, 24/800 name, recipient and kind, pill), three fact tiles (Opens, Link, Includes), the Address / Recipient / Opens with / Views / Expires / Deleted rows, Activity (a rail of what is known), the amber "Someone has seen this password" band with Change password, and the buttons: Copy link, Extend, Open record, Revoke.
   - Revoke's two steps are kept: the question joins the top of the modal's body (the body keeps its resting height, so the buttons do not move) and Cancel takes Revoke's slot.
   - Person shares and Shared with me are cards on the same page; a share opens a modal with the two-step Stop sharing; an import opens a small modal; a record somebody shares opens in the vault.
2. **The 0.x frames onto `Dialog`:** the sequence builder, the app-window picker, the in-form password generator, the record import form, `record-share`. Each gets a `ui_preview` surface.
3. **The edit view's 2FA card:** Replace / Remove into the design's treatment.
4. **Sweep** every preview against the three v2 references; fix what still reads 0.x.

## Tests

The 0.x geometry pins (header bands, row bands, the 54/62 pt rows, the actions on the title's line, the keycaps) are retired; every behaviour test is kept or rewritten against the new controls. `Painted` tells the page's words from the modal's by where the scrim falls in the paint order (`in_list` / `in_detail` keep their old meanings: what is on the page, what is in the modal).

## Rulings


Ruling: the Sends screen is one page of cards plus modals; 0.15's list column and detail pane (and every pinned geometry test of them: the 54/62 pt rows, header strips, actions on the title line, fact-card bands) are retired, the behaviour tests rewritten against the cards and the modal -- the design has no two-pane Sends screen -- cost if wrong: none for behaviour, the old pixel pins are gone.
Ruling: Revoke keeps its two steps (spec B 7 says "no confirmation") -- the app's Revoke deletes the Send, so the confirmation stays; the question joins the top of the modal's body, which keeps its resting height, so Cancel stands exactly where Revoke was -- cost if wrong: one extra click against the design.
Ruling: the modal's Extend keeps 0.15's one-week step and its label (spec says "Add 1 day"); Open record stays as a fourth button; Revoke is offered on an ended Send too (to delete it) though spec hides actions then -- no 0.x behaviour may go -- cost if wrong: buttons the design lacks.
Ruling: Activity is built from what a Send keeps (made, sent to, opens, how it ends) with the 0.15 answer sentence as its lead; there is no per-open timeline -- Bitwarden has none -- cost if wrong: a shorter Activity than the design's.
Ruling: no Mod+C / Del shortcuts on Copy link / Revoke and no card entrance animation -- 0.15 had neither -- cost if wrong: the hover hints and motion are missing.
Ruling: a card's Revoke selects that Send and asks, so the question appears in the modal; Esc or the scrim on a modal with a question up reports CancelDelete -- cost if wrong: none.
Ruling: the seen-password band sits with the buttons below the scrolling body, not inside it -- a tall modal must not hide the one thing it asks the user to do -- cost if wrong: a taller fixed foot.
Ruling: in the Shared with me page the people-received cards open the record in the vault (as the 0.15 rows did) and the org-grouping hairline is kept; "Paste an invite link" keeps `share_ui::draw_paste_and_waiting` (still 0.x inline field and small button) -- cost if wrong: that one row still looks 0.x.
Ruling: `ShareRow`, `ReceivedShareRow` and their fields are `pub` and `PeopleRows` gained `with_shares` / `with_received` / `confirming_stop`, and four modules gained `preview_*` / `SharePreview` doors, so `ui_preview` can draw them -- cost if wrong: a wider public surface.
Ruling: the five 0.x frames are `kit::modal::Dialog`s (ids `record-share`, `record-import`, `password-generator`, `app-window-picker`, `sequence-builder`; scrims unchanged in `MODAL_SCRIM_AREAS`, plus `send-detail`). The scrim and the corner mark now close the generator (Cancel), the picker and the builder (Discard, which still asks first if edited); the picker and builder keep their own Escape rules -- spec B 2 -- cost if wrong: a click past the card dismisses it.
Ruling: the generator drops the CTRL+R keycap (the chord is in New's hint), the share and import forms drop the 0.x header bands, footer bands and the Enter glyph on Share; the sequence builder's Save loses its CTRL+S caption (hover says it); the picker keeps 8b's list rows and match band inside the dialog -- cost if wrong: a few 0.x touches inside otherwise v2 frames.
Ruling: the edit form's 2FA card Replace / Remove are the kit's small outline / danger buttons and the empty state is the tinted "Add 2FA code" with the scan frame (50 high); the rest of the form's small row buttons (Copy, Remove field, Add a field) are slice 4's and unchanged -- cost if wrong: they still read 0.x beside it.
Ruling: sweep result -- the Password health screens (`vault_password_health*.png`), the rehearsal window and the edit form's small row buttons still read 0.x against `vault_shell_light.png`; not changed here -- cost if wrong: three surfaces for the next pass.

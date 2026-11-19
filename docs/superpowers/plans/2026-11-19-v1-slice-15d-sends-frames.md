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

(see below, appended as the slice went)

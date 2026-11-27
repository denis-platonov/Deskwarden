# Old design gone: vault window widgets (slice "vault-kit")

Goal: the vault window's egui views stop calling the v1 widget helpers in
`deskwarden/src/theme.rs` and draw with `crate::kit` widgets and
`kit::tokens::palette()` instead. Behaviour, actions, keyboard paths and
widget ids are unchanged. The theme.rs helpers are NOT deleted here.

## Tasks

1. Kit additions (each with a test in its own module):
   - `kit::input::{field, field_fill, field_secret, field_secret_fill}`: the
     v2 text and secret fields for a form row, id = `ui.next_auto_id().with("field")`
     so focus survives frames the way the bare `TextEdit` did.
   - `kit::input::{field_disabled, field_disabled_fill, FIELD_INSET}`: the greyed
     read-out row (elides its text, can reserve room for a chip).
   - `kit::input::inline`: the compact 42 pt field of the Send access rows.
   - `kit::controls::{dashed_button, dashed_button_width, chip_button, CHIP, link}`.
   - `kit::card::hairline`, `kit::avatar::tile`.
2. Swap the calls: `section_text_field[_within]`, `section_password_field[_within]`,
   `section_disabled_text_field[_within/_reserving]`, `field_label`, `row_button[_danger/_width]`,
   `destructive_button`, `dashed_button`, `inline_field`, `row_rule`, `hairline`, `action_link`,
   `avatar` / `avatar_artwork_tile`+`avatar_image`, `text_area` in
   `vault_window/{detail_edit,sequence_builder,share_ui,send_ui,detail,record_ui}.rs`.
   Hand-styled `palette_button` / `small_chip_button` and the stock `egui::Button` /
   `TextEdit` in the edit form's bank filter, bank list, program-file wait box and
   literal box are replaced by kit widgets.
3. Tests that pinned v1 geometry or colours are re-pinned to the v2 ones.
4. `ui_preview -- --all` looked at: edit_card, edit_login, edit_steps_testing,
   modal_sequence_builder, modal_share, modal_record_import.

## Rulings

Ruling: row-form fields are the kit's 50 pt v2 field (16 pt type, 16 pt inset), not a 34 pt "section" box -- spec B 0.6/1.4 draw one field size -- rows grow 16 pt taller; the expiry/code boxes are re-measured for the new insets.
Ruling: `destructive_button` (red fill, white type) becomes `ButtonKind::Danger` at `SMALL` -- the kit has no filled red button and the v2 confirm is the red-edged outline -- the confirm is a little quieter than before.
Ruling: the key-sequence chips (`<`, `>`, `x`, palette names) are `controls::chip_button`, an outline button 30 pt high -- spec B 1.4 chips are larger than the 0.x 11 pt chip -- the step list rows are taller.
Ruling: the dashed "+ Text / + Key / + Wait" and the kit `dashed_button` use spec B 1.4 F (44 high, radius 12, 15/600, dashes in `border` mixed halfway to `text_ghost`, blue on hover) -- the spec's #c9c6c2 is not a palette token -- dark theme reads as a mid grey dash.
Ruling: `theme::row_rule` / `theme::hairline` become the kit's (`#f1efec` / `#e8e6e3`) -- the detail pane already mixed both -- tests that pinned the v1 colours accept either.
Ruling: avatar tiles in the app picker, the matched-app row and the sequence band are `kit::avatar::tile` (initials on the palette pair, or the icon on white with its ring), 1 pt `blue_edge` when marked, radius size/4 -- v1's artwork-filled tile is gone -- icons now sit inset on white like the list's favicons.
Ruling: the theme colour CONSTANTS (about 540 uses of `theme::INK`, `TEXT_FAINT`, `BLUE` ... in these six files) and the layout framework (`section_row`, `section_card`, `number_box`, `dropdown`, `date_picker`, `eye_toggle`, `state_pill`, `form_card`, `link_label`, `eyebrow`, ...) are NOT converted in this slice -- they are layout and colour plumbing, not widgets, and swapping them is a rewrite of every card in the edit form -- the lead decides whether a second pass is wanted.

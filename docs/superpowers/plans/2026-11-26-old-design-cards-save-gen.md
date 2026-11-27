# Old design gone: the Save login card (3c) and the New password card (3d)

Spec: `docs/design/deskwarden-v2-design-spec.md` Part D, sections 1.7 and 1.8.
Reference card: `unlock_prompt.rs`, `notice_card.rs`. Both cards are bare Win32 and
stay bare Win32.

## Starting point

Both cards had already been moved onto `panel_draw` in part (the Save card has the
preview block and `panel_draw::skin`; the generator uses the lockup, the 410
width and the 15 px label). What was still the old design:

* Save card: a 13 px outline *Generate* button, a hard-coded `ENTER` chip inside
  *Save*, a thin ghost-grey ✕ with no hover.
* Generator: the `New password` title row, an (unpainted) header rule and footer
  band left as spacing, a 17 px one-line ellipsised value with a per-character
  blue tint, a `Strong`-only badge, *Regenerate* as a 156 px pill with a
  `CTRL+R` chip, a footer of `Save to vault` + `Copy` at 200/100 px, controls on
  the white card at radius 7.

## Tasks

1. `panel_draw` additions (no existing behaviour changed): `draw_close_button`
   (D 1.7's 32 x 32 ✕, `#8a8785`, hover `#f1f0ee`), `draw_accent_link`,
   `icon_button_colours` / `draw_icon_button_frame` (D 1.8's 46 x 46 icon button:
   1 px edge, hover turns edge and icon `#1b3fa0`), `draw_refresh_icon`,
   `draw_copy_icon`. Test: the numbers and the hover colours.
2. Save card: ✕ through `draw_close_button` with hover tracking (`WM_MOUSEMOVE` +
   `TrackMouseEvent`/`WM_MOUSELEAVE`); drop the `ENTER` chip (and `SAVE_SHORTCUT`,
   the chip font); *Generate* becomes an accent text link in the Password caption's
   row.
3. Generator: new layout per 1.8. Header = lockup + fitted strength pill + ✕.
   Label `Suggested password`. Value 21/600 mono, up to two lines broken at any
   character (`break_all`, pure and tested), 15 px for a longer value, ellipsis past
   two lines. Button row `Use this password` (flex) + regenerate and copy icon
   buttons. Every option (kind run, length slider, class tiles, minimum digits,
   look-alikes switch) in one `#f7f6f5` r14 well under the row. Header rule, footer
   rule/band, `ButtonSkin::panel_*`/`secondary` removed; `panel_draw::skin` used.
   Tab order follows the new reading order (Use, Regenerate, Copy, then the well).
4. Strings: new key `generate.use_password` in all 36 locales.
5. Both preview examples take `--shot PATH` (paints, saves ONLY the card's own
   window rect via `PrintWindow`, answers with the weakest answer).

## Rulings

Ruling: the Save card's preview block shows the app name and the "no folder" line, not the design's username and masked password -- the card cannot read a username or a password from the window behind it (and must not), and the user types both into the two fields below, so there is nothing true to show in those two slots -- cost if wrong: the preview carries less than the mock's.

Ruling: the Save card keeps its two editable fields (Username, Password) between the preview block and the buttons -- they are the only way either value gets into the new login, and the design has no slot for them -- cost if wrong: the card is taller than the mock's.

Ruling: *Generate* (hand-off to design 3d) is an accent-coloured text link right-aligned in the Password caption row, not a button -- the design has no slot for it and a link keeps it out of the way of Save / Not now -- cost if wrong: less discoverable than a button.

Ruling: the `Enter` chip inside *Save* is dropped; Enter still saves -- D 1.7 draws a plain primary and a keyboard hint is not in it -- cost if wrong: users lose the visual cue for Enter.

Ruling: the generator keeps its ✕ (32 x 32, after the strength pill) although D 1.8's header has only mark, wordmark and pill -- Esc already cancels but a pointer-only user needs a way out of an always-on-top window -- cost if wrong: an extra glyph in the header.

Ruling: the generator keeps every option (kind run, length slider, character classes, minimum digits, look-alikes) in a `#f7f6f5` r14 well below the button row, in the order and with the controls it had -- D 1.8 draws none of them and dropping any would remove a capability (all are bw-generate parameters) -- cost if wrong: the card is ~290 px taller than the mock's.

Ruling: *Copy* is a third 46 x 46 icon button after *Regenerate* (copy glyph from the spec's icon set), and both icon buttons carry their label as the control's window text (never painted) so a screen reader announces them -- D 1.8 has only the primary and regenerate, and Copy is how the password reaches the app the user is looking at -- cost if wrong: an icon-only Copy with no visible word.

Ruling: the *Regenerate* button no longer prints the `CTRL+R` chord; Ctrl+R still regenerates -- D 1.8's button is an icon only -- cost if wrong: the shortcut is undiscoverable from the card.

Ruling: the primary says `Use this password` (new key `generate.use_password`), replacing `Save to vault`; the old `generate.save_to_vault` and `generate.new_password` keys stay in the catalogs because `theme.rs`'s GDI width table still names them -- the owner of `theme.rs` should retire those two rows and add a row for the new key (Archivo-Bold 15, 246 px) -- cost if wrong: two unused catalog keys until then.

Ruling: the `New password` title row is gone -- D 1.8 has no title, the label `Suggested password` carries it -- cost if wrong: none (the lockup says whose card it is).

Ruling: the strength pill keeps the crate's four ratings and labels (`Weak/Fair/Good/Strong`) rather than the spec's four (`Weak/Fair/Strong/Very strong`), with washes mapped Weak = danger (`#fdf3f2`/`#b42318`), Fair = caution (`#fef6e7`/`#7a4f05`), Good = blue (`#eef2fc`/`#14307a`), Strong = the design's green (`#e8f3ec`/`#17673a`) -- `password_strength` is shared with the egui views and its labels are theirs -- cost if wrong: the top tier reads `Strong` where the mock says `Very strong`.

Ruling: the value's per-character blue tint is dropped (D 1.8 sets the value in the ink colour) and the value is two lines of 21 px broken at any character; a value longer than 58 characters drops to 15 px (82 characters in two lines), and one longer than that ends in an ellipsis -- the window cannot grow, so the box is a fixed two lines -- cost if wrong: a 20-word passphrase is cut in the preview (Use and Copy still take the whole value).

Ruling: the options well's controls take radius 10 (was 7) and the kind run fills the well's width -- v2 rounds controls to 10 and a short run stranded left of an empty slot looked unfinished -- cost if wrong: none.

Ruling: the segmented run's and the stepper's cells are still painted by `win32_draw::draw_segment_cell` (shared with other windows, not antialiased) -- changing the shared painter is outside this slice -- cost if wrong: slightly harder corners on those two controls than on the rest of the card.

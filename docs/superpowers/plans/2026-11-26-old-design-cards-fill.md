# Old design gone: the two autofill cards (slice "cards-fill")

Spec: `docs/design/deskwarden-v2-design-spec.md` Part D, section 1.3 (Fill panel) and 1.5 (No match).
Files: `deskwarden/src/prompt_card.rs` (2a), `deskwarden/src/picker_prompt.rs` (3a and its list/palette/search modes), locales, theme.rs GDI budgets.

## Audit

### prompt_card.rs (matched-item card)
Rendered with `examples/prompt_preview.rs` (default, `--long`). Every measured value already matches 1.3:
panel 400, header `14 16 10` with the 20x23 mark and 15/800 wordmark, `?` badge 20x20 (#eceae7 / #6b6866)
with the key-hint tooltip, list `0 8 8` gap 2, rows pad 11x10 r13 gap 13, 42x42 r12 avatar, 16/700 name,
14 #7d7979 sub, selected row #eef2fc with the trailing blue `Enter` badge. It paints no v1 header rule,
no footer band and no Cancel button. Nothing to fix; no source change.

### picker_prompt.rs
| Mode | Deviation found | Fix |
|---|---|---|
| list / palette / search | no header `x`: the only mouse route to cancel was Esc | `x` (20 px glyph, same as prompt_card) as the header's last item; the `?` moves left of it |
| no match | 400 wide, pad 16/14, stacked full-width buttons, "New login", title naming the app, body about process names | 410 wide, pad 18, gap 14, header = mark + wordmark + `x`, title 19/800 "No login for this app yet", body 15 faint, two equal 46 px r13 buttons side by side with gap 8 (primary "Save new login", outline "Search vault") |

List, palette and search rows already went through `panel_draw::draw_account_row` (1.3 row anatomy).

## Tasks
1. Layout: `Layout.close_glyph`; per-shape width/margins; side-by-side empty buttons (`row_at_empty`, `EMPTY_RING`). Tests updated in `picker_prompt::card_tests`.
2. Win32: `x` painted and clickable (`WM_LBUTTONDOWN` -> `Event::Cancel`), excluded from the caption drag; empty buttons painted inside their ring margin.
3. Strings: `picker.nomatch_title`, `picker.nomatch_body`, `picker.save_new_login` replace `picker.empty_title`, `picker.empty_body`, `picker.new_login`; `picker.empty_new_login_sub` is gone (the button no longer has a sub line). All 36 locale files carry real translations (the repo translates every key, it does not copy English). GDI width budgets in `theme.rs` follow.

## Rulings
Ruling: keep the header `x` on the Fill panel and on the account picker although 1.3 draws none -- the card is a frameless always-on-top window that does not close on blur, and Esc is keyboard only; the brief asks for cancel via Esc/`x` -- cost if wrong: one 20 px glyph to delete.
Ruling: the No-match title no longer names the app ("No login for this app yet", D 1.5 verbatim) and the "matches by process name and title" body is dropped -- the spec's words win; `empty_text` keeps its `app_name` parameter (now unused) so `app::handle_no_match`'s call and its source pin stay untouched -- cost if wrong: the card is less specific about which app it means.
Ruling: the empty buttons' keyboard ring is painted in a 2 px margin the control owns (control = button grown by 2) instead of shrinking the button -- the buttons are now side by side and have no spare row height to hold a ring -- cost if wrong: none visible.
Ruling: translations for the three new strings are mine, short forms where the button is narrow (e.g. fr "Enregistrer") -- cost if wrong: a native speaker may prefer other wording; the key set and budgets are right.

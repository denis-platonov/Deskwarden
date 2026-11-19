# Slice 11: the autofill panel (design 1b)

Design D section 1 (1.2 onward; 1.1 is the demo host window and is context
only) draws the panel the daemon puts under the focused field: **Fill**,
**No match**, **Locked**, **Save login**, **New password**, with key hints and
a done check. This slice restyles the 0.15 fill-time cards to that look.

## Ground rules

- The cards are bare Win32 and stay bare Win32 (an egui window costs the daemon
  ~50 MB for ever). Nothing is ported to egui.
- Triggering, anchoring (`app::overlay_position`, `prompt_card::place`), focus
  and filling do not change. Every state and keyboard behaviour stays.
- What GDI cannot do cheaply (alpha ramps, blur, bezier keyframes, antialiased
  corners) is simplified and recorded as a Ruling below.
- `picker_ui.rs` is the egui *app-binding* picker window, not the fill panel;
  it is untouched.

## Map: design case -> 0.15 card

| Design | Card |
|---|---|
| Fill (account list) | `picker_prompt` list / palette / search; `prompt_card` (one account's ways to type) |
| No match | `picker_prompt` empty mode |
| Locked | `locked_card` |
| Save login | `save_login_card` |
| New password | `generate_prompt` |
| Done check | `notice_card::show_done` (new), hooked after a saved login |

## Tasks

1. **`panel_draw`** (new, `OPENS_NO_WINDOW`): the design's geometry constants,
   the "Deskwarden" lockup, avatar tile, `Enter` badge, pill, `?` badge and dark
   key-hint tooltip, outline/primary/deep button skins, regenerate glyph and the
   done check with its animation as pure functions of elapsed ms.
   Tests: widths/heights equal the spec, badge geometry, tooltip stays inside
   the window, easing/progress monotonic, checkmark path cut by progress.
2. **`prompt_card`**: panel header (lockup, `?`, close), account rows as design
   rows (avatar 42/r12, name 16/700, sub 14, blue `Enter` badge on the selected
   row), `?` hover tooltip, no footer rules.
3. **`picker_prompt`**: the same rows for candidates / fields / search results,
   400 wide; No match as the design's card (title, body, two 46 px buttons).
4. **`locked_card`**: 410 card, `Locked` pill, 19/800 title, 15 body, 54 px deep
   button.
5. **`save_login_card`**: 370 card, "Save this login?", preview block, stacked
   fields, Save / Not now at 46 px, "Never for this app" link.
6. **`generate_prompt`**: 410 card, "Suggested password" over a 21 px mono value,
   strength pill in the header, 46 px primary/outline footer.
7. **`notice_card`**: restyled to the panel look; `show_done` = the done check
   card (green disc, drawn-on check, title, subtitle), non-activating, ~1.6 s.
   Hooked after the save-login card saves.
8. Previews (`examples/*_preview.rs`), CHANGELOG, final gate.

## Rulings

(filled in as the slice lands)

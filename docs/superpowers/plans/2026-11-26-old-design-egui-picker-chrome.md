# Old design gone: egui picker, chrome and loading (slice `egui-picker-chrome`)

Brief: `.superpowers/slice-agent-brief.md` and `.superpowers/old-design-addendum.md`.
Spec: `docs/design/deskwarden-v2-design-spec.md` Part A 2.1 / 2.4 / 3.1, Part B 4, Part C 5.4.

## Tasks

1. **Picker windows** (`picker_ui.rs`): both pickers drawn by public pure functions
   (`draw_item_picker`, `draw_app_picker`, `draw_picker_notice`) from `kit` only: a
   `card_frame` holding the title, `kit::input::text` search, and a list on `list_bg` whose
   rows are `kit::avatar` tiles with the list pane's selected ring; buttons through
   `kit::controls::button`; backend note through `kit::modal::note`; the preparing line
   through `kit::auth::progress` (no round spinner). The four `MessageBoxW` calls became
   `PickerNotice` states of the same window (same i18n text, a Close button).
   Test: the existing save-gate / source pins keep passing (the save block is moved, not
   rewritten); `loading_ui::show_while` is still the wait before either window.
2. **Window chrome** (`login_ui::draw_window_chrome_with_extra`): paints from `palette()`
   (window_bg, card, hairline, border, text_icon, text_ghost, hover) and the mark from
   `[blue_deep, blue_bright, blue_soft, blue]`, so it follows the dark palette.
   `ChromeMetrics::title_color` became `title_ink` (`Nav` / `Ink`), resolved per frame.
   Tests: the live/ghosted glyph pins now read the light palette's `text_icon` / `text_ghost`.
3. **Loading** (`loading_ui.rs`): `theme::progress_bar` -> `kit::auth::progress` (4 high, 34%
   knob, `meter_track` / `blue`); the local WARN trio -> `caution_wash/edge/mark`; every
   remaining v1 colour constant -> palette. Tests: the bar pins read 4px / 34% / palette
   colours; the "one place draws the waiting stack" guard now counts `kit::auth::progress`.
4. **Leftovers**: `scratch_window.rs` (CHECK_GREEN -> `done_mark`; the dark transcript panel's
   inks are `Palette::LIGHT` entries; refusal band -> `danger_wash/edge`), and the
   send-create banner in `vault_window/mod.rs` (stock `egui::Button` -> `kit::controls::row_button`,
   colours -> palette).
5. **Preview**: nine new `ui_preview` surfaces (`--pickers`, also in `--all`).

## Rulings

Ruling: the two pickers keep the OS's own window frame (`eframe::run_ui_native`), and only their body is v2 -- `window_host` records that "the two Add app pickers ... have the OS's own frame and nothing to round", and the foreground guards pin `run_ui_native(ADD_APP_TITLE,` / `PICK_ITEM_TITLE` in this file -- if the owner wants them frameless like the sign-in card, that is a follow-up that also moves them to `window_host::run_ui_native` with a transparent viewport.
Ruling: the empty / error states open in the SAME window (`PICK_ITEM_TITLE`, 480x400) as a `PickerNotice` instead of the item list -- one `run_ui_native` call keeps the raise guard at one -- cost if wrong: none visible; the old message boxes were modal OS dialogs with the same words.
Ruling: windows grew from 440 to 480 wide (and the list rows to 60 high) -- the v2 type roles are 1-3 points larger than the v1 sizes -- cost: a slightly larger window.
Ruling: the chrome's caption glyphs use `text_icon` (#6b6866, spec A 2.1 / B) where v1 used the lighter `TEXT_FAINT` (#7d7979), and hover uses `hover` (#f1f0ee) where v1 used CANVAS -- follows the spec's caption colour -- cost: glyphs a shade darker.
Ruling: the chrome mark is painted with palette fills rather than `theme::paint_mark`, so the dark palette lifts `blue_deep` -- cost: none in light.
Ruling: `ChromeMetrics::title_color: Color32` replaced by `title_ink: TitleInk` -- a `const` cannot hold a palette value -- cost: any external `ChromeMetrics` literal must change (there are none).
Ruling: the loading bar is the kit's 4px bar (spec C 5.4), not design 7's 3px track at 32% -- the brief names `kit` / Part C 5.4 -- cost: the bar is 1px taller and the knob 2% wider.
Ruling: `scratch_window`'s dark transcript panel is dark in both themes: its inks are `Palette::LIGHT` fields, not the context palette -- reading the context palette would invert the panel under light text in the dark theme -- cost: the panel does not follow the dark theme (it is a terminal-like readout in the design).
Ruling: the banner's Dismiss is `row_button` (40 high) in place of the 24 high stock button -- the kit has no smaller size -- cost: the banner is 16px taller.
Ruling: the picker's search field is not auto-focused (v1's was not) -- "keep behaviour identical".

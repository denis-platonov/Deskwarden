# Slice 15b: Settings, fully v2

**Goal:** every Settings page, in every state, drawn in the v2 row/card style of `docs/design/Deskwarden v2.dc.html` and spec Part C 2 (row 17x24, title 16/600, sub 14, 50 px switch, 42 px outline buttons, v2 segmented, card with the settings radius); the tray's Preferences window shows the same v2 Settings screen. Nothing 0.x did is dropped.

## Cause
Slice 7 drew the simple rows in v2 but placed the old `prefs_ui` panes (clipboard, breaches, backend, disk cache, sync, sharing, API keys, updates, shortcuts, account row) inside the pages. Those panes are built from `prefs_ui`'s own primitives (`card`, `card_row`, `row_text`, `toggle_row`, `control_row`, steppers, `scan_button`, `update_button`, `key_button`, ...) which are the 3e look (28 px controls, 14/12 px type, hairline-bordered cards, 40x22 pills). The tray window still drew the 3e nav + sections around them.

## Tasks
1. Re-point the primitives to the v2 kit: card, row padding, rule, title/sub type, switch, value, outline buttons (one `controls::button` for scan/update/key/reset/chord), steppers and text fields (42 px, v2 border), progress track, backend picker, in-card labels; palette-driven so Dark reaches them. Tests: existing pane tests keep passing; a guard that the primitives no longer use the 3e constants.
2. Tray Preferences window and every `prefs_*` preview draw `settings_v2::draw` (one Settings). `PrefsState` carries the page; `show(Section)` selects the page that owns it.
3. Pages: group titles for the multi-card panes, the Shortcuts key-chip well (spec 2.7).
4. Previews for every page and state; verify by PNG.

## Rulings

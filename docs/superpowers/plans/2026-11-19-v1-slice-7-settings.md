# Slice 7: Settings screen Implementation Plan

**Goal:** Preferences stops being a modal in the vault window and becomes a full screen (spec Part C section 2) with the 12-page v2 nav; every setting that exists today is reachable on a v2 page.

**Architecture:** `prefs_ui/settings_v2.rs` (a child module of `prefs_ui`, so it reuses the existing private pane code and constants) draws the nav + page frame, the v2 row kinds (switch, segmented, outline button, value) and the page bodies. `vault_window/mod.rs` keeps `prefs: Option<PrefsState>` as the screen's state, adds `settings_selected` (a `sidebar::Screens` flag like `generator`), draws the screen in place of the list and detail, and hands the edited settings to the daemon when the screen is left. The old nav-plus-content `draw_prefs_body` stays for the tray's standalone Preferences window.

**Spec:** `docs/design/deskwarden-v2-design-spec.md` Part C section 2.

## Page map (existing setting -> v2 page)
- General: Sidebar style (Navy/Light).
- Autofill: Show autofill prompts; Fill shortcut (shows the picker chord, goes to Shortcuts).
- Apps: Local API (service switch, API keys).
- Security: Locking (auto-lock), Privacy (clipboard clearing), Breaches (check + scan).
- Vault: Display (site icons + children, card logos, TOTP secret), backend, disk cache.
- Generator: defaults over `Settings.generator` (type, length read-out, symbols, look-alikes).
- Shortcuts: the five global chords with the capture editor.
- Account: account row, Sync (live updates, polling), Sharing.
- About: version, Updates (automatic check + the update flow).
- Notifications, Import & export, Devices: a marked-disabled row (their behaviour is slice 8/9).

## Tasks
1. Settings v2 frame: `Page`, nav, header, v2 rows, `draw`. Tests: nav order, Back, page switch, capture cancelled on page change.
2. Pages bodies and the reachability test (every existing preference label is painted on some page).
3. Wire into the vault window: `Screen::Settings`, shell request opens the screen, Back/leaving delivers, chord gate, retire modal guards.
4. ui_preview surfaces; verification.

## Rulings

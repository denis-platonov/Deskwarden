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
Ruling: Settings is a full screen in place of the list and detail (a `Screens` flag like the Generator), not a page opened by a new `Screen` route table -- `prefs: Option<PrefsState>` stays the screen's state (Some exactly while it is up) so the edited-settings cell, `deliver_if_needed` and `apply_edited_settings` are untouched -- cost if wrong: two sources of truth (flag and Option) that must agree; the end-of-frame check closes `prefs` whenever the flag is down.
Ruling: leaving the screen (Back, any other rail row, a draft opening) delivers the edited settings to the daemon at once, as the modal's Close did -- cost if wrong: one extra file write per visit.
Ruling: the vault window's chords keep working on the Settings screen; `keyboard_shortcuts_enabled` now takes "a shortcut row is recording" instead of "prefs is open" -- why: a recording row owns the next keystroke, and nothing else on a screen needs the window inert (no scrim) -- cost if wrong: a chord pressed on the screen acts behind it (Ctrl+N leaves the screen for the new draft by design).
Ruling: retired `exactly_one_block_draws_the_preferences_modal` (modal draw-order guard) for `exactly_one_block_draws_the_settings_screen`, and retargeted `the_shortcuts_are_dead_while_preferences_is_up` to `..._only_while_a_shortcut_is_being_recorded` -- why: the vault window no longer draws the modal, so a scrim order has nothing to order -- cost if wrong: none, the modal's own tests in `prefs_ui` still cover `draw_prefs_modal`.
Ruling: `draw_prefs_modal`, `draw_prefs_body` and their `prefs_ui` tests are kept; the tray's standalone Preferences window still uses the old 12-section body -- why: the lead asked for it to keep working and the previews/guards pin it -- cost if wrong: two Settings layouts until the tray window moves to the screen (not scheduled).
Ruling: only the panes' own cards stay in the 3e style (breach scan, backend, disk cache, sync, sharing, API keys, clipboard, auto-lock, shortcut editor, update flow); simple switches and choices are v2 rows -- why: the brief allows reuse of the existing pane drawing; the numeric steppers (auto-lock minutes, clipboard seconds, polling minutes) are not expressible as the spec's preset segments (5 min / 15 min / 1 hour / Never) without losing values -- cost if wrong: Security and Vault pages mix two card styles.
Ruling: Local API lives on the Apps page; Sync and Sharing on Account; Breaches on Security; Updates on About; the backend choice and disk cache on Vault -- why: the spec has no page for them, and these are the nearest subjects -- cost if wrong: a user looks for Local API under Security.
Ruling: the Generator page edits `Settings.generator` (shared with the Generator screen) and writes it through `persist_generator`; the spec's separate "genType independent of the Generator view" is not modelled -- why: one stored mode exists -- cost if wrong: the screen and the defaults cannot differ.
Ruling: `prefs_seed` now always takes `generator` from the file, even from an in-flight copy -- why: the Generator screen writes the file directly, so an in-flight copy is stale there and persist_preferences ignores that field anyway -- cost if wrong: none observed.
Ruling: the Shortcuts page shows the five existing global fill chords, not the spec's Fill/Open/Search/Generate/Lock -- why: only these are rebindable today -- cost if wrong: slice 8 adds the rest.
Ruling: spec rows with no behaviour yet are not drawn (Startup group, theme, text size, language; Autofill panel position, auto sign-in, 2FA, match-by, default steps; Apps per-app rules; lock on sleep/screen lock, hide on screen share; cards in autofill, default folder, folders, trash; notifications; import/export buttons; devices; account plan/delete, security score, Windows Hello, sync now, sign out; early updates, help, feedback, logs). Notifications, Import & export and Devices carry one marked-disabled row each so the page is not blank -- cost if wrong: three near-empty pages in the nav until slice 8/9.
Ruling: tests build a 2600 px tall frame -- why: egui paints no label below the fold, and the reachability test needs every page whole -- cost if wrong: none.
Ruling: the build ran with CARGO_TARGET_DIR on C: because E: had under 1 GB free (several worktrees' targets) -- cost: none to the tree.

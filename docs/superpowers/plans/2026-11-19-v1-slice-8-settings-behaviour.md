# Slice 8: Settings, new behaviour

**Goal:** the Settings rows slice 7 left out because nothing backed them, in the spec's priority order. Spec: Part C 2.4-2.8; decisions D2, D4, D6, D7, D8.

## Tasks
1. Settings fields (serde defaults, `persist_preferences`, exhaustive literals): `theme`, `text_size`, `lock_on_sleep`, `lock_on_screen_lock`, `hide_on_screen_share`.
2. General page: Startup (logon entry read/written through `autostart_repair`), Theme, Text size (egui zoom), Language (D6).
3. Dark palette `Palette::DARK`, `ThemeChoice`, Windows' app theme (`system_theme`), applied to the Settings screen.
4. Security: lock on sleep / screen lock (`away_lock::switched_on`), hide on screen share (`screen_share`, `WDA_EXCLUDEFROMCAPTURE`).

Not reached (see the report): security score, autofill options, About rows, Devices, Account plan/delete, Notifications, Import row.

## Rulings
Ruling: the startup switches have no `Settings` field; the registry Run value is the only copy ("open" = a value exists; "tray" = it carries `--autostart`; tray off writes `--window`, which `launch_intent` ignores and `repaired_value` does not "repair") -- why: the installer and the repair already own that value and a second copy would disagree -- cost if wrong: a tray-off entry written by this build reads as a plain window launch on older builds, which is what it means.
Ruling: the Startup rows are drawn only when the state was given the registry (`with_scan_history`, the production constructor), never in `PrefsState::new` -- why: no test may write the real Run key -- cost if wrong: previews do not show the rows.
Ruling: "Keep running when I close the window" is a read-only value row ("Always") -- why: the daemon in the tray is what runs autofill, and closing the window only ever hides it; a switch that could turn it off would need a quit-on-close path that does not exist -- cost if wrong: no way to quit by closing the window (the tray's Quit stays).
Ruling: `theme` defaults to Light, not the spec's System -- why: only the Settings screen follows the palette (below), so System on a dark Windows would change one screen of a light app on upgrade -- cost if wrong: flip `Settings::default` and the serde default to System.
Ruling: the dark palette is applied to the Settings screen (its ground now paints `window_bg`) and to nothing else; the vault list, detail pane, Generator, lock and sign-in keep `Palette::LIGHT` -- why: they draw text and fills from hard-coded `theme::` constants, and a dark palette under dark-ink text is unreadable; restyling them is out of scope -- cost if wrong: Dark changes one screen until those are converted.
Ruling: DARK keeps `blue`/`blue_bright` as fills (white text on them passes 3.5:1) and lifts `blue_deep` to a light blue because it is used as text on `blue_wash`; D2 says "the same blues" -- why: contrast tests in `kit::tokens` -- cost if wrong: hover of a primary button lightens in dark.
Ruling: text size is egui's zoom factor 1.0 / 1.125 / 1.25 for the vault window process -- why: the spec gives no values -- cost if wrong: change `TextSize::zoom_factor`.
Ruling: the lock-on-sleep / lock-on-screen-lock switches are asked IN ADDITION to the auto-lock gate (`locks_the_vault` is unchanged): auto-lock "Never" still means no lock on walking away -- why: never weaker than 0.15 -- cost if wrong: a user with auto-lock off and both switches on still is not locked on Win+L.
Ruling: hide-on-screen-share excludes the whole vault window from capture, which also hides it from the user's own screenshot tools -- why: Windows offers no per-control affinity (spec: "whole app") -- cost if wrong: switch it off.
Ruling: the Import & export page's Import row is left as it was -- why: `ShellRequest::OpenImport` (slice 9) is not in this tree; the lead wires it at merge.

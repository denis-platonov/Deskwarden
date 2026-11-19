# Deskwarden for macOS: plan

**Goal:** Deskwarden 1.x on macOS (Apple silicon and Intel), feature-for-feature with Windows where macOS allows it, from ONE codebase: everything that is not about the operating system is shared, and the macOS-only code lives in its own crate beside the Windows-only one.

**Starts after:** 1.0.0 ships on Windows. Nothing here changes Windows behaviour; phase M0–M1 are pure refactors on Windows and must leave every Windows test green.

**Hard rules carried over:** 100% compatible with official Bitwarden (same API, same item format, `deskwarden:*` custom fields only); no secret on disk outside the platform's protected store; every 1.0 feature either works on macOS or is listed below as not possible, with the reason.

## Where the code stands (measured 2026-11-19, production code only)

| | files | lines |
|---|---|---|
| Never calls Windows | 122 | ~75k |
| Calls Windows somewhere | 57 | ~91k |

Most of the 57 are portable logic with a thin Windows edge (`vault_window/mod.rs` 16k lines / 19 Windows references, `theme.rs` 10.8k / 7, `main.rs` 6.5k / 8). The genuinely Windows-shaped code is about 20 files: `panel_draw`, `win32_draw`, `prompt_card`, `picker_prompt`, `generate_prompt`, `unlock_prompt`, `save_login_card`, `notice_card`, `locked_card`, `tray_popup`, `region_overlay`, `screen_capture`, `webcam`, `injector/*`, `foreground`, `window_watch`, `window_list`, `job_object`, `single_instance`, `app_mutex`, `hello`, `autostart_repair`, `clipboard`, `screen_share`, `signature`, `bw_acquire` (installer verification), `file_picker`.

## Target shape: a Cargo workspace

```
deskwarden/                 (workspace)
  core/      deskwarden-core      no UI, no OS: vault model, rest/, crypto, sync, settings,
                                  password_gen, generator_mode, import formats, record/send logic,
                                  app_match rules, sign-in sequence model, health, i18n catalogs
  ui/        deskwarden-ui        egui screens and kit: vault window, settings, sign-in, modals,
                                  theme, previews; talks to the OS only through `platform` traits
  platform/  deskwarden-platform  the traits below + plain data types; no implementations
  win/       deskwarden-win       Windows implementations (today's Win32 code, moved)
  mac/       deskwarden-mac       macOS implementations (new)
  app/       deskwarden (bin)     main: picks win or mac with #[cfg(target_os)], wires processes
```

Rule enforced by a guard test in each shared crate: `core`, `ui` and `platform` contain no `windows::`, `objc2`, `#[cfg(target_os` or `#[cfg(windows)]`. All OS choice happens in `app` and in the two platform crates.

## The platform traits (`deskwarden-platform`)

Each is one Windows module today; each gets a macOS twin.

| Trait | Windows today | macOS |
|---|---|---|
| `FieldReader` — find the focused sign-in field, its app and window | UI Automation (`injector/ui_automation`, `window_watch`) | Accessibility API (`AXUIElement`, `AXObserver`), needs the Accessibility permission |
| `Typist` — type text and keys into the focused app | `SendInput` (`injector/send_input`) | `CGEventPost` keyboard events (same permission) |
| `AppIdentity` — which program is in front, its path, signer | process path + Authenticode (`app_identity`, `signature`) | `NSWorkspace.frontmostApplication`, bundle id, code signature (`SecStaticCode`) |
| `Panels` — the small autofill / picker / save / unlock cards near a field | layered Win32 windows (`panel_draw`, `win32_draw`, `*_prompt`, `*_card`) | borderless non-activating `NSPanel`s drawing the same egui content (one egui viewport per panel) |
| `Tray` — icon, left-click popup, right-click menu | `tray-icon` + `tray_popup` | `NSStatusItem`; the popup is an `NSPanel` under the menu-bar icon (`tray-icon` already supports macOS for the icon and menu) |
| `Hotkeys` | `RegisterHotKey` (`hotkey`) | Carbon `RegisterEventHotKey` (works without extra permission); key names show ⌘ ⇧ ⌥ |
| `BiometricUnlock` | Windows Hello + DPAPI (`hello`, `session_store`) | Touch ID via `LocalAuthentication` + a Keychain item with a biometry access control |
| `SecretStore` | DPAPI files | Keychain (generic password items, this-device-only) |
| `Clipboard` — copy, clear after N s, keep out of history | `clipboard` (exclude-from-history format) | `NSPasteboard` with the `org.nspasteboard.ConcealedType` / `TransientType` markers |
| `ScreenShareGuard` — hide the window from screen sharing | `SetWindowDisplayAffinity` (`screen_share`) | `NSWindow.sharingType = .none` |
| `RegionCapture` — drag a box to read a QR | `region_overlay`, `screen_capture` | ScreenCaptureKit (needs the Screen Recording permission); the overlay is a full-screen transparent panel |
| `Camera` | `webcam` | AVFoundation (via `nokhwa`'s macOS backend if it passes review, else a small `objc2` wrapper) |
| `SingleInstance` | named mutex (`single_instance`, `app_mutex`) | a lock file + Unix socket in `~/Library/Application Support/Deskwarden` |
| `ChildProcesses` — daemon/window processes die together | job objects (`job_object`) | process group + `kqueue NOTE_EXIT` watch on the parent |
| `Autostart` | Run key (`autostart_repair`) | `SMAppService.mainApp` login item |
| `AwayEvents` — lock on sleep / screen lock | WTS session + power messages (`away_lock`) | `NSWorkspace` sleep/screens-did-sleep and `com.apple.screenIsLocked` distributed notifications |
| `FilePicker` | `rfd`/Win32 (`file_picker`) | `rfd` (already portable) |
| `SystemTheme` | registry (`system_theme`) | `NSApp.effectiveAppearance` |
| `Updater` | own downloader + Inno installer (`updater`, `update_panel`) | Sparkle 2 (EdDSA-signed appcast), or the same own downloader replacing the `.app` — decide in M7 |
| `CliPath` — where `bw` lives / how it is verified | `bw_path`, `bw_acquire` (Authenticode) | `bw` from Bitwarden's macOS zip, verified by its Apple code signature |

## Phases (each ends green on Windows CI; from M2 also on a `macos-14` runner)

- **M0 — Workspace split on Windows, no behaviour change.** Move `core` modules first (they already touch no OS), then `ui`. Guard tests for the no-OS rule. Every existing test moves with its module. Done when `cargo test --workspace` on Windows equals today's counts.
- **M1 — Traits, Windows behind them.** Introduce `deskwarden-platform`; move each Windows module into `deskwarden-win` behind its trait, one trait per commit. `ui` and `app` call traits only. Done when the guard test passes and the 0.x/1.0 behaviour suite is unchanged.
- **M2 — macOS skeleton.** `deskwarden-mac` with stub traits; the app builds as a universal `.app`, opens the vault window, signs in (built-in client and `bw`), syncs, unlocks with the master password, locks. CI builds and runs `core` + `ui` tests on macOS.
- **M3 — Menu bar, hotkeys, single instance, autostart, away-lock, clipboard, screen-share guard.**
- **M4 — Touch ID unlock + Keychain secret store.**
- **M5 — Autofill.** AX field reading, app matching by bundle id (the shared `app_match` rules gain a bundle-id key beside the exe path; stored in the same `deskwarden:*` fields so a login matched on Windows and on Mac lives in one item), typing via CGEvent, the panels. First-run explains and requests the Accessibility permission.
- **M6 — QR from screen (Screen Recording permission), webcam, the rehearsal window.**
- **M7 — Ship.** Developer ID signing, hardened runtime, notarization, DMG, updater, release workflow job, `SCREENSHOTS.md` for macOS.

## What will differ on macOS

- **Permissions:** Accessibility (autofill) and Screen Recording (QR from screen) are user-granted in System Settings; the app must explain and deep-link (`x-apple.systempreferences:`).
- **Secure input fields:** when an app turns on Secure Event Input (Terminal password prompts, some password fields), macOS blocks synthetic typing; Deskwarden must detect it (`IsSecureEventInputEnabled`) and fall back to copy-to-clipboard with a clear note.
- **Shortcuts** shown with ⌘/⇧/⌥; default Fill shortcut ⌘⇧F per the design's macOS variant.

## Mac App Store: likely not possible for the full app

The Mac App Store requires the App Sandbox. A sandboxed app cannot read other apps' accessibility trees or post keyboard events into them, which is Deskwarden's core feature (autofill into desktop apps). App Review generally rejects Accessibility-driven control of other apps. Plan: **distribute outside the store** (Developer ID + notarization + DMG + updater), the way most desktop autofill tools do. A store build would be the vault window only (no app autofill) — decide whether that is worth listing; it is not planned here.

## Estimate

M0–M1 are the bulk of the risk (moving ~166k lines into crates without changing behaviour); M2–M7 are new code of roughly 15–25k lines. Budget in slices of the 1.0 kind: M0 2, M1 4, M2 2, M3 2, M4 1, M5 3, M6 1, M7 2 — about 17 slices.

## Open decisions for the owner

1. Updater on macOS: Sparkle 2 (standard, adds a framework) or the own downloader.
2. Whether a sandboxed, autofill-less Mac App Store build is wanted at all.
3. Apple Developer Program account ($99/year) for signing and notarization — required for any distribution macOS will open without warnings.

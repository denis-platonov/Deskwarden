# Native Windows App Autofill — Design

Date: 2026-07-28
Status: Approved for planning

## Problem

Bitwarden's autofill only works inside browsers (DOM-based, via the browser extension). Keeper offers autofill into arbitrary native Windows applications (e.g. the Mabl desktop app, Rockstar Games Launcher) by watching the foreground window, matching it to a saved login, and typing credentials in directly. This feature is missing from Bitwarden and is the gap being closed here.

Why Bitwarden doesn't already have this: its autofill is architecturally scoped to the browser extension (DOM content scripts). Native-window autofill requires OS-level integration — foreground-window hooks, window/process matching, and keystroke or UI Automation injection — a different engineering domain (Win32 systems programming) than the extension's DOM-based approach. Nothing about it is architecturally impossible; it's simply out of scope for the existing codebase.

## Goals (v1)

- Detect when a known native Windows application's window comes to the foreground.
- Fill that application's username/password fields from the user's existing Bitwarden vault.
- Let the user set up a match by picking a running process from a simple picker (like Task Manager), not by hand-writing window-title regexes.
- Support three trigger modes, configurable per saved item: auto-prompt overlay, global hotkey, fully automatic.
- Do this without modifying Bitwarden's server or client codebases.

## Non-goals (v1)

- macOS/Linux support.
- Executable path/signature verification (anti-spoofing hardening).
- Auto-submit after fill (pressing Enter/clicking Login automatically).
- Org vaults / multiple simultaneous vaults.
- Any change to Bitwarden's browser extension or desktop app.

## Architecture

Two independent pieces:

1. **Bitwarden vault (unmodified).** App-match metadata is stored in a custom field on the existing login item, e.g. field name `nodewarden:app-match`, value:
   ```json
   {"process": "RockstarGamesLauncher.exe", "trigger": "prompt"}
   ```
   This syncs through Bitwarden's normal end-to-end encrypted vault. No backend or schema changes.

2. **`nodewarden-native`** — a new Rust background/tray application for Windows. This is the only thing being built.

## Components (`nodewarden-native`)

- **Window watcher** — registers `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` to get notified on foreground-window changes. Event-driven, no polling.
- **Vault bridge** — talks to the official `bw serve` local REST API (localhost only) to read/write vault items. Owns its own unlock session, independent of the Bitwarden desktop app's unlock state:
  - Session key is DPAPI-encrypted at rest on disk.
  - Has its own lock timeout (configurable, mirrors typical vault-timeout UX).
  - Prompts its own unlock UI when the session is locked/expired.
- **Capture UI (process picker)** — a searchable list of currently running processes, similar to Task Manager's "Details" view. Used when the user adds/edits an app-match on a vault item: they pick the running process, and the app writes the `nodewarden:app-match` custom field back to that item via `bw serve`.
- **Match engine** — maintains an in-memory cache of app-match items (refreshed after vault sync / on demand). On each foreground-window change, reads the new foreground window's process name and checks it against the cache.
- **Injector** — fills matched fields using:
  1. UI Automation (preferred): walks the target window's accessibility tree for username/password `Edit` controls and sets their value via `ValuePattern`. Reliable, no timing/focus races.
  2. `SendInput` (fallback): simulates Tab/type/Tab/type keystrokes, used when the window doesn't expose a usable accessibility tree (common in some Electron UIs and game launchers).
- **Trigger** — per-item configurable, read from the `trigger` field on the match:
  - `prompt` (default): small native overlay near the target window offering Fill / Dismiss.
  - `hotkey`: user manually focuses the target field(s), then presses a configured global hotkey to fill.
  - `auto`: fills immediately on window-match with no confirmation.

## Data flow

```
Bitwarden vault item (existing schema, custom field)
   --sync-->
bw serve (local REST API)
   --queried by-->
Vault bridge (in-memory cache)
   --checked against-->
Match engine (on foreground-window change)
   --on trigger (prompt/hotkey/auto)-->
Injector (UI Automation, fallback SendInput)
   --types into-->
Target application window
```

Decrypted secrets exist only transiently in the `nodewarden-native` process's memory during a fill operation; nothing new is persisted outside Bitwarden's own vault.

## Matching semantics (v1)

- Match key: process name only (e.g. `RockstarGamesLauncher.exe`), captured via the process picker — no manual regex authoring required.
- Known limitation: process-name-only matching can be spoofed by a renamed executable. Accepted for v1 as a personal-use tool; path/signature verification is a natural v2 hardening step if this becomes a concern.

## Security notes

- No secondary secret store — all credentials remain in Bitwarden's existing encryption boundary; `nodewarden-native` only ever holds a transient, in-memory, decrypted copy needed for the current fill.
- CLI session key is DPAPI-protected at rest, with its own timeout independent of the desktop app.
- `SendInput`-based fills can occasionally be flagged by anti-cheat/DRM systems in game launchers. Known, accepted limitation of the fallback path — UI Automation is tried first specifically to minimize reliance on synthetic input.

## Testing approach

- Unit tests: match engine (process name → item lookup) and app-match field parsing/serialization.
- Manual verification against real target apps (starting with the two examples that motivated this: Mabl desktop app, Rockstar Games Launcher), covering both the UI Automation and SendInput injection paths.
- No GUI-automation CI planned for v1; this is a personal tool and manual smoke testing is sufficient.

## Open questions for later (not blocking v1)

- Multi-monitor overlay positioning details for the `prompt` trigger.
- Whether the process picker should also support "app not yet running" (pre-register by browsing to the exe) rather than only currently-running processes.

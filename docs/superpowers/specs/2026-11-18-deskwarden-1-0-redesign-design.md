# Deskwarden 1.0: the v2 redesign

## Goal

Build the v2 design ([`docs/design/Deskwarden v2.dc.html`](../../design/Deskwarden%20v2.dc.html),
broken down in [`docs/design/deskwarden-v2-design-spec.md`](../../design/deskwarden-v2-design-spec.md))
as Deskwarden 1.0. Every surface and every feature it shows is in scope. Nothing
the 0.15 app does is dropped, and everything stays compatible with official
Bitwarden servers and clients.

## How it is delivered

- Branch `v1`, from `main` at 0.15.31-dev. `main` keeps taking fixes and they
  are merged into `v1` as they land.
- One slice at a time. Each slice has its own plan in
  `docs/superpowers/plans/`, written when the slice starts. A slice ends green:
  `cargo test --lib --bins`, `cargo check --all-targets` with no warnings,
  `cargo deny`, and the `ui_preview` screenshots of every surface it touched
  compared side by side with the design.
- Between slices the app must still run end to end. A screen that is not yet
  restyled keeps its 0.15 look rather than being half-built.
- When the last slice is done, `v1` merges to `main` and is cut as **1.0.0**.

## Slices

Ordered so that each one builds on what is already done. The size column is a
rough guide (S < M < L < XL).

| # | Slice | Covers (spec part) | Depends on | Size |
|---|---|---|---|---|
| 1 | **Foundations** | v2 tokens (A §1), Navy and Light sidebar palettes, type scale, radii and shadows, keyframe timings as egui animation helpers. Shared widgets: card, field row, switch, segmented control, outline and danger buttons, `?` help bubble, hover hint with key chips, toast (B §0, C §0). v2 shield mark (D §2), regenerated `.ico` and window icon. | none | L |
| 2 | **Shell** | 1280×820 frame, sidebar (VAULT / TOOLS / FOLDERS, counts, New folder inline, Lock), top bar (search, sync pill Synced/Syncing/Offline, avatar), launch splash and intro, offline banner (A §2, C §5). The 0.15 extras (Identities, Passkeys, SSH keys, Archive, Apps, organisations, Sends) find their places. The Navy/Light setting. | 1 | L |
| 3 | **Vault list and detail** | List pane rows, tags, selection, stagger, empty and no-results states. Detail pane header and actions, field cards, TOTP ring, Fills in, card hero, notes, custom fields, open/close animation. Shared view (A §3–4). | 2 | XL |
| 4 | **Edit view** | Header and title editing, avatar menu (favicon / image / app icon / colour), login, card and note fields, live network detection, bank picker, 2FA card, Fills in, sign-in steps and Test, custom fields, history, trash (B §1). New: app-icon source and colour choice stored in `deskwarden:icon`. | 3 | XL |
| 5 | **Modals** | Modal frame, New item chooser, Ctrl K command palette (new), Share/Send and Send detail (decision D3), QR / add 2FA (B §2–8). | 2 (palette), 3–4 (send, qr) | L |
| 6 | **Tools** | Generator as its own screen, with PIN mode (new) and persisted defaults. Trash. Password health with Weak / Reused / Old (new, D5) and Breached (C §1, §3–4). | 2 | M |
| 7 | **Settings: the screen and existing settings** | Settings view with 12-page nav, row kinds, and every setting that exists today moved onto its v2 page: General, Autofill, Security, Vault, Generator, Shortcuts, About, plus Sync / Sharing / Local API / Breaches folded in (C §2). | 1, 2 | L |
| 8 | **Settings: new behaviour** | Theme Light/Dark/System (D2), text size, open at sign-in, start in tray, keep running on close, lock-on-sleep and lock-on-screen-lock toggles, hide passwords during screen share (whole app), autofill options (panel position, auto sign-in, type 2FA codes, match mode, default steps), Vault display options, Notifications (D7), Devices (list, sign out one, sign out all others), Account (plan, delete, D8), About (early updates, help, feedback, copy diagnostic logs), Security score (D4), Language (D6). | 7 | XL |
| 9 | **Import** | Import wizard (B §3): Bitwarden JSON/CSV, 1Password, Keeper, LastPass, Chrome/Edge, generic CSV, mapped to Bitwarden item types and posted with the server's import endpoint. Duplicate detection. | 5 | L |
| 10 | **Lock, sign-in, signed out** | Sign-in split screen with server choice, lock screen with the Hello stage and password stage, signed-out state (C §5). The current error states are kept and restyled. | 1 | M |
| 11 | **Autofill panel** | 1b: the panel under the focused field, Fill / No match / Locked / Save login / New password, key hints, the done check animation (D §1). Replaces the 0.15 overlay cards. | 1 | L |
| 12 | **Tray popup** | 1d: popup panel with search, recent items, copy username and password, Generate / New item / Share, status footer. Right-click native menu kept (D9). | 1, 6 | M |
| 13 | **Card and bank logos** | 1e: bundled network and bank marks (D1), text fallback, bank linked from a login, badge composition in list, detail, card face and tray (D §4). Mir added to card detection. | 3, 4 | M |
| 14 | **1.0 release** | Version 1.0.0, CHANGELOG, README and SCREENSHOTS refreshed, docs, merge `v1` → `main`, cut. | all | S |

Slices 10–13 depend only on Foundations, so they can be brought forward if
one of them is wanted sooner.

## Testing

Every slice adds tests in the crate's existing style: behaviour tests next to
the code, layout assertions where geometry is the requirement (the way
`the_update_status_is_level_with_its_button` does it), and new `ui_preview`
scenes for each new surface and state. The preview PNGs are the visual
evidence for each slice and are compared with the design artboard before the
slice is called done.

## Risks

- **Size.** Slices 3, 4 and 8 are each larger than any change made to the app
  so far. Each plan splits them further into tasks that each end green.
- **egui and the design.** Some effects (backdrop blur under modals, 12 ms
  per-character stagger, check-draw strokes) have no direct egui primitive.
  Each is drawn with the painter or simplified, and every simplification is
  noted in the slice plan.
- **New server calls** (devices, delete account, import, notifications
  polling) are each checked against the official Bitwarden API and against the
  NodeWarden server before they are built.
- **Source pins.** `job_object.rs` pins `Cargo.toml` and other files. Edits
  keep CRLF line endings and re-pin by the documented process.

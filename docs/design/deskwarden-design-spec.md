# Deskwarden design specification

Machine-readable breakdown of the Deskwarden visual identity and every UI
surface in the design. Written for AI-assisted development: everything here
is extracted from the design canvas so a session can implement or extend a
surface without re-parsing the HTML.

- **Source of truth**: [`Deskwarden.dc.html`](Deskwarden.dc.html) (open in a
  browser next to [`support.js`](support.js) to view). Exported from the
  claude.ai/design project "Deskwarden overlay design"
  (`https://claude.ai/design/p/3459a537-03e9-4e3d-a427-d54acb1acba6`).
- **Section IDs** (`2a`, `3g`, …) below match the `id=` anchors in that file.
- **Implementation**: `deskwarden/src/theme.rs` holds the tokens, mark, and
  shared widgets; surfaces implemented so far are marked in
  [Implementation status](#implementation-status).

---

## 1. Design tokens

### 1.1 Color

One blue hue in four values, warm greys for everything else. Red is reserved
for real errors only — never decoration.

| Token (theme.rs)  | Hex       | Usage |
|-------------------|-----------|-------|
| `INK`             | `#201E1D` | Primary text |
| `TEXT_SECONDARY`  | `#444141` | Secondary text, wordmark in card headers |
| `TEXT_MUTED`      | `#605D5D` | Labels, section headers inside cards |
| `TEXT_FAINT`      | `#7D7979` | Descriptions, hints, metadata |
| `TEXT_GHOST`      | `#9B9797` | Counts, placeholders, disabled-ish text |
| `CANVAS`          | `#F3F2F2` | Window/canvas background (warm grey) |
| `CARD`            | `#FFFFFF` | Card background |
| `CARD_TINT`       | `#FBFAF9` | Footers, table header rows |
| `BORDER`          | `#DEDBD9` | Card borders |
| `HAIRLINE`        | `#EAE7E7` | Separators inside cards |
| `BORDER_STRONG`   | `#D7D3D3` | Interactive-control borders (buttons, inputs) |
| `BLUE_DEEP`       | `#14307A` | Mark quadrant 1; emphasized text on blue washes; links |
| `BLUE`            | `#1B3FA0` | Mark quadrant 2; primary buttons; focused-input border; toggles-on |
| `BLUE_BRIGHT`     | `#3B74E8` | Mark quadrant 3 |
| `BLUE_SOFT`       | `#7FA4EF` | Mark quadrant 4 |
| `BLUE_WASH`       | `#EEF2FC` | Selected rows, badges, unlock panel background |
| `BLUE_EDGE`       | `#B8C7EA` | Borders on blue-washed elements; text selection |
| `FOCUS_RING`      | `#DBE4F7` | 3px ring around the focused input |
| `ERROR`           | `#B42318` | Error text only (chosen in implementation; design shows no red) |

Secondary row-separator inside cards: `#F3F2F2` (same value as CANVAS, used
as an even fainter hairline between table rows).

### 1.2 Typography

Family: **Archivo** (Google Fonts, OFL). Bundled in the app at
`deskwarden/assets/fonts/` as Regular / SemiBold / Bold; egui exposes them as
the default proportional family plus named families `Archivo-SemiBold`
(`theme::semibold`) and `Archivo-Bold` (`theme::bold`). Monospace: system
`ui-monospace` stack in the design; egui's default monospace in the app.

| Role | Size | Weight | Notes |
|------|------|--------|-------|
| Page h1 (design canvas only)    | 42px | 800 | letter-spacing −0.02em |
| Window heading ("Autofill")     | 24px | 800 | letter-spacing −0.02em |
| Item-detail title               | 21–22px | 800 | |
| Dialog/section title            | 17–18px | 800 | |
| Card row title / body strong    | 13–14px | 600–700 | |
| Body                            | 13px | 400 | line-height 1.5 |
| Description / hint              | 11–12px | 400 | `TEXT_FAINT` |
| Card header wordmark            | 11px | 700 | UPPERCASE, letter-spacing 0.1em |
| Section label (in-card)         | 11–12px | 700 | UPPERCASE, letter-spacing 0.06–0.1em, `TEXT_MUTED`/`TEXT_GHOST` |
| Keyboard hints / chips          | 10–11px | mono | |
| Monogram in avatar              | ~0.38 × tile size | 700 | |

### 1.3 Shape, elevation, spacing

| Property | Value |
|----------|-------|
| Card corner radius        | 10px (large windows 12px) |
| Row / avatar radius       | 7–8px (avatar ≈ 25% of tile size) |
| Button radius             | 7px |
| Pill radius               | 999px (fully rounded) |
| Card border               | 1px `BORDER` or `BORDER_STRONG` (floating cards) |
| Floating-card shadow      | `0 8px 20–28px rgba(45,43,43, .12–.18)` |
| Window shadow             | `0 10px 30px rgba(45,43,43, .14)` |
| Focused input             | 1px `BLUE` border + 3px `FOCUS_RING` outer ring, radius 8px, height 38px |
| Card padding              | 14–16px; card headers 9–11px vertical |
| List row padding          | 9–10px vertical, 10–12px horizontal |

---

## 2. The mark (section `3g` — "Quartered shield, four vaults, one guard")

A shield split into four quadrants — one per vault kind (logins, passkeys,
cards, notes) — in four values of one blue. Reads as a single blue shield at
icon size, resolves into quarters when large.

### 2.1 Geometry (SVG, viewBox `0 0 24 28`)

```svg
<svg viewBox="0 0 24 28" fill="none">
  <path d="M12 2H4.4A2.4 2.4 0 0 0 2 4.4V14h10Z"      fill="#14307a"/> <!-- top-left -->
  <path d="M12 2h7.6A2.4 2.4 0 0 1 22 4.4V14H12Z"     fill="#1b3fa0"/> <!-- top-right -->
  <path d="M2 14h10v12C6.6 23.2 3.2 19.4 2 14Z"       fill="#3b74e8"/> <!-- bottom-left -->
  <path d="M12 14h10c-1.2 5.4-4.6 9.2-10 12Z"         fill="#7fa4ef"/> <!-- bottom-right -->
</svg>
```

Body bounds: x ∈ [2, 22], y ∈ [2, 26]; top corners rounded r=2.4; quadrants
meet at (12, 14); bottom tip at (12, 26).

### 2.2 Variants

| Variant  | Rendering | Use |
|----------|-----------|-----|
| Full     | four blues | default, ≥14px |
| Solid    | all `#14307A` | below 16px, single-color contexts |
| Reversed | all white on `BLUE` rounded square (12px radius, 6px padding at 48px) | app-store/badge contexts |
| Ink      | all `#201E1D` | monochrome tray icons |

The full mark holds at 14px (tray, field badge, favicon).

### 2.3 Lockup

Mark + wordmark "Deskwarden" (800 weight, letter-spacing −0.03em) + tag
"FILLS NATIVE WINDOWS" (600, 11px, letter-spacing 0.16em, uppercase,
`TEXT_FAINT`). Mark at 48px beside 28px wordmark.

Implementation: `theme::paint_mark` / `theme::paint_mark_tinted` /
`theme::mark`; icon generated by `deskwarden/assets/generate-icon.py`.

---

## 3. Shared components

### 3.1 Primary button
Filled `BLUE`, white 600-weight 12–13px text, radius 7px, height 30–34px,
padding 0 12–14px. Optional trailing keyboard hint in 10px mono at 80–85%
opacity. Examples: `Search vault /`, `Save ↵`, `Fill & save to vault ↵`,
`Fill in app CTRL+⇧+F`, `+ New ⌘N`.

### 3.2 Secondary button
White fill, 1px `BORDER_STRONG` border, `INK` 600-weight text, same metrics.
Examples: `Not now`, `Copy`, `Edit`, `Cancel`, `New login CTRL+N`.

### 3.3 Keyboard chip
Small mono chip: 10px text, radius 4–5px, padding ~3×7px. On blue/selected
surfaces: white text on `BLUE`. On neutral surfaces: `TEXT_FAINT` on
`CANVAS`/white.

### 3.4 Avatar / monogram tile
Rounded square with 2-letter monogram. Neutral: `CANVAS` fill, `HAIRLINE`
border, `TEXT_MUTED` text. Emphasized/selected: `BLUE_WASH` fill (white in
already-washed rows), `BLUE_EDGE` border, `BLUE`/`BLUE_DEEP` text. Sizes:
22–44px.

### 3.5 List row
Avatar + two text lines (13px 600 title, 11px `TEXT_FAINT` subtitle),
radius 8–10px, padding ~10×10px. Selected: `BLUE_WASH` background (overlay)
or white with 1px `BLUE` border (vault list), title in `BLUE_DEEP`; may
carry a trailing keyboard chip (`↵`) or badge.

### 3.6 Badge / pill
999px radius, 10–11px 600 text, padding 2–3px × 6–9px. Blue: `BLUE_DEEP` on
`BLUE_WASH` (states: `Locked`, `Strong`, `Show list`, `app` on selected
rows). Neutral: `TEXT_SECONDARY`/`TEXT_FAINT` on `#F3F2F2` (`Best match`,
`Hotkey only`, `Never`, `2FA`).

### 3.7 Segmented control
Joined options, 1px `BORDER_STRONG` frame, radius 7px; selected segment
filled `BLUE` with white 600 text; others white with 400 text and 1px left
border. Examples: `Words | Letters | PIN`, `Below field | Above | At cursor`.

### 3.8 Toggle
40×22px pill; on: `BLUE` track, knob right; off: `#E4E2E0` track, knob
left; 18px white knob, 2px inset.

### 3.9 Card with header/footer
White card, 1px border, radius 10px. Header row: 9–11px vertical padding,
hairline below; either the Deskwarden brand header (16px mark + 11px
uppercase wordmark + right-aligned ghost status) or an uppercase section
label. Footer: `CARD_TINT` background, hairline above — houses either action
buttons or keyboard hints (11px `TEXT_FAINT`, e.g. `↑↓ Move · ↵ Fill · ⇥
Fill & submit · Esc Dismiss`).

### 3.10 Search field
Height 30–34px, radius 8px (999px in the macOS toolbar), 1px `BORDER_STRONG`
border, magnifier icon, ghost placeholder, right-aligned mono shortcut
(`CTRL+K` / `⌘K`).

### 3.11 Field badge
The 18px mark sits right-aligned inside focused login inputs, marking fields
Deskwarden can fill.

---

## 4. Surfaces

### 4.1 `2a` — Autofill overlay (Windows)

Frameless card ("3 rows, no chrome"), width 380px, anchored under the
focused field. Structure:

1. **Header**: 16px mark · `DESKWARDEN` (11px, 700, uppercase, 0.1em,
   `TEXT_SECONDARY`) · right: `3 matches` (11px `TEXT_GHOST`).
2. **Rows** (padding 6px around the group): matched credentials. First row
   selected (`BLUE_WASH`, white avatar w/ `BLUE_EDGE` border, `↵` chip).
   Row = username (13px 600) over item name (11px faint). Optional inline
   TOTP on the selected row: mono code `482 913` in `BLUE_DEEP` + 26×3px
   countdown bar (`BLUE` on `BLUE_EDGE`).
3. **Footer hints**: `↑↓ Move` `↵ Fill` `⇥ Fill & submit` `Esc Dismiss`.

Sample data: `a.novak@ledgerline.com` / Ledgerline (selected),
`deploy-bot@ledgerline.com` / Shared with me, `a.novak@ledgerline.dev` /
Ledgerline Staging. Host window: "Ledgerline Desktop — Sign in".

### 4.2 `3a` — No match

Same header, but right side shows the process name (`Atlas Licence.exe`).
Body: title **"No saved login for this app"** (14px 700), description
"Search the vault, or create an item that fills here from now on." (12px
faint), then buttons `Search vault /` (primary) + `New login CTRL+N`
(secondary). Footer hints: `/ Search` `Esc Dismiss` `Ctrl+⇧+D Never here`.

### 4.3 `3b` — Locked

Header right: `Locked` pill (`BLUE_DEEP` on `BLUE_WASH`). Body:

- Title: **"3 logins for Ledgerline Desktop"** — matches are *counted but
  never named* while locked. Subtitle: "Unlock to see and fill them."
- Unlock panel: `BLUE_WASH` rounded row — 30px white tile with padlock
  outline icon (`BLUE` stroke), "Confirm with Windows Hello" (13px 600
  `BLUE_DEEP`), right `↵` chip (white bg, `BLUE` text).
- Fallback line: "Or **enter PIN** · vault re-locks after 15 min idle"
  (12px faint; "enter PIN" in `BLUE_DEEP` 600).

### 4.4 `3c` — Save a new login

Dialog card (shadowed). Header: 16px mark + **"Save this login?"** (13px
700) + `✕` dismiss. Form rows (80px label column, 12px faint labels):

- **App**: 22px monogram + "Tracker Desktop" + mono chip `tracker.exe`.
- **Username**: bordered input, prefilled `a.novak@ledgerline.com`.
- **Password**: masked dots + `Reveal` link (11px `BLUE_DEEP` 600).
- **Folder**: select showing `Engineering ▾`.

Footer (`CARD_TINT`): `Save ↵` (primary), `Not now` (secondary), right:
"Never for this app" (12px faint text link).

### 4.5 `3d` — Generate & fill

Shown on a *password* field with no match; leads with a fresh password.

- Header row: `GENERATED` (11px 700 uppercase faint) + `Strong` pill.
- Password preview: 17px mono, digits/symbols tinted `BLUE`
  (`tq7Rvk29mzpLx4-hd8` with R/9/L/- in blue).
- Controls row: segmented `Words | Letters | PIN` · "20 chars" (12px faint)
  · right `CTRL+R NEW` (11px mono `BLUE_DEEP` 600).
- Footer: `Fill & save to vault ↵` (primary) + `Copy` (secondary).

### 4.6 `3e` — Preferences → Autofill (macOS mock)

Window 1000×700, titlebar with traffic lights (first dot `BLUE`), title
"Preferences". Left nav (208px, white): General / **Autofill** (selected:
`BLUE_WASH` + `BLUE_DEEP` 700) / Native apps / Security / Shortcuts / Sync &
account / About; bottom: "Deskwarden 1.4.0 / Bitwarden account linked".

Content: h1 "Autofill" (24px 800) + "How Deskwarden behaves when a native
login field takes focus."

**Settings card** (toggle rows, 14px 600 title + 12px faint description):
1. "Show the overlay when a login field is focused" — ON. "Off means the
   overlay only opens on the hotkey."
2. "Fill and submit on Tab" — ON. "Enter fills only; Tab also presses the
   app's default button."
3. "Require Touch ID before filling" — ON. "Windows Hello on Windows.
   Applies to passwords and one-time codes."
4. "Copy the one-time code after filling" — OFF. "Clipboard clears after 30
   seconds."
5. "Overlay position" — segmented `Below field | Above | At cursor`.
   "Falls back automatically near a screen edge."

**Per-app behaviour** table (`+ Add app` button; columns Application /
Matched by / On focus):

| Application | Matched by (mono) | On focus (pill) |
|---|---|---|
| Ledgerline Desktop | `ledgerline.exe` | Show list (blue) |
| Vantage VPN | `io.vantage.client` | Best match |
| Remote Desktop | `mstsc.exe` | Hotkey only |
| Banking apps (4) | `group` | Never |

### 4.7 `2b` / `3f` — Vault window (Windows / macOS)

Three-pane: sidebar 212px · item list 380–400px · detail.

**Toolbar** — Windows (`2b`): mark + "Deskwarden" left; right: sync pill
("● Synced 1 min ago"), `Lock CTRL+L` (secondary), avatar circle `AN`
(dark), then window controls. macOS (`3f`): traffic lights + mark left;
centered 300px pill search "Search vault ⌘K"; right `Lock ⌘L`, `+ New ⌘N`
(primary), avatar.

**Sidebar** (uppercase ghost section labels, rows with right-aligned
counts): VAULT — All items 214 · Favorites 12 · **Logins 180** (selected) ·
Passkeys 9 · Cards 4 · Secure notes 21 · Archive 18 · Trash 6. FOLDERS —
Engineering 64 · Personal 98 · Shared with me 52 · No folder 7. Bottom:
"Locks in 11:42".

**Item list**: search "Search 180 logins CTRL+K" + `+ New` (Windows);
rows = 32px monogram, name, username, optional right badge (`app`, `2FA`).
Selected row: white, 1px `BLUE` border, name in `BLUE_DEEP`, blue `app`
badge. Sample rows: Ledgerline, Atlas Studio, Vantage VPN, Git Host,
Postgres — Prod, Remote Desktop — Bastion (`CORP\anovak`), Studio Licence
Server, Tracker.

**Detail pane** (Ledgerline / Vantage VPN examples):
- Title bar: 44px monogram, item name (22px 800), "Login · Engineering"
  (12px faint), `Fill in app CTRL+⇧+F` primary (⌘⇧F on macOS), `Edit`
  secondary.
- Card "LOGIN CREDENTIALS": Username row (+`Copy`/`⌘B`), Password row
  (masked dots + `Reveal` + `Copy`/`⌘C`), One-time code row (17px mono
  `482 913`, 96×4px countdown bar, seconds left, `Copy`/`⌘T`).
- Card "AUTOFILL TARGETS": Website row (`app.ledgerline.com` + `Open`);
  Native apps row: mono pills `ledgerline.exe`, `LedgerlineSync.exe`
  (macOS: bundle ids like `io.vantage.client`), dashed `+ add` pill.
- Metadata strip: "Updated 3 days ago · Filled 41 times · Strength: strong".

---

## 5. Interaction & keyboard model

| Key | Context | Action |
|-----|---------|--------|
| `↵` Enter | overlay | Fill selected match |
| `⇥` Tab | overlay | Fill & submit (presses app's default button) |
| `↑↓` | overlay | Move selection |
| `Esc` | overlay | Dismiss |
| `/` | overlay (no match) | Search vault |
| `Ctrl+N` | overlay (no match) | New login |
| `Ctrl+⇧+D` | overlay | Never show for this app |
| `Ctrl+R` | generator | New password |
| `Ctrl+⇧+F` / `⌘⇧F` | vault detail | Fill in app |
| `Ctrl+K` / `⌘K` | vault | Focus search |
| `Ctrl+L` / `⌘L` | vault | Lock |
| `Ctrl+N` / `⌘N` | vault | New item |
| `⌘B` / `⌘C` / `⌘T` | vault detail (macOS) | Copy username / password / TOTP |

Conventions: every primary action displays its shortcut inline; Windows
mocks use `CTRL+…` text, macOS mocks use `⌘` glyphs. Matches are counted
but never named while the vault is locked. The overlay opens below the
focused field, falling back automatically near screen edges (configurable:
below / above / at cursor).

---

## Implementation status

As of 2026-07-28 (see `deskwarden/src/theme.rs` and callers):

| Design section | Status |
|---|---|
| 3g mark + palette + lockup | ✅ `theme.rs`, icon regenerated (`assets/generate-icon.py`) |
| Typography (Archivo) | ✅ bundled Regular/SemiBold/Bold, `assets/fonts/` |
| 2a overlay | ✅ `overlay_ui.rs` (single-match row; multi-row/TOTP pending vault support) |
| 3b locked wording | ✅ folded into `login_ui.rs` unlock window (overlay-locked state itself pending) |
| Login window (not in design; built from tokens + 3b language) | ✅ `login_ui.rs` |
| Pickers (not in design; built from tokens + 3e "On focus" vocabulary) | ✅ `picker_ui.rs` |
| 3a no-match overlay | ⬜ needs focus-driven overlay trigger (today the overlay only opens on a match) |
| 3c save-login prompt | ⬜ needs successful-sign-in detection + vault write flow |
| 3d generate & fill | ⬜ needs password generator + password-field detection |
| 3e preferences window | ⬜ needs a settings store; today per-app trigger lives on vault items |
| 2b/3f vault window | ⬜ needs a full vault browser (list/detail/search/TOTP) |

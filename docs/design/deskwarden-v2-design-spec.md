# Deskwarden v2 design specification

The v2 design ("Friendlier, bigger, alive") broken down for implementation.
It is what Deskwarden 1.0 is built from, on the `v1` branch. The v1 design and
its spec ([`deskwarden-design-spec.md`](deskwarden-design-spec.md)) remain the
record of what 0.x shipped.

- **Source of truth**: [`Deskwarden v2.dc.html`](Deskwarden%20v2.dc.html), next
  to [`support.js`](support.js). Exported from the claude.ai/design project
  "Deskwarden overlay design"
  (`https://claude.ai/design/p/3459a537-03e9-4e3d-a427-d54acb1acba6`). To view
  it, serve this folder (`python -m http.server --directory docs/design`) and
  open the file. A `file://` URL also works in most browsers.
- **Artboards**: `1a` desktop app, `1b` autofill, `1c` brand, `1d` tray,
  `1e` card and bank logos. These are the `id=` anchors in the file.
- **Line references** in parts A to D were taken from a copy of the file with
  its `@font-face` blocks removed. **Add 120** to get the line in
  `Deskwarden v2.dc.html`. For example, artboard `1a` is at line 80 in the
  stripped copy and line 200 in the file. Script references ("`_design.js` line
  N") count from the opening `<script type="text/x-dc">` tag, which is at line
  1637 in the file.
- **Part A**: tokens, shell, vault and shared views. **Part B**: edit view and
  modals. **Part C**: generator, settings, trash, health, lock, sign-in and
  splash. **Part D**: autofill panel, brand, tray, card and bank logos.
  **Appendix**: what the 0.15 app already has, feature by feature.

---

## 0. Rules for building from this design

These rules override anything in parts A to D that conflicts with them.

1. **Nothing the app does today is dropped.** The design draws a smaller app
   than 0.15. It has no Identities, Passkeys, SSH keys, Archive, Apps filter,
   per-organisation filters, Send sub-filters, or the Sync, Sharing, Local API
   and Breaches settings. Each of these gets the v2 look in the place where the
   design's nearest equivalent sits. For example, Identities joins the VAULT
   section and Breaches joins Security. None is removed.
2. **100% Bitwarden-compatible.** Anything the design shows that a Bitwarden
   server does not store is kept in the existing `deskwarden:*` custom-field
   convention, which other clients ignore. Examples are app matching, sign-in
   steps, item icons and the linked bank. Anything a Bitwarden server cannot do
   is listed in section 0.2 together with the decision on it. The app never
   needs a server Bitwarden does not run.
3. **Mock data is mock.** Every name, count, date, code and score in the
   design is an example. A real TOTP takes its digits and period from the item,
   not a fixed 30 s / 6. "Fill" does what the app's fill does today. The design
   shows only a toast.
4. **Windows only.** The macOS window chrome and menu-bar variants are
   ignored.
5. **Copy is verbatim** unless this section changes it. Dashes follow the app's
   rule: an em dash `\u{2014}` in UI copy.
6. **Errors and confirmations the design leaves out keep the app's current
   ones.** These cover sign-in and unlock failures, network errors, the
   two-click confirm on delete forever and on empty trash, and the discard
   confirm. Each is restyled in v2 tokens.
7. **No remote fetches the app does not already make.** The design loads logos
   from `cdn.simpleicons.org` and favicons from DuckDuckGo. The app does
   neither (see 0.2).

### 0.1 Decisions already made (2026-11-18)

| Decision | Choice |
|---|---|
| Scope of 1.0 | Every surface and feature in the design, not only a restyle |
| Sidebar | Navy and Light, chosen by a setting (General > Look), Navy by default |
| Delivery | Built on branch `v1`, slice by slice, each green and previewable; merged and released as one 1.0.0 |
| Version | 1.0.0 (the design's "2.0.1" is mock copy) |

### 0.2 Decisions still open

Each item gives a recommendation. It is decided before the slice that needs it.

| # | Question | Recommendation |
|---|---|---|
| D1 | Card-network and bank logos (1e): the design pulls 19 Simple Icons from a CDN. | Bundle those SVGs into the app. Simple Icons is CC0. Draw them offline, with the design's text tiles as the fallback. Keep the existing user-supplied `brand-marks` folder as an override. |
| D2 | Theme Light / Dark / System: the design has a Theme control but draws no dark palette. | Derive a dark palette from the v2 tokens: navy sidebar unchanged, warm greys inverted, the same four blues. Review it in previews before it ships. |
| D3 | Share / Send (B §6–7): the design shows a `send.deskwarden.app` link, a recipient field, per-field sharing and a per-open timeline. | Use the server's own Send link. Map the field chips onto the existing record Send ("advanced sharing"). Drop the recipient field: the server rejects email-only Sends (501). Build the timeline from what Bitwarden keeps: created, access count, last change, expiry, revoked. |
| D4 | Security score ("82 / 100", C §2): there is no formula in the design. | Score = 100 minus a weight per weak, reused, old or breached item, floored at 0. The "Fix N items" count is the real number of items to fix. |
| D5 | "Old" passwords (health tab): what counts as old? | The password has not changed in 365 days, using `passwordRevisionDate` (falling back to `revisionDate`). |
| D6 | Language: the design offers a Language row with only English. | Ship English only. The row shows "English" and the design's toast "More languages are on the way". |
| D7 | Notifications (shared link opened, password weak or reused, new device, update ready): Bitwarden pushes none of these to clients. | Poll what the server exposes: Send access counts, `/devices`, the health scan and the updater. Raise a Windows toast notification. |
| D8 | Account plan and delete account. | Plan comes from the profile's `premium` flag. Delete uses Bitwarden's own `DELETE /accounts` (master password required, two-step confirm), exactly as the web vault does. |
| D9 | Tray popup (1d) replaces the native tray menu. | Left click opens the popup. Right click keeps a short native menu (Open, Lock, Quit) so keyboard and accessibility users still have one. |

---

---

# Part A: tokens, shell, vault and shared views

Source: `design_v2/_stripped.html` artboard 1a (lines 80-1269) and `_design.js`. The artboard is 1280 px wide (page-level), the app window is 1280 x 820, radius 18, `overflow: hidden`, bg `#f5f4f2`, shadow `0 30px 70px rgba(32,30,29,.18), 0 0 0 1px rgba(32,30,29,.08)`. Note: v2 is a scale-up of v1 (bigger text and targets, radii 12-22). Treat all px below as logical points (egui points at 1.0 scale).

Token names refer to `deskwarden/src/theme.rs`. "NEW" means no existing token has that hex.

---

## 1. Design tokens

### 1.1 Colours used in 1a

| Hex | theme.rs token | Role in v2 |
|---|---|---|
| `#201e1d` | `INK` | Primary text; avatar "AN" fill; toast bg; hint tooltip bg; `?`-tooltip bg; dark avatar tile |
| `#444141` | `TEXT_SECONDARY` | (not used in 1a) |
| `#4a4745` | NEW (`TEXT_NAV`) | Light-sidebar nav text, settings nav text, field-label text in login form, "Sent" detail text, shared-card meta |
| `#605d5d` | `TEXT_MUTED` | Copy/eye icon buttons' resting colour |
| `#6b6866` | NEW (`TEXT_ICON`) | Close-X, window-control glyphs, `?` badge glyph, app-chip icon stroke, 2FA countdown number |
| `#7d7979` | `TEXT_FAINT` | List subtitle, "Nothing matches" sub, Shared page subtitle, detail kind label |
| `#8a8785` | NEW (`TEXT_LABEL`) | Field labels in detail cards ("Username", "2FA code", "Fills in"), search icon stroke, unfilled star stroke, shared-card clock/eye icons |
| `#9b9797` | `TEXT_GHOST` | List count, kbd-chip text, chevron stroke |
| `#ffffff` | `CARD` | Top bar bg, cards, selected row bg, search-chip |
| `#faf9f8` | NEW (`LIST_BG`) | List-pane background |
| `#f5f4f2` | NEW (`WINDOW_BG_V2`; v1 `WINDOW_BG` is `#f7f6f5`) | Window body / detail-pane background, card-avatar border colour in detail (`border: 3px solid #f5f4f2`) |
| `#ecebe9` | n/a | Design-canvas page bg only (ignore) |
| `#f3f2f0` | NEW (`FIELD_BG`) | Search-field bg, "Fills in" app chip bg, light-sidebar hover, copy-link button bg, segmented track (`#f1f0ee`) |
| `#f1f0ee` | NEW (`HOVER_GREY`) | Row hover, icon-button hover, window-control hover, segmented track |
| `#eceae7` | NEW | `?` badge bg (18px), ring track of 2FA countdown, grey avatar |
| `#e3e1de` | NEW | `?` badge bg (20px, Shared page) |
| `#dcdad7` | NEW | Search kbd-chip border, "Replay launch" border (canvas only) |
| `#e4e1dd` | NEW (`TAG_NEUTRAL_BG`) | Folder chip bg, `App` tag bg |
| `#d9d5d0` | NEW | Folder chip hover bg |
| `#2e2b29` | NEW | Folder chip text, `App` tag text |
| `#e8e6e3` | NEW (v1 `HAIRLINE` is `#eae7e7`; v2 uses this) | List-pane right border; generator chips' off border |
| `#ebe9e6` | NEW (`CARD_RING`) | 1px ring around cards (the `0 0 0 1px` of card shadow) |
| `#f1efec` | NEW (`CARD_ROW_RULE`) | Row dividers inside detail cards |
| `#e0deda` | NEW (`BORDER_V2`, near `BORDER_STRONG #d7d3d3`) | Secondary buttons' border, input borders, login form inputs |
| `#e6e3df` | NEW | Inset 1px ring on favicon avatars |
| `#1b3fa0` | `BLUE` | Primary button, selection ring, focus border, "Shared" tag bg, 2FA code ring/text |
| `#14307a` | `BLUE_DEEP` | Sidebar bg (Navy), active-nav fg (Light), "Add 2FA code" title, splash/lock bg, hover of primary on login |
| `#3b74e8` | `BLUE_BRIGHT` | Mark quadrant, lock Unlock button (hover `#2f63cf`, NEW) |
| `#7fa4ef` | `BLUE_SOFT` | Mark quadrant, "Uses your existing..." text on navy |
| `#eef2fc` | `BLUE_WASH` | Light-sidebar active bg, copy-button hover, 2FA add-icon bg, settings-nav active bg |
| `#b8c7ea` | `BLUE_EDGE` | Text selection bg, mark quadrant 4 (sidebar), subtext on navy |
| `#dbe4f7` | `FOCUS_RING` | Avatar fill (Ledgerline), light avatar |
| `#f7f8fc` | NEW | "Add 2FA code" row hover bg |
| `#f5f7fd` | NEW | Generator/Send chip on-bg |
| `#1b7a3f` | `DONE_MARK` | Sync dot (online), toast check-circle bg |
| `#17673a` | `DONE_INK` | Sync pill fg (online), `2FA` tag fg |
| `#e8f3ec` | `DONE_WASH` | Sync pill bg (online) |
| `#dff0e5` | NEW | `2FA` tag bg |
| `#fef6e7` | `CAUTION_WASH` | Offline sync pill bg, offline banner bg |
| `#8a5a06` | `CAUTION_MARK` | Offline pill fg, banner icon stroke |
| `#7a4f05` | `CAUTION_INK` | Offline banner text |
| `#f2d99b` | `CAUTION_EDGE` | Offline banner border |
| `#b7791f` | NEW (`CAUTION_DOT`) | Offline pill dot |
| `#b42318` | `ERROR` | Revoke text; 2FA code colour when `secs <= 5`; "Weak" strength |
| `#f0cfca` | NEW (`DANGER_EDGE_SOFT`; theme `DANGER_EDGE` is `#e8a9a2`) | Revoke button border, danger button border |
| `#fdf3f2` | `DANGER_WASH` | Revoke hover bg |
| `#c42b1c` | NEW | Windows close-button hover bg (white glyph) |
| `#f5b301` | NEW (`STAR`) | Filled favourite star (fill and stroke) |
| `#d9d6d2` | NEW | Switch off track |
| `#f7f6f5` | `WINDOW_BG` | Hover bg of "Log in with another device" |
| `#3b74e8` / `#2f63cf` | `BLUE_BRIGHT` / NEW | Lock-screen Unlock button |

Avatar palette (item `av` / `avFg`): `#1b3fa0/#ffffff`, `#dbe4f7/#14307a`, `#201e1d/#ffffff`, `#7fa4ef/#14307a`, `#eceae7/#201e1d` (also `#eceae7` for Git), `#3b74e8/#ffffff`, `#e8f3ec/#17673a`, `#14307a/#ffffff`, `#fef6e7/#8a5a06`. Card/bank brands override (see 4.2).

Shadows (rgba values):
- Card: `0 1px 3px rgba(32,30,29,.06), 0 0 0 1px #ebe9e6`.
- Selected list row: `0 2px 8px rgba(32,30,29,.08), 0 0 0 2px #1b3fa0` (bg `#ffffff`).
- Primary button: `0 6px 16px rgba(27,63,160,.28)`, hover `0 10px 22px rgba(27,63,160,.34)`.
- Bank-card hero: `0 18px 40px rgba(20,26,48,.28)`.
- Toast: `0 14px 34px rgba(0,0,0,.25)`. Hint tooltip: `0 8px 20px rgba(0,0,0,.25)`. `?` tooltip: `0 12px 28px rgba(0,0,0,.22)`.

### 1.2 Typography

Family: **Archivo** (as v1) for everything; monospace stack `ui-monospace, SFMono-Regular, Menlo, Consolas, monospace` for the card number and CVV on the card hero only. `-webkit-font-smoothing: antialiased`.

| Role | Size | Weight | Letter-spacing | Line-height | Where |
|---|---|---|---|---|---|
| Page title ("Shared") / detail item title | 32 | 800 | -0.03em | 1 (detail), default (Shared) | Detail header, Shared h1 |
| List-pane header | 26 | 800 | -0.02em | normal | "All items" |
| Wordmark | 20 | 800 | -0.02em | | sidebar |
| Splash text | 24 | 800 | -0.01em | | launch |
| Lock title | 32 | 800 | -0.02em | | "Welcome back, Alex" |
| Field value | 18 | 500 | 0 (plain) / 0.02em (revealed pw) / 0.12em (masked) / 0.06em (card number) | | Detail cards |
| 2FA code | 26 | 700 | 0.12em, `tabular-nums` | | |
| Primary button / list row title / empty title | 16-17 | 700 | | | Fill = 16/700; list name 16/700; "Nothing matches" 17/700 |
| Nav item / New item / Lock | 15 | 600 (New item 700) | | | |
| Search input, secondary body, "Add 2FA" sub | 15 / 14 | 400 | | | |
| List subtitle | 14 | 400 | | | |
| Field label | 13 | 600 | | | colour `#8a8785` |
| Count (nav) | 13 | 400 at opacity .7 | | | |
| Section label (VAULT/TOOLS/FOLDERS) | 12 | 700 | 0.12em, UPPERCASE | | |
| Tag (list) | 11 | 700 | | | pad 2x7, radius 6 |
| Tag (detail) | 12 | 700 | | | pad 3x9, radius 7 |
| Folder chip | 13 | 600 | | | |
| `?` badge glyph | 11 (18px badge) / 12 (20px badge) | 800 | | | |
| Tooltip (`?`) | 14 | 500 | | 1.45 (1.5 on Shared) | |
| Hint tooltip | 13 | 600 | | | key chip 11/700 |
| Sync pill / segmented | 13 / 14 | 600 / 700 | | | |

### 1.3 Radii, spacing, sizing rhythm

| Property | Value |
|---|---|
| Window | 18 |
| Cards (detail field card, Fills-in, Shared card) | 18 |
| Hero bank card | 22 |
| List row, offline banner, search field, buttons (Fill/Share/Edit/fav), login inputs | 14 |
| Nav items, New item btn, copy/eye/close icon buttons (42), pair-card icon buttons (38 -> radius 11) | 12 (New item, nav, 42px buttons); 11 (38px buttons, app chip) |
| List avatar 44 -> radius 12; detail avatar 72 -> radius 20; Shared avatar 46 -> radius 13 | |
| Tags: 6 (list), 7 (detail), pills 999 | |
| Spacing rhythm | 4 / 6 / 8 / 10 / 12 / 14 / 16 / 18 / 20 / 24 / 28 / 32 (multiples of 2; gaps 4 in list, 18 in detail stack) |
| Touch target | nav 44, buttons 48, icon buttons 42, top bar controls 34-46 |

### 1.4 Motion

| Keyframe | Definition | Used for |
|---|---|---|
| `dw-up` | opacity 0 + translateY(10px) -> none | List rows (`0.4s ease-out both`, delay `i*0.035s`), Shared view (`.35s`), Shared cards (`.4s`, delay `i*0.06s`), new-folder input (`.2s`), no-results (`.3s`), offline banner (`.3s`) |
| `dw-fade` | opacity 0 -> 1 | Vault root (`.35s ease-out`), favicon (`.25s`), hint tooltip (`.14s`) |
| `dw-pop` | opacity 0 + translateY(12px) + scale(.97) -> none | Bank-card hero (`.45s cubic-bezier(.2,.9,.3,1.15)`), lock shield (`.4s ease-out`) |
| `dw-drop` | opacity 0, translateY(-8px), scale(.98) -> none | (menus; not in my scope) |
| `dw-tip` | opacity 0 + translateY(4px) -> none | `?` tooltips (`.16s ease-out`) |
| `dw-line` | translateX(-110%) -> translateX(330%) | Splash progress bar (`1.2s ease-in-out infinite`) |
| `dw-breathe` | scale 1 -> 1.06 -> 1 | Splash shield (`1.6s ease-in-out infinite`) |
| `dw-toast` | opacity 0 + translate(-50%,14px) -> translate(-50%,0) | Toast (`.28s cubic-bezier(.2,.9,.3,1.2)`) |
| `dw-pulse-w` | box-shadow 0 0 0 0 rgba(255,255,255,.55) -> 0 0 0 22px transparent | Lock "Checking..." (`.9s ease-out infinite`) |
| `dw-pulse-b` | box-shadow 0 0 0 0 rgba(27,63,160,.45) -> 0 0 0 16px transparent | Shortcut record, Windows Hello pulse (`.9s ease-out infinite`) |
| `dw-scan`, `dw-draw`, `dw-ring`, `dw-blink` | scan 6->190px top; stroke-dashoffset 60->0; scale .6->1 + fade; caret blink 0/49% on | QR/other (other sections) |

Transitions: nav items `background .2s, color .2s`; sidebar bg `.3s`; New item `transform .15s` (hover translateY(-1px)); list row `background .2s, box-shadow .2s`; Fill button `transform .15s, box-shadow .15s` (hover translateY(-1px) + bigger shadow; `:active` translateY(1px)); secondary buttons `border-color .15s, color .15s`; fav star hover scale(1.06), active scale(.92); copy buttons `background .15s, color .15s`, active scale(.92); close X hover rotate(90deg) `transform .2s`; 2FA ring `stroke-dashoffset 1s linear, stroke .3s`; code colour `.3s`; detail pane `opacity .3s ease, transform .45s cubic-bezier(.2,.8,.2,1)`; list/detail grid `grid-template-columns .45s cubic-bezier(.2,.8,.2,1)`; Shared status pill `background .3s, color .3s`; search focus `border-color .2s, background .2s`.

Easing vocabulary: `ease-out` (entrances), `cubic-bezier(.2,.8,.2,1)` (pane layout), `cubic-bezier(.2,.9,.3,1.2)`/`(.2,.9,.3,1.15)` (overshoot pops).

---

## 2. App shell

### 2.1 Window frame

1280 x 820, flex row: **sidebar 252 px** + **main column** (flex 1). Windows chrome is drawn inside the main column's top bar (no separate title bar). macOS traffic lights (12px circles `#ff5f57` `#febc2e` `#28c840`, gap 8) exist only in the design for `isMac`; skip.

### 2.2 Sidebar (252 wide, padding `22 14 16`, gap 4, flex column, bg `sb.bg`, transition `background .3s`)

`sb` palettes (JS `renderVals`, prop `sidebar` default `Navy`):

| Key | Navy (default) | Light |
|---|---|---|
| `bg` | `#14307a` (`BLUE_DEEP`) | `#ffffff` |
| `text` (wordmark) | `#ffffff` | `#201e1d` |
| `muted` (section labels, New folder) | `rgba(255,255,255,.5)` | `#9b9797` |
| `navFg` | `rgba(255,255,255,.82)` | `#4a4745` |
| `hover` | `rgba(255,255,255,.08)` | `#f3f2f0` |
| `active` (bg) | `rgba(255,255,255,.16)` | `#eef2fc` |
| `activeFg` | `#ffffff` | `#14307a` |
| `newBg` | `#ffffff` | `#1b3fa0` |
| `newFg` | `#14307a` | `#ffffff` |
| `q1` (mark quadrant 1) | `#ffffff` | `#14307a` |

Recommendation: ship Navy as the default; Light as the alternate (the `sidebar` prop selects it; maps to a theme setting if wanted).

**Logo row**: padding `2 10 22`, gap 11. Mark 28 x 32 (viewBox `0 0 24 28`), then wordmark "Deskwarden" 20/800/-0.02em colour `sb.text`.
Mark paths in the sidebar (differs from v1 geometry: outer radius 5, body x 0.5-23.5, y 1-27.2):
- TL `M12 1H5.5A5 5 0 0 0 .5 6v8H12Z` fill `sb.q1`
- TR `M12 1h6.5a5 5 0 0 1 5 5v8H12Z` fill `#3b74e8`
- BL `M.5 14H12v13.2C5 25.4.9 21 .5 14Z` fill `#7fa4ef`
- BR `M12 14h11.5c-.4 7-4.5 11.4-11.5 13.2Z` fill `#b8c7ea`

Other mark colourways in the same file: canvas header (blue) `#14307a #1b3fa0 #3b74e8 #7fa4ef`; sign-in, lock and splash screens (reversed on navy) `#ffffff #b8c7ea #7fa4ef #3b74e8` (sign-in 34x39 with 22/800 wordmark; lock 64x75; splash 84x98).

**New item button**: height 46, radius 12, bg `sb.newBg`, text `sb.newFg` 15/700, padding `0 16`, gap 10, bottom margin 18, plus icon 18px stroke 2.4 round (`M12 5v14M5 12h14`). Hover `translateY(-1px)`. Hint `New item|Mod+N`. Click: opens "new item type" modal (`modal='newtype'`; other section).

**Scroll region** (flex 1, overflow auto, gap 2, margin `0 -6`, padding `0 6`):

Section label style: 12/700, 0.12em, UPPERCASE, colour `sb.muted`; padding `0 12 6` (VAULT) and `20 12 6` (TOOLS, FOLDERS).

Nav row (VAULT, TOOLS): height 44, radius 12, padding `0 12`, gap 12, 15/600, icon 19px (stroke 2, round caps/joins, 24x24 viewBox), label flex 1, count 13px at opacity .7. Folder row: height 40, icon 18, label ellipsis, `flex-shrink 0`. Active: bg `sb.active`, fg `sb.activeFg`, hover keeps active bg; inactive hover bg `sb.hover`.

| Section | Rows (label / count / icon / click) |
|---|---|
| VAULT | **All items** / total items / 4-square grid / filter `all` (active only if `view==='vault' && filter==='all'`)<br>**Favorites** / favourites count / star / filter `fav`<br>**Logins** / count type login / key / `login`<br>**Cards** / count type card / card / `card`<br>**Secure notes** / count type note / doc / `note`<br>**Trash** / `trash.length` / bin / `view='trash'` |
| TOOLS | **Shared** / active sends (status waiting or opened) / paper plane / `view='shared'`<br>**Generator** / (no count) / refresh arrow / `view='gen'`<br>**Settings** / (no count) / sliders / `view='settings'` |
| FOLDERS | one row per folder (`Engineering`, `Personal`, `Shared with me`) / count of items in folder / folder icon / filter `f:<name>` |

Icon path data (24x24, stroke): all `M3 3h7v7H3zM14 3h7v7h-7zM14 14h7v7h-7zM3 14h7v7H3z`; fav `M12 2.8l2.8 5.8 6.4.9-4.6 4.5 1.1 6.3L12 17.3l-5.7 3 1.1-6.3-4.6-4.5 6.4-.9z`; login `M15 7a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM12 10v11M12 15h4M12 19h3`; card `M2 6a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2zM2 10h20`; note `M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8zM14 2v6h6M16 13H8M16 17H8`; trash `M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14`; shared `M22 2 11 13M22 2l-7 20-4-9-9-4z`; gen `M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6`; settings `M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6`; folder `M4 4h5l2 3h9a1 1 0 0 1 1 1v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z`.
Counts are live (favourite toggles update Favorites count).

**New folder row** (end of FOLDERS): closed state = row height 40, radius 12, plus icon 18, text **"New folder"** 15/600 colour `sb.muted`, hover bg `sb.hover`. Click -> open state: input row height 40, radius 12, bg `sb.hover`, padding `0 12`, animation `dw-up .2s`; input 14px colour `sb.text`, transparent, autofocus, placeholder **"Folder name, then Enter"**. Enter with non-blank name: appends folder, closes, toast **"Folder created"**. Esc or blur closes without saving.

**Lock row** (pinned below scroll region): height 44, radius 12, padding `0 12`, gap 12, 15/600, colour `sb.navFg`, padlock icon 19 (`rect x4 y11 16x10 rx2` + `M8 11V7a4 4 0 0 1 8 0v4`), label **"Lock"**, hover bg `sb.hover`. Hint `Lock|Mod+L`. Click: shows lock screen (see 2.5).

### 2.3 Top bar (72 high, bg `#ffffff`, padding `0 0 0 28`, gap 14, no bottom border)

| Element | Spec |
|---|---|
| Search field | 460 x 46, radius 14, bg `#f3f2f0`, 1px transparent border -> `#1b3fa0` on focus-within; padding `0 16`, gap 10; magnifier 18px stroke `#8a8785` 2.2; input 15px `#201e1d`, placeholder **"Search your vault"**. Typing sets `q` and forces `view='vault'`. |
| Kbd chip | right inside field; text `Ctrl K` (Windows; `⌘K` mac), 12px `#9b9797`, 1px `#dcdad7` border, radius 6, padding `2 6`; hover border + text `#1b3fa0`. Hint `Quick search|Mod+K`. Click (and Ctrl/Cmd+K anywhere) opens command palette (`modal='palette'`, `pq=''`). |
| spacer | flex 1 |
| Sync pill | height 34, padding `0 12`, radius 999, gap 8, 13/600, hover `filter: brightness(.97)`, transition bg `.3s`. 7px dot + label. Hint `Sync now|Mod+Shift+Y`. |
| Avatar | 38 circle, bg `#201e1d`, white, 14/700, text **"AN"** (user initials). |
| Window controls (Windows only) | `align-self: flex-start`, text `#6b6866` 13px, three buttons 46 x 34: `—` (hover `#f1f0ee`), `▢` (hover `#f1f0ee`), `✕` (hover bg `#c42b1c`, white). Sit flush top-right (no right padding on the bar). On Mac a 14px spacer replaces them. |

Sync pill states:

| State | Label | bg | fg | dot | Trigger |
|---|---|---|---|---|---|
| Online | **Synced** | `#e8f3ec` | `#17673a` | `#1b7a3f` | default |
| Syncing | **Syncing…** | same as online | | | click while online: `syncing=true` for 1100 ms, then toast **"Everything is up to date"** |
| Offline | **Offline** | `#fef6e7` | `#8a5a06` | `#b7791f` | prop `connection==='Offline'`. Click gives toast **"Still can’t reach Bitwarden"** (curly apostrophe) |

(`syncText` "Just now"/"2 hours ago"/"Syncing…" and `syncDeg` spin are computed for other screens; not rendered in 1a shell.)

### 2.4 Launch intro (`intro()`, `loading`, `leaving`, `ready`)

Initial state `loading:true, leaving:false, ready:false`. At mount and on "Replay launch" and on sign-in: `intro()`:
- t=0: splash overlay shown.
- t=1800 ms: `leaving=true, ready=true` (overlay opacity fades to 0 over `.45s ease`; the vault view mounts, since `showVault = ready && view==='vault'`, so list rows' `dw-up` stagger plays under the fading splash).
- t=2300 ms: `loading=false` (overlay removed).

Splash overlay: absolute inset 0, bg `#14307a`, z 50, flex column centred, gap 28. Windows chrome `—  ▢(opacity .4)  ✕` top-right in `#b8c7ea` (13px, 46x34 cells). Shield 84 x 98 with `dw-breathe 1.6s ease-in-out infinite` (reversed colourway). Under it (gap 18): text 24/800/-0.01em white **"Opening your vault"** (offline: **"Opening your saved copy"**), then a 220 x 4 pill track `rgba(255,255,255,.16)` with a 34%-wide white bar running `dw-line 1.2s ease-in-out infinite`.

### 2.5 Lock overlay (shell-level; z 45)

Triggered by Lock button / palette. bg `#14307a`, flex column centred gap 26, `dw-fade .35s`; closes with `opacity` to 0 over `.4s` then removed after 420 ms. Shield 64 x 75 (`dw-pop .4s`), title **"Welcome back, Alex"** 32/800/-0.02em white, sub **"Your vault is locked"** 17px `#b8c7ea`.
- Stage `hello`: white button 62 high, padding `0 30`, radius 18, text `#14307a` 17/700, fingerprint icon 24, label **"Unlock with Windows Hello"** (macOS: "Unlock with Touch ID"); hover translateY(-2px). Click: label becomes **"Checking…"** with `dw-pulse-w` for 1300 ms, then unlocks. Below it link **"Use master password instead"** 15/600 `#b8c7ea` (hover white) -> stage `pw`.
- Stage `pw`: row width 440, gap 10: password input (flex 1, height 58, radius 16, white, padding `0 18`, 17px, placeholder **"Master password"**, Enter unlocks) + **Unlock** button (58 high, padding `0 24`, radius 16, bg `#3b74e8`, hover `#2f63cf`, white 16/700). Link under it shows the Hello label (back to `hello` stage).
(Detail of lock/sign-in screens is probably in another agent's section; included for shell completeness.)

### 2.6 Toasts

Absolute, `left 50%`, `bottom 28`, `translateX(-50%)`, height 50, padding `0 22 0 16`, radius 999, bg `#201e1d`, white 15/600, gap 10, shadow `0 14px 34px rgba(0,0,0,.25)`, z 30, anim `dw-toast .28s cubic-bezier(.2,.9,.3,1.2)`. Leading badge 26 circle bg `#1b7a3f` with a white 15px check (`M20 6 9 17l-5-5`, stroke 3, round). One toast at a time; a new toast replaces the old; auto-dismiss after **2000 ms** (timer restarts). Toast strings relevant to my surfaces:

| Trigger | Copy |
|---|---|
| Copy username | `Username copied` |
| Copy password | `Password copied` |
| Copy note | `Note copied` |
| Card: name / number / expiry / security code / ZIP / address | `Name copied`, `Card number copied`, `Expiry copied`, `Security code copied`, `ZIP copied`, `Address copied` |
| Copy 2FA | `2FA code copied` |
| Fill button | `Click the sign-in field in <first app label, else item name>` e.g. "Click the sign-in field in SAP Logon" |
| Sync click (online) | `Everything is up to date` |
| Sync click (offline) / Retry | `Still can’t reach Bitwarden` |
| Folder created | `Folder created` |
| Shared: Copy link / Revoke | `Link copied` / `Link revoked` |

### 2.7 Hover hint tooltips (`data-hint="label|keys"`)

Mechanics (JS `onOver`): global `mouseover` finds nearest `[data-hint]` with a `[data-hint-root]` ancestor (root id `app`); `mousedown` clears. `x` = element centre relative to root, clamped to `[90, rootWidth-90]`; `top`/`bottom` from element rect; if `top < 56` the tip shows **below** (`y = bottom + 8`, `translateX(-50%)`), else **above** (`y = top - 8`, `translate(-50%,-100%)`). Value split on `|`: label, then keys split on `+` (empty keys -> no chip). `Mod` -> `Ctrl` (Win)/`⌘`; `Shift` -> `Shift`/`⇧`; other tokens verbatim (`Esc`, `Del`, `N`, ...). Items with `{{` unresolved in the attribute are ignored.

Bubble: absolute, z 80, `pointer-events none`, flex row gap 8, padding `7 8 7 11`, radius 10, bg `#201e1d`, white 13/600 `nowrap`, shadow `0 8px 20px rgba(0,0,0,.25)`, `dw-fade .14s ease-out`. Key chips: gap 3, each min-width 20, height 20, padding `0 5`, radius 5, bg `rgba(255,255,255,.16)`, 11/700, centred.

Hints in my scope:

| Element | `data-hint` (label \| keys) |
|---|---|
| New item | `New item` \| Mod+N |
| Search kbd chip | `Quick search` \| Mod+K |
| Sync pill | `Sync now` \| Mod+Shift+Y |
| Lock | `Lock` \| Mod+L |
| Detail close | `Close` \| Esc |
| Fill | `Fill into the app` \| Mod+Shift+F |
| Share | `Share` \| Mod+Shift+S |
| Edit | `Edit` \| Mod+E |
| Favourite | `Favourite` \| Mod+D |
| Copy username | `Copy username` \| Mod+B |
| Copy password | `Copy password` \| Mod+C |
| Copy card number | `Copy card number` \| Mod+C |
| Copy security code | `Copy security code` \| Mod+Shift+C |
| Copy note | `Copy note` \| Mod+C |
| Other copy fields (name on card, expires, ZIP, address) | `Copy <label lowercase>` with no keys (label lowercased: `Copy name on card`, `Copy expires`, `Copy billing zip`, `Copy billing address`) |
| Reveal on any secret field | `Show <label lowercase>` / `Hide <label lowercase>` \| Mod+Shift+R, e.g. `Show password`, `Hide password`, `Show card number`, `Show security code`, `Show note` |
| 2FA copy | `Copy 2FA code` \| Mod+T |
| Card hero eye (number) | `Show card number` / `Hide card number` \| Mod+Shift+R |
| Card hero eye (CVV) | `Show security code` / `Hide security code` (no keys) |
| Linked-login pill on card | `Open linked login` (no keys) |
| Pencil on card hero | `Change linked login` \| Mod+E |
| Shared: Copy link | `Copy link` \| Mod+C |
| Shared: Revoke | `Revoke link` \| Del |
| (canvas only) Replay launch | `Replay the launch` |

Quirk: the security-code reveal hint on the pair row (`withHints`) uses `s.revCvv` for open/closed; the Card-number pair/single hint uses `s.revNum`. Everything else uses `s.reveal`.

### 2.8 `?` help tooltips (`tips.<name>`)

State `tip` holds one open key; `mouseenter` on the `?` badge sets it, `mouseleave` clears. 13 named tips: `fills, once, share, status, ovkeys, totp, look, qr, steps, only, test, apps, server` plus per-setting `set_<k>` / `st<g>_<r>` keys (settings section).

Badge: 18 x 18 (20 x 20 on Shared h1) circle, bg `#eceae7` (`#e3e1de` on Shared), glyph `?` `#6b6866` 11/800 (12 on 20px), `cursor: help`. Bubble: absolute, `left -12`, `bottom: calc(100% + 10px)` (on Shared, `top: calc(100% + 10px)`), width 250-280 (see below), padding `11 13` (`12 14` Shared), radius 12, bg `#201e1d`, white 14/500, line-height 1.45 (1.5 Shared), shadow `0 12px 28px rgba(0,0,0,.22)`, z 40, `pointer-events none`, `dw-tip .16s ease-out`.

Tips in my scope (verbatim):

| Key | Location | Width | Text |
|---|---|---|---|
| `totp` | next to "2FA code" label | 250 | The second step some sites ask for. Deskwarden types it for you after the password. |
| `fills` | next to "Fills in" label | 260 | Deskwarden suggests this login when one of these apps or sites asks you to sign in. |
| `status` | next to "Shared" h1 (opens below) | 280 | Waiting — nobody has opened it yet. Opened — the person has seen it. Links stop working when they expire or you revoke them. |
| `server` (sign-in screen) | | 260 | Most people use bitwarden.com. Pick EU if your account was made there, or Self-hosted if your company runs its own. |

---

## 3. Vault view

Visible when `ready && view==='vault'`. Root: flex 1, `min-height 0`, CSS grid, `grid-template-columns: {{vaultCols}}`, transition `grid-template-columns .45s cubic-bezier(.2,.8,.2,1)`, enters with `dw-fade .35s`.

`vaultCols`: detail open `380px 648px` (sums to 1028 = 1280-252); detail closed `1028px 0px` (list stretches). `detailOpen` defaults open (`!== false`).

### 3.1 List pane (380 wide when open; bg `#faf9f8`; right border 1px `#e8e6e3`; flex column)

**Header**: padding `24 24 14`, baseline aligned, gap 10. Title 26/800/-0.02em; count 15px `#9b9797` = number of items in current list.
Title logic: if search `q` non-empty -> **"Results"**; folder filter -> folder name; else map `all -> "All items"`, `fav -> "Favorites"`, `login -> "Logins"`, `card -> "Cards"`, `note -> "Secure notes"`.

**Offline banner** (only when `offline`): margin `0 14 10`, padding `12 14`, radius 14, bg `#fef6e7`, border 1px `#f2d99b`, gap 12, `dw-up .3s`. Left: wifi-off icon 18px stroke `#8a5a06` 2.2 (`M1 1l22 22M16.7 11.1A10.9 10.9 0 0 1 19 12.6M5 12.6a10.9 10.9 0 0 1 5.2-2.5M10.7 5A16 16 0 0 1 22.6 9M1.4 9a15.9 15.9 0 0 1 4.7-2.9M8.5 16.1a5 5 0 0 1 7 0M12 20h.01`). Middle column: **"You're offline"** 14/700 `#7a4f05` (straight apostrophe), under it **"Filling still works"** 13px `#7a4f05`. Right: **"Retry"** 14/700 `#7a4f05`, clickable -> toast `Still can’t reach Bitwarden`.

**Row list** (scroll; padding `4 14 14`, gap 4):
Row: flex, align centre, gap 14, padding 12, radius 14, cursor pointer; bg transparent -> hover `#f1f0ee`. Enter animation `dw-up .4s ease-out both`, per-row delay `i * 0.035s`.
**Selected** (`it.id === selId && detailOpen`): bg `#ffffff`, shadow `0 2px 8px rgba(32,30,29,.08), 0 0 0 2px #1b3fa0` (2px blue ring), hover stays white. When the detail pane is closed no row shows selected.
Click row: `selId = id`, `reveal/revNum/revCvv = false`, `detailOpen = true`.

Row content:
- **Avatar** 44 x 44, radius 12 (see 4.1 `visual()` for fill rules); wrapper `position: relative`.
  - Initials: 15/700, colour `avFg`.
  - Favicon: 22 x 22 `background-image`, `dw-fade .25s`; avatar bg `#ffffff` with `inset 0 0 0 1px #e6e3df` ring.
  - Brand logo image: 14 high x 36 wide (max 86%); text fallback: 13/900 white, `letter-spacing .02em`, italic for Visa.
  - Network badge (card row with a bank, or bank-linked): absolute `right -7 bottom -6`, 30 x 20, radius 6, bg = network colour, border 2px `#ffffff`, logo image 10 x 26 or text 7/900 white.
- **Text column** (flex 1, gap 3): title 16/700 single-line ellipsis; subtitle row (gap 6): username/subtitle 14px `#7d7979` ellipsis (shrinks), then **tags** (11/700, padding `2 7`, radius 6, no-shrink).

Tags (`tagsFor`), in this order:
| Tag | Condition | bg | fg |
|---|---|---|---|
| `App` | item has `app: true` | `#e4e1dd` | `#2e2b29` |
| `2FA` | `totp[item.id]` set | `#dff0e5` | `#17673a` |
| `Shared` | an active (waiting/opened) send exists for the item | `#1b3fa0` | `#ffffff` |

Filtering: `filter all` all; `f:<name>` by `FOLDER[id]`; `fav` by favourites; otherwise by `type`. Search: case-insensitive substring of `name + ' ' + user`.

**No-results**: padding `48 20`, centred column gap 8, `dw-up .3s`; title **"Nothing matches"** 17/700; sub **"Try a site, an app or a username"** 15px `#7d7979`.

### 3.2 `visual(item)` avatar logic

Inputs: item `type`, `site`, `bank`, `card.bank`, `card.number`, `ini`, `av`, `avFg`.
1. `net` = card network from card number (regex table below), only for `type==='card'`.
2. `bank` = `item.bank` or `card.bank`.
3. `mainKey` = `'b:'+bank` if bank else `net`.
4. If `mainKey` -> brand tile: bg = brand colour, logo/text white (`avFg` forced `#ffffff`). Logo is the image `https://cdn.simpleicons.org/<slug>/ffffff` (setting `cardLogos` default `Image`; `Text` mode or image-load failure uses the text label in 900 weight, italic for Visa).
5. Else if `site` and favicon didn't fail -> `https://icons.duckduckgo.com/ip3/<site>.ico` on white bg with ring `inset 0 0 0 1px #e6e3df`; initials hidden.
6. Else initials on `av`/`avFg`.
7. `hasNet = bank && net`: show the small network badge at the corner (list: 30x20; detail avatar: 42x28, radius 8, border 3px `#f5f4f2`, logo 13x34 / text 9px).
Initials rule (`initials()` for new items): two words -> first letters; one word -> first 2 chars; empty -> `?`; upper-cased.
Favicon/logo load failure falls back to initials/text (the JS pre-loads with `Image` and flags failures).

Network table (key, name, short label, long label, italic, bg, rule):
amex `#2e77bc` /^3[47]/; diners `#0079be` /^3(0[0-5]|[68])/; jcb `#0b4ea2` /^35/; visa `#1a1f71` italic /^4/; mir `#0f754e` /^220[0-4]/; mc (Mastercard) `#201e1d` /^(5[1-5]|2[2-7])/; maestro `#0a3a82` /^(50|5[6-8]|6[37])/; discover `#e65c1f` /^(6011|65)/; unionpay `#d7001f` /^62/. First match in that order wins.
Banks: chase `#117aca`, revolut `#191c1f`, hsbc `#db0011`, barclays `#00aeef`, monzo `#14233c`, wise `#163300`, citi `#003b70`, bofa `#012169`, ing `#ff6200`, santander `#ec0000`; short labels `CHASE, R, HSBC, B, M, WISE, citi, BofA, ING, S`; long label = full name.

### 3.3 Detail pane (648 wide inside a 0-648 clipped column)

Outer cell: `overflow hidden`, relative. Inner: width 648, full height, padding `28 32`, flex column, gap 18, scroll. Animation: when `detailOpen` false -> `opacity 0` + `translateX(40px)` (transition `opacity .3s ease, transform .45s cubic-bezier(.2,.8,.2,1)`), while the grid column collapses `648px -> 0px` and the list column grows `380 -> 1028`. Close: X button (see below) or `Esc` (hint) -> `detailOpen=false`. Selecting any row re-opens it.

**Header** (flex, align centre, gap 20):
- Avatar 72 x 72, radius 20 (same logic as 3.2; initials 24/800; favicon 34x34; logo 22 x 57; text 19/900).
- Title column (flex 1, gap 8): name 32/800/-0.03em, line-height 1. Under it a row (gap 8): kind label 15px `#7d7979` (`Login`, `Payment card`, `Secure note`); folder chip (if item has a folder): padding `3 9 3 7`, radius 7, bg `#e4e1dd`, 13/600 `#2e2b29`, gap 6, 13px folder icon, hover bg `#d9d5d0`, click -> `filter='f:<folder>'`; then tags (12/700, padding `3 9`, radius 7; same colours as 3.1).
- Close button: 42 x 42, radius 12, X icon 20 (stroke 2.2), colour `#6b6866`, top-aligned; hover bg `#f1f0ee` + rotate 90deg. Hint `Close|Esc`.

**Action row** (gap 10; all height 48, radius 14):
| Button | Visible | Style | Click |
|---|---|---|---|
| **Fill** | `type==='login'` only | bg `#1b3fa0`, white 16/700, padding `0 22`, gap 10, icon 18 (`M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4M10 17l5-5-5-5M15 12H3`), shadow `0 6px 16px rgba(27,63,160,.28)`; hover lift + shadow `0 10px 22px rgba(27,63,160,.34)`; active translateY(1px) | toast `Click the sign-in field in <app>` |
| **Share** | always | bg `#ffffff`, 1px `#e0deda`, 16/600, padding `0 20`, icon 18 (paper plane); hover border + text `#1b3fa0` | opens Send modal (`modal='send'`, `sendDone=false`) |
| **Edit** | always | same as Share; pencil icon `M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z` | opens Edit view |
| Favourite | always | 48 x 48 square, border 1px `#e0deda`, bg white; star 20px; hover scale 1.06, active scale .92. Fav on: fill + stroke `#f5b301`; off: no fill, stroke `#8a8785` | toggles `favs[id]` (updates sidebar Favorites count) |

**Card hero** (only `type==='card'`): 440 x 258, radius 22, padding `22 26`, bg `linear-gradient(135deg, B 0%, B 45%, #201e1d 160%)` where `B` = bank colour, else network colour, else `#14307a`; white text; shadow `0 18px 40px rgba(20,26,48,.28)`; decorative circles (right -70/top -80, 240px, `rgba(255,255,255,.07)`; right 40/bottom -120, 220px, `rgba(255,255,255,.05)`); anim `dw-pop .45s cubic-bezier(.2,.9,.3,1.15)`; hover `translateY(-3px) rotate(-.6deg)` (`transition .3s`). Rows (space-between):
1. Top (height 28): bank logo (image 24 x 62, or text 18/900 long label) + optional **linked-login pill** (height 28, padding `0 6 0 11`, radius 999, bg `rgba(255,255,255,.16)`, hover `.28`, 13/700 white, name ellipsis + chevron 14; click opens linked login: `selId=linked.id, filter='all', reveal=false`; hint `Open linked login`) and a 28 round pencil (hover bg `rgba(255,255,255,.2)`, opens Edit view, hint `Change linked login|Mod+E`); contactless icon 22 at opacity .75 on the right. Linked login = the item of `type==='login'` with the same `bank`.
2. Chip: 46 x 34, radius 7, gradient `135deg #f3d488 -> #c9a227`, inset ring `rgba(0,0,0,.12)`.
3. Number: monospace 22px, letter-spacing 0.1em, `•••• •••• •••• 4821` masked / full when revealed; 32 round eye toggle (bg `rgba(255,255,255,.14)`, hover `.28`), icon 16.
4. Bottom row (gap 16, align end): `CARD HOLDER` / `EXPIRES` / `CODE` (labels 11/700, 0.12em, uppercase, opacity .65; values 16/700). Code is mono 16/700, 0.08em, min-width 30, `•••` masked or CVV, with a 24 round eye (icon 12). At far right a 76 x 34 box with network logo (image 30 x 78 or text 20/900 long label).
Eye state `revNum` / `revCvv` is shared with the field card below and reset to false on row selection.

**Field card** (bg `#ffffff`, radius 18, card shadow, `overflow hidden`, no-shrink): rows separated by 1px `#f1efec`. Fields by type:

| Type | Fields |
|---|---|
| login | `Username` (plain), `Password` (secret) |
| card | `Name on card`; `Card number` (secret; own reveal); pair `Expires` + `Security code` (secret; own reveal); pair `Billing ZIP` + `Billing address` (`—` if empty) |
| note | `Note` (secret; masked `•` x 18) |

Value shapes: `{label, display, isSecret, spacing, onCopy, onReveal?, eye?}`; pair: `{isPair:true, a:{...}, b:{...}}`; `single` = not pair.

Single row (`f.single`): padding `16 20`, gap 14, align centre. Text col (gap 4): label 13/600 `#8a8785`; value 18/500, `white-space: nowrap`, ellipsis, letter-spacing = `spacing` (see 1.2). Right: optional **eye** button (`isSecret`), then **copy** button; both 42 x 42, radius 12, icon 20 (eye) / 19 (copy), colour `#605d5d`. Eye hover bg `#f1f0ee`; copy hover bg `#eef2fc` + text `#1b3fa0`, active scale .92. Copy icon = `rect x9 y9 12x12 rx2` + `M5 15V5a2 2 0 0 1 2-2h10`. Eye icons: open `M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z`; hidden-state `M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19M1 1l22 22` (eye-off shown while the value is revealed).
Masking: password masked as 14 `•` (`letter-spacing .12em`); revealed -> the password (`.02em`). Card number masked `•••• •••• •••• <last4>` (`.06em`); CVV masked `•••` (`.12em`); note masked 18 `•`. Name/expiry/zip/addr are never masked. Reveal resets to hidden on selecting another row. Password reveal state is `reveal`; card number `revNum`; CVV `revCvv`.

Pair row (`f.isPair`): grid 2 equal columns (`minmax(0,1fr)`), bottom border `#f1efec`; left cell has a right border `#f1efec`; each cell padding `16 12 16 20`, gap 6, min-width 0; buttons are 38 x 38, radius 11, icons 19 (eye) / 18 (copy), `flex-shrink 0`; same label/value type.

**2FA row** (login with `totp[id]`): padding `16 20`, gap 14. Left col (gap 4): row (gap 8) label **"2FA code"** 13/600 `#8a8785` + `?` tip `totp`; code `NNN NNN` 26/700, 0.12em, tabular-nums, colour `codeColor`. Ring: 40 x 40 svg rotated -90deg: track circle r 15 `#eceae7` stroke 4; progress circle r 15 stroke 4 round, `stroke-dasharray 94.25`, `stroke-dashoffset = 94.25 * (1 - secs/30)` (drains as time runs out), colour `codeColor`; centred seconds number 12/700 `#6b6866`. `codeColor` = `#b42318` when `secs <= 5` else `#1b3fa0`. Copy button 42 x 42 (hint `Copy 2FA code|Mod+T`) -> toast `2FA code copied`. Timer: `secs = 30 - floor(now/1000) % 30`; code regenerated every 30 s window (`floor(now/30000)`) from a seed; state `now` ticks each second. Real implementation: RFC 6238 TOTP from the item's secret.
**Add-2FA row** (login without `totp[id]`): row padding `16 20`, gap 14, hover bg `#f7f8fc`, cursor pointer: 42 x 42 icon tile (radius 12, bg `#eef2fc`, icon `#1b3fa0`, 20px scan glyph `M3 7V5a2 2 0 0 1 2-2h2M17 3h2a2 2 0 0 1 2 2v2M21 17v2a2 2 0 0 1-2 2h-2M7 21H5a2 2 0 0 1-2-2v-2M7 12h10`); title **"Add 2FA code"** 16/700 `#14307a`; sub **"Scan the QR code from your screen"** 14px `#7d7979`; chevron 18 `#9b9797`. Click -> QR modal (`modal='qr', qr='choose', key=''`).

**Fills in card** (login with at least one app/site): separate card (same shadow, radius 18), padding `18 20`, gap 12. Header: label **"Fills in"** 13/600 `#8a8785` + `?` tip `fills`. Chips wrap (gap 8): height 38, padding `0 14`, radius 11, bg `#f3f2f0`, 15/600, gap 9; 16px icon stroke `#6b6866`: app `M3 4h18v12H3zM8 20h8M12 16v4`, web `M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18`. Data: `apps: [[kind('app'|'web'), label]]`. Items with no apps (e.g. Office WiFi, cards, notes) hide the card.

**Notes / custom fields**: not shown on the vault detail pane in 1a (they appear only in Edit; `NOTES`, `CUSTOM` are used in the edit view). Note: the **Secure note** body appears as the single `Note` field card above.

Defaults and data shape: `ITEMS[]` = `{id, site?, name, user, type: 'login'|'card'|'note', ini, av, avFg, app?, fav?, pw?, apps?: [[kind,label]], bank?, card?: {holder, number, exp 'MM / YY', cvv, bank, zip, addr}, note?}`; `FOLDER` map id -> folder; default selection `sap`; `favs`, `totp`, `sends` state; folders default `['Engineering','Personal','Shared with me']`.

---

## 4. Shared view (`view==='shared'`)

Root: flex 1, scroll, padding `32 40`, column gap 24, `dw-up .35s`.

Header: **"Shared"** 32/800/-0.03em + 20px `?` badge (tip `status`, bubble opens below). Sub **"Links you've sent. Revoke any of them instantly."** 16px `#7d7979` (straight apostrophe).

Grid: 2 columns `repeat(2, minmax(0,1fr))`, gap 16. Each send is a card: bg `#fff`, radius 18, card shadow, padding 20, column gap 16, `dw-up .4s ease-out both` with delay `i * 0.06s`, `opacity` .62 for inactive (expired/revoked) (transition `.4s`).
- Top row (click opens send-detail modal `modal='senddetail', sendSel=id`): gap 14. Avatar 46 x 46, radius 13, 15/700 initials (item's `av/avFg`); name 17/700 + recipient (`to`) 14px `#7d7979` ellipsis; **status pill** (13/700, padding `5 11`, radius 999, bg/fg from table; transition `.3s`); chevron 18 `#9b9797`.
- Meta row (gap 24, 14px `#4a4745`): clock icon (16, stroke `#8a8785`) + expiry text; eye icon + views text.
- Actions (only when `active` = waiting or opened): gap 8. **Copy link**: flex 1, height 42, radius 12, bg `#f3f2f0`, 15/600; hover bg `#eef2fc` + text `#1b3fa0`; hint `Copy link|Mod+C`; toast `Link copied`. **Revoke**: flex 1, height 42, radius 12, 1px `#f0cfca`, text `#b42318` 15/700; hover bg `#fdf3f2`; hint `Revoke link|Del`; sets status `revoked`, exp `Revoked just now`, appends event `['Revoked by you','Just now']`, toast `Link revoked`.

Status pills:
| Status | Label | bg | fg |
|---|---|---|---|
| waiting | Waiting | `#eef2fc` | `#14307a` |
| opened | Opened | `#e8f3ec` | `#17673a` |
| expired | Expired | `#f1f0ee` | `#7d7979` |
| revoked | Revoked | `#fdf3f2` | `#b42318` |

Data shape: `sends[] = {id, item, to, status, exp, views, fields[], events: [[title, time]]}`. Example copy strings: exp `Ends in 23 h`, `Ends in 6 days`, `Ended 2 days ago`, `Revoked just now`; views `Opened once`, `3 of 10 opens`, `Not opened yet`, `Never opened`; `to`: an email or `Anyone with the link`. New sends created from the Send modal are inserted first as `{to:'Anyone with the link', status:'waiting', exp:'Ends in <expiry>', views:'Not opened yet'}`. There is no empty state for Shared in the design.

---

## 5. Behaviours index (shell-level)

| Interaction | Effect |
|---|---|
| Ctrl/Cmd+K anywhere | open command palette (`pq=''`); Esc closes any modal |
| Search typing | `q` set, `view='vault'`, list title becomes "Results" |
| Sidebar nav | sets `view`/`filter`; vault nav rows only active in vault view; Trash/Shared/Generator/Settings rows set `view` |
| Row click | select + reopen detail, resets reveals |
| Close (X / Esc) | `detailOpen=false` -> list widens to 1028 |
| Favourite | toggles `favs[id]` |
| Lock / palette "Lock now" | lock overlay |
| Replay launch (canvas) | `intro()` |
| Sign out (settings) | `signedOut` overlay (z 46); sign-in calls `intro()` |

---

## 6. Ambiguities and notes for the implementer

1. v2 tokens that do not exist in theme.rs are marked NEW; several are near-duplicates of existing greys (`#e0deda` vs `BORDER_STRONG #d7d3d3`, `#e8e6e3` vs `HAIRLINE #eae7e7`, `#f5f4f2` vs `WINDOW_BG #f7f6f5`). The design is internally consistent on the v2 values, so a theme revision (not reuse) is probably intended.
2. The tooltip text colour set (`#4a4745`, `#6b6866`, `#8a8785`) are NEW greys between TEXT_MUTED and TEXT_GHOST; either add tokens or snap to the nearest existing.
3. The ring/underline in the list selected state is a 2px ring in `BLUE` (not the BLUE_WASH of v1).
4. 2FA ring seconds: design resets code every 30 s window by wall clock; a real TOTP uses the item's period/digits (this design hard-codes 30 s, 6 digits, `NNN NNN` formatting).
5. Fill button copy is a mock toast; the real behaviour (start the native fill flow for the first associated app) is not specified here.
6. Logo images come from `cdn.simpleicons.org` and favicons from `icons.duckduckgo.com` in the design; for Bitwarden compatibility and privacy the native app should use the existing icon-server setting or local initials/text labels. Initials/text fallback path is fully specified.
7. Detail pane shows no notes/custom fields in vault view; custom fields (`CUSTOM`) and notes (`NOTES`) are edit-only in this canvas (edit view is another section).
8. `data-hint` placement computes `below` when the element top is under 56 px from the window top (top bar controls), otherwise above; x clamp 90 px from edges.
9. Windows window controls are drawn in-app (46x34 glyphs); a native app will use real caption buttons or a custom title bar equivalent; hover close `#c42b1c` is the Windows standard.

---

# Part B: edit view and modals

Source: `_stripped.html` L655-1168 + `_design.js` (`openEditView` L253, `runTest` L257, `startImport` L269, `qrPath` L201, `renderVals` L326+). Font everywhere: Archivo (body), mono = `ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`. `input/textarea { font-family: inherit; outline: none }`. Text selection `#b8c7ea`.

Note: v2 is a different visual language from v1 (bigger, rounder). Neutrals used here (not in v1 token table): border `#e0deda`, card hairline `#ebe9e6`/`#e8e6e3`, row sep `#f1efec`, hover fill `#f3f2f0`/`#f7f6f5`/`#f1f0ee`, track-off `#d9d6d2`, muted text `#8a8785`, secondary `#4a4745`, `#6b6866`, `#7d7979`, ghost `#9b9797`. Danger: text `#b42318`, border `#f0cfca`, wash `#fdf3f2`. Success: `#1b7a3f` icon, `#e8f3ec` wash, `#17673a` text. Amber: `#fef6e7` bg, `#f2d99b` border, `#7a4f05` text, `#8a5a06` icon. Secret-step tint: bg `#fdf3f2`, border `#f0cfca`, text `#8c3c33`. Blue family unchanged: `#14307a` deep, `#1b3fa0`, `#3b74e8`, `#7fa4ef`, wash `#eef2fc`, `#f5f7fd`, edge `#dbe4f7`/`#b8c7ea`.

## 0. Shared primitives

### 0.1 Keyframes (all)
| Name | Definition |
|---|---|
| `dw-up` | opacity 0->1, translateY(10px)->0 |
| `dw-fade` | opacity 0->1 |
| `dw-pop` | opacity 0->1, translateY(12px) scale(.97) -> none |
| `dw-drop` | opacity 0->1, translateY(-8px) scale(.98) -> none |
| `dw-tip` | opacity 0->1, translateY(4px)->0 |
| `dw-scan` | `top: 6px` -> `190px` |
| `dw-draw` | stroke-dashoffset 60 -> 0 (checkmark draw) |
| `dw-ring` | scale(.6) opacity 0 -> scale(1) opacity 1 |
| `dw-blink` | opacity 1 for 0-49%, 0 for 50-100% (used `1s step-end infinite`) |
| `dw-toast` | opacity 0, translate(-50%,14px) -> opacity 1, translate(-50%,0) |

### 0.2 Segmented control (`seg()` helper; used for Folder, presets, expiry)
Track: padding 4, radius 13 (modal expiry: radius 14), bg `#f1f0ee`, `align-self:flex-start` (expiry: full width, items `flex:1; text-align:center`). Option: padding 9px 15px (expiry: 10px 0), radius 10 (expiry 11), 14px/700 (expiry 15px/700), nowrap, `transition: background .2s, box-shadow .2s`. Selected: bg `#fff`, text `#201e1d`, shadow `0 1px 3px rgba(32,30,29,.14)`. Unselected: transparent, text `#6b6866`, no shadow. Click sets value.

### 0.3 Switch (`sw()`)
Track 50x30, radius 999, padding 3, bg `#1b3fa0` on / `#d9d6d2` off, `transition: background .2s`. Knob 24x24 white, shadow `0 1px 3px rgba(0,0,0,.2)`, `translateX(20px)` on / `0` off, `transition: transform .22s cubic-bezier(.3,1.4,.5,1)`.

### 0.4 "?" help tip
Anchor: 18x18 circle (QR modal: 20x20, 12px font), bg `#eceae7`, text `#6b6866`, 11px/800, `cursor:help`. Tip shown on mouse-enter, hidden on mouse-leave (state `tip` = one name; TIPS list: fills, once, share, status, ovkeys, totp, look, qr, steps, only, test, apps, server). Bubble: absolute, `left:-12px; bottom: calc(100% + 10px)` (QR: `top: calc(100% + 10px)`), width 260 (once: 250, qr: 280), padding 11px 13px, radius 12, bg `#201e1d`, text white 14px/500 lh 1.45, shadow `0 12px 28px rgba(0,0,0,.22)`, z 40, `pointer-events:none`, anim `dw-tip .16s ease-out`.

### 0.5 Hint tooltip (`data-hint="Label|Key+Key"`)
Elements with `data-hint` get a global hover tooltip (rendered by root, z 80): flex, gap 8, padding 7px 8px 7px 11px, radius 10, bg `#201e1d`, white 13px/600, nowrap, shadow `0 8px 20px rgba(0,0,0,.25)`, `dw-fade .14s`. Key chips: min-w 20, h 20, padding 0 5, radius 5, bg `rgba(255,255,255,.16)`, 11px/700. Placed centred above target (8px gap); flips below if target top < 56px. Hidden on any mousedown. `Mod` renders as `Ctrl` (Windows) / command glyph (mac), `Shift` as `Shift`/shift glyph. Hints in this section listed per control below.

### 0.6 Cards / fields (edit view)
- Card: bg `#fff`, radius 18, shadow `0 1px 3px rgba(32,30,29,.06), 0 0 0 1px #ebe9e6`, padding 22px 24px, column gap 16 (some override to 8/12). Card title 18px/800, letter-spacing -0.01em.
- Field label 14px/700 `#4a4745`, 7px gap to control.
- Text field: height 50 (password 52), radius 14, border 1px `#e0deda`, padding 0 16, input 16px (password 17px mono) transparent, borderless. Focus-within: border `#1b3fa0` (transition .2s). No focus ring in v2.
- Secondary button: h 48/50, radius 14, border 1px `#e0deda`, 16px/600, hover bg `#f7f6f5`.
- Primary button: bg `#1b3fa0`, white, 16px/700, hover bg `#14307a`.
- Tinted button: bg `#eef2fc`, text `#14307a`, 700, hover bg `#dbe4f7`.
- Danger outline: border `#f0cfca`, text `#b42318`, hover bg `#fdf3f2`.

### 0.7 Toast (global; copy used by this section)
Bottom-centre (`left:50%; bottom:28px`), h 50, padding 0 22px 0 16px, radius 999, bg `#201e1d`, white 15px/600, gap 10, shadow `0 14px 34px rgba(0,0,0,.25)`, z 30, anim `dw-toast .28s cubic-bezier(.2,.9,.3,1.2)`. Leading 26px circle `#1b7a3f` with white 15px check (stroke 3). Auto-dismiss 2000 ms; a new toast resets the timer. (Toasts are always success-styled.)

---

## 1. Edit view (`view === 'edit'`, `showEdit`)

Fills the main content area (flex column, `dw-up .35s ease-out` on entry). Reached via `openEditView(isNew, type)` from item detail Edit, New item chooser, palette "New login", Send-detail "Change password", Health "Change".

### 1.1 Header bar
Flex row, gap 16, padding 18px 32px, bottom border 1px `#e8e6e3`, bg `#fff`, non-shrinking.

| Part | Spec |
|---|---|
| Back button | 44x44, radius 13, border 1px `#e0deda`, arrow-left icon 20px stroke 2.2 (`M19 12H5M12 19l-7-7 7-7`), colour `#4a4745`; hover border+icon `#1b3fa0`. Hint "Close without saving" + `Esc`. Click = `closeEdit` (view back to 'vault', addMenu false; **no confirmation, no save**). |
| Avatar | 64x64, radius 18, bg `eAv.bg`, text `eAv.fg`, 22px/800 initials; `overflow:hidden`; `transition: transform .15s, background .25s`; hover `scale(1.04)`. Click toggles image menu. Hint "Change image" (no key). When image present, a span fills it (`background-size` per mode, `dw-fade .3s`). |
| Camera badge | Absolute right -6 / bottom -6, 28x28 circle, bg `#fff`, icon `#1b3fa0` 14px (camera path), shadow `0 2px 8px rgba(32,30,29,.2)`. Click toggles menu. Hint "Change image". |
| Hidden `<input type=file accept="image/*">` | `display:none`, triggered by "Upload image". |
| Kind label | 13px/600 `#8a8785`: `editKind`. |
| Title (view mode) | Click-to-edit row: padding 2px 8px, margin-left -8, radius 11, hover bg `#f3f2f0`, `cursor:text`, hint "Rename" + `F2`. Text 28px/800, ls -0.02em, nowrap + ellipsis; colour `#201e1d` if titled else `#9b9797`; shows `eTitle` or placeholder **"Untitled"**. Pencil chip: 30x30 radius 9, bg `#eef2fc`, icon `#1b3fa0` 15px. |
| Title (edit mode) | `<input>` autofocus, 28px/800 ls -0.02em, no border except bottom 2px `#1b3fa0`, padding 0 0 2px, transparent, width 100% max 520, colour `#201e1d`, placeholder **"Give it a name"**. Enter or Escape or blur -> back to view mode (value is live-bound on change). |
| Cancel | h 48, padding 0 22, radius 14, border 1px `#e0deda`, 16px/600; hover bg `#f7f6f5`; hint "Close without saving" + `Esc`; = `closeEdit`. |
| Save | h 48, padding 0 28, radius 14, bg `#1b3fa0`, white 16px/700, shadow `0 6px 16px rgba(27,63,160,.28)`, hover bg `#14307a`; hint "Save" + `Mod+S`. |

`editKind` copy: new -> **"New login" / "New card" / "New secure note"**; existing -> **"Editing login" / "Editing card" / "Editing secure note"**.

Save behaviour (`saveEdit`): if title trimmed is empty -> title enters edit mode + toast **"Give it a name first"**, no save. Otherwise view -> 'vault', image menu closed, toast **"Saved"**. (Mock does not persist field edits; real app must persist.)

Avatar menu opens on `openEditView` with `imgMenu:false`; `titleEditing` is true for new items only.

### 1.2 Avatar image menu (`imgMenu`)
Anchored under avatar: `left:0; top: calc(100% + 12px)`, width 300, bg `#fff`, radius 18, shadow `0 18px 44px rgba(20,26,48,.22), 0 0 0 1px rgba(20,26,48,.06)`, padding 8, z 30, gap 2, anim `dw-drop .2s`. (No click-outside handler exists in the mock; closes via selecting an option or re-clicking avatar/badge.)

| Row | Layout | Copy | Action |
|---|---|---|---|
| Upload image | row padding 10px 12px, radius 12, 15px/600, hover `#f3f2f0`; icon tile 34x34 radius 10 bg `#eef2fc` icon `#1b3fa0` 17px (upload glyph) | title **"Upload image"**, sub (13px/500 `#8a8785`) **"PNG, JPG or SVG"** | Closes menu, opens OS file picker. On pick: read as data URL, `eImg` set, `eMode='img'`, toast **"Image added"**. Image rendered `background-size: cover`, avatar bg `#f1f0ee`. |
| Use the app's icon | same; tile bg `#f1f0ee` icon `#4a4745` (monitor glyph) | title **"Use the app's icon"**, sub **"From {firstApp}"** or **"Add an app first"** | If no app in "Fills in": close menu + toast **"Add an app under Fills in first"**. Else `eMode='app'`: avatar bg `#0a6ed1`, white text = first word of first app, first 3 chars uppercased (mock; real app should extract the exe icon). |
| Section label | padding 10px 12px 4px, 12px/800, UPPERCASE ls .12em `#9b9797` | **"Colour"** | |
| 8 swatches | flex-wrap gap 8, padding 4px 12px 10px; each 28x28 circle, hover `scale(1.12)`, `transition .15s` | (no text) | Sets `eColor/eColorFg`, `eMode='initials'`, closes menu. Selected (only when mode is initials and colour matches): ring `0 0 0 2px #fff, 0 0 0 4px #1b3fa0`; unselected `inset 0 0 0 1px rgba(32,30,29,.08)`. |
| Divider + Remove image | only if `canRemove` (mode fav/img/app). Divider 1px `#f1efec` margin 2px 4px. Row colour `#b42318`, hover bg `#fdf3f2`, tile bg `#fdf3f2` with trash icon | **"Remove image"** | `eImg=null`, `eMode='initials'`, close, toast **"Image removed"** |

Swatch list (bg / fg): `#1b3fa0/#fff`, `#14307a/#fff`, `#3b74e8/#fff`, `#dbe4f7/#14307a`, `#201e1d/#fff`, `#e8f3ec/#17673a`, `#fef6e7/#8a5a06`, `#eceae7/#201e1d`.

Avatar modes (`eMode`): `'fav'` (existing item with a `site` and favicon not failed; initial for existing sited items) -> white bg, favicon background 34px contain (DuckDuckGo `ip3/{domain}.ico`); `'img'`; `'app'`; `'initials'` (default for new/no site; bg = `eColor` (new default `#1b3fa0`/white, existing = item's `av/avFg`), initials via `initials(title)`: 2 words -> first letters, else first 2 chars, uppercase; empty -> "?"). Initials update live as title is typed.

### 1.3 Body layout
Scroll area padding 24px 32px 48px. Grid `minmax(0,1fr) 320px`, gap 20, `align-items:start`. Left column: gap 18 stack of cards. Right column: gap 18.

### 1.4 Left column cards (in order)

**A. "Details"** (always)
- Title **"Details"**.
- Folder: label **"Folder"**; segmented control with options **Engineering / Personal / Shared** (`folder` state, default Engineering; reset to Engineering on open). (Note: sidebar folder is called "Shared with me"; this control says "Shared" - mock inconsistency.)
- **Login** (`editType==='login'`):
  - Username: label **"Username"**, placeholder **"Email or username"**, default value item.user (empty if new).
  - Password: label **"Password"**; field h 52, padding `0 6px 0 16px`, gap 6; input mono 17px, placeholder **"Type or generate one"**, `type=password` unless `reveal` (shared with the detail view's reveal flag). Eye button 40x40 radius 11, colour `#605d5d`, hover `#f1f0ee`, icon eye/eyeOff 19px; hint "Show or hide" + `Mod+Shift+R`. **Generate** button h 40, padding 0 14, radius 11, tinted-blue style, 14px/700, refresh icon 16px, gap 7; hint "Generate a password" + `Mod+G`; sets `editPw = make()` (20-char password from generator state) and `reveal=true`.
  - Strength meter below (padding-top 4, gap 12): track h 6 radius 999 `#eeece9`, fill width `ePw.width`, colour `ePw.color`, `transition: width .45s cubic-bezier(.2,.8,.2,1), background .3s`; right label 13px/700 width 86 right-aligned in same colour. Estimate: pool = 26 if any lowercase + 26 uppercase + 10 digit + 10 other; bits = len * log2(pool). Weak <40 `#b42318`; Fair <70 `#b7791f`; Strong <100 `#1b3fa0`; **Very strong** >=100 `#1b7a3f`. Width = clamp(bits/1.3, 6, 100)%. Empty password: label empty, colour `#d9d6d2`, width 0%.
- **Card** (`editType==='card'`): 2-col grid gap 16 of fields: **Name on card** (placeholder "As printed on the card"), **Card number** (mono, placeholder **"1234 5678 9012 3456"**), **Expires** (mono, **"MM / YY"**), **Security code** (`type=password`, mono, **"3 digits on the back"**), **Billing ZIP / postcode** (**"e.g. 10001"**), **Billing address** (**"Street, city"**).
  - Card number input is controlled: on change strip non-digits, cap at 19 digits, group in 4s with spaces (`replace(/(.{4})(?=.)/g,'$1 ')`).
  - Live brand badge inside the number field (right end, shown only when `brandOf(digits)` matches): h 28, min-w 44, padding 0 8, radius 7, bg = network colour, `dw-ring .25s`. Contents: logo image (SimpleIcons `https://cdn.simpleicons.org/{slug}/ffffff`, h 14 w 36 contain) or fallback text (11px/900, ls .02em, white; italic for Visa) if image failed to load (or card-logo prop = "Text"). Order of tests matters (first match wins):

    | Key | Name | Rule (digits) | Colour | Text fallback |
    |---|---|---|---|---|
    | amex | American Express | `^3[47]` | `#2e77bc` | AMEX |
    | diners | Diners Club | `^3(0[0-5]\|[68])` | `#0079be` | DC |
    | jcb | JCB | `^35` | `#0b4ea2` | JCB |
    | visa | Visa | `^4` | `#1a1f71` | VISA (italic) |
    | mir | Mir | `^220[0-4]` | `#0f754e` | MIR |
    | mc | Mastercard | `^(5[1-5]\|2[2-7])` | `#201e1d` | mastercard |
    | maestro | Maestro | `^(50\|5[6-8]\|6[37])` | `#0a3a82` | maestro |
    | discover | Discover | `^(6011\|65)` | `#e65c1f` | DISC |
    | unionpay | UnionPay | `^62` | `#d7001f` | UP |

  - Existing card: holder/number/exp/cvv/zip/addr prefilled from item; number also initialises `eCardNum`.
  - **Linked bank login** subsection (gap 9): label **"Linked bank login"**; flex-wrap gap 8 of chips: h 46, padding `0 14px 0 8px`, radius 13, border **2px** (`#1b3fa0` selected / `#e8e6e3`), bg (`#f5f7fd` / `#fff`), gap 10, 14px/700, `transition .2s`. Left tile 30x30 radius 8, 12px/800, bg = bank colour (None: `#eceae7`, text `#6b6866`, glyph "–"), logo image h 12 w 31 or 8px/900 text fallback. Options = **None** + every vault login that has a `bank` (data: label = login's name e.g. "Chase Business", "Revolut"; key = bank slug). Single-select, `eBank` state.
  - Helper 13px `#8a8785`: **"The bank's logo goes on the card, and its login is one click away."**
  - Bank set (for logos): chase `#117aca`, revolut `#191c1f`, hsbc `#db0011`, barclays `#00aeef`, monzo `#14233c`, wise `#163300`, citi `#003b70`, bofa `#012169`, ing `#ff6200`, santander `#ec0000` (short labels CHASE, R, HSBC, B, M, WISE, citi, BofA, ING, S).
- **Secure note** (`editType==='note'`): Details card shows only Folder; there is no dedicated note body field; the shared **Notes** card is the text area.

**B. "2FA code"** (login only)
- Title row: **"2FA code"** + "?" tip: **"The second step some sites ask for. Deskwarden types it for you after the password."**
- If item has TOTP (`sel.showTotp`): row gap 16: code 26px/700, ls .12em, tabular-nums, colour `codeColor` (`#1b3fa0`, `#b42318` when <=5 s left; `transition color .3s`), format `NNN NNN`; countdown ring 34x34 (r 15 in 40 viewBox, rotated -90deg, track `#eceae7` stroke 4, arc stroke 4 round cap, dasharray 94.25, offset `94.25*(1 - secs/30)`, `transition: stroke-dashoffset 1s linear`); spacer; **Replace** (h 42, padding 0 16, radius 12, border `#e0deda`, 15px/600, hover border+text `#1b3fa0`) opens the QR modal; **Remove** (h 42, padding 0 14, text `#b42318`, hover bg `#fdf3f2`) = set totp false + toast **"2FA code removed"** (immediate, no confirm).
- Else (`showAddTotp`): **Add 2FA code** button, h 50, padding 0 18, radius 14, tinted-blue, 16px/700, scan-frame icon 19px, gap 10, `align-self:flex-start`; opens QR modal.
- TOTP code is a mock hash of (item id + 30 s epoch window) as 6 digits; real app uses real TOTP with the stored secret.

**C. "Fills in"** (always)
- Title **"Fills in"** + tip: **"Deskwarden suggests this login when one of these apps or sites asks you to sign in."**
- Chips (flex-wrap gap 8): h 40, padding `0 6px 0 14px`, radius 12, bg `#f3f2f0`, 15px/600, gap 8, `dw-up .25s both`; text = app/site label; trailing remove button 26x26 radius 8, glyph "✕" 12px `#8a8785`, hover bg `#e6e4e1` + text `#201e1d`. Data: `editApps: string[]` (labels only, loses app-vs-web kind; seeded from `item.apps[i][1]`).
- Add row (gap 8): input field h 50, flex 1, placeholder **"Add an app or website"**; Enter or **Add** button (h 50, padding 0 20, radius 14, secondary, 16px/600) appends trimmed non-empty value and clears input. **Pick a window** button (h 50, padding 0 18, radius 14, tinted blue, 15px/700, crosshair icon 17px, gap 8): toast **"Click the window you want to add"**; after 1500 ms appends **"SAP GUI for Windows"** and toast **"Added SAP GUI for Windows"** (mock; real app should enter a window-picking mode and capture the clicked window's app).

**D. "Sign-in steps"** (login only) - sequence of keystroke steps typed on autofill
- Header row (wrap, gap 8): title **"Sign-in steps"**, tip: **"What Deskwarden presses to sign you in. Most apps work with Standard — only change it if a sign-in goes wrong."**, spacer, preset segmented control: **Standard / Password first / With 2FA**. The active preset is the one whose key list equals the current steps; if the user customises, no segment is highlighted. Clicking a preset replaces steps (copy) and clears test state.

  | Preset | Steps |
  |---|---|
  | Standard | user, tab, pw, enter |
  | Password first | pw, stab, user, enter |
  | With 2FA | user, tab, pw, enter, wait, totp, enter |

  `openEditView` default: `Standard`; but for existing item `sap` the sequence is `user, tab, wait, pw, enter` (a custom sequence, so no preset highlighted).
- Step vocabulary (`STEP`): 

  | key | Label | Icon (24-viewBox path) | Secret |
  |---|---|---|---|
  | user | **Type username** | person (`M20 21a8 8 0 0 0-16 0M12 13a5 5 0 1 0 0-10 5 5 0 0 0 0 10z`) | no |
  | pw | **Type password** | padlock (`M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4`) | yes |
  | totp | **Type 2FA code** | shield-check (`M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10zM9 12l2 2 4-4`) | no |
  | tab | **Press Tab** | arrow to bar (`M3 12h14M13 6l6 6-6 6M21 5v14`) | no |
  | stab | **Press Shift+Tab** | mirrored (`M21 12H7M11 6l-6 6 6 6M3 5v14`) | no |
  | enter | **Press Enter** | return (`M9 10l-5 5 5 5M20 4v7a4 4 0 0 1-4 4H4`) | no |
  | wait | **Wait ½ second** | hourglass (`M5 22h14M5 2h14M17 22v-4.2a2 2 0 0 0-.6-1.4L12 12l-4.4 4.4a2 2 0 0 0-.6 1.4V22M7 2v4.2a2 2 0 0 0 .6 1.4L12 12l4.4-4.4a2 2 0 0 0 .6-1.4V2`) | no |

- Step chips (flex-wrap, gap 8, aligned centre; each wrapper `dw-up .25s both`): chip h 46, padding `0 6px 0 14px`, radius 13, border **2px**, gap 9, 15px/700, icon 17px stroke 2, trailing remove "✕" button 26x26 radius 8 12px `#9b9797` (hover bg `rgba(32,30,29,.07)`, text `#201e1d`). Colours: normal border `#e8e6e3` bg `#fff` text `#201e1d`; **secret (password) step** border `#f0cfca` bg `#fdf3f2` text `#8c3c33`; **live (currently running in Test)** border `#1b3fa0` bg `#eef2fc`, `transform: scale(1.07)`, shadow `0 8px 20px rgba(27,63,160,.25)`, `transition: transform .25s cubic-bezier(.3,1.4,.5,1), box-shadow .25s, background .25s, border-color .25s`. Between chips an arrow icon 16px stroke `#c9c6c2` width 2.4 (none after last). Removing a step also cancels test highlight (`testIdx=-1`) but does not stop a running interval (mock quirk).
- Add button: 46x46, radius 13, **dashed 2px** border `#c9c6c2`, "+" 22px `#6b6866`; hover border+colour `#1b3fa0`. Opens menu: absolute `left:0; top: calc(100% + 8px)`, width 240, bg `#fff`, radius 16, shadow `0 18px 40px rgba(20,26,48,.2), 0 0 0 1px rgba(20,26,48,.06)`, padding 6, z 30, `dw-drop .2s`. Rows (h 42, radius 11, padding 0 12, gap 11, 15px/600, icon 16px, hover `#f3f2f0`; password row text `#8c3c33`) list all 7 steps in order user, pw, totp, tab, stab, enter, wait; click appends and closes menu. (No click-outside close; Test and closeEdit also close it.)
- "Only" switch row (gap 12): switch (0.3), label 15px/600 **"Only type into {editApps[0] or 'the matched app'}"**, "?" tip: **"Your password is never typed into any other window — not a chat, not a search box — even if you press the shortcut there."** State `onlyApp` default true. (Implementation: restrict typing to the matched app window.)
- **Test box**: row padding 12, radius 16, bg `#f7f6f5`, gap 12. Button **"Test it"** / **"Testing…"** (h 46, padding 0 18, radius 13, primary, 15px/700, play-triangle icon 14px filled, gap 9). Output pane: flex 1, h 46, radius 12, bg `#fff`, border 1px `#e8e6e3`, padding 0 14, mono 15px, `white-space: pre`, overflow hidden; empty text **"Fake data appears here"** in `#9b9797`, otherwise `#201e1d`; while testing a 2x20 `#201e1d` blinking caret (`dw-blink 1s step-end infinite`) follows the text. "?" tip: **"Plays the steps with fake data, so you can check them without using your real password."**
  - `runTest`: clears prior timer, `testIdx=0`, `testText=''`, closes add menu; every **620 ms** appends the next step's output and sets `testIdx` to the 1-based index (chip `testIdx-1` is "live"). Outputs: user `sample-user`, pw `••••••••`, totp `123456`, tab ` ⇥ `, stab ` ⇤ `, enter ` ⏎`, wait ` … `. After the last step (one extra tick): `testIdx=-1`, toast **"Test finished — every step worked"**. Re-clicking while running restarts. Test text remains after finishing; it is cleared only on preset change or reopening the editor. Note: it is a pure simulation (no real keystrokes); the real app should simulate to the pane only (never type real data).

**E. "Notes"** (always)
Title **"Notes"**; textarea min-h 96, radius 14, border 1px `#e0deda`, padding 14px 16px, 16px, lh 1.5, vertical resize, colour `#201e1d`, placeholder **"Anything worth remembering"**. Prefilled from item note (mock only for SAP: "Client 100 is production. Ask Maria before changing the password."). No focus-border style on textarea.

### 1.5 Right column (320 px)
**F. "Custom fields"** (card gap 8): list rows padding 11px 0, bottom border 1px `#f1efec`, `dw-up .25s both`: label 13px/600 `#8a8785`, value 16px/500 (`#201e1d`; extra new fields show **"Click to add a value"** in `#9b9797`). Data: `[label, value][]` (SAP: Client 100 / System PRD / Language EN; Ledgerline: Workspace engineering). **+ Add field** button: h 44, margin-top 8, radius 12, 1px **dashed** `#c9c6c2`, 15px/600 `#4a4745`, hover border+text `#1b3fa0`. Click appends an entry labelled **"New field"** (no real editing in mock; real app needs name/type/value inputs; Bitwarden custom field types text/hidden/boolean).

**G. "Password history"** (login only, gap 12): two static rows: mono 15px ls .12em `••••••••••` left, date 14px `#8a8785` right (**12 Mar**, **4 Jan**). Mock only.

**H. "About"** (gap 12): rows 15px, label `#8a8785` left, value right: **Created** `4 Jan 2026`; **Last changed** `3 days ago`; **Last filled** `Today, 09:14`; **Filled** `41 times`. Static in mock (should be live metadata; for new items hide or show "Just now").

**I. Move to trash**: h 50, radius 14, border 1px `#f0cfca`, text `#b42318`, 16px/700, centred, hover bg `#fdf3f2`. Click: prepends item to trash list (`when: "Deleted just now"`), view -> vault, toast **"Moved to trash"**. No confirmation (soft delete). Shown even for new items (mock quirk; hide for unsaved new items).

### 1.6 Edit state summary
`view, isNew, editType, eTitle, titleEditing, eCardNum, eBank, eMode/eImg/eColor/eColorFg, imgMenu, editPw, editApps, editAppInput, folder, steps, testIdx, testText, addMenu, onlyApp, extraFields`. Edit is a full-page view, not a modal (`mEdit` / `modal:'edit'` exists but is unused).

---

## 2. Modal frame (`anyModal`)

- Scrim: absolute inset 0 over the whole window content, bg `rgba(20,26,48,.38)`, `backdrop-filter: blur(4px)`, flex centred, z 20, `dw-fade .2s`.
- A full-size invisible click-catcher sits behind the dialog; clicking the scrim = `closeModal` (clears timers, `modal=null`, `tip=null`).
- **Esc** closes any modal (global keydown). **Ctrl/Cmd+K** opens the palette from anywhere (preventDefault) and resets query.
- Standard dialog: bg `#fff`, radius 24, shadow `0 30px 80px rgba(20,26,48,.35)`, padding 30, column gap 22, `dw-pop .3s cubic-bezier(.2,.9,.3,1.1)`, `position:relative`.
- Close "X": absolute right 18, top 18, 38x38, radius 12, glyph "✕", colour `#6b6866`, hover bg `#f1f0ee`. Title blocks reserve `padding-right` 40-44px.
- Dialog titles 26px/800 ls -0.02em; subtitle 16px `#7d7979`.
- Widths: Import 620, Palette 640 (special), New 660, Send detail 600, Send 560, QR 580.
- Modals are mutually exclusive (`modal` is a single string: import, palette, newtype, senddetail, send, qr).

---

## 3. Import wizard (`modal:'import'`, `impStep`: source -> file -> progress -> done)

Entry points: Settings > Import & export "Import" row, palette action "Import passwords". Opens with `impStep='source'`.

Titles (`impTitles`): 
| Step | Title | Subtitle |
|---|---|---|
| source | **Import passwords** | **Where are they now?** |
| file | **From {source}** | **Export a file from {source}, then add it here.** |
| progress | **Importing…** | **This takes a few seconds** |
| done | **All set** | (empty) |

(Note: on `done` the body shows its own headline; the header still reads "All set" with an empty subtitle.)

**Source step**: 3-col grid, gap 10, 6 tiles (data `IMPS` = [label, initials]): Bitwarden `BW`, 1Password `1P`, Keeper `KE`, LastPass `LP`, **Chrome or Edge** `CR`, **CSV file** `CSV`. Tile h 104, radius 16, border 2px `#e8e6e3`, column centred gap 10, `cursor:pointer`, `transition: transform .18s, border-color .18s, box-shadow .18s`; entrance `dw-up .3s both` with delay `i*0.04s`; hover `translateY(-3px)`, border `#1b3fa0`, shadow `0 10px 22px rgba(27,63,160,.12)`. Inside: 42x42 radius 12 bg `#f1f0ee` with 13px/800 initials; label 15px/700. Click -> `impSrc=label`, step file. (No logos, just monogram tiles.)

**File step**: drop zone h 220, radius 20, border 2px **dashed** `#b8c7ea`, bg `#f5f7fd`, column centred gap 10, `dw-up .3s`; hover border `#1b3fa0` + bg `#eef2fc`. Icon tile 58x58 radius 17 bg `#fff` icon `#1b3fa0` (upload, 26px) shadow `0 6px 16px rgba(27,63,160,.14)`. Copy: **"Drop the file here"** (18px/800 `#14307a`), **"or click to choose it"** (15px `#4a4745`). Click anywhere in the zone calls `startImport` (mock - no real file chosen/drag handling; real app: file dialog + drop, parse per source, then progress). Footer row: **Back** (h 48, padding 0 20, radius 14, secondary, 16px/600; -> step source) and note 14px `#8a8785` lh 1.45: **"Delete the exported file once you're done — it isn't encrypted."**

**Progress step**: padding 30px 0, centred column gap 16: big counter 56px/800 `#14307a`, ls -0.03em, tabular-nums, lh 1 (`impCount`); label **"items brought in"** (16px `#7d7979`); bar 320x5 radius 999 bg `#eeece9`, fill `#1b3fa0` width `round(n/128*100)%` (`transition: width .1s linear`). Mock timing: +3 items every 22 ms up to 128, then 300 ms later step -> done. (128 is hard-coded; real total = parsed item count.)

**Done step**: centred (padding 8px 0, gap 14): 80x80 circle `#e8f3ec` (`dw-ring .35s cubic-bezier(.2,.9,.3,1.3)`) with 40px check stroke `#1b7a3f` width 2.6, dasharray 60, `dw-draw .5s .2s ease-out both`; headline 26px/800 **"128 items imported"**; sub 16px `#7d7979` **"3 duplicates were skipped"** (both hard-coded in mock; make dynamic: "{n} items imported" / "{k} duplicates were skipped"). Buttons (gap 10, both flex 1, h 50, radius 14, 16px): **Done** (secondary, 600; closes) and **See my items** (primary, 700; closes modal, view vault, filter all).

---

## 4. Command palette (Ctrl K, `modal:'palette'`)

Not a standard dialog: width 640, `align-self:flex-start; margin-top:90px` (top-aligned in scrim), radius 22, shadow `0 30px 80px rgba(20,26,48,.35)`, `overflow:hidden`, `dw-drop .22s`. No close X.

- Search row: padding 18px 20px, gap 12, bottom border 1px `#eeece9`. Magnifier icon 20px stroke `#8a8785` width 2.2. Input autofocus, flex 1, no border, 19px, transparent, placeholder **"Search or jump to…"**. Right-side chip **"Esc"**: 12px `#9b9797`, border 1px `#dcdad7`, radius 6, padding 2px 7px.
- Results scroller: padding 8, max-h 420, overflow auto, gap 2.
- Section headings: 12px/800 UPPERCASE ls .12em `#9b9797`; padding 10px 12px 6px for **"Items"**, 12px 12px 6px for **"Actions"**. Sections only render when non-empty.
- **Items** (`palItems`): items whose `name + ' ' + user` contains the lowercased trimmed query; empty query = all; **max 5**. Row: padding 9px 12px, radius 12, gap 12, hover bg `#eef2fc`; leading 36x36 radius 10 avatar (bg = item colour or white with `inset 0 0 0 1px #e6e3df` ring when favicon; bank/network brand colour with logo when applicable; 13px/700 initials; favicon 20px contain `dw-fade .25s`); name 15px/700 + user 13px `#8a8785`. Click: close palette, `view='vault'`, `filter='all'`, select item, clear `q`.
- **Actions** (`palActions`), filtered by label contains query (case-insensitive); rows: tile 36x36 radius 10 bg `#f1f0ee` icon `#4a4745` 18px; label 15px/600 flex 1; hint 12px `#9b9797` right.

  | Label | Icon | Hint | Effect |
  |---|---|---|---|
  | New login | key | `Ctrl N` | `openEditView(true,'login')` |
  | Generate a password | refresh | `Ctrl G` | close, view gen |
  | Share {selected item name} | send | (none) | open Send modal (`sendDone=false`) |
  | Import passwords | download-tray | (none) | open Import modal at source |
  | Open settings | sliders | `Ctrl ,` | close, view settings |
  | Lock now | padlock | `Ctrl L` | close, lock screen |

  (Mac shows the command glyph instead of "Ctrl"; hints are plain strings `mod + ' N'`.)
- Empty (no items and no actions): **"Nothing found"** centred, padding 32, 15px `#8a8785`.
- No keyboard list navigation (arrow/Enter) in the mock - hover only. Real app should add Up/Down/Enter with the same `#eef2fc` highlight for the active row.

---

## 5. New item chooser (`modal:'newtype'`; `mNew`)

Standard dialog width 660. Title **"What are you adding?"** (26px/800). 3-col grid gap 12 of cards: radius 18, border 2px `#e8e6e3`, padding 20, column gap 16, `dw-up .3s both` delay `j*0.05s`; hover `translateY(-3px)`, shadow `0 12px 26px rgba(27,63,160,.14)`, border `#1b3fa0`; icon tile 52x52 radius 15 with 24px icon; title 18px/800, sub 14px `#6b6866` lh 1.45.

| Key | Title | Sub | Tile bg / icon |
|---|---|---|---|
| login | **Login** | **For an app or a website** | `#eef2fc` / `#1b3fa0` |
| card | **Card** | **Pay without hunting for your wallet** | `#e8f3ec` / `#17673a` |
| note | **Secure note** | **Text only you can read** | `#fef6e7` / `#8a5a06` |

Click -> `openEditView(true, key)` (modal closes, edit view in "New ..." mode).

---

## 6. Share / Send modal (`modal:'send'`, `mSend`)

Width 560. Opened from item detail "Share" and palette. Two states via `sendDone`.

### 6.1 Form (`sendForm`)
- Header: title **"Share {item name}"**, sub **"Send a secure link. Revoke it any time."** (padding-right 40).
- **What to share** (gap 10): label 15px/700 + "?" tip **"Only what you pick travels. The person never gets access to the rest of your vault."**. Chips (wrap gap 8): h 42, padding 0 16, radius 12, border 2px, 15px/700, gap 8, `transition: all .18s`. Options (state `sendFields`, defaults): **Username** on, **Password** on, **2FA code** off, **Notes** off. Selected: border `#1b3fa0`, bg `#f5f7fd`, text `#14307a`, leading check icon 16px (stroke 3, `dw-ring .2s`). **Password chip is red-tinted** when selected: border `#b42318`, bg `#fdf3f2`, text `#8c3c33`. Unselected: border `#e8e6e3`, bg `#fff`, text `#6b6866`. (Data fixed to those four regardless of item type.)
- **Link works for**: label 15px/700 + segmented control (full width, 0.2) options **1 hour / 1 day / 7 days / 30 days** (default `1 day`).
- **Open only once**: row; label 15px/700, tip (width 250) **"The link stops working right after it's opened, even if it hasn't expired."**, spacer, switch (default **on**).
- Recipient field: h 50, radius 14, border 1px `#e0deda`, padding 0 16, gap 10, envelope icon 18px `#8a8785`; input 15px placeholder **"Only this person can open it (optional)"** (uncontrolled; value is ignored in the mock - recipient always becomes "Anyone with the link"; real app should store/limit it).
- **Create link** button: h 54, radius 16, primary, 17px/700, shadow `0 8px 20px rgba(27,63,160,.3)`, hover `#14307a`, active `translateY(1px)`. If no field picked: toast **"Pick at least one thing to share"**. Else `sendDone=true` and a new send record is prepended: `{ item, to:'Anyone with the link', status:'waiting', exp:'Ends in {exp}', views:'Not opened yet', fields:[picked], events:[['Created by you','Just now']] }`.

### 6.2 Done (`sendDone`)
Centred column (padding 10px 0 0, gap 16): 76x76 circle `#e8f3ec` check (38px, stroke `#1b7a3f` 2.6, same ring/draw anims); **"Link ready"** (26px/800); summary 16px `#7d7979`: `{picked joined by ", "} · {exp}` plus ` · opens once` if once-switch on (e.g. "Username, Password · 1 day · opens once").
Link bar: h 56, radius 16, bg `#f3f2f0`, padding `0 8px 0 18px`, gap 12; URL mono 15px `#4a4745` ellipsis: **`send.deskwarden.app/x7Kq2mR9vB`** (hard-coded mock); **Copy** button h 42, padding 0 18, radius 12, primary 15px/700; hint "Copy link" + `Mod+C`; click toast **"Link copied"**.
Buttons (gap 10, flex 1, h 48, radius 14, secondary 16px/600): **Done** (close), **See in Shared** (close, view shared).

NOTE: the link domain `send.deskwarden.app` is a design placeholder. For Bitwarden compatibility this must use the official Bitwarden Send (`bitwarden.com`/server `/#/send/...` URLs), with Bitwarden Send options (max access count, expiry, password, hide email) - the design's "Open only once" = max access count 1, and expiry maps to deletion date.

---

## 7. Send detail (`modal:'senddetail'`, `mSendDetail`)

Opened by clicking a row on the Shared screen. Width 600. Data: `sends: { id, item, to, status('waiting'|'opened'|'expired'|'revoked'), exp, views, fields[], events: [title, time][] }`.

Status pill table (`STATUS`): Waiting bg `#eef2fc` fg `#14307a`; Opened `#e8f3ec`/`#17673a`; Expired `#f1f0ee`/`#7d7979`; Revoked `#fdf3f2`/`#b42318`. Pill: 13px/700, padding 5px 11px, radius 999.

- Header (gap 16, padding-right 44): 56x56 radius 16 avatar (item colours, 18px/800 initials); name 24px/800 ls -0.02em; recipient (`to`) 15px `#7d7979`; status pill at right.
- Facts grid: 3 cols gap 10, tiles radius 14, bg `#f7f6f5`, padding 14px 16px, label 13px/600 `#8a8785`, value 16px/700. Labels: **Opens** (`views`, e.g. "Opened once", "3 of 10 opens", "Not opened yet", "Never opened"), **Link** (`exp`, e.g. "Ends in 23 h", "Ended 2 days ago", "Revoked just now"), **Includes** (fields joined ", ", or "—").
- **Activity** timeline: heading 15px/800 (padding-bottom 12). Each event row gap 14 (`dw-up .3s both`): left rail 12px wide with dot 11x11 (margin-top 5; last event `#1b3fa0`, earlier `#c9c6c2`) and a 2px connector (`#ebe9e6`, min-h 22) to the next event; text title 15px/600 + time 14px `#8a8785`, padding-bottom 14. Sample events: "Created by you", "Link sent to m.reyes", "Opened in Edge · Berlin", "Opened 3 times", "Expired unopened", "Revoked by you", "Extended by a day". Times like "16 Aug, 14:20", "Today, 08:40", "Last today, 10:12", "Just now".
- **Warning** (only when status is `opened` AND fields include Password): row padding 14px 16px, radius 16, bg `#fef6e7`, border 1px `#f2d99b`, gap 14; triangle icon 20px `#8a5a06`; title 15px/700 `#7a4f05` **"Someone has seen this password"**; body 14px `#7a4f05` **"Change it once they're done."**; button **"Change password"** (h 40, padding 0 14, radius 11, bg `#fff`, border `#f2d99b`, 14px/700 `#7a4f05`; no hover style) -> close modal, select that item, open edit view.
- **Actions** (only when status is waiting/opened; gap 10, each flex 1, h 48, radius 14, 16px):
  - **Copy link** (primary, 700; hint "Copy link" + `Mod+C`) -> toast **"Link copied"**
  - **Add 1 day** (secondary, 600) -> status unchanged, `exp='Ends in 2 days'` (hard-coded in mock; real app adds 24 h to deletion date), appends event `['Extended by a day','Just now']`, toast **"Link extended by a day"**
  - **Revoke** (danger outline 700; hint "Revoke link" + `Del`) -> `status='revoked'`, `exp='Revoked just now'`, appends event `['Revoked by you','Just now']`, toast **"Link revoked"**. No confirmation. Modal stays open and re-renders with the Revoked pill and no action row.
- Expired/revoked: actions hidden.

---

## 8. QR / Add 2FA modal (`modal:'qr'`, `mQr`; state `qr`: choose -> scan -> found | manual)

Width 580. Opened by "Add 2FA code" / "Replace" (resets to `choose`, key `''`). Header: title **"Add 2FA code"** (26px/800) + 20px "?" tip placed **below** the icon (top: calc(100% + 10px), width 280): **"When you turn on two-step sign-in, the site shows a QR code. Scan it here and you won't need a phone app."**; sub **"For {item name}"** (16px `#7d7979`).

**choose** (`dw-up .3s`): 2-col grid gap 12; two cards radius 18, padding 22, column gap 14; icon tile 52x52 radius 15 (26px icon).
- **Scan from screen** (primary-emphasis): border 2px `#1b3fa0`, bg `#f5f7fd`, tile bg `#1b3fa0` white scan-frame icon; title 18px/800 `#14307a` **"Scan from screen"**, sub 15px `#4a4745` **"Draw a box around the code"**; hover `translateY(-3px)` + shadow `0 12px 26px rgba(27,63,160,.16)`. -> `startScan`.
- **Type the key**: border 2px `#e8e6e3`, tile bg `#f1f0ee` keyboard icon `#4a4745`; title 18px/800 **"Type the key"**, sub 15px **"The code of letters under the QR"**; hover `translateY(-3px)`, shadow `0 12px 26px rgba(32,30,29,.1)`, border `#cfccc8`. -> `startManual`.

**scan** (`dw-fade .3s`): panel h 300, radius 20, bg `#1d1f2b`, centred column gap 18. Inside a 200x200 white radius 10 tile: 170x170 SVG (viewBox 25x25, `crispEdges`, fill `#201e1d`) of a decorative QR (25x25 module grid; three 7x7 finder patterns at TL/TR/BL, rest pseudo-random by `((x*73 + y*151 + x*y*7) % 11) < 5`). Four corner brackets (28x28, 4px `#7fa4ef`, outer radius 10) offset -8px outside each corner; a 3px scan line (`#3b74e8`, left/right 6, glow `0 0 14px 4px rgba(59,116,232,.55)`) animated `dw-scan .9s ease-in-out infinite alternate` (top 6px <-> 190px). Caption **"Looking for a code…"** 16px/600 `#e8eaf2`. After **1900 ms** -> `found`. No cancel button (X/scrim/Esc closes). The real app: region-select overlay (Draw a box), screen capture, decode `otpauth://` URI.

**found** (`dw-up .3s`): card padding 18px 20px, radius 18, bg `#f5f7fd`, border 1px `#dbe4f7`, gap 14: 48px success circle with 24px check (`dw-ring`/`dw-draw .5s .15s`); **"Found it"** 18px/800; **"Git Host · anovak"** 15px `#4a4745` (hard-coded; real = issuer · account from the otpauth label); right-aligned live code 26px/700 ls .12em `#14307a` tabular-nums (`code('qr'+id)`) and **"new code in {secs}s"** 13px `#6b6866` (secs = 30 - epoch s mod 30). Below, centred 15px `#6b6866`: **"Does this match the code on the site?"**. Buttons (gap 10, flex 1, h 52, radius 15, 16px): **Scan again** (secondary 600; -> scan) and **Save code** (primary 700, shadow `0 8px 20px rgba(27,63,160,.3)`) -> close modal, totp[item]=true, toast **"2FA code added"**.

**manual** (`dw-up .3s`): key field h 58, radius 16, border **2px** (`#1b3fa0` when valid, `#e0deda` otherwise; `transition .2s`), padding 0 18; input mono 18px, ls .1em, UPPERCASE (CSS `text-transform`), placeholder **"JBSW Y3DP EHPK 3PXP"**. Validity (`keyValid`): strip whitespace, uppercase; length >= 16 and matches `^[A-Z2-7]+=*$` (base32). When valid, a preview row appears (`dw-up .25s`): padding 16px 20px, radius 16, bg `#f5f7fd`, check icon 20px `#1b7a3f`, **"Your code right now"** 15px `#4a4745`, live code 24px/700 ls .12em `#14307a` tabular-nums. Buttons (flex 1, h 52, radius 15): **Back** (secondary; -> choose) and **Save code** (primary; `opacity .4` until valid, `transition .2s`, still shows hover; click is a no-op if invalid) -> same save + toast **"2FA code added"**. No inline error text for invalid keys (only the dimmed Save + neutral border).

---

## 9. Toasts and microcopy index (this section)
**Edit:** Image added; Image removed; Add an app under Fills in first; Click the window you want to add; Added SAP GUI for Windows; Test finished — every step worked; 2FA code removed; Give it a name first; Saved; Moved to trash.
**Modals:** Pick at least one thing to share; Link copied; Link extended by a day; Link revoked; 2FA code added.
**Hints (tooltips):** Close without saving (Esc); Change image; Rename (F2); Save (Mod+S); Show or hide (Mod+Shift+R); Generate a password (Mod+G); Copy link (Mod+C) x2; Revoke link (Del).
**Tip bubbles:** totp, fills, steps, only, test, share, once, qr (verbatim above).

## 10. Ambiguities / mock-isms to resolve at implementation
1. Edit changes are not persisted in the mock (Username, Card fields, Notes use uncontrolled `defaultValue`); real app must wire all.
2. Folder control offers "Shared" while sidebar folder is "Shared with me"; real app should list actual Bitwarden folders + "No folder".
3. Step list, "Only type into", Fills-in apps, Test and preset selection are NodeWarden/Deskwarden-local (Bitwarden has no equivalent): store them locally or in a custom field to keep Bitwarden compatibility.
4. Send share link domain and "recipient" restriction are placeholders; map to Bitwarden Send semantics. Field-level sharing (Username/Password/2FA/Notes) has no Bitwarden Send equivalent (Send carries text/file) - needs a decision.
5. Hard-coded values: import result (128 / 3 duplicates), "Add 1 day" -> "Ends in 2 days", Found-it label "Git Host · anovak", the Password history dates, and About metadata.
6. No click-outside close for the avatar menu or Add-step menu; no keyboard navigation in the palette; Esc inside the edit view closes the title editor only when the title input is focused (global Esc handler only closes modals; the header "Esc" hint implies Esc also leaves the editor, which the mock does not implement).
7. "Move to trash" appears for new, unsaved items.
8. Note-type edit view has no dedicated body field beyond Notes card.

---

# Part C: generator, settings, trash, health, lock, sign-in, splash

Source: `_stripped.html` (lines 402-654, 1169-1266) and `_design.js`. All hex values, px values and copy are verbatim from the design. "Archivo" is the font everywhere unless noted; "mono" = `ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`.

## 0. Shared context (needed to read the rest)

### 0.1 Window shell
| Property | Value |
|---|---|
| Artboard "1a Desktop app" | 1280 x 820, radius 18, bg `#f5f4f2`, shadow `0 30px 70px rgba(32,30,29,.18), 0 0 0 1px rgba(32,30,29,.08)`, `overflow:hidden`, `position:relative`, flex row |
| Sidebar | 252 wide, padding `22px 14px 16px`; Navy default `#14307a` (Light variant `#ffffff`). Nav rows 44 high, radius 12, 15px/600 |
| Main column | flex 1; top bar 72 high white; page content area below (`flex:1; min-height:0`) |
| Top bar | search 460x46 radius 14 `#f3f2f0`; sync pill (34 high, radius 999, 13px/600); avatar 38 circle `#201e1d` "AN"; Windows caption buttons 46x34 (—, ▢, ✕; ✕ hovers `#c42b1c`/white) |
| Body | `background #ecebe9`, text `#201e1d`, `-webkit-font-smoothing: antialiased`, `::selection #b8c7ea` |
| Overlays (z-index) | toast 30, tooltips 40, lock 45, sign-in 46, splash 50, hint tooltip 80; all `position:absolute` inside the 1280x820 window |

### 0.2 Keyframes (all used by this section)
| Name | Definition |
|---|---|
| `dw-up` | opacity 0 -> 1, translateY(10px) -> none |
| `dw-fade` | opacity 0 -> 1 |
| `dw-pop` | opacity 0 -> 1, translateY(12px) scale(.97) -> none |
| `dw-tip` | opacity 0 -> 1, translateY(4px) -> none |
| `dw-ring` | scale(.6) opacity 0 -> scale(1) opacity 1 |
| `dw-line` | translateX(-110%) -> translateX(330%) |
| `dw-breathe` | scale 1 -> 1.06 -> 1 (50%) |
| `dw-pulse-w` | box-shadow `0 0 0 0 rgba(255,255,255,.55)` -> `0 0 0 22px rgba(255,255,255,0)` |
| `dw-pulse-b` | box-shadow `0 0 0 0 rgba(27,63,160,.45)` -> `0 0 0 16px rgba(27,63,160,0)` |
| `dw-toast` | opacity 0, translate(-50%,14px) -> opacity 1, translate(-50%,0) |

### 0.3 Reusable components (identical markup repeated everywhere; implement once)
| Component | Spec |
|---|---|
| **Card** | bg `#fff`, radius 20, shadow `0 1px 3px rgba(32,30,29,.06), 0 0 0 1px #ebe9e6` (a 1px ring, no real border) |
| **Row separator** | `1px solid #f1efec` bottom border on every row except (design omits it) the last row of a hand-written card; generated `setGroups` rows ALL carry the border, including the last |
| **Switch** | track 50x30, radius 999, padding 3; knob 24x24 white, shadow `0 1px 3px rgba(0,0,0,.2)`. On: track `#1b3fa0`, knob `translateX(20px)`. Off: track `#d9d6d2`, knob 0. Transitions: track bg .2s; knob transform .22s `cubic-bezier(.3,1.4,.5,1)` (overshoot) |
| **Segmented control** (`seg()`) | container padding 4, radius 13, bg `#f1f0ee`; option padding `8px 13px` (settings rows) / `8px 14px` (Apps rows) / `9px 15px` (Health tabs, server picker) / `10px 20px` radius 11, 15px (Generator mode tabs); option radius 10, 14px/700, nowrap; selected: bg `#fff`, fg `#201e1d`, shadow `0 1px 3px rgba(32,30,29,.14)`; unselected: transparent bg, fg `#6b6866`, no shadow. Transition bg/shadow .2s (colour too on the Generator tabs). No hover state |
| **Outline button** | height 42, padding `0 16px`, radius 12, border `1px solid #e0deda`, 15px/600, text `#201e1d`, nowrap. Hover: bg `#f7f6f5` (settings generic row) or border+text `#1b3fa0` (hand-written rows). Danger: border `#f0cfca`, text `#b42318` |
| **`?` help dot** | 18x18 circle bg `#eceae7`, fg `#6b6866`, 11px/800, `cursor:help`. Tooltip shows on mouse-enter of the dot, hides on leave |
| **Help tooltip bubble** | `position:absolute; left:-12px; bottom:calc(100% + 10px)`; width 240 (Generator) or 260 (all others); padding `11px 13px`; radius 12; bg `#201e1d`; text `#fff` 14px/500, line-height 1.45; shadow `0 12px 28px rgba(0,0,0,.22)`; `pointer-events:none`; anim `dw-tip .16s ease-out`. State: `s.tip` holds ONE tooltip key at a time. Keys: `look`, `apps`, `server`, per-row `st{groupIdx}_{rowIdx}`. Switching settings page clears `tip` |
| **Round icon-pill / status pill** | radius 999 |
| **Avatar tile** | 44x44 radius 12, 15px/700, bg/fg per item (Trash tile at opacity .75) |

### 0.3b State defaults (from `state` init) relevant here
`view:'vault'`, `setTab:'General'`, `rec:-1`, `tip:null`, `toast:null`, `hKind:'weak'`, `signedOut:false`, `server:'bitwarden.com'`, `locked:false`, `lockStage:'hello'`, `lockLeaving:false`, `loading:true`, `leaving:false`, `ready:false`, `lock:'15 min'`, `settings:{suggest:true,hello:true,enter:false,clip:true}`, `syncing:false`.

`gMode:'Password'`, `gLens:{Password:20,Passphrase:5,PIN:6}`, `gSets:{Uppercase,Lowercase,Digits,Symbols all true}`, `gLook:true`, `spin:0`, `genPw` generated in `componentDidMount`.

`prefs` defaults (settings pages): `startup:true, minimized:false, closeTray:true, theme:'System', textSize:'Default', suggest:true, panelPos:'Below the field', autoSignIn:false, fill2fa:true, matchBy:'App and window', lockSleep:true, lockScreen:true, clip:'30 s', hideShare:true, icons:true, cardsInFill:false, genType:'Password', genSymbols:true, nShared:true, nHealth:true, nDevice:true, nUpdate:false, beta:false`.

`trash` (3 seeded): t1 "Old VPN" (OV, `#eceae7`/`#201e1d`, "Deleted 3 days ago"); t2 "Jira Cloud" (JC, `#dbe4f7`/`#14307a`, "Deleted 12 days ago"); t3 "Printer admin" (PR, `#eceae7`/`#201e1d`, "Deleted 25 days ago").

Platform prop: `platform` default `'Windows'` (macOS variant exists; this spec focuses on Windows, notes Mac diffs). `plat` = "Windows"; `mod` = "Ctrl", `shf` = "Shift" (Mac: ⌘ / ⇧).

---

## 1. Generator view (`showGen`, `s.view==='gen'`)

Reached via sidebar Tools > Generator or palette action "Generate a password" (`Ctrl G` hint).

### 1.1 Layout
| Element | Spec |
|---|---|
| Page wrapper | `flex:1; overflow:auto; padding:32px 40px`; centred (`justify-content:center`); enter anim `dw-up .35s ease-out` |
| Column | width 100%, max-width **760**, column gap **18** |
| Title | "Generator", 32px/800, letter-spacing -0.03em |
| Card 1 (output) | Card (radius 20), padding **28**, column gap **20** |
| Card 2 (options) | Card, padding `24px 28px`, column gap **22** |

### 1.2 Card 1: generated value row
| Element | Spec |
|---|---|
| Value box | min-height 80, flex centre-aligned; mono 30px/600, line-height 1.35, `word-break:break-all`, letter-spacing .02em; inner is `display:flex; flex-wrap:wrap` so each char is its own flex item (wraps anywhere) |
| Per-character `<span>` | colour by class: digit `/[0-9]/` -> `#1b3fa0`; letter `/[A-Za-z]/` -> `#201e1d`; anything else (symbols AND passphrase hyphens) -> `#3b74e8`. Each char animates `dw-fade .25s ease-out both` with `animation-delay = i * 0.012s` (staggered reveal on every regen). Passphrase words are letters (ink) separated by blue hyphens |
| Action row | flex, centre, gap **14**: [strength block flex:1][New button][Copy button] |
| Strength block | column gap 8. Bar: height 8, radius 999, track `#eeece9`, overflow hidden; fill height 8, radius 999, width = `strength.width`, colour = `strength.color`; transitions `width .5s cubic-bezier(.2,.8,.2,1), background .3s`. Label: 14px/700, colour = strength colour, transition colour .3s |
| **New** button | height 50, padding `0 20px`, radius 14, border `1px solid #e0deda`, gap 10, 16px/600; hover border+text `#1b3fa0`. Icon: 19x19 refresh glyph (`M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6`, stroke 2.2, round); icon rotates `spin*360deg` with `transition: transform .5s cubic-bezier(.2,.8,.2,1)` (each regen adds one 360 turn). Label "New". Hover hint tooltip: "New password" + key chip `Mod+R` (shown as Ctrl R). Click = `regen()` |
| **Copy** button | height 50, padding `0 24px`, radius 14, bg `#1b3fa0`, text `#fff` 16px/700, shadow `0 6px 16px rgba(27,63,160,.28)`; hover bg `#14307a`; active `translateY(1px)`. Hint: "Copy password" + `Mod+C`. Click -> toast "Password copied" (always this text, even for passphrase/PIN; real clipboard write not in the design) |

### 1.3 Strength model (`renderVals`, NOT the `strengthOf` method; the method is used only for the edit-view password field and has a different min width)
- bits: PIN = `len * 3.32`; Passphrase = `len * 12.9`; Password = `len * log2(pool)` where pool = sum of enabled class sizes (Upper 26, Lower 26, Digits 10, Symbols 10; if none, 26).
- Label/colour thresholds: `<40` Weak `#b42318`; `<70` Fair `#b7791f`; `<100` Strong `#1b3fa0`; else "Very strong" `#1b7a3f`.
- Bar width: `clamp(bits/1.3, 8, 100)` %, rounded integer. (`strengthOf()` is identical except minimum 6%.)
- Defaults: Password 20 chars, pool 72 -> 123 bits -> "Very strong", 95%.

### 1.4 Card 2: options
| Element | Spec |
|---|---|
| Mode tabs | segmented, `align-self:flex-start`, container padding 4 radius **14** bg `#f1f0ee`; options "Password", "Passphrase", "PIN"; padding `10px 20px`, radius 11, 15px/700. Click -> `regen({gMode})` (regenerates immediately; each mode remembers its own length) |
| Length row | baseline space-between. Left label 16px/600: "Length" (Password, PIN) or "Words" (Passphrase). Right: current number 22px/800 `#1b3fa0` |
| Slider | native `<input type=range>`, width 100%, `accent-color:#1b3fa0`, height 22, pointer. Range: Password 8-64 (default 20), Passphrase 3-10 (default 5), PIN 4-12 (default 6). Change -> regen with new length. In egui, a custom slider is needed (track/thumb in `#1b3fa0`) |
| Class tiles (Password mode only; hidden for Passphrase/PIN) | grid 4 columns, gap 10. Each tile: height 76, radius 14, border `2px solid`, padding `0 14px`, column, left-aligned, centred vertically, gap 4; transition border/bg .2s. Top line: sample string mono 18px/700; bottom label 14px/600. On: border `#1b3fa0`, bg `#f5f7fd`, sample+label colour `#14307a` (label sub `#14307a`). Off: border `#e8e6e3`, bg `#fff`, sample `#9b9797`, label `#8a8785`. Tiles & samples: **Uppercase `ABC`**, **Lowercase `abc`**, **Digits `123`**, **Symbols `#$&`**. Click toggles + regenerates. If all are off the generator silently falls back to Lowercase (tiles all show off; strength pool falls back to 26) |
| Look-alike row (Password mode only) | flex, centre, gap 12. Label "Avoid look-alike characters" 16px/600; then `?` dot (tooltip below); spacer; Switch (50x30). State `gLook` default **true**; click regenerates |
| Tooltip copy (`tips.look`, width 240) | "Leaves out 0 and O, 1 and l — easier when you have to type it by hand." |

No history list, no separate "copy" per segment, no extra options exist in this design.

### 1.5 Generation rules (`make`)
- PIN: `len` random digits 0-9.
- Passphrase: `len` words from `WORDS` joined by `-`. Word list (18): maple, orbit, canyon, velvet, harbor, pixel, juniper, falcon, ember, tundra, cobalt, meadow, quartz, lantern, saffron, glacier, willow, copper. (Design placeholder; the app should use the real EFF-style list and probably an options set.)
- Password: pool of enabled classes; guarantees at least one char from each enabled class, fills remainder from the pool, Fisher-Yates shuffles, slices to `len`.
  - look-alike ON ("safe") sets: Upper `ABCDEFGHJKLMNPQRSTUVWXYZ` (no I, O); Lower `abcdefghijkmnpqrstuvwxyz` (no l); Digits `23456789` (no 0, 1); Symbols `!@#$%&*?-_`.
  - look-alike OFF ("full") sets: full A-Z, a-z, 0-9; same symbols.
- Regenerate triggers: first mount, New, mode change, length change, class toggle, look-alike toggle.

---

## 2. Settings view (`showSettings`, `s.view==='settings'`)

### 2.1 Layout
| Element | Spec |
|---|---|
| Wrapper | grid `232px minmax(0,1fr)`; enter anim `dw-fade .3s ease-out` |
| Left nav | border-right `1px solid #e8e6e3`, bg `#faf9f8`, padding `26px 12px`, column gap 2, scrolls. Heading "Settings" 26px/800, letter-spacing -0.02em, padding `0 12px 16px` |
| Nav row | height 42, radius 12, flex centre gap 12, padding `0 12px`, 15px/600, `flex-shrink:0`; icon 18x18 stroke 2 round. Normal: bg transparent, fg `#4a4745`, hover bg `#f1f0ee`. Selected: bg `#eef2fc`, fg `#14307a` (hover stays `#eef2fc`). Transition bg/colour .2s. Click -> `setTab=k, rec=-1, tip=null` |
| Right pane | `overflow:auto; padding:32px 40px 48px`, centred; content column max-width **760**, gap **18** |
| Page header | column gap 4, anim `dw-up .3s`. Title = page name, 30px/800, letter-spacing -0.03em. Subtitle (if any) 16px `#7d7979` |
| Group | column gap 10, anim `dw-up .3s ease-out both`. Optional group title: 12px/800, letter-spacing .12em, UPPERCASE, `#8a8785`, padding `6px 4px 0`. Then a Card |
| Setting row | flex centre, gap 14, padding `17px 24px`, bottom border `#f1efec`. Optional icon tile 42x42 radius 12 bg `#f1f0ee` fg `#4a4745` with 20x20 stroke-2 icon. Text block (min-width 0, column, gap 3): title 16px/600 (+ optional `?` dot, 8px gap); optional sub 14px `#8a8785`. Spacer (flex 1). Control on the right (one of: Switch, Segmented, Outline button, Value text) |
| Value (read-only) | 15px/600 `#4a4745`, nowrap |

Page order is the order of the `SECTIONS` object (12 pages, nav icons from `SETICON`). Page content rendering order inside the right pane: header, then page-specific hand-written blocks (Apps / Security / Shortcuts / Account), then the generated `setGroups` (Security and Account have BOTH).

### 2.2 Row kinds (from `R` helper)
| Kind | Helper | Control | Click behaviour |
|---|---|---|---|
| `sw` | `R.sw(key,title,sub,tip)` | Switch bound to `prefs[key]` | toggles pref |
| `sg` | `R.sg(key,title,opts,sub,tip)` | Segmented bound to `prefs[key]` (Security "Lock after" bound to `s.lock`) | sets pref to the option label string |
| `bt` | `R.bt(title,btn,onClick,sub,danger,tip)` | Outline button; `danger` -> red text `#b42318` + border `#f0cfca` | runs onClick (toast / navigation) |
| `va` | `R.va(title,value,sub,tip)` | Read-only value text | none |
Optional `icon` (Devices page only) renders the 42x42 icon tile. Optional `tip` renders the `?` dot.

### 2.3 Nav list (12 pages, icon source)
General (sliders icon = `ICON.settings`), Autofill, Apps (monitor), Security (shield), Vault (archive box), Generator (refresh), Notifications (bell), Import & export (download tray), Devices (laptop), Shortcuts (keyboard), Account (user), About (info circle). SVG path data in `SETICON` (24x24 viewBox, stroke 2, round caps/joins, no fill):
- General `M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6`
- Autofill `M15 4V2M15 16v-2M8 9h2M20 9h2M17.8 11.8 19 13M17.8 6.2 19 5M3 21l9-9M12.2 6.2 11 5`
- Apps `M3 4h18v12H3zM8 20h8M12 16v4`
- Security `M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z`
- Vault `M3 4h18v4H3zM5 8v12h14V8M10 12h4`
- Generator `M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6`
- Notifications `M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0`
- Import & export `M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3`
- Devices `M4 5h16v11H4zM2 20h20`
- Shortcuts `M2 6h20v12H2zM6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10`
- Account `M20 21a8 8 0 0 0-16 0M12 13a5 5 0 1 0 0-10 5 5 0 0 0 0 10z`
- About `M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 16v-4M12 8h.01`

### 2.4 Pages, group by group (copy verbatim; "default" = initial state)

Legend: SW switch, SEG segmented, BTN button, VAL value.

#### General: subtitle "How Deskwarden starts and looks"
**Group "Startup"**
| Row | Type | Default | Notes |
|---|---|---|---|
| Open when I sign in to Windows (Mac: "...to macOS") | SW `startup` | on | launch at login |
| Start in the system tray (Mac: "menu bar") | SW `minimized` | off | start hidden |
| Keep running when I close the window | SW `closeTray` | on | `?` tip: "Autofill only works while Deskwarden is running. Closing the window just hides it." |

**Group "Look"**
| Row | Type | Options | Default |
|---|---|---|---|
| Theme | SEG `theme` | Light, Dark, System | System |
| Text size | SEG `textSize` | Default, Large, Larger | Default |
| Language | BTN "English" | click -> toast "More languages are on the way" | n/a |

#### Autofill: subtitle "What happens when an app asks you to sign in"
**Group "Suggestions"**
| Row | Type | Default | Notes |
|---|---|---|---|
| Suggest logins when I click a sign-in field | SW `suggest` | on | tip: "A small panel opens under the field with the logins that match that app." |
| Show the panel | SEG `panelPos` | "Below the field" / "Near the cursor"; default Below the field | |
| Sign in automatically after filling | SW `autoSignIn` | off | |
| Type 2FA codes for me | SW `fill2fa` | on | tip: "If a code screen follows the password, Deskwarden types the code too." |

**Group "Matching"**
| Row | Type | Default | Notes |
|---|---|---|---|
| Match apps by | SEG `matchBy` | "App name", "App and window", "Exact"; default App and window | tip: "App name is the most forgiving. Exact only matches the same program and window — the safest choice for banking apps." |
| Default sign-in steps | BTN "Standard" | sub: "Username, Tab, password, Enter"; click -> toast "Change steps for one login from its Edit page" | |

**Group "Shortcut"**
| Row | Type | Notes |
|---|---|---|
| Fill shortcut | BTN "Ctrl Shift F" (keys joined by space; Mac "⌘ ⇧ F") | sub: "Works in any app, even without the panel"; click -> `setTab='Shortcuts'` (navigates to Shortcuts page) |

#### Apps: no subtitle (page has its own intro line), `setApps`
See 2.5.

#### Security: subtitle "Keep your vault safe"
Page-specific blocks first (2.6), then generated groups:
**Group "Locking"**
| Row | Type | Options | Default |
|---|---|---|---|
| Lock the vault after | SEG bound to `s.lock` | "5 min", "15 min", "1 hour", "Never" | 15 min |
| Lock when the computer sleeps | SW `lockSleep` | | on |
| Lock when I lock my screen | SW `lockScreen` | | on |

**Group "Privacy"**
| Row | Type | Options | Default |
|---|---|---|---|
| Clear copied items after | SEG `clip` | "10 s", "30 s", "1 min", "Never" | 30 s |
| Hide passwords while I share my screen | SW `hideShare` | | on; tip: "Passwords stay as dots during Teams, Zoom or Meet screen sharing — even if you press reveal." |

#### Vault: subtitle "How items are shown and organised"
**Group "Display"**: "Show app and website icons" SW `icons` (on); "Offer cards and notes in autofill" SW `cardsInFill` (off).
**Group "Organise"**
| Row | Control | Sub | Click |
|---|---|---|---|
| Default folder for new items | BTN "No folder" | none | toast "Pick a folder from the list" |
| Folders | BTN "Manage" | "{n} folders" (n = `folders.length`, seed 3, grows with sidebar New folder) | goes to vault with filter `f:<first folder>` ("Engineering") |
| Trash | BTN "Open" | "{n} items · emptied after 30 days" (n = trash count, seed 3) | `view='trash'` |

#### Generator: subtitle "Defaults for new passwords"
One untitled group:
| Row | Type | Default | Notes |
|---|---|---|---|
| Make new passwords as | SEG `genType` | Password / Passphrase / PIN; default Password | independent of the Generator view's `gMode` |
| Length | VAL "{gLens.Password} characters" -> "20 characters" | read-only; mirrors the generator page's password length | |
| Include symbols | SW `genSymbols` | on | |
| Avoid look-alike characters | SW bound to `s.gLook` (NOT a pref; shared with the Generator view) | on | toggling regenerates the generator value |

#### Notifications: subtitle "When Deskwarden should tell you something"
One untitled group, all SW: "Someone opens a link I shared" `nShared` on; "A password becomes weak or reused" `nHealth` on; "A new device signs in to my account" `nDevice` on; "An update is ready" `nUpdate` **off**.

#### Import & export: subtitle "Move passwords in and out"
One untitled group:
| Row | Control | Sub | Click |
|---|---|---|---|
| Import from another app | BTN "Import" | "1Password, Keeper, LastPass, Chrome or a CSV file" | opens Import modal at step "source" |
| Export vault | BTN "Export" | "An encrypted file only you can open" | toast "Encrypted export saved to Downloads" |
| Last import | VAL "4 Jan · 128 items" | | |

#### Devices: subtitle "Where your account is signed in"
Group 1 (untitled): one row per device, each with an icon tile (laptop `M4 5h16v11H4zM2 20h20`, phone `M7 2h10v20H7zM11 18h2`, web globe path = `ICON.web`).
| Device (title) | Sub | Icon | Control |
|---|---|---|---|
| ALEX-LAPTOP | Windows 11 · Deskwarden · Active now | laptop | VAL "This device" (current device flag) |
| MacBook Air | macOS · Bitwarden app · 2 days ago | laptop | BTN "Sign out" (danger) |
| iPhone 16 | iOS · Bitwarden app · Yesterday | phone | BTN "Sign out" (danger) |
| Chrome | Bitwarden extension · Today | web | BTN "Sign out" (danger) |
Sign out removes that device from the list and toasts "{name} signed out".
Group 2 (untitled): "Sign out everywhere else" BTN "Sign out all" (danger, no sub): removes every non-current device, toast "Signed out on all other devices".

#### Shortcuts: no subtitle
See 2.7.

#### Account: no subtitle
Hand-written blocks (2.8), then group "Plan" (title uppercase "PLAN"):
| Row | Control | Notes |
|---|---|---|
| Bitwarden Premium | VAL "Renews 2 Mar 2027" | |
| Delete account | BTN "Delete" (danger) | sub "Removes your vault for good"; click -> toast "Ask your Bitwarden admin to remove it" (no destructive action) |

#### About: subtitle "Deskwarden 2.0.1"
One untitled group:
| Row | Control | Notes |
|---|---|---|
| Updates | BTN "Check now" | sub "Up to date"; toast "You have the latest version" |
| Get early updates | SW `beta` | off |
| Help center | BTN "Open" | toast "Opening help…" |
| Send feedback | BTN "Write" | toast "Thanks — we read every note" |
| Diagnostic logs | BTN "Copy" | tip: "Logs never contain passwords, codes or anything you type."; toast "Logs copied" |

### 2.5 Apps page (`setApps`)
No subtitle (subtitle slot empty); instead an intro row: flex centre gap 10: text "What happens when each app asks you to sign in" 16px `#6b6866` + `?` dot.
- Tooltip (`tips.apps`, width 260): "Suggest shows the panel under the sign-in field. Auto-fill types right away. Never keeps Deskwarden quiet in that app."
- Card containing 4 rows (padding `16px 24px`, separator `#f1efec`):
  - Tile 42x42 radius 12 bg `#f1f0ee`, fg `#4a4745`, 14px/700 initials.
  - Name 16px/600 (flex 1).
  - Segmented (padding 4, radius 13, `#f1f0ee`; options padding `8px 14px`, 14px/700): **Suggest / Auto-fill / Never**.
- App list and default rule: SAP Logon (SL) = Suggest; Ledgerline Desktop (LG) = Auto-fill; Remote Desktop (RD) = Suggest; Banking apps (BK) = Never.
- Footer row inside card: "+ Add an app", padding `18px 24px`, 15px/700, `#1b3fa0`, hover `#14307a`, pointer. No handler in the design (inert).

### 2.6 Security page, hand-written blocks (`setSecurity`)
**Score card** (Card, padding 24, column gap 20, anim `dw-up .3s`):
- Row: ring (88x88) + text (gap 20).
  - Ring: SVG 88x88 rotated -90deg; two circles r=36, cx/cy 44, stroke-width 8; track `#eceae7`; progress `#1b7a3f`, `stroke-linecap:round`, `stroke-dasharray 226.2`, `stroke-dashoffset 40.7` (= 82%), animates `dw-ring .5s ease-out`. Centred number "82" 26px/800.
  - Title "Your vault is in good shape" 21px/800, letter-spacing -0.01em. Sub "Fix 6 items to reach 100." 15px `#7d7979`. (Score and "6 items" are hard-coded; note the health tiles below total 7.)
- Three tiles (grid 3 cols, gap 10): bg `#f7f6f5`, radius 16, padding 16, column gap 6; hover `translateY(-2px)`, bg `#f1f0ee` (transition .18s). Contents: big number 28px/800 in category colour; label 15px/600; "Review" 14px/700 `#1b3fa0`. Tiles: **2 / Weak passwords** (`#b42318`), **2 / Reused passwords** (`#b7791f`), **3 / Older than a year** (`#1b3fa0`). Counts = `HEALTH[k].length`. Click -> Password health view with that tab selected.

**Account-security card** (Card, anim `dw-up .3s`), rows padding `18px 24px`, border `#f1efec` (last row none):
| Row | Content |
|---|---|
| Master password | title "Master password" 16px/600, sub "Changed 4 months ago" 14px `#8a8785`; outline button "Change" (hover border/text blue) |
| Windows Hello | title = `helloLabel` ("Unlock with Windows Hello"; Mac "Unlock with Touch ID"); Switch bound to `s.settings.hello` (default **on**) |
| Two-step login | title "Two-step login for your account", sub "Extra check when you sign in on a new device"; green pill "On" (13px/700, padding `5px 11px`, radius 999, bg `#e8f3ec`, fg `#17673a`); outline button "Manage" |
| Export vault | title "Export vault", sub "Download an encrypted copy"; outline button "Export" |
Change / Manage / Export buttons have no handlers (inert). Then the generated Locking and Privacy groups follow.

### 2.7 Shortcuts page (`setShortcuts`)
Card (anim `dw-up .3s`), 5 rows, each padding `16px 24px`, separator `#f1efec`:
| Label | Keys (Windows) |
|---|---|
| Fill the sign-in in front of you | Ctrl, Shift, F |
| Open Deskwarden | Ctrl, Shift, D |
| Search your vault | Ctrl, K |
| Generate a password | Ctrl, G |
| Lock now | Ctrl, L |
(Mac replaces with ⌘ and ⇧.) Label 16px/600, flex 1.
- Key-chip well (clickable): flex gap 6, padding 6, radius 12, border `2px solid` (transparent normally, `#1b3fa0` while recording), pointer; border transition .2s; hover bg `#f7f6f5`.
- Key chip: min-width 32, height 32, padding `0 10px`, radius 9, bg `#f3f2f0`, `border-bottom: 2px solid #d9d6d2`, 14px/700, centred.
- **Recording state** (`rec === j`): chips replaced by text "Press new keys…" (height 32, padding `0 10px`, 14px/700, `#14307a`); well border `#1b3fa0`; animation `dw-pulse-b .9s ease-out infinite` (blue ring expanding 0->16px and fading).
- Behaviour: click sets `rec=j`; after **1600 ms** the design auto-ends recording (no real key capture) and toasts "Shortcut saved". Only one row records at a time (clicking another overrides; timer restarts). Leaving the page resets `rec=-1`. Implementation should capture a real key combo (require a modifier), Esc to cancel.

### 2.8 Account page (`setAccount`), hand-written blocks
**Identity card** (Card, padding 24, flex centre gap 18):
- Avatar 60x60 circle `#201e1d`, white "AN" 20px/800.
- Name "Alex Novak" 21px/800 (letter-spacing -0.01em); email "a.novak@ledgerline.com" 15px `#7d7979`.
- Right badge pill "Bitwarden account": 13px/700, padding `6px 12px`, radius 999, bg `#eef2fc`, fg `#14307a`.

**Sync/server card** (Card, anim `dw-up .3s`), rows padding `18px 24px`:
| Row | Content |
|---|---|
| Last synced | title "Last synced"; sub = `syncText`: "Just now" (online idle), "Syncing…" (while syncing), "2 hours ago" (offline). Button "Sync now" (outline, 42 high, with 16x16 refresh icon rotating `syncSpin*360deg`, transition `transform 1s cubic-bezier(.2,.8,.2,1)`; hover border/text blue). Hint "Sync now" + `Ctrl Shift Y`. Behaviour: offline -> toast "Still can’t reach Bitwarden"; online -> `syncing=true` 1100 ms then toast "Everything is up to date" (sidebar-header sync pill shows Syncing… / Synced; offline pill amber) |
| Server | title "Server"; value "bitwarden.com" 15px `#4a4745` (static text, not bound to `s.server`) |
| Deskwarden 2.0.1 | sub "Up to date"; outline button "Check for updates" (no handler) |
| Sign out | text-only red "Sign out" 16px/700 `#b42318`, pointer; no divider (last row); click -> `signedOut=true, view='vault', setTab='General'` -> sign-in screen (section 5.2) |
Then the "PLAN" group (Section 2.4).

---

## 3. Trash view (`showTrash`, `s.view==='trash'`)

Reached from sidebar Vault > Trash (count badge = `trash.length`) or Settings > Vault > Trash > Open.

| Element | Spec |
|---|---|
| Wrapper | `flex:1; overflow:auto; padding:32px 40px`; column gap 22; anim `dw-up .35s ease-out`. Not width-capped (full width of pane, unlike Health/Generator) |
| Header | flex, bottom-aligned, gap 14. Title "Trash" 32px/800 -0.03em; sub "Items are deleted for good after 30 days." 16px `#7d7979` (gap 6). Spacer. **Empty trash** button (only when trash non-empty): height 46, padding `0 18px`, radius 14, border `1px solid #f0cfca`, text `#b42318` 15px/700; hover bg `#fdf3f2`. Click -> `trash=[]`, toast "Trash emptied" (no confirm dialog in design) |
| List card | Card; rows padding `16px 22px`, gap 14, separator `#f1efec`; per-row entry anim `dw-up .3s ease-out both`, delay `index*0.05s` |
| Row contents | tile 44x44 radius 12 (item av colours, **opacity .75**, initials 15px/700); name 16px/700 + "when" line 14px `#8a8785` (e.g. "Deleted 3 days ago"); **Restore** outline button (height 42, padding `0 16px`, radius 12, border `#e0deda`, 15px/600, hover blue border+text); **Delete forever** text button (height 42, padding `0 14px`, radius 12, 15px/600, `#b42318`, hover bg `#fdf3f2`) |
| Restore | removes from trash, toast "{name} restored" |
| Delete forever | removes, toast "Deleted for good" (no confirm) |
| Items deleted from the Edit view | prepended with when = "Deleted just now", toast "Moved to trash" |
| Empty state | centred, padding `80px 0`, gap 12, anim `dw-up .3s`: tile 72x72 radius 22 bg `#f1f0ee` fg `#8a8785` containing a 30x30 trash icon (`M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14`, stroke 2); title "Trash is empty" 20px/800; sub "Deleted items wait here for 30 days." 15px `#7d7979`. The Empty-trash button is hidden in this state |

---

## 4. Password health view (`showHealth`, `s.view==='health'`)

Entered from the Security page tiles (`hKind` = weak/reused/old). Back arrow returns to Settings > Security.

| Element | Spec |
|---|---|
| Wrapper | `overflow:auto; padding:32px 40px`, centred, anim `dw-up .35s`; column max-width **760**, gap 20 |
| Header | flex centre gap 14: **Back** button 44x44 radius 13, border `1px solid #e0deda`, fg `#4a4745`, hover border/text `#1b3fa0`, 20x20 left arrow (`M19 12H5M12 19l-7-7 7-7`, stroke 2.2); title "Password health" 30px/800 -0.03em. Back sets `view='settings', setTab='Security'` |
| Tabs | segmented, `align-self:flex-start`, options padding `9px 15px`: **Weak / Reused / Old**; selection = `hKind`. Default Weak |
| List card | Card; rows padding `16px 22px`, gap 14, separator `#f1efec`, entry anim `dw-up .3s ease-out both` with delay `index*0.05s` |
| Row | avatar tile 44x44 radius 12 (item colours, full opacity); name 16px/700; reason 14px/600 coloured by category; action button |
| Reason colours | weak `#b42318`, reused `#b7791f`, old `#1b3fa0` |
| Action | **Change password** (primary): height 42, padding `0 18px`, radius 12, bg `#1b3fa0`, white 15px/700; hover `#14307a`. Click -> selects that item and opens the Edit view (`openEditView(false)`) |
| Footer note | "Changes sync to Bitwarden on all your devices." 14px `#8a8785` |
| Empty state | none designed (every tab has rows in the sample data); the app should add one |

Sample data (item -> reason):
- **Weak**: Office WiFi -> "Easy to guess"; Postgres — Prod -> "Only 16 characters, no symbols".
- **Reused**: Ledgerline -> "Same as Vantage VPN"; Vantage VPN -> "Same as Ledgerline".
- **Old**: Atlas Studio -> "Not changed for 2 years"; Git Host -> "Not changed for 14 months"; Vantage VPN -> "Not changed for 13 months".
Tile labels on the Security page for these: "Weak passwords", "Reused passwords", "Older than a year".

---

## 5. Full-window states

### 5.1 Toast
| Property | Value |
|---|---|
| Placement | `position:absolute; left:50%; bottom:28px; transform:translateX(-50%)` inside the window; z 30 |
| Size | height 50, padding `0 22px 0 16px`, radius 999, gap 10 |
| Colours | bg `#201e1d`, text `#fff`, 15px/600 |
| Shadow | `0 14px 34px rgba(0,0,0,.25)` |
| Leading badge | 26x26 circle `#1b7a3f` with a white 15x15 check (`M20 6 9 17l-5-5`, stroke 3, round). Always a success check; there is no error variant in the design |
| Animation | `dw-toast .28s cubic-bezier(.2,.9,.3,1.2)` (slides up 14px with slight overshoot) |
| Lifetime | **2000 ms**; a new toast resets the timer and replaces text. No manual dismiss |
Toast copy used in this section: "Password copied"; "Shortcut saved"; "More languages are on the way"; "Change steps for one login from its Edit page"; "Pick a folder from the list"; "Encrypted export saved to Downloads"; "{device} signed out"; "Signed out on all other devices"; "Ask your Bitwarden admin to remove it"; "You have the latest version"; "Opening help…"; "Thanks — we read every note"; "Logs copied"; "Trash emptied"; "{name} restored"; "Deleted for good"; "Moved to trash"; "Everything is up to date"; "Still can’t reach Bitwarden" (uses U+2019 apostrophe).

### 5.2 Signed-out / sign-in screen (`signedOut`)
Triggered by Settings > Account > Sign out. Full overlay: `position:absolute; inset:0; z 46; bg #fff; grid 1fr 1fr; anim dw-fade .35s`. (Window 1280 -> two 640 halves.)

**Left (brand) half**: bg `#14307a`, padding `64px 56px`, column, `justify-content:space-between`.
- Top: mark 34x39 (white/`#b8c7ea`/`#7fa4ef`/`#3b74e8` quadrants; inverted shield variant: TL `#fff`, TR `#b8c7ea`, BL `#7fa4ef`, BR `#3b74e8`; paths: `M12 1H5.5A5 5 0 0 0 .5 6v8H12Z`, `M12 1h6.5a5 5 0 0 1 5 5v8H12Z`, `M.5 14H12v13.2C5 25.4.9 21 .5 14Z`, `M12 14h11.5c-.4 7-4.5 11.4-11.5 13.2Z`) + wordmark "Deskwarden" 22px/800 `#fff` letter-spacing -0.02em, gap 12.
- Middle (anim `dw-up .5s ease-out both`, gap 18): headline "Your Bitwarden vault, in every app." 48px/800 `#fff`, line-height 1.04, letter-spacing -0.035em; sub "Sign in once. Deskwarden fills SAP, VPNs and every desktop login for you." 18px `#b8c7ea`, line-height 1.5, max-width 30ch.
- Bottom: "Uses your existing Bitwarden account" 14px `#7fa4ef`.
- Mac only: traffic-light dots at left 18 / top 16 (12px circles `#ff5f57`, `#febc2e`, `#28c840`, gap 8).

**Right (form) half**: centred; Windows caption buttons top-right (colour `#6b6866`, 13px; ▢ at opacity .4). Form: width **420**, column gap 18, anim `dw-up .45s .08s ease-out both`.
| Element | Spec |
|---|---|
| Title | "Log in" 32px/800 -0.03em |
| Email field | label "Email" 14px/700 `#4a4745`; box height 50, radius 14, border `1px solid #e0deda`, padding `0 16px`; focus-within border `#1b3fa0` (transition .2s); input 16px transparent; default value "a.novak@ledgerline.com"; placeholder "you@company.com" |
| Master password field | label "Master password"; same box; `type=password`; placeholder "Your Bitwarden master password" |
| Server picker | label row: "Server" 14px/700 + `?` dot; below a segmented control (`align-self:flex-start`) with options **bitwarden.com / EU / Self-hosted**, default bitwarden.com (`s.server`); padding `9px 15px`. Tooltip (`tips.server`, 260 wide): "Most people use bitwarden.com. Pick EU if your account was made there, or Self-hosted if your company runs its own." (No URL input is shown for Self-hosted; the app must add one.) |
| Primary button | "Log in": height 56, margin-top 4, radius 16, bg `#1b3fa0`, white 17px/700, shadow `0 8px 20px rgba(27,63,160,.3)`; hover `#14307a`; active `translateY(1px)`; transition transform .15s. Click -> `signedOut=false` and `intro()` (splash) |
| Secondary button | "Log in with another device": height 52, radius 14, border `1px solid #e0deda`, 16px/600, gap 10, centred, leading phone icon 18x18 (`M5 2h14v20H5zM12 18h.01`); hover bg `#f7f6f5`. Same handler as Log in (no real device flow) |
There is no validation, error state, loading state or 2FA step in the design; the app needs those (use `ERROR #B42318` for errors).

### 5.3 Lock screen (`locked`)
Overlay `inset:0; z 45; bg #14307a`; column centred, gap 26; `opacity = lockLeaving ? 0 : 1` with `transition: opacity .4s ease`; mount anim `dw-fade .35s`. Windows caption buttons top-right (colour `#b8c7ea`); Mac traffic lights top-left.

Sequence of content (top to bottom):
1. Mark 64x75 (same inverted white shield), anim `dw-pop .4s ease-out`.
2. Heading "Welcome back, Alex" 32px/800 `#fff`, letter-spacing -0.02em; sub "Your vault is locked" 17px `#b8c7ea`; gap 8.
3. Stage content (anim `dw-up .3s ease-out`, gap 18, centred):

**Stage `hello`** (`lockHello` = stage is not `pw`, includes `scan`; default start stage):
- Button: height 62, padding `0 30px`, radius 18, bg `#fff`, text `#14307a` 17px/700, gap 12, leading 24x24 fingerprint icon (stroke 2, path from `lockHello` markup), hover `translateY(-2px)`, transition transform .15s. Label = `lockLabel`: "Unlock with Windows Hello" (Mac "Unlock with Touch ID"); during scan "Checking…". While scanning the button animates `dw-pulse-w .9s ease-out infinite` (white ring expanding to 22px).
- Click: ignored if already scanning; else stage -> `scan`, after **1300 ms** `unlock()` (sets `lockLeaving`, fades 420 ms, then `locked=false`, stage reset to `hello`).
- Link below: "Use master password instead" 15px/600 `#b8c7ea`, hover `#fff` -> stage `pw`.

**Stage `pw`** (`lockPw`):
- Row width **440**, gap 10: input box flex 1, height 58, radius 16, bg `#fff`, padding `0 18px`, `type=password`, placeholder "Master password", 17px; **Unlock** button height 58, padding `0 24px`, radius 16, bg `#3b74e8`, white 16px/700, hover `#2f63cf`.
- Enter key in the input or Unlock click -> `unlock()` (design accepts any password, including empty; the app must verify and show an error on failure).
- Link below: `helloLabel` text ("Unlock with Windows Hello") 15px/600 `#b8c7ea`, hover `#fff` -> back to stage `hello`.

Lock is entered via sidebar "Lock" row (hint "Lock" + Ctrl L), palette "Lock now", or the tray. `lock()` sets `locked=true, lockStage='hello', lockLeaving=false`. The Hello pane is shown regardless of the `hello` setting in the design (the app should show only the password stage when Hello is off/unavailable).

### 5.4 Loading / launch screen (`loading`)
Overlay `inset:0; z 50; bg #14307a`; column centred gap 28; `opacity = splashOpacity (leaving ? 0 : 1)`, `transition: opacity .45s ease`. Caption buttons (Win, `#b8c7ea`) / traffic lights (Mac) as above.
- Mark 84x98 (white shield) with `dw-breathe 1.6s ease-in-out infinite`.
- Text `splashText`: "Opening your vault" (online) or "Opening your saved copy" (offline); 24px/800 `#fff`, letter-spacing -0.01em.
- Progress: track 220x4 radius 999 `rgba(255,255,255,.16)`, overflow hidden; thumb 34% wide, 4 high, `#fff`, `dw-line 1.2s ease-in-out infinite` (slides -110% -> 330%). Indeterminate.
- Timeline (`intro()`): `loading=true, leaving=false, ready=false` at t=0; at **1800 ms** `leaving=true, ready=true` (fade out begins, vault view mounts; `showVault` requires `ready`); at **2300 ms** `loading=false` (overlay removed). Replayed by the canvas-only "Replay launch" button and after sign-in. (The vault list items then play their `dw-up` stagger.)

### 5.5 Offline state (`connection` prop = 'Offline')
Not a full-window overlay; it changes:
| Surface | Online | Offline |
|---|---|---|
| Top-bar sync pill (height 34, radius 999, padding `0 12px`, 13px/600, 7px dot) | "Synced", bg `#e8f3ec`, fg `#17673a`, dot `#1b7a3f` (or "Syncing…" while syncing) | "Offline", bg `#fef6e7`, fg `#8a5a06`, dot `#b7791f`. Hover `filter:brightness(.97)`; click = sync now (offline: toast "Still can’t reach Bitwarden") |
| Vault list banner (above the list, margin `0 14px 10px`, padding `12px 14px`, radius 14, bg `#fef6e7`, border `1px solid #f2d99b`, anim `dw-up .3s`) | hidden | shown: 18x18 wifi-off icon stroke `#8a5a06`; title "You're offline" 14px/700 `#7a4f05`; sub "Filling still works" 13px `#7a4f05`; right text button "Retry" 14px/700 `#7a4f05` -> toast "Still can’t reach Bitwarden" |
| Account page "Last synced" sub | "Just now" | "2 hours ago" |
| Splash text | "Opening your vault" | "Opening your saved copy" |
| Tray footer | "Synced · locks in 14 min" | "Offline · filling still works" |
Sync spin: icon rotates 360deg per click only when online.

---

## 6. Ambiguities / things the app must decide

1. Settings "Length" (Generator page) is read-only and shows only the Password length, but the Generator view length is per-mode; "Make new passwords as", "Include symbols" (`genSymbols`) are prefs that nothing else reads in the design. "Avoid look-alike characters" is shared state (`gLook`) with the Generator view, not a pref.
2. Hard-coded Security score "82" and "Fix 6 items to reach 100." vs tiles 2+2+3 = 7 (Vantage VPN counted in Reused and Old). Needs a real scoring formula.
3. Master-password "Change", "Manage" (2FA), "Export" (Security), "Check for updates", "+ Add an app" have no handlers in the design.
4. Account "Server" row is static "bitwarden.com" and ignores `s.server`.
5. `setTabs` (General/Apps/Security/Shortcuts/Account segmented list) and `setRows`/`SETTINGS` (4-switch legacy list incl. "Confirm with Windows Hello before filling", "Clear copied passwords after 30 seconds") are computed in JS but not rendered in the template; treat as dead.
6. The design shows no confirmation for Empty trash / Delete forever / Delete account / Sign out; Bitwarden semantics suggest the app should confirm.
7. Tooltip hint chips (`data-hint="Label|Mod+X"`) on New (Ctrl R), Copy (Ctrl C), Sync now (Ctrl Shift Y), Lock (Ctrl L) imply shortcuts that are not in the Shortcuts list (Ctrl R, Ctrl C, Ctrl Shift Y are undocumented there).
8. Lock screen has no wrong-password error state and a Hello path regardless of the setting; sign-in has no error/loading/2FA/self-hosted-URL states.
9. Font sizes are larger than v1 (15-16px body, 14px secondary); these are design-px at 1280x820, may need scaling in egui.

---

# Part D: autofill, brand, tray, card and bank logos

Source: `design_v2/_stripped.html` (1b L1270-1398, 1c L1399-1421, 1d L1422-1480, 1e L1481-1516) and `design_v2/_design.js`.
All px values are design-canvas px (1 canvas px = 1 egui point). Fonts: Archivo (weights 400/500/600/700/800/900 used); monospace = `ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`.

## 0. Shared tokens used by these four artboards

| Token | Hex | Notes |
|---|---|---|
| ink | `#201e1d` | primary text, tooltip bg |
| text-2 | `#4a4745` | field labels, icon buttons |
| muted | `#6b6866` | |
| faint | `#7d7979` | sub-lines |
| ghost | `#8a8785` | tertiary icons/text |
| ghost-2 | `#9b9797` | section label |
| blue-deep | `#14307a` | |
| blue (brand) | `#1b3fa0` | primary buttons |
| blue-bright | `#3b74e8` | |
| blue-soft | `#7fa4ef` | |
| blue-wash | `#eef2fc` | selected row / hover wash |
| blue-edge | `#b8c7ea` | |
| soft-bg | `#f7f6f5` | tiles / hover |
| search-bg | `#f3f2f0` | |
| hover-icon | `#f1f0ee` | |
| outline-btn border | `#e0deda` | |
| success wash / fg | `#e8f3ec` / `#17673a` (check ring `#1b7a3f` for strokes, check stroke `#1b7a3f`) | |
| SAP blue (host app, context only) | `#0a6ed1`, disabled `#7fb3e6` | |

Keyframes (global `<style>`, L51-64):

| Name | Definition |
|---|---|
| `dw-up` | opacity 0 -> 1, translateY(10px) -> none |
| `dw-fade` | opacity 0 -> 1 |
| `dw-drop` | opacity 0 -> 1, translateY(-8px) scale(.98) -> none |
| `dw-tip` | opacity 0 -> 1, translateY(4px) -> none |
| `dw-draw` | stroke-dashoffset 60 -> 0 |
| `dw-ring` | scale(.6) opacity 0 -> scale(1) opacity 1 |
| `dw-blink` | 0-49% opacity 1; 50-100% opacity 0 (used `1s step-end infinite`) |
| `dw-pulse-b` | box-shadow 0 0 0 0 rgba(27,63,160,.45) -> 0 0 0 16px rgba(27,63,160,0) |

Panel shadow (all overlay/tray panels): `0 24px 60px rgba(20,26,48,.28), 0 0 0 1px rgba(20,26,48,.06)`.
Panel enter easing: `cubic-bezier(0.2, 0.9, 0.3, 1.15)` (slight overshoot).

Shared panel header mark: 20 x 23 px quartered shield (see 1c for paths), then "Deskwarden" 15 px / 800.

---

## 1. Section 1b - Autofill ("Every moment the panel shows up")

Artboard: 780 px wide column, 16 px gap. Title row: badge "1b" (bg `#1b3fa0`, white, 13/700, pad 4x10, r8), title "Autofill" 20/800 (-0.01em), subtitle "Every moment the panel shows up" 14 `#7d7979`, right-aligned **Replay** button (h34, pad 0 14, r10, bg #fff, 1px `#dcdad7`, 14/600; hover border+text `#1b3fa0`).

Case switcher (segmented): container pad 4, r14, bg `#e3e1de`, align-self flex-start. Each segment pad 9x16, r11, 14/700, transition bg+shadow .2 s. Active: bg `#ffffff`, fg `#201e1d`, shadow `0 1px 3px rgba(32,30,29,.14)`; inactive: transparent, fg `#6b6866`. Labels in order (verbatim, `OVCASES`): **Fill**, **No match**, **Locked**, **Save login**, **New password** -> internal `fill`, `nomatch`, `locked`, `save`, `newpw`.

Stage: 780 x 600, r18, overflow hidden, bg `linear-gradient(160deg, #dfe5f2, #eceae6)`, ring `0 0 0 1px rgba(32,30,29,.08)`.

### 1.1 Demo host window (context only - not part of the product)
Fake "SAP Logon 760" window: abs left 70, top 56, 640 x 470, r12, white, shadow `0 24px 60px rgba(32,30,29,.22)`.
- Titlebar h40, bg `#f7f7f8`, bottom border `#ebebee`; title "SAP Logon 760" 13 `#444141`; Windows caption glyphs `—` `▢` `✕` (44x40 each, 12 `#6b6866`); macOS traffic lights (12 px dots `#ff5f57 #febc2e #28c840`) when platform=macOS.
- Body pad 44x56, gap 20: SAP chip (h34, pad 0 10, r6, `#0a6ed1`, white 16/800) + form title 22/700.
- Fields: label 14 `#4a4745`; input 380 x 46, r8, border 2 px, pad 0 14, transition border-color/box-shadow .2 s. Field 2 always border 2 px `#d9d9de`, 20 px text, letter-spacing .1em.
- Button 140 x 44, r8, white 15/700, transition bg .3 s.
- The panel anchor = the focused field 1 (left 70+56=126, field bottom). Panel abs `left:126; top:296` in the stage.

Host copy per case:

| | fill / nomatch / locked / save | newpw |
|---|---|---|
| Form title | `Log on to PRD` | `Change your password` |
| Label 1 | `User` | `New password` |
| Label 2 | `Password` | `Repeat password` |
| Button | `Log On` | `Change` |
| Field-1 font | 16 px, ls 0 | 20 px, ls .1em |

### 1.2 Anchoring / focus (`ovCaret`, `ovShowPanel`, field ring)
- State `ov` in {`open`, `typing`, `done`, `closed`}; `ovCase` in {fill,nomatch,locked,save,newpw}.
- Field 1 focus: border colour `#0a6ed1` + ring `0 0 0 4px rgba(10,110,209,.16)` while `ov==='open'`; when `ov==='closed'` border `#d9d9de`, no ring. (During `typing`/`done` border stays `#0a6ed1`, ring off.)
- `ovCaret = ov==='open' && !ovUser && ovCase!=='save'` -> 2 x 20 px caret, `#201e1d`, margin-left 1, `dw-blink 1s step-end infinite`.
- Clicking field 1 when `ov==='closed'` reopens the panel (`ov='open', ovSel=0`).
- Panel placement: top-left anchored at (126, 296) = directly under field 1, left-aligned to the field's left edge minus 0 (field left = 126). Entry animation `dw-drop .3 s cubic-bezier(.2,.9,.3,1.15)`, `transform-origin: top left`. The **Save login** card is the exception: top-right of the stage (`right:22; top:22`, width 370, `dw-drop .35 s`, z-index 5) - a notification-style prompt.
- Visibility: `ovShowPanel = ov==='open' && case==='fill'`; `ovNoMatch/ovLocked/ovSave/ovNewPw = ov==='open' && case==...`. After `ov` becomes `typing` / `done` / `closed` every panel disappears.

### 1.3 Case "Fill" - account list panel
Panel: width 400, bg #fff, r18, overflow hidden, panel shadow.
- Header: flex, gap 10, pad `14 16 10`. Mark 20x23, "Deskwarden" 15/800, spacer, **help badge** `?`: 20 x 20 circle, bg `#eceae7`, fg `#6b6866`, 12/800, `cursor: help`.
- Key-hint tooltip (`tips.ovkeys`, shown on mouse-enter of the `?` wrapper): abs `right:-8; bottom: calc(100% + 10px)`, width 230, pad 12x14, r12, bg `#201e1d`, white 14/500, line-height 1.6, shadow `0 12px 28px rgba(0,0,0,.22)`, z 40, pointer-events none, `dw-tip .16 s ease-out`. Text, 4 lines verbatim:
  ```
  ↑ ↓ choose an account
  Enter fill
  Tab fill and sign in
  Esc close
  ```
- List: pad `0 8 8`, column gap 2. Each row: flex, gap 13, pad 11x10, r13, cursor pointer, transition bg .15 s, enter animation `dw-up .3 s ease-out both` with `animation-delay = 0.08 + i*0.05 s` (0.08, 0.13, 0.18).
  - Avatar 42 x 42, r12, 14/700 initials, colours per account.
  - Text column: username 16/700; sub 14 `#7d7979`.
  - Selected (`r.active`, i === ovSel): row bg `#eef2fc` and a trailing **Enter badge**: h28, pad 0 10, r8, bg `#1b3fa0`, white 13/700, text `Enter`, `dw-fade .15 s`. Non-selected: transparent, no badge.
  - Hover (mouse-enter) sets `ovSel=i` only while `ov==='open'`. Click -> `ovFill(i)`.
- Data (`OV`, verbatim):

| # | user | sub | initials | avatar bg | avatar fg |
|---|---|---|---|---|---|
| 0 | `a.novak` | `SAP Production` | SP | `#1b3fa0` | `#ffffff` |
| 1 | `deploy-bot` | `Shared with me` | SP | `#dbe4f7` | `#14307a` |
| 2 | `a.novak.qa` | `SAP Quality` | SQ | `#eceae7` | `#201e1d` |

### 1.4 Fill animation (`ovFill(i)`, `ovDone`)
Guard: only runs if `ov==='open' && ovCase==='fill'`. Sets `ov='typing'` (panel vanishes), `ovSel=i`, clears `ovUser/ovPw`. Interval 42 ms per tick:
1. Types the account's username into field 1, one char per tick (caret hidden since `ovUser` non-empty... it is hidden once any char is present; `ov` is no longer `open` anyway).
2. Then 14 ticks of `•` appended to field 2 (`'•'.repeat(p)`, p to 14).
3. Then waits 300 ms and sets `ov='done'`.
- "Log On" button bg: `#7fb3e6` until `ovPw.length >= 14` or `ov==='done'`, then `#0a6ed1` (transition .3 s).
- **Done overlay** (`ovDone`): covers the host window below the title bar (`inset: 40px 0 0 0`), bg `rgba(255,255,255,.92)`, column centred, gap 16, `dw-fade .3 s`.
  - Check disc 84 x 84, bg `#e8f3ec`, `dw-ring .4 s cubic-bezier(.2,.9,.3,1.3)`; inside a 42 px checkmark (`M20 6 9 17l-5-5`, stroke `#1b7a3f`, width 2.6, round caps/joins, dasharray 60, `dw-draw .5 s .2s ease-out both`).
  - Title 24/800, subtitle 15 `#6b6866`. Copy by case (`doneText`):

| case | title | subtitle |
|---|---|---|
| fill | `Signed in` | `Filled by Deskwarden in 1.2 s` |
| save | `Login saved` | `Deskwarden fills it next time` |
| newpw | `Password updated` | `Saved to SAP Production` |
| other | `Signed in` | (empty) |

- Replay (`ovReplay`): stops timers, sets `ov='closed'` with fields cleared, then after 450 ms re-runs `setOvCase(current)` (panel drops in again).
- `setOvCase(c)`: clears timers; sets `{ovCase:c, ov:'open', ovSel:0, ovUser:'', ovPw:'', ovHello:false}`; for `save`: `ovUser='m.reyes'`, `ovPw='•'.repeat(12)` (field 1 shows m.reyes, no caret); for `newpw`: `ovNew = newPw()`.

### 1.5 Case "No match"
Panel 410 wide, pad 18, gap 14 (same position, `dw-drop .3 s`).
- Header: mark + "Deskwarden" only.
- Title block gap 4: `No login for this app yet` 19/800 (-0.01em); body `Save one now, or find it in your vault.` 15 `#7d7979`.
- Two equal buttons (gap 8, h46, r13, 15 px):
  - Primary `Save new login` bg `#1b3fa0`, white, 700; hover `#14307a`; -> `setOvCase('save')`.
  - Secondary `Search vault` border 1px `#e0deda`, 600; hover bg `#f7f6f5`; -> `setOvCase('fill')`.

### 1.6 Case "Locked"
Panel 410 wide, pad 18, gap 14.
- Header: mark, "Deskwarden", spacer, status pill `Locked` (12/700, pad 4x10, r999, bg `#eef2fc`, fg `#14307a`).
- Title `3 logins match` 19/800; body `Unlock to see and fill them.` 15 `#7d7979`.
- Unlock button: h54, r15, bg `#14307a`, white 16/700, gap 12, hover bg `#1b3fa0`; 22 px fingerprint icon (stroke currentColor 2) + label. Label: `Unlock with Windows Hello` (Windows) / `Unlock with Touch ID` (macOS).
- Click (`ovHelloGo`): if already `ovHello` ignore; else `ovHello=true`; label becomes `Checking…`, button animation `dw-pulse-b .9s ease-out infinite`; after 1100 ms `setOvCase('fill')` (i.e. unlocks and shows the Fill list).

### 1.7 Case "Save login" (`ovSave`) - top-right prompt
Card 370 wide at `right:22; top:22`, pad 18, gap 14, z 5.
- Header: mark, `Save this login?` 17/800, spacer, close button `✕` 32 x 32, r10, `#8a8785`, hover bg `#f1f0ee` (-> `ovNotNow` = `ov='closed'`).
- Preview block: flex, gap 12, pad 12, r14, bg `#f7f6f5`: 42 x 42 r12 tile bg `#0a6ed1`, `SAP` white 13/800; username `m.reyes` 16/700; masked `••••••••••••` 14 `#7d7979`, ls .1em.
- Buttons (gap 8, h46, r13): `Save` primary (`#1b3fa0`, hover `#14307a`, -> `ovSaveLogin`: `ov='done'` -> done overlay "Login saved"); `Not now` outline (-> `ovNotNow`).
- Footer link `Never for this app` 14/600, `#8a8785`, centred, hover `#201e1d` (also `ovNotNow` in the design).

### 1.8 Case "New password" (`ovNewPw`, `ovUseNew`)
Panel 410 wide, pad 18, gap 14.
- Header: mark, "Deskwarden", spacer, pill `Very strong` (12/700, pad 4x10, r999, bg `#e8f3ec`, fg `#17673a`) - static.
- Label `Suggested password` 15 `#7d7979`.
- Value: monospace 21/600, `word-break: break-all`, line-height 1.35, `dw-fade .3 s`. Generated by `newPw()`: 18 chars, upper+lower+digits+symbols, "look-alike safe" pool (no `I O l 1 0`; symbols `!@#$%&*?-_`), guarantees >= 1 char from every class, Fisher-Yates shuffled.
- Buttons (gap 8, h46): `Use this password` primary flex 1 (r13); regenerate icon button 46 x 46 r13, 1px `#e0deda`, icon circular-arrow 18 px stroke 2.2, colour `#4a4745`, hover border+icon `#1b3fa0` -> `ovNew = newPw()`.
- `ovUseNew`: `ov='typing'`; interval 35 ms, counter p++: field 1 (new password) shows `'•'.repeat(min(p,n))`, field 2 (repeat) `'•'.repeat(max(0, min(p-4,n)))` (lags by 4), n = password length (18). At `p >= n+4` waits 300 ms then `ov='done'` -> overlay `Password updated` / `Saved to SAP Production`.

### 1.9 Hover hint tooltip on this artboard
1b has **no** `data-hint-root`; the only hover tooltip is `tips.ovkeys` described in 1.3 (the dark 230 px card above the `?` badge, right edge offset -8, 10 px gap above). The generic key-chip hint (see 1d) is not active here.

---

## 2. Section 1c - Brand ("Quartered shield - four vaults, one guard")

Artboard 780 wide; title row badge `1c`, title `Brand`, subtitle `Quartered shield · four vaults, one guard`. Card: r18, white, ring `0 0 0 1px rgba(32,30,29,.08)`, pad 44, gap 40.

### 2.1 The mark - exact SVG (viewBox `0 0 24 28`, fill none) - **v2 geometry**
Note: differs from the v1 spec (v1 used viewBox with 2..22 bounds and r2.4). v2 body spans x 0.5-23.5, y 1-27.2, top corner radius 5.

```svg
<svg viewBox="0 0 24 28" fill="none">
  <path d="M12 1H5.5A5 5 0 0 0 .5 6v8H12Z"           fill="#14307a"/> <!-- top-left -->
  <path d="M12 1h6.5a5 5 0 0 1 5 5v8H12Z"            fill="#1b3fa0"/> <!-- top-right -->
  <path d="M.5 14H12v13.2C5 25.4.9 21 .5 14Z"        fill="#3b74e8"/> <!-- bottom-left -->
  <path d="M12 14h11.5c-.4 7-4.5 11.4-11.5 13.2Z"    fill="#7fa4ef"/> <!-- bottom-right -->
</svg>
```
Quadrants meet at (12,14); bottom tip at (12,27.2). The SVG is stretched `width/height: 100%` in a box with ratio 20:23 (~0.857:1; 72x84, 48x56, 32x37, 22x26, 14x16, 20x23, 22x26, 16x19, 13x15).

Reversed palette (on `#14307a` or `#1b3fa0` tiles): TL `#ffffff`, TR `#b8c7ea`, BL `#7fa4ef`, BR `#3b74e8`.
Mono/ink: all four paths `#201e1d`.

### 2.2 Wordmark / tagline (hero row)
Mark 72 x 84 + gap 20 + column gap 6: wordmark `Deskwarden` 48/800, letter-spacing -0.035em, line-height 1; tagline `Bitwarden, in every desktop app` 16 `#7d7979`.

### 2.3 Size ramp
Row `align-items:flex-end; gap:26`. Marks (full colour) at 48x56, 32x37, 22x26, 14x16, then caption `Holds at 14 px — tray and field badge` 14 `#8a8785`, padding-bottom 2.

### 2.4 App-icon tiles
Grid of 3, gap 14, each h150, r18:
| Tile | Background | Mark |
|---|---|---|
| Navy | `#14307a` flat | reversed palette, 52 x 60 |
| Light | `#f3f2f0` page; inner icon 92x92, r24, bg `#1b3fa0`, shadow `0 10px 24px rgba(27,63,160,.35)` | reversed palette, 46 x 53 |
| Mono | white with 1px `#ebe9e6` border | all `#201e1d`, 52 x 60 |

### 2.5 Brand colours (grid of 4, gap 12; swatch h64 r14; name 14/700; hex 13 `#8a8785`)
| Name | Hex |
|---|---|
| Deep | `#14307A` |
| Brand | `#1B3FA0` |
| Bright | `#3B74E8` |
| Light | `#7FA4EF` |

(Mapping: Deep = mark TL, Brand = TR, Bright = BL, Light = BR.)

---

## 3. Section 1d - Tray ("Quick access, no window")

Artboard 520 wide. Title: badge `1d`, name = `trayName` (`Tray` on Windows, `Menu bar` on macOS), subtitle `Quick access, no window`.

Stage: `data-hint-root="tray"`, 520 x 640, r18, overflow hidden, bg `linear-gradient(160deg, #cfd8ec, #e8e5df)`, ring `0 0 0 1px rgba(32,30,29,.08)`, flex column.

### 3.1 Taskbar context
- **Windows** (default): bottom bar h48, bg `rgba(243,243,243,.92)` + backdrop blur 10, top border `rgba(0,0,0,.06)`, items right-aligned gap 4, pad 0 10, 12 px `#201e1d`: chevron-up (32x36, 16 px icon `#4a4745`), **Deskwarden tray icon** tile 36x36 r6 bg `rgba(0,0,0,.07)` containing full-colour mark 16x19 (shown as active), wifi glyph (32x36), clock column right-aligned (line-height 1.25): `14:20` / `18/11/2026`.
- **macOS**: top menu bar h30, bg `rgba(255,255,255,.55)` + blur 10, right-aligned gap 16, pad 0 14, 13/600; Deskwarden item = mono ink mark 13x15 in chip pad 3x6 r5 bg `rgba(0,0,0,.12)`, then clock `Wed 18 Nov 14:20`. Panel justify = flex-start (under bar) vs flex-end (above taskbar) -> `trayJustify`.

### 3.2 Panel
Container: flex-1, column, `align-items:flex-end`, `justify-content: trayJustify` (flex-end on Windows so it sits just above the taskbar; flex-start on macOS), padding 14 (panel is 14 px from right edge).
Panel: width 390, white, r22, panel shadow, pad 16, column gap 14, entry `dw-up .35 s ease-out`.

1. **Header row** (gap 10): mark 22 x 26; `Deskwarden` 16/800 flex 1; two icon buttons 36 x 36, r11, colour `#4a4745`, hover bg `#f1f0ee`, 18 px icons stroke 2:
   - Lock (padlock `M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4`) -> `lockNow`; hint `Lock` + chip `Mod+L` (Ctrl L / ⌘ L).
   - Open window (external-link arrow `M15 3h6v6M10 14 21 3M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6`), no hint attribute.
2. **Search**: h44, r13, bg `#f3f2f0`, pad 0 14, gap 10; magnifier 16 px stroke `#8a8785` 2.2; borderless input placeholder `Search`, 15 px.
3. **Section label** `Recent` (rendered with `text-transform: uppercase` -> RECENT): 12/800, ls .12em, `#9b9797`, pad 0 4.
4. **Recent list** (margin-top -6, gap 2): first four **logins** from the vault (`ITEMS.filter(type==='login').slice(0,4)`): SAP Production / a.novak, Ledgerline / a.novak@ledgerline.com, Atlas Studio / a.novak@studio.atlas.com, Vantage VPN / a.novak@vantage.io. Row: flex, gap 12, pad 8, r13, hover bg `#f7f6f5`, enter `dw-up .3 s ease-out both` delay `0.05 + j*0.05` s.
   - Avatar 38 x 38, r11, 13/700 initials or favicon (20x20 contain, `dw-fade .25 s`; ring `inset 0 0 0 1px #e6e3df` and bg white when favicon). Uses `visual(it)` (see 1e) - same favicon/brand logic as vault list.
   - Name 15/700; username 13 `#8a8785`, single-line ellipsis.
   - Two action buttons 36 x 36, r10, `#6b6866`, hover bg `#eef2fc` colour `#1b3fa0`, 17 px icons stroke 2:
     - Copy username (person icon `M20 21a8 8 0 0 0-16 0M12 13a5 5 0 1 0 0-10 5 5 0 0 0 0 10z`), hint `Copy username` + `Mod+B`, toast `Username copied`.
     - Copy password (padlock icon), hint `Copy password` + `Mod+C`, toast `Password copied`.
5. **Action tiles**: grid 3 cols gap 8; each h74, r15, bg `#f7f6f5`, column centred gap 7, 13/700 `#201e1d`, transition bg .15 s; hover bg `#eef2fc`, colour `#14307a`; 20 px icons stroke 2: `Generate` (circular arrow `M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6`), `New item` (plus `M12 5v14M5 12h14`), `Share` (paper plane `M22 2 11 13M22 2l-7 20-4-9-9-4z`).
6. **Status footer**: row gap 8, pad 2x4, 13 `#6b6866`; dot 7 x 7 round + text. Online: `Synced · locks in 14 min`, dot `#1b7a3f`. Offline (connection prop = Offline): `Offline · filling still works`, dot `#b7791f`.

### 3.3 Hover hint tooltip (`hintTray`)
Mechanism: a global `mouseover` handler finds the closest `[data-hint]` inside a `[data-hint-root]`; attribute format `Label|Mod+Key` (split on `|`; keys split on `+`, `Mod` -> `⌘`/`Ctrl`, `Shift` -> `⇧`/`Shift`). Computed position: x = element centre relative to root, clamped to [90, rootWidth-90]; if element top < 56 the tip goes below (`y = bottom+8`, `translateX(-50%)`), else above (`y = top-8`, `translate(-50%,-100%)`). Cleared on mouse-leave of any hint element or any mousedown.
Style: abs, z 80, pointer-events none, flex gap 8 align centre, pad `7 8 7 11`, r10, bg `#201e1d`, white 13/600, nowrap, shadow `0 8px 20px rgba(0,0,0,.25)`, `dw-fade .14 s ease-out`. Key chips: gap 3; each min-width 20, h20, pad 0 5, r5, bg `rgba(255,255,255,.16)`, 11/700, centred.
Tray hints: `Lock` [Ctrl][L]; `Copy username` [Ctrl][B]; `Copy password` [Ctrl][C].

---

## 4. Section 1e - Card & bank logos
Artboard 1280 wide. Title: badge `1e`, `Card & bank logos`, subtitle `Network from the card number · bank from the linked login · text when an image can't load`. Card r18 white ring, pad 36, gap 22.

Reference sheet: section label `Card networks` (13/800 UPPERCASE ls .12em `#8a8785`) with note `Image · text fallback` (13 `#9b9797`); 3-column grid gap 12 of cells (border 1px `#ebe9e6`, r16, pad 14, gap 10): two 84 x 54 r10 tiles on the network colour (left = Image mode, 62x24 logo, text fallback 16/900; right = Text mode, 15/900) + column: name 15/700 and rule 13 `#8a8785`. Divider 1px `#f1efec`. `Banks` label (same note); 5-column grid gap 12, cells r16 pad 14 gap 10: two flex-1 h50 r10 tiles (image 57x22 / text 14px, text 13px) + name 14/700.

### 4.1 NETS table (order as in sheet; first match wins in `brandOf` array order)
`brandOf(number)`: strip non-digits; empty -> null; first NETS entry whose regex matches the digits wins.

| key | Name | Short (text fallback) | Long | Style | Colour | Rule text (verbatim) | Regex |
|---|---|---|---|---|---|---|---|
| amex | American Express | AMEX | AMERICAN EXPRESS | normal | `#2e77bc` | Starts with 34 or 37 | `^3[47]` |
| diners | Diners Club | DC | Diners Club | normal | `#0079be` | Starts with 30, 36 or 38 | `^3(0[0-5]\|[68])` |
| jcb | JCB | JCB | JCB | normal | `#0b4ea2` | Starts with 35 | `^35` |
| visa | Visa | VISA | VISA | **italic** | `#1a1f71` | Starts with 4 | `^4` |
| mir | Mir | MIR | MIR | normal | `#0f754e` | Starts with 2200–2204 | `^220[0-4]` |
| mc | Mastercard | MC | mastercard | normal | `#201e1d` | Starts with 51–55 or 22–27 | `^(5[1-5]\|2[2-7])` |
| maestro | Maestro | MAE | maestro | normal | `#0a3a82` | Starts with 50, 56–58, 63 or 67 | `^(50\|5[6-8]\|6[37])` |
| discover | Discover | DISC | DISCOVER | normal | `#e65c1f` | Starts with 6011 or 65 | `^(6011\|65)` |
| unionpay | UnionPay | UP | UnionPay | normal | `#d7001f` | Starts with 62 | `^62` |

Image slugs (SimpleIcons): americanexpress, dinersclub, jcb, visa, mir, mastercard, maestro, discover, unionpay. Note regex overlap: Mir `2200-2204` is tested before Mastercard `22-27`, so Mir wins for those prefixes; Maestro `50` only matches after Mastercard `51-55` has not.

### 4.2 BANKS table
| key | Name | Short | Colour | Image slug |
|---|---|---|---|---|
| chase | Chase | CHASE | `#117aca` | chase |
| revolut | Revolut | R | `#191c1f` | revolut |
| hsbc | HSBC | HSBC | `#db0011` | hsbc |
| barclays | Barclays | B | `#00aeef` | barclays |
| monzo | Monzo | M | `#14233c` | monzo |
| wise | Wise | WISE | `#163300` | wise |
| citi | Citi | citi | `#003b70` | citi |
| bofa | Bank of America | BofA | `#012169` | bankofamerica |
| ing | ING | ING | `#ff6200` | ing |
| santander | Santander | S | `#ec0000` | santander |

Banks: `long` = Name, style normal. Bank logoset keys are prefixed `b:`.

### 4.3 Logo images - no inline data
There are **no** embedded SVG/base64 logos. Every logo is a remote monochrome SVG `https://cdn.simpleicons.org/<slug>/ffffff` (white glyph, 19 URLs = 9 nets + 10 banks), drawn as a CSS `background-image` (`contain`, `no-repeat`, `center`, `role="img"`, `aria-label` = short label) on the brand-coloured tile. Favicons come from `https://icons.duckduckgo.com/ip3/<site>.ico`. In the product these must be bundled assets or a text fallback (Bitwarden-compatible: nothing here is stored in the vault; derived at render time).

### 4.4 `mark(key, force)` behaviour
Input: logoset key (null for none) and optional forced mode. Mode = `force` or the `cardLogos` prop (`Image` default | `Text`).
Returns:
- `noBrand = !L` (no key / unknown key) - draw nothing in the logo slot.
- `useImg = L && !fail && mode !== 'Text'` - draw the SVG image.
- `useText = L && (fail || mode==='Text')` - draw the text label instead (`fail` = the image errored in a preload `new Image()` probe at mount, tracked in `state.logoFail[key]`).
- `label = L.short`, `longLabel = L.long`, `fstyle = L.style` (italic for Visa), `logoCss = url(...)`.
Text fallback typography: white, weight 900, letter-spacing .02em, line-height 1, nowrap; size depends on slot (see 4.6). Image max-width 86% of the tile.

### 4.5 How a card is composed (`visual(it)`)
```
net     = it.type==='card' ? brandOf(it.card.number) : null
bank    = it.bank || it.card?.bank
mainKey = bank ? 'b:'+bank : net          // bank wins as the main badge
fav     = !mainKey && it.site && !favFail[it.id]
```
- Main tile colour: logoset colour if mainKey, else favicon -> white tile with 1px inset ring `#e6e3df`, else item colour (`av`/`avFg`).
- **Secondary network chip** (`hasNet = bank && net`): when a bank is the main badge and the card number identifies a network, the network is drawn as a small chip overlapping the bottom-right of the tile.
- A bank is *linked from a login*: a login item may have `bank: 'chase'` (Chase Business, Revolut); a card has `card.bank`. `linked = ITEMS.find(x => x.bank===card.bank && x.type==='login')`. Detail header shows the linked login's name as a pill that opens it (`linkOpen` sets selection to that login and filter `all`).
- Sample data: Corporate Visa (4821 0093 1180 4821, bank chase, exp 08 / 28) -> Visa network on Chase tile; Personal Mastercard (5412..., bank revolut) -> Mastercard on Revolut tile. Chase Business `#117aca` and Revolut `#191c1f` logins have bank set and show the bank mark as their main badge.

### 4.6 Badge composition by surface
| Surface | Main tile | Main logo | Text fallback | Network chip |
|---|---|---|---|---|
| Vault list row | 44 x 44, r12, `av`; initials 15/700, favicon 22 | image 36 x 14 (max 86%) | 13/900 short label | pos abs `right:-7; bottom:-6`, 30 x 20, r6, bg net colour, 2px white border, overflow hidden; logo 26 x 10; text 7/900 short |
| Detail header | 72 x 72, r20; initials 24/800; favicon 34; | image 57 x 22 | 19/900 short | abs `right:-9; bottom:-7`, 42 x 28, r8, 3px border `#f5f4f2`; logo 34 x 13; text 9/900 short |
| Tray recent | 38 x 38, r11; initials 13/700; favicon 20 | via `visual()` - (tray row markup uses initials/favicon only) | - | none |
| Card face (detail) bank name | inline left of the card face | image 62 x 24 | 18/900 **longLabel** (e.g. "Bank of America", "Chase") | - |
| Card face network (right) | 76 x 34 box, right-aligned | image 78 x 30 (max 86%) | 20/900 **longLabel** (e.g. "AMERICAN EXPRESS", "mastercard", "VISA" italic) | - |
| Reference sheet nets | 84 x 54 r10 | 62 x 24 | 16/900 (Image tile) / 15/900 (Text tile), short | - |
| Reference sheet banks | flex-1 h50 r10 | 57 x 22 | 14/900 / 13/900, short | - |

Card-face background (detail, `cardBg`): `linear-gradient(135deg, base 0%, base 45%, #201e1d 160%)` where `base` = bank colour if bank, else network colour, else `#14307a`.

Linked-login pill (card face, only when `linkHas`): h28, pad 0 6 0 11, r999, bg `rgba(255,255,255,.16)` (hover `.28`), white 13/700, label = login name with ellipsis + 14 px chevron; hint `Open linked login`. Next to it a 28 x 28 round pencil button (hover bg `rgba(255,255,255,.2)`), hint `Change linked login` [Ctrl][E] -> opens edit view.

### 4.6a Edit-form helpers (referenced in JS, context)
- `eBrand = mark(brandOf(eCardNum))` + `has`, `bg` (net colour or `#201e1d`) -> live network badge while typing the card number.
- `bankOpts`: `None` + every login that has `bank` (`Chase Business`, `Revolut`); tile colour = bank colour or `#eceae7`; selected border `#1b3fa0` + bg `#f5f7fd`, otherwise border `#e8e6e3` + bg `#fff`.

---

## 5. Ambiguities / notes for implementer
1. Tray panel `Open window` button has no `data-hint`; no tooltip text defined (suggest none or "Open Deskwarden").
2. Tray `Search` input is an uncontrolled `<input>` in the design; behaviour (filtering RECENT) not specified.
3. Tray Recent rows use initials/favicon via `visual()` but the row markup has no logo/useImg branch, so bank/network logos never show in the tray (only favicon or initials).
4. `Never for this app` in the Save card is wired to `ovNotNow` (same as Not now) - the "never" persistence is not modelled.
5. Fill "Enter" badge shows on hover/keyboard selection; the key hints text mentions `Tab fill and sign in` and `Esc close` but only click/hover are implemented in the design.
6. Mark geometry: the v1 spec (viewBox 24x28, body 2..22) differs from v2 (body .5..23.5, r5, tip y 27.2); this spec follows v2 exactly.
7. Logos rely on `cdn.simpleicons.org`/DuckDuckGo at design time; the Rust app should bundle SVGs (white on brand tile) or use the text fallback.
8. `dw-pulse-b` Locked "Checking…" lasts 1100 ms then loads the Fill list; the real Windows Hello result is not designed.

---

# Appendix: what 0.15 already has

Paths are under deskwarden\src unless stated. Based on grep/read only (no builds). "VW" = vault_window/.
Settings live in `settings.rs` (`struct Settings`, serde(default), ~line 1059; `Shortcuts` line 828) and are edited in `prefs_ui.rs` (`enum Section`, line 1212; `draw_*` per section).
Existing prefs sections (12): General, Shortcuts, View, Lock, Breaches, Vault, Sync, Sharing, Local API, Clipboard, Updates, About. General holds ONE toggle (`prompt_on_match`).
Global fact: the app is LIGHT-THEME ONLY (`theme.rs:825 ctx.set_theme(egui::Theme::Light)`; no dark palette, no theme setting).

## Shell and vault

| Feature | Status | Where | Notes |
|---|---|---|---|
| Three-pane vault window | EXISTS | VW/mod.rs (run loop), VW/sidebar.rs `draw_sidebar`, VW/item_list.rs, VW/detail.rs | 42k-line mod.rs; sidebar / list / detail panes, slide-in detail (detail_slide.rs) |
| Sidebar: All items, Favorites, Logins, Cards, Secure notes, Trash | EXISTS | sidebar.rs `SidebarFilter` + rows ~L1227-1249 | Also has extras v2 lacks: Apps, Passkeys, Identities, SSH keys, Archive, Password health, Sends (+3 sub-filters), per-org filters. Live counts via `badge_for`/`count_for` |
| Sidebar: Shared | PARTIAL | sidebar.rs `RECEIVED_ROW_LABEL = "Shared with me"`, `Screens.received` | Exists as "Shared with me" (received shares, read-only) under a SHARING section |
| Sidebar: Generator, Settings nav rows | MISSING | - | Preferences opens as modal/window via account menu / tray (`prefs_ui::draw_prefs_window`, `draw_prefs_modal`). Generator is not a screen (see Tools) |
| Folders list with counts, New folder | EXISTS | sidebar.rs FOLDERS header with "+" (`SidebarAction::NewFolder`), `EditFolder`, virtual "No Folder" (`SidebarFilter::Unfiled`); drag item onto folder (`MoveItemToFolder`) | |
| Folder create / rename / delete | EXISTS | VW/folder_modal.rs (`FolderEditState`, `draw_folder_edit_modal`); rest/api.rs `create_folder/update_folder/delete_folder` | |
| Lock button | PARTIAL | VW/mod.rs `LOCK_LABEL="Lock"`, `LOCK_SHORTCUT="CTRL+L"` (account dropdown row + Ctrl+L) | No titlebar/sidebar Lock button: "the titlebar Lock pill is gone" by owner decision; sidebar bottom shows auto-lock countdown ("Locks in 11:42") |
| Global search with Ctrl K | PARTIAL | VW/mod.rs ~L3007-3050 (`ctrl_k` -> `request_focus("vault-search")`) | Ctrl K only focuses the in-window search field; it is NOT global (no cross-app hotkey) and is not a palette |
| Sync status pill (Synced/Offline) | PARTIAL | VW/mod.rs ~L2883 "Synced 2m ago" pill that doubles as sync button; `VAULT_NOT_REFRESHED_PILL` L14746 | "Synced Xm ago" and "Vault not refreshed" states exist; no explicit "Offline" state |
| Offline mode | PARTIAL | settings `cache_vault_to_disk` (default false), `read_through_cache` (default true); vault_disk_cache.rs, vault_cache.rs; prefs Vault page `DISK_CACHE_LABEL` | Encrypted on-disk vault copy usable when server unreachable; no user-facing Offline toggle/pill |
| Account avatar | EXISTS | VW/mod.rs `avatar_initials` (~L2797), account dropdown (~L14320+); theme.rs `avatar*`, `initials` | Initials tile; dropdown has Lock, Add account..., Remove this account..., Export vault..., account switch. No uploaded photo |
| Launch / intro loading screen | EXISTS | loading_ui.rs (7a loading, 7b slow/failed); VW/mod.rs first-window skeleton (`VaultSkeleton` login_ui.rs L1208) | Previews: first_window_loading/slow/unreachable PNGs |
| Toasts | PARTIAL | VW/detail.rs `copy_toast_id/forget_copy_toast` (copy toast); notice_card.rs (Win32 notice card for daemon); inline statuses | No general toast system; a copy-toast + per-screen messages |
| Detail actions Fill / Share / Edit / Favourite | EXISTS | VW/detail.rs `DetailAction::{Edit, ToggleFavorite, ...}` (L677+), "Fill in app" header button (ctrl+shift+F), `Share` (Send a record + share with person in share_ui.rs) | Favourite = star at detail.rs ~L4432 |
| 2FA code with countdown | EXISTS | VW/detail.rs TOTP row (96x4 progress bar + seconds, `TotpState`, `current_totp_seconds_left`); `CopyTotp`; otpauth.rs | |
| "Fills in" (apps/sites) | EXISTS | VW/detail.rs "AUTOFILL TARGETS" card; app_match.rs `AppMatch {process, title}` stored in custom field `APP_MATCH_FIELD_NAME`; match_engine.rs; uri_match.rs; browser_url.rs | Apps via custom field (Bitwarden-compatible, other clients see a field) + URIs per Bitwarden rules |
| Card detail with network + bank logo | EXISTS | VW/detail.rs card body; card_brand.rs (`CardBrand`, `brand_for_number`), card_mark.rs (wordmark or user-supplied PNG), brand_mark.rs; bank via custom field `favicon::BANK_DOMAIN_FIELD`="deskwarden:bank-domain" | See Card & bank logos |
| Secure notes | EXISTS | `ItemKind::SecureNote`; detail NOTES card; edit form | |
| Custom fields | EXISTS | detail.rs shows fields; detail_edit.rs edits them (text/hidden/boolean) | |
| Shared-with-me view | EXISTS | sidebar `Screens.received`, VW/send_ui.rs, VW/share_ui.rs (accept invite by pasting link) | Real org-share to a person also exists (share_ui.rs, rest/sharing.rs) |

## Edit

| Feature | Status | Where | Notes |
|---|---|---|---|
| Create/edit login, card, secure note | EXISTS | VW/detail_edit.rs (`NewItem`, kinds Login/Card/SecureNote/Identity/SshKey) | Also Identity and SSH key; Ctrl+N new |
| Custom item icon | EXISTS (stored as custom field) | item_icon.rs `ICON_FIELD_NAME = "deskwarden:icon"` (one JSON field, `IconChoice`, `read_icon_file`), VW/icon_modal.rs ("Select icon": URL or file), favicon.rs (`fetch_chosen_icon`, `icon_domain_for`) | Per-item visual is a custom field on the cipher (syncs, invisible/harmless to other clients). Options: favicon (auto), URL, uploaded image file. Monogram fallback = `theme::initials`/avatar. NO "app icon" (exe icon) source, NO colour picker for initials (colour is deterministic from name) |
| Sign-in steps editor + Test run | EXISTS | VW/sequence_builder.rs (4a), `detail_edit::template_editor` (4c), VW/rehearsal.rs + scratch_window.rs (4d test run), VW/preflight.rs (gate, card removed); key_sequence.rs, injector/sequence.rs (`Step::{Text,Key,Wait}`) | Per-item sequence stored on item (custom field); fill shortcut `Shortcuts.sequence` |
| Apps a login fills in (picker) | EXISTS | `detail_edit::app_window_picker`, `running_app_rows` (8b); app_candidates.rs, window_list.rs | Match by process (+ hosted-frame window title); not three radios |
| Delete | EXISTS | VW/delete_modal.rs (trash / permanent / archive, 2-click confirm) | |
| Edit history, websites editor, dirty/discard confirm | EXISTS (extra) | detail_edit.rs | previews edit_login_history, edit_discard_confirm |

## Modals

| Feature | Status | Where | Notes |
|---|---|---|---|
| Import wizard (Bitwarden, 1Password, Keeper, LastPass, Chrome/Edge, CSV) | MISSING | - | Only a per-record Send import (record/import.rs, VW/record_ui.rs "Import from Send") exists. No vault import of any format. No CSV code (grep csv hits only a test banning it) |
| Vault export (encrypted) | EXISTS | vault_export.rs (bw spawn), rest/export.rs `encrypted_json`/`export_document` (direct REST), VW/mod.rs `EXPORT_VAULT="Export vault..."` | Encrypted JSON only, by deliberate decision (no CSV/plain JSON) - compatible with Bitwarden import |
| Command palette Ctrl K (items + actions) | MISSING | VW/mod.rs L3048 only focuses search | No palette overlay, no action list |
| New item chooser | PARTIAL | VW/mod.rs Ctrl+N and new-item control; detail_edit `NewItem` kind | Check whether a type chooser modal vs dropdown: not confirmed as a v2-style chooser |
| Send/share link creation with options | PARTIAL | VW/send_ui.rs (`draw_access_block`: lifetime/expiry, max views `max_access_count`, share password), VW/record_ui.rs (field-level record Send), send.rs | Expiry, max opens, password EXIST. Hide-my-email MISSING (writes `hideEmail` null on purpose). Recipient/email-only deliberately not built (server 501) |
| Send detail: access events / extend / revoke | PARTIAL | VW/send_ui.rs, sent_history.rs, receive_history.rs; extend by a week, `RevokeSend` | Extend + revoke + state (Opened/Expired/etc) + access count EXIST. Per-access timeline cannot exist (Bitwarden Send has no audit) |
| Add 2FA via QR (screen/region/webcam) or manual key | EXISTS | VW/totp_add.rs (6a-6d), screen_capture.rs, region_overlay.rs, webcam.rs, qr.rs, scan_history.rs, otpauth.rs; also open image file | |

## Tools

| Feature | Status | Where | Notes |
|---|---|---|---|
| Password generator | PARTIAL | password_gen.rs (`generate`, `generate_password`, `generate_passphrase`); vault_bridge `PasswordRecipe {length, uppercase, lowercase, number, special, min_number, min_special, avoid_ambiguous}`, `PassphraseRecipe {words, separator, capitalize, include_number}`; UI in detail_edit.rs (`GENERATOR_TITLE`, generator state ~L2233) and generate_prompt.rs (Win32 card, 3d) | Password + passphrase with classes, avoid-ambiguous, length: EXIST. PIN mode MISSING. Strength meter: `password_strength.rs` + `strength_bars` (4 bars) exist for item passwords. No standalone Generator screen, no persisted generator defaults in settings |
| Password health (weak / reused / old, breached) | PARTIAL | VW/password_health.rs (`HealthFilter::{All, Reused, Weak, Breached}`), breach.rs, breach_scan.rs, prefs Breaches page (`check_breaches`) | Weak, Reused, Breached EXIST (HIBP-style). "Old" MISSING |
| Trash with restore and 30-day purge | PARTIAL | sidebar `OutOfVault::Trash`, `RowCommand::Restore`, `PurgeForever` (detail.rs/item_list.rs), delete_modal.rs | Trash view, restore, permanent delete EXIST. 30-day auto-purge is Bitwarden SERVER behaviour; no UI text/countdown found |

## Settings pages (v2 list vs current)

| v2 page / item | Status | Where / key | Notes |
|---|---|---|---|
| General: open at sign-in | PARTIAL | installer/deskwarden.iss L159 (Run key `--autostart`, Inno task `autostart`); main.rs `AUTOSTART_FLAG`; autostart_repair.rs | Installer-only; NO in-app setting/toggle |
| General: start in tray | PARTIAL | `--autostart` launches to tray without window (main.rs L11184+) | No setting; implied by autostart |
| General: keep running on close | PARTIAL | `Settings.keep_ui_loaded` ("Open the vault instantly", prefs `UI_LOADED_LABEL`), `keep_backend_running` ("Keep the Bitwarden CLI running") | Daemon always stays in tray; these are memory trade-offs, not "keep running on close" |
| General: theme Light/Dark/System | MISSING | theme.rs:825 forces Light | Whole palette is light tokens (CANVAS, CARD, ...) |
| General: text size | MISSING | - | no zoom/scale setting |
| General: language | MISSING | - | English only; Archivo + NotoSans-Cyrillic bundled (fonts) |
| Autofill: suggest on field click | PARTIAL | `Settings.prompt_on_match` ("Show autofill prompts", prefs General) | Single global switch for overlay; overlay raises on field focus/match (window_watch.rs, app.rs). No separate "suggest on click" |
| Autofill: panel position | MISSING | prompt_card.rs `place` / `app::overlay_position` | Anchored beside the focused field automatically; not configurable |
| Autofill: auto sign-in after fill (press Enter) | PARTIAL | per-item sequences can include Enter (key_sequence.rs, injector); `Settings.shortcuts.sequence` | No global default toggle |
| Autofill: type 2FA codes | PARTIAL | `Shortcuts.totp` chord + TOTP step in sequences | No global "type 2FA after fill" setting |
| Autofill: match apps by name/window/exact | PARTIAL | match_engine.rs (process; hosted-frame title), `AppMatch{process,title}`; uri_match.rs for URLs (Bitwarden Base/Host/Exact...) | Not a user setting; engine rule fixed |
| Autofill: default sign-in steps | MISSING | - | sequences are per item only |
| Autofill: fill shortcut | EXISTS | `Settings.shortcuts: Shortcuts{picker, username, password, totp, sequence}` (default picker CTRL+ALT+B); prefs `draw_shortcuts` (L4816), hotkey.rs | Rebindable chords + live status |
| Apps (per-app rules) | PARTIAL | `Settings.never_save_for_apps: Vec<String>` (no UI page found); per-item AppMatch targets; sidebar "Apps" filter | No Apps settings page |
| Security: lock after N | EXISTS | `auto_lock_enabled`, `auto_lock_minutes`; prefs `draw_lock` (`AUTO_LOCK_*_LABEL`) | |
| Security: lock on sleep / screen lock | PARTIAL | away_lock.rs (WM_WTSSESSION_CHANGE lock, WM_POWERBROADCAST suspend) - always on | No toggles in Settings |
| Security: clear clipboard after | EXISTS | `clear_clipboard`, `clear_clipboard_seconds`, `clear_clipboard_on_lock/_on_account_change/_on_quit`; clipboard.rs; prefs Clipboard | More granular than v2 |
| Security: hide passwords during screen share | MISSING | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` is applied only to Win32 cards (generate_prompt, prompt_card, locked_card, picker_prompt, unlock_prompt, save_login_card, notice_card, region_overlay) | Not applied to the egui vault window; no setting |
| Vault: show icons | EXISTS | `fetch_icons` ("Show site icons"), `fetch_icons_direct`, `icons_ignore_tls_on_private_hosts`, `use_brand_logos`, `reveal_totp_seed`; prefs View page | |
| Vault: offer cards/notes in autofill | MISSING | - | |
| Vault: default folder | MISSING | grep hit only in app_profile.rs (unrelated) | |
| Vault: manage folders / trash | PARTIAL | sidebar + folder_modal.rs | In vault window, not in settings |
| Generator defaults | MISSING | - | recipe defaults are code (`PassphraseRecipe::default`, `PasswordRecipe`), not persisted settings |
| Notifications (shared link opened, weak/reused, new device, update ready) | MISSING | notice_card.rs is for fill-refusal notices only; update status shown in prefs About/Updates | No notification settings |
| Import & export page | PARTIAL | export only (account menu) | no page, no import |
| Devices (list, sign out one / all others) | MISSING | no `/devices` API use (rest/api.rs has none) | Bitwarden has /devices endpoints (listing, deauthorize) |
| Shortcuts (rebind fill/open/search/generate/lock) | PARTIAL | prefs `draw_shortcuts`; `Shortcuts` has picker, username, password, totp, sequence | Global chords are fill-oriented; open/search/generate/lock are not global/rebindable (Ctrl+K/L/N fixed in VW/mod.rs) |
| Account (plan/premium, delete account) | MISSING | prefs ACCOUNT_LABEL only shows "Bitwarden account"; accounts.rs delete_account_dir is local-only | no premium/ delete-account server call |
| About: updates check | EXISTS | `check_for_updates`; updater.rs (GitHub releases, SHA-256), prefs `draw_updates`, `draw_update_card`, update_panel.rs | |
| About: early/beta updates | MISSING | updater.rs has no prerelease channel | |
| About: help, feedback | MISSING | | |
| About: diagnostic logs copy | MISSING | logging.rs writes `deskwarden.log` in config dir; no copy-to-clipboard UI | |
| Extra current pages not in v2 | n/a | Breaches, Sync (push/polling), Sharing (advanced_sharing), Local API (service_enabled, API keys), Backend choice (bw serve vs direct REST) | |

## Lock and sign-in

| Feature | Status | Where | Notes |
|---|---|---|---|
| Lock screen with Windows Hello + master password | EXISTS | login_ui.rs (unlock mode; `LoginForm`), hello.rs (`HelloState`, enroll), unlock_prompt.rs (Win32 unlock card on fill), locked_card.rs (3b) | previews login_unlock, login_unlock_busy |
| Sign-in screen with server URL | EXISTS | login_ui.rs `ServerChoice` + `server_choice_dropdown`, `configure_server_in`, second factor (`SecondFactorRequest`), CLI setup + backend choice; API-key login (api_key_ui.rs) | Dropdown US/EU/custom |
| Signed-out state | EXISTS | login_ui.rs sign-in card; accounts.rs (multi-account, `AccountsState`) | |

## Autofill overlay

| Feature | Status | Where | Notes |
|---|---|---|---|
| Panel under focused sign-in field with matching logins (Enter to fill) | PARTIAL | prompt_card.rs (2a matched card, Win32/GDI), picker_prompt.rs + picker_ui.rs (account picker with Up/Down/Enter), app::overlay_position | Card is placed BESIDE the field (anchor from field locator / UI Automation), not a dropdown list under it; matched card offers fill + options; picker lists several accounts |
| No-match state | EXISTS | prompt_card.rs, `app::handle_no_match` (3a) | |
| Locked state | EXISTS | locked_card.rs (3b), unlock_prompt.rs | |
| Save-login prompt after new sign-in | EXISTS | save_login_card.rs (3c), `never_save_for_apps` | |
| New-password suggestion | EXISTS | generate_prompt.rs (3d generate & fill) | Win32 card, triggered via link in 3c / hotkey |

## Tray

| Feature | Status | Where | Notes |
|---|---|---|---|
| Tray icon | EXISTS | tray.rs (`tray_icon`, icon resource id 1, tooltip "Deskwarden") | |
| Tray popup panel (search, recent items, copy user/pass, Generate/New item/Share, status footer "Synced - locks in 14 min") | MISSING | tray.rs builds a NATIVE context menu only: Open Vault, Add app..., fill hint, Sync, Preferences..., Accounts submenu (Signed in: x, Add account...), Quit | No custom popup, no search/recent items/actions/status footer |

## Brand

| Feature | Status | Where | Notes |
|---|---|---|---|
| App icon / shield mark | EXISTS | theme.rs `mark`, `window_icon` (64px), assets/generate-icon.py -> assets/deskwarden.ico; quartered blue shield (design 3g) | |

## Card and bank logos

| Feature | Status | Where | Notes |
|---|---|---|---|
| Card network detection from number | PARTIAL | card_brand.rs `CardBrand {Visa, Mastercard, AmericanExpress("Amex"), Discover, DinersClub, Jcb, Maestro, UnionPay, RuPay, Other}` + `brand_for_number`; mirrors Bitwarden's 10-brand list | Amex, Diners, JCB, Visa, Mastercard, Maestro, Discover, UnionPay EXIST. **Mir MISSING** (RuPay is there instead; Bitwarden's enum has no Mir, so adding it as a stored value would NOT be Bitwarden-compatible - detection/display only) |
| Network mark drawing | EXISTS | card_mark.rs: wordmark in type; real logo PNGs only if `use_brand_logos` on and file in `%APPDATA%\Deskwarden\brand-marks` (brand_mark.rs `search_dirs`); no images are compiled in | v2 brand logos would have to be shipped as files/embedded |
| Bank logos (Chase, Revolut, HSBC, ...) linked via a login | PARTIAL | favicon.rs `BANK_DOMAIN_FIELD` ("deskwarden:bank-domain" custom field on a card) + `icon_domain_for`; favicon fetch (site icon via Bitwarden icon service or direct) | Bank logo = the favicon of a domain the user sets on the card; no bundled bank logo set, no "link via a login" (card references a domain string, not a login item) |

## UI modules (one line each)

- `vault_window/mod.rs` - the whole vault window: frame loop, toolbar/search/account menu, sync pill, Ctrl+K/L/N/Shift+S chords, export, background loaders, modals dispatch.
- `vault_window/sidebar.rs` - left rail: VAULT filters + counts, FOLDERS, Password health, Sends/Shared screens, auto-lock countdown, drag-to-folder.
- `vault_window/item_list.rs` - middle list: rows with icons/kind marks/shared pill, search matching, row context menu.
- `vault_window/detail.rs` - right pane read view: header actions, login/card/note/identity/SSH bodies, TOTP, custom fields, AUTOFILL TARGETS, SHARING card, copy toast.
- `vault_window/detail_edit.rs` - edit/create form for every kind, websites, custom fields, card details, generator, app-window picker, template editor.
- `vault_window/detail_slide.rs` - slide animation between list and detail.
- `vault_window/sequence_builder.rs`, `rehearsal.rs`, `preflight.rs` - sign-in steps builder (4a), test run (4d), fill gate.
- `vault_window/folder_modal.rs`, `delete_modal.rs`, `icon_modal.rs` - rename/delete folder; delete/trash/permanent/archive; "Select icon".
- `vault_window/password_health.rs` - weak/reused/breached report screen.
- `vault_window/send_ui.rs`, `record_ui.rs`, `share_ui.rs` - Sends screen + composer + states; field-level record Send/import; share with a person / Shared with me.
- `vault_window/totp_add.rs` - add one-time code (picker, scan, manual, confirm).
- `vault_window/relative_time.rs` - "2m ago" formatting.
- `prefs_ui.rs` - Preferences (12 sections, modal or own window): General, Shortcuts, View, Lock, Breaches, Vault/backend, Sync, Sharing, Local API, Clipboard, Updates, About.
- `login_ui.rs` - sign-in / unlock window, server picker, 2FA, CLI setup, backend choice, window chrome, vault skeleton.
- `loading_ui.rs`, `api_key_ui.rs`, `second_factor_ui.rs`, `update_panel.rs` - loading/slow/failed first window; API-key sign-in; 2FA prompt; update panel.
- `picker_ui.rs` (+ `picker_prompt.rs`) - account picker list (Up/Down/Enter); Win32 wrapper.
- `prompt_card.rs`, `locked_card.rs`, `save_login_card.rs`, `generate_prompt.rs`, `unlock_prompt.rs`, `notice_card.rs` - bare-Win32/GDI overlay cards drawn by the daemon (matched, locked, save login, generator, unlock, notices).
- `tray.rs` - native tray icon + context menu only.
- `theme.rs` - design tokens (light only), Archivo fonts, mark/shield, avatar/initials, folder_mark, widgets, `window_icon`.
- `card_mark.rs`, `brand_mark.rs`, `kind_mark.rs`, `favicon.rs`, `item_icon.rs` - network marks, brand logo loader, item-kind marks, site icons, per-item chosen icon.

## How screens are previewed

- `examples/ui_preview.rs` (~4150 lines): `cargo run --example ui_preview` (interactive save-login form), flags `--login`, `--cli-setup`, `--backend-choice`, `--vault`, `--sends`, `--totp`, `--edit`, `--kinds`, `--all`, each with `--screenshot` that renders to PNGs under `target/ui_preview*/` (e.g. `target/ui_preview/*.png`, `target/ui_preview_sends/`, `_totp/`, `_edit/`). Self-screenshotting needs no human.
- Other examples: `picker_preview.rs`, `prompt_preview.rs`, `locked_preview.rs`, `save_login_preview.rs`, `unlock_prompt_preview.rs`, `generate_preview.rs` (Win32 cards); `*_probe.rs` are system probes (UI Automation, field locator, window watch, REST).
- Existing PNGs in `deskwarden/target/ui_preview/`: detail_*, edit_*, first_window_*, item_kind_marks, login_*, prefs_* (about, breaches, clipboard, updates, vault), progress_bar_cycle, record_composer, rehearsal, sends_*, totp_add_*, vault_item_list*, vault_password_health*, vault_rail, vault_setup_spinner.

## Caveats
- Status "PARTIAL" for the New item chooser and for the exact Windows-Hello/lock-on-sleep toggles is from limited reading; the lock-on-sleep/screen-lock behaviour (away_lock.rs) is confirmed always-on, with no setting.
- Compat note: item icon, bank domain, app match and sign-in steps are all `deskwarden:*` custom fields on ciphers (Bitwarden-compatible, other clients just see extra custom fields). Hide-my-email on Sends and Mir as a stored brand have no Bitwarden-wire counterpart.

# Slice 15e: Settings descriptions as the design writes them

**Goal:** every Settings row reads the way `docs/design/Deskwarden v2.dc.html` writes it (the `R.sw/sg/bt/va(k, title, sub, tip)` helpers and the `SECTIONS` object): a short title, an optional sub of a few words, and any explanation in a `tip` -- a small `?` dot after the title that shows the text on hover. Today many rows carry a 0.x paragraph as their grey sub-line (Vault > "Show site icons" is four lines).

## Cause
The 0.x preferences panes explained themselves under each row. Slice 15b moved those rows into the v2 row style but kept the paragraphs as the sub, so the page reads as a wall of grey text next to the design's one-line rows.

## Tasks
1. The shared row (`prefs_ui::control_row_tip`, `row_text_ink`, and `settings_v2::row`/`switch_row`/`segment_row`/`button_row`/`value_row`, plus `prefs_ui::toggle_row`/`child_toggle_row`/`value_row`/`note_row`/`control_row`) takes an optional `tip`: `kit::tips::help_dot` after the title, bubble 320 wide above the dot. `control_row_w` is gone (every caller states its tip, or `""`).
2. Every row's copy, per the table below. Where the design has the row, its title/sub/tip are used word for word (rulings below for the three places that would be false). Where it has none, the sub is at most ~8 words or empty and the old explanation is the tip, trimmed to two sentences, keeping every privacy and cost fact.
3. Tests that looked for a description painted on the page read the tip by hovering the dot (`hover_tip` / `tip_on` in the `prefs_ui` tests) or assert the new sub.
4. `ui_preview`: `settings_vault_tip` shows a tip open (`kit::tips::pin_open`, `prefs_ui::pin_row_tip`: the example has no pointer to hover with).

## The table (old sub -> new sub / tip)

Rows the design has:

| Row | Old sub | New sub | Tip |
|---|---|---|---|
| Keep running when I close the window | "Autofill only works while ... just hides it." | none | the same sentence, as the tip (design) |
| Default sign-in steps | "Username, Tab, password" | same (see Ruling 3) | none |
| Fill shortcut | "Works in any app, even without the panel" | same (design) | none |
| Hide passwords while I share my screen | "The whole window stays out of ... press reveal." | none | the same sentence (see Ruling 2) |
| Start in the system tray | "Start hidden; open the window from the tray icon" | none (design has none) | none |
| Sidebar (Look) | "Navy or light, beside your vault." | none | none |
| Help center | "How Deskwarden works, in your browser" | none (design has none) | none |
| Send feedback | "Tell us what is missing or broken, in your browser" | none (design has none) | none |
| Diagnostic logs | "Copies where the log file is, not what is in it" | none | "Copies where the log file is, not what is in it. Read it before you share it." (Ruling 4) |
| Import from another app (was "Import passwords") | "From Bitwarden, 1Password, Keeper, LastPass, Chrome or Edge, or any CSV file" | "1Password, Keeper, LastPass, Chrome or a CSV file" (design) | "Bitwarden and Edge exports work too." |
| Generator rows, Theme, Text size, Language, Lock on sleep / screen lock, Open at sign-in | none | none | none |
| Version | "Unofficial, and unaffiliated with Bitwarden, Inc." | same (6 words) | none |

Rows only Deskwarden has (sub <= ~8 words, explanation in the tip, <= 2 sentences):

| Row | Old sub | New sub | Tip keeps |
|---|---|---|---|
| Show autofill prompts | 3-sentence paragraph | none | what opens on its own, that nothing is typed until you ask, that CTRL+ALT+B is the only way to fill when off |
| Lock the vault when you step away | "Off means the vault stays unlocked ..." | none | the same sentence |
| Lock the vault after | "Minutes of no activity. One minute is the shortest." | "Minutes of no activity" | "One minute is the shortest." |
| Show site icons | 4 lines | none | the domain is what is sent, to the icon service (your own server's if you self-host); never the username, password or account; off shows initials and nothing leaves the PC |
| Fetch site icons from the sites themselves | 6 lines | "Sites you hold entries for see a request" | on: each site learns an entry for it exists and roughly when you looked; own-network addresses are always fetched directly |
| Trust any certificate on your own network | 5 lines | "Icons from your own network only" | scope (own-network addresses, icons only, never the internet) and the cost (another machine answering for that address chooses the picture and learns you hold an entry) |
| Show card network logos | 4 lines | "Uses PNG files you add yourself" | the brand-marks folder, nothing downloaded, fallback to the printed name |
| Show TOTP secrets on the details screen | 3 lines | "Masked until you click the eye" | the code expires, the secret does not |
| Keep the Bitwarden CLI running | "Faster, and uses about 118 MB ..." | "Faster, but uses more memory" | 118 MB, off runs it only with the vault window, autofill unaffected |
| Open the vault instantly | 4 lines | "Opens at once next time" | 76 MB while unlocked; lock/switch closes it; toggling closes the open window |
| Which client opens this vault | 6 lines (own), 3 lines (ghost) | "Applies the next time you sign in" / "Only available on a self-hosted server" | 118 MB vs 19 MB, and that the built-in client's stored key never expires; ghost: cloud accounts always use the official CLI |
| Keep an encrypted copy of your vault on this PC | 6 lines | "Stays on disk, even while the vault is locked" | what is in the file, who can read it, that it survives a lock (deleted on log out or after 7 days) |
| Read from that copy first | 3 lines | "Filling works even when Bitwarden is unreachable" / ghost "Needs the copy above" | what on and off do |
| Live updates from the server | 3 lines | "Changes arrive within seconds" | needs the built-in client and server support; off falls back to polling |
| Update via polling | 4 lines | "Also checks when you switch back" | pauses while live updates are connected; off still updates on open and on Sync |
| Check every | 2 sentences | "Minutes between checks" | each check downloads the vault |
| Use advanced sharing | 2 sentences | "Off sends the whole record" | off sends username, password, websites and notes |
| Serve this vault to programs on this PC | 5 lines | "Off by default; nothing is listening" / "On, and listening on 127.0.0.1" | what is served, to whom, that a key is the whole credential |
| Name / Expires after / Item id (mint form) | 2 sentences each | "Name it after the program" / "Days from now; empty never expires" / "The one item this key may reach" | the rest of each sentence |
| Mint a key note | "A key is shown once, when it is made, and after that only its hash is kept." | "Shown once; only its hash is kept." | |
| Check passwords against known breaches | 3 sentences | "Sends part of a hash, never the password" | the 5-character SHA-1 prefix to Have I Been Pwned, rest matched here |
| Scan the whole vault (idle) | 2 sentences | "One request per distinct password" | what leaves the PC |
| Check for updates automatically | 3 sentences | "Asks GitHub; says nothing about you" | the request names nothing about you; off means no security-fix notices |
| Update card, idle | "Ask GitHub whether ... Nothing is downloaded until you ask for it." | "Asks GitHub; nothing downloads until you ask." | |
| Take copied secrets back off the clipboard | 3 sentences | "Off leaves copied secrets in place" | off: no timer, none of the three below; history exclusion is separate |
| Clear when the vault locks / account changes / Deskwarden quits | 2-3 sentences each | "However the vault locks" / "Switching, adding or removing one" / "Not after a crash or power cut" | the full cases, including what a crash leaves behind |
| Clear after | 3 sentences | "Minutes, from 0.5 to 60" | decimals, the floor, the ceiling |
| Reset to default | 3 sentences | "Nothing on other pages changes" | what the defaults are; no confirmation |
| Clipboard history and sync are always excluded | 3-sentence note | "Kept out of Win+V history, never synced to other devices" | why there is no switch |
| Shortcut rows | one sentence + the "needs a matched window" paragraph | one clause ("Types the matched item's username") | what happens with nothing matched; unchanged status lines (capture hint, refusals, "not working" reasons) stay as the sub |
| Not-yet pages | 2 sentences | "Arrives in a later release" | |

Status lines are not row descriptions and keep their text: the scan progress and outcome, the update stages, the interval's refusals, the shortcut capture hint and refusals, the account line, the empty states.

## Rulings
Ruling: the privacy/cost warnings moved into tips and the sub names the one fact a user weighs (for example "Stays on disk, even while the vault is locked") -- why: the owner's rule is that no warning disappears, and a tip is only seen on hover, so the sub carries the headline for the rows where a user would otherwise never find out -- cost if wrong: a sub that is too terse for a hover-less touch screen; the tip is the full text.
Ruling: "Hide passwords while I share my screen" keeps this app's tip ("The whole window stays out of Teams, Zoom or Meet screen sharing, even if you press reveal.") rather than the design's "Passwords stay as dots during ..." -- why: the window is excluded from capture (display affinity), not masked, so the design's sentence would be false here -- cost if wrong: none; the wording differs from the mock in one clause.
Ruling: "Default sign-in steps" keeps the sub "Username, Tab, password" where the design says "..., Enter" -- why: `key_sequence::DEFAULT_SEQUENCE` is `{USERNAME}{TAB}{PASSWORD}` and presses no Enter, so the design's sub would say something the app does not do -- cost if wrong: none.
Ruling: "Diagnostic logs" keeps its own tip ("Copies where the log file is, not what is in it. Read it before you share it.") instead of the design's "Logs never contain passwords, codes or anything you type." -- why: the log is written from many parts of the app and this screen cannot vouch for every line (see `log_location`), so the design's promise would be one we cannot keep -- cost if wrong: none.
Ruling: "Show site icons" keeps the title "Show site icons" (the design says "Show app and website icons") and gets a tip where the design has none -- why: the setting governs the website icon request only, and the privacy fact (the domain goes to the icon service) must not disappear -- cost if wrong: a one-word title difference.
Ruling: "Show autofill prompts" keeps its own title and a tip of its own rather than the design's "Suggest logins when I click a sign-in field" row -- why: Deskwarden raises the prompt when a window that wants a password comes to the front, not on a field click, so the design's wording describes a different behaviour -- cost if wrong: none; the design's panel position and 2FA rows do not exist here.
Ruling: "Start in the system tray", "Help center", "Send feedback" and the "Sidebar" style lose their subs outright -- why: the design has no sub on the first three and none of the four sub-lines carried a cost or privacy fact -- cost if wrong: the Sidebar row no longer says "beside your vault"; its two cells say Navy and Light.
Ruling: "Import passwords" is now "Import from another app" with the design's sub; the Bitwarden and Edge sources moved into a tip -- why: the design's title and sub, and no source is dropped (all are still in the wizard) -- cost if wrong: a source is only named in a tip.
Ruling: the tip bubble is 320 wide, opens above the dot, and the dot is the kit's 18 px `help_dot` -- why: the design's tip sits after the title and opens above; the width is that of the widest tip at two sentences -- cost if wrong: a bubble that clips at the window top for the first row of a page (`tips::paint_help_bubble` keeps it inside the window).
Ruling: `ui_preview` shows a tip open through `kit::tips::pin_open` / `prefs_ui::pin_row_tip` -- why: the example has no pointer to hover with and a screenshot of the design's hover state is the point -- cost if wrong: two small public functions used only by the example.

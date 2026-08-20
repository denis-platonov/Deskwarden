# Changelog

Notable changes to Deskwarden. Releases before 0.8.2 are described on the
[releases page](https://github.com/denis-platonov/deskwarden/releases); this
file starts here because 0.8.2 is the first release large enough to need one.

Dates are the release date. This project follows [semantic
versioning](https://semver.org/) loosely: the leading zero means the shape of
things can still change between minor versions.

## Unreleased

### Fixed: Preferences said this build could not scan or check for updates

Opening **Preferences** from inside the vault window showed *"This build
cannot scan: nothing set the scan up when Deskwarden started"* on the Breaches
page and *"This build cannot check for updates. Please report it — it is a
defect, not a setting"* on About. Both buttons did nothing.

**Your build was fine.** Both messages were honest reports of a real missing
piece, and the missing piece was ordering: two things Deskwarden sets up at
startup were being set up *after* the first vault window opened, and that
window blocks until you close it. Preferences reached from the tray icon —
the same pages, a different route in — worked normally throughout, which is
why this went unnoticed.

Both are now set up before any window opens, so both routes behave the same.
The messages themselves stay: an app that genuinely cannot do something should
say so plainly, and these two are the reason this was diagnosable at all. If
you reported this — thank you, and sorry for the alarm.

### Starting Deskwarden while an older copy is running now works

Launching a new build while an older one was running showed *"Deskwarden is
already running on this PC"* and quit. The older copy predates the handover
mechanism, so there was nobody to ask it to close — and rather than run two
copies at once, the new one refused. From your side you double-clicked
Deskwarden and got nothing.

The new copy now takes over regardless.

- **Asking still comes first, and is still the normal path.** A running
  Deskwarden that can be asked to stand down is asked, and closes through its
  own door: vault cache wiped, copied password taken back off the clipboard,
  backend shut down. Nothing about an ordinary relaunch changes.
- **Only when there is nobody to ask, or no answer within five seconds**, is
  the running copy ended outright — and only a process running the same
  program, signed in as you, in your own session. Another signed-in user's
  Deskwarden is never touched, and neither is an installer applying an update.
- **What ending it costs, stated plainly.** The backend still shuts down with
  it, so nothing is left holding your unlocked vault. But a password that copy
  had copied can stay on the clipboard until its own clipboard timer clears it
  — the new copy cannot clear a clipboard it cannot prove is Deskwarden's, and
  quietly emptying yours would be a bug of its own.
- **If it still cannot be closed**, you get the same message and the same
  advice as before — quit it from its tray icon — rather than two Deskwardens
  running at once. Everything the new copy did, and why, is in the log.

### Card network marks can be the networks' own logos

**Preferences → General → Show card network logos**, off unless you turn it
on. With it on, a card's network mark is drawn as that network's own logo
instead of its printed name.

- **You supply the images.** Nothing is downloaded and no logo ships with
  Deskwarden. Put PNGs named after the network — `visa.png`, `mastercard.png`,
  `amex.png`, `discover.png`, `diners.png`, `jcb.png`, `maestro.png`,
  `unionpay.png`, `rupay.png` — in `%APPDATA%\Deskwarden\brand-marks`. A
  `brand-marks` folder beside the program is read too, and yours wins, so a
  mark you supply always replaces one that was installed for you.
- **A brand with no image keeps its word.** So does one whose file cannot be
  read, is not a PNG, or is too large. Six logos and three words is a normal
  state, not a broken one — nothing is ever left blank.
- **Both shapes of asset work.** An isolated mark on transparency and a
  reversed mark full-bleed on the brand's own colour are fitted the same way:
  each is scaled until its lettering stands as tall as the word it replaced
  and centred on the same line, so a folder filled from two brand centres
  still reads as one set. A logo can never take more room from the item's name
  than the longest wordmark already did.
- **Files are re-read while Deskwarden runs**, so dropping one into the folder
  and reopening the vault window shows it — no reinstall, and no rebuild.

### The "no saved login" card can search the vault

The card that appears when you focus a password field Deskwarden has nothing
for offered one button, *New login*. It was designed with two; the second one
is here now.

- ***Search vault*** opens the vault window **with that app's name already in
  the search box**, so a login saved under a name the matcher did not
  recognise is one click away instead of one click and some typing. The name
  goes in without its `.exe` — searching for `Ledgerline.exe` would find
  nothing, which is the one answer this card must not give twice.
- **The query is a starting point, not a filter you are stuck with.** Clear it
  or edit it like any other search. Opening the vault any other way — the tray
  menu, a tray click, startup — still opens it on the whole list, and if that
  window locks or you switch account while it is up, the one that comes back
  is unfiltered.
- **The locked card does not offer it**, for the same reason it offers no
  *New login*: while the vault is locked there is nothing to search, and a
  window opened onto an empty list would read as "you have nothing" rather
  than "it is locked".
- The card's second line no longer says "Open Deskwarden to search the vault"
  — the button does that now. It says what would otherwise be a mystery
  instead: matching is by process name and window title.

### Card rows name the network beside the icon, not over it

A card row drew its network badge inside the bank icon's tile, overlapping
the artwork in the lower-right corner. It now sits **beside** the tile: icon,
then the network in its pill, then the name and its `(*9988)` suffix.

- **The networks are spelled out.** `MC`, `DC`, `DISC` and `UP` were never an
  abbreviation style -- they were the 32pt tile the badge had to fit inside.
  They are now `MASTERCARD`, `DINERS`, `DISCOVER` and `UNIONPAY`. `AMEX`
  stays as it is: `AMERICAN EXPRESS` measures a third of the row.
- **The pill is set well below the name**, at 9pt against its 13. The card's
  name is the thing being identified and its network is a qualifier on it,
  and that is what the type sizes now say. The size was chosen off a rendered
  ladder of four candidates read unmagnified, not by argument.
- **The icon, the pill and the name sit on one line** -- their ink, not the
  boxes their ink sits in, which is what "the same mid line" means to a
  reader and what the row was missing.
- **A long name still truncates properly.** The pill is taken out of the
  row's width before the name is laid into what is left, so a name that has
  to be cut is cut a little sooner rather than running underneath the pill.
- A card with no bank icon now shows the same monogram tile every other row
  without an icon shows, instead of the network filling the tile.

### The card brand list matches Bitwarden's, one for one

- **American Express interoperates again.** This app stored the brand as
  `American Express`; Bitwarden's value is `Amex`. Cards created here showed
  no card art in the web vault, and Amex cards created anywhere else showed
  no network mark in the item list here. Cards already carrying the old
  spelling keep working.
- **Maestro and RuPay are recognised**, both as a stored brand and, where the
  digits are unambiguous, from the number. RuPay numbers in the `6521`/`6522`
  block are left to Discover rather than guessed at -- picking either way
  would mis-read real cards of the other network.
- **`Other` is offered**, as Bitwarden offers it, and shows an `OTHER` mark.
  It is a brand and not a network: a card marked `Other` is masked and
  grouped exactly as a card with no brand at all is.

### Smaller things

- The card pane's **"Cardholder name" row is now just "Cardholder"**, and the
  copy shortcut's description says the same.
### "What's new" reads like release notes

The update panel showed a GitHub release body as literal characters, so `**`
and `#` and `[..](..)` were on screen as themselves, and only the newest
release's notes were ever fetched.

- **Markdown is rendered**, in a deliberately small subset: headings, bullet
  lists, bold, italic, inline code, and a link's words with its destination
  beside them. Nothing on the page is clickable and no image is fetched --
  release notes arrive over the network onto the page that says what is about
  to be installed, and that is the one place where a link would turn
  misleading styling into somewhere the reader can be sent. Anything outside
  the subset -- raw HTML, tables, an unclosed `**` -- is painted as the
  characters it is.
- **Blank lines are space again.** The paragraph breaks between a release's
  sections no longer collapse into a wall of text.
- **Every release you skipped is shown**, newest first, each under its own
  version heading. Updating from 0.8.1 to 0.8.4 now tells you what changed in
  0.8.2 and 0.8.3 as well. Drafts and prereleases are excluded, and a release
  that published no notes is named rather than dropped out of the range.
- **The installer still comes from exactly one release**, the newest, with its
  download URL and its SHA-256 read from the same asset in the same response.
  The notes may be a union of several releases; the file being verified and
  installed is not.
- **The notes' scrollbar appears only when there is more to read.** It used to
  be pinned open beside three lines of text. The card is the same width either
  way.

### The About page says which account is signed in

The "Bitwarden account" row told you to open the vault window to find out,
while the app already knew. It now shows the signed-in address and the server
the vault lives on -- and says "Checking..." while it is asking, which is not
the same claim as saying nobody is signed in.

### The preferences window's nav rail meets the titlebar

There was an eight-point strip of window background between the bottom of the
titlebar and the top of the left-hand navigation column.

### Check every password in the vault against known breaches

A new **Breaches** page in Preferences, with a **Scan all passwords now**
button on it. The switch that used to sit on General moved here, so the
setting, the button and the sentence explaining the difference between them
are readable in one glance.

- **The button always scans, whatever the setting says.** Pressing it is you
  asking for the check, in the same breath as consenting to it. What the
  setting governs is what Deskwarden does *on its own* -- the breach badge on
  an item you open -- and the page says exactly that, under the button.
- **Nothing scans by itself.** Not on opening the app, not on unlocking, not
  on a timer, and not when you open the health report. A scan happens because
  you pressed the button, and there is no other way to start one.
- **One request per distinct password, not per item.** A vault where the same
  password is on six logins asks about it once, so a large vault is far fewer
  requests than it has entries. Four requests at a time, never more: Have I
  Been Pwned is free and run at somebody else's expense.
- **A check that fails is counted and shown**, while the scan runs and after
  it finishes. A run that says "checked 60, found 3" while 40 lookups failed
  is a lie you would go on trusting, so the failures are the last thing every
  sentence says. Each lookup is retried twice before it is given up on.

### Password health shows what the scan found

The report gains a **Found in breaches** section, grouped by how many times
each password was seen, worst first, with the advice on every row.

- **Filters: All, Reused, Weak, Breached.** Only findings are ever listed,
  under every filter -- a list of two hundred healthy logins buries the six
  that matter -- and a filter with nothing in it says so instead of leaving a
  blank pane.
- **A password that could not be checked is shown as unknown**, never left
  out. "Not on the list" must never come to mean "fine".
- **A group of items under one breach count says whether they really are one
  password.** The report already knows: the reuse grouping is exact, so it is
  stated as a fact rather than left as an inference.
- **The screen still checks nothing itself.** It displays what a scan you ran
  produced, and the footer says so in both states of the setting.

### Breach scans are recorded, as counts and nothing else

The last twenty scans are kept in `scan_history.json`, beside `settings.json`
in `%APPDATA%\Deskwarden` -- a separate file, because a record is not a
preference.

Each entry is five numbers: when it finished, how many distinct passwords were
checked, how many items those covered, how many were found, and how many could
not be checked. **No password, no item name, no item id, no hash, and nothing
derived from a password ever goes in it.** A per-item history would be a
genuinely useful feature and it is refused: it would be an unencrypted list of
which of your entries are compromised, sitting next to your settings, readable
by anything running as you and surviving every lock.

An older file, a missing one, an empty one and an unreadable one all read as
"no scans yet", and none of them stops the next scan being recorded.

### A Send's expiry date is the day on your own calendar

The line under the lifetime picker read "The link stops working after 7 days --
on 18 Aug 2026 (UTC)." It was computed from the UTC instant, and a Send that
dies at 00:30 UTC dies the **previous evening** anywhere in the Americas. So
the one place in this app where being wrong about a lifetime is the harm was
naming a day the link would already be dead on, and offering "(UTC)" as the
explanation.

- The date is now converted to this machine's local time, and no label says
  "UTC" to you. Daylight saving is resolved for the instant in question rather
  than once at startup, so a date the far side of a clock change is still the
  right one.
- **What is stored has not moved.** The `deletionDate` handed to `bw` is UTC
  and stays UTC: the change is to how the moment is described, never to when
  the link actually dies.

### The TOTP-secret setting names a row that exists

The description under "Show TOTP secrets on the details screen" said the
secret appears "under its one-time code". That row is labelled **TOTP** on the
details screen and has been since it was renamed, so the copy sent you looking
for something the app does not paint -- which reads as the setting not having
worked.

## 0.8.4 - 2026-08-20

> The manual pre-release checklist was not run for this version either.
> This release changes what happens when Deskwarden starts and when a
> second copy is launched, and adds a self-update that can now actually
> install. None of those three has been exercised outside the test suite.
> If an update, a launch, or a takeover misbehaves, please open an issue.

### Cyrillic names render in the app's own typeface, at the right weight

Item names, usernames, folder names and notes written in Cyrillic came out in
a different, lighter typeface than the Latin beside them -- and in the *same*
typeface at the same weight wherever they appeared, whether the design asked
for 400, 600, 700 or 800. All four bundled Archivo faces cover no Cyrillic at
all, so every such string fell through to the one fallback face the UI toolkit
ships.

- **Four Cyrillic faces are now bundled**, one per weight (Noto Sans' Cyrillic
  subset, SIL OFL 1.1 -- `deskwarden/assets/fonts/OFL-NotoSans.txt`), each
  sitting behind its matching Archivo cut and ahead of the toolkit's own
  fallbacks. 64 KB in total.
- **Latin is untouched.** The subsets carry no Latin letters whatsoever, so
  they cannot be reached for Latin text however the fonts are ordered; every
  existing measurement in the app is unchanged, and that is asserted rather
  than assumed.
- Greek, Hebrew, Arabic and CJK still come from the toolkit's fallback; this
  release fixes Cyrillic only.

### Deskwarden no longer dies because something else has CTRL+ALT+B

A real session ended with exit code 101, an hour and three quarters in, with a
vault window open. Nothing was shown; the app simply vanished. The cause was
the global shortcut: `RegisterHotKey` is first-come-first-served across the
whole logon session, another program held CTRL+ALT+B, and the failure was an
`expect`.

- **No registration failure ends the app now.** Not the conflict that was
  reported, not a keyboard hook Windows refuses to give, not anything else
  `global-hotkey` can return. Deskwarden runs on without the shortcut;
  everything else -- the tray, the overlay, the vault window, the clipboard,
  filling from the overlay -- is untouched.
- **Preferences > Shortcuts says so**, names the shortcut, and says what to do
  about it. Not a dialog at startup: a shortcut somebody else claimed is a
  missing convenience, not a failure to start.
- **It is retried every 30 seconds** while it is unavailable, so closing the
  program that took the keys gets the shortcut back without restarting
  Deskwarden. A shortcut that is working never re-attempts anything.

### One Deskwarden per session, and starting it again takes over

Launching Deskwarden while it was already running used to leave two copies
running, the second of which was the one that died on the shortcut above.

- **The newly launched copy takes over.** It asks the running one to stand
  down and takes its place -- which is what launching an app you already have
  running should do.
- **The outgoing copy leaves through its own door**, not a kill: it zeroizes
  the decrypted item cache and takes a copied password back off the clipboard
  before it exits, and `bw serve` goes with it. A forced kill would have left
  the password pasteable.
- **If the running copy will not go within five seconds**, the new one says so
  and stops, rather than starting a second copy or hanging with no window.

### Password health rows line up with the item list again

The two panes take turns in the same column of the same window, and their rows
did not start and stop at the same place.

- **The same 10pt inset on both sides**, and the same scrollbar handling: the
  bar is drawn inside that padding rather than in a lane of its own, so no row
  changes width when a report starts or stops scrolling, or when the pointer
  enters the pane. The rows used to shift by 10pt with the pointer.
- **A report short enough to fit paints no scrollbar**, exactly as a short
  item list does.
- **A finding whose item has no name at all now says `[No name]`** instead of
  showing its grey detail line over blank space. A finding that does not say
  which item it is about cannot be acted on.

### Password health: long item names no longer run off their row

A finding whose item name was wider than the pane -- a saved page title such
as *"Visual Studio App Center | iOS, Android, Xamarin & React Native App
Development"* -- had its name painted straight past the right edge of its
white tile and over the background behind it.

- **The name is now truncated with an ellipsis** at the same 12pt inset from
  the right edge that it starts at from the left, at every pane width.
- **A name that fits is untouched** -- nothing gains an ellipsis it does not
  need.
- **The weak-password detail line** ("9 characters, lowercase letters and
  digits") is bounded by the same tile, for the same reason.

### Preferences is now a mixer mark

Reported as "also settings glyph I prefer to have vertical with just cross
bars like a DJ mixer, maybe".

- **Three vertical faders**, each a full-height track with a filled block
  riding it, replacing the two horizontal slider rows.
- **The blocks sit at three clearly different heights**, which is what makes
  it read as a mixing desk rather than as a fence.
- **It is more legible than what it replaces**, not just different: the old
  ring knobs were small enough at this size to blur into their own lines,
  and a solid cap does not.

### The favourite star is lighter, rounder and no longer the loudest mark

Reported as "Star (fav) glyph looks too bold now compared to the other
glyphs, and maybe there is some with more rounded (wider) edges so it looks
bit more modern".

- **It was the only mark on the detail header not drawn at the family's line
  weight** -- 2.2 against everything else's 1.3, because the fat line was
  what blunted the star's points. The rounding moved into the shape itself,
  so the weight could go back to the family's. The outlined star's ink drops
  by 44% on the rendered strip.
- **Every corner is a proper fillet now**, in the path, so the filled star is
  rounded too -- which a stroke-side trick could never do.
- **A little smaller, with the valleys kept deep**, so the five points still
  read as points: 17.01 x 16.24 against the old 18.29 x 17.50, and 12% less
  ink when it is filled.
- **The filled star no longer shows seams through itself.** It was drawn as a
  fan of separately anti-aliased triangles; it is one mesh.

### The reveal eye is taller and rounder

Reported as "same for eye glyphs - bit taller\more rounded".

- **17.0 x 10.0 becomes 17.0 x 12.8** -- a 1.7:1 letterbox becomes a 1.33:1
  almond. The width is untouched, because a masked row budgets its controls
  against it.
- **The lids are fuller across their whole length**, not just at the centre,
  so the eye has a body rather than a thin crescent at each end.
- **The pupil grows with the eye**, from 2.4 to 2.9, so the middle of the
  mark is neither cramped nor rattling around.

### Browsers no longer get the "no saved login" card

In a browser that card could never be right. Every sign-in page has a password
field, so it appeared on all of them; and Deskwarden matches apps by their
**executable** while a browser's logins belong to **sites**, so saving one
would not have made it stop.

- **The unmatched cards are switched off in browsers** -- Firefox, Chrome,
  Edge, Brave, Opera, Vivaldi, and the other Chromium and Gecko builds
  Deskwarden knows by name.
- **A browser Deskwarden does not recognise behaves as it always has**: one
  card, with *Never for this app* on it, which silences it for good.
- **A browser you matched to a vault item on purpose still prompts to fill
  it.** That rule is one you wrote by hand, and it is still obeyed.

### The autofill prompt setting now silences every pop-up, not half of them

Turning **Prompt on match** off in Preferences stopped the overlay for apps
you had saved a login for, and left the "No saved login for <app>" card
appearing for the apps you had not -- which is the wrong half. The setting is
now the one switch for every card the overlay raises on its own.

- **Off means nothing opens by itself**: no fill prompt, no "no saved login"
  card, no "Deskwarden is locked" card. `CTRL+ALT+B` still fills a matched
  window, exactly as before -- this was never an autofill switch and still is
  not.
- **The toggle is called "Show autofill prompts"** now, because "on match" was
  no longer true of what it governs. Nothing on disk changed: your existing
  choice is read and written under the same key, so an upgrade keeps it.

### Starting up is one window now, not two

Launching Deskwarden while it still had a good session showed a small
"Setting up your vault..." window on its own, for however long the Bitwarden
backend took to answer — eight seconds is normal on a cold start — and then
closed it. The vault, when it arrived, was a different window at a different
size somewhere else on the screen.

- **There is one window from the first frame.** It opens at the vault
  window's own size and position, shows the spinner while the backend starts,
  and becomes your item list in place. Nothing moves, resizes or re-centres
  when the list arrives, because it is the same window throughout.
- **It can still be closed while the spinner is up**, and closing it means
  what it always meant: Deskwarden carries on starting in the background and
  lands in the tray. The one difference is that it no longer pops a second
  spinner window back at you to say it is trying again — it just tries again,
  quietly.
- **If the backend never becomes usable**, the window closes and the same
  recovery as before runs: Deskwarden restarts the backend and asks for your
  master password.
- **Signing in is unchanged.** That launch has been one window since 0.8.0;
  this is the other launch catching up with it.

### Check for updates actually installs the update now

**The in-app updater has never once applied an update.** It refused every
release it was ever offered, including all the genuine ones, and the reason
was a placeholder: it required the downloaded installer to carry an
Authenticode signature matching a certificate thumbprint that did not exist
yet, so the check could not pass. If you have been updating Deskwarden, you
have been downloading the installer from the releases page and running it
yourself.

It now verifies the installer against a **SHA-256 digest** instead, and that
check can pass.

- **The digest comes from GitHub, on the same connection as the download
  link.** The releases API publishes a SHA-256 for each asset alongside its
  URL; Deskwarden reads both out of the one response it was already making,
  so there is no second request to fail and no checksum file to fall out of
  step with the build.
- **The file is hashed twice**: once when the download finishes, and again
  immediately before the installer is started. The second one is the one that
  matters — it closes the gap between "this file was checked" and "this file
  is running".
- **Every way of not knowing is a refusal.** A release that publishes no
  digest is not offered as an update at all. A digest that is malformed, a
  file whose hash does not match, or a file that cannot be read to hash it
  stops the update and **deletes the downloaded installer**, so a rejected
  file is not left in the cache folder where a later run — or you — could
  run it by hand.

#### What this does and does not protect you from

Being straight about it, because the previous behaviour was not: **this is
not the same as a signed build, and Deskwarden's releases are still
unsigned.** What the digest proves is that the bytes that run are the bytes
GitHub's API described — which catches a corrupted, truncated, or swapped
download. What it cannot prove is *who built them*: the digest comes from the
same GitHub account the file does, so anyone who could replace the installer
could generally replace the digest beside it.

That is the same trust root you were relying on downloading the file by hand.
The difference is that something now checks it. Code signing is still the
goal, and is still waiting on a certificate; when it arrives the signature
check comes back **in addition to** this one, not instead of it. See
[docs/code-signing-policy.md](docs/code-signing-policy.md).
### A card's Expires / Code line

- **Clicking anywhere on the line copies its half.** The Expires and Code
  halves only responded to a strip through the middle of the row — the row's
  own padding, above and below them and at either end, copied nothing and lit
  nothing ("Expires\Code - only part in the middle gets it copied and
  highlighted"). Both halves are now the full band, exactly as the Number row
  above them has always been.
- **The Code label is inset inside its half** by the same padding the Expires
  label has inside the line, so the two read as a matched pair. The hit area
  still starts at the seam: the padding moved the ink, not the tile.

### The vault window's list and rail

- **A card's network badge says the network's name.** It used to be a
  geometric placeholder — a play triangle for Visa, a diamond for Mastercard,
  two bars for American Express, all on the same blue square — which named
  nothing ("VISA icon supposed to be visa and not some Play sign") and did not
  tell the seven networks apart either. The badge is now the network set in
  type: **VISA**, **MC**, **AMEX**, **DISC**, **JCB**, **DC**, **UP**, in the
  list and on the card's detail pane.
  - Words rather than logos deliberately: the network marks are registered
    trademarks whose guidelines restrict them to licensed issuers and
    merchants, and this is an MIT-licensed community project. Naming the
    network your own card is on is a statement of fact.
  - Diners Club and UnionPay get two-letter forms because the badge is drawn
    inside the row's 32pt tile and a longer word there is an unreadable smear.
- **A favicon now fills its tile.** It was inset 4pt a side inside a 32pt tile
  — a 24pt icon in a bordered box, which read as an icon adrift in a frame
  ("icon is not fully taking the rounded rectangle"). The icon takes the whole
  32pt tile now, clipped to the tile's own rounded corners so no corner
  overhangs the curve, with the tile's 1px border drawn back over the top so a
  pale icon still has the same edge every monogram beside it has.
- **Sends and Password health are their own group at the foot of the rail**,
  below the folders and behind a second separator of the same kind as the one
  above FOLDERS. They used to sit in the middle of the VAULT rows, which are
  cuts of the item list — something neither of them is.
- **The rail scrolls.** It never did: a vault with more folders than the window
  is tall simply had the last ones off the bottom with no way to reach them,
  and the two rows above would have joined them there. The auto-lock countdown
  stays pinned to the floor.

### The item detail pane

- **"One-time code" is called TOTP now**, because it is shorter and it is what
  the field is called everywhere else. The live code's row, its copy
  confirmation and the CTRL+T chord all read **TOTP**; the seed rows read
  **TOTP secret**, **TOTP seed** and **TOTP seed (sealed)**; and the form that
  adds one is **Add a TOTP**. The qualifiers stay on purpose — the pane can
  show a live code and its seed one above the other, and two rows with the
  same label are two rows you cannot tell apart.
- **The live TOTP code sits on the row's line.** It was painted 1.5pt high —
  the code, its countdown bar and the seconds beside it are three type sizes
  on one line, and each was centred in its own box rather than on a shared
  baseline, which reads as the digits floating above everything next to them
  ("TOTP code itself not in the center vertically"). The seconds were 0.5pt
  high for the same reason and have moved with it. Both now use the same
  measurement the card-number rows have used since they were reported for
  exactly this, so the three digit rows and the code cannot drift apart.
- **The close ✕ is as legible as the controls beside it.** It rested at the
  faintest ink the palette has for something still meant to be clicked, which
  read as disabled next to the ✉ and the ⋮ ("close button feels too
  gray/thin compared to the rest on details screen"). It now rests where they
  rest. It is still never red — that is the point of the rule, because a
  Delete that arms on its first click is one control away — and a neutral
  grey was never at risk of being mistaken for it.
- **The five header icons read as one set.** They always shared a hit target —
  all five sit in the same 34pt square — but the marks drawn inside them did
  not match ("also those icons are not same size feels like"). Measured, the
  close ✕ covered 12.3pt against 15.4 to 19.3 for everything else, and the
  favourite ★ was the largest thing on the strip despite being the only one
  that fills in. The ✕ is now drawn at the ⋮'s size and the ★ at the ⏱'s. The
  ✉ and the ⋮ are untouched.
- **Masked values show one bullet per character.** Every secret used to draw
  the same ten dots however long it really was, which also meant a card number
  — which has always masked to its real length — was the odd one out. Now a
  password, a TOTP secret and a previous password all match the length of what
  they hide, and if the window is too narrow to show the whole run it is cut
  to the row rather than wrapped into a block of dots. Nothing is ever pushed
  off the row: the reveal eye and the copy chord stay where they are at every
  width.
  - This does publish a secret's length to anyone looking at your screen,
    which is why it did not before. It is a deliberate trade, and it is what
    Bitwarden itself does.
  - **The SSH private key keeps a fixed ten.** A private key runs to well over
    a thousand characters, so a true-length run would be cut at every window
    size there is — a value column filled edge to edge, indistinguishable from
    a truncated sixty-character password, telling you only "longer than the
    line". Ten dots at least read honestly as "this is hidden".

## 0.8.3 - 2026-08-19

> The manual pre-release checklist was not run for this version either. In
> addition to 0.8.2's overlay probe, the update flow on Preferences > About
> is new and every part of it is stubbed in the tests: the request to
> GitHub, the installer download, and the signature check. It has not been
> run end to end against the real service. If Check for updates does not
> behave, please open an issue rather than assuming it is your network.

### The overlay can generate the password it is about to save

The save-a-login card — the one that appears when you focus a password field
nothing in the vault matches — made you type a password in. Now it can make
you one.

- **A *Generate* link on the Password row** opens a new card: a fresh
  password, a **Words / Characters / PIN** selector, and a size stepper with a
  live readout that says what it is counting — "4 words" or "20 characters",
  not one fixed number that means different things in different modes.
  - **Words** is a passphrase; **Characters** is Deskwarden's usual
    twenty-character, four-class password; **PIN** is digits only. There are
    no character-class switches here — those stay in the vault window's edit
    form, because this card floats over whatever you are doing and every
    control on it is one more thing to fit on a window that cannot scroll.
  - The sizes you can pick are the ones the vault will honour without quietly
    changing them, which is why the shortest PIN offered is five digits rather
    than four.
- **Ctrl+R**, the *New* link, and any change of kind or size ask for another
  one. Only one request is ever outstanding.
- **The password comes from `bw serve`**, not from Deskwarden. Nothing in this
  app generates randomness for a password.
- **If it cannot reach the vault it says so on the card and stays open**, with
  *New* still live — a failed generate is not a dead card.
- ***Copy*** puts the password on the clipboard, under your usual clipboard
  clearing setting. ***Save to vault*** hands it back to the save-a-login
  card, with the username you had already typed still in it, where *Save*
  writes the item.
  - It says *Save to vault* and not *Fill*: this path cannot type into the
    window behind it, and a button claiming otherwise would be worse than no
    button.

### The tray stops claiming an update that is not there

The tray menu had an "Update available" item. It was created with those words
already on it and disabled, and the daily check's only effect was to *enable*
it. So on every session where no update existed — nearly all of them — the
menu asserted that one did, and then refused the click it was inviting.

- **The update item is gone from the tray.** So is the tooltip that announced
  updates: a tooltip is visible only while the pointer rests on a 16px icon,
  which is no way to report something you asked for and are waiting on. The
  tray tooltip still reports a sync, which is started from that menu.
- **The whole flow now lives on Preferences → About**, where it can say the
  thing the tray never could: that you are on the latest release.
  - A **Check for updates** button, so you can ask now instead of waiting out
    the 24 hours between automatic checks.
  - When there is one: the version, **the release notes**, and a Download
    button. Notes come from the GitHub release and are shown as plain text —
    nothing in them is treated as markup and nothing in them is clickable —
    inside a region that scrolls, so a long release note cannot push the
    buttons off a window that does not resize.
  - **Progress while it downloads**, on the page you started it from, then a
    *Restart to install* prompt. A failure says why, on the page, with a
    retry beside it.
- **The button works even when automatic checks are switched off.** That
  setting is about Deskwarden contacting GitHub on its own; pressing the
  button is you asking it to. The page says so where the button is, and
  `PRIVACY.md` now describes both checks rather than only the automatic one.

Nothing about what is downloaded or how it is trusted has changed: a check
reads a version and some notes, the installer is fetched only when you ask,
and it is verified against the signing certificate before anything launches
it.

### The vault can say which of its passwords are bad

A new **Password health** entry in the vault window's sidebar, under *Sends*.

- **Reused passwords**, grouped: the same password on more than one item, with
  every item in the group listed. This is exact rather than a guess, and it is
  the finding worth acting on first.
- **Weak passwords**, with the reason stated rather than scored — "9
  characters, lowercase letters and digits" instead of a number out of a
  hundred. It is the same rule the detail pane already uses for its
  *Strength* line, so the two cannot disagree about the same password.
- **Clicking a finding opens that item** beside the report, which stays where
  it is. Twelve reused logins is twelve clicks, not twelve trips through the
  sidebar.
- A long single-class passphrase is deliberately **not** called weak. Twenty
  lowercase characters is a wider space than eight mixed ones, and flagging it
  would be crying wolf at passwords this app's own generator produces.
- **Nothing here is sent anywhere.** The whole report is computed on your
  machine from the vault already in memory, and it makes no breach lookup in
  either state of that setting — opening a report over a large vault is not
  consent to a request per password. Breach status stays where you asked for
  it, on the item.
- Items with no password — cards, notes, SSH keys — are excluded rather than
  counted, and two items with empty passwords are not a reused pair.

### Internal

- The four preflight mutations that this project treats as a merge gate now
  live in `mutations/` as a harness that applies each one and reports which
  tests kill it. They had been recorded as prose, and the figure quoted in
  three places turned out not to be reproducible from it: three separate
  attempts produced three different numbers. The gate itself was never weaker
  than it looked — only the record of it was.

## 0.8.2 - 2026-08-19

> The manual pre-release checklist was not run for this version. The
> overlay's password-field probe is a cross-process call measured at a
> median of 27ms and a p90 of 133ms, whose cost falls on the application
> you switch to rather than on Deskwarden, and it has not been exercised
> outside the test suite. If focusing an application that asks for a
> password feels slower than it did, that is the likely cause; please open
> an issue.

### The overlay says something when nothing matches

Until now the autofill overlay could only appear when Deskwarden recognised
the application you were in. Anything else got silence — which looks exactly
like the app being broken.

- **The overlay now opens when a window asks for a password and nothing in
  the vault matches it**, and says so. It knows to ask because it checks
  whether the focused window contains a masked password box, through Windows'
  accessibility interface. It reads whether the box is masked, never what is
  in it. See [PRIVACY.md](PRIVACY.md).
- **A locked vault says it is locked**, rather than saying there is no saved
  login. Locking clears the match table, so before this the overlay would
  assert "no saved login" about applications that have one — a false
  statement about your own vault, from the surface whose whole job is to be
  trusted about that.
- An ordinary window with no password box is still silent, in both cases.

### One-time codes

- **A one-time code can be added by dragging a box around the QR code already
  on your screen.** The desktop dims, the dragged box stays lit, and the
  confirmation shows a live code before anything is written to the vault.
- **Or from a PNG file**, through the same picker.
- **Or by typing the key by hand.**
- An `otpauth://` URI is parsed strictly, and a bad one is refused by a name
  you can act on rather than being silently accepted.
- A window that is protected from screen capture produces a named refusal
  rather than a blank rectangle.

### The item detail pane

- **A close mark (✕)** on the detail header hides the pane and leaves the
  results, and it stays closed across a sync.
- **Send a record moved off the title bar** into the item's own header, next
  to the star, where the rest of the per-item controls are.
- **A one-time code control** joined that header.
- **The kebab menu can move an item between folders and clone it.** Cloning
  opens the create form pre-filled rather than copying in silence. It will
  not change an item's type — `bw` has no route for that, and the refusal is
  pinned by a test rather than remembered.
- **The folder name in the subtitle gets a drawn folder mark**, so two words
  side by side are no longer ambiguous.
- **The metadata strip says how old the password is**, and says nothing at
  all when it cannot know, rather than guessing from a zero date.
- Keyboard chords moved out of the printed labels and into hover tooltips,
  which gave the security code's reveal control its column back.

### Elsewhere

- **The vault locks when you lock Windows** (`Win+L`) or switch away from the
  session, through the same path every other lock takes.
- **The item list can be walked from the keyboard.**
- **A Send's link is copied the way a password is** — kept out of `Win+V`
  history and off your other devices, and taken back off the clipboard. The
  link carries the Send's decryption key after the `#`, so it *is* the
  credential, even though it does not look like one.

### Internal

- The tray menu's intermittent test failures were a use-after-free in
  `muda`'s `MenuItem::text()`, not flakiness: the `Vec` it hands Win32 is
  freed before Win32 fills it. Reproduced 9 times in 230 runs; 0 in 400 after.
- Four test path literals had been corrupted by escape interpretation across
  two generations that concealed each other, and are repaired.

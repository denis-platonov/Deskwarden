# Changelog

Notable changes to Deskwarden. Releases before 0.8.2 are described on the
[releases page](https://github.com/denis-platonov/deskwarden/releases); this
file starts here because 0.8.2 is the first release large enough to need one.

Dates are the release date. This project follows [semantic
versioning](https://semver.org/) loosely: the leading zero means the shape of
things can still change between minor versions.

## Unreleased

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

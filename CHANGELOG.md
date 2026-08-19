# Changelog

Notable changes to Deskwarden. Releases before 0.8.2 are described on the
[releases page](https://github.com/denis-platonov/deskwarden/releases); this
file starts here because 0.8.2 is the first release large enough to need one.

Dates are the release date. This project follows [semantic
versioning](https://semver.org/) loosely: the leading zero means the shape of
things can still change between minor versions.

## Unreleased

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

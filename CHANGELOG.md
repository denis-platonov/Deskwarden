# Changelog

Notable changes to Deskwarden. Releases before 0.8.2 are described on the
[releases page](https://github.com/denis-platonov/deskwarden/releases); this
file starts here because 0.8.2 is the first release large enough to need one.

Dates are the release date. This project follows [semantic
versioning](https://semver.org/) loosely: the leading zero means the shape of
things can still change between minor versions.

## Unreleased

Builds from this section report their version with a `-dev` suffix -- see
`Cargo.toml`. If the About line in Preferences reads `0.15.14-dev`, the build
is from the working tree and not from a
[GitHub release](https://github.com/denis-platonov/deskwarden/releases).

## 0.15.13 - 2026-09-04

### Notes, cards, identities and SSH keys have icons of their own

A vault is not all logins. Everything without a website used to wear the same
grey square with two letters in it, so a page of notes, cards and identities
all looked alike at a glance.

Each kind now has its own drawn mark: a written page for a secure note, a card
with its stripe, a head and shoulders for an identity, a key for an SSH key.
Logins keep their letters, because a login's name is a brand and its initials
say something. A site's real favicon still wins wherever there is one.

### Website icons are one size, centred, and no longer blurry

Favicons were drawn at whatever size the site happened to serve, so a list of
them was ragged — some filling the tile edge to edge, some lost in the middle
of it — and small ones were stretched to fit and came out soft.

Every icon is now drawn at the same size in the middle of its tile, with the
same space around it on every row. Icons that ship with empty space baked into
them are trimmed first, so what gets centred is the artwork you can actually
see rather than the file it arrived in.

### The details panel slides in instead of appearing

Opening an item from an empty panel now slides the details in from the right
edge, and closing it slides them back out. Clicking between items while the
panel is already open changes the contents in place, with no animation — the
movement marks the panel opening or closing, not every click.

### Searching your vault from the autofill card no longer costs the tray 40 MB

When autofill found nothing for an app, its card offered *Search vault* — and
that one button opened the vault window inside the background process instead
of in a process of its own. It was the last ordinary door that did.

The reason was the search term. Everything the background process tells a
window it starts goes on a command line, and command lines are readable by
every program on your PC; the name of an app you are signed into does not
belong there. The term now travels in a file in Deskwarden's own settings
folder, which nothing else can read, so that door opens a window the same way
every other door does.

The tray keeps the roughly 19 MB it is meant to sit at, and stays clickable
while the window is up — searching from the card used to freeze the tray icon
for as long as the vault stayed open.

### A window that fails to open now says so

If Deskwarden could not start the process that draws the vault window, it drew
one in the background process instead. That was a poor trade: it spent the
graphics driver, permanently, on a failure that is usually momentary — and it
froze the tray for as long as the window stayed up.

Deskwarden now tells you it could not open the window and leaves the tray
working, so *Open Vault* can simply be tried again.

### The vault window says why it did not open

Opening the vault could do nothing at all — no window, no error, no dialog.
The window runs in its own process, and when that process could not read the
vault it exited quietly; the only trace was a line in the log file. Five
different failures shared one exit code, so even the log could not always say
which.

Each cause now carries its own code home, and Deskwarden puts it on the screen
with the one thing you can do about it: a missing vault key, a missing server
address, a missing sign-in, a missing Bitwarden CLI, or an unreadable settings
folder.

### Signing in to your own server no longer costs the tray 40 MB

On an account served by the built-in client, the sign-in card now opens in the
same short-lived process as the vault window rather than in the background one.

This matters because of how graphics drivers work: the first window a process
draws maps the OpenGL driver into it, and closing the window does not give that
memory back — only ending the process does. Deskwarden's background process is
meant to sit in the tray at about 19 MB, and it did, right up until the launch
where it had to ask for your master password. From then on it held roughly
59 MB for as long as it ran, whether or not a window was open.

The window that asks now exits when you close it, and takes the driver with it.
The background process stays a tray icon, on every launch, signed in or not.

**Accounts on the official Bitwarden CLI are unchanged for now.** That sign-in
produces a session token the background process itself has to start `bw serve`
with, so it still happens there; moving it needs a different mechanism and is
being done separately.

### Switching to the built-in client no longer strands the account

The built-in client reads your vault with a master key kept on this PC, and
that key is written only by a sign-in taken through Deskwarden. An account that
switched to the built-in client — or whose sign-in did not leave a key behind —
had nothing for the vault window's process to read, and that process could not
ask for a master password.

It can now: the window opens on a sign-in card, stores the key, and carries on
into your vault, which finishes the switch without the background process ever
drawing anything.

### The tray icon no longer disappears while you sign in

On a launch that needed your master password, the tray icon was missing until
you closed the vault — the background process was busy being the window. It
builds the tray first now, so the icon, the fill hotkey and autofill are all
there while the sign-in card is still on screen.

## 0.15.12 - 2026-09-03

### "Restart to update" installs the update

Pressing it did nothing at all: no new version, no error, nothing in the
log. Deskwarden asked Windows to run the installer and Windows did — but
setup found Deskwarden still running, and because a silent install has
nobody to ask, it gave up without a word.

The window you press the button in is not the part of Deskwarden that setup
objects to. Deskwarden now stands the rest of itself down and waits until it
is really gone before starting the installer, and says so if it cannot.

**Every way this can fail now says why**, on the page and in the log. Before,
three of the paths through that button ended in silence.

### A downloaded update is not deleted while you are about to install it

Deskwarden clears abandoned downloads when it starts. It decided what was
abandoned from the file's name alone — so a restart while the Updates page
sat ready would delete the very installer that page was about to run.

The process that downloads one now holds a claim on it, and the clean-up
skips anything still claimed. Genuinely abandoned downloads are still
removed, at the same moment as before.

### The Updates page names the release once

It stacked the release's name three times above the notes. Now there is one
heading: the release's own.

## 0.15.11 - 2026-09-03

### Unlocking a self-hosted vault actually unlocks it

On a vault served by Deskwarden's built-in client, the **Unlock** button on
the locked screen ran Bitwarden's command-line program — against a profile
that was never signed in, on a machine that need not even have that program.
It could not succeed, and it never gave up: it showed an error and asked
again.

Unlocking now uses whichever client actually holds your vault.

### Sync, Sends and exporting no longer decide for themselves

Six things — Sync, exporting, and the four Send actions — each chose
between the two clients on their own, and each of those choices was a place
the choice could be forgotten. Every bug fixed in the last two releases was
one of them forgetting.

They now go through one place that makes that decision once. The other
nineteen vault operations already worked this way and have never had a bug
of this kind.

### A leftover from an old sign-in no longer wakes the CLI

An account moved to the built-in client, but still carrying a session token
from when it used the command-line program, asked that program a question at
every startup. It no longer does.

## 0.15.10 - 2026-09-03

### Opening your vault is about three times faster

On your own server, opening the vault window fetched and decrypted your
whole vault three times over — once for a readiness check that is only
meaningful for the Bitwarden CLI, once for the items, once more for the
folders. It now takes all of it from a single request.

If you turn on **Keep an encrypted copy of your vault on this PC**, the
window paints from that copy on its first frame and fetches nothing at all.

### Closing to the tray can be undone

With **Open the vault instantly** on, closing the window hid it correctly but
nothing could bring it back: the window was waiting to be raised only from a
frame that a hidden window never runs. Deskwarden thought it had reopened it.
The window stayed in memory, invisible and unreachable.

### Icons for machines on your own network

Deskwarden asked `/favicon.ico` and two other fixed paths. qBittorrent's web
interface serves none of them — it names its icon in its page instead, as
many self-hosted tools do. Deskwarden now reads that, for addresses on your
own network only, following only what the page declares and only on the same
address.

The icon code also had no logging at all, so a missing icon was silent. It
now records which address it asked and what came back.

### Smaller things

- The item type and folder can be selected and copied, like the title.
- **Open the vault instantly** no longer blames a settings change for closing
  the window; it says what actually does.
- A vault on your own server no longer retries a backend start every 30
  seconds for a backend it never had.

## 0.15.9 - 2026-09-03

### Unlocking your vault no longer asks which client to use

Unlocking a vault you are already signed in to put the "Which client should
open this vault?" question in the way — and worse, the unlock never
completed: the vault stayed locked and autofill stopped matching apps until
you used Sync.

That question belongs to a new sign-in and now only appears there.

### Sync works on your own server

The Sync button ran Bitwarden's command-line program even on an account that
does not use it, so it failed on every press. It now re-fetches your vault
from your own server, and only says it synced if that worked.

### Your Sends list opens

Listing Sends on a self-hosted account always failed with a lock message.
The credential it loads from disk has to refresh before its first request,
and this one call did not — so it asked your server with nothing and was
turned away. Creating and revoking were unaffected.

### Site icons for machines on your own network

An address like `192.168.1.5:8080` never showed an icon: icons are fetched
by your Bitwarden server, which has no route into your network. Deskwarden
now fetches those itself — and keeps the port, which it used to drop.

A new setting under **Show site icons** lets it fetch every icon that way
instead of asking an icon service. **Off by default**, because it means each
site you hold an entry for receives a request from your PC. `PRIVACY.md`
describes exactly what that discloses.

### Small things

- An item's title can be selected and copied like ordinary text.
- The two buttons in the client question are the same size.

## 0.15.8 - 2026-09-02

### The tray stays a tray

Signing in used to happen in the tray process itself, and the window it drew
held on to about 40 MB of graphics memory for as long as Deskwarden kept
running — long after you closed it. Worse, while that window was up the tray
menu, the fill shortcut and the app-matching all stopped: the tray was busy
drawing.

Signing in now happens in the window's own process, for both kinds of
account. The tray keeps its own memory, the menu keeps working while the
vault is open, and the shortcut keeps firing.

**Searching your vault from the fill overlay** did the same thing and no
longer does.

If opening the vault window ever fails, Deskwarden now tells you instead of
quietly drawing the window itself and keeping the cost forever.

One screen is still drawn by the tray on purpose: the one that appears when
the vault cannot be reached at all, which needs what it finds to stay where
the fill shortcut can use it.

## 0.15.7 - 2026-09-02

### Your own server needs no Bitwarden CLI, in every window

Three things still reached for that program on a self-hosted account using
the built-in client, and all three are fixed:

- **The vault window refused to open at all** without it, and told you to
  reinstall Deskwarden to get it back — which would not have worked, since
  setup stopped installing it in 0.15.1.
- **Exporting your vault and anything to do with Sends** reached for it from
  inside that window, and failed.
- The window and the background service each decided which client to use
  before being told which one you had chosen, so both assumed the CLI.

Deskwarden now settles that question once in every process it starts. There
are tests that count what actually runs and require zero.

### The memory figures are ones you can check

Every memory number shown in the app is now measured on the counter Windows
Task Manager shows in its **Memory** column, and says which state it applies
to. The tray is about **19 MB**; the vault window about **76 MB** while it is
open. A figure of "100 MB in a second process" appeared nowhere in any
measurement and is gone.

## 0.15.6 - 2026-09-01

### Your first sign-in is remembered straight away

After signing in for the first time, Deskwarden kept showing you as signed
in to nothing: **Settings** claimed you were using the Bitwarden CLI when you
had chosen the built-in client, and the account menu showed a long code
instead of your address. The vault itself was fine — only the record of it
was missing, and it was not written until you closed the vault window.

It is written the moment you sign in now.

### The Vault settings page follows the client you chose

**Which client opens this vault** is now a pair of named choices rather than
a switch for one of them, and the settings that only apply to the Bitwarden
CLI disappear when you are not using it.

Changing it takes effect the next time you sign in to that account — not
after a restart, as the old wording said.

### The sign-in question is shorter, and two of its warnings were wrong

It said the built-in client could not put a file in a Send, or ask for an
e-mail address before one is opened. Neither is a reason to choose one
client over the other: the Bitwarden CLI does not do those things here
either. Both claims are gone.

What remains is the honest comparison: about 21 MB of memory and no wait
after a restart, against about 118 MB and roughly 8 seconds — and the one
real cost, that the key unlocking your vault is then kept on this PC.

## 0.15.5 - 2026-09-01

### Signing in asks which client should open your vault

On a **self-hosted** server, signing in now asks whether to use Deskwarden's
built-in client or Bitwarden's official CLI, and says what each one costs
you. It asks every time, with your last answer already selected, so changing
your mind is one click rather than a hunt through Preferences. Either answer
takes effect on that sign-in.

On **bitwarden.com or bitwarden.eu** there is no choice to make — those
servers need the CLI — so it tells you what it is about to install
instead of asking.

### A window that cannot open now says why

Switching to the built-in client could leave the vault window silently
failing to open: no window, no error, nothing in front of you. The tray and
autofill kept working, which made it stranger.

The window now opens anyway — and where it genuinely cannot, it says which
of the five reasons stopped it instead of exiting quietly.

## 0.15.4 - 2026-09-01

### Signing in to your own server really does not need the Bitwarden CLI

0.15.3 stopped one call to that program on this path. There was a second,
and it was the one that mattered: the sign-in itself. Typing a self-hosted
address on a machine without the program failed with **The system cannot
find the file specified**, because the sign-in was deciding how to
authenticate from what Deskwarden knew when it started — before you had
typed anything — rather than from the address you just entered.

It now decides from the address you typed. On a self-hosted server with the
built-in client, signing in runs no part of the Bitwarden CLI, and there is
a test that counts and requires zero.

## 0.15.3 - 2026-09-01

### Signing in to your own server no longer asks for the Bitwarden CLI

Typing a self-hosted address and pressing Continue failed with an error
about `bw config server` if you did not have Bitwarden's command-line
program installed — on the one path that is not supposed to need it. The
sign-in now only configures that program when it is the one serving your
vault.

Your server address is still recorded either way, so the account knows where
it lives.

### The "Bitwarden CLI is required" box shows its text

Signing in to bitwarden.com opened a box that was empty apart from **Cancel**
and **OK** — no heading, no explanation of what you were agreeing to. The
words were being drawn off the top of the window. They are on screen now.

### The fill shortcut is in the tray menu

The keyboard shortcut for filling is shown next to **Add app...**, so it is
there when you need it rather than remembered from Preferences. It is a
label, not a button: filling goes to whichever window is in front, and
opening the tray menu would make the tray that window.

## 0.15.2 - 2026-09-01

### Settings for one account no longer change another

If you have more than one account — say one on bitwarden.com and one on
your own server — opening **Settings** from the vault window could quietly
move the account you were using onto the other one's backend. It did not
need you to change anything: opening the panel and closing it was enough.

The panel was reading that one setting from a file shared by the whole
computer, left over from before the setting became per-account. It now reads
it from the account you are actually using, which is what the tray menu
already did.

If this happened to you, the fix is to set the option the way you want once,
under **Settings—Vault**. It will stay put now.

## 0.15.1 - 2026-08-31

### Deskwarden starts even when the Bitwarden CLI has gone missing

If you use bitwarden.com or bitwarden.eu and the CLI disappeared — your
antivirus quarantined it, a cleanup removed it, or an update went wrong —
Deskwarden refused to start at all. A message box said it could not start
its backend, and that was the end of it: no window, no way to sign out, no
way in.

It now offers to reinstall the CLI, in the same way it does when you first
choose a Bitwarden server — downloading it from Bitwarden and checking
Bitwarden signed it. If you decline, or it cannot be fetched, **the app
still opens** and says what is unavailable, instead of closing.

Autofill keeps working from your encrypted local copy while the backend is
unavailable, if you have that copy switched on.

### Uninstalling can take your data with it, if you want

The uninstaller now offers a checkbox: **Also delete all data and
settings**. It is off unless you tick it, so uninstalling for a reinstall or
a version change leaves your vault alone.

Leave it clear and your settings, signed-in accounts, the encrypted copy of
your vault and the key that unlocks it stay on this computer under
`%APPDATA%\Deskwarden`. The uninstaller says so on screen rather than
leaving you to wonder. Tick it and that folder goes, key included.

Either way, uninstalling now removes the rest of what Deskwarden put on your
PC: the Bitwarden CLI it installed for you, the folder it installed it in,
the entry it added to your PATH, the start-with-Windows entry, and its
cached site icons. A `bw` you installed somewhere else yourself is left
alone.

## 0.15.0 - 2026-08-31

The short version: if your vault lives on your own server, Deskwarden no
longer needs Bitwarden's command-line program to do anything. It is faster to
start, and there is one less thing installed on your PC.

### Setup no longer installs the Bitwarden CLI -- Deskwarden asks, then does it

Installing Deskwarden used to spend a wizard page, a minute of your time and
about 37 MB of your connection downloading the Bitwarden CLI, before you had
chosen a server that needs it. It does not any more. Setup copies the app and
finishes.

Instead, the first time you sign in to a Bitwarden server (bitwarden.com or
bitwarden.eu), Deskwarden tells you plainly that the official Bitwarden CLI is
required and asks whether to install it:

- **OK** downloads it from Bitwarden, checks that Bitwarden signed it, and
  installs it -- with a progress bar that names each step, not a spinner. It
  then tells you the version and size of what landed, and continues the
  sign-in you were already doing. You press Continue once.
- **Cancel** goes back to the server choice. Nothing is downloaded, because
  the question comes before the first byte.

It happens once, ever. A self-hosted server with the built-in client never
asks and never downloads anything, and a machine that already has a
Bitwarden-signed `bw.exe` is not asked either.

**What is checked, and what that is worth.** Bitwarden publishes no signature
file and no checksum file for the CLI; what their documentation calls a
checksum is GitHub's own per-asset digest, which arrives over the same
connection as the download. Deskwarden checks it, and it catches a corrupted
or truncated transfer -- but it proves integrity, not origin. The check that
proves origin is the Authenticode signature on `bw.exe` itself, issued by a
certificate authority that had to validate the name *Bitwarden Inc.* That is
verified before the file is installed, and the file is never executed --
not even to read its version. A download Deskwarden cannot prove came from
Bitwarden is deleted, and it is not retried.

If anything goes wrong, the message says which thing was required, that
bitwarden.com cannot be used without it, and that a self-hosted server can.

This also removes the second copy of a security check the app already had.
The installer verified signatures with PowerShell and kept its own
hand-maintained certificate-name parser "in sync by hand" with the app's.
Both are gone: Deskwarden now verifies Authenticode in one way, in one place.

Uninstalling still deliberately leaves `bw` and its `PATH` entry behind --
you may be using it independently of Deskwarden.

### Your own server is now the built-in client's, by default

A new self-hosted account uses Deskwarden's built-in client without you
turning anything on. Nothing is downloaded and no command-line program is
involved.

**Accounts you already have are not touched.** An account that exists before
this update keeps the backend it has today, whatever your settings said, and
keeps working exactly as it does now. There is no migration and nothing to
redo.

**The choice is now per account, not per machine.** You can keep a
bitwarden.com account served by the official CLI and a self-hosted account
served by the built-in client on the same PC, and switching between them
does not disturb either. Preferences shows the setting for whichever account
is active.

### Your own server needs nothing but Deskwarden

Deskwarden used to run Bitwarden's command-line program, `bw`, behind the
scenes — for your passwords, your Sends, your name in the window, signing in
and exporting. On a self-hosted server it does all of that itself now.

You will notice it as a window that opens straight away instead of pausing
for a couple of seconds, and as an app that no longer needs a 126 MB
companion program on disk.

**Using bitwarden.com or bitwarden.eu still needs that program.** Deskwarden
now offers to install it when you pick one of those servers, instead of
installing it for everybody at setup time whether they need it or not.

### Share a link, and open one

Creating and revoking a Send works without the companion program. So does
**opening** one — which matters more than it sounds: Bitwarden changed how
their servers hand out shared links, and the older program cannot open them
on an up-to-date server. Deskwarden can.

Sends that hold a file, or that ask the recipient for an emailed code, are
still not offered. They never were — but now Deskwarden says so plainly
instead of failing in a way that looked like the link was broken.

### Export your vault

**Export vault...** no longer needs the companion program either. The file is
the same file: a Bitwarden `encrypted_json` export, locked with your own
account key, which imports back into Bitwarden.

A plaintext export could previously be reported as an encrypted one. It
cannot now.

### The window comes back instantly

With **Keep the interface loaded** turned on, closing the vault window now
hides it rather than shutting it down, so the next click brings it straight
back. Settings you changed before closing still take effect — including the
case where a setting could not be switched on, which is still reported to you
rather than quietly recorded as on.

Minimising behaves exactly as it did.

### Settings show what applies to you

Settings that describe the companion program are no longer shown to people
who are not using it.

### The sign-in screen knows who you are

On a self-hosted account the sign-in screen used to say you were signed out
while your vault was open, and offer an email box it then ignored. It reads
your account now, and the server box opens on your own server's address
instead of Bitwarden's.

## 0.14.0 - 2026-08-30

### Keeping an encrypted copy on this PC no longer asks for anything

Turning that setting on used to stop Deskwarden starting at all: it wanted
Windows Hello every launch, and when the prompt did not appear it waited for
it forever, with no window and nothing in the log.

The copy is now protected the same way the key that unlocks your vault
already is — kept on this PC, protected by Windows. **No prompt, ever.** A PC
without Windows Hello can use the setting now, which it could not before.

**What that means, plainly:** anyone who can run programs as you on this PC
can read the copy, and so can someone who takes this disk and knows your
Windows password. That is the same protection your vault key already had, and
the same trade other password managers make when you tell them not to lock.
When it locks is still yours to choose, under **Lock the vault when you step
away**.

The setting's description said it was protected by a TPM chip. That stopped
being true and now says what actually gates the file.

**Your existing copy is rebuilt once** on first launch after updating, because
the old one cannot be opened with the new key. Nothing is lost — it is a cache.

## 0.13.2 - 2026-08-30

### Fixed

- **The whole app could vanish when you pressed CTRL+ALT+B on a window with
  no saved login** -- taking the tray, the vault window and an unlocked
  session with it, and leaving nothing in the log. Drawing a card with an
  empty piece of text read an invalid address and Windows killed the process
  outright. Not new in 0.13, but easy to meet.
- **The Updates page said it could not check for updates.** It was right:
  the window that draws Preferences had not been told how. Same for the
  Breaches page, which would have accepted a click and done nothing.

## 0.13.1 - 2026-08-29

### Fixed

- **The vault window could not load on a self-hosted account that talks to
  the server directly.** It showed "Your vault could not be loaded" after a
  long wait, over a vault that was perfectly reachable. The window had moved
  into its own process in 0.13.0 and kept asking for the vault through the
  Bitwarden CLI's local server -- which, on that kind of account, Deskwarden
  correctly never starts. It now reads through the same backend the rest of
  the app uses.

## 0.13.0 - 2026-08-29

### Two-step login works, and the Bitwarden CLI is no longer in the way

If your account uses two-step login, Deskwarden can now ask for the code
itself. Before this it could not, and said so: it told you to run
`bw login` in a terminal and come back.

It handles the three factors the Bitwarden CLI handles -- **authenticator
app, emailed code, and YubiKey** -- and for accounts using Duo or a
passkey, which the CLI cannot do either, there is now a **personal API
key** sign-in instead. That is the same escape hatch `bw login --apikey`
gives those accounts.

A wrong code no longer costs you your master password: the code box
clears and the password is not asked for again.

**On a server Deskwarden talks to directly, signing in no longer runs the
CLI at all.** It used to run `bw` first and only then do its own sign-in,
which is why a two-step account could never get in -- `bw` cannot answer a
second factor on its own, so it failed, and everything after it was
skipped.

### Locking Windows now closes the vault window

Since the vault window moved into its own process, locking your PC left
that window running with your vault decrypted behind the lock screen.
It now closes, on the same setting that governs locking (**Preferences →
Vault → Lock the vault when you step away**). With that setting off,
nothing changes -- the window stays, as it does today.

### The vault window opens in its own process

It used to be drawn by the same process that holds the tray icon, which
made that process load the graphics driver and keep it for the rest of
the session: **98.6 MB against 33 MB**. It also meant no tray icon at all
while the window was open. Both are fixed.

### Open the vault instantly (optional)

A new switch under **Preferences → Vault**. With it on, closing the vault
window hides it instead of ending it, so the next open is immediate. It
holds about 100 MB while the vault is unlocked, and it is **off by
default**. Locking, switching account or changing settings still close it
fully.

### Fixed

- An install created before the `--autostart` flag existed kept starting
  Deskwarden as if you had double-clicked it, which drew a window and
  loaded the graphics driver into the tray process on every sign-in.
  Existing installs now repair their own logon entry.
- The tray icon no longer disappears while the vault window is open.
- Five Preferences pages that had nothing on them are gone.

### For contributors

- The test suite is trustworthy again. It had been failing 27 to 54 tests
  a run out of 4160, with a different set failing each time, which meant
  a real failure was indistinguishable from noise -- a permanently red
  test hid in it for days. The mock HTTP server used by 209 test call
  sites is now hand-rolled rather than `mockito`, and the library suite
  runs green.
  **Known remainder:** twelve sites in `main.rs` cannot reach that seam,
  because it is `cfg(test)`-gated in the library and the binary is a
  separate crate. Those still produce 3-4 intermittent failures out of
  333 in the binary suite. Fixing it means a feature gate, not a rename.
- CI no longer runs the full matrix for documentation-only pushes, and
  pushes to `main` no longer evict each other's pending runs -- which had
  been silently skipping commits from the `compiles` job's range checks.

## 0.12.0 - 2026-08-28

### Your own scripts can read the vault, over a local API

Deskwarden can now answer HTTP on `127.0.0.1` so a program you write can
read your vault — a backup script, a deploy step, anything that needs a
password without a human typing it.

It speaks the **same API `bw serve` does**, so anything already written
against that keeps working with one change: it must send a key.
`bw serve` asks for no credential at all, and this does.

**It is off by default**, and while it is on it serves decrypted vault
items to any program on this PC holding a key. Turn it on under
**Preferences → Local API**, where you also mint keys.

Each key has a name, an optional expiry, and a scope: everything, one
category, or a single item — read, write, or both. A key is shown **once**
when you make it and never stored: the file keeps only a hash, so a copy
of it grants nobody anything. An empty scope grants nothing at all.

Start it with `deskwarden.exe --service`, or `--service installed` for a
service that stays up whether or not the app is open.

**Not built yet, and answered honestly rather than silently:** writes are
refused with `501` — the scope model accepts write grants, but no write is
performed. Signing in with a master password over the API is also `501`;
the service uses the credential the app already stores when you sign in,
and never asks for a master password itself.

### Settings about the vault are on one page, and the API has its own

**Preferences → Vault** now carries everything that decides how Deskwarden
reaches your vault: whether the `bw` CLI or Deskwarden's own client does
the decryption, whether the backend stays running, and the encrypted copy
on this PC. Those were spread over two pages.

Switching between `bw` and the built-in client now **asks first**, and says
what it costs: a restart and a fresh sign-in, plus — when switching back to
`bw` — that the stored vault key is deleted from this PC.

### Fixed: save-memory mode could stop the vault window loading

With **Keep the backend running** turned off, opening the vault could sit
for a minute and then show *Your vault could not be loaded* with nothing in
it. The backend was stopped whenever nothing needed it, and opening a
window did not count as needing it.

It does now, in both directions: opening the vault starts the backend, and
it is no longer stopped while a window is open.

This was worth fixing quickly because it was self-trapping — the switch for
that setting lives inside the window that would not load, so there was no
way back through the app.


## 0.11.1 - 2026-08-26

0.11.0 was tagged and never released: its build failed on three tests, so no
installer was ever published under that number. Everything below was meant for
it and ships here instead.


### Self-hosted vaults can skip the Bitwarden CLI entirely

If your vault is on your own server, Deskwarden can now talk to it directly
instead of running the official Bitwarden CLI in the background. Turn off **Use
official bw for crypto** on the Sync & account page and restart.

What that changes, on a 1,666-item vault:

- The background `bw` process is gone. It was holding about 118 MB.
- Opening the vault window settles in about 20 milliseconds instead of about
  200.
- Deskwarden does the decryption itself, which means the key that unlocks your
  vault is kept on this PC, protected by Windows. Unlike the session it
  replaces, that key does not expire. The setting says so before you turn it
  on, and this is the trade: less memory and more speed, against a stronger
  secret at rest.

The setting is only offered on a self-hosted server. On bitwarden.com and
bitwarden.eu the vault always goes through the official CLI.

**Signing in still uses the Bitwarden CLI**, including on this path, so the CLI
is still installed. Accounts with two-factor authentication are refused by name
rather than left to fail as a wrong password.

### Two settings about the same thing became one card

**Keep the Bitwarden backend running** used to live on the General page, two
pages away from the switch that decides whether there is a backend at all --
and with the direct connection selected it did nothing, while still looking
like a live switch. It now sits directly under that switch on Sync & account,
and goes quiet when there is no background process to keep running, with a
sentence saying why.

### Deskwarden generates its own passwords

Password and passphrase generation used to be one more thing asked of the
Bitwarden CLI. It is now done in Deskwarden, which is what lets the direct
connection offer it at all. Passphrases come from a 4,096-word list, which is
exactly 12 bits of entropy per word -- so the strength shown is a number that
can be justified rather than estimated.

### Fixed: an edit could quietly delete a field Deskwarden could not read

On the direct connection, a field Deskwarden failed to decrypt -- a card
number, a one-time-code seed, an SSH private key -- was dropped when the item
was next saved, and an unreadable name was replaced with an empty one. Saving
an app match during an ordinary autofill was enough to trigger it. Anything
that cannot be decrypted is now carried through an edit byte for byte.

### Fixed: archiving and un-archiving, and a delete that only trashed

Both went to routes this app had guessed at. Archive reported that it might not
have worked when it had, and **Delete forever** on an item that was not already
in the trash moved it to the trash instead and reported success. Both are
verified against a real server now.

## 0.10.0 - 2026-08-26

### Pressing the hotkey on an app you have never bound now offers your accounts

Until now, `CTRL+ALT+B` on an app you had not configured said Deskwarden had
nothing for you -- even when your vault held the login. It only ever looked for
apps you had explicitly bound.

It now looks for accounts that plausibly belong to the window in front of you,
by web address, by name, and by the words in the window's title, and lists what
it finds. Pick one, then pick what to type: username, password, one-time code, a
custom field, all of it, or the item's own saved sequence. Nothing is ever typed
without you choosing it, which is what lets the search be generous.

Press `S` to search the whole vault from the same card, `N` to start a new
login, and `1` to `9` to pick straight off the list.

### The vault window now gives its memory back when you close it

Deskwarden's window is an accelerated graphics window, and the graphics driver
never returns what it takes -- so once you had opened the window, this app held
about 50 MB for the rest of the session, whether or not you ever opened it
again.

The window is now its own process, and closing it ends that process. Sitting in
the tray, Deskwarden is back to roughly 10 MB and stays there. Closing the
window also no longer freezes the tray icon or the hotkey while it is open, and
a crash in the window no longer takes the rest of the app down with it.

### Every small card is drawn by hand instead of by the graphics stack

The cards that appear during a fill -- the offer to type a saved login, the
"vault is locked" card, the offer to save a new login, the generator, the unlock
prompt and the send confirmation -- no longer start a graphics context. They are
drawn directly by Windows, cost under 2 MB each, and look the same. This is what
keeps the tray at 10 MB even after you have used it.


### Filling a password is faster

Every autofill used to make a private copy of your entire vault just to look up
the one item it was about to type -- several megabytes of copying, on a large
vault, sitting between the moment you pressed the key and the moment the
password appeared. Deskwarden now asks for the one item it needs. On a
1,663-item vault that removes about 3.7 milliseconds and roughly 34,000 memory
allocations from every fill; the bigger your vault, the more it was costing
you. Nothing about what gets filled has changed, and a fill still works with
the connection to Bitwarden fully stopped.

### A locked vault can be unlocked from the card that says it is locked

Focus a password box while Deskwarden is locked and a small card appears
saying so. Until now that was all it did: to unlock, you had to open
Deskwarden's window, type your master password, close it again, and go back to
what you were doing. The card told you what was wrong and gave you no way to
fix it.

The card now has an **Unlock** button. Pressing it opens a small
master-password prompt -- not the app's window -- and a correct password
unlocks the vault, brings the connection to Bitwarden back up, and then looks
at the window you were in front of again. If it has a saved login, you get the
same offer to fill it that you would have had if the vault had been open all
along.

The prompt is deliberately small. Deskwarden's own window is an accelerated
graphics window: showing one costs about 95 MB that Windows does not give back
when it closes, and about 4 MB more every time it is opened and closed again.
The unlock prompt costs about 1 MB while it is on screen and nothing after it,
because it is a plain Windows dialog with no graphics stack behind it. It is
excluded from screen capture and screen sharing, as the app's own
master-password box is.

A few details worth knowing:

* **A wrong password keeps the prompt open**, with Bitwarden's own reason
  underneath the box. Nothing is lost and nothing is locked out -- a refused
  password is refused by the Bitwarden CLI, not by Deskwarden, and there is no
  attempt counter here. Try again, or press Cancel.
* **Cancel leaves everything exactly as it was.** The vault stays locked,
  nothing is armed, and no fill is waiting. The card is not put back: you
  pressed a button on it, and it appears again by itself the next time you
  focus a password box.
* **If you moved to another window while typing your password**, nothing is
  typed into it. Deskwarden never types into a window that is not the one it
  was asked about; the offer is still armed for the original window, so
  clicking back into it and pressing `CTRL+ALT+B` fills it.
* **The card's wording changed** with the button: it used to tell you to go and
  unlock somewhere else, and now the button is the instruction. It still does
  not claim to know whether the vault has a login for the app -- while locked,
  it cannot.

If Deskwarden's overlay is switched off in Preferences, this card does not
appear, exactly as before.

### Editing an item shows what it has, and an Add control for the rest

The edit form used to draw every box the item type has, filled in or not. An
identity is eighteen of them, and a real one uses three or four -- so the
fields with something in them were scattered through fourteen that were empty,
and finding the one you came to change meant reading past the rest.

Editing an item now opens showing the fields it actually has. Everything else
is behind an **Add...** control that lists the missing fields by name; pick
one and its row appears, ready to type into. Each optional row has a **Remove**
beside its label that takes it away and clears it.

Some rows are always there, because an edit form without them would be a form
for something else: a login's username and password, a card's number and
brand, an identity's first and last name, a note's body. A card's brand is
always shown for a second reason -- the form fills it in from the number, and
a value the form sets should not be one the user cannot see.

A field you add and then leave empty is not saved. It disappears again, which
is the same thing that happens to a field you clear -- an empty box has always
meant "this item does not have this", and it still does. The Add menu says so
where you pick.

Creating an item is unchanged: everything is empty on a new item by
definition, so there is nothing to hide and no list worth putting behind a
control.

### Websites can be edited, and a login can have more than one

The edit form had no box for an item's website. The item could have one --
the item's page showed it, and its icon came from it -- but there was nowhere
to change it, add one, or take one away. Editing anything else left it exactly
as it was, which is why the gap was easy to miss.

The form now shows every website the login carries, one box each, with a
Remove beside it and an Add under them. Bitwarden lets one login list several
addresses for the same account -- the app, the single-sign-on host it
redirects to, the mobile package -- and all of them are shown and editable.
When there is more than one, a line says which is the first: that is the one
the item's page displays and the one its icon is fetched from.

The per-website match-detection setting that Bitwarden's other apps offer is
not shown here, because nothing in Deskwarden behaves differently according to
it. Whatever those apps have set is kept exactly as it is through every save
this form makes, and the block says so.

A website can be added once an item exists; the "new item" form says so rather
than offering a box whose contents it would have to throw away.

### "What is new" uses the whole page instead of a small box

The release notes on **Preferences -> Updates** were shown in a box of a fixed
height with everything past it scrolled out of sight, whatever the release
actually said. Short notes sat in that box; long ones were cut off a few lines
in. The area now takes whatever room the page has left and only scrolls once
the notes run past the bottom of the window, so most releases are readable
without scrolling at all and a one-line note takes one line.

Nothing below the notes can be pushed out of reach by a long release body,
which is what the fixed height was protecting against: the area grows into the
space that is left rather than to the length of the text, and there is nothing
underneath it to displace.

### "Full changelog" and other links in the notes can be clicked

Links in the release notes were painted to look like links and did nothing.
They can now be followed, and they open in your normal browser.

Only `https` links are followable. Anything else -- an `http` address, a
`file:` path, a `ms-settings:` link -- is shown as ordinary text rather than
as something to click, so nothing on the page looks like a link that will not
behave as one. Every link still shows its destination beside its words,
followable or not, so you can see where one goes before deciding to go there.

### Release notes lead with what the release does

A published release used to open with the note about the build being unsigned,
which meant the app's "What is new" panel did too. A release now starts with a
short list of what changed -- taken from this file's entry for that version, so
it is written once -- followed by a link to the full changelog as it stood at
that release, and then the unsigned-build and digest-verification notes.

## 0.9.0 - 2026-08-21

> The manual pre-release checklist was not run for this version either. The
> vault can now be kept on this PC, encrypted under a key Windows Hello holds
> in the TPM -- it is **off by default**, and with the setting off no file is
> ever written. A login that has a copy now reaches the tray without waiting
> for the Bitwarden CLI. None of that, nor the first window's states, has run
> outside a test. If a launch, an update or the local copy misbehaves, please
> open an issue.

### Fixed: Delete in an item's ⋮ menu did nothing and did not light up

The first click on **Delete** arms a confirmation -- the entry changes to
"Delete? Click to confirm" and a second click does the deed. But the menu shut
itself the instant that first click landed, so the armed entry was never on
screen to be clicked and the item was never deleted. Reopening the menu did
not help: every fresh first click simply re-armed. The only sign that anything
had happened was the ⋮ itself turning red.

The same entry also never lit up under the pointer, while Edit, Clone and
"Move to folder" above it all did -- which made its row read as a label rather
than as something you could click.

Both are fixed. A click inside this menu now closes it only when the entry it
landed on asks to close, and Delete highlights across the full width of the
menu exactly like its neighbours. Deleting is still two clicks, still a short
moment apart, and still leaves the item recoverable from bitwarden.com.

### Keep an encrypted copy of your vault on this PC (off by default)

Deskwarden starts unlocked from a cached session token, but the vault itself
is rebuilt on every launch by asking the Bitwarden backend for it -- and that
backend is a bundled Node process whose cold start is about eight seconds.
Autofill is dead for those eight seconds; so is the vault window's content.

A new setting, **Preferences -> General -> "Keep an encrypted copy of your
vault on this PC"**, keeps that snapshot in a file so the next launch reads it
in milliseconds instead. It is **off by default**, and with it off no file is
written at all.

What gates the file, because that is the part worth reading before turning it
on. The copy holds your usernames, passwords, notes and two-factor secrets. A
random key encrypts it; that key is sealed under a key only a Windows Hello
verification can produce, whose private half lives in this PC's TPM chip; and
the whole thing is wrapped for your Windows account exactly as the cached
session token already is. So a copied or stolen disk cannot be read on another
machine even with your Windows password -- which is the one thing the wrapping
alone could not promise, and the whole reason the setting exists in this shape.
Anything running as you on this PC that can pass Windows Hello can read it.

It is **not** deleted when your vault locks: surviving a restart is what it is
for. It is deleted when you log out, whenever you are asked for your master
password again, and when you turn the setting off; and it is refused and
deleted if it is more than seven days old, belongs to a different account, was
written by a different version, or cannot be opened. Each of those is decided
before Windows Hello is asked for anything, so a file Deskwarden is about to
throw away never costs you a fingerprint.

Without Windows Hello the setting is unavailable and says why. Deskwarden does
not offer a weaker file under the same description.

The vault window's sync pill knows the difference between a snapshot and a
sync: a vault restored from the file reads "Loaded from cache - 3 h old" until
a sync actually succeeds in that session, and a failed sync keeps the age in
view rather than collapsing to a bare failure.

This also finishes the job the save-memory setting started. With both on,
turning the backend off no longer means the first operation after a restart
pays that eight-second cold start.

### The waiting screens can now open that copy

When the backend is slow to answer, the "Still syncing with Bitwarden" screen
offers **Open the local copy**; when it does not answer at all, the
"Couldn't reach Bitwarden" screen offers **Continue offline**. Both say how
old the copy is, in the same words the vault window's own pill uses -- "Your
copy on this machine - 3 h old" -- so you are told what you are opening before
you open it. Retry is still there and still comes first while it has attempts
left; once they are spent, continuing offline is what is left.

If you dismissed the Windows Hello prompt earlier in the session, the copy is
still offered: the file has not been touched, and the button asks for the
prompt again rather than pretending the copy is gone. That is the only place a
dismissed prompt is re-asked, and only because you pressed the button.
Opening the copy this way does not rewrite it, so its age stays honest.

Where there is genuinely nothing to open -- the setting is off, no copy has
been written yet, or one was refused and deleted -- **no button is drawn at
all**. Not a greyed one, and not a "coming soon".

### A login start with a copy on disk reaches the tray immediately

With the encrypted copy turned on, a sign-in start had the whole vault in
memory within milliseconds -- and then sat there for about eight seconds
anyway, waiting for the Bitwarden backend to finish starting before it put its
icon in the tray. For that stretch there was no tray icon to click, the
Ctrl+Alt+B shortcut was not claimed, and autofill matched nothing, on a launch
that had everything it needed to do all three.

It no longer waits. When a login start restores a usable copy, the tray icon,
the global shortcut and autofill come up straight away, filling from that
copy. The backend starts behind them, and when it answers, its vault replaces
the restored one: an item you edited on your phone shows its new name, an item
you deleted elsewhere disappears, and a vault you emptied empties here too.
The sync pill stops saying "Loaded from cache" at the same moment, because by
then it is not.

**Until the backend answers, Deskwarden can read but not write.** Editing,
deleting, favouriting and moving an item all need the backend, and they say so
rather than appearing to work -- there is no state in which a change looks
saved and quietly is not. If the backend never comes up at all, the tray's
**Sync** entry says so and offers a retry; everything that only reads keeps
working from the copy for as long as you leave the app running.

Restoring the copy this way does not rewrite it or reset its age, so a launch
that fetched nothing does not come up claiming a fresh vault.

This changes nothing for a launch with no copy to restore, and nothing for a
double-click, which still shows its window.

### A login start goes to the tray; a double-click shows the window

Deskwarden could not tell the two apart. The installer's autostart entry ran
`deskwarden.exe` with no arguments, so a start at sign-in and a double-click
arrived identically -- and both opened a full-size window, on every boot.

The autostart entry now passes `--autostart`, and a launch that carries it
stays in the tray: the vault loads in the background, autofill, the tray icon,
the global shortcut and background sync all come up as usual, and no window is
ever shown. You may not open one until your next reboot, which is the point.

If that background load fails, the window appears and says so -- the same
"Couldn't reach Bitwarden" screen a visible launch would have shown, with the
same Retry. A silent launch is silent only while it is working.

Installs made before this release keep their existing autostart entry until
they are reinstalled, and go on showing the window at sign-in exactly as they
do today. Nothing about them changes.

### Waiting is a sliding bar, not a spinning disc

Every screen shown before the vault list -- the launch's loading and slow
bodies, the standalone setup wait, the sign-in card while your password is with
the server, and the vault window's own first load -- drew a rotating disc.
Design turn 7 draws the same wait as a short bar sliding inside a track, and
that is what all of them draw now: one widget in the shared theme rather than
five hand-drawn copies, at the design's own proportions (a 32% knob, 3px tall,
one 1.4s eased cycle).
### The breach warning gets its own line

An item's detail pane ends in a strip of facts: when it was updated, how old
its password is, how many times it has been filled, how strong it is. When the
breach check is on and the password turns up on a public list, the warning —
"Found in a known data breach (1,644,583 times). Change this password." — was
appended to that same run and wrapped wherever the pane happened to run out of
width. In a reported screenshot it broke after "Found in a", so the most urgent
sentence on the pane was positioned by an accident of the column's width.

It is now painted on its own line beneath the strip, still in the palette's
red, while the facts above it stay grey. The neutral strip ends cleanly on
"Strength: …" — no separator left hanging at the end of the line.

Only the warning moved. "Breach check: checking…", "Not in any known breach"
and "Breach check unavailable" are reports on the check rather than warnings
about the password, and they still flow in the strip as another
`·`-separated segment; a pane with nothing to warn about is unchanged, down to
the pixel. The Password Health pane already gave the same sentence a line of
its own under each item's name, and was left alone.

## 0.8.5 - 2026-08-21

> The manual pre-release checklist was not run for this version either.
> This is the first release whose own updater can install the next one, so
> a failure in Preferences > Updates now costs every future release rather
> than one. The first window's three states and the forced single-instance
> takeover have also never run outside a test. If an update, a launch or a
> takeover misbehaves, please open an issue.

### The recovery no longer opens small windows of its own

When Deskwarden's Bitwarden backend would not come up, the launch fell back to a
recovery that put a 360×220 dialog on screen captioned "Setting up your
vault...", then a second small one captioned "Signed in — starting your
vault...", and then a modal error box whose OK button ended the app. Every
ordinary launch had already been merged into one full-size window; the recovery
was the one path left behind.

The recovery now runs in that same window: the vault window's own size and
position, from its first frame, with only the middle of it changing. It shows
what it is doing ("Loading your vault"), admits after three seconds that it is
slow and says how many seconds it has been, and — when the backend really
cannot be reached — says so in the window itself, with a **Retry** that runs a
real readiness probe. Three attempts in all; when they are spent the button is
removed rather than greyed out, and the window says what closing it means.
Close and minimise work throughout.

There is no "Open the local copy" and no "Continue offline", because there is no
offline copy to open yet. The window says what it can actually do.

The footer names the account being opened and says what the autofill shortcut
is really doing — including "Autofill starts when your vault opens · Ctrl+Alt+B"
before anything has tried to claim the shortcut, rather than claiming it is
already listening.

The free retry that closing the window buys now shows nothing at all, instead of
reopening a differently worded copy of the window you just closed.

### A card's network mark moved to the right-hand edge of its list row

`VISA`, `MASTERCARD` and the rest used to sit between the item's icon and its
name, which put a piece of secondary information in the middle of the run a
reader scans a list by. The mark is now at the trailing edge of the row, where
the `app` and `2FA` chips already sit, so every row's leading run is just the
icon and the name, and the marks line up in one column down the right-hand
side.

The mark is drawn exactly as it was — same size, same treatment, same support
for a supplied logo. Only its place on the row changed.

The room the item's name is truncated into is derived again from where the mark
now sits, rather than adjusted: it is measured from inside the row's trailing
run, after anything else there has taken its width. The old calculation
described a mark with the icon on one side and the name on the other, reserved
one gap too many, and could not see the trailing chips at all. A long card name
on a narrow pane still loses its tail and keeps its `(*1234)`.

### Preferences no longer says the fill shortcut works before anything has tried it

Preferences ▸ Shortcuts read a process-wide status whose starting value was
"registered". Deskwarden claims `CTRL+ALT+B` only after the startup vault
window closes — that window blocks for its whole life and hosts Preferences as
a modal drawn in its own loop — so anyone who opened Shortcuts from that first
window was shown a working shortcut before a single `RegisterHotKey` call had
been made. If the attempt then failed, which is the case the shortcut's
degrade-instead-of-crash handling exists for, the page had already said the
opposite.

The page now says so plainly on that route: the chord is greyed and the line
under it reads *"Deskwarden has not tried to claim CTRL+ALT+B yet…"*, followed
by the real answer as soon as the attempt is made. Nothing moved: registration
still happens where it has to, and an unavailable shortcut is still re-tried
every thirty seconds with the page updating when it succeeds.

The starting value was the whole defect — a default is not a fact — so the
status now begins as *nothing published at all*, which is how every other
published value in the app already worked. A new source-walking guard requires
that of all of them: no shared status may start life holding a fully formed
answer, so the next one written cannot repeat this.

### A vault snapshot can be restored without asking the backend for it

`VaultCache` gained `populate_with_vault`: a caller that already holds the
whole vault — items *and* folders — can write it into the cache with no HTTP
round-trip at all. Until now the closest door, `populate_with`, still fetched
the folders itself, so "seed this cache with what I already have" was
impossible to say. The encrypted disk cache needs exactly this to restore a
snapshot from disk at startup, and the test suite needed it sooner: every
fixture that wanted a populated cache was standing up a local HTTP server for
one folder request whose answer it already knew.

Nothing about the cache's rules changed. There is still exactly one place a
snapshot is written back, the era guard and the replay of newer local writes
still run there, and `clear` is still the only thing that begins an era — the
fetching entry points are now thin wrappers over that one place rather than
being it.

The visible effect is a test suite that no longer flakes on the network in
places that never had anything to do with the network. Several fixtures now
point the cache at an address that is dead for the life of the process, which
turns "this code path reads the in-memory snapshot and does not call the
backend" from a comment into something that fails loudly when it stops being
true.

### Updates has a Preferences page of its own, and About is simple again

Everything about updating was split across two pages: the switch that decides
whether Deskwarden looks for a release by itself sat at the foot of
**General**, while the *Check for updates* button, the release notes, the
download, the progress bar and *Restart to install* were all a card at the
bottom of **About**. Neither page told you the whole story, and About — the
page that says which build you are running — had grown into the page that
installs a different one.

- **A new Updates page** carries both halves: the automatic-check switch, and
  directly below it the check, the notes, the download and the restart. This
  is the arrangement the Breaches page already uses for the same reason. The
  button deliberately works whether or not the switch is on, because pressing
  it is you asking; that sentence only reads as honest with the switch in the
  same glance, which it now is.
- **The switch is called "Check for updates automatically"**, not "Check for
  updates". The button one card below says exactly that, and two identical
  labels — one a switch, one a control the switch does not govern — would have
  erased the distinction the page exists to make.
- **About is two facts and nothing that acts**: which build this is, and which
  Bitwarden account the vault comes from. The version stays there, because
  that is what an About page is for; nothing that *does* something about the
  version does.
- Nothing about how updating works changed. A download you start keeps running
  if you click to another section, and reports where it actually got to when
  you come back.

### Fixed: Preferences said this build could not scan or check for updates

Opening **Preferences** from inside the vault window showed *"This build
cannot scan: nothing set the scan up when Deskwarden started"* on the Breaches
page and *"This build cannot check for updates. Please report it — it is a
defect, not a setting"* on Updates (the About page, before the entry above
moved it). Both buttons did nothing.

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

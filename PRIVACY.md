# Privacy Policy

**Last updated:** 2026-08-19

Deskwarden is a Windows companion application for Bitwarden-compatible
vaults. It is unofficial and unaffiliated with Bitwarden, Inc.

## The short version

**Deskwarden has no servers, no accounts, and no analytics.** Nothing is sent
to the developer — not usage data, not crash reports, not telemetry of any
kind. There is no mechanism in the software to do so.

Your vault credentials are handled by the Bitwarden CLI (`bw`), which you
install and sign in to yourself. Deskwarden talks to it locally.

## What stays on your machine

- **Vault contents.** Usernames, passwords, one-time-code seeds, cards and
  notes are read from a local `bw serve` process over `localhost` and held in
  memory only. Secrets are wiped from memory when no longer needed.
- **Your settings**, including which applications you have matched to which
  vault items, in `%APPDATA%\Deskwarden`.
- **Cached site icons**, stored as image files in the same folder.
- **Card network logo files, if you supply any.** With *Show card network
  logos* switched on (it is off unless you turn it on), Deskwarden reads PNG
  files from `%APPDATA%\Deskwarden\brand-marks` and from a `brand-marks`
  folder beside the program itself. It only ever reads files named after a
  card network — `visa.png`, `mastercard.png` and so on — it never writes to
  either folder, and nothing about them leaves your PC. No logo is downloaded
  and none is included with Deskwarden: these are files you put there. With
  the setting off, neither folder is opened at all.

## What Deskwarden looks at, outside its own windows

To offer to fill a password, Deskwarden has to notice that you are being
asked for one. Two things are read from whichever window you are working in,
both through Windows' own accessibility interface (UI Automation) — the same
interface a screen reader uses.

- **The application's identity** — its executable path and, where the file
  carries one, its description, so that "chrome.exe" can be shown to you as
  "Google Chrome". This is what a match is keyed on, and it is the only part
  stored: the applications you have matched sit in `%APPDATA%\Deskwarden`.
- **Whether the window contains a masked password box.** One question, asked
  when the focused window changes, answered yes or no, and not written down
  anywhere.

**What that second question does not do is worth stating plainly, because
"reads your password field" is the reasonable thing to assume and it is not
what happens.** It asks Windows whether any text box in the window has its
*password* property set — the property that draws dots instead of letters.
It does not read the box's contents, and it could not: masking those
contents from other programs is that property's entire purpose. Nothing you
type into another application is read, logged, or sent anywhere.

The answer is used to decide one thing: whether to show you a card saying
there is no saved login for this application. Without it, an application
with nothing saved is met with silence, which is indistinguishable from
Deskwarden being broken.

## What leaves your machine, and to whom

Deskwarden makes network requests to exactly four destinations. Three are
third parties; one is your own vault.

### 1. Your Bitwarden server, via the Bitwarden CLI

Over `localhost` to `bw serve`, which you started. `bw` then talks to
whichever Bitwarden server you signed in to — theirs, or your own if you
self-host. Deskwarden never contacts your vault server directly and never
sees your master password after handing it to `bw`.

### 2. Site icons — `icons.bitwarden.net`

To show a recognisable icon beside a vault entry, Deskwarden requests that
site's icon by **domain name**. If you self-host, it uses your server's icon
service instead.

**This is the request with the most privacy weight, so it is worth stating
plainly:** it discloses to whoever runs that service that someone looked up
that domain. Over time that is a partial picture of which sites you hold
entries for. It does not disclose your username, your password, or which
account the entry belongs to. Icons are cached on disk so a given domain is
normally requested once.

**You can switch it off.** Preferences → General → *Show site icons*. On by
default, because the icon service belongs to the same server your vault is
already on — your own machine if you self-host — rather than to a third
party that would otherwise learn nothing about you. With it off, no domain
is sent and none is even worked out; items show coloured initials instead.

### 3. Breach checking — `api.pwnedpasswords.com`

When you ask Deskwarden to check whether a password appears in a known
breach, it uses Have I Been Pwned's range API. **Your password is never
sent.** It is hashed locally with SHA-1 and only the **first five characters
of that hash** are transmitted; the service returns every matching suffix and
the comparison happens on your machine. This is the standard k-anonymity
scheme and it is why the check cannot identify which password you asked
about.

### 4. Update checks — `api.github.com`

Deskwarden asks GitHub for the latest release of its own public repository.
This request carries no information about you or your vault beyond what any
HTTP request necessarily reveals to the server (your IP address, and the fact
that a request was made).

There are **two** ways this request is made, and the setting governs one of
them.

**The automatic check** runs once at startup and once a day thereafter. This
is what Preferences → General → *Check for updates* switches off. It is on by
default, because an app that quietly stops telling you a security fix exists
is a worse outcome than the IP address the check discloses — and one you
would have no way to notice.

**The manual check** is the *Check for updates* button on Preferences →
About. **It works whether or not the automatic check is switched off**, and
it makes the same request to the same address. Switching the setting off asks
Deskwarden not to contact GitHub on its own; pressing this button is you
asking it to contact GitHub now. The About page says so, next to the button,
so this is not a behaviour you have to come here to discover.

**Neither check downloads anything.** A check reads the latest release's
version number and its release notes and stops there. The installer is
fetched only after you press *Download* on that page, and it is verified
against Deskwarden's signing certificate before anything can launch it.
Release notes come from the GitHub release and are shown as **plain text**:
they are never interpreted as markup, and nothing in them becomes a link that
can be clicked.

An earlier version of this policy said that builds distributed through the
Microsoft Store do not perform this check. **There is no Microsoft Store
build of Deskwarden**, so that sentence described an intention rather than
the software, and it has been removed. Should one ever ship, this section
will say what it actually does before it does it.

## What Deskwarden does not do

- No telemetry, analytics, crash reporting, or usage statistics.
- No advertising, and no data sold, shared, or brokered — there is no data to
  sell.
- **The autofill path never uses the clipboard.** Credentials are typed
  directly into the target window, deliberately, because the clipboard is
  readable by every other process on the machine.

  The clipboard *is* used when **you explicitly copy something** — the copy
  buttons and their shortcuts for username, password, website, card number,
  security code, SSH key and one-time code, and the **Copy link** button on
  a Send. That is the point of those commands. Two things are done about what
  happens next, and one thing is not.

  **The Send link is treated as a secret, and this is worth saying because
  it does not look like one.** A Send's access link carries that Send's
  decryption key in the part after the `#`, so anyone holding the link can
  open the Send — the link *is* the credential. It is meant to be handed to
  somebody else, but handing it to somebody else is not a reason to leave a
  copy of it in your own clipboard history or on your other devices, so it
  goes through exactly the same two things below as a password does. Note
  that this covers the copy on *your* machine only: once you have sent the
  link on, where it goes is between you and whoever you sent it to.

  **A copied secret is kept out of clipboard history and off your other
  devices.** Windows normally keeps recent clipboard entries behind `Win+V`,
  and, if you have clipboard sync switched on, copies them to your other
  signed-in devices. Deskwarden asks Windows not to do either, using the
  formats Windows provides for the purpose, at the moment the value is
  copied. This is not a setting: there is no version of "keep my passwords in
  `Win+V`" worth offering.

  **A copied secret is taken back off the clipboard, and this part you
  control.** Out of the box it is cleared one minute after you copy it, and
  immediately when you lock the vault (by hand, from the tray, after idling,
  or because the session expired), when you switch, add or remove an account,
  and when you quit Deskwarden from the tray or it shuts down to install an
  update.

  All four are settings — Preferences → *Clipboard*. One switch turns the
  whole thing off; three more govern the vault-locks, account-changes and
  quits cases individually; and the interval can be set to anything from half
  a minute to an hour. They all ship **on**, at one minute. Turning any of
  them off is a real reduction in what Deskwarden does for you, and it is
  offered because the cost of getting the trade wrong falls on your own
  paste, not on anyone else.

  Whatever those settings say, each clear checks first that what is on the
  clipboard is still what Deskwarden put there — if you have copied anything
  else since, from any program, Deskwarden leaves it alone. That check is not
  a setting and cannot be turned off. Closing the vault window on its own
  does *not* clear it, because closing it in order to paste somewhere is the
  ordinary thing to do; the timer still applies.

  **What is not fixed:** for as long as the value is on the clipboard, any
  other program on your machine can read it. That is a property of Windows
  rather than of Deskwarden, and it is why the autofill path types instead.
  Clearing on quit also only covers a quit that actually happens — if
  Deskwarden is killed or the machine loses power while a secret is on the
  clipboard, it stays there, and no setting can change that.
- No secret is written to a log file. Types that carry key material have
  hand-written debug output that refuses to print it.

## Children

Deskwarden is a password-management utility and is not directed at children.
It collects no personal information from anyone, of any age.

## Your choices, and the ones you do not currently have

- **Breach checking is off by default** and only runs if you turn it on. It
  is the one network feature that is opt-in, because it is keyed on your
  passwords and making that call on your behalf is not the developer's
  decision to make.
- **Site icons are on by default and can be turned off** — Preferences →
  General → *Show site icons*. On rather than off because the icon service
  belongs to the server your vault is already on, so the request re-uses a
  relationship you have already chosen instead of creating a new one with a
  third party. That is a weaker reason to ask first than breach checking's,
  not no reason, which is why the switch is there.
- **The update check is on by default and can be turned off** —
  Preferences → General → *Check for updates*. On rather than off for the
  reason given above: the disclosure is an IP address, and the cost of the
  other default is a user who is never told about a security fix and has no
  symptom to notice.
- **Clearing a copied secret is on by default and every part of it can be
  changed** — Preferences → *Clipboard*. A master switch, one switch each for
  clearing when the vault locks, when the account changes and when Deskwarden
  quits, and the interval, which accepts a decimal number of minutes anywhere
  from `0.5` (thirty seconds) to `60`. Thirty seconds is the shortest on
  purpose: below it the clipboard tends to expire before a slow sign-in page
  is ready, which teaches you to copy twice and is worse than not clearing at
  all. There is no "never" in the interval — that is what the master switch
  is for, and a setting that means "off" should say so.
- **Clipboard history exclusion is not a choice, deliberately.** Keeping a
  copied password out of `Win+V` and off your other devices has no setting
  and cannot be turned off, because there is no reason anyone would want the
  other behaviour. It is unaffected by every switch in the paragraph above:
  those govern whether the value is taken back *afterwards*, not whether
  Windows was allowed to keep a copy of it in the first place. It is listed
  here so that "everything is switchable" is not read as a promise this
  section does not make.
- **Everything else is local**, and deleting `%APPDATA%\Deskwarden` removes
  it.

**Every network request this application makes on its own is something you
can switch off**, and each of the three has its own row on Preferences →
General. That is a statement about network requests specifically, not about
every behaviour: some of what is described above is optional and some is not,
and the clipboard section is both — when a copied secret is taken back is
yours to set, while keeping it out of `Win+V` in the first place is not.
The vault server is the exception, and only in the sense that
Deskwarden never contacts it: that is `bw`, which you installed and signed
in to.

## Changes

Material changes to this policy will be noted in the release notes of the
version that introduces them, and this file's "last updated" date will
change. The policy lives in the repository, so its full history is public.

## Contact

**denis@napps.pw** — or open an issue at
<https://github.com/denis-platonov/deskwarden/issues>.

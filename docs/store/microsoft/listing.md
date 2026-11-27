# Microsoft Store: listing kit

Copy-paste text for Partner Center -> *Store listings* -> English (United
States). Field limits are the ones Partner Center enforced when this was
written (2026-11-19); the form shows the live limits, and wins.

`<<...>>` marks what only the owner can fill. Nothing here contains a
credential, a real account or a private URL.

## Product name

**Deskwarden**

Reserve it first (CHECKLIST step M3); the reservation holds the name for three
months. Risk, stated plainly: the `-warden` ending sits close to Bitwarden's
trademark, and the name is the one thing in this kit that could draw a
trademark complaint rather than a policy one. The repository already says
"Unofficial and unaffiliated with Bitwarden" in the same breath as the name
(`README.md`, `PRIVACY.md`, the installer's `AppPublisher`), and the listing
below does the same. If the name reservation or a later complaint fails,
the fallback is renaming the **Store listing only** (the in-app name stays);
decide before submitting whether you would rather pick a plainer name now.

Short title (optional, shown on tiles): `Deskwarden`

## Short description (one sentence, for tiles and search results)

Fill sign-in boxes in desktop apps and games from your Bitwarden-compatible
vault. Unofficial; not made by Bitwarden.

## Description (up to 10,000 characters)

```
Deskwarden fills your saved logins into the Windows programs a browser
extension cannot reach: desktop apps, game launchers, installers, remote
tools. Press a hotkey, or let it prompt you when a program asks for a
sign-in, and it types the username and password for you.

Deskwarden works WITH your Bitwarden-compatible vault. It is an independent,
community project. It is not made by, endorsed by, supported by or
connected to Bitwarden, Inc. "Bitwarden" is a trademark of Bitwarden, Inc.,
used here only to say which vaults this app works with.

WHAT IT DOES
- Native-app autofill: match a program once and choose how it fills - on
  focus, from a small prompt, or with a global hotkey. The choice is saved
  with the vault item, so it syncs with the rest of your vault.
- Typed, not pasted: credentials are entered as keystrokes, and the autofill
  path never touches the clipboard.
- A picker when more than one login could match, and a prompt to save a new
  login when you sign in to something your vault does not know yet.
- A full vault window from the system tray: folders, favourites, search,
  logins, cards, identities, notes and SSH keys; create, edit, delete and
  restore; live one-time codes with a countdown.
- Add a one-time code by scanning its QR code off your own screen, from a
  picture file, or with your webcam.
- Password generator and passphrases that run entirely on your PC; strength
  on every item; optional breach checking that never sends your password
  (only the first five characters of its hash).
- Bitwarden Send, per-item vault-password re-prompt, import and export.
- Copied passwords are kept out of Windows clipboard history and cleared
  after a minute (both adjustable).
- Windows Hello quick unlock.

PRIVACY
No account, no analytics, no telemetry, no servers of ours. Nothing about
you or your vault is sent to the developer. The only network connections are
to your own vault server and, if you leave them on, Bitwarden's icon
service; every one can be switched off. Read the full policy at <<PRIVACY URL>>.

WHAT YOU NEED
- A Bitwarden-compatible account: a self-hosted server (Vaultwarden,
  NodeWarden, Bitwarden self-hosted) works directly. For accounts on
  bitwarden.com or bitwarden.eu, Deskwarden uses Bitwarden's own command-line
  program, which you install yourself from bitwarden.com/help/cli/ (the
  Microsoft Store edition never downloads programs).
- Windows 10 version 1809 or newer, 64-bit.

WHAT IT NEEDS PERMISSION FOR, AND WHY
Deskwarden reads the window you are working in through Windows' own
accessibility interface (UI Automation, the same one screen readers use) to
notice a sign-in box and the program's name, and types into it with
Windows' SendInput. It does not read what you type. It can start when you
sign in to Windows (off until you turn it on), and uses the network only as
described above. The webcam is used only on the one screen where you choose
to scan a QR code with it.

Open source (AGPL-3.0): <<SOURCE URL>>.
```

## Product features (up to 20 bullets, 200 characters each)

1. Autofill logins into native Windows apps and game launchers, by hotkey, prompt or on focus.
2. Works with Bitwarden-compatible vaults: self-hosted servers directly, bitwarden.com with Bitwarden's own CLI.
3. Typed, not pasted: the autofill path never uses the clipboard.
4. Full vault window from the tray: folders, search, logins, cards, identities, notes and SSH keys.
5. Live one-time codes; add one by scanning a QR code on screen, from a file, or with a webcam.
6. Password and passphrase generator that runs entirely on your PC.
7. Optional breach check: your password never leaves the PC, only five characters of its hash.
8. Copied secrets stay out of clipboard history and clear themselves.
9. Windows Hello quick unlock; automatic lock when you step away.
10. No account, no telemetry, no analytics. Open source (AGPL-3.0).

## Search terms (up to 7, each up to 30 characters)

`password manager`, `autofill`, `vault`, `passwords`, `one-time code`, `game launcher login`, `self-hosted vault`

(Avoid `Bitwarden` as a bare keyword: keyword-stuffing another company's
trademark is a policy risk. The description says "works with Bitwarden-
compatible vaults" in running text, which is nominative and accurate.)

## What's new in this version (up to 1,500 characters)

For 1.0.0:

```
Deskwarden 1.0: a redesigned vault window, a new sign-in experience, and
everything from the 0.x releases in one place. This Microsoft Store edition
is updated by the Store and never downloads programs itself.
```

For later versions, write from `CHANGELOG.md`, shortened; the release body
composer in `.github/workflows/release.yml` already produces a
Store-length summary of the section.

## URLs and contact (owner fills)

| Field | Value |
|---|---|
| Website | `https://github.com/denis-platonov/deskwarden` (exists) or `<<project website>>` |
| Support contact | `<<support URL or email>>`: the README says issues go to `https://github.com/denis-platonov/deskwarden/issues`; `denis@napps.pw` is in `PRIVACY.md`. **Use an address you are willing to publish.** |
| Privacy policy URL | `<<PRIVACY URL>>`: must be a public page that renders without signing in. `PRIVACY.md` on GitHub (`https://github.com/denis-platonov/deskwarden/blob/main/PRIVACY.md`) qualifies; a page on your own site is better (see privacy-review.md) |
| Copyright and trademark info | `(c) <<year>> <<owner name>>. Deskwarden is licensed AGPL-3.0-or-later. "Bitwarden" is a trademark of Bitwarden, Inc.; Deskwarden is not affiliated with, endorsed or sponsored by Bitwarden, Inc.` |
| Additional license terms (optional) | Leave empty to use the Standard Application License Terms, or link `LICENSE` (AGPL-3.0) |

## Store logos and screenshots

Logos: `store-package\generated-assets\listing\` after running the build
script: `store-logo-1x1-300.png` (required), `store-logo-1x1-1080.png`,
`store-logo-2x3-720x1080.png`. Screenshots: [`screenshots.md`](screenshots.md).

## Trademark and affiliation: wording check

Checked against the repository on 2026-11-19; re-check at submission.

- **Rule.** Deskwarden must never read as made, approved or supported by
  Bitwarden. Allowed: "works with Bitwarden-compatible vaults",
  "for Bitwarden vaults", "unofficial", "not affiliated with Bitwarden,
  Inc.". Not allowed: "the Bitwarden desktop app", "Bitwarden for Windows",
  "official", "by Bitwarden", the Bitwarden name or shield in the title,
  the icon, tiles, screenshots or the publisher display name.
- **README / PRIVACY.** Both open with a prominent unaffiliated notice and
  call the project "unofficial": good; the listing copies that language.
  `README.md` compares itself with Bitwarden in a table: fine for a repo,
  but **the listing carries none of that comparison** (comparative claims
  invite review).
- **Icon.** The mark is the app's own quartered shield; it is not
  Bitwarden's. A shield is a generic password-manager symbol; the quartered
  blue construction is the project's. No Bitwarden logo is used in the app,
  the package assets or the listing.
- **Screenshots.** The `ui_preview` surfaces show the app's own UI. Any
  screenshot that shows a Bitwarden-hosted web page, the Bitwarden CLI's
  window, or the Bitwarden logo must not be used; `login_cli_setup` and
  `prefs_vault_official_cli` mention "Bitwarden CLI" as a program name in
  text, which is nominative use: acceptable, but they are not in the
  recommended set.
- **Publisher display name.** Your own name or studio; never one containing
  "Bitwarden".
- **Open point.** The product name itself (above). Reserve it, submit with
  the unaffiliated sentence in the first paragraph of the description, and
  keep the rename fallback in mind.

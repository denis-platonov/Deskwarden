# Privacy Policy

**Last updated:** 2026-08-17

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
that a request was made). Builds distributed through the Microsoft Store do
not perform this check, because the Store handles updates.

## What Deskwarden does not do

- No telemetry, analytics, crash reporting, or usage statistics.
- No advertising, and no data sold, shared, or brokered — there is no data to
  sell.
- **The autofill path never uses the clipboard.** Credentials are typed
  directly into the target window, deliberately, because the clipboard is
  readable by every other process on the machine.

  The clipboard *is* used when **you explicitly copy something** — the copy
  buttons and their shortcuts for username, password, website and one-time
  code. That is the point of those commands, but it is worth knowing that
  anything copied this way is readable by other software on your machine
  until it is overwritten, and that this is a property of Windows rather than
  of Deskwarden.
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
- **Everything else is local**, and deleting `%APPDATA%\Deskwarden` removes
  it.

**Two things you cannot currently switch off, stated plainly rather than
omitted:**

- **Icon fetching has no setting.** If a vault entry has a website, its
  domain is sent to the icon service described above. There is no toggle for
  this today. Given it is the request with the most privacy weight, it should
  have one, and that is a known gap rather than a deliberate choice.
- **The update check has no setting.** It runs against GitHub's API on its
  own schedule. Microsoft Store builds do not perform it at all, because the
  Store handles updates.

Both are tracked as work to be done. This section will be corrected when they
are.

## Changes

Material changes to this policy will be noted in the release notes of the
version that introduces them, and this file's "last updated" date will
change. The policy lives in the repository, so its full history is public.

## Contact

**denis@napps.pw** — or open an issue at
<https://github.com/denis-platonov/deskwarden/issues>.

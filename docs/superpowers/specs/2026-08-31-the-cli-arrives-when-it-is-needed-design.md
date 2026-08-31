# The CLI Arrives When It Is Needed

**Nobody is sent away to install anything by hand. On the one backend that still
needs the Bitwarden CLI, Deskwarden says plainly that it is required, fetches
it, proves it is Bitwarden's, and installs it — at the moment the user chooses
that backend, and never before.**

## Why

The owner:

> "fix the installer - so it doesn't even ask about the bw cli but when user
> attempts to connect to oficial servers - it will check and install it"

refining, not replacing, the standing rule:

> "for bitwarden servers - it is mandatory, for self-hosted - no"

under the general one:

> "users should not know about how it works underhood"

The installer today spends a wizard page, a PowerShell script, a PATH write and
four error dialogs on a dependency that most of the app no longer has. Since
0.13.0 a direct-REST account reaches its vault, its Sends, and its own identity
without spawning anything; the sign-in status probe and vault export are the
last two, and both are being removed on their own branches. What is left is one
sentence: **`bw serve` is still the vault for accounts on `bitwarden.com` and
`bitwarden.eu`**, because Deskwarden's own client is deliberately not pointed at
Bitwarden's production servers (`backend_policy::is_self_hosted`, and the
owner's rule above).

So the dependency has stopped being a property of *the product* and become a
property of *one choice inside it*. An installer that asks about it is asking
every user a question that concerns some of them, before they have made the
choice that would make it concern them at all.

## The two pieces, and why they are two

This is two changes and they are not the same size:

* **Acquisition** (this document): the app learns to fetch and verify the CLI
  on demand. New network code, new disk writes, a new trust boundary.
* **The installer forgets it**
  (`2026-08-31-the-installer-forgets-the-cli-design.md`): deletion, entirely.

They are separable to *write* and **not separable to ship, in that order**.
`backend_policy::choose(None, true)` is `BwServe` — a fresh install has no
account, so `server_url` is `None`, which is `bitwarden.com` by definition — and
`main`'s gate makes a missing `bw.exe` a `fatal_startup_error` on exactly that
arm. **Landing the installer deletion first bricks every new install at first
launch.** Acquisition lands first, or the two land together. This is stated
here because it is the kind of ordering that reads as a nicety and is not one.

## Where a user meets it

`login_ui.rs`'s sign-in window already has all three of the moments this needs.

**The dropdown states the requirement and downloads nothing.**
`ServerChoice::{UsCloud, EuCloud, SelfHosted}` is a `selectable_value` combo at
the bottom of the card (`login_ui.rs:1832`). Selecting `UsCloud` or `EuCloud`,
on a machine with no Bitwarden-signed `bw.exe`, puts a line in the card:

> **bitwarden.com requires the Bitwarden CLI.** Deskwarden will download and
> install it from Bitwarden when you continue (about 37 MB, once).

Selecting `SelfHosted` removes it. **The notice appears before the download and
not as an explanation after it** — this is the moment the user still has a
choice, and it is where the cost of the choice belongs. Browsing the dropdown
still makes no request; a user comparing three options must not trigger two
downloads.

**Continue is the moment.** The `Some(LoginAction::Submit)` arm
(`login_ui.rs:2968`) already computes `server_configured` from `form.server_choice`
*before* `spawn_auth`, and that block is already the first line in the app that
requires `bw.exe` on an official account — `configure_server_in` (`login_ui.rs:3008`,
its sole call site) shells out to `bw config server` through `bw_command_in` and
hard-fails without a verified binary. **Acquisition goes between the blank-field
check and that call — `login_ui.rs:2985/2986` — and nowhere else.** Placing it
after `configure_server_in` would mean the first thing needing the CLI runs
before the CLI is there; placing it on the dropdown means selecting an option
starts a download. Nothing about the window's shape changes.

**A missing `bw.exe` is already survivable in this window.** `build_login_frame`
opens with `check_bw_status_details_in` (`login_ui.rs:2720`), a `bw status`
spawn whose failure falls back to `unknown_status_details()` — `Unauthenticated`,
no email, no server. So a fresh install with no CLI reaches a drawn, usable
sign-in card today; it is only Submit that cannot complete. That is exactly the
shape acquisition needs, and it is why the insertion point is at Submit rather
than at window open.

**The window already knows how to be busy.** `auth_in_progress` greys the whole
credential zone, paints `theme::disabled_password_field` over a buffer that is
still there, disables Continue and Enter and the Hello panel, and drives a
spinner. Acquisition reuses that state rather than inventing a second one.

## Nothing here is silent, and why that is not a contradiction

The owner, ruling on exactly this tension:

> "yes, no silent - we say that it is requared period"

So: **the app names the Bitwarden CLI, says it is required for the server the
user just chose, and says it is downloading and installing it.** Not a bare
spinner, not "Setting up…", not a euphemism, and not an explanation offered
afterwards.

This sits beside the owner's other standing rule —

> "users should not know about how it works underhood"

— and a future reader will be tempted to cite that rule to "fix" this
disclosure away. **They do not conflict, and the distinction is worth stating
precisely, because getting it wrong in either direction is a real defect.**

The earlier rule bans exposing **internal machinery the user has no decision to
make about**: which process draws a window, when the disk cache is written,
whether the vault came from `bw serve` or a direct REST call, that a `bw status`
spawn failed and fell back. Telling a user any of that hands them a fact they
cannot act on and a vocabulary they did not ask to learn. That is the leaky
kind of disclosure, and the Preferences split
(`2026-08-30-preferences-per-backend-design.md`) exists because of it.

**This is not that.** A third-party program is being downloaded from the
internet and installed on the user's machine, and it is a hard requirement of
the server they just selected — not a preference, not an implementation detail,
and not something Deskwarden gets to decide quietly on their behalf. A user is
entitled to know what is being put on their computer and why. That is the
honest kind of disclosure.

The test that separates them, and the one to apply to any future sentence here:

> **Does the user have a decision to make about this fact?** If yes, say it
> plainly and name the thing. If no, it is machinery and it stays inside.

Choosing `bitwarden.com` over a self-hosted server *is* a decision, the CLI
requirement *is* a consequence of it, and 38,695,474 bytes (measured — see
Authentication below) over somebody's tethered connection *is* a cost they are
about to pay. All three are theirs to know.

### What that means concretely

* **The requirement is stated on selection, before anything is fetched.**
  Choosing an official server in the dropdown puts the notice in the card while
  the user can still change their mind. Stating it only once the download is
  running would make it a retroactive justification rather than a disclosure.
* **The progress line names what it is doing** — "Downloading the Bitwarden CLI
  from Bitwarden…" over a determinate bar. `updater::download_and_verify`
  already reports `(done, total)` per chunk through `on_progress` and
  `update_panel::download_fraction` already turns that pair into a fraction;
  both are reused, neither is rewritten. A spinner would hide the size, which
  is the part that costs the user something.
* **Verification is named too**, because it is the reassuring half: "Checking
  it is signed by Bitwarden…". A user watching an unknown binary arrive is
  owed the fact that it is being checked.
* **It happens once, ever**, and only on official servers.
* **Every failure says plainly that this server cannot be used without the
  CLI.** No vague trouble. See the failure matrix below — every row names the
  thing.

### Declining

There is still no modal consent prompt, but the reasoning has changed and so has
the outcome. The notice is the disclosure; the dropdown is the control. A user
who does not want this **declines by choosing Self-hosted or by closing the
window**, and both are available with the notice on screen and nothing yet
downloaded.

What the ruling settles is what they are told when they do, or when the machine
cannot: **not a vague failure.** A user who is offline, or whose download fails,
or who closes the window mid-install, is told that `bitwarden.com` cannot be
used without the Bitwarden CLI, and that a self-hosted server can be used
without it. That second half is the actionable part, and it is the sentence a
euphemism would have swallowed.
## Authentication — the whole of it

This is the highest-risk thing the app would do, so what follows is what was
established from primary sources on 2026-08-31, not what is assumed.

### Where the artefact comes from

`https://api.github.com/repos/bitwarden/clients/releases` — the same monorepo
`installer/bootstrap-bw.ps1` already resolves against, with the same problem:
releases for `cli`, `desktop`, `browser` and `web` interleave by date, so the
repo's generic "latest" is not the CLI's. Filtering on the `cli-v*` tag prefix,
excluding prereleases and drafts, is still required.

Confirmed live: the newest such release is **`cli-v2026.8.0`**, published
`2026-08-20T16:25:21Z`, `prerelease: false`, `draft: false`, carrying 13 assets
including **`bw-windows-2026.8.0.zip`** (38,695,474 bytes) — and, beside it,
`bw-oss-windows-2026.8.0.zip`. **A glob of `bw*windows*.zip` matches both**, and
the OSS build lacks paid-tier features. The prefix must be anchored.

Independently confirmed: **`https://bitwarden.com/download/?app=cli&platform=windows`**
— Bitwarden's own documented download link, quoted from `bitwarden.com/help/cli/`
— answers `302` to
`https://github.com/bitwarden/clients/releases/download/cli-v2026.8.0/bw-windows-2026.8.0.zip`.
Bitwarden's own redirector and the tag-prefix filter agree on the same file, so
the filter is not this project's guess about what "current" means.

### What Bitwarden publishes to authenticate it

**No detached signature. No PGP. No `SHA256SUMS` asset.** All 13 assets of
`cli-v2026.8.0` were enumerated; none is a checksum or signature file.
`bitwarden.com/help/cli/` says only:

> "The Bitwarden Password Manager CLI build pipeline creates SHA-256 checksum
> files that are available on GitHub."

and the security FAQ it points at resolves that to comparing against "the
`sha:...` value listed immediately adjacent to the executable or package you
downloaded from GitHub" — which is **GitHub's own per-asset `digest` field**, not
a Bitwarden-published file. For `bw-windows-2026.8.0.zip` that field reads
`sha256:26a6bb9a88ca9eeaad9e59db1816dcceb3ce6cc80a30b33e1324b0642f4a0f32`.

So there are exactly two checks available, and they are not equal:

1. **Integrity, from the transport.** The API's `digest` arrives over TLS from
   `api.github.com`; the file arrives over TLS from GitHub's asset CDN. Same
   trust root. This catches a truncated, corrupted or partially-written
   download. **It is not an independent authenticity proof and this design does
   not claim it is** — anyone who could substitute the asset could substitute
   the digest.
2. **Authenticity, from Bitwarden.** The Authenticode signature on the `bw.exe`
   *inside* the zip. This is issued by a public CA that had to validate the
   organization name, it does not depend on how the file reached the machine,
   and it is the load-bearing check. This repo measured it on 2026-08-10 and
   recorded the measurement in two places
   (`main::TRUSTED_BW_SIGNER_ORGANIZATIONS`, `bootstrap-bw.ps1`): `Status:
   Valid`, `O=Bitwarden Inc.`, issuer `CN=DigiCert Trusted G4 Code Signing
   RSA4096 SHA384 2021 CA1`, thumbprint
   `80375A0C9630A51ECB7EC79B37A8174C8DACCCED`, `NotAfter 2027-07-30`. That
   measurement was not re-taken here; what was re-taken is the artefact naming
   and the absence of any published checksum or signature file, both of which
   hold.

**The verdict on the trust question: yes, there is an authentication story this
repo's existing machinery can enforce, and it is the one the repo is already
enforcing twice.** Not one check but two, in the only order that is safe:
digest first (cheap, catches the boring failure), Authenticode second, and the
binary is **never executed** — not even to read `bw --version` — at any point
before or after. Verify, then install; never run to test.

The organization pin is not tightened to a thumbprint. `bootstrap-bw.ps1` already
records why, and it survives the move: a thumbprint pin is right for a binary
verifying its own future builds (which is what `updater.rs` does for
Deskwarden's own releases) and wrong for a third party whose certificate may
rotate without warning — and this one expires 2027-07-30.

## What is reused, and what is new

Nothing about downloading or trusting is invented. The pieces already exist and
this design's job is mostly to point them at a second repository.

| Need | Existing machinery | File |
| --- | --- | --- |
| Bounded HTTP, no bare `ureq` | `http_agent::bounded_total` / `bounded_stall`, newtypes with private inners, guarded by `bare_ureq_calls_are_confined_to_this_module` | `http_agent.rs` |
| Read GitHub's per-asset `sha256:` digest | `updater::parse_asset_digest`, `ASSET_DIGEST_PREFIX`, `Sha256Digest` | `updater.rs:78,99,108` |
| Hash a file on disk and compare | `updater::file_sha256`, and the discard-on-mismatch shape of `discard_rejected_installer` | `updater.rs:1061,1143` |
| Report `(done, total)` while streaming | `updater::copy_reporting`, `NO_PROGRESS` | `updater.rs:1168,1182` |
| Turn that pair into a bar | `update_panel::download_fraction`, `download_label` | `update_panel.rs:460,473` |
| Authenticode, in-process, no PowerShell | `signature::verify_authenticode` (`WinVerifyTrust` + `CryptQueryObject`) | `signature.rs:56` |
| "Is this signer Bitwarden?" | `signature::is_trusted_organization`, `dn_component`, against `main::TRUSTED_BW_SIGNER_ORGANIZATIONS` | `signature.rs:348,296` |
| Where the file must land | `bw_path`'s own `install_bin_candidate` — `<InstallDir>\bin\bw.exe` | `bw_path.rs:333` |
| The fn-pointer seam idiom | `UpdaterEnv` (`hash`/`launch`), `SecondFactorSeam`, `production()` + `fn_addr_eq` guard | `updater.rs:1347`, `login_ui.rs:2238` |

What is genuinely new is one module and one insertion point.

**A note that matters more than it looks.** `signature.rs` no longer shells out
to PowerShell — `verification_needs_no_external_process` pins that it uses
`WinVerifyTrust` directly, because `Get-AuthenticodeSignature` fails wherever
`Microsoft.PowerShell.Security` cannot autoload. `bootstrap-bw.ps1` still uses
the PowerShell one, and hand-maintains a *second* DN parser
(`Get-CertificateDnComponent`) whose own doc says it is "kept in sync by hand
with `dn_component` in `src/signature.rs`". Moving acquisition into the app does
not merely relocate the check — **it collapses two verification mechanisms and
two hand-synchronised DN parsers into one.** That is an independent reason to
do this, beyond anything the owner asked for.

## The trust ordering, precisely

`bw_path::VERIFIED_BW_EXE` is a `OnceLock`, deliberately first-wins, so that "no
code path can downgrade the process to a binary that was never checked". `main`
calls `remember_verified_bw_exe(bw_exe)` **unconditionally**, including when the
file does not exist — `resolve_bw_exe` returns the expected install path in that
case, and `check_bw_signature` is skipped precisely because there is nothing to
check.

Acquisition therefore runs in a process that has already recorded a path to a
file that was not there. Two consequences, and the design turns on both:

1. **Acquisition must install to exactly `install_bin_candidate(exe_dir)` and
   nowhere else.** Then the recorded path becomes correct by the file appearing
   underneath it, the `OnceLock` is never re-`set`, and the invariant is
   untouched. A second spelling of that path — a literal `"bin\\bw.exe"` joined
   somewhere else — would leave the process holding a path to one file while a
   different file sat on disk. This is pinned by a test, not by a comment.
2. **Acquisition owns the signature check for the file it installs**, because
   startup's did not run. It verifies before the copy, so a file that fails is
   never at the recorded path at all.

## Versions, and what this does not do

**Acquisition is present-or-absent, never present-or-stale.** Nothing in the app
pins a CLI version today; the only version-shaped question it asks is
`multi_account_availability`, which is about a directory beside `bw.exe`, not
about a release. Building a second updater — for somebody else's binary, with no
signal about which versions this app needs — is a larger feature wearing this
one's clothes. If a future `bw` breaks something, that is a bug report, not a
background download.

Also out of scope: any prompt, any Preferences row, any tray affordance for
this. It has no settings.

## Failure, offline, and the user who already has it

Every arm below is a distinct, nameable outcome. None is a spinner that stops,
and **none is vague**: every failure names the Bitwarden CLI, says
`bitwarden.com` cannot be used without it, and says a self-hosted server can.
That last clause is the actionable half and it appears in every failing row.

| Situation | What happens | What the user reads |
| --- | --- | --- |
| A Bitwarden-signed `bw.exe` is already resolvable | **Nothing.** No notice, no request, no progress bar. There is no requirement to state — it is already met. Sign-in proceeds exactly as today. | *(nothing)* |
| A `bw.exe` exists, validly signed by someone else (Scoop, Chocolatey) | The notice appears and acquisition runs, installing a Bitwarden-signed copy at `<InstallDir>\bin\bw.exe`, which `resolve_bw_exe` prefers over `PATH`. Next launch, `classify_bw_signature` returns `Trusted` instead of `AskUnrecognizedOrg` — these users stop being asked a question they could not answer. | the ordinary notice and progress |
| A `bw.exe` exists and is invalidly signed or tampered | Unchanged: `BwSignatureVerdict::Refuse` at startup, before this window exists. Acquisition does not paper over it. | existing refusal |
| Self-hosted server chosen | Acquisition never runs, on any arm, ever, and no notice is shown. | *(nothing)* |
| Official server selected, nothing pressed yet | The requirement is stated. Nothing is fetched. | "**bitwarden.com requires the Bitwarden CLI.** Deskwarden will download and install it from Bitwarden when you continue (about 37 MB, once)." |
| No network, DNS failure, connect timeout | Refused **before credentials leave the window**. `form.password` is not wiped, the form stays filled, Continue is live again. | "Deskwarden couldn't download the Bitwarden CLI, which bitwarden.com requires. Check your connection and try again. A self-hosted server can be used without it." |
| Transfer stalls mid-body | Same, via `http_agent::bounded_stall`'s existing stall timeout. | same as above |
| No `cli-v*` release, no `bw-windows-*.zip` asset, or no `sha256:` digest on it | Refused, fail-closed. `parse_asset_digest` already returns a required value rather than an `Option`, and that decision carries. | "Deskwarden couldn't find the Bitwarden CLI download, which bitwarden.com requires. Try again later, or install it yourself from bitwarden.com/help/cli/. A self-hosted server can be used without it." |
| SHA-256 mismatch | File discarded. Refused. Retry allowed — this is the failure that is usually a bad connection. | "The Bitwarden CLI download didn't arrive intact, so Deskwarden discarded it. Try again. bitwarden.com requires it; a self-hosted server does not." |
| **Authenticode invalid, or `O=` not in the trusted list** | File discarded. Refused. **Not retried automatically** — a retry loop against a substituted artefact is a loop. | "Deskwarden downloaded the Bitwarden CLI but could not confirm it came from Bitwarden, so it did not install it and did not run it. bitwarden.com requires the CLI, so you cannot sign in to it until this is resolved. You can install the CLI yourself from bitwarden.com/help/cli/, or use a self-hosted server, which does not need it." |
| Offline at sign-in, already set up | No acquisition is attempted and no notice is shown. The guard is "is there a verified `bw.exe`", not "is there network". Sign-in fails on its own terms. | existing |
| Window closed mid-download | Cancelled. Nothing is left at the install path — the copy is the last step and it did not happen. A stray temp file is swept the way `cleanup_stale_downloads` sweeps installers. **Nothing is retried in the background**; the next sign-in states the requirement again from the top. | *(nothing, until next time)* |
| User declines — picks Self-hosted, or closes with the notice showing | Nothing was fetched, because the notice precedes the download. The decline costs nothing and leaves nothing behind. | the notice they read, and no more |
| A newer CLI version exists | Nothing. See *Versions* above. | *(nothing)* |


## What happens to existing installs

**Nothing, and that is the whole answer.** A user on an earlier version already
has `<InstallDir>\bin\bw.exe` and an `HKCU\Environment` PATH entry pointing at
`<InstallDir>\bin`, both written by `bootstrap-bw.ps1`. The self-update runs the
new installer `/VERYSILENT`, and the new installer touches neither. Startup
resolves the same file, `check_bw_signature` verifies it as it does today,
acquisition sees a verified binary and never runs. The migration is a no-op with
no data to move.

Two consequences worth writing down:

* Acquisition must write the **same two things to the same two places** — the
  file at `install_bin_candidate`, and `<InstallDir>\bin` on the user's PATH. An
  existing user who deletes their `bw.exe` must get a replacement in the same
  spot, not a differently-placed one that leaves a stale PATH entry pointing at
  nothing.
* Uninstall keeps deliberately leaving both behind, per the existing spec. That
  reasoning ("the user may be using `bw` independently of deskwarden") is
  untouched by this change.

## How it will be known to work

The house defect is "a test that passes because it never reached the thing it
names", and this feature has a particularly inviting version of it: *a test that
asserts acquisition succeeded.* Such a test passes against an implementation
that downloads `bw-oss-windows`, or `bw-linux`, or accepts any signature at all.
So every acceptance below is paired with the rejection that would otherwise be
invisible.

1. **The right asset.** Given a release listing that contains
   `bw-oss-windows-2026.8.0.zip`, `bw-linux-2026.8.0.zip`,
   `bw-macos-2026.8.0.zip` **and** `bw-windows-2026.8.0.zip`, the resolver picks
   the last. Control: with `bw-windows-*` removed from the same listing it
   returns an error rather than falling back to the OSS build.
2. **The right release.** Given `desktop-v*`, `browser-v*`, a *newer*
   `cli-v*` marked `prerelease`, and an older stable `cli-v*`, it picks the older
   stable one. Control: with the prerelease flag cleared it picks the newer one,
   so the test is reading the flag and not the ordering.
3. **A wrong digest is refused**, nothing is written to the install path, and the
   temp file is gone. Control: the same bytes with the right digest install.
4. **A wrong signer is refused.** `O=Not Bitwarden`, `O=Bitwarden Solutions
   LLC`, and a `CN=Bitwarden Inc.` with a different `O=` are each refused;
   nothing reaches the install path. Control: `O=Bitwarden Inc.` installs.
   (`signature.rs`'s own tests already cover the DN parsing; these cover that
   acquisition *consults* it.)
5. **An unsigned binary is refused**, and so is one whose check errors.
6. **Nothing is ever executed.** A source pin over the acquisition module: it
   contains no `Command`, and none of `job_object::CHILD_STARTERS`
   (`spawn`/`output`/`status`) appears in it. This is the strongest single
   statement the design can make and it is nearly free, because the scanner
   exists.
7. **The destination is `bw_path`'s own.** A test asserting the install
   destination is the value `bw_path` computes, not a second spelling of it.
   Control: a hand-written `exe_dir.join("bin").join("bw.exe")` in the test must
   be shown to *equal* it, so the assertion is about agreement rather than about
   a string.
8. **Self-hosted never acquires.** Driven through
   `backend_policy::choose`, over the same combination table
   `the_whole_decision_table` walks, so the gate cannot become a second decision
   about which backend an account has.
9. **The dropdown states, and does not acquire.** Selecting `UsCloud` in the
   combo, with no Submit, paints the requirement notice **and** performs zero
   requests through the seam. Both halves in one test, because they are the two
   ways this moment goes wrong: a notice that arrives after the download has
   started is not a disclosure, and a disclosure that costs 37 MB to read is not
   one either. Control: selecting `SelfHosted` paints no notice, so the test is
   reading the choice rather than always finding the string.
10. **Every refusal names the CLI and the way out.** The owner's ruling, held
    by the file rather than by review: each `AcquireRefusal::message()` contains
    "Bitwarden CLI", names the server that requires it, and names self-hosting
    as the alternative — and none is empty or shorter than a sentence, which is
    what a vacuous version of this test would accept. Control: a deliberately
    euphemistic string ("Something went wrong. Try again.") must fail the same
    predicate the real messages pass.
11. **The requirement is stated before the first byte.** An ordering assertion
    over the seam: on a fresh official-server sign-in, the notice is painted at
    a frame strictly earlier than the first `resolve` call. This is the one
    property the ruling turns on and it is invisible to every other test here.

## Status

Design. Not implemented. Sequenced ahead of
`2026-08-31-the-installer-forgets-the-cli-design.md`, which must not land first.

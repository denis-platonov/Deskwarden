# deskwarden installer

Inno Setup script that builds `deskwarden`'s Windows installer:
per-user install (no admin rights needed), Start Menu shortcut,
autostart-on-login (on by default, opt-out checkbox), and an install-time
step that makes sure the Bitwarden CLI (`bw.exe`) is available.

## Build-time dependencies

- **[Inno Setup 6](https://jrsoftware.org/isinfo.php)** (`ISCC.exe`, the
  command-line compiler). Install it and make sure `ISCC.exe` is on `PATH`
  (or use its full path below), e.g. via Chocolatey:

  ```
  choco install innosetup
  ```

- That's it — **no other plugins are required**, and none ever will be for a
  download step: the script has no `[Code]` section at all. See *What this
  installer no longer does* below.

- A release build of `deskwarden.exe` at `..\target\release\deskwarden.exe`
  (i.e. build the Rust crate first: `cargo build --release` from
  `deskwarden/`).

## Building locally

From the `deskwarden/` directory:

```
cargo build --release
iscc installer\deskwarden.iss /DAppVersion=0.1.0
```

This produces `installer\deskwarden-0.1.0-installer.exe` (the script sets
`OutputDir=.` so the compiled installer lands directly in `installer/`,
matching what the release CI workflow expects to find and publish).

If `AppVersion` isn't passed via `/D`, the script falls back to `0.0.0` so
it still compiles for a quick local sanity check.

## What this installer no longer does

**It no longer installs the Bitwarden CLI.** Until 0.14.0 the script carried a
`[Code]` section that ran a 370-line `bootstrap-bw.ps1` at `ssPostInstall`: it
queried `https://api.github.com/repos/bitwarden/clients/releases`, filtered for
`cli-v*` tags, downloaded the Windows archive, verified its Authenticode
signature with `Get-AuthenticodeSignature`, extracted `bw.exe` into
`<InstallDir>in`, and added that directory to the user's `PATH`. A custom
`TOutputProgressWizardPage` existed to give it visible feedback, and four
`SuppressibleMsgBox` dialogs reported its failures -- every one of them ending
by telling the user to go and install a command-line tool by hand.

All of it is deleted. The reasons:

1. **It ran for everybody, before anybody had chosen a server.** The CLI is
   required only where `bw serve` is the vault. An installer that spends a
   minute and ~37 MB on it is charging every user for a dependency some of
   them will never need.
2. **It was the app's second, divergent Authenticode mechanism.**
   `src/signature.rs` verifies in-process with `WinVerifyTrust`, because
   `Get-AuthenticodeSignature` fails wherever `Microsoft.PowerShell.Security`
   cannot autoload -- a trust gate that cannot answer. The script used the
   cmdlet, and hand-maintained a second X.500 DN parser
   (`Get-CertificateDnComponent`) whose own doc admitted it was "kept in sync
   by hand" with `dn_component`. Two parsers of one grammar, in two
   languages, held together by a comment.

`src/bw_acquire.rs` does this now, from the sign-in window, at the moment a
server is chosen that requires it -- asking first, with a modal the user can
cancel. It creates `<InstallDir>in` and writes the `PATH` entry itself,
because nothing else does any more.

**One finding from the deleted script is still live** and moved with it: the
monorepo tag filter. `bitwarden/clients` publishes cli, desktop, browser and
web releases interleaved by date, so GitHub's generic "latest release" for the
repo is *not* the CLI's latest release; resolving the right one means
filtering the releases list on a `cli-v` tag prefix. That reasoning now lives
on `bw_acquire::pick_artefact`, along with a second one the script did not
have: `bw-oss-windows-<version>.zip` sits beside `bw-windows-<version>.zip`
and sorts before it, so the asset prefix must be anchored at the front.

Three tests in `src/main.rs` hold the installer to this --
`the_installer_says_nothing_about_the_bitwarden_cli`,
`the_installer_shells_out_to_nothing`, and
`the_bootstrap_script_is_not_in_the_tree`. They are plain text scans of
`deskwarden.iss`, so a *mention* in a comment reads exactly like a call; that
is why this history lives in this file rather than in the script.

**Uninstall still deliberately leaves `bw.exe` and its `PATH` entry behind.**
The user may be using the Bitwarden CLI independently of Deskwarden. That
reasoning now lives with the code that writes both, in `bw_acquire`'s module
docs.


## Testing without Inno Setup installed

`ISCC.exe` was not available in the environment this script was authored
in (a `choco install innosetup` attempt failed for lack of admin rights).
The script was reviewed carefully instead. That mattered more when there was
a `[Code]` section to get wrong; there is not any more, so what remains is
declarative sections a compile either accepts or rejects outright. Actual
compilation is deferred to Task 6's
release CI workflow (`.github/workflows/release.yml`), which installs
Inno Setup via `choco install innosetup -y` on a `windows-latest` runner —
this will be the first real compile of `deskwarden.iss`.

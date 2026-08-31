# deskwarden installer

Inno Setup script that builds `deskwarden`'s Windows installer:
per-user install (no admin rights needed), Start Menu shortcut, and
autostart-on-login (on by default, opt-out checkbox).

It installs no Bitwarden CLI — the app acquires that itself, if and when a
server is chosen that needs it (*What this installer no longer does*, below).
Uninstall takes back everything the app created, and offers a checkbox for the
user's own data (*What uninstall removes*).

## Build-time dependencies

- **[Inno Setup 6](https://jrsoftware.org/isinfo.php)** (`ISCC.exe`, the
  command-line compiler). Install it and make sure `ISCC.exe` is on `PATH`
  (or use its full path below), e.g. via Chocolatey:

  ```
  choco install innosetup
  ```

- That's it — **no other plugins are required**, and none ever will be for a
  download step. The script's `[Code]` section runs on the uninstall path
  only; the install path is declarative. See *What this installer no longer
  does* and *What uninstall removes* below.

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
`<InstallDir>\bin`, and added that directory to the user's `PATH`. A custom
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
cancel. It creates `<InstallDir>\bin` and writes the `PATH` entry itself,
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

## What uninstall removes

Found by inspecting a real machine after the owner deleted the app: Deskwarden
left about 17 MB behind, including the key that decrypts the vault.

**A plain uninstall — ticking nothing — now removes everything Deskwarden
created except the user's own data:**

- `<InstallDir>\bin`, and the Bitwarden CLI in it.
- The `PATH` entry for that directory, in `HKCU\Environment`.
- The icon cache and the update downloads, under
  `%LOCALAPPDATA%\Deskwarden\Deskwarden\cache` and
  `%LOCALAPPDATA%\deskwarden\cache`.
- The `HKCU\...\Run` autostart value, **if it points at this installation**.

Inno knew about none of these. It removes what setup copied, and every item
above was written at runtime by the app — which is why `{app}` survived the
uninstall as a non-empty directory.

**`bw.exe` is the one that changed its mind, and it is worth saying why.**
Until now the rule was that uninstall leaves it: the user may be using `bw`
independently, and deleting a working command-line tool because an unrelated
tray app went away is worse than leaving 40 MB behind. That was written for
`bootstrap-bw.ps1`, when the CLI could plausibly be one the user already had.
It is no longer the file it guards. `bw_acquire` puts the CLI under
`<InstallDir>\bin` — a directory this app creates inside its own install
location, from a download this app made, on a `PATH` entry this app wrote.
Nobody's independent copy of `bw` lives there. Keeping it meant stranding a
40 MB binary and a `PATH` entry inside the install directory of an app that no
longer exists. A `bw` installed anywhere else is untouched, which was always
the real content of the old rule.

**The user's data is removed only if they ask.** The uninstaller shows one
dialog with a checkbox, *"Also delete all data and settings"*, **unchecked by
default**. Left clear, `%APPDATA%\Deskwarden` stays exactly as it is:
`settings.json`, the per-account `data.json` and `session.bin`, the encrypted
`vault-cache.bin`, the Windows Hello enrolment, the log, and `userkey.bin` —
the DPAPI-wrapped master key, which never expires. The dialog says so in those
terms, so a user who wants it gone can find the option and a user who is
reinstalling cannot lose the vault by not reading carefully. A silent
uninstall (`/VERYSILENT`) takes the default and keeps the data.

The two registry entries are removed only after a path check. The `Run` value
is compared against `{app}\deskwarden.exe` before deletion, exactly as
`autostart_repair::repaired_value` compares it before a write, and for the
reason its doc gives: a value pointing somewhere else is not ours to rewrite,
nor ours to delete. The `PATH` entry is matched case-insensitively with
trailing separators stripped, the same comparison `add_to_user_path` used
going in; every other entry is copied through verbatim.

`uninsdeletevalue` on the `[Registry]` line does **not** cover the `Run` value
on its own. That flag removes only what setup wrote, and setup writes it only
when the autostart task was selected — but `autostart_repair::repair_logon_entry`
in the app also writes it, so a value setup does not own can exist. One did, on
the owner's machine, pointing at a `deskwarden.exe` that was already gone.


## Compile it. Reviewing it is not enough.

This file used to say that `ISCC.exe` was unavailable here, that the script had
been reviewed carefully instead, and that this was tolerable because there was
no `[Code]` section left to get wrong. All three parts of that have expired.

`ISCC.exe` **is** available, at
`%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe` — a per-user install, which is
why looking in `Program Files` and on `PATH` finds nothing and concludes the
wrong thing. Compile before claiming anything about this file:

```
& "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" installer\deskwarden.iss /DAppVersion=0.15.0
```

It needs `..\target\release\deskwarden.exe` to exist; `cargo build --release`
first, or drop a placeholder there for a syntax-only check.

**Review does not catch Pascal.** The uninstall `[Code]` section above was
read over, looked right, and failed to compile three times running:

1. `;` comments. In every other section of this file `;` starts a comment;
   inside `[Code]` it is a statement separator, and the compiler reports
   `'BEGIN' expected` on the line the comment block *starts* on.
2. `CreateCustomForm()`. It takes four arguments, not none —
   `Examples\CodeClasses.iss` in the Inno directory has the shape.
3. `{ }` comments containing `{app}`. Pascal block comments do not nest, so
   the `}` of a constant ends the comment and the rest of the sentence is
   parsed as code.

Note also that ISCC prints `Reading [Code] section` early and *then*
`Compiling [Code] section` much later, after the `[Files]` entries are
resolved. Getting past the first one means nothing. A compile that stops on a
missing `deskwarden.exe` has not compiled the Pascal at all — which is exactly
the misreading that lets a broken `[Code]` section reach a release.

The release CI workflow (`.github/workflows/release.yml`) installs Inno Setup
via `choco install innosetup -y` on a `windows-latest` runner and compiles
there too. That is the backstop, not the first check.

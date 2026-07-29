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

- That's it — **no other plugins are required.** An earlier draft of
  `deskwarden.iss` used the Inno Download Plugin (`idp.iss`) for the
  Bitwarden CLI download step; the shipped version instead shells out to
  Windows PowerShell (via `bootstrap-bw.ps1`, included alongside the
  script), which ships on every supported Windows install and can do the
  GitHub-releases JSON parsing that plugin can't. See the comment on
  `InstallBwCliIfMissing` in `deskwarden.iss` for the full reasoning.

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

## What the bw-CLI bootstrap step does

`InstallBwCliIfMissing` (in `deskwarden.iss`, invoked at `ssPostInstall`)
extracts and runs `bootstrap-bw.ps1`, which:

1. Skips everything if `bw.exe` is already on `PATH` or already present in
   this install's `bin` folder (e.g. a reinstall/upgrade).
2. Otherwise, queries `https://api.github.com/repos/bitwarden/clients/releases`
   and picks the newest release whose tag matches `cli-v*` — `bitwarden/clients`
   is a monorepo that also publishes desktop/browser/web releases interleaved
   by date, so GitHub's generic "latest release" for the repo is not
   reliably the CLI's latest.
3. Downloads that release's `bw-windows-<version>.zip` asset (the official
   standalone Windows build — not `bw-oss-windows-*`, which lacks
   paid-tier vault features) and extracts `bw.exe` from it.
4. Verifies `bw.exe`'s Authenticode signature via PowerShell's
   `Get-AuthenticodeSignature` (same mechanism `src/signature.rs` uses for
   deskwarden's own self-update verification) — checks both that the
   signature is `Valid` and that the signer's certificate subject actually
   names Bitwarden. Refuses to install it otherwise.
5. Copies the verified `bw.exe` into `<install dir>\bin\bw.exe` and adds
   `<install dir>\bin` to the current user's `PATH` (`HKCU`, no admin
   needed) — deskwarden invokes the CLI as a bare `bw` command (see
   `src/bw_serve.rs`, `src/login_ui.rs`), relying entirely on `PATH`
   resolution, so this step is required for it to actually find the binary.

Verified against Bitwarden's real, current release infrastructure on
2026-07-28: repo `bitwarden/clients`, latest CLI tag `cli-v2026.7.0`, asset
`bw-windows-2026.7.0.zip` at
`https://github.com/bitwarden/clients/releases/download/cli-v2026.7.0/bw-windows-2026.7.0.zip`.

## Uninstall behavior

Uninstalling deskwarden removes deskwarden's own installed files and the
`HKCU\...\Run` autostart entry. It deliberately does **not** remove
`bw.exe` or the `PATH` entry added for it — the user may be using the
Bitwarden CLI independently of deskwarden, per the distribution design
spec's Installer section.

## Testing without Inno Setup installed

`ISCC.exe` was not available in the environment this script was authored
in (a `choco install innosetup` attempt failed for lack of admin rights).
The script was reviewed carefully instead, and `bootstrap-bw.ps1` was
separately validated by parsing it with PowerShell's own script parser
(`[System.Management.Automation.Language.Parser]::ParseFile`), which
reported no syntax errors. Actual compilation is deferred to Task 6's
release CI workflow (`.github/workflows/release.yml`), which installs
Inno Setup via `choco install innosetup -y` on a `windows-latest` runner —
this will be the first real compile of `deskwarden.iss`.

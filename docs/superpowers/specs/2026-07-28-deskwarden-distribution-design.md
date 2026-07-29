# Deskwarden Distribution — Rename, GitHub Repo, Auto-Update, Installer — Design

Date: 2026-07-28
Status: Approved for planning

## Problem

`nodewarden-native` (the native Windows autofill companion built in the prior phase) exists only as local commits with no public repository, no way for anyone else to install it, and no way to receive updates. To actually publish this as a free, open-source tool per the project's stated goal, it needs: a public home, an installable package that handles its one external dependency (the Bitwarden CLI), and a way for users to get security fixes without manually rebuilding from source.

## Rename: nodewarden-native → deskwarden

The public product name is **deskwarden**, chosen over the working name `nodewarden-native` at repo-creation time. This is a full rename, not just the repo name, to avoid a confusing mismatch between what users see and what the code is called:

- Crate/package name and binary name: `nodewarden-native` → `deskwarden`.
- `directories::ProjectDirs` identifiers (used for the config dir, session-token cache, log file path) updated to match.
- Window titles, tray tooltip, and all other user-visible strings updated.
- The vault custom-field name changes from `nodewarden:app-match` to `deskwarden:app-match`. Safe to rename outright — nothing has shipped publicly yet, so there's no existing-user migration to handle.
- `LICENSE`/`README.md` updated to reference the new name throughout.

## GitHub repo

- Public repository: `denis-platonov/deskwarden` (personal account, not the `nappsllc` org).
- Full existing git history is pushed, including all design/spec/plan docs and every implementation task's commits — good provenance for an open-source project, nothing sensitive in that history.
- Actually running `gh repo create` and pushing happens as an explicit, confirmed step during implementation (not assumed here) — creating a public resource gets a final go-ahead at the point it's actually about to happen, same as any other publish action.

## Release pipeline

New GitHub Actions workflow, `.github/workflows/release.yml`, triggered on a version tag push (e.g. `v0.1.0`):

1. Build `deskwarden.exe` in release profile.
2. Code-sign it via SignPath's free open-source signing program. **This requires the user to independently apply to and be approved by SignPath, and to wire the resulting signing identity into this workflow — that account setup is a manual prerequisite this design does not automate.** The workflow is built assuming a working SignPath integration exists; if it isn't set up yet by the time this is implemented, the pipeline should still function with an explicit "unsigned build" fallback path documented, not silently ship unsigned binaries as if they were signed.
3. Build the Inno Setup installer (bundling the signed `deskwarden.exe`).
4. Code-sign the installer with the same certificate/identity as step 2.
5. Publish both the standalone `deskwarden.exe` and the installer as GitHub Release assets under the pushed tag, with the tag name as the version.

## Auto-update

New `updater.rs` module in the crate, wired into the existing tray/main-loop structure (following the same pattern as the periodic match-engine refresh already in `main.rs`).

- **Check**: on startup, and on a periodic timer (e.g. once every 24 hours), call GitHub's public Releases API (`GET /repos/denis-platonov/deskwarden/releases/latest`, unauthenticated — public API, no token needed) and compare the returned tag against the running binary's own compiled-in version (`env!("CARGO_PKG_VERSION")`).
- **Notify, don't auto-apply**: if a newer version is available, surface it via the tray (a badge/changed tooltip, plus a new "Update available" menu item) — mirroring the explicit "no silent background installs" decision made for this security-sensitive tool. The user must click to proceed.
- **Verify before applying** — this is the security-critical part:
  1. Download the new installer asset from the GitHub Release over HTTPS.
  2. Verify its Authenticode signature is valid (chains to a trusted root) using Windows' native trust-verification API (`WinVerifyTrust` or the `windows` crate's equivalent — investigate the exact binding the same way prior Win32 API work in this project did).
  3. **Pin the signer**: additionally check that the signing certificate's thumbprint matches the thumbprint of the certificate that signed the *currently running* binary (read at build/startup time, not just "any validly-signed file passes"). A bare "is this signed by someone" check is insufficient — pinning to the specific expected signer is what prevents a compromised-but-differently-signed release from being accepted.
  4. Only if both checks pass: launch the installer silently (Inno Setup's `/VERYSILENT /SUPPRESSMSGBOXES`, with the installer configured to relaunch the app after completing) and exit the current process cleanly (reusing the existing `bw serve` shutdown path already built for the Quit flow, so an update doesn't orphan the CLI subprocess).
  5. If either check fails: refuse to apply the update, log a clear error, leave the running app untouched.

## Installer

New `installer/deskwarden.iss` (Inno Setup script), built in the release pipeline (see above).

- **Per-user install** to `%LOCALAPPDATA%\deskwarden` — avoids requiring admin rights / a UAC prompt for a personal tray tool.
- Start Menu shortcut created.
- **Autostart on login, on by default**: a checkbox during install (checked by default) registers a standard `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` entry; unchecking skips it. The uninstaller removes this entry if present.
- **CLI dependency check-and-install**: an install-time step (Inno Setup `[Code]` Pascal script) checks whether `bw.exe` is already available (PATH and common install locations). If missing:
  1. Download Bitwarden's official standalone CLI build directly from Bitwarden's own GitHub releases, over HTTPS.
  2. Verify the downloaded `bw.exe`'s Authenticode signature (Bitwarden signs their own releases) before running/installing it — no unsigned third-party binary is executed.
  3. Install it alongside deskwarden and ensure it's reachable on the user's PATH.
- **Uninstaller**: standard Inno Setup uninstall flow. Removes the autostart registry entry and deskwarden's own files. Deliberately leaves the `bw` CLI installed (the user may be using it independently of deskwarden) and does not touch anything vault-side (custom fields on vault items are server-side data, nothing local to clean up).

## Non-goals (v1)

- Cross-platform support (this whole project, including distribution, stays Windows-only, consistent with the original design).
- Delta/differential updates — each update is a full new installer download.
- MSI packaging / enterprise group-policy deployment (WiX Toolset was considered and explicitly declined in favor of Inno Setup for this project's scale).
- Automating SignPath's account-approval process itself (external, manual, one-time prerequisite).
- Telemetry or crash reporting of any kind.

## Open questions for later (not blocking v1)

- Exact update-check interval (24h chosen as a reasonable default; not deeply validated).
- Whether the installer should also offer a "portable, no install" zip distribution alongside the Inno Setup installer.

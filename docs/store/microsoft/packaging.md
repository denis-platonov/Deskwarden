# Microsoft Store: the package

Everything the owner needs to turn this repository into a file Partner Center
accepts. Nothing here creates an account, signs in, uploads or downloads
anything; those steps are in [`../CHECKLIST.md`](../CHECKLIST.md).

## Decision: MSIX, full trust (`runFullTrust`)

The Store takes two shapes of Win32 app: an **MSIX package** (the Store hosts
and signs it) or a link to an **EXE/MSI installer** you host yourself
(Partner Center "EXE or MSI app"). The choice for Deskwarden is **MSIX**.

| | MSIX (chosen) | EXE/MSI link |
|---|---|---|
| Who signs | The Store, on ingestion: no certificate to buy | The installer must carry a code-signing certificate from a CA the Store trusts. This project ships **unsigned** today (`docs/code-signing-policy.md`) and has no certificate |
| Who hosts and updates | The Store: updates are the Store's, which is what the policy asks for | The owner hosts a versioned, never-changing URL; the installer may not self-update; every release needs a re-submission against a fresh URL |
| Silent install | Not applicable (the Store installs) | Required, with documented switches; the Inno installer supports `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER`, but certification expects a per-machine, signed installer |
| Uninstall | Clean: the Store removes the package and its virtualised `%APPDATA%` | Inno uninstaller (works, asks about user data) |
| Fit for the app | See below | Fits equally, but needs the certificate |

**Does a tray app that runs at login, uses UI Automation and `SendInput`, and
starts a background process fit in MSIX?** Yes, as a *full-trust desktop
app*: the manifest declares `<rescap:Capability Name="runFullTrust"/>` and a
Win32 entry point (`EntryPoint="Windows.FullTrustApplication"`). A full-trust
package process runs **outside** the AppContainer, at the user's own
integrity level, with the user's token. So:

- **UI Automation** to other applications' windows works (it is a COM client
  call, and the package is not sandboxed). Unaffected.
- **`SendInput`** into the foreground window works, same reason. (Elevated
  target windows are refused exactly as on the installer build: UIPI, not
  packaging.)
- **Background processes**: the daemon and the window process are children of
  the same package identity and are launched from the package's own
  `Deskwarden.exe`; the job-object tie between them is plain Win32. Fine.
- **Tray icon**: plain Win32 (`tray-icon`). Fine.
- **Run at login**: the manifest's `windows.startupTask` extension (below)
  replaces the installer's `HKCU\...\Run` value.
- **Registry and file writes**: `HKCU` and `%APPDATA%` writes by a packaged
  app are *virtualised* into the package's private store and removed on
  uninstall. The app's settings and DPAPI files live in `%APPDATA%\Deskwarden`
  and keep working; they are simply package-private. See "Known engineering
  follow-ups" for the two places this matters.

The EXE/MSI route stays available as a fallback if Microsoft's certifiers
refuse the full-trust declaration; it would need a code-signing certificate
first (`docs/code-signing-policy.md`).

## What is in the repository

| File | What it is |
|---|---|
| `deskwarden/Cargo.toml` `[features] store` | The Store flavour of the program (below) |
| `scripts/store/AppxManifest.xml` | Manifest **template**: four `@TOKENS@` for Partner Center identity and the version |
| `scripts/store/make-assets.py` | Generates every logo size from the app's own icon geometry (stdlib only, no drawing, no downloads) |
| `scripts/store/build-msix.ps1` | Builds the `store` flavour, generates the logos, stages, runs `makepri` and `makeappx pack` |
| `.github/workflows/release.yml` job `store-package` | Builds the package on a release tag as a **workflow artifact** (placeholder identity, `UNSUBMITTABLE` in the file name; never uploaded anywhere) |

Tools on the build machine: Rust, Python 3 and the Windows SDK's
`makeappx.exe` / `makepri.exe`. On the maintainer's PC (checked 2026-11-19)
`makeappx.exe`, `makepri.exe` and `signtool.exe` are all present in
`C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64` (and 10.0.28000.0).
The script looks there, newest SDK first, and **does not download an SDK**;
without `makeappx` it stops and says so. GitHub's `windows-latest` runner has
the SDK installed, which is what the workflow job relies on.

### Building the file to upload

```powershell
scripts\store\build-msix.ps1 `
  -PackageName 'YourID.Deskwarden' `
  -Publisher 'CN=XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX' `
  -PublisherDisplayName 'Your Name'
# -> store-package\Deskwarden_1.0.0.0_x64.msix   (UNSIGNED: upload as is)
# -> store-package\generated-assets\listing\     (the Partner Center logos)
```

The three identity values come from Partner Center -> *Product management* ->
*Product identity*, after the name is reserved. The script refuses without
them (or without `-CiBuild`). The version is read from `Cargo.toml` with the
`-dev` suffix dropped and `.0` appended: **the Store reserves the 4th
component**, so 1.0.0 becomes `1.0.0.0`, 1.0.1 becomes `1.0.1.0`, and every
submitted package needs a higher version than the last.

To try the package on the owner's own PC before submitting: create a
self-signed certificate whose subject equals `-Publisher`, trust it, pass
`-SignPfx`, and install the signed copy with `Add-AppxPackage`. **Never
upload that signed copy.**

### Visual assets

`make-assets.py` writes, from the app's icon geometry, under `Assets\`:
`StoreLogo` (50 px), `Square44x44Logo` (44), `Square71x71Logo`,
`Square150x150Logo`, `Wide310x150Logo`, `Square310x310Logo`, `SplashScreen`
(620x300), each at scale 100/125/150/200/400, plus `Square44x44Logo`
target sizes 16/24/32/48/256 in plated and `_altform-unplated` forms; and the
Partner Center store logos (1:1 300 and 1080, 2:3 720x1080, 71x71). The mark
is the quartered blue shield on transparency. It is **not** derived from
Bitwarden's branding.

## The `store` build flavour

`cargo build --release --features store` (off by default; `store` changes no
type, adds no dependency):

| Behaviour | Default | `--features store` |
|---|---|---|
| `updater::SELF_UPDATE` | `true` | `false` |
| `updater::check_for_update` | asks GitHub | returns `Ok(None)`, **no request** |
| `updater::download_and_verify` | downloads + SHA-256 | refuses before creating any file |
| `updater::apply_update` | runs the installer | refuses before hashing or launching anything |
| `bw_acquire::acquire_if_needed` | downloads, verifies and installs Bitwarden's `bw` | refuses with `AcquireRefusal::NotInThisEdition`, **no request** (a `bw` the user installed is still used) |
| `updater::store_updates_notice()` | `None` | `Some("Updates come from the Microsoft Store")` |

Why: The Microsoft Store Policies (the security section; re-read the current text
at submission time, the numbering moves) forbid an app updating itself
outside the Store and restrict downloading executable code. Both the self-updater and the `bw` downloader would do
exactly that. Tests: `cargo test --lib --features store` (both flavours of
each gate are pinned, and a source guard keeps the gates in gating position).

### The one UI change this branch deliberately did not make

`src/prefs_ui.rs` and `src/prefs_ui/*` were off-limits to this slice. The
Preferences -> Updates page needs this hook, at the top of whatever draws the
page body:

```rust
if let Some(notice) = deskwarden::updater::store_updates_notice() {
    ui.label(notice);   // "Updates come from the Microsoft Store"
    return;             // no switch, no Check button, no Download button
}
```

(or the crate-local `crate::updater::store_updates_notice()`). Until it
lands, the Store build's Updates page shows its normal controls and pressing
Check answers "no update", which is harmless but is wrong copy for a Store
listing; **land the hook before building the file that gets submitted.**

### Rulings

- **Ruling: the Store flavour refuses the `bw` download instead of routing
  every account to the built-in client** -- the built-in client is validated
  for self-hosted servers only (`backend_policy::choose` sends bitwarden.com
  and bitwarden.eu to `bw serve` because they need the official CLI); forcing
  `DirectRest` for the cloud would change sign-in behaviour for those servers
  without anyone having tested it against them. So in the Store edition a
  self-hosted account uses the built-in client and never needs `bw`, while a
  bitwarden.com/.eu account needs a `bw` the user installed themselves
  (the sign-in card says so, `AcquireRefusal::NotInThisEdition`) -- running a
  program the user installed is not downloading code -- cost if wrong: the
  Store listing must say plainly that bitwarden.com accounts need the
  Bitwarden CLI installed by the user, and a certifier may call that
  friction. The alternative (cloud over the built-in client) needs a real
  bitwarden.com account to prove it works, and is a product decision for the
  owner (see CHECKLIST.md, decision D1).
- **Ruling: MSIX over EXE/MSI** -- the Store signs MSIX itself and no
  code-signing certificate exists yet; the app's UI Automation, `SendInput`,
  tray and child processes all work in a full-trust package -- cost if wrong:
  if certification refuses `runFullTrust`, the fallback is the EXE/MSI route,
  which needs a certificate.
- **Ruling: the startup task ships `Enabled="false"`** -- the Inno
  installer's autostart task is an unticked opt-in and the Store expects
  the same -- cost if wrong: users must switch it on once (Settings -> Apps ->
  Startup).
- **Ruling: `webcam` and `privateNetworkClientServer` capabilities are
  declared** -- the QR-from-webcam screen and LAN/self-hosted servers need
  them in a packaged app -- cost if wrong: a certifier asks for a
  justification (certification-notes.md has one) or one is removed.
- **Ruling: the self-update gate is a `const bool`, not a runtime check on
  the package identity** -- a build that was never packaged cannot become a
  Store build by running somewhere, and the Store build cannot update itself
  by being run unpackaged -- cost if wrong: a Store-flavour exe copied out of
  the package would also not update itself (acceptable).

- **Ruling: `store` is a declared Cargo feature, so `job_object`'s
  build-divergence guard was amended** -- that test said a feature may only ever
  ADD test code, and a Store edition that cannot self-update is by definition
  a subtraction. The amendment is narrow: declared features are exactly
  `test-support` and `store`; the crate may contain exactly ONE `cfg!` naming
  `store` (the definition of `updater::SELF_UPDATE`, pinned to `updater.rs`);
  everything else, tests included, asks the constant. The `Cargo.toml` byte pin
  was re-pinned (30230 bytes; no dependency moved; the recomputing script first
  reproduced the old pair). The tested-equals-shipped property is kept by
  testing both editions: CI gained a `cargo test --lib --features store` step
  -- cost if wrong: a future feature could hide behind the same exception; the
  guard still names the one allowed `cfg!` and fails on a second.
- **Ruling: tests that need the real updater or CLI download are no-ops under
  `--features store`, and the Store-only assertions are no-ops without it** --
  the guard allows no `#[cfg(feature)]` on tests, so they read
  `SELF_UPDATE` at run time -- cost if wrong: a default-only run shows the
  Store tests green without exercising them; that is why CI runs both.
- **Ruling: the manifest template is `scripts/store/AppxManifest.xml`, not under
  `deskwarden/`** -- a guard pins the exact set of non-Rust files in the crate
  directory, and the package is a release-engineering asset, not part of the
  program -- cost if wrong: none (a path).

### Known engineering follow-ups (not done here; each is small)

1. **Autostart under MSIX.** `autostart_repair` writes `HKCU\...\Run`. In a
   packaged app that write is virtualised and has no effect; the manifest's
   startup task is the real mechanism. The Store flavour should skip the Run
   key and offer the task (`StartupTask.RequestEnableAsync`) from the
   Autostart setting. A startup-task launch carries **no `--autostart`
   argument**, so the app must treat "activated by startup task" as
   `--autostart` (read `AppInstance.GetActivatedEventArgs().Kind ==
   ActivationKind::StartupTask`).
2. **Where `bw` lives.** `bw_acquire` installs into `{app}\bin`, which is
   read-only in a package; in the Store flavour the path is the user's own
   (`%LOCALAPPDATA%` is virtualised too), so a user-installed `bw` must be
   found on `PATH` (it is, via `bw_path`) -- verify on a packaged install.
3. **Single-instance and the installer's `AppMutex`** are unaffected.

## Release workflow

The `store-package` job in `release.yml` runs the script with `-CiBuild` on
each `v*` tag, in parallel with the installer, and keeps the result as a
workflow **artifact** (14-day retention). It uploads nothing to the Store and
touches no secret. The file is named `...-UNSUBMITTABLE.msix` because its
identity is a placeholder: the owner builds the real one locally (above) or
adds the three identity values as repository variables and drops `-CiBuild`
(not done: it would put the Partner Center identity in CI, which is a
decision for the owner).

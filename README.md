# deskwarden

> **Unofficial and unaffiliated with Bitwarden.** This is an independent,
> community-built tool. It is not made by, endorsed by, supported by, or
> connected to Bitwarden, Inc. in any way. "Bitwarden" is a trademark of
> Bitwarden, Inc., used here only to describe what this tool interoperates
> with. If you have a problem with this tool, do not contact Bitwarden support
> — open an issue here instead.

**deskwarden** fills credentials from your Bitwarden vault into **native
Windows applications** — the kind of desktop app or game launcher a browser
extension can't reach — and gives you a vault browser to manage them from,
right in the system tray.

- **[Download the latest release](https://github.com/denis-platonov/deskwarden/releases/latest)**
  — a per-user Windows installer, no admin rights required.
- **[Full documentation](deskwarden/README.md)** — requirements, trigger modes,
  self-hosted servers, troubleshooting, and security notes.

## Why this exists

Password managers with a browser extension autofill logins on web pages
just fine. None of them reach into a native desktop app or a game launcher's
own login window — that's a browser-extension blind spot every one of them
shares. Some commercial managers (Keeper is one) ship this as a native-app
autofill feature; Bitwarden doesn't, and switching to it from one of those
meant losing that specific capability.

deskwarden fills that one gap, nothing more: it watches which window has
focus, matches it against your vault by process name, and types the
credentials in — the same idea a QA automation tool like Mabl or a game
launcher's own saved-login flow uses, aimed at a password manager instead.
Everything else (the vault browser, folders, TOTP, sync) exists because a
tray app that can fill into a native window is also the natural place to
*manage* the vault items that do the filling, without a browser extension
open.

## What it does

- **Native-app autofill.** Match an app once (by process name), pick a
  trigger — auto-fill on focus, a small overlay prompt, or a global hotkey —
  and it's stored on the vault item itself, so it syncs like any other vault
  data.
- **A vault window**, opened from the tray (left-click, or right-click →
  Open Vault): folders, search, item create/edit/delete, live TOTP codes,
  a password-strength indicator, real website favicons, and a manual/auto
  sync — everything short of full Bitwarden-client parity, scoped to what a
  tray app plausibly needs.
- **Nothing here re-implements vault security.** Every credential read or
  write goes through the official Bitwarden CLI's local `bw serve` bridge;
  deskwarden never touches encryption, key derivation, or sync logic itself.

## Stack, and why

| Choice | Reasoning |
| --- | --- |
| **Rust** | Native Win32/COM interop (UI Automation, DPAPI, WinTrust, Job Objects) without an FFI layer on top of a managed runtime, and a single static binary with no runtime to install. |
| **[`windows`](https://crates.io/crates/windows) crate (raw Win32/WinRT bindings)**, not a higher-level GUI-automation library | The two things that actually need OS-level access — reading whatever window is in the foreground, and typing into an arbitrary native control — don't have a portable abstraction worth building on. Direct bindings also make the Authenticode signature check on the bundled `bw.exe` (real `WinVerifyTrust`, not a shelled-out PowerShell call) and the DPAPI-encrypted session cache possible without another dependency. |
| **[`eframe`/`egui`](https://github.com/emilk/egui) (immediate-mode GUI)** | A tray app with a handful of small windows (login, an autofill overlay, a picker, the vault browser) doesn't need a retained-mode widget tree or a bundled browser engine — egui compiles into the same static binary and adds single-digit megabytes, not a WebView2 dependency. |
| **The Bitwarden CLI (`bw`) as the only vault backend**, via its local `bw serve` REST bridge | The alternative is reimplementing Bitwarden's crypto, sync protocol, and API client from scratch — a security liability for a community tool with no reason to exist. deskwarden only ever talks to `localhost`; the real vault server is whatever the CLI itself is configured against (official cloud or self-hosted). |
| **DPAPI** for the cached session token, **Windows Hello** (`KeyCredentialManager`) for optional quick-unlock | Both are already the OS's own answer to "encrypt this for the current Windows user" — no key management of deskwarden's own to get wrong. |

## Size

Numbers from an actual release build (`cargo build --release`, with LTO +
single codegen unit + stripped symbols), measured live and side-by-side
against the official Bitwarden desktop app on the same machine — not vendor
figures, not estimates.

deskwarden depends on the official Bitwarden CLI (`bw serve`, a bundled Node
runtime) for all real vault work, spawned as its own subprocess. That
process's footprint isn't something deskwarden's own code controls, so it's
broken out separately below rather than hidden inside one number.

**RAM** — private working set (the same figure Windows Task Manager's
"Memory" column shows: resident memory unique to that process, excluding
anything shared with sibling processes of the same app). This matters
because a naive "total resident memory" sum inflates a multi-process app
like Electron, which shares a lot of memory *between* its own processes —
private working set is the number that's actually comparable, and the one
you can verify yourself in Task Manager:

| | Tray only | Window open |
| --- | --- | --- |
| **deskwarden.exe** (own process) | 44 MB | 52 MB |
| `bw serve` (bundled CLI, unavoidable) | 76 MB | 80 MB |
| **deskwarden total** | **120 MB** | **132 MB** |
| Bitwarden Desktop (all 4 of its processes) | 132 MB | 135 MB |

deskwarden's *own* process alone is consistently smaller — roughly a third
to two-fifths of Bitwarden Desktop's total. But once the `bw` CLI dependency
is counted in (which it has to be — it's what does the real vault work),
the **total is close to parity** with Bitwarden Desktop, not the dramatic
gap a shared-memory-inflated number would suggest. Where deskwarden actually
wins clearly is disk footprint and its own process's isolated cost; RAM
parity mostly comes from a Node.js CLI dependency neither app's own UI code
controls.

**Disk**:

| | deskwarden | Bitwarden Desktop |
| --- | --- | --- |
| App itself | ~11 MB | 456 MB |
| Full install (app + bundled `bw` CLI) | 164 MB | 456 MB |

Source: ~11,800 lines of Rust across 36 modules.

## Dependencies

Everything deskwarden links against, and what it's for:

| Crate | What it's for |
| --- | --- |
| `windows` | Win32/WinRT bindings: UI Automation, `SendInput`, DPAPI, WinVerifyTrust (Authenticode), Job Objects, DWM, GDI, Shell, Windows Hello. |
| `eframe` / `egui` (glow backend) | The GUI: login, overlay, pickers, vault window. |
| `tray-icon`, `global-hotkey` | The system tray icon/menu and the global fill hotkey. |
| `ureq` | HTTP client — talks to the local `bw serve` bridge and, for real website favicons, the Bitwarden icon service (or a self-hosted server's own icon proxy). |
| `serde`, `serde_json` | (De)serializing everything that crosses the `bw serve` HTTP boundary. |
| `aes-gcm`, `sha2` | Sealing the master password under a Windows Hello signature-derived key for quick unlock. |
| `zeroize` | Wiping decrypted secrets (session tokens, the master password buffer) from memory after use. |
| `png` | Decoding fetched favicons to RGBA. |
| `directories` | Locating the standard Windows config/cache directories. |
| `semver` | Comparing versions for the update checker. |
| `log`, `env_logger` | The log file — this is a console-less GUI-subsystem binary, so the log file is the only diagnostic channel. |
| `mockito` (dev-only) | Mocking `bw serve`'s HTTP API in tests. |

Full versions and feature flags: [`deskwarden/Cargo.toml`](deskwarden/Cargo.toml).

## Repository layout

| Path | What's in it |
| --- | --- |
| [`deskwarden/`](deskwarden/) | The Rust crate: the app itself, plus its Inno Setup installer under `deskwarden/installer/`. |
| [`docs/`](docs/) | Design specs and implementation plans. |
| [`.github/workflows/`](.github/workflows/) | The release pipeline (build, package, publish on a `vX.Y.Z` tag). |

## Building from source

Requires a Rust toolchain and Windows 10 or 11:

```
cd deskwarden
cargo build --release
```

See [`deskwarden/installer/README.md`](deskwarden/installer/README.md) for how
the installer is built.

## License

MIT — see [LICENSE](LICENSE). (The same license also ships with the crate at
[`deskwarden/LICENSE`](deskwarden/LICENSE).)

---

<p align="center">
  <a href="https://star-history.com/#denis-platonov/deskwarden&Date">
    <img src="https://api.star-history.com/svg?repos=denis-platonov/deskwarden&type=Date" alt="Star History Chart" width="600">
  </a>
</p>

# Deskwarden

[![CI](https://github.com/denis-platonov/deskwarden/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/denis-platonov/deskwarden/actions/workflows/ci.yml)
[![Release](https://github.com/denis-platonov/deskwarden/actions/workflows/release.yml/badge.svg)](https://github.com/denis-platonov/deskwarden/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/denis-platonov/deskwarden?sort=semver)](https://github.com/denis-platonov/deskwarden/releases/latest)
[![License: AGPL v3](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE)


> **Unofficial and unaffiliated with Bitwarden.** This is an independent,
> community-built tool. It is not made by, endorsed by, supported by, or
> connected to Bitwarden, Inc. in any way. "Bitwarden" is a trademark of
> Bitwarden, Inc., used here only to describe what this tool interoperates
> with. If you have a problem with this tool, do not contact Bitwarden support
> — open an issue here instead.

**Deskwarden** fills credentials from your Bitwarden vault into **native
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

Deskwarden fills that one gap, nothing more: it watches which window has
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
- **Two vault backends, and you choose.** By default everything goes through
  the official Bitwarden CLI's local `bw serve` bridge: Deskwarden never
  touches encryption, key derivation or sync logic, and reads come from an
  in-memory snapshot of what the CLI returned. On a **self-hosted** server you
  can instead turn off *Use official bw for crypto* (Preferences -> Sync &
  account) and Deskwarden talks to your server itself -- no background CLI
  process, and it does the decryption itself. That path is faster and much
  lighter, and it means the key that unlocks your vault is kept on this PC
  under DPAPI and does not expire; the setting says so before you turn it on.
  **Signing in uses the CLI either way.** That snapshot is memory-only unless
  you turn on
  **"Keep an encrypted copy of your vault on this PC"** (off by default), which
  also writes it to a file encrypted under a key Windows Hello holds in this
  PC's TPM, so a copied disk cannot be read on another machine. That file
  survives the vault locking — it exists to survive a restart — and is deleted
  when you log out, when you are asked for your master password again, or after
  seven days.

## Stack, and why

| Choice | Reasoning |
| --- | --- |
| **Rust** | Native Win32/COM interop (UI Automation, DPAPI, WinTrust, Job Objects) without an FFI layer on top of a managed runtime, and a single static binary with no runtime to install. |
| **[`windows`](https://crates.io/crates/windows) crate (raw Win32/WinRT bindings)**, not a higher-level GUI-automation library | The two things that actually need OS-level access — reading whatever window is in the foreground, and typing into an arbitrary native control — don't have a portable abstraction worth building on. Direct bindings also make the Authenticode signature check on the bundled `bw.exe` (real `WinVerifyTrust`, not a shelled-out PowerShell call) and the DPAPI-encrypted session cache possible without another dependency. |
| **[`eframe`/`egui`](https://github.com/emilk/egui) (immediate-mode GUI)** | A tray app with a handful of small windows (login, an autofill overlay, a picker, the vault browser) doesn't need a retained-mode widget tree or a bundled browser engine — egui compiles into the same static binary and adds single-digit megabytes, not a WebView2 dependency. |
| **The Bitwarden CLI (`bw`) as the default vault backend**, via its local `bw serve` REST bridge — with a direct one beside it for self-hosted servers | Reimplementing Bitwarden's cryptography is a security liability a community tool should not take on lightly, so for a long time it was not taken on at all. It has been now, for self-hosted servers only and behind a setting that is off by default: `bw serve` is a bundled Node runtime costing ~118 MB of RAM, which is more than the rest of the app put together. The direct path is checked against Bitwarden's own published test vectors, and every write is laid over the JSON the server sent so a field this app cannot decrypt survives an edit untouched. On the default path Deskwarden still only ever talks to `localhost`. |
| **DPAPI** for the cached session token, **Windows Hello** (`KeyCredentialManager`) for optional quick-unlock | Both are already the OS's own answer to "encrypt this for the current Windows user" — no key management of Deskwarden's own to get wrong. |

## Size

Numbers from an actual release build (`cargo build --release`, with LTO +
single codegen unit + stripped symbols), measured live and side-by-side
against the official Bitwarden desktop app on the same machine — not vendor
figures, not estimates.

Deskwarden has two backends and two processes, so a single number would be
wrong four ways. What follows is private working set -- the figure Windows
Task Manager's "Memory" column shows -- measured on one machine, on 0.11.1.

**The tray, which is the state that lasts hours:**

| | Deskwarden's own process | `bw serve` beside it |
| --- | --- | --- |
| Default backend (official CLI) | ~10 MB | ~118 MB |
| Direct backend (self-hosted only) | ~21 MB | **none** |

The direct backend's own process is larger because it holds the vault and the
cryptography itself. It is still about a seventh of the pair it replaces.

**The vault window** is its own process and costs ~76 MB while open. It exits
when you close it, and that is the point: it is an accelerated-graphics window,
and the GPU driver never returns what it takes, so the only way to get that
memory back is for the process holding it to end. Everything that appears
during an autofill -- the fill prompt, the locked-vault card, the save-login
card, the generator, the unlock prompt -- is drawn directly by Windows instead,
costing under 2 MB each and loading no driver at all.

**Against Bitwarden Desktop**, measured side by side on the same machine: it
sits at 132-135 MB across its four processes. Deskwarden on the default backend
is close to parity once the CLI is counted -- which it has to be, because it is
what does the vault work. On the direct backend, idling in the tray, it is
~21 MB against ~132 MB.

If idle RAM matters and you are on the default backend, `bw serve` can be shut
down when nothing needs it: Preferences -> Sync & account -> "Keep the
Bitwarden backend running". Autofill stays instant because vault data is served
from an in-memory cache. Turning it off used to mean the first operation after
a restart paid the backend's ~8 s cold start; with the encrypted disk copy on as
well it does not, because the snapshot is already there. The two settings were
built for each other.
**Disk**:

| | Deskwarden | Bitwarden Desktop |
| --- | --- | --- |
| App itself | ~16 MB | 456 MB |
| Full install (app + bundled `bw` CLI) | ~169 MB | 456 MB |

Source: ~188,000 lines of Rust across 103 modules, roughly a third of it
production code and the rest tests.

## Dependencies

Everything Deskwarden links against, and what it's for:

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
cd Deskwarden
cargo build --release
```

See [`deskwarden/installer/README.md`](deskwarden/installer/README.md) for how
the installer is built.

## Privacy

Deskwarden has no servers, no accounts and no analytics. What it reads, what
it stores, the four network requests it makes and which of them you can turn
off are set out in [PRIVACY.md](PRIVACY.md).

Changes between releases are in [CHANGELOG.md](CHANGELOG.md).

## License

AGPL-3.0-or-later — see [LICENSE](LICENSE). (The same license also ships with the crate at
[`deskwarden/LICENSE`](deskwarden/LICENSE).)

---

<p align="center">
  <a href="https://star-history.com/#denis-platonov/deskwarden&Date">
    <img src="https://api.star-history.com/svg?repos=denis-platonov/deskwarden&type=Date" alt="Star History Chart" width="600">
  </a>
</p>

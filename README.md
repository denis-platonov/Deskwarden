# deskwarden

> **Unofficial and unaffiliated with Bitwarden.** This is an independent,
> community-built tool. It is not made by, endorsed by, supported by, or
> connected to Bitwarden, Inc. in any way. "Bitwarden" is a trademark of
> Bitwarden, Inc., used here only to describe what this tool interoperates
> with. If you have a problem with this tool, do not contact Bitwarden support
> — open an issue here instead.

**deskwarden** fills credentials from your Bitwarden vault into **native
Windows applications** — the kind of desktop app or game launcher a browser
extension can't reach. It sits in the system tray, watches which window comes
to the foreground, matches its process name against your vault, and types the
matching username and password into it. Vault access goes entirely through the
official Bitwarden CLI's local `bw serve` bridge; deskwarden never touches your
vault's encryption.

- **[Download the latest release](https://github.com/denis-platonov/deskwarden/releases/latest)**
  — a per-user Windows installer, no admin rights required.
- **[Full documentation](deskwarden/README.md)** — requirements, trigger modes,
  self-hosted servers, troubleshooting, and security notes.

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

# deskwarden

> **Unofficial and unaffiliated with Bitwarden.**
> This is an independent, community-built tool. It is not made by, endorsed by,
> supported by, or connected to Bitwarden, Inc. in any way. "Bitwarden" is a
> trademark of Bitwarden, Inc., used here only to describe what this tool
> interoperates with. If you have a problem with this tool, do not contact
> Bitwarden support — open an issue here instead.

`deskwarden` fills credentials into **native Windows applications** —
the kind of desktop app or game launcher a browser extension can't reach. It
runs quietly in the system tray, watches which window comes to the foreground,
matches its process name against your vault items, and types the matching
username and password into the app using UI Automation (with a simulated
keystroke fallback for windows that don't expose a usable automation tree). It
never touches your vault's encryption: all vault access goes through the
official Bitwarden CLI's local `bw serve` HTTP bridge, running as a separate
process on your own machine.

## Requirements

- Windows 10 or 11.
- The [Bitwarden CLI](https://bitwarden.com/help/cli/) (`bw`) installed and on
  your `PATH`. Check with `bw --version`.
- A Rust toolchain, if you're building from source.

## Running it

```
cargo run --release
```

On first launch you'll be asked to log in and unlock your vault. The session
token is then cached locally, protected with Windows DPAPI (so only your
Windows user account can read it), and re-verified on every start — if it has
gone stale, you're asked to unlock again rather than the app silently doing
nothing.

Once running, it lives in the system tray:

- **Add app...** — a two-step picker. First choose the vault item whose
  credentials you want to use, then choose the running process to attach it to
  and pick a trigger mode.
- **Quit** — exits and shuts down the `bw serve` bridge.

### Trigger modes

Each app match has one of three trigger modes:

| Mode | Behaviour |
| --- | --- |
| `prompt` | A small overlay appears offering to fill. Nothing happens unless you click Fill. |
| `hotkey` | Nothing happens automatically; press **Ctrl+Alt+B** while the matched window is focused to fill. |
| `auto` | Fills as soon as the matched window comes to the foreground. |

App matches are stored in your vault, in a custom field named
`deskwarden:app-match` on the item they belong to, so they sync between
machines like any other vault data.

## Official and self-hosted servers

Both work. `deskwarden` doesn't talk to any server directly — it talks
to your local `bw` CLI, so whatever server that CLI is configured against is
what gets used. For an official Bitwarden account, just log in. For a
self-hosted server (Vaultwarden, deskwarden, or any Bitwarden-API-compatible
server), tick **Self-hosted server** on the login screen and enter its URL;
that runs the standard `bw config server <url>` for you. You can also configure
it yourself beforehand with the CLI and the app will follow.

## Troubleshooting

There's no console window, so everything is written to a log file:

```
%APPDATA%\deskwarden\deskwarden\config\deskwarden.log
```

Set `RUST_LOG=debug` before launching for more detail. Common cases the log
will tell you about: the `bw` CLI not being on `PATH`, a stale session, or
something already listening on port 8087 (usually an orphaned `bw serve` from
a previous run).

## Security notes

- Your master password is only ever passed to the `bw` CLI through an
  environment variable, never as a command-line argument (which would be
  visible to other processes), and its buffer is wiped after use.
- The cached session token is encrypted at rest with DPAPI, and decrypted
  copies are wiped from memory after use.
- `bw serve` binds only to localhost and is placed in a Windows job object that
  terminates it if this app exits for any reason — including a crash — so an
  unlocked vault is never left served in the background.
- Matching is by process name only. A deliberately renamed executable can
  therefore impersonate a matched app; this is a known limitation.

## License

MIT — see [LICENSE](LICENSE).

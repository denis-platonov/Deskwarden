# Deskwarden

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
never touches your vault's encryption: credentials originate only from the
official Bitwarden CLI's local `bw serve` HTTP bridge, running as a separate
process on your own machine, and all writes and sync go through it; reads are
served from an in-memory snapshot of what the CLI returned. That snapshot is
memory-only unless you turn on the encrypted disk copy described under
**Security notes** below, which is off by default.

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

- **Left-click the tray icon** (or right-click → **Open Vault**) — opens the
  vault window: folders, search, item create/edit/delete, live TOTP codes, a
  password-strength indicator, real website favicons, and manual/auto sync.
  Full CRUD is Login-item only; Cards/Secure notes/Identities show up in the
  list and counts but aren't editable here yet.
- **Add app...** — a two-step picker. First choose the vault item whose
  credentials you want to use, then choose the running process to attach it to
  and pick a trigger mode. You can also do this from the vault window's
  **Fill in app** button on a login item that's already matched.
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
self-hosted server (Vaultwarden, nodewarden, or any Bitwarden-API-compatible
server), tick **Self-hosted server** on the login screen and enter its URL;
that runs the standard `bw config server <url>` for you. You can also configure
it yourself beforehand with the CLI and the app will follow.

## Troubleshooting

There's no console window, so everything is written to a log file:

```
%APPDATA%\Deskwarden\Deskwarden\config\deskwarden.log
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
- While the vault is unlocked, deskwarden holds an in-memory snapshot of your
  vault items, so it can serve reads without `bw serve` running. Each item's
  password and TOTP seed are wrapped so they are zeroized on every drop of
  that snapshot — the vault locking, the app quitting, and every intermediate
  clone the app makes along the way, not just one designated copy. Other
  fields Bitwarden's CLI sends inside a login, including password history and
  notes, are not individually modeled and are not zeroized.
- That zeroizing only covers the cached snapshot itself. Plaintext still
  passes through copies this app does not control: the value typed into a
  matched app, an item open for editing (and egui's own text-field state
  while it's open), a revealed password on screen (egui's text-rendering
  cache), the Windows clipboard after "Copy password", and the JSON buffers
  built to send a write to `bw serve` or to parse its response to a vault
  read (which returns your whole vault in one payload). None of these are
  wiped after use, and the memory they occupied can persist until the process
  exits.
- **Off by default:** "Keep an encrypted copy of your vault on this PC"
  (Preferences → General) also writes that snapshot to `vault-cache.bin`, in
  the active account's own directory beside its `session.bin` and `hello.bin`.
  The file holds your usernames, passwords, notes and two-factor secrets. A
  random content key encrypts it with AES-256-GCM; that key is sealed under a
  key derived from a Windows Hello signature, whose private half lives in this
  PC's TPM, and the whole thing is DPAPI-wrapped for this Windows user as
  `session.bin` and `hello.bin` are. The TPM binding is what DPAPI alone
  cannot give: a copied disk plus your Windows account password yields the
  file's header and two ciphertexts and nothing else. Anything running as you
  on this PC that can pass Windows Hello can read it. It is **not** deleted
  when the vault locks — surviving a restart is what it is for — and is
  deleted on log out, on any master-password re-prompt, and when the setting
  is turned off. It is refused and deleted after seven days, on a change of
  account, or if it cannot be opened. The setting is unavailable without
  Windows Hello, and no weaker file is written instead.
- `bw serve` binds only to localhost and is placed in a Windows job object that
  terminates it if this app exits for any reason — including a crash — so an
  unlocked vault is never left served in the background.
- Matching is by process name only. A deliberately renamed executable can
  therefore impersonate a matched app; this is a known limitation.

## License

MIT — see [LICENSE](LICENSE).

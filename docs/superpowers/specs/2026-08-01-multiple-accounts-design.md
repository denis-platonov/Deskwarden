# Multiple accounts — design

**Status:** approved, ready for planning
**Date:** 2026-08-01

## Goal

Let a user configure several Bitwarden accounts and switch between them. Switching
switches everything: the vault list, the detail pane, autofill, the match engine,
and the underlying `bw` CLI.

## The decisions, made and settled

Recorded so they are not re-derived.

**One active account at a time.** Switching accounts switches the view. There is no
merged "all vaults" list.

**Autofill follows the active account.** The match engine, the hotkey picker and the
prompt overlay all serve whichever account is selected. This is how the browser
extension behaves, and it is what keeps the feature small.

**Switching prompts for the master password.** No long-lived session tokens are kept
for accounts that are not active. The cost is a prompt per switch; the benefit is that
an idle account holds no usable credential.

**Rejected: concurrent accounts.** Loading every account's vault at once means one
`bw serve` per account (each ~111 MB at idle — the reason `keep_backend_running`
exists at all), N caches, N match engines, and a rule for which account wins when two
both match `notepad.exe`. That last question has no obvious right answer, and the
concurrency machinery around a *single* vault took fifteen consecutive review findings
to get right. Not worth it for a feature whose UI shows one account anyway.

## Why this is smaller than it looks

The hard part already exists. `main.rs` already has the sequence:

    stop `bw serve` → re-authenticate → start `bw serve` → wait for readiness →
    `cache.clear()` → populate → rebuild the match engine → reconcile the backend policy

It exists because a re-authentication has always been able to land a *different*
account, and `VaultCache`'s era/epoch machinery was built so a previous account's items
can never render after one. Switching accounts is that same sequence with a different
data directory. **This feature should reuse it, not reimplement it.** If an
implementation finds itself writing a second teardown-and-repopulate path, that is the
signal it has gone wrong.

## Architecture

### 1. One profile directory per account

The `bw` CLI holds exactly one active account per data directory — there is no `switch`
command and every command resolves through a single `activeAccount$`. But the data
directory is selectable, so each account gets its own:

    %APPDATA%\Deskwarden\Deskwarden\accounts\<account-id>\

`bw serve` and every `bw` invocation for that account run with
`BITWARDENCLI_APPDATA_DIR` pointing at it. Each directory keeps its own logged-in
account, so switching needs only an **unlock**, never a fresh `bw login` with email,
password and 2FA.

`<account-id>` is a locally generated opaque id, not the email. The directory name
should not disclose whose vault it is.

**A trap that will silently break this.** The CLI resolves its data directory as:

```ts
if (fs.existsSync(relativeDataDir)) { p = relativeDataDir; }        // <-- FIRST
else if (process.env.BITWARDENCLI_APPDATA_DIR) { ... }
```

`relativeDataDir` is a `bitwarden-cli` directory beside the executable. If one exists —
a portable install, or a leftover — **our environment variable is ignored and every
account shares one profile**, which presents as switching that appears to work and then
doesn't stick, with no error anywhere. The implementation must detect this at startup
and refuse to offer multi-account rather than corrupting one profile with another
account's state.

### 2. What a switch does

1. Stop the current `bw serve` (the existing job-object teardown).
2. Point the next spawn at the target account's data directory.
3. Prompt for that account's master password (or Windows Hello, if enrolled — see §4).
4. Start `bw serve`, wait for readiness.
5. `cache.clear()`, populate, rebuild the match engine, reconcile the backend policy.

Steps 1 and 5 are the existing lock/re-auth recovery. Step 2 is the only genuinely new
behaviour.

**The era machinery is what makes this safe** and must not be bypassed: `clear()` bumps
the era, `snapshot_unless_superseded` refuses a read whose era has moved, and the
pending-write log is emptied. A populate for the previous account that is still in
flight when the switch lands is discarded rather than painting one account's items under
another's chrome. That is not a new guarantee to build; it is an existing one to route
through.

### 3. Where accounts are stored

A list on `Settings` — id, display email, server URL, data directory. **No secrets.**
`#[serde(default)]` on the struct already lets an older `settings.json` parse, and
`persist_preferences` destructures `self`, so adding a field is a compile error until
its owner is declared. The account list is owned by the account code, not by the
preferences window.

Which account was last active is persisted so a restart resumes it.

### 4. Session tokens and quick unlock

Today there is one `session.bin` (DPAPI-wrapped) and one `hello.bin`. Both become
per-account, under that account's directory.

- The **active** account keeps its session token, so restarting the app resumes it
  without a prompt — today's behaviour, preserved.
- Switching **discards** the outgoing account's token. An account you are not using
  holds no usable credential.
- Windows Hello quick unlock is per-account: enrolling for one account must not
  unlock another. `hello.rs` currently uses a single credential named
  `deskwarden-quick-unlock`; per-account blobs need per-account key derivation, which
  the existing pattern already supports through its domain-separation label. **It must
  never call `RequestCreateAsync(ReplaceExisting)`** — that would silently rotate the
  shared credential and destroy every other account's enrollment.

### 5. UI

**Switcher in the vault window**, in the title bar beside the existing account avatar —
which is where 3e's "Sync & account" section and 2b's avatar already point. A tray entry
so an account can be switched without opening the window.

**Adding an account** runs the existing login flow against a fresh data directory. The
login window already exists and already handles 2FA; it needs to be told where to run.

**Removing an account** runs `bw logout` in that directory and deletes it, and must
delete the per-account `session.bin` and `hello.bin` with it — the same reasoning
`login_ui`'s existing log-out handler already applies when it calls `hello::unenroll()`:
a sealed credential for an account the CLI no longer knows is a liability, not a feature.

## Error handling

Every failure falls back to the state before the switch was attempted. A half-switched
app — new data directory, old cache — is the one outcome that must not be reachable.

- **Wrong master password**: stay on the current account, current vault intact.
- **`bw serve` fails to start for the target**: report it, return to the previous
  account, restart its backend. Note the existing `try_start_backend` failure path calls
  `fatal_startup_error` in one place; a switch must not inherit that — killing the app
  because the *other* account's backend would not start is not acceptable.
- **The target account's session is invalid** (password changed elsewhere): treat as a
  normal re-authentication, which the app already handles.
- **`relativeDataDir` detected**: multi-account is unavailable; say why, plainly, and
  keep working as a single-account app.

## Testing

- The account list round-trips through `settings.json`, and an older file without it
  still parses (the existing partial-file test must keep passing).
- A switch clears the cache and rebuilds the match engine — assert the previous
  account's matches are gone, not merely that the new ones are present. This is the
  property the era machinery exists for and the one with the worst failure mode.
- A populate in flight across a switch is discarded (the existing era tests are the
  model).
- A failed switch leaves the previous account fully working: cache populated, engine
  armed, backend up.
- The `relativeDataDir` detection, since it is the difference between working and
  silently sharing one profile.
- Per-account `session.bin` and `hello.bin` paths never collide.

## Out of scope

- Concurrent accounts and cross-account autofill (see the rejected decision above).
- Organisations and collections.
- Moving items between accounts.
- Any change to how autofill matches, injects, or arbitrates — with one active account
  there is nothing to arbitrate.

## Risk

The dominant one: **a switch that half-lands.** The existing lock/re-auth path is the
only sequence in this app that has been hardened against exactly this, across fifteen
findings. The temptation will be to write a fresh, simpler switch path because the
existing one is entangled with re-authentication. That would be a second implementation
of the hardest code in the codebase, and it would not have the tests.

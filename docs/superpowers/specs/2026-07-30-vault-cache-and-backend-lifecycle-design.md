# Vault cache and `bw serve` lifecycle — design

**Status:** approved, ready for planning
**Date:** 2026-07-30

## Goal

Let the user decide whether deskwarden trades idle memory for speed, and make
the memory-saving choice cost almost nothing perceptible.

Today `bw serve` must be running for *every* read, so it runs permanently.
Measured on a real 1657-item vault: `bw serve` holds **111 MB** private
working set at idle, against deskwarden's own **39 MB**. The backend is
roughly three quarters of the tray's entire footprint, and none of it is UI.

## Background

Three measurements shape this design. All were taken against the live app,
not estimated:

- `bw serve` idle: **111 MB** (cold, shortly after start: ~76 MB — it grows as
  it serves).
- `GET /list/object/items`: **1.14 s and 1.08 MB** cold, ~60 ms warm, for 1657
  items — plus deserialisation.
- `bw serve` spawn to ready: **~8 s** (from the app's own log:
  `19:44:26 bw sync completed` → `19:44:34 bw serve ready after 1 retry`).

That ~8 s is why the backend cannot simply be started on demand for autofill:
focusing a matched game launcher and waiting 8 s before anything is typed
would make the app's headline feature feel broken, not slow.

## Architecture

### 1. `vault_cache` — an in-memory snapshot

A new module owning items and folders in memory, populated once per unlock
through the existing `VaultBridge`, and read by everything that does an
HTTP read today.

It is a cache *in front of* `VaultBridge`, not a replacement for it. It
performs no decryption, implements no sync logic, and writes nothing to
disk. Every credential still originates from the official CLI.

**Memory only.** The snapshot is dropped on lock and on quit. This preserves
the property `main()` already maintains deliberately — it `drop()`s the
fetched vault after building match entries specifically so idle memory holds
no vault contents. The cache changes how *long* decrypted items are held
while unlocked; it must not change the fact that idle holds none.

A disk cache was considered and rejected. It would make the first open after
a restart instant, but it puts decrypted vault data at rest, which
contradicts the README's claim that deskwarden "never touches encryption,
key derivation, or sync logic itself". DPAPI would protect it as
`session.bin` is protected, so this is a posture decision rather than a
crypto one — but the claim is worth keeping true as written.

### 2. `bw serve` becomes a sync/write backend

With the cache in place the backend is needed for exactly four things:

1. Populating the cache at unlock
2. `bw sync`
3. Item and folder writes
4. TOTP codes (`GET /object/totp/{id}` — the CLI generates them)

Reads — the vault window's list and detail panes, and the autofill match
path — never touch it.

### 3. Lifecycle policy

One setting, defaulting to today's behaviour:

| | Backend | Autofill | Window opens | TOTP / write / sync | Idle |
|---|---|---|---|---|---|
| **Keep running** (default) | always | instant | instant | instant | ~111 MB |
| **Save memory** | warmed on window open, down on close | instant | instant | usually instant; spinner if the user outruns the warm-up | ~0 |

In "save memory" mode the backend is started **on a background thread the
moment the vault window opens**, overlapping with the window painting and
the user orienting themselves. Because the window renders from cache it does
not wait for the backend at all. By the time the user has searched, navigated
and selected something that needs it, the backend is very likely ready. If
they outrun it, the affected control shows a spinner — not the window.

The rule is *while the vault window is open*, not literally per operation.
TOTP polls once a second and writes are frequent while the window is open, so
tearing the backend down between them would be pathological. Once the window
closes nothing needs it, and idle — the state that lasts hours — costs
nothing.

### 4. `settings` — persisted preferences

A serde struct saved as `settings.json` in the config directory, following
`fill-stats.json`'s existing load/save pattern. A missing file, missing
field, or unparseable file falls back to defaults rather than failing
startup; a settings file is never a reason the app cannot run.

### 5. Preferences window

Built to design 3e, whose layout is already specified and whose `toggle_pill`
widget already exists in `theme.rs`.

Scoped to a **General** section containing:

- the backend lifecycle toggle, with a description stating the trade plainly
- the auto-lock timeout, because `AUTO_LOCK_TIMEOUT` is already marked in
  code as "hardcoded until the 3e preferences window exists"

3e's other six sections (Autofill, Native apps, Security, Shortcuts, Sync &
account, About) stay unbuilt until they have real settings behind them.

## Data flow

**Unlock** → start backend → populate cache → if "save memory", stop backend.

**Vault window opens** → render from cache immediately; if "save memory",
start backend on a background thread in parallel.

**Read** (list, search, detail) → cache. Never touches the backend.

**Autofill** → cache. Never touches the backend. Works with it fully down.

**Write** (create/edit/delete item or folder) → await backend if not ready →
`VaultBridge` → update cache on success.

**TOTP** → await backend if not ready → `VaultBridge`.

**Sync** → await backend → `bw sync` → repopulate cache.

**Vault window closes** → if "save memory", stop backend.

**Lock / quit** → drop the cache, stop the backend.

## Error handling

- **Backend fails to start** — surface on the control that needed it, not as
  a modal. Reads keep working from cache, so the window stays usable.
- **A write fails** — the cache is not updated, so it continues to reflect
  the server rather than an optimistic guess.
- **Settings file unreadable** — fall back to defaults and log; never fatal.
- **Cache empty when a read arrives** (a state that should not occur while
  unlocked) — fall back to a direct `VaultBridge` read rather than showing an
  empty vault, and log it as a bug signal.

## Staleness — the main risk

The cache introduces a class of bug that does not exist today: a write that
does not update the cache leaves the window showing something the vault no
longer contains. This is the single most likely way to get this wrong.

Mitigation: **all writes route through the cache module.** No call site
performs a write directly against `VaultBridge` and separately remembers to
update the cache — there is exactly one place that can be wrong, and it is
covered by tests.

## Testing

- Cache: population, read-through, invalidation on lock, and **empty after
  lock** (asserting the idle-holds-nothing property directly).
- Writes: each write path leaves the cache consistent with what was written;
  a failed write leaves it unchanged.
- Settings: round-trip, missing file, missing field, malformed file — all
  yield usable defaults.
- Lifecycle policy: a pure function of (setting, window open, unlocked) →
  should the backend run. Table-tested rather than verified by hand.

## Out of scope

- The single-window refactor, and the "keep the UI window alive" setting that
  depends on it. That toggle is worth ~8.5 MB against this one's ~111 MB, and
  it slots into the preferences window once it exists.
- The other six 3e preference sections.
- Any change to how credentials are decrypted, synced, or stored.

## Follow-up noted during design

The README states `bw serve` uses 76 MB when tray-only. That was a *cold*
measurement; steady state after serving a real vault is ~111 MB. The figure
should be corrected or labelled as cold — it is not wrong for the moment it
described, but it is not what a user sees after a day in the tray.

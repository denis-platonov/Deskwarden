# The daemon owns the vault, and the window is its first client

**Status:** designed 2026-09-26; built in slices on the branch this note
lands on. Supersedes the "each process holds its own vault" state that
`2026-08-23-daemon-and-ui-processes-design.md` left the direct-REST account
in, and takes the shape `2026-08-27-one-door-to-the-vault.md` asked for:
**one door, and every app is a client of it** -- with the door inside the
daemon rather than in a separate service process.

The owner: "back-end should own it - whether cached or no - doesn't matter
and both clients get it from there like we had bw cli".

## Why

A launch that opened the window read the whole vault twice on the server:
once in the daemon (for the match engine, the hotkey and autofill) and once
in the `--ui` window process, which built a `RestBackend` of its own. On the
owner's D1-backed server each read is ~1,700 rows. `bw serve` never had this
problem: the daemon ran it and the window was an HTTP client of it. This
design puts that shape back, with the daemon itself answering.

## The model

- **The daemon holds the one decrypted vault** -- the `VaultCache` it already
  has, over the backend it already settles. It is the only process that
  talks to the server about vault data.
- **The daemon answers a local API** on `127.0.0.1`, an ephemeral port. The
  routes are `bw serve`'s own (`/list/object/items`, `/object/item/{id}`,
  `/restore/item/{id}`, `/object/folder`, `/object/totp/{id}`, `/sync`, ...),
  answered out of the daemon's cache, plus one route `bw serve` never had:
  **`GET /changes`**, a long-poll change feed. Using `bw serve`'s protocol is
  deliberate: `VaultBridge` is already a complete, tested client of it, and a
  second protocol would be a second thing for every write to be wrong in.
- **The window is a client.** Its vault backend is `DaemonBackend` -- a
  `VaultBridge` pointed at the daemon, with the window's credential on every
  request -- inside the same `VaultCache` it has always had. Nothing in the
  window's frame code changes: it still loads, writes, and applies
  announcements through the cache.

### Reads

Answered from the daemon's snapshot with no server round trip. A daemon whose
vault is not loaded yet answers `503`; the window's existing readiness wait
retries through its existing spinner, and never falls back to a read of its
own. The window's Trash and Archive lists are passed through to the daemon's
backend (they were never cached, by design -- see `VaultCache::
list_trash_unless_superseded`).

### Writes

Every write the window makes -- create, edit, favourite, move, app match,
trash, restore, archive, purge, folders -- is a `bw serve`-shaped request the
daemon answers by calling **its own** `VaultCache` write, which writes to the
server and through to the daemon's snapshot. So the daemon's vault stays the
single truth, and the window's copy is updated from the server's answer
exactly as before. `bw serve` answers restore and unarchive on one route; the
daemon reads the item by id (one row) to tell which one it is.

### Changes

The notifications hub runs **in the daemon only**. A worker in the daemon
holds the listener, applies each settled batch to the daemon's cache with the
same rule the window used (`targeted_changes`: one record per announced
cipher, a whole re-read only for what cannot be targeted), and only then
publishes the batch to the change feed. The window's `HubListener` follows
the feed instead of a websocket, so the frame's heartbeat is untouched: an
announced item is read back **from the daemon**, which already holds it --
moved to the top of the list, as before -- and a whole-vault announcement
becomes the window's ordinary sync, which is `POST /sync` to the daemon.

`POST /sync` always asks the server, and always the cheap question first: the
account's revision, one row. Only a revision that moved is followed by a
whole read. The revision the daemon's copy stands at is the last whole read's,
unless the hub worker has since applied changes one record at a time -- then it
is the revision the worker read just BEFORE applying them, so a change landing
in between reads as a move rather than as held.

After each applied batch the daemon's own loop rebuilds its match engine from
the cache (one rebuild per settled burst), so a pushed app match is armed for
autofill without a reload. The sharing directory (organisations, collections,
rosters) the daemon's read built is served as `GET /organizations`, and a
window holds it with the vault it loads.

Send pushes (12/13/14) are a separate kind of change on the same feed, and
the window's answer to them is to drop its Sends list. They never reach the
vault inbox, so they can never cost a vault read. A feed page carries whether
the daemon's hub is live, so the window's polling fallback still stands down
only while pushes can actually arrive, and a window that polls asks the
daemon to re-check rather than reading the server itself.

### What stays in the window, and why

- **Sends, Send receive and export** keep using the window's own stored
  credential (`userkey.bin`, DPAPI-wrapped, as today). None of them reads the
  vault any more: a Sends list is the Sends route plus the profile route (a
  handful of rows), a receive is anonymous, and an export is the user asking
  for the server's own ciphertext, which the daemon's decrypted snapshot
  cannot produce. Routing them through the daemon would add a hop and hold
  nothing extra.
- **TOTP** is computed from the item's seed as before; a seed this app cannot
  read is asked of the daemon, which reads that one record.
- **Password generation** is local (`password_gen`), as on the direct-REST
  backend it already was.
- **Favicons** are not vault data and are unchanged.

## The API is client-agnostic, and the transport is separate

Three layers, so that a second kind of client can use the first two:

1. **`vault_api`** -- a request (method, path, query, body, presented
   credential) in, a status and a JSON body out. No socket, no thread, no
   notion of "the window". It is the whole of what may be asked and what is
   answered, and it is where the tests are.
2. **`change_feed`** -- a sequence-numbered log of changes with a long-poll
   `wait_after(seq)`. A page says `reset` when the client is too far behind
   (or the daemon restarted), which a client answers with a whole reload.
3. **`vault_host`** -- the HTTP transport (tiny_http on loopback, a small
   worker pool so a long-poll does not block a read) and the daemon's hub
   worker. Nothing above it knows it exists.

## Credentials

- **Per client, per launch.** The daemon mints a 256-bit token
  (`service_token::mint`) when it starts a window, and hands it -- with the
  port -- down the child's **stdin**, one JSON line, then closes the pipe. It
  is never on the command line (`no_argument_of_a_ui_spawn_could_be_a_secret`
  still holds: the argv is the same two words) and never in the environment.
- The daemon keeps only a SHA-256 of each token, compared in constant time.
- **A grant is bound to the vault era it was minted in.** A lock, a logout
  and an account switch all `clear` the daemon's cache, which starts a new
  era -- so every outstanding grant stops working at that instant without any
  of those paths having to remember to revoke it. A window's grant is also
  revoked when the daemon reaps its process.
- This listener exists whether or not the user-facing local service
  (`service_enabled`, a separate process with API keys) is switched on, and it
  honours none of those keys: the only credentials it accepts are grants the
  daemon minted for a process it started.
- Nothing decrypted is written anywhere by any of this. The optional
  encrypted disk cache is unchanged and is still the only file.

## Lifecycle

- The daemon starts the listener before any window can be opened.
- A window whose grant is refused (a lock, a sign-out, an account switch) or
  whose daemon stops answering (a restart, which an update does -- a restarted
  daemon listens on another port and knows no old grant) says so and closes:
  its change-feed poll notices, on a `401` at once or after three polls in a
  row with no answer, and the window clears the clipboard if it holds a
  secret it copied, tells the user to open the vault again from the tray, and
  exits. It never falls back to reading the server itself. Relinking was
  rejected as the harder safe choice: the new daemon did not start this
  process and has no pipe to hand it a grant down.
- A window that has to sign in first (the first launch, or after a sign-out)
  keeps its line: its sign-in stores the key and puts the daemon's backend in
  its slot, its existing ring tells the daemon to adopt the key and make the
  one whole read, and its vault waits on its spinner for that read.
- A window with no link -- a `bw serve` account, or a daemon whose listener
  could not be bound -- behaves exactly as it did.

## Counts

Proved by request-counting tests against a mock server, through the real
`RestBackend`, the real listener on a real loopback port and the real
`DaemonBackend`:

- a launch with a window open: **one** `GET /api/sync`, the daemon's;
- an item edit from the window: **no** sync;
- a push reaching the window: **no** sync;
- a Sync with nothing changed: **no** sync, **one** revision question;
- a first launch that signs in in the window: **one** `GET /api/sync`, the
  daemon's.

## Future clients

A browser extension is the expected second client. It would plug in at layer
1 and layer 2 unchanged:

- **Transport.** Chrome reaches a local program through a native-messaging
  host: a process the browser starts, speaking length-prefixed JSON on stdio.
  That host would be a third transport beside HTTP -- a small program (or a
  mode of this binary) that turns each frame into a `vault_api` request,
  forwards it to the daemon, and relays `change_feed` pages as unsolicited
  messages. No route, body or feed shape changes.
- **Credentials.** A per-client grant exactly like the window's, minted by
  the daemon for that client. What it would still need, and this design does
  not build:
  - **pairing and approval** -- the daemon did not start the extension's host,
    so it cannot hand a token down a pipe it owns. A first-run pairing (the
    user approves the extension in the tray, the grant is persisted
    DPAPI-wrapped, revocable from Preferences) replaces that;
  - **scoped reads** -- an autofill client should be able to ask "the logins
    for this origin" and receive only those, never the whole list. The
    `service_keys` scope model (all / category / item) is the precedent; an
    origin scope is the addition, and the list route would filter by it the
    way `service_body::may_see` filters by category today;
  - **write policy** -- whether an extension may write at all, per grant.
- Nothing window-specific is in the protocol today: the window's only
  privileges are the grant it holds, and a grant says nothing about who holds
  it.

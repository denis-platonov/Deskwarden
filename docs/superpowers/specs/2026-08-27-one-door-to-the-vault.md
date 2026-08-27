# One door to the vault, and two apps that use it

**Status:** designed, not started. Supersedes the ownership model in
`2026-08-23-daemon-and-ui-processes-design.md` ("the daemon owns `bw serve`"),
the reader model in `2026-08-27-the-vault-lives-in-a-place-not-a-process.md`
("one writer, two readers of the file"), and unblocks
`2026-08-26-startup-window-in-its-own-process.md` by removing the question it
was stuck on. `2026-08-26-two-binaries-design.md`'s *shape* stands; what
changes is which process owns the vault.

## The model

**Everything reaches the vault through one door, and the door is REST or `bw`.**

- **The vault service** — the direct-REST client, or `bw serve` — is the single
  entry point. Nothing else opens a vault file, decrypts a cipher, or holds a
  snapshot on its own.
- **Two apps use it, independently**: the Win32 daemon (tray, hotkey, match
  engine, fill) and the vault window. Either runs without the other. Neither
  owns the other.
- **The cache lives behind the door**, not beside it. A read is
  *cache → service → server* or *service → server*, and which one is a
  setting. **No consumer ever reads the cache file directly.**

That last line is the whole difference from the previous design, and it is
what makes the rest work: with one door there is no question of which app
holds the vault, so there is no reason for either to be subordinate to the
other.

### What this deletes

The question `2026-08-26-startup-window-in-its-own-process.md` stalled on --
*does the window become a UI process, or does it fetch items itself?* -- stops
existing. Both apps are clients of the same service, so the window is not a
guest in the daemon's process and does not need the daemon to have started
anything first.

It also deletes the failure that broke that plan's third attempt on 2026-08-26:
the window came up before `bw serve` and had nothing to read. Under this model
the window's first request either reaches a running service or starts one; see
the lifecycle below.

## Lifecycle

**Reference-counted, and the count is held by the kernel.**

- **First app to need the vault starts the service.**
- **Last app to stop needing it exits the service.**
- **If both apps exit while the service is still running**, the next app to
  start **reconnects** to it.
- **If it cannot reconnect**, it restarts the service.

**Reconnect before restart, always.** A service that is up and answering is
worth more than a clean slate: restarting it costs the user a cold start
(~8 s on `bw serve`) and, on the direct backend, another Windows Hello prompt.

### The count must not be bookkeeping

A crashed app does not decrement anything. A design that counted clean exits
would leak: the service would stay up with nobody using it, forever, holding
the vault.

So **liveness is a kernel fact, not a number this app maintains** — a named
object per app whose handle the OS releases on process death, whether the exit
was clean, a crash, or a kill. `app_mutex::APP_MUTEX_NAME` is already this
crate's idiom for exactly that question, and `2026-08-23` already uses it for
"is Deskwarden running?" -- this is the same mechanism asked about two
processes instead of one.

**"Nobody is using it" must therefore be observable, not remembered.**

## The two settings

Both are per account, and one already exists.

| setting | values | today |
| --- | --- | --- |
| **which service** | official `bw` CLI, or direct REST | **exists** -- *Use official bw for crypto*, `Settings::use_official_bw_crypto`, ghosted off a self-hosted server |
| **which read path** | cache → service, or service only | **to add** |

The read-path setting is new, and it is not the same question as
`cache_vault_to_disk`. That one asks *may an encrypted copy exist on disk*;
this one asks *does a read consult it first*. They will usually move together
and are still two questions: a user may want the file to exist for a fast cold
start while a particular session reads through to the server for freshness.

**Whether they collapse into one row is a UI decision, not an architectural
one**, and it should be taken when the row is drawn rather than assumed here.

## What this means for the work already done

`per-item-cache-read` landed the version 2 cache file: a secret-free facts
section, one sealed blob per item, each bound to its id, with `open_item` and
a `clear()` that takes the content key away. **All of that stands** -- it is
what lets the service answer "one item" without decrypting a vault.

One piece is now in the wrong place. **`VaultCache::item_from_disk` is a
consumer-facing direct file read**, which this design forbids: a consumer must
ask the service. It should move behind the service boundary rather than being
called from `app.rs`. It is not wasted -- it is the mechanism the service will
use -- but its caller must change.

## What this costs

**The service becomes a thing that outlives both apps**, and that is new. It
can be running with no window and no tray, which is a state a user can observe
in Task Manager and will ask about. It needs a name they can recognise and a
reason it is there.

**Starting is racy and must be made not to be.** Two apps launching together
both find no service and both start one. Whatever wins that race must be
decided once -- the single-instance takeover already solves this shape for the
app itself and is the precedent, not a second scheme.

**`bw serve` gets the same lifecycle, and that accepts a real cost.**
Decided 2026-08-27, against keeping it as it is. Today the daemon owns it
through a kill-on-close job object, so the **kernel** guarantees it cannot
outlive the app; reference-counting it means moving that ownership to
whatever holds the count, and an orphaned `bw serve` then holds a session
token on a fixed port with no app running.

That is a genuine loss and is accepted for a genuine reason: two lifecycles
would mean "who owns the vault service" has two answers depending on a
setting, which is the split this whole design exists to remove. **One rule
is worth more than one guarantee**, provided the orphan window is bounded --
which it is, because the next app to start reconnects rather than leaving it
stranded, and because the existing auto-lock and the seven-day cache expiry
still apply to what it holds.

**It is bounded, not eliminated**, and an implementation must not pretend
otherwise: there is a window in which a process holding a session token is
running with no window and no tray. Whatever supervises the count is
responsible for closing it, and a test that only drives clean exits does not
show that it does.

**Two settings can express a configuration nobody wants** -- `bw serve` with a
cache-first read, say, where the cache is refreshed by a subprocess the user
asked not to keep running. The table above needs walking, one combination at a
time, before the second row is drawn.

## Testing

- **The door is the seam.** Every read goes through the service, so a test can
  drive both apps against a fake one without a vault, a file, or a socket.
- **A source pin that no consumer reads the cache file**: the file APIs are
  reachable only from the service module. This is `bw_serve_gate`'s idiom, and
  it is what stops the second door being reopened by a well-meaning edit.
- **Liveness is tested by killing, not by exiting.** A test that only asserts
  the clean path proves nothing about the case the design exists for.
- **Nothing here reaches the network, a real vault, or a real prompt.**

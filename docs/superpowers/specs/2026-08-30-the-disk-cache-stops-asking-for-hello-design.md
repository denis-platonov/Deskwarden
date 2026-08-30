# The Disk Cache Stops Asking For Hello

**The encrypted on-disk snapshot is sealed with DPAPI alone, and prompts for
nothing.**

## Why

Turning on **Keep an encrypted copy on this PC** made Deskwarden stop
starting. The owner met it as an app that simply did not launch: daemon
alive, no window, no further log line, main thread parked in
`WaitForSingleObject`. The installed 0.13.1 behaved identically, so this
predates the process split and is not a regression from it.

The cause is `vault_disk_cache.rs:1300`. The snapshot's content key is sealed
under a key derived from a **Windows Hello signature**, and taking that
signature is what puts the OS prompt on screen. When the prompt does not
appear -- and on this machine it did not -- startup waits for it forever.

That is two defects wearing one coat:

1. **Startup blocks on a modal prompt.** Wrong on its own terms, whatever the
   prompt is for. An app that cannot start and cannot say why is worse than
   one that starts degraded.
2. **It asks at all.** The owner's rule, and it is the right one: *"users
   should not know about how it works under the hood... if they say do not
   ask, do not ask, period."*

## The security argument, which is the part that matters

Removing a Hello gate looks like weakening something. Here it is not, and the
reason is in this app's own Preferences copy.

On a direct-REST account the master key lives in
[`crate::user_key_store`], **DPAPI-wrapped and never expiring**. The
built-in-client row says so to the user in as many words: *"the key that
unlocks your vault is kept on this PC, protected by Windows, and unlike a
session it never expires. Anyone who can run programs as you on this PC can
use it."*

So the disk cache -- a *derivative* of the vault -- was gated **more strictly
than the master key that opens the whole vault**. An attacker running as the
user takes the master key and never looks at the cache. The Hello gate
therefore protects nothing that is not already available without it, and its
entire practical effect is a prompt on every launch.

The owner's second observation completes it: **most people lock the PC, not
the app.** Win+L is the lock users actually reach for, which is why
`2026-08-29-the-lock-closes-the-window-design.md` exists. A per-launch Hello
prompt defends a threat model users do not hold, at a cost they meet every
day.

## The shape

`DiskCacheEnv::hello_key: fn() -> Result<Zeroizing<[u8; 32]>, String>`
becomes a key that is **derived without any UI**, and the field is renamed so
nothing reads as a Hello dependency any more.

DPAPI protects data rather than yielding a key, and its output is not
deterministic, so the key is one this app stores: **32 random bytes,
DPAPI-wrapped, in a file beside the cache** -- exactly the shape
`user_key_store` already uses for something more valuable. First use mints
it; later reads unwrap it.

The seam gains the directory it needs (`fn(&Path) -> ...`); the one call site
already holds `self.paths`.

**The file format does not change.** The whole file stays DPAPI-wrapped, the
header stays plaintext inside that envelope as additional authenticated data,
and the content key stays sealed -- under a key that no longer requires a
human. Existing cache files cannot be read with the new key and are discarded
and rebuilt, which is what a cache is for.

## What this is not

- **Not removing Windows Hello from the app.** Quick unlock
  (`hello::enroll_for`) is a separate feature the user opts into, and it
  keeps its prompt, because there the prompt IS the feature.
- **Not a change to the master key.** `user_key_store` is untouched.
- **Not a licence to prompt elsewhere.** The rule this establishes is that
  nothing on a startup path may block on UI.

## How it will be known to work

- **A test that the production env cannot reach Hello**: the disk cache's
  key function is compared against the Hello one by address, the way
  `export_wiring` compares its seams, so a future edit that points it back
  fails rather than prompting.
- **A test that a missing key file mints one**, and that a second read
  returns the same key -- otherwise every launch silently rebuilds the cache
  and the feature buys nothing.
- **A live check, which is the only thing that settles it**: turn the setting
  on, restart, and confirm the app starts with no prompt and the vault is on
  screen in milliseconds rather than seconds. That is the check the current
  code fails.

## Status

Design, approved 2026-08-30. Not implemented: it is a change to how the
vault's on-disk copy is sealed, and it was specified at the end of a long
session rather than built in one. **The setting is off on the owner's machine
and must stay off until this lands** -- with it on, the app does not start.

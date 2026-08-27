# The vault lives in a place, not a process

**Status:** designed, not started. Supersedes the parts of
`2026-08-26-two-binaries-design.md` and
`2026-08-23-daemon-and-ui-processes-design.md` that assume the daemon owns the
vault and the UI asks it for things. Their conclusions about *processes* stand;
what changes is where the data is.

## Three failures on 2026-08-26, and they were one failure

Each was chased separately and fixed or reverted separately. They are the same
defect seen from three sides.

**The daemon holds every password you own, in plaintext, for the whole
session.** `VaultCache::snapshot` is a `Vec<VaultItem>` and
`LoginData::password` is a `Zeroizing<String>` — so 1,666 decrypted passwords
sit in the process that owns the tray, the hotkey and the match engine.
`clear()` empties it on lock, but the owner's settings are
`auto_lock_enabled: false` with a 999-minute timeout, so in practice it is
"until you quit".

**The vault window cannot open unless the daemon's backend is up.** Making the
startup window a UI process — which every automated check passed — produced
this, in the user's hands:

> Your vault could not be loaded — the vault backend did not become ready
> within the deadline; last error: `http://localhost:8087/list/object/items:
> connection timed out`

The child had nothing to read, because the only copy of the vault was in
another process's memory, behind a backend that had not started.

**The daemon pays ~50 MB of graphics driver, permanently, on every
hand-launch.** Because it draws windows. Because that is where the data is.

**One root: the vault lives in a *process* rather than in a *place*.** Every
consumer therefore has to be that process, or talk to it. Move the vault to a
place and both processes become readers of it, which is what the owner
proposed at the outset — *"REST with cache (on disk) separately, tray and UI
consume it"* — and what three days of work has arrived at the long way round.

## The substrate already exists, and is stronger than the thing it replaces

`vault_disk_cache` is not a sketch. Today, behind
`Settings::cache_vault_to_disk` (off by default):

- a random 32-byte content key encrypts the snapshot with **AES-256-GCM**, the
  plaintext header as additional authenticated data;
- that content key is sealed under a key derived from a **Windows Hello**
  signature, whose private key lives in this machine's **TPM**;
- the whole thing is **DPAPI-wrapped**;
- the header is plaintext *inside* the DPAPI envelope on purpose, so an
  expired or foreign-account file is deleted without ever popping a Hello
  prompt for a file about to be thrown away;
- it is deleted on lock, on logout, on re-auth, and after seven days.

**A stolen or imaged disk plus the Windows account password yields a header and
two ciphertexts.** DPAPI alone cannot make that claim, because DPAPI derives
from credentials that travel with the image.

Compare what it would replace: plaintext in RAM, for as long as the user stays
unlocked, in a process that lives for days. **This is a security improvement
before it is anything else**, and that is the argument for it — not the memory.

## The shape

**One writer, two readers, no IPC.**

- **The writer** is the daemon, through whichever backend the account uses. It
  is the only thing that syncs, and it is the only thing that writes the file.
  Nothing else may, and one writer is the whole of the coherency story.
- **The readers** are the tray and the vault window. Each opens the file
  itself, under the same user's credentials, and takes **only the projection it
  needs**. There is no pipe, no protocol and no rendezvous — the same finding
  `2026-08-23` made about settings and the session token, applied to the vault.

### The projections are the point

Reading the whole snapshot into both processes would move the problem rather
than fix it. So:

| reader | needs | must never hold |
| --- | --- | --- |
| tray / daemon | the app-match records (**5** on the owner's vault, against 1,666 items), plus name / username / URI metadata for the account picker and search | any password, TOTP seed, note or card number |
| vault window | everything, while it is open | — (it exits, and takes it with it) |

The owner's framing — *"it only needs app type records"* — is exactly right for
the fill path, and the metadata line is what keeps the picker and *Search
vault* working. Both already have precedent in this crate:

- **`app::SEARCH_CORPUS` holds no secrets today** and says so in its own
  comment. The corpus is proof the projection idea works.
- **The fill already fetches by id.** `app.rs:444` calls
  `cache.bridge().get_item(item_id)` on a cache miss. Today that is the *miss*
  arm; under this design it is the only arm, and the cached password becomes
  unnecessary rather than merely undesirable.

### What this fixes, by construction rather than by fix

- The window opens against a **file**, not a backend, so there is nothing to
  race. The startup-window plan's fourth constraint — *start the backend, ask,
  then probe* — stops existing rather than getting a workaround.
- The daemon holds five records and some metadata, so **there is no vault in
  the tray process to leak, page out, or hold across a lock that never comes**.
- The daemon has no reason to draw a window, which is the two-binaries design's
  goal reached from the other end.

## What this costs, honestly

**It promotes an optional optimisation to a dependency.** The cache is off by
default and gated on Windows Hello. Making it the substrate means a machine
without Hello needs an answer, and **this is the one genuinely new design
question in this document.** Two candidates, neither chosen here:

1. **DPAPI-only sealing when Hello is absent**, with the weaker claim stated on
   the settings row — an imaged disk plus the account password reads it. This
   is what `session.bin` already accepts for the session token.
2. **No cache, and the old in-memory path** for those machines, which means
   keeping both designs alive and is the option that quietly doubles the
   surface.

**A fill may pay a round trip.** With the password no longer cached, a fill
fetches by id. On the direct-REST backend that is an HTTPS call in the low
hundreds of milliseconds. On `bw serve` with `keep_backend_running` off it is a
cold start — the ~8 s the disk cache was invented to avoid — so on that path
the projection has to carry enough to fill, or the backend has to be running.
**The two backends genuinely differ here and the design must say which it is
optimising for.**

**Staleness moves rather than disappears.** `2026-08-23` left this open and it
is still open: a match added in the window is not known to the tray until
something re-reads. With a file both processes read, the cheapest answer — the
reader re-reads when the writer's header says the generation changed — is
better than any of the three that document listed, but it is still an answer
somebody has to write.

**Two readers means two Hello unseals.** Both are the same user on the same
machine, so this is a prompt-frequency question rather than a security one —
but a design that pops two Hello prompts on one launch has failed.

## What is explicitly out of scope

- **The binary split.** `deskwarden-tray.exe` not linking `eframe` is still the
  structural guarantee and still gated on the updater swapping a set of files
  atomically. This design makes it *easier* — a tray with no vault has less
  reason to draw — and does not replace it.
- **Two-factor, organisations, attachments and Sends**, which are
  `2026-08-26-dropping-the-bw-cli-design.md`.
- **Changing what any window looks like.**

## Testing

- **Nothing here may reach the OS in a test.** Hello and DPAPI are already
  behind `DiskCacheEnv`'s `fn` pointers, so every decision on this side of the
  seam is drivable without a prompt, a TPM or a wrapped byte. That seam is why
  this design is affordable at all.
- **The projection is asserted to be a projection.** A test that reads a
  written cache as the tray would and fails if a password, TOTP seed, note or
  card number is reachable in what it loaded. This is the assertion the whole
  design exists for, and it must not be a comment.
- **One writer** is a source pin in `bw_serve_gate`'s idiom: exactly one call
  site writes the file.
- **The window opens with the backend stopped.** The failure that started this
  document, as a test: a UI process against a cache and no backend must show
  the vault.

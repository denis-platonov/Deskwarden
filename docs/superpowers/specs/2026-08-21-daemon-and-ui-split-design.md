# Daemon and UI: what the memory is, and what to do about it

**Status:** measured, not started. Three investigations on 2026-08-21 settled
the numbers; nothing here is built.

**Machine:** one Windows 11 box, NVIDIA driver, 1,663 vault items. Every figure
below comes from that machine. Vendor-specific parts are marked.

---

## The headline

A deskwarden process that has **never created a window** costs **1.61 MB** of
private commit and 7.82 MB of working set, with 27 modules loaded.

The moment it creates one, that becomes **~102 MB**. Closing the window returns
**6.7%**. The cost is the OpenGL driver's own committed memory, and nothing the
app can do releases it.

| | private commit | non-heap `VirtualAlloc` | modules |
| --- | --- | --- | --- |
| before any window exists | **1.61 MB** | 0.05 MB | 27 |
| window open | 101.97 MB | 85.55 MB | 65 |
| after the window is closed | 95.30 MB | 78.88 MB | 65 |

The owner's live 0.8.5 instance, tray-only, measured 97.09 MB private. A bare
instrumented eframe window landed at 95.30 MB, which is why the split above is
taken as transferring.

---

## What it is NOT

Three plausible explanations were measured and are dead. They are recorded so
nobody spends a week on them again.

**Not the vault.** The whole 1,663-item snapshot is **6.22 MB** resident
(3,925 B/item), measured with a counting global allocator in release. That is
7% of the idle footprint.

**Not allocator retention from parsing.** Peak during parse and populate is
**1.00x** steady. `serde_json` over a borrowed `&str` builds its `String`s
directly, with no large transient. After `clear()` the heap returns to 0.03 MB
and the process gives back all but 1.1 MB.

**Not the Rust heap at all.** Live Rust heap is **0.39 MB idle**, 3.63 MB with
the window open, all-time peak 5.70 MB. Every Windows heap together is ~9 MB.
The remaining ~79 MB is raw `VirtualAlloc` outside every heap.

**Not the glyph atlas or the fonts.** The eight bundled faces are
`include_bytes!` in the image, demand-paged; the atlas lives in the ~9 MB of
heap, not the 79.

---

## What it is

The OpenGL driver's own committed allocations. `nvoglv64.dll` and
`nvgpucomp64.dll` load at first window creation, alongside 36 other modules
(UIAutomation, windows.storage, explorerframe, SETUPAPI, textinputframework),
and never unload. The survivors are a few large arenas — a 32.00 MB region
present in every run, plus roughly 17-18 MB and 9.13 MB.

Image size is not commit: 218 MB of modules are mapped but only 5.17 MB is
writable/copy-on-write. The cost is the arenas, not the DLLs.

---

## Why the app cannot release it

**eframe already destroys the context.** `wglGetCurrentContext()` returns NULL
the moment `run_ui_native` returns. The window is destroyed, the context is
deleted — and ~63 of 69 MB survives anyway. This is not "the context is still
alive"; it is a driver that keeps its arenas after the context that created
them is gone.

**Explicit teardown does not help.** Dropping or finishing the glow context
before close changed nothing measurable. `FreeLibrary` on the driver modules
down to refcount zero — 65 iterations each — did unload `nvoglv64.dll` and
returned a further **17 MB**, leaving **47 MB**. It is not a candidate: it
permanently breaks GL in that process and `OPENGL32.dll` stays mapped anyway.
It was run to get the number.

**And the cost ratchets.** Opening and closing the window twice in one process
adds **~4 MB per cycle**, reproducible across three runs. A long-lived tray
process that opens the vault window dozens of times a day does not sit at a
fixed 95 MB — it creeps.

**Cold versus warm matters when quoting figures.** The first run after a build
(driver shader-cache miss) measured 129.9 MB open / 106.1 MB closed against
76-78 / 70-72 warm. Warm-run variance is under 2 MB. Quote a range.

---

## The conclusion

**A process that exits is the only mechanism that returns this memory.** The OS
reclaims driver commit unconditionally on exit, with no cooperation from
eframe, glow or the vendor.

So the split is not an optimisation of the current architecture — it is the
only available implementation of "give the memory back".

---

## The shape

**Daemon** — tray, global hotkey, match engine, `bw serve` ownership, and a
**bare Win32 prompt** for the light, frequent interactions: unlocking, and the
overlay's simple states. Never creates a GL context. Plausibly ~2-8 MB.

**UI** — a spawned process for anything rich: the vault window, Preferences,
the save-login form, the generator, the sequence editor, rehearsal. egui as
today. Pays the ~90 MB while open and **exits** when its window closes, giving
all of it back.

### The rule that keeps it safe

**Every surface lives in exactly one renderer.** If the same card exists in
both, that is the "two things that must agree" defect at the worst possible
place — the surface that types passwords. A Win32 "no saved login" card and an
egui save-login form are different cards, and *New login* is a handoff, not a
duplicate.

### What the Win32 side must carry over, not reinvent

- `SetWindowDisplayAffinity`, so a password field is excluded from screen capture.
- Zeroized buffers for anything holding a secret.
- The overlay's height discipline: frameless, always-on-top, no scrolling, so a
  control past the bottom edge is unreachable. The egui side has tests
  enforcing this; a Win32 rewrite inherits the constraint and not the tests.

---

## What this costs, honestly

**Secrets cross a process boundary.** Today the posture is "one process; a
password never leaves it". Split, and the UI must ask for what it displays or
types. A named pipe with a correct DACL can be done, but it is a new surface
where none exists, and the kind that is hard to prove right.

**Two processes must agree, always** — version-matched protocol, two-part atomic
update, doubled single-instance logic, and a daemon that must not outlive an
uninstall. The single-instance takeover took a full session to get right for one
process.

**The UI pays the full ~90 MB every time it opens.** The win is only real if the
steady state is genuinely tray-idle. For someone who lives in the vault window,
this changes nothing.

---

## Ordering

1. **The disk cache**
   (`docs/superpowers/plans/2026-07-31-encrypted-vault-disk-cache.md`). A daemon
   holding few items needs somewhere to restore the rest from without an
   8-second `bw serve` cold start. It also unblocks design turn 7's "Open the
   local copy" and "Continue offline", which are drawn and cannot be built.
2. **The split, with the Win32 prompt.** Where the memory actually arrives.
3. **The resident subset** — keeping only app-matched items in the daemon.
   **Deprioritised on the evidence:** the ratio is 268:1 (6.22 MB to 21.6 KB),
   but that is 6 MB of ~90. It is a security argument (fewer decrypted secrets
   resident) rather than a memory one, and should be argued on those terms.

---

## Worth doing regardless of the split

**`VaultCache::items()` deep-clones the whole vault**: 5.66 MB and 46,494
allocations, 5.6-9.4 ms per call. `app.rs:416` and `app.rs:1717` call it to find
**one item by id, on every autofill** — so that cost sits between the keypress
and the password appearing. A `get_by_id` that clones one item is small,
self-contained, and costs nothing representationally. This is a latency fix, not
a memory one.

**`VaultItem::other` costs ~184 B per retained key** for keys that are mostly
`null` or `[]` — 3.66 MB, 58.7% of the snapshot. If ever addressed, the shape
that preserves round-tripping is retaining the raw unmodelled JSON *slice* per
item rather than a parsed `serde_json::Map`.

---

## Open questions

- **How much of this is NVIDIA specifically?** Unanswered. Mesa llvmpipe could
  not be measured — CI downloads it and the network was out of scope, and
  Windows' built-in software GL is 1.1 while `egui_glow` needs 2.0+. A second
  vendor would settle whether the ~90 MB is "the driver" or "OpenGL".
- **Would running the event loop on a worker thread release per-thread driver
  arenas at `DLL_THREAD_DETACH`?** Untried: it needs a `winit` dependency that
  eframe 0.35 does not re-export. Rated unlikely — the 32 MB arena looks
  process-scoped — but it is the one remaining cheap idea.
- **Does the daemon need a GL context for anything at all?** If any surface that
  must live in the daemon cannot be drawn in Win32, the floor returns and the
  split loses most of its value. The overlay's rich states are the risk.

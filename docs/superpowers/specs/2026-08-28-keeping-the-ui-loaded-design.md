# Keeping the UI Loaded

**A setting that keeps the vault window's process resident and hidden, so
every open after the first is instant.**

## Why

The vault window now runs in a process of its own
(`2026-08-26-startup-window-in-its-own-process.md`). That bought the daemon
its memory back -- 33.5 MB against 98.6 MB, with `nvoglv64.dll`'s 41.1 MB
moved into a process that gives it back when it exits -- and it cost the
thing a resident process would have kept: **opening the vault is a cold
process start every time.** Measured on the owner's machine, on the launch
this change was verified against: 263 ms to the first frame, and 5.65 s to
1668 items on screen.

The setting trades that back, deliberately and only when asked for. It is
the same shape of trade as `keep_backend_running`, which already sits in
Preferences saying *instant, at the cost of ~111 MB held at idle*. The
difference worth stating plainly is that this memory is held in **a process
that can be killed**, not in the tray for the life of the session. That is
why the process split had to come first: without it "keep the UI loaded"
would mean "keep the OpenGL driver in the daemon until sign-out", which is
the defect that split was for.

## What this is not

- **Not the default.** `false`, and an older `settings.json` without the
  field parses as `false`.
- **Not a second window.** The one-window rule is unchanged; a hidden
  window still counts as open.
- **Not a change to how a vault session ends.** Lock, re-auth, switch and
  add/remove account all leave through exactly today's path.

## The shape

### The setting

`keep_ui_loaded: bool`, defaulting to `false`, beside `keep_backend_running`
in Preferences and written the same way: instant reopen, ~100 MB held while
the vault is unlocked.

Nothing new is passed to the child. `run_as_a_ui_process` (`main.rs:10039`)
already calls `settings::Settings::load` for itself at `main.rs:10082`, so
the child reads this field directly, and reads it **again on each close** --
so turning the setting off while a window is hidden means the next close
exits rather than hiding.

### Hide or exit, decided by an outcome that already exists

`vault_follow_up` (`main.rs:5135`) returns exactly three answers:

| Follow-up | Meaning today | With this setting on |
| --- | --- | --- |
| `AccountAction` | switch, add or remove account | **exits**, as today |
| `Resettle` | `locked` or `needs_reauth` | **exits**, as today |
| `Done` | "the window closed for good" | **hides** |

**`Done` hides; everything else exits** -- **and `Done` is not quite
enough on its own.** `vault_follow_up` does not read `edited_settings`, but
the daemon does, above the match: `main.rs:7121` applies the edited
settings to `est.settings` and runs `apply_disk_cache_change` for the disk
cache the gear may have just turned off. A window that hid after a visit to
the gear would withhold both, and the daemon would go on running against
settings the user had changed.

So the rule is **`Done` AND `edited_settings.is_none()`**. Visiting the
gear and closing exits, as it does today; the reopen after it is a cold
one. That is the rare case and the cheap answer -- the alternative is a
live result channel existing solely so that a preferences edit can be
delivered without an exit, which is most of the machinery this design
avoids, bought for the least frequent thing a window does.

Everything else the daemon acts on still arrives by the route it arrives by
now -- result file, exit, reap, resettle -- so the session machinery is
untouched.

Auto-lock needs no special case. It surfaces as `locked: true`, which is
`Resettle`, so a hidden window whose auto-lock fires exits like a visible
one. The decision is a pure function:

```rust
pub enum OnClose { Hide, Exit }
pub fn on_close(keep_loaded: bool, result: &UiVaultResult) -> OnClose
```

Written against `UiVaultResult` -- the type that already crosses between
the two processes -- rather than against `VaultFollowUp`, which lives in
the binary and the library cannot see. That is also what lets the rule be
stricter than `Done` without a second rule: it reads `edited_settings`
itself.

### The new mechanism, and no more than this

1. **`ui_process::open_decision` gains a third answer**,
   `ShowTheHiddenOne { pid }`. `FocusTheOpenOne` cannot serve:
   `foreground::raise_process` raises a window that exists, and a hidden
   viewport has none to raise.

   **Corrected during implementation.** This first said `OpenUiWindow`
   gains a `hidden: bool`, and that was wrong in a way only running it
   showed: the daemon sets that flag and the *child* changes state, in
   another process, through no channel. It read `false` for ever -- so
   `bw serve` stayed up behind a hidden window and *Open Vault* raised a
   window that was not there. There is no stored answer now. The child
   holds a named mutex, `ui_show::visible_name`, while its window is on
   screen and drops it on hide; the daemon asks the kernel. Windows
   releases it if the process dies, so a crashed window reads as gone.

   A mutex rather than a second event because this is a **state** somebody
   asks about, not a message -- and it reuses `vault_service`'s own
   `hold`/`is_held` rather than opening kernel handles a second way.

2. **A named auto-reset event, `Local\Deskwarden-UI-Show-<pid>`.** The
   daemon sets it; the child waits on it and answers with
   `ViewportCommand::Visible(true)` and a raise. Named kernel objects are
   how this crate already does cross-process signalling -- `vault_service`'s
   attachment slots are `CreateMutexW`/`OpenMutexW` under `Local\` -- so
   this is the existing idiom rather than a new one.

   **`SYNCHRONIZATION_SYNCHRONIZE`, from the `windows` crate, not a
   literal.** `vault_service` shipped `0x0010` for a right that is
   `0x0010_0000`, every `OpenMutexW` returned ACCESS_DENIED, and all 23
   tests passed because the fake kernel never reached the call. The source
   pin that now guards it exists for this.

3. **The child releases its attachment slot when it hides and retakes it
   when it shows.** Without this the setting silently pins the backend too:
   `stop_backend_if_idle` reads `vault_service::anyone_attached`, so a
   hidden child that stayed attached would hold ~111 MB of `bw serve`
   against a user who never asked for that -- and who may have
   `keep_backend_running` off precisely to avoid it. Two settings, two
   answers, neither one quietly deciding the other.

### When it goes wrong

If the event cannot be signalled, or the child does not show within a short
deadline, **the daemon kills it and spawns a fresh one.** A stuck hidden
process must never become an *Open Vault* that does nothing: under the
one-window rule that is not a slow window, it is no window ever again.

The fallback is the ordinary cold path, which is the behaviour with the
setting off -- so the failure mode is "this open was slow", not "this open
did not happen".

## How it will be known to work

**Unit tests**, on the two pure functions: `hide_or_exit` over all three
follow-ups times both settings, and `open_decision`'s third arm.

**A source pin** that the hide path is reachable only from `Done`. The
house defect class is a test that passes because it never reached the thing
it names, so this pin carries a positive control asserting the hide call
exists at all.

**A live check**, because none of the above observes a real process:

- daemon memory unchanged while a hidden child holds its own (the daemon
  must not have gained the driver back);
- close and reopen with the setting on shows the window without a new pid
  in the log, and the reopen is visibly immediate;
- with the setting on and `keep_backend_running` off, `bw serve` still
  stops behind a hidden window -- the attachment-slot release, observed
  rather than argued;
- lock from inside a hidden-capable window still exits the process and
  still resettles the daemon.

## Status

Design, approved 2026-08-28. No plan, no code.

Built and verified live 2026-08-28. The plan is
`docs/superpowers/plans/2026-08-28-keeping-the-ui-loaded.md`; the one
correction the build forced is recorded above, under the mechanism it
changed.

## Adjacent, and deliberately not in scope

**A workstation lock does not close the vault window.**
`lock_after_walking_away` (`main.rs:7382`) resettles the session -- clears the
cache, stops `bw serve`, re-authenticates -- but never sees `ui_windows`,
so the window process keeps running with its decrypted vault after Win+L.
That is true **today**, for a visible window, and is a consequence of the
process split rather than of this setting; residency only makes it last
longer. It wants its own fix and its own test, and folding it in here would
hide a real defect inside a feature.

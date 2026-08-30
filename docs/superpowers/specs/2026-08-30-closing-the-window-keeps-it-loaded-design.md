# Closing the Window Keeps It Loaded

**A preferences edit reaches the daemon without the window exiting, so that
`keep_ui_loaded` stops asking for Windows Hello on the very next open.**

## The report, and why it is exactly the rule I wrote

The owner turned *Open the vault instantly* on and said:

> "when enabled - it prompts windows hello everytime - shouldn't unless set in
> settings"
>
> "and only goes on minimize - on close it closes and asks hello"

Both sentences are one defect, and the defect is a specified behaviour doing
what it was specified to do.

`ui_process::on_close` hides only for a **completely empty** `UiVaultResult`:

```rust
let nothing_to_report = !result.locked
    && !result.needs_reauth
    && !result.add_account
    && !result.remove_account
    && result.switch_to.is_none()
    && result.edited_settings.is_none();
```

`edited_settings` counts, and it counts for a reason that is still true: the
daemon reads it *above* `vault_follow_up` (`main.rs`, `if let Some(edited) =
result.edited_settings`) to copy the preferences into its own estate, to run
`apply_disk_cache_change`, to `persist_preferences`, and to re-install the
clipboard policy. **The only route that field has ever had to the daemon is
the child process exiting.** A window that hid while holding it would withhold
all four.

Now compose that with `VaultWindowResult::edited_settings`'s own contract:

> `Some` means the modal was opened and dismissed at least once … it is never
> reset to `None` when the modal closes.

So **one visit to the gear makes every close for the rest of that window's
life an exit**, and the setting the user is looking for lives behind that
gear. They turned it on, closed the window, and got a cold start: a new
process, a fresh unlock, Windows Hello.

The two halves of the report follow directly:

- *"prompts hello everytime"* — every close after the Preferences visit exits,
  and every reopen is a cold process that re-unlocks.
- *"only goes on minimize"* — a minimized window never runs `close_or_hide` at
  all. It stays resident because it was never asked to go away. That is the
  one path in the feature that was working, and it was working by accident of
  never consulting the rule.

**The rule is not wrong. Its premise is.** "The only way `edited_settings`
reaches the daemon is this process exiting" is a statement about the
*transport*, not about the field. Give the field a second transport and the
rule can relax without losing anything — which is what this design does.

## The question: how does a preferences edit reach the daemon without the window exiting?

### Option 1 — the child persists settings itself, the daemon re-reads

The child already loads `Settings` at startup, already knows `settings_path`,
and `persist_preferences` is a read-modify-write of the preference fields
only, so it is safe to call from a second process. The child writes; the
daemon re-reads.

**Cost:** small, on the surface. No new IPC payload.

**What goes wrong, and it is disqualifying:** `apply_disk_cache_change` is not
a value the daemon stores, it is a **side effect that can refuse**. Enabling
the disk cache calls `cache.enable_disk_persistence()`, and when that fails it
puts a message box on screen saying *"The setting has been left off"* and
returns `false` — and the daemon then writes `cache_vault_to_disk: false` to
the file. That correction only exists because the daemon writes *last*. If the
child writes first, `settings.json` records `true` for a disk cache that was
never enabled, which is exactly the state `apply_disk_cache_change`'s own doc
calls *"the worst of the three possible states"* — on while nothing is ever
written.

The second problem is smaller and still real: the daemon needs to be **told**
to re-read. There is no natural moment. `main`'s loop does not re-read
`settings.json`; `estate.settings` is loaded once at startup and mutated in
place, deliberately — `persist_accounts`'s doc spends a paragraph on what
happens when a stale in-memory copy is written back over a fresher file. So a
signal is needed either way, and once a signal exists, carrying the payload on
it costs nothing extra. Option 1 buys nothing and gives up the correction.

### Option 3 — keep exiting, make the exit cheap

The exit hurts because reopening re-unlocks. Is the Hello prompt avoidable on
its own?

Partly, and partly already done: `fce7d3d` ("Seal the disk cache with a stored
key, so startup asks nobody") removed Hello from the disk cache's startup path.
But this is **not a fix for the reported defect**, for a reason that is
structural rather than a matter of degree: `keep_ui_loaded` does not exist to
avoid an unlock. It exists to avoid the ~50 MB of OpenGL arenas and the
1–3 second cold start that `ui_process`'s module doc opens with. A process that
exits pays all of that whatever the unlock costs. Under option 3 the feature
would remain: *visiting Preferences turns the feature off until the next
launch* — silently, and with no way for the user to know it.

Hello-on-reopen is worth pursuing **on its own axis** and is out of scope here.
It is a mitigation of a symptom, and taking it would leave the cause in place.

### Option 2 — a live channel. Chosen.

The child sends the edited settings to the daemon **while it is still alive**,
and the daemon applies them through the code path it already has, side effect
and all. Once delivered, the field is no longer something a hidden window is
withholding, and `on_close` may hide on it.

The whole design is in one sentence: **`edited_settings` gets a second way
home, so the hide rule stops having to be the thing that guarantees it gets
home at all.**

## The channel

### Shape

Two named things, both already this crate's idiom, both keyed by the child's
pid — which `ui_show`'s doc already establishes as *"the daemon's whole record
that a window exists"*:

| | Name | Who creates | Who writes |
| --- | --- | --- | --- |
| Payload | `%CONFIG%\ui-settings-<pid>.json` | child | child, atomically |
| Doorbell | `Local\Deskwarden-UI-Settings-<pid>` | **daemon**, at spawn | child, by `SetEvent` |

The doorbell is the mirror image of `ui_show`'s existing show signal — same
`Local\` scoping, same auto-reset, same `Signal`/`ShowEnv` types, opposite
direction. Auto-reset for the reason that module already gives: it is *a token
to be consumed*, one ask one read, and the kernel does the resetting.

**The daemon polls it with `Signal::wait(0)`**, once per pass of `main`'s loop,
next to the `try_wait` on the child and the `is_held` on the visibility mutex
that are already there. A blocking wait is impossible — that loop is the thing
answering the hotkey — and `wait(0)` is a non-blocking read that already exists
on the type.

### Why a second file rather than the result file

`ui-result-<pid>.json` is the child's *last act*, written once on the way out.
Writing it early would mean the daemon reading a path the child is about to
rewrite, and `std::fs::write` truncates before it writes. Its own generation of
that file is also `forget_result`ed by the daemon after a reap, which would put
a delete and a write on the same path from two processes.

A separate path has none of that, and it is written **temp-then-rename** so
that a daemon reading on the doorbell can never see half a file. The daemon
deletes it after applying, the same way `forget_result` tidies the other one.

### Ordering, and the one failure that matters

Write the file, *then* set the event. The daemon only ever reads on the event,
so it never reads a file that is not finished.

`SetEvent` on a name nobody holds returns `false` — that is what
`ui_show::ask_to_show` already relies on. So the child learns whether delivery
landed:

- **`true`** → a daemon process holds that event, and the daemon is a live
  polling loop that will consume it within one pass. The child records
  `settings_delivered = true`.
- **`false`** → nobody is listening. The child records nothing, and the
  edit keeps its old transport: `on_close` sees an undelivered
  `edited_settings` and **exits**, exactly as today.

That is the whole failure story, and it degrades to current behaviour rather
than to a lost setting.

### `edited_settings` is NOT cleared on delivery

Deliberately, and this is the subtle part. `VaultWindowResult::edited_settings`
stays `Some` for the rest of the window's life; that contract is load-bearing
(the modal's seed on a second open reads it, `effective_auto_lock` reads it,
the check-breaches and reveal-TOTP reads at `vault_window/mod.rs:2676` and
`:3528` read it) and this design does not touch it. Delivery is tracked in a
**separate cell** the daemon never sees.

So an exit that happens later still carries `edited_settings` home, the daemon
applies it a second time, and that is harmless because the daemon's write-back
is already guarded by `if edited != est.settings`. Re-delivery is idempotent by
construction, and always was.

### When delivery happens

Two moments, one helper:

1. **When the gear's modal is dismissed** (`PrefsAction::Close`, `vault_window/mod.rs:4947`).
   This is a genuine improvement beyond the bug: today a `keep_backend_running`
   change made in the vault window does not reach the daemon until the window
   closes. Now it lands at once — which is what the tray's Preferences item has
   always done.
2. **On the way through `close_or_hide`**, if an edit is still undelivered —
   which covers Alt+F4 with the modal up, the case the every-frame write at
   `:4946` exists for.

Both go through one `deliver_if_needed`, so there is one delivery site and one
place that sets the delivered flag.

## What the daemon does with it: one write-back, not three

The daemon has **two** copies of the settings write-back today — the tray's
Preferences handler and the vault loop's — and the vault loop's carries a
comment naming the duplication as *"this file's house defect"*. This design
would make it three.

It makes it **one** instead. The two existing copies are, on inspection,
identical in effect: the tray writes

```rust
if !apply_disk_cache_change(..) { settings.cache_vault_to_disk = false }
else { settings.cache_vault_to_disk = edited.cache_vault_to_disk }
```

and the vault loop writes

```rust
apply_disk_cache_change(..) && edited.cache_vault_to_disk
```

which agree on all four inputs. Both then assign, both call
`persist_preferences`, both call `clipboard::configure`. So the extraction is a
true de-duplication rather than a generalisation over a difference:

```rust
/// Apply one edited `Settings` to the daemon's estate: the disk-cache side
/// effect (which can refuse), the assignment, the preferences-only save, and
/// the clipboard re-install. The ONE place a preference edit lands, whichever
/// of the three shells produced it.
fn apply_edited_settings(
    cache: &VaultCache,
    settings: &mut settings::Settings,
    settings_path: &Path,
    edited: settings::Settings,
);
```

Three callers: the tray handler, the vault loop's write-back, the live channel.
**`apply_disk_cache_change` runs on all three**, which is the requirement, and
it now runs from one line rather than from three that have to be kept in step.

## The rule, after

```rust
pub fn on_close(
    keep_loaded: bool,
    result: &UiVaultResult,
    settings_delivered: bool,
) -> OnClose
```

Hide when `keep_loaded` **and** the five undelivered-only fields are empty
**and** `edited_settings` is either absent or already delivered.

`locked`, `needs_reauth`, `switch_to`, `add_account`, `remove_account` are
untouched and still force an exit. Each of them is *a reason the window
closed* — the daemon's response to every one is to tear something down and
build it again, and the window has to be gone for that. `edited_settings` is
the one field that is explicitly **not a reason the window closed** (its own
doc says so: *"this rides along"*), and it is the only one this changes.

### `keep_loaded` is re-read, not the construction-time copy

A window whose user has just turned `keep_ui_loaded` **off** in the modal must
not hide. Today that is unreachable — the edit forces an exit — and the fix
makes it reachable, so it has to be handled in the same change.

The answer already exists in this file: `effective_auto_lock` (`vault_window/mod.rs:1506`)
prefers the modal's live value over the parameter for exactly this reason, and
its comment records the same defect in its own domain ("turning auto-lock off
in Settings changed nothing until the vault was closed and reopened"). This
gets `effective_keep_ui_loaded`, the same shape, and `close_or_hide` passes its
answer as `on_close`'s `keep_loaded`.

## The 64-combination pin moves, and it is a re-pin

`the_hide_rule_is_stricter_than_done` walks all 64 combinations of the six
fields and asserts **Hide ⇒ `vault_follow_up == Done`**, with a control that
exactly one combination hides.

**The property being asserted does not change.** It is: *a result that hides
loses nothing the daemon would have acted on.* What changes is one of its
inputs — `edited_settings` is no longer lost by hiding, because it travelled
already.

The implication still holds for the newly-hiding combination, and it holds for
the reason the test was written: a result carrying only `edited_settings` lands
on `Done`, because *"editing preferences is not a reason a window closed"*.
The test's own doc says the danger is *"an outcome swallowed by a window that
never came home to deliver it"* — and this one came home by another door.

The pin is re-stated as **two passes over the same 64**:

| Pass | `settings_delivered` | Hides | Meaning |
| --- | --- | --- | --- |
| A | `false` | **1** — the empty one | Byte-for-byte today's rule. Undelivered is unchanged. |
| B | `true` | **2** — the empty one, and `edited_settings`-only | The one combination this design moves. |

Hide ⇒ `Done` is asserted in both passes. Pass A is the control that makes the
flag load-bearing: if `settings_delivered` were ignored, A would count 2 and
fail. Pass B's count of exactly 2 is the control that the relaxation is *one*
combination and not a widening — a rule that dropped the `locked` conjunct
would count 4 and fail.

A loosening would be a rule that lets an *undelivered* outcome hide. Nothing
here does. Every one of the six fields still has a guaranteed route to the
daemon; one of them now has two.

## Does `minimize` need a change? No — but it needs a reason

`ChromeAction::Minimize` sends `ViewportCommand::Minimized(true)`
unconditionally, never consults `keep_ui_loaded`, never runs `close_or_hide`,
and keeps both the visibility mutex and the vault-service attachment.

That is **correct, and it should stay**, but it is currently correct by
omission. The argument, written down:

- A minimized window **is still in use**. It has a taskbar button, the user can
  restore it with one click and no daemon involvement, and it is holding a
  decrypted vault on a machine its owner is at. `vault_is_in_use` reads the
  visibility name and should keep answering `true`, so `bw serve` stays up
  behind it — a restore that had to wait for the backend to come back would be
  the save-memory defect in reverse.
- A hidden window is the opposite on every count: no taskbar button, no way
  back except the daemon's named event, attachment dropped so save-memory can
  reclaim the backend.
- Therefore **minimize must not be gated on `keep_ui_loaded`**. Gating it would
  mean the setting changes what the minimize button does, which nobody asked
  for and which would make a window with the setting off unminimizable.

So: no behavioural change, a doc comment that says the above, and a test
pinning that `ChromeAction::Minimize` neither calls `close_or_hide` nor touches
the hide hooks. It becomes deliberate rather than accidental, which is what the
owner's second sentence deserves.

After the fix the two converge from the user's side — close and minimize both
keep the process — and stay different underneath, which is right.

## What this does not fix, and I would rather say so

**The refused disk cache diverges in a resident window.** If the user turns the
disk cache on and `enable_disk_persistence` fails, the daemon corrects its own
copy and the file to `false` and shows a message box. The child's
`edited_settings` cell still says `true`, so a second click on the gear seeds
the modal with a checkbox that is on while the file says off, until the window
is actually closed and reopened.

Today the window always closes in that scenario, so the divergence is invisible.
It is not new, it is not a data loss, and the user has just been told in a
message box that the setting was left off. The fix would be an acknowledgement
carrying the corrected settings back down the channel — a second event and a
second file in the other direction — and it is not worth that here. It is
recorded as a known residual, not overlooked.

## Testing

- **`on_close` over all 64 combinations, twice**, as above — the re-pin.
- **`effective_keep_ui_loaded`** with a positive control: the parameter wins
  when the modal never opened, the modal wins when it did, in both directions.
- **Delivery is attempted and its answer is believed**: a fake deliver hook
  that returns `false` leaves the window exiting; one that returns `true`
  lets it hide. Each is the other's control.
- **`apply_edited_settings` runs `apply_disk_cache_change`** — asserted at the
  point of effect over a real `VaultCache` in a temp directory, off→on→off,
  reading the file's existence rather than a flag.
- **The doorbell over the real kernel**, in `ui_show`'s own idiom: created by
  one side, set by the other, consumed once, `false` when nobody listens. That
  module's tests already do exactly this for the show signal, and its own
  comment records why a fake would not do — `vault_service` shipped a wrong
  access right and passed 23 tests.
- **A live check**, which is the only thing that settles it: turn the setting
  on, close, reopen, and see no Hello. No unit test in this crate observes a
  real window hiding.

## Sequencing

1. `apply_edited_settings` — the de-duplication, on its own, with the two
   existing callers and no behaviour change. Green before anything else moves.
2. The channel — `ui_show::settings_name`, `ui_process::edited_settings_path`
   and its atomic write/read/forget. Pure and kernel-testable, no window.
3. The child — the fourth hook, `effective_keep_ui_loaded`, `on_close`'s third
   parameter and the re-pin.
4. The daemon — the doorbell at spawn, the `wait(0)` in the loop, the third
   caller of `apply_edited_settings`.
5. Minimize's doc and pin.
6. The live check.

## Status

Design, 2026-08-30. Written against `ui_process.rs`, `main.rs`,
`vault_window/mod.rs`, `ui_show.rs` and `settings.rs` as of `fce7d3d`.

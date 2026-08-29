# The Lock Closes the Window

**Win+L must end the vault window's process, not merely lock what is behind
it.**

## Why

`away_lock` was written to answer exactly one state: *app in the tray, vault
unlocked, no window*. Its module doc says so in as many words, and it is
explicit that the other state is covered by something else:

> One consequence, stated rather than hidden: the pump does not run while a
> vault window is up, because that window runs its own nested `eframe` loop.
> In that state the idle auto-lock inside the vault window is what covers the
> user, which is the arrangement that already existed.

`lock_after_walking_away` repeats the claim in its own doc: *"A vault WINDOW
being open cannot reach here at all."*

**Both sentences stopped being true when the vault window moved into its own
process.** `spawn_the_vault_window_in_its_own_process` starts
`deskwarden.exe --ui vault`; the daemon does not block on it; `UiWindows`
polls it once per pass; and the daemon's loop — pump included — runs the whole
time it is up. `UiWindows::ask_for_the_vault_window` says this out loud where
it explains why *Open Vault* can now be clicked twice: *"With the loop live,
the tray is clickable while a window is up, so this is now a case that can
actually happen."*

So the pump *does* run, `away_event` *does* fire, `lock_after_walking_away`
*does* get called — and it does the wrong half of the job. It clears the
clipboard and resettles the daemon's own session, and the second process, the
one holding a decrypted vault rendered on screen, is not in its parameter list
and cannot be reached from it. The user presses Win+L and walks away from a
machine on which their entire vault is decrypted in a process that nothing is
watching.

This is not the daemon's cache surviving; the resettle clears that. It is a
separate address space that no part of the away-lock path has a handle to.

## What this is not

**It is not a new preference.** `locks_the_vault` already owns the decision
and already takes the three inputs it needs. This work adds no fourth input
and no setting beside `auto_lock_enabled`.

**It is not a new IPC channel.** There is no daemon→child message protocol
today; the child talks home through an exit code and one small JSON file, both
written as it dies. Inventing a request channel for this would mean a security
property whose enforcement depends on a child process being willing and able
to answer — a child that may be inside a modal, inside a blocking `bw` call,
or hung. Anything with a timeout behind it is a kill with a delay in front of
it.

**It is not a graceful save.** In-progress edits in an open item form are
lost, and that is stated rather than hidden. See below.

**It is not a fix to `away_lock`'s event model.** `away_event`,
`pick_notification_window` and `register_on` are untouched.

## The shape

### There is no "locked but resident" state to ask for

The obvious alternative — tell the window to lock itself rather than killing
it — dissolves on contact with the code. The window's own auto-lock, the
mechanism a reader would reach for as the thing to reuse, does this
(`vault_window/mod.rs`, the frame closure):

```rust
let lock_countdown = match idle_frame(auto_lock_now, last_activity.elapsed()) {
    IdleFrame::Lock => {
        *locked_for_closure.borrow_mut() = true;
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        return;
    }
    IdleFrame::Sidebar(text) => text,
};
```

**In this app, a lock in the vault window *is* the window closing and the
process exiting.** There is no code path that leaves a locked-but-alive UI
process holding a key it must then wipe, because the process holding the key
is the thing that goes away. "Ask it to lock" and "close it" are the same end
state reached two different ways: one that depends on the child cooperating,
one that does not.

So: **the daemon kills it**, from the same place and in the same shape as
`UiWindows::close_on_quit`.

The cost is real and is the same cost the existing idle auto-lock already
imposes: an item form open with unsaved text loses it. That trade was already
made and shipped for the timer that fires on a guess; it is easier to justify
for the signal that is not a guess. `farewell_to_an_open_window`'s own doc
already accepts it for *Quit* — *"The cost is an edit in progress in that
window."*

### A third reason, answered by the same decision

`farewell_to_an_open_window(DaemonExit, Option<u32>) -> Farewell` is the
precedent and it is nearly the right shape already. It gains a third reason,
and `DaemonExit` does not, because the daemon is **not** exiting here:

```rust
pub enum WhyClose {
    DaemonIsQuitting,   // tray Quit; was DaemonExit::UserQuit
    DaemonIsRestarting, // update or crash; was DaemonExit::Restart
    TheUserWalkedAway,  // Win+L, switch-user, or suspend
}

impl From<DaemonExit> for WhyClose { .. }

pub fn farewell_to_an_open_window(reason: WhyClose, open: Option<u32>) -> Farewell
```

`DaemonExit` stays, because the quit path genuinely is asking a
daemon-lifecycle question and should keep saying so; `WhyClose` is what the
window-closing decision is actually over. The `match` gains an arm, so a
fourth reason later is a compile error at the one place that must weigh it.

**`TheUserWalkedAway` is only ever constructed downstream of
`locks_the_vault`.** The gate is not duplicated and no second opinion about
auto-lock exists: `lock_after_walking_away` already returns early when
`locks_the_vault` says no, and the window close goes *after* that early
return.

### Order, and why it is this order

Inside `lock_after_walking_away`, after the gate:

1. `clipboard::clear_if_still_ours_for(ClearTrigger::Lock)` — unchanged, and
   still first, for the reason already written there.
2. **Close the window.** Before the resettle, because `resettle_session`
   blocks on a master-password prompt that can stand there for hours. A window
   closed after that prompt is answered is a window that survived the entire
   absence the feature exists to cover.
3. `resettle_session` — unchanged.

### The slot must be emptied, or the kill reports a lock

`std::process::Child::kill` on Windows is `TerminateProcess` **with exit code
1**, and `UiVaultResult::EXIT_LOCKED` is **1**. If the away-lock kill left the
`UiWindows` slot occupied, the next `poll_the_vault_window` would reap that
child, read status 1, decode `locked: true`, and hand `run_vault_loop` a
second lock to act on — a second `resettle_session`, a second master-password
prompt, for a window the daemon killed itself.

`close_on_quit` avoids this by taking the slot (`self.vault.take()`) and
deleting the result file rather than reading it, and it can afford not to
explain itself because `process::exit(0)` is on the next line. **Here the loop
keeps running, so taking the slot is load-bearing** and gets a test of its
own.

### The hidden window, and the branch it is not on

`keep_ui_loaded` — a setting on another branch — leaves the UI process
resident and hidden rather than exiting when the window closes. A hidden
process holding a decrypted vault across a workstation lock is strictly worse
than a visible one: nothing on screen reminds the user it is there, and the
"it closed, so it's gone" intuition is exactly backwards.

**This design is visibility-blind by construction, and that is the answer.**
`UiWindows` tracks process ids, not window handles; `farewell_to_an_open_window`
takes `Option<u32>`; the kill is `Child::kill`. Nothing on this path asks
whether a window is on screen, so a resident-hidden process registered in the
same slot is killed by the same line with no change.

**The dependency is stated rather than assumed**, in both directions:

- If `keep_ui_loaded` lands **with** the resident process still recorded in
  `UiWindows.vault`, this work covers it and nothing further is needed.
- If it lands with the resident process recorded **anywhere else** — a second
  registry, a detached handle, a slot emptied on hide — then this work does
  *not* cover it, and closing that gap is that branch's obligation. The
  rendezvous is a line in `keep_ui_loaded`'s own design pointing here.

We do not guess which. The plan carries a task that reads the merged state at
integration time and reports, rather than writing code against a branch that
is not here.

**Warm start is not a reason to keep it.** `keep_ui_loaded` buys a faster
*Open Vault*; the price after a workstation lock is a cold start, and a cold
start is the correct price for having left the machine.

## The four questions, answered

### 1. Close, or ask it to lock? — **Close.**

There is no resident-locked state in this codebase to ask for; the window's
own auto-lock closes the process. A cooperative protocol would make a security
property depend on a child that may be modal or hung, and its failure mode is
a kill with a delay in front of it. The unsaved-edit cost is the cost the
existing idle auto-lock already charges.

### 2. The hidden window? — **Killed by the same line, if it is in the same slot.**

The path is over pids, not visibility. Dependency on `keep_ui_loaded`'s
registration is recorded, not assumed.

### 3. Auto-lock OFF? — **The window stays open. Same gate, no exception.**

This is the sharpest question and the answer is to change nothing about
`locks_the_vault`.

The case for closing anyway — "walking away from an unlocked machine is
different from idling at it" — is a real argument, and it is already the
argument that got `away_lock` written. But it was answered once, in
`locks_the_vault`'s own doc: `AutoLock::Never` means *"do not lock this vault
behind my back"*, and there is no reading of Win+L under which closing the
window is not exactly that.

What settles it is that closing the window under `Never` **buys almost no
security**. The daemon's cache stays decrypted, `bw serve` stays up on
localhost serving that vault, and the session token stays live — because
`locks_the_vault` said not to lock. Killing the window would remove one
rendering of a vault that is still fully available to anything on the machine,
while the lock screen has already blanked that rendering off the display. The
gain is cosmetic; the loss is a user's unsaved edit and a setting quietly not
meaning what it says.

A design in which the window closes but the vault stays unlocked is also a
design with two lock policies, and the second one is not in Settings.

The honest residual: under `Never`, a workstation lock leaves everything
exactly as the user configured it, which is a decrypted vault on a locked
machine. That is the setting working, not the feature failing, and it is the
same exposure `Never` already produces for the idle timer.

### 4. Sleep and switch-user? — **Same answer, for a stated reason.**

`away_lock` already collapses `WorkstationLocked` and `Suspending` into one
decision and argues at length for why a suspend is treated as a departure —
*"the vault survives, decrypted, across a night in a bag."* A vault window is
the largest such survivor there is, and on suspend it is a decrypted vault
written into a hibernation image. Nothing about the window makes the two
events want different answers, and `locks_the_vault` is called once with
whichever arrived.

**One honesty about suspend**, flagged rather than fixed: `PBT_APMSUSPEND`
gives a process a short and unguaranteed window before the machine goes down.
`TerminateProcess` is prompt and does not wait for the target, which is the
best available behaviour, but a suspend that lands between the kill and the
kernel reclaiming those pages can still put them in the hibernation file.
There is no in-process arrangement that closes this, exactly as
`clear_if_still_ours_for`'s doc says of a power cut and the clipboard.

## How it will be known to work

**Pure tests, in `ui_process.rs`:** `TheUserWalkedAway` with a pid yields
`CloseIt`, with no pid yields `NothingOpen`; `DaemonIsRestarting` still yields
`NothingOpen` (the positive control that the new arm did not flatten the
existing distinction); the `From<DaemonExit>` mapping is the identity the two
old call sites relied on.

**A source pin, in `main.rs`:** the away-lock path closes the window *before*
`resettle_session`, and the slot is taken rather than left. Both are
properties of a function no test in this crate can call — the same reason
`bw_serve_gate` reads source text — and both have a positive control that the
needle they searched for was found at all.

**A collision test:** `EXIT_LOCKED == 1` and `Child::kill`'s status is 1, so
an away-lock kill that left the slot occupied would be decoded as a lock. The
test asserts the collision exists, which is what makes taking the slot
necessary rather than tidy.

**A live check, which is the only one that settles it.** No unit test in this
crate observes a real process surviving a real Win+L, and this is a security
property about a real process. The check is scripted in the plan's last task:
open the vault window, record its pid, press Win+L, come back, and confirm
that pid is gone — plus the negative control with `auto_lock_enabled = false`,
where that pid must **still be there**, because a live check that only ever
saw processes die would pass on a build that killed the window
unconditionally.

## Status

Design, 2026-08-29. Written against `two-factor-without-the-cli`, reading only.
Two doc comments in the tree assert the opposite of this document's premise —
`away_lock`'s module doc and `lock_after_walking_away`'s — and are stale as of
the daemon/UI process split; correcting them is a task in the plan, not a
footnote.

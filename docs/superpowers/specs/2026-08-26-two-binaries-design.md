# Two binaries: a tray that cannot draw, and a UI that can

**Status:** designed, not started. Supersedes the "Why one binary and not two"
section of `2026-08-23-daemon-and-ui-processes-design.md`. Everything else in
that document stands -- its lifetimes, its "the UI needs nothing passed to it"
finding, and its rule that every surface lives in exactly one renderer. What
changed is not the goal but the evidence about how the goal is kept.

## Why this exists: the rule was a discipline, and the discipline failed

`2026-08-23` argued for one executable with a `--ui` mode. Its case rested on
avoiding version skew, keeping the installer single-target, and not writing
the single-instance protocol twice. Those costs are real and are answered
below.

What that document could not know is that the arrangement it chose had already
been broken by the time it shipped. Measured on 2026-08-26, on the merged
`rest-backend` build, launching Deskwarden by **double-clicking it**:

| process | private working set | `nvoglv64.dll` loaded |
| --- | --- | --- |
| daemon, launched from the tray shortcut, window never shown | 20.9 MB | no |
| daemon, **launched by double-click** | **107 MB, and 62 MB after the window closes** | **yes** |
| the vault window as its own `--ui vault` process | 76.2 MB | yes, and it leaves with the process |

The second row is the defect. `main.rs:1341` reads:

```rust
if surface == FirstSurface::ShowTheWindow {
    // **ONE WINDOW: the spinner, then the vault.**
```

and builds the vault frame **in the daemon**. `first_surface` maps
`LaunchIntent::UserLaunch` to `ShowTheWindow` and `LaunchIntent::LoginAutostart`
to `StayInTheTray`, so:

* **Autostart never hits it.** That is why the release's measurements looked
  right and why nothing caught this.
* **A double-click always hits it.** The OpenGL driver loads into the daemon,
  its arenas are never returned, and the process holds ~50 MB above its
  pristine state until it is restarted. `2026-08-21` established that only
  process exit returns that memory.

The process split moved the *on-demand* path -- tray, hotkey -- onto
`UiSpawnPlan`. It did not move the startup path. So the app has two ways to
open the same window and only one of them keeps the promise the release was
made on.

**The point is not that a path was missed. The point is what kind of thing
would have caught it.** In a single binary, "the daemon must not create a GL
context" is a rule somebody has to keep, in a file with 29,000 lines of
`main.rs` and eleven other entry points. Nothing in the type system, the
linker, or the test suite makes the wrong call impossible -- and the honest
worry is not this path but the third one nobody has found.

## The shape

**Two binaries in one workspace, sharing one library crate.**

* **`deskwarden.exe`** -- tray, global hotkey, match engine, the vault backend
  (`bw serve` or direct REST), and the bare-Win32 surfaces: unlock prompt,
  account picker, prompt card, locked card, save-login card, generator card,
  send preflight. **Does not depend on `eframe` at all.**
* **`deskwarden-ui.exe`** -- the vault window, Preferences, the sequence
  editor, rehearsal. Depends on `eframe`. Pays the ~90-115 MB while open and
  **exits when its window closes**, which is the only mechanism that returns
  the driver's memory.

The guarantee is structural: a binary that does not link `eframe` cannot load
`nvoglv64.dll`, whatever a future edit to `main.rs` says. The rule stops being
a rule.

### What does NOT become a process, and why

The obvious next question is whether the other pieces should split too. They
should not, and for different reasons in each case.

**`rest` stays a library.** `src/rest/` is client-side cryptography plus HTTP
calls made by whoever needs them. Giving it its own process means a local
server on a port, with a lifecycle, a readiness probe, and a state where it is
running when it should not be -- which is `bw serve`, precisely what the
direct-REST backend exists to remove. It would trade a Node subprocess for a
Rust one and keep every problem that made the Node one worth removing.

**`password_gen` stays a library.** It is a pure function over `getrandom` and
a 29 KB wordlist read on demand. It draws nothing; the generator *card*
(`generate_prompt.rs`) is bare Win32 and costs ~1.8 MB with no GL context.
There is nothing to isolate -- the expensive thing was never the generator.

So the split is **two**, not four, and the line it is drawn on is exactly one
question: *does this code create a GL context?* Nothing else about the
architecture changes.

## A second defect this fixes, which is not about memory

`first_surface`'s own comment records it:

> It is **NOT** reached in production, and the report says why: `main` reads
> this decision a thousand lines below the single-instance takeover, so a
> `--ui` process routed through it would first ask the daemon that spawned it
> to stand down.

A child process that contains, and can execute, the protocol for retiring its
own parent is a hazard that exists **only because the two are one binary**. On
2026-08-26 a launch of a second instance produced three stand-down messages
and left *nothing* running -- the new instance took the app mutex and was then
itself asked to stand down. That was not reproduced, and it is not being
claimed here as the same bug. It is offered as the shape of what this
arrangement makes possible.

A separate `deskwarden-ui.exe` does not contain the takeover, the app mutex
ownership, or `first_surface`. It cannot retire a daemon by any code path,
because the code is not in it.

## What has to move

**The startup window becomes a spawn.** On `FirstSurface::ShowTheWindow` the
daemon spawns `deskwarden-ui.exe` rather than building the frame in-process.
Three things about that:

* **The spinner stage moves with it.** The "one window: spinner, then vault"
  behaviour exists because an earlier build showed a 360x220 loading window,
  closed it, and let the vault appear later somewhere else at a different
  size. That report must not be re-opened by this change: the UI process shows
  the same one window with the same two stages.
* **The daemon still arms autofill and seeds the cache first.** It already
  does, one line above the branch. That ordering is preserved, not moved.
* **The result already has a channel.** `ui_process::UiVaultResult` carries all
  six daemon-actionable outcomes and needs nothing added.

**First launch stays exempt, and this is the easy thing to get wrong.** A UI
process whose daemon *went away* exits; a plain double-click that finds no
daemon must start one. `2026-08-23` states the rule and a test already pins the
difference. With two binaries the question changes shape -- double-clicking
`deskwarden.exe` starts a daemon which then spawns the UI -- but the rule and
its test still have to hold.

## What this costs, honestly

**The updater must swap both files or neither.** This is the real cost and it
is the one that can hurt a user. Today self-update replaces one binary; with
two, a half-applied update leaves an old daemon spawning a new UI or the
reverse. The mitigation is that they ship and version together as a set, so
skew is a property of the *updater's atomicity* rather than of the split --
but that atomicity now has to be written and proved, and the current updater
does not have it. **Nothing in this design may be implemented before the
updater can swap a set.**

**Two artifacts to sign.** SignPath integration is already pending manual
account approval for one binary (`docs/code-signing-policy.md`). This doubles
what that approval has to cover.

**Two things to observe in a bug report.** Already true of the daemon and the
UI process today, but a second *file* makes "which one is old" a question that
can be asked.

**A workspace refactor.** The shared code becomes a lib crate with two `[[bin]]`
targets. Cheap in Rust and cheap in this crate specifically -- `lib.rs` already
exposes every module publicly and `examples/` already links against it -- but
it touches `Cargo.toml`, which is byte-pinned in `job_object.rs`, and it
changes what `build.rs` embeds an icon into.

## What is explicitly out of scope

* **Any change to what the surfaces look like.** This moves where code runs,
  not what it draws.
* **The `wgpu` renderer.** Measured at 412.8 MB against glow's 124.1 MB and
  rejected; see `2026-08-23`'s amendment and the `wgpu-renderer` branch's own
  commit messages. Halving the UI's cost is a separate question and this design
  neither helps nor hinders it.
* **Splitting `rest` or `password_gen`**, for the reasons above.

## Testing

The house defect is "a test that passes because it never reached the thing it
names", and this design's whole claim is about something that *cannot* happen,
which is the hardest kind of claim to test. So:

* **The linker is the primary assertion.** `deskwarden.exe` must not depend on
  `eframe`. A dependency-graph check in `job_object.rs`'s idiom -- read
  `Cargo.toml`, not a list somebody maintains -- fails the day someone adds it
  back.
* **A module-load check on the daemon.** After exercising the whole fill path
  and a startup, the daemon's loaded modules must not include `opengl32.dll`
  or `nvoglv64.dll`. This is the assertion that would have caught the defect
  this document exists for, and it must be taken **after** the fill path runs
  -- `2026-08-23`'s amendment records that a daemon measured before first use
  measures nothing.
* **The first-launch distinction** keeps its existing test: a UI whose daemon
  went away exits; a launch that finds no daemon starts one.
* **The updater's atomicity** is pinned before the split lands, not after.
* **No test touches** the network, the real vault, the clipboard, the screen,
  `%APPDATA%\Deskwarden`, a real dialog, or spawns `bw`.

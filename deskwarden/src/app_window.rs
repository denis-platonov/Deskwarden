//! The single window: sign-in, then the spinner, then the vault -- one eframe
//! app, one OS window, one event loop.
//!
//! The report this exists for: "When login (not lock) after entering - screen
//! closes and opens another small screen with a spinner, then it closes and
//! tray app loads - should be single screen from the beginning, spinner and
//! same screen with vault open - user needs to close it manually to tray".
//! Three windows on the way in, two of which nobody asked for -- and at the end
//! of them the user was dropped to the tray without ever being shown the vault
//! they had just unlocked, because `open_vault_window` ran only from a tray
//! click or the hotkey.
//!
//! # What this is NOT for
//!
//! **A launch with a valid cached session must not gain a window.** That launch
//! shows nothing today and goes straight to the tray, which is correct and was
//! not part of the report; `main` keeps its old path for it, spinner window and
//! all. This runs only on the launch that actually signs in. `loading_ui::
//! show_while` therefore stays a real window and stays used.
//!
//! **The lock/re-auth recovery still closes and reopens.** That recovery is
//! `main::resettle_session`, and it cannot run from inside a frame closure on a
//! worker thread: it borrows a `tray::AppTray` (thread-bound Win32) and its own
//! body opens two more eframe windows. It is the one hardened
//! teardown-and-repopulate sequence in this app and gets reused whole rather
//! than reimplemented, which means the window it needs gone has to actually be
//! gone. The user scoped that out themselves -- "login (not lock)".
//!
//! # The shape
//!
//! The work between the stages -- starting `bw serve`, waiting for it to
//! answer, asking the CLI who is signed in -- is synchronous and used to run
//! while no window was up. Inside one app it may not block the frame closure,
//! or the window freezes exactly where it is meant to be showing a spinner. So
//! it runs on a detached worker thread and the frame closure drains the result,
//! which is the shape `vault_window::spawn_vault_load` already uses for the
//! item load.
//!
//! Neither sub-UI opens its own window: both are frame closures built by
//! `login_ui::build_login_frame` and `vault_window::build_frame`, drawn by this
//! module's own closure. eframe cannot nest event loops, which is the whole
//! reason those two functions exist separately from their own hosts.

use crate::accounts::Account;
use crate::{foreground, loading_ui, login_ui, theme, vault_window};
use eframe::egui;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The OS-level window title. Named rather than inlined, because
/// `foreground::raise_window` and `login_ui::round_window_corners` both find
/// this window BY it -- one declaration means the three cannot drift.
///
/// The same string the vault window uses, deliberately: this window BECOMES
/// that window. Nothing outside this process should be able to tell that the
/// vault the user ends up looking at was not opened by a tray click.
const WINDOW_TITLE: &str = "Deskwarden";

/// How often the working stage re-checks the worker. No user input drives that
/// stage, so without an explicit repaint request egui would sit still --
/// neither animating the spinner nor noticing the channel has a value. The same
/// 80ms `loading_ui::show_while` has always used.
const WORKING_POLL: Duration = Duration::from_millis(80);

/// How many times `wait_for_vault_ready` calls `vault.list_items()` on the
/// schedule the working stage runs it with.
///
/// **One more than the number of delays, not the same as it.**
/// `bw_serve::wait_for_vault_ready` calls first and sleeps second, giving up
/// only once `attempt >= schedule.len()` -- so a 10-delay schedule is 11 calls.
/// That off-by-one is the difference between crediting the readiness phase 100s
/// of network budget and crediting it 110s.
///
/// Reconciled against the real `readiness_schedule` by
/// `the_deadline_covers_every_phase_the_worker_runs`, rather than agreed with it
/// by hand: `readiness_schedule` is not a `const fn` (it builds a `Vec`), so the
/// count cannot be computed where a `const` needs it. A test that recomputes it
/// from the live function is the reconciliation.
const READINESS_ATTEMPTS: u64 = 11;

/// How long the working stage may go on before it ends itself.
///
/// **Derived from what actually bounds each phase, which is not what each phase
/// is named after.** The worker runs `StartupWork::produce`, three phases:
///
///   1. `try_start_backend` -> `bw_serve::run_bw_sync`, a bare
///      `Command::output()` with no timeout. `bw_serve::BACKEND_OP_TIMEOUT` is
///      the number this crate already uses everywhere else as the upper bound
///      on a legitimate backend start or sync (`main`'s wedge check, the
///      picker's probe), so it is what one such phase costs here. Its unlisted
///      sub-step, `wait_for_port_free` up to `bw_serve::PORT_RELEASE_GRACE`
///      (5s), fits inside that 90s rather than adding to it.
///   2. `wait_for_vault_ready` on `readiness_schedule(READINESS_DEADLINE)`.
///      **`READINESS_DEADLINE` does not bound this phase.** It bounds the
///      *sleeps* only -- `readiness_schedule` stops once the accumulated wait
///      would exceed it -- and the phase is sleeps INTERLEAVED WITH network
///      calls, each of which is bounded separately by the bridge's own
///      whole-request budget, `vault_bridge::READ_DEADLINE` -- the sum below
///      reads that constant rather than restating its value, so raising it
///      lengthens this deadline instead of quietly making it too short. (It is
///      `READ_DEADLINE` and not `WRITE_DEADLINE`: `wait_for_vault_ready` probes
///      with `list_items`, a GET on the bridge's `read_agent`.)
///      `readiness_schedule(30s)` yields 10 delays summing 27.75s and therefore
///      11 `list_items()` calls, so the real worst case is 30s of sleeping plus
///      110s of waiting on a backend that answers slowly instead of not at all:
///      ~140s, not 30s.
///   3. `login_ui::check_bw_status_details_bounded()`, unconditional and on the
///      failure path too -- and **the one phase that now bounds ITSELF**. The
///      bare `check_bw_status_details` is a `Command::output()` with no
///      timeout, and while `produce` called it this term was the only one here
///      that described nothing: it charged `BACKEND_OP_TIMEOUT` because "an
///      untimed `bw` spawn costs what the others do", but no clock was on the
///      spawn at all. When this deadline fired the child was still running and
///      the user had watched a frozen spinner through the whole budget. The
///      bounded form waits `login_ui::STATUS_DEADLINE` and then reports
///      "unknown", so the term is read from the constant that actually bounds
///      the phase, the same way phase 2 reads `READ_DEADLINE`.
///
/// Hence 90 + (30 + 11*10) + 30 = **260s**, down 60s from the 320s that
/// credited phase 3 with a backend-start budget it was not spending. A shorter
/// deadline here is the better one, not a concession: the 60s given back was
/// time the window spent refusing to give up on a `bw status` whose only
/// consumer is the toolbar's account label. `STATUS_DEADLINE` is small for
/// exactly that reason (see its own doc) -- losing that phase costs a label,
/// while losing this deadline's race costs the whole sign-in.
///
/// The step before that fixed phase 2: the sum used to credit it 30s and come
/// to 210s, which a slow-but-healthy startup can exceed while still on its way
/// to succeeding.
///
/// Still deliberately generous where generosity is what is at stake: this is a
/// watchdog on a stage the user cannot leave by any other route, and a false
/// timeout on a slow machine throws away a healthy sign-in. Phases 1 and 2
/// remain bounds on the WINDOW rather than on their subprocesses -- 230s of
/// this total is still a budget the worker can overrun while `produce` runs on.
/// Phase 3 is the first one where the worker itself stops waiting.
pub const WORKING_DEADLINE: Duration = Duration::from_secs(
    crate::bw_serve::BACKEND_OP_TIMEOUT.as_secs()
        + crate::bw_serve::READINESS_DEADLINE.as_secs()
        + READINESS_ATTEMPTS * crate::vault_bridge::READ_DEADLINE.as_secs()
        + crate::login_ui::STATUS_DEADLINE.as_secs(),
);

/// What the one window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The sign-in card over the flat backdrop -- `login_ui`'s frame.
    SignIn,
    /// The spinner -- `loading_ui`'s own body, so it is the same spinner the
    /// other windows show and not a second one that drifts.
    Working,
    /// The vault -- `vault_window`'s frame.
    Vault,
}

/// What happened that could move the window on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Sign-in produced a session token.
    SignedIn,
    /// The background work finished and there is a vault to show.
    WorkReady,
    /// There is no vault to show: the work finished without one, OR the worker
    /// died without finishing, OR it has been longer than `WORKING_DEADLINE`
    /// and it is not coming. The window ends and
    /// `main` runs the recovery it has always run -- see
    /// `recover_from_failed_vault_wait`, which is why this is `Close` rather
    /// than a fourth stage that apologises.
    WorkFailed,
    /// **The vault reported a lost session** -- the Lock button, CTRL+L, the
    /// auto-lock timer, or a write that came back 401. The window does NOT go
    /// away for it: it shows the spinner while the teardown runs, which is the
    /// whole of the in-window lock. Caught as a refused close, because all
    /// three lock routes ask for one themselves.
    Locked,
    /// **The teardown reached the point only this thread can pass**: the old
    /// session is gone and the next step is a master password. The window
    /// shows the sign-in card, in place, instead of `main::reauthenticate`
    /// opening one of its own.
    TeardownDone,
}

/// Where an event leaves the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Show(Stage),
    Close,
}

/// The transition table.
///
/// A pure function and not a `match` written inline in the frame closure,
/// because the frame closure is the one thing in this feature no test can run:
/// `eframe::Frame` has no public constructor, so the closure cannot even be
/// called. Everything the window DECIDES lives here, where it can be checked;
/// what is left in the closure is drawing and wiring, and the wiring is pinned
/// by source position in `startup_window_tests`.
///
/// Total rather than exhaustive-with-`unreachable!()`: the pairs that cannot
/// happen (a `WorkReady` while the sign-in card is up) are no-ops rather than
/// panics, because the cost of getting that judgement wrong is the whole app
/// dying at launch and the benefit is nothing the log would not already say.
pub fn advance(stage: Stage, event: Event) -> Next {
    match (stage, event) {
        (Stage::SignIn, Event::SignedIn) => Next::Show(Stage::Working),
        (Stage::Working, Event::WorkReady) => Next::Show(Stage::Vault),
        (Stage::Working, Event::WorkFailed) => Next::Close,
        (Stage::Vault, Event::Locked) => Next::Show(Stage::Working),
        (Stage::Working, Event::TeardownDone) => Next::Show(Stage::SignIn),
        (stage, _) => Next::Show(stage),
    }
}

/// Why the working stage gave up on itself.
///
/// Carried rather than collapsed to a bare `Event::WorkFailed`, because the two
/// are the same decision reached for opposite reasons and the log line is the
/// only thing that will ever tell them apart in the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkFailure {
    /// The worker thread is gone without ever sending -- it panicked, or was
    /// otherwise torn down. `try_recv` says `Disconnected`, which it can only
    /// say once the closure has let go of its own sender.
    WorkerDied,
    /// The worker may well still be alive; the stage has simply been up longer
    /// than [`WORKING_DEADLINE`].
    Deadline,
}

impl WorkFailure {
    /// What to log. `&'static str` so this cannot become a formatting site that
    /// runs 12 times a second on the polling path.
    pub fn reason(self) -> &'static str {
        match self {
            WorkFailure::WorkerDied => {
                "the thread preparing the vault died without answering (it panicked); ending \
                 the setup stage so the startup recovery can run"
            }
            WorkFailure::Deadline => {
                "the vault backend did not finish starting within the setup stage's deadline; \
                 ending the stage so the startup recovery can run, rather than leaving a \
                 spinner up that has no ✕, no tray and no way out"
            }
        }
    }
}

/// What the working stage does when `try_recv` produced no work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkPoll {
    /// Nothing yet, and there is still time. Repaint and look again.
    KeepWaiting,
    /// The stage is over: `Event::WorkFailed`, for this reason.
    Failed(WorkFailure),
}

/// The channel half of the decision: a failed `try_recv` on its own.
///
/// **The bug this exists to make impossible:** the poll arm used to be
/// `Err(_) => request_repaint_after(..)`, which treats a dead worker exactly
/// like a busy one. A `Disconnected` is not "not yet" -- nothing will ever
/// arrive on that channel again, and the stage refuses every close, so treating
/// it as "not yet" is a spinner that spins until Task Manager. Pure and total so
/// it can be asserted directly; `TryRecvError` is `Copy` and has two variants,
/// both spelled out.
pub fn work_channel_poll(err: mpsc::TryRecvError) -> WorkPoll {
    match err {
        mpsc::TryRecvError::Empty => WorkPoll::KeepWaiting,
        mpsc::TryRecvError::Disconnected => WorkPoll::Failed(WorkFailure::WorkerDied),
    }
}

/// The clock half: how long the stage has been up.
///
/// A live worker that never answers -- `bw sync` on a hung network, the case
/// `BACKEND_OP_TIMEOUT` exists for elsewhere -- keeps the channel open forever,
/// so `work_channel_poll` alone would still spin. This is the bound that does
/// not depend on the worker being well behaved at all.
pub fn work_deadline_poll(elapsed: Duration) -> WorkPoll {
    if elapsed >= WORKING_DEADLINE {
        WorkPoll::Failed(WorkFailure::Deadline)
    } else {
        WorkPoll::KeepWaiting
    }
}

/// Both halves, in the order the closure needs them: a dead worker is reported
/// as a dead worker even if the deadline happens to have passed too, because
/// "it panicked" and "it is slow" call for different investigations.
///
/// This is what the frame closure calls, once, so that deleting the call is a
/// deletion of the whole watchdog rather than of one half of it -- and so that
/// the closure holds no `if` of its own about any of this.
pub fn poll_working(err: mpsc::TryRecvError, elapsed: Duration) -> WorkPoll {
    match work_channel_poll(err) {
        WorkPoll::Failed(why) => WorkPoll::Failed(why),
        WorkPoll::KeepWaiting => work_deadline_poll(elapsed),
    }
}

/// **Ask this window to close, and stand the refusal down first.**
///
/// A function rather than two lines in the frame closure because the two lines
/// are not independent and the second one is invisible to every test that
/// checks the first. `closing.decide()` on its own leaves the window exactly as
/// unleaveable as the bug: the stage stops ending itself, the ✕ is still
/// `Disabled`, and only Alt+F4 gets through. `send_viewport_cmd` on its own is
/// worse -- the refusal below cancels the window's own exit. Here they cannot be
/// separated, and `the_stage_ending_itself_actually_asks_the_window_to_close`
/// runs this against a real `egui::Context` and reads the command back out.
///
/// The flag is set BEFORE the command deliberately: eframe reports this very
/// command back as a `close_requested` on a later frame, while `Stage::Working`
/// is still the stage being drawn.
fn close_this_window(ctx: &egui::Context, closing: &mut Closing) {
    closing.decide();
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

/// **The working stage's refusal to be closed by anything but itself.**
///
/// The ✕ is drawn `Disabled` and registers no interaction, so this is about the
/// routes that do not go through the chrome at all -- Alt+F4 and the system
/// menu. Refusing them keeps the `bw serve` the worker is holding off the port
/// the recovery needs.
///
/// Returns whether it actually refused, so a test can tell "declined to refuse"
/// from "was never asked" -- two states that are identical in the viewport
/// output when no command is sent.
fn refuse_close_while_working(ctx: &egui::Context, closing: Closing) -> bool {
    if closing.decided() || !ctx.input(|i| i.viewport().close_requested()) {
        return false;
    }
    log::info!(
        "the single window was asked to close while the vault backend was still starting; \
         refusing, so the backend it is holding is not orphaned on the port the recovery needs"
    );
    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    true
}

/// What gets logged when the stage gives up, as a value rather than as a
/// `log::error!` the tests cannot see.
///
/// Split out so [`WorkFailure::reason`] is on a path a test can run. Folded into
/// the `log::error!` it used to be, the two reasons could be replaced by one
/// fixed string with every test green -- `assert_ne!(WorkerDied.reason(),
/// Deadline.reason())` would then be a true statement about two functions
/// nothing called, since both variants reach the same `Event::WorkFailed` and
/// the same `Next::Close` and the log line is their only user-visible
/// difference.
fn give_up_message(why: WorkFailure, elapsed: Duration) -> String {
    format!("{} (after {elapsed:?})", why.reason())
}

/// **What a `WorkPoll::Failed` does to the window**, whole: name the reason,
/// take the transition, and -- only if the transition really is out of the
/// window -- close it.
///
/// `advance(Stage::Working, Event::WorkFailed)` is spelled here rather than in
/// the closure so that it is under test. Swapped to `Event::WorkReady` this
/// returns `Next::Show(Stage::Vault)` and sends nothing: a `Vault` stage whose
/// `vault_fn` is `None`, which the `Vault` arm draws as a permanently blank
/// window. That is a worse outcome than the bug and it used to be one token
/// away.
fn give_up_working(
    ctx: &egui::Context,
    closing: &mut Closing,
    why: WorkFailure,
    elapsed: Duration,
) -> Next {
    log::error!("{}", give_up_message(why, elapsed));
    let next = advance(Stage::Working, Event::WorkFailed);
    if let Next::Close = next {
        close_this_window(ctx, closing);
    }
    next
}

/// **Whether the working stage has already decided to end**, as a type rather
/// than as a `bool` a single token can invert.
///
/// The value the stage starts on is load-bearing twice over, and neither use is
/// visible from outside the frame closure, which no test can call. Started at
/// "already decided", `refuse_close_while_working` returns on its first branch
/// forever -- the stage refuses NOTHING, so an Alt+F4 or a system-menu close
/// during `bw serve`'s startup is honoured and strands a listening backend on
/// the port the recovery needs -- and the closure's drain guard never opens, so
/// the worker's answer is never taken: the vault never appears, the watchdog
/// never runs, and the spinner spins forever behind a ghosted close control
/// with no tray icon and no Quit item anywhere in the process. That is the
/// originally reported bug, strictly worse, and as a bare `bool` it was one
/// token away with the whole suite green.
///
/// So the field is private to this module: [`Closing::not_yet`] is the only way
/// to make one and [`Closing::decide`] the only way to move one. The starting
/// value is therefore something a test can CALL -- see
/// `the_stage_starts_out_refusing_every_close_and_still_draining_the_worker` --
/// rather than a literal inside a closure a test can only read.
mod closing {
    /// Constructed only by [`Closing::not_yet`]. The field is private so that
    /// `= true` cannot be written at a call site at all.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Closing(bool);

    impl Closing {
        /// What the working stage starts on: nothing has asked to close yet, so
        /// every close that arrives belongs to somebody else.
        pub const fn not_yet() -> Self {
            Self(false)
        }

        /// Whether the stage has decided to end itself.
        pub const fn decided(self) -> bool {
            self.0
        }

        /// Record that it has. Reached from `close_this_window` and nowhere
        /// else, which is counted by
        /// `the_refusal_starts_armed_and_is_stood_down_in_exactly_one_place`.
        pub fn decide(&mut self) {
            self.0 = true;
        }
    }
}

use closing::Closing;

/// What one run of the single window produced.
pub struct StartupOutcome<P> {
    /// The session token sign-in produced. `None` means the user closed the
    /// window on the card instead -- the same fact `login_ui::
    /// run_login_flow_for` reports with `None`, and it costs the same thing.
    pub token: Option<String>,
    /// Whatever the caller's `prepare` returned. `None` means the window ended
    /// before the work landed, which after [`run`] returns can only be a
    /// sign-in the user abandoned.
    pub prepared: Option<P>,
    /// The vault session's outcome -- lock, re-auth, Preferences, account
    /// switch, or a plain close. `Some` **only if the vault stage was actually
    /// entered**, which is what makes this the reachability evidence: a
    /// `None` here on a launch that signed in successfully means the user was
    /// dropped to the tray without their vault, which is precisely the half of
    /// the report that was never about window count.
    pub vault: Option<vault_window::VaultWindowResult>,
    /// Every stage the window actually PAINTED, in order, first entry first.
    ///
    /// Recorded because "the transition table is correct" and "the vault is
    /// ever shown" are different claims, and this codebase has shipped the
    /// first without the second more than once -- three functions at a time,
    /// on one occasion. `main` logs it, so a launch that never reached
    /// `Vault` says so in the log rather than being something a user has to
    /// notice and report.
    pub stages: Vec<Stage>,
}

/// **The one place this module puts an OS window on the screen**, and the one
/// place it asks for the foreground.
///
/// Both hosts go through it, and that is load-bearing rather than tidy.
/// `foreground::tests::every_window_this_crate_opens_asks_to_be_brought_to_the_front`
/// counts this file's eframe-launch sites and the raise calls beside them and
/// requires them to be one apiece -- a claim worth making only while there
/// really is one site. **Neither needle is spelled out in this doc**: that test
/// counts over the raw source, so a comment naming what it looks for would
/// inflate its count and it would be guarding its own prose.
/// A second host with its own copy of this block would have been a second
/// window-opening site, a second first-frame styling pass, and a second chance
/// to forget the raise; `app_window` would have had to be relisted as opening
/// two windows, which is the strictly weaker statement.
///
/// **The first frame draws nothing.** egui applies a new font set at the START
/// of the next frame, not the one that calls `set_fonts` -- drawing
/// Archivo-styled text in this same frame would look up a family that does not
/// exist yet and panic. The OS window does exist by this first painted frame
/// (the same hook `round_window_corners` uses), which is why the raise is here
/// and why both sub-frames are built `pre_styled`: a vault frame raising the
/// window again would yank forward a window the user may have deliberately
/// sent behind something while `bw serve` started.
fn run_the_one_window(
    options: eframe::NativeOptions,
    mut draw: impl FnMut(&mut egui::Ui, &mut eframe::Frame) + 'static,
) {
    let mut styled = false;
    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, frame| {
        if !styled {
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            login_ui::round_window_corners(WINDOW_TITLE);
            let _ = foreground::raise_window(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }
        draw(ui, frame);
    });
}

/// **How far a lock has actually GOT**, as one ordered value.
///
/// This was two independent `bool` fields, `worker_started` and
/// `teardown_reported`, and three consecutive reviews found the same class of
/// defect on them. The guards pinned WHERE each `= true` is written -- first
/// by byte distance, then by adjacency -- and nothing pinned that it STAYS
/// written. Both of these were measured green across the whole suite:
///
/// * `self.teardown_reported = false;` appended to the `NeedsSignIn` arm. The
///   arm's write is still its first statement, the count is still two, the
///   rule and its table and the call-site pin are untouched -- and a worker
///   that reported a step and then died has its teardown RETRACTED, so `main`
///   runs a second teardown of a session already dismantled.
/// * `self.worker_started = false;` appended to the lock catch, below the
///   `spawn < started < claim` ordering the position pin measures. The
///   retraction then never fires: `relocked` stays true, `main` skips its own
///   recovery, and a worker that died having torn nothing down leaves the
///   vault showing "locked" with `bw serve` still holding the session. That
///   is the v0.5.0 defect verbatim.
///
/// A third positional pin would not have caught either, because neither moves
/// anything the position pins look at. So the two facts are ONE value with an
/// ORDER on it; a lock only ever moves forward along that order
/// ([`lock_reach_after`], a pure total function with its own exhaustive table,
/// for the same reason [`session_torn_down`] and [`retracts_the_teardown`]
/// are); and the value is kept in [`lock_stage::LockStage`], whose field is
/// private to a module the rest of this file is not inside. `= false` in any
/// spelling -- a literal, a `match` arm, `std::mem::replace`, a shadowing
/// binding -- is a TYPE ERROR rather than a silent regression, and the one
/// remaining wrong write that still compiles has to name `LockStage::fresh`
/// out loud, which `the_lock_reach_is_not_assignable_only_advanceable` bans.
///
/// The order is not an arrangement of convenience. A step can only be reported
/// by a worker, and a worker only exists if the spawn returned, so
/// `StepReported` implies `WorkerStarted` in the code as much as in the
/// ordering. [`retracts_the_teardown`] keeps all eight rows of its own table;
/// two of them are merely states this value cannot be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LockReach {
    /// No lock has been caught in this window yet, or the one that was caught
    /// found its `FnOnce` teardown already spent and started nothing.
    Nothing,
    /// The lock arm spawned the teardown worker. A THREAD EXISTS; nothing is
    /// yet known to have been torn down, which is the whole reason
    /// [`retracts_the_teardown`] exists.
    WorkerStarted,
    /// The worker reported one of its two steps, so the teardown really did
    /// get underway and must not be retracted.
    StepReported,
}

/// **A lock never un-reaches what it reached.**
///
/// The later of the two, always -- so no call, wherever it is written and
/// whatever it is passed, can move a lock backwards. That is the property the
/// two `bool` fields did not have and could not be given: every `= false` this
/// file's reviews found was a legal write of a legal value.
///
/// Total and pure, with a nine-row table in
/// `a_lock_only_ever_reaches_further`, because a rule this small written
/// inline is exactly what the two inversions of [`retracts_the_teardown`]'s
/// condition proved cannot be trusted to a source pin.
pub fn lock_reach_after(before: LockReach, reached: LockReach) -> LockReach {
    if reached > before {
        reached
    } else {
        before
    }
}

/// **The only place a [`LockReach`] can be kept**, and the reason the wrong
/// value is unrepresentable rather than merely unpinned.
///
/// A private field in a CHILD module: privacy in Rust reaches the defining
/// module and its descendants, and the rest of this file is neither. So
/// outside these few lines there is no expression at all that lowers a stage
/// -- `self.stage.0 = ..` does not compile, and `self.stage = false` does not
/// typecheck. The only mutation reachable from [`InWindowLock`] is
/// [`LockStage::reached`], which routes through [`lock_reach_after`].
mod lock_stage {
    use super::LockReach;

    /// The lock's progress. Deliberately NOT `Clone`, `Copy` or `Default`: a
    /// copy is a stage that can go stale, and a `Default` is a second way to
    /// spell "back to the beginning" without naming it.
    #[derive(Debug)]
    pub struct LockStage(LockReach);

    impl LockStage {
        /// A window that has not locked. Called ONCE, in
        /// [`super::InWindowLock::new`], and banned everywhere else by
        /// `the_lock_reach_is_not_assignable_only_advanceable`.
        pub fn fresh() -> Self {
            Self(LockReach::Nothing)
        }

        /// Records that the lock got this far. Monotone by construction.
        pub fn reached(&mut self, reached: LockReach) {
            self.0 = super::lock_reach_after(self.0, reached);
        }

        /// A teardown worker thread was spawned. See
        /// [`super::retracts_the_teardown`], which is the only reader.
        pub fn worker_started(&self) -> bool {
            self.0 >= LockReach::WorkerStarted
        }

        /// The worker reported at least one of its two steps.
        pub fn teardown_reported(&self) -> bool {
            self.0 >= LockReach::StepReported
        }
    }
}

use lock_stage::LockStage;

/// **The lock's touch points, in one value, for both hosts.**
///
/// [`run_from_vault`] catches the lock today; [`run`] has to catch the same
/// one when the startup window learns to survive it. The sequence -- cancel
/// the frame's own close, end the pre-lock session through `finish`, start the
/// teardown worker, answer its two steps, rebuild, and route every write of
/// `relocked` through [`session_torn_down`] -- is the most heavily guarded in
/// this module, and a second copy of it inside the startup host is exactly the
/// move the recorded design rejects by name. Both hosts already share
/// [`run_the_one_window`]; this is that argument one level in.
///
/// **A value rather than a handful of free functions**, because the state the
/// sequence turns on has to survive between frames and must not be writable
/// from anywhere else: the two `FnOnce` closures (see [`run_from_vault`]'s
/// floor note on what a second lock in one session costs), the two channels,
/// and the two flags the retraction is decided from. `worker_started` and
/// `teardown_reported` are private and are each written in exactly one arm --
/// which is what `the_retraction_asks_the_rule_rather_than_deciding_it_inline`
/// pins, both count and position, over this body.
///
/// **The three `Rc` cells are the HOST's, cloned in.** The host reads them
/// after the event loop returns, which is the only way anything gets out of an
/// eframe update closure. This value is moved INTO that closure and never
/// comes back, so the tail is [`finish_the_locked_session`] over the host's
/// own clones rather than a method here.
struct InWindowLock<T, B> {
    /// The teardown, taken by the first lock. `None` afterwards, which is what
    /// makes the second lock of one session report
    /// [`LockProgress::TeardownAlreadySpent`] rather than claim a worker.
    teardown: Option<T>,
    /// The rebuild, taken by [`TeardownStep::Finished`].
    rebuild_vault: Option<B>,
    /// **The worker gets the ONLY sender.** Cloned into the thread instead,
    /// this side would hold a sender that never sends and never drops, so a
    /// worker that panicked mid-teardown would answer `Empty` forever -- and
    /// the working stage refuses every close. `try_recv` says `Disconnected`
    /// only because nothing is kept here.
    step_tx: Option<mpsc::Sender<TeardownStep>>,
    step_rx: mpsc::Receiver<TeardownStep>,
    /// The other direction: the master password the card produces. In an
    /// `Option` so that LEAVING the sign-in card without a token drops it --
    /// the worker's own `recv` then fails, which is how a user who closes the
    /// window on the card reaches the declined arm instead of blocking a
    /// thread on a password that is never coming.
    token_tx: Option<mpsc::Sender<String>>,
    token_rx: Option<mpsc::Receiver<String>>,
    /// **The two facts that turn "a thread was spawned" into "the teardown
    /// actually got underway".** `teardown` is `FnOnce`, so at most one worker
    /// ever starts in one window's life and these need no per-lock reset. See
    /// [`LockProgress::TeardownNeverRan`], which is what the pair is read into.
    ///
    /// **One ordered value and not two `bool`s**, so that no write anywhere
    /// can un-report a step or un-start a worker: see [`LockReach`] for the
    /// two measured survivors that shape cost, and
    /// [`lock_stage::LockStage`] for why lowering it does not compile.
    stage: LockStage,
    /// The PRE-LOCK session's outcome, written by the lock catch and read by
    /// the rebuild and by [`finish_the_locked_session`].
    result: Rc<RefCell<Option<vault_window::VaultWindowResult>>>,
    relocked: Rc<RefCell<bool>>,
    vault_handles: Rc<RefCell<Option<vault_window::VaultFrameHandles>>>,
}

impl<T, B> InWindowLock<T, B>
where
    T: FnOnce(&mpsc::Sender<TeardownStep>, mpsc::Receiver<String>) + Send + 'static,
    B: FnOnce(
        Option<crate::settings::Settings>,
    ) -> Option<(vault_window::VaultFrameFn, vault_window::VaultFrameHandles)>,
{
    /// Both channels are made HERE and not by the host, so neither host can
    /// wire them to each other's ends.
    fn new(
        teardown: T,
        rebuild_vault: B,
        result: Rc<RefCell<Option<vault_window::VaultWindowResult>>>,
        relocked: Rc<RefCell<bool>>,
        vault_handles: Rc<RefCell<Option<vault_window::VaultFrameHandles>>>,
    ) -> Self {
        let (step_tx, step_rx) = mpsc::channel::<TeardownStep>();
        let (token_tx, token_rx) = mpsc::channel::<String>();
        Self {
            teardown: Some(teardown),
            rebuild_vault: Some(rebuild_vault),
            step_tx: Some(step_tx),
            step_rx,
            token_tx: Some(token_tx),
            token_rx: Some(token_rx),
            stage: LockStage::fresh(),
            result,
            relocked,
            vault_handles,
        }
    }

    /// **The lock catch**, called from either host's vault stage every frame.
    ///
    /// The vault frame asks for the close itself on all three lock routes (the
    /// account menu's Lock, CTRL+L, and the auto-lock timer), so the lock
    /// arrives as a close with the flag already set -- which is why no lock
    /// site needed editing for this feature.
    ///
    /// Answers whether the lock was caught. The caller's only remaining job is
    /// the stage transition, which is the machine's and not the lock's.
    fn catch_the_lock(
        &mut self,
        ctx: &egui::Context,
        vault_fn: &mut Option<vault_window::VaultFrameFn>,
    ) -> bool {
        let lost = self.vault_handles.borrow().as_ref().is_some_and(|h| h.lost_session());
        match vault_close(ctx.input(|i| i.viewport().close_requested()), lost) {
            // Nothing to do, and deliberately spelled out rather than folded
            // into a `_`: "no close" and "a close we honour" are the two
            // answers that must NOT keep the window, and a wildcard here would
            // swallow a fourth answer added later.
            VaultClose::Ignore | VaultClose::LetGo => false,
            VaultClose::Lock => {
                // The window's own exit, cancelled. Without this the vault
                // frame's `ViewportCommand::Close` is honoured and the window
                // goes -- which is the blink.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                // **This vault session ends here**, the way every vault
                // session ends: `finish` persists the geometry and reads the
                // outcome cells. Read NOW, because the frame that reported the
                // lock is about to be dropped and replaced -- and its
                // `edited_settings` is a visit to the gear that would
                // otherwise be silently lost.
                let ended = self.vault_handles.borrow_mut().take();
                if let Some(handles) = ended {
                    *self.result.borrow_mut() = Some(handles.finish());
                }
                *vault_fn = None;
                // THE TEARDOWN GOES TO A THREAD, for the reason [`run`]'s
                // `prepare` does: it stops and restarts `bw serve`, which is
                // seconds at best, and run here it would freeze the window on
                // the very frame that is supposed to start showing the
                // spinner.
                //
                // **`relocked` is set from whether the worker ACTUALLY
                // STARTED, not from having reached this arm.** The difference
                // is the second lock of one session: the closures are
                // `FnOnce`, so `teardown.take()` answers `None` there and
                // nothing is torn down. Set unconditionally, this flag would
                // tell the caller "the teardown has already run" about a lock
                // that tore nothing down, and the caller -- whose whole use
                // for it is to SKIP its own recovery -- would skip the only
                // teardown that lock was ever going to get. The vault would
                // report itself locked with the cache still full and
                // `bw serve` still holding a live session, which is the v0.5.0
                // defect in a new place and invisible to every test that
                // cannot run a frame.
                let progress = if let (Some(teardown), Some(step_tx), Some(token_rx)) =
                    (self.teardown.take(), self.step_tx.take(), self.token_rx.take())
                {
                    std::thread::spawn(move || {
                        let step_tx = step_tx;
                        teardown(&step_tx, token_rx);
                    });
                    self.stage.reached(LockReach::WorkerStarted);
                    LockProgress::TeardownStarted
                } else {
                    LockProgress::TeardownAlreadySpent
                };
                let was = *self.relocked.borrow();
                *self.relocked.borrow_mut() = session_torn_down(was, progress);
                true
            }
        }
    }

    /// The card produced a master password; down the channel the worker is
    /// blocked on. The frame thread only ever draws -- it is the worker that
    /// authenticates and starts the backend.
    fn hand_over_the_token(&self, produced: String) {
        if let Some(token_tx) = self.token_tx.as_ref() {
            let _ = token_tx.send(produced);
        }
    }

    /// **Both of the teardown's steps**, answered from either host's working
    /// stage. `Err` is the channel's own answer, handed straight back for
    /// [`poll_working`] -- not swallowed here, because the watchdog is the
    /// stage's and is shared with the startup host.
    fn answer_the_teardown(
        &mut self,
        vault_fn: &mut Option<vault_window::VaultFrameFn>,
    ) -> Result<Event, mpsc::TryRecvError> {
        match self.step_rx.try_recv() {
            Ok(TeardownStep::NeedsSignIn) => {
                self.stage.reached(LockReach::StepReported);
                log::info!(
                    "the lock's teardown is done and the vault needs a master password; \
                     showing the sign-in card in the window that is already open"
                );
                Ok(Event::TeardownDone)
            }
            Ok(TeardownStep::Finished) => {
                self.stage.reached(LockReach::StepReported);
                // Dropped here rather than left alive for the rest of the
                // window: the worker is finished, and a sender this side keeps
                // would stop the channel ever reporting `Disconnected` again.
                self.token_tx = None;
                // The gear visit the PRE-LOCK session produced -- read out of
                // the cell the lock catch's `finish` wrote, and cloned rather
                // than borrowed across the call because `build` is a caller's
                // closure and this cell is alive for the rest of the window.
                // `main` has not written `settings.json` yet, so this is the
                // only place the rebuilt vault can learn the new policy.
                let edited_before_lock =
                    self.result.borrow().as_ref().and_then(|before| before.edited_settings.clone());
                let built = self.rebuild_vault.take().and_then(|build| build(edited_before_lock));
                // **The session is LIVE again on the `Some` arm, so the
                // teardown stops being outstanding there and only there.**
                // Left set through a rebuild, a session that locked, signed
                // back in and was then locked AGAIN would tell the caller a
                // teardown had run when the second lock's `FnOnce` teardown
                // was already spent -- and that lock would be honoured by
                // nobody.
                let (event, progress) = match built {
                    Some((rebuilt, handles)) => {
                        *vault_fn = Some(rebuilt);
                        *self.vault_handles.borrow_mut() = Some(handles);
                        (Event::WorkReady, LockProgress::VaultRebuilt)
                    }
                    None => (Event::WorkFailed, LockProgress::RebuildFailed),
                };
                let was = *self.relocked.borrow();
                *self.relocked.borrow_mut() = session_torn_down(was, progress);
                Ok(event)
            }
            // **Not `Err(_) =>` folded in above.** The kinds are the
            // watchdog's to tell apart, and this arm exists only to hand the
            // error on unchanged.
            Err(err) => Err(err),
        }
    }

    /// **The worker died having reported NOTHING.**
    ///
    /// `relocked` was set from the spawn returning, which says a thread exists
    /// and not that the teardown ran; if the frame thread's forwarding of the
    /// two channel ends never arrived, or the worker panicked before it asked
    /// for the master password, nothing was drained, stopped or cleared.
    /// Retract the claim here so the caller runs the recovery that is now the
    /// only teardown this lock will get.
    ///
    /// The CONDITION is not spelled out here. It was, and two one-token
    /// inversions of it were measured green across the whole suite -- so it is
    /// [`retracts_the_teardown`], a pure function with an exhaustive table,
    /// for the same reason [`session_torn_down`] is. Its doc carries why each
    /// of the three inputs is load-bearing, `Deadline` included.
    fn retract_if_the_teardown_never_ran(&mut self, why: WorkFailure) {
        if retracts_the_teardown(why, self.stage.worker_started(), self.stage.teardown_reported()) {
            log::error!(
                "the lock's teardown worker ended without ever reporting a step, so nothing \
                 was torn down; the session is reported as still live and the caller's own \
                 lock recovery runs"
            );
            let was = *self.relocked.borrow();
            *self.relocked.borrow_mut() = session_torn_down(was, LockProgress::TeardownNeverRan);
        }
    }
}

/// **The tail of a session that may have locked**, shared by both hosts for
/// the same reason the catch is.
///
/// The vault frame that is still up when the window ends -- an ordinary close,
/// or the one rebuilt after a lock -- ends the way every vault session ends:
/// `finish` writes the geometry and reads the outcome cells. `None` there is
/// the failed-rebuild path, where the lock catch took the handles and nothing
/// put any back.
///
/// **MERGED with the lock's own result, not substituted for it.** This used to
/// be an unconditional overwrite, and `build_frame` gives every frame a FRESH
/// `edited_settings` cell -- so a gear visit made before the lock was thrown
/// away by the very session the lock catch's `finish` exists to preserve. See
/// [`carry_settings_forward`] for which field survives from which session and
/// why it is only the one.
///
/// A free function and not a method on [`InWindowLock`], because the lock is
/// moved into the frame closure and does not come back: what the host still
/// holds out here is its own clones of the two cells.
fn finish_the_locked_session(
    result: &Rc<RefCell<Option<vault_window::VaultWindowResult>>>,
    vault_handles: &Rc<RefCell<Option<vault_window::VaultFrameHandles>>>,
) -> Option<vault_window::VaultWindowResult> {
    let rebuilt = vault_handles.borrow().as_ref().map(|handles| handles.finish());
    carry_settings_forward(result.borrow_mut().take(), rebuilt)
}

/// Runs the single window. Blocks until it closes.
///
/// `prepare` runs on a detached worker thread with the session token, and is
/// everything that used to happen between the login window closing and the
/// tray appearing: start `bw serve`, wait for it to answer, ask who is signed
/// in. It must not touch anything this thread owns -- that is what `Send`
/// enforces -- and in particular it must not need the tray, which is why the
/// resettle sequence is not expressible here and does not try to be.
///
/// `build_vault` runs on THIS thread, in the frame that drains the worker's
/// answer, and is the caller's chance to turn the prepared work into a vault
/// frame: populate the cache, then `vault_window::build_frame`. It answers
/// `None` when there is no vault to show, which ends the window and leaves
/// `main` to run its existing recovery.
pub fn run<P, W, V>(
    account: Option<(&Path, &Account)>,
    first_run: bool,
    working_message: &'static str,
    prepare: W,
    build_vault: V,
) -> StartupOutcome<P>
where
    P: Send + 'static,
    W: FnOnce(String) -> P + Send + 'static,
    V: FnOnce(&str, &mut P) -> Option<(vault_window::VaultFrameFn, vault_window::VaultFrameHandles)>
        + 'static,
{
    // **The vault window's placement, read the way the vault window reads it**
    // -- by calling its own `initial_placement`, exactly as `login_ui` already
    // does, so this window opens where the vault will be and the vault does
    // not move when it arrives. That is the whole of "one window" as the user
    // experiences it: not a window count, but nothing jumping.
    let placement = vault_window::initial_placement(
        crate::settings::default_path()
            .as_deref()
            .and_then(|path| crate::settings::Settings::load(path).vault_window),
        &login_ui::monitor_work_areas(),
    );
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([placement.width as f32, placement.height as f32])
        // The VAULT window's viewport, not the login window's, because this
        // window ends up being the vault window. `with_resizable(true)` is
        // inert on its own under `with_decorations(false)` -- the grabbable
        // edges are painted by `login_ui::draw_resize_handles`, which the
        // vault frame calls and the login frame does not. So the window is
        // un-resizable while the card is up and resizable once the vault
        // arrives, with no viewport command needed to switch: the affordance
        // is drawn or it is not.
        .with_resizable(true)
        .with_min_inner_size([
            crate::settings::MIN_VAULT_WINDOW_SIZE.0 as f32,
            crate::settings::MIN_VAULT_WINDOW_SIZE.1 as f32,
        ])
        .with_decorations(false)
        .with_icon(theme::window_icon());
    if let Some((x, y)) = placement.position {
        viewport = viewport.with_position([x as f32, y as f32]);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    // `pre_styled: true` and `close_on_success: false`: this window's own first
    // frame installs the fonts, rounds the corners and raises it, and a
    // produced token must NOT close the window -- it has two more states to
    // enter. See `login_ui::build_login_frame`.
    let (_login_options, mut login_fn, login_handles) =
        login_ui::build_login_frame(account, first_run, true, false);

    // Read back after the event loop returns. `Rc<RefCell<_>>` rather than
    // return values because the update closure is `FnMut + 'static` and cannot
    // hand anything back -- the same handoff every window in this crate uses,
    // and safe for the same reason: eframe runs the closure on this thread,
    // which is blocked inside `run_ui_native` for the whole time.
    let token: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let prepared: Rc<RefCell<Option<P>>> = Rc::new(RefCell::new(None));
    // The HANDLES, not the frame: the frame is `FnMut` and stays owned by the
    // closure, while `finish` -- the geometry write and the four-cell outcome
    // read -- has to be callable out here, after the window is gone.
    let vault_handles: Rc<RefCell<Option<vault_window::VaultFrameHandles>>> =
        Rc::new(RefCell::new(None));
    let stages: Rc<RefCell<Vec<Stage>>> = Rc::new(RefCell::new(Vec::new()));

    let token_for_closure = token.clone();
    let prepared_for_closure = prepared.clone();
    let vault_handles_for_closure = vault_handles.clone();
    let stages_for_closure = stages.clone();

    // **In an `Option`, and handed to the worker whole.** It used to be cloned
    // into the thread, which left the original alive inside the closure for the
    // window's whole life -- so a worker that panicked before sending left the
    // receiver with a sender that would never send and never drop, and
    // `try_recv` answered `Empty` forever instead of `Disconnected`. The closure
    // must hold the receiving end and nothing else.
    let (work_tx, work_rx) = mpsc::channel::<P>();
    let mut work_tx = Some(work_tx);
    let mut prepare = Some(prepare);
    let mut build_vault = Some(build_vault);
    let mut vault_fn: Option<vault_window::VaultFrameFn> = None;
    let mut stage = Stage::SignIn;
    // When sign-in was accepted. The span from here to the first vault frame is
    // the number this change is judged by -- everything the user experiences as
    // "and then it took a while" -- and it is not visible from either end
    // alone, which is why it is measured here rather than inside `prepare`.
    let mut signed_in_at: Option<Instant> = None;
    // When the working stage was entered -- the stopwatch `WORKING_DEADLINE` is
    // read against. Its own binding rather than a reuse of `signed_in_at`, whose
    // job is a log line: one of the two could reasonably move later without the
    // other, and only this one is load-bearing.
    let mut working_since: Option<Instant> = None;
    // Set once the stage has decided to end. The `close_requested` guard below
    // refuses every close, INCLUDING the one this module sends itself: eframe
    // reports `ViewportCommand::Close` back as a `close_requested` on the next
    // frame, and the working stage is still the stage being drawn when it
    // arrives. Without this flag the stage cancels its own exit and the window
    // is exactly as unleaveable as before.
    //
    // A `Closing` and not a `bool`: what it starts as decides both whether the
    // stage refuses anything at all and whether the worker's answer is ever
    // drained, and the token that would say so sits inside a closure no test can
    // call. See the type's own doc.
    let mut closing = Closing::not_yet();

    run_the_one_window(options, move |ui, frame| {
        // Recorded on the frame the stage is actually PAINTED, not on the frame
        // the transition is decided, and deduplicated so this is a list of
        // stages rather than a list of frames. See `StartupOutcome::stages`:
        // this is the difference between a transition table that is right and a
        // window that ever gets there.
        if stages_for_closure.borrow().last() != Some(&stage) {
            stages_for_closure.borrow_mut().push(stage);
            log::info!("single window: showing {stage:?}");
        }

        match stage {
            Stage::SignIn => {
                login_fn(ui, frame);
                // The card records its token and does NOT close the window
                // (`close_on_success: false`); this is where that token is
                // noticed. `take_token` takes, so this cannot fire twice.
                if let Some(produced) = login_handles.take_token() {
                    *token_for_closure.borrow_mut() = Some(produced.clone());
                    signed_in_at = Some(Instant::now());
                    // THE WORK GOES TO A THREAD. Everything `prepare` does is
                    // synchronous and slow -- a `bw serve` cold start alone is
                    // regularly several seconds -- and doing any of it here
                    // would freeze the window on the frame that is supposed to
                    // start showing the spinner.
                    if let Some(prepare) = prepare.take() {
                        // `take`, not `clone`: the worker gets the only sender,
                        // so its death is a `Disconnected` the stage can act on.
                        if let Some(work_tx) = work_tx.take() {
                            std::thread::spawn(move || {
                                let _ = work_tx.send(prepare(produced));
                            });
                        }
                    }
                    if let Next::Show(next) = advance(stage, Event::SignedIn) {
                        stage = next;
                        if next == Stage::Working {
                            working_since = Some(Instant::now());
                        }
                    }
                    ui.ctx().request_repaint();
                }
            }
            Stage::Working => {
                // **This stage cannot be closed, and its ✕ says so.** It no
                // longer owns the only handle to a `bw serve` that is starting
                // up: the worker publishes its `Child` through
                // `StartupChildHandoff` the instant it spawns, and `main`
                // claims it before the arms split, so a close here strands
                // nothing and the recovery `main` then runs is handed the real
                // process to stop. What a close here still costs is the work
                // itself -- the whole `prepare` result is discarded, and the
                // user is sent back through the master password for a backend
                // that was very likely seconds from ready. That is the reason
                // the refusal stays, and it is a worse-experience reason now
                // rather than a stranded-port one. The spinner now wears the
                // same heading as every other window, so there IS a ✕ on this
                // stage; `CloseControl::Disabled` draws it ghosted and registers
                // no interaction for it, rather than leaving it looking live and
                // silently refusing every click. The guard below is therefore
                // about the routes that do not go through the chrome at all --
                // Alt+F4 and the system menu.
                //
                // **What bounds it is below, and it is not the readiness
                // probe.** This comment used to claim the probe's own deadline
                // ended the stage; it does not. The probe is one of three
                // phases inside `prepare` and the two around it are untimed
                // `bw` spawns, and a `prepare` that panics answers nothing at
                // all. Since there is no tray icon yet at this point in
                // `main` -- `tray::build_tray` runs after this window returns
                // -- a stage that never ends has no Quit anywhere in the
                // process, and Task Manager is the only way out. So the poll
                // arm ends the stage on a dead worker, and `WORKING_DEADLINE`
                // ends it on a live one that never answers.
                match loading_ui::draw_spinner_body(
                    ui,
                    working_message,
                    login_ui::CloseControl::Disabled,
                ) {
                    // Minimising strands nothing, so it is served.
                    login_ui::ChromeAction::Minimize => ui
                        .ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                    // Unreachable while the ✕ is disabled, and deliberately not
                    // wired to a close: if the chrome ever stops honouring
                    // `CloseControl`, the failure is a ✕ that does nothing
                    // rather than an orphaned `bw serve`.
                    login_ui::ChromeAction::Close | login_ui::ChromeAction::None => {}
                }

                refuse_close_while_working(ui.ctx(), closing);

                // **Not polled once the stage has decided to stop.** After an
                // `Ok(work)` is drained the worker has ended and dropped the
                // only sender; if `build_vault` answered `None` the stage stays
                // `Working` and the `request_repaint` below guarantees another
                // Working frame, whose `try_recv` would now say `Disconnected`.
                // That is a true fact about the channel and a false story about
                // the run -- it would be logged at error level as "the thread
                // preparing the vault died without answering (it panicked)" on
                // the one path that already logged the real reason a frame
                // earlier. `closing` is set only by the two exits, each of which
                // has already asked the window to close, so nothing live is
                // skipped here: the deadline cannot still be owed to a stage
                // that has already ended itself.
                if !closing.decided() {
                    match work_rx.try_recv() {
                        Ok(mut work) => {
                            let signed_in = token_for_closure.borrow().clone();
                            let built = match (build_vault.take(), signed_in.as_deref()) {
                                (Some(build), Some(token)) => build(token, &mut work),
                                _ => None,
                            };
                            *prepared_for_closure.borrow_mut() = Some(work);
                            let event = match built {
                                Some((vault, handles)) => {
                                    vault_fn = Some(vault);
                                    *vault_handles_for_closure.borrow_mut() = Some(handles);
                                    Event::WorkReady
                                }
                                None => Event::WorkFailed,
                            };
                            match advance(stage, event) {
                                Next::Show(next) => {
                                    if let Some(at) = signed_in_at {
                                        log::info!(
                                            "single window: vault ready {:?} after sign-in \
                                             was accepted",
                                            at.elapsed()
                                        );
                                    }
                                    stage = next;
                                }
                                Next::Close => {
                                    log::warn!(
                                        "the single window has no vault to show; closing so \
                                         the startup recovery can run"
                                    );
                                    close_this_window(ui.ctx(), &mut closing);
                                }
                            }
                            ui.ctx().request_repaint();
                        }
                        // **Not `Err(_)`.** The decision is `poll_working`'s,
                        // whole -- a `Disconnected` means the worker is gone and
                        // nothing will ever arrive, and an `Empty` past the
                        // deadline means it is alive and not coming back either.
                        // Both land on `Event::WorkFailed`, which `advance`
                        // sends to `Next::Close`: the window ends and `main`'s
                        // `recover_from_failed_vault_wait` takes over, which is
                        // a fresh login the user can close. That is the point of
                        // the fix -- not a fourth stage that apologises with the
                        // same disabled ✕.
                        Err(err) => {
                            let elapsed = working_since.map_or(Duration::ZERO, |at| at.elapsed());
                            match poll_working(err, elapsed) {
                                WorkPoll::KeepWaiting => {
                                    ui.ctx().request_repaint_after(WORKING_POLL)
                                }
                                WorkPoll::Failed(why) => {
                                    give_up_working(ui.ctx(), &mut closing, why, elapsed);
                                    ui.ctx().request_repaint();
                                }
                            }
                        }
                    }
                }
            }
            Stage::Vault => {
                // The vault window's own frame, in this window. It draws its
                // own chrome, its own resize handles and its own close, and it
                // reports lock / re-auth / Preferences / switch through the
                // handles above exactly as it does when a tray click opens it.
                if let Some(vault_fn) = vault_fn.as_mut() {
                    vault_fn(ui, frame);
                }
            }
        }
    });

    // The vault session ends the way EVERY vault session ends -- `finish`
    // persists the geometry and reads the four outcome cells. `None` when the
    // vault stage was never entered, which is the whole of the difference
    // between "the user closed their vault" and "the user never saw it".
    let vault = vault_handles.borrow().as_ref().map(|handles| handles.finish());
    let stages = stages.borrow().clone();
    if !stages.contains(&Stage::Vault) {
        log::warn!(
            "the single window closed without ever showing the vault; stages were {stages:?}"
        );
    }
    // Taken into locals first: a temporary `RefMut` living inside the struct
    // expression would still be borrowing `token` at the end of the statement,
    // after the `Rc` it borrows from has already been dropped.
    let token = token.borrow_mut().take();
    let prepared = prepared.borrow_mut().take();
    StartupOutcome { token, prepared, vault, stages }
}

/// What the spinner says while a lock's teardown runs.
///
/// The file's voice: `main`'s `SETUP_MESSAGE` is "Setting up your vault...".
/// The message the stage shows CHANGES mid-stage on this host -- the teardown
/// and the post-sign-in repopulate are both `Stage::Working` -- which is why
/// this host holds a `working_message` local rather than taking one
/// `&'static str` parameter the way [`run`] does.
pub const LOCK_MESSAGE: &str = "Locking your vault...";

/// What a close arriving while the vault is up MEANS.
///
/// A value and a pure function rather than a condition inside the frame
/// closure, because the ordinary close-to-tray gesture and the lock arrive by
/// exactly the same route -- `ViewportCommand::Close` -- and the difference
/// between them is one flag. Getting it backwards either strands the user in a
/// window they cannot leave, or reinstates the blink this whole feature exists
/// to remove, and neither is visible from a closure no test can call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultClose {
    /// Nothing asked to close. The commonest answer by far -- it is what every
    /// frame that is merely being drawn gets.
    Ignore,
    /// The vault reported a lost session, so this close IS the lock. The
    /// window stays and the teardown starts behind a spinner.
    Lock,
    /// An ordinary close: the ✕, or a Preferences/switch outcome that closes
    /// and is handled by the caller. It is honoured.
    LetGo,
}

/// [`VaultClose`], whole.
///
/// `lost_session` is the vault frame's own `locked || needs_reauth` --
/// [`vault_window::VaultFrameHandles::lost_session`] -- which is the same
/// disjunction `main`'s `vault_follow_up` reads, so the 401 recovery shares
/// this path by construction rather than by a second condition that could
/// drift from it.
pub fn vault_close(close_requested: bool, lost_session: bool) -> VaultClose {
    match (close_requested, lost_session) {
        (false, _) => VaultClose::Ignore,
        (true, true) => VaultClose::Lock,
        (true, false) => VaultClose::LetGo,
    }
}

/// The four things a lock session can do that change whether the session is
/// torn down.
///
/// A value, so that [`session_torn_down`] can be a pure total function over
/// them. The three that a frame closure would otherwise decide by assigning a
/// `bool` in three different places are exactly the three that got it wrong
/// (see [`VaultSessionOutcome::relocked`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockProgress {
    /// The lock arm ran and the teardown worker STARTED. From here the old
    /// session is being dismantled.
    TeardownStarted,
    /// The lock arm ran and found its `FnOnce` teardown already spent -- the
    /// second lock of one session (see [`run_from_vault`]'s floor note).
    /// **Nothing is being torn down**, so this lock still needs the caller's
    /// own recovery.
    TeardownAlreadySpent,
    /// A vault was rebuilt behind the spinner. The session is LIVE again.
    VaultRebuilt,
    /// The recovery produced no vault. The session stays down, and the
    /// teardown that took it down has already authenticated once.
    RebuildFailed,
    /// **The worker that [`TeardownStarted`](Self::TeardownStarted) was
    /// reported for ENDED WITHOUT EVER REPORTING A STEP.**
    ///
    /// `TeardownStarted` is reported from `std::thread::spawn` returning --
    /// which says a thread exists, not that the teardown ran. A worker that
    /// never receives the two channel ends the frame thread is supposed to
    /// forward, or that panics before it reaches the sign-in request, drops
    /// the only `TeardownStep` sender and the working stage hears
    /// `Disconnected` with no step ever seen. Nothing was drained, `bw serve`
    /// was never stopped and the cache was never cleared, so the session is
    /// NOT torn down and the caller's own recovery is the only teardown this
    /// lock is ever going to get.
    ///
    /// Without this step the window answers `relocked: true` there -- the
    /// v0.5.0 defect exactly: a vault reporting itself locked while a live
    /// `bw serve` still answers out of a full cache.
    TeardownNeverRan,
}

/// Whether the session is torn down and not rebuilt, after one more step.
///
/// **This is the rule [`VaultSessionOutcome::relocked`] carries out of the
/// window, and getting it wrong is a vault that reports itself locked while
/// `bw serve` still answers.** It is a pure function and not three
/// assignments inside the frame closure for the reason [`vault_close`] is:
/// the closure cannot be called by a test, so a rule written inside it is a
/// rule nothing checks.
///
/// `current` is carried rather than ignored because two of the five steps
/// deliberately change nothing: a lock with a spent teardown must not claim a
/// teardown, and a failed rebuild must not retract the one that did run.
///
/// [`LockProgress::TeardownNeverRan`] RETRACTS one, and is the only step that
/// does so other than a rebuild: it is the discovery that the teardown the
/// lock arm optimistically claimed never actually happened.
pub fn session_torn_down(current: bool, step: LockProgress) -> bool {
    match step {
        LockProgress::TeardownStarted => true,
        LockProgress::VaultRebuilt | LockProgress::TeardownNeverRan => false,
        LockProgress::TeardownAlreadySpent | LockProgress::RebuildFailed => current,
    }
}

/// **Whether a failed working stage is the discovery that the teardown never
/// ran** -- i.e. whether this is the moment to report
/// [`LockProgress::TeardownNeverRan`] and retract the claim the lock arm made.
///
/// A pure function for the reason [`vault_close`] and [`session_torn_down`]
/// are: this decision used to be three bare terms inside `run_from_vault`'s
/// frame closure, and no test in this crate can call that closure --
/// `eframe::Frame` has no public constructor. A rule the closure decides
/// inline is a rule nothing checks, and a measured mutation run showed exactly
/// that: inverting `!teardown_reported` there, and swapping
/// [`WorkFailure::WorkerDied`] for [`WorkFailure::Deadline`], were each one
/// token and each left the whole suite green. The first of those two IS the
/// v0.5.0 defect restored -- a vault reporting itself locked with a full cache
/// and a live `bw serve` -- inside the fix written for it.
///
/// All three arguments are load-bearing, and the table above
/// `the_retraction_rule_is_a_table_over_every_combination_of_its_three_inputs`
/// spells out all eight combinations rather than deriving them:
///
/// * **`why` must be [`WorkFailure::WorkerDied`] and not
///   [`WorkFailure::Deadline`].** On the deadline the worker is ALIVE and
///   mid-sequence; telling the caller to start a second teardown against a
///   session another thread is still dismantling is the multiple-owner shape
///   the parked estate exists to remove. It is also not needed there, and for
///   a better reason than "the worker is probably fine": `EstatePark::with`
///   holds the slot mutex across the whole sequence, so the caller's
///   `park.reclaim()` BLOCKS until the teardown finishes and the worker's
///   writes have landed. (The progress ledger's row claiming an abandoned
///   worker "writes into an empty slot" is wrong for this host; `EstatePark`'s
///   own doc has it right.)
/// * **`worker_started` must hold.** Without it, the second lock of one
///   session -- whose `FnOnce` teardown is spent, so nothing was ever spawned
///   -- would "retract" a claim it never made. `session_torn_down` answers
///   `false` for `TeardownAlreadySpent` from `false` anyway, so this is not
///   currently a behaviour change, but reporting a step about a worker that
///   does not exist is a lie the next reader would build on.
/// * **`teardown_reported` must NOT hold.** A worker that reported
///   `NeedsSignIn` or `Finished` and then died really did drain the cache,
///   stop `bw serve` and take the session down; retracting there sends `main`
///   to run a SECOND teardown against a session already dismantled, and
///   inverting this term also stops the genuine retraction happening at all.
pub fn retracts_the_teardown(
    why: WorkFailure,
    worker_started: bool,
    teardown_reported: bool,
) -> bool {
    why == WorkFailure::WorkerDied && worker_started && !teardown_reported
}

/// **What one window's session produced, when the window held TWO sessions.**
///
/// The in-window lock ends one vault session and (usually) starts another in
/// the same window, and each has its own outcome cells:
/// [`vault_window::build_frame`] makes a fresh `edited_settings` cell per
/// frame. `main` reads exactly one [`vault_window::VaultWindowResult`] per
/// window and is the only writer of `settings.json`, so whichever result does
/// not survive this merge is a preference change the user made and watched
/// vanish.
///
/// **The rule: the LATER session decides everything except a gear visit, and a
/// gear visit survives from either.** Every other field is a request about the
/// session that is ending right now -- `locked`, `needs_reauth`, `switch_to`,
/// `add_account`, `remove_account`, `account_details` -- and carrying the
/// pre-lock session's `locked: true` forward would tell `main` to run a
/// recovery against the vault this window has just rebuilt. `edited_settings`
/// is the one field that is deliberately NOT about the session: it is `Some`
/// for the rest of a window's life once the gear has been clicked (see
/// [`vault_window::VaultWindowResult::edited_settings`]), and a lock in the
/// middle of a window's life must not shorten that life.
///
/// **When BOTH carry one, the rebuilt session's wins**, because it is the
/// later of the two visits to the gear and the two are not merged field by
/// field -- `Settings` is edited as a whole in the modal. This is not a lossy
/// choice: the rebuilt frame is constructed with the pre-lock edit already
/// applied (`run_from_vault` hands it to `rebuild_vault`), so its gear opens
/// on the carried-forward value and what comes back out of it is the pre-lock
/// edit plus whatever the user did next.
pub fn carry_settings_forward(
    before_lock: Option<vault_window::VaultWindowResult>,
    after_rebuild: Option<vault_window::VaultWindowResult>,
) -> Option<vault_window::VaultWindowResult> {
    match (before_lock, after_rebuild) {
        // The failed-rebuild path: there is no later session, so the one that
        // locked is the whole answer -- including its `locked: true`, which is
        // what makes `main`'s branches see a lock at all.
        (Some(before), None) => Some(before),
        (before, Some(mut after)) => {
            if after.edited_settings.is_none() {
                after.edited_settings = before.and_then(|before| before.edited_settings);
            }
            Some(after)
        }
        (None, None) => None,
    }
}

/// How far the teardown worker has got.
///
/// Two messages and not one, because the sequence has a hole in the middle
/// that only this thread can fill: the old session is stopped and the cache is
/// cleared BEFORE anything authenticates, and authenticating means a master
/// password, and a master password means a window. The worker therefore stops
/// and says so, the host shows the sign-in card in the window that is already
/// open, and the token goes back down a channel. That round trip is what
/// replaces `main::reauthenticate`'s own eframe window on this path -- which
/// is the second half of "it should never reload different windows".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeardownStep {
    /// The old session is gone and the worker is blocked on the token
    /// channel. Show the card.
    NeedsSignIn,
    /// The whole sequence is over. Whatever state it left is the state the
    /// caller reads back out of its own park.
    Finished,
}

/// What one `run_from_vault` session produced.
pub struct VaultSessionOutcome {
    /// The outcome cells of whichever vault frame was up when the session
    /// ended -- read through `finish`, so the geometry write happens exactly
    /// as it does on every other vault path.
    ///
    /// **A lock's cells are read at the moment of the lock**, not at the end:
    /// the frame that reported `locked` is torn down and replaced, and its
    /// `edited_settings` (a visit to the gear earlier in the same session)
    /// would otherwise be lost. `None` only if the window ended with no vault
    /// frame at all, which is the failed-repopulate path.
    pub result: Option<vault_window::VaultWindowResult>,
    /// Every stage the window actually PAINTED, in order. Same evidence
    /// [`StartupOutcome::stages`] is, and for the same reason: "the table is
    /// right" and "the window ever got there" are different claims.
    pub stages: Vec<Stage>,
    /// **Whether this window leaves the session TORN DOWN AND NOT REBUILT.**
    ///
    /// Not "was a lock seen here" -- that is the question this field used to
    /// answer, and it is the wrong one. The caller's only use for it is to
    /// decide whether to run its own lock recovery, so it must be true
    /// exactly when running that recovery would be a second teardown of a
    /// session that is already gone:
    ///
    /// * a lock whose teardown worker actually STARTED sets it, because from
    ///   that moment the old session is being dismantled;
    /// * a rebuilt vault CLEARS it, because the session is live again and a
    ///   later lock in the same window is a fresh one;
    /// * a lock that started no worker -- the second lock of one session,
    ///   whose `FnOnce` teardown is spent (see [`run_from_vault`]'s floor) --
    ///   never sets it, so the caller still runs the recovery that lock needs.
    ///
    /// Set on merely reaching the lock arm, this field reports a teardown
    /// that did not happen, the caller skips the recovery, and the vault says
    /// "locked" with its cache full and `bw serve` still answering. That is
    /// the v0.5.0 defect in a new place.
    pub relocked: bool,
}

/// **The second host: a vault session that survives its own lock.**
///
/// [`run`] is the STARTUP host -- sign-in, spinner, vault. This one starts at
/// the vault and runs the same machine backwards: vault, spinner, sign-in
/// card, spinner, vault again. One `advance`, two hosts; the alternative --
/// a lock-specific state machine of its own -- is a second machine, and this
/// crate's ledger is a list of what a second copy of a hardened sequence
/// costs.
///
/// It lives in THIS module and not in `main` because the pieces it is built
/// from are this module's: [`close_this_window`],
/// [`refuse_close_while_working`], [`give_up_working`], [`poll_working`],
/// [`WORKING_DEADLINE`] and `Closing` -- whose constructor is private
/// precisely so a second host cannot start the working stage on "already
/// decided" and ship a spinner that spins forever behind a ghosted ✕.
///
/// `teardown` runs on a detached worker thread and is everything the lock
/// means: drain the in-flight backend operation, stop `bw serve`, clear the
/// cache, authenticate, start a fresh backend, repopulate, rebuild the match
/// engine. It gets the token channel's receiving end and a sender to report
/// its two steps on; like [`run`]'s `prepare` it must be `Send + 'static`,
/// which is why the caller lifts the tray out of it rather than passing one.
///
/// **One lock per session, and what the second one costs.** `teardown`,
/// `build_sign_in` and `rebuild_vault` are all `FnOnce`, so a vault that is
/// locked, re-signed-into and then locked AGAIN in the same window finds
/// nothing left to run. What happens then is not a hang and not a skipped
/// teardown: the second lock still leaves the vault stage, the working stage
/// finds the step channel `Disconnected` -- the first worker dropped the only
/// sender when it finished -- and `poll_working` ends the stage on the spot, so
/// the window closes and the caller runs the recovery it has always run. The
/// second lock therefore still locks; it blinks. That is a deliberate floor
/// rather than an oversight: making the closures `FnMut` would mean a teardown
/// worker per lock with no bound on how many are in flight against one parked
/// estate, which is the multiple-owner shape the estate exists to remove.
///
/// **`rebuild_vault` is handed the preference edit the PRE-LOCK session
/// produced**, because the estate the worker reads the rebuilt vault's
/// settings out of is the pre-edit one: `main` is the only writer of
/// `settings.json` and it does not run until this window is gone. Without
/// this, a gear visit that changed the auto-lock policy and was then followed
/// by a lock would come back to a window still running the OLD policy -- the
/// regression that undoes "the open window honours an auto-lock change at
/// once" across a lock. Whatever the rebuilt session then reports wins; see
/// [`carry_settings_forward`].
///
/// `build_sign_in` and `rebuild_vault` both run on THIS thread. They are
/// closures rather than parameters because both are lazy on purpose:
/// `login_ui::build_login_frame` spawns a `bw status` of its own, and a vault
/// session that pays for one on every open -- when the overwhelmingly common
/// outcome is an ordinary close -- would be a regression measured in seconds
/// on every single click.
pub fn run_from_vault<T, S, B>(
    vault: (
        eframe::NativeOptions,
        vault_window::VaultFrameFn,
        vault_window::VaultFrameHandles,
    ),
    after_sign_in_message: &'static str,
    teardown: T,
    build_sign_in: S,
    rebuild_vault: B,
) -> VaultSessionOutcome
where
    T: FnOnce(&mpsc::Sender<TeardownStep>, mpsc::Receiver<String>) + Send + 'static,
    S: FnOnce() -> (login_ui::LoginFrameFn, login_ui::LoginFrameHandles) + 'static,
    B: FnOnce(
            Option<crate::settings::Settings>,
        ) -> Option<(vault_window::VaultFrameFn, vault_window::VaultFrameHandles)>
        + 'static,
{
    let (options, vault_frame, handles) = vault;

    // Read back after the event loop returns, for the same reason every window
    // in this crate uses `Rc<RefCell<_>>`: the update closure is
    // `FnMut + 'static` and cannot hand anything back, and eframe runs it on
    // this thread, which is blocked inside `run_ui_native` throughout.
    let result: Rc<RefCell<Option<vault_window::VaultWindowResult>>> = Rc::new(RefCell::new(None));
    let stages: Rc<RefCell<Vec<Stage>>> = Rc::new(RefCell::new(Vec::new()));
    let relocked: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    // The HANDLES, not the frame, in a cell: the frame is `FnMut` and stays
    // owned by the closure, while `finish` -- the geometry write and the
    // outcome read -- has to be callable out here, after the window is gone.
    // Exactly the split [`run`] makes, and for the same reason.
    let vault_handles: Rc<RefCell<Option<vault_window::VaultFrameHandles>>> =
        Rc::new(RefCell::new(Some(handles)));

    let stages_for_closure = stages.clone();
    // **The lock lives in the shared value, not in this closure.** Everything
    // the lock touches -- the cancelled close, the worker, both teardown
    // steps, the rebuild, every write of `relocked` and the retraction --
    // moved into [`InWindowLock`] so that the startup host reaches the same
    // code rather than a second copy of it. What is left here is this host's
    // own machine: which stage is up, what the spinner says, and when its
    // stopwatch started.
    let mut lock = InWindowLock::new(
        teardown,
        rebuild_vault,
        result.clone(),
        relocked.clone(),
        vault_handles.clone(),
    );

    let mut build_sign_in = Some(build_sign_in);

    let mut vault_fn: Option<vault_window::VaultFrameFn> = Some(vault_frame);
    let mut login: Option<(login_ui::LoginFrameFn, login_ui::LoginFrameHandles)> = None;

    let mut stage = Stage::Vault;
    // The message the spinner shows. A LOCAL and not a parameter, because it
    // changes mid-stage: the teardown and the post-sign-in repopulate are the
    // same `Stage::Working`. See [`LOCK_MESSAGE`].
    let mut working_message: &'static str = LOCK_MESSAGE;
    // Restarted on every ENTRY to the working stage, not once. The stage is
    // entered twice on the lock path, and a stopwatch started at the first
    // entry would charge the repopulate for however long the user spent typing
    // their master password -- a deadline that fires while the backend is
    // healthily coming up, which throws away the sign-in that just happened.
    let mut working_since: Option<Instant> = None;
    let mut closing = Closing::not_yet();

    run_the_one_window(options, move |ui, frame| {
        if stages_for_closure.borrow().last() != Some(&stage) {
            stages_for_closure.borrow_mut().push(stage);
            log::info!("vault window: showing {stage:?}");
        }

        match stage {
            Stage::Vault => {
                if let Some(vault_fn) = vault_fn.as_mut() {
                    vault_fn(ui, frame);
                }
                // The lock catch is [`InWindowLock::catch_the_lock`]'s; what
                // is decided here is only where the window goes next, which is
                // the machine's business and not the lock's.
                if lock.catch_the_lock(ui.ctx(), &mut vault_fn) {
                    if let Next::Show(next) = advance(stage, Event::Locked) {
                        stage = next;
                        if next == Stage::Working {
                            working_message = LOCK_MESSAGE;
                            working_since = Some(Instant::now());
                        }
                    }
                    ui.ctx().request_repaint();
                }
            }
            Stage::SignIn => {
                if login.is_none() {
                    if let Some(build) = build_sign_in.take() {
                        login = Some(build());
                    }
                }
                if let Some((login_fn, login_handles)) = login.as_mut() {
                    login_fn(ui, frame);
                    if let Some(produced) = login_handles.take_token() {
                        lock.hand_over_the_token(produced);
                        if let Next::Show(next) = advance(stage, Event::SignedIn) {
                            stage = next;
                            if next == Stage::Working {
                                working_message = after_sign_in_message;
                                working_since = Some(Instant::now());
                            }
                        }
                        ui.ctx().request_repaint();
                    }
                }
            }
            Stage::Working => {
                match loading_ui::draw_spinner_body(
                    ui,
                    working_message,
                    login_ui::CloseControl::Disabled,
                ) {
                    login_ui::ChromeAction::Minimize => ui
                        .ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                    login_ui::ChromeAction::Close | login_ui::ChromeAction::None => {}
                }

                refuse_close_while_working(ui.ctx(), closing);

                if !closing.decided() {
                    match lock.answer_the_teardown(&mut vault_fn) {
                        Ok(event) => {
                            match advance(stage, event) {
                                Next::Show(next) => stage = next,
                                Next::Close => {
                                    // `relocked` stays SET here, so the caller
                                    // does not re-run the recovery: this
                                    // teardown already ran and already
                                    // authenticated, and the reason there is
                                    // no vault is that the backend would not
                                    // come back -- which `resettle_session_
                                    // with` has already answered by standing
                                    // autofill down. A second pass would ask
                                    // for the master password the user just
                                    // gave, to retry the thing that just
                                    // failed.
                                    log::warn!(
                                        "the lock's recovery produced no vault to show; the \
                                         session is torn down and stays down, so the window \
                                         closes to the tray rather than asking for the master \
                                         password a second time"
                                    );
                                    close_this_window(ui.ctx(), &mut closing);
                                }
                            }
                            ui.ctx().request_repaint();
                        }
                        Err(err) => {
                            let elapsed = working_since.map_or(Duration::ZERO, |at| at.elapsed());
                            match poll_working(err, elapsed) {
                                WorkPoll::KeepWaiting => {
                                    ui.ctx().request_repaint_after(WORKING_POLL)
                                }
                                WorkPoll::Failed(why) => {
                                    lock.retract_if_the_teardown_never_ran(why);
                                    give_up_working(ui.ctx(), &mut closing, why, elapsed);
                                    ui.ctx().request_repaint();
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let stages = stages.borrow().clone();
    let relocked = *relocked.borrow();
    let result = finish_the_locked_session(&result, &vault_handles);
    VaultSessionOutcome { result, stages, relocked }
}

#[cfg(test)]
mod transition_tests {
    use super::*;

    /// The whole table, spelled out rather than derived, so a rewritten
    /// `advance` cannot be checked against itself.
    #[test]
    fn signing_in_leads_to_the_spinner_and_the_spinner_leads_to_the_vault() {
        assert_eq!(
            advance(Stage::SignIn, Event::SignedIn),
            Next::Show(Stage::Working),
            "a produced token does not move the window to the spinner, so the card stays up \
             with nothing happening on it"
        );
        assert_eq!(
            advance(Stage::Working, Event::WorkReady),
            Next::Show(Stage::Vault),
            "the vault is never shown: the user signs in, watches a spinner, and is dropped \
             to the tray without the vault they just unlocked -- which is the half of the \
             report that was never about window count"
        );
    }

    #[test]
    fn work_that_produced_no_vault_ends_the_window() {
        assert_eq!(
            advance(Stage::Working, Event::WorkFailed),
            Next::Close,
            "a failed startup leaves the spinner up forever instead of closing so `main`'s \
             recovery can run"
        );
    }

    /// Every stage is reachable from the first one by SOME sequence of events.
    /// Written as a walk rather than as three assertions about `advance`,
    /// because "the table is right" and "the table connects" are different
    /// claims and this codebase has shipped the first without the second.
    #[test]
    fn the_vault_is_reachable_from_the_sign_in_card() {
        let mut stage = Stage::SignIn;
        let mut seen = vec![stage];
        for event in [Event::SignedIn, Event::WorkReady] {
            match advance(stage, event) {
                Next::Show(next) => {
                    if next != stage {
                        stage = next;
                        seen.push(stage);
                    }
                }
                Next::Close => break,
            }
        }
        assert_eq!(
            seen,
            vec![Stage::SignIn, Stage::Working, Stage::Vault],
            "the walk from the sign-in card does not reach the vault"
        );
    }

    // The hand-written list of refused pairs that used to be here is now the
    // exhaustive walk in `lock_transition_tests::
    // an_event_that_does_not_belong_to_the_current_stage_moves_nothing`. Two
    // new events took it from six entries to ten, and a hand-written ten is a
    // list the next event silently under-counts -- so it is derived from the
    // two variant lists and the one list of moves instead, with a count
    // control that fails if either variant list loses a member.
    //
    // The positive control that used to sit at the bottom of it -- that the
    // comparison can tell a move from a stay -- moved with it.
}

/// The working stage's watchdog, decided away from the frame closure so it can
/// be asserted at all.
///
/// Every test here is about a window the user cannot close: while the stage is
/// up there is no live ✕, no tray icon (`tray::build_tray` runs after
/// `app_window::run` returns) and therefore no Quit item anywhere in the
/// process. Anything that reaches "keep waiting" and stays there is Task
/// Manager.
#[cfg(test)]
mod working_watchdog_tests {
    use super::*;

    /// **The reported defect, as an assertion.** A worker that panics drops its
    /// sender; with the closure holding no second one, `try_recv` answers
    /// `Disconnected` -- and this is what that must mean.
    #[test]
    fn a_worker_that_died_ends_the_stage_rather_than_being_polled_forever() {
        assert_eq!(
            work_channel_poll(mpsc::TryRecvError::Disconnected),
            WorkPoll::Failed(WorkFailure::WorkerDied),
            "a dead worker is treated as a busy one, so the spinner polls a channel nothing \
             can ever send on again -- with no ✕, no tray and no Quit, that is Task Manager"
        );
        assert_eq!(
            work_channel_poll(mpsc::TryRecvError::Empty),
            WorkPoll::KeepWaiting,
            "a worker that simply has not answered yet ends the stage, which throws away a \
             healthy sign-in on any machine slower than this one"
        );
    }

    /// The `Disconnected` half is real only if the channel can actually produce
    /// it -- which is the whole reason the closure hands the worker its sender
    /// instead of a clone. Asserted against a live channel of the same shape,
    /// because "the mapping is right" and "the input ever occurs" are the two
    /// claims this codebase keeps shipping one of.
    #[test]
    fn a_sender_that_only_the_worker_holds_disconnects_when_the_worker_dies() {
        let (tx, rx) = mpsc::channel::<u32>();
        let worker = std::thread::spawn(move || {
            let _tx = tx;
            panic!("the worker died before sending");
        });
        assert!(worker.join().is_err(), "control: the worker was supposed to panic");
        assert_eq!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected),
            "the receiver never learns its worker is gone, so `Disconnected` is a case that \
             cannot occur and the arm handling it is dead code"
        );
        // The negative control: a second sender kept behind -- the shape the
        // closure had -- and the same dead worker is indistinguishable from a
        // slow one. This is the bug, reproduced.
        let (tx, rx) = mpsc::channel::<u32>();
        let kept = tx.clone();
        let worker = std::thread::spawn(move || {
            let _tx = tx;
            panic!("the worker died before sending");
        });
        assert!(worker.join().is_err());
        assert_eq!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "control: with a sender kept behind, a dead worker no longer reads as Empty -- so \
             the test above proves nothing about handing the worker the only sender"
        );
        drop(kept);
    }

    /// A worker that is alive and never answers keeps the channel open, so the
    /// clock is the only thing left.
    #[test]
    fn a_live_worker_that_never_answers_is_bounded_by_the_deadline() {
        assert_eq!(
            work_deadline_poll(Duration::ZERO),
            WorkPoll::KeepWaiting,
            "the stage gives up on its own first frame"
        );
        assert_eq!(
            work_deadline_poll(WORKING_DEADLINE - Duration::from_millis(1)),
            WorkPoll::KeepWaiting,
            "the stage gives up a millisecond early, which on a slow machine throws away a \
             sign-in that was about to land"
        );
        assert_eq!(
            work_deadline_poll(WORKING_DEADLINE),
            WorkPoll::Failed(WorkFailure::Deadline),
            "the stage is not bounded by the clock either, so an untimed `bw sync` on a hung \
             network is a window with no way out"
        );
        assert_eq!(
            work_deadline_poll(WORKING_DEADLINE * 100),
            WorkPoll::Failed(WorkFailure::Deadline),
            "the deadline is a moment rather than a bound: past it, the stage waits again"
        );
    }

    /// The combination, including the ordering: a dead worker is reported as
    /// dead even when the deadline has also passed.
    #[test]
    fn the_two_halves_combine_and_a_dead_worker_is_named_as_one() {
        assert_eq!(
            poll_working(mpsc::TryRecvError::Empty, Duration::ZERO),
            WorkPoll::KeepWaiting
        );
        assert_eq!(
            poll_working(mpsc::TryRecvError::Empty, WORKING_DEADLINE),
            WorkPoll::Failed(WorkFailure::Deadline),
            "`poll_working` ignores the clock, so only a dead worker can end the stage"
        );
        assert_eq!(
            poll_working(mpsc::TryRecvError::Disconnected, Duration::ZERO),
            WorkPoll::Failed(WorkFailure::WorkerDied),
            "`poll_working` ignores the channel, so a panicking worker still polls forever"
        );
        assert_eq!(
            poll_working(mpsc::TryRecvError::Disconnected, WORKING_DEADLINE * 10),
            WorkPoll::Failed(WorkFailure::WorkerDied),
            "a panicked worker is reported as a timeout, which sends the next person \
             investigating this to the network rather than to the panic"
        );
        // Both reasons say something, and something different: this is the only
        // record of which of the two happened on a user's machine.
        assert_ne!(
            WorkFailure::WorkerDied.reason(),
            WorkFailure::Deadline.reason()
        );
        assert!(!WorkFailure::Deadline.reason().is_empty());
    }

    /// **Both endings are endings the user can leave.** `WorkFailed` is only a
    /// fix if what it lands on is not another stage with a ghosted ✕ -- and
    /// there is no fourth stage: it closes the window, which returns from `run`
    /// and hands `main` its existing `recover_from_failed_vault_wait`.
    #[test]
    fn every_way_the_stage_can_give_up_closes_the_window() {
        for (err, elapsed) in [
            (mpsc::TryRecvError::Disconnected, Duration::ZERO),
            (mpsc::TryRecvError::Empty, WORKING_DEADLINE),
        ] {
            let WorkPoll::Failed(_) = poll_working(err, elapsed) else {
                panic!("{err:?} after {elapsed:?} does not end the stage at all");
            };
            assert_eq!(
                advance(Stage::Working, Event::WorkFailed),
                Next::Close,
                "the stage gives up into a stage rather than out of the window -- and every \
                 stage this window has except the vault refuses to close"
            );
        }
    }

    /// The deadline is derived from the numbers the crate already agrees on, so
    /// a change to any of them moves it rather than leaving it stale.
    #[test]
    fn the_deadline_covers_every_phase_the_worker_runs() {
        use crate::bw_serve::{BACKEND_OP_TIMEOUT, READINESS_DEADLINE, readiness_schedule};
        use crate::login_ui::STATUS_DEADLINE;
        use crate::vault_bridge::READ_DEADLINE;
        assert_eq!(
            WORKING_DEADLINE,
            BACKEND_OP_TIMEOUT
                + READINESS_DEADLINE
                + READ_DEADLINE * READINESS_ATTEMPTS as u32
                + STATUS_DEADLINE,
            "the working stage's deadline is no longer the sum of what actually bounds the \
             three phases `StartupWork::produce` runs -- an untimed backend start, the \
             readiness probe's sleeps AND its per-attempt bridge reads, and a `bw status` \
             bounded by `STATUS_DEADLINE` -- so a healthy-but-slow startup can be cut off"
        );
        // **The third term is a real bound and not a borrowed guess.** It was
        // `BACKEND_OP_TIMEOUT` while the phase was an untimed `Command::output()`
        // -- a number describing a call nothing was timing. Reading
        // `STATUS_DEADLINE` here is only honest while `produce` actually applies
        // it, which `main.rs`'s
        // `the_startup_worker_bounds_the_status_call_the_deadline_charges_for`
        // is the other half of.
        assert!(
            STATUS_DEADLINE < BACKEND_OP_TIMEOUT,
            "the status phase is budgeted as heavily as starting the backend again, which is \
             the guess this term replaced rather than the bound it is supposed to be"
        );
        // **The claim `READINESS_ATTEMPTS` makes, checked against the real
        // function rather than agreed with it.** `wait_for_vault_ready` calls
        // first and sleeps second, giving up once `attempt >= schedule.len()`,
        // so the call count is one more than the delay count. Recomputed here
        // because `readiness_schedule` builds a `Vec` and cannot run in a
        // `const`.
        assert_eq!(
            readiness_schedule(READINESS_DEADLINE).len() as u64 + 1,
            READINESS_ATTEMPTS,
            "the readiness schedule no longer makes {READINESS_ATTEMPTS} attempts, so the \
             deadline is crediting that phase the wrong amount of network time"
        );
        // The phase the old sum got wrong, stated on its own: the readiness
        // probe costs far more than the deadline it is named after, because
        // `READINESS_DEADLINE` bounds only the sleeps between its attempts.
        assert!(
            READINESS_DEADLINE + READ_DEADLINE * READINESS_ATTEMPTS as u32
                > READINESS_DEADLINE * 4,
            "control: the readiness phase's real bound has collapsed back to roughly its \
             sleep budget, which is the mistake this sum was rewritten to fix"
        );

        // **The absolute bounds -- and the only ones here that are.** Every
        // assertion above is a restatement of `WORKING_DEADLINE`'s own
        // definition: each right-hand side is a sub-expression of it, so
        // `WORKING_DEADLINE` minus the nominal three-phase budget is identically
        // `READINESS_ATTEMPTS * READ_DEADLINE`, which is never negative -- it
        // cannot fail however far the source constants move. Not a
        // hypothetical: halving `BACKEND_OP_TIMEOUT` drops this deadline from
        // 260s to 215s with all of the above green, which is precisely what the
        // comment that used to stand here claimed could not happen. The two
        // below are compared against LITERALS declared in this module, so
        // nothing in `bw_serve` or `vault_bridge` can move both sides at once.
        assert!(
            WORKING_DEADLINE <= SPINNER_PATIENCE,
            "the working stage may now hold a window the user cannot close for longer than \
             anyone will sit in front of it ({WORKING_DEADLINE:?} > {SPINNER_PATIENCE:?}); \
             past this the watchdog is not a way out, it is Task Manager with extra steps"
        );
        assert!(
            WORKING_DEADLINE >= MINIMUM_STARTUP_GRACE,
            "the working stage now abandons a startup after {WORKING_DEADLINE:?}, less than the \
             {MINIMUM_STARTUP_GRACE:?} a cold start can legitimately need -- so a sign-in that \
             is merely slow is thrown away and the user is sent back through a fresh login for \
             nothing, which is the one harm this deadline's generosity exists to avoid"
        );
        // **The floor's own argument, checked.** `MINIMUM_STARTUP_GRACE` is a
        // literal so that rearranging the source constants cannot move it -- but
        // a literal nothing checks is just a number someone once liked. Its
        // stated reason is that it clears the part of the startup still bounded
        // only by this WINDOW and not by the worker itself: phases 1 and 2. The
        // status phase is excluded on purpose, because `produce` now bounds it.
        // Unlike the two assertions above this one CAN fail on a source-constant
        // change, and that is the point: if the untimed phases grow past the
        // floor, the floor's argument is stale and has to be re-made rather than
        // silently outgrown.
        let still_unbounded_by_the_worker =
            BACKEND_OP_TIMEOUT + READINESS_DEADLINE + READ_DEADLINE * READINESS_ATTEMPTS as u32;
        assert!(
            MINIMUM_STARTUP_GRACE > still_unbounded_by_the_worker,
            "the floor ({MINIMUM_STARTUP_GRACE:?}) no longer clears the \
             {still_unbounded_by_the_worker:?} of startup that only this window bounds, so it \
             has stopped protecting a slow-but-healthy cold start from being thrown away"
        );
        assert!(
            MINIMUM_STARTUP_GRACE < WORKING_DEADLINE,
            "the floor has caught up with the deadline it is a floor ON; a floor equal to the \
             sum is not a margin, it is the same claim written twice"
        );
    }

    /// The longest anyone will sit in front of a spinner with no ✕, no tray and
    /// no Quit before Task Manager is the reasonable response -- and therefore
    /// the point past which a longer watchdog buys nothing, because the user has
    /// already killed the process.
    ///
    /// Six minutes: generous against the ~4m20s the phases above can legitimately
    /// cost, and deliberately not derived from them, so that it is a bound on the
    /// definition rather than a restatement of it.
    ///
    /// Unchanged when phase 3 stopped being an untimed spawn and the deadline
    /// fell to 260s. This is a claim about a PERSON -- how long anyone will sit
    /// in front of a spinner with no way out -- and nothing about how the app
    /// spends the time changes it. It simply has more headroom now.
    const SPINNER_PATIENCE: Duration = Duration::from_secs(6 * 60);

    /// The shortest deadline a startup may ever be given before the window
    /// abandons it.
    ///
    /// **Four minutes -- re-argued, not relaxed.** It was five, and the argument
    /// was that the three phases can legitimately cost ~5m20s on a cold `bw
    /// serve`. That reasoning was sound while phase 3 was an untimed `bw status`
    /// credited a 90s backend-start budget. It is not sound now:
    /// `StartupWork::produce` calls `check_bw_status_details_bounded`, which
    /// stops waiting after `login_ui::STATUS_DEADLINE` and reports "unknown", so
    /// that phase CANNOT cost more than 30s however slow the machine is. A
    /// five-minute floor now asserts time no phase is able to spend, and the
    /// only thing it could do is force a future deadline to be padded past the
    /// work it covers.
    ///
    /// Four minutes is where the same argument lands at the real phase costs.
    /// What the floor protects is the part still bounded only by this WINDOW and
    /// not by the worker: phase 1's untimed backend start (`BACKEND_OP_TIMEOUT`,
    /// 90s) plus phase 2's sleeps and per-attempt bridge reads (30s + 11x10s),
    /// which is 230s of work a slow-but-healthy cold start can genuinely
    /// consume, and cutting it off costs the user the entire sign-in they have
    /// just completed while buying nothing -- the recovery `main` runs
    /// afterwards starts the same backend over again from scratch. 240s clears
    /// that with the smallest margin that is still a margin, and leaves the
    /// guard its teeth: halving `BACKEND_OP_TIMEOUT` yields 215s and trips this,
    /// which is the exact scenario this floor was written for.
    ///
    /// Still a LITERAL and deliberately not a sum of the constants
    /// `WORKING_DEADLINE` is built from -- a floor spelled with those moves
    /// whenever they do, which is how a 320s deadline could have become 230s
    /// with this whole file green. Re-deriving the literal when a phase's real
    /// bound changes is the intended way to change it; rearranging the source
    /// constants is not.
    const MINIMUM_STARTUP_GRACE: Duration = Duration::from_secs(4 * 60);
}

/// **The window's own close, run for real.**
///
/// The frame closure cannot be called by a test -- `eframe::Frame` has no public
/// constructor -- but the three things it does to the viewport are ordinary
/// functions over an `egui::Context`, and a headless `Context` records every
/// command sent to it. So these are behavioural: they send the command and read
/// it back, rather than searching the source for the line that sends it.
///
/// This is the file's central property. The reviewer of `6fc3792` could not
/// construct a state the user cannot leave, but observed that deleting the two
/// `ViewportCommand::Close` lines -- keeping `closing = true` -- left the whole
/// suite green while restoring most of the hang.
#[cfg(test)]
mod window_close_tests {
    use super::*;

    /// Every viewport command one frame sent, in order.
    fn commands_of(output: &egui::FullOutput) -> Vec<egui::ViewportCommand> {
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.commands.clone())
            .unwrap_or_default()
    }

    /// A frame's input, optionally carrying the close the OS reports when the
    /// user hits Alt+F4 -- or that eframe echoes back after this module's own
    /// `ViewportCommand::Close`.
    fn input(close_requested: bool) -> egui::RawInput {
        let mut raw = egui::RawInput::default();
        if close_requested {
            raw.viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
        }
        raw
    }

    /// **The deleted line, as an assertion.** `closing = true` on its own leaves
    /// the stage refusing nothing and ending nothing: the ✕ is still `Disabled`,
    /// so the window stops asking to close and only Alt+F4 gets through.
    #[test]
    fn the_stage_ending_itself_actually_asks_the_window_to_close() {
        let ctx = egui::Context::default();
        let mut closing = Closing::not_yet();
        let output = ctx.run_ui(input(false), |ui| {
            close_this_window(ui.ctx(), &mut closing);
        });
        assert!(
            commands_of(&output).contains(&egui::ViewportCommand::Close),
            "the stage decided to stop and never asked the window to close, so the spinner \
             stays up with a ghosted ✕, no tray and no Quit anywhere in the process: {:?}",
            commands_of(&output)
        );
        assert!(
            closing.decided(),
            "the close was sent without disarming the refusal, so the next frame cancels the \
             window's own exit"
        );
        // Positive control on the harness: a frame that sends nothing records
        // nothing, so `contains` above is a statement about `close_this_window`
        // and not about every frame.
        let quiet = ctx.run_ui(input(false), |_ui| {});
        assert!(
            !commands_of(&quiet).contains(&egui::ViewportCommand::Close),
            "control: the harness reports a Close for a frame that sent none"
        );
    }

    /// The refusal, both ways round.
    #[test]
    fn the_stage_refuses_a_close_it_did_not_ask_for_and_only_that_one() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(input(true), |ui| {
            assert!(
                refuse_close_while_working(ui.ctx(), Closing::not_yet()),
                "the working stage let an Alt+F4 through, stranding the `bw serve` it is \
                 holding on the port the recovery needs"
            );
        });
        assert!(
            commands_of(&output).contains(&egui::ViewportCommand::CancelClose),
            "the refusal decided to refuse and sent nothing, so the window closes anyway: {:?}",
            commands_of(&output)
        );
        // Not asked at all: no close request, so nothing to refuse.
        let output = ctx.run_ui(input(false), |ui| {
            assert!(!refuse_close_while_working(ui.ctx(), Closing::not_yet()));
        });
        assert!(
            !commands_of(&output).contains(&egui::ViewportCommand::CancelClose),
            "the stage cancels a close nobody asked for, which is a `CancelClose` on every \
             frame of the spinner"
        );
    }

    /// **The value the stage starts on, RUN rather than read.**
    ///
    /// `Closing::not_yet` is what the frame closure initialises its own local
    /// to, and inverting it ships two failures at once with every other test in
    /// this file green: a working stage that refuses no close at all, and a
    /// worker whose answer is never drained. The closure cannot be called by a
    /// test -- `eframe::Frame` has no public constructor -- but the starting
    /// value itself is an ordinary function, so both halves are asserted here
    /// against a real `egui::Context`, and only which local the closure builds
    /// from it is left to source position.
    #[test]
    fn the_stage_starts_out_refusing_every_close_and_still_draining_the_worker() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(input(true), |ui| {
            assert!(
                refuse_close_while_working(ui.ctx(), Closing::not_yet()),
                "the working stage starts out refusing NOTHING, so an Alt+F4 or a system-menu \
                 close while `bw serve` is starting is honoured and leaves it listening on the \
                 port the recovery needs"
            );
        });
        assert!(
            commands_of(&output).contains(&egui::ViewportCommand::CancelClose),
            "the stage refused the close it did not ask for and sent nothing, so the window \
             closes anyway: {:?}",
            commands_of(&output)
        );
        // The other half of the same token, and the worse one: the closure
        // drains the worker only while this is false. Started decided, the
        // vault never appears, the watchdog never runs, and the spinner spins
        // behind a ghosted close control with no tray and no Quit in existence.
        assert!(
            !Closing::not_yet().decided(),
            "the working stage begins already believing it has decided to end, so the frame \
             closure never drains the worker's answer: the vault is never shown and the spinner \
             runs forever with no way out of it"
        );
    }

    /// **The property the whole fix exists for, end to end.** Frame one: the
    /// stage gives up and asks to close. Frame two: eframe reports that very
    /// close back while `Stage::Working` is still the stage being drawn -- and
    /// the refusal must stand down for it, or the window cancels its own exit
    /// and is exactly as unleaveable as the bug.
    #[test]
    fn the_close_the_stage_sends_itself_is_not_then_refused_by_the_stage() {
        let ctx = egui::Context::default();
        let mut closing = Closing::not_yet();

        let first = ctx.run_ui(input(false), |ui| {
            give_up_working(ui.ctx(), &mut closing, WorkFailure::Deadline, WORKING_DEADLINE);
        });
        assert!(
            commands_of(&first).contains(&egui::ViewportCommand::Close),
            "giving up does not ask the window to close at all"
        );

        let second = ctx.run_ui(input(true), |ui| {
            refuse_close_while_working(ui.ctx(), closing);
        });
        assert!(
            !commands_of(&second).contains(&egui::ViewportCommand::CancelClose),
            "the stage cancelled the close IT sent, so `WorkFailed` never actually ends the \
             window and the user is back to Task Manager: {:?}",
            commands_of(&second)
        );

        // The control, and the bug: with the flag never set, the same second
        // frame refuses the window's own exit. This is what an unconditional
        // refusal ships.
        let bug = ctx.run_ui(input(true), |ui| {
            refuse_close_while_working(ui.ctx(), Closing::not_yet());
        });
        assert!(
            commands_of(&bug).contains(&egui::ViewportCommand::CancelClose),
            "control: the second frame cannot refuse anything, so the assertion above holds \
             for a reason other than the flag"
        );
    }

    /// **Which transition giving up takes, and that it is the one that leaves.**
    ///
    /// `advance(Stage::Working, Event::WorkReady)` returns `Next::Show(Vault)`,
    /// and the vault stage's `vault_fn` is `None` on this path -- the `Vault` arm
    /// then draws nothing at all. A permanently blank window is worse than the
    /// spinner it replaces, and it was one token away with the suite green.
    #[test]
    fn giving_up_leaves_the_window_rather_than_landing_on_a_blank_vault() {
        for why in [WorkFailure::WorkerDied, WorkFailure::Deadline] {
            let ctx = egui::Context::default();
            let mut closing = Closing::not_yet();
            let output = ctx.run_ui(input(false), |ui| {
                assert_eq!(
                    give_up_working(ui.ctx(), &mut closing, why, Duration::ZERO),
                    Next::Close,
                    "{why:?} moves the window to another stage instead of out of it -- and the \
                     only stage it can reach that way is a vault with no frame built for it, \
                     which paints nothing"
                );
            });
            assert!(
                commands_of(&output).contains(&egui::ViewportCommand::Close),
                "{why:?} reached `Next::Close` and still did not ask the window to close"
            );
            assert!(
                closing.decided(),
                "{why:?} left the refusal armed against its own close"
            );
        }
    }

    /// **The reason survives, and the two reasons stay different.**
    ///
    /// Both variants reach the same `Event::WorkFailed` and the same
    /// `Next::Close`, so this line is their ONLY user-visible difference and the
    /// only record of which happened on a user's machine. Asserted through
    /// `give_up_message`, which is what production logs, so replacing
    /// `why.reason()` with a fixed string fails here -- rather than through
    /// `reason()` alone, which nothing on the live path would then call.
    #[test]
    fn the_two_ways_of_giving_up_are_told_apart_in_the_log() {
        let elapsed = Duration::from_secs(7);
        let died = give_up_message(WorkFailure::WorkerDied, elapsed);
        let timed_out = give_up_message(WorkFailure::Deadline, elapsed);
        assert_ne!(
            died, timed_out,
            "a panicked worker and a hung backend log the same line, so the next person \
             investigating this cannot tell which one they are looking at"
        );
        for (message, why) in [(&died, WorkFailure::WorkerDied), (&timed_out, WorkFailure::Deadline)]
        {
            assert!(
                message.contains(why.reason()),
                "{why:?} is logged as something other than its own reason: {message}"
            );
            assert!(
                message.contains("7s"),
                "the log line drops how long the stage had been up, which is the one number \
                 that says whether the deadline or the worker ended it: {message}"
            );
        }
    }
}

/// The half of this window no harness can reach.
///
/// `eframe::Frame` has no public constructor, so the frame closure cannot be
/// called by a test at all -- deleting the `Stage::Vault` arm, or the spawn
/// that starts the background work, leaves every test above green and ships a
/// window that never shows a vault or freezes on the frame it signs in. Held by
/// source position instead, the same way every other window in this crate is.
#[cfg(test)]
mod startup_window_tests {
    fn source() -> &'static str {
        include_str!("app_window.rs")
    }

    /// Everything before the first `#[cfg(test)]`. Split with `concat!` so the
    /// marker exists in the binary but appears in this file only where the real
    /// attributes are -- otherwise this needle would find ITSELF, above all the
    /// production code, and every slice below would be empty.
    ///
    /// **What is below the cut is invisible to every guard that uses this**,
    /// and the length check here does not change that: it holds for any file
    /// with a test module at all. The region below the cut is held instead by
    /// `nothing_but_gated_test_modules_lives_below_the_guards_cut`, which
    /// walks it in full and requires it to be test modules and nothing else.
    pub(super) fn production() -> &'static str {
        let source = source();
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file");
        let production = &source[..end];
        assert!(
            !production.is_empty() && production.len() < source.len(),
            "control: the slice is empty or is the whole file, so it is not the production \
             half of anything"
        );
        production
    }

    /// Everything on a line before a `//`.
    ///
    /// Load-bearing for the guards below, not tidiness, and in two ways.
    ///
    /// The first: the comment above the spinner call names
    /// `CloseControl::Disabled` out loud, so a guard that matched the raw source
    /// would go on passing after the argument itself was changed. This crate has
    /// already shipped exactly that mistake once (the icon guard that matched
    /// the comment naming the thing it was looking for).
    ///
    /// The second, and the reason this runs BEFORE the slices are cut rather
    /// than after: the bounds are themselves needles. A comment inside the
    /// working arm containing `Stage::Vault =>`, or one above the closure
    /// carrying its head, would end a slice early -- and every guard whose
    /// needle fell outside the truncated region would then be a statement about
    /// code that is no longer in it. That is silent where it matters most: a
    /// truncation that still satisfies the positive controls vacates the
    /// negative guards entirely. Stripping first means a comment cannot be a
    /// bound at all, and the length checks below catch anything else that
    /// shortens one.
    pub(super) fn code(source: &str) -> String {
        source
            .lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The frame closure: from its head to the end of production code, comments
    /// already stripped.
    fn closure() -> String {
        let production = code(production());
        // **Anchored on the host, not on the eframe call.** Both hosts hand
        // their closure to `run_the_one_window` now -- there is exactly one
        // eframe-launch site in this file and it is in there -- so
        // the old anchor would land in the shared opener and every guard below
        // would be a statement about eight lines of styling. See
        // `both_hosts_go_through_the_one_window_opener`, which is what now
        // holds the fact the old anchor incidentally held.
        let at = production
            .find(concat!("pub fn ", "run<P, W, V>("))
            .expect(
                "no startup host in this file -- if `run` is gone, the single-window startup \
                 is gone entirely",
            );
        let rest = &production[at..];
        // **Bounded forward at the second host**, not run to the end of
        // production. `run_from_vault` has a `Stage::Working` arm of its own;
        // left unbounded, `working_arm` below would still cut at THIS host's
        // arms (they come first) but every `contains` here would be satisfied
        // by either host -- so deleting the startup host's spinner call would
        // leave this file green on the strength of the lock host's.
        let closure = match rest.find(concat!("pub fn run_from_", "vault<")) {
            Some(end) => rest[..end].to_string(),
            None => rest.to_string(),
        };
        assert!(
            closure.len() > 4_000,
            "the frame closure sliced down to {} bytes, which is not the whole of it -- the \
             guards below would then be statements about a region that stops short of the code \
             they name",
            closure.len()
        );
        closure
    }

    #[test]
    fn every_stage_the_table_names_is_drawn_by_the_closure() {
        let closure = closure();
        for (arm, what) in [
            (
                concat!("Stage::SignIn ", "=>"),
                ("the sign-in card is never drawn", "login_fn(ui, frame);"),
            ),
            (
                concat!("Stage::Working ", "=>"),
                (
                    "the spinner is never drawn",
                    "loading_ui::draw_spinner_body(",
                ),
            ),
            (
                concat!("Stage::Vault ", "=>"),
                ("THE VAULT IS NEVER DRAWN", "vault_fn(ui, frame);"),
            ),
        ] {
            let (why, draws) = what;
            assert!(
                closure.contains(arm),
                "the closure has no {arm:?} arm, so {why} -- the stage is reachable in the \
                 table and paints nothing in the window, which is a blank screen"
            );
            assert!(
                closure.contains(draws),
                "the {arm:?} arm does not call {draws:?}: {why}"
            );
        }
        // Positive control on the slice: it really is the closure and not the
        // whole file, so "contains" above is a statement about the closure.
        assert!(
            !closure.contains(concat!("pub fn ", "advance(")),
            "control: the sliced region reaches back above the closure"
        );
    }

    /// The `Stage::Working` arm alone, cut out of the already-stripped closure.
    /// Bounded forward to the next arm rather than by a byte count, so it cannot
    /// overrun into the vault's own chrome call -- which passes
    /// `CloseControl::Active` and would satisfy a careless search.
    fn working_arm() -> String {
        let closure = closure();
        let start = closure
            .find(concat!("Stage::Working ", "=>"))
            .expect("the closure has no working stage at all");
        let rest = &closure[start..];
        let end = rest
            .find(concat!("Stage::Vault ", "=>"))
            .expect("the working arm is not followed by the vault arm");
        let arm = rest[..end].to_string();
        assert!(
            arm.len() > 3_000,
            "the working arm sliced down to {} bytes, which is not the whole of it -- the \
             negative guards below (no stand-down by hand, no `Err(_)`, no vault draw) would \
             then be statements about a region that stops before the code they are about",
            arm.len()
        );
        arm
    }

    /// **The stage that refuses to close shows a ✕ that refuses to be clicked.**
    ///
    /// Before the spinner wore a heading there was no ✕ here at all, so the
    /// refusal below was invisible by accident. Now it is a decision, and this
    /// is where it is made -- passing `Active` here would ship a control that
    /// looks live, does nothing, and logs a line the user never sees.
    #[test]
    fn the_stage_that_refuses_to_close_draws_a_disabled_close_control() {
        let arm = working_arm();
        assert!(
            arm.contains(concat!("CloseControl::", "Disabled")),
            "the spinner stage's ✕ is live while the stage refuses every close, so clicking it \
             does nothing at all: {arm}"
        );
        assert!(
            arm.contains(concat!("refuse_close_while_", "working(ui.ctx(), closing)")),
            "the working stage no longer refuses a close it did not draw the affordance for \
             (Alt+F4, the system menu), so a `bw serve` still starting up is stranded on the \
             port the recovery needs. What it refuses and when is behavioural -- see \
             `window_close_tests` -- but nothing there can tell whether the arm CALLS it: {arm}"
        );
        // Positive control on the slice: it really is the working arm and stops
        // before the vault's, whose chrome passes `CloseControl::Active`.
        assert!(
            arm.contains(concat!("draw_spinner_", "body(")),
            "control: the sliced region is not the arm that draws the spinner: {arm}"
        );
        assert!(
            !arm.contains(concat!("vault_fn(ui, ", "frame);")),
            "control: the sliced region runs on into the vault arm: {arm}"
        );
    }

    #[test]
    fn the_slow_work_runs_on_a_thread_rather_than_on_the_frame() {
        let closure = closure();
        assert!(
            closure.contains(concat!("std::thread::", "spawn(move || {")),
            "the startup work no longer runs on a worker thread. Run inline, it blocks the \
             frame closure -- so the window FREEZES on the frame it was supposed to start \
             showing the spinner on, for however long `bw serve` takes to come up."
        );
        assert!(
            closure.contains(concat!("work_rx.", "try_recv()")),
            "the closure never drains the worker's answer, so the spinner runs forever"
        );
        // `try_recv`, never `recv`: a blocking receive here is the same freeze
        // as running the work inline, just harder to see.
        assert!(
            !closure.contains(concat!("work_rx.", "recv()")),
            "the closure BLOCKS on the worker, which freezes the window exactly as running \
             the work inline would"
        );
    }

    /// **The watchdog is only a fix if the closure asks it anything.**
    ///
    /// `poll_working` and its tests are pure and would stay green with the call
    /// deleted from the frame -- which is exactly the failure this crate keeps
    /// shipping (three functions complete, correct and unreachable at once). The
    /// arm's poll must therefore be pinned by source position, the same way the
    /// spinner and the vault's own draw calls are.
    #[test]
    fn the_working_arm_asks_the_watchdog_and_does_not_swallow_the_error() {
        let arm = working_arm();
        assert!(
            arm.contains(concat!("poll_", "working(")),
            "the working arm never asks the watchdog, so a dead worker and a hung `bw sync` \
             both leave a spinner up that has no live ✕, no tray and no Quit anywhere in the \
             process yet: {arm}"
        );
        assert!(
            arm.contains(concat!("WorkPoll::", "Failed")),
            "the working arm ignores what the watchdog answered, so it can only ever keep \
             waiting: {arm}"
        );
        assert!(
            !arm.contains(concat!("Err(_) ", "=>")),
            "the poll arm is back to catching every `TryRecvError` the same way, which treats \
             a worker that panicked as one that is merely slow: {arm}"
        );
        // **The ARGUMENT, not just the call.** A stopwatch that is never
        // started reads zero forever, so `poll_working` would still be called,
        // every deadline test would still pass, and the deadline would never
        // fire on a user's machine. This crate has shipped exactly that shape
        // once already (the observer that read a window title and then passed
        // `""` on), so both needles are here.
        assert!(
            arm.contains(concat!("working_since.map_or(Duration::ZERO, ", "|at| at.elapsed())")),
            "the watchdog is asked about something other than how long this stage has been \
             up, so the deadline is measuring nothing: {arm}"
        );
        assert!(
            closure().contains(concat!("working_since = Some(", "Instant::now());")),
            "nothing ever starts the working stage's stopwatch, so it reads zero on every \
             frame and the deadline can never fire"
        );
        // **A stage that has already stopped is not polled again.** After an
        // `Ok(work)` whose `build_vault` answered `None`, the worker has ended
        // and dropped the only sender, and the repaint that follows guarantees
        // another Working frame -- whose `try_recv` says `Disconnected`, which
        // this arm would report at error level as a panic that never happened,
        // on the one path that already logged the real reason. The guard is
        // `!closing`, and `closing` is set only by the two exits that have
        // already asked the window to close, so no live path loses its deadline.
        assert!(
            arm.contains(concat!("if !closing.", "decided() {")),
            "the working arm polls the channel after the stage has decided to stop, so a \
             `build_vault` that answered `None` is logged a frame later as a worker that \
             panicked: {arm}"
        );
    }

    /// The other half of the same fix, and the half that is invisible in the
    /// arm: a `Disconnected` can only ever be observed if the closure lets go of
    /// its sender. A `clone` here is the whole defect back, with every test
    /// above still green.
    #[test]
    fn the_worker_gets_the_only_sender() {
        let closure = closure();
        assert!(
            closure.contains(concat!("work_tx.", "take()")),
            "the closure no longer hands the worker its sender outright, so it is keeping one \
             of its own -- and a worker that panics leaves `try_recv` answering Empty forever \
             instead of Disconnected, which is the spinner that never ends"
        );
        assert!(
            !closure.contains(concat!("work_tx.", "clone()")),
            "the closure keeps a sender of its own alive for the window's whole life, so the \
             channel can never disconnect and the dead-worker arm is unreachable"
        );
    }

    /// **Both ways the stage ends go through the one function that ends it.**
    ///
    /// What that function does -- set the flag AND send the command, in that
    /// order -- is behavioural, in `window_close_tests`. What no test there can
    /// see is whether the arm still calls it on both of its exits, because the
    /// arm is inside the frame closure. So the call sites are counted here, and
    /// only the call sites.
    ///
    /// Counted rather than merely `contains`ed: the stage has exactly two exits
    /// -- work that produced no vault, and the watchdog giving up -- and a fix
    /// that reaches one of them is the defect this file keeps shipping.
    #[test]
    fn both_of_the_stages_exits_go_through_the_window_close() {
        let arm = working_arm();
        // One each, not two between them: a sum would still read 2 if one exit
        // were duplicated and the other deleted, which is the exact shape of
        // "the fix reached one of the two paths".
        for (call, exit) in [
            (
                concat!("close_this_", "window(ui.ctx(), &mut closing)"),
                "work arrived but built no vault",
            ),
            (
                concat!("give_up_", "working(ui.ctx(), &mut closing,"),
                "the watchdog gave up on a dead or hung worker",
            ),
        ] {
            assert_eq!(
                arm.matches(call).count(),
                1,
                "the working stage's exit for {exit:?} no longer ends the window -- it stops \
                 the stage without asking to close, leaving a ghosted ✕, no tray and no Quit \
                 anywhere in the process: {arm}"
            );
        }
        // The flag is never set in the arm any more; it is set inside
        // `close_this_window`, where the command it must accompany cannot be
        // deleted without a behavioural test failing. If it comes back here, the
        // two have been split again.
        assert!(
            !arm.contains(concat!("closing.", "decide()")),
            "the arm stands the refusal down by hand again, which is how it became possible to \
             delete the close it is supposed to accompany: {arm}"
        );
        // Positive control on the slice, in both directions.
        assert!(
            arm.contains(concat!("poll_", "working(err, elapsed)")),
            "control: the sliced region is not the arm that polls the watchdog: {arm}"
        );
    }

    /// **The watchdog's verdict must reach the window as a close.**
    ///
    /// `give_up_working` is behaviourally tested -- it sends the command, and
    /// swapping its `Event::WorkFailed` for `WorkReady` fails
    /// `giving_up_leaves_the_window_rather_than_landing_on_a_blank_vault`. This
    /// is the one link that test cannot cover: that the `WorkPoll::Failed` arm
    /// hands its verdict to it rather than dropping it.
    #[test]
    fn the_watchdogs_verdict_is_handed_to_the_window() {
        let arm = working_arm();
        assert!(
            arm.contains(concat!("give_up_", "working(ui.ctx(), &mut closing, why, elapsed)")),
            "the watchdog answered `Failed` and the arm does nothing with it -- the stage \
             computes that it should stop and then keeps waiting: {arm}"
        );
    }

    /// **Where the refusal starts, and the one place it may be stood down.**
    ///
    /// What the starting value MEANS is behavioural -- see
    /// `the_stage_starts_out_refusing_every_close_and_still_draining_the_worker`
    /// -- but which value the frame closure builds its own local from, and
    /// whether some other arm stands the refusal down before the working stage
    /// has even begun, is inside the closure and therefore invisible to every
    /// behavioural test in this file. A `closing.decide()` added to the
    /// `Stage::SignIn` arm, before the transition to `Working`, leaves the
    /// working stage refusing nothing for its whole life -- an Alt+F4 during
    /// `bw serve`'s startup then strands it on the port the recovery needs --
    /// and leaves the worker's answer undrained, with every other test green.
    ///
    /// Counted, not merely `contains`ed: there is exactly one place the stage
    /// ends itself, and it is `close_this_window`, where the flag and the
    /// command it must accompany cannot be separated.
    #[test]
    fn the_refusal_starts_armed_and_is_stood_down_in_exactly_one_place() {
        let production = code(production());
        assert!(
            production.contains(concat!("let mut closing = Closing::", "not_yet();")),
            "the frame closure no longer starts the working stage's refusal armed, so the stage \
             refuses nothing AND never drains the worker: a close during `bw serve`'s startup \
             strands it on the port the recovery needs, and the vault is never shown at all"
        );
        assert_eq!(
            production.matches(concat!("closing.", "decide()")).count(),
            1,
            "the refusal is stood down somewhere other than `close_this_window` -- so a stage \
             that has not asked to close is no longer refusing anything, and the frame closure \
             has stopped draining the worker's answer"
        );
    }

    #[test]
    fn the_window_is_raised_once_and_the_sub_frames_do_not_raise_it_again() {
        let production = production();
        assert_eq!(
            production.matches(concat!("raise_window(", "WINDOW_TITLE)")).count(),
            1,
            "this window must ask to be brought to the front exactly once, on its first frame"
        );
        // Both sub-frames are built `pre_styled`, which is what stops them
        // raising the window again from inside it. Spelled as the argument
        // lists, because a bare `true` names nothing.
        assert!(
            production.contains(concat!("build_login_frame(account, first_run, ", "true, false)")),
            "the login frame is no longer built `pre_styled`/non-closing: either it re-raises \
             this window from inside it, or a produced token closes the window before the \
             spinner and vault it is supposed to become"
        );
    }
    // -----------------------------------------------------------------
    // The region BELOW the cut -- the half no source guard in this file
    // reads.
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and so it cannot be found by a guard
    /// looking for the real attributes.
    const BELOW_CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// The literal the source guards in this file cut the file at. Split for
    /// the same reason: an unsplit copy would BE the first occurrence, and
    /// every production slice in this file would come back empty.
    const BELOW_CUT_MARKER: &str = BELOW_CUT_GATE;

    /// Column-0 lines that live below the cut but are the CONTENTS OF A
    /// STRING LITERAL rather than source. Each is controlled below: it must
    /// still occur in this file exactly once, so a stale entry here cannot
    /// quietly widen the hole this test exists to close.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Deliberately exact rather than a `starts_with`:
    /// `mod x { fn escape() {} }` on one line is not a module opener as far
    /// as this walk is concerned, and must fail it.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// **Below the cut there is nothing but test-only modules, and the cut is
    /// where every guard in this file believes it is.**
    ///
    /// This file guards a great deal of behaviour by slicing its own source at
    /// the first `cfg(test)` attribute and counting needles in the half ABOVE that cut.
    /// Two things can silently empty those guards, and neither of them changes
    /// a single guard's own text:
    ///
    /// 1. **Anything appended below the test modules is invisible to all of
    ///    them.** It is not counted, not forbidden and not read. Confirmed by
    ///    mutation, in this crate, tonight: a second production item appended
    ///    under the test module of `main.rs` -- a duplicate startup handoff,
    ///    the exact defect a guard pins at "exactly one" -- left the whole
    ///    binary suite green with zero warnings. The bottom of a file is a
    ///    natural place to add a helper, and until this test existed nothing
    ///    stopped it.
    /// 2. **The cut can move UPWARDS.** The slice is a `find` of a literal, so
    ///    that literal appearing in a comment or a string above the real test
    ///    modules truncates the production half and blinds every guard to
    ///    everything after it. This file already contains that literal in prose
    ///    elsewhere; the guards survive it today only because the marker they
    ///    match is spelled to avoid it, which is an accident of spelling and
    ///    not a check.
    ///
    /// So the whole region from the cut to EOF is walked and required to be a
    /// sequence of `#[cfg(test)]`-gated, column-0 module blocks and nothing
    /// else, and the cut itself is pinned against a production anchor that must
    /// still be found (a positive control) immediately above it.
    ///
    /// This is a source-analysis test, which is the class that has failed in
    /// this codebase repeatedly, so every part of it carries its own control:
    /// the anchor's occurrence count, the module count, the close count, the
    /// number of lines actually visited, and the string-literal exceptions.
    /// A walk that visited nothing would fail on all five.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_guards_cut() {
        let source = include_str!("app_window.rs");

        // 1. The cut lands where the guards think it does.
        let cut = source.find(BELOW_CUT_MARKER).unwrap_or_else(|| {
            panic!(
                "{BELOW_CUT_MARKER:?} is not in this file at all -- every source guard here \
                 slices at it, and a slice that cannot be made is a guard that reads nothing"
            )
        });
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the marker was matched inside a \
             comment or a string literal rather than at a real declaration; that truncates \
             the production half and blinds every source guard in this file to everything \
             below the truncation point"
        );

        // 2. Positive control on where the cut is: the production half must
        //    still reach the LAST production item in the file. If the marker
        //    were matched earlier than the real test modules, this anchor
        //    would fall below the cut instead of just above it.
        const LAST_PRODUCTION_ITEM: &str = concat!("VaultSessionOutcome { result, ", "stages, relocked }");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: {LAST_PRODUCTION_ITEM:?} is not in this file exactly once, so it no \
             longer pins anything -- repoint it at the last production item above the test \
             modules"
        );
        let anchor = source.find(LAST_PRODUCTION_ITEM).expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, which means \
             the cut moved up and the production half every guard in this file reads is \
             truncated"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk. `lines()` strips the `\r` of this file's CRLF endings,
        //    so every comparison below is against the line's real text.
        let mut depth = 0usize;
        let mut gated = false;
        let mut modules = 0usize;
        let mut closes = 0usize;
        let mut visited = 0usize;
        for line in source[cut..].lines() {
            visited += 1;
            if depth == 0 {
                // Between modules NOTHING is allowed but blanks, comments,
                // the gate and a module opener -- at any indentation, because
                // an indented `fn` at file scope is still a top-level item.
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                if trimmed == BELOW_CUT_GATE {
                    gated = true;
                    continue;
                }
                assert!(
                    !line.starts_with(char::is_whitespace) && below_cut_is_module_opener(trimmed),
                    "top-level source below the cut: {line:?}. Every source guard in this file \
                     slices at {BELOW_CUT_MARKER:?} and reads only what is above it, so an item \
                     down here is read by none of them: it can duplicate a call site pinned at \
                     exactly one, or reintroduce a construct banned by name, and the suite stays \
                     green. Move it above the test modules."
                );
                assert!(
                    gated,
                    "the module {line:?} below the cut is not {BELOW_CUT_GATE:?}-gated, so it \
                     ships -- and it ships in the half of the file no source guard here reads"
                );
                gated = false;
                depth = 1;
                modules += 1;
            } else if !line.is_empty() && !line.starts_with(char::is_whitespace) {
                // Inside a test module every item is indented, so the only
                // column-0 line is the module's own closing brace.
                if line == "}" {
                    depth = 0;
                    closes += 1;
                    continue;
                }
                assert!(
                    BELOW_CUT_STRING_LINES.contains(&line),
                    "a column-0 line inside a test module below the cut: {line:?}. Either a \
                     top-level item escaped the brace count, or this is the contents of a \
                     string literal and belongs in BELOW_CUT_STRING_LINES"
                );
            }
        }

        // 4. The walk is not vacuous, and it finished.
        assert!(
            visited > 100,
            "control: the walk visited only {visited} lines below the cut, which is not a test \
             module's worth -- the slice is empty or nearly so and this test proves nothing"
        );
        assert_eq!(
            depth, 0,
            "a test module below the cut is never closed by a column-0 `}}`, so the walk ran \
             off the end of the file inside it and stopped inspecting top-level lines"
        );
        assert_eq!(
            modules, 6,
            "the number of top-level test modules below the cut changed. That is fine -- but \
             this count is the control that proves the walk really visited them, so update it \
             deliberately rather than loosening it"
        );
        assert_eq!(
            closes, modules,
            "control: every module the walk opened must also have been closed at column 0"
        );
        for known in BELOW_CUT_STRING_LINES {
            assert_eq!(
                source.matches(known).count(),
                1,
                "control: the string-literal exception {known:?} is not in this file exactly \
                 once, so it is stale and is widening this check for nothing"
            );
        }
    }
}

/// The lock catch, and the two events the in-window lock added to the table.
///
/// Every test here is about a window the user asked to LOCK. The failure this
/// whole feature exists to remove is the window going away and coming back --
/// the blink -- and the failure it must not introduce instead is a window that
/// refuses an ordinary close, which is the same gesture arriving with one flag
/// down.
#[cfg(test)]
mod lock_transition_tests {
    use super::*;

    /// Every stage, in the order the enum declares them. Written out rather
    /// than derived, and held to that by `every_stage_and_event_is_in_the_walk`
    /// below -- a `strum`-free crate has no way to iterate a `Copy` enum, and a
    /// list that silently missed a variant would vacate the exhaustive table.
    const ALL_STAGES: &[Stage] = &[Stage::SignIn, Stage::Working, Stage::Vault];

    /// Every event, likewise.
    const ALL_EVENTS: &[Event] = &[
        Event::SignedIn,
        Event::WorkReady,
        Event::WorkFailed,
        Event::Locked,
        Event::TeardownDone,
    ];

    /// The pairs that MOVE, spelled out with what they move to, so a rewritten
    /// `advance` cannot be checked against itself.
    const LEGAL: &[(Stage, Event, Next)] = &[
        (Stage::SignIn, Event::SignedIn, Next::Show(Stage::Working)),
        (Stage::Working, Event::WorkReady, Next::Show(Stage::Vault)),
        (Stage::Working, Event::WorkFailed, Next::Close),
        (Stage::Vault, Event::Locked, Next::Show(Stage::Working)),
        (Stage::Working, Event::TeardownDone, Next::Show(Stage::SignIn)),
    ];

    /// **The two new rows, as the behaviour they buy.**
    ///
    /// `(Vault, Locked)` is the whole feature: a lock that leaves the vault
    /// stage for the spinner *in the same window*. Answering `Next::Close`
    /// here is the shipped bug -- the window is torn down and `main` opens
    /// another one, which is the blink the report is about.
    ///
    /// `(Working, TeardownDone)` is the half that makes the first one
    /// survivable: without it the spinner has nowhere to go once the old
    /// session is gone, and the stage sits there until `WORKING_DEADLINE`
    /// gives up on a worker that is not late but waiting -- for a master
    /// password nobody is being asked for.
    #[test]
    fn a_lock_moves_the_vault_to_the_spinner_and_the_teardown_moves_it_to_the_card() {
        assert_eq!(
            advance(Stage::Vault, Event::Locked),
            Next::Show(Stage::Working),
            "a lock does not move the vault stage to the spinner, so the window is torn down \
             and reopened -- the blink this feature exists to remove"
        );
        assert_eq!(
            advance(Stage::Working, Event::TeardownDone),
            Next::Show(Stage::SignIn),
            "the finished teardown does not reach the sign-in card, so the spinner waits out \
             `WORKING_DEADLINE` for a master password nobody is asking for"
        );
    }

    /// **The whole lock round trip, walked**, because "the two rows are right"
    /// and "the window gets back to the vault" are different claims and this
    /// crate has shipped the first without the second.
    #[test]
    fn the_lock_walk_leaves_the_vault_and_comes_back_to_it() {
        let mut stage = Stage::Vault;
        let mut seen = vec![stage];
        let mut steps = 0;
        for event in [
            Event::Locked,
            Event::TeardownDone,
            Event::SignedIn,
            Event::WorkReady,
        ] {
            steps += 1;
            match advance(stage, event) {
                Next::Show(next) => {
                    if next != stage {
                        stage = next;
                        seen.push(stage);
                    }
                }
                Next::Close => break,
            }
        }
        assert_eq!(steps, 4, "control: the walk stopped early, so it proves less than it says");
        assert_eq!(
            seen,
            vec![
                Stage::Vault,
                Stage::Working,
                Stage::SignIn,
                Stage::Working,
                Stage::Vault
            ],
            "the lock's round trip does not return to the vault in one window"
        );
    }

    /// **The table, exhaustively**: every one of the fifteen pairs is either a
    /// listed move or a no-op, and nothing else.
    ///
    /// This replaces a hand-written list of refused pairs. Two new events
    /// multiplied that list from six entries to ten, and a hand-written ten is
    /// a list the next event will silently under-count -- which is how a stray
    /// `Locked` arriving while the sign-in card is up could jump to a spinner
    /// with no worker behind it, refusing every close with nothing to wait for.
    #[test]
    fn an_event_that_does_not_belong_to_the_current_stage_moves_nothing() {
        let mut checked = 0;
        let mut moved = 0;
        for &stage in ALL_STAGES {
            for &event in ALL_EVENTS {
                checked += 1;
                match LEGAL
                    .iter()
                    .find(|(s, e, _)| *s == stage && *e == event)
                {
                    Some((_, _, expected)) => {
                        moved += 1;
                        assert_eq!(
                            advance(stage, event),
                            *expected,
                            "{event:?} on {stage:?} does not do what the table says"
                        );
                    }
                    None => assert_eq!(
                        advance(stage, event),
                        Next::Show(stage),
                        "{event:?} moved the window away from {stage:?}"
                    ),
                }
            }
        }
        assert_eq!(
            checked,
            ALL_STAGES.len() * ALL_EVENTS.len(),
            "control: the walk did not visit every pair"
        );
        assert_eq!(
            checked, 15,
            "control: three stages and five events is fifteen pairs; a different number means \
             one of the two lists is missing a variant it was supposed to enumerate, and every \
             pair it omits is unchecked"
        );
        assert_eq!(
            moved,
            LEGAL.len(),
            "control: some listed move was never reached by the walk, so listing it asserted \
             nothing"
        );
        assert_eq!(moved, 5, "control: the table has five moves, not {moved}");
    }

    /// **The ordinary close-to-tray gesture, and the lock, are the same
    /// gesture.** They differ by one flag, and this is where the difference is
    /// decided. Getting it backwards costs either the blink (a lock let go) or
    /// a window the user cannot close (an ordinary close caught).
    #[test]
    fn only_a_close_that_carries_a_lost_session_is_a_lock() {
        assert_eq!(
            vault_close(true, true),
            VaultClose::Lock,
            "a lock is let go, so the window closes and reopens -- the blink"
        );
        assert_eq!(
            vault_close(true, false),
            VaultClose::LetGo,
            "an ordinary close is caught as a lock: the ✕ starts a teardown of a session \
             nobody asked to end, and the window will not go away"
        );
        assert_eq!(
            vault_close(false, true),
            VaultClose::Ignore,
            "a vault frame that has ALREADY reported `locked` -- its own flag stays set for \
             the rest of its life -- starts a second teardown on every later frame, twelve \
             times a second, if the close is not what triggers this"
        );
        assert_eq!(vault_close(false, false), VaultClose::Ignore);
    }

    /// The three answers are three, and the two that keep the window are not
    /// the same one. A `vault_close` collapsed to a `bool` is one token from
    /// treating "nothing asked" as "let it go" -- which is inert here and
    /// disastrous in a host that acts on `LetGo`.
    #[test]
    fn the_three_answers_stay_three() {
        assert_ne!(VaultClose::Ignore, VaultClose::LetGo);
        assert_ne!(VaultClose::Ignore, VaultClose::Lock);
        assert_ne!(VaultClose::LetGo, VaultClose::Lock);
    }

    /// Every step, in the order the enum declares them. Written out rather
    /// than derived, and held to that by the exhaustive walk below.
    const ALL_PROGRESS: &[LockProgress] = &[
        LockProgress::TeardownStarted,
        LockProgress::TeardownAlreadySpent,
        LockProgress::VaultRebuilt,
        LockProgress::RebuildFailed,
        LockProgress::TeardownNeverRan,
    ];

    /// **The whole of [`session_torn_down`], spelled out rather than derived
    /// from the function it checks**, and asserted to have visited every one
    /// of the ten `(current, step)` pairs -- a table that silently stopped
    /// covering a pair would be a rule nobody checks for that pair.
    #[test]
    fn the_torn_down_rule_is_a_table_and_every_pair_is_in_it() {
        // (current, step, expected)
        let expected: &[(bool, LockProgress, bool)] = &[
            (false, LockProgress::TeardownStarted, true),
            (true, LockProgress::TeardownStarted, true),
            (false, LockProgress::TeardownAlreadySpent, false),
            (true, LockProgress::TeardownAlreadySpent, true),
            (false, LockProgress::VaultRebuilt, false),
            (true, LockProgress::VaultRebuilt, false),
            (false, LockProgress::RebuildFailed, false),
            (true, LockProgress::RebuildFailed, true),
            (false, LockProgress::TeardownNeverRan, false),
            (true, LockProgress::TeardownNeverRan, false),
        ];
        let mut visited = 0usize;
        for &(current, step, want) in expected {
            assert_eq!(
                session_torn_down(current, step),
                want,
                "session_torn_down({current}, {step:?}) is the wrong way round, and this rule \
                 is what tells the caller whether to run its own lock recovery"
            );
            visited += 1;
        }
        assert_eq!(visited, 10, "the table stopped covering all ten pairs");
        assert_eq!(
            ALL_PROGRESS.len() * 2,
            10,
            "a `LockProgress` variant was added or removed, so the table above is no longer \
             exhaustive -- add its two rows"
        );
        for step in ALL_PROGRESS {
            assert!(
                expected.iter().any(|(_, s, _)| s == step),
                "{step:?} has no row in the table, so its rule is unchecked"
            );
        }
    }

    /// **The second lock of one session must still lock.**
    ///
    /// `teardown` is `FnOnce`, so the second lock in a window starts no
    /// worker and tears nothing down; the caller's own recovery is the only
    /// one that lock will ever get. This is the regression that the field
    /// being set on merely REACHING the lock arm would have shipped: the
    /// caller reads "a teardown has already run", skips its recovery, and the
    /// vault reports itself locked with its cache full and `bw serve` still
    /// answering with a live session.
    #[test]
    fn a_lock_that_started_no_teardown_never_claims_one() {
        assert!(
            !session_torn_down(false, LockProgress::TeardownAlreadySpent),
            "a lock whose FnOnce teardown was already spent claimed a teardown it did not \
             start, so the caller will skip the only recovery that lock can get and the vault \
             will not actually lock"
        );
        // The whole walk, in order: lock, rebuild, lock again.
        let after_first = session_torn_down(false, LockProgress::TeardownStarted);
        let after_rebuild = session_torn_down(after_first, LockProgress::VaultRebuilt);
        let after_second = session_torn_down(after_rebuild, LockProgress::TeardownAlreadySpent);
        assert!(after_first, "the first lock's teardown is not reported at all");
        assert!(
            !after_rebuild,
            "a rebuilt vault still reports the session as torn down, so every later lock in \
             this window is silently disarmed"
        );
        assert!(
            !after_second,
            "the SECOND lock of one session reports a teardown that never happened -- the \
             caller skips its recovery and the vault does not lock"
        );
    }

    /// The other direction, which the same field is also the only guard for:
    /// a teardown that DID run must not be retracted by the rebuild failing,
    /// or the caller asks for the master password the user just gave, to
    /// retry the backend start that just failed.
    #[test]
    fn a_failed_rebuild_does_not_retract_the_teardown_that_ran() {
        let after_lock = session_torn_down(false, LockProgress::TeardownStarted);
        assert!(
            session_torn_down(after_lock, LockProgress::RebuildFailed),
            "a teardown that ran and then failed to repopulate is reported as no teardown at \
             all, so the caller runs the whole sequence again on a session that is already gone"
        );
    }

    /// **A worker that reported NOTHING never tore anything down.**
    ///
    /// The lock arm sets the flag from `std::thread::spawn` returning, which
    /// is a claim about a thread existing and not about the teardown running.
    /// If the frame thread's forwarding of the two channel ends is dropped --
    /// or the worker panics before it asks for the master password -- the
    /// step channel goes `Disconnected` with no step ever seen, and the
    /// window would otherwise tell the caller "already torn down" about a
    /// session whose cache is full and whose `bw serve` still answers. That
    /// is the v0.5.0 defect, reachable through the in-window lock.
    #[test]
    fn a_teardown_that_never_reported_a_step_retracts_its_own_claim() {
        let after_lock = session_torn_down(false, LockProgress::TeardownStarted);
        assert!(after_lock, "control: the lock arm's own step must claim the teardown");
        assert!(
            !session_torn_down(after_lock, LockProgress::TeardownNeverRan),
            "a lock whose worker ended without ever reporting a step still reports the session \
             as torn down, so the caller skips the only recovery that lock can get and the \
             vault does not lock"
        );
    }

    /// **A lock only ever reaches FURTHER**, all nine pairs.
    ///
    /// The property the two `bool` flags did not have. `worker_started` and
    /// `teardown_reported` were ordinary fields, so every `= false` a review
    /// found -- appended to the `NeedsSignIn` arm, appended to the lock catch
    /// -- was a legal write of a legal value that no guard in this file
    /// looked at. Here going backwards is not a defect to be pinned, it is an
    /// answer [`lock_reach_after`] cannot give.
    #[test]
    fn a_lock_only_ever_reaches_further() {
        use LockReach::{Nothing, StepReported, WorkerStarted};
        const ALL: [LockReach; 3] = [Nothing, WorkerStarted, StepReported];
        // (before, reached, after)
        let expected = [
            (Nothing, Nothing, Nothing),
            (Nothing, WorkerStarted, WorkerStarted),
            (Nothing, StepReported, StepReported),
            (WorkerStarted, Nothing, WorkerStarted),
            (WorkerStarted, WorkerStarted, WorkerStarted),
            (WorkerStarted, StepReported, StepReported),
            (StepReported, Nothing, StepReported),
            (StepReported, WorkerStarted, StepReported),
            (StepReported, StepReported, StepReported),
        ];
        for &(before, reached, after) in &expected {
            assert_eq!(
                lock_reach_after(before, reached),
                after,
                "lock_reach_after({before:?}, {reached:?}) is a row of this table that is \
                 wrong, and every row of it is a lock either forgetting a teardown that ran \
                 or claiming one that did not"
            );
        }
        for &before in &ALL {
            for &reached in &ALL {
                assert!(
                    expected.iter().any(|&(b, r, _)| b == before && r == reached),
                    "({before:?}, {reached:?}) has no row in the table, so its answer is \
                     unchecked"
                );
                assert!(
                    lock_reach_after(before, reached) >= before,
                    "a lock at {before:?} was moved BACKWARDS by {reached:?}. That is the \
                     whole defect class: un-reporting a step retracts a teardown that really \
                     ran and `main` dismantles the session twice; un-starting a worker stops \
                     the retraction firing at all and the vault says locked with `bw serve` \
                     still holding the session"
                );
            }
        }
    }

    /// **The two facts the rule reads, and that neither can be lost.**
    ///
    /// `LockStage` is what `InWindowLock` actually keeps, so this is the
    /// accessor pair [`retracts_the_teardown`] is fed. The last line is the
    /// measured survivor `self.teardown_reported = false;` in the only
    /// spelling that still typechecks.
    #[test]
    fn a_stage_never_un_reports_a_step_or_un_starts_a_worker() {
        let mut stage = LockStage::fresh();
        assert!(
            !stage.worker_started() && !stage.teardown_reported(),
            "a window that has not locked already claims a worker or a reported step, so the \
             retraction is decided from facts that were never true"
        );
        stage.reached(LockReach::WorkerStarted);
        assert!(
            stage.worker_started() && !stage.teardown_reported(),
            "the spawn does not record a started worker, or it records a REPORTED step -- the \
             first leaves the retraction unable to fire, the second makes it unreachable"
        );
        stage.reached(LockReach::StepReported);
        assert!(
            stage.worker_started() && stage.teardown_reported(),
            "a reported step does not imply the worker that reported it"
        );
        stage.reached(LockReach::Nothing);
        assert!(
            stage.worker_started() && stage.teardown_reported(),
            "a stage was talked back down to `Nothing`, which is the `= false` write this \
             file has now had three reviews about"
        );
    }

    /// A lock with no frame behind it: the closures are stubs, the rebuild
    /// answers `None` (the failed-rebuild path, which needs no
    /// `VaultFrameHandles` -- there is no way to build one from this module),
    /// and nothing here spawns a process, opens a window or touches disk.
    fn a_lock_under_test(
        relocked: &Rc<RefCell<bool>>,
    ) -> InWindowLock<
        impl FnOnce(&mpsc::Sender<TeardownStep>, mpsc::Receiver<String>) + Send + 'static,
        impl FnOnce(
            Option<crate::settings::Settings>,
        )
            -> Option<(vault_window::VaultFrameFn, vault_window::VaultFrameHandles)>,
    > {
        InWindowLock::new(
            |_step_tx: &mpsc::Sender<TeardownStep>, _token_rx: mpsc::Receiver<String>| {},
            |_edited: Option<crate::settings::Settings>| None,
            Rc::new(RefCell::new(None)),
            relocked.clone(),
            Rc::new(RefCell::new(None)),
        )
    }

    /// **A teardown that REPORTED a step is never retracted** -- driven
    /// through the real value, not through the rule.
    ///
    /// This is the measured survivor A1 as behaviour. `catch_the_lock` cannot
    /// be called from a test (it needs a live `VaultFrameHandles` reporting a
    /// lost session, and this module cannot build one), so the lock arm is
    /// reproduced here in its two effects -- the stage advance and the write
    /// of `relocked` through the rule -- and then the drain and the
    /// retraction are the production ones. A write that un-reports the step,
    /// in the arm or anywhere else, ends with `relocked` false and `main`
    /// tearing down a session that is already dismantled.
    #[test]
    fn a_worker_that_reported_a_step_and_then_died_keeps_its_teardown() {
        let relocked = Rc::new(RefCell::new(false));
        let mut lock = a_lock_under_test(&relocked);
        lock.stage.reached(LockReach::WorkerStarted);
        *relocked.borrow_mut() = session_torn_down(false, LockProgress::TeardownStarted);
        let step_tx = lock.step_tx.take().expect("the lock holds the only sender");
        step_tx.send(TeardownStep::NeedsSignIn).expect("the receiver is the lock own end");
        let mut vault_fn: Option<vault_window::VaultFrameFn> = None;
        assert!(
            matches!(lock.answer_the_teardown(&mut vault_fn), Ok(Event::TeardownDone)),
            "the drain did not answer the step the worker really sent"
        );
        lock.retract_if_the_teardown_never_ran(WorkFailure::WorkerDied);
        assert!(
            *relocked.borrow(),
            "a worker that reported `NeedsSignIn` and then died had its teardown RETRACTED, \
             so `main` runs a second teardown of a session already dismantled -- the master \
             password is asked for again to redo work that is done"
        );
    }

    /// The same walk with the `Finished` step, whose arm also rebuilds.
    #[test]
    fn a_worker_that_finished_and_then_died_keeps_its_teardown() {
        let relocked = Rc::new(RefCell::new(false));
        let mut lock = a_lock_under_test(&relocked);
        lock.stage.reached(LockReach::WorkerStarted);
        *relocked.borrow_mut() = session_torn_down(false, LockProgress::TeardownStarted);
        let step_tx = lock.step_tx.take().expect("the lock holds the only sender");
        step_tx.send(TeardownStep::Finished).expect("the receiver is the lock own end");
        let mut vault_fn: Option<vault_window::VaultFrameFn> = None;
        assert!(
            matches!(lock.answer_the_teardown(&mut vault_fn), Ok(Event::WorkFailed)),
            "the rebuild stub answered `None`, so this walk must be the failed-rebuild one"
        );
        lock.retract_if_the_teardown_never_ran(WorkFailure::WorkerDied);
        assert!(
            *relocked.borrow(),
            "a worker that finished its teardown and then died had it retracted, so the \
             session is torn down twice"
        );
    }

    /// **And the other direction, which is the one the vault actually hangs
    /// on:** a worker that started and reported NOTHING must retract, so the
    /// caller runs the only recovery that lock will ever get.
    #[test]
    fn a_worker_that_reported_nothing_retracts_through_the_real_value() {
        let relocked = Rc::new(RefCell::new(false));
        let mut lock = a_lock_under_test(&relocked);
        lock.stage.reached(LockReach::WorkerStarted);
        *relocked.borrow_mut() = session_torn_down(false, LockProgress::TeardownStarted);
        assert!(*relocked.borrow(), "control: the lock arm claims the teardown it started");
        lock.retract_if_the_teardown_never_ran(WorkFailure::WorkerDied);
        assert!(
            !*relocked.borrow(),
            "a teardown worker that died having reported nothing still reports the session as \
             torn down. `main` skips its own recovery, and the vault shows `locked` with the \
             cache full and `bw serve` still holding the session -- the v0.5.0 defect"
        );
    }

    /// **The whole of [`retracts_the_teardown`], spelled out rather than
    /// derived** -- all eight combinations of its three inputs, in one table,
    /// so a rewritten predicate cannot be checked against itself.
    ///
    /// This test exists because the condition it covers used to be three bare
    /// terms inside `run_from_vault`'s frame closure, where nothing could
    /// reach it. Two separate one-token inversions of it were MEASURED green
    /// across the entire suite, and one of them was the v0.5.0 defect
    /// restored: a worker that dies having reported nothing keeps
    /// `relocked: true`, `main` skips its recovery, and the vault reports
    /// itself locked with a full cache and a live `bw serve`.
    ///
    /// The exhaustiveness is what kills a predicate that IGNORES one of its
    /// arguments: every input is the sole difference between some pair of rows
    /// below, so dropping any one of the three makes two rows disagree.
    #[test]
    fn the_retraction_rule_is_a_table_over_every_combination_of_its_three_inputs() {
        // (why, worker_started, teardown_reported, retracts)
        let expected: &[(WorkFailure, bool, bool, bool)] = &[
            // The one row that retracts, and the only one: the worker really
            // started, it really died, and it never reported a step.
            (WorkFailure::WorkerDied, true, false, true),
            // It reported a step first, so the teardown really ran -- a
            // retraction here sends `main` to tear down a session already
            // dismantled, and the claim it retracts was true.
            (WorkFailure::WorkerDied, true, true, false),
            // No worker was ever spawned (the second lock of one session,
            // `FnOnce` spent), so there is no claim of this shape to retract.
            (WorkFailure::WorkerDied, false, false, false),
            (WorkFailure::WorkerDied, false, true, false),
            // The deadline: the worker is ALIVE and mid-sequence. Never.
            (WorkFailure::Deadline, true, false, false),
            (WorkFailure::Deadline, true, true, false),
            (WorkFailure::Deadline, false, false, false),
            (WorkFailure::Deadline, false, true, false),
        ];
        for &(why, worker_started, teardown_reported, retracts) in expected {
            assert_eq!(
                retracts_the_teardown(why, worker_started, teardown_reported),
                retracts,
                "retracts_the_teardown({why:?}, {worker_started}, {teardown_reported}) is the \
                 wrong way round. Retracting when it should not re-runs a teardown against a \
                 session that is already down, or against one another thread is still \
                 dismantling; NOT retracting when it should leaves the window claiming a \
                 teardown that never happened -- `main` skips its recovery and the vault \
                 reports itself locked with the cache full and `bw serve` still answering"
            );
        }
        assert_eq!(expected.len(), 8, "the table is no longer all eight combinations");
        // Exhaustiveness on `WorkFailure` itself: a third variant makes this
        // fail to compile, rather than quietly leaving four rows unwritten.
        for &(why, ..) in expected {
            match why {
                WorkFailure::WorkerDied | WorkFailure::Deadline => {}
            }
        }
        // Each argument is the SOLE difference across some pair of rows above,
        // so a predicate that ignores it cannot satisfy both. Stated here as
        // well as implied by the table, because "the table is exhaustive" and
        // "every argument matters" are different claims.
        assert_ne!(
            retracts_the_teardown(WorkFailure::WorkerDied, true, false),
            retracts_the_teardown(WorkFailure::Deadline, true, false),
            "`why` is ignored: the deadline and a dead worker are answered alike"
        );
        assert_ne!(
            retracts_the_teardown(WorkFailure::WorkerDied, true, false),
            retracts_the_teardown(WorkFailure::WorkerDied, false, false),
            "`worker_started` is ignored: a lock that spawned nothing retracts a claim it \
             never made"
        );
        assert_ne!(
            retracts_the_teardown(WorkFailure::WorkerDied, true, false),
            retracts_the_teardown(WorkFailure::WorkerDied, true, true),
            "`teardown_reported` is ignored: a worker that reported a step is treated as one \
             that reported none"
        );
    }

    /// The rule and [`session_torn_down`] read together, which is the claim
    /// the window actually makes: on the one retracting row the flag really
    /// goes back to `false`, and on the neighbouring rows it really stays set.
    #[test]
    fn only_the_retracting_row_puts_the_flag_back_to_false() {
        let after_lock = session_torn_down(false, LockProgress::TeardownStarted);
        assert!(after_lock, "control: the lock arm's own step must claim the teardown");
        for (why, worker_started, teardown_reported) in [
            (WorkFailure::WorkerDied, true, true),
            (WorkFailure::Deadline, true, false),
        ] {
            assert!(
                !retracts_the_teardown(why, worker_started, teardown_reported),
                "control: this row is supposed to leave the claim standing"
            );
            assert!(
                after_lock,
                "a row that does not retract must leave `relocked` exactly as the lock arm set \
                 it -- the window must not answer `false` about a session that IS being torn \
                 down"
            );
        }
        assert!(
            retracts_the_teardown(WorkFailure::WorkerDied, true, false),
            "control: the retracting row"
        );
        assert!(
            !session_torn_down(after_lock, LockProgress::TeardownNeverRan),
            "the retraction does not actually clear the flag"
        );
    }

    /// One vault result, spelled out field by field so a new field cannot be
    /// silently defaulted into these fixtures.
    fn result_with(
        locked: bool,
        edited: Option<crate::settings::Settings>,
    ) -> vault_window::VaultWindowResult {
        vault_window::VaultWindowResult {
            locked,
            needs_reauth: false,
            edited_settings: edited,
            switch_to: None,
            add_account: false,
            remove_account: false,
            account_details: None,
        }
    }

    /// A `Settings` that is NOT the default, so "the edit survived" cannot be
    /// satisfied by a struct nobody wrote to.
    fn edited_settings() -> crate::settings::Settings {
        let mut settings = crate::settings::Settings::default();
        settings.auto_lock_enabled = !settings.auto_lock_enabled;
        assert_ne!(
            settings,
            crate::settings::Settings::default(),
            "control: the fixture equals the default, so every assertion below would pass \
             against a `Settings` the gear never touched"
        );
        settings
    }

    /// **THE FINDING: a gear visit made before a lock reached `main` as
    /// nothing at all.**
    ///
    /// `build_frame` gives every frame a fresh `edited_settings` cell, so the
    /// rebuilt session's result carries `None` -- and the tail used to
    /// overwrite the lock's own carefully preserved result with it. `main` is
    /// the only writer of `settings.json`, so the change was not merely
    /// delayed: it was gone.
    #[test]
    fn a_gear_visit_made_before_the_lock_survives_the_rebuilt_session() {
        let merged = carry_settings_forward(
            Some(result_with(true, Some(edited_settings()))),
            Some(result_with(false, None)),
        )
        .expect("a window with two sessions must produce a result");
        assert_eq!(
            merged.edited_settings,
            Some(edited_settings()),
            "the preference change the user made before locking was discarded by the session \
             that replaced it"
        );
        assert!(
            !merged.locked,
            "the LATER session decides the lock/close fields; carrying `locked` forward would \
             send `main` to run a lock recovery against the vault this window just rebuilt"
        );
    }

    /// The other half of the same rule: everything that is *not* a gear visit
    /// comes from the later session, and the rebuilt session's own gear visit
    /// is not overwritten by the older one.
    #[test]
    fn the_rebuilt_sessions_own_gear_visit_wins_when_both_carry_one() {
        let mut newer = edited_settings();
        newer.auto_lock_minutes += 7;
        assert_ne!(newer, edited_settings(), "control: the two fixtures are the same edit");
        let merged = carry_settings_forward(
            Some(result_with(true, Some(edited_settings()))),
            Some(result_with(false, Some(newer.clone()))),
        )
        .expect("a window with two sessions must produce a result");
        assert_eq!(
            merged.edited_settings,
            Some(newer),
            "the gear visit made AFTER the lock was overwritten by the one made before it, so \
             the window's last word about the user's preferences is its first one"
        );
    }

    /// **The failed-rebuild path must not be broken by the fix for the
    /// succeeding one.** No vault came back, so the lock's own result -- the
    /// only one there is, `locked: true` included -- is the whole answer.
    #[test]
    fn a_lock_whose_rebuild_failed_still_reports_the_lock_and_its_gear_visit() {
        let merged = carry_settings_forward(Some(result_with(true, Some(edited_settings()))), None)
            .expect("the failed-rebuild path still has the lock's own result");
        assert!(
            merged.locked,
            "a lock whose rebuild failed stopped reporting the lock, so `main`'s branches see \
             an ordinary close of a vault that is not there"
        );
        assert_eq!(merged.edited_settings, Some(edited_settings()));
    }

    /// An ordinary close of the second host -- no lock at all -- still
    /// produces the frame's own cells. Deleting the tail entirely made this
    /// return `None`, which `main` answers with an all-`false` fallback: a
    /// close that silently discards a gear visit, an account switch and the
    /// warm account details together.
    #[test]
    fn an_ordinary_close_reports_the_only_session_there_was() {
        let merged = carry_settings_forward(None, Some(result_with(false, Some(edited_settings()))))
            .expect("an ordinary close must still produce outcome cells");
        assert_eq!(merged.edited_settings, Some(edited_settings()));
        assert!(
            carry_settings_forward(None, None).is_none(),
            "a window that had no vault frame at all invents a result, which `main` would read \
             as an ordinary close"
        );
    }

    /// How one non-crossing field is read out of a result. A `String` and not
    /// the value, because `VaultWindowResult` is neither `Debug` nor `Clone`
    /// and the six fields are six different types.
    type FieldRead = fn(&vault_window::VaultWindowResult) -> String;

    /// **The six fields [`carry_settings_forward`]'s doc says must come from
    /// the LATER session** -- every field of `VaultWindowResult` except
    /// `edited_settings`.
    ///
    /// A table rather than six assertions, so a seventh field added to the
    /// struct is one row here rather than a whole test nobody writes. The
    /// count is asserted against the fixtures' own field count below.
    fn non_crossing_fields() -> [(&'static str, FieldRead); 6] {
        [
            ("locked", |r| format!("{}", r.locked)),
            ("needs_reauth", |r| format!("{}", r.needs_reauth)),
            ("switch_to", |r| format!("{:?}", r.switch_to)),
            ("add_account", |r| format!("{}", r.add_account)),
            ("remove_account", |r| format!("{}", r.remove_account)),
            ("account_details", |r| format!("{:?}", r.account_details)),
        ]
    }

    /// A result whose six non-crossing fields are ALL set to the value that
    /// asks `main` to do something (`loud`), or all to the value that asks for
    /// nothing (`quiet`).
    ///
    /// This exists because `result_with` built every one of them as `false` /
    /// `None`, so no fixture in this file could tell a field that crossed the
    /// lock from a field that did not: a merge that additionally did
    /// `after.needs_reauth |= before.needs_reauth` -- a master-password prompt
    /// on a vault the window had just rebuilt -- was measured green across the
    /// entire suite.
    ///
    /// Spelled out field by field, and `edited_settings` deliberately `None`
    /// on both sides so the crossing field is tested where it is tested and
    /// not accidentally here.
    fn every_field_set(loud: bool, id: &crate::accounts::AccountId) -> vault_window::VaultWindowResult {
        vault_window::VaultWindowResult {
            locked: loud,
            needs_reauth: loud,
            edited_settings: None,
            switch_to: loud.then(|| id.clone()),
            add_account: loud,
            remove_account: loud,
            account_details: loud.then(|| crate::login_ui::BwStatusDetails {
                status: crate::login_ui::BwStatus::Unlocked,
                user_email: Some("loud@example.invalid".to_string()),
                server_url: Some("https://example.invalid".to_string()),
            }),
        }
    }

    /// **THE FINDING: of the six fields the doc says must not cross the lock,
    /// exactly one was pinned.**
    ///
    /// `locked` had a test; `needs_reauth`, `switch_to`, `add_account`,
    /// `remove_account` and `account_details` had none, and every fixture in
    /// this file built all six as `false`/`None`, so nothing could distinguish
    /// crossing from not. A merge that additionally did
    /// `after.needs_reauth |= before.needs_reauth` survived the whole suite:
    /// a pre-lock re-auth request then reaches `main` about a session the
    /// window has just REBUILT, which is a master password demanded on a live
    /// vault.
    ///
    /// Both directions are checked, because they fail to different mutations:
    /// this one to a field leaking FORWARD out of the ending session, and
    /// [`the_rebuilt_sessions_own_requests_all_survive_the_merge`] to a field
    /// being clobbered on the way through.
    #[test]
    fn no_request_of_the_ending_session_crosses_the_lock_except_the_gear_visit() {
        let id = crate::accounts::AccountId::generate();
        let before = every_field_set(true, &id);
        let after = every_field_set(false, &id);
        let fields = non_crossing_fields();
        assert_eq!(fields.len(), 6, "the table no longer covers every non-crossing field");
        // Control: the two fixtures really do disagree on every field, so a
        // green run below is a fact about the merge and not about two fixtures
        // that were equal all along -- which is precisely the state this file
        // was in.
        for (name, read) in fields {
            assert_ne!(
                read(&before),
                read(&after),
                "control: the two fixtures agree on `{name}`, so the assertion below cannot \
                 tell a field that crossed the lock from one that did not"
            );
        }

        let expected: Vec<String> = fields.iter().map(|(_, read)| read(&after)).collect();
        let merged = carry_settings_forward(Some(before), Some(after))
            .expect("a window with two sessions must produce a result");
        for ((name, read), want) in fields.into_iter().zip(expected) {
            assert_eq!(
                read(&merged),
                want,
                "`{name}` crossed the lock. Every field but `edited_settings` is a request \
                 about the session that is ENDING; carried into the rebuilt session's result \
                 it tells `main` to act on a vault this window has already brought back -- a \
                 lock recovery, a master-password prompt, an account switch or an account \
                 removal against a live session"
            );
        }
    }

    /// The other direction: nothing the REBUILT session asked for is lost or
    /// overwritten by the older result. Without this, a merge that simply
    /// zeroed the six fields would pass the test above.
    #[test]
    fn the_rebuilt_sessions_own_requests_all_survive_the_merge() {
        let id = crate::accounts::AccountId::generate();
        let before = every_field_set(false, &id);
        let after = every_field_set(true, &id);
        let fields = non_crossing_fields();
        for (name, read) in fields {
            assert_ne!(
                read(&before),
                read(&after),
                "control: the two fixtures agree on `{name}`"
            );
        }
        let expected: Vec<String> = fields.iter().map(|(_, read)| read(&after)).collect();
        let merged = carry_settings_forward(Some(before), Some(after))
            .expect("a window with two sessions must produce a result");
        for ((name, read), want) in fields.into_iter().zip(expected) {
            assert_eq!(
                read(&merged),
                want,
                "`{name}` was dropped or overwritten by the pre-lock session's answer, so a \
                 request the user made AFTER signing back in is silently discarded"
            );
        }
    }

    /// The gear visit crosses in the presence of all six, which the two tests
    /// above deliberately do not check (their fixtures carry no edit at all).
    /// Together the three say: exactly one field crosses, and it is that one.
    #[test]
    fn the_gear_visit_crosses_while_the_six_beside_it_do_not() {
        let id = crate::accounts::AccountId::generate();
        let mut before = every_field_set(true, &id);
        before.edited_settings = Some(edited_settings());
        let after = every_field_set(false, &id);
        let merged = carry_settings_forward(Some(before), Some(after))
            .expect("a window with two sessions must produce a result");
        assert_eq!(
            merged.edited_settings,
            Some(edited_settings()),
            "the gear visit made before the lock did not survive alongside the six fields that \
             must not"
        );
        for (name, read) in non_crossing_fields() {
            assert_eq!(
                read(&merged),
                read(&every_field_set(false, &id)),
                "`{name}` crossed the lock when a gear visit was present, so the one field \
                 that is allowed to cross drags the other six with it"
            );
        }
    }
}

/// The vault host's own wiring, pinned by source position -- the same way
/// `startup_window_tests` pins the startup host's, and for the same reason:
/// `eframe::Frame` has no public constructor, so neither frame closure can be
/// called by a test at all.
#[cfg(test)]
mod lock_host_tests {
    use super::startup_window_tests::{code, production};

    /// **One eframe launch, one raise, two hosts.**
    ///
    /// `foreground.rs` lists this module as opening exactly one window titled
    /// `WINDOW_TITLE` and raising it exactly once, and that file is where the
    /// count lives. This is the same fact asserted from the side that can
    /// explain it: a second host that opened its own window would need that
    /// list relaxed to two, which is the strictly weaker statement -- and it
    /// would ship a second first-frame styling pass that can forget the raise
    /// independently of the first.
    ///
    /// Counted over the COMMENT-STRIPPED production half, because the needles
    /// are the sort a doc comment wants to name; this crate has shipped a
    /// guard that matched its own prose before.
    #[test]
    fn both_hosts_go_through_the_one_window_opener() {
        let production = code(production());
        assert_eq!(
            production.matches(concat!("run_ui_", "native(WINDOW_TITLE,")).count(),
            1,
            "this module opens its window somewhere other than `run_the_one_window`, or in \r
             more than one place -- `foreground.rs` says it opens exactly one"
        );
        assert_eq!(
            production.matches(concat!("raise_window(", "WINDOW_TITLE)")).count(),
            1,
            "this module asks for the foreground somewhere other than the one opener"
        );
        assert_eq!(
            production.matches(concat!("run_the_one_", "window(options, move |ui, frame|")).count(),
            2,
            "the two hosts do not both hand their closure to the one opener, so one of them \r
             either draws nothing or opens a window of its own"
        );
    }

    /// **The host's own body**: `run_from_vault` from its head to the end of
    /// production code, comments stripped.
    ///
    /// **The lock's touch points are NO LONGER IN HERE**, and that is the
    /// whole of what changed under these guards. `InWindowLock` and
    /// `finish_the_locked_session` sit ABOVE `run` -- with the one window
    /// opener, which is where this module keeps what both hosts share -- so
    /// this slice is the CALL SITES and nothing else. Every guard below
    /// therefore says which of the two regions it is a statement about, and
    /// the ones that moved carry a control asserting the needle is NOT in the
    /// other region. A guard that quietly matched the lifted body from here,
    /// or the call site from there, would pass for free, which is this
    /// ledger's house defect in its precise form for a lift.
    ///
    /// Stripping first is not tidiness: the arms below are guarded on names
    /// -- `CancelClose`, `vault_close(`, `TeardownStep::` -- that this
    /// module's own prose says out loud, and a guard that matched the raw
    /// source would go on passing after the call itself was deleted. This
    /// crate has shipped exactly that mistake before.
    fn closure() -> String {
        let production = code(production());
        let at = production
            .find(concat!("pub fn run_from_", "vault<T, S, B>("))
            .expect(
                "the vault host is gone entirely -- the lock is back to tearing the window \
                 down and reopening it",
            );
        let closure = production[at..].to_string();
        assert!(
            closure.len() > 3_000,
            "the vault host sliced down to {} bytes, which is not the whole of it -- every \
             guard below would then be a statement about a region that stops short of the \
             code it names",
            closure.len()
        );
        // **The lifted value is really outside this slice.** If it were ever
        // moved below the host, every "the call site does X" guard here would
        // be satisfied by the lifted body itself and every "the lifted body
        // does X" control would be satisfied twice.
        assert!(
            !closure.contains(concat!("struct InWindow", "Lock<T, B> {")),
            "the shared lock value has moved BELOW the vault host, so this slice contains it \
             and the two regions the guards below distinguish are one region"
        );
        closure
    }

    /// The shared lock value: from its declaration to the startup host, which
    /// is the next production item below it. Comments already stripped.
    ///
    /// Bounded forward at `run` rather than run to the end of production, for
    /// the reason `startup_window_tests::closure` is bounded at
    /// `run_from_vault`: unbounded, every `contains` here would be satisfied
    /// by either host's own body and deleting the lifted code would leave this
    /// file green on the strength of a call site.
    fn lock_value() -> String {
        let production = code(production());
        let at = production
            .find(concat!("struct InWindow", "Lock<T, B> {"))
            .expect(
                "the shared lock value is gone -- the lock's touch points are back inside one \
                 host, so the other host can only get them by copying them",
            );
        let rest = &production[at..];
        let end = rest
            .find(concat!("pub fn ", "run<P, W, V>("))
            .expect("the shared lock value is not above the startup host");
        let value = rest[..end].to_string();
        assert!(
            value.len() > 2_000,
            "the shared lock value sliced down to {} bytes, which is not the whole of it",
            value.len()
        );
        // Positive control on the slice: it really is the lifted region and
        // not a region that reaches into a host.
        assert!(
            !value.contains(concat!("run_the_one_", "window(options, move |ui, frame|")),
            "control: the lifted region reaches into a host's frame closure"
        );
        value
    }

    /// One method of the lifted value, bounded by the next item below it.
    /// Every guard that moved names the METHOD it moved into, not merely the
    /// value: a needle that drifted from the catch into the drain would
    /// otherwise still be "in the lifted body".
    fn lifted(from: &str, to: &str) -> String {
        let value = lock_value();
        let start = value
            .find(from)
            .unwrap_or_else(|| panic!("the lifted value has no {from:?}: {value}"));
        let rest = &value[start..];
        let end = rest
            .find(to)
            .unwrap_or_else(|| panic!("{from:?} is not followed by {to:?}: {value}"));
        let body = rest[..end].to_string();
        assert!(
            body.len() > 200,
            "{from:?} sliced down to {} bytes, which is not the whole of it",
            body.len()
        );
        body
    }

    /// The two regions a lock decision could live in: the shared value and
    /// the host that calls it. **Not the whole production half** -- the rule
    /// `retracts_the_teardown` is itself written in terms of `worker_started`
    /// and `teardown_reported`, so a ban over the file would forbid the rule
    /// its own body and the guard would be red on a correct tree.
    fn where_a_lock_decision_could_live() -> String {
        format!("{}\n{}", lock_value(), closure())
    }

    fn catch_body() -> String {
        lifted(concat!("fn catch_the_", "lock("), concat!("fn hand_over_the_", "token("))
    }

    fn drain_body() -> String {
        lifted(
            concat!("fn answer_the_", "teardown("),
            concat!("fn retract_if_the_teardown_never_", "ran("),
        )
    }

    fn retract_body() -> String {
        lifted(
            concat!("fn retract_if_the_teardown_never_", "ran("),
            concat!("fn finish_the_locked_", "session("),
        )
    }

    fn tail_body() -> String {
        let value = lock_value();
        let at = value
            .find(concat!("fn finish_the_locked_", "session("))
            .expect("the shared tail is gone, so each host merges its two sessions by hand");
        value[at..].to_string()
    }

    /// The host's `Stage::Vault` arm alone: the call site of the lock catch,
    /// and nothing else.
    fn vault_arm() -> String {
        let closure = closure();
        let start = closure
            .find(concat!("Stage::Vault ", "=>"))
            .expect("the vault host has no vault stage at all");
        let rest = &closure[start..];
        let end = rest
            .find(concat!("Stage::SignIn ", "=>"))
            .expect("the vault arm is not followed by the sign-in arm");
        let arm = rest[..end].to_string();
        assert!(
            arm.len() > 200,
            "the vault arm sliced down to {} bytes, which is not the whole of it",
            arm.len()
        );
        arm
    }

    /// **THE FEATURE, as a source guard.** The vault frame asks for the close
    /// itself on all three lock routes; if the catch does not cancel it, the
    /// window goes away and `main` opens another -- which is the blink, with
    /// every behavioural test in this file still green, because none of them
    /// can run a frame.
    ///
    /// **Two halves now, and both are load-bearing.** The cancel and the
    /// question live in `InWindowLock::catch_the_lock`, so that the startup
    /// host reaches the same code rather than a second copy; the host's vault
    /// arm has to CALL it. Pinned separately, with a control on each that the
    /// needle is not in the other region -- a lift whose body is perfect and
    /// whose call site was dropped is a window that closes on every lock, and
    /// it is invisible to any guard that reads the two regions as one.
    #[test]
    fn the_lock_arm_keeps_the_window_instead_of_letting_it_close() {
        let catch = catch_body();
        assert!(
            catch.contains(concat!("ViewportCommand::", "CancelClose")),
            "the lock catch does not cancel the vault frame's own close, so the window is torn \
             down and reopened -- the blink this feature exists to remove: {catch}"
        );
        // **Both arguments, in order.** `lost` replaced by `false` was measured
        // green across the whole suite here and at `8556e21`, where this call
        // was still inline: a session the backend has lost then reads as an
        // ordinary close, the window goes and `main` opens another -- the
        // blink, on the one route the user did not ask for. The rule cannot
        // see it (both are `bool`), so it is pinned here.
        assert!(
            catch.contains(concat!(
                "vault_",
                "close(ctx.input(|i| i.viewport().close_requested()), lost)"
            )),
            "the lock catch no longer asks `vault_close` what the close meant, with the \
             close request and the lost session in that order, so the decision is back inside \
             a frame closure no test can call -- or it is asking about something other than \
             this frame: {catch}"
        );
        assert!(
            catch.contains(concat!(
                "let lost = self.vault_handles.borrow().as_ref().is_some_and(|h| h.lost_",
                "session());"
            )),
            "the lost-session half of the question is read from something other than the \
             handles of the vault frame that is up, so a lock forced by a backend that dropped \
             the session is not caught: {catch}"
        );
        // Positive control on the slice: it really is the catch.
        assert!(
            catch.contains(concat!("VaultClose::", "Lock =>")),
            "control: the sliced region is not the method that decides what a close meant: \
             {catch}"
        );

        let closure = closure();
        assert_eq!(
            closure
                .matches(concat!("if lock.catch_the_", "lock(ui.ctx(), &mut vault_fn) {"))
                .count(),
            1,
            "the vault host does not ask the shared lock value to catch its close, exactly \
             once, with this frame's context and this host's vault frame slot. A MISSING call \
             is a perfectly lifted catch that nothing runs -- every guard over the lifted body \
             stays green and the window closes on every lock, which is the blink restored by \
             the refactor meant to enable removing it: {closure}"
        );
        // Positive control on the slice: it really is the arm that draws the
        // vault, and the call is in THAT arm rather than anywhere in the host.
        let arm = vault_arm();
        assert!(
            arm.contains(concat!("vault_fn(ui, ", "frame);")),
            "control: the sliced region is not the arm that draws the vault: {arm}"
        );
        assert!(
            arm.contains(concat!("if lock.catch_the_", "lock(ui.ctx(), &mut vault_fn) {")),
            "the catch is called from somewhere other than the vault arm, so a lock that \
             arrives while the vault is on screen is not caught at all: {arm}"
        );
        // **The call IS the condition, and nothing stands in front of it.**
        // `if false && lock.catch_the_lock(..)` left the needle in place and
        // the whole suite green: short-circuited away, the catch never runs,
        // so nothing is cancelled and nothing is torn down and the window
        // closes on every lock. Pinning the `if ` and the `{` is what makes
        // the guard a statement about a call the code REACHES rather than one
        // it merely contains -- the same reachability-over-mention lesson the
        // teardown flags' adjacency pin records.
        let at = arm
            .find(concat!("lock.catch_the_", "lock(ui.ctx(), &mut vault_fn)"))
            .expect("pinned just above");
        let transition = arm
            .find(concat!("advance(stage, Event::", "Locked)"))
            .unwrap_or_else(|| panic!("the vault arm takes no `Locked` transition: {arm}"));
        assert!(
            transition > at && transition - at < 200,
            "the `Locked` transition is not inside the block the catch guards, so the catch's \
             answer is computed and discarded: the window stays on the vault stage with its \
             close cancelled and no spinner ever appears"
        );

        // **Neither region holds the other's needle.** Without these two the
        // guard above would pass for free the moment the lifted body moved
        // back into the host, or the call site grew a second inline copy.
        assert!(
            !closure.contains(concat!("ViewportCommand::", "CancelClose")),
            "the vault host cancels a close of its own, beside the shared catch -- a second \
             copy of the touch point this lift exists to share: {closure}"
        );
        assert!(
            !catch.contains(concat!("lock.catch_the_", "lock(")),
            "control: the lifted catch contains its own call site, so the two slices overlap"
        );

        // The plain Close nobody may send, over BOTH regions.
        for (region, what) in [(&closure, "the vault host"), (&catch, "the lock catch")] {
            assert!(
                !region.contains(concat!("ViewportCommand::", "Close)")),
                "{what} sends a plain Close of its own: {region}"
            );
        }
    }

    /// **Every write of `relocked` goes through [`session_torn_down`], and
    /// none is a literal.**
    ///
    /// The name says "two" because that is what there were when the mutant
    /// below was first killed; there are THREE now -- the lock, the rebuild,
    /// and the retraction when the teardown worker dies having reported
    /// nothing. The count moved, the rule did not, and the mutant the name
    /// records (a literal `= true` in the lock arm) is still killed here.
    ///
    /// **Counted twice, deliberately.** Three inside the lifted value says
    /// every write the lock makes is routed; three in the WHOLE production
    /// half says there is no fourth anywhere else -- in particular not a copy
    /// left behind in a host, which counting the lifted region alone could
    /// never see.
    #[test]
    fn the_hosts_two_relocked_writes_both_go_through_the_rule() {
        let value = lock_value();
        let production = code(production());
        for (region, where_) in [
            (&value, "the shared lock value"),
            (&production, "this file"),
        ] {
            assert_eq!(
                region.matches(concat!("session_torn_", "down(was, ")).count(),
                3,
                "{where_} does not have exactly three writes of `relocked` routed through the \
                 rule -- one at the lock, one at the rebuild, and one retracting a teardown \
                 that never ran. A write that bypasses it is a `relocked` nothing checks; a \
                 MISSING one is a lock that claims a teardown nothing performed; a FOURTH is a \
                 second copy of a touch point that is supposed to exist once: {region}"
            );
        }
        // The two counts agreeing is the statement: all three writes are in
        // the lifted value and none survives in a host.
        for literal in [
            concat!("self.relocked.borrow_mut() = ", "true"),
            concat!("self.relocked.borrow_mut() = ", "false"),
            concat!("relocked_for_closure.borrow_mut() = ", "true"),
            concat!("relocked_for_closure.borrow_mut() = ", "false"),
        ] {
            assert!(
                !production.contains(literal),
                "this file assigns `relocked` the literal {literal:?} instead of asking \
                 `session_torn_down`, which is the shape that shipped a lock claiming a \
                 teardown it never started: {production}"
            );
        }
        // Positive controls: all five of the rule's decision points are really
        // in the lifted value, so the counts above are over the real sites.
        for needle in [
            concat!("LockProgress::", "TeardownStarted"),
            concat!("LockProgress::", "TeardownAlreadySpent"),
            concat!("LockProgress::", "VaultRebuilt"),
            concat!("LockProgress::", "RebuildFailed"),
            concat!("LockProgress::", "TeardownNeverRan"),
        ] {
            assert!(
                value.contains(needle),
                "control: {needle:?} is not in the shared lock value, so one of the five steps \
                 the rule is defined over is never actually reported: {value}"
            );
        }
    }

    /// **The retraction ASKS [`retracts_the_teardown`]; it does not decide
    /// inline.**
    ///
    /// The unit table above holds the rule to all eight combinations of its
    /// inputs. This is the other half, and it is the half that was missing:
    /// the condition selecting the retraction lived in a frame closure as
    /// three bare terms, and two separate one-token inversions of it --
    /// inverting `!teardown_reported`, and `WorkerDied` for `Deadline` -- were
    /// MEASURED green across the whole suite. `eframe::Frame` has no public
    /// constructor, so nothing behavioural in this crate can reach the arm; a
    /// source pin on the call is what ties the code to the tabled rule.
    ///
    /// The call is pinned AS WRITTEN, arguments and order included, so a
    /// swapped pair is caught here rather than by the rule (which cannot see
    /// it -- both are `bool`).
    ///
    /// **All of it now measures the lifted value**, because that is where the
    /// two flags live: they are private fields written in one arm each, and
    /// the host cannot reach them at all. The one thing measured at the CALL
    /// SITE is that the host asks for the retraction -- a lifted retraction
    /// nobody calls is a lock whose dead worker still reports itself torn
    /// down.
    #[test]
    fn the_retraction_asks_the_rule_rather_than_deciding_it_inline() {
        let value = lock_value();
        let retract = retract_body();
        assert_eq!(
            value
                .matches(concat!(
                    "retracts_the_teardown(why, self.stage.worker_started(), ",
                    "self.stage.teardown_reported())"
                ))
                .count(),
            1,
            "the shared lock value does not select the retraction through \
             `retracts_the_teardown`, called exactly once with `why`, `worker_started` and \
             `teardown_reported` in that order. A MISSING call is a lock whose teardown never \
             ran still reporting itself torn down -- `main` skips the only recovery it can get \
             and the vault says locked with the cache full and `bw serve` answering. A SECOND \
             call is a second decision point the rule does not govern: {value}"
        );
        assert!(
            retract.contains(concat!(
                "retracts_the_teardown(why, self.stage.worker_started(), ",
                "self.stage.teardown_reported())"
            )),
            "the call is not in `retract_if_the_teardown_never_ran`, so it is somewhere the \
             failure kind is not the one being answered: {retract}"
        );
        // The inline shape, banned term by term, over the shared value AND
        // the host -- the two places a decision could be written back by
        // hand, and not the rule's own body, which is written in exactly
        // these terms and must stay legal. Each of these three is one of the
        // mutations that survived when the condition lived in the closure.
        let regions = where_a_lock_decision_could_live();
        for (fragment, why) in [
            (
                concat!("why == WorkFailure::", "WorkerDied"),
                "the failure kind is tested by hand again, so which kinds retract is back to \
                 being a decision no test can reach",
            ),
            (
                concat!("&& worker_", "started"),
                "`worker_started` is conjoined inline instead of being handed to the rule",
            ),
            (
                concat!("!teardown_", "reported"),
                "`teardown_reported` is negated inline -- the exact term whose inversion \
                 restored the v0.5.0 defect and left the whole suite green",
            ),
        ] {
            assert!(
                !regions.contains(fragment),
                "{why}: {fragment:?} is back in the lock's own code: {regions}"
            );
        }
        // The host asks for it, exactly once, and hands the kind straight on.
        let closure = closure();
        assert_eq!(
            closure
                .matches(concat!("lock.retract_if_the_teardown_never_", "ran(why)"))
                .count(),
            1,
            "the vault host does not ask the shared value to retract, exactly once, with the \
             failure kind the watchdog just decided. A MISSING call leaves a perfectly lifted \
             retraction that nothing runs: a teardown worker that dies having reported nothing \
             still reports the session torn down, `main` skips the only recovery that lock can \
             get, and the vault says locked with `bw serve` still answering: {closure}"
        );
        // Positive controls. The two facts the rule is asked about are really
        // maintained in the lifted value, and the call really does guard the
        // retraction -- otherwise the pin above would be a call whose answer
        // is discarded.
        for (needle, why) in [
            (
                concat!("self.stage.reached(LockReach::", "WorkerStarted);"),
                "nothing ever records that a worker started, so the rule is asked about a flag \
                 that is always `false` and the retraction never fires",
            ),
            (
                concat!("self.stage.reached(LockReach::", "StepReported);"),
                "nothing ever records that a step arrived, so the rule is asked about a flag \
                 that is always `false` and a genuine teardown gets retracted",
            ),
            (
                concat!("LockProgress::", "TeardownNeverRan)"),
                "control: the step the retraction reports is not in the lifted value at all, \
                 so the call pinned above guards nothing",
            ),
        ] {
            assert!(value.contains(needle), "{why}: {needle:?} is not in the shared lock value");
        }
        // **Where the two flags are SET, not merely that they are set.** A
        // correct rule fed a flag written in the wrong arm is this file's
        // recurring defect one level down: `teardown_reported = true;` hoisted
        // into the lock catch makes the retraction unreachable, and
        // `worker_started = true;` moved outside the spawn makes it claim a
        // worker that the spent-`FnOnce` path never started -- and both leave
        // the rule, the table and the call above completely untouched.
        assert_eq!(
            regions.matches(concat!("stage.reached(LockReach::", "WorkerStarted);")).count(),
            1,
            "`worker_started` is set somewhere other than exactly once: {regions}"
        );
        let catch = catch_body();
        let spawn = catch
            .find(concat!("std::thread::", "spawn(move || {"))
            .expect("the spawn is pinned by the_lock_arm_starts_the_teardown_on_a_thread");
        let started =
            catch.find(concat!("stage.reached(LockReach::", "WorkerStarted);")).expect("counted above");
        let claim = catch
            .find(concat!("LockProgress::", "TeardownStarted"))
            .expect("pinned by the_hosts_two_relocked_writes_both_go_through_the_rule");
        assert!(
            spawn < started && started < claim,
            "`worker_started` is not set between the spawn and the `TeardownStarted` claim, so \
             it no longer means what the rule is told it means -- set before the spawn it is \
             `true` on the second lock of one session, whose `FnOnce` teardown is spent and \
             which starts no worker at all"
        );
        // Both of the teardown's steps set `teardown_reported`, and each does
        // it inside its own arm. The two arms are pinned by
        // `the_host_answers_both_teardown_steps`.
        let drain = drain_body();
        assert_eq!(
            regions.matches(concat!("stage.reached(LockReach::", "StepReported);")).count(),
            2,
            "`teardown_reported` is not set exactly twice -- once per teardown step. A \
             MISSING one is a worker whose reported step is forgotten, so a session that \
             really was torn down gets a second teardown from `main`; an EXTRA one, or one \
             hoisted out of these arms, makes the retraction unreachable and restores the \
             v0.5.0 defect: {regions}"
        );
        for step in [
            concat!("Ok(TeardownStep::", "NeedsSignIn) =>"),
            concat!("Ok(TeardownStep::", "Finished) =>"),
        ] {
            let arm = drain
                .find(step)
                .unwrap_or_else(|| panic!("control: {step:?} is not in the drain: {drain}"));
            let next = drain[arm..]
                .find(concat!("self.stage.reached(LockReach::", "StepReported);"))
                .unwrap_or_else(|| panic!("no `teardown_reported` write follows {step:?}: {drain}"));
            assert!(
                next < 400,
                "the `teardown_reported` write nearest to {step:?} is {next} bytes below it, so \
                 it is not this arm's own write -- one of the two steps no longer records that \
                 the worker reported anything"
            );
            // **REACHABILITY, not distance.** The byte budget above only
            // says the write is NEAR its arm; it says nothing about whether
            // the arm ever executes it. Wrapping the write in
            // `if closing.decided() { ... }` -- inside an arm that is
            // already under `if !closing.decided()`, so the write is dead --
            // costs about 60 bytes against a 400-byte budget and left the
            // whole suite green: count still 2, both writes still near their
            // arms, rule and table and call-site pin all untouched, and a
            // worker that reports `NeedsSignIn` and then dies has its
            // teardown RETRACTED, so `main` tears down a session already
            // dismantled. That is the "MISSING one" case the count's own
            // message names, reached without moving the count.
            //
            // So the write is pinned as the arm's FIRST STATEMENT: between
            // the arm's marker and the write there may be the arm's own
            // opening brace and nothing else. Comments are already stripped
            // by `code`, so this is a statement about code. Any wrapper --
            // `if`, `match`, a nested block, a closure -- puts a second
            // token in that gap and fails here, whatever its size. The
            // distance assertion above is KEPT rather than replaced: it is
            // what kills a write hoisted OUT of its arm, which adjacency
            // alone would not distinguish from a correct arm whose own write
            // moved.
            let between = drain[arm + step.len()..arm + next].trim();
            assert_eq!(
                between, "{",
                "the `teardown_reported` write for {step:?} is not that arm's first \
                 statement -- {between:?} stands between the arm and the write, so the \
                 write is nested inside something that may not run. A write the arm does \
                 not reach leaves `teardown_reported` false for a step that WAS reported, \
                 and the retraction then un-reports a teardown that really happened: \
                 `main` runs a second teardown of a session already torn down"
            );
        }

        // The call and the write it guards are adjacent -- the retraction is
        // not a call whose answer is computed and then thrown away.
        let at = retract
            .find(concat!(
                "retracts_the_teardown(why, self.stage.worker_started(), ",
                "self.stage.teardown_reported())"
            ))
            .expect("counted just above");
        let write = retract
            .find(concat!("session_torn_down(was, LockProgress::", "TeardownNeverRan)"))
            .unwrap_or_else(|| {
                panic!("the retraction's write of `relocked` is not in this method: {retract}")
            });
        assert!(
            write > at && write - at < 1_200,
            "the retraction's write is not inside the block the rule guards, so the rule's \
             answer is computed and discarded -- a correct condition reaching nothing"
        );
    }

    /// **The lock actually starts the teardown**, on a thread.
    ///
    /// The v0.5.0 defect in its new home: a lock that reaches the spinner and
    /// never tears anything down leaves `bw serve` holding a live session with
    /// the vault "locked" on screen. Run on the frame thread instead, it
    /// freezes the window on the frame that is meant to start showing the
    /// spinner.
    ///
    /// Measured over the lifted catch, with a control that no host spawns a
    /// teardown of its own beside it -- which is the second copy this lift
    /// exists to make impossible.
    #[test]
    fn the_lock_arm_starts_the_teardown_on_a_thread() {
        let catch = catch_body();
        assert!(
            catch.contains(concat!("std::thread::", "spawn(move || {")),
            "the lock does not start its teardown on a worker thread: either nothing is torn \
             down at all -- the vault says locked and `bw serve` still holds the session -- or \
             it runs on the frame thread and freezes the spinner it is meant to be showing: \
             {catch}"
        );
        assert!(
            catch.contains(concat!("teardown(&step_tx, ", "token_rx);")),
            "the spawned thread does not run the caller's teardown: {catch}"
        );
        assert!(
            catch.contains(concat!("handles.", "finish()")),
            "the vault session that just locked is not ended through `finish`, so its geometry \
             is never written and a visit to the gear earlier in the same session is lost: \
             {catch}"
        );
        let production = code(production());
        assert_eq!(
            production.matches(concat!("teardown(&step_tx, ", "token_rx);")).count(),
            1,
            "the caller's teardown is run from more than one place in this file, which is the \
             second copy of the lock's touch points that the shared value exists to prevent: \
             {production}"
        );
        let closure = closure();
        assert!(
            !closure.contains(concat!("std::thread::", "spawn(move || {")),
            "the vault host starts a thread of its own beside the shared catch: {closure}"
        );
    }

    /// **The working stage is this module's working stage, not a second one.**
    ///
    /// A host that re-implemented the refusal, the watchdog or the closing flag
    /// would be a second copy of the most heavily guarded sequence in this
    /// crate -- which is the move the recorded design rejects by name.
    ///
    /// These five needles did NOT move: the stage machinery is the host's and
    /// stays there. What moved is what the stage's drain does, which is the
    /// guard below.
    #[test]
    fn the_vault_host_reuses_the_one_working_stage_rather_than_writing_a_second() {
        let closure = closure();
        for (needle, why) in [
            (
                concat!("refuse_close_while_", "working(ui.ctx(), closing)"),
                "the vault host's spinner refuses no close it did not draw the affordance for, \
                 so an Alt+F4 mid-teardown leaves a half-stopped backend",
            ),
            (
                concat!("poll_", "working(err, elapsed)"),
                "the vault host's spinner has no watchdog: a teardown worker that panics or \
                 hangs leaves a spinner that spins forever, refusing every close",
            ),
            (
                concat!("give_up_", "working(ui.ctx(), &mut closing, why, elapsed)"),
                "the vault host does not end its own stage the way the startup host does",
            ),
            (
                concat!("Closing::", "not_yet()"),
                "the vault host's working stage starts on something other than `not_yet`, and \
                 started on `decided` it refuses nothing and drains nothing",
            ),
            (
                concat!("CloseControl::", "Disabled"),
                "the vault host's spinner draws a live ✕ over a stage that refuses every close",
            ),
        ] {
            assert!(closure.contains(needle), "{why}: {needle:?} is not in the vault host");
        }
        // Positive control: the slice really is the vault host and not the
        // whole production half, whose startup host contains all of the above
        // too.
        assert!(
            !closure.contains(concat!("pub fn ", "advance(")),
            "control: the sliced region reaches back above the vault host"
        );
        assert!(
            closure.contains(concat!("lock.answer_the_", "teardown(&mut vault_fn)")),
            "control: the sliced region is not the host that drains the teardown's steps"
        );
    }

    /// **Both of the teardown's two steps are handled**, and they are handled
    /// differently. Collapsed to one, either the card is never shown (the
    /// spinner waits out the deadline for a password nobody is asked for) or
    /// the vault is never rebuilt (the window closes and `main` reopens it --
    /// the blink, one step later).
    ///
    /// The two arms are the lifted drain's; the transition they produce is the
    /// host's, because the state machine belongs to the window and not to the
    /// lock. Both halves are pinned, and each carries the control that the
    /// other region does not hold its needle.
    #[test]
    fn the_host_answers_both_teardown_steps() {
        let drain = drain_body();
        for (needle, why) in [
            (
                concat!("Ok(TeardownStep::", "NeedsSignIn) =>"),
                "the card is never shown when the teardown asks for a password",
            ),
            (
                concat!("Ok(TeardownStep::", "Finished) =>"),
                "nothing notices the teardown finished, so the vault is never rebuilt",
            ),
            (
                concat!("Event::", "TeardownDone"),
                "the drain does not produce the `TeardownDone` transition",
            ),
            (
                concat!("Event::", "WorkReady"),
                "a rebuilt vault does not produce the transition that shows it",
            ),
            (
                concat!("Event::", "WorkFailed"),
                "a failed rebuild does not produce the transition that closes the window",
            ),
        ] {
            assert!(drain.contains(needle), "{why}: {needle:?} is not in the lifted drain");
        }
        let closure = closure();
        assert_eq!(
            closure.matches(concat!("lock.answer_the_", "teardown(&mut vault_fn)")).count(),
            1,
            "the vault host does not drain the teardown's steps through the shared value, \
             exactly once, handing it this host's vault frame slot. A MISSING call is a \
             perfectly lifted drain that nothing runs: the spinner waits out the deadline for \
             a step that has already arrived: {closure}"
        );
        for (needle, why) in [
            (
                concat!("advance(stage, ", "event)"),
                "the host does not feed the drain's answer to the transition table, so a \
                 finished teardown moves the window nowhere",
            ),
            (
                concat!("Event::", "Locked"),
                "the host does not take the `Locked` transition, so the lock moves nothing",
            ),
        ] {
            assert!(closure.contains(needle), "{why}: {needle:?} is not in the vault host");
        }
        // The kinds stay apart, in BOTH regions: the drain hands its `Err`
        // straight on and the host gives it to the watchdog whole.
        for (region, what) in [(&drain, "the lifted drain"), (&closure, "the vault host")] {
            assert!(
                !region.contains(concat!("Err(", "_) =>")),
                "{what} treats every channel error alike, so a dead teardown worker is polled \
                 as a busy one -- a spinner that refuses every close, forever: {region}"
            );
        }
    }

    /// **The tail MERGES the two sessions' results; it does not overwrite one
    /// with the other.**
    ///
    /// The unit tests above hold `carry_settings_forward` to the rule. This is
    /// the other half: a rule nothing calls is a rule that governs nothing,
    /// and the shape it replaced was
    /// `*result.borrow_mut() = Some(handles.finish());` -- an unconditional
    /// overwrite sitting directly under a comment explaining why the lock's
    /// own result had been preserved.
    ///
    /// The merge is `finish_the_locked_session`, shared for the same reason
    /// the catch is: the startup host ends the same two sessions.
    #[test]
    fn the_tail_merges_the_two_sessions_rather_than_replacing_one() {
        let tail = tail_body();
        assert!(
            tail.contains(concat!(
                "carry_settings_",
                "forward(result.borrow_mut().take(), rebuilt)"
            )),
            "the shared tail does not merge the pre-lock result into the rebuilt one, so a \
             visit to the gear made before a lock is discarded by the session that replaced \
             it -- and `main`, the only writer of `settings.json`, never hears about it: \
             {tail}"
        );
        assert!(
            tail.contains(concat!("vault_handles.borrow().as_ref().map(|handles| handles.", "finish())")),
            "the shared tail does not end the surviving vault session through `finish`, so \
             whichever session is up when the window closes never writes its geometry: {tail}"
        );
        let closure = closure();
        assert_eq!(
            closure
                .matches(concat!("finish_the_locked_", "session(&result, &vault_handles)"))
                .count(),
            1,
            "the vault host does not end its session through the shared tail, exactly once, \
             over its own two cells. A MISSING call leaves a perfectly lifted merge that \
             nothing runs: {closure}"
        );
        // The overwrite shape, banned over the whole production half -- it is
        // what either region would grow back.
        let production = code(production());
        assert!(
            !production.contains(concat!("*result.borrow_mut() = Some(handles.", "finish());")),
            "the tail is back to overwriting the lock's own result with the rebuilt frame's, \
             whose `edited_settings` cell is always fresh: {production}"
        );
        // Positive controls: the host really does contain the tail it calls
        // the merge from, and the lock catch's own preserving write -- the one
        // the merge consumes -- is still there. Without the second, the merge
        // would be reading a cell nothing ever fills.
        assert!(
            closure.contains(concat!("VaultSessionOutcome { result, ", "stages, relocked }")),
            "control: the sliced region stops before the tail it is a claim about: {closure}"
        );
        assert!(
            catch_body()
                .contains(concat!("*self.result.borrow_mut() = Some(handles.", "finish());")),
            "control: the lock catch no longer records the ending session at all, so the merge \
             above has nothing to merge"
        );
        assert!(
            !closure.contains(concat!("carry_settings_", "forward(")),
            "the vault host merges by hand beside the shared tail: {closure}"
        );
    }

    /// **The rebuilt vault is built with the preference edit the pre-lock
    /// session produced.**
    ///
    /// The estate the worker reads `auto_lock` out of is the PRE-EDIT one --
    /// `main` is the only writer of `settings.json` and it does not run until
    /// this window is gone. Handed nothing, the rebuilt window silently
    /// reverts to the old auto-lock policy, which undoes "the open window
    /// honours an auto-lock change at once" across a lock.
    #[test]
    fn the_rebuild_is_handed_the_gear_visit_that_preceded_the_lock() {
        let drain = drain_body();
        assert!(
            drain.contains(concat!("build(", "edited_before_lock)")),
            "the rebuilt vault is built without the preference edit made before the lock, so a \
             window that changed its auto-lock policy and then locked comes back running the \
             OLD policy: {drain}"
        );
        assert!(
            drain.contains(concat!("before.edited_settings.", "clone()")),
            "the value handed to the rebuild is not the pre-lock session's gear visit: {drain}"
        );
        // The cell it is read out of is the one the catch wrote, and it is the
        // SHARED one -- read from a cell of the drain's own the value would
        // always be `None` and this guard would still pass.
        assert!(
            drain.contains(concat!("self.result.borrow().as_ref()", ".and_then")),
            "the gear visit is read from something other than the lock's own result cell, so \
             the rebuild is handed whatever that other cell happens to hold: {drain}"
        );
    }

    /// **Each lifted piece has ONE definition and ONE caller, and the caller
    /// is named.**
    ///
    /// `the_lock_arm_keeps_the_window_instead_of_letting_it_close` and the
    /// three guards beside it each pin their own call site; this is the same
    /// property stated once, over every entry point at once, and it is the one
    /// that bites when a piece grows a SECOND caller. A second caller is not a
    /// second copy, so nothing that counts copies can see it -- and a second
    /// caller of the catch is two teardown workers against one estate, which
    /// is the multiple-owner shape the estate exists to remove.
    ///
    /// **This guard is what forces step 5 through a test edit.** When the
    /// startup host wires the lock in, every count here goes to three, and the
    /// honest edit is to name the second call site as well -- not to raise a
    /// number. That is the shape finding C prescribes and the shape step 1's
    /// `the_lifted_teardown_pieces_have_one_definition_and_one_caller_each`
    /// already uses in `main.rs`.
    #[test]
    fn the_lifted_lock_pieces_have_one_definition_and_one_caller_each() {
        let production = code(production());
        let value = lock_value();
        let closure = closure();
        for (define, call) in [
            (concat!("fn ", "new("), concat!("InWindow", "Lock::new(")),
            (concat!("fn catch_the_", "lock("), concat!("lock.catch_the_", "lock(")),
            (concat!("fn hand_over_the_", "token("), concat!("lock.hand_over_the_", "token(")),
            (concat!("fn answer_the_", "teardown("), concat!("lock.answer_the_", "teardown(")),
            (
                concat!("fn retract_if_the_teardown_never_", "ran("),
                concat!("lock.retract_if_the_teardown_never_", "ran("),
            ),
            (
                concat!("fn finish_the_locked_", "session("),
                concat!("finish_the_locked_", "session(&result"),
            ),
        ] {
            assert_eq!(
                value.matches(define).count(),
                1,
                "{define:?} is not defined exactly once in the shared lock value -- a second \
                 definition is the copy this lift exists to prevent, and none at all is a \
                 touch point that has gone back inside a host: {value}"
            );
            assert_eq!(
                production.matches(call).count(),
                1,
                "{call:?} is called from more than one place in this file, or from none. A \
                 SECOND caller is not a second copy, so nothing that counts copies can see it \
                 -- and a second caller of the catch is two teardown workers against one \
                 session, which is the multiple-owner shape the estate exists to remove: \
                 {production}"
            );
            assert_eq!(
                closure.matches(call).count(),
                1,
                "{call:?} is not called exactly once from the vault host, so the one call \
                 counted above is somewhere else: {closure}"
            );
        }
    }

    /// **The stage is ADVANCED, never assigned.**
    ///
    /// The type does the work: [`LockReach`] lives behind a private field in
    /// a child module, so `self.stage = false` is a type error and
    /// `self.stage.0 = ..` is a privacy error. Exactly one wrong write still
    /// compiles -- re-seeding the field with a fresh stage -- and it has to
    /// name `LockStage::fresh` out loud. This is the whole remaining surface,
    /// and it is banned as a CLASS rather than pinned to a position: the two
    /// positional pins on these flags each caught the shape they were given
    /// and left the class open.
    #[test]
    fn the_lock_reach_is_not_assignable_only_advanceable() {
        let regions = where_a_lock_decision_could_live();
        assert_eq!(
            regions.matches(concat!("LockStage::", "fresh()")).count(),
            1,
            "the lock stage is seeded somewhere other than exactly once. A SECOND seed is a \
             lock talked back to `Nothing` after it started a worker or answered a step, \
             which is the `= false` write this file has had three reviews about, in the one \
             spelling the type system still allows: {regions}"
        );
        let seed = lifted(concat!("fn ", "new("), concat!("fn catch_the_", "lock("));
        assert!(
            seed.contains(concat!("stage: LockStage::", "fresh()")),
            "the one seed is not the constructor own, so the stage is re-seeded somewhere a \
             lock is already under way: {seed}"
        );
        for (region, name) in [
            (catch_body(), "the lock catch"),
            (drain_body(), "the teardown drain"),
            (retract_body(), "the retraction"),
            (closure(), "the vault host"),
        ] {
            // `.stage =` and not `stage =`: both hosts keep a LOCAL called
            // `stage` for the state machine, which is a different thing
            // entirely and is assigned every frame.
            for banned in [concat!("LockStage::", "fresh"), concat!(".stage", " =")] {
                assert!(
                    !region.contains(banned),
                    "{name} contains {banned:?}: the stage is being written rather than \
                     advanced, which is how a worker that started reads as one that did not \
                     -- the retraction never fires and the vault says locked with `bw serve` \
                     still holding the session: {region}"
                );
            }
            assert!(
                !region.contains(concat!("LockReach::", "Nothing")),
                "{name} advances the stage to `Nothing`, which is a write dressed as a step: \
                 {region}"
            );
        }
        // Positive control: the two advances the rule is fed really are in
        // the lifted value, so the bans above are not vacuous.
        for needle in [
            concat!("self.stage.reached(LockReach::", "WorkerStarted);"),
            concat!("self.stage.reached(LockReach::", "StepReported);"),
        ] {
            assert!(
                regions.contains(needle),
                "control: {needle:?} is not in the lock own code, so nothing ever records \
                 the fact the retraction is decided from"
            );
        }
    }
}

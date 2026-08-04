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

/// How long the working stage may go on before it ends itself.
///
/// **Derived from what the code already budgets, not chosen.** The worker runs
/// `StartupWork::produce`, which is three phases and only the middle one is
/// bounded by anything:
///
///   1. `try_start_backend` -> `bw_serve::run_bw_sync`, a bare
///      `Command::output()` with no timeout. `bw_serve::BACKEND_OP_TIMEOUT` is
///      the number this crate already uses everywhere else as the upper bound
///      on a legitimate backend start or sync (`main`'s wedge check, the
///      picker's probe), so it is what one such phase costs here.
///   2. `wait_for_vault_ready` on `readiness_schedule(READINESS_DEADLINE)` --
///      the only self-limiting part, and it runs AFTER the unbounded phase
///      rather than over it.
///   3. `login_ui::check_bw_status_details()`, unconditional and on the failure
///      path too: a second untimed `bw` spawn, so a second
///      `BACKEND_OP_TIMEOUT`.
///
/// Hence `2 * BACKEND_OP_TIMEOUT + READINESS_DEADLINE` = 210s. Deliberately
/// generous: this is a watchdog on a stage the user cannot leave by any other
/// route, and a false timeout on a slow machine throws away a healthy sign-in,
/// while a slow one only makes a wait the user is already watching a spinner
/// through longer. It bounds the WINDOW, not the subprocess -- `produce` is
/// still untimed and its worker may still be running when this fires.
pub const WORKING_DEADLINE: Duration = Duration::from_secs(
    2 * crate::bw_serve::BACKEND_OP_TIMEOUT.as_secs()
        + crate::bw_serve::READINESS_DEADLINE.as_secs(),
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
    let mut styled = false;
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
    let mut closing = false;

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame, not
            // the one that calls set_fonts -- drawing Archivo-styled text in
            // this same frame would look up a family that doesn't exist yet and
            // panic. Skip drawing this frame; the real UI starts on the next
            // one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            login_ui::round_window_corners(WINDOW_TITLE);
            // The OS window exists by this first painted frame (the same hook
            // `round_window_corners` uses), and this is where it is brought to
            // the front. Done ONCE, here, for all three stages -- which is why
            // both sub-frames are built `pre_styled`: a vault frame raising the
            // window again would yank forward a window the user may have
            // deliberately sent behind something while `bw serve` started.
            let _ = foreground::raise_window(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

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
                // **This stage cannot be closed, and its ✕ says so.** It owns
                // the only handle to a `bw serve` that is starting up, and that
                // handle is inside the worker's answer -- closing here would
                // strand the process holding the port, so the recovery `main`
                // would then run could not bind it. The spinner now wears the
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

                if !closing && ui.ctx().input(|i| i.viewport().close_requested()) {
                    log::info!(
                        "the single window was asked to close while the vault backend was \
                         still starting; refusing, so the backend it is holding is not \
                         orphaned on the port the recovery needs"
                    );
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }

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
                                        "single window: vault ready {:?} after sign-in was \
                                         accepted",
                                        at.elapsed()
                                    );
                                }
                                stage = next;
                            }
                            Next::Close => {
                                log::warn!(
                                    "the single window has no vault to show; closing so the \
                                     startup recovery can run"
                                );
                                closing = true;
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        ui.ctx().request_repaint();
                    }
                    // **Not `Err(_)`.** The decision is `poll_working`'s, whole
                    // -- a `Disconnected` means the worker is gone and nothing
                    // will ever arrive, and an `Empty` past the deadline means
                    // it is alive and not coming back either. Both land on
                    // `Event::WorkFailed`, which `advance` sends to
                    // `Next::Close`: the window ends and `main`'s
                    // `recover_from_failed_vault_wait` takes over, which is a
                    // fresh login the user can close. That is the point of the
                    // fix -- not a fourth stage that apologises with the same
                    // disabled ✕.
                    Err(err) => {
                        let elapsed = working_since.map_or(Duration::ZERO, |at| at.elapsed());
                        match poll_working(err, elapsed) {
                            WorkPoll::KeepWaiting => {
                                ui.ctx().request_repaint_after(WORKING_POLL)
                            }
                            WorkPoll::Failed(why) => {
                                log::error!("{} (after {elapsed:?})", why.reason());
                                if let Next::Close = advance(stage, Event::WorkFailed) {
                                    closing = true;
                                    ui.ctx()
                                        .send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                                ui.ctx().request_repaint();
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

    /// The pairs that cannot happen are no-ops, not moves. Without this, a
    /// stray `WorkReady` arriving while the card is up -- a worker from an
    /// abandoned attempt, say -- would jump straight to a vault stage whose
    /// frame was never built, and the `Vault` arm draws nothing at all in that
    /// case: a blank window.
    #[test]
    fn an_event_that_does_not_belong_to_the_current_stage_moves_nothing() {
        for (stage, event) in [
            (Stage::SignIn, Event::WorkReady),
            (Stage::SignIn, Event::WorkFailed),
            (Stage::Working, Event::SignedIn),
            (Stage::Vault, Event::SignedIn),
            (Stage::Vault, Event::WorkReady),
            (Stage::Vault, Event::WorkFailed),
        ] {
            assert_eq!(
                advance(stage, event),
                Next::Show(stage),
                "{event:?} moved the window away from {stage:?}"
            );
        }
        // Positive control on the same comparison: it can tell a move from a
        // stay, so the six assertions above are not all trivially true.
        assert_ne!(
            advance(Stage::SignIn, Event::SignedIn),
            Next::Show(Stage::SignIn)
        );
    }
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

    /// The deadline is derived from the two numbers the crate already agrees
    /// on, so a change to either moves it rather than leaving it stale.
    #[test]
    fn the_deadline_covers_every_phase_the_worker_runs() {
        use crate::bw_serve::{BACKEND_OP_TIMEOUT, READINESS_DEADLINE};
        assert_eq!(
            WORKING_DEADLINE,
            BACKEND_OP_TIMEOUT + READINESS_DEADLINE + BACKEND_OP_TIMEOUT,
            "the working stage's deadline is no longer the sum of the three phases \
             `StartupWork::produce` runs -- an untimed backend start, the readiness probe, and \
             an untimed `bw status` -- so a healthy-but-slow startup can be cut off"
        );
        assert!(
            WORKING_DEADLINE > BACKEND_OP_TIMEOUT,
            "the window gives up before a single backend operation is even considered wedged"
        );
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
    fn production() -> &'static str {
        let source = source();
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file");
        &source[..end]
    }

    /// The frame closure: from its head to the end of production code.
    fn closure() -> &'static str {
        let production = production();
        let at = production
            .find(concat!("run_ui_", "native(WINDOW_TITLE, options, move |ui, frame|"))
            .expect(
                "no frame closure in this file -- if `run` stopped opening a window, the \
                 single-window startup is gone entirely",
            );
        &production[at..]
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

    /// Everything on a line before a `//`.
    ///
    /// Load-bearing for the guard below, not tidiness: the comment above the
    /// spinner call names `CloseControl::Disabled` out loud, so a guard that
    /// matched the raw source would go on passing after the argument itself was
    /// changed. This crate has already shipped exactly that mistake once (the
    /// icon guard that matched the comment naming the thing it was looking for).
    fn code(source: &str) -> String {
        source
            .lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The `Stage::Working` arm alone, comments stripped. Bounded forward to the
    /// next arm rather than by a byte count, so it cannot overrun into the
    /// vault's own chrome call -- which passes `CloseControl::Active` and would
    /// satisfy a careless search.
    fn working_arm() -> String {
        let closure = closure();
        let start = closure
            .find(concat!("Stage::Working ", "=>"))
            .expect("the closure has no working stage at all");
        let rest = &closure[start..];
        let end = rest
            .find(concat!("Stage::Vault ", "=>"))
            .expect("the working arm is not followed by the vault arm");
        code(&rest[..end])
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
            arm.contains(concat!("Cancel", "Close")),
            "the working stage no longer refuses a close it did not draw the affordance for \
             (Alt+F4, the system menu), so a `bw serve` still starting up is stranded on the \
             port the recovery needs"
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

    /// The refusal must not outlive the decision to stop. eframe reports this
    /// module's own `ViewportCommand::Close` back as a `close_requested` on a
    /// later frame, while `Stage::Working` is still the stage being drawn -- so
    /// an unconditional `CancelClose` cancels the window's own exit and the
    /// stage is exactly as unleaveable as it was before any of this.
    #[test]
    fn the_refusal_stands_down_once_the_stage_has_decided_to_close() {
        let arm = working_arm();
        assert!(
            arm.contains(concat!("!closing && ", "ui.ctx().input(")),
            "the working stage refuses EVERY close, including the one it sends itself when it \
             gives up -- so `WorkFailed` never actually closes the window: {arm}"
        );
        assert_eq!(
            arm.matches("closing = true;").count(),
            2,
            "one of the two ways the stage ends does not disarm the refusal first, so that one \
             cancels its own close and hangs: {arm}"
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
}

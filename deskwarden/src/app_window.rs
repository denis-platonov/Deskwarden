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
    /// The background work finished and there is not. The window ends and
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

    let (work_tx, work_rx) = mpsc::channel::<P>();
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
                        let work_tx = work_tx.clone();
                        std::thread::spawn(move || {
                            let _ = work_tx.send(prepare(produced));
                        });
                    }
                    if let Next::Show(next) = advance(stage, Event::SignedIn) {
                        stage = next;
                    }
                    ui.ctx().request_repaint();
                }
            }
            Stage::Working => {
                loading_ui::draw_spinner_body(ui, working_message);

                // **This stage cannot be closed.** It owns the only handle to a
                // `bw serve` that is starting up, and that handle is inside the
                // worker's answer -- closing here would strand the process
                // holding the port, so the recovery `main` would then run could
                // not bind it. There is no affordance to close with (the
                // spinner paints no chrome), so this only catches Alt+F4, and
                // it is bounded: the readiness probe gives up after its own
                // deadline and this stage ends itself with `WorkFailed`.
                if ui.ctx().input(|i| i.viewport().close_requested()) {
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
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                        ui.ctx().request_repaint();
                    }
                    Err(_) => ui.ctx().request_repaint_after(WORKING_POLL),
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
                    "loading_ui::draw_spinner_body(ui, working_message);",
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

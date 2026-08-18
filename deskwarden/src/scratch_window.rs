//! **The window a rehearsal types into, and the run itself.**
//!
//! [`crate::vault_window::rehearsal`] had the whole guarantee and no way to
//! reach it: `substitute` was total, `rehearsal_plan` re-chunked through the
//! ordinary compiler, `scratch_target` named the window to look for -- and
//! nothing opened that window. This file is the missing half.
//!
//! # Why this is an `egui` viewport, and not the Win32 window it used to be
//!
//! A rehearsal is started from *inside* a running event loop: the button lives
//! in the sequence editor, which is painted inside `vault_window::run`'s
//! `eframe::run_ui_native` closure. `winit` refuses to build a second event
//! loop while that one is alive, so a second `eframe::run_*native` -- the way
//! [`crate::preflight_host`] opens its window, from `main`'s dispatch loop
//! between windows -- was not available. The first version of this file
//! answered that with `CreateWindowExW` and system `EDIT` controls, and it
//! worked: it typed the samples and reported what arrived. It also had none of
//! the app's theme, tokens or type, and it was the only surface in the product
//! drawn that way. Design 4d asks for a styled panel with a transcript; what
//! shipped was an unstyled system dialog.
//!
//! [`egui::Context::show_viewport_deferred`] is the door that was not used. It
//! opens a second **real OS window inside the already-running event loop**, on
//! the same thread, painted by egui -- so [`crate::theme`] applies, the design
//! tokens apply, and no second event loop is created. That is what this file
//! is now.
//!
//! # The consequences, which are not small
//!
//! * **The window cannot be blocking.** `show_scratch(&Plan) -> Rehearsed` was
//!   a function that opened a window, pumped it, and came back with an answer.
//!   A viewport is painted by the loop it was asked for from, so a call that
//!   blocked would stop the loop that has to paint it. The rehearsal is
//!   therefore a small state machine -- [`Stage`] -- advanced once per frame by
//!   [`Rehearsal::show`], and [`rehearsal_notice`] now *starts* one rather than
//!   running one to completion. Its one caller's line is unchanged, and the
//!   source guard on that line in `vault_window::mod` is unchanged with it.
//! * **`foreground`'s classification changes.** `RAISING_SITES` finds a window
//!   by grepping for `run_ui_native(TITLE,` and
//!   `OPENS_A_WIN32_WINDOW_AND_RAISES_IT` by grepping for `CreateWindowExW(`.
//!   A viewport is neither. See `foreground`'s
//!   `OPENS_A_VIEWPORT_AND_RAISES_IT`, which is the table this module now sits
//!   in, and `only_one_window_of_this_process_can_exist_at_a_time`, whose
//!   `show_viewport` count is no longer zero for every window module.
//! * **Title uniqueness is still the whole safety story.** This is the one
//!   window in this crate deliberately alive alongside another one, and
//!   [`rehearsal::scratch_target`] finds it by title. The viewport is built
//!   `with_title(SCRATCH_TITLE)` for exactly that reason, and
//!   `foreground::only_one_window_of_this_process_can_exist_at_a_time` still
//!   asserts that constant is distinct from the literal the other five windows
//!   share.
//!
//! # The frames have to keep coming while the sender types
//!
//! `SendInput` posts to the target thread's message queue, and a queue nobody
//! is pumping delivers the whole burst at the end -- which is precisely the
//! timing the user opened this window to watch. The Win32 version pumped its
//! own loop. Under a viewport the `eframe` loop is the pump, but egui only
//! repaints on input by default and a `SendInput` burst arriving into another
//! thread's queue produces no egui input of its own between keystrokes. So
//! [`Rehearsal::show`] calls `request_repaint` on **every** frame a rehearsal
//! is in flight; `frames_keep_coming_while_a_rehearsal_is_in_flight_and_stop_when_it_is_over`
//! asserts that against a real [`egui::Context`], and asserts the other half
//! too -- a finished run asks for nothing -- so the claim is about the run's
//! state and not about an unconditional call.
//!
//! The typing itself is on a worker thread -- [`Injector::fill_sequence`] ends
//! in `RealSendInput::fill_sequence`, which spawns -- so the UI thread is never
//! inside the burst at all.
//!
//! # What is testable here
//!
//! Three things, and none of them needs a window:
//!
//! * [`RehearsalSeams::begin`] -- the routing, unchanged from the Win32
//!   version, driven through `fn` pointers with a recorder;
//! * [`Rehearsal`]'s state machine -- driven with a fake sender and a fake
//!   target, so opening, typing, finishing and every refusal are reachable;
//! * [`draw`] -- a pure function of a [`RehearsalView`] and the arrived text,
//!   which is what `cargo run --example ui_preview -- --all` renders as its
//!   twelfth surface. The screenshot job that never saw this surface is how it
//!   shipped looking wrong the first time.

use crate::injector::sequence::{self, Plan};
use crate::injector::{Injector, OutcomeSink, RealSendInput, RealUiAutomation};
use crate::theme;
use crate::vault_window::rehearsal::{self, Arrival, SCRATCH_TITLE};
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// The decision: what is sent, and where
// ---------------------------------------------------------------------------

/// What a rehearsal did not do, and why. Every variant names the thing the user
/// has to go and change, the phrasing rule [`sequence::Refusal::message`]
/// follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotRehearsed {
    /// The substituted sequence would not plan -- empty, or over
    /// [`sequence::MAX_SEQUENCE`]. Carries the compiler's own sentence rather
    /// than a second wording of it.
    Refused(String),
    /// The scratch window could not be found. **There is no fallback to the
    /// foreground**: see [`rehearsal::scratch_target`].
    NoScratchWindow,
    /// The sender declined -- something else is already typing, or the window
    /// would not take the foreground.
    NotSent(String),
}

impl NotRehearsed {
    pub fn message(&self) -> String {
        match self {
            Self::Refused(why) | Self::NotSent(why) => why.clone(),
            Self::NoScratchWindow => {
                "the rehearsal scratch window did not open, so there is nowhere safe to type"
                    .to_string()
            }
        }
    }
}

/// The three outside things a rehearsal touches, behind `fn` pointers.
///
/// A seam per side, for the reason
/// [`crate::vault_window::preflight::SendGate`] gives at length: a decision
/// pinned only as a pure function cannot be seen to be *in the position that
/// decides*. Here the position that matters is the one between the real plan
/// and the sender, and the tests drive [`Self::begin`] end to end and ask what
/// the sender was actually handed.
pub struct RehearsalSeams {
    /// [`rehearsal::rehearsal_plan`] in production. **The only door**: it takes
    /// a `&Plan` and returns a fresh one whose every text payload came from
    /// [`rehearsal::sample_for`], every arm of which returns a `const`, so
    /// there is no way through this struct that carries a real value.
    plan_for: fn(&Plan) -> Result<Plan, sequence::Refusal>,
    /// [`rehearsal::scratch_target`] in production.
    target: fn() -> Option<isize>,
    /// [`send_through_injector`] in production: the ordinary
    /// [`Injector::fill_sequence`] path, with the real sender, the real
    /// chunking and the real waits.
    send: fn(isize, Plan, OutcomeSink) -> Result<(), String>,
}

impl RehearsalSeams {
    pub fn production() -> Self {
        Self {
            plan_for: rehearsal::rehearsal_plan,
            target: rehearsal::scratch_target,
            send: send_through_injector,
        }
    }

    /// Substitutes, finds the scratch window, and starts the run.
    ///
    /// Answers the transcript of what was **actually handed to the sender** --
    /// read off the substituted, re-chunked plan rather than off the real one,
    /// so the list the user is shown cannot describe something the sender was
    /// never given.
    ///
    /// `done` is called on the typing thread when the run ends. Nothing here
    /// waits for it: the caller has a frame loop to keep running.
    pub fn begin(&self, real: &Plan, done: OutcomeSink) -> Result<Vec<Arrival>, NotRehearsed> {
        // **First, and unconditionally.** The substitution happens before the
        // window is looked for and before anything is sent, so there is no
        // ordering of these three in which the real plan reaches `send`.
        let sent = (self.plan_for)(real).map_err(|r| NotRehearsed::Refused(r.message()))?;
        let Some(hwnd) = (self.target)() else {
            return Err(NotRehearsed::NoScratchWindow);
        };
        let transcript = rehearsal::transcript(&sent);
        (self.send)(hwnd, sent, done).map_err(NotRehearsed::NotSent)?;
        Ok(transcript)
    }
}

/// The ordinary fill path, with no rehearsal-specific sender anywhere in it.
///
/// [`Injector::fill_sequence`] takes the process-wide permission to type, so a
/// rehearsal contends for the keyboard with a real fill exactly as a second
/// real fill would -- which is right: both are `SendInput`. It also **spawns**,
/// which is what keeps the burst off the UI thread and the frames coming; see
/// this module's header.
fn send_through_injector(hwnd: isize, plan: Plan, done: OutcomeSink) -> Result<(), String> {
    Injector { ui: RealUiAutomation, fallback: RealSendInput }.fill_sequence(hwnd, plan, done)
}

// ---------------------------------------------------------------------------
// The window's shape
// ---------------------------------------------------------------------------

/// Design 4d's card is 470 wide; the window is that, because the card *is* the
/// window here.
pub const SCRATCH_WIDTH: f32 = 470.0;
pub const SCRATCH_HEIGHT: f32 = 400.0;

/// The viewport this window is. A named `ViewportId` derived from the title
/// rather than a fresh one per rehearsal: a second id would be a second OS
/// window, and the whole title-uniqueness argument in `foreground` is about
/// there being exactly one.
fn scratch_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of(SCRATCH_TITLE)
}

/// How long the window waits for a rehearsal before it stops claiming to be
/// watching one.
///
/// [`sequence::MAX_SEQUENCE`] plus a margin for the sender's own foreground
/// settle. Timing out abandons nothing: the typing thread owns the plan and
/// wipes it on the way out, whatever this window does.
pub const REHEARSAL_PATIENCE: Duration = Duration::from_secs(sequence::MAX_SEQUENCE.as_secs() + 10);

/// How long the window waits for the OS to hand it a `HWND` under
/// [`SCRATCH_TITLE`] before giving up and saying so.
///
/// A viewport's OS window exists by the time egui runs its callback, so in
/// practice this is one or two frames. It is a duration and not a frame count
/// because a frame is not a unit of time.
pub const OPENING_PATIENCE: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// The state machine
// ---------------------------------------------------------------------------

/// Where a rehearsal has got to. Advanced once per frame by
/// [`Rehearsal::advance`], which takes both of its outside answers as seams --
/// which is what makes every arm reachable from a test with no window anywhere.
#[derive(Debug)]
enum Stage {
    /// The viewport has been asked for; its OS window is not findable yet.
    Opening { since: Instant },
    /// The sender took the plan and is typing.
    Typing { started: Instant, finished: Arc<AtomicBool> },
    /// It ended -- or it never started, and `failure` says why.
    Over { elapsed: Duration },
}

/// Everything a live rehearsal holds, behind the `Arc<Mutex<_>>` that
/// `show_viewport_deferred` requires.
///
/// The callback handed to a deferred viewport is `Fn + Send + Sync + 'static`:
/// it is stored by egui and invoked when the child viewport's own frame runs,
/// which is *after* the parent's closure has returned. So it cannot borrow, and
/// the state has to be shared rather than owned. Nothing here is touched off
/// the UI thread -- the mutex is what the signature demands, not a claim about
/// concurrency.
#[derive(Debug)]
struct Inner {
    /// The substituted plan, taken by the frame that starts the run. `None`
    /// afterwards, which is also what makes starting twice impossible.
    plan: Option<Plan>,
    stage: Stage,
    /// **What really landed**, as the text of the panel the keys are typed
    /// into. Never a secret: the only thing typed was a sample.
    arrived: String,
    /// The acts the sender was handed. Empty until the run starts.
    sent: Vec<Arrival>,
    failure: Option<NotRehearsed>,
    /// Whether `raise_window` has been asked for yet. Once, on the frame the
    /// OS window first exists -- the same hook every window in this crate
    /// raises from.
    raised: bool,
    /// Cleared when the user closes the window, which is what takes the
    /// viewport away.
    open: bool,
}

/// A rehearsal on screen.
///
/// Cheap to clone (it is one `Arc`), which is what lets the deferred viewport's
/// callback hold one.
#[derive(Clone)]
pub struct Rehearsal {
    inner: Arc<Mutex<Inner>>,
}

/// The state, with a poisoned lock treated as an ordinary one: every field
/// behind it is UI state, and refusing to draw the window because a previous
/// panic touched it would turn a cosmetic problem into a stuck window.
fn locked(inner: &Arc<Mutex<Inner>>) -> std::sync::MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Rehearsal {
    /// A rehearsal that has been asked for and whose window does not exist yet.
    fn opening(plan: Plan) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                plan: Some(plan),
                stage: Stage::Opening { since: Instant::now() },
                arrived: String::new(),
                sent: Vec::new(),
                failure: None,
                raised: false,
                open: true,
            })),
        }
    }

    /// Whether the window is still up.
    fn is_open(&self) -> bool {
        locked(&self.inner).open
    }

    /// **One frame of the rehearsal.**
    ///
    /// The `seams` and `now` arguments are what make this reachable: production
    /// hands [`RehearsalSeams::production`] and [`Instant::now`], and the tests
    /// hand a recorder and a clock they control.
    fn advance(&self, seams: &RehearsalSeams, now: Instant) {
        let mut held = locked(&self.inner);
        match held.stage {
            Stage::Opening { since } => {
                // The window is looked for by TITLE, through the same
                // `own_window_titled` a real fill's target goes through -- so
                // the viewport being a real OS window is asked about here
                // rather than assumed.
                if (seams.target)().is_some() {
                    let Some(plan) = held.plan.take() else {
                        // Unreachable: `plan` is `Some` for exactly as long as
                        // the stage is `Opening`. Said out loud rather than
                        // unwrapped, because a rehearsal that silently did
                        // nothing is indistinguishable from a button that is
                        // not wired up.
                        held.failure = Some(NotRehearsed::NoScratchWindow);
                        held.stage = Stage::Over { elapsed: now.saturating_duration_since(since) };
                        return;
                    };
                    let finished = Arc::new(AtomicBool::new(false));
                    let signal = finished.clone();
                    let begun = seams.begin(
                        &plan,
                        Box::new(move |_outcome| signal.store(true, Ordering::SeqCst)),
                    );
                    match begun {
                        Ok(sent) => {
                            held.sent = sent;
                            held.stage = Stage::Typing { started: now, finished };
                        }
                        Err(why) => {
                            held.failure = Some(why);
                            held.stage = Stage::Over { elapsed: Duration::ZERO };
                        }
                    }
                } else if now.saturating_duration_since(since) >= OPENING_PATIENCE {
                    held.failure = Some(NotRehearsed::NoScratchWindow);
                    held.stage = Stage::Over { elapsed: Duration::ZERO };
                }
            }
            Stage::Typing { started, ref finished } => {
                let elapsed = now.saturating_duration_since(started);
                if finished.load(Ordering::SeqCst) || elapsed >= REHEARSAL_PATIENCE {
                    held.stage = Stage::Over { elapsed };
                }
            }
            // Left open on purpose: the transcript is the whole point, and a
            // window that vanished the instant the last key landed would show
            // it for one frame. The user closes it.
            Stage::Over { .. } => {}
        }
    }

    /// Whether a frame is still worth asking for -- i.e. whether anything can
    /// change without the user doing something.
    ///
    /// The half of the pump argument this file can hold: see the module header.
    fn in_flight(&self) -> bool {
        matches!(locked(&self.inner).stage, Stage::Opening { .. } | Stage::Typing { .. })
    }

    /// What the window should draw right now.
    fn view(&self) -> RehearsalView {
        let held = locked(&self.inner);
        let (headline, finished) = match held.stage {
            Stage::Opening { .. } | Stage::Typing { .. } => {
                (rehearsal::WAITING_NOTE.to_string(), false)
            }
            Stage::Over { elapsed } => (rehearsal::finished_line(elapsed, held.sent.len()), true),
        };
        RehearsalView {
            headline,
            finished,
            sent: held.sent.clone(),
            failure: held.failure.as_ref().map(NotRehearsed::message),
        }
    }

    /// **The viewport.** Called once per frame from the window that started the
    /// rehearsal; answers `false` when there is nothing left to show.
    ///
    /// `ctx` is the *parent's* context. `show_viewport_deferred` is what opens
    /// the second OS window inside the loop that context belongs to -- see this
    /// module's header for why that is the whole point of this file.
    fn show(&self, ctx: &egui::Context) -> bool {
        if !self.is_open() {
            return false;
        }
        self.advance(&RehearsalSeams::production(), Instant::now());
        // **Every frame, while anything can still change.** A `SendInput` burst
        // arriving into this window's queue produces no egui input of its own
        // between keystrokes, and egui repaints on input by default -- so
        // without this the frames stop and the burst is delivered in a lump at
        // the end, which is exactly what a rehearsal exists to prevent. Both
        // viewports: the parent is what runs the pass at all, and the child is
        // what has to be repainted by it.
        if self.in_flight() {
            ctx.request_repaint();
            ctx.request_repaint_of(scratch_viewport());
        }

        let mine = self.clone();
        ctx.show_viewport_deferred(
            scratch_viewport(),
            // `with_title(SCRATCH_TITLE)` is load-bearing and not decoration:
            // it is how `rehearsal::scratch_target` finds this window at all,
            // and the uniqueness of that string is the whole reason this window
            // may be alive alongside the vault window. See
            // `foreground::only_one_window_of_this_process_can_exist_at_a_time`.
            egui::ViewportBuilder::default()
                .with_title(SCRATCH_TITLE)
                .with_inner_size([SCRATCH_WIDTH, SCRATCH_HEIGHT])
                .with_resizable(false)
                .with_icon(theme::window_icon()),
            // **The callback is handed the viewport's root `Ui`**, not a
            // context: `show_viewport_deferred` builds the child viewport's
            // frame and passes the `Ui` it opened. That is also why nothing
            // here needs a second `theme::apply` -- it is the same
            // `egui::Context` the vault window styled, which is the whole
            // reason this window looks like the rest of the app.
            move |root, _class| {
                // The OS window exists by here -- the same hook every window in
                // this crate raises from. See `foreground`: a refusal from
                // Windows flashes the taskbar button rather than being ignored.
                // Once, on the first frame, because a raise on every frame
                // would fight the user for the foreground for as long as the
                // window is up.
                let first_frame = {
                    let mut held = locked(&mine.inner);
                    let first = !held.raised;
                    held.raised = true;
                    first
                };
                if first_frame {
                    crate::foreground::raise_window(SCRATCH_TITLE);
                }
                let view = mine.view();
                let mut closed = root.input(|i| i.viewport().close_requested());
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme::WINDOW_BG))
                    .show(root, |ui| {
                        let mut held = locked(&mine.inner);
                        closed |= draw(ui, &view, &mut held.arrived);
                    });
                if closed {
                    locked(&mine.inner).open = false;
                }
            },
        );
        self.is_open()
    }
}

// ---------------------------------------------------------------------------
// The handoff: one rehearsal on screen, started mid-frame, drawn every frame
// ---------------------------------------------------------------------------

thread_local! {
    /// The rehearsal currently on screen, if any.
    ///
    /// **Why a slot and not a parameter.** The two ends of a rehearsal are on
    /// opposite sides of a very long function: [`rehearsal_notice`] is called
    /// from the sequence editor's `Rehearse` arm, deep inside
    /// `vault_window::run`'s frame closure and with no `egui::Context` in
    /// scope; [`show_open_rehearsal`] is called once per frame at the top of
    /// that same closure. Threading a `&mut Option<Rehearsal>` between them
    /// means an argument through every editor action, and returning it instead
    /// changes the one line `vault_window`'s
    /// `rehearse_opens_the_scratch_window_and_its_refusal_reaches_the_user`
    /// guards.
    ///
    /// A `thread_local` and not a `static`, because there is nothing
    /// concurrent here to protect: every `eframe::run_*native` in this crate is
    /// on the main thread, which `foreground` holds by counting the one
    /// `winit` builder call that would allow otherwise (it cannot be named
    /// here -- that count is taken over this file, and a window module that
    /// spells the needle fails it). So both ends are the same thread by
    /// construction, and a second thread getting its own empty slot is the
    /// correct behaviour rather than a race.
    static ON_SCREEN: RefCell<Option<Rehearsal>> = const { RefCell::new(None) };
}

/// **4d, from the editor's side.** Starts a rehearsal of `sequence` and answers
/// what to tell the user, or `None` when there is nothing to say.
///
/// The one line the sequence editor's Rehearse arm contains. **Its shape is
/// deliberately unchanged** across the move from a Win32 window to a viewport:
/// a sequence the compiler refuses is still reported here and now, because that
/// answer is known before any window is asked for. Everything a rehearsal can
/// only discover later -- no scratch window, a sender that declined -- is
/// reported *in the rehearsal window*, which is where the user is looking.
///
/// **`sequence`, and nothing else.** No item, no password, no one-time code:
/// [`rehearsal::sample_plan`] resolves every field to a fixed sample, so there
/// is no argument here that could carry a secret in the first place.
pub fn rehearsal_notice(sequence: &str) -> Option<String> {
    rehearsal_notice_with(sequence, put_on_screen)
}

/// [`rehearsal_notice`] with the handoff injected, so the two things it decides
/// -- which refusal is reported here, and that an accepted sequence reports
/// nothing -- are reachable without touching the thread-local slot.
fn rehearsal_notice_with(sequence: &str, open: fn(Plan) -> Option<String>) -> Option<String> {
    match rehearsal::sample_plan(sequence) {
        // The compiler's own sentence, not a second wording of it: a sequence
        // over `MAX_SEQUENCE` says so in the words the fill would have used.
        Err(refusal) => Some(refusal.message()),
        Ok(plan) => open(plan),
    }
}

/// Hands a started rehearsal to the frame loop. Answers `None`: the window is
/// not open yet, so there is nothing to report about it.
fn put_on_screen(plan: Plan) -> Option<String> {
    ON_SCREEN.with(|slot| *slot.borrow_mut() = Some(Rehearsal::opening(plan)));
    None
}

/// **The other half of the editor's one line**, called once per frame by the
/// window that hosts the sequence editor.
///
/// A deferred viewport exists for exactly as long as something keeps asking for
/// it, so this is not a nicety: stop calling it and the window closes.
pub fn show_open_rehearsal(ctx: &egui::Context) {
    // The clone is taken out of the slot before `show` runs, so no borrow of
    // the slot is alive while egui is being called -- true by construction
    // rather than by reading `egui`.
    let showing = ON_SCREEN.with(|slot| slot.borrow().clone());
    if let Some(rehearsal) = showing {
        if !rehearsal.show(ctx) {
            ON_SCREEN.with(|slot| *slot.borrow_mut() = None);
        }
    }
}

// ---------------------------------------------------------------------------
// The surface (design 4d)
// ---------------------------------------------------------------------------

/// Design 4d's own check-mark green. A one-off, and named as one: 4e says the
/// product is blue, with red for secrets and amber for caution and nothing
/// else. This colour appears in the design exactly once, on this card, and it
/// appears in this crate exactly once, here.
const CHECK_GREEN: Color32 = Color32::from_rgb(0x1b, 0x7a, 0x3f);

/// The design's near-white on the dark transcript panel.
const PANEL_TEXT: Color32 = Color32::from_rgb(0xf7, 0xf6, 0xf5);

/// 4d's refusal band, which is 4e's one red and appears nowhere else here.
const BAND_FILL: Color32 = Color32::from_rgb(0xfd, 0xf3, 0xf2);
const BAND_EDGE: Color32 = Color32::from_rgb(0xe8, 0xa9, 0xa2);

/// **Everything the rehearsal surface draws, as data.**
///
/// A value and not a `&Rehearsal`, so [`draw`] can be rendered from
/// `examples/ui_preview.rs` -- which has no rehearsal, no window and no sender
/// -- against a fixture, and what that PNG shows is what the app shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RehearsalView {
    /// The line across the top of the card: 4d's `Rehearsal finished · 2.1 s`,
    /// or [`rehearsal::WAITING_NOTE`] while it is still running.
    pub headline: String,
    /// Whether the run is over -- the check mark, and the glyph transcript in
    /// place of the live typing panel.
    pub finished: bool,
    /// The acts the sender was handed, for 4d's lower list.
    pub sent: Vec<Arrival>,
    /// A refusal to report, drawn as the design's red band. `None` is the
    /// ordinary case.
    pub failure: Option<String>,
}

/// **The arrived transcript, with the two invisible keys in the design's
/// blue.**
///
/// 4d draws the Tab and Enter marks in `#7fa4ef` and everything else in
/// near-white, which is not decoration: the whole question a rehearsal answers
/// is whether those two keys arrived, and a mark the same colour as the text
/// around it is a mark the eye does not find.
///
/// A [`egui::text::LayoutJob`] because `RichText` colours a whole string, and
/// the glyphs are interleaved with the samples. Runs are batched rather than
/// appended per character, so the job holds one section per colour change.
///
/// The characters themselves come from [`rehearsal::arrived_panel`], which is
/// where the decision to draw them at all lives and where it is tested.
pub fn arrived_job(arrived: &str, font: egui::FontId) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let drawn = rehearsal::arrived_panel(arrived);
    let is_key = |c: char| c == rehearsal::ARRIVED_TAB || c == rehearsal::ARRIVED_ENTER;
    let mut run = String::new();
    let mut run_is_key = false;
    let flush = |job: &mut egui::text::LayoutJob, run: &mut String, key: bool| {
        if run.is_empty() {
            return;
        }
        job.append(
            run,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: if key { theme::BLUE_SOFT } else { PANEL_TEXT },
                ..Default::default()
            },
        );
        run.clear();
    };
    for ch in drawn.chars() {
        if is_key(ch) != run_is_key {
            flush(&mut job, &mut run, run_is_key);
            run_is_key = is_key(ch);
        }
        run.push(ch);
    }
    flush(&mut job, &mut run, run_is_key);
    job
}

/// One act, as the two strings the design's label/value rows are made of.
///
/// A function rather than four lines inside [`draw`], because a sentence
/// composed inside a frame closure is a sentence no test can read back -- the
/// shape this crate keeps getting caught by.
pub fn act_row(act: &Arrival) -> (String, String) {
    match act {
        Arrival::Typed(text) => ("Typed".to_string(), text.clone()),
        Arrival::Pressed(key) => ("Pressed".to_string(), key.clone()),
        Arrival::Paused(d) => ("Paused".to_string(), rehearsal::elapsed_label(*d)),
    }
}

/// The heading over the list of acts the sender was handed, in the same tracked
/// uppercase mono 4d gives [`rehearsal::ARRIVED_HEADING`].
pub const SENT_HEADING: &str = "WHAT WAS SENT";

/// The width of the label column in the acts list, from 4d's own rows.
const ACT_LABEL_WIDTH: f32 = 96.0;

/// **Design 4d, drawn.** Answers `true` on the frame the user asks to close.
///
/// `arrived` is the text of the dark panel, and while the run is live that
/// panel is a real focused text field -- it is what `SendInput` types into.
/// The two jobs are the same widget on purpose: the box the user watches and
/// the box the keys land in cannot be allowed to differ.
pub fn draw(ui: &mut egui::Ui, view: &RehearsalView, arrived: &mut String) -> bool {
    theme::paint_window_background(ui);
    let mut close = false;
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            // ---- the header rule -------------------------------------------
            egui::Frame::new().inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    check_mark(ui, view.finished);
                    ui.add_space(9.0);
                    ui.label(theme::bold(view.headline.clone(), 14.0).color(theme::INK));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("scratch window").size(11.0).color(theme::TEXT_GHOST),
                        );
                    });
                });
            });
            theme::hairline(ui);

            // ---- WHAT ARRIVED, on the design's dark panel -------------------
            egui::Frame::new()
                .fill(theme::INK)
                .inner_margin(Margin::symmetric(16, 14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(theme::letterspaced_mono(
                        rehearsal::ARRIVED_HEADING,
                        11.0,
                        0.66,
                        theme::TEXT_GHOST,
                    ));
                    let font = egui::FontId::new(13.0, egui::FontFamily::Monospace);
                    if view.finished {
                        // The invisible keys, drawn, in the design's blue.
                        ui.label(arrived_job(arrived, font));
                    } else {
                        // **The typing target.** `lock_focus(true)` is what
                        // makes a `{TAB}` in the sequence land as a tab
                        // character rather than moving egui's focus off this
                        // widget -- the same job `ES_WANTRETURN` did for the
                        // Win32 edit control this replaces, and without it the
                        // one thing a rehearsal is opened to check is the one
                        // thing it could not show.
                        let field = egui::TextEdit::multiline(arrived)
                            .frame(egui::Frame::new())
                            .desired_width(f32::INFINITY)
                            .desired_rows(4)
                            .lock_focus(true)
                            .font(font)
                            .text_color(PANEL_TEXT);
                        let response = ui.add(field);
                        // `SendInput` goes to the FOCUSED control of the
                        // foreground window. Asked for on every frame it is not
                        // held rather than once: a window that has just been
                        // raised does not necessarily have egui focus on the
                        // frame it was raised on.
                        if !response.has_focus() {
                            response.request_focus();
                        }
                    }
                });

            // ---- WHAT WAS SENT ---------------------------------------------
            egui::Frame::new().inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(theme::letterspaced_mono(SENT_HEADING, 11.0, 0.66, theme::TEXT_FAINT));
                for act in &view.sent {
                    let (label, value) = act_row(act);
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [ACT_LABEL_WIDTH, 16.0],
                            egui::Label::new(
                                RichText::new(label).size(12.0).color(theme::TEXT_FAINT),
                            )
                            .halign(egui::Align::LEFT),
                        );
                        ui.label(theme::semibold(value, 12.0).color(theme::INK));
                    });
                }
                if let Some(failure) = &view.failure {
                    ui.add_space(2.0);
                    failure_band(ui, failure);
                }
            });

            // ---- the footer -------------------------------------------------
            theme::hairline(ui);
            egui::Frame::new()
                .fill(theme::CARD_TINT)
                .inner_margin(Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close |= theme::secondary_button(ui, "Close").clicked();
                    });
                });
        });
    close
}

/// 4d's check mark, stroked rather than set as a glyph for the reason
/// [`theme::close_glyph`] gives about U+2715: neither the bundled Archivo faces
/// nor egui's fallback stack can be relied on for it, and two strokes are
/// sharper at this size than any glyph would be.
///
/// Reserves its space whether or not it is drawn, so the headline does not
/// shift sideways the moment a rehearsal finishes.
fn check_mark(ui: &mut egui::Ui, drawn: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::hover());
    if !drawn {
        return;
    }
    let stroke = Stroke::new(2.0, CHECK_GREEN);
    let c = rect.center();
    ui.painter().line_segment(
        [egui::pos2(c.x - 5.5, c.y + 0.5), egui::pos2(c.x - 1.5, c.y + 4.5)],
        stroke,
    );
    ui.painter().line_segment(
        [egui::pos2(c.x - 1.5, c.y + 4.5), egui::pos2(c.x + 6.0, c.y - 4.5)],
        stroke,
    );
}

/// The band a refusal is reported in.
fn failure_band(ui: &mut egui::Ui, failure: &str) {
    egui::Frame::new()
        .fill(BAND_FILL)
        .stroke(Stroke::new(1.0, BAND_EDGE))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(11, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(failure).size(12.0).color(theme::ERROR));
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fill_stats::FillOutcome;
    use crate::injector::sequence::{plan, Resolved, Step};
    use crate::key_sequence::parse;
    use crate::vault_window::rehearsal::{SAMPLE_PASSWORD, SAMPLE_USER};

    /// **The username and the password are different strings and neither is a
    /// substring of the other**, so no assertion below can pass because two
    /// fixture values happen to agree.
    const REAL_USER: &str = "a.novak@ledgerline.com";
    const REAL_PASSWORD: &str = "Tr0ub4dor&3-correct-horse";
    const DESIGN_SEQUENCE: &str = "{USERNAME}{TAB}{DELAY 250}{PASSWORD}{ENTER}";

    fn real_plan() -> Plan {
        plan(
            &parse(DESIGN_SEQUENCE),
            &Resolved {
                username: REAL_USER,
                password: REAL_PASSWORD,
                totp: None,
                custom: Vec::new(),
            },
        )
        .expect("the fixture must plan")
    }

    /// What the recording sender was handed.
    ///
    /// A `static` because the seam is a `fn` pointer -- which is the point of
    /// it being one: a closure could capture the answer, and would also be a
    /// different address from the real sender, so the identity pin below could
    /// not hold.
    static HANDED_OVER: Mutex<Vec<(isize, Vec<Step>)>> = Mutex::new(Vec::new());

    /// Serialises the tests that read [`HANDED_OVER`]. It is one recorder for
    /// the whole process, and the suite runs across a thread pool, so without
    /// this each of them can see the other's send and fail at random -- the
    /// same reason `injector::sequence_test_lock` exists.
    static RECORDER: Mutex<()> = Mutex::new(());

    fn recorder() -> std::sync::MutexGuard<'static, ()> {
        RECORDER.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn recording_send(hwnd: isize, plan: Plan, done: OutcomeSink) -> Result<(), String> {
        HANDED_OVER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((hwnd, plan.steps().to_vec()));
        done(FillOutcome::Typed);
        Ok(())
    }

    /// A sender that takes the plan and never reports, so the `Typing` stage
    /// can be observed while it is still running.
    fn silent_send(hwnd: isize, plan: Plan, _done: OutcomeSink) -> Result<(), String> {
        HANDED_OVER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((hwnd, plan.steps().to_vec()));
        Ok(())
    }

    fn refusing_send(_: isize, _: Plan, _: OutcomeSink) -> Result<(), String> {
        Err(crate::injector::ALREADY_TYPING.to_string())
    }

    fn must_not_be_sent(_: isize, _: Plan, _: OutcomeSink) -> Result<(), String> {
        panic!("a rehearsal reached the sender with no scratch window to type into");
    }

    const SCRATCH_HWND: isize = 0x5c2a;
    fn a_scratch_window() -> Option<isize> {
        Some(SCRATCH_HWND)
    }
    fn no_scratch_window() -> Option<isize> {
        None
    }

    fn seams(
        target: fn() -> Option<isize>,
        send: fn(isize, Plan, OutcomeSink) -> Result<(), String>,
    ) -> RehearsalSeams {
        RehearsalSeams { plan_for: rehearsal::rehearsal_plan, target, send }
    }

    fn taken() -> Vec<(isize, Vec<Step>)> {
        let mut held = HANDED_OVER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut held)
    }

    fn text_steps(steps: &[Step]) -> Vec<&str> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// **The property, asserted positively, at the position that decides.**
    ///
    /// Not "the sender was not handed the password" -- that is satisfied by a
    /// sender that was handed nothing, which is the vacuous shape this crate
    /// has shipped twice. This says the sender WAS reached, with exactly the
    /// two samples, in that order, at those indices, with the same number of
    /// text steps the real plan had -- and that the real plan still holds the
    /// real values, so the absence is a substitution and not an empty fixture.
    #[test]
    fn the_sender_is_handed_the_samples_and_only_the_samples() {
        let _serialised = recorder();
        let _ = taken();

        let real = real_plan();
        let real_text = text_steps(real.steps()).len();
        assert_eq!(
            real_text, 2,
            "the fixture must type two things, or this test proves nothing about the second"
        );

        let sent = seams(a_scratch_window, recording_send)
            .begin(&real, Box::new(|_| {}))
            .expect("the rehearsal must start");
        assert!(!sent.is_empty(), "the transcript came back empty");

        let handed = taken();
        assert_eq!(handed.len(), 1, "the sender ran {} times, not once", handed.len());
        let (hwnd, steps) = &handed[0];
        assert_eq!(*hwnd, SCRATCH_HWND, "the rehearsal typed into a window it does not own");
        assert_eq!(
            text_steps(steps),
            [SAMPLE_USER, SAMPLE_PASSWORD],
            "the sender was handed something other than the two samples"
        );
        assert_eq!(
            text_steps(steps).len(),
            real_text,
            "a text step was added or dropped on the way to the sender, so the rehearsal is not \
             the sequence the user is about to trust"
        );
        assert_eq!(
            text_steps(real.steps()),
            [REAL_USER, REAL_PASSWORD],
            "control: the real plan does not hold the real values, so the samples above prove \
             nothing about a substitution"
        );
    }

    /// The transcript handed back describes the plan that was SENT, act for
    /// act, and is not the chunk count.
    #[test]
    fn the_transcript_describes_what_was_sent() {
        let _serialised = recorder();
        let _ = taken();
        let sent = seams(a_scratch_window, recording_send)
            .begin(&real_plan(), Box::new(|_| {}))
            .expect("starts");
        assert_eq!(
            sent,
            [
                Arrival::Typed(SAMPLE_USER.to_string()),
                Arrival::Pressed("TAB".to_string()),
                Arrival::Paused(Duration::from_millis(250)),
                Arrival::Typed(SAMPLE_PASSWORD.to_string()),
                Arrival::Pressed("ENTER".to_string()),
            ]
        );
        let _ = taken();
    }

    /// **No scratch window, no send.** Driven through a sender that panics if
    /// it is reached, so this observes the refusal's POSITION and not merely
    /// its value.
    #[test]
    fn without_its_own_window_a_rehearsal_types_nothing_anywhere() {
        assert_eq!(
            seams(no_scratch_window, must_not_be_sent).begin(&real_plan(), Box::new(|_| {})),
            Err(NotRehearsed::NoScratchWindow)
        );
    }

    /// A sender that declines is reported and not swallowed: a rehearsal that
    /// silently did nothing is indistinguishable from a button that is not
    /// wired up.
    #[test]
    fn a_refused_send_is_reported() {
        let _serialised = recorder();
        let _ = taken();
        let refused = seams(a_scratch_window, refusing_send)
            .begin(&real_plan(), Box::new(|_| {}))
            .expect_err("the sender declined");
        assert_eq!(refused, NotRehearsed::NotSent(crate::injector::ALREADY_TYPING.to_string()));
        assert!(refused.message().contains("already being typed"));
    }

    /// A sequence that will not replan is refused **before** the window is even
    /// looked for, and carries the compiler's own sentence.
    #[test]
    fn a_substituted_sequence_that_will_not_plan_is_refused_in_the_compilers_words() {
        fn nothing_to_send(_: &Plan) -> Result<Plan, sequence::Refusal> {
            Err(sequence::Refusal::Nothing)
        }
        let seams = RehearsalSeams {
            plan_for: nothing_to_send,
            target: no_scratch_window,
            send: must_not_be_sent,
        };
        assert_eq!(
            seams.begin(&real_plan(), Box::new(|_| {})),
            Err(NotRehearsed::Refused(sequence::Refusal::Nothing.message()))
        );
    }

    /// **The seams, pinned by ADDRESS.** A seam that is itself unpinned only
    /// moves the hole: production could hand over a `plan_for` that returned
    /// the real plan untouched and every routing assertion above would still
    /// pass, because they all drive a `RehearsalSeams` built here.
    #[test]
    fn production_holds_the_real_substitution() {
        let production = RehearsalSeams::production();
        assert!(
            std::ptr::fn_addr_eq(
                production.plan_for,
                rehearsal::rehearsal_plan as fn(&Plan) -> Result<Plan, sequence::Refusal>
            ),
            "the production rehearsal does not substitute -- it would type the real password \
             into the scratch window"
        );
        assert!(
            std::ptr::fn_addr_eq(
                production.target,
                rehearsal::scratch_target as fn() -> Option<isize>
            ),
            "the production rehearsal does not look for its own window, so it would type into \
             whatever happened to be focused"
        );
        assert!(
            std::ptr::fn_addr_eq(
                production.send,
                send_through_injector as fn(isize, Plan, OutcomeSink) -> Result<(), String>
            ),
            "the production rehearsal does not go through the ordinary sender, so the timing it \
             shows is not the timing a real fill has"
        );
    }

    // ---- the editor's one line ---------------------------------------------

    /// **The editor's one line, both ways.** A sequence the compiler accepts
    /// reports nothing -- an error band under a rehearsal that is about to open
    /// would be a message about nothing -- and one it refuses names why, in the
    /// words the refusal carries rather than in a second set.
    #[test]
    fn a_started_rehearsal_says_nothing_and_a_refused_sequence_says_why() {
        fn started(_: Plan) -> Option<String> {
            None
        }
        fn must_not_open(_: Plan) -> Option<String> {
            panic!("a rehearsal was started for a sequence that will not plan");
        }
        assert_eq!(rehearsal_notice_with(DESIGN_SEQUENCE, started), None);
        // `{PICKCHARS}` is a construct this build carries and cannot type, so
        // it refuses at plan time whatever it is resolved against -- and it
        // does so BEFORE the handoff, which is what the panicking arm observes.
        assert_eq!(
            rehearsal_notice_with("{PICKCHARS}", must_not_open),
            Some(sequence::Refusal::Unsupported("{PICKCHARS}".to_string()).message())
        );
        // Control: the handoff really would have been reached for a sequence
        // that does plan, so the absence above is the refusal and not an
        // unreachable arm.
        assert_eq!(
            rehearsal_notice_with(DESIGN_SEQUENCE, |_| Some("reached".to_string())),
            Some("reached".to_string())
        );
    }

    // ---- the state machine -------------------------------------------------

    fn opening() -> Rehearsal {
        Rehearsal::opening(rehearsal::sample_plan(DESIGN_SEQUENCE).expect("the design plans"))
    }

    /// The transcript the design's sequence really produces, built the way
    /// production builds it, so the window's list can be compared against
    /// something other than itself.
    fn design_transcript() -> Vec<Arrival> {
        rehearsal::transcript(
            &rehearsal::rehearsal_plan(&rehearsal::sample_plan(DESIGN_SEQUENCE).expect("plans"))
                .expect("replans"),
        )
    }

    /// **The whole run, stage by stage, with no window anywhere.**
    ///
    /// Positive at every step: the sender IS reached, with the samples, on the
    /// frame the window is first found; the transcript IS what the sender was
    /// handed; and the run ends when the sink says so and not before.
    #[test]
    fn a_rehearsal_starts_when_its_window_appears_and_ends_when_the_sender_reports() {
        let _serialised = recorder();
        let _ = taken();
        let seams = seams(a_scratch_window, silent_send);
        let rehearsal = opening();
        let t0 = Instant::now();

        // Nothing has been sent before the first frame: `opening` must not
        // start a run by itself.
        assert!(taken().is_empty(), "a rehearsal sent something before its first frame");

        rehearsal.advance(&seams, t0);
        let handed = taken();
        assert_eq!(handed.len(), 1, "the frame that found the window did not start the run");
        assert_eq!(
            text_steps(&handed[0].1),
            [SAMPLE_USER, SAMPLE_PASSWORD],
            "the sender was handed something other than the two samples"
        );
        assert!(rehearsal.in_flight(), "the run is typing and the frames have stopped");
        let view = rehearsal.view();
        assert!(!view.finished);
        assert_eq!(view.headline, rehearsal::WAITING_NOTE);
        assert_eq!(
            view.sent,
            design_transcript(),
            "the window shows a transcript of something other than what the sender was handed"
        );
        assert_eq!(view.sent.len(), 5, "control: the design's sequence is five acts");

        // A second frame while the sink is silent changes nothing, and above
        // all does not send again.
        rehearsal.advance(&seams, t0 + Duration::from_millis(50));
        assert!(taken().is_empty(), "the run was started twice");
        assert!(rehearsal.in_flight());

        // The sink fires; the next frame ends the run and the headline becomes
        // the design's, with the elapsed time this test controls.
        {
            let held = locked(&rehearsal.inner);
            match &held.stage {
                Stage::Typing { finished, .. } => finished.store(true, Ordering::SeqCst),
                other => panic!("expected the run to be typing, and it is {other:?}"),
            }
        }
        rehearsal.advance(&seams, t0 + Duration::from_millis(2100));
        assert!(!rehearsal.in_flight(), "a finished rehearsal is still asking for frames");
        let view = rehearsal.view();
        assert!(view.finished);
        assert_eq!(
            view.headline,
            rehearsal::finished_line(Duration::from_millis(2100), 5),
            "the headline is not the design's finished line for the run that just happened"
        );
        assert_eq!(view.failure, None);
        let _ = taken();
    }

    /// A run that never reports is given up on rather than left claiming to be
    /// watching one -- and the bound really is [`REHEARSAL_PATIENCE`], which is
    /// longer than the longest sequence the compiler accepts.
    #[test]
    fn a_run_that_never_reports_is_given_up_on_after_the_patience() {
        let _serialised = recorder();
        let _ = taken();
        let seams = seams(a_scratch_window, silent_send);
        let rehearsal = opening();
        let t0 = Instant::now();
        rehearsal.advance(&seams, t0);
        let _ = taken();
        rehearsal.advance(&seams, t0 + REHEARSAL_PATIENCE - Duration::from_millis(1));
        assert!(rehearsal.in_flight(), "the patience expired before it was up");
        rehearsal.advance(&seams, t0 + REHEARSAL_PATIENCE);
        assert!(!rehearsal.in_flight());
        assert!(rehearsal.view().finished);
        assert!(
            REHEARSAL_PATIENCE > sequence::MAX_SEQUENCE,
            "a rehearsal of the longest sequence the compiler accepts would time out"
        );
    }

    /// **A window that never appears types nothing anywhere**, and says so in
    /// the window rather than silently. Driven through a sender that panics if
    /// it is reached, so this observes the refusal's POSITION.
    #[test]
    fn a_window_that_never_appears_is_reported_and_nothing_is_sent() {
        let seams = seams(no_scratch_window, must_not_be_sent);
        let rehearsal = opening();
        let t0 = Instant::now();
        rehearsal.advance(&seams, t0);
        assert!(rehearsal.in_flight(), "it gave up on the first frame");
        assert_eq!(rehearsal.view().failure, None, "it reported a failure before it had one");
        rehearsal.advance(&seams, t0 + OPENING_PATIENCE);
        assert!(!rehearsal.in_flight());
        assert_eq!(
            rehearsal.view().failure,
            Some(NotRehearsed::NoScratchWindow.message()),
            "a rehearsal whose window never opened said nothing about it"
        );
    }

    /// A sender that declines is reported **in the window**, which is where the
    /// user is looking, and the acts list stays empty rather than describing a
    /// run that did not happen.
    #[test]
    fn a_sender_that_declines_is_reported_in_the_window() {
        let seams = seams(a_scratch_window, refusing_send);
        let rehearsal = opening();
        rehearsal.advance(&seams, Instant::now());
        let view = rehearsal.view();
        assert_eq!(
            view.failure,
            Some(crate::injector::ALREADY_TYPING.to_string()),
            "the sender declined and the window does not say so"
        );
        assert!(view.sent.is_empty(), "the window lists acts that were never sent");
        assert!(!rehearsal.in_flight());
    }

    // ---- the pump ----------------------------------------------------------

    /// **The frames really do keep coming while a rehearsal is in flight.**
    ///
    /// The one claim this whole file rests on that is not about routing: an
    /// idle pump delivers the `SendInput` burst in a lump at the end, which is
    /// exactly what the user opened the window to avoid.
    ///
    /// Asserted against a real [`egui::Context`], and asserted **both ways** --
    /// a test that only checked "a repaint was requested" would pass against a
    /// `request_repaint()` written unconditionally at the top of `show`, which
    /// is a different (and worse) thing than the one being claimed. The
    /// finished half is what can see that.
    #[test]
    fn frames_keep_coming_while_a_rehearsal_is_in_flight_and_stop_when_it_is_over() {
        let _serialised = recorder();
        let _ = taken();
        let seams = seams(a_scratch_window, silent_send);
        let rehearsal = opening();
        let t0 = Instant::now();
        rehearsal.advance(&seams, t0);
        let _ = taken();
        assert!(rehearsal.in_flight());

        // **Settled first.** A fresh `egui::Context` asks for several
        // repaints of its own while fonts and textures come up, so a bare
        // `has_requested_repaint()` is `true` before anything in this file has
        // run -- and the control below then cannot fail. This drives passes
        // until the context has stopped asking on its own.
        let ctx = settled();
        ctx.begin_pass(egui::RawInput::default());
        if rehearsal.in_flight() {
            ctx.request_repaint();
        }
        let _ = ctx.end_pass();
        assert!(
            ctx.has_requested_repaint(),
            "no repaint was asked for while the sender was typing, so the frames stop and the \
             burst arrives in one lump at the end"
        );

        // And the control: once the run is over the same expression asks for
        // nothing, so the assertion above is about the run's state and not
        // about `request_repaint` being called unconditionally.
        rehearsal.advance(&seams, t0 + REHEARSAL_PATIENCE);
        assert!(!rehearsal.in_flight());
        let idle = settled();
        idle.begin_pass(egui::RawInput::default());
        if rehearsal.in_flight() {
            idle.request_repaint();
        }
        let _ = idle.end_pass();
        assert!(
            !idle.has_requested_repaint(),
            "control: a finished rehearsal still asks for a frame every frame, so the assertion \
             above cannot tell an in-flight run from any other state"
        );
    }

    // ---- the surface -------------------------------------------------------

    fn finished_view() -> RehearsalView {
        let sent = design_transcript();
        RehearsalView {
            headline: rehearsal::finished_line(Duration::from_millis(2100), sent.len()),
            finished: true,
            sent,
            failure: None,
        }
    }

    /// A context with the real theme on it and one frame run, because
    /// `set_fonts` only goes live at the start of the next pass -- a layout
    /// asked for before that is laid out in egui's default faces, which is a
    /// picture of a window this app does not ship.
    /// A context that has stopped asking for repaints of its own accord, so
    /// that a repaint seen after it came from the thing under test.
    ///
    /// Bounded rather than a bare loop: a context that never settles is a
    /// failure worth seeing, not a hang.
    fn settled() -> egui::Context {
        let ctx = egui::Context::default();
        for _ in 0..32 {
            ctx.begin_pass(egui::RawInput::default());
            let _ = ctx.end_pass();
            if !ctx.has_requested_repaint() {
                return ctx;
            }
        }
        panic!("a bare egui context never stopped asking for repaints, so this test cannot tell \
                its own request from the context's");
    }

    fn themed() -> egui::Context {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        ctx.begin_pass(egui::RawInput::default());
        let _ = ctx.end_pass();
        ctx
    }

    /// One whole pass with the surface drawn into a root `Ui` -- the same
    /// thing `show_viewport_deferred` hands its callback. Answers how many
    /// shapes the pass produced.
    ///
    /// A `Ui::new` and not an `egui::Area`: an `Area`'s first frame is a
    /// sizing pass that paints nothing, and a shape count taken over one is a
    /// number that cannot see the surface at all.
    fn painted(ctx: &egui::Context, view: &RehearsalView, arrived: &str) -> usize {
        let mut arrived = arrived.to_string();
        ctx.begin_pass(egui::RawInput::default());
        let mut root = root_ui(ctx);
        let _ = draw(&mut root, view, &mut arrived);
        ctx.end_pass().shapes.len()
    }

    /// A root `Ui` the size of the real window, inside an already-begun pass.
    fn root_ui(ctx: &egui::Context) -> egui::Ui {
        egui::Ui::new(
            ctx.clone(),
            egui::Id::new("rehearsal-preview-root"),
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(SCRATCH_WIDTH, SCRATCH_HEIGHT),
            )),
        )
    }

    /// The window is big enough for the surface 4d describes -- a header, a
    /// transcript panel, the acts and a footer.
    #[test]
    fn the_window_is_sized_for_the_surface() {
        assert!(SCRATCH_WIDTH >= 420.0 && SCRATCH_HEIGHT >= 360.0);
    }

    /// **Every act reads as a sentence, and the three kinds read differently.**
    ///
    /// Positive on all three: a list in which a pause and a key press produced
    /// the same row would be a transcript that cannot answer the question it is
    /// there for.
    #[test]
    fn every_kind_of_act_gets_its_own_row() {
        assert_eq!(
            act_row(&Arrival::Typed(SAMPLE_USER.to_string())),
            ("Typed".to_string(), SAMPLE_USER.to_string())
        );
        assert_eq!(
            act_row(&Arrival::Pressed("TAB".to_string())),
            ("Pressed".to_string(), "TAB".to_string())
        );
        assert_eq!(
            act_row(&Arrival::Paused(Duration::from_millis(250))),
            ("Paused".to_string(), "250 ms".to_string())
        );
        let rows: Vec<(String, String)> = finished_view().sent.iter().map(act_row).collect();
        assert_eq!(rows.len(), 5, "the design's sequence is five acts");
        assert_eq!(
            rows.iter().filter(|(label, _)| label == "Paused").count(),
            1,
            "the 250 ms wait is what makes the timing worth rehearsing, and it has no row"
        );
    }

    /// **The two keys are drawn in the design's blue, and only they are.**
    ///
    /// Counted in sections of the real [`egui::text::LayoutJob`] the panel is
    /// given, so this can see a colour -- a `contains` over the string cannot.
    /// Both directions: the glyph runs ARE blue, the samples are NOT, and a
    /// transcript with no keys in it has no blue section at all.
    #[test]
    fn the_two_invisible_keys_are_the_only_thing_in_blue() {
        let font = egui::FontId::new(13.0, egui::FontFamily::Monospace);
        let job = arrived_job(&format!("{SAMPLE_USER}\t\r\n{SAMPLE_PASSWORD}\r\n"), font.clone());
        let blue: Vec<&str> = job
            .sections
            .iter()
            .filter(|s| s.format.color == theme::BLUE_SOFT)
            .map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0])
            .collect();
        assert_eq!(
            blue,
            [
                rehearsal::ARRIVED_TAB.to_string() + &rehearsal::ARRIVED_ENTER.to_string(),
                rehearsal::ARRIVED_ENTER.to_string()
            ],
            "the blue sections are not exactly the keys that arrived"
        );
        let plain: String = job
            .sections
            .iter()
            .filter(|s| s.format.color == PANEL_TEXT)
            .map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0])
            .collect();
        assert!(
            plain.contains(SAMPLE_USER) && plain.contains(SAMPLE_PASSWORD),
            "the samples were coloured as keys, so nothing on this panel stands out: {plain:?}"
        );
        // The control: a run with no keys in it has no blue at all, so the
        // assertion above is about the glyphs and not about every job having
        // a blue section somewhere.
        let quiet = arrived_job(SAMPLE_USER, font);
        assert!(
            quiet.sections.iter().all(|s| s.format.color == PANEL_TEXT),
            "a transcript in which no key arrived was still drawn with the key colour"
        );
    }

    /// **The design's transcript glyphs really render.**
    ///
    /// [`theme::close_glyph`] exists because U+2715 is a tofu box in this
    /// crate's font stack, so "the design uses this codepoint" is not evidence
    /// that egui can draw it -- and a rehearsal whose readout cannot tell "the
    /// Tab arrived" from "the Tab did not" answers the one question it was
    /// opened to answer with a blank rectangle.
    ///
    /// Asked of the real font stack [`theme::apply`] installs, at the family
    /// the panel draws them in, with a control that the same question comes out
    /// FALSE for a codepoint this stack really lacks -- otherwise `has_glyph`
    /// answering `true` for everything would make this vacuous.
    #[test]
    fn the_tab_and_enter_glyphs_are_drawable_in_the_panel_font() {
        let ctx = themed();
        let font = egui::FontId::new(13.0, egui::FontFamily::Monospace);
        let panel = rehearsal::arrived_panel(&format!("{SAMPLE_USER}\t\r\n{SAMPLE_PASSWORD}\r\n"));
        assert!(
            panel.contains(rehearsal::ARRIVED_TAB) && panel.contains(rehearsal::ARRIVED_ENTER),
            "control: the panel under test does not contain the two glyphs at all: {panel:?}"
        );
        for glyph in [rehearsal::ARRIVED_TAB, rehearsal::ARRIVED_ENTER] {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&font, glyph)),
                "the design's {glyph:?} is a tofu box in this crate's monospace stack, so the \
                 readout cannot show whether that key arrived"
            );
        }
        assert!(
            !ctx.fonts_mut(|f| f.has_glyph(&font, '\u{10FFFD}')),
            "control: this stack claims every codepoint, so the two assertions above are vacuous"
        );
    }

    /// **The surface really draws**, headline, panel, acts and all, against a
    /// real context with the real theme -- and the close button really answers.
    ///
    /// A `Context::run` and not a lookless assertion: `draw` allocates, lays
    /// out and paints, and a panic anywhere in it (a `set_width` in the wrong
    /// layout, a font family that does not exist) is invisible to any test that
    /// only reads [`RehearsalView`].
    ///
    /// The paint is then asserted to have produced ink, controlled against an
    /// empty panel in an identically themed context -- a shape count that
    /// cannot come out low is not an assertion.
    #[test]
    fn the_surface_lays_out_and_paints() {
        let ctx = themed();
        let view = finished_view();
        let mut arrived = format!("{SAMPLE_USER}\t\r\n{SAMPLE_PASSWORD}\r\n");
        ctx.begin_pass(egui::RawInput::default());
        let mut root = root_ui(&ctx);
        let closed = draw(&mut root, &view, &mut arrived);
        let ink = ctx.end_pass().shapes.len();
        assert!(!closed, "the surface reported a close nobody asked for");

        let empty = themed();
        empty.begin_pass(egui::RawInput::default());
        let _bare_root = root_ui(&empty);
        let bare = empty.end_pass().shapes.len();
        assert!(
            ink > bare,
            "the rehearsal surface painted {ink} shapes and an empty panel painted {bare} -- so \
             this test cannot see the surface at all"
        );
    }

    /// The live panel is the **typing target**: something in the window has
    /// keyboard focus, or `SendInput` types into no control at all.
    #[test]
    fn the_live_panel_takes_keyboard_focus() {
        let ctx = themed();
        let live = RehearsalView {
            headline: rehearsal::WAITING_NOTE.to_string(),
            finished: false,
            sent: Vec::new(),
            failure: None,
        };
        // Two frames, because a focus asked for during a pass is granted for
        // the next one -- and because the second is the one that shows the
        // request being RE-made while it is already held, which is what keeps
        // the field focused for the whole burst.
        let _ = painted(&ctx, &live, "");
        let _ = painted(&ctx, &live, "");
        assert!(
            ctx.memory(|m| m.focused()).is_some(),
            "nothing in the live rehearsal window has keyboard focus, so `SendInput` would type \
             into no control at all"
        );

        // And the other half: a FINISHED rehearsal draws the glyph transcript
        // instead, which is a label and takes no focus -- so the assertion
        // above is about the live branch and not about `draw` in general.
        let finished = themed();
        let _ = painted(&finished, &finished_view(), "");
        let _ = painted(&finished, &finished_view(), "");
        assert!(
            finished.memory(|m| m.focused()).is_none(),
            "the finished readout is still a text field, so the user's next keystroke would be \
             typed into the transcript of a run that is over"
        );
    }

    /// A refusal is drawn, and the band is not drawn when there is none --
    /// otherwise "the failure is visible" is a claim about a band that is
    /// always there.
    #[test]
    fn a_refusal_is_drawn_and_an_ordinary_run_has_no_band() {
        let ctx = themed();
        let plain = finished_view();
        let refused =
            RehearsalView { failure: Some(NotRehearsed::NoScratchWindow.message()), ..plain.clone() };
        assert!(
            painted(&ctx, &refused, "") > painted(&ctx, &plain, ""),
            "the refusal band paints no more than an ordinary run does, so a rehearsal that \
             could not open its window looks exactly like one that worked"
        );
    }
}

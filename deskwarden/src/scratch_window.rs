//! **The window a rehearsal types into, and the run itself.**
//!
//! [`crate::vault_window::rehearsal`] had the whole guarantee and no way to
//! reach it: `substitute` was total, `rehearsal_plan` re-chunked through the
//! ordinary compiler, `scratch_target` named the window to look for -- and
//! nothing opened that window, so the module's own doc said in as many words
//! that "nothing in this module is reachable from a running app". This file is
//! the missing half.
//!
//! # Why this is not an `eframe` window, when `preflight_host` is
//!
//! [`crate::preflight_host::show_preflight`] is a blocking `eframe::run_native`
//! and it can be, because it is opened from `main`'s dispatch loop -- between
//! windows, on the main thread, with no event loop running. **A rehearsal is
//! started from inside one.** The button lives in the sequence editor, which is
//! painted inside `vault_window::run`'s `eframe::run_ui_native` closure, and
//! `winit` refuses to build a second event loop while that one is alive. The
//! call would answer `Err`, every `run_*native` result in this crate is
//! discarded, and the button would do nothing at all with the whole suite
//! green -- which is the exact defect shape this codebase keeps getting caught
//! by, so it is not the shape used here.
//!
//! So the scratch window is a plain Win32 window with an edit control in it,
//! opened and pumped by this file -- the same thing
//! [`crate::file_picker::pick_executable`] does with the shell's file dialog,
//! and for the same reason: it is modal, it is opened mid-frame, and it does
//! not touch `winit`. It is therefore also the **first window in this crate
//! that is alive at the same time as another one**, which is safe for exactly
//! one reason: its title is unique. See
//! `foreground::only_one_window_of_this_process_can_exist_at_a_time`.
//!
//! # The pump has to keep running while the sequence types
//!
//! `SendInput` posts to the target thread's message queue. A rehearsal that
//! started the typing and then blocked waiting for it would receive every
//! keystroke in one lump at the end -- which is precisely the timing the user
//! opened this window to watch. So the loop below starts the run and then keeps
//! pumping, and the run reports back through the ordinary
//! [`crate::injector::OutcomeSink`].
//!
//! # What is testable here
//!
//! The window is not: it is `CreateWindowExW` and a message loop. Every
//! decision is in [`RehearsalSeams::begin`], which reaches the outside world
//! through three `fn` pointers -- what to send, where to send it, and how -- so
//! a test drives the whole routing with a recorder and asserts **positively**
//! on the steps the sender was handed. [`RehearsalSeams::production`] holds the
//! real three by identity and `production_holds_the_real_substitution` pins
//! that with `fn_addr_eq`, so a wrapper that quietly passed the real plan
//! through is a different address and fails there.

use crate::injector::sequence::{self, Plan};
use crate::injector::{Injector, OutcomeSink, RealSendInput, RealUiAutomation};
use crate::vault_window::rehearsal::{self, Arrival};

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
    /// waits for it: the caller has a message pump to keep running.
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
/// real fill would -- which is right: both are `SendInput`.
fn send_through_injector(hwnd: isize, plan: Plan, done: OutcomeSink) -> Result<(), String> {
    Injector { ui: RealUiAutomation, fallback: RealSendInput }.fill_sequence(hwnd, plan, done)
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

pub const SCRATCH_WIDTH: i32 = 520;
pub const SCRATCH_HEIGHT: i32 = 420;

/// How long the window waits for a rehearsal before it stops claiming to be
/// watching one.
///
/// [`sequence::MAX_SEQUENCE`] plus a margin for the sender's own foreground
/// settle. Timing out abandons nothing: the typing thread owns the plan and
/// wipes it on the way out, whatever this window does.
pub const REHEARSAL_PATIENCE: std::time::Duration =
    std::time::Duration::from_secs(sequence::MAX_SEQUENCE.as_secs() + 10);

/// What a finished rehearsal is, for the caller that has to say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rehearsed {
    /// The characters that really landed in the scratch window's edit control.
    /// **Never a secret**: the only thing typed was a sample.
    pub arrived: String,
    /// The acts the sender was handed, for the count beside the transcript.
    pub sent: Vec<Arrival>,
    pub elapsed: std::time::Duration,
}

/// Opens the scratch window, runs the rehearsal in it, and blocks until the
/// user closes it.
///
/// Nothing in this crate can call it: it opens a real window and pumps a real
/// message loop. Every decision it forwards to is tested where that decision
/// lives -- [`RehearsalSeams::begin`] for the routing,
/// [`rehearsal::report_text`] for the readout.
pub fn show_scratch(real: &Plan) -> Result<Rehearsed, NotRehearsed> {
    win32::run(real)
}

/// **4d, from the editor's side.** Runs a rehearsal of `sequence` and answers
/// what to tell the user, or `None` when there is nothing to say.
///
/// The one line the sequence editor's Rehearse arm contains. A function and not
/// four lines in that arm, because the arm is inside `vault_window::run`'s
/// frame closure and nothing in this crate can enter it -- a sentence composed
/// there is a sentence no test can read back, which is the shape this crate
/// keeps getting caught by.
///
/// **`sequence`, and nothing else.** No item, no password, no one-time code:
/// [`rehearsal::sample_plan`] resolves every field to a fixed sample, so there
/// is no argument here that could carry a secret in the first place.
pub fn rehearsal_notice(sequence: &str) -> Option<String> {
    rehearsal_notice_with(sequence, show_scratch)
}

/// [`rehearsal_notice`] with the window injected, so the two things it decides
/// -- which refusal is reported, and that a finished rehearsal reports nothing
/// -- are reachable without opening one.
fn rehearsal_notice_with(
    sequence: &str,
    run: fn(&Plan) -> Result<Rehearsed, NotRehearsed>,
) -> Option<String> {
    match rehearsal::sample_plan(sequence) {
        // The compiler's own sentence, not a second wording of it: a sequence
        // over `MAX_SEQUENCE` says so in the words the fill would have used.
        Err(refusal) => Some(refusal.message()),
        Ok(plan) => run(&plan).err().map(|why| why.message()),
    }
}

/// The Win32 calls themselves, and the loop. No decisions: everything branched
/// on here is a handle being usable or a flag being set.
mod win32 {
    use super::{
        NotRehearsed, RehearsalSeams, Rehearsed, REHEARSAL_PATIENCE, SCRATCH_HEIGHT, SCRATCH_WIDTH,
    };
    use crate::injector::sequence::Plan;
    use crate::vault_window::rehearsal::{self, SCRATCH_TITLE};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetWindowTextLengthW,
        GetWindowTextW, PeekMessageW, PostQuitMessage, RegisterClassW, SetWindowTextW,
        ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, ES_AUTOVSCROLL, ES_MULTILINE,
        ES_READONLY, ES_WANTRETURN, MSG, PM_REMOVE, SW_SHOW, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY,
        WM_QUIT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU, WS_VISIBLE,
        WS_VSCROLL,
    };

    /// Registered once per process. A second `RegisterClassW` under the same
    /// name fails, and the failure is not interesting: the class is already
    /// there, which is what the caller wanted.
    static CLASS_REGISTERED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    const CLASS_NAME: PCWSTR = w!("DeskwardenRehearsalScratch");

    /// Set by `WM_DESTROY`. A `static` rather than window data because there is
    /// exactly one scratch window at a time: it is opened from the vault
    /// window's frame closure, which is not re-entered while this loop runs.
    static CLOSED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                CLOSED.store(true, Ordering::SeqCst);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn register_class() {
        CLASS_REGISTERED.get_or_init(|| unsafe {
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            RegisterClassW(&class);
        });
    }

    fn text_of(hwnd: HWND) -> String {
        unsafe {
            let len = GetWindowTextLengthW(hwnd);
            if len <= 0 {
                return String::new();
            }
            let mut buffer = vec![0u16; len as usize + 1];
            let copied = GetWindowTextW(hwnd, &mut buffer);
            String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
        }
    }

    fn set_text(hwnd: HWND, text: &str) {
        let text = HSTRING::from(text);
        unsafe {
            let _ = SetWindowTextW(hwnd, &text);
        }
    }

    /// Every message currently queued for this thread, dispatched.
    ///
    /// `PeekMessage` and not `GetMessage`: both loops below have a second
    /// condition to re-check, and `GetMessage` would sit inside Windows until
    /// the next message arrived -- which, during a `{DELAY 2000}`, is never.
    fn pump() {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    CLOSED.store(true, Ordering::SeqCst);
                    return;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn edit(parent: HWND, y: i32, height: i32, extra: u32) -> Option<HWND> {
        // `ES_WANTRETURN` on the typing box is what makes an `{ENTER}` in the
        // sequence land as a line break rather than being swallowed as a
        // default-button press, so the arrival really shows the key that was
        // sent. `ES_READONLY` on the readout lets the transcript be selected
        // and copied but not typed over.
        let style = WS_CHILD.0 | WS_VISIBLE.0 | WS_BORDER.0 | WS_VSCROLL.0 | extra;
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                PCWSTR::null(),
                windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(style),
                12,
                y,
                SCRATCH_WIDTH - 40,
                height,
                parent,
                None,
                None,
                None,
            )
            .ok()
        }
    }

    pub fn run(real: &Plan) -> Result<Rehearsed, NotRehearsed> {
        register_class();
        CLOSED.store(false, Ordering::SeqCst);

        let title = HSTRING::from(SCRATCH_TITLE);
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS_NAME,
                &title,
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                200,
                200,
                SCRATCH_WIDTH,
                SCRATCH_HEIGHT,
                None,
                None,
                None,
                None,
            )
        };
        // No window means no target, which is the one answer that stops a
        // rehearsal rather than sending it somewhere else.
        let Ok(window) = window else {
            return Err(NotRehearsed::NoScratchWindow);
        };
        let typing = (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32;
        let reading = (ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY) as u32;
        let (Some(target), Some(readout)) = (
            edit(window, 12, 140, typing),
            edit(window, 164, SCRATCH_HEIGHT - 230, reading),
        ) else {
            unsafe {
                let _ = DestroyWindow(window);
            }
            return Err(NotRehearsed::NoScratchWindow);
        };

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            let _ = SetFocus(target);
        }
        // The OS window exists by here -- the same hook every window in this
        // crate raises from. See `foreground`: a refusal from Windows flashes
        // the taskbar button rather than being ignored.
        crate::foreground::raise_window(SCRATCH_TITLE);
        set_text(readout, rehearsal::WAITING_NOTE);

        // Drain what is already queued, so the window is mapped, painted and
        // focused before the first keystroke is synthesised into it.
        pump();

        let finished = Arc::new(AtomicBool::new(false));
        let signal = finished.clone();
        let started = Instant::now();
        let begun = RehearsalSeams::production()
            .begin(real, Box::new(move |_outcome| signal.store(true, Ordering::SeqCst)));
        let sent = match begun {
            Ok(sent) => sent,
            Err(why) => {
                unsafe {
                    let _ = DestroyWindow(window);
                }
                pump();
                return Err(why);
            }
        };

        // **Pumping, not waiting.** See this module's header: a blocked pump
        // delivers every synthetic keystroke in one lump at the end, which is
        // the one thing a rehearsal exists to let the user watch happen slowly.
        while !finished.load(Ordering::SeqCst)
            && !CLOSED.load(Ordering::SeqCst)
            && started.elapsed() < REHEARSAL_PATIENCE
        {
            pump();
            std::thread::sleep(Duration::from_millis(4));
        }
        let elapsed = started.elapsed();
        let arrived = text_of(target);
        set_text(readout, &rehearsal::report_text(&arrived, elapsed, sent.len()));

        // Left open on purpose: the transcript is the whole point, and a window
        // that vanished the instant the last key landed would show it for one
        // frame. The user closes it.
        while !CLOSED.load(Ordering::SeqCst) {
            pump();
            std::thread::sleep(Duration::from_millis(8));
        }

        Ok(Rehearsed { arrived, sent, elapsed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fill_stats::FillOutcome;
    use crate::injector::sequence::{plan, Resolved, Step};
    use crate::key_sequence::parse;
    use crate::vault_window::rehearsal::{SAMPLE_PASSWORD, SAMPLE_USER};
    use std::sync::Mutex;
    use std::time::Duration;

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

    /// Serialises the two tests that read [`HANDED_OVER`]. It is one recorder
    /// for the whole process, and the suite runs across a thread pool, so
    /// without this each of them can see the other's send and fail at random --
    /// the same reason `injector::sequence_test_lock` exists.
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

    /// **The editor's one line, both ways.** A rehearsal that ran says nothing
    /// -- an error band after a successful rehearsal would be a message about
    /// nothing -- and one that did not names why, in the words the refusal
    /// carries rather than in a second set.
    #[test]
    fn a_finished_rehearsal_says_nothing_and_a_refused_one_says_why() {
        fn finished(_: &Plan) -> Result<Rehearsed, NotRehearsed> {
            Ok(Rehearsed {
                arrived: format!("{SAMPLE_USER}	"),
                sent: Vec::new(),
                elapsed: Duration::from_millis(2100),
            })
        }
        fn no_window(_: &Plan) -> Result<Rehearsed, NotRehearsed> {
            Err(NotRehearsed::NoScratchWindow)
        }
        assert_eq!(rehearsal_notice_with(DESIGN_SEQUENCE, finished), None);
        assert_eq!(
            rehearsal_notice_with(DESIGN_SEQUENCE, no_window),
            Some(NotRehearsed::NoScratchWindow.message())
        );
    }

    /// A sequence the compiler will not accept is refused **before** a window
    /// is opened, in the compiler's own sentence. Driven through a runner that
    /// panics if it is reached, so this observes the refusal's position.
    #[test]
    fn a_sequence_that_will_not_plan_opens_no_window_at_all() {
        fn must_not_open(_: &Plan) -> Result<Rehearsed, NotRehearsed> {
            panic!("a scratch window was opened for a sequence that will not plan");
        }
        // `{PICKCHARS}` is a construct this build carries and cannot type, so
        // it refuses at plan time whatever it is resolved against.
        let refused = rehearsal_notice_with("{PICKCHARS}", must_not_open)
            .expect("an untypable sequence must be reported");
        assert_eq!(refused, sequence::Refusal::Unsupported("{PICKCHARS}".to_string()).message());
        // Control: the runner really would have been reached for a sequence
        // that does plan, so the absence above is the refusal and not an
        // unreachable arm.
        assert_eq!(
            rehearsal_notice_with(DESIGN_SEQUENCE, |_| Err(NotRehearsed::NoScratchWindow)),
            Some(NotRehearsed::NoScratchWindow.message())
        );
    }

    /// The window is big enough for the surface 4d describes -- a typing box
    /// and a transcript under it -- and the patience outlasts the longest
    /// sequence the compiler will accept.
    #[test]
    fn the_window_is_sized_for_the_surface_and_waits_longer_than_a_sequence_can_run() {
        assert!(SCRATCH_WIDTH >= 420 && SCRATCH_HEIGHT >= 360);
        assert!(
            REHEARSAL_PATIENCE > sequence::MAX_SEQUENCE,
            "a rehearsal of the longest sequence the compiler accepts would time out"
        );
    }
}

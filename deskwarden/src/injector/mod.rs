pub mod send_input;
pub mod sequence;
pub mod ui_automation;

use crate::fill_stats::FillOutcome;
use sequence::Plan;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where a fill's **real** outcome goes, once something knows it.
///
/// A boxed `FnOnce` and not a channel. The outcome is produced on the typing
/// thread and there is nobody left on the UI thread waiting for it, so a
/// channel would need a receiver polled from the message pump -- one more
/// place to forget, and a fill whose outcome arrives after the app closes
/// would simply be lost. Worse, a channel invites a `recv()`, and the one
/// thing this design may never do is make the UI thread wait on the typing
/// thread; that is the entire reason the thread exists.
///
/// A closure that owns everything it needs instead (see
/// `app::fill_outcome_sink`: a `FillStats`, which is a `PathBuf`, and a copy
/// of the item id) reports from the thread that knows, borrows nothing owned
/// by the UI, and blocks nobody. `Send + 'static` is not decoration -- it is
/// the compiler enforcing that second property.
pub type OutcomeSink = Box<dyn FnOnce(FillOutcome) + Send + 'static>;

pub trait UiAutomationFiller {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String>;
}

/// Whether a typing run is in flight. See [`SequenceGuard`].
static SEQUENCE_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Proof that the holder, and nobody else, may synthesise keystrokes right now.
///
/// # What goes wrong without it
///
/// [`SendInputFiller::fill_sequence`] performs a sequence on a fresh thread,
/// because a `{DELAY 2000}` performed on the caller's thread would freeze the
/// app. Nothing stopped a second hotkey press *during* that delay from
/// starting a second thread. Both threads then see the same foreground window
/// and both pass every check they make -- the checks are individually correct
/// and jointly useless -- so the two sequences interleave their keystrokes
/// into the same field. Each thread's [`Plan`] still wipes on drop, so this
/// corrupts a login rather than leaking a password, but a half-typed
/// interleaving of two passwords is not a state a user can diagnose.
///
/// # Refuse, rather than queue
///
/// A queued sequence types into whatever is in front when its turn arrives,
/// which by then may be a window the user has navigated away from. That is
/// precisely the hazard the whole `sequence` module is built to prevent, so
/// queueing would reintroduce it at the one layer that cannot check for it:
/// the plan was resolved against an `hwnd` chosen a fill ago. Refusing loses
/// nothing a second hotkey press cannot recover, and the refusal reaches the
/// user (`app::fill_from_vault` passes it to the notifier), so a fill that
/// declined to happen is never silent.
///
/// # Scope, release and liveness
///
/// The flag is **process-global, not per-[`Injector`]**. `Injector` is `Clone`
/// and is cloned onto worker threads; what is being contended for is not an
/// injector but the machine's one keyboard and one foreground window, so two
/// clones typing at once is exactly the case this exists to stop.
///
/// Released by `Drop`, which runs on a panicking typing thread too -- this
/// crate unwinds -- so a panic mid-sequence cannot strand the flag set and
/// wedge auto-type for the rest of the session. **Nothing ever blocks on it**:
/// a caller that cannot have it is told so and gives up immediately, so there
/// is no lock here to deadlock on and no waiter to strand.
///
/// # It also carries the outcome back
///
/// The guard is already the one thing that is moved onto the typing thread
/// and whose `Drop` runs there on every exit including a panic. That makes it
/// the right place to hang "what did this fill actually do?" on: a separate
/// reporting channel would have to duplicate the same move and the same
/// drop-on-unwind guarantee, and could be forgotten on a path the guard
/// cannot be. See [`OutcomeSink`] and [`SequenceGuard::report`].
pub struct SequenceGuard {
    /// Where to report what the typing did. `None` for a guard nobody is
    /// counting -- the tests that contend for the flag and nothing else.
    sink: Option<OutcomeSink>,
    /// What will be reported if nothing says otherwise.
    ///
    /// **`NotTyped` by default, on purpose.** A typing thread that panics
    /// part-way never reaches its `report` call, and unwinding still runs
    /// this `Drop`, so the count stays where it was rather than crediting a
    /// fill nobody can vouch for. The failure mode of the default is an
    /// undercount, which is recoverable by filling again; the failure mode of
    /// defaulting the other way is a password that was never typed climbing
    /// the picker.
    outcome: FillOutcome,
}

impl SequenceGuard {
    /// `None` if a sequence is already running.
    pub fn acquire() -> Option<Self> {
        SEQUENCE_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { sink: None, outcome: FillOutcome::NotTyped })
    }

    /// Attaches the callback that will be handed this guard's outcome, exactly
    /// once, when it drops -- on whichever thread that turns out to be.
    pub fn reports_to(&mut self, sink: OutcomeSink) {
        self.sink = Some(sink);
    }

    /// Records what the typing actually did. The last call before the drop is
    /// the one that counts; no call at all means [`FillOutcome::NotTyped`].
    pub fn report(&mut self, outcome: FillOutcome) {
        self.outcome = outcome;
    }
}

impl Drop for SequenceGuard {
    fn drop(&mut self) {
        // The flag first, unconditionally, before anything that could go
        // wrong: releasing the right to type must not be contingent on a
        // bookkeeping callback behaving.
        SEQUENCE_IN_FLIGHT.store(false, Ordering::Release);
        if let Some(sink) = self.sink.take() {
            let outcome = self.outcome;
            // A `Drop` that panics while the thread is *already* unwinding
            // aborts the process. The sink writes a small JSON file and
            // swallows its own errors by design, so it should never panic --
            // and "should never" is not a thing to bet a process abort on.
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || sink(outcome)));
        }
    }
}

/// Serialises every test that passes through [`Injector::fill_sequence`].
///
/// [`SEQUENCE_IN_FLIGHT`] is process-global, which is the correct scope for
/// the machine's one keyboard and the wrong scope for a suite running fifteen
/// hundred tests across a thread pool: two unrelated tests inside
/// `fill_sequence` at the same instant would leave one of them holding the
/// flag while the other was told "already typing", and it would fail at
/// random. Taking this first means the only contention a test ever observes is
/// the contention it arranged itself.
#[cfg(test)]
static SEQUENCE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn sequence_test_lock() -> std::sync::MutexGuard<'static, ()> {
    // A test that panicked while holding this poisons it. The protected data
    // is `()`, so there is no invariant left broken; propagating the poison
    // would turn one genuine failure into a cascade of unrelated ones that
    // hides it.
    SEQUENCE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub trait SendInputFiller {
    /// `hwnd` is the window the caller *intends* to fill. Implementations must
    /// verify it actually has foreground before typing: `SendInput` goes to
    /// whatever holds keyboard focus, which is not necessarily the window we
    /// matched (see `send_input::ensure_foreground`).
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String>;

    /// Performs a planned auto-type sequence against `hwnd`.
    ///
    /// Takes the [`Plan`] **by value** so it can be moved onto whichever
    /// thread performs it: a sequence contains `{DELAY}`s, and performing one
    /// on the caller's thread would freeze the app for as long as the user
    /// asked it to wait. Owning it also means the plan's `Drop` -- the wipe --
    /// runs wherever it finishes, rather than leaving a plaintext password on
    /// a caller's stack for the duration.
    ///
    /// **Required, with no default body.** It used to default to a refusal,
    /// on the argument that a filler which had not opted in must not look like
    /// one that had. The argument was right and the mechanism was wrong:
    /// replacing that default with `Ok(())` left the entire suite green, and
    /// it silently re-pointed `main.rs`'s `NeverTypes` -- which overrides
    /// `fill` with a panic precisely to prove the fallback is unreachable, but
    /// inherited the default here -- at a body that succeeds and records a
    /// fill. A defaulted method cannot be pinned by a test, because there is
    /// no implementor whose absence anything can notice. This trait has
    /// exactly two real implementors; requiring the method costs them one line
    /// each and makes forgetting it a compile error.
    ///
    /// `guard` is the process-wide permission to type, taken by
    /// [`Injector::fill_sequence`] before it calls this. It is passed by value
    /// rather than acquired here so that an implementation which spawns a
    /// thread must *move* it onto that thread, tying the flag's release to the
    /// end of the typing rather than to the return of this call -- which
    /// happens almost immediately and would make the guard meaningless.
    ///
    /// **It is also how the outcome gets home.** `Ok(())` from this method
    /// means "started", so an implementation must call
    /// [`SequenceGuard::report`] with what the typing actually did before the
    /// guard drops. Saying nothing is not a bug, it is a claim:
    /// [`FillOutcome::NotTyped`], which is what a panicking thread reports by
    /// default and the only safe thing to assume of a run that never spoke.
    fn fill_sequence(&self, hwnd: isize, plan: Plan, guard: SequenceGuard)
        -> Result<(), String>;
}

#[derive(Clone)]
pub struct Injector<A: UiAutomationFiller, B: SendInputFiller> {
    pub ui: A,
    pub fallback: B,
}

impl<A: UiAutomationFiller, B: SendInputFiller> Injector<A, B> {
    pub fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String> {
        match self.ui.fill(hwnd, user, pass) {
            Ok(true) => Ok(()),
            Ok(false) => self.fallback.fill(hwnd, user, pass),
            Err(e) => {
                log::warn!("UI Automation fill failed for hwnd {hwnd} ({e}); using SendInput");
                self.fallback.fill(hwnd, user, pass)
            }
        }
    }

    /// The sequence path. **It does not try UI Automation first**, and that is
    /// the answer to whether the default fill is "just a sequence".
    ///
    /// It is not. UI Automation fills *named fields*: it walks the window's
    /// automation tree, finds the control whose type says password, and sets
    /// its value -- without depending on focus, without synthesising a single
    /// keystroke, and without caring what the tab order is. A sequence types
    /// *keystrokes at whatever has focus*. Those are different acts with
    /// different failure modes, and the UIA one is strictly safer where it
    /// works, which is why the default fill still starts there.
    ///
    /// Collapsing them would have been elegant and wrong twice over. It would
    /// have deleted the UIA path for every existing item in every existing
    /// vault (all of which store no sequence), turning a fill that needs no
    /// foreground into one that does. And it could not have worked anyway:
    /// UIA has no way to express `{ENTER}`, a `{DELAY 2000}`, or a second
    /// screen. `key_sequence::DEFAULT_SEQUENCE` is an honest description of
    /// what the *SendInput fallback* does -- which is what the preview should
    /// show a user with no sequence -- but the default fill is a different
    /// act, so it keeps a different path.
    /// Refuses outright if a sequence is already being typed -- see
    /// [`SequenceGuard`] for why refusing beats queueing.
    ///
    /// `on_outcome` is handed what the fill actually did, once something
    /// knows -- which for the sequence path is later, on the typing thread.
    /// **Every** exit reports exactly once, including this one, so a caller
    /// that counts fills never has to infer anything from the return value.
    pub fn fill_sequence(
        &self,
        hwnd: isize,
        plan: Plan,
        on_outcome: OutcomeSink,
    ) -> Result<(), String> {
        let Some(mut guard) = SequenceGuard::acquire() else {
            // Explicit, and not merely "let it fall out of scope": this is the
            // one exit from this function that holds a resolved password and
            // builds no `Plan`-owning thread to wipe it later.
            drop(plan);
            // Equally explicit. A refused fill typed nothing, and saying so
            // out loud beats leaving `on_outcome` to be dropped uncalled and
            // relying on a reader to work out that a sink never invoked and a
            // sink invoked with `NotTyped` happen to mean the same thing.
            on_outcome(FillOutcome::NotTyped);
            return Err(
                "an auto-type sequence is already being typed; wait for it to \
                 finish, or switch away to stop it, then press the hotkey again"
                    .to_string(),
            );
        };
        guard.reports_to(on_outcome);
        self.fallback.fill_sequence(hwnd, plan, guard)
    }
}

#[derive(Clone, Copy)]
pub struct RealUiAutomation;
impl UiAutomationFiller for RealUiAutomation {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String> {
        ui_automation::fill_via_ui_automation(hwnd, user, pass).map_err(|e| e.to_string())
    }
}

/// **What a finished [`sequence::run`] meant for the fill count**, as a pure
/// function.
///
/// One line, extracted, because the body it came out of runs on a typing
/// thread behind `SendInput` and cannot be entered from a test -- and it is
/// the line that decides whether a password the user interrupted counts as
/// typed. Left inline it would have been exactly the shape this crate keeps
/// getting caught by: a value at a call site that nothing can read back.
///
/// `Err` covers both an abandoned run (the foreground re-check refused before
/// a step) and a failed keystroke. Both are [`FillOutcome::Partial`]: typing
/// had begun, and it did not finish. Neither counts -- see
/// [`crate::fill_stats::counts_as_a_fill`].
fn outcome_of_a_run(result: &Result<(), String>) -> FillOutcome {
    match result {
        Ok(()) => FillOutcome::Typed,
        Err(_) => FillOutcome::Partial,
    }
}

#[derive(Clone, Copy)]
pub struct RealSendInput;
impl SendInputFiller for RealSendInput {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String> {
        // Verify (and if necessary restore) foreground before typing anything.
        // On mismatch this returns Err and nothing is typed, rather than
        // blasting a password into an unverified window.
        send_input::ensure_foreground(hwnd)?;
        send_input::fill_via_send_input(user, pass).map_err(|e| e.to_string())
    }

    /// Restores foreground once, then hands the plan to a thread.
    ///
    /// The one [`send_input::ensure_foreground`] call is the *same* one the
    /// default fill makes and is there for the same reason: right after our
    /// own overlay closes, Windows has not necessarily handed focus back yet,
    /// and refusing on that transient would refuse most Prompt-mode fills.
    /// After that single restore, every further check is
    /// `RealKeyboard::holds_foreground`, which is passive -- see its doc for
    /// why re-stealing focus mid-sequence would be exactly backwards.
    ///
    /// The thread is what keeps a `{DELAY 2000}` from freezing the app. It
    /// means `Ok(())` here means "started", not "typed"; the outcome the user
    /// needs to know about is an abort, and that is reported by the notifier
    /// from inside the thread rather than through a return value nobody is
    /// still waiting on -- and, since the count of fills must mean "typed" and
    /// not "started" either, through `guard.report` on the same thread.
    fn fill_sequence(&self, hwnd: isize, plan: Plan, guard: SequenceGuard) -> Result<(), String> {
        let mut guard = guard;
        if let Err(e) = send_input::ensure_foreground(hwnd) {
            // The one exit that returns before a thread exists. Nothing was
            // typed, and this says so rather than leaning on the default.
            guard.report(FillOutcome::NotTyped);
            return Err(e);
        }
        std::thread::spawn(move || {
            // `guard` is moved onto this thread so the "already typing" flag
            // stays set for as long as this thread is really typing, and not
            // merely until the spawn returns.
            let mut guard = guard;
            let result = sequence::run(&send_input::RealKeyboard, hwnd, &plan);
            guard.report(outcome_of_a_run(&result));
            if let Err(e) = result {
                sequence::Notifier::refused(&sequence::RealNotifier, &e);
            }
            // `plan` is dropped here, on this thread, wiping the password --
            // whether it finished, aborted, or failed. `guard` is dropped with
            // it, releasing the flag and reporting the outcome; on a panic,
            // unwinding does both, and the outcome it reports is the default
            // `NotTyped` rather than a fill nothing can vouch for.
        });
        Ok(())
    }
}

#[cfg(test)]
mod orchestration_tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};

    /// A sink that remembers every outcome reported through it.
    ///
    /// A `Vec<FillOutcome>` and not a counter: the assertions below care
    /// *which* outcome arrived and *how many times*, and a counter would let
    /// `Typed` and `NotTyped` stand in for one another -- which is precisely
    /// the confusion this whole change exists to remove.
    #[derive(Clone, Default)]
    struct Reported(Arc<Mutex<Vec<FillOutcome>>>);

    impl Reported {
        fn sink(&self) -> OutcomeSink {
            let seen = self.0.clone();
            Box::new(move |outcome| seen.lock().unwrap().push(outcome))
        }
        fn seen(&self) -> Vec<FillOutcome> {
            self.0.lock().unwrap().clone()
        }
    }

    /// For the tests that are about the flag and not about counting.
    fn ignored() -> OutcomeSink {
        Box::new(|_| {})
    }

    struct FakeUi {
        result: Result<bool, String>,
        calls: RefCell<u32>,
    }
    impl UiAutomationFiller for FakeUi {
        fn fill(&self, _hwnd: isize, _user: &str, _pass: &str) -> Result<bool, String> {
            *self.calls.borrow_mut() += 1;
            self.result.clone()
        }
    }

    struct FakeFallback {
        calls: RefCell<u32>,
        last_hwnd: RefCell<Option<isize>>,
        result: Result<(), String>,
    }
    impl FakeFallback {
        fn new() -> Self {
            Self {
                calls: RefCell::new(0),
                last_hwnd: RefCell::new(None),
                result: Ok(()),
            }
        }
        fn failing() -> Self {
            Self {
                calls: RefCell::new(0),
                last_hwnd: RefCell::new(None),
                result: Err("target window is not foreground".into()),
            }
        }
    }
    impl SendInputFiller for FakeFallback {
        fn fill(&self, hwnd: isize, _user: &str, _pass: &str) -> Result<(), String> {
            *self.calls.borrow_mut() += 1;
            *self.last_hwnd.borrow_mut() = Some(hwnd);
            self.result.clone()
        }

        /// These tests are about the *default* fill's fallback chain. Reaching
        /// the sequence path here would mean `Injector::fill` had dispatched
        /// somewhere it must never dispatch, so it fails loudly rather than
        /// recording a call some assertion might mistake for the real thing.
        fn fill_sequence(&self, _: isize, _: Plan, _: SequenceGuard) -> Result<(), String> {
            panic!("the default fill must not reach the sequence path")
        }
    }

    // -- one sequence at a time ---------------------------------------------

    fn a_plan() -> Plan {
        sequence::plan(
            &crate::key_sequence::parse("{USERNAME}{TAB}{PASSWORD}"),
            &sequence::Resolved {
                username: "work.account@contoso.com",
                password: "Zq7-tremulous-BADGER",
                totp: None,
                custom: vec![],
            },
        )
        .expect("plans")
    }

    /// A filler that **keeps** the guard instead of releasing it: what the
    /// real one looks like from the outside while its thread is still typing.
    struct StillTyping {
        held: RefCell<Vec<SequenceGuard>>,
        calls: RefCell<u32>,
    }
    impl StillTyping {
        fn new() -> Self {
            Self { held: RefCell::new(Vec::new()), calls: RefCell::new(0) }
        }
        /// Ends the "typing" run without saying what it did -- a thread that
        /// died, or one that simply never reported.
        fn finish(&self) {
            self.held.borrow_mut().clear();
        }

        /// Ends the "typing" run the way the real filler's thread does:
        /// reporting the outcome, then dropping the guard.
        fn finish_reporting(&self, outcome: FillOutcome) {
            let mut held = self.held.borrow_mut();
            for guard in held.iter_mut() {
                guard.report(outcome);
            }
            held.clear();
        }
    }
    impl SendInputFiller for StillTyping {
        fn fill(&self, _: isize, _: &str, _: &str) -> Result<(), String> {
            panic!("this test is about the sequence path")
        }
        fn fill_sequence(&self, _: isize, _: Plan, guard: SequenceGuard) -> Result<(), String> {
            *self.calls.borrow_mut() += 1;
            self.held.borrow_mut().push(guard);
            Ok(())
        }
    }

    /// **Two hotkey presses during one `{DELAY}` must not interleave two
    /// passwords into the same field.**
    ///
    /// Nothing guarded this: `fill_sequence` spawned a thread per call, both
    /// threads saw the same foreground window, and both passed every check.
    #[test]
    fn a_second_sequence_is_refused_while_the_first_is_still_typing() {
        let _serialised = sequence_test_lock();
        let injector = Injector { ui: FakeUi { result: Ok(false), calls: RefCell::new(0) }, fallback: StillTyping::new() };

        injector.fill_sequence(7, a_plan(), ignored()).expect("the first fill starts");

        let err = injector
            .fill_sequence(7, a_plan(), ignored())
            .expect_err("the second fill must be refused, not started");
        assert!(err.contains("already being typed"), "got: {err}");
        assert_eq!(
            *injector.fallback.calls.borrow(),
            1,
            "the refused fill still reached the filler"
        );
    }

    /// The other half: the refusal is temporary, not a one-shot latch. Without
    /// this, a guard that was never released would pass the test above.
    #[test]
    fn a_sequence_is_allowed_again_once_the_previous_one_finishes() {
        let _serialised = sequence_test_lock();
        let injector = Injector { ui: FakeUi { result: Ok(false), calls: RefCell::new(0) }, fallback: StillTyping::new() };

        injector.fill_sequence(7, a_plan(), ignored()).expect("the first fill starts");
        assert!(
            injector.fill_sequence(7, a_plan(), ignored()).is_err(),
            "expected a refusal"
        );

        injector.fallback.finish();

        injector
            .fill_sequence(7, a_plan(), ignored())
            .expect("a fill after the first finished");
        assert_eq!(*injector.fallback.calls.borrow(), 2, "the third fill did not reach the filler");
    }

    /// A typing thread that panics must not wedge auto-type for the session.
    /// `Drop` runs while unwinding, so the flag is released on the way out.
    #[test]
    fn a_panicking_sequence_releases_the_guard() {
        let _serialised = sequence_test_lock();

        // The panic below is the fixture, not a failure. Muting the hook for
        // its duration keeps a deliberate panic from printing a backtrace that
        // reads like a broken test in the suite's output.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let panicked = std::panic::catch_unwind(|| {
            let guard = SequenceGuard::acquire().expect("nothing else holds it");
            let _moved_onto_the_doomed_thread = guard;
            panic!("the typing thread died");
        });
        std::panic::set_hook(previous);
        assert!(panicked.is_err(), "the fixture did not actually panic");

        assert!(
            SequenceGuard::acquire().is_some(),
            "a panic left the guard held and auto-type wedged"
        );
    }

    // -- what the fill actually did -----------------------------------------

    /// **A run that performed every step typed.** The positive control for
    /// the two below: without it `outcome_of_a_run` could answer `Partial`
    /// unconditionally and no sequence would ever be counted again.
    #[test]
    fn a_run_that_finished_counts_as_typed() {
        assert_eq!(outcome_of_a_run(&Ok(())), FillOutcome::Typed);
    }

    /// **A run that stopped part-way did not type.**
    ///
    /// The user alt-tabbed after the username: `run` returns the "no longer in
    /// front, nothing further was typed" error, and the password never
    /// arrived. Answering `Typed` here is the defect this change exists to
    /// remove, transplanted one layer down.
    #[test]
    fn a_run_that_was_abandoned_counts_as_partial() {
        let abandoned = Err(
            "auto-type stopped: window 7 is no longer in front (after 2 of 5 steps).              Nothing further was typed."
                .to_string(),
        );
        assert_eq!(outcome_of_a_run(&abandoned), FillOutcome::Partial);
    }

    /// A keystroke that failed outright takes the same answer -- typing had
    /// begun and did not finish, whatever stopped it.
    #[test]
    fn a_run_whose_keystroke_failed_also_counts_as_partial() {
        assert_eq!(
            outcome_of_a_run(&Err("SendInput refused the keystroke".to_string())),
            FillOutcome::Partial
        );
    }

    /// Neither answer a run can give is one that a count may be taken from
    /// without the decision in [`crate::fill_stats::counts_as_a_fill`] --
    /// which is to say: finishing is the only thing that counts.
    #[test]
    fn only_a_finished_run_is_countable_as_a_fill() {
        use crate::fill_stats::counts_as_a_fill;
        assert!(counts_as_a_fill(outcome_of_a_run(&Ok(()))));
        assert!(!counts_as_a_fill(outcome_of_a_run(&Err("stopped".to_string()))));
    }

    /// **Silence means nothing was typed.**
    ///
    /// A guard that drops without a `report` is the shape a panicking typing
    /// thread leaves behind, and the shape of any future path that forgets.
    /// Defaulting the other way would credit a fill nobody can vouch for.
    #[test]
    fn a_guard_that_reports_nothing_says_nothing_was_typed() {
        let _serialised = sequence_test_lock();
        let reported = Reported::default();

        let mut guard = SequenceGuard::acquire().expect("nothing else holds it");
        guard.reports_to(reported.sink());
        assert!(reported.seen().is_empty(), "reported before the guard was released");
        drop(guard);

        assert_eq!(reported.seen(), vec![FillOutcome::NotTyped]);
    }

    /// The positive control for the test above: a guard that *was* told
    /// something reports that, and reports it once.
    #[test]
    fn a_guard_reports_what_it_was_told_exactly_once() {
        let _serialised = sequence_test_lock();
        let reported = Reported::default();

        let mut guard = SequenceGuard::acquire().expect("nothing else holds it");
        guard.reports_to(reported.sink());
        guard.report(FillOutcome::Typed);
        drop(guard);

        assert_eq!(reported.seen(), vec![FillOutcome::Typed]);
    }

    /// A run that started typing and then stopped reports the stop, not the
    /// start. Without this, a filler could report `Typed` the moment it began
    /// and nothing would notice -- which is the exact defect being fixed.
    #[test]
    fn a_later_report_replaces_an_earlier_one() {
        let _serialised = sequence_test_lock();
        let reported = Reported::default();

        let mut guard = SequenceGuard::acquire().expect("nothing else holds it");
        guard.reports_to(reported.sink());
        guard.report(FillOutcome::Typed);
        guard.report(FillOutcome::Partial);
        drop(guard);

        assert_eq!(reported.seen(), vec![FillOutcome::Partial]);
    }

    /// **A typing thread that panics part-way must not report a fill.**
    ///
    /// `panic = unwind`, so `Drop` runs on the way out and the sink is called;
    /// what it is called with is the default, because the `report` the thread
    /// was going to make never happened.
    #[test]
    fn a_panicking_typing_thread_reports_that_nothing_was_typed() {
        let _serialised = sequence_test_lock();
        let reported = Reported::default();

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let sink = reported.sink();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let mut guard = SequenceGuard::acquire().expect("nothing else holds it");
            guard.reports_to(sink);
            let _moved_onto_the_doomed_thread = guard;
            panic!("the typing thread died");
        }));
        std::panic::set_hook(previous);
        assert!(panicked.is_err(), "the fixture did not actually panic");

        assert_eq!(
            reported.seen(),
            vec![FillOutcome::NotTyped],
            "a panicking run reported something other than 'nothing was typed'"
        );
        assert!(
            SequenceGuard::acquire().is_some(),
            "a panic left the guard held and auto-type wedged"
        );
    }

    /// **A sequence refused because another is already typing typed nothing**,
    /// and says so. This is one of the two cases the old `Ok(()) =>
    /// record_fill` arm got wrong: the refusal never reaches a keyboard, and
    /// it used to be indistinguishable from a fill.
    #[test]
    fn a_sequence_refused_while_another_is_typing_reports_that_nothing_was_typed() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: StillTyping::new(),
        };
        let first = Reported::default();
        let second = Reported::default();

        injector.fill_sequence(7, a_plan(), first.sink()).expect("the first fill starts");
        injector
            .fill_sequence(7, a_plan(), second.sink())
            .expect_err("the second fill must be refused");

        assert_eq!(
            second.seen(),
            vec![FillOutcome::NotTyped],
            "the refused fill did not report that it typed nothing"
        );
    }

    /// **The UI thread does not learn the outcome, and does not wait for it.**
    ///
    /// `fill_sequence` has already returned `Ok(())` while the filler is still
    /// holding the guard -- exactly as the real one returns while its thread is
    /// still typing -- and nothing has been reported yet. The report arrives
    /// when the typing ends, from wherever it ended.
    #[test]
    fn the_outcome_arrives_when_the_typing_ends_not_when_the_call_returns() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: StillTyping::new(),
        };
        let reported = Reported::default();

        injector.fill_sequence(7, a_plan(), reported.sink()).expect("the fill starts");
        assert!(
            reported.seen().is_empty(),
            "the outcome was reported before the typing finished, so `Ok(())` still \
             means 'started' to whoever is counting"
        );

        injector.fallback.finish_reporting(FillOutcome::Typed);

        assert_eq!(reported.seen(), vec![FillOutcome::Typed]);
    }

    /// The other half of the test above with the outcome that must not count:
    /// a run abandoned mid-way reports `Partial`, and the same wiring carries
    /// it. Substituting `Typed` here is what the fixed defect looked like.
    #[test]
    fn an_abandoned_sequence_reports_that_it_only_partly_typed() {
        let _serialised = sequence_test_lock();
        let injector = Injector {
            ui: FakeUi { result: Ok(false), calls: RefCell::new(0) },
            fallback: StillTyping::new(),
        };
        let reported = Reported::default();

        injector.fill_sequence(7, a_plan(), reported.sink()).expect("the fill starts");
        injector.fallback.finish_reporting(FillOutcome::Partial);

        assert_eq!(reported.seen(), vec![FillOutcome::Partial]);
    }

    #[test]
    fn does_not_fall_back_when_ui_automation_succeeds() {
        let ui = FakeUi { result: Ok(true), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::new() };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.ui.calls.borrow(), 1);
        assert_eq!(*injector.fallback.calls.borrow(), 0);
    }

    #[test]
    fn falls_back_when_ui_automation_finds_no_fields() {
        let ui = FakeUi { result: Ok(false), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::new() };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.fallback.calls.borrow(), 1);
    }

    #[test]
    fn falls_back_when_ui_automation_errors() {
        let ui = FakeUi { result: Err("com failure".into()), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::new() };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.fallback.calls.borrow(), 1);
    }

    #[test]
    fn passes_the_target_hwnd_to_the_fallback() {
        // The fallback has to know which window it's meant to be typing into
        // so it can verify foreground; before this it typed blind.
        let ui = FakeUi { result: Ok(false), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::new() };

        injector.fill(4242, "u", "p").unwrap();

        assert_eq!(*injector.fallback.last_hwnd.borrow(), Some(4242));
    }

    #[test]
    fn surfaces_a_fallback_refusal_as_an_error() {
        // If the fallback refuses because the target isn't foreground, that
        // must reach the caller (which logs it), not be swallowed.
        let ui = FakeUi { result: Ok(false), calls: RefCell::new(0) };
        let injector = Injector { ui, fallback: FakeFallback::failing() };

        let err = injector.fill(1, "u", "p").unwrap_err();
        assert!(err.contains("not foreground"), "got: {err}");
    }
}

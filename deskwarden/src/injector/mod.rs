pub mod send_input;
pub mod sequence;
pub mod ui_automation;

use sequence::Plan;
use std::sync::atomic::{AtomicBool, Ordering};

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
pub struct SequenceGuard(());

impl SequenceGuard {
    /// `None` if a sequence is already running.
    pub fn acquire() -> Option<Self> {
        SEQUENCE_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(()))
    }
}

impl Drop for SequenceGuard {
    fn drop(&mut self) {
        SEQUENCE_IN_FLIGHT.store(false, Ordering::Release);
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
    pub fn fill_sequence(&self, hwnd: isize, plan: Plan) -> Result<(), String> {
        let Some(guard) = SequenceGuard::acquire() else {
            // Explicit, and not merely "let it fall out of scope": this is the
            // one exit from this function that holds a resolved password and
            // builds no `Plan`-owning thread to wipe it later.
            drop(plan);
            return Err(
                "an auto-type sequence is already being typed; wait for it to \
                 finish, or switch away to stop it, then press the hotkey again"
                    .to_string(),
            );
        };
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
    /// still waiting on.
    fn fill_sequence(&self, hwnd: isize, plan: Plan, guard: SequenceGuard) -> Result<(), String> {
        send_input::ensure_foreground(hwnd)?;
        std::thread::spawn(move || {
            // `guard` is moved onto this thread so the "already typing" flag
            // stays set for as long as this thread is really typing, and not
            // merely until the spawn returns. Naming it here keeps it captured
            // by the closure whether or not the body below ever mentions it.
            let _guard = guard;
            if let Err(e) = sequence::run(&send_input::RealKeyboard, hwnd, &plan) {
                sequence::Notifier::refused(&sequence::RealNotifier, &e);
            }
            // `plan` is dropped here, on this thread, wiping the password --
            // whether it finished, aborted, or failed. `_guard` is dropped
            // with it, releasing the flag; on a panic, unwinding does both.
        });
        Ok(())
    }
}

#[cfg(test)]
mod orchestration_tests {
    use super::*;
    use std::cell::RefCell;

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
        /// Ends the "typing" run.
        fn finish(&self) {
            self.held.borrow_mut().clear();
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

        injector.fill_sequence(7, a_plan()).expect("the first fill starts");

        let err = injector
            .fill_sequence(7, a_plan())
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

        injector.fill_sequence(7, a_plan()).expect("the first fill starts");
        assert!(injector.fill_sequence(7, a_plan()).is_err(), "expected a refusal");

        injector.fallback.finish();

        injector.fill_sequence(7, a_plan()).expect("a fill after the first finished");
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

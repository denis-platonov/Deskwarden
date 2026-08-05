pub mod send_input;
pub mod sequence;
pub mod ui_automation;

use sequence::Plan;

pub trait UiAutomationFiller {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String>;
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
    /// **The default body refuses.** A `SendInputFiller` that has not opted in
    /// must not silently succeed at typing nothing: the test fillers in this
    /// crate exist to prove the fallback is *not* reached, and a default that
    /// returned `Ok(())` would let a broken wiring look like a working one.
    fn fill_sequence(&self, hwnd: isize, plan: Plan) -> Result<(), String> {
        drop(plan);
        Err(format!("this filler cannot type an auto-type sequence into {hwnd}"))
    }
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
    pub fn fill_sequence(&self, hwnd: isize, plan: Plan) -> Result<(), String> {
        self.fallback.fill_sequence(hwnd, plan)
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
    fn fill_sequence(&self, hwnd: isize, plan: Plan) -> Result<(), String> {
        send_input::ensure_foreground(hwnd)?;
        std::thread::spawn(move || {
            if let Err(e) = sequence::run(&send_input::RealKeyboard, hwnd, &plan) {
                sequence::Notifier::refused(&sequence::RealNotifier, &e);
            }
            // `plan` is dropped here, on this thread, wiping the password --
            // whether it finished, aborted, or failed.
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

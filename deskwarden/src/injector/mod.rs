pub mod send_input;
pub mod ui_automation;

pub trait UiAutomationFiller {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String>;
}

pub trait SendInputFiller {
    /// `hwnd` is the window the caller *intends* to fill. Implementations must
    /// verify it actually has foreground before typing: `SendInput` goes to
    /// whatever holds keyboard focus, which is not necessarily the window we
    /// matched (see `send_input::ensure_foreground`).
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String>;
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

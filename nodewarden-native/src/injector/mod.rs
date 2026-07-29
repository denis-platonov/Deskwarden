pub mod send_input;
pub mod ui_automation;

pub trait UiAutomationFiller {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String>;
}

pub trait SendInputFiller {
    fn fill(&self, user: &str, pass: &str) -> Result<(), String>;
}

pub struct Injector<A: UiAutomationFiller, B: SendInputFiller> {
    pub ui: A,
    pub fallback: B,
}

impl<A: UiAutomationFiller, B: SendInputFiller> Injector<A, B> {
    pub fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String> {
        match self.ui.fill(hwnd, user, pass) {
            Ok(true) => Ok(()),
            Ok(false) => self.fallback.fill(user, pass),
            Err(_) => self.fallback.fill(user, pass),
        }
    }
}

pub struct RealUiAutomation;
impl UiAutomationFiller for RealUiAutomation {
    fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<bool, String> {
        ui_automation::fill_via_ui_automation(hwnd, user, pass).map_err(|e| e.to_string())
    }
}

pub struct RealSendInput;
impl SendInputFiller for RealSendInput {
    fn fill(&self, user: &str, pass: &str) -> Result<(), String> {
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
    }
    impl SendInputFiller for FakeFallback {
        fn fill(&self, _user: &str, _pass: &str) -> Result<(), String> {
            *self.calls.borrow_mut() += 1;
            Ok(())
        }
    }

    #[test]
    fn does_not_fall_back_when_ui_automation_succeeds() {
        let ui = FakeUi { result: Ok(true), calls: RefCell::new(0) };
        let fallback = FakeFallback { calls: RefCell::new(0) };
        let injector = Injector { ui, fallback };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.ui.calls.borrow(), 1);
        assert_eq!(*injector.fallback.calls.borrow(), 0);
    }

    #[test]
    fn falls_back_when_ui_automation_finds_no_fields() {
        let ui = FakeUi { result: Ok(false), calls: RefCell::new(0) };
        let fallback = FakeFallback { calls: RefCell::new(0) };
        let injector = Injector { ui, fallback };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.fallback.calls.borrow(), 1);
    }

    #[test]
    fn falls_back_when_ui_automation_errors() {
        let ui = FakeUi { result: Err("com failure".into()), calls: RefCell::new(0) };
        let fallback = FakeFallback { calls: RefCell::new(0) };
        let injector = Injector { ui, fallback };

        injector.fill(1, "u", "p").unwrap();

        assert_eq!(*injector.fallback.calls.borrow(), 1);
    }
}

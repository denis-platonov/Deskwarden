use crate::dispatch::may_type_into;
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
    VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

/// Pause between individual simulated keystrokes.
///
/// `SendInput` delivers keystrokes far faster than a human types, and controls
/// that do per-character work on the UI thread (the game launchers this
/// fallback exists for, in particular) can drop characters under that rate. A
/// few milliseconds per character is imperceptible to the user and
/// substantially reduces the risk. This is a mitigation, not a guarantee:
/// detecting and recovering from a partially delivered batch is out of scope.
const INTER_KEYSTROKE_DELAY: Duration = Duration::from_millis(3);

/// How long to let a foreground transition settle before re-checking it.
const FOREGROUND_SETTLE: Duration = Duration::from_millis(120);

/// How many times to retry handing focus to `hwnd` before giving up. Windows
/// does not synchronously return focus to the previous window when ours
/// closes -- observed live against Epic Games Launcher, a single 120ms
/// settle window was sometimes not enough (heavier, Chromium-based apps in
/// particular can take longer to actually repaint as foreground), and one
/// miss meant the whole fill was refused even though the target window
/// showed up as foreground moments later.
const FOREGROUND_RETRY_ATTEMPTS: u32 = 5;

/// Makes sure `hwnd` is the foreground window before anything is typed.
///
/// `SendInput` types into whatever currently holds keyboard focus, with no
/// reference to the window we meant to fill. For the `hotkey` trigger that's
/// usually harmless (the user just clicked the field), but for `prompt` and
/// `auto` -- especially in the moments right after our overlay window closes
/// and focus is being handed back -- focus placement is not guaranteed. Typing
/// a password into the wrong window is the worst failure this app can produce,
/// so a mismatch after every retry is reported as an error and nothing is
/// typed.
pub fn ensure_foreground(hwnd: isize) -> Result<(), String> {
    let current = unsafe { GetForegroundWindow() }.0 as isize;
    if may_type_into(hwnd, current) {
        return Ok(());
    }

    for _ in 0..FOREGROUND_RETRY_ATTEMPTS {
        // Only call SetForegroundWindow when it's actually needed: calling it
        // on a window that is already foreground can reset focus to that
        // window's default control, undoing the field the user just clicked
        // into.
        unsafe {
            let _ = SetForegroundWindow(HWND(hwnd as *mut core::ffi::c_void));
        }
        std::thread::sleep(FOREGROUND_SETTLE);

        let current = unsafe { GetForegroundWindow() }.0 as isize;
        if may_type_into(hwnd, current) {
            return Ok(());
        }
    }

    let current = unsafe { GetForegroundWindow() }.0 as isize;
    Err(format!(
        "refusing to type: target window {hwnd} is not foreground (foreground is {current}) \
         after {FOREGROUND_RETRY_ATTEMPTS} attempts"
    ))
}

/// Types `username`, presses Tab, then types `password`, into whatever
/// control currently has keyboard focus, using simulated raw keystrokes via
/// `SendInput`. This is the fallback injector for windows that don't expose
/// a usable UI Automation tree (see `ui_automation.rs`).
pub fn fill_via_send_input(username: &str, password: &str) -> windows::core::Result<()> {
    type_text(username)?;
    press_tab()?;
    type_text(password)?;
    Ok(())
}

fn type_text(text: &str) -> windows::core::Result<()> {
    for ch in text.encode_utf16() {
        send_unicode_char(ch)?;
        std::thread::sleep(INTER_KEYSTROKE_DELAY);
    }
    Ok(())
}

fn send_unicode_char(ch: u16) -> windows::core::Result<()> {
    let mut down = keybd_input(0, KEYEVENTF_UNICODE);
    down.wScan = ch;
    let mut up = keybd_input(0, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
    up.wScan = ch;

    send(&[to_input(down), to_input(up)])
}

fn press_tab() -> windows::core::Result<()> {
    let down = keybd_input(VK_TAB.0, KEYBD_EVENT_FLAGS(0));
    let up = keybd_input(VK_TAB.0, KEYEVENTF_KEYUP);
    send(&[to_input(down), to_input(up)])
}

fn keybd_input(vk: u16, flags: KEYBD_EVENT_FLAGS) -> KEYBDINPUT {
    KEYBDINPUT {
        wVk: VIRTUAL_KEY(vk),
        wScan: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    }
}

fn to_input(ki: KEYBDINPUT) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }
}

fn send(inputs: &[INPUT]) -> windows::core::Result<()> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(windows::core::Error::from_win32());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The auto-type sequence keyboard
// ---------------------------------------------------------------------------

/// The real [`crate::injector::sequence::Keyboard`]: the one place a planned
/// [`Step`](crate::injector::sequence::Step) becomes actual synthetic input.
///
/// Deliberately tiny. Every decision -- what to type, in what order, how long
/// a burst may be, when to give up -- lives in `sequence.rs` where it is a
/// pure function under test. What is left here is four bodies that each do one
/// Win32 call, so the part no test can reach is also the part with nothing in
/// it to get wrong.
pub struct RealKeyboard;

impl crate::injector::sequence::Keyboard for RealKeyboard {
    /// **A passive check. It does not call `SetForegroundWindow`.**
    ///
    /// [`ensure_foreground`] above *restores* focus, which is right once, at
    /// the start of a fill, when our own overlay has just closed and Windows
    /// has not yet handed focus back. It is exactly wrong in the middle of a
    /// sequence: if the user alt-tabbed away during a `{DELAY}`, yanking their
    /// window back so we can finish typing a password into it is the opposite
    /// of the thing this check exists to prevent. Mid-sequence the only honest
    /// question is "is it still there", and the only honest answer to "no" is
    /// to stop.
    fn holds_foreground(&self, hwnd: isize) -> bool {
        let current = unsafe { GetForegroundWindow() }.0 as isize;
        may_type_into(hwnd, current)
    }

    fn type_text(&self, text: &str, rate: Duration) -> Result<(), String> {
        for ch in text.encode_utf16() {
            send_unicode_char(ch).map_err(|e| e.to_string())?;
            std::thread::sleep(rate);
        }
        Ok(())
    }

    fn press_key(
        &self,
        key: &'static crate::key_sequence::KeyDef,
        mods: crate::injector::sequence::ModSet,
    ) -> Result<(), String> {
        let Some((vk, extended)) = crate::injector::sequence::virtual_key(key.token) else {
            // Unreachable: `plan` refuses a key with no code before any of
            // this runs. Reported rather than ignored so that if the two ever
            // disagree it is a failure and not a silent no-op.
            return Err(format!("no key code for {{{}}}", key.token));
        };

        // Modifiers down, key down, key up, modifiers up -- **in one
        // `SendInput` batch**. A stuck Ctrl is a real failure mode: it leaves
        // the user's keyboard in a state where every subsequent keystroke is a
        // shortcut, and they have to work out to tap Ctrl to clear it. Sending
        // the whole chord as one batch means there is no window between the
        // down and the up in which a failure or an early return could strand a
        // modifier held.
        let extended_flag =
            if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
        let mut inputs: Vec<INPUT> = Vec::with_capacity(8);
        let held = modifier_keys(mods);
        for vk in &held {
            inputs.push(to_input(keybd_input(*vk, KEYBD_EVENT_FLAGS(0))));
        }
        inputs.push(to_input(keybd_input(vk, extended_flag)));
        inputs.push(to_input(keybd_input(vk, extended_flag | KEYEVENTF_KEYUP)));
        for vk in held.iter().rev() {
            inputs.push(to_input(keybd_input(*vk, KEYEVENTF_KEYUP)));
        }
        send(&inputs).map_err(|e| e.to_string())
    }

    fn wait(&self, how_long: Duration) {
        std::thread::sleep(how_long);
    }
}

/// The virtual keys held for `mods`, in the order they are pressed. Released
/// in the reverse order, which is what a real keyboard does and what
/// applications watching for chords expect.
fn modifier_keys(mods: crate::injector::sequence::ModSet) -> Vec<u16> {
    let mut out = Vec::with_capacity(3);
    if mods.ctrl {
        out.push(VK_CONTROL.0);
    }
    if mods.alt {
        out.push(VK_MENU.0);
    }
    if mods.shift {
        out.push(VK_SHIFT.0);
    }
    out
}

#[cfg(test)]
mod modifier_tests {
    use super::*;
    use crate::injector::sequence::ModSet;

    /// The order is what makes the release order the reverse of the press
    /// order rather than an accident of which `if` came first.
    #[test]
    fn a_full_chord_presses_ctrl_then_alt_then_shift() {
        assert_eq!(
            modifier_keys(ModSet { ctrl: true, alt: true, shift: true }),
            vec![VK_CONTROL.0, VK_MENU.0, VK_SHIFT.0]
        );
    }

    #[test]
    fn no_modifiers_holds_no_keys() {
        assert!(modifier_keys(ModSet::default()).is_empty());
    }

    #[test]
    fn each_modifier_maps_to_its_own_key() {
        assert_eq!(modifier_keys(ModSet { shift: true, ..Default::default() }), vec![VK_SHIFT.0]);
        assert_eq!(modifier_keys(ModSet { ctrl: true, ..Default::default() }), vec![VK_CONTROL.0]);
        assert_eq!(modifier_keys(ModSet { alt: true, ..Default::default() }), vec![VK_MENU.0]);
    }
}

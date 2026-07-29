use crate::dispatch::may_type_into;
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_TAB,
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

/// Makes sure `hwnd` is the foreground window before anything is typed.
///
/// `SendInput` types into whatever currently holds keyboard focus, with no
/// reference to the window we meant to fill. For the `hotkey` trigger that's
/// usually harmless (the user just clicked the field), but for `prompt` and
/// `auto` -- especially in the moments right after our overlay window closes
/// and focus is being handed back -- focus placement is not guaranteed. Typing
/// a password into the wrong window is the worst failure this app can produce,
/// so a mismatch is reported as an error and nothing is typed.
pub fn ensure_foreground(hwnd: isize) -> Result<(), String> {
    let current = unsafe { GetForegroundWindow() }.0 as isize;
    if may_type_into(hwnd, current) {
        return Ok(());
    }

    // Only call SetForegroundWindow when it's actually needed: calling it on a
    // window that is already foreground can reset focus to that window's
    // default control, undoing the field the user just clicked into.
    unsafe {
        let _ = SetForegroundWindow(HWND(hwnd as *mut core::ffi::c_void));
    }
    std::thread::sleep(FOREGROUND_SETTLE);

    let current = unsafe { GetForegroundWindow() }.0 as isize;
    if may_type_into(hwnd, current) {
        Ok(())
    } else {
        Err(format!(
            "refusing to type: target window {hwnd} is not foreground (foreground is {current})"
        ))
    }
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

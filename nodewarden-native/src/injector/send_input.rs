use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_TAB,
};

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

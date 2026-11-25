use crate::dispatch::may_type_into;
use std::time::Duration;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

// The pause between individual simulated keystrokes used to be a second 3ms
// constant here, next to `sequence::DEFAULT_RATE`. There is now one typing path
// and therefore one constant: see `sequence::DEFAULT_RATE`, which carries the
// reason (controls that do per-character work on their own UI thread drop
// characters delivered faster than that) and is the rate `plan` stamps onto
// every `Step::Text` a default fill produces.

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
        log::info!(
            "typing into window {hwnd}: it was already in front; keyboard focus is in {}",
            describe_keyboard_focus()
        );
        return Ok(());
    }

    for attempt in 1..=FOREGROUND_RETRY_ATTEMPTS {
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
            log::info!(
                "typing into window {hwnd}: brought it to the front on attempt {attempt}; \
                 keyboard focus is in {}",
                describe_keyboard_focus()
            );
            return Ok(());
        }
    }

    let current = unsafe { GetForegroundWindow() }.0 as isize;
    Err(format!(
        "refusing to type: target window {hwnd} is not foreground (foreground is {current}) \
         after {FOREGROUND_RETRY_ATTEMPTS} attempts"
    ))
}

/// **Where keystrokes sent now would land**, for the log: the window class and
/// owning process id of the foreground thread's keyboard-focus window.
///
/// Diagnostic only, and it names no text: a class name and a number. It exists
/// because a window can be in front while its keyboard focus is not in the
/// field the user was in -- a Microsoft account dialog is an `explorer.exe`
/// frame around a `WWAHost.exe` page, and the frame can hold the focus itself
/// -- and "typed, but nothing appeared" is otherwise invisible in the log.
pub fn describe_keyboard_focus() -> String {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };
    let foreground = unsafe { GetForegroundWindow() };
    let thread = unsafe { GetWindowThreadProcessId(foreground, None) };
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a live, correctly sized GUITHREADINFO; a thread that
    // has gone away makes the call fail, which is reported rather than read.
    if unsafe { GetGUIThreadInfo(thread, &mut info) }.is_err() {
        return "an unknown window (GetGUIThreadInfo failed)".to_string();
    }
    if info.hwndFocus.0.is_null() {
        return "no window (the foreground thread has no focus window)".to_string();
    }
    let mut buffer = [0u16; 128];
    // SAFETY: the buffer is a live stack array whose length is handed over.
    let copied = unsafe { GetClassNameW(info.hwndFocus, &mut buffer) };
    let class = String::from_utf16_lossy(&buffer[..copied.max(0) as usize]);
    let mut pid = 0u32;
    // SAFETY: `pid` is a live u32 the call writes through.
    unsafe { GetWindowThreadProcessId(info.hwndFocus, Some(&mut pid)) };
    format!("a {class:?} window of process {pid} (focus window {:?})", info.hwndFocus.0)
}

// `fill_via_send_input`, its `type_text` and its `press_tab` used to live here:
// a straight line that typed the username, pressed Tab and typed the password
// with one foreground check in front of the lot and none inside it. The default
// fill now goes through `injector::default_plan` and `sequence::run` like every
// other fill, which re-verifies the foreground before each burst, so the three
// of them are gone rather than left behind as a second way to type that nothing
// calls and no test covers.

fn send_unicode_char(ch: u16) -> windows::core::Result<()> {
    let mut down = keybd_input(0, KEYEVENTF_UNICODE);
    down.wScan = ch;
    let mut up = keybd_input(0, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
    up.wScan = ch;

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
    ///
    /// **The same app counts, and a moment of nothing in front is waited
    /// out.** An app's own pop-up -- Chromium's autocomplete list under a
    /// field it has just been typed into, for one -- can take the foreground
    /// from the app's main window for a moment, and Windows reports no
    /// foreground window at all part way through an activation change. Either
    /// stopped a fill half way into apps the user never touched (mabl, after
    /// the username and the Tab). So a window owned by the same top-level
    /// window or run by the same process still holds the foreground -- the
    /// app the user chose is still the one receiving the keys -- and anything
    /// else is re-checked for [`SETTLE`] before the run is abandoned. Nothing is
    /// typed while another app is in front: the wait is before the step.
    fn holds_foreground(&self, hwnd: isize) -> bool {
        let deadline = std::time::Instant::now() + SETTLE;
        loop {
            let current = unsafe { GetForegroundWindow() }.0 as isize;
            if may_type_into(hwnd, current) {
                return true;
            }
            if current != 0 && same_app(hwnd, current) {
                log::info!(
                    "auto-type: {} is in front of {}, which is the same app, so typing goes on",
                    describe_window(current),
                    describe_window(hwnd)
                );
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn describe(&self, hwnd: isize) -> String {
        describe_window(hwnd)
    }

    fn in_front_instead(&self, hwnd: isize) -> Option<String> {
        let current = unsafe { GetForegroundWindow() }.0 as isize;
        (current != 0 && current != hwnd).then(|| describe_window(current))
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

/// How long a run waits for the foreground to come back to the target before
/// it gives up: long enough for an activation change or a closing pop-up,
/// short enough that a user who really switched away sees the run stop.
const SETTLE: Duration = Duration::from_millis(500);

/// Whether `other` belongs to the same app as `target`: the same top-level
/// owner (an owned pop-up), or the same process.
fn same_app(target: isize, other: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetWindowThreadProcessId, GA_ROOTOWNER};
    let (t, o) = (HWND(target as *mut core::ffi::c_void), HWND(other as *mut core::ffi::c_void));
    unsafe {
        let (rt, ro) = (GetAncestor(t, GA_ROOTOWNER), GetAncestor(o, GA_ROOTOWNER));
        if !rt.is_invalid() && rt == ro {
            return true;
        }
        let (mut pt, mut po) = (0u32, 0u32);
        GetWindowThreadProcessId(t, Some(&mut pt));
        GetWindowThreadProcessId(o, Some(&mut po));
        pt != 0 && pt == po
    }
}

/// A window as a person would name it: its title and program, the title
/// alone or the program alone when one is missing, and its number only when
/// both are.
pub(crate) fn describe_window(hwnd: isize) -> String {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let title = crate::window_watch::window_title(hwnd);
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut core::ffi::c_void), Some(&mut pid)) };
    let exe = crate::window_watch::process_name_for_pid(pid);
    name_window(hwnd, title.trim(), exe.as_deref())
}

/// [`describe_window`]'s wording, apart from the Win32 reads. Pure.
fn name_window(hwnd: isize, title: &str, exe: Option<&str>) -> String {
    match (title.is_empty(), exe) {
        (false, Some(exe)) => format!("\u{201c}{title}\u{201d} ({exe})"),
        (false, None) => format!("\u{201c}{title}\u{201d}"),
        (true, Some(exe)) => exe.to_string(),
        (true, None) => hwnd.to_string(),
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

#[cfg(test)]
mod naming_tests {
    use super::name_window;

    #[test]
    fn a_window_is_named_by_its_title_and_program_and_by_its_number_only_as_a_last_resort() {
        assert_eq!(name_window(7, "mabl", Some("mabl.exe")), "\u{201c}mabl\u{201d} (mabl.exe)");
        assert_eq!(name_window(7, "mabl", None), "\u{201c}mabl\u{201d}");
        assert_eq!(name_window(7, "", Some("mabl.exe")), "mabl.exe");
        assert_eq!(name_window(7, "", None), "7");
    }
}

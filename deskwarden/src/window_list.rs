//! Enumerates the windows a user could plausibly point at as "that app" --
//! for the "Add app..." picker (`picker_ui::run_picker`), which used to list
//! every process on the system by executable name, most of which have no
//! window at all and none of which show what's actually on screen.

use crate::window_watch;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
};

pub struct WindowInfo {
    pub hwnd: isize,
    pub pid: u32,
    /// Full path, for loading the exe's icon (`icon::extract_small_icon`).
    pub exe_path: String,
    pub exe_name: String,
    pub title: String,
}

/// Lists visible, titled top-level windows, excluding `exclude_pid` (so the
/// picker doesn't offer to match itself) and anything without a real,
/// user-visible presence: invisible windows, `WS_EX_TOOLWINDOW` helper
/// windows, and DWM-cloaked windows (on modern Windows, `IsWindowVisible`
/// can report `true` for background UWP/shell surfaces that were never
/// actually shown -- listing those would just be unclickable noise).
pub fn list_windows(exclude_pid: u32) -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut out as *mut Vec<WindowInfo> as isize));
    }
    out.retain(|w| w.pid != exclude_pid);
    out
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // Every early return below is "skip this window, keep enumerating", not
    // "stop" -- BOOL(1) either way.
    const CONTINUE: BOOL = BOOL(1);

    if !IsWindowVisible(hwnd).as_bool() {
        return CONTINUE;
    }

    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return CONTINUE;
    }

    let mut cloaked: u32 = 0;
    let _ = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut core::ffi::c_void,
        std::mem::size_of::<u32>() as u32,
    );
    if cloaked != 0 {
        return CONTINUE;
    }

    let len = GetWindowTextLengthW(hwnd);
    if len == 0 {
        return CONTINUE;
    }
    let mut buffer = vec![0u16; len as usize + 1];
    let copied = GetWindowTextW(hwnd, &mut buffer);
    if copied == 0 {
        return CONTINUE;
    }
    let title = String::from_utf16_lossy(&buffer[..copied as usize]);

    let mut owner_pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut owner_pid));
    if owner_pid == 0 {
        return CONTINUE;
    }

    let Some(owner_exe) = window_watch::process_name_for_pid(owner_pid) else {
        return CONTINUE;
    };

    // Same attribution the foreground watcher does, and for the same reason:
    // every Store/UWP window in this list is owned by
    // `ApplicationFrameHost.exe`, so without this the picker offers the user
    // the host as "that app" -- which is precisely the entry that then
    // matches every other Store app. A window whose hosted app cannot be
    // identified is dropped rather than listed under the host's name: an
    // unpickable row is better than a row that saves the wrong thing.
    let hwnd_value = hwnd.0 as isize;
    let window_watch::Attribution::Attributed { pid, exe_name } =
        window_watch::resolve_window_attribution(hwnd_value, owner_pid, &owner_exe)
    else {
        return CONTINUE;
    };

    // Resolved from the ATTRIBUTED pid, not the owner's: for a hosted app the
    // host's image path would load the wrong icon (and give the picker a path
    // that has nothing to do with the app the user clicked).
    let Some(exe_path) = window_watch::process_image_path_for_pid(pid) else {
        return CONTINUE;
    };

    let out = &mut *(lparam.0 as *mut Vec<WindowInfo>);
    out.push(WindowInfo {
        hwnd: hwnd_value,
        pid,
        exe_path,
        exe_name,
        title,
    });

    CONTINUE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live half. `list_windows` walks the real desktop, so what it
    /// returns depends on the machine -- which is why the assertion below is
    /// paired with a control that does not.
    ///
    /// The bug this guards: the picker offering `ApplicationFrameHost.exe` as
    /// "that app" for a Store window, which is the entry that then matches
    /// every Store app. Reverting `enum_proc` to name the owning process
    /// makes every open Store window a host row here.
    #[test]
    fn no_listed_window_is_offered_under_a_window_hosts_name() {
        let hosts: Vec<&str> = list_windows(0)
            .iter()
            .filter(|w| window_watch::is_host_process(&w.exe_name))
            .map(|_| "host row")
            .collect();
        assert!(
            hosts.is_empty(),
            "the picker would offer a window host as an app: {hosts:?}"
        );
    }

    /// The control for the test above, which is vacuous on a machine with no
    /// Store app open (and on a session with no windows at all).
    ///
    /// It drives the SAME predicate the filter uses, rather than re-asserting
    /// a planted string: if `is_host_process` answered `false` for everything
    /// -- the one mutation that would make the live test pass no matter what
    /// `enum_proc` does -- this fails.
    #[test]
    fn the_filter_that_test_uses_really_does_recognise_the_frame_host() {
        assert!(window_watch::is_host_process("ApplicationFrameHost.exe"));
        assert!(!window_watch::is_host_process("Speedtest.exe"));
    }
}

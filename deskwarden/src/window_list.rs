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

    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return CONTINUE;
    }

    let Some(exe_path) = window_watch::process_image_path_for_pid(pid) else {
        return CONTINUE;
    };
    let exe_name = exe_path
        .rsplit('\\')
        .next()
        .unwrap_or(&exe_path)
        .to_string();

    let out = &mut *(lparam.0 as *mut Vec<WindowInfo>);
    out.push(WindowInfo {
        hwnd: hwnd.0 as isize,
        pid,
        exe_path,
        exe_name,
        title,
    });

    CONTINUE
}

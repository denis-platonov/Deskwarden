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
    /// Whether this window's **top-level owner** is a window host
    /// ([`window_watch::is_host_process`]) while `exe_name` names something
    /// else -- a Microsoft Store / UWP app presenting inside an
    /// `ApplicationFrameHost.exe` frame.
    ///
    /// It is the one thing about a row that says whether its title may ever be
    /// matched on (`picker_ui::app_match_for` copies it into
    /// `AppMatch::hosted`, and `MatchEngine::rebuild` files a title only for a
    /// match that carries it). It has to be recorded HERE, at capture time,
    /// because it is the only place both names are known: by the time the row
    /// reaches the picker, `exe_name` is the attributed app's and the owner's
    /// name is gone.
    ///
    /// `false` for an unattributable host frame as well as for an ordinary
    /// window: such a row is listed under the host's own name and the picker
    /// refuses to save it at all (`picker_ui::host_process_refusal`), so
    /// "hosted" here means specifically "a host frame with a real app resolved
    /// inside it", which is the only row whose title identifies anything.
    pub hosted: bool,
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
    // matches every other Store app.
    //
    // **An unattributable host window is still LISTED, under the host's
    // name.** That was measured, not assumed: on the reporting machine both
    // live Store frames (titled "Speedtest" and "Settings", both pid 12472)
    // had no `Windows.UI.Core.CoreWindow` child at all -- a minimised UWP app
    // is suspended and its CoreWindow goes with it -- so dropping the row
    // makes the app the user is looking at simply vanish from the picker with
    // no explanation. The row stays, and `picker_ui::host_process_refusal`
    // declines the save and says why and what to do instead. Silently
    // removing the row would be the same "silent no-op" this window has twice
    // been patched to stop doing.
    // Whether the OWNER is a host is read before the attribution result
    // replaces the owner's name, because that pair -- "a host frame, with a
    // real app resolved inside it" -- is what makes the row's title an
    // identity worth saving. See `WindowInfo::hosted`.
    let owner_is_host = window_watch::is_host_process(&owner_exe);
    let hwnd_value = hwnd.0 as isize;
    let (pid, exe_name, hosted) =
        match window_watch::resolve_window_attribution(hwnd_value, owner_pid, &owner_exe) {
            window_watch::Attribution::Attributed { pid, exe_name } => {
                (pid, exe_name, owner_is_host)
            }
            // The HOST's pid, because there is no other one to give: the whole
            // of `UnresolvedHost` is "no child process could be identified".
            // So every unattributable row on a machine shares one pid -- both
            // Store frames on the reporting machine were 12472 -- and `pid` is
            // therefore NOT a key for these rows.
            // `picker_ui::selected_window` keys the picker's selection by
            // `hwnd` for exactly that reason.
            // `hosted: false`, because nothing was resolved inside the frame:
            // the row is listed under the host's own name and the picker
            // refuses to save it, so there is no app for its title to name.
            window_watch::Attribution::UnresolvedHost { host } => (owner_pid, host, false),
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
        hosted,
    });

    CONTINUE
}

/// Source guard for the one line of glue that cannot be reached from a test:
/// that `enum_proc` names each row by ATTRIBUTION rather than by the process
/// that owns the window.
///
/// `enum_proc` is an `extern "system"` callback driven by `EnumWindows`, and
/// its only observable output (`list_windows`) depends entirely on what the
/// machine happens to be showing -- and, since an unattributable host window
/// is deliberately still listed under the host's name, a live assertion
/// cannot tell the fixed build from the reverted one at all on a desktop
/// whose Store apps are all suspended (which is what the reporting machine's
/// were). So the revert is pinned by source position instead.
///
/// What it can and cannot see: it pins that the call is there, exactly once.
/// It cannot see a call whose result is thrown away -- that is visible in any
/// diff touching these lines. What it guards is the revert.
#[cfg(test)]
mod attribution_wiring_tests {
    // SPLIT ACROSS TWO LITERALS, on ONE line: `include_str!` pulls this module
    // in too, so a whole needle would match its own declaration, and a needle
    // containing a newline would pass on an LF checkout and fail on a CRLF one
    // (this repo has both).
    const CALL: &str = concat!("resolve_window_attribution", "(");
    /// The other piece of glue only this callback can perform: reading whether
    /// the window's OWNER is a host, which is what makes the row's title
    /// matchable at all (`WindowInfo::hosted`, review 31's Important 1). The
    /// mutation is a one-liner -- `let owner_is_host = false;` compiles, every
    /// row records `hosted: false`, the picker stops recording a title for
    /// Store apps, and a suspended one can never be matched again. No test can
    /// see it: the picker's own tests build `WindowInfo` by hand.
    const OWNER_HOST_CHECK: &str = concat!("is_host_process", "(&owner_exe)");

    fn source() -> &'static str {
        include_str!("window_list.rs")
    }

    /// The same counting the real assertion uses, so the positive control
    /// drives this code rather than a re-implementation of it.
    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn the_counter_finds_a_call_that_is_really_there() {
        let planted = concat!("match resolve_window_attribution", "(h, p, e) {");
        assert_eq!(occurrences(planted, CALL), 1, "planted: {planted}");
        assert_eq!(occurrences("nothing here", CALL), 0);
        let planted_check = concat!("let owner_is_host = window_watch::is_host_process", "(&owner_exe);");
        assert_eq!(occurrences(planted_check, OWNER_HOST_CHECK), 1, "planted: {planted_check}");
        assert_eq!(occurrences("nothing here", OWNER_HOST_CHECK), 0);
    }

    #[test]
    fn each_row_records_whether_a_host_frame_owned_it() {
        assert_eq!(
            occurrences(source(), OWNER_HOST_CHECK),
            1,
            "expected {OWNER_HOST_CHECK:?} exactly once in window_list.rs -- the row's `hosted` \
             flag. Zero means every row is recorded as not hosted, the picker stops capturing a \
             title for the Microsoft Store apps that are the only reason titles are stored at \
             all, and a suspended Store app can never be matched again"
        );
    }

    #[test]
    fn every_picker_row_is_named_by_attribution_not_by_the_owning_process() {
        assert_eq!(
            occurrences(source(), CALL),
            1,
            "expected {CALL:?} exactly once in window_list.rs -- `enum_proc`'s attribution of the \
             row it is about to push. Zero means the picker went back to naming each row after \
             the process that owns the window, and every Microsoft Store app would again be \
             offered as ApplicationFrameHost.exe: one saved match that fires on all of them"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live, and deliberately weak: it asserts only that a host row, IF the
    /// desktop produces one, is one the picker will refuse to save (see
    /// `picker_ui::host_process_refusal`). It cannot distinguish the fix from
    /// the revert -- `every_picker_row_is_named_by_attribution_not_by_the_
    /// owning_process` above is what does that.
    ///
    /// It is here because it is the only thing that runs `list_windows`
    /// against a real desktop at all, and a panic or a hang in the enumeration
    /// (a bad `LPARAM` cast, an unterminated walk) has no other test.
    #[test]
    fn enumerating_the_real_desktop_yields_rows_the_save_gate_can_answer_for() {
        for w in list_windows(0) {
            assert!(!w.exe_name.is_empty(), "a row with no process name: {}", w.title);
            assert!(!w.exe_name.contains('\\'), "expected a file name: {}", w.exe_name);
        }
    }

    /// The predicate the picker's refusal turns on, exercised directly --
    /// the one mutation that would make every host-related assertion in this
    /// crate vacuous is `is_host_process` answering `false` for everything.
    #[test]
    fn the_frame_host_is_recognised_and_a_real_app_is_not() {
        assert!(window_watch::is_host_process("ApplicationFrameHost.exe"));
        assert!(!window_watch::is_host_process("Speedtest.exe"));
    }
}

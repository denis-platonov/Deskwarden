use std::cell::RefCell;
use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, TranslateMessage,
    CHILDID_SELF, EVENT_SYSTEM_FOREGROUND, MSG, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
};

pub struct ForegroundEvent {
    pub hwnd: isize,
    pub pid: u32,
    pub exe_name: String,
}

thread_local! {
    static CALLBACK: RefCell<Option<Box<dyn FnMut(ForegroundEvent)>>> = RefCell::new(None);
}

pub fn watch_foreground_windows(
    callback: impl FnMut(ForegroundEvent) + 'static,
) -> windows::core::Result<()> {
    CALLBACK.with(|c| *c.borrow_mut() = Some(Box::new(callback)));

    unsafe {
        let hook: HWINEVENTHOOK = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.is_invalid() {
            return Err(windows::core::Error::from_win32());
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    Ok(())
}

/// Resolves the executable file name (not the full path) for a process id.
///
/// Returns `None` when the process can't be opened (permissions, or it exited
/// between the event and this call) or has no resolvable image name.
pub fn process_name_for_pid(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buffer = [0u16; MAX_PATH as usize];
        let mut size = buffer.len() as u32;
        let name = if QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
        .is_ok()
        {
            let full_path = String::from_utf16_lossy(&buffer[..size as usize]);
            Some(
                full_path
                    .rsplit('\\')
                    .next()
                    .unwrap_or(&full_path)
                    .to_string(),
            )
        } else {
            None
        };

        let _ = CloseHandle(handle);
        name.filter(|n| !n.is_empty())
    }
}

/// Builds a [`ForegroundEvent`] for whatever window is foreground *right now*.
///
/// The `SetWinEventHook` watcher only reports foreground *changes*, so an app
/// that was already focused when nodewarden started would never be matched
/// until the user switched away and back. This lets startup seed the pipeline
/// with the current window once.
pub fn current_foreground_event() -> Option<ForegroundEvent> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }

    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let exe_name = process_name_for_pid(pid)?;

    Some(ForegroundEvent {
        hwnd: hwnd.0 as isize,
        pid,
        exe_name,
    })
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _id_event_thread: u32,
    _dwms_event_time: u32,
) {
    // Only genuine top-level window events. `EVENT_SYSTEM_FOREGROUND` is also
    // raised for accessibility sub-objects (caret, client area, menu items,
    // ...), identified by a non-`OBJID_WINDOW` `idObject`, and for individual
    // children of a window (`idChild != CHILDID_SELF`). Those are not window
    // focus changes and must not drive a credential fill.
    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }

    if hwnd.0.is_null() {
        return;
    }

    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return;
    }

    let Some(exe_name) = process_name_for_pid(pid) else {
        return;
    };

    CALLBACK.with(|c| {
        if let Some(cb) = c.borrow_mut().as_mut() {
            cb(ForegroundEvent {
                hwnd: hwnd.0 as isize,
                pid,
                exe_name,
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_this_processs_own_image_name() {
        let name = process_name_for_pid(std::process::id())
            .expect("should resolve our own process image name");
        assert!(name.to_lowercase().ends_with(".exe"), "got {name}");
        assert!(!name.contains('\\'), "expected file name, got path: {name}");
    }

    #[test]
    fn returns_none_for_pid_zero() {
        assert!(process_name_for_pid(0).is_none());
    }
}

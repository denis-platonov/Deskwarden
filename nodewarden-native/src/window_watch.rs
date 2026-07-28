use std::cell::RefCell;
use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, GetWindowThreadProcessId, TranslateMessage, MSG,
    EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
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

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _dwms_event_time: u32,
) {
    if hwnd.0.is_null() {
        return;
    }

    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return;
    }

    let exe_name = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
        Ok(handle) => {
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
                full_path
                    .rsplit('\\')
                    .next()
                    .unwrap_or(&full_path)
                    .to_string()
            } else {
                String::new()
            };
            let _ = CloseHandle(handle);
            name
        }
        Err(_) => String::new(),
    };

    if exe_name.is_empty() {
        return;
    }

    CALLBACK.with(|c| {
        if let Some(cb) = c.borrow_mut().as_mut() {
            cb(ForegroundEvent { hwnd: hwnd.0 as isize, pid, exe_name });
        }
    });
}

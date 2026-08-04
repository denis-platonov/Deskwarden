use std::cell::RefCell;
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, MAX_PATH};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumChildWindows, GetClassNameW, GetForegroundWindow, GetMessageW,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, TranslateMessage, CHILDID_SELF,
    EVENT_SYSTEM_FOREGROUND, MSG, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
};

pub struct ForegroundEvent {
    pub hwnd: isize,
    pub pid: u32,
    /// The application this window was attributed to -- or, when it could not
    /// be attributed at all, the name of the host that owns it. A consumer
    /// must therefore never treat this as "the app": route it through
    /// [`crate::match_engine::MatchEngine::lookup`], which decides what an
    /// unattributable window is allowed to match, or through
    /// [`crate::app::window_label`], which decides what it is called on screen.
    pub exe_name: String,
    /// The window's own title, as `GetWindowTextW` reports it.
    ///
    /// Empty is possible and means "this window has no title", not "not
    /// looked up": plenty of real top-level windows have none.
    pub title: String,
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
    process_image_path_for_pid(pid).map(|full_path| {
        full_path
            .rsplit('\\')
            .next()
            .unwrap_or(&full_path)
            .to_string()
    })
}

/// Resolves the full executable path for a process id -- unlike
/// [`process_name_for_pid`], the path is kept rather than trimmed to the
/// file name, since callers that need to load the exe's icon
/// (`window_list::list_windows`) need somewhere to load it from.
///
/// Returns `None` when the process can't be opened (permissions, or it exited
/// between the event and this call) or has no resolvable image name.
pub fn process_image_path_for_pid(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buffer = [0u16; MAX_PATH as usize];
        let mut size = buffer.len() as u32;
        let path = if QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
        .is_ok()
        {
            Some(String::from_utf16_lossy(&buffer[..size as usize]))
        } else {
            None
        };

        let _ = CloseHandle(handle);
        path.filter(|n| !n.is_empty())
    }
}

/// Executables that own top-level windows on behalf of *other* applications,
/// so that "the process that owns this window" is not the application the
/// user is looking at.
///
/// **One entry, and it is the one with evidence.** Microsoft Store / UWP apps
/// do not own their own top-level window: `ApplicationFrameHost.exe` creates
/// the frame for every one of them. The user's report is exactly this --
/// `ApplicationFrameHost.exe` (pid 12472) owned the window titled "Speedtest"
/// while `Speedtest.exe` (pid 45996) had `MainWindowHandle: 0` -- and a match
/// recorded against the host name therefore matched *every* Store app they
/// focused. Other candidates get named in folklore (`WWAHost.exe`,
/// `RuntimeBroker.exe`, ...); none of them was observed here, and a host list
/// is not free: every name on it costs a real application the ability to be
/// matched at all (see [`attribute_window`]'s `UnresolvedHost` arm). So this
/// list grows only on evidence, never on a hunch.
const HOST_PROCESSES: [&str; 1] = ["ApplicationFrameHost.exe"];

/// The window class of the hosted application's own window inside a host
/// frame. That child window belongs to the *real* process, which is what
/// makes the hosted app recoverable at all.
pub const HOSTED_APP_WINDOW_CLASS: &str = "Windows.UI.Core.CoreWindow";

/// True when `exe_name` is a window host (see [`HOST_PROCESSES`]) rather than
/// an application in its own right.
///
/// Case-insensitive, because every other exe-name comparison in this crate is
/// (`MatchEngine::lookup`, `app::find_window_for_process`) and Windows paths
/// are.
pub fn is_host_process(exe_name: &str) -> bool {
    HOST_PROCESSES.iter().any(|host| host.eq_ignore_ascii_case(exe_name))
}

/// One child window of a top-level window, as observed by
/// [`child_windows_of`] -- everything [`attribute_window`] is allowed to know
/// about it.
///
/// `exe_name` is `None` when the child's process could not be opened or named
/// (see [`process_name_for_pid`]); that is a distinct case from "named
/// something useless", and the decision treats it as such.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildWindow {
    pub class_name: String,
    pub pid: u32,
    pub exe_name: Option<String>,
}

/// Which application a window should be attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    /// The window belongs to this process, and matching may use it.
    Attributed { pid: u32, exe_name: String },
    /// The window is owned by a known host ([`HOST_PROCESSES`]) and the
    /// hosted application could not be identified from its children.
    ///
    /// **Deliberately not "fall back to the host name".** Falling back is the
    /// bug: the host's name is a name that matches every Store app, so
    /// answering with it is worse than answering with nothing. A window we
    /// cannot attribute is simply not matched.
    UnresolvedHost { host: String },
}

/// **The resolution decision, as a pure function.** Given the process that
/// owns a window and everything observable about that window's children, say
/// which application the window should be attributed to.
///
/// The Win32 calls that gather the children cannot be unit-tested; this can,
/// and it holds every rule:
///
///  * A window owned by a non-host process is that process. No child is
///    consulted, whatever it contains -- otherwise any application that
///    happened to host a `CoreWindow` child would be attributed away from
///    itself.
///  * A window owned by a host is the first child of class
///    [`HOSTED_APP_WINDOW_CLASS`] whose own process is nameable and is not the
///    host again. The class filter lives here, not in the enumeration, so
///    that "which child counts" is a tested decision rather than an untested
///    loop condition.
///  * A host with no such child is [`Attribution::UnresolvedHost`]. A
///    minimised or suspended Store app has its `CoreWindow` reparented away
///    from the frame, so this is a state the real system produces and not
///    just a defensive arm.
pub fn attribute_window(owner_pid: u32, owner_exe: &str, children: &[ChildWindow]) -> Attribution {
    if !is_host_process(owner_exe) {
        return Attribution::Attributed { pid: owner_pid, exe_name: owner_exe.to_string() };
    }

    for child in children {
        if !child.class_name.eq_ignore_ascii_case(HOSTED_APP_WINDOW_CLASS) {
            continue;
        }
        let Some(name) = child.exe_name.as_deref() else {
            continue;
        };
        if name.is_empty() || is_host_process(name) {
            continue;
        }
        return Attribution::Attributed { pid: child.pid, exe_name: name.to_string() };
    }

    Attribution::UnresolvedHost { host: owner_exe.to_string() }
}

/// **The whole of what the watcher decides about one window, as a pure
/// function**: the [`ForegroundEvent`] a window should produce, or `None` if
/// it should produce none at all.
///
/// `win_event_proc` is an `extern "system"` callback that nothing outside the
/// Windows message loop can drive, and `current_foreground_event` answers for
/// whatever the machine happens to be showing -- so if the "attribute, then
/// build the event" step lived inside either of them, the fix could be
/// reverted with every test still green. It lives here instead, and both of
/// them are glue over it.
///
/// **An unattributable host frame now raises an event, under the host's name,
/// as long as it has a title.** It used to raise none at all, and that was
/// right while the host's name was the only thing such a window could be
/// matched on -- a name that means "every Microsoft Store app". It has one
/// other thing: the title on the frame, which a suspended Store app keeps when
/// its `CoreWindow` (and with it any way of naming its executable) is gone.
/// The event carries both, and
/// [`crate::match_engine::MatchEngine::lookup`] is where the host's name stops
/// being usable as an identity -- it routes a host-named window to the stored
/// titles and to nothing else. A frame with no title has neither, and still
/// raises nothing.
pub fn foreground_event_from(
    hwnd: isize,
    owner_pid: u32,
    owner_exe: &str,
    title: &str,
    children: &[ChildWindow],
) -> Option<ForegroundEvent> {
    match attribute_window(owner_pid, owner_exe, children) {
        Attribution::Attributed { pid, exe_name } => Some(ForegroundEvent {
            hwnd,
            pid,
            exe_name,
            title: title.to_string(),
        }),
        Attribution::UnresolvedHost { host } if title.is_empty() => {
            log::debug!(
                "ignoring foreground window {hwnd}: hosted by {host}, no hosted app identified \
                 and no title to identify it by"
            );
            None
        }
        Attribution::UnresolvedHost { host } => {
            log::debug!(
                "foreground window {hwnd} is hosted by {host} with no hosted app identified; \
                 it can only match on its title, {title:?}"
            );
            // The HOST's pid, because there is no other one: nothing behind
            // this frame could be named. Callers that key on a pid must know
            // that (`window_list::enum_proc` says the same of its rows).
            Some(ForegroundEvent {
                hwnd,
                pid: owner_pid,
                exe_name: host,
                title: title.to_string(),
            })
        }
    }
}

/// Observes `hwnd`'s children (when it needs to) and hands them to
/// [`foreground_event_from`] -- the one live entry point, called by both
/// `win_event_proc` and [`current_foreground_event`].
///
/// The children are enumerated only when the owner is a host, because for
/// every ordinary application the answer cannot depend on them (see
/// [`attribute_window`]) and this runs on every foreground change. The
/// non-host case still goes *through* the pure function, with no children,
/// rather than being answered here.
///
/// The title, by contrast, is read on both paths: it is one cheap call, and
/// making it conditional would mean the event's contents depended on which
/// branch produced it.
pub fn observe_foreground_event(
    hwnd: isize,
    owner_pid: u32,
    owner_exe: &str,
) -> Option<ForegroundEvent> {
    let title = window_title(hwnd);
    if !is_host_process(owner_exe) {
        return foreground_event_from(hwnd, owner_pid, owner_exe, &title, &[]);
    }
    let children = child_windows_of(hwnd);
    foreground_event_from(hwnd, owner_pid, owner_exe, &title, &children)
}

/// The window's title, or an empty string if it has none (or it could not be
/// read).
///
/// Pure observation, no decisions -- the same shape as [`child_windows_of`].
/// `window_list::enum_proc` reads titles the same way, but *drops* untitled
/// windows because an untitled row is nothing a user could click; here an
/// untitled window is an ordinary case that simply cannot be matched by title.
fn window_title(hwnd: isize) -> String {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        if copied <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..copied as usize])
    }
}

/// Attributes `hwnd` to a real application, for callers that want the
/// attribution rather than an event -- `window_list::list_windows`, which is
/// building picker rows and not foreground events.
pub fn resolve_window_attribution(hwnd: isize, owner_pid: u32, owner_exe: &str) -> Attribution {
    if !is_host_process(owner_exe) {
        return attribute_window(owner_pid, owner_exe, &[]);
    }
    let children = child_windows_of(hwnd);
    attribute_window(owner_pid, owner_exe, &children)
}

/// Observes every descendant window of `hwnd`: its class, its owning process
/// id, and that process's executable name.
///
/// Pure observation, no decisions -- see [`attribute_window`] for those.
fn child_windows_of(hwnd: isize) -> Vec<ChildWindow> {
    let mut out: Vec<ChildWindow> = Vec::new();
    unsafe {
        let _ = EnumChildWindows(
            HWND(hwnd as *mut core::ffi::c_void),
            Some(child_enum_proc),
            LPARAM(&mut out as *mut Vec<ChildWindow> as isize),
        );
    }
    out
}

unsafe extern "system" fn child_enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // Every path here is "keep enumerating".
    const CONTINUE: BOOL = BOOL(1);

    let mut buffer = [0u16; 256];
    let copied = GetClassNameW(hwnd, &mut buffer);
    if copied <= 0 {
        return CONTINUE;
    }
    let class_name = String::from_utf16_lossy(&buffer[..copied as usize]);

    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));

    let out = &mut *(lparam.0 as *mut Vec<ChildWindow>);
    out.push(ChildWindow {
        class_name,
        pid,
        exe_name: process_name_for_pid(pid),
    });

    CONTINUE
}

/// Builds a [`ForegroundEvent`] for whatever window is foreground *right now*.
///
/// The `SetWinEventHook` watcher only reports foreground *changes*, so an app
/// that was already focused when deskwarden started would never be matched
/// until the user switched away and back. This lets startup seed the pipeline
/// with the current window once.
pub fn current_foreground_event() -> Option<ForegroundEvent> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }

    let mut owner_pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner_pid)) };
    let owner_exe = process_name_for_pid(owner_pid)?;

    observe_foreground_event(hwnd.0 as isize, owner_pid, &owner_exe)
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

    let mut owner_pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut owner_pid));
    if owner_pid == 0 {
        return;
    }

    let Some(owner_exe) = process_name_for_pid(owner_pid) else {
        return;
    };

    // The owning process is not necessarily the application: Store/UWP apps
    // are all owned by `ApplicationFrameHost.exe`. Attributing the window is
    // what stops one match against the host from firing on every Store app
    // the user focuses. `None` means this window is not attributable to any
    // application, and no event is raised for it at all.
    let Some(event) = observe_foreground_event(hwnd.0 as isize, owner_pid, &owner_exe) else {
        return;
    };

    CALLBACK.with(|c| {
        if let Some(cb) = c.borrow_mut().as_mut() {
            cb(ForegroundEvent {
                hwnd: event.hwnd,
                pid: event.pid,
                exe_name: event.exe_name.clone(),
                title: event.title.clone(),
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

    const HOST: &str = "ApplicationFrameHost.exe";

    fn child(class_name: &str, pid: u32, exe_name: Option<&str>) -> ChildWindow {
        ChildWindow {
            class_name: class_name.to_string(),
            pid,
            exe_name: exe_name.map(str::to_string),
        }
    }

    /// The hosted app's own window, as the real system presents it.
    fn core_window(pid: u32, exe_name: &str) -> ChildWindow {
        child(HOSTED_APP_WINDOW_CLASS, pid, Some(exe_name))
    }

    #[test]
    fn the_frame_host_is_a_host_and_an_ordinary_app_is_not() {
        // Positive control first, in the same assertion pair: a guard that
        // answered `true` for everything would pass the first line alone.
        assert!(is_host_process(HOST), "{HOST} is the process in the bug report");
        assert!(
            !is_host_process("Speedtest.exe"),
            "a real application must still be matchable"
        );
    }

    #[test]
    fn host_detection_ignores_case_the_way_every_other_exe_comparison_does() {
        assert!(is_host_process("applicationframehost.EXE"));
        // ...and is not merely "contains", which would swallow a real app
        // whose name happens to embed the host's.
        assert!(!is_host_process("MyApplicationFrameHost.exe.exe"));
    }

    #[test]
    fn an_ordinary_window_is_attributed_to_the_process_that_owns_it() {
        // The positive control for every "is not attributed" assertion below:
        // an implementation that attributed nothing would fail here.
        assert_eq!(
            attribute_window(4242, "Speedtest.exe", &[]),
            Attribution::Attributed { pid: 4242, exe_name: "Speedtest.exe".to_string() }
        );
    }

    #[test]
    fn an_ordinary_window_is_never_attributed_away_by_a_hosted_child() {
        // A non-host owner's answer must not depend on its children at all.
        // Dropping the `is_host_process` guard at the top of
        // `attribute_window` -- i.e. always scanning children -- gives
        //     left: Attributed { pid: 99, exe_name: "Impostor.exe" }
        //     right: Attributed { pid: 4242, exe_name: "Speedtest.exe" }
        assert_eq!(
            attribute_window(
                4242,
                "Speedtest.exe",
                &[core_window(99, "Impostor.exe")]
            ),
            Attribution::Attributed { pid: 4242, exe_name: "Speedtest.exe".to_string() }
        );
    }

    /// THE BUG. `ApplicationFrameHost.exe` (pid 12472) owned the window titled
    /// "Speedtest"; `Speedtest.exe` (pid 45996) had no window of its own. A
    /// match saved against the host name matched every Store app the user
    /// focused.
    #[test]
    fn a_hosted_window_is_attributed_to_the_app_behind_the_frame() {
        // Reverting the host arm to `Attributed { owner_pid, owner_exe }` --
        // the behaviour that shipped -- gives
        //     left: Attributed { pid: 12472, exe_name: "ApplicationFrameHost.exe" }
        //     right: Attributed { pid: 45996, exe_name: "Speedtest.exe" }
        assert_eq!(
            attribute_window(
                12472,
                HOST,
                &[
                    child("ApplicationFrameInputSinkWindow", 12472, Some(HOST)),
                    core_window(45996, "Speedtest.exe"),
                ]
            ),
            Attribution::Attributed { pid: 45996, exe_name: "Speedtest.exe".to_string() }
        );
    }

    #[test]
    fn only_a_core_window_child_identifies_the_hosted_app() {
        // The class filter is a decision, not an enumeration detail: a frame
        // has several children of the host's own making, and the first one
        // that happens to belong to another process is not the app. Deleting
        // the class check gives
        //     left: Attributed { pid: 777, exe_name: "TextInputHost.exe" }
        //     right: UnresolvedHost { host: "ApplicationFrameHost.exe" }
        assert_eq!(
            attribute_window(
                12472,
                HOST,
                &[child("ApplicationFrameTitleBarWindow", 777, Some("TextInputHost.exe"))]
            ),
            Attribution::UnresolvedHost { host: HOST.to_string() }
        );

        // Positive control on the same call: the ONLY difference is the
        // child's class, so this cannot pass against a build that resolves
        // nothing.
        assert_eq!(
            attribute_window(
                12472,
                HOST,
                &[child(HOSTED_APP_WINDOW_CLASS, 777, Some("TextInputHost.exe"))]
            ),
            Attribution::Attributed { pid: 777, exe_name: "TextInputHost.exe".to_string() }
        );
    }

    #[test]
    fn a_host_frame_with_no_identifiable_child_is_not_attributed_to_the_host() {
        // The arm that matters most: falling back to the owner's name here is
        // exactly the shipped bug, so this must be `UnresolvedHost` and not
        // merely "something other than Speedtest". A `_ => Attributed { owner }`
        // fallback gives
        //     left: Attributed { pid: 12472, exe_name: "ApplicationFrameHost.exe" }
        //     right: UnresolvedHost { host: "ApplicationFrameHost.exe" }
        for children in [
            // Nothing at all -- a suspended Store app, whose CoreWindow is
            // reparented off the frame.
            vec![],
            // A CoreWindow that is still the host's own.
            vec![core_window(12472, HOST)],
            // A CoreWindow whose process could not be opened or named.
            vec![child(HOSTED_APP_WINDOW_CLASS, 45996, None)],
            // A CoreWindow that named an empty string.
            vec![child(HOSTED_APP_WINDOW_CLASS, 45996, Some(""))],
        ] {
            assert_eq!(
                attribute_window(12472, HOST, &children),
                Attribution::UnresolvedHost { host: HOST.to_string() },
                "children: {children:?}"
            );
        }

        // Positive control, same function, same host: one nameable CoreWindow
        // belonging to another process IS resolved. Without this the loop
        // above passes against a build that never attributes a hosted window.
        assert_eq!(
            attribute_window(12472, HOST, &[core_window(45996, "Speedtest.exe")]),
            Attribution::Attributed { pid: 45996, exe_name: "Speedtest.exe".to_string() }
        );
    }

    #[test]
    fn a_hosted_window_produces_an_event_naming_the_hosted_app_not_the_host() {
        // `foreground_event_from` is what `win_event_proc` and
        // `current_foreground_event` are both glue over, so this is the shape
        // of the event the match engine is actually handed. Reverting it to
        // build the event from `owner_pid`/`owner_exe` gives
        //     left: ("ApplicationFrameHost.exe", 12472)
        //     right: ("Speedtest.exe", 45996)
        let event = foreground_event_from(
            0x1234,
            12472,
            HOST,
            "Speedtest",
            &[core_window(45996, "Speedtest.exe")],
        )
        .expect("a frame with an identifiable hosted app must raise an event");
        assert_eq!(
            (event.exe_name.as_str(), event.pid, event.hwnd),
            ("Speedtest.exe", 45996, 0x1234)
        );
    }

    #[test]
    fn every_event_carries_the_window_title_it_was_built_from() {
        // The title is the whole of what a suspended Store app can be matched
        // by, so an event that dropped it would make `MatchEngine`'s title
        // table unreachable in production while every test of that table
        // stayed green. Dropping the field from the `Attributed` arm gives
        //     left: ""  right: "Untitled - Notepad"
        let event = foreground_event_from(0x1234, 4242, "notepad.exe", "Untitled - Notepad", &[])
            .expect("an ordinary window raises an event");
        assert_eq!(event.title, "Untitled - Notepad");
    }

    /// **What used to be `an_unattributable_host_frame_raises_no_event_at_all`.**
    /// It now raises one, because the title on the frame is a real identity
    /// even when nothing behind it can be named -- see `foreground_event_from`.
    /// The host's NAME is still not an identity, and that is enforced in
    /// `MatchEngine::lookup`, which is the only thing that reads it.
    #[test]
    fn an_unattributable_host_frame_is_reported_under_the_host_name_with_its_title() {
        // Reverting the `UnresolvedHost` arm to `None` gives
        //     "a titled frame is the only chance a suspended Store app has"
        let event = foreground_event_from(0x1234, 12472, HOST, "Speedtest", &[])
            .expect("a titled frame is the only chance a suspended Store app has");
        assert_eq!(
            (event.exe_name.as_str(), event.pid, event.title.as_str()),
            (HOST, 12472, "Speedtest")
        );
    }

    #[test]
    fn an_unattributable_host_frame_with_no_title_raises_no_event_at_all() {
        // Nothing can identify this window: not its process, not its title.
        // Deleting the `if title.is_empty()` arm gives
        //     "a window with no identity at all must raise no event"
        assert!(
            foreground_event_from(0x1234, 12472, HOST, "", &[]).is_none(),
            "a window with no identity at all must raise no event"
        );
        // Positive control on the same function and the same host: the ONLY
        // difference is the title, so this cannot pass against a watcher that
        // has gone entirely inert.
        assert!(foreground_event_from(0x1234, 12472, HOST, "Speedtest", &[]).is_some());
        // ...and an ordinary untitled window is unaffected: it is identified
        // by its process, and the title guard must not reach it.
        assert!(foreground_event_from(0x1234, 4242, "Speedtest.exe", "", &[]).is_some());
    }
}

/// Source guard for the ONE line of glue the pure functions above cannot
/// reach: that the two live entry points -- the `SetWinEventHook` callback and
/// the startup seed -- actually route through
/// `observe_foreground_event`.
///
/// Both are unreachable from a test (`win_event_proc` is an `extern "system"`
/// callback driven only by the Windows message loop; `current_foreground_event`
/// answers for whatever the machine is showing), and this repo's repeated
/// finding is that decisions get tested while the wiring that reaches them does
/// not. Rewiring either call site back to `process_name_for_pid` leaves every
/// test above green and makes the whole fix inert -- except this one.
///
/// What it can and cannot see: it pins the spelling and the COUNT of the
/// calls. It cannot see a call whose result is then discarded; that is visible
/// in any diff touching these lines. What it guards is the revert.
#[cfg(test)]
mod watcher_wiring_tests {
    // SPLIT ACROSS TWO LITERALS, DELIBERATELY, and on ONE line each.
    // `include_str!` pulls this module in too, so a needle written whole would
    // always match its own declaration; and a needle containing a newline
    // would pass on an LF checkout and fail on a CRLF one (this repo has
    // both). `concat!` joins at compile time.
    const CALL: &str = concat!("observe_foreground_event", "(");

    /// The other call this module has to pin, and for a sharper reason: every
    /// test of the title rule injects the title as an argument, so
    /// `foreground_event_from(hwnd, owner_pid, owner_exe, "", &children)` --
    /// one deleted call -- compiles, leaves the whole suite green, and makes
    /// the title table unreachable on a real desktop. That is the exact shape
    /// this repo has been bitten by twice.
    const TITLE_CALL: &str = concat!("window_title", "(");

    /// And the other half of it. `TITLE_CALL` alone pins only that the title is
    /// READ: the mutation that rewrites both `foreground_event_from` calls to
    /// pass `""` leaves the `window_title` call sitting there, counted, with
    /// its result dropped -- measured, not assumed, and it is exactly the "call
    /// whose result is thrown away" the sibling guards in this crate disclaim.
    /// This needle is the argument itself, at both call sites.
    ///
    /// Together they are tight: this one alone would accept a `let title =
    /// String::new()`, and that one alone accepts the empty literal.
    const TITLE_ARGUMENT: &str = concat!("&title", ", &");

    fn source() -> &'static str {
        include_str!("window_watch.rs")
    }

    /// The same counting the real assertion uses, so the positive control
    /// below drives this code and not a re-implementation of it.
    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn the_counter_finds_a_call_that_is_really_there() {
        // Positive control: without it, an `occurrences` that always returned
        // 3 would satisfy the test below.
        let planted = concat!("let x = observe_foreground_event", "(1, 2, \"a\");");
        assert_eq!(occurrences(planted, CALL), 1, "planted: {planted}");
        assert_eq!(occurrences("nothing here", CALL), 0);

        let planted_title = concat!("let t = window_title", "(hwnd);");
        assert_eq!(occurrences(planted_title, TITLE_CALL), 1, "planted: {planted_title}");
        assert_eq!(occurrences("nothing here", TITLE_CALL), 0);

        let planted_arg = concat!("f(hwnd, owner_pid, owner_exe, &title", ", &[])");
        assert_eq!(occurrences(planted_arg, TITLE_ARGUMENT), 1, "planted: {planted_arg}");
        assert_eq!(occurrences(concat!("f(hwnd, owner_exe, \"\"", ", &[])"), TITLE_ARGUMENT), 0);
    }

    #[test]
    fn the_observer_reads_the_windows_real_title_rather_than_passing_an_empty_one() {
        assert_eq!(
            occurrences(source(), TITLE_CALL),
            2,
            "expected {TITLE_CALL:?} exactly twice in window_watch.rs -- its own definition and \
             the one call, in `observe_foreground_event`. One means every foreground event now \
             carries an empty title, which silently disables the only way a suspended Microsoft \
             Store app can be matched: `MatchEngine`'s title table is keyed on a title no live \
             event would ever supply"
        );
    }

    #[test]
    fn the_title_it_read_is_the_title_it_hands_to_the_decision() {
        assert_eq!(
            occurrences(source(), TITLE_ARGUMENT),
            2,
            "expected {TITLE_ARGUMENT:?} exactly twice in window_watch.rs -- one for each of \
             `observe_foreground_event`'s two calls to `foreground_event_from`, the host path \
             and the ordinary one. Fewer means a call went back to passing a literal, so every \
             foreground event on that path carries no title: a suspended Microsoft Store app \
             then has no identity at all and can never be matched"
        );
    }

    #[test]
    fn both_live_entry_points_route_through_the_attributing_observer() {
        assert_eq!(
            occurrences(source(), CALL),
            3,
            "expected {CALL:?} exactly three times in window_watch.rs -- its own definition, the \
             `win_event_proc` callback, and `current_foreground_event`. Two means one of the two \
             live entry points went back to building its event straight from \
             `process_name_for_pid`, which is the shipped bug: every Microsoft Store window is \
             owned by ApplicationFrameHost.exe, so that path names the host and one saved match \
             fires on every Store app"
        );
    }
}

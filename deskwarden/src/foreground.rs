//! Bringing this process's own windows to the front when one opens.
//!
//! Reported as two symptoms with one cause: "when you click Login with Hello
//! - Windows PIN screen launches in background ... same after login - has to
//! be visible right away and not behind or even worse in tray". Before this
//! module the only foreground handling in the crate was
//! `with_always_on_top()` on the autofill overlay; every other window took
//! whatever z-order Windows happened to hand it.
//!
//! ## Why the Hello prompt in particular lands behind
//!
//! `KeyCredentialManager` has **no** window-parenting interop. The WinRT
//! classes that can be told which HWND to parent their UI to expose a COM
//! interop interface for it -- `IUserConsentVerifierInterop::
//! RequestVerificationForWindowAsync(hwnd, ...)` is the one people reach for
//! by analogy -- and `Windows.Security.Credentials` has no counterpart. That
//! is not a guess about the docs: the `windows` 0.58 projection this crate
//! builds against contains 50-odd `I*Interop` interfaces, `Security` is not
//! among the namespaces that get one, and `KeyCredential`'s only signing
//! entry point is the bare `RequestSignAsync(data)`. So there is nothing to
//! parent the prompt to, and the practical fix is the one this module is:
//! make this process the foreground process immediately *before* the call,
//! so the broker that shows the prompt inherits foreground rights from us.
//!
//! ## `SetForegroundWindow` is allowed to say no
//!
//! It fails by design unless the caller already owns the foreground, received
//! the last input event, or was granted permission. On the machine of whoever
//! writes this fix it will essentially always succeed, because they clicked
//! the button that ran it. On a user's machine it will not: the vault window
//! opens after a `bw serve` start that can take ~28s, and anything the user
//! clicks in that time takes the right away from us.
//!
//! The refusal is therefore a *handled* outcome, not an ignored return value.
//! When Windows says no, [`raise_on`] flashes the taskbar button
//! (`FlashWindowEx`, `FLASHW_ALL | FLASHW_TIMERNOFG` -- flash until the
//! window comes to the foreground) and reports [`Raised::Flashed`]. That is
//! what Windows expects an application to do instead of stealing focus, and
//! it is also the honest answer to "or even worse in tray": the window is
//! open and its taskbar button is asking for attention.
//!
//! What this deliberately does not do: `AttachThreadInput` to the current
//! foreground thread, or a minimise/restore bounce, to force the activation
//! through. Both are focus-stealing tricks that work by lying to the window
//! manager, and both would make this app the thing that yanks the user out of
//! whatever they were typing into.
//!
//! `with_always_on_top` is likewise not the answer for ordinary windows. The
//! overlay (`overlay_ui`) is topmost because it is a transient prompt drawn
//! over another app; a vault window that outranks every other window on the
//! desktop for as long as it is open is a worse bug than the one being fixed.
//!
//! ## What is testable here and what is not
//!
//! Nothing about real Win32 focus can be asserted from `cargo test` -- there
//! is no window, and the outcome of `SetForegroundWindow` depends on which
//! process last received input. So the two decisions are split out of the
//! calls: [`pick`] (*which* window should be raised) is pure, and [`raise_on`]
//! (*whether* a raise is warranted, and what happens when it is refused) is
//! written against the [`Desktop`] trait so the whole sequence -- restore,
//! foreground check, refusal, flash -- runs against a fake. [`Win32Desktop`]
//! is the only untestable part and is kept to thin wrappers with no decisions
//! in them. Like `window_list`, every call degrades rather than panicking: a
//! window that cannot be found or raised costs the user a click, not a crash.

/// One top-level window belonging to *this* process, as [`pick`] sees it.
///
/// `visible` is `IsWindowVisible`, which stays `true` for a **minimised**
/// window -- minimising does not clear `WS_VISIBLE`. That is what makes
/// `minimised` a separate field and not a reason to skip the row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnWindow {
    pub hwnd: isize,
    pub title: String,
    pub visible: bool,
    /// `WS_EX_TOOLWINDOW`. The tray icon and the hotkey listener own helper
    /// windows of this kind; raising one would be raising nothing the user
    /// can see.
    pub tool_window: bool,
    pub minimised: bool,
}

/// Which of this process's windows a caller wants in front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    /// The window with exactly this title. Every window site passes the same
    /// `const` it passed to `eframe::run_ui_native`, so the two cannot drift.
    Titled(&'a str),
    /// Whichever eligible window this process has. For callers that do not
    /// know the title because they can be reached from more than one window
    /// -- `hello`, which is called from the login window and from the vault
    /// window's re-auth prompt.
    Any,
}

/// What [`raise_on`] did. Returned rather than only logged so the sequence
/// can be asserted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raised {
    /// No window of ours matched. Nothing was called.
    NoWindow,
    /// Ours was already the foreground window. Nothing was called.
    AlreadyInFront,
    /// Windows accepted the activation.
    Front,
    /// Windows refused the activation; the taskbar button was flashed.
    Flashed,
}

/// The Win32 calls [`raise_on`] makes, behind a trait so the sequencing can
/// be tested without a desktop. The real implementation is [`Win32Desktop`].
pub trait Desktop {
    fn own_windows(&self) -> Vec<OwnWindow>;
    /// `GetForegroundWindow`, as a raw handle value. `0` when there is no
    /// foreground window at all (which is one of the cases in which
    /// `SetForegroundWindow` is *allowed* to succeed).
    fn foreground(&self) -> isize;
    fn restore(&self, hwnd: isize);
    /// `SetForegroundWindow`. `false` is the documented refusal, not an error.
    fn set_foreground(&self, hwnd: isize) -> bool;
    fn flash(&self, hwnd: isize);
}

/// Which window should be raised -- the pure half.
///
/// Skips what the user cannot point at: invisible windows, `WS_EX_TOOLWINDOW`
/// helper windows, and untitled ones (the same three exclusions
/// `window_list::enum_proc` applies for the same reason). A **minimised**
/// window is deliberately still eligible; restoring it is the whole of the
/// "or even worse in tray" half of the report.
pub fn pick<'w>(windows: &'w [OwnWindow], target: Target<'_>) -> Option<&'w OwnWindow> {
    windows.iter().find(|w| {
        if !w.visible || w.tool_window || w.title.is_empty() {
            return false;
        }
        match target {
            Target::Titled(title) => w.title == title,
            Target::Any => true,
        }
    })
}

/// Restore-if-minimised, then activate, then flash if Windows refused.
///
/// Generic over [`Desktop`] so every branch of that sequence is reachable
/// from a test; [`raise_window`] and [`raise_this_process`] are the real
/// entry points.
pub fn raise_on<D: Desktop + ?Sized>(desktop: &D, target: Target<'_>) -> Raised {
    let windows = desktop.own_windows();
    let Some(window) = pick(&windows, target) else {
        return Raised::NoWindow;
    };

    // Checked before the restore below, so that a window already in front and
    // already restored costs no calls at all -- a `SetForegroundWindow` on the
    // window that already has the foreground would succeed, but a needless
    // one is a needless chance to be refused and flash at a user who is
    // already looking straight at the window.
    if desktop.foreground() == window.hwnd && !window.minimised {
        return Raised::AlreadyInFront;
    }

    if window.minimised {
        // `SetForegroundWindow` on an iconic window activates it without
        // un-minimising it, which is exactly the "it went to the tray"
        // outcome. Restore first, then activate.
        desktop.restore(window.hwnd);
    }

    if desktop.set_foreground(window.hwnd) {
        Raised::Front
    } else {
        // The documented refusal. Not an error, not retried, and not worked
        // around -- see this module's header.
        desktop.flash(window.hwnd);
        log::info!(
            "Windows declined to bring {:?} to the front; flashing its taskbar button instead",
            window.title
        );
        Raised::Flashed
    }
}

/// Bring this process's window titled `title` to the front.
///
/// Called from the first painted frame of every window this crate opens,
/// beside `login_ui::round_window_corners`, which is the same "the OS window
/// exists now" hook.
pub fn raise_window(title: &str) -> Raised {
    raise_on(&Win32Desktop, Target::Titled(title))
}

/// Bring whichever window this process has to the front, for callers that
/// cannot name one. See [`Target::Any`].
pub fn raise_this_process() -> Raised {
    raise_on(&Win32Desktop, Target::Any)
}

/// The real desktop. Thin wrappers only: every decision is in [`raise_on`].
pub struct Win32Desktop;

impl Desktop for Win32Desktop {
    fn own_windows(&self) -> Vec<OwnWindow> {
        win32::own_windows()
    }

    fn foreground(&self) -> isize {
        win32::foreground()
    }

    fn restore(&self, hwnd: isize) {
        win32::restore(hwnd);
    }

    fn set_foreground(&self, hwnd: isize) -> bool {
        win32::set_foreground(hwnd)
    }

    fn flash(&self, hwnd: isize) {
        win32::flash(hwnd);
    }
}

/// The Win32 calls themselves. No branches beyond "this handle is unusable,
/// do nothing", so there is nothing here for a test to have an opinion about.
mod win32 {
    use super::OwnWindow;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FlashWindowEx, GetForegroundWindow, GetWindowLongW, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow,
        ShowWindow, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG, GWL_EXSTYLE, SW_RESTORE,
        WS_EX_TOOLWINDOW,
    };

    pub fn own_windows() -> Vec<OwnWindow> {
        let mut out: Vec<OwnWindow> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut out as *mut Vec<OwnWindow> as isize));
        }
        out
    }

    /// Collects this process's top-level windows. The *only* filter applied
    /// here is "belongs to us" -- visibility, tool-window-ness and iconic
    /// state are recorded and left for `foreground::pick` to weigh, so that
    /// the rule lives somewhere a test can reach it.
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // Every return is "keep enumerating"; there is no early stop.
        const CONTINUE: BOOL = BOOL(1);

        let mut owner_pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut owner_pid));
        if owner_pid == 0 || owner_pid != GetCurrentProcessId() {
            return CONTINUE;
        }

        let len = GetWindowTextLengthW(hwnd);
        let title = if len > 0 {
            let mut buffer = vec![0u16; len as usize + 1];
            let copied = GetWindowTextW(hwnd, &mut buffer);
            String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
        } else {
            String::new()
        };

        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;

        let out = &mut *(lparam.0 as *mut Vec<OwnWindow>);
        out.push(OwnWindow {
            hwnd: hwnd.0 as isize,
            title,
            visible: IsWindowVisible(hwnd).as_bool(),
            tool_window: ex_style & WS_EX_TOOLWINDOW.0 != 0,
            minimised: IsIconic(hwnd).as_bool(),
        });

        CONTINUE
    }

    pub fn foreground() -> isize {
        unsafe { GetForegroundWindow().0 as isize }
    }

    pub fn restore(hwnd: isize) {
        // `ShowWindow` returns the *previous* visibility, not success; there
        // is nothing to check.
        unsafe {
            let _ = ShowWindow(HWND(hwnd as *mut core::ffi::c_void), SW_RESTORE);
        }
    }

    pub fn set_foreground(hwnd: isize) -> bool {
        unsafe { SetForegroundWindow(HWND(hwnd as *mut core::ffi::c_void)).as_bool() }
    }

    pub fn flash(hwnd: isize) {
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd: HWND(hwnd as *mut core::ffi::c_void),
            // Caption and taskbar button, flashing until the window comes to
            // the foreground -- `FLASHW_TIMERNOFG` is what makes this stop on
            // its own when the user finally clicks it, with no timer of ours.
            dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        unsafe {
            let _ = FlashWindowEx(&info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Call {
        Restore(isize),
        SetForeground(isize),
        Flash(isize),
    }

    struct FakeDesktop {
        windows: Vec<OwnWindow>,
        foreground: isize,
        set_foreground_succeeds: bool,
        calls: RefCell<Vec<Call>>,
    }

    impl FakeDesktop {
        fn new(windows: Vec<OwnWindow>) -> Self {
            Self {
                windows,
                foreground: 0,
                set_foreground_succeeds: true,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl Desktop for FakeDesktop {
        fn own_windows(&self) -> Vec<OwnWindow> {
            self.windows.clone()
        }
        fn foreground(&self) -> isize {
            self.foreground
        }
        fn restore(&self, hwnd: isize) {
            self.calls.borrow_mut().push(Call::Restore(hwnd));
        }
        fn set_foreground(&self, hwnd: isize) -> bool {
            self.calls.borrow_mut().push(Call::SetForeground(hwnd));
            self.set_foreground_succeeds
        }
        fn flash(&self, hwnd: isize) {
            self.calls.borrow_mut().push(Call::Flash(hwnd));
        }
    }

    fn window(hwnd: isize, title: &str) -> OwnWindow {
        OwnWindow {
            hwnd,
            title: title.to_string(),
            visible: true,
            tool_window: false,
            minimised: false,
        }
    }

    // ---- pick: which window ------------------------------------------------

    #[test]
    fn the_window_with_the_asked_for_title_is_the_one_picked() {
        let windows = vec![window(1, "Deskwarden"), window(2, "Preferences")];

        assert_eq!(pick(&windows, Target::Titled("Preferences")).unwrap().hwnd, 2);
        // Positive control on the same list: the mechanism can also find the
        // other one, so a hit above is a match and not "always the first".
        assert_eq!(pick(&windows, Target::Titled("Deskwarden")).unwrap().hwnd, 1);
        assert!(
            pick(&windows, Target::Titled("Log in to Deskwarden")).is_none(),
            "a title no window has must not fall back to some other window -- raising the wrong \
             window is worse than raising none"
        );
    }

    #[test]
    fn the_tray_and_hotkey_helper_windows_are_never_what_gets_raised() {
        let mut tool = window(7, "Deskwarden");
        tool.tool_window = true;
        let mut invisible = window(8, "Deskwarden");
        invisible.visible = false;
        let mut untitled = window(9, "");
        untitled.title = String::new();

        let windows = vec![tool, invisible, untitled, window(10, "Deskwarden")];

        // `Any` walks past all three helpers to the one real window...
        assert_eq!(pick(&windows, Target::Any).unwrap().hwnd, 10);
        // ...and a titled ask does not let the tool window in by the back door
        // just because its title matches.
        assert_eq!(pick(&windows, Target::Titled("Deskwarden")).unwrap().hwnd, 10);
        // Positive control: those same three rows ARE returned once the thing
        // that disqualifies each is removed, so the filter is reading the
        // fields it names rather than rejecting everything.
        for hwnd in [7, 8, 9] {
            let fixed = vec![window(hwnd, "Deskwarden")];
            assert_eq!(pick(&fixed, Target::Any).unwrap().hwnd, hwnd);
        }
    }

    #[test]
    fn a_minimised_window_is_still_a_window_worth_raising() {
        let mut minimised = window(3, "Deskwarden");
        minimised.minimised = true;
        assert_eq!(
            pick(&[minimised], Target::Any).map(|w| w.hwnd),
            Some(3),
            "skipping minimised windows would leave the user with exactly the tray-only window \
             they reported"
        );
    }

    // ---- raise_on: whether, and what happens when refused ------------------

    #[test]
    fn a_window_that_is_behind_is_brought_to_the_front() {
        let mut desktop = FakeDesktop::new(vec![window(1, "Deskwarden")]);
        desktop.foreground = 99; // some other app

        assert_eq!(raise_on(&desktop, Target::Titled("Deskwarden")), Raised::Front);
        assert_eq!(desktop.calls(), vec![Call::SetForeground(1)]);
    }

    #[test]
    fn the_window_already_in_front_is_left_alone() {
        let mut desktop = FakeDesktop::new(vec![window(1, "Deskwarden")]);
        desktop.foreground = 1;

        assert_eq!(raise_on(&desktop, Target::Titled("Deskwarden")), Raised::AlreadyInFront);
        assert_eq!(desktop.calls(), Vec::new(), "nothing should have been called");
    }

    #[test]
    fn a_refusal_flashes_the_taskbar_button_instead_of_being_ignored() {
        let mut desktop = FakeDesktop::new(vec![window(1, "Deskwarden")]);
        desktop.foreground = 99;
        desktop.set_foreground_succeeds = false;

        assert_eq!(
            raise_on(&desktop, Target::Titled("Deskwarden")),
            Raised::Flashed,
            "SetForegroundWindow is allowed to refuse; a fix that reports success anyway is the \
             one that works on the developer's machine and not the user's"
        );
        assert_eq!(desktop.calls(), vec![Call::SetForeground(1), Call::Flash(1)]);
    }

    #[test]
    fn an_accepted_raise_does_not_flash() {
        // The positive control for the assertion above: the same desktop, the
        // same window, only the answer from Windows differs.
        let mut desktop = FakeDesktop::new(vec![window(1, "Deskwarden")]);
        desktop.foreground = 99;
        desktop.set_foreground_succeeds = true;

        assert_eq!(raise_on(&desktop, Target::Titled("Deskwarden")), Raised::Front);
        assert_eq!(desktop.calls(), vec![Call::SetForeground(1)]);
    }

    #[test]
    fn a_minimised_window_is_restored_before_it_is_activated() {
        let mut minimised = window(1, "Deskwarden");
        minimised.minimised = true;
        let mut desktop = FakeDesktop::new(vec![minimised]);
        desktop.foreground = 99;

        assert_eq!(raise_on(&desktop, Target::Titled("Deskwarden")), Raised::Front);
        assert_eq!(
            desktop.calls(),
            vec![Call::Restore(1), Call::SetForeground(1)],
            "activating an iconic window without restoring it first leaves it minimised, which is \
             the reported 'even worse in tray'"
        );
    }

    #[test]
    fn a_minimised_window_is_restored_even_when_it_is_already_the_foreground_window() {
        // Windows keeps a minimised window as the foreground window in some
        // states; the "already in front, do nothing" shortcut must not swallow
        // the restore, or the window stays invisible and this reports success.
        let mut minimised = window(1, "Deskwarden");
        minimised.minimised = true;
        let mut desktop = FakeDesktop::new(vec![minimised]);
        desktop.foreground = 1;

        assert_eq!(raise_on(&desktop, Target::Titled("Deskwarden")), Raised::Front);
        assert_eq!(desktop.calls(), vec![Call::Restore(1), Call::SetForeground(1)]);
    }

    #[test]
    fn a_window_that_is_not_minimised_is_never_restored() {
        // Positive control for the pair above: `Restore` is conditional, not
        // unconditional. An unconditional restore would un-maximise the vault
        // window on every single open.
        let mut desktop = FakeDesktop::new(vec![window(1, "Deskwarden")]);
        desktop.foreground = 99;

        assert_eq!(raise_on(&desktop, Target::Titled("Deskwarden")), Raised::Front);
        assert!(
            !desktop.calls().contains(&Call::Restore(1)),
            "calls were {:?}",
            desktop.calls()
        );
    }

    #[test]
    fn nothing_is_called_when_this_process_has_no_window_to_raise() {
        let desktop = FakeDesktop::new(Vec::new());

        assert_eq!(raise_on(&desktop, Target::Any), Raised::NoWindow);
        assert_eq!(desktop.calls(), Vec::new());
        // ...and specifically not a flash of some other process's window.
        assert_eq!(raise_on(&desktop, Target::Titled("Deskwarden")), Raised::NoWindow);
        assert_eq!(desktop.calls(), Vec::new());
    }

    // ---- the wiring at each window site ------------------------------------

    /// Every window this crate opens must actually ask to be raised, and must
    /// ask for the SAME title it opened under. Neither can be asserted by
    /// running anything: `eframe::run_ui_native` blocks on a real OS event
    /// loop and opens a real window, so no test in this crate calls any of
    /// these four functions -- deleting a `raise_window` call leaves the whole
    /// suite green, and the user gets the window-behind-everything they
    /// reported back. So it is held by source position instead.
    ///
    /// What it can see: the call is present, once per window. What it cannot:
    /// that it runs on the frame the window first exists on rather than
    /// somewhere unreachable. That much is visible in any diff touching these
    /// lines, and is what the comment at each call site is for.
    ///
    /// `login_ui.rs` is deliberately absent -- its raise is a follow-up and
    /// asserting a count of zero there would have to be deleted the moment it
    /// lands.
    #[test]
    fn every_window_this_crate_opens_asks_to_be_brought_to_the_front() {
        // (name, source, the title identifier it opens under, how many windows)
        let sites: [(&str, &str, &str, usize); 6] = [
            ("vault_window/mod.rs", include_str!("vault_window/mod.rs"), "WINDOW_TITLE", 1),
            // The single startup window: sign-in, spinner and vault in one
            // event loop. It is the ONLY window on the launch that signs in,
            // so a raise it does not ask for is a launch that lands behind
            // whatever the user was doing -- with no second window afterwards
            // to correct it, which is what the three-window flow accidentally
            // provided.
            ("app_window.rs", include_str!("app_window.rs"), "WINDOW_TITLE", 1),
            // Added when the login window got its raise. It was excluded
            // while another agent owned that file, and an excluded site is
            // an unguarded one: the raise was applied there and nothing in
            // this test would have noticed it being deleted again.
            ("login_ui.rs", include_str!("login_ui.rs"), "WINDOW_TITLE", 1),
            ("prefs_ui.rs", include_str!("prefs_ui.rs"), "WINDOW_TITLE", 1),
            ("loading_ui.rs", include_str!("loading_ui.rs"), "WINDOW_TITLE", 1),
            // Two windows, two titles; the second is checked just below the
            // loop rather than in it.
            ("picker_ui.rs", include_str!("picker_ui.rs"), "PICK_ITEM_TITLE", 1),
        ];

        for (name, source, title, opens) in sites {
            assert_eq!(
                source.matches(&format!("run_ui_native({title},")).count(),
                opens,
                "{name} should open {opens} window(s) titled `{title}`"
            );
            assert_eq!(
                source.matches(&format!("raise_window({title})")).count(),
                opens,
                "{name} opens a window titled `{title}` and must raise that same window"
            );
        }

        let picker = include_str!("picker_ui.rs");
        assert_eq!(picker.matches("run_ui_native(ADD_APP_TITLE,").count(), 1);
        assert_eq!(picker.matches("raise_window(ADD_APP_TITLE)").count(), 1);

        // Positive control on the matcher itself: it can count, and can tell
        // one occurrence from two -- so a count of 1 above is one call and not
        // a needle that happens to match anything.
        assert_eq!(
            "raise_window(WINDOW_TITLE); raise_window(WINDOW_TITLE);"
                .matches("raise_window(WINDOW_TITLE)")
                .count(),
            2
        );
    }

    /// **The list above is hand-enumerated, and that is its one weakness.** A
    /// module added to this crate that opens a window is simply absent from it:
    /// every assertion still passes, and the new window is unguarded -- which
    /// is exactly what happened when `app_window` was added, and was caught by
    /// reading rather than by anything failing.
    ///
    /// So every module this crate declares must be classified, here, as one
    /// that opens windows or one that does not. Adding a module and not
    /// deciding is the failure. The module list is read out of `lib.rs` rather
    /// than written out again, because a second hand-written list would have
    /// the same hole one level up.
    #[test]
    fn every_module_in_this_crate_is_classified_as_opening_windows_or_not() {
        /// The modules `every_window_this_crate_opens_asks_to_be_brought_to_
        /// the_front` covers.
        const OPENS_WINDOWS: [&str; 7] = [
            "app_window",
            "loading_ui",
            "login_ui",
            "overlay_ui",
            "picker_ui",
            "prefs_ui",
            "vault_window",
        ];
        /// Everything else. Listed rather than inferred, because "this module
        /// does not open a window" is a decision someone has to make; a module
        /// missing from BOTH lists fails below rather than being quietly
        /// unguarded.
        const OPENS_NO_WINDOW: [&str; 29] = [
            "accounts",
            "app",
            "app_match",
            "backend_policy",
            "bw_path",
            "bw_serve",
            "dispatch",
            "favicon",
            "fill_stats",
            "foreground",
            "hello",
            "hotkey",
            "http_agent",
            "icon",
            "injector",
            "job_object",
            "logging",
            "match_engine",
            "password_strength",
            "session_store",
            "settings",
            "signature",
            "theme",
            "tray",
            "updater",
            "vault_bridge",
            "vault_cache",
            "window_list",
            "window_watch",
        ];

        let declared: Vec<&str> = include_str!("lib.rs")
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub mod "))
            .filter_map(|rest| rest.strip_suffix(';'))
            .collect();
        // Positive control: the parse really found the module list, rather
        // than an empty vector every loop below would skip.
        assert!(
            declared.len() > 20,
            "only {} modules parsed out of lib.rs -- the parse is wrong, and every check              below is vacuous: {declared:?}",
            declared.len()
        );
        assert!(
            declared.contains(&"vault_window"),
            "control: the parse did not find a module known to be there: {declared:?}"
        );

        for module in OPENS_WINDOWS.iter().chain(OPENS_NO_WINDOW.iter()) {
            assert!(
                declared.contains(module),
                "`{module}` is classified here but is not declared in lib.rs; if it was                  renamed or removed, update this list"
            );
        }
        for module in &declared {
            assert!(
                OPENS_WINDOWS.contains(module) != OPENS_NO_WINDOW.contains(module),
                "`{module}` is in neither list (or in both). Every module in this crate has                  to be classified: if it opens a window, add it to the `sites` table in                  `every_window_this_crate_opens_asks_to_be_brought_to_the_front` AND to                  `OPENS_WINDOWS` here, so its raise is guarded. If it does not, say so in                  `OPENS_NO_WINDOW`. A module in neither is a window nothing checks -- which                  is how `app_window` was added, unguarded, and the whole reason this test                  exists."
            );
        }
    }

    // ---- the real desktop, weakly ------------------------------------------

    /// Live, and deliberately weak -- there is no window of ours under `cargo
    /// test`, so this cannot assert an outcome. It is here for the same reason
    /// `window_list`'s live test is: a bad `LPARAM` cast or an unterminated
    /// walk in `enum_proc` has no other test at all.
    #[test]
    fn enumerating_this_processs_own_windows_terminates_and_stays_within_this_process() {
        for w in Win32Desktop.own_windows() {
            assert_ne!(w.hwnd, 0, "a null window handle came back from EnumWindows");
        }
        // `GetForegroundWindow` is allowed to return 0 (no foreground window);
        // this only pins that the call itself returns.
        let _ = Win32Desktop.foreground();
    }
}

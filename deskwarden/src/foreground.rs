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

/// The handle of **this process's** top-level window titled `title`, if it
/// has one.
///
/// The process-scoped answer to "which window is ours". `FindWindowW(None,
/// title)` is the obvious way to ask and it is the wrong one: it walks the
/// whole desktop and returns whichever window `EnumWindows` reaches first,
/// so a File Explorer window open on a folder named `Deskwarden` -- or a
/// second copy of this app -- answers it. Callers here only ever want their
/// OWN window, and `win32::own_windows` already filters by
/// `GetCurrentProcessId`, so this is both the safer question and the one
/// that was actually meant.
///
/// Unlike [`pick`] this does NOT skip invisible windows: its one caller
/// ([`crate::login_ui::round_window_corners`]) runs from the first painted
/// frame, before the window is necessarily mapped, and a corner preference
/// set on a not-yet-visible window is applied when it appears.
///
/// In a test process, which has no windows at all, this is `None` and every
/// caller becomes a no-op -- which is why no test can reach out of the
/// process through it.
pub fn own_window_titled(title: &str) -> Option<isize> {
    win32::own_windows().into_iter().find(|w| w.title == title).map(|w| w.hwnd)
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

    /// A module's source with its comments cut off, so a needle counted below
    /// can only be satisfied by code.
    ///
    /// The same helper `app_window.rs`'s source-position tests use, and for the
    /// same reason: `include_str!` hands back the file's raw bytes, so a guard
    /// that counts `with_always_on_top(` was satisfiable by writing that text
    /// into a doc comment. The M2E escape (delist `loading_ui` from
    /// [`RAISING_SITES`], excuse it, delete its `raise_window` call) went green
    /// with one planted line reading
    /// `/// Kept above everything with with_always_on_top( in the builder.`
    /// Everything from the first `//` on a line is dropped, which also takes
    /// the tail of a line whose *string literal* contains `//` -- accepted,
    /// because the needles here name Rust call syntax and never a URL.
    fn code(source: &str) -> String {
        source
            .lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. The same shape `breach.rs` walks its own cut
    /// with, deliberately exact rather than a `starts_with`: a whole module
    /// written on one line is not a module opener as far as this walk is
    /// concerned.
    fn is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// **A module's source with its test modules cut out**, and how many were
    /// cut.
    ///
    /// The reason every cross-file needle below is counted over this rather
    /// than over the raw `include_str!` bytes. An exact-count pin -- and the
    /// raise guard pins `raise_window(WINDOW_TITLE)` at exactly ONE occurrence
    /// in each of six other files -- **false-fires the day a fixture in one of
    /// those files spells the needle**, which none of those files' owners has
    /// any reason to expect. A guard that fires for a reason unrelated to what
    /// it guards gets deleted rather than obeyed, and a deleted guard is how
    /// the original window-behind-everything bug survived in the first place.
    ///
    /// Not "everything above the first `#[cfg(test)]`", which is what
    /// `settings.rs` and `breach.rs` can do for themselves: these six files
    /// **interleave** production and test modules -- `app_window.rs` has a test
    /// module at line 790 and a `raise_window` call at 1814 -- so a
    /// split-at-the-first-gate would truncate the very lines being pinned. It
    /// would fail loudly rather than silently, the count going to zero, but it
    /// would not work at all.
    ///
    /// So each gated module is skipped instead: a line that is exactly
    /// `#[cfg(test)]` followed immediately by a column-0 module opener starts a
    /// skip that runs to the next column-0 `}`. Inside a module every item is
    /// indented, so that brace is the module's own.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison here is against the line's real
    /// text on a CRLF working tree and on an LF one alike. This repository
    /// stores LF blobs and only `core.autocrlf=true` makes the working tree
    /// CRLF, so a needle written with a carriage return in it would match
    /// nothing on a plain checkout -- green, and reading nothing.
    fn production_half(source: &str) -> (String, usize) {
        let mut kept: Vec<&str> = Vec::new();
        let mut cut = 0usize;
        let mut gated = false;
        let mut skipping = false;
        for line in source.lines() {
            if skipping {
                if line == "}" {
                    skipping = false;
                }
                continue;
            }
            if gated && is_module_opener(line) {
                // The `#[cfg(test)]` line itself was pushed on the previous
                // turn; it belongs to the module being cut.
                kept.pop();
                skipping = true;
                cut += 1;
                gated = false;
                continue;
            }
            gated = line.trim() == "#[cfg(test)]";
            kept.push(line);
        }
        assert!(
            !skipping,
            "a test module was opened and never closed by a column-0 brace, so the rest of the \
             file was dropped and every needle counted over this reads nothing"
        );
        (kept.join("\n"), cut)
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

    /// **Every module that opens a window and raises it**, with the source it
    /// lives in, the title identifier it opens under, and how many windows.
    ///
    /// A `const` rather than a local of
    /// `every_window_this_crate_opens_asks_to_be_brought_to_the_front`, because
    /// `every_module_in_this_crate_is_classified_as_opening_windows_or_not`
    /// reconciles `OPENS_WINDOWS` against *this exact list*. It used to hold a
    /// second, hand-written copy of the same knowledge, and two lists that
    /// agree by hand is not a guard: deleting `loading_ui`'s row from here and
    /// its `raise_window` call from `loading_ui.rs` left the whole suite green,
    /// because the classification test still found `loading_ui` in
    /// `OPENS_WINDOWS` and asked nothing further.
    ///
    /// First field is the MODULE name, not the file name, so it can be
    /// compared with the module list parsed out of `lib.rs` without a second
    /// mapping in between (`vault_window` is the only one where those differ,
    /// and it is exactly the row a hand-written mapping would get wrong).
    /// The modules that put a window on screen. Each must then appear in
    /// exactly one of [`RAISING_SITES`] (it raises, and
    /// `every_window_this_crate_opens_asks_to_be_brought_to_the_front` holds
    /// that) or [`OPENS_A_WINDOW_AND_DELIBERATELY_DOES_NOT_RAISE`] (it does
    /// not, and carries the reason). This list used to claim to be "the modules
    /// that test covers", which was false: `overlay_ui` was in it and covered
    /// by nothing.
    ///
    /// At this scope rather than inside
    /// `every_module_in_this_crate_is_classified_as_opening_windows_or_not`,
    /// where it used to live, so that
    /// `only_one_window_of_this_process_can_exist_at_a_time` can count against
    /// it. That test's coverage control used to re-derive its expectation from
    /// the two tables it was chaining, which made it unfailable; this list is
    /// reconciled with `lib.rs` by a different test, so counting against it is
    /// a claim that can actually come out false.
    const OPENS_WINDOWS: [&str; 9] = [
        "app_window",
        "loading_ui",
        "login_ui",
        "overlay_ui",
        "picker_ui",
        // The 4b preflight confirmation. Excused from raising for the same
        // reasons `overlay_ui` is, and for one more that is stronger -- see
        // its row in `OPENS_A_WINDOW_AND_DELIBERATELY_DOES_NOT_RAISE`.
        "preflight_host",
        "prefs_ui",
        // The 4d rehearsal scratch window. The one window in this crate
        // `eframe` does not start a loop for -- it is an egui VIEWPORT, opened
        // inside the vault window's running loop. See
        // [`OPENS_A_VIEWPORT_AND_RAISES_IT`], which is the table that holds
        // its raise.
        "scratch_window",
        "vault_window",
    ];

    const RAISING_SITES: [(&str, &str, &str, usize); 6] = [
        ("vault_window", include_str!("vault_window/mod.rs"), "WINDOW_TITLE", 1),
        // The single startup window: sign-in, spinner and vault in one
        // event loop. It is the ONLY window on the launch that signs in,
        // so a raise it does not ask for is a launch that lands behind
        // whatever the user was doing -- with no second window afterwards
        // to correct it, which is what the three-window flow accidentally
        // provided.
        ("app_window", include_str!("app_window.rs"), "WINDOW_TITLE", 1),
        // Added when the login window got its raise. It was excluded
        // while another agent owned that file, and an excluded site is
        // an unguarded one: the raise was applied there and nothing in
        // this test would have noticed it being deleted again.
        ("login_ui", include_str!("login_ui.rs"), "WINDOW_TITLE", 1),
        ("prefs_ui", include_str!("prefs_ui.rs"), "WINDOW_TITLE", 1),
        ("loading_ui", include_str!("loading_ui.rs"), "WINDOW_TITLE", 1),
        // Two windows, two titles; the second is checked just below the
        // loop rather than in it.
        ("picker_ui", include_str!("picker_ui.rs"), "PICK_ITEM_TITLE", 1),
    ];

    /// **Opens a window `eframe` did not start a loop for, and raises it.**
    ///
    /// The third category, and it exists because [`RAISING_SITES`] holds its
    /// windows by grepping for `run_ui_native(TITLE,` -- a needle that names
    /// `eframe`'s entry point and nothing else. `scratch_window` opens neither
    /// that nor a Win32 window: a rehearsal is started from *inside* the vault
    /// window's event loop, `winit` will not build a second one, and the answer
    /// is `show_viewport_deferred` -- a second real OS window on the SAME loop,
    /// painted by egui. It would count zero under `run_ui_native(`.
    ///
    /// Zero is exactly the wrong answer. Left in [`RAISING_SITES`] the guard
    /// would fail on a window that does everything right; moved to the
    /// exemption list it would be excused from raising when it is the one
    /// window in this crate the user is meant to sit and watch. So it is held
    /// here instead, by the needle that names how it really opens.
    ///
    /// **This table was `OPENS_A_WIN32_WINDOW_AND_RAISES_IT`, and its needle
    /// was `CreateWindowExW(`.** That was true of the rehearsal window's first
    /// implementation, which was a raw Win32 window with system `EDIT` controls
    /// and was therefore the only surface in the product with none of the app's
    /// theme, tokens or type. The table is renamed rather than joined by a
    /// fourth: there is no Win32 window left in this crate for the old one to
    /// hold, and a table kept alive for a case that no longer exists is a guard
    /// that counts zero forever.
    const OPENS_A_VIEWPORT_AND_RAISES_IT: [(&str, &str, &str); 1] =
        [("scratch_window", include_str!("scratch_window.rs"), "SCRATCH_TITLE")];

    /// **Opens a window, and deliberately does not raise it -- because.**
    ///
    /// The third category, and the one whose absence made the docstring on
    /// `OPENS_WINDOWS` false as written: `overlay_ui` opens a window, has never
    /// had a row in [`RAISING_SITES`], and nothing anywhere recorded that this
    /// was a decision rather than the same omission that let `app_window` in
    /// unguarded. "Not in the raise list" and "must not be in the raise list"
    /// are different claims and now live in different places.
    ///
    /// The reason is carried in the source because it is the whole content of
    /// the exemption; the test below refuses a blank one, and refuses a module
    /// listed here that turns out to raise after all.
    const OPENS_A_WINDOW_AND_DELIBERATELY_DOES_NOT_RAISE: [(&str, &str, &str); 2] = [
        (
        "preflight_host",
        include_str!("preflight_host.rs"),
        "The preflight is `with_always_on_top()`, so the OS already keeps it above everything.          It also opens while ANOTHER app is foreground -- it is the confirmation shown BEFORE a          sequence is typed into that app -- and it opens under the same literal `\"Deskwarden\"`          title `vault_window`, `app_window` and `loading_ui` all raise under, so a          `raise_window` here could bring one of those forward instead. And there is a reason          particular to this window: `preflight::verdict` was computed from the foreground          described a moment before it opened, and `dispatch_with` describes the foreground          again after it closes. A raise is a deliberate change to which window is in front,          made by the one surface in the app whose whole job is to tell the truth about which          window is in front.",
    ),
    (
        "overlay_ui",
        include_str!("overlay_ui.rs"),
        "The autofill prompt is `with_always_on_top()`, so the OS already keeps it above \
         everything and a raise would buy nothing. It is also the one window of ours that \
         opens while ANOTHER app is foreground -- anchored beside the field the user is in, \
         whose `hwnd` the fill is injected back into once the card is clicked -- so taking \
         the foreground is the opposite of what this window wants. And it opens through \
         `eframe::run_native(\"Deskwarden\", ..)` -- the same literal title THREE other \
         windows open under: `vault_window`, `app_window` and `loading_ui`, all of which do \
         raise. (It said \"two\" while there were three; the count is now the one \
         `only_one_window_of_this_process_can_exist_at_a_time` reads off the sources.) \
         `raise_window` matches this process's own windows BY TITLE and `pick` takes the \
         FIRST match in `EnumWindows` order, so a raise here could just as easily bring one \
         of those forward instead.",
    )];

    /// Every window this crate opens must actually ask to be raised, and must
    /// ask for the SAME title it opened under. Neither can be asserted by
    /// running anything: `eframe::run_ui_native` blocks on a real OS event
    /// loop and opens a real window, so no test in this crate calls any of
    /// these window functions -- deleting a `raise_window` call leaves the
    /// whole suite green, and the user gets the window-behind-everything they
    /// reported back. So it is held by source position instead. ("these four"
    /// is what this said while there were four; a count in prose goes stale
    /// silently, and every count in this test is now read off
    /// [`RAISING_SITES`].)
    ///
    /// What it can see: the call is present, once per window. What it cannot:
    /// that it runs on the frame the window first exists on rather than
    /// somewhere unreachable. That much is visible in any diff touching these
    /// lines, and is what the comment at each call site is for.
    ///
    /// The list itself is [`RAISING_SITES`], and a module missing from it is
    /// not merely unchecked here -- it fails
    /// `every_module_in_this_crate_is_classified_as_opening_windows_or_not`,
    /// which is what makes delisting alone insufficient to escape this test.
    #[test]
    fn every_window_this_crate_opens_asks_to_be_brought_to_the_front() {
        for (name, source, title, opens) in RAISING_SITES {
            // **The production half, not the whole file.** These are exact
            // counts over six OTHER modules' sources, and an exact count over
            // a whole file false-fires the day a fixture in one of them spells
            // the needle -- see `production_half`.
            let (source, cut) = production_half(source);
            assert!(
                cut > 0,
                "no test module was cut out of `{name}`, so this is still counting over that \
                 file's fixtures as well as its code"
            );
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

        let (picker, cut) = production_half(include_str!("picker_ui.rs"));
        assert!(cut > 0, "no test module was cut out of `picker_ui`");
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

        // **Positive control on the cut itself, which is the new thing here.**
        // Two claims, and neither is provable from the counts above:
        //
        // 1. It removes a gated test module. Fed a file whose test module
        //    spells the needle a second time, the raw count is 2 -- the false
        //    fire this change exists to stop -- and the cut count is 1.
        // 2. It removes ONLY that, and specifically not production that
        //    happens to sit below a test module. That is the interleaving
        //    every one of these six files has and the reason this is a walk
        //    rather than a split at the first gate: a split would read 1 here
        //    (the fixture's, having thrown the second real call away) and this
        //    control would still pass on the count alone. So the tail is
        //    asserted to have survived by name.
        let interleaved = concat!(
            "fn open() { run_ui_native(WINDOW_TITLE, ..); raise_window(WINDOW_TITLE); }\n",
            "#[cfg(test)]\n",
            "mod fixtures {\n",
            "    const SAMPLE: &str = \"raise_window(WINDOW_TITLE)\";\n",
            "}\n",
            "fn open_later() { raise_window(WINDOW_TITLE); let _ = SURVIVOR; }\n"
        );
        assert_eq!(
            interleaved.matches("raise_window(WINDOW_TITLE)").count(),
            3,
            "control: the fixture no longer spells the needle, so the cut below proves nothing"
        );
        let (cut_fixture, cuts) = production_half(interleaved);
        assert_eq!(cuts, 1, "the walk did not find the gated test module");
        assert_eq!(
            cut_fixture.matches("raise_window(WINDOW_TITLE)").count(),
            2,
            "the walk did not remove the occurrence inside the test module"
        );
        assert!(
            cut_fixture.contains("SURVIVOR"),
            "the walk threw away production below the test module, which is exactly what a \
             split at the first `#[cfg(test)]` would have done to these six files"
        );
    }

    /// The same claim [`every_window_this_crate_opens_asks_to_be_brought_to_the_front`]
    /// makes, for the window `eframe` did not start a loop for.
    ///
    /// What it can see: the module really opens a viewport, really opens it
    /// under the title it raises -- which is the whole of why this window may
    /// be alive alongside the vault window, see
    /// [`only_one_window_of_this_process_can_exist_at_a_time`] -- really raises
    /// that title, and has not quietly become an `eframe` window, which would
    /// move it into the other table's remit and out of this one's with both
    /// guards then reading zero.
    ///
    /// **Exactly one viewport, not at least one.** A second
    /// `show_viewport_deferred` here would be a second OS window that no table
    /// holds and no title distinguishes, which is precisely the collision the
    /// title-uniqueness argument exists to rule out.
    #[test]
    fn every_viewport_window_this_crate_opens_asks_to_be_brought_to_the_front() {
        for (name, source, title) in OPENS_A_VIEWPORT_AND_RAISES_IT {
            // The production half and then `code()`, for both reasons the
            // exemption loop below gives: an exact count over another module's
            // file false-fires on a fixture that spells the needle, and a
            // comment can hold a needle as well as a call can.
            let (production, cut) = production_half(source);
            assert!(cut > 0, "no test module was cut out of `{name}`");
            let source = code(&production);
            assert_eq!(
                source.matches("show_viewport_deferred(").count(),
                1,
                "`{name}` is listed as opening exactly one viewport and opens a different \
                 number of them"
            );
            assert_eq!(
                source.matches(&format!("with_title({title})")).count(),
                1,
                "`{name}` raises `{title}` but does not OPEN its viewport under that title. \
                 `own_window_titled` matches on the string, so a viewport built without it is a \
                 window no raise can find and no rehearsal has a target in."
            );
            assert_eq!(
                source.matches(&format!("raise_window({title})")).count(),
                1,
                "`{name}` opens a window titled `{title}` and must raise that same window"
            );
            assert_eq!(
                source.matches("run_ui_native(").count(),
                0,
                "`{name}` has become an `eframe` window. Move it to `RAISING_SITES`, which \
                 counts `run_ui_native(TITLE,` -- otherwise neither table holds its raise."
            );
        }
        // Positive control on the two zero-counts' needles, through the same
        // `code()` the loop uses: they can be found where they really are, so a
        // zero above is an absence and not a typo.
        assert!(RAISING_SITES.iter().all(|(_, s, ..)| code(s).contains("run_ui_native(")));
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
    ///
    /// **And being classified as opening a window is not enough on its own.**
    /// Until this test reconciled the two, `OPENS_WINDOWS` and the raise list
    /// were independent hand-written lists that merely happened to agree, so a
    /// module could be dropped from the raise table and lose its raise with
    /// every assertion still passing -- `loading_ui` was demonstrably deletable
    /// that way. It also let `overlay_ui` sit in `OPENS_WINDOWS` with no row in
    /// the raise table at all and nothing recording why. Both are closed below,
    /// by requiring `OPENS_WINDOWS` to be exactly `RAISING_SITES` plus
    /// `OPENS_A_WINDOW_AND_DELIBERATELY_DOES_NOT_RAISE`, with nothing in two
    /// lists and nothing in none.
    #[test]
    fn every_module_in_this_crate_is_classified_as_opening_windows_or_not() {
        /// Everything else. Listed rather than inferred, because "this module
        /// does not open a window" is a decision someone has to make; a module
        /// missing from BOTH lists fails below rather than being quietly
        /// unguarded.
        const OPENS_NO_WINDOW: [&str; 43] = [
            "accounts",
            "app",
            // Reads an executable's version resource and its shell icon.
            // Draws nothing itself; the edit form paints what it returns.
            "app_identity",
            "app_match",
            // Creates one named kernel object and holds its handle, so the
            // installer can tell that this app is running. No UI of any kind:
            // the window this module causes to appear is Inno Setup's "please
            // close Deskwarden" box, which belongs to setup, lives in setup's
            // process, and is the whole point -- see `app_mutex`'s own docs
            // for why setup asks rather than force-closing.
            "app_mutex",
            "backend_policy",
            // A test-only brace matcher over source text. It is
            // `#[cfg(test)]` at its declaration and draws nothing.
            "below_cut",
            // Hashes a password and talks to the Have I Been Pwned range
            // API. Pure logic plus one HTTP call; it draws nothing.
            "breach",
            "bw_path",
            "bw_serve",
            // The card network table: a prefix lookup and two string masks,
            // pure data with no I/O. The panes that draw what it returns are
            // `detail` and `item_list`, and neither is this module.
            "card_brand",
            // The network marks: a lookup returning `include_bytes!`-ed PNG
            // bytes. It hands a picture to whoever is painting; the tile and
            // the detail pane draw it, and neither is this module.
            "card_mark",
            // Reads this crate's own source to check that no type carrying a
            // secret derives `Debug`. Like `below_cut` it is `#[cfg(test)]`
            // at its declaration and draws nothing.
            "debug_leak_guard",
            // Puts a copied secret on the Windows clipboard through raw Win32
            // and decides when to take it back off again. It talks to the
            // system clipboard, which is not a window: it opens nothing,
            // paints nothing, and has no title for `raise_window` to match.
            "clipboard",
            "dispatch",
            "favicon",
            // **Judgement call, recorded rather than assumed.** It puts the
            // shell's `IFileOpenDialog` on screen, so "opens no window" is
            // true only in the sense this list means it: the tables above are
            // about windows THIS crate opens through `eframe::run_ui_native`
            // and must therefore raise itself, and this is not one -- it is a
            // system-modal dialog the shell owns, foregrounds and destroys,
            // with no title constant of ours and nothing for `raise_window`
            // to match on. `RAISING_SITES` greps for `run_ui_native(TITLE,`,
            // which this file has not got and cannot be given.
            "file_picker",
            "fill_stats",
            "foreground",
            "hello",
            "hotkey",
            "http_agent",
            "icon",
            "injector",
            "job_object",
            "key_sequence",
            "logging",
            "match_engine",
            "password_strength",
            // The record payload written into a Send and read back out of
            // one, plus the passphrase seal over the TOTP seed. Pure data
            // with no I/O at all; the surfaces that will draw it are a later
            // step and are not this module.
            "record",
            // **Judgement call, recorded rather than assumed**, and the same
            // one `file_picker` above carries. The master-password re-prompt
            // does put a window on screen -- the Windows Hello verification
            // dialog -- but it is not a window THIS crate opens: it belongs
            // to the OS, which foregrounds and destroys it, and this module
            // has no title constant and no `run_ui_native` call for
            // `RAISING_SITES` to find. The module itself is a decision and a
            // gate over one call into `hello`, which is on this list for
            // exactly the same reason.
            "reprompt",
            // Builds the argument vector, the stdin JSON and the failure
            // classification for `bw send`. Pure data; the Sends screen that
            // will draw it is a later step and is not this module.
            "send",
            "session_store",
            "settings",
            "signature",
            "theme",
            "tray",
            "updater",
            "vault_bridge",
            "vault_cache",
            // Plans an export, builds the `bw export` command and
            // classifies the result. The save dialog it is pointed at
            // is the shell's, opened elsewhere; this module draws
            // nothing and calls no `run_ui_native`.
            "vault_export",
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

        // ---- and the raise lists reconcile with OPENS_WINDOWS --------------
        let raises: Vec<&str> = RAISING_SITES
            .iter()
            .map(|(module, ..)| *module)
            .chain(OPENS_A_VIEWPORT_AND_RAISES_IT.iter().map(|(module, ..)| *module))
            .collect();
        let excused: Vec<&str> = OPENS_A_WINDOW_AND_DELIBERATELY_DOES_NOT_RAISE
            .iter()
            .map(|(module, ..)| *module)
            .collect();

        // Positive controls on the two projections, so every check below is
        // reading real names rather than iterating an empty vector.
        assert_eq!(
            raises.len(),
            RAISING_SITES.len() + OPENS_A_VIEWPORT_AND_RAISES_IT.len(),
            "control: a name per raising site, `eframe`'s loops and the viewport alike"
        );
        assert!(
            raises.contains(&"scratch_window"),
            "control: the viewport raise table is being read at all: {raises:?}"
        );
        assert!(
            raises.contains(&"vault_window"),
            "control: the raise table is being read, and by module name rather than file name \
             -- `vault_window` lives in `vault_window/mod.rs` and is the one row where those \
             differ: {raises:?}"
        );
        assert!(!excused.is_empty(), "control: the exemption list is being read");

        for module in OPENS_WINDOWS {
            assert!(
                raises.contains(&module) != excused.contains(&module),
                "`{module}` opens a window but is in neither `RAISING_SITES` nor \
                 `OPENS_A_WINDOW_AND_DELIBERATELY_DOES_NOT_RAISE` (or is in both). Saying a \
                 module opens a window is only half the decision; the other half is whether it \
                 brings that window to the front. If it should raise, add its row to \
                 `RAISING_SITES` so \
                 `every_window_this_crate_opens_asks_to_be_brought_to_the_front` holds the call \
                 -- delisting it from there is otherwise enough to delete the raise with the \
                 whole suite green, which is exactly what `loading_ui` was shown to do. If it \
                 should not, say so in the exemption list AND say why."
            );
        }
        for module in raises.iter().chain(excused.iter()) {
            assert!(
                OPENS_WINDOWS.contains(module),
                "`{module}` is listed as opening a window by the raise tables but is missing \
                 from `OPENS_WINDOWS`, so the two disagree about what this crate even puts on \
                 screen"
            );
        }

        // The exemption has to be falsifiable, or it is just a place to put
        // modules to make this test stop asking. A blank reason is not a
        // decision, and a module that DOES raise is in the wrong list -- which
        // is the failure mode of an excuse nobody rereads.
        for (module, source, reason) in OPENS_A_WINDOW_AND_DELIBERATELY_DOES_NOT_RAISE {
            assert!(
                reason.len() > 40,
                "`{module}` is excused from raising with no reason worth the name: {reason:?}"
            );
            // **And the reason has to be true of the source, not merely long.**
            // A reason string is prose: `loading_ui` was demonstrably
            // delistable from `RAISING_SITES` into this list behind a
            // plausible forty-character excuse, with its `raise_window` call
            // deleted and the whole suite green. What separates the real
            // exemption from that forgery is structural, and it is the
            // exemption's own first sentence: the OS keeps this window above
            // everything because the window asks it to. `with_always_on_top(`
            // appears exactly once in `overlay_ui.rs` and nowhere else in this
            // crate, so this both holds the real excuse to its claim and fails
            // any module moved here that cannot make it.
            //
            // **Counted over `code()`, not the raw file.** Against the raw
            // `include_str!` bytes the whole M2E escape went green with one
            // extra planted line -- a doc comment mentioning
            // `with_always_on_top(` -- because a comment can hold the needle
            // as well as a call can. Stripping comments first means only a
            // real builder call satisfies this, and only a real call site
            // trips the `raise_window(` count below.
            //
            // **And over the production half, for the same reason the raise
            // guard is**: this is an exact count over another module's file,
            // so a fixture in `overlay_ui.rs`'s own test modules spelling
            // `with_always_on_top(` would take it to 2 and false-fire.
            let (production, cut) = production_half(source);
            assert!(cut > 0, "no test module was cut out of `{module}`");
            let source = code(&production);
            assert_eq!(
                source.matches("with_always_on_top(").count(),
                1,
                "`{module}` is excused from raising because the OS already keeps it above \
                 everything -- but its window is not `with_always_on_top()`. A window that is \
                 neither raised nor always-on-top is a window that can open behind something."
            );
            assert_eq!(
                source.matches("raise_window(").count(),
                0,
                "`{module}` is listed as deliberately not raising, but its source calls \
                 `raise_window(`. Move it to `RAISING_SITES` -- with the title identifier it \
                 opens under -- so the call is actually guarded. The stale excuse reads: \
                 {reason:?}"
            );
        }
        // Positive control for that last needle, through the same
        // comment-stripping the loop uses: it can find a raise where one
        // really is, so a count of 0 above is an absence and not a typo -- and
        // `code()` does not eat the real calls while eating the fake ones.
        assert_eq!(
            RAISING_SITES
                .iter()
                .filter(|(_, source, ..)| code(source).contains("raise_window("))
                .count(),
            RAISING_SITES.len(),
            "control: every raising site's source really does contain the needle this counted \
             to zero in the exempt ones"
        );
        // And a control on `code()` itself, in both directions -- a stripper
        // that stripped everything, or nothing, would make one of the two
        // counts above vacuous. The first line is the exact plant that took
        // the M2E escape green.
        let sample = code(concat!(
            "/// Kept above everything with with_always_on_top( in the builder.\n",
            "    let builder = builder.with_always_on_top();\n",
            "    raise_window(WINDOW_TITLE); // and this trailing one goes too\n",
        ));
        assert_eq!(
            sample.matches("with_always_on_top(").count(),
            1,
            "control: `code()` drops the needle written in a comment and keeps the one written \
             as a call -- if it dropped both, the exemption's always-on-top check passes for \
             every module; if it dropped neither, a comment is still enough to satisfy it: \
             {sample:?}"
        );
        assert_eq!(
            sample.matches("raise_window(").count(),
            1,
            "control: `code()` strips a trailing comment without eating the code before it: \
             {sample:?}"
        );
    }

    /// **The invariant that makes matching a window BY TITLE safe at all.**
    ///
    /// [`RAISING_SITES`] compares title *identifiers*; it never looks at their
    /// values. **Five** modules open a window titled literally `"Deskwarden"`
    /// -- `vault_window`, `app_window` and `loading_ui`, which all raise, and
    /// `overlay_ui` and `preflight_host`, which are excused. (This said "four"
    /// while there were four, and went stale the day the preflight host
    /// landed; a count in prose goes stale silently, which is why the one
    /// below is asserted.) [`pick`] is a `find`, so if two of them were ever on
    /// screen together a raise would take whichever `EnumWindows` happened to
    /// hand back first, silently and non-deterministically.
    ///
    /// **`scratch_window` is the sixth window and is deliberately not the sixth
    /// of those**, because it is the one window here that provably IS alive
    /// alongside another: it is opened from inside the vault window's frame
    /// closure and pumps its own message loop while that window is still up.
    /// Serialisation is therefore not what makes it safe -- a unique title is,
    /// and that is asserted below rather than left in prose.
    ///
    /// For the five that do share a title, the collision is not reachable
    /// today, and the reason is worth naming because it is the thing being
    /// pinned rather than the collision itself:
    ///
    /// * **No `with_any_thread`.** On Windows, `winit` refuses to build an
    ///   event loop off the main thread -- it panics -- unless the builder was
    ///   given `with_any_thread()`. So without that call anywhere, every
    ///   `eframe::run_*native` in this crate is on the main thread, and a
    ///   window opened from a spawned thread is not a second window but a
    ///   crash. This is what makes the spawn side unnecessary to check
    ///   separately: `with_any_thread` is the only door to it.
    /// * **One viewport per loop, and exactly one place that opens one.**
    ///   `show_viewport*` opens a second OS window inside a running loop, on
    ///   the same thread, with no `run_*native` call for `RAISING_SITES` to
    ///   count and no title for it to compare. This bullet said "Nothing in
    ///   this crate calls it" until design 4d's rehearsal window stopped being
    ///   a raw Win32 window and became one. `scratch_window` now calls
    ///   `show_viewport_deferred` exactly once, under `SCRATCH_TITLE`, and
    ///   `OPENS_A_VIEWPORT_AND_RAISES_IT` is the table that holds it. The count
    ///   below is therefore per-module rather than globally zero -- and the
    ///   title it opens under is asserted, above, to be distinct from the one
    ///   the five `eframe` windows share, which is what keeps `pick`'s `find`
    ///   exact even while two windows are on screen.
    ///
    /// Together with `run_*native` blocking until its window closes -- which
    /// the queued-tray-click behaviour is itself the evidence of -- that makes
    /// the four windows strictly one-at-a-time, and `pick`'s `find` exact.
    ///
    /// **Why this rather than distinct titles.** Asserting the raising sites'
    /// title strings are pairwise distinct would fail today, so it is not a
    /// guard that can be added -- it is a demand to rename three windows. That
    /// rename was rejected: the titles are load-bearing outside this crate.
    /// `round_window_corners` (`login_ui.rs`) reaches for its window by title
    /// through [`own_window_titled`] -- process-scoped now, but still matching
    /// on the string -- and `raise_window` matches on it too; and all
    /// three files are owned elsewhere. Distinct titles would also only remove
    /// the *consequence*. Serialization is the property the code actually
    /// relies on, so it is the one written down here.
    #[test]
    fn only_one_window_of_this_process_can_exist_at_a_time() {
        // **The one window that is not serialised with the rest**, and the one
        // property that makes that safe. `scratch_window` is open while the
        // vault window is, so a title it shared with anything would make
        // `pick`'s `find` non-deterministic for exactly as long as a rehearsal
        // is on screen -- which is when a `SendInput` burst is in flight.
        //
        // Two comparisons and not one: these are the two title constants this
        // module can name, and both spell the literal the other three windows
        // also open under, so a `SCRATCH_TITLE` changed to `"Deskwarden"` fails
        // here rather than in production.
        assert_ne!(
            crate::vault_window::rehearsal::SCRATCH_TITLE,
            crate::vault_window::WINDOW_TITLE,
            "the rehearsal scratch window shares a title with a window it is open alongside"
        );
        assert_ne!(
            crate::vault_window::rehearsal::SCRATCH_TITLE,
            crate::preflight_host::PREFLIGHT_TITLE
        );
        // Control on the pair above: those two constants really are the same
        // string, so an `assert_ne!` against either is an assertion about the
        // shared literal and not about two unrelated names.
        assert_eq!(crate::vault_window::WINDOW_TITLE, crate::preflight_host::PREFLIGHT_TITLE);

        let sources = RAISING_SITES
            .iter()
            .map(|(module, source, ..)| (*module, *source))
            .chain(
                OPENS_A_VIEWPORT_AND_RAISES_IT
                    .iter()
                    .map(|(module, source, _)| (*module, *source)),
            )
            .chain(
                OPENS_A_WINDOW_AND_DELIBERATELY_DOES_NOT_RAISE
                    .iter()
                    .map(|(module, source, _)| (*module, *source)),
            );
        // Positive control: the loop below really walks every window module.
        //
        // Counted against [`OPENS_WINDOWS`] and not against the lengths of the
        // two tables being chained. That is what this used to do --
        // `sources.count() == RAISING_SITES.len() + EXCUSED.len()` -- and
        // `sources` is *built* by chaining exactly those two iterators, so it
        // re-derived its expectation from the thing under test and could not
        // come out false for any edit at all. `OPENS_WINDOWS` is an
        // independently written list, reconciled with `lib.rs`'s `pub mod`
        // lines by
        // `every_module_in_this_crate_is_classified_as_opening_windows_or_not`,
        // so a window module dropped from both tables now fails here too.
        let walked: Vec<&str> = sources.clone().map(|(module, _)| module).collect();
        for module in OPENS_WINDOWS {
            assert!(
                walked.contains(&module),
                "`{module}` opens a window but no source for it reaches the checks below, so \
                 nothing says it cannot open a second one off the main thread: {walked:?}"
            );
        }
        assert_eq!(
            walked.len(),
            OPENS_WINDOWS.len(),
            "the window modules walked here do not match `OPENS_WINDOWS` one for one -- either \
             a module is listed in both raise tables, or one is checked here that is not \
             classified as opening a window at all: {walked:?}"
        );

        for (module, source) in sources {
            // Control on the string, not on the needle: both counts below are
            // zero, and a zero over the wrong string -- an `include_str!`
            // pointed elsewhere, or a table row whose module stopped opening a
            // window -- is indistinguishable from a zero over the right one.
            // Every window-opening module in this crate builds a viewport.
            //
            // The two counts below are deliberately NOT taken over `code()`,
            // unlike the exemption's needles: those must be 1, so a comment
            // holding the needle is an escape; these must be 0, so a comment
            // holding it is a false alarm and nothing worse. Raw is the
            // stricter side of that trade, and the strict side is the one a
            // zero-count guard wants. (The cost is that documenting
            // `with_any_thread` by name inside a window module fails this test
            // -- which is a build failure that says exactly what it means.)
            assert!(
                source.contains("ViewportBuilder"),
                "control: `{module}`'s source does not build a viewport at all, so it is
                 either not the file this thinks it is or not a window-opening module -- and
                 the counts below are then zero for a reason that has nothing to do with the
                 invariant. (This used to accept `CreateWindowExW` as well, for the rehearsal
                 window's first, raw-Win32 implementation. That window is now an egui
                 viewport, and every window this crate opens builds a `ViewportBuilder`.)"
            );
            assert_eq!(
                source.matches("with_any_thread").count(),
                0,
                "`{module}` builds its event loop with `with_any_thread`, which lets a window \
                 open off the main thread. Four of this crate's windows are titled literally \
                 `\"Deskwarden\"`, and `raise_window` matches by title with `pick` taking the \
                 first hit in `EnumWindows` order -- so two of them alive at once is a raise \
                 that brings an arbitrary one forward. Without this call winit panics off the \
                 main thread, which is what keeps them one at a time."
            );
            // **This count is zero for every module except the one that
            // deliberately opens a viewport, and that module is checked
            // elsewhere rather than loosely here.**
            //
            // The zero is raw -- not over `code()`, not over
            // `production_half` -- for the reason given above: it must be 0, so
            // a comment holding the needle is a false alarm and nothing worse,
            // and raw is the stricter side of that trade. That strictness is
            // exactly why `scratch_window` cannot be checked here for a count
            // of ONE: its own header and its own tests discuss
            // `show_viewport_deferred` by name, and a raw count over that file
            // is seven. Loosening this to `code(production_half(..))` for
            // everyone would weaken the zero for the eight modules the zero is
            // actually about.
            //
            // So the viewport module is skipped here and held by
            // `every_viewport_window_this_crate_opens_asks_to_be_brought_to_the_front`,
            // which counts `show_viewport_deferred(` over that file's stripped
            // production half and requires exactly one -- together with the
            // `with_title(SCRATCH_TITLE)` that gives the window the unique
            // title this whole test rests on. Skipping is safe because the
            // module is not skipped from the loop: the `with_any_thread` count
            // above still runs over it, and
            // `every_module_in_this_crate_is_classified_as_opening_windows_or_not`
            // still requires it to be in exactly one raise table.
            if OPENS_A_VIEWPORT_AND_RAISES_IT.iter().any(|(name, ..)| *name == module) {
                continue;
            }
            assert_eq!(
                source.matches("show_viewport").count(),
                0,
                "`{module}` opens a second viewport, which is a second OS window on the same \
                 event loop -- with no `run_*native` call for `RAISING_SITES` to count and no \
                 title for it to compare. If that is deliberate, it belongs in \
                 `OPENS_A_VIEWPORT_AND_RAISES_IT`, which holds the title it opens under. See \
                 the title collision described on this test."
            );
        }

        // **The controls on the two needles, and why they are not `matches`
        // against a string literal.**
        //
        // They used to be exactly that: `"builder.with_any_thread(true);"
        // .matches("with_any_thread").count() == 1`. That proved `str::matches`
        // works on a literal written three lines above it in this same file --
        // it could not fail for any edit to this crate, and it never showed the
        // needle naming anything real. Worse, it read as coverage.
        //
        // What the zeros above actually depend on is that these two needles
        // name the *real* spellings of two APIs, and that the strings they are
        // counted over are the real sources.
        //
        // Neither can be controlled the way the rest of this file controls a
        // needle -- by finding it somewhere it really occurs -- because "this
        // crate contains no such call" is the invariant itself: a source-text
        // control would have to match a call that must not exist. What the
        // previous controls did instead was count each needle in a string
        // literal written three lines above them. That proved `str::matches`
        // works on a literal in the same file. It could not fail for any edit
        // to this crate, and it read as coverage.
        //
        // (They also cannot be turned into scans of `foreground.rs` itself:
        // the literal `with_any_thread` occurs in this file, so adding it to
        // the scanned set would fail the zero above on this test's own text.)
        //
        // So the two halves are controlled separately, and both can fail:
        //
        // 1. **The sources are real** -- controlled in the loop above, by a
        //    needle every window-opening module must contain. An
        //    `include_str!` pointed at the wrong file, or a table row whose
        //    source no longer opens a window, is a zero for the wrong reason.
        // 2. **`show_viewport_deferred` is the real API name** -- controlled
        //    by the compiler in `_the_show_viewport_needle_names_a_real_api`
        //    below, which calls it. A rename upstream is then a build failure
        //    here, not a needle that counts to zero forever.
        //
        // `with_any_thread` gets no compiler control, and that is a gap worth
        // naming rather than papering over: it is a `winit` trait method, and
        // `winit` is not a direct dependency of this crate (`eframe` re-exports
        // `EventLoopBuilder` and `UserEvent`, but not
        // `platform::windows::EventLoopBuilderExtWindows`, so the method cannot
        // be named here). Taking a dependency on `winit` to hold a test needle
        // was rejected as the larger risk -- it pins a second version of a
        // crate whose renderer version conflicts are already documented on this
        // crate's `eframe` dependency. What holds it instead is that the same
        // method name appears in `NativeOptions::event_loop_builder`'s own
        // documentation, which is what anyone adding the call would read.
        let _control = _the_show_viewport_needle_names_a_real_api;
    }

    /// **The compiler-checked half of the `show_viewport` needle in
    /// [`only_one_window_of_this_process_can_exist_at_a_time`].**
    ///
    /// Never called -- referenced from that test only so it cannot be dropped
    /// without the test noticing. Its whole content is that it compiles:
    /// `show_viewport_deferred` is named here in call position on the real
    /// `egui` context, so if that API is renamed upstream this file stops
    /// building instead of silently counting a stale needle to zero in every
    /// window module forever.
    #[allow(dead_code)]
    fn _the_show_viewport_needle_names_a_real_api() {
        let ctx = eframe::egui::Context::default();
        ctx.show_viewport_deferred(
            eframe::egui::ViewportId::ROOT,
            eframe::egui::ViewportBuilder::default(),
            |_, _| {},
        );
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

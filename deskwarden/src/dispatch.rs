//! Pure decision logic for the main event loop.
//!
//! These functions exist as named, side-effect-free helpers (rather than
//! inline `if`s in `main`) precisely so they can be unit-tested: the bugs they
//! guard against -- the overlay re-triggering itself forever, or a fallback
//! typing a password into an unverified window -- are impossible to reproduce
//! in an automated test if the decision only exists inside `main()`.

use crate::window_watch::ForegroundEvent;

/// True when `pid` belongs to this process.
///
/// Our own UI windows (the prompt overlay, the process picker, the login
/// window) are focused, always-on-top windows: showing one fires
/// `EVENT_SYSTEM_FOREGROUND` for *our* process. Those events must never be
/// treated as "a matched app came to the foreground".
pub fn is_own_process(pid: u32) -> bool {
    pid == std::process::id()
}

/// Decides whether a foreground event should trigger a match/dispatch.
///
/// Two independent suppressions, both needed to stop the `prompt` trigger from
/// re-triggering itself in a loop:
///
/// 1. Events from our own process are never dispatched (see
///    [`is_own_process`]). Showing the overlay steals foreground, which is an
///    event for *us*.
/// 2. An hwnd that was *just* dispatched is not dispatched again. When the
///    overlay closes, Windows restores foreground to the target app, which
///    fires a second `EVENT_SYSTEM_FOREGROUND` -- for the target this time.
///    Without this, that refocus re-matches the same window and shows the
///    overlay again, so "Dismiss" never actually dismisses.
///
/// A *different* window becoming foreground always dispatches, and re-focusing
/// the original window after visiting another one dispatches again (because
/// `last_dispatched_hwnd` has moved on by then).
pub fn should_dispatch(event: &ForegroundEvent, last_dispatched_hwnd: Option<isize>) -> bool {
    if is_own_process(event.pid) {
        return false;
    }
    last_dispatched_hwnd != Some(event.hwnd)
}

/// Decides whether the `SendInput` fallback is allowed to type.
///
/// `SendInput` types into whatever has keyboard focus, with no reference to
/// the window we intended to fill. Typing a password into the wrong window is
/// the worst failure this app can have, so the fallback only proceeds when the
/// observed foreground window *is* the target. `foreground_hwnd == 0` means
/// `GetForegroundWindow` returned nothing (e.g. a lock screen or a foreground
/// transition in progress) and is never a match.
pub fn may_type_into(target_hwnd: isize, foreground_hwnd: isize) -> bool {
    foreground_hwnd != 0 && target_hwnd != 0 && target_hwnd == foreground_hwnd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(pid: u32, hwnd: isize) -> ForegroundEvent {
        ForegroundEvent {
            hwnd,
            pid,
            exe_name: "target.exe".to_string(),
            title: "Target".to_string(),
        }
    }

    /// A pid that is definitely not ours. pid 0 is the System Idle Process on
    /// Windows and is never a real foreground-window owner.
    fn other_pid() -> u32 {
        std::process::id().wrapping_add(1)
    }

    #[test]
    fn is_own_process_recognises_this_process() {
        assert!(is_own_process(std::process::id()));
        assert!(!is_own_process(other_pid()));
    }

    #[test]
    fn dispatches_a_freshly_foregrounded_window() {
        assert!(should_dispatch(&event(other_pid(), 42), None));
    }

    #[test]
    fn suppresses_the_same_hwnd_twice_in_a_row() {
        let e = event(other_pid(), 42);
        assert!(should_dispatch(&e, None));
        // ...having dispatched 42, the target regaining focus after our
        // overlay closes must not dispatch again.
        assert!(!should_dispatch(&e, Some(42)));
    }

    #[test]
    fn dispatches_a_different_hwnd() {
        assert!(should_dispatch(&event(other_pid(), 99), Some(42)));
    }

    #[test]
    fn returning_to_a_window_after_visiting_another_dispatches_again() {
        // 42 dispatched, then 99 dispatched (last becomes 99), then back to 42.
        assert!(should_dispatch(&event(other_pid(), 42), Some(99)));
    }

    #[test]
    fn never_dispatches_our_own_process_regardless_of_hwnd() {
        let own = std::process::id();
        assert!(!should_dispatch(&event(own, 42), None));
        assert!(!should_dispatch(&event(own, 42), Some(42)));
        assert!(!should_dispatch(&event(own, 7), Some(42)));
    }

    #[test]
    fn may_type_only_when_foreground_is_the_target() {
        assert!(may_type_into(42, 42));
        assert!(!may_type_into(42, 99));
    }

    #[test]
    fn may_not_type_when_there_is_no_foreground_window() {
        assert!(!may_type_into(42, 0));
        assert!(!may_type_into(0, 0));
        assert!(!may_type_into(0, 42));
    }
}

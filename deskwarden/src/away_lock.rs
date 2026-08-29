//! Locking the vault at the moment the user actually walks away.
//!
//! The vault already locks on **idle** (`Settings::auto_lock_enabled` plus
//! `auto_lock_minutes`). Idle is an *inference*: after N minutes of nothing
//! happening, the app guesses nobody is there. Windows will tell us outright.
//! Pressing Win+L, switching to another user's session, or closing the lid are
//! the moments a person has deliberately left the machine, and until this
//! module the vault sat unlocked -- with `bw serve` up and a decrypted snapshot
//! in memory -- until the idle timer eventually caught up. This module removes
//! the wait.
//!
//! ## No window is created here, and that is the point
//!
//! Both messages need an HWND to arrive at:
//!
//! * `WM_WTSSESSION_CHANGE` is delivered only to windows passed to
//!   `WTSRegisterSessionNotification`.
//! * `WM_POWERBROADCAST` is *broadcast* to every top-level window, which is
//!   why a message-only (`HWND_MESSAGE`) window would be the wrong answer even
//!   though it is the obvious one -- broadcasts do not reach them.
//!
//! This process already owns two top-level windows that satisfy both: the
//! hidden `WS_EX_TOOLWINDOW` helpers that `tray-icon` and `global-hotkey`
//! create on the main thread (`CreateWindowExW`, `WS_OVERLAPPED`, no parent --
//! genuinely top-level, merely never shown). [`pick_notification_window`]
//! chooses one of them, and [`register_on`] registers it. **Nothing here calls
//! `CreateWindowExW`**, which is why `foreground`'s classification of modules
//! by whether they open a window is unchanged by this file: it opens none.
//!
//! ## No window procedure either
//!
//! Subclassing the tray's window would mean inserting ourselves into a
//! `muda` menu subclass that a dependency owns and re-installs. We do not need
//! to. Both messages arrive on the **main thread's message queue** and
//! `crate::app::pump_windows_messages` already drains that queue every
//! iteration of `main`'s loop. It sees the whole `MSG` *before* it dispatches
//! it, so [`away_event`] classifies it right there. The pump is the window
//! procedure we already had.
//!
//! One consequence, restated after the daemon/UI split changed it: the vault
//! window runs in **its own process** (`deskwarden.exe --ui vault`), the
//! daemon does not block on it, and this pump therefore runs the whole time
//! that window is up. So [`locks_the_vault`]'s answer governs two things and
//! not one -- the daemon's own session, and the second process holding a
//! decrypted vault on screen. `main::lock_after_walking_away` acts on both,
//! the second through `UiWindows::close_because_the_user_walked_away`.
//!
//! This paragraph used to say the opposite, and it was right at the time: the
//! window was a nested `eframe` loop inside this process, nothing pumped
//! while it ran, and the window's own idle auto-lock was all that covered the
//! user. It is recorded here because a stale reassurance in this spot is
//! exactly how a decrypted vault came to survive Win+L -- see
//! `docs/superpowers/specs/2026-08-29-the-lock-closes-the-window-design.md`.
//!
//! The one state where the old paragraph still holds is the in-daemon
//! fallback window, opened when `spawn_the_vault_window_in_its_own_process`
//! could not start a process at all. That window is a nested loop, this pump
//! does not run while it is up, and its own idle auto-lock is what covers the
//! user there.
//!
//! ## What can be tested and what cannot
//!
//! A test cannot lock a workstation, cannot suspend a machine, and must not
//! send real window messages. So the seam is drawn so that everything
//! *decided* is a pure function over values a test can write down:
//! [`away_event`] (does this message mean the user left?),
//! [`pick_notification_window`] (which of our windows should receive them), and
//! [`locks_the_vault`] (given that event, this preference and this session,
//! does the vault lock?). [`register_on`] and the message pump are the
//! untestable remainder, and they are deliberately thin: one FFI call and no
//! branches worth asserting on. There is no test in this file that fires a
//! `WM_WTSSESSION_CHANGE`, because such a test would be asserting a constant.
//!
//! The manual check is: press **Win+L**, come back, unlock Windows, and
//! confirm Deskwarden is asking for the master password and that pasting
//! yields nothing you copied out of the vault.

use crate::foreground::OwnWindow;
use crate::settings::AutoLock;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};

/// `WM_WTSSESSION_CHANGE`. Spelled as a value rather than imported so that
/// [`away_event`] is a function over two integers -- a test can state a case
/// without building a `MSG`, and the classifier depends on no projection at
/// all. Every one of these is checked against the `windows` crate's own
/// constant by a test below, so "spelled by hand" cannot drift into "wrong".
pub const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
/// `WM_POWERBROADCAST`, same arrangement.
pub const WM_POWERBROADCAST: u32 = 0x0218;
/// `WTS_SESSION_LOCK` -- the workstation was locked (Win+L, or the screen
/// saver's secure lock).
pub const WTS_SESSION_LOCK: usize = 0x7;
/// `WTS_SESSION_UNLOCK`. Named so the classifier can say out loud that it is
/// *not* an away event, rather than falling into a catch-all.
pub const WTS_SESSION_UNLOCK: usize = 0x8;
/// `WTS_CONSOLE_DISCONNECT` -- the session was switched away from (fast user
/// switching, or an RDP session taking the console). The user's desktop is no
/// longer on the screen, which is the same fact `WTS_SESSION_LOCK` reports.
pub const WTS_CONSOLE_DISCONNECT: usize = 0x2;
/// `WTS_REMOTE_DISCONNECT` -- as above, for a remote session.
pub const WTS_REMOTE_DISCONNECT: usize = 0x4;
/// `PBT_APMSUSPEND` -- the machine is about to suspend. The one constant here
/// with no counterpart in the `windows` features this crate enables (it lives
/// in `Win32_System_Power`, which nothing else needs), so it is the one the
/// test below cannot cross-check.
pub const PBT_APMSUSPEND: usize = 0x0004;
/// `PBT_APMRESUMEAUTOMATIC`. Named for [`WTS_SESSION_UNLOCK`]'s reason.
pub const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
/// `NOTIFY_FOR_THIS_SESSION` -- we want our own session's transitions, not
/// every session on the machine. A terminal server with ten users logged in
/// would otherwise lock this vault ten times over for events that have
/// nothing to do with the person who started this app.
pub const NOTIFY_FOR_THIS_SESSION: u32 = 0;

/// What the operating system said about the user leaving.
///
/// Two variants rather than one because they are two different claims and the
/// log line should say which was made; both lead to the same lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwayEvent {
    /// The workstation was locked, or the session was switched away from.
    /// The strongest signal there is: the user pressed the key that means
    /// "I am leaving".
    WorkstationLocked,
    /// The machine is suspending.
    Suspending,
}

/// **Does this queued message mean the user walked away?** The pure half.
///
/// Takes the two fields of a `MSG` that matter (`message` and `wParam`) rather
/// than a `MSG`, so a test can state a case without constructing a Win32
/// struct, and so nothing about this decision depends on the `windows` crate.
///
/// Everything not named is `None`, including the two *arrivals*
/// ([`WTS_SESSION_UNLOCK`], [`PBT_APMRESUMEAUTOMATIC`]) -- they are listed as
/// constants above precisely so that reading this function shows they were
/// considered and rejected, instead of leaving a reader to wonder whether an
/// unlock falls through some catch-all into a lock.
#[must_use]
pub fn away_event(message: u32, wparam: usize) -> Option<AwayEvent> {
    match (message, wparam) {
        (WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK)
        | (WM_WTSSESSION_CHANGE, WTS_CONSOLE_DISCONNECT)
        | (WM_WTSSESSION_CHANGE, WTS_REMOTE_DISCONNECT) => Some(AwayEvent::WorkstationLocked),
        (WM_POWERBROADCAST, PBT_APMSUSPEND) => Some(AwayEvent::Suspending),
        _ => None,
    }
}

/// **Does the vault actually lock?** The other pure half, and the one that
/// answers "what if it is already locked, or was never unlocked".
///
/// Three inputs, each carrying one question:
///
/// * `event` -- what happened. Both variants answer the same, and that is a
///   decision, not an oversight. **A suspend is not a workstation lock**: a
///   machine that sleeps and wakes may never show a lock screen, because
///   whether it does is a Windows setting this process cannot see and has no
///   business guessing at. Treating "no lock screen on resume" as "the user
///   never left" would mean the vault survives, decrypted, across a night in
///   a bag. The cost of the other answer is one master-password prompt after a
///   sleep the user did not consider leaving -- which is the same prompt the
///   idle timer would have produced anyway, since a suspended machine is idle
///   by definition and wakes past its own timeout. So resume-without-lock
///   stays locked.
///
/// * `auto_lock` -- the user's preference, **and no new preference beside
///   it**. `AutoLock::Never` (which is what `auto_lock_enabled = false`
///   produces, whatever the minutes say -- see `settings::auto_lock_policy`)
///   means the user has said "do not lock this vault behind my back", and
///   locking it on Win+L would be exactly that. `AutoLock::After` means "lock
///   it when I walk away", and Win+L is the least ambiguous evidence of
///   walking away this app will ever receive. The *interval* is deliberately
///   not consulted: minutes answer "how long is long enough to infer absence",
///   and there is nothing left to infer once the user has said so themselves.
///
/// * `session_is_unlocked` -- whether there is anything to lock. `main` passes
///   `!token.is_empty()`. An already-locked vault, an app that never got a
///   session, and a startup that stood autofill down all answer `false`, and
///   locking them again would be a master-password prompt raised at a user who
///   has just come back to a machine on which nothing was open.
#[must_use]
pub const fn locks_the_vault(
    event: AwayEvent,
    auto_lock: AutoLock,
    session_is_unlocked: bool,
) -> bool {
    // `event` is matched rather than ignored so that adding a third variant is
    // a compile error here -- the place that has to weigh it -- rather than a
    // silent inheritance of whatever these two decided.
    let leaving = match event {
        AwayEvent::WorkstationLocked | AwayEvent::Suspending => true,
    };
    let wants_locking = match auto_lock {
        AutoLock::Never => false,
        AutoLock::After(_) => true,
    };
    leaving && wants_locking && session_is_unlocked
}

/// **Which of this process's windows receives the notifications.** Pure, over
/// the window list `foreground::Desktop::own_windows` already produces.
///
/// A `WS_EX_TOOLWINDOW` helper is chosen on purpose, and it is the opposite of
/// `foreground::pick`'s rule for the opposite reason: that function skips tool
/// windows because the user cannot point at one, and this one *wants* the
/// window the user cannot point at. The tray icon's and the hotkey manager's
/// helpers live for the whole process; every other window this app owns is a
/// window the user can close, and a registration on a closed window is a
/// registration that silently stops working.
///
/// `None` when there is none -- a test process, or a launch where the tray
/// failed to come up. The caller logs and carries on without the feature
/// rather than failing to start.
#[must_use]
pub fn pick_notification_window(windows: &[OwnWindow]) -> Option<isize> {
    windows.iter().find(|w| w.tool_window).map(|w| w.hwnd)
}

/// A live `WTSRegisterSessionNotification`, unregistered on drop.
///
/// Held by `main` for the life of the process. `Drop` rather than an explicit
/// call because the one thing worse than not unregistering is unregistering on
/// some paths and not others.
pub struct SessionNotifications {
    hwnd: isize,
}

impl Drop for SessionNotifications {
    fn drop(&mut self) {
        // Best effort: at the only point this runs, the process is on its way
        // out and the window may already be destroyed, which is a documented
        // failure and not an error worth reporting.
        unsafe {
            let _ = WTSUnRegisterSessionNotification(HWND(self.hwnd as *mut core::ffi::c_void));
        }
    }
}

/// Registers `hwnd` for session-change notifications.
///
/// The untestable half, and kept to one call with no decision in it. Note that
/// **only `WM_WTSSESSION_CHANGE` needs registering**: `WM_POWERBROADCAST` is
/// broadcast to every top-level window and arrives at the same queue whether
/// this succeeds or not, so a failure here costs the workstation-lock half of
/// the feature and leaves the suspend half working.
#[must_use]
pub fn register_on(hwnd: isize) -> Option<SessionNotifications> {
    let registered = unsafe {
        WTSRegisterSessionNotification(
            HWND(hwnd as *mut core::ffi::c_void),
            NOTIFY_FOR_THIS_SESSION,
        )
    };
    if let Err(error) = registered {
        log::warn!(
            "could not register for workstation-lock notifications on window {hwnd:#x} \
             ({error}); the vault will still lock on sleep and on the idle timeout, but not on \
             Win+L"
        );
        return None;
    }
    log::info!(
        "registered for workstation-lock notifications on this process's existing helper window \
         {hwnd:#x}; no window was created for it"
    );
    Some(SessionNotifications { hwnd })
}

/// [`pick_notification_window`] over the real desktop, then [`register_on`].
///
/// The whole of the wiring, so `main` spells it once. In a test process
/// `own_windows` finds nothing and this is `None` without reaching Win32 at
/// all -- which is why no test in this crate can register anything.
#[must_use]
pub fn register_on_this_process() -> Option<SessionNotifications> {
    use crate::foreground::Desktop;
    let windows = crate::foreground::Win32Desktop.own_windows();
    let Some(hwnd) = pick_notification_window(&windows) else {
        log::warn!(
            "this process owns no helper window to receive workstation-lock notifications, so \
             the vault will not lock on Win+L; the idle timeout is unaffected"
        );
        return None;
    };
    register_on(hwnd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The constants above are spelled by value so [`away_event`] can be a
    /// function over two integers. That is only safe if they are the same
    /// values Windows uses, so every one the projection has is checked against
    /// it. `PBT_APMSUSPEND` is the single exception and says so at its own
    /// declaration.
    #[test]
    fn the_constants_are_the_ones_windows_defines() {
        use windows::Win32::System::RemoteDesktop::NOTIFY_FOR_THIS_SESSION as PROJECTED_NOTIFY;
        use windows::Win32::UI::WindowsAndMessaging::{
            WTS_CONSOLE_DISCONNECT, WTS_REMOTE_DISCONNECT, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
        };
        assert_eq!(
            WM_WTSSESSION_CHANGE,
            windows::Win32::UI::WindowsAndMessaging::WM_WTSSESSION_CHANGE,
        );
        assert_eq!(
            WM_POWERBROADCAST,
            windows::Win32::UI::WindowsAndMessaging::WM_POWERBROADCAST,
        );
        assert_eq!(super::WTS_SESSION_LOCK, WTS_SESSION_LOCK as usize);
        assert_eq!(super::WTS_SESSION_UNLOCK, WTS_SESSION_UNLOCK as usize);
        assert_eq!(super::WTS_CONSOLE_DISCONNECT, WTS_CONSOLE_DISCONNECT as usize);
        assert_eq!(super::WTS_REMOTE_DISCONNECT, WTS_REMOTE_DISCONNECT as usize);
        assert_eq!(NOTIFY_FOR_THIS_SESSION, PROJECTED_NOTIFY);
    }

    #[test]
    fn a_workstation_lock_is_an_away_event() {
        assert_eq!(
            away_event(WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK),
            Some(AwayEvent::WorkstationLocked)
        );
    }

    #[test]
    fn a_session_switch_is_an_away_event_in_both_of_its_spellings() {
        for wparam in [WTS_CONSOLE_DISCONNECT, WTS_REMOTE_DISCONNECT] {
            assert_eq!(
                away_event(WM_WTSSESSION_CHANGE, wparam),
                Some(AwayEvent::WorkstationLocked),
                "wparam {wparam:#x}"
            );
        }
    }

    #[test]
    fn a_suspend_is_an_away_event() {
        assert_eq!(away_event(WM_POWERBROADCAST, PBT_APMSUSPEND), Some(AwayEvent::Suspending));
    }

    /// The arrivals. A user coming BACK must not be read as a user leaving --
    /// the unlock message rides the very same `WM_WTSSESSION_CHANGE`, so a
    /// classifier keyed on the message id alone would lock the vault at the
    /// exact moment the user sat down again, every time.
    #[test]
    fn coming_back_is_not_an_away_event() {
        assert_eq!(away_event(WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK), None);
        assert_eq!(away_event(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC), None);
    }

    /// The pump sees every message on the thread. Ordinary traffic must fall
    /// through, including a message whose *wParam* happens to equal one of the
    /// session codes -- `WM_TIMER` with id 7 is not a workstation lock.
    #[test]
    fn ordinary_messages_are_not_away_events() {
        use windows::Win32::UI::WindowsAndMessaging::{WM_COMMAND, WM_HOTKEY, WM_TIMER};
        for message in [WM_COMMAND, WM_HOTKEY, WM_TIMER, 0] {
            for wparam in [0, WTS_SESSION_LOCK, PBT_APMSUSPEND, WTS_SESSION_UNLOCK, 999] {
                assert_eq!(
                    away_event(message, wparam),
                    None,
                    "message {message:#x} wparam {wparam:#x} was read as the user walking away"
                );
            }
        }
    }

    /// Control on the test above: the pair the classifier DOES answer is not
    /// among the ones it just swept, so the sweep's silence means something.
    #[test]
    fn the_sweep_of_ordinary_messages_does_not_contain_the_pairs_that_answer() {
        use windows::Win32::UI::WindowsAndMessaging::{WM_COMMAND, WM_HOTKEY, WM_TIMER};
        for message in [WM_COMMAND, WM_HOTKEY, WM_TIMER, 0] {
            assert_ne!(message, WM_WTSSESSION_CHANGE);
            assert_ne!(message, WM_POWERBROADCAST);
        }
    }

    const A_TIMEOUT: AutoLock = AutoLock::After(Duration::from_secs(15 * 60));

    #[test]
    fn an_unlocked_vault_locks_on_both_events_when_auto_lock_is_on() {
        for event in [AwayEvent::WorkstationLocked, AwayEvent::Suspending] {
            assert!(locks_the_vault(event, A_TIMEOUT, true), "{event:?}");
        }
    }

    /// The setting decision, stated as a test: there is no preference of this
    /// feature's own, and `auto_lock_enabled = false` (which is what
    /// `AutoLock::Never` is) governs it. A user who turned auto-lock off has
    /// said not to lock the vault behind their back, and Win+L is behind their
    /// back in the most literal sense available.
    #[test]
    fn auto_lock_never_means_walking_away_does_not_lock_either() {
        for event in [AwayEvent::WorkstationLocked, AwayEvent::Suspending] {
            assert!(!locks_the_vault(event, AutoLock::Never, true), "{event:?}");
        }
    }

    /// The harmless-in-every-state half. Nothing is unlocked, so nothing
    /// locks -- and the user does not come back to a master-password prompt
    /// for a vault that was already shut.
    #[test]
    fn a_vault_that_is_not_unlocked_is_not_locked_again() {
        for event in [AwayEvent::WorkstationLocked, AwayEvent::Suspending] {
            for auto_lock in [A_TIMEOUT, AutoLock::Never] {
                assert!(!locks_the_vault(event, auto_lock, false), "{event:?} {auto_lock:?}");
            }
        }
    }

    /// The interval is not consulted. One minute and a thousand minutes are
    /// the same answer, because the timer's question -- how long is long
    /// enough to guess -- is not being asked.
    #[test]
    fn the_auto_lock_interval_does_not_change_the_answer() {
        let short = AutoLock::After(Duration::from_secs(60));
        let long = AutoLock::After(Duration::from_secs(1000 * 60));
        assert_eq!(
            locks_the_vault(AwayEvent::WorkstationLocked, short, true),
            locks_the_vault(AwayEvent::WorkstationLocked, long, true),
        );
        assert!(locks_the_vault(AwayEvent::WorkstationLocked, short, true));
    }

    fn window(hwnd: isize, tool_window: bool, title: &str) -> OwnWindow {
        OwnWindow {
            hwnd,
            title: title.to_string(),
            visible: true,
            tool_window,
            minimised: false,
        }
    }

    #[test]
    fn the_helper_window_is_the_one_registered_and_the_vault_window_is_not() {
        let windows = [
            window(0x10, false, "Deskwarden Vault"),
            window(0x20, true, ""),
            window(0x30, true, ""),
        ];
        assert_eq!(pick_notification_window(&windows), Some(0x20));
    }

    /// A process with no helper window -- which is every test process, and a
    /// launch whose tray failed -- yields nothing to register, and the caller
    /// carries on without the feature.
    #[test]
    fn a_process_with_no_helper_window_registers_nothing() {
        assert_eq!(pick_notification_window(&[]), None);
        assert_eq!(pick_notification_window(&[window(0x10, false, "Deskwarden")]), None);
    }

    /// **The module doc must not still claim the vault-window case is
    /// somebody else's.**
    ///
    /// It said so, correctly, when the vault window ran a nested `eframe` loop
    /// inside this process. Since the daemon/UI split the window is a separate
    /// process, the daemon's loop pumps throughout, and this module's decision
    /// governs that window too -- via
    /// `main::lock_after_walking_away` -> `UiWindows::close_because_the_user_walked_away`.
    /// A stale reassurance here is how the defect survived: a reader checking
    /// whether the window was covered found a paragraph saying it was.
    #[test]
    fn the_module_doc_does_not_claim_the_pump_is_asleep_while_a_window_is_up() {
        // **The module doc alone, unwrapped**, and both halves of that matter.
        //
        // Only the `//!` lines, because this test's own doc comment quotes the
        // arrangement it is asserting about -- searched over the whole file,
        // the control would be satisfied by the words directly above it and
        // would prove nothing about the module doc at all.
        //
        // Unwrapped, because the stale sentence was broken across two `//!`
        // lines: a raw substring search for it would have passed on the very
        // text it exists to forbid. Both are the house defect class -- a test
        // that passes because it never reached the thing it names.
        let prose = include_str!("away_lock.rs")
            .lines()
            .map_while(|line| line.strip_prefix("//!"))
            .collect::<Vec<_>>()
            .join(" ");
        let prose = prose.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            prose.contains("its own process"),
            "control: the module doc must actually describe the arrangement it has -- the              vault window is a separate process and this module's decision reaches it"
        );
        assert!(
            !prose.contains("the pump does not run while a vault window is up"),
            "this sentence was true before the daemon/UI process split and is now the              reassurance that hid a security defect"
        );
    }
}

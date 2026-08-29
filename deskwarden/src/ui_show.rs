//! **Waking a hidden vault window.**
//!
//! When [`crate::settings::Settings::keep_ui_loaded`] is on, a plain close
//! hides the vault window's viewport and its process stays alive. The daemon
//! then needs a way to say *show yourself* to a process it shares no memory
//! with -- and [`crate::foreground::raise_process`] cannot do it, because a
//! hidden viewport has no window to raise.
//!
//! A named event is that way, and it is this crate's existing idiom:
//! [`crate::single_instance`] already asks a running Deskwarden to stand down
//! through one, and `vault_service`'s attachment slots are named mutexes
//! under the same `Local\` scope.
//!
//! # Auto-reset, unlike `single_instance`'s
//!
//! That module's quit event is deliberately manual-reset, because its signal
//! means *stand down* -- a one-way fact about the process, which a second
//! asker must not find clear again.
//!
//! This one means *show yourself now*, which is a token to be consumed: one
//! ask, one show. A manual-reset event here would stay signalled after the
//! first show, so the window would be re-shown every time the child looked --
//! and the reset would have to happen on a path in the child that must never
//! be forgotten. Auto-reset makes the kernel do it.

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};

/// The event a UI process waits on to be shown.
///
/// **Named by pid**, because that is the daemon's whole record that a window
/// exists: it is what the spawn returned, what the result file is named by,
/// and what the show is aimed at. There is no second registry that could
/// disagree with it.
///
/// **Under `Local\`**, so the name is scoped to the logon session -- the same
/// scope `vault_service`'s attachment slots use. A global name would let one
/// user's daemon signal another user's window.
#[must_use]
pub fn signal_name(pid: u32) -> String {
    format!(r"Local\Deskwarden-UI-Show-{pid}")
}

/// A live handle to the event, closed when this is dropped.
pub struct Signal(HANDLE);

impl Signal {
    /// Waits up to `timeout_ms` milliseconds to be shown.
    ///
    /// `true` means somebody asked. `false` means the timeout passed or the
    /// wait failed, and a caller must read it as "carry on waiting" or "give
    /// up and close" -- never as "show the window", which would put a window
    /// on screen that nobody asked for.
    #[must_use]
    pub fn wait(&self, timeout_ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.0, timeout_ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for Signal {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// The kernel calls this module makes, behind `fn` pointers so a caller can
/// be tested without them.
///
/// `cfg(test)` seams are banned crate-wide; this is the `ServiceEnv` idiom
/// that ban points at.
pub struct ShowEnv {
    /// Creates the named event and keeps it alive until dropped. `None` when
    /// it could not be created at all.
    pub create: fn(&str) -> Option<Signal>,
    /// Sets the named event. `false` when no process is listening on that
    /// name.
    pub set: fn(&str) -> bool,
}

impl ShowEnv {
    #[must_use]
    pub fn production() -> Self {
        Self { create: create_show_event, set: set_show_event }
    }
}

/// Auto-reset (`bManualReset` false), initially unsignalled. See this
/// module's doc for why auto-reset rather than manual.
fn create_show_event(name: &str) -> Option<Signal> {
    unsafe { CreateEventW(None, false, false, &HSTRING::from(name)) }.ok().map(Signal)
}

fn set_show_event(name: &str) -> bool {
    unsafe {
        // **`EVENT_MODIFY_STATE` is the right to set, and it is all this
        // needs** -- the same right `single_instance::signal_quit` opens
        // with. Named constant rather than a hand-written mask: the
        // truncated-`SYNCHRONIZE` bug `vault_service` shipped could only
        // happen because a standard right was spelled out by hand, and the
        // test below forbids that shape here.
        let Ok(handle) = OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(name)) else {
            return false;
        };
        let set = SetEvent(handle).is_ok();
        let _ = CloseHandle(handle);
        set
    }
}

/// **Ask the UI process `pid` to show itself.**
///
/// `false` means nothing is listening on that name: the process died, or it
/// never created the event. The caller's answer to `false` is to spawn a
/// fresh window, never to give up -- under the one-window rule a refusal that
/// opens nothing is an *Open Vault* that never opens again.
#[must_use]
pub fn ask_to_show(env: &ShowEnv, pid: u32) -> bool {
    (env.set)(&signal_name(pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Per-process, and inside the logon session.** A name without the pid
    /// would let a stale event from a dead process wake the wrong window, and
    /// one without `Local\` would cross between users.
    #[test]
    fn the_name_is_scoped_to_the_logon_session_and_the_process() {
        let name = signal_name(1234);
        assert!(name.starts_with(r"Local\"), "not logon-session scoped: {name}");
        assert!(name.contains("1234"), "not per-process: {name}");
        assert_ne!(signal_name(1234), signal_name(1235));
    }

    /// **No access mask is written by hand here, and this is the pin.**
    ///
    /// `vault_service` shipped `0x0010` for `SYNCHRONIZE`, which is
    /// `0x0010_0000`. Every `OpenMutexW` returned ACCESS_DENIED, the module
    /// reported "nobody attached" while two processes held slots, and all 23
    /// of its tests passed -- because the fake kernel never reached the call.
    /// This module uses named constants only.
    #[test]
    fn no_access_right_is_written_as_a_literal() {
        let source = include_str!("ui_show.rs");
        let production = source.split("mod tests").next().expect("a production half");
        assert!(
            !production.contains("0x00"),
            "a raw access-right literal is in `ui_show`; use the `windows` crate's constants, \
             as this crate now does after shipping exactly that bug in `vault_service`"
        );
        assert!(
            production.contains("EVENT_MODIFY_STATE"),
            "control: this module no longer names an access right at all, so the assertion \
             above is guarding nothing"
        );
    }

    /// **The whole point, over the real kernel.** A fake would prove only
    /// that the fake works, which is the defect class this crate keeps
    /// finding -- `vault_service`'s wrong access right passed 23 tests.
    #[test]
    fn a_signal_set_from_outside_wakes_the_waiter() {
        // A pid this process does not have, so a parallel test that really
        // is this process cannot collide with it.
        let pid = std::process::id() ^ 0x5555;
        let env = ShowEnv::production();
        let signal = (env.create)(&signal_name(pid)).expect("the event should be creatable");

        assert!(!signal.wait(0), "the event was signalled before anybody set it");
        assert!(ask_to_show(&env, pid), "setting the event failed");
        assert!(signal.wait(1_000), "the waiter never saw the signal");
        assert!(
            !signal.wait(0),
            "the event did not auto-reset, so one ask would show the window for ever"
        );
    }

    /// Asking after a window is gone is a clean `false`, not a panic and not
    /// a hang. The daemon reads it as "spawn a fresh one".
    #[test]
    fn asking_a_process_that_has_no_signal_fails_cleanly() {
        assert!(!ask_to_show(&ShowEnv::production(), 0xFFFF_FFF0));
    }
}

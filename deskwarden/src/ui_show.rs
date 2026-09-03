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

/// **The name a UI process holds while its window is ON SCREEN**, and drops
/// while hidden.
///
/// This is how the daemon tells "a window exists" from "a window is showing",
/// and it has to be asked rather than remembered. The daemon's first attempt
/// stored the answer in a `hidden` flag on its own record of the child --
/// which nothing ever set, because the child hides itself and the two
/// processes share no memory. The flag read `false` for ever: `bw serve`
/// stayed up behind a hidden window, and *Open Vault* raised a window that
/// was not there.
///
/// A held name cannot go stale that way. The child holds it while visible and
/// drops it on hide, and **Windows releases it if the process dies**, so a
/// crashed window is indistinguishable from a closed one -- which is exactly
/// right, because for this question it is one.
///
/// Deliberately a mutex rather than a second event: this is a STATE somebody
/// asks about, not a message. [`crate::vault_service`]'s attachment slots are
/// the same shape for the same reason, and this reuses their `hold`/`is_held`
/// rather than opening kernel handles a second way -- that module's access
/// right was wrong once already.
#[must_use]
pub fn visible_name(pid: u32) -> String {
    format!(r"Local\Deskwarden-UI-Visible-{pid}")
}

/// **The name a UI process presses to say "I have edited settings for
/// you".**
///
/// The mirror of [`signal_name`]: that one is the daemon asking the child
/// to show itself, this one is the child asking the daemon to read a file.
/// Same `Local\` scope and same per-pid keying for the same two reasons --
/// a global name would cross between users, and a name without the pid
/// would let a dead process's doorbell be answered on behalf of a live one.
///
/// **Created by the DAEMON**, right after the spawn that produces the pid,
/// and polled with `Signal::wait(0)` once per pass of `main`'s loop. A
/// blocking wait is impossible there: that loop is what drains the hotkey
/// and answers the tray.
///
/// Auto-reset, like [`signal_name`]'s and unlike `single_instance`'s: this
/// means *there is something to read now*, which is a token to be consumed.
#[must_use]
pub fn settings_name(pid: u32) -> String {
    format!(r"Local\Deskwarden-UI-Settings-{pid}")
}

/// **The name a UI process presses to say "I have signed in; start
/// `bw serve` with the token I just stored".**
///
/// A sibling of [`settings_name`] and not a reuse of it. Both are the child
/// ringing the daemon, but they mean different things at different moments,
/// and a shared name would have the daemon reading a settings file when a
/// sign-in rang -- or, far worse, consuming the sign-in's ring while looking
/// for a preferences edit, leaving the window waiting for a backend nobody
/// was ever going to start.
///
/// **Created by the DAEMON**, at the spawn that produces the pid, for
/// [`settings_name`]'s reason: "the daemon is listening" is what the child's
/// `SetEvent` answer MEANS, and a doorbell created lazily would have the
/// child concluding no daemon exists.
///
/// # What crosses, and what does not
///
/// The ring says only *there is something to read*. What the daemon then
/// reads is `ui-signin-<pid>.json`, a [`crate::login_ui::SignedInIdentity`]:
/// an account id, an email, a server URL and a backend answer -- the three
/// things this app writes into `settings.json` in the clear anyway.
///
/// **The token does not travel this way and neither does the password.** The
/// child writes the session token into that account's own DPAPI
/// `session.bin` before it rings, and the daemon loads it back out of the
/// same store. So the secret goes to the place that already holds it, under
/// the same user's credentials, and the file that crosses carries no
/// credential at all.
///
/// Auto-reset, like every other event in this module: this means *there is
/// something to read now*, which is a token to be consumed.
#[must_use]
pub fn signin_name(pid: u32) -> String {
    format!(r"Local\Deskwarden-UI-SignIn-{pid}")
}

/// **The name the daemon presses to say "`bw serve` is up on the token you
/// stored; go and draw the vault".**
///
/// The only DAEMON-TO-CHILD event in this module other than
/// [`signal_name`], and the answer to [`signin_name`]'s ring.
///
/// # Why an event and not a port probe
///
/// The child's obvious move after ringing is to poll the `bw serve` port
/// until something answers, and that is wrong in a way that is invisible
/// until it bites: **the port can be up with a stale token.** An orphaned
/// `bw serve` from a previous unclean exit holds
/// `crate::bw_serve::BW_SERVE_PORT` and answers a TCP handshake perfectly
/// well while serving a session that has nothing to do with the password
/// just typed. A child that took that as readiness would open a vault
/// window onto somebody else's vault, or -- more usually -- onto a locked
/// one, and report it as an empty vault with no explanation.
///
/// So readiness is asserted by the one process that KNOWS: the daemon,
/// which owns the `bw serve` child, started it itself, and started it with
/// this token. Nothing else in the system can honestly answer the question.
///
/// **Created by the CHILD, before it rings**, which is the reverse of
/// [`signin_name`] and is load-bearing. The daemon's `SetEvent` lands the
/// instant the backend is up; if the child created this name after ringing,
/// that set could arrive with nobody listening and the child would then wait
/// out its whole deadline for a signal that had already been sent. Creating
/// it first makes the ordering impossible to get wrong.
#[must_use]
pub fn backend_ready_name(pid: u32) -> String {
    format!(r"Local\Deskwarden-UI-Backend-Ready-{pid}")
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

// **A kernel handle belongs to the process, not to a thread**, and waiting on
// an event from a thread other than the one that created it is what
// `WaitForSingleObject` is for -- `single_instance`'s takeover listener does
// exactly that with its own event.
//
// It has to cross a thread here because the wait must NOT run on the frame
// thread: a hidden window whose frame thread is blocked could never paint the
// show it is waiting for. The handle is owned by this `Signal`, closed once in
// `Drop`, and never duplicated, so there is no second owner to race with.
unsafe impl Send for Signal {}
unsafe impl Sync for Signal {}

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

    /// The visibility name is scoped and per-process for the same reasons the
    /// show signal is, and is a DIFFERENT name -- one is a message, the other
    /// a state, and a process holding the one it should be waiting on would
    /// deadlock itself.
    #[test]
    fn the_visible_name_is_its_own_name() {
        let name = visible_name(1234);
        assert!(name.starts_with(r"Local\"), "not logon-session scoped: {name}");
        assert!(name.contains("1234"), "not per-process: {name}");
        assert_ne!(name, signal_name(1234), "the state and the message share a name");
        assert_ne!(visible_name(1234), visible_name(1235));
    }

    /// A THIRD name, and the third one is the daemon's ear rather than the
    /// child's. Sharing a name with either of the other two would have the
    /// daemon consuming the child's show signal, or the child's visibility
    /// mutex answering a question about settings.
    #[test]
    fn the_settings_doorbell_is_its_own_name() {
        let name = settings_name(1234);
        assert!(name.starts_with(r"Local\"), "not logon-session scoped: {name}");
        assert!(name.contains("1234"), "not per-process: {name}");
        assert_ne!(name, signal_name(1234), "the doorbell shares a name with the show signal");
        assert_ne!(
            name,
            visible_name(1234),
            "the doorbell shares a name with the visibility mutex"
        );
        assert_ne!(settings_name(1234), settings_name(1235));
    }

    /// **Over the real kernel, in the direction production uses it**: the
    /// daemon creates and polls with a zero timeout, the child sets, and one
    /// set is consumed by one read.
    ///
    /// The zero-timeout poll is the whole point -- `main`'s loop is what
    /// answers the hotkey and can never block -- so it is what is asserted,
    /// not a convenient long wait.
    #[test]
    fn the_daemon_can_poll_the_doorbell_without_blocking() {
        // A pid this process does not have, so a parallel test that really is
        // this process cannot collide with it.
        let pid = std::process::id() ^ 0x3333;
        let env = ShowEnv::production();
        let ear = (env.create)(&settings_name(pid)).expect("the event should be creatable");

        assert!(!ear.wait(0), "the doorbell rang before anybody pressed it");
        assert!((env.set)(&settings_name(pid)), "pressing the doorbell failed");
        assert!(ear.wait(0), "a rung doorbell did not read as rung on a zero-timeout poll");
        assert!(
            !ear.wait(0),
            "the doorbell did not auto-reset; one edit would be applied forever"
        );
    }

    /// Nobody listening is a clean `false`. The child reads that as "keep the
    /// old transport": do not hide, exit and carry the settings home in the
    /// result file, exactly as before this feature.
    #[test]
    fn pressing_a_doorbell_nobody_holds_fails_cleanly() {
        assert!(!(ShowEnv::production().set)(&settings_name(0xFFFF_FFE0)));
    }

    /// **Five names, all different.** The sign-in doorbell and the
    /// readiness answer join the three that were here, and every pair of the
    /// five has to differ: two of these are the two ends of one handshake,
    /// and a handshake whose ends share a name is a process signalling
    /// itself.
    ///
    /// The pair that would hurt most is `signin`/`settings`, because both
    /// are the child ringing the daemon and the daemon polls both on the
    /// same pass -- a shared name would have a sign-in consumed by the
    /// settings poll, and the window would then wait out its deadline for a
    /// backend nobody was going to start.
    #[test]
    fn the_sign_in_doorbell_and_its_answer_are_their_own_names() {
        for name in [signin_name(1234), backend_ready_name(1234)] {
            assert!(name.starts_with(r"Local\"), "not logon-session scoped: {name}");
            assert!(name.contains("1234"), "not per-process: {name}");
        }
        assert_ne!(signin_name(1234), signin_name(1235));
        assert_ne!(backend_ready_name(1234), backend_ready_name(1235));

        // Every pair of the five, by construction rather than by a list a
        // sixth name could be left off of.
        let all = [
            ("show", signal_name(1234)),
            ("visible", visible_name(1234)),
            ("settings", settings_name(1234)),
            ("signin", signin_name(1234)),
            ("backend-ready", backend_ready_name(1234)),
        ];
        for (i, (a_name, a)) in all.iter().enumerate() {
            for (b_name, b) in all.iter().skip(i + 1) {
                assert_ne!(a, b, "`{a_name}` and `{b_name}` share the name {a}");
            }
        }
    }

    /// **The whole handshake, over the real kernel, in the direction
    /// production runs it.**
    ///
    /// A fake would prove only that the fake works, which is this crate's
    /// standing defect class. Both halves are here because the ORDER is the
    /// design: the child creates its readiness ear BEFORE it rings, so the
    /// daemon's answer cannot land on nobody.
    ///
    /// The daemon's poll is zero-timeout, because `main`'s loop is what
    /// answers the tray and can never block -- so that is what is asserted
    /// rather than a convenient long wait.
    #[test]
    fn the_sign_in_doorbell_is_answered_by_the_readiness_event() {
        // A pid this process does not have, so a parallel test that really
        // is this process cannot collide with it.
        let pid = std::process::id() ^ 0x2222;
        let env = ShowEnv::production();

        // THE DAEMON, at the spawn: it creates the ear it will poll.
        let doorbell = (env.create)(&signin_name(pid)).expect("the doorbell should be creatable");
        // THE CHILD, before it rings: its own ear for the answer.
        let ready = (env.create)(&backend_ready_name(pid)).expect("the ear should be creatable");

        assert!(!doorbell.wait(0), "the doorbell rang before anybody pressed it");
        assert!(!ready.wait(0), "the backend read as ready before anybody said so");

        // THE CHILD rings, having already stored its token.
        assert!((env.set)(&signin_name(pid)), "pressing the sign-in doorbell failed");
        assert!(
            doorbell.wait(0),
            "a rung sign-in doorbell did not read as rung on a zero-timeout poll, so the \
             daemon's loop would never learn a child had signed in"
        );
        assert!(
            !doorbell.wait(0),
            "the sign-in doorbell did not auto-reset; one sign-in would restart `bw serve` \
             on every pass of the daemon's loop"
        );

        // THE DAEMON answers, having started `bw serve` with that token.
        assert!((env.set)(&backend_ready_name(pid)), "answering the sign-in failed");
        assert!(
            ready.wait(1_000),
            "the child never saw the readiness answer, so it would sit on the spinner until \
             its deadline and then report a vault it could have drawn"
        );
        assert!(!ready.wait(0), "the readiness event did not auto-reset");
    }

    /// Nobody listening is a clean `false` at both ends, which is what makes
    /// the mid-sign-in failures survivable rather than hangs.
    ///
    /// A daemon that died between the spawn and the ring: the child's press
    /// answers `false` and it stops waiting. A child that died between the
    /// ring and the vault: the daemon's answer answers `false`, and the
    /// backend it just started is still correct and still serving.
    #[test]
    fn ringing_and_answering_a_process_that_is_gone_fails_cleanly() {
        let env = ShowEnv::production();
        assert!(!(env.set)(&signin_name(0xFFFF_FFD0)));
        assert!(!(env.set)(&backend_ready_name(0xFFFF_FFD0)));
    }

    /// **Held means visible, and the holding really is observable from
    /// outside** -- which is the whole point, since the process that asks is
    /// not the process that holds.
    ///
    /// Over the real kernel, through `vault_service`'s own `hold`/`is_held`,
    /// because those are what production uses and a fake would only prove the
    /// fake works.
    #[test]
    fn a_held_visibility_name_is_visible_to_another_asker() {
        let env = crate::vault_service::windows_env();
        let name = visible_name(std::process::id() ^ 0x7777);

        assert!(!(env.is_held)(&name), "the name was held before anybody took it");
        let held = (env.hold)(&name).expect("the name should be takeable");
        assert!((env.is_held)(&name), "a held name does not read as held");
        drop(held);
        assert!(
            !(env.is_held)(&name),
            "the name stayed held after being dropped, so a hidden window would still read as \
             showing and the backend would never stop behind it"
        );
    }
}

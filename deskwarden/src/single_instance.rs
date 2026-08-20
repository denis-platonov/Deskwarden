//! **One Deskwarden per logon session, and the newest one wins.**
//!
//! # What this replaces
//!
//! `app_mutex` could always *detect* a second copy -- `Acquired::AlreadyRunning`
//! -- and deliberately did nothing with it beyond a log line saying the global
//! hotkey "will not register in this one". That line was a claim nobody
//! implemented: `main` went on to register the hotkey unconditionally an hour
//! and a half later, `RegisterHotKey` refused because the first copy held the
//! chord, and the `expect` behind it took the process down with exit 101. Two
//! statements that had to agree, in two places, which is this codebase's house
//! defect.
//!
//! Both halves are now gone. The hotkey cannot be fatal any more (see
//! `crate::hotkey`), and **the state that log line described no longer exists**:
//! a second Deskwarden does not run alongside the first hotkey-less, it takes
//! over. The newly launched copy is the one the user just asked for, so it is
//! the one that gets to be Deskwarden; the copy already running stands down.
//!
//! # "Kill" means the outgoing copy walks out of its own door
//!
//! The outgoing process is a password manager that may hold an unlocked vault,
//! a decrypted item cache and a password sitting on the clipboard. A
//! `TerminateProcess` -- which is what a Restart-Manager kill or a
//! `taskkill /F` would be -- runs no destructor and no exit path, and would
//! leave that password pasteable and `bw serve` orphaned serving a decrypted
//! vault on localhost. That is materially worse than the crash this work
//! started from. It is also the exact argument `app_mutex`'s own docs make for
//! why the *installer* asks rather than force-closing, and the same argument
//! binds here.
//!
//! So the handover is a request, not a kill: a named event
//! ([`QUIT_EVENT_NAME`]) that the running copy waits on from a thread of its
//! own, and answers by running the shutdown it would run for a tray Quit --
//! zeroize the item cache, take the copied secret back off the clipboard, exit
//! 0 -- which lets the kernel's kill-on-close job object take `bw serve` down
//! with it (`job_object`).
//!
//! # Why a thread of its own, and why the hook is published
//!
//! The wait cannot live in the main loop. The reported run spent 1h43m inside
//! the startup vault window before the main loop existed at all, and a takeover
//! request during that hour has to be answered -- so the listener is a thread
//! started as early as there is anything to tear down, and what it tears down
//! is a hook `main` publishes ([`on_takeover`]) rather than state it borrows.
//! Same idiom, and the same reason, as `update_panel::install_env`'s
//! `before_install`: a background thread cannot borrow `main`'s locals.
//!
//! Before that hook is installed there is no decrypted vault to protect -- the
//! cache does not exist yet -- so a request arriving in that window is answered
//! by exiting, which is all there is to do.
//!
//! # What the outgoing instance does NOT do
//!
//! It does not wait for work in flight -- a sync, a Send being created, an
//! update download. All three run on background threads and all three are
//! ended by the exit. That is deliberate and it is the smaller harm: none of
//! them can leave local state half-written (the vault of record is
//! Bitwarden's, and a create either reached it or did not), whereas waiting
//! for them would make the handover unbounded exactly when the user is
//! staring at a launch that has produced nothing yet -- and the one operation
//! most likely to be in flight, a sync, is also the one it costs least to
//! repeat.
//!
//! It also does not try to hand its vault window over. The user sees the old
//! copy's window and tray icon disappear and the new copy's startup take their
//! place, with both trays visible for the few milliseconds in between. A
//! window that closed *without* a new one arriving would be alarming; this is
//! a relaunch, and it looks like one.
//!
//! # The self-update handover is untouched
//!
//! `updater::launch_installer` releases the mutex immediately before spawning
//! setup, precisely so the silent install is not blocked by the app that asked
//! for it. Nothing here changes that: this module never holds the mutex itself,
//! it asks `app_mutex` for it, and the outgoing instance's listener exits the
//! process rather than releasing anything. An installer run is not a second
//! Deskwarden and never creates [`QUIT_EVENT_NAME`], so it cannot be mistaken
//! for one.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
};

use crate::app_mutex::{self, Acquired};

/// The named event a running Deskwarden waits on to be asked to stand down.
///
/// Same GUID and same `Local\` scope as [`app_mutex::APP_MUTEX_NAME`], and for
/// the same two reasons: the GUID makes the name un-collidable with an
/// unrelated program, and the scope makes this a per-logon-session question,
/// which is the only scope in which "another Deskwarden is running" is even
/// meaningful -- two users signed in at once each get their own Deskwarden and
/// neither may stand the other's down.
///
/// [`the_quit_event_is_scoped_and_named_like_the_mutex`] pins both against the
/// one place the GUID is authored.
pub const QUIT_EVENT_NAME: &str = "Local\\Deskwarden-63CBCB72-5383-4AE7-AFB7-5EE0530E4630-quit";

/// How long the incoming instance waits for the outgoing one to be gone.
///
/// **Five seconds.** What is being waited for is not a UI teardown: the
/// listener thread runs `cache.clear()`, one clipboard call and
/// `process::exit(0)`, none of which touch the main thread, so none of it can
/// be blocked by a modal dialog, a stuck `bw serve` fetch or a vault window
/// mid-paint. The work is microseconds; the five seconds are headroom for a
/// badly loaded machine and for the kernel to finish tearing the process down
/// after the exit, not for the shutdown itself.
///
/// It is bounded because the alternative outcomes are both worse: hanging
/// forever gives the user a launched app with no window and no explanation,
/// and proceeding anyway restores the two-instance state this module exists to
/// abolish -- with, in the worst case, two trays and two vaults of the same
/// account.
pub const TAKEOVER_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the incoming instance re-checks. Fifty checks inside
/// [`TAKEOVER_TIMEOUT`], which makes the common case -- the outgoing instance
/// gone in a few milliseconds -- cost a few milliseconds, not a fixed wait.
pub const POLL_EVERY: Duration = Duration::from_millis(100);

/// What starting up found, and what this process should do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startup {
    /// Nothing else was running. Carry on.
    Sole,
    /// Another Deskwarden was running, was asked to stand down, and has gone.
    /// Carry on, as the only one.
    TookOver { waited: Duration },
    /// Another Deskwarden is running and this process must not.
    GaveUp(GaveUp),
}

/// The two ways a takeover can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaveUp {
    /// The mutex name exists but nothing is listening on [`QUIT_EVENT_NAME`] --
    /// most likely a Deskwarden older than this mechanism, possibly a
    /// half-dead process. There is nobody to ask, so the user is asked
    /// instead.
    NoOneToAsk,
    /// It was asked and it is still there after [`TAKEOVER_TIMEOUT`].
    StillRunning,
}

impl GaveUp {
    /// What the user is told when the new copy cannot become the only copy.
    ///
    /// This one *is* a dialog, unlike the hotkey's status line, and the
    /// difference is the difference between a degraded convenience and a
    /// process that is about to exit having shown nothing at all: the user
    /// double-clicked Deskwarden and something has to account for the fact
    /// that no new window appeared.
    pub fn message(self) -> &'static str {
        match self {
            GaveUp::NoOneToAsk => {
                "Deskwarden is already running on this PC, and this copy could not ask it to \
                 close.\n\nQuit the running Deskwarden from its tray icon, then start it \
                 again."
            }
            GaveUp::StillRunning => {
                "Deskwarden is already running on this PC and did not close when this copy \
                 asked it to.\n\nQuit the running Deskwarden from its tray icon, then start \
                 it again."
            }
        }
    }
}

/// The outside-world half of a takeover, as **`fn` pointers**.
///
/// The `VaultFrameEnv` idiom, and the constraint that forces it is absolute: a
/// test of this decision may not spawn a second process, so the two things
/// that touch one -- asking it to quit, and finding out whether it is gone --
/// have to be substitutable. What is left over is the part worth testing: the
/// order, the bound, and which of [`Startup`]'s three answers each situation
/// produces.
pub struct TakeoverEnv {
    /// Signals [`QUIT_EVENT_NAME`]. `false` means nothing was listening.
    pub ask_to_quit: fn() -> bool,
    /// Drops **this** process's handle on the app mutex.
    ///
    /// Not housekeeping -- the wait does not work without it. `app_mutex::
    /// acquire` opened a handle to the existing name on the way to reporting
    /// `AlreadyRunning`, and a named kernel object lives as long as any handle
    /// does. Left held, this process would keep alive the very name it is
    /// waiting to see disappear and would time out against itself, every time,
    /// no matter how promptly the other copy left.
    pub let_go: fn(),
    /// `true` once the app mutex is held by *this* process, i.e. the other one
    /// is gone.
    pub take_the_mutex: fn() -> bool,
    /// Waits. Substituted in tests so the bound can be exercised without
    /// spending it.
    pub sleep: fn(Duration),
}

impl TakeoverEnv {
    /// The real one.
    pub fn production() -> Self {
        TakeoverEnv {
            ask_to_quit: signal_quit,
            let_go: app_mutex::release,
            take_the_mutex: app_mutex::take_if_free,
            sleep: std::thread::sleep,
        }
    }
}

/// **The whole decision, in one place.**
///
/// `main` calls this once, with what `app_mutex::acquire` found, and does what
/// it says. There is no second place that consults the mutex state and no
/// second place that decides what a duplicate launch means -- which is the
/// defect that produced the reported crash, in its general form.
pub fn resolve(acquired: Result<Acquired, String>, env: &TakeoverEnv) -> Startup {
    match acquired {
        Ok(Acquired::First) => Startup::Sole,
        // A mutex that could not be created is not evidence of a second
        // instance, and `app_mutex` already treats its own failure as
        // non-fatal: the mutex is how the installer notices this app, not how
        // the app works. Refusing to start the user's password manager over a
        // housekeeping handle would be a worse answer than the one it is
        // guarding against.
        Err(_) => Startup::Sole,
        Ok(Acquired::AlreadyRunning) => take_over(env),
    }
}

/// Ask, then wait for the name to come free, then take it.
fn take_over(env: &TakeoverEnv) -> Startup {
    if !(env.ask_to_quit)() {
        // Nothing was listening, so nothing is going to leave and this
        // process is not going to become the only one. The handle stays held:
        // there is nothing to hand it over to, and this process is on its way
        // out with a dialog.
        return Startup::GaveUp(GaveUp::NoOneToAsk);
    }
    // Before the first poll, never after it -- see `TakeoverEnv::let_go`.
    (env.let_go)();
    // The `+ 1` is not slack: `POLL_EVERY` divides `TAKEOVER_TIMEOUT` exactly,
    // and integer division of equal durations would otherwise leave the final
    // interval unchecked -- the app would wait the full timeout and then never
    // look.
    let polls = (TAKEOVER_TIMEOUT.as_millis() / POLL_EVERY.as_millis()) as u32;
    for poll in 1..=polls {
        (env.sleep)(POLL_EVERY);
        if (env.take_the_mutex)() {
            return Startup::TookOver { waited: POLL_EVERY * poll };
        }
    }
    Startup::GaveUp(GaveUp::StillRunning)
}

// ---------------------------------------------------------------------------
// The outgoing side
// ---------------------------------------------------------------------------

/// What the outgoing instance runs before it exits.
///
/// Published rather than borrowed, for the reason in the module docs: the
/// listener is a thread, and the vault cache it has to zeroize is a `main`
/// local. `None` until `main` has something to tear down.
static ON_TAKEOVER: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

/// Publishes the shutdown the listener runs. Called once, by `main`, as soon
/// as the vault cache exists.
pub fn on_takeover(shutdown: Arc<dyn Fn() + Send + Sync>) {
    if let Ok(mut held) = ON_TAKEOVER.lock() {
        *held = Some(shutdown);
    }
}

/// Runs the published shutdown, if there is one. Separated from the exit so a
/// test can drive it: a `-> !` body is unreachable from a test runner by
/// construction, and "the secrets really were taken down" is the half that
/// matters -- the same split `main`'s
/// `take_the_session_down_after_a_second_lock` is written with.
pub fn run_takeover_shutdown() {
    run_shutdown(ON_TAKEOVER.lock().ok().and_then(|held| held.clone()));
}

/// [`run_takeover_shutdown`] with the published hook supplied, so a test can
/// drive both of its cases without touching process-wide state that another
/// test in the same binary may be reading at the same moment.
fn run_shutdown(shutdown: Option<Arc<dyn Fn() + Send + Sync>>) {
    match shutdown {
        Some(shutdown) => shutdown(),
        // Nothing published yet means startup has not built a vault cache, so
        // there is nothing decrypted in this process to take down.
        None => log::info!(
            "a newer Deskwarden asked this one to stand down before it had a vault to take \
             down; exiting"
        ),
    }
}

/// Starts the listener thread. Called once, by `main`, in the instance that
/// holds the mutex.
///
/// Returns whether the event could be created. `false` costs this instance
/// nothing except the ability to be taken over gracefully -- a later launch
/// gets [`GaveUp::NoOneToAsk`] and tells the user to close this one by hand,
/// which is the same outcome as before this module existed and is not worth
/// refusing to start over.
pub fn listen_for_takeover() -> bool {
    let handle = match create_quit_event() {
        Ok(handle) => handle,
        Err(e) => {
            log::warn!(
                "could not create the takeover event ({e}); a second Deskwarden launched later \
                 will ask the user to close this one instead of closing it itself"
            );
            return false;
        }
    };
    // `HANDLE` is a raw pointer and so not `Send`. It is moved into exactly
    // one thread, which owns it for the life of the process and never shares
    // it, so the `usize` round-trip below is the honest way to say that rather
    // than a hole: nothing else can reach the value.
    let raw = handle.0 as usize;
    std::thread::Builder::new()
        .name("deskwarden-takeover".into())
        .spawn(move || {
            let handle = HANDLE(raw as *mut _);
            // INFINITE: this thread has one job for the life of the process
            // and nothing to poll for. A timed wait would be a loop that woke
            // up forever to learn nothing.
            let waited = unsafe { WaitForSingleObject(handle, INFINITE) };
            if waited != WAIT_OBJECT_0 {
                log::warn!("the takeover listener stopped waiting ({waited:?}); ignoring");
                return;
            }
            log::info!(
                "a newer Deskwarden asked this one to stand down; clearing the vault cache \
                 and the clipboard, then exiting"
            );
            run_takeover_shutdown();
            // `bw serve` is not stopped here by hand: it is a member of the
            // kill-on-close job object, so the kernel takes it down with this
            // process however this process ends -- the same reasoning the
            // update path's `before_install` is written with.
            std::process::exit(0);
        })
        .map(|_| true)
        .unwrap_or_else(|e| {
            log::warn!("could not start the takeover listener ({e})");
            // The handle leaks with the failed thread. One kernel handle, once
            // per process, in a branch that means thread creation is failing;
            // closing it here would race nothing but is not worth the unsafe.
            false
        })
}

/// Creates the manual-reset named event, owned for the life of the process.
///
/// Manual reset: the signal means "stand down", which is a one-way fact about
/// this process, not a token to be consumed. An auto-reset event would let a
/// second asker find it clear again and conclude nothing had happened.
fn create_quit_event() -> windows::core::Result<HANDLE> {
    unsafe { CreateEventW(None, true, false, &HSTRING::from(QUIT_EVENT_NAME)) }
}

/// Signals a running instance's quit event. `false` means there was none to
/// open -- see [`GaveUp::NoOneToAsk`].
fn signal_quit() -> bool {
    unsafe {
        let Ok(handle) = OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(QUIT_EVENT_NAME))
        else {
            return false;
        };
        let set = SetEvent(handle).is_ok();
        let _ = CloseHandle(handle);
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // The substituted world. `fn` pointers cannot capture, so the script each
    // test wants is kept in statics and reset by the test that reads it.
    // Every test below is `#[serial]`-free by construction: each uses its own
    // pair of counters and no two tests share one.
    static ASKS: AtomicU32 = AtomicU32::new(0);
    static SLEEPS: AtomicU32 = AtomicU32::new(0);
    static POLLS: AtomicU32 = AtomicU32::new(0);
    /// How many polls the substituted outgoing instance survives.
    static SURVIVES: AtomicU32 = AtomicU32::new(0);

    fn reset(survives: u32) {
        ASKS.store(0, Ordering::SeqCst);
        SLEEPS.store(0, Ordering::SeqCst);
        POLLS.store(0, Ordering::SeqCst);
        SURVIVES.store(survives, Ordering::SeqCst);
    }

    fn asks_and_is_heard() -> bool {
        ASKS.fetch_add(1, Ordering::SeqCst);
        true
    }

    fn asks_and_is_not_heard() -> bool {
        ASKS.fetch_add(1, Ordering::SeqCst);
        false
    }

    fn sleeps(_: Duration) {
        SLEEPS.fetch_add(1, Ordering::SeqCst);
    }

    /// Free once the scripted number of polls has gone by.
    fn free_after_the_script() -> bool {
        let poll = POLLS.fetch_add(1, Ordering::SeqCst) + 1;
        poll >= SURVIVES.load(Ordering::SeqCst)
    }

    fn never_free() -> bool {
        POLLS.fetch_add(1, Ordering::SeqCst);
        false
    }

    static LETS_GO: AtomicU32 = AtomicU32::new(0);

    fn lets_go() {
        LETS_GO.fetch_add(1, Ordering::SeqCst);
    }

    fn env(ask: fn() -> bool, take: fn() -> bool) -> TakeoverEnv {
        TakeoverEnv { ask_to_quit: ask, let_go: lets_go, take_the_mutex: take, sleep: sleeps }
    }

    /// **The ordinary launch, asserted positively.** Nothing else was running,
    /// so nothing is asked, nothing is waited for, and the app starts.
    ///
    /// Without this the rest would pass with the feature deleted: a `resolve`
    /// that always returned `Sole` satisfies "a lone launch starts".
    #[test]
    fn a_lone_launch_asks_nobody_and_starts() {
        reset(1);
        let env = env(asks_and_is_heard, free_after_the_script);
        assert_eq!(resolve(Ok(Acquired::First), &env), Startup::Sole);
        assert_eq!(ASKS.load(Ordering::SeqCst), 0, "a lone launch signalled a takeover at nobody");
        assert_eq!(POLLS.load(Ordering::SeqCst), 0);
    }

    /// **The takeover: asked, waited for, and taken.**
    ///
    /// The outgoing instance is scripted to be gone on the second poll, so
    /// this pins that the wait ends when the other copy goes rather than when
    /// the timeout does -- a takeover that always spent `TAKEOVER_TIMEOUT`
    /// would pass a "does it eventually start" test while making every
    /// relaunch feel broken.
    #[test]
    fn a_second_launch_asks_the_first_to_go_and_becomes_the_only_one() {
        reset(2);
        let env = env(asks_and_is_heard, free_after_the_script);
        assert_eq!(
            resolve(Ok(Acquired::AlreadyRunning), &env),
            Startup::TookOver { waited: POLL_EVERY * 2 }
        );
        assert_eq!(ASKS.load(Ordering::SeqCst), 1, "the outgoing instance was asked more than once");
        assert_eq!(POLLS.load(Ordering::SeqCst), 2, "the wait did not stop when the name came free");
        assert_eq!(SLEEPS.load(Ordering::SeqCst), 2);
    }

    /// **The bound is real, and it is the bound that was chosen.**
    ///
    /// An outgoing instance that never goes must end in `GaveUp`, after
    /// exactly `TAKEOVER_TIMEOUT` worth of polling and not one poll more --
    /// and never in a second instance carrying on regardless.
    #[test]
    fn an_outgoing_instance_that_will_not_go_stops_this_one() {
        reset(0);
        let env = env(asks_and_is_heard, never_free);
        assert_eq!(
            resolve(Ok(Acquired::AlreadyRunning), &env),
            Startup::GaveUp(GaveUp::StillRunning)
        );
        let polls = POLLS.load(Ordering::SeqCst);
        assert_eq!(
            POLL_EVERY * polls,
            TAKEOVER_TIMEOUT,
            "the wait was {polls} polls, which is not the timeout this module documents"
        );
    }

    // The ordering pin below keeps its own pair of counters, because it is
    // the only test that cares about the ORDER of two of the seams and the
    // rest share theirs.
    static HELD_AT_FIRST_POLL: AtomicU32 = AtomicU32::new(0);
    static STILL_HOLDING: AtomicU32 = AtomicU32::new(1);

    fn releases_the_handle() {
        STILL_HOLDING.store(0, Ordering::SeqCst);
    }

    fn polls_and_notes_whether_we_let_go() -> bool {
        HELD_AT_FIRST_POLL.fetch_add(STILL_HOLDING.load(Ordering::SeqCst), Ordering::SeqCst);
        true
    }

    /// **This process lets go of the name before it waits for the name to go
    /// away.**
    ///
    /// The failure this pins is silent and total: `acquire` leaves a handle
    /// open on the existing mutex, a named object outlives its last handle
    /// and no longer, so an incoming instance that polled while still holding
    /// one would keep the name alive itself and time out every single time --
    /// including when the outgoing copy left instantly. Nothing about the
    /// symptom would point here.
    #[test]
    fn the_wait_starts_by_letting_go_of_this_process_s_own_handle() {
        let env = TakeoverEnv {
            ask_to_quit: asks_and_is_heard,
            let_go: releases_the_handle,
            take_the_mutex: polls_and_notes_whether_we_let_go,
            sleep: sleeps,
        };
        assert!(matches!(resolve(Ok(Acquired::AlreadyRunning), &env), Startup::TookOver { .. }));
        assert_eq!(
            HELD_AT_FIRST_POLL.load(Ordering::SeqCst),
            0,
            "the takeover polled for a free mutex name while still holding a handle to it, so \
             the name can never come free and every takeover times out"
        );
    }

    /// Nothing listening means nothing to wait for: the user is told at once
    /// rather than after a five-second stare at nothing.
    #[test]
    fn a_first_instance_that_cannot_be_asked_is_not_waited_for() {
        reset(1);
        let env = env(asks_and_is_not_heard, free_after_the_script);
        assert_eq!(resolve(Ok(Acquired::AlreadyRunning), &env), Startup::GaveUp(GaveUp::NoOneToAsk));
        assert_eq!(POLLS.load(Ordering::SeqCst), 0);
        assert_eq!(SLEEPS.load(Ordering::SeqCst), 0);
    }

    /// A mutex that could not be created is not a second instance. Starting
    /// the user's password manager wins over a housekeeping handle -- the same
    /// call `app_mutex::acquire`'s own docs make about its failure.
    #[test]
    fn a_mutex_that_could_not_be_created_does_not_stop_the_app() {
        reset(1);
        let env = env(asks_and_is_heard, free_after_the_script);
        assert_eq!(resolve(Err("access denied".to_string()), &env), Startup::Sole);
        assert_eq!(ASKS.load(Ordering::SeqCst), 0);
    }

    /// Both refusals say what happened and how to get out of it. A launch that
    /// exits silently is the failure mode this whole task started from.
    #[test]
    fn both_refusals_name_the_way_out() {
        for gave_up in [GaveUp::NoOneToAsk, GaveUp::StillRunning] {
            let message = gave_up.message();
            assert!(
                message.contains("already running"),
                "{gave_up:?} does not say why nothing opened: {message:?}"
            );
            assert!(
                message.contains("tray icon"),
                "{gave_up:?} does not tell the user where the running copy is: {message:?}"
            );
        }
    }

    /// The published shutdown is what the listener runs, and it runs it.
    ///
    /// This is the half of the handover that makes it a handover rather than a
    /// kill: the cache zeroize and the clipboard clear live in a closure
    /// `main` publishes, and a listener that dropped it on the floor would
    /// leave a password pasteable on a machine whose Deskwarden had, as far as
    /// the user could tell, just closed.
    #[test]
    fn the_listener_runs_the_shutdown_main_published() {
        let ran = Arc::new(AtomicU32::new(0));
        let ran_in_hook = Arc::clone(&ran);
        run_shutdown(Some(Arc::new(move || {
            ran_in_hook.fetch_add(1, Ordering::SeqCst);
        })));
        assert_eq!(
            ran.load(Ordering::SeqCst),
            1,
            "the takeover exited without running the teardown, so the outgoing instance would \
             leave a decrypted cache and a copied password behind it"
        );
    }

    /// And with nothing published -- a takeover during startup, before the
    /// vault cache exists -- it is a no-op rather than a panic.
    #[test]
    fn a_takeover_before_there_is_anything_to_tear_down_is_survivable() {
        run_shutdown(None);
    }

    /// The published hook really is what `run_takeover_shutdown` reads --
    /// the control for the two tests above, which drive `run_shutdown`
    /// directly.
    #[test]
    fn what_main_publishes_is_what_the_listener_reads() {
        let ran = Arc::new(AtomicU32::new(0));
        let ran_in_hook = Arc::clone(&ran);
        on_takeover(Arc::new(move || {
            ran_in_hook.fetch_add(1, Ordering::SeqCst);
        }));
        run_takeover_shutdown();
        assert!(
            ran.load(Ordering::SeqCst) >= 1,
            "`on_takeover` published a shutdown that `run_takeover_shutdown` cannot see"
        );
    }

    /// The event name is the mutex name's sibling: same GUID, same scope.
    /// Authored apart, they could drift into a takeover that asks a different
    /// program to quit, or one that cannot be asked at all.
    #[test]
    fn the_quit_event_is_scoped_and_named_like_the_mutex() {
        assert!(
            QUIT_EVENT_NAME.starts_with(app_mutex::APP_MUTEX_NAME),
            "{QUIT_EVENT_NAME:?} is not derived from {:?}, so the two can name different \
             programs",
            app_mutex::APP_MUTEX_NAME
        );
        assert!(
            QUIT_EVENT_NAME.starts_with("Local\\"),
            "{QUIT_EVENT_NAME:?} is not session-local: one signed-in user's launch would stand \
             down another user's running Deskwarden"
        );
        assert_ne!(
            QUIT_EVENT_NAME,
            app_mutex::APP_MUTEX_NAME,
            "the event and the mutex are the same name, which is two different kernel objects \
             fighting over one string"
        );
    }

    /// The timeout is a bound with a shape, not a number someone typed: it has
    /// to be long enough to poll more than once and short enough that a user
    /// staring at a launch that has produced no window is not left waiting.
    #[test]
    fn the_takeover_bound_is_a_bound() {
        assert!(POLL_EVERY < TAKEOVER_TIMEOUT);
        assert_eq!(
            TAKEOVER_TIMEOUT.as_millis() % POLL_EVERY.as_millis(),
            0,
            "the poll interval does not divide the timeout, so the wait is not the timeout"
        );
        assert!(
            TAKEOVER_TIMEOUT <= Duration::from_secs(10),
            "a launch that shows nothing for more than a few seconds reads as a launch that \
             did nothing"
        );
    }
}

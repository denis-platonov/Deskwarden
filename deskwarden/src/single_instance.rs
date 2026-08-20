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
//! # And when there is nobody to ask, it is a kill after all
//!
//! Everything above is the *normal* path and stays the normal path. What it
//! could not answer is the case that actually happened: a build older than
//! this mechanism was running. It creates the mutex but never creates
//! [`QUIT_EVENT_NAME`], so there is nobody listening -- and the incoming copy
//! put up "another Deskwarden is running and would not stand down" and exited
//! with status 1. The user had launched the new build and got nothing at all.
//! That is the wrong answer, and it is the wrong answer for the reason at the
//! top of this file: **the copy the user just launched is the one that gets to
//! be Deskwarden.** If the running one cannot be asked to leave, it is ended.
//!
//! So the order is: ask, and wait; and *only* if there was nobody to ask, or
//! nobody answered inside [`TAKEOVER_TIMEOUT`], find the running process and
//! end it ([`end_other_deskwardens`]), then wait [`FORCE_TIMEOUT`] for the
//! mutex name to come free. Never the other way round, and never both. An
//! instance that stands down when asked is never force-ended, which is what
//! [`tests::a_first_instance_that_stands_down_is_never_forced`] is for.
//!
//! ## What the force costs, and what it does not
//!
//! A forced end runs no destructor and no exit path. Two things follow, and
//! both belong in the record:
//!
//! * **`bw serve` still dies.** It is a member of the outgoing process's
//!   kill-on-close job object (`job_object`), and the kernel closes that job
//!   however the process ends -- `TerminateProcess` included. So no orphaned
//!   backend is left serving a decrypted vault on localhost, which is the harm
//!   that would have made this trade unacceptable.
//! * **A copied secret may be left on the clipboard.** The graceful path takes
//!   it back off; this one cannot. The exposure is bounded, but not by
//!   anything *this* process does: the outgoing build runs its own clipboard
//!   timer, on by default at `clipboard::DEFAULT_CLEAR_AFTER` (one minute) --
//!   so the exposure is bounded by that timer if it fired before the kill, and
//!   after the kill nothing will fire at all.
//!
//! **The incoming instance does not clear the clipboard, and that is
//! deliberate.** `clipboard::clear_if_still_ours_for` keys on a sequence
//! number *this* process recorded when *this* process copied something, and
//! that is exactly what makes it safe: it can prove the clipboard still holds
//! what it wrote. An incoming instance has no such proof about another
//! process's copy -- what it would be emptying is just as likely to be the
//! paragraph the user copied a minute ago -- and silently clobbering the
//! user's clipboard on every relaunch is a defect of its own, not a
//! mitigation. So the cost is written into the log in plain words (see
//! `main`'s [`Handover::Forced`] arm) rather than paid for with data that
//! cannot be shown to be ours.
//!
//! ## Which process, and how sure
//!
//! A mutex has no readable owner in Win32, so the running instance has to be
//! found by enumerating processes -- and ending the wrong one is far worse
//! than the bug being fixed. [`end_other_deskwardens`] documents the four
//! checks a candidate has to pass before `TerminateProcess` is called on it.
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
//!
//! **The forced path does not disturb this either**, for two independent
//! reasons. `launch_installer` releases the mutex *before* it spawns setup, so
//! an instance launched while an install is in progress finds
//! `Acquired::First` and never reaches [`take_over`] at all; and even if it
//! did, [`end_other_deskwardens`] only ever ends a process whose image file
//! name equals **this** executable's, which setup's does not
//! ([`tests::the_installer_is_not_named_like_the_app_it_installs`]). An
//! installer must never be force-killed by the app it is replacing, and there
//! is no path here on which it can be.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, OpenEventW, OpenProcess, SetEvent, TerminateProcess,
    WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE, PROCESS_TERMINATE,
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

/// How long the incoming instance waits for the mutex name after ending the
/// other process by force.
///
/// **Two seconds**, and shorter than [`TAKEOVER_TIMEOUT`] on purpose: nothing
/// is being waited *for* here. `TerminateProcess` has already returned, the
/// outgoing process runs no shutdown, and all that is left is the kernel
/// closing its handles -- which is what frees the mutex name. Two seconds is
/// headroom for a loaded machine, not time budgeted for work. Still bounded,
/// for [`TAKEOVER_TIMEOUT`]'s reason exactly: the alternative to giving up is
/// double-running, which is what this module exists to abolish.
pub const FORCE_TIMEOUT: Duration = Duration::from_secs(2);

/// What starting up found, and what this process should do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Startup {
    /// Nothing else was running. Carry on.
    Sole,
    /// Another Deskwarden was running and has gone -- see [`Handover`] for by
    /// which of the two routes. Carry on, as the only one.
    ///
    /// `waited` is how long the mutex name took to come free *after* the
    /// route named by `how` had done its part, so on the forced route it does
    /// not include the [`TAKEOVER_TIMEOUT`] that may have been spent asking
    /// first.
    TookOver { waited: Duration, how: Handover },
    /// Another Deskwarden is running and this process must not.
    GaveUp(GaveUp),
}

/// By which of the two routes the outgoing instance left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handover {
    /// **The ordinary case.** It was asked, and it walked out of its own door:
    /// vault cache zeroized, clipboard cleared, `bw serve` taken down with it.
    Asked,
    /// It could not be asked, or would not answer, so `ended` process(es) were
    /// ended with `TerminateProcess`.
    ///
    /// The caller is expected to say so in the log, and to say what it cost --
    /// see the module docs: `bw serve` still dies with the job object, but a
    /// copied secret may be left on the clipboard.
    Forced { why: Unasked, ended: usize },
}

/// Why the graceful ask did not finish the job, and so why force was reached.
///
/// Carried through [`Handover::Forced`] and [`GaveUp`] rather than dropped,
/// because the two mean quite different things to whoever reads the log: one
/// is an expected consequence of upgrading from an old build, the other is a
/// running instance that is wedged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unasked {
    /// The mutex name exists but nothing is listening on [`QUIT_EVENT_NAME`] --
    /// a Deskwarden older than this mechanism, or a half-dead process.
    NoOneToAsk,
    /// It was asked and it was still there after [`TAKEOVER_TIMEOUT`].
    NoAnswer,
}

impl Unasked {
    /// The clause a log line puts after "another Deskwarden was running and".
    pub fn what_happened(self) -> &'static str {
        match self {
            Unasked::NoOneToAsk => {
                "was not listening for a request to stand down (a build older than that \
                 mechanism, or a half-dead process)"
            }
            Unasked::NoAnswer => "did not answer a request to stand down",
        }
    }
}

/// The two ways a takeover can fail -- **both of them after force has already
/// been tried**, because asking is no longer the last word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaveUp {
    /// Nothing was ended: either no running process could be identified as a
    /// Deskwarden with enough confidence to end it, or every attempt was
    /// refused (a different user's session, an elevated process, one already
    /// exiting).
    CouldNotEnd(Unasked),
    /// Something was ended and the app mutex is *still* held after
    /// [`FORCE_TIMEOUT`]. Whatever holds the name is not the process that was
    /// ended, so this launch must not carry on regardless.
    StillRunning(Unasked),
}

impl GaveUp {
    /// What the user is told when the new copy cannot become the only copy.
    ///
    /// This one *is* a dialog, unlike the hotkey's status line, and the
    /// difference is the difference between a degraded convenience and a
    /// process that is about to exit having shown nothing at all: the user
    /// double-clicked Deskwarden and something has to account for the fact
    /// that no new window appeared.
    ///
    /// Neither message mentions the forced attempt. From the user's side the
    /// situation and the way out of it are identical, and "this copy tried to
    /// end the running one and could not" is a sentence that invites a
    /// question the dialog cannot answer. The log says it; the dialog says
    /// what to do.
    pub fn message(self) -> &'static str {
        match self {
            GaveUp::CouldNotEnd(_) => {
                "Deskwarden is already running on this PC, and this copy could not close \
                 it.\n\nQuit the running Deskwarden from its tray icon, then start it \
                 again."
            }
            GaveUp::StillRunning(_) => {
                "Deskwarden is already running on this PC and did not close when this copy \
                 asked it to.\n\nQuit the running Deskwarden from its tray icon, then start \
                 it again."
            }
        }
    }

    /// Why force was reached at all, for the log line that reports the
    /// refusal.
    pub fn unasked(self) -> Unasked {
        match self {
            GaveUp::CouldNotEnd(why) | GaveUp::StillRunning(why) => why,
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
    /// **Ends the running Deskwarden(s) by force**, returning how many were
    /// actually terminated.
    ///
    /// Zero means nothing was ended, and the decision does not care which of
    /// the two zeroes it is -- nothing found, or found and refused -- because
    /// the answer is the same either way. The production implementation logs
    /// the difference; see [`end_other_deskwardens`].
    ///
    /// Behind the seam for the same absolute constraint as the rest of this
    /// struct, with more force: **no test in this crate may enumerate real
    /// processes or terminate anything.** What is left on this side of the
    /// seam is the part worth testing -- that asking comes first, that a
    /// listening instance never reaches here at all, and that a refusal ends
    /// in [`GaveUp`] rather than in two Deskwardens.
    pub end_by_force: fn() -> usize,
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
            end_by_force: end_other_deskwardens,
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

/// **Ask, wait, and only then force.**
///
/// The order is the whole design and it is written in one place so it cannot
/// be two statements that have to agree. Asking always happens first, and a
/// running instance that answers is never touched again -- the force branch is
/// reachable only through an ask that found nobody or a wait that ran out.
fn take_over(env: &TakeoverEnv) -> Startup {
    // Asked first, always. `None` here means it was asked and heard.
    let unheard = (!(env.ask_to_quit)()).then_some(Unasked::NoOneToAsk);
    // Before the first poll, never after it -- see `TakeoverEnv::let_go`. It
    // has to happen on the forced path too, and for the identical reason:
    // this process's own handle keeps the name alive, so a poll that still
    // held one would time out against itself no matter how thoroughly the
    // other process had been ended.
    (env.let_go)();
    if unheard.is_none() {
        // It was heard. Give it the whole graceful bound to leave through its
        // own door, which is the outcome worth waiting for: only that route
        // clears the clipboard and zeroizes the cache.
        if let Some(waited) = wait_for_the_name(env, TAKEOVER_TIMEOUT) {
            return Startup::TookOver { waited, how: Handover::Asked };
        }
    }
    // Nothing was listening, or nothing answered. Either way the polite route
    // is exhausted, and the user is standing in front of a launch that has
    // produced no window.
    let why = unheard.unwrap_or(Unasked::NoAnswer);
    let ended = (env.end_by_force)();
    if ended == 0 {
        // Nothing could be ended, so nothing has changed and waiting again
        // would only spend `FORCE_TIMEOUT` learning that. Refuse honestly
        // rather than double-run.
        return Startup::GaveUp(GaveUp::CouldNotEnd(why));
    }
    match wait_for_the_name(env, FORCE_TIMEOUT) {
        Some(waited) => Startup::TookOver { waited, how: Handover::Forced { why, ended } },
        // Something was ended and the name is still held, so whatever holds
        // it is not what was ended. Same refusal as before this path existed:
        // a launch that cannot become the only Deskwarden does not become the
        // second one.
        None => Startup::GaveUp(GaveUp::StillRunning(why)),
    }
}

/// Polls for the mutex name to come free, up to `bound`, returning how long it
/// took.
///
/// `1..=polls` is not slack: `POLL_EVERY` divides both bounds exactly, and
/// integer division of equal durations would otherwise leave the final
/// interval unchecked -- the app would wait the full bound and then never
/// look.
fn wait_for_the_name(env: &TakeoverEnv, bound: Duration) -> Option<Duration> {
    let polls = (bound.as_millis() / POLL_EVERY.as_millis()) as u32;
    for poll in 1..=polls {
        (env.sleep)(POLL_EVERY);
        if (env.take_the_mutex)() {
            return Some(POLL_EVERY * poll);
        }
    }
    None
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

// ---------------------------------------------------------------------------
// The forced path
// ---------------------------------------------------------------------------

/// The logon session a process id belongs to, or `None` if it cannot be read.
///
/// `None` is treated as "not ours" by every caller, which is the safe
/// direction: a process whose session cannot be established is one this code
/// cannot show is in scope, and the scope is the point (see
/// [`end_other_deskwardens`]).
fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0u32;
    unsafe { ProcessIdToSessionId(pid, &mut session) }.ok().map(|()| session)
}

/// The file name of this executable, lowercased, or `None` if it cannot be
/// read.
///
/// Read from `current_exe` rather than written down as `"deskwarden.exe"` on
/// purpose. A hard-coded name is a second statement that has to agree with the
/// first -- this codebase's house defect -- and it would be wrong in exactly
/// the situation this feature is for: a build run from `target\release\` under
/// a different name, or a renamed copy, would enumerate for a name no running
/// process has. Comparing against our own name compares like with like, and it
/// is also what makes the installer safe: setup is not named what we are named.
fn my_image_name() -> Option<String> {
    let path = std::env::current_exe().ok()?;
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    (!name.is_empty()).then_some(name)
}

/// The file name at the end of a full image path, lowercased.
fn image_name_of(path: &str) -> String {
    path.rsplit('\\').next().unwrap_or(path).to_lowercase()
}

/// **Ends every running Deskwarden that is not this one**, returning how many
/// were terminated.
///
/// Reached only from [`take_over`], and only after asking has failed.
///
/// # Which processes, and how sure
///
/// Win32 offers no way to read a mutex's owner, so the process holding the app
/// mutex has to be *found*. Ending the wrong process is far worse than the bug
/// this fixes, so a candidate has to pass all four of these before
/// `TerminateProcess` is called on it:
///
/// 1. **Its image file name equals this executable's**, case-insensitively --
///    see [`my_image_name`] for why it is read rather than written down.
/// 2. **It is not this process.** Obvious, and it is also the only check whose
///    absence would be instantly fatal rather than merely wrong.
/// 3. **It is in this logon session.** This is the check that does the real
///    work, and it is not a nicety: the name being contended for is
///    `Local\`-scoped ([`app_mutex::APP_MUTEX_NAME`]), so the process holding
///    it is *necessarily* in this session, and a same-named process in another
///    session is *necessarily* not it. Without this check, two users signed in
///    at once would have each other's Deskwarden killed out from under them by
///    a relaunch -- the exact harm the `Local\` scope was chosen to prevent.
/// 4. **Its image path still resolves, to the same file name.** The snapshot's
///    `szExeFile` is a stale copy taken at enumeration time; re-reading the
///    path through `QueryFullProcessImageNameW` on a freshly opened handle
///    re-establishes the name against the process that pid means *now*, which
///    is what closes the window in which a pid is recycled between the
///    snapshot and the kill.
///
/// # What is deliberately NOT checked
///
/// Nothing here proves the candidate is *this build*, or that it is the
/// process holding the mutex rather than a second one that lost the race. Both
/// are unavailable: an old build is precisely what this path exists for, so a
/// version check would refuse the only case that matters, and a mutex has no
/// owner to ask. What the four checks above establish is the honest claim --
/// *a process running this same executable name, in this same logon session*
/// -- and that is the population the single-instance rule is defined over in
/// the first place.
///
/// # The log
///
/// Every decision this function makes is written down, at `warn` for a kill
/// and at `debug` for a candidate passed over. A forced termination that
/// showed up later only as a process that was no longer there is the kind of
/// thing that costs somebody an afternoon.
fn end_other_deskwardens() -> usize {
    let Some(mine) = my_image_name() else {
        log::warn!(
            "cannot end the running Deskwarden: this process's own executable name could not \
             be read, and it is the name every candidate is matched against"
        );
        return 0;
    };
    let me = unsafe { GetCurrentProcessId() };
    let Some(my_session) = session_of(me) else {
        log::warn!(
            "cannot end the running Deskwarden: this process's logon session could not be \
             read, and no process is ended without matching it"
        );
        return 0;
    };

    let mut seen = 0usize;
    let mut ended = 0usize;
    for pid in same_named_processes(&mine, me) {
        seen += 1;
        match session_of(pid) {
            Some(session) if session == my_session => {}
            other => {
                log::debug!(
                    "pid {pid} is named {mine} but is in logon session {other:?}, not \
                     {my_session}; leaving it alone -- another signed-in user's Deskwarden is \
                     not this one's to end"
                );
                continue;
            }
        }
        let Some(path) = crate::window_watch::process_image_path_for_pid(pid) else {
            log::debug!(
                "pid {pid} is named {mine} but its image path could not be re-read (it may \
                 have exited already); leaving it alone"
            );
            continue;
        };
        if image_name_of(&path) != mine {
            log::debug!(
                "pid {pid} was listed as {mine} but now resolves to {path}; the id was reused \
                 between the snapshot and now, so it is left alone"
            );
            continue;
        }
        log::warn!(
            "ending Deskwarden pid {pid} ({path}) with TerminateProcess, because it could not \
             be asked to stand down and this launch is the one the user just started. It runs \
             NO shutdown: its `bw serve` dies with it (kill-on-close job object, so no \
             decrypted vault is left served on localhost), but a password it had copied stays \
             on the clipboard until that copy's own clipboard timer clears it -- and this \
             process cannot clear a clipboard it cannot show is its own"
        );
        if terminate(pid) {
            ended += 1;
        }
    }

    if seen == 0 {
        log::warn!(
            "the app mutex is held but no other process named {mine} is running in this logon \
             session, so there is nothing to end"
        );
    } else {
        log::warn!("ended {ended} of {seen} running Deskwarden process(es) by force");
    }
    ended
}

/// Every process id in the system whose snapshot image name equals `mine`,
/// except `me`.
///
/// Split out so that the walk -- which is all `unsafe` and all bookkeeping --
/// is separate from the decisions made about what it returns.
fn same_named_processes(mine: &str, me: u32) -> Vec<u32> {
    let mut found = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            log::warn!("could not enumerate processes, so no running Deskwarden can be found");
            return found;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok {
            let end = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase();
            // pid 0 is the idle process and can never be a candidate; `me` is
            // this process, and ending it would be the one mistake with no
            // recovery at all.
            if name == mine && entry.th32ProcessID != me && entry.th32ProcessID != 0 {
                found.push(entry.th32ProcessID);
            }
            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

/// `TerminateProcess` on one pid, reporting whether it worked.
///
/// **Failure is expected and survivable**, which is why it is reported rather
/// than asserted: `OpenProcess` is refused for a process in another security
/// context or at a higher integrity level, and both calls fail for a process
/// that has exited since it was enumerated. The caller counts what actually
/// happened, and a count of zero becomes [`GaveUp::CouldNotEnd`] -- a launch
/// that says why it will not start, never a second Deskwarden.
///
/// Exit code 1: this is a kill, not a clean exit, and it should not be
/// mistaken for one by anything reading the outgoing process's exit code.
fn terminate(pid: u32) -> bool {
    unsafe {
        let handle = match OpenProcess(PROCESS_TERMINATE, false, pid) {
            Ok(handle) => handle,
            Err(e) => {
                log::warn!(
                    "could not open Deskwarden pid {pid} to end it ({e}); it may be running as \
                     another user or at a higher integrity level, or it may have exited already"
                );
                return false;
            }
        };
        let killed = TerminateProcess(handle, 1);
        let _ = CloseHandle(handle);
        match killed {
            Ok(()) => true,
            Err(e) => {
                log::warn!("could not end Deskwarden pid {pid} ({e})");
                false
            }
        }
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

    /// A world in which force ends nothing. Deliberately counter-free: it is
    /// shared by every test that uses [`env`], and a counter shared between
    /// tests that run in parallel asserts nothing. Tests that care what force
    /// did bring their own, below.
    fn ends_nothing() -> usize {
        0
    }

    fn env(ask: fn() -> bool, take: fn() -> bool) -> TakeoverEnv {
        forcing_env(ask, take, ends_nothing)
    }

    fn forcing_env(ask: fn() -> bool, take: fn() -> bool, force: fn() -> usize) -> TakeoverEnv {
        TakeoverEnv {
            ask_to_quit: ask,
            let_go: lets_go,
            take_the_mutex: take,
            end_by_force: force,
            sleep: sleeps,
        }
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
            Startup::TookOver { waited: POLL_EVERY * 2, how: Handover::Asked }
        );
        assert_eq!(ASKS.load(Ordering::SeqCst), 1, "the outgoing instance was asked more than once");
        assert_eq!(POLLS.load(Ordering::SeqCst), 2, "the wait did not stop when the name came free");
        assert_eq!(SLEEPS.load(Ordering::SeqCst), 2);
    }

    /// **The graceful bound is real, and it is the bound that was chosen.**
    ///
    /// An outgoing instance that never goes must spend exactly
    /// `TAKEOVER_TIMEOUT` worth of polling and not one poll more before force
    /// is even considered -- and, with force ending nothing, must end in
    /// `GaveUp` rather than in a second instance carrying on regardless.
    #[test]
    fn an_outgoing_instance_that_will_not_go_stops_this_one() {
        reset(0);
        let env = env(asks_and_is_heard, never_free);
        assert_eq!(
            resolve(Ok(Acquired::AlreadyRunning), &env),
            Startup::GaveUp(GaveUp::CouldNotEnd(Unasked::NoAnswer))
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
            end_by_force: ends_nothing,
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

    /// Nothing listening means nothing to wait for: the graceful bound is not
    /// spent staring at a process that was never going to answer, and the
    /// decision goes straight to force. Here force ends nothing, so the launch
    /// refuses -- and refuses without having polled once.
    #[test]
    fn a_first_instance_that_cannot_be_asked_is_not_waited_for() {
        reset(1);
        let env = env(asks_and_is_not_heard, free_after_the_script);
        assert_eq!(
            resolve(Ok(Acquired::AlreadyRunning), &env),
            Startup::GaveUp(GaveUp::CouldNotEnd(Unasked::NoOneToAsk))
        );
        assert_eq!(POLLS.load(Ordering::SeqCst), 0);
        assert_eq!(SLEEPS.load(Ordering::SeqCst), 0);
    }

    // -----------------------------------------------------------------------
    // The forced path. Every test below keeps its OWN statics: the four at the
    // top of this module are shared by the tests above, and a counter shared
    // between tests the runner may execute in parallel proves nothing about
    // either of them.
    // -----------------------------------------------------------------------

    /// **Nobody listening: ended by force, and the launch continues.**
    ///
    /// This is the reported case exactly -- a build older than
    /// [`QUIT_EVENT_NAME`] was running, so the ask found nothing -- and the
    /// answer is no longer a message box and exit 1.
    ///
    /// The mutex is scripted to come free **only once force has happened**,
    /// which is what makes this an ordering assertion and not just an outcome
    /// one: a `take_over` that polled before forcing would never see the name
    /// free and could not reach `TookOver` at all.
    #[test]
    fn an_instance_with_nobody_listening_is_ended_by_force_and_the_launch_starts() {
        static ASKS: AtomicU32 = AtomicU32::new(0);
        static FORCES: AtomicU32 = AtomicU32::new(0);
        static FORCES_WHEN_ASKED: AtomicU32 = AtomicU32::new(u32::MAX);

        fn ask() -> bool {
            FORCES_WHEN_ASKED.store(FORCES.load(Ordering::SeqCst), Ordering::SeqCst);
            ASKS.fetch_add(1, Ordering::SeqCst);
            false
        }
        fn force() -> usize {
            FORCES.fetch_add(1, Ordering::SeqCst);
            1
        }
        fn free_once_forced() -> bool {
            FORCES.load(Ordering::SeqCst) > 0
        }

        let env = forcing_env(ask, free_once_forced, force);
        assert_eq!(
            resolve(Ok(Acquired::AlreadyRunning), &env),
            Startup::TookOver {
                waited: POLL_EVERY,
                how: Handover::Forced { why: Unasked::NoOneToAsk, ended: 1 },
            },
            "a running Deskwarden that cannot be asked to stand down still stopped this launch, \
             which is the defect: the copy the user just started got no window and no app"
        );
        assert_eq!(ASKS.load(Ordering::SeqCst), 1, "it was not asked exactly once");
        assert_eq!(FORCES.load(Ordering::SeqCst), 1, "force ran more than once");
        assert_eq!(
            FORCES_WHEN_ASKED.load(Ordering::SeqCst),
            0,
            "a process was ended by force BEFORE it had been asked to stand down. Asking is the \
             only route that clears the outgoing copy's clipboard and zeroizes its vault cache, \
             so it must always come first"
        );
    }

    /// **Asked first, waited out in full, and only then forced.**
    ///
    /// The other entrance to the forced path: it *was* listening, and it did
    /// not answer. The pin is the count of polls that had already been spent
    /// when force ran -- the whole graceful bound, not a shortcut to the kill.
    #[test]
    fn an_instance_that_does_not_answer_is_asked_first_and_only_then_forced() {
        static ASKS: AtomicU32 = AtomicU32::new(0);
        static FORCES: AtomicU32 = AtomicU32::new(0);
        static POLLS: AtomicU32 = AtomicU32::new(0);
        static POLLS_WHEN_FORCED: AtomicU32 = AtomicU32::new(u32::MAX);

        fn ask() -> bool {
            ASKS.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn force() -> usize {
            POLLS_WHEN_FORCED.store(POLLS.load(Ordering::SeqCst), Ordering::SeqCst);
            FORCES.fetch_add(1, Ordering::SeqCst);
            2
        }
        fn free_once_forced() -> bool {
            POLLS.fetch_add(1, Ordering::SeqCst);
            FORCES.load(Ordering::SeqCst) > 0
        }

        let env = forcing_env(ask, free_once_forced, force);
        assert_eq!(
            resolve(Ok(Acquired::AlreadyRunning), &env),
            Startup::TookOver {
                waited: POLL_EVERY,
                how: Handover::Forced { why: Unasked::NoAnswer, ended: 2 },
            }
        );
        assert_eq!(ASKS.load(Ordering::SeqCst), 1);
        assert_eq!(FORCES.load(Ordering::SeqCst), 1);
        let graceful = (TAKEOVER_TIMEOUT.as_millis() / POLL_EVERY.as_millis()) as u32;
        assert_eq!(
            POLLS_WHEN_FORCED.load(Ordering::SeqCst),
            graceful,
            "force ran after {} of the {graceful} graceful polls. A wedged instance is given \
             the WHOLE of `TAKEOVER_TIMEOUT` to leave through its own door before it is killed \
             -- shortening that is trading the clipboard clear for a second or two",
            POLLS_WHEN_FORCED.load(Ordering::SeqCst)
        );
    }

    /// **An instance that stands down when asked is never force-killed.**
    ///
    /// The negative half of the feature, and it is asserted against a world
    /// where force *would* have worked: `force` here returns 1, so a
    /// `take_over` that reached it would sail on to `TookOver` and the outcome
    /// assertion alone would not notice. `FORCES` is what notices.
    ///
    /// This test would also pass with the whole forced path deleted, which is
    /// exactly why the two tests above it exist.
    #[test]
    fn a_first_instance_that_stands_down_is_never_forced() {
        static FORCES: AtomicU32 = AtomicU32::new(0);
        static POLLS: AtomicU32 = AtomicU32::new(0);

        fn ask() -> bool {
            true
        }
        fn force() -> usize {
            FORCES.fetch_add(1, Ordering::SeqCst);
            1
        }
        fn free_on_the_second_poll() -> bool {
            POLLS.fetch_add(1, Ordering::SeqCst) + 1 >= 2
        }

        let env = forcing_env(ask, free_on_the_second_poll, force);
        assert_eq!(
            resolve(Ok(Acquired::AlreadyRunning), &env),
            Startup::TookOver { waited: POLL_EVERY * 2, how: Handover::Asked }
        );
        assert_eq!(
            FORCES.load(Ordering::SeqCst),
            0,
            "a Deskwarden that answered the request and walked out of its own door was \
             `TerminateProcess`ed anyway, throwing away the clipboard clear and the cache \
             zeroize that made the graceful path worth having"
        );
    }

    /// **A refused termination does not become a silent double-run.**
    ///
    /// `force` finds nothing to end (or is refused by every candidate), so the
    /// launch refuses -- and the trap is that `take_the_mutex` here says the
    /// name IS free. A `take_over` that polled anyway would report a takeover
    /// that never happened and start a second Deskwarden alongside the first.
    #[test]
    fn a_refused_termination_does_not_become_a_second_deskwarden() {
        static POLLS: AtomicU32 = AtomicU32::new(0);
        static FORCES: AtomicU32 = AtomicU32::new(0);

        fn force_that_ends_nothing() -> usize {
            FORCES.fetch_add(1, Ordering::SeqCst);
            0
        }
        fn always_free() -> bool {
            POLLS.fetch_add(1, Ordering::SeqCst);
            true
        }

        let env = forcing_env(asks_and_is_not_heard, always_free, force_that_ends_nothing);
        assert_eq!(
            resolve(Ok(Acquired::AlreadyRunning), &env),
            Startup::GaveUp(GaveUp::CouldNotEnd(Unasked::NoOneToAsk))
        );
        assert_eq!(FORCES.load(Ordering::SeqCst), 1, "the forced attempt was not made at all");
        assert_eq!(
            POLLS.load(Ordering::SeqCst),
            0,
            "nothing was ended, yet the mutex was polled for and taken anyway -- so a launch \
             whose kill was refused would run as a SECOND Deskwarden, which is worse than the \
             refusal it replaces"
        );
    }

    /// **Ended, and the name is still held: still a refusal, still bounded.**
    ///
    /// Something was terminated but the app mutex did not come free, so
    /// whatever holds it is not what was ended. The launch stops, after
    /// exactly `FORCE_TIMEOUT` worth of polling and not one poll more.
    #[test]
    fn an_ended_instance_whose_name_stays_held_stops_this_one() {
        static POLLS: AtomicU32 = AtomicU32::new(0);

        fn ends_one() -> usize {
            1
        }
        fn never_free_here() -> bool {
            POLLS.fetch_add(1, Ordering::SeqCst);
            false
        }

        let env = forcing_env(asks_and_is_not_heard, never_free_here, ends_one);
        assert_eq!(
            resolve(Ok(Acquired::AlreadyRunning), &env),
            Startup::GaveUp(GaveUp::StillRunning(Unasked::NoOneToAsk))
        );
        let polls = POLLS.load(Ordering::SeqCst);
        assert_eq!(
            POLL_EVERY * polls,
            FORCE_TIMEOUT,
            "the wait after the kill was {polls} polls, which is not `FORCE_TIMEOUT`"
        );
    }

    /// The bound after the kill is a bound, and it is shorter than the one
    /// before it -- nothing is being waited FOR once `TerminateProcess` has
    /// returned, only the kernel closing handles.
    #[test]
    fn the_force_bound_is_a_shorter_bound() {
        assert!(POLL_EVERY < FORCE_TIMEOUT);
        assert_eq!(
            FORCE_TIMEOUT.as_millis() % POLL_EVERY.as_millis(),
            0,
            "the poll interval does not divide `FORCE_TIMEOUT`, so the wait is not the bound"
        );
        assert!(
            FORCE_TIMEOUT <= TAKEOVER_TIMEOUT,
            "waiting LONGER after a kill than for a polite shutdown has it backwards: the \
             polite path has work to do and this one has none"
        );
    }

    /// **The installer cannot be mistaken for a Deskwarden to end.**
    ///
    /// `end_other_deskwardens` matches on this executable's own file name, so
    /// the claim that a self-update can never be force-killed by the app it
    /// replaces rests on setup being named something else. Inno authors that
    /// name in the `.iss`; this reads it, so the two cannot drift into a
    /// running installer being terminated by the copy it is replacing.
    #[test]
    fn the_installer_is_not_named_like_the_app_it_installs() {
        const ISS: &str = include_str!("../installer/deskwarden.iss");
        let setup = ISS
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with(';'))
            .find_map(|line| line.strip_prefix("OutputBaseFilename="))
            .expect("control: installer/deskwarden.iss has no `OutputBaseFilename=` line")
            .to_lowercase();
        assert_ne!(
            setup, "deskwarden",
            "the installer is built as {setup}.exe, which is this app's own executable name. \
             `end_other_deskwardens` matches on that name, so a launch during a self-update \
             would `TerminateProcess` the running installer"
        );
        // And the app's own name really is what the `.iss` installs, so the
        // assertion above is comparing setup against the right thing rather
        // than against a string that appears nowhere.
        assert!(
            ISS.contains("deskwarden.exe"),
            "control: the `.iss` no longer mentions `deskwarden.exe`, so the name this test \
             compares against is not the name that ships"
        );
    }

    /// The file name at the end of a path, which is the whole of what
    /// candidate identification compares -- and it is case-insensitive,
    /// because Windows paths are.
    #[test]
    fn an_image_path_is_reduced_to_its_file_name_case_insensitively() {
        assert_eq!(image_name_of(r"C:\Program Files\Deskwarden\DeskWarden.EXE"), "deskwarden.exe");
        assert_eq!(image_name_of("deskwarden.exe"), "deskwarden.exe");
        assert_ne!(
            image_name_of(r"C:\x\notdeskwarden.exe"),
            "deskwarden.exe",
            "a longer name ending in the app's name matched it, so an unrelated program could \
             be terminated"
        );
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
        for why in [Unasked::NoOneToAsk, Unasked::NoAnswer] {
            for gave_up in [GaveUp::CouldNotEnd(why), GaveUp::StillRunning(why)] {
                let message = gave_up.message();
                assert!(
                    message.contains("already running"),
                    "{gave_up:?} does not say why nothing opened: {message:?}"
                );
                assert!(
                    message.contains("tray icon"),
                    "{gave_up:?} does not tell the user where the running copy is: {message:?}"
                );
                assert_eq!(
                    gave_up.unasked(),
                    why,
                    "{gave_up:?} lost the reason force was reached, which is the half of the \
                     log line that tells an old build apart from a wedged one"
                );
            }
            assert!(
                !why.what_happened().is_empty(),
                "{why:?} has no clause for the log line to put after \"another Deskwarden was \
                 running and\""
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

//! Who is currently using the vault, as a fact the kernel keeps.
//!
//! # The question this answers
//!
//! `docs/superpowers/specs/2026-08-27-one-door-to-the-vault.md` makes the
//! vault service something **both apps** start, reconnect to and let go of:
//! the first to need the vault starts it, the last to stop needing it exits
//! it. That needs an answer to "is anybody still using this?" which survives
//! an app dying without being asked.
//!
//! A count this app maintains cannot do it. A crashed app decrements nothing,
//! so the service would stay up forever holding a vault nobody is looking at
//! -- and that is the exact failure the design is trying not to introduce
//! while it gives up the job object's kill-on-close guarantee.
//!
//! # Why one name per app, and not one shared name
//!
//! The obvious design is a single named object every app opens. It does not
//! work, and [`crate::app_mutex`] already documents why:
//!
//! > a `CreateMutexW` that finds the name already there still *opens a handle
//! > to it*, and a named object lives as long as any handle does
//!
//! So the process **asking** whether anyone is attached keeps the name alive
//! by asking. `app_mutex::take_if_free` exists precisely because an instance
//! polling with `acquire` "would wait out its whole timeout against itself".
//! Any design where a supervisor tests one shared name has that bug.
//!
//! A mutex *owned* by each app fails differently: a mutex has one owner at a
//! time, and two apps have to be attached at once.
//!
//! Counting the connection would work for a persistent pipe -- a client
//! vanishing is a kernel-visible disconnect -- but `bw serve` is HTTP on a
//! port with no persistent per-client connection, and a mechanism that only
//! works for one of the two services reintroduces the split this design
//! exists to remove.
//!
//! **So: one name per app.** Each app creates its own, holds it while it needs
//! the vault, and the OS releases it on death however that death happened. A
//! supervisor asks by *opening* each recorded name and dropping the handle
//! immediately -- a name with no live holder cannot be opened, and a handle
//! dropped at once does not keep one alive.
//!
//! # There is no list of names
//!
//! Something has to say which names to try. The first version of this module
//! kept a register of the names in use -- and that register was in-process,
//! so two apps in two processes did not share it, which is exactly the case
//! it existed for.
//!
//! A **fixed slot space** removes it. The names are `Deskwarden-Attach-0`
//! through `-15`; an app takes the first one it can create, and a supervisor
//! probes all sixteen. Every process derives the same names from the same
//! constant, so there is no bookkeeping to share, go stale, or be written
//! before a crash. The cost is a ceiling of sixteen simultaneous apps, which
//! is eight times more than this design has uses for.

use std::sync::Arc;

/// The kernel calls this module makes, as `fn` pointers.
///
/// `single_instance::TakeoverEnv` and `vault_disk_cache::DiskCacheEnv`'s
/// idiom, for their reason: no test in this crate may create a named kernel
/// object, and every decision on this side of the seam -- which names are
/// tried, what an unopenable name means, when nobody is attached -- is worth
/// driving directly.
pub struct ServiceEnv {
    /// Creates a name and returns a token that keeps it alive until dropped.
    /// `None` when the name could not be created at all.
    pub hold: fn(&str) -> Option<Held>,
    /// Whether `name` currently has a live holder.
    ///
    /// **Must not retain a handle.** An implementation that kept one would
    /// answer "yes" forever after the first ask, which is this module's whole
    /// hazard.
    pub is_held: fn(&str) -> bool,
    /// Ends the service listening on a port.
    ///
    /// Present so that [`verify`] refusing to call it is a fact a test can
    /// assert rather than a property of a signature. An unverifiable
    /// process is never passed here.
    pub stop: fn(u16),
}

/// A live hold on one name. Dropping it releases the name; so does the
/// process ending, by any route.
pub struct Held {
    /// Never read, and that is the point: the handle exists so that dropping
    /// it releases the name. Named with a leading underscore because a
    /// value held purely for its `Drop` is what this is.
    _handle: Arc<dyn Send + Sync>,
}

impl Held {
    /// Wraps whatever the platform hands back. The type is opaque because
    /// nothing above this module has any business inspecting a handle.
    #[must_use]
    pub fn new(inner: Arc<dyn Send + Sync>) -> Self {
        Self { _handle: inner }
    }
}

/// One app's claim on the vault, for as long as it holds this.
///
/// Dropping it is the clean release. Not dropping it -- a crash, a kill -- is
/// the case the design exists for, and is handled by the same mechanism: the
/// name goes with the process.
pub struct Attachment {
    slot: usize,
    _held: Held,
}

impl Attachment {
    /// Which slot this claim occupies. Exposed for logging: a user looking
    /// at two attached apps should be able to tell them apart.
    #[must_use]
    pub fn slot(&self) -> usize {
        self.slot
    }
}

/// How many apps can be attached at once.
///
/// Two today -- the daemon and the vault window. Sixteen so that a stuck
/// name, a second account, or a future third app costs nothing, and small
/// enough that probing every slot is one cheap loop rather than a scan.
///
/// **This constant is what replaces the register.** An earlier version of
/// this module kept a list of the names in use, and that list was
/// in-process: two apps in two processes did not share it. A fixed slot
/// space needs no list at all -- the names are the same in every process
/// because they are a constant, so the bookkeeping that could go stale is
/// gone rather than moved into a file.
pub const SLOTS: usize = 16;

/// The name of one attachment slot, per logon session.
#[must_use]
pub fn attach_slot_name(slot: usize) -> String {
    format!("Local\\Deskwarden-Attach-{slot}")
}

/// Claims the first free slot, or `None` if all of them are taken.
///
/// `hold` is exclusive -- `app_mutex::take_if_free`'s idiom -- so two apps
/// racing for the same slot cannot both win it, and the loser simply moves
/// to the next one.
///
/// The slot is claimed **before** the caller can use the vault, so a
/// supervisor looking during start-up sees an attached app rather than
/// missing one.
#[must_use]
pub fn attach(env: &ServiceEnv) -> Option<Attachment> {
    (0..SLOTS).find_map(|slot| {
        (env.hold)(&attach_slot_name(slot)).map(|held| Attachment { slot, _held: held })
    })
}

/// Whether anybody OTHER than this process is attached.
///
/// **The daemon needs this rather than [`anyone_attached`], and the
/// difference is not pedantic:** the daemon holds a slot of its own from
/// the moment it starts, so `anyone_attached` is true for the whole of its
/// life and would answer "somebody needs the vault" even when nobody but
/// itself does. Asked that way it would pin `bw serve` up forever and
/// quietly undo save-memory mode.
///
/// `mine` is the slot to ignore. `None` means this process holds none, in
/// which case this is exactly [`anyone_attached`].
#[must_use]
pub fn anyone_else_attached(env: &ServiceEnv, mine: Option<usize>) -> bool {
    (0..SLOTS)
        .filter(|slot| Some(*slot) != mine)
        .any(|slot| (env.is_held)(&attach_slot_name(slot)))
}

/// Whether any slot still has a live holder.
///
/// Every slot is asked about: a dead slot is not evidence about the others,
/// and slots are not claimed in order once one is released.
#[must_use]
pub fn anyone_attached(env: &ServiceEnv) -> bool {
    (0..SLOTS).any(|slot| (env.is_held)(&attach_slot_name(slot)))
}

/// What starting the vault service did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Startup {
    /// A service was already running and proved itself. **Reconnect always
    /// precedes restart**: restarting costs a cold start and, on the direct
    /// backend, another Windows Hello prompt.
    Adopted,
    /// Nothing was running, and this app started one.
    StartedIt,
    /// Another app was starting one at the same moment. Losing that race is
    /// not an error -- the loser waits for the winner's service and adopts
    /// it, which is the whole point of there being one door.
    AdoptedAfterLosingRace,
    /// Something is on the port that this app cannot identify, or a service
    /// for another account is running. Nothing is started and nothing is
    /// stopped; the caller reports that it has no backend.
    Refused(Refusal),
    /// The race was lost and the winner's service never appeared.
    RaceWonByNobody,
}

/// Starting a service, as `fn` pointers, for [`ServiceEnv`]'s reason.
pub struct StartEnv {
    /// The account fingerprint whatever is on `port` claims to serve, or
    /// `None` if nothing answered. Never trusted -- see [`verify`].
    pub probe: fn(u16) -> Option<String>,
    /// Spawns the service. `false` if it could not be spawned.
    pub start: fn(u16) -> bool,
    /// Claims the right to be the one that starts it. `None` means another
    /// app got there first. The same named-object race `single_instance`
    /// already resolves, rather than a second scheme.
    pub take_start_lock: fn() -> Option<Held>,
    /// The loser's brief wait, before it looks at the port a second time.
    ///
    /// A seam and not a `sleep` inside this function, for the usual reason:
    /// a hidden sleep makes every test that reaches this path slow, and a
    /// test cannot assert that the wait happened at all. Losing the race and
    /// re-probing in the same instant would find nothing almost every time,
    /// which would turn every race into a spurious `RaceWonByNobody`.
    pub settle: fn(),
}

/// Adopt a running service, or start one, in that order.
#[must_use]
pub fn ensure_running(env: &ServiceEnv, start: &StartEnv, ours: &str, port: u16) -> Startup {
    // Reconnect before restart, always.
    match verify(env, ours, (start.probe)(port).as_deref(), port) {
        Verdict::Adopt => return Startup::Adopted,
        Verdict::Refuse(Refusal::NothingAnswered) => {}
        Verdict::Refuse(other) => return Startup::Refused(other),
    }

    let Some(_lock) = (start.take_start_lock)() else {
        // Lost the race. The winner is starting one; give it a moment and ask
        // the port again, rather than starting a second service on top of it.
        (start.settle)();
        return match verify(env, ours, (start.probe)(port).as_deref(), port) {
            Verdict::Adopt => Startup::AdoptedAfterLosingRace,
            Verdict::Refuse(Refusal::NothingAnswered) => Startup::RaceWonByNobody,
            Verdict::Refuse(other) => Startup::Refused(other),
        };
    };

    if (start.start)(port) { Startup::StartedIt } else { Startup::RaceWonByNobody }
}

/// Releases this app's claim, and stops the service if it was the last one.
///
/// Takes the `Attachment` by value: the release and the check cannot be
/// separated, because a check that ran before the release would always see
/// this app still attached and never stop anything.
pub fn release(env: &ServiceEnv, attachment: Attachment, port: u16) {
    drop(attachment);
    if !anyone_attached(env) {
        (env.stop)(port);
    }
}

/// The named object the service itself holds, per account and per logon
/// session.
///
/// The fingerprint is in the **name**, not in an answer read off the port.
/// An answer can be forged by anything that has read this repository; a name
/// binds the object to the account before it is opened, so `verify` never
/// has to trust what the port said about *which* account it serves -- only
/// to notice when it disagrees.
///
/// `Local\\` for [`crate::app_mutex`]'s reason: a second logon session is a
/// different user, and its service is not ours to adopt.
#[must_use]
pub fn service_object_name(account_fingerprint: &str) -> String {
    format!("Local\\Deskwarden-Vault-{account_fingerprint}")
}

/// What to do about a service that is already listening.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// It is ours, for this account. Reconnect rather than restart.
    Adopt,
    /// Leave it alone, and say why.
    Refuse(Refusal),
}

/// Why a listening service was not adopted.
///
/// These are kept apart because the caller acts differently on each:
/// silence means start one, the other two mean report that the backend could
/// not be started and change nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Nothing answered on the port.
    NothingAnswered,
    /// Something answered, but no service object is held for the account it
    /// claimed. Any process can bind a loopback port and answer our shape of
    /// JSON; loopback is not an authentication boundary.
    NothingHoldsTheServiceObject,
    /// A real service, holding a real object -- for a different account.
    ServesAnotherAccount,
}

/// Whether the thing listening on `port` may be adopted.
///
/// `claimed` is the account fingerprint the listener says it is serving, or
/// `None` if nothing answered. It is **not trusted**: it selects which name
/// to test, and a listener that claims an account it cannot prove fails at
/// the next line.
///
/// The pid is deliberately not part of this. Pids are reused, and the case
/// this has to survive is exactly the one where the app that remembered a
/// pid is gone.
///
/// # What this does not defend against
///
/// A process in **this same logon session** could create the service name
/// first and squat it. That is not a boundary this can hold: a same-session
/// process already has this user's DPAPI. What it does mean is that our own
/// service then fails to create its name and refuses to start, which is the
/// safe direction -- stated here rather than left to be discovered.
#[must_use]
pub fn verify(env: &ServiceEnv, ours: &str, claimed: Option<&str>, port: u16) -> Verdict {
    let _ = port;
    let Some(claimed) = claimed else {
        return Verdict::Refuse(Refusal::NothingAnswered);
    };
    // Asked about the name the LISTENER claims, not the name we want. Testing
    // only our own name would report a second account's service as an
    // impostor, and the two need different handling.
    if !(env.is_held)(&service_object_name(claimed)) {
        return Verdict::Refuse(Refusal::NothingHoldsTheServiceObject);
    }
    if claimed != ours {
        return Verdict::Refuse(Refusal::ServesAnotherAccount);
    }
    Verdict::Adopt
}


/// How often the supervisor re-checks whether anybody is still attached.
///
/// **This number is the size of the orphan window**, and it is stated here
/// rather than left to be inferred. Between the last app dying and the next
/// check, a `bw serve` holding an unlocked vault can still be listening on
/// `BW_SERVE_PORT`. The job object this design gives up made that window
/// zero; this makes it five seconds, which is the honest trade and not an
/// elimination.
///
/// Five rather than one because each check is sixteen `OpenMutexW` calls
/// against a service that is almost always in use, and rather than sixty
/// because a minute of an orphaned unlocked vault is a long time.
pub const ORPHAN_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// How supervision ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supervised {
    /// Nobody was attached any more, so the service was stopped. This is the
    /// outcome for a clean last release AND for every app crashing, which is
    /// the case the supervisor exists for.
    Stopped,
    /// The loop was told to end while somebody was still attached -- the
    /// supervisor itself is shutting down. **The service is left running**,
    /// because an app is still using it.
    GaveUp,
}

/// The supervisor's clock, as a `fn` pointer.
pub struct SupervisorEnv {
    /// Waits one [`ORPHAN_CHECK_INTERVAL`] and returns whether to keep going.
    ///
    /// A seam rather than a `sleep` for [`StartEnv::settle`]'s reason, and
    /// one more here: a test that had to wait five real seconds per check
    /// would be deleted by the next person in a hurry.
    pub tick: fn() -> bool,
}

/// Stops the service once nobody is attached, however the apps went away.
///
/// This is what bounds the orphan window rather than eliminating it. A clean
/// release goes through [`release`] and does not wait for a tick; this is
/// the path for the case where nothing clean happened at all -- every app
/// killed, so no `Drop` ran anywhere and no count was decremented.
///
/// It stops the service **only** when no slot is held. A supervisor that
/// closed a vault somebody was still using would be a worse bug than the
/// leak it was written to prevent.
#[must_use]
pub fn supervise(env: &ServiceEnv, sup: &SupervisorEnv, port: u16) -> Supervised {
    loop {
        if !anyone_attached(env) {
            (env.stop)(port);
            return Supervised::Stopped;
        }
        if !(sup.tick)() {
            return Supervised::GaveUp;
        }
    }
}


/// What stopping the service is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopAction {
    /// End the `bw serve` child THIS process spawned, through the handle it
    /// owns.
    EndTheChildWeStarted,
    /// Do nothing. Something is on the port that this process did not start,
    /// so it is not this process's to end -- the same refusal [`verify`]
    /// makes when it declines to adopt an unidentifiable service and
    /// declines to kill it.
    LeaveItAlone,
}

/// The whole stop decision, as a pure function.
///
/// Separated from the doing so the rule is a thing tests drive rather than a
/// branch buried in a `fn` pointer. The rule is one line: **a handle, never
/// a port.** A process listening on `BW_SERVE_PORT` may be another account's
/// service, another user's, or something that took the port first; ending it
/// because it is in the way is precisely what this design refuses.
#[must_use]
pub fn stop_action(we_started_it: bool) -> StopAction {
    if we_started_it { StopAction::EndTheChildWeStarted } else { StopAction::LeaveItAlone }
}

/// The `bw serve` this process started, if it started one.
///
/// `app_mutex::HELD`'s idiom: a process-wide handle that outlives the stack
/// frame that made it. It exists because the switch-over takes `bw serve`
/// out of the kill-on-close job, and something then has to be able to end it
/// -- and the only honest something is the handle we already hold.
static OUR_SERVICE: std::sync::Mutex<Option<std::process::Child>> =
    std::sync::Mutex::new(None);

/// Records the child this process spawned, so [`supervise`] can end it.
///
/// Call it **immediately** after the spawn. A child spawned and not recorded
/// is the orphan this whole design is trying not to create: out of the job,
/// and unknown to the only code that would stop it.
pub fn remember_our_service(child: std::process::Child) {
    if let Ok(mut slot) = OUR_SERVICE.lock() {
        if let Some(mut previous) = slot.replace(child) {
            // Replacing without ending the old one would leak exactly the
            // orphan this exists to prevent.
            log::warn!("a second bw serve was recorded; ending the first");
            crate::bw_serve::stop_bw_serve(&mut previous);
        }
    }
}

/// Hands back the child, so nothing can stop it twice.
///
/// Taking rather than borrowing is deliberate: a second stop would `kill` a
/// pid that has been reaped and may by then belong to somebody else.
#[must_use]
pub fn take_our_service() -> Option<std::process::Child> {
    OUR_SERVICE.lock().ok().and_then(|mut slot| slot.take())
}

/// Whether this process started the service that is running.
#[must_use]
pub fn we_started_the_service() -> bool {
    OUR_SERVICE.lock().is_ok_and(|slot| slot.is_some())
}

/// The real kernel behind [`ServiceEnv`], for the running app.
///
/// `app_mutex::create_named`'s idiom, with the one difference this module
/// needs: **exclusive**. `app_mutex` wants a handle either way and reads
/// `ERROR_ALREADY_EXISTS` only to report it; here a name already taken means
/// the slot is somebody else's, so the handle is dropped and `None` comes
/// back. Keeping it would claim a slot this process does not own AND keep
/// the name alive after its owner died, which is the bug the whole module
/// is arranged around.
#[must_use]
pub fn windows_env() -> ServiceEnv {
    ServiceEnv { hold: win_hold, is_held: win_is_held, stop: win_stop }
}

fn win_hold(name: &str) -> Option<Held> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    unsafe {
        let handle = CreateMutexW(None, false, &HSTRING::from(name)).ok()?;
        // Read immediately: `CreateMutexW` succeeds either way and separates
        // the two cases only through the last-error code, which anything
        // else would overwrite.
        let existed = GetLastError() == ERROR_ALREADY_EXISTS;
        let owned = OwnedHandle::from_raw_handle(handle.0);
        if existed {
            // Dropping it is the `CloseHandle`. The owner's handle keeps the
            // name alive; ours would too, which is why it does not survive
            // this branch.
            drop(owned);
            return None;
        }
        Some(Held::new(Arc::new(owned)))
    }
}

/// Opens the name and drops the handle in the same expression.
///
/// **The drop is the point.** A handle kept here would answer "yes" forever
/// after the first ask, which is this module's whole hazard; `SYNCHRONIZE`
/// is the crate's own `SYNCHRONIZATION_SYNCHRONIZE` and **not a hand-written
/// constant**: an earlier version of this spelled it `0x0010`, which is a
/// meaningless bit for this object -- `SYNCHRONIZE` is a STANDARD right,
/// `0x0010_0000`. Every `OpenMutexW` failed with `ERROR_ACCESS_DENIED`, so
/// this function reported nothing attached while two processes were holding
/// slots, and every unit test passed because the fake kernel never reached
/// this line. `examples/vault_slots_probe.rs` is what found it.
fn win_is_held(name: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};

    unsafe {
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(name)) {
            Ok(handle) => {
                use std::os::windows::io::{FromRawHandle, OwnedHandle};
                drop(OwnedHandle::from_raw_handle(handle.0));
                true
            }
            Err(_) => false,
        }
    }
}

/// Ends the `bw serve` this process started, and nothing else.
///
/// The `port` is logged and **not used to find anything**. It is here
/// because [`ServiceEnv::stop`] is shaped for a service identified by port,
/// and keeping the parameter while refusing to act on it is the honest way
/// to say that the identification comes from somewhere better.
fn win_stop(port: u16) {
    match stop_action(we_started_the_service()) {
        StopAction::EndTheChildWeStarted => match take_our_service() {
            Some(mut child) => {
                log::info!("nobody is attached; ending the bw serve we started on port {port}");
                crate::bw_serve::stop_bw_serve(&mut child);
            }
            // Lost the race with another stop between the check and the
            // take. Nothing to do, and nothing to complain about.
            None => log::debug!("the service on port {port} was already ended"),
        },
        StopAction::LeaveItAlone => log::info!(
            "nobody is attached, but this process did not start the service on port {port}; \
             leaving it alone"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // The fake kernel is the two statics below rather than a value: the seam
    // is `fn` pointers, which cannot close over a fixture. `Held` carries a
    // guard whose `Drop` removes the name, so releasing a hold in a test is
    // dropping the value -- and *abandoning* one, the crash case, is
    // `std::mem::forget`.
    /// **The fake kernel is process-wide, so these tests are not parallel.**
    ///
    /// `ServiceEnv` is `fn` pointers, which cannot close over a fixture, so
    /// the fake's state has to live in statics -- and Rust runs `#[test]`
    /// functions on many threads at once. Without this lock one test's
    /// `reset` wipes another's slots mid-run.
    ///
    /// Found the way it should be: these passed serially and failed in
    /// parallel, and the parallel run is the one CI does. They had been
    /// green by luck of scheduling until enough tests were added elsewhere
    /// to change the order.
    ///
    /// Poison is deliberately ignored. A panicking test poisons this lock,
    /// and every later test failing to acquire it would report the first
    /// failure over and over instead of its own.
    static SERIALISE: Mutex<()> = Mutex::new(());

    static LIVE: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    static OPENS: AtomicUsize = AtomicUsize::new(0);

    fn reset() {
        *LIVE.lock().unwrap() = Some(HashSet::new());
        OPENS.store(0, Ordering::SeqCst);
    }

    struct Guard(String);

    impl Drop for Guard {
        fn drop(&mut self) {
            if let Ok(mut live) = LIVE.lock() {
                if let Some(set) = live.as_mut() {
                    set.remove(&self.0);
                }
            }
        }
    }

    /// Exclusive, as the real `CreateMutexW` + `ERROR_ALREADY_EXISTS` check
    /// is: a name already held cannot be held again. A permissive fake here
    /// would let `attach` hand two apps the same slot and every slot test
    /// below would pass without meaning anything.
    fn hold(name: &str) -> Option<Held> {
        let mut live = LIVE.lock().ok()?;
        if !live.as_mut()?.insert(name.to_string()) {
            return None;
        }
        Some(Held::new(Arc::new(Guard(name.to_string()))))
    }

    fn is_held(name: &str) -> bool {
        OPENS.fetch_add(1, Ordering::SeqCst);
        LIVE.lock()
            .ok()
            .and_then(|live| live.as_ref().map(|set| set.contains(name)))
            .unwrap_or(false)
    }

    fn env() -> ServiceEnv {
        ServiceEnv { hold, is_held, stop }
    }

    /// Two apps attached at once, which one shared mutex could not express.
    #[test]
    fn two_apps_can_be_attached_at_the_same_time() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let a = attach(&env()).expect("a");
        let b = attach(&env()).expect("b");
        assert_ne!(a.slot(), b.slot(), "two apps were handed the same slot");
        assert!(anyone_attached(&env()));
        drop(a);
        assert!(anyone_attached(&env()), "one release ended the vault for both apps");
        drop(b);
        assert!(!anyone_attached(&env()));
    }

    /// A released slot is reusable, which is what makes a fixed space of
    /// sixteen enough: without this, sixteen launches would exhaust it.
    #[test]
    fn a_released_slot_is_handed_out_again() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let first = attach(&env()).expect("first");
        let slot = first.slot();
        drop(first);
        assert_eq!(attach(&env()).expect("second").slot(), slot);
    }

    /// **The daemon must not see its own slot as a reason to stay up.**
    ///
    /// It holds one for its whole life, so asking `anyone_attached` would
    /// answer yes forever and pin `bw serve` up, undoing save-memory mode
    /// in the name of the counting that was meant to make it work.
    #[test]
    fn a_process_does_not_count_itself_as_somebody_needing_the_vault() {
        reset();
        let mine = attach(&env()).expect("the daemon's own slot");
        assert!(
            anyone_attached(&env()),
            "control: with only its own slot held, the unfiltered question says yes"
        );
        assert!(
            !anyone_else_attached(&env(), Some(mine.slot())),
            "the daemon counted its own slot, which would keep bw serve up forever"
        );

        // And a second holder -- the service -- is seen.
        let theirs = attach(&env()).expect("the service's slot");
        assert!(
            anyone_else_attached(&env(), Some(mine.slot())),
            "a second process needing the vault went unnoticed"
        );
        drop(theirs);
        assert!(!anyone_else_attached(&env(), Some(mine.slot())));
    }

    /// With no slot of its own, the filtered question is the plain one.
    #[test]
    fn holding_no_slot_makes_the_two_questions_the_same() {
        reset();
        assert!(!anyone_else_attached(&env(), None));
        let _theirs = attach(&env()).expect("somebody");
        assert!(anyone_else_attached(&env(), None));
    }

    /// **The test the design exists for.** An app that dies without releasing
    /// anything must still read as detached: a crash decrements no count, and
    /// a service that waited for a clean exit would hold the vault forever.
    #[test]
    fn an_abandoned_attachment_still_reads_as_detached() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let attached = attach(&env()).expect("attached");
        let name = attach_slot_name(attached.slot());
        assert!(anyone_attached(&env()), "control: it was attached first");

        // The crash: the `Attachment` is abandoned, so neither its `Drop` nor
        // the guard's runs -- exactly what a killed process does to a Rust
        // value. The kernel is what ends the hold, and the fake stands in for
        // that here.
        std::mem::forget(attached);
        LIVE.lock().unwrap().as_mut().unwrap().remove(&name);

        assert!(
            !anyone_attached(&env()),
            "a crashed app still reads as attached, so the service would hold the vault with \
             nobody using it"
        );
    }

    /// **Asking must not create.** The hazard this module is built around is
    /// a checker that keeps alive what it is checking -- `CreateMutexW` on an
    /// existing name opens a handle to it, and a named object lives as long
    /// as any handle does.
    #[test]
    fn asking_never_makes_a_name_live() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        for _ in 0..5 {
            assert!(!anyone_attached(&env()), "asking made a name live");
        }
        assert!(
            OPENS.load(Ordering::SeqCst) >= 5 * SLOTS,
            "control: the fake kernel was not consulted for every slot"
        );
        // And the names really are unclaimed afterwards -- if asking had
        // created them, this would find no free slot.
        assert!(attach(&env()).is_some());
    }

    /// The space is finite, and running out is a refusal rather than a slot
    /// handed out twice.
    #[test]
    fn a_full_slot_space_refuses_rather_than_overlapping() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let held: Vec<_> = (0..SLOTS).map(|_| attach(&env()).expect("slot")).collect();
        assert!(attach(&env()).is_none());
        drop(held);
        assert!(attach(&env()).is_some());
    }

    // ---- Task 2: proving a running service is ours --------------------

    static STOPS: AtomicUsize = AtomicUsize::new(0);

    fn stop(_: u16) {
        STOPS.fetch_add(1, Ordering::SeqCst);
    }

    const OURS: &str = "aa11";
    const THEIRS: &str = "bb22";

    fn verify_env() -> ServiceEnv {
        STOPS.store(0, Ordering::SeqCst);
        ServiceEnv { hold, is_held, stop }
    }

    /// A service that holds its own object AND is serving this account is
    /// the one we started, or one an earlier launch started. Adopt it: the
    /// alternative costs a cold start and, on the direct backend, another
    /// Windows Hello prompt.
    #[test]
    fn a_service_holding_its_object_for_our_account_is_adopted() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let env = verify_env();
        let _service = (env.hold)(&service_object_name(OURS)).expect("service holds its name");
        assert_eq!(verify(&env, OURS, Some(OURS), 8087), Verdict::Adopt);
        assert_eq!(STOPS.load(Ordering::SeqCst), 0);
    }

    /// The second account on one machine. It is a real service, holding a
    /// real object -- just not ours. Adopting it would hand this app another
    /// user's vault, which is the failure the fingerprint exists to stop.
    #[test]
    fn a_service_serving_another_account_is_refused() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let env = verify_env();
        let _service = (env.hold)(&service_object_name(THEIRS)).expect("their service");
        assert_eq!(
            verify(&env, OURS, Some(THEIRS), 8087),
            Verdict::Refuse(Refusal::ServesAnotherAccount)
        );
    }

    /// **The security-critical one.** Something answered on the port and
    /// said the right thing -- anyone who has read this repository can do
    /// that -- but holds no service object. It is refused, and it is
    /// **not stopped**: killing a process this app cannot identify is worse
    /// than declining to use it.
    #[test]
    fn a_port_that_answers_while_holding_nothing_is_refused_and_not_stopped() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let env = verify_env();
        assert_eq!(
            verify(&env, OURS, Some(OURS), 8087),
            Verdict::Refuse(Refusal::NothingHoldsTheServiceObject)
        );
        assert_eq!(
            STOPS.load(Ordering::SeqCst),
            0,
            "an unverifiable process on the port was killed; this app could not identify it"
        );
    }

    /// Nothing answered at all. Distinct from an impostor, because the
    /// caller acts differently: here it starts a service, there it refuses
    /// to and says why.
    #[test]
    fn a_silent_port_is_refused_as_silent_and_not_as_an_impostor() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        let env = verify_env();
        assert_eq!(verify(&env, OURS, None, 8087), Verdict::Refuse(Refusal::NothingAnswered));
        assert_eq!(STOPS.load(Ordering::SeqCst), 0);
    }

    /// The account check must come from comparing fingerprints, not from a
    /// name that happens not to open. Without this, refusing every unknown
    /// account would look identical to a squatted-name bug.
    #[test]
    fn the_object_name_separates_accounts() {
        assert_ne!(service_object_name(OURS), service_object_name(THEIRS));
        assert!(
            service_object_name(OURS).starts_with("Local\\"),
            "the service object must be logon-session scoped, as app_mutex is"
        );
    }
    // ---- Task 3: start, reconnect, exit -------------------------------

    const PORT: u16 = 8087;
    static STARTS: AtomicUsize = AtomicUsize::new(0);
    /// What the fake service on the port claims to serve.
    static CLAIM: Mutex<Option<String>> = Mutex::new(None);
    /// The service object the fake service holds while it runs. Kept in a
    /// static because `start` is a `fn` pointer and cannot own it.
    static SERVICE: Mutex<Option<Held>> = Mutex::new(None);

    fn probe(_: u16) -> Option<String> {
        CLAIM.lock().ok()?.clone()
    }

    /// Starting the service does the two things a real one does: it holds
    /// its own named object, and it answers on the port.
    fn start(_: u16) -> bool {
        STARTS.fetch_add(1, Ordering::SeqCst);
        *SERVICE.lock().unwrap() = hold(&service_object_name(OURS));
        *CLAIM.lock().unwrap() = Some(OURS.to_string());
        true
    }

    fn take_start_lock() -> Option<Held> {
        hold("Local\\Deskwarden-Vault-Starting")
    }

    static SETTLES: AtomicUsize = AtomicUsize::new(0);

    fn settle() {
        SETTLES.fetch_add(1, Ordering::SeqCst);
    }

    fn start_env() -> StartEnv {
        StartEnv { probe, start, take_start_lock, settle }
    }

    /// Resets the Task 3 fakes on top of [`reset`].
    ///
    /// **Takes no lock.** Its callers already hold [`SERIALISE`], and a
    /// second acquisition here deadlocks -- a `std::sync::Mutex` is not
    /// reentrant. That is not hypothetical: adding the lock mechanically to
    /// every `reset` call did exactly this and hung the suite.
    fn reset_service() {
        reset();
        STARTS.store(0, Ordering::SeqCst);
        STOPS.store(0, Ordering::SeqCst);
        *CLAIM.lock().unwrap() = None;
        *SERVICE.lock().unwrap() = None;
        SETTLES.store(0, Ordering::SeqCst);
    }

    /// Nothing running, so this app starts one.
    #[test]
    fn the_first_app_starts_the_service() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        assert_eq!(ensure_running(&env(), &start_env(), OURS, PORT), Startup::StartedIt);
        assert_eq!(STARTS.load(Ordering::SeqCst), 1);
        assert_eq!(SETTLES.load(Ordering::SeqCst), 0, "the winner waited for itself");
    }

    /// **Reconnect precedes restart, always.** Restarting costs a cold start
    /// and, on the direct backend, another Windows Hello prompt -- so a
    /// service that proves itself must not be replaced.
    #[test]
    fn a_running_service_is_adopted_without_starting_a_second_one() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        start(PORT);
        STARTS.store(0, Ordering::SeqCst);
        assert_eq!(ensure_running(&env(), &start_env(), OURS, PORT), Startup::Adopted);
        assert_eq!(STARTS.load(Ordering::SeqCst), 0, "a second service was started on top of ours");
    }

    /// Two apps launching together must not start two services, and **losing
    /// that race is not an error**: the loser adopts the winner's service.
    ///
    /// The interleaving matters, and the first version of this test got it
    /// wrong. If the winner is already answering when the loser looks, the
    /// loser adopts at the *first* probe and never reaches the race at all --
    /// which is correct, and is what `a_running_service_is_adopted...` above
    /// covers. The losing path needs the harder ordering: the port is silent
    /// when the loser looks, the start lock is gone, and the winner comes up
    /// while the loser is deciding. `probe_winner_arrives_late` is that
    /// timing, not a convenience.
    #[test]
    fn two_concurrent_starts_produce_one_service_and_the_loser_attaches() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        let _lock = take_start_lock().expect("winner holds the start lock, having not yet spawned");

        static PROBES: AtomicUsize = AtomicUsize::new(0);
        fn probe_winner_arrives_late(port: u16) -> Option<String> {
            if PROBES.fetch_add(1, Ordering::SeqCst) == 0 {
                return None; // the loser looks: nothing is up yet
            }
            // Between the two looks, the winner finished starting.
            *SERVICE.lock().unwrap() = hold(&service_object_name(OURS));
            let _ = port;
            Some(OURS.to_string())
        }
        PROBES.store(0, Ordering::SeqCst);

        let late = StartEnv { probe: probe_winner_arrives_late, start, take_start_lock, settle };
        assert_eq!(ensure_running(&env(), &late, OURS, PORT), Startup::AdoptedAfterLosingRace);
        assert_eq!(
            STARTS.load(Ordering::SeqCst),
            0,
            "the loser started a second service instead of attaching to the winner's"
        );
        assert_eq!(
            SETTLES.load(Ordering::SeqCst),
            1,
            "the loser re-probed in the same instant it lost, which finds nothing almost every time"
        );
    }

    /// The race was lost and the winner never got a service up. Reported as
    /// its own outcome rather than as an adoption, because the caller has no
    /// backend and has to say so.
    #[test]
    fn losing_the_race_to_an_app_that_never_starts_one_is_reported() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        let _lock = take_start_lock().expect("winner takes the start lock");
        assert_eq!(
            ensure_running(&env(), &start_env(), OURS, PORT),
            Startup::RaceWonByNobody
        );
    }

    /// A service for a different account is neither adopted nor replaced.
    #[test]
    fn a_service_for_another_account_is_left_alone() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        let _theirs = hold(&service_object_name(THEIRS)).expect("their service");
        *CLAIM.lock().unwrap() = Some(THEIRS.to_string());
        assert_eq!(
            ensure_running(&env(), &start_env(), OURS, PORT),
            Startup::Refused(Refusal::ServesAnotherAccount)
        );
        assert_eq!(STARTS.load(Ordering::SeqCst), 0, "a second service was started on their port");
        assert_eq!(STOPS.load(Ordering::SeqCst), 0, "another account's service was stopped");
    }

    /// The last attachment released exits the service.
    #[test]
    fn releasing_the_last_attachment_stops_the_service() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        let only = attach(&env()).expect("attached");
        release(&env(), only, PORT);
        assert_eq!(STOPS.load(Ordering::SeqCst), 1);
    }

    /// Releasing one of two does not -- the other app is still using it.
    #[test]
    fn releasing_one_of_two_leaves_the_service_running() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        let first = attach(&env()).expect("first");
        let second = attach(&env()).expect("second");
        release(&env(), first, PORT);
        assert_eq!(STOPS.load(Ordering::SeqCst), 0, "the second app lost its vault");
        release(&env(), second, PORT);
        assert_eq!(STOPS.load(Ordering::SeqCst), 1);
    }
    // ---- Task 4: the window that is bounded, not eliminated ------------

    static TICKS: AtomicUsize = AtomicUsize::new(0);

    /// Ten ticks, then the loop is told to give up -- so a test that fails
    /// to stop the service ends rather than hanging.
    fn tick() -> bool {
        TICKS.fetch_add(1, Ordering::SeqCst) < 10
    }

    fn supervisor() -> SupervisorEnv {
        TICKS.store(0, Ordering::SeqCst);
        SupervisorEnv { tick }
    }

    /// **The Task 4 test.** Every app crashed -- attachments abandoned, not
    /// released -- so nothing ran the code that stops the service. A suite
    /// that only drove clean exits would pass while this case leaked an
    /// unlocked vault on a held port forever.
    #[test]
    fn a_service_whose_apps_all_crashed_is_stopped_anyway() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        let first = attach(&env()).expect("first");
        let second = attach(&env()).expect("second");
        let names = [attach_slot_name(first.slot()), attach_slot_name(second.slot())];

        // Both processes are killed: no `Drop` runs anywhere. The kernel is
        // what releases the names, and the fake stands in for it.
        std::mem::forget(first);
        std::mem::forget(second);
        for name in &names {
            LIVE.lock().unwrap().as_mut().unwrap().remove(name);
        }

        assert_eq!(supervise(&env(), &supervisor(), PORT), Supervised::Stopped);
        assert_eq!(STOPS.load(Ordering::SeqCst), 1);
    }

    /// The other half: a supervisor that stops a service somebody is still
    /// using is worse than the leak it was written to prevent.
    #[test]
    fn a_service_with_a_live_app_is_never_stopped_however_long_it_waits() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        let _still_here = attach(&env()).expect("attached");
        assert_eq!(supervise(&env(), &supervisor(), PORT), Supervised::GaveUp);
        assert_eq!(STOPS.load(Ordering::SeqCst), 0, "a vault in use was closed underneath its app");
        assert!(TICKS.load(Ordering::SeqCst) > 1, "control: the supervisor did not actually loop");
    }

    /// One app crashes and the other keeps working. The crash must not take
    /// the survivor down with it.
    #[test]
    fn one_app_crashing_does_not_stop_the_service_for_the_other() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        let crashed = attach(&env()).expect("crashed");
        let _alive = attach(&env()).expect("alive");
        let name = attach_slot_name(crashed.slot());
        std::mem::forget(crashed);
        LIVE.lock().unwrap().as_mut().unwrap().remove(&name);

        assert_eq!(supervise(&env(), &supervisor(), PORT), Supervised::GaveUp);
        assert_eq!(STOPS.load(Ordering::SeqCst), 0);
    }

    /// The service is stopped once, not once per tick.
    #[test]
    fn the_service_is_stopped_only_once() {
        let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        reset_service();
        assert_eq!(supervise(&env(), &supervisor(), PORT), Supervised::Stopped);
        assert_eq!(STOPS.load(Ordering::SeqCst), 1);
    }

    /// The size of the orphan window is a number, and it is stated rather
    /// than implied. If someone changes it, this test makes them say so.
    #[test]
    fn the_orphan_window_is_a_stated_bound() {
        assert_eq!(ORPHAN_CHECK_INTERVAL, std::time::Duration::from_secs(5));
    }
    /// The bug no unit test in this module could have caught, pinned so it
    /// cannot come back.
    ///
    /// `win_is_held` once spelled `SYNCHRONIZE` as a hand-written
    /// `SYNCHRONIZATION_ACCESS_RIGHTS(0x0010)`. That is a meaningless bit --
    /// `SYNCHRONIZE` is a standard right, `0x0010_0000` -- so every
    /// `OpenMutexW` returned `ERROR_ACCESS_DENIED` and this module reported
    /// nothing attached while two real processes held slots. Every test here
    /// passed, because the fake kernel never reaches that call.
    ///
    /// A source pin rather than a behavioural test because the behaviour
    /// needs two processes and a real kernel: that is
    /// `examples/vault_slots_probe.rs`, and this is what stops a future edit
    /// from quietly reintroducing a literal.
    #[test]
    fn the_access_rights_come_from_the_windows_crate_and_are_not_written_by_hand() {
        let source = include_str!("vault_service.rs");
        let cut = source.find("#[cfg(test)]").expect("control: this file has no test module");
        let production = &source[..cut];
        assert!(
            production.contains("SYNCHRONIZATION_SYNCHRONIZE"),
            "control: the production half no longer opens a mutex at all, so this pin is vacuous"
        );
        assert!(
            !production.contains("SYNCHRONIZATION_ACCESS_RIGHTS("),
            "an access right is built from a literal here. Use the constant the windows crate \
             defines: a wrong bit fails at runtime as ACCESS_DENIED, which this module reports \
             as -- nobody is attached."
        );
    }
    // ---- the switch-over: stopping only what we started ----------------

    /// **The refusal that must not be lost in the switch-over.** `verify`
    /// declines to adopt a process it cannot identify AND declines to kill
    /// it. Stopping by port would reintroduce exactly that killing through
    /// the back door: whatever is listening is not necessarily ours.
    #[test]
    fn a_service_we_did_not_start_is_never_stopped() {
        assert_eq!(stop_action(false), StopAction::LeaveItAlone);
    }

    /// The child this process spawned is ours to end, and the handle is what
    /// says so -- not the port, and not a pid we remembered.
    #[test]
    fn the_child_we_started_is_the_one_we_stop() {
        assert_eq!(stop_action(true), StopAction::EndTheChildWeStarted);
    }

    /// Stopping by port is the thing this must never become, and an absence
    /// cannot be read -- so it is pinned in the source.
    #[test]
    fn nothing_here_stops_a_process_by_port() {
        let source = include_str!("vault_service.rs");
        let cut = source.find("#[cfg(test)]").expect("control: this file has no test module");
        let production = &source[..cut];
        assert!(
            production.contains("stop_action("),
            "control: the stop decision no longer runs through `stop_action`, so this pin guards nothing"
        );
        for forbidden in ["TerminateProcess", "OpenProcess", "taskkill", "by_port", "find_pid"] {
            assert!(
                !production.contains(forbidden),
                "`{forbidden}` appears here. The service is stopped through the child handle this process owns; ending whatever holds the port would kill a process this app cannot identify, which `verify` exists to refuse."
            );
        }
    }
}

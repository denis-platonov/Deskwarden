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
//! # The list of names is a hint, not a count
//!
//! Something has to say which names to try. That is the one piece of
//! bookkeeping here, and it is deliberately the kind that degrades safely: a
//! stale entry costs one failed open, and a missing entry costs an app that is
//! attached but unseen -- which is why an entry is written *before* the vault
//! is used and not after.

use std::sync::{Arc, Mutex};

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
    name: String,
    _held: Held,
    register: Arc<Mutex<Vec<String>>>,
}

impl Drop for Attachment {
    fn drop(&mut self) {
        if let Ok(mut names) = self.register.lock() {
            names.retain(|n| n != &self.name);
        }
    }
}

/// The names this machine has been told to watch.
///
/// In-process for now, and that is a real limitation stated rather than
/// hidden: two apps in two processes do not share this `Vec`. Making it
/// cross-process -- a small file, or the registry -- is the next step and is
/// deliberately not taken here, so the decision about *where* it lives is
/// made once, with the service's own start-up, rather than twice.
#[derive(Clone, Default)]
pub struct Register(Arc<Mutex<Vec<String>>>);

impl Register {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Claims the vault under `name`, if the name can be created.
    ///
    /// The name is recorded **before** the caller can use the vault, so a
    /// supervisor that looks in the window between creating and recording
    /// sees an attached app rather than missing one.
    #[must_use]
    pub fn attach(&self, env: &ServiceEnv, name: &str) -> Option<Attachment> {
        let held = (env.hold)(name)?;
        if let Ok(mut names) = self.0.lock() {
            names.push(name.to_string());
        }
        Some(Attachment { name: name.to_string(), _held: held, register: self.0.clone() })
    }

    /// Whether any recorded name still has a live holder.
    ///
    /// Every name is asked about; the first live one is enough, but the loop
    /// does not stop early on a *dead* one -- a dead name is not evidence
    /// about the others.
    #[must_use]
    pub fn anyone_attached(&self, env: &ServiceEnv) -> bool {
        let Ok(names) = self.0.lock() else {
            // A poisoned lock is not evidence that nobody is attached, and
            // answering "no" here would stop the service out from under a
            // running app. The safe answer is the one that keeps the vault
            // available.
            return true;
        };
        names.iter().any(|name| (env.is_held)(name))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // The fake kernel is the two statics below rather than a value: the seam
    // is `fn` pointers, which cannot close over a fixture. `Held` carries a
    // guard whose `Drop` removes the name, so releasing a hold in a test is
    // dropping the value -- and *abandoning* one, the crash case, is
    // `std::mem::forget`.
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

    fn hold(name: &str) -> Option<Held> {
        let mut live = LIVE.lock().ok()?;
        live.as_mut()?.insert(name.to_string());
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
        reset();
        let register = Register::new();
        let a = register.attach(&env(), "app-a").expect("a");
        let b = register.attach(&env(), "app-b").expect("b");
        assert!(register.anyone_attached(&env()));
        drop(a);
        assert!(register.anyone_attached(&env()), "one release ended the vault for both apps");
        drop(b);
        assert!(!register.anyone_attached(&env()));
    }

    /// **The test the design exists for.** An app that dies without releasing
    /// anything must still read as detached: a crash decrements no count, and
    /// a service that waited for a clean exit would hold the vault forever.
    #[test]
    fn an_abandoned_attachment_still_reads_as_detached() {
        reset();
        let register = Register::new();
        let attached = register.attach(&env(), "app-crashes").expect("attached");
        assert!(register.anyone_attached(&env()), "control: it was attached first");

        // The crash: the `Attachment` is abandoned, so neither its `Drop` nor
        // the guard's runs -- exactly what a killed process does to a Rust
        // value. The kernel is what ends the hold, and the fake stands in for
        // that below.
        std::mem::forget(attached);
        LIVE.lock().unwrap().as_mut().unwrap().remove("app-crashes");

        assert!(
            !register.anyone_attached(&env()),
            "a crashed app still reads as attached, so the service would hold the vault with \
             nobody using it"
        );
    }

    /// **Asking must not create.** The hazard this module is built around is
    /// a checker that keeps alive what it is checking -- `CreateMutexW` on an
    /// existing name opens a handle to it, and a named object lives as long
    /// as any handle does.
    ///
    /// Driven against an ABANDONED name rather than a cleanly released one,
    /// because a clean release removes the entry from the register and then
    /// there is nothing left to ask about -- the first version of this test
    /// asked about an empty register and its own control caught it.
    #[test]
    fn asking_never_makes_a_name_live() {
        reset();
        let register = Register::new();
        let attached = register.attach(&env(), "app-a").expect("a");
        std::mem::forget(attached);
        LIVE.lock().unwrap().as_mut().unwrap().remove("app-a");

        for _ in 0..5 {
            assert!(
                !register.anyone_attached(&env()),
                "asking whether anyone is attached made the name live again"
            );
        }
        assert!(
            OPENS.load(Ordering::SeqCst) >= 5,
            "control: the fake kernel was not consulted, so this proves nothing about asking"
        );
    }

    /// A name that cannot be created is not recorded, so a later ask does not
    /// report an app that never attached.
    #[test]
    fn a_name_that_cannot_be_created_attaches_nothing() {
        reset();
        fn never(_: &str) -> Option<Held> {
            None
        }
        let register = Register::new();
        let env = ServiceEnv { hold: never, is_held, stop };
        assert!(register.attach(&env, "app-a").is_none());
        assert!(!register.anyone_attached(&env));
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
}

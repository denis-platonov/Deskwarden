//! The one global shortcut this app registers, and what happens when it
//! cannot be registered.
//!
//! # The crash this module exists to have stopped producing
//!
//! `register_fill_hotkey` used to be four lines, two of which were `expect`,
//! and a real run died on the second one:
//!
//! ```text
//! ERROR deskwarden] panicked: failed to register Ctrl+Alt+B:
//!   AlreadyRegistered(HotKey { mods: Modifiers(ALT | CONTROL), key: KeyB, ... })
//! ```
//!
//! `RegisterHotKey` is first-come-first-served **across the whole logon
//! session**. Any program at all -- a launcher, a game overlay, another
//! password manager, a macro tool -- can be holding `Ctrl+Alt+B` when
//! Deskwarden starts, and there is nothing Deskwarden can do about that and
//! nothing it should do about it. Under `windows_subsystem = "windows"` the
//! panic is invisible: the process exits 101 and the app simply vanishes,
//! taking an unlocked vault's tray, its Preferences and its vault window with
//! it, over a keyboard shortcut.
//!
//! **Nothing about a hotkey justifies killing a password manager the user has
//! unlocked.** So no registration failure is fatal here -- not
//! `AlreadyRegistered`, not a manager that could not be created (`OsError`),
//! not `FailedToRegister`, not a variant this version of `global-hotkey` has
//! not got yet. Every one of them lands in [`HotkeyStatus::Unavailable`], is
//! logged, is shown to the user on Preferences -> Shortcuts, and costs the
//! global shortcut and nothing else. The tray, the overlay, the vault window,
//! the clipboard and autofill from the overlay all work exactly as before
//! without it.
//!
//! # Why a status is reported at all
//!
//! A shortcut that silently does nothing is its own confusing failure: the
//! user presses `Ctrl+Alt+B`, something else on their machine answers (or
//! nothing does), and Deskwarden looks broken with no way to find out why.
//! [`publish`]/[`availability`] carry the answer to the Shortcuts page, which
//! is where a user goes to ask exactly this question. It is deliberately not
//! a startup dialog: this is a degraded convenience, not a failure to start,
//! and a modal at launch over a keyboard shortcut would be worse than the
//! silence it replaces.
//!
//! # Why it is retried
//!
//! The conflict is usually somebody else's *program*, not somebody else's
//! *machine*: the launcher gets closed, the game exits, the other password
//! manager is uninstalled. A status decided once at startup and held for the
//! life of a tray app that runs for days would be wrong for most of that
//! time. So [`retry_if_unavailable`] re-attempts on a fixed interval from the
//! main loop -- see [`RETRY_EVERY`] for the interval and why it is an
//! interval and not "when the vault window closes".

use std::sync::Mutex;
use std::time::{Duration, Instant};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// How often an unavailable hotkey is re-attempted.
///
/// **An interval, rather than an event.** The obvious event-shaped triggers --
/// the vault window closing, Preferences opening -- each cover one moment and
/// leave the others uncovered, and "the other program exited" correlates with
/// none of them: it is a thing that happens while Deskwarden is sitting in the
/// tray doing nothing at all. An interval covers every case with one rule,
/// including both of those events, and it means the Shortcuts page is never
/// more than `RETRY_EVERY` stale whichever of its two shells it is opened
/// from -- which matters because that page cannot be handed a value from
/// `main` (see [`availability`]).
///
/// Thirty seconds is chosen against the cost, which is one `RegisterHotKey`
/// call -- a single non-blocking user32 call on a thread that is already
/// pumping messages -- and against the wait a user will accept between
/// closing the conflicting program and the shortcut starting to work. It is
/// only ever paid while the hotkey is unavailable; an armed hotkey
/// re-attempts nothing, ever.
pub const RETRY_EVERY: Duration = Duration::from_secs(30);

/// Why the global shortcut is not working.
///
/// Deliberately coarse. A user cannot act on the difference between an
/// `OsError` and a `FailedToRegister`, and the exact text of every one of them
/// goes to the log; what a user *can* act on is "something else has these
/// keys", which is [`Self::TakenByAnotherProgram`] and by far the likeliest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// `AlreadyRegistered` -- another program in this logon session got to
    /// `Ctrl+Alt+B` first. The one actually observed in the wild.
    TakenByAnotherProgram,
    /// The hotkey manager itself could not be created, so there was nothing
    /// to register on.
    NoManager,
    /// Registration was refused for some other reason, including any variant
    /// `global-hotkey` adds later -- its error enum is `#[non_exhaustive]`,
    /// and a future variant must degrade like the rest rather than fall off
    /// the end of a `match`.
    Refused,
    /// **Nothing has tried yet.** Not a failure: the reason the shortcut is
    /// not working is that this process has not reached
    /// [`register_fill_hotkey`] -- which it does after the startup vault
    /// window closes, six hundred lines below where that window opens.
    ///
    /// This exists because the Shortcuts page can be reached *before* that
    /// point, from the Preferences modal inside that window, and
    /// [`availability`] used to answer that route with [`HotkeyStatus::Armed`]
    /// -- an assertion that the chord was registered, made before anything had
    /// tried to register it, and wrong on exactly the machine the rest of this
    /// module exists for. [`classify`] never produces it and nothing ever
    /// [`publish`]es it; it is what [`availability`] says when nothing has
    /// been published at all.
    NotYetAttempted,
}

impl Unavailable {
    /// The one line the Shortcuts page shows under the shortcut.
    ///
    /// Actionable where it can be and honest where it cannot: the first names
    /// the thing to do and how long the fix takes to be noticed, the next two
    /// say plainly that the shortcut is off and that nothing else is, because
    /// there is no action to offer. All of them end on the same reassurance,
    /// which is the part that stops a missing shortcut reading as a broken
    /// app.
    ///
    /// [`Unavailable::NotYetAttempted`]'s line is the odd one and is written
    /// to be: it must not blame the machine, another program or Windows for
    /// something none of them has done, because at that moment nothing has
    /// happened at all. It says so, and says the page will have the real
    /// answer shortly -- which it will, because the first attempt is made the
    /// moment the startup window closes and an unavailable one is re-attempted
    /// every [`RETRY_EVERY`] after that.
    pub fn message(self) -> &'static str {
        match self {
            Unavailable::TakenByAnotherProgram => {
                "Another program on this PC is already using CTRL+ALT+B, so Deskwarden could \
                 not claim it. Close that program and Deskwarden will pick the shortcut up \
                 within half a minute. Everything else works normally."
            }
            Unavailable::NoManager => {
                "Windows would not give Deskwarden a keyboard hook, so this shortcut is off \
                 for now. Everything else works normally."
            }
            Unavailable::Refused => {
                "Windows refused this shortcut, so it is off for now. Everything else works \
                 normally; the log has the reason."
            }
            Unavailable::NotYetAttempted => {
                "Deskwarden has not tried to claim CTRL+ALT+B yet, so it is not working at \
                 this moment. It makes the attempt as soon as this window is out of the way, \
                 and this page will then say whether it got the shortcut. Everything else \
                 works normally."
            }
        }
    }
}

/// Whether the global shortcut is working, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyStatus {
    /// Registered: pressing the chord anywhere reaches this process.
    Armed,
    /// Not registered. The app runs on without it.
    Unavailable(Unavailable),
}

/// **The single decision, as a pure function.**
///
/// Every registration attempt in the process funnels through here, so
/// "a registration failure is not fatal" is one statement in one place rather
/// than a rule each call site has to remember -- which is exactly what the
/// two `expect`s this replaced were.
pub fn classify(result: Result<(), &global_hotkey::Error>) -> HotkeyStatus {
    match result {
        Ok(()) => HotkeyStatus::Armed,
        Err(global_hotkey::Error::AlreadyRegistered(_)) => {
            HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram)
        }
        Err(global_hotkey::Error::OsError(_)) => HotkeyStatus::Unavailable(Unavailable::NoManager),
        // Everything else, INCLUDING variants this version of the crate does
        // not have: the enum is `#[non_exhaustive]`, and the whole point of
        // this module is that an unrecognised failure degrades rather than
        // ends the process.
        Err(_) => HotkeyStatus::Unavailable(Unavailable::Refused),
    }
}

/// The chord, built in one place so the id and the registration cannot
/// disagree.
fn fill_chord() -> HotKey {
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyB)
}

/// The outside-world half of a registration attempt, as a **`fn` pointer**.
///
/// The `VaultFrameEnv` idiom, for the reason that one exists: `RegisterHotKey`
/// is a session-wide claim on a key combination, so a test that ran the real
/// attempt would be taking `Ctrl+Alt+B` away from whatever the person running
/// the tests has bound it to -- and would half the time be *testing* whether
/// their machine happened to be free. No test in this crate may register a
/// real hotkey; this is the seam that makes the decisions above reachable
/// without one.
///
/// It yields `Option<GlobalHotKeyManager>` rather than the manager itself so
/// that a substituted attempt can report success without conjuring a manager
/// it has no way to build: `Ok(None)` is "armed, and nothing was registered",
/// which is what a test needs and what production never returns.
pub type RegisterAttempt = fn(HotKey) -> Result<Option<GlobalHotKeyManager>, global_hotkey::Error>;

/// The production attempt: make a manager, register the chord on it.
///
/// Both halves can fail and neither `expect`s. The manager is handed back so
/// the caller can hold it -- dropping a `GlobalHotKeyManager` unregisters
/// everything on it -- and a manager that was built but could not register is
/// dropped here, since keeping a hidden Win32 window alive for a registration
/// that does not exist buys nothing.
fn attempt_registration(
    hotkey: HotKey,
) -> Result<Option<GlobalHotKeyManager>, global_hotkey::Error> {
    let manager = GlobalHotKeyManager::new()?;
    manager.register(hotkey)?;
    Ok(Some(manager))
}

/// The fill hotkey and its current state.
pub struct FillHotkey {
    /// `None` when unavailable, and also when a substituted [`RegisterAttempt`]
    /// reported success without one. Held for the life of the process purely
    /// so its `Drop` unregisters.
    _manager: Option<GlobalHotKeyManager>,
    hotkey: HotKey,
    hotkey_id: u32,
    status: HotkeyStatus,
    /// When the last attempt was made, so [`retry_if_unavailable`] can pace
    /// itself. Set on success too, so the field never has to be an `Option`.
    last_attempt: Instant,
    attempt: RegisterAttempt,
    /// Where a new status is published, as a **`fn` pointer**, for the same
    /// reason [`RegisterAttempt`] is one and one more besides: the real
    /// publisher writes [`STATUS`], which is process-wide, and the tests in
    /// this binary run in parallel -- a test that let its scripted failures
    /// reach it would be telling `prefs_ui`'s painting tests that the machine
    /// they are running on has lost its hotkey.
    publish_to: fn(HotkeyStatus),
}

impl FillHotkey {
    /// Whether the shortcut is actually working.
    pub fn availability(&self) -> HotkeyStatus {
        self.status
    }

    /// The `WM_HOTKEY` id the chord arrives under.
    ///
    /// Derived from the chord itself and so correct whether or not the
    /// registration succeeded -- an unavailable hotkey simply never receives
    /// an event under it.
    pub fn id(&self) -> u32 {
        self.hotkey_id
    }
}

/// Registers the fill hotkey, or reports why it could not be.
///
/// **Never panics and never exits.** See the module docs.
pub fn register_fill_hotkey() -> FillHotkey {
    register_fill_hotkey_with(attempt_registration, announce, Instant::now())
}

/// [`register_fill_hotkey`] with its two seams and its clock supplied.
pub fn register_fill_hotkey_with(
    attempt: RegisterAttempt,
    publish_to: fn(HotkeyStatus),
    now: Instant,
) -> FillHotkey {
    let hotkey = fill_chord();
    let (manager, status) = run_attempt(attempt, hotkey);
    publish_to(status);
    FillHotkey {
        _manager: manager,
        hotkey,
        hotkey_id: hotkey.id(),
        status,
        last_attempt: now,
        attempt,
        publish_to,
    }
}

/// One attempt, classified. The only place either of those happens.
fn run_attempt(
    attempt: RegisterAttempt,
    hotkey: HotKey,
) -> (Option<GlobalHotKeyManager>, HotkeyStatus) {
    match attempt(hotkey) {
        Ok(manager) => (manager, HotkeyStatus::Armed),
        Err(e) => {
            log::warn!(
                "the global shortcut CTRL+ALT+B could not be registered ({e}); Deskwarden is \
                 carrying on without it -- see Preferences > Shortcuts"
            );
            (None, classify(Err(&e)))
        }
    }
}

/// Publishes a status and logs the transition when it is one worth reading.
fn announce(status: HotkeyStatus) {
    let previous = publish(status);
    match (previous, status) {
        // `Some(..)`, so this line means a real earlier failure and not the
        // absence of one. Matching a bare `Unavailable(_)` here is what would
        // have gone wrong the moment `STATUS` stopped being seeded with
        // `Armed`: "nothing published yet" is now an unavailable-shaped answer
        // too, so an ordinary launch where the chord was free would have
        // reported "whatever was holding it has let go" about a conflict that
        // never existed. `None` falls through to the plain line below.
        (Some(HotkeyStatus::Unavailable(_)), HotkeyStatus::Armed) => log::info!(
            "the global shortcut CTRL+ALT+B is registered after all; whatever was holding it \
             has let go"
        ),
        (_, HotkeyStatus::Armed) => log::info!("the global shortcut CTRL+ALT+B is registered"),
        // The failure itself is logged with its error text in `run_attempt`.
        // Repeating it every `RETRY_EVERY` would fill the log of an app that
        // runs for days with one unchanging line, and the log is the thing
        // somebody reads to find out why the app vanished.
        (_, HotkeyStatus::Unavailable(_)) => {}
    }
}

/// Whether an unavailable hotkey is due another attempt.
///
/// Pure, and separated from the attempt for [`classify`]'s reason: the pacing
/// rule is the part worth pinning, and it is the part that would otherwise
/// only be observable by waiting half a minute.
pub fn should_retry(status: HotkeyStatus, since_last_attempt: Duration) -> bool {
    matches!(status, HotkeyStatus::Unavailable(_)) && since_last_attempt >= RETRY_EVERY
}

/// Re-attempts registration if the hotkey is unavailable and enough time has
/// passed. Reports whether an attempt was made.
///
/// Called from the main loop, which is where it has to be called from:
/// `RegisterHotKey` binds to the calling thread and `WM_HOTKEY` is delivered
/// only to that thread's message queue -- the same reason the first attempt
/// happens on the main thread rather than on the window-watch thread.
pub fn retry_if_unavailable(fh: &mut FillHotkey, now: Instant) -> bool {
    if !should_retry(fh.status, now.saturating_duration_since(fh.last_attempt)) {
        return false;
    }
    fh.last_attempt = now;
    let (manager, status) = run_attempt(fh.attempt, fh.hotkey);
    fh.status = status;
    if status == HotkeyStatus::Armed {
        fh._manager = manager;
    }
    (fh.publish_to)(status);
    true
}

/// The process-wide answer to "is the global shortcut working?".
///
/// **A published value rather than a parameter, deliberately.** The page that
/// shows it is `prefs_ui`'s Shortcuts section, and that page has two shells:
/// the tray's own Preferences window (`prefs_ui::run`, called from `main`,
/// where the [`FillHotkey`] is in scope) and the vault window's Preferences
/// modal (`prefs_ui::PrefsState::new`, called from inside
/// `vault_window::build_frame`, where it is not and cannot be without widening
/// `VaultFrameEnv` and its four call sites to carry a value the vault window
/// itself never uses -- see `prefs_ui::ACCOUNT_STATUS` for the same trade-off
/// decided the same way). One writer, in this module, at the one moment the
/// answer changes.
///
/// **`Option`, and `None` until somebody publishes.** It used to hold a bare
/// `HotkeyStatus` seeded with [`HotkeyStatus::Armed`], and that seed was a
/// defect: `register_fill_hotkey` runs *after* `main` opens the startup vault
/// window, that call blocks for the whole life of the window, and the
/// Preferences modal inside it is drawn in that window's own loop -- so the
/// Shortcuts page on that route read the seed and told the user the chord was
/// working before anything had tried to claim it. A default that is a
/// well-formed answer is an assertion nothing has established, which is what
/// [`Unavailable::NotYetAttempted`] and this `None` exist to stop being
/// spellable. Every other published static in this crate is already shaped
/// this way -- `prefs_ui::PUBLISHED_ACCOUNT`, `app::NEVER_APPS`,
/// `clipboard::ARMED`, `single_instance::ON_TAKEOVER` -- and
/// `no_published_status_static_defaults_to_a_claim` in `main.rs` now requires
/// it of all of them.
static STATUS: Mutex<Option<HotkeyStatus>> = Mutex::new(None);

/// Reads the published status.
///
/// Answers "nothing has been published" as
/// [`Unavailable::NotYetAttempted`] rather than as
/// [`HotkeyStatus::Armed`], which makes the one route that can observe it
/// truthful: the Preferences modal inside the startup vault window, which runs
/// entirely before the first registration attempt. The page ghosts the chord
/// and says nothing has tried yet, instead of showing it as working and then
/// being contradicted half a second later by a chord another program is
/// holding.
///
/// A test process observes the same thing, and that is right too: a test
/// registers nothing, so "not registered" is the true answer -- and unlike the
/// old default it says so without blaming a machine it is not running on.
///
/// The poisoned-lock arm answers the same way for the same reason it does not
/// panic below: an unreadable status is not a working shortcut.
pub fn availability() -> HotkeyStatus {
    STATUS
        .lock()
        .ok()
        .and_then(|s| *s)
        .unwrap_or(HotkeyStatus::Unavailable(Unavailable::NotYetAttempted))
}

/// Publishes a status, handing back the one it replaced -- `None` when this is
/// the first thing ever published, which is what tells [`announce`] that an
/// armed hotkey is a first success rather than a recovery.
pub fn publish(status: HotkeyStatus) -> Option<HotkeyStatus> {
    match STATUS.lock() {
        Ok(mut held) => held.replace(status),
        // A poisoned lock here means another thread panicked mid-write of a
        // small `Copy` enum. Publishing a status is not worth turning that
        // into a second panic, in the module whose whole subject is that this
        // feature does not take the process down. Reported as "the previous
        // value was this one", so the caller logs a plain statement of the
        // status rather than a transition it cannot actually vouch for.
        Err(_) => Some(status),
    }
}

/// Drains the global hotkey event channel and reports whether the fill
/// hotkey was pressed. Only `HotKeyState::Pressed` counts -- `global-hotkey`
/// emits a separate `Released` event for every key-up, and without this
/// filter a single Ctrl+Alt+B press would be observed twice (once on the
/// way down, once on the way up), double-firing the fill.
pub fn fill_hotkey_pressed(fh: &FillHotkey) -> bool {
    if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
        return event.id == fh.hotkey_id && event.state == HotKeyState::Pressed;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chord the wild crash was over, as `global-hotkey` reported it.
    fn conflicting_chord() -> HotKey {
        HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyB)
    }

    /// Publishes nowhere. `STATUS` is process-wide and these tests run in
    /// parallel with `prefs_ui`'s painting tests, which read it.
    fn unpublished(_: HotkeyStatus) {}

    /// A substituted attempt that succeeds without registering anything.
    fn succeeds(_: HotKey) -> Result<Option<GlobalHotKeyManager>, global_hotkey::Error> {
        Ok(None)
    }

    /// A substituted attempt that fails the way the reported run failed.
    fn already_registered(hk: HotKey) -> Result<Option<GlobalHotKeyManager>, global_hotkey::Error> {
        Err(global_hotkey::Error::AlreadyRegistered(hk))
    }

    /// The manager itself refusing to exist.
    fn no_manager(_: HotKey) -> Result<Option<GlobalHotKeyManager>, global_hotkey::Error> {
        Err(global_hotkey::Error::OsError(std::io::Error::other("no hook for you")))
    }

    /// Some other refusal.
    fn refused(_: HotKey) -> Result<Option<GlobalHotKeyManager>, global_hotkey::Error> {
        Err(global_hotkey::Error::FailedToRegister("nope".into()))
    }

    /// **The ordinary case, asserted positively.**
    ///
    /// Without this every other test here would pass with the feature
    /// deleted: "does not panic" is satisfied by a function that registers
    /// nothing and reports nothing. So this pins that a successful attempt
    /// really does arm the hotkey, and that the id it will listen on is the
    /// id of `Ctrl+Alt+B` and not of some other chord.
    #[test]
    fn a_successful_registration_is_armed_and_listening_on_the_ctrl_alt_b_id() {
        let fh = register_fill_hotkey_with(succeeds, unpublished, Instant::now());
        assert_eq!(fh.availability(), HotkeyStatus::Armed);
        assert_eq!(
            fh.id(),
            conflicting_chord().id(),
            "the hotkey is armed on an id that is not CTRL+ALT+B's, so every press would \
             arrive under an id `fill_hotkey_pressed` rejects"
        );
        // And an armed hotkey never re-attempts, however long it runs.
        assert!(!should_retry(fh.availability(), Duration::from_secs(60 * 60 * 24)));
    }

    /// **The reported crash, as a decision.** The run that produced
    /// `AlreadyRegistered` must come back as a live `FillHotkey` reporting
    /// why, not as a process that has ended.
    #[test]
    fn already_registered_leaves_a_live_app_that_knows_why() {
        let fh = register_fill_hotkey_with(already_registered, unpublished, Instant::now());
        assert_eq!(
            fh.availability(),
            HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram),
            "the case that killed a real run is not classified as a conflict, so the user is \
             told the wrong thing about the one failure they can act on"
        );
        // The chord is still known, so a later retry re-attempts the right
        // one and a press that arrives after it succeeds is still recognised.
        assert_eq!(fh.id(), conflicting_chord().id());
    }

    /// **Every other way registration can fail is non-fatal too**, including
    /// a variant this crate version has not got: `global_hotkey::Error` is
    /// `#[non_exhaustive]`, and a `match` that stopped covering the rest
    /// would be the same defect in a new coat.
    #[test]
    fn no_registration_failure_is_fatal() {
        for (attempt, expected) in [
            (no_manager as RegisterAttempt, Unavailable::NoManager),
            (refused as RegisterAttempt, Unavailable::Refused),
        ] {
            let fh = register_fill_hotkey_with(attempt, unpublished, Instant::now());
            assert_eq!(fh.availability(), HotkeyStatus::Unavailable(expected));
        }
        // And each of them says something to the user rather than nothing.
        for reason in [
            Unavailable::TakenByAnotherProgram,
            Unavailable::NoManager,
            Unavailable::Refused,
            Unavailable::NotYetAttempted,
        ] {
            let message = reason.message();
            assert!(
                message.contains("Everything else works normally"),
                "{reason:?} tells the user the shortcut is off without telling them the rest \
                 of the app is not: {message:?}"
            );
        }
    }

    /// The pacing rule, in both directions.
    #[test]
    fn only_an_unavailable_hotkey_retries_and_only_after_the_interval() {
        let taken = HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram);
        assert!(!should_retry(taken, RETRY_EVERY - Duration::from_millis(1)));
        assert!(should_retry(taken, RETRY_EVERY));
        assert!(!should_retry(HotkeyStatus::Armed, RETRY_EVERY * 100));
    }

    /// **A conflict that goes away is picked up**, which is the whole reason
    /// the status is not decided once at startup.
    ///
    /// The clock is supplied rather than waited out: a test that slept
    /// [`RETRY_EVERY`] would add half a minute to every run to observe a
    /// comparison.
    #[test]
    fn a_hotkey_that_was_taken_arms_itself_once_the_other_program_lets_go() {
        let start = Instant::now();
        let mut fh = register_fill_hotkey_with(already_registered, unpublished, start);
        assert_eq!(fh.availability(), HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram));

        // Too soon: nothing is attempted and nothing changes.
        assert!(!retry_if_unavailable(&mut fh, start + Duration::from_secs(5)));
        assert_eq!(fh.availability(), HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram));

        // The conflicting program exits. The next due attempt arms it.
        fh.attempt = succeeds;
        assert!(retry_if_unavailable(&mut fh, start + RETRY_EVERY));
        assert_eq!(
            fh.availability(),
            HotkeyStatus::Armed,
            "the retry ran and succeeded but the hotkey is still reported as unavailable, so \
             the user is told to close a program they have already closed"
        );
        // ... and having armed, it stops re-attempting for good.
        assert!(!retry_if_unavailable(&mut fh, start + RETRY_EVERY * 10));
    }

    /// A retry that fails again leaves the reason it failed *this* time, not
    /// a stale one.
    #[test]
    fn a_retry_that_fails_again_reports_the_new_reason() {
        let start = Instant::now();
        let mut fh = register_fill_hotkey_with(already_registered, unpublished, start);
        fh.attempt = no_manager;
        assert!(retry_if_unavailable(&mut fh, start + RETRY_EVERY));
        assert_eq!(fh.availability(), HotkeyStatus::Unavailable(Unavailable::NoManager));
    }

    /// `classify` is the one decision, so it is pinned directly as well --
    /// including the `Ok` arm, which is what makes "armed" mean anything.
    #[test]
    fn classify_maps_every_outcome_to_a_survivable_one() {
        assert_eq!(classify(Ok(())), HotkeyStatus::Armed);
        let already = global_hotkey::Error::AlreadyRegistered(conflicting_chord());
        assert_eq!(
            classify(Err(&already)),
            HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram)
        );
        let os = global_hotkey::Error::OsError(std::io::Error::other("x"));
        assert_eq!(classify(Err(&os)), HotkeyStatus::Unavailable(Unavailable::NoManager));
        let other = global_hotkey::Error::FailedToWatchMediaKeyEvent;
        assert_eq!(classify(Err(&other)), HotkeyStatus::Unavailable(Unavailable::Refused));
    }

    /// **Before anything has tried, the page says nothing has tried -- not
    /// that the shortcut works.**
    ///
    /// This is the defect this variant was added for. `register_fill_hotkey`
    /// runs after `main` opens the startup vault window; that call blocks for
    /// the window's whole life and the Preferences modal inside it is drawn in
    /// that window's own loop, so Preferences > Shortcuts on that route is
    /// reached before a single `RegisterHotKey` call has been made. The
    /// default was [`HotkeyStatus::Armed`], so the page told those users the
    /// chord was working -- and if the attempt then failed, which is the one
    /// case this whole module exists for, the page had already said otherwise.
    ///
    /// It is asserted rather than round-tripped through [`publish`] on
    /// purpose: this is process-wide state that `prefs_ui`'s painting tests
    /// read, and the tests in this binary run in parallel, so a test that set
    /// it would be setting it for whatever else was painting a Shortcuts page
    /// at that instant. The write side is covered where it cannot race --
    /// `register_fill_hotkey_with` publishes through `announce` on every path
    /// above, and the *reading* of a published value is
    /// `prefs_ui::draw_section`'s one line.
    #[test]
    fn a_process_that_has_not_tried_yet_says_so_rather_than_claiming_the_shortcut_works() {
        assert_eq!(
            availability(),
            HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
            "the published default is a claim again: the Shortcuts page inside the startup \
             vault window reads this before anything has called RegisterHotKey, so this is \
             what it would show the user"
        );
    }

    /// **...and it says it without blaming anything.**
    ///
    /// The other three reasons name a cause -- another program, a refused
    /// hook, a refused chord -- because by the time they are published there
    /// is one. Here there is not: nothing has happened yet. A line that
    /// borrowed one of theirs would send a user hunting for a conflicting
    /// program half a second before Deskwarden claimed the chord without
    /// trouble, which is a worse failure than the silence it replaced.
    #[test]
    fn the_not_yet_attempted_line_blames_nothing_and_promises_an_answer() {
        let message = Unavailable::NotYetAttempted.message();
        assert!(
            !message.contains("Another program") && !message.contains("refused"),
            "the not-yet-attempted line blames a cause that does not exist yet: {message:?}"
        );
        assert!(
            message.contains("has not tried"),
            "the not-yet-attempted line does not say that nothing has tried yet, which is the \
             only fact there is at that moment: {message:?}"
        );
        assert!(
            message.contains("CTRL+ALT+B"),
            "the line names no chord, so a user cannot tell which shortcut it is about: \
             {message:?}"
        );
        // And it is distinct from every other line, so the page cannot show
        // this state wearing another one's words.
        for other in [
            Unavailable::TakenByAnotherProgram,
            Unavailable::NoManager,
            Unavailable::Refused,
        ] {
            assert_ne!(other.message(), message, "{other:?} and NotYetAttempted read the same");
        }
    }

    /// **`NotYetAttempted` is never a *decided* status.**
    ///
    /// It is what [`availability`] synthesises for "nothing published", and
    /// nothing else may produce it: a registration that came back as
    /// "not attempted" would be a `FillHotkey` that reported it had never run
    /// the attempt it had just run.
    #[test]
    fn no_attempt_can_ever_report_that_it_was_not_attempted() {
        let not_attempted = HotkeyStatus::Unavailable(Unavailable::NotYetAttempted);
        for attempt in [succeeds as RegisterAttempt, already_registered, no_manager, refused] {
            let fh = register_fill_hotkey_with(attempt, unpublished, Instant::now());
            assert_ne!(fh.availability(), not_attempted);
        }
        assert_ne!(classify(Ok(())), not_attempted);
        let already = global_hotkey::Error::AlreadyRegistered(conflicting_chord());
        assert_ne!(classify(Err(&already)), not_attempted);
        // It does, however, retry like any other unavailable state, so a
        // status that somehow reached the main loop would go and find out
        // rather than sit there being not-yet-attempted forever.
        assert!(should_retry(not_attempted, RETRY_EVERY));
    }
}

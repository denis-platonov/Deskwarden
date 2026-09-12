//! The five global shortcuts this app registers, and what happens when one
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
//! logged, is shown to the user on Preferences -> Shortcuts, and costs that
//! one global shortcut and nothing else. The tray, the overlay, the vault
//! window, the clipboard and autofill from the overlay all work exactly as
//! before without it.
//!
//! # Five bindings, and why that changes nothing above and one thing below
//!
//! There is no longer one chord. There are five, all remappable from
//! Preferences -> Shortcuts: the picker chord that has always been
//! `CTRL+ALT+B`, and four that type one thing at the item Deskwarden has
//! already matched -- see [`crate::app::FillShortcut`], which is where the
//! routing from a chord to a [`crate::app::FillChoice`] is written down.
//!
//! Everything above applies to each of them **separately**, and that
//! separation is the whole of what this module had to grow. A `RegisterHotKey`
//! refusal is per-chord: the user's machine can perfectly well have
//! `CTRL+ALT+P` taken by a screenshot tool and the other four free, and a pass
//! that gave up on the first refusal would cost them four working shortcuts to
//! report one broken one. So [`register_all`] attempts every binding
//! regardless of what the ones before it did, each gets its own
//! [`HotkeyStatus`], each is published under its own [`crate::app::FillShortcut`],
//! and [`retry_if_unavailable`] re-attempts exactly the ones that failed.
//!
//! **One manager, five registrations.** `GlobalHotKeyManager::new` creates a
//! hidden `WS_EX_TOOLWINDOW` window bound to the calling thread, and
//! `foreground` and `job_object` both count this process's windows; five
//! managers would be five helper windows for no gain, since one manager
//! registers as many chords as it is asked to. The manager is threaded through
//! the [`RegisterAttempt`] seam rather than created inside it so that the five
//! attempts share one, and is dropped again when the pass armed nothing --
//! keeping a hidden Win32 window alive for zero registrations buys nothing,
//! which is the reasoning the single-chord version applied to its own manager.
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
//! **This is the second half of the conflict story, and it is the half the
//! capture widget cannot do.** Preferences refuses a chord it can see is
//! unusable at the moment it is pressed -- no modifier, or already on another
//! of these five rows. It cannot ask Windows, because the Preferences page is
//! also drawn inside the vault window, which is a *different process* from the
//! daemon holding these registrations: a trial `RegisterHotKey` there would
//! collide with Deskwarden's own claim and report every working chord as
//! taken. So a chord lost to another program is discovered where it can only
//! be discovered -- at registration, in the daemon -- and surfaces as a live
//! `Unavailable` on that chord's row, with the reason. Nothing is ever
//! silently rewritten by either half.
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
//!
//! # Why the retry is quiet
//!
//! The retry is not the log. An attempt twice a minute that wrote a line each
//! time put 3,416 copies of one sentence into `deskwarden.log` on a machine
//! where another program simply owns `Ctrl+Alt+S`, which is roughly 2,880 a
//! day and enough to bury the problems somebody opens that file to find. The
//! attempts still happen -- the same log records the chord being claimed later
//! on, which only happened because something kept trying -- but the sentence
//! is written when the *state* changes rather than when an attempt is made:
//! first failure, a failure for a new reason, the recovery, and one reminder
//! an hour while nothing moves. See [`REMIND_EVERY`] and
//! [`should_log_failure`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::app::FillShortcut;

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
/// call per *unavailable* binding -- a single non-blocking user32 call on a
/// thread that is already pumping messages -- and against the wait a user will
/// accept between closing the conflicting program and the shortcut starting to
/// work. It is only ever paid while a binding is unavailable; an armed hotkey
/// re-attempts nothing, ever, and neither does a cleared one.
pub const RETRY_EVERY: Duration = Duration::from_secs(30);

/// How long an unchanged failure stays quiet in the log before it is worth
/// one more line.
///
/// **The retry is right; logging it was not.** `deskwarden.log` carried 3,416
/// copies of one sentence -- `CTRL+ALT+S ... could not be registered (HotKey
/// already registerd)` -- because [`register_all`] wrote a `warn!` on every
/// attempt and [`RETRY_EVERY`] makes an attempt twice a minute. That is about
/// 2,880 lines a day for a machine where something else simply owns that
/// chord, and it buried real problems during two days of debugging this app:
/// a log nobody can read is a log that is not there.
///
/// The retry itself is untouched, because it works and it matters -- the same
/// log records the chord being registered later, which only happened because
/// something kept trying. What changes is that the *sentence* is written when
/// it is news: the first failure, a failure for a different reason than the
/// last one, the recovery (`announce`'s line), and then one reminder an hour
/// so that "this has been broken all afternoon" is still answerable from the
/// log without reconstructing it from silence.
///
/// **An hour, and at `warn!` rather than `debug!`.** Quieter than that and a
/// long-running instance could go a whole session with one line near the top
/// about a shortcut that has been dead since; noisier and it is the same
/// defect with a smaller multiplier. An hour turns 2,880 lines a day into 24
/// while leaving the condition visible to somebody grepping for warnings,
/// which is what the first report was actually made of.
pub const REMIND_EVERY: Duration = Duration::from_secs(60 * 60);

// ---------------------------------------------------------------------------
// A chord
// ---------------------------------------------------------------------------

/// One key combination, as the user chose it.
///
/// **A type of this crate's own rather than `global_hotkey::hotkey::HotKey`**,
/// which is what actually gets registered. Three reasons, and none of them is
/// taste:
///
/// * `HotKey` carries a precomputed `id` field, so two `HotKey`s that describe
///   the same combination compare equal only because that id is derived -- a
///   thing to rely on rather than a thing to state. A `Chord` is the two
///   things a user picked and nothing else.
/// * `HotKey`'s own `Display`/`FromStr` spell the key as `KeyB` and the
///   modifiers as `control+alt`, which is `global-hotkey`'s wire format and
///   not text to show a person or to write into `settings.json`.
/// * It keeps the persisted form of a preference out of a third-party crate's
///   hands. `settings.json` is read by builds years apart; the spelling of a
///   chord in it is this app's promise, not a dependency's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    /// Only `CONTROL`, `ALT`, `SHIFT` and `SUPER` are ever set -- see
    /// [`Chord::new`], which drops everything else rather than persisting a
    /// modifier `RegisterHotKey` has no concept of.
    pub mods: Modifiers,
    pub code: Code,
}

/// The modifiers a chord may carry: the four `RegisterHotKey` understands.
///
/// `Modifiers::META` is deliberately absent even though `keyboard-types` has
/// it: `HotKey::new` silently rewrites `META` to `SUPER`, so a chord that
/// stored `META` would round-trip through registration as a *different* chord
/// than the one on disk, and the row would name a combination that is not the
/// one listening.
const REGISTRABLE_MODIFIERS: Modifiers = Modifiers::from_bits_truncate(
    Modifiers::CONTROL.bits() | Modifiers::ALT.bits() | Modifiers::SHIFT.bits()
        | Modifiers::SUPER.bits(),
);

/// Every key this app will bind, with the word the user sees for it.
///
/// **A closed table, in both directions.** It is what [`Chord::parse`] reads
/// and what [`Chord`]'s `Display` writes, so a key that is not here cannot be
/// captured, cannot be persisted, and cannot come back off disk -- which is
/// the property that makes the round trip total rather than merely usually
/// right. `Code` has around two hundred variants, most of them keys no
/// keyboard in front of a user has (`BrowserFavorites`, `LaunchMail`,
/// `Lang3`), and a table that admitted them would let a chord be stored that
/// nothing can press.
///
/// **`Escape` is not here on purpose.** It is the capture widget's cancel key
/// (see `prefs_ui::capture_chord`), so a user pressing it to back out of a
/// capture must never find they have bound it instead.
///
/// **Neither are the arrow keys' and function keys' graphics-driver
/// neighbours.** They *are* here -- `F1`-`F12` and the four arrows -- because
/// refusing a key on the grounds that some driver might want it would be this
/// table deciding what is on the user's machine. What that hazard actually
/// gets is a *default* that avoids them; see `settings::Shortcuts::default`.
const KEYS: &[(Code, &str)] = &[
    (Code::KeyA, "A"),
    (Code::KeyB, "B"),
    (Code::KeyC, "C"),
    (Code::KeyD, "D"),
    (Code::KeyE, "E"),
    (Code::KeyF, "F"),
    (Code::KeyG, "G"),
    (Code::KeyH, "H"),
    (Code::KeyI, "I"),
    (Code::KeyJ, "J"),
    (Code::KeyK, "K"),
    (Code::KeyL, "L"),
    (Code::KeyM, "M"),
    (Code::KeyN, "N"),
    (Code::KeyO, "O"),
    (Code::KeyP, "P"),
    (Code::KeyQ, "Q"),
    (Code::KeyR, "R"),
    (Code::KeyS, "S"),
    (Code::KeyT, "T"),
    (Code::KeyU, "U"),
    (Code::KeyV, "V"),
    (Code::KeyW, "W"),
    (Code::KeyX, "X"),
    (Code::KeyY, "Y"),
    (Code::KeyZ, "Z"),
    (Code::Digit0, "0"),
    (Code::Digit1, "1"),
    (Code::Digit2, "2"),
    (Code::Digit3, "3"),
    (Code::Digit4, "4"),
    (Code::Digit5, "5"),
    (Code::Digit6, "6"),
    (Code::Digit7, "7"),
    (Code::Digit8, "8"),
    (Code::Digit9, "9"),
    (Code::F1, "F1"),
    (Code::F2, "F2"),
    (Code::F3, "F3"),
    (Code::F4, "F4"),
    (Code::F5, "F5"),
    (Code::F6, "F6"),
    (Code::F7, "F7"),
    (Code::F8, "F8"),
    (Code::F9, "F9"),
    (Code::F10, "F10"),
    (Code::F11, "F11"),
    (Code::F12, "F12"),
    (Code::Space, "SPACE"),
    (Code::Enter, "ENTER"),
    (Code::Tab, "TAB"),
    (Code::Backspace, "BACKSPACE"),
    (Code::Delete, "DELETE"),
    (Code::Insert, "INSERT"),
    (Code::Home, "HOME"),
    (Code::End, "END"),
    (Code::PageUp, "PAGEUP"),
    (Code::PageDown, "PAGEDOWN"),
    (Code::ArrowUp, "UP"),
    (Code::ArrowDown, "DOWN"),
    (Code::ArrowLeft, "LEFT"),
    (Code::ArrowRight, "RIGHT"),
    (Code::Minus, "-"),
    (Code::Equal, "="),
    (Code::BracketLeft, "["),
    (Code::BracketRight, "]"),
    (Code::Backslash, "\\"),
    (Code::Semicolon, ";"),
    (Code::Quote, "'"),
    (Code::Comma, ","),
    (Code::Period, "."),
    (Code::Slash, "/"),
    (Code::Backquote, "`"),
];

/// The word for a key, or `None` for one this app will not bind.
pub fn key_label(code: Code) -> Option<&'static str> {
    KEYS.iter().find(|(c, _)| *c == code).map(|(_, label)| *label)
}

/// The key a word names, or `None`. Case-insensitive, because a hand-edited
/// `settings.json` will say `ctrl+alt+b` as often as `CTRL+ALT+B`.
pub fn key_from_label(label: &str) -> Option<Code> {
    KEYS.iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(label))
        .map(|(code, _)| *code)
}

impl Chord {
    /// A chord with only the modifiers `RegisterHotKey` understands.
    ///
    /// Truncating rather than refusing: the callers are a capture widget
    /// reading egui's modifier set (which has `command`, a Mac alias) and a
    /// parser reading a file, and neither has anything useful to do with a
    /// modifier this platform cannot claim. Dropping it here means the chord
    /// that is stored is exactly the chord that is registered.
    pub fn new(mods: Modifiers, code: Code) -> Self {
        Self { mods: mods & REGISTRABLE_MODIFIERS, code }
    }

    /// The thing `RegisterHotKey` is actually given.
    pub fn to_hotkey(self) -> HotKey {
        HotKey::new(Some(self.mods), self.code)
    }

    /// The `WM_HOTKEY` id this chord's presses arrive under.
    ///
    /// Derived from the chord itself and so correct whether or not the
    /// registration succeeded -- an unavailable binding simply never receives
    /// an event under it.
    pub fn id(self) -> u32 {
        self.to_hotkey().id()
    }

    /// Whether this chord carries at least one modifier.
    ///
    /// A bare key is a global claim on that key for the whole logon session:
    /// bind `P` and every `P` typed into every program on the machine comes
    /// here instead. `RegisterHotKey` will happily do it, which is exactly why
    /// the refusal has to be ours -- see `prefs_ui::capture_chord`.
    pub fn has_a_modifier(self) -> bool {
        !self.mods.is_empty()
    }

    /// The chord a piece of text names, or `None` if it names none.
    ///
    /// Total in the direction that matters: anything this returns `Some` for
    /// prints back as text this returns the same `Some` for
    /// (`a_chord_survives_the_round_trip_through_its_own_text`). Unknown
    /// modifier words, unknown keys, an empty string, two keys, no key: all
    /// `None`, and every caller of this treats `None` as "use the default"
    /// rather than as "unbound" -- see `settings::Shortcuts`.
    pub fn parse(text: &str) -> Option<Chord> {
        let mut mods = Modifiers::empty();
        let mut code = None;
        for token in text.split('+') {
            let token = token.trim();
            if token.is_empty() {
                return None;
            }
            let as_modifier = match token.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => Some(Modifiers::CONTROL),
                "ALT" => Some(Modifiers::ALT),
                "SHIFT" => Some(Modifiers::SHIFT),
                "WIN" | "SUPER" | "META" => Some(Modifiers::SUPER),
                _ => None,
            };
            match as_modifier {
                Some(m) => {
                    // A repeated modifier is a malformed chord, not a
                    // harmless one: `CTRL+CTRL+B` was written by something
                    // that does not know what it wrote.
                    if mods.contains(m) {
                        return None;
                    }
                    // Modifiers first, exactly as the text prints -- a second
                    // key or a modifier after the key is refused rather than
                    // quietly reordered.
                    if code.is_some() {
                        return None;
                    }
                    mods |= m;
                }
                None => {
                    if code.is_some() {
                        return None;
                    }
                    code = Some(key_from_label(token)?);
                }
            }
        }
        Some(Chord { mods, code: code? })
    }
}

/// The chord as the Shortcuts page shows it and as `settings.json` stores it:
/// `CTRL+ALT+B`.
///
/// **Modifiers in a fixed order**, and not the order they were pressed in:
/// `CTRL+ALT+B` and `ALT+CTRL+B` are the same claim on the same keyboard, and
/// two spellings of one chord is two rows that look different and collide.
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, word) in [
            (Modifiers::CONTROL, "CTRL"),
            (Modifiers::ALT, "ALT"),
            (Modifiers::SHIFT, "SHIFT"),
            (Modifiers::SUPER, "WIN"),
        ] {
            if self.mods.contains(flag) {
                write!(f, "{word}+")?;
            }
        }
        // The `unwrap_or` is unreachable through `parse` and the capture
        // widget, both of which only ever build a `Chord` from `KEYS`. It is
        // here rather than an `expect` because a panic while *painting the
        // Preferences window* is the failure this whole module is about not
        // having, one subsystem over.
        f.write_str(key_label(self.code).unwrap_or("?"))
    }
}

impl FromStr for Chord {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Chord::parse(text).ok_or(())
    }
}

// ---------------------------------------------------------------------------
// Whether a binding is working
// ---------------------------------------------------------------------------

/// Why a global shortcut is not working.
///
/// Deliberately coarse. A user cannot act on the difference between an
/// `OsError` and a `FailedToRegister`, and the exact text of every one of them
/// goes to the log; what a user *can* act on is "something else has these
/// keys", which is [`Self::TakenByAnotherProgram`] and by far the likeliest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// `AlreadyRegistered` -- another program in this logon session got to
    /// this chord first. The one actually observed in the wild.
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
    /// [`register_fill_hotkeys`] -- which it does after the startup vault
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
    /// The one line the Shortcuts page shows under a shortcut that is off.
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
    ///
    /// **The chord is an argument, and a `String` comes back.** It used to be
    /// `&'static str` with `CTRL+ALT+B` written into three of the four
    /// sentences, which was true while there was one chord and it could not be
    /// changed. There are five now and every one of them is remappable, so a
    /// line that named a hardcoded chord would tell a user who had rebound the
    /// password shortcut to go and look for a conflict on a combination they
    /// no longer use.
    pub fn message(self, chord: &str) -> String {
        match self {
            Unavailable::TakenByAnotherProgram => format!(
                "Another program on this PC is already using {chord}, so Deskwarden could not \
                 claim it. Close that program, or pick a different shortcut here, and \
                 Deskwarden will pick it up within half a minute. Everything else works \
                 normally."
            ),
            Unavailable::NoManager => format!(
                "Windows would not give Deskwarden a keyboard hook, so {chord} is off for now. \
                 Everything else works normally."
            ),
            Unavailable::Refused => format!(
                "Windows refused {chord}, so it is off for now. Everything else works normally; \
                 the log has the reason."
            ),
            Unavailable::NotYetAttempted => format!(
                "Deskwarden has not tried to claim {chord} yet, so it is not working at this \
                 moment. It makes the attempt as soon as this window is out of the way, and \
                 this page will then say whether it got the shortcut. Everything else works \
                 normally."
            ),
        }
    }
}

/// Whether a global shortcut is working, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyStatus {
    /// Registered: pressing the chord anywhere reaches this process.
    Armed,
    /// Not registered. The app runs on without it.
    Unavailable(Unavailable),
    /// **The user cleared this row**, so there is no chord to register and
    /// nothing is wrong.
    ///
    /// A third state rather than an [`Unavailable`] reason, because the two
    /// are opposite kinds of fact and the page treats them as such: an
    /// unavailable shortcut is a thing the user wanted that they have not got,
    /// and is drawn as a warning with a reason and a retry behind it; an
    /// unbound one is a thing they said they did not want, and a warning about
    /// it would be the app arguing with a decision it just carried out.
    /// [`should_retry`] says no to it forever, which is the same distinction
    /// spelled as behaviour.
    Unbound,
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

// ---------------------------------------------------------------------------
// Registering them
// ---------------------------------------------------------------------------

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
/// **The manager goes in and comes back out**, which is what lets five
/// registrations share one hidden Win32 window (see the module docs) while
/// still being five separately-classified attempts. `Option` on the way in is
/// "nothing has made one yet"; `Option` on the way out lets a substituted
/// attempt report success without conjuring a manager it has no way to build,
/// which is what a test needs and what production never does.
///
/// A manager that was built and then failed to register is handed **back**,
/// not dropped: by the time a later chord fails, earlier ones may be live on
/// it, and dropping a `GlobalHotKeyManager` unregisters everything on it.
/// [`register_all`] drops it if the whole pass armed nothing.
pub type RegisterAttempt = fn(
    Option<GlobalHotKeyManager>,
    HotKey,
) -> (Option<GlobalHotKeyManager>, Result<(), global_hotkey::Error>);

/// The production attempt: make a manager if there is not one, register the
/// chord on it.
///
/// Both halves can fail and neither `expect`s.
fn attempt_registration(
    held: Option<GlobalHotKeyManager>,
    hotkey: HotKey,
) -> (Option<GlobalHotKeyManager>, Result<(), global_hotkey::Error>) {
    let manager = match held {
        Some(manager) => manager,
        None => match GlobalHotKeyManager::new() {
            Ok(manager) => manager,
            // No manager and nothing registered. Reported per-chord, so the
            // next binding in the pass tries to make one again rather than
            // inheriting this one's verdict -- a transient refusal of a
            // keyboard hook must not cost four shortcuts on the strength of
            // having cost one.
            Err(e) => return (None, Err(e)),
        },
    };
    let outcome = manager.register(hotkey);
    (Some(manager), outcome)
}

/// One binding: the chord the user chose, and how it is getting on.
struct Binding {
    /// `None` is *cleared by the user*, not *failed*. See
    /// [`HotkeyStatus::Unbound`].
    chord: Option<Chord>,
    /// The `WM_HOTKEY` id presses arrive under, or `None` when there is no
    /// chord to arrive under.
    ///
    /// **An `Option` and not a sentinel `0`.** `0` was tried and is wrong:
    /// `HotKey::id` is `mods.bits() << 16 | key as u32`, and
    /// `keyboard_types::Code`'s first variant is `Backquote` with discriminant
    /// zero -- so a bare backtick, which [`KEYS`] does allow, has id 0. A
    /// cleared row holding `0` would therefore answer to every press of that
    /// chord. `a_cleared_row_answers_to_no_press_at_all` is what caught it.
    id: Option<u32>,
    status: HotkeyStatus,
    /// When this binding's current trouble was last written to the log, or
    /// `None` while there is nothing to be quiet about.
    ///
    /// Cleared the moment the binding stops being unavailable -- see
    /// [`register_all`] -- so that a chord which works, is lost again and
    /// fails again is reported again rather than swallowed by an hour of
    /// quiet earned by the previous outage. See [`REMIND_EVERY`].
    complained_at: Option<Instant>,
}

/// The five fill hotkeys and their current states.
pub struct FillHotkeys {
    /// `None` when nothing is armed, and also when a substituted
    /// [`RegisterAttempt`] reported success without one. Held for the life of
    /// the process purely so its `Drop` unregisters.
    manager: Option<GlobalHotKeyManager>,
    bindings: [Binding; FillShortcut::COUNT],
    /// When the last pass was made, so [`retry_if_unavailable`] can pace
    /// itself. Set on success too, so the field never has to be an `Option`.
    last_attempt: Instant,
    attempt: RegisterAttempt,
    /// Where a new status is published, as a **`fn` pointer**, for the same
    /// reason [`RegisterAttempt`] is one and one more besides: the real
    /// publisher writes [`STATUS`], which is process-wide, and the tests in
    /// this binary run in parallel -- a test that let its scripted failures
    /// reach it would be telling `prefs_ui`'s painting tests that the machine
    /// they are running on has lost its hotkeys.
    publish_to: fn(FillShortcut, Option<Chord>, HotkeyStatus),
}

impl FillHotkeys {
    /// Whether one shortcut is actually working.
    pub fn availability(&self, which: FillShortcut) -> HotkeyStatus {
        self.bindings[which.index()].status
    }

    /// The chord one shortcut is bound to, or `None` if it is cleared.
    pub fn chord(&self, which: FillShortcut) -> Option<Chord> {
        self.bindings[which.index()].chord
    }

    /// The `WM_HOTKEY` id one shortcut's presses arrive under, or `None` for a
    /// cleared row -- which has no chord and so no id to arrive under.
    pub fn id(&self, which: FillShortcut) -> Option<u32> {
        self.bindings[which.index()].id
    }
}

/// Registers the fill hotkeys, or reports which of them could not be.
///
/// **Never panics and never exits.** See the module docs.
pub fn register_fill_hotkeys(chords: [Option<Chord>; FillShortcut::COUNT]) -> FillHotkeys {
    register_fill_hotkeys_with(chords, attempt_registration, announce, Instant::now())
}

/// [`register_fill_hotkeys`] with its two seams and its clock supplied.
pub fn register_fill_hotkeys_with(
    chords: [Option<Chord>; FillShortcut::COUNT],
    attempt: RegisterAttempt,
    publish_to: fn(FillShortcut, Option<Chord>, HotkeyStatus),
    now: Instant,
) -> FillHotkeys {
    let mut fh = FillHotkeys {
        manager: None,
        bindings: chords.map(|chord| Binding {
            chord,
            id: chord.map(Chord::id),
            // Overwritten by `register_all` below for every one of them. The
            // seed is the honest one rather than `Armed`, for the reason
            // `STATUS` starts at `None`: a well-formed answer nothing has
            // established is the defect, not the placeholder.
            status: HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
            complained_at: None,
        }),
        last_attempt: now,
        attempt,
        publish_to,
    };
    register_all(&mut fh, now, |_| true);
    fh
}

/// One pass over the bindings `wanted` selects, each attempted and classified
/// independently, then published.
///
/// **`wanted` is what makes a retry a retry**: the first pass takes every
/// binding, a retry takes only the unavailable ones, and neither needs its own
/// copy of the attempt-classify-publish sequence.
fn register_all(fh: &mut FillHotkeys, now: Instant, wanted: impl Fn(HotkeyStatus) -> bool) {
    for index in 0..FillHotkeys::COUNT {
        let which = FillShortcut::ALL[index];
        if !wanted(fh.bindings[index].status) {
            continue;
        }
        let previous = fh.bindings[index].status;
        let status = match fh.bindings[index].chord {
            // Nothing to register, and nothing wrong. Published like any
            // other status so the page has an answer for this row rather than
            // whatever the row held before it was cleared.
            None => HotkeyStatus::Unbound,
            Some(chord) => {
                let (manager, outcome) = (fh.attempt)(fh.manager.take(), chord.to_hotkey());
                fh.manager = manager;
                let status = classify(outcome.as_ref().map(|_| ()).map_err(|e| e));
                if let Err(e) = &outcome {
                    // **Written when it is news, not when it happens.** See
                    // [`REMIND_EVERY`]: the attempt repeats every
                    // [`RETRY_EVERY`] and must, but the sentence about it does
                    // not. The elapsed time is carried on the reminder because
                    // "still, an hour later" is the whole of what the second
                    // line adds over the first.
                    let quiet_for = fh.bindings[index]
                        .complained_at
                        .map(|at| now.saturating_duration_since(at));
                    if should_log_failure(previous, status, quiet_for) {
                        match quiet_for {
                            None => log::warn!(
                                "the global shortcut {chord} ({}) could not be registered ({e}); \
                                 Deskwarden is carrying on without it -- see Preferences > \
                                 Shortcuts",
                                which.label()
                            ),
                            Some(quiet) => log::warn!(
                                "the global shortcut {chord} ({}) still could not be registered \
                                 after {} minutes of retrying ({e}); Deskwarden is carrying on \
                                 without it -- see Preferences > Shortcuts",
                                which.label(),
                                quiet.as_secs() / 60
                            ),
                        }
                        fh.bindings[index].complained_at = Some(now);
                    }
                }
                status
            }
        };
        // An armed or cleared row has nothing to be quiet about, so the next
        // failure -- whenever it comes -- is news again.
        if !matches!(status, HotkeyStatus::Unavailable(_)) {
            fh.bindings[index].complained_at = None;
        }
        fh.bindings[index].status = status;
        (fh.publish_to)(which, fh.bindings[index].chord, status);
    }
    // **A manager holding nothing is a hidden window holding nothing.** The
    // single-chord version dropped a manager it could not register on for
    // exactly this reason, and the reason survives the move to five: it is
    // only ever right to keep one while at least one chord is live on it,
    // because dropping it unregisters every chord that is. A later retry makes
    // a new one.
    if !fh.bindings.iter().any(|b| b.status == HotkeyStatus::Armed) {
        fh.manager = None;
    }
}

impl FillHotkeys {
    /// Spelled here as well as on [`FillShortcut`] so that `register_all`'s
    /// loop bound and the array it indexes cannot come from two places.
    const COUNT: usize = FillShortcut::COUNT;
}

/// Publishes a status and logs the transition when it is one worth reading.
fn announce(which: FillShortcut, chord: Option<Chord>, status: HotkeyStatus) {
    let previous = publish(which, chord, status);
    let label = which.label();
    match (previous, status) {
        // `Some(..)`, so this line means a real earlier failure and not the
        // absence of one. Matching a bare `Unavailable(_)` here is what would
        // have gone wrong the moment `STATUS` stopped being seeded with
        // `Armed`: "nothing published yet" is now an unavailable-shaped answer
        // too, so an ordinary launch where the chord was free would have
        // reported "whatever was holding it has let go" about a conflict that
        // never existed. `None` falls through to the plain line below.
        (Some(HotkeyStatus::Unavailable(_)), HotkeyStatus::Armed) => log::info!(
            "the global shortcut for {label} is registered after all; whatever was holding it \
             has let go"
        ),
        (_, HotkeyStatus::Armed) => log::info!("the global shortcut for {label} is registered"),
        // The failure itself is logged with its error text in `register_all`,
        // which paces it -- see `REMIND_EVERY`. Saying it a second time from
        // here would fill the log of an app that runs for days with one
        // unchanging line per unavailable chord, and the log is the thing
        // somebody reads to find out why the app vanished.
        (_, HotkeyStatus::Unavailable(_)) => {}
        (_, HotkeyStatus::Unbound) => log::info!("the global shortcut for {label} is cleared"),
    }
}

/// Whether a failed registration is worth a line in the log this time.
///
/// Pure, and separated from the logging for [`should_retry`]'s reason: the
/// pacing rule is the part worth pinning, and a rule about what happens after
/// an hour of half-minute retries is otherwise only observable by waiting an
/// hour. `previous` is the binding's status **before** this attempt and `next`
/// the one it just produced; `quiet_for` is how long since this binding's
/// trouble was last written about, or `None` if it has not been.
///
/// The three things that are news, in the order they are asked:
///
/// * Nothing has been said yet -- the first failure after a launch, a rebind,
///   or a recovery. Always logged, which is what keeps "the shortcut is not
///   working" in the log at all.
/// * The reason changed -- *taken by another program* becoming *refused*, say.
///   A different failure is a different fact even while the symptom is the
///   same, and the two have different answers.
/// * It has been unchanged for [`REMIND_EVERY`]. One line an hour, so that a
///   log covering a long session still says the condition persisted rather
///   than leaving a reader to infer it from a gap.
///
/// Everything else is the same sentence the log already carries, and is
/// dropped.
pub fn should_log_failure(
    previous: HotkeyStatus,
    next: HotkeyStatus,
    quiet_for: Option<Duration>,
) -> bool {
    match quiet_for {
        None => true,
        Some(quiet) => previous != next || quiet >= REMIND_EVERY,
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

/// Re-attempts registration for every binding that is unavailable, if enough
/// time has passed. Reports whether a pass was made.
///
/// Called from the main loop, which is where it has to be called from:
/// `RegisterHotKey` binds to the calling thread and `WM_HOTKEY` is delivered
/// only to that thread's message queue -- the same reason the first attempt
/// happens on the main thread rather than on the window-watch thread.
///
/// **The interval is per-pass and not per-binding**, so five unavailable
/// chords cost five `RegisterHotKey` calls every [`RETRY_EVERY`] rather than
/// one every six seconds in rotation. An armed binding is not re-attempted at
/// all, so the ordinary machine -- where all five are armed -- pays nothing,
/// exactly as it did with one.
pub fn retry_if_unavailable(fh: &mut FillHotkeys, now: Instant) -> bool {
    let since = now.saturating_duration_since(fh.last_attempt);
    if !fh.bindings.iter().any(|b| should_retry(b.status, since)) {
        return false;
    }
    fh.last_attempt = now;
    register_all(fh, now, |status| {
        matches!(status, HotkeyStatus::Unavailable(_))
    });
    true
}

/// Re-registers everything if the user has changed a chord, and reports
/// whether it did.
///
/// **Called from the main loop rather than from the two places that write
/// settings**, and that is the point. Preferences is edited from three shells
/// -- the tray's own window, the vault window's modal, and whatever comes next
/// -- and each writes `estate.settings` through `apply_edited_settings`. A
/// rebind hung off those call sites would be a rule each of them had to
/// remember; one reconciliation against the settings the daemon actually holds
/// covers all of them and cannot be forgotten by a fourth.
///
/// **Everything is torn down and rebuilt, not just the row that changed.** Two
/// chords can *swap* -- the user gives the password shortcut the combination
/// the username shortcut had, then gives the username shortcut something else
/// -- and a per-row rebind would try to register the password chord while the
/// username row was still holding it and report a conflict with ourselves. The
/// pass is a handful of user32 calls made only when a chord actually changed.
pub fn rebind_if_changed(
    fh: &mut FillHotkeys,
    chords: [Option<Chord>; FillShortcut::COUNT],
    now: Instant,
) -> bool {
    if (0..FillHotkeys::COUNT).all(|i| fh.bindings[i].chord == chords[i]) {
        return false;
    }
    // Dropping the manager unregisters every chord on it, which is what makes
    // the *old* combinations stop answering. Without this the machine would
    // accumulate claims: rebind five times and the first four chords are all
    // still registered to this process and all still firing.
    fh.manager = None;
    for (index, chord) in chords.into_iter().enumerate() {
        fh.bindings[index] = Binding {
            chord,
            id: chord.map(Chord::id),
            status: HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
            // A new chord is a new situation: whatever quiet the old one had
            // earned is not this one's to inherit, so a rebind onto a chord
            // that is also taken says so at once. See [`REMIND_EVERY`].
            complained_at: None,
        };
    }
    fh.last_attempt = now;
    register_all(fh, now, |_| true);
    true
}

// ---------------------------------------------------------------------------
// Publishing the answer
// ---------------------------------------------------------------------------

/// The process-wide answer to "is this global shortcut working?".
///
/// **A published value rather than a parameter, deliberately.** The page that
/// shows it is `prefs_ui`'s Shortcuts section, and that page has two shells:
/// the tray's own Preferences window (`prefs_ui::run`, called from `main`,
/// where the [`FillHotkeys`] is in scope) and the vault window's Preferences
/// modal (`prefs_ui::PrefsState::new`, called from inside
/// `vault_window::build_frame`, where it is not and cannot be without widening
/// `VaultFrameEnv` and its four call sites to carry a value the vault window
/// itself never uses -- see `prefs_ui::ACCOUNT_STATUS` for the same trade-off
/// decided the same way). One writer, in this module, at the one moment an
/// answer changes.
///
/// **`Option`, and `None` until somebody publishes.** It used to hold a bare
/// `HotkeyStatus` seeded with [`HotkeyStatus::Armed`], and that seed was a
/// defect: registration runs *after* `main` opens the startup vault window,
/// that call blocks for the whole life of the window, and the Preferences
/// modal inside it is drawn in that window's own loop -- so the Shortcuts page
/// on that route read the seed and told the user the chord was working before
/// anything had tried to claim it. A default that is a well-formed answer is
/// an assertion nothing has established, which is what
/// [`Unavailable::NotYetAttempted`] and this `None` exist to stop being
/// spellable. Every other published static in this crate is already shaped
/// this way -- `prefs_ui::PUBLISHED_ACCOUNT`, `app::NEVER_APPS`,
/// `clipboard::ARMED`, `single_instance::ON_TAKEOVER` -- and
/// `no_published_status_static_defaults_to_a_claim` in `main.rs` now requires
/// it of all of them.
///
/// **One cell for five answers, rather than five cells.** They are written in
/// one pass by one thread and read in one pass by one page, so an array behind
/// one lock is both the smaller surface and the one that cannot be half
/// updated as far as a reader is concerned.
///
/// **And the chord travels with the status, in the same cell.** The tray's
/// *Fill: CTRL+ALT+B* label and the first window's footnote both have to name
/// the chord, and both used to spell it out as a constant because it could not
/// change. It can now. Two statics -- one for the status, one for the chord --
/// would be two things a reader could find disagreeing; one entry per shortcut
/// cannot be half right.
static STATUS: Mutex<Option<[Published; FillShortcut::COUNT]>> = Mutex::new(None);

/// One shortcut, as the rest of the process may read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Published {
    chord: Option<Chord>,
    status: HotkeyStatus,
}

// ---------------------------------------------------------------------------
// Across the process boundary
// ---------------------------------------------------------------------------

/// The file the daemon writes its shortcut status into, in the config
/// directory, for a UI process to read.
///
/// # Why this exists at all
///
/// [`STATUS`] is process-wide, and the two processes that care about it are
/// not the same one. **The daemon registers every global shortcut** -- one
/// call, `main`'s `register_fill_hotkeys`, on the thread that pumps the
/// message queue `WM_HOTKEY` is delivered to -- and **the vault window draws
/// the Shortcuts page**, in `deskwarden.exe --ui vault`, a process that
/// registers nothing and never will.
///
/// So that page read a `STATUS` that was empty by construction and rendered
/// the empty answer honestly: *"Deskwarden has not tried to claim CTRL+ALT+B
/// yet."* Every row, every time, on a machine where the log two lines up said
/// `the global shortcut for Fill the focused app is registered`. The sentence
/// was written for the STARTUP window, which runs inside the daemon before
/// the registration line and where it is true; it became permanent and wrong
/// the moment the window moved into a process of its own.
///
/// # Why a file, and this file
///
/// It is the idiom this boundary already uses in the other direction --
/// `ui_process`'s result, edited-settings and sign-in files, all small files
/// in the config directory. Nothing here is secret (a chord and whether
/// Windows granted it), so the "no secrets on a command line" rule that
/// shapes the outbound half does not bind, and a command line would be stale
/// the moment a retry changed an answer anyway.
///
/// **Not keyed on a pid, unlike every `ui-*.json` beside it.** Those travel
/// child -> daemon and there may be several children; this travels daemon ->
/// child and there is exactly one daemon. A pid in the name would be a
/// filename the reader cannot construct.
const STATUS_FILE_NAME: &str = "daemon-shortcuts.txt";

/// Where [`STATUS_FILE_NAME`] lives.
pub fn status_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(STATUS_FILE_NAME)
}

/// This shortcut's name in the status file.
///
/// A `match` rather than a derive or an index, so a sixth shortcut is a
/// compile error here. Positional lines were the alternative and were
/// rejected: a file written by an older build, or truncated by a full disk,
/// would then assign each answer to the wrong row silently, which is a worse
/// failure than not reading it at all.
fn file_key(which: FillShortcut) -> &'static str {
    match which {
        FillShortcut::Picker => "picker",
        FillShortcut::Username => "username",
        FillShortcut::Password => "password",
        FillShortcut::Totp => "totp",
        FillShortcut::Sequence => "sequence",
    }
}

/// This status's word in the status file, and back.
///
/// Short stable words rather than `{:?}` output: a `Debug` rendering is a
/// thing a rename changes without anybody noticing, and this pair is a format
/// two builds of this app can meet over -- a daemon left running across an
/// upgrade is exactly the case.
fn status_word(status: HotkeyStatus) -> &'static str {
    match status {
        HotkeyStatus::Armed => "armed",
        HotkeyStatus::Unbound => "unbound",
        HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram) => "taken",
        HotkeyStatus::Unavailable(Unavailable::NoManager) => "no-manager",
        HotkeyStatus::Unavailable(Unavailable::Refused) => "refused",
        HotkeyStatus::Unavailable(Unavailable::NotYetAttempted) => "not-yet",
    }
}

/// [`status_word`] read back. An unknown word answers `None`, and the caller
/// drops that line rather than the file -- a word a newer daemon writes must
/// cost one row, not five.
fn status_from_word(word: &str) -> Option<HotkeyStatus> {
    Some(match word {
        "armed" => HotkeyStatus::Armed,
        "unbound" => HotkeyStatus::Unbound,
        "taken" => HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram),
        "no-manager" => HotkeyStatus::Unavailable(Unavailable::NoManager),
        "refused" => HotkeyStatus::Unavailable(Unavailable::Refused),
        "not-yet" => HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
        _ => return None,
    })
}

/// What this process would write into the status file: one line per
/// shortcut, `key<TAB>chord<TAB>status`, with an empty chord field for a row
/// that has none.
///
/// Tab-separated text rather than JSON because [`Chord`] has no `serde`
/// implementation and deliberately so -- see its doc, which keeps the
/// persisted spelling of a chord out of a third-party crate's hands. The
/// spelling here is [`Chord::to_string`] and [`Chord::parse`], the same pair
/// `settings.json` already round-trips through, so this file cannot come to
/// disagree with that one about what `CTRL+ALT+B` means.
///
/// Answers `None` when nothing has been published, which is the daemon before
/// its first registration pass. Writing "not-yet" five times then would put a
/// claim in a file for a reader to adopt, and the reader's own default
/// already says exactly that.
pub fn status_report() -> Option<String> {
    Some(encode_status(&STATUS.lock().ok().and_then(|held| *held)?))
}

/// [`status_report`]'s encoding half, against a value rather than the static.
///
/// Split out for [`parse_status_report`]'s reason and the same one:
/// [`STATUS`] is process-wide and this crate's tests share a process, so a
/// test that drove the real static would be a test that changed what every
/// other test in the run observes.
fn encode_status(all: &[Published; FillShortcut::COUNT]) -> String {
    let mut out = String::new();
    for which in FillShortcut::ALL {
        let entry = all[which.index()];
        let chord = entry.chord.map(|c| c.to_string()).unwrap_or_default();
        out.push_str(file_key(which));
        out.push('\t');
        out.push_str(&chord);
        out.push('\t');
        out.push_str(status_word(entry.status));
        out.push('\n');
    }
    out
}

/// Writes [`status_report`] into the config directory, for a UI process to
/// adopt.
///
/// **Called by the daemon and only by the daemon.** A failure is logged and
/// otherwise ignored: the cost is a Shortcuts page that says nothing has been
/// attempted, which is what it said before this file existed, and it is not
/// worth failing a launch over.
pub fn publish_status_file(config_dir: &Path) {
    let Some(report) = status_report() else { return };
    // **Written only when it has changed**, because the caller is the
    // daemon's main loop and that loop turns many times a second. Five
    // shortcuts that are all armed produce the same forty bytes forever, and
    // writing them at the loop's rate would be a disk write per iteration for
    // a value that changes on a rebind, a retry that succeeded, or never.
    //
    // The remembered copy is per process and starts empty, so the first
    // publish of a run always writes -- which is what makes a file left by a
    // previous run get replaced rather than trusted.
    static LAST_WRITTEN: Mutex<Option<String>> = Mutex::new(None);
    if let Ok(mut last) = LAST_WRITTEN.lock() {
        if last.as_deref() == Some(report.as_str()) {
            return;
        }
        *last = Some(report.clone());
    }
    let path = status_file_path(config_dir);
    if let Err(e) = std::fs::write(&path, report) {
        log::debug!(
            "could not write the shortcut status to {} ({e}); a vault window opened from here \
             will say the shortcuts have not been attempted",
            path.display()
        );
    }
}

/// Removes the status file.
///
/// The daemon calls this on its way out, because the answers in it are only
/// true while the process that registered those chords is alive. A file left
/// behind would tell the next UI process that shortcuts are armed in a
/// process that has exited.
pub fn forget_status_file(config_dir: &Path) {
    let path = status_file_path(config_dir);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::debug!("could not remove {} ({e})", path.display());
        }
    }
}

/// Reads the daemon's status file into this process's [`STATUS`].
///
/// **Called by a UI process and only by a UI process.** Calling it in the
/// daemon would overwrite what that process actually knows with a copy of
/// what it wrote a moment ago, and -- on the startup window, which runs
/// inside the daemon *before* the first registration pass -- it would replace
/// an honest "nothing has been attempted" with a stale answer from the
/// previous run.
///
/// Adopts wholesale and announces nothing: [`announce`]'s log lines are the
/// registering process's to write, and a second copy of them from every
/// window that happens to be open would say the shortcut had just been
/// claimed each time.
///
/// A missing file, an unreadable one, or a line this build does not
/// understand each cost exactly what they should -- that row keeps whatever
/// this process already had, which for a UI process is the honest default.
pub fn adopt_status_file(config_dir: &Path) {
    let path = status_file_path(config_dir);
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    adopt_status_report(&text);
}

/// [`adopt_status_file`]'s parsing half, against text rather than a path, so
/// the format can be driven from a test without a config directory.
pub fn adopt_status_report(text: &str) {
    let parsed = parse_status_report(text);
    if parsed.is_empty() {
        return;
    }
    if let Ok(mut held) = STATUS.lock() {
        let all = held.get_or_insert(
            [Published {
                chord: None,
                status: HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
            }; FillShortcut::COUNT],
        );
        for (which, entry) in parsed {
            all[which.index()] = entry;
        }
    }
}

/// [`adopt_status_report`]'s reading half, answering the rows it understood.
///
/// **Pure, and that is why it is separate.** [`STATUS`] is process-wide and
/// this crate's tests share one process, so a test driving the real static
/// would change what every other test in the run observes -- the same reason
/// `register_all`'s tests publish nowhere. Every rule about what a malformed
/// file costs is in here and is asserted directly.
fn parse_status_report(text: &str) -> Vec<(FillShortcut, Published)> {
    let mut parsed: Vec<(FillShortcut, Published)> = Vec::new();
    for line in text.lines() {
        // `splitn(3, ..)`, so a chord that ever grew a tab could not silently
        // become a status word. It cannot today -- `Chord::to_string` writes
        // `+`-joined names -- and this is the cheap way for that to stay
        // true.
        let mut fields = line.splitn(3, '\t');
        let (Some(key), Some(chord), Some(word)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Some(which) = FillShortcut::ALL.into_iter().find(|w| file_key(*w) == key) else {
            continue;
        };
        let Some(status) = status_from_word(word.trim()) else { continue };
        // An empty field is a row with no chord; a non-empty one this build
        // cannot parse is dropped WITH its row, because a status shown
        // against the wrong chord is worse than no status.
        let chord = if chord.is_empty() {
            None
        } else {
            match Chord::parse(chord) {
                Some(chord) => Some(chord),
                None => continue,
            }
        };
        parsed.push((which, Published { chord, status }));
    }
    parsed
}

/// What the tray and the first window call this shortcut's chord.
///
/// **`&str`-shaped answer for a cleared row**, rather than an `Option` every
/// caller has to decide about: the two callers are both writing a sentence,
/// and *not set* is the phrase that belongs in one. A row that has never been
/// published reads as its own default is NOT assumed here -- nothing has been
/// published, so there is nothing to name, and the sentence those callers
/// write for `NotYetAttempted` does not need one.
pub fn published_chord_text(which: FillShortcut) -> String {
    published_chord(which).map(|c| c.to_string()).unwrap_or_else(|| "not set".to_string())
}

/// The chord one shortcut was last registered (or cleared) on, or `None`
/// before anything has been published for it.
pub fn published_chord(which: FillShortcut) -> Option<Chord> {
    STATUS.lock().ok().and_then(|s| *s).and_then(|all| all[which.index()].chord)
}

/// Reads the published status of one shortcut.
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
pub fn availability(which: FillShortcut) -> HotkeyStatus {
    STATUS
        .lock()
        .ok()
        .and_then(|s| *s)
        .map(|all| all[which.index()].status)
        .unwrap_or(HotkeyStatus::Unavailable(Unavailable::NotYetAttempted))
}

/// Publishes one shortcut's chord and status, handing back the status it
/// replaced -- `None` when nothing had ever been published for it, which is
/// what tells [`announce`] that an armed hotkey is a first success rather than
/// a recovery.
pub fn publish(
    which: FillShortcut,
    chord: Option<Chord>,
    status: HotkeyStatus,
) -> Option<HotkeyStatus> {
    match STATUS.lock() {
        Ok(mut held) => {
            let previous = held.map(|all| all[which.index()].status);
            // Seeded with the honest placeholder rather than with `status`
            // repeated five times: publishing the picker's answer must not
            // also assert the other four, which have not been attempted yet
            // at the moment the first publish of a pass happens. The chord is
            // seeded as `None` for the same reason -- naming a chord for a row
            // nothing has looked at yet is the same kind of claim.
            let all = held.get_or_insert(
                [Published {
                    chord: None,
                    status: HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
                }; FillShortcut::COUNT],
            );
            all[which.index()] = Published { chord, status };
            previous
        }
        // A poisoned lock here means another thread panicked mid-write of a
        // small `Copy` array. Publishing a status is not worth turning that
        // into a second panic, in the module whose whole subject is that this
        // feature does not take the process down. Reported as "the previous
        // value was this one", so the caller logs a plain statement of the
        // status rather than a transition it cannot actually vouch for.
        Err(_) => Some(status),
    }
}

/// Drains the global hotkey event channel and reports **which** fill shortcut
/// was pressed.
///
/// Only `HotKeyState::Pressed` counts -- `global-hotkey` emits a separate
/// `Released` event for every key-up, and without this filter a single press
/// would be observed twice (once on the way down, once on the way up),
/// double-firing the fill.
///
/// **The id is looked up rather than compared against one**, which is the
/// whole of what this had to become: five bindings arrive on one channel under
/// five ids, and the caller needs to know which action was asked for, not
/// merely that something was. A press under an id no binding holds -- a chord
/// this process registered before a rebind and Windows has not finished
/// forgetting -- answers `None`, exactly as a press of somebody else's hotkey
/// would.
pub fn fill_hotkey_pressed(fh: &FillHotkeys) -> Option<FillShortcut> {
    let event = GlobalHotKeyEvent::receiver().try_recv().ok()?;
    if event.state != HotKeyState::Pressed {
        return None;
    }
    // `Some(id) == Some(event.id)`: a cleared row's `None` matches nothing at
    // all, which is what makes "a shortcut the user turned off can never claim
    // a press" a property of the type rather than of a sentinel value that
    // turned out to be reachable. See [`Binding::id`].
    FillShortcut::ALL.into_iter().find(|which| fh.id(*which) == Some(event.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chord the wild crash was over, as `global-hotkey` reported it.
    fn conflicting_chord() -> Chord {
        Chord::new(Modifiers::CONTROL | Modifiers::ALT, Code::KeyB)
    }

    /// The five chords this crate ships with, as the settings default has
    /// them. Read from `settings` rather than restated, so these tests cannot
    /// pass on a set of chords the app does not actually use.
    fn shipped() -> [Option<Chord>; FillShortcut::COUNT] {
        crate::settings::Shortcuts::default().as_chords()
    }

    /// Publishes nowhere. `STATUS` is process-wide and these tests run in
    /// parallel with `prefs_ui`'s painting tests, which read it.
    fn unpublished(_: FillShortcut, _: Option<Chord>, _: HotkeyStatus) {}

    /// A substituted attempt that succeeds without registering anything.
    fn succeeds(
        held: Option<GlobalHotKeyManager>,
        _: HotKey,
    ) -> (Option<GlobalHotKeyManager>, Result<(), global_hotkey::Error>) {
        (held, Ok(()))
    }

    /// A substituted attempt that fails the way the reported run failed.
    fn already_registered(
        held: Option<GlobalHotKeyManager>,
        hk: HotKey,
    ) -> (Option<GlobalHotKeyManager>, Result<(), global_hotkey::Error>) {
        (held, Err(global_hotkey::Error::AlreadyRegistered(hk)))
    }

    /// The manager itself refusing to exist.
    fn no_manager(
        _: Option<GlobalHotKeyManager>,
        _: HotKey,
    ) -> (Option<GlobalHotKeyManager>, Result<(), global_hotkey::Error>) {
        (None, Err(global_hotkey::Error::OsError(std::io::Error::other("no hook for you"))))
    }

    /// Some other refusal.
    fn refused(
        held: Option<GlobalHotKeyManager>,
        _: HotKey,
    ) -> (Option<GlobalHotKeyManager>, Result<(), global_hotkey::Error>) {
        (held, Err(global_hotkey::Error::FailedToRegister("nope".into())))
    }

    /// **One chord fails and the other four do not.**
    ///
    /// The seam is a `fn` pointer with no captured state, so "fail exactly
    /// `CTRL+ALT+P`" has to be written as a function that inspects the chord
    /// it is handed -- which is the honest shape anyway, since that is what
    /// the real `RegisterHotKey` does.
    fn only_the_password_chord_is_taken(
        held: Option<GlobalHotKeyManager>,
        hk: HotKey,
    ) -> (Option<GlobalHotKeyManager>, Result<(), global_hotkey::Error>) {
        if hk.key == Code::KeyP {
            (held, Err(global_hotkey::Error::AlreadyRegistered(hk)))
        } else {
            (held, Ok(()))
        }
    }

    // -- the chord itself --------------------------------------------------

    /// **The persisted spelling is total in the direction that matters.**
    ///
    /// Every key this app will bind, in every combination of the four
    /// modifiers it will bind, prints to text that parses back to the same
    /// chord. Without this, a chord could be captured, written to
    /// `settings.json`, and read back on the next launch as a *different*
    /// combination -- which is the one failure a preference must not have,
    /// because the user cannot see it happen.
    #[test]
    fn a_chord_survives_the_round_trip_through_its_own_text() {
        let mut checked = 0usize;
        for (code, _) in KEYS {
            for bits in 0..16u8 {
                let mut mods = Modifiers::empty();
                for (bit, flag) in [
                    (1, Modifiers::CONTROL),
                    (2, Modifiers::ALT),
                    (4, Modifiers::SHIFT),
                    (8, Modifiers::SUPER),
                ] {
                    if bits & bit != 0 {
                        mods |= flag;
                    }
                }
                let chord = Chord::new(mods, *code);
                let text = chord.to_string();
                assert_eq!(
                    Chord::parse(&text),
                    Some(chord),
                    "{chord:?} prints as {text:?}, which does not read back as itself -- a \
                     settings.json written by this build would be read wrong by it"
                );
                checked += 1;
            }
        }
        assert_eq!(
            checked,
            KEYS.len() * 16,
            "control: the sweep stopped covering every key in every modifier combination"
        );
    }

    /// **`CTRL+ALT+B` is still `CTRL+ALT+B`.**
    ///
    /// The one spelling that already existed in this app's UI (`prefs_ui`'s
    /// `FILL_HOTKEY`) and in three of `Unavailable`'s sentences. A `Display`
    /// that produced `Control+Alt+KeyB` would be correct by the round-trip
    /// test above and wrong on every screen.
    #[test]
    fn the_picker_chord_prints_the_way_the_app_has_always_written_it() {
        assert_eq!(conflicting_chord().to_string(), "CTRL+ALT+B");
        assert_eq!(Chord::parse("CTRL+ALT+B"), Some(conflicting_chord()));
        // Order-insensitive on the way in, canonical on the way out: a
        // hand-edited file may say either, and both are the same claim on the
        // same keyboard.
        assert_eq!(Chord::parse("ALT+CTRL+B"), Some(conflicting_chord()));
        assert_eq!(Chord::parse("ctrl+alt+b"), Some(conflicting_chord()));
    }

    /// Text that names no chord is refused rather than guessed at.
    #[test]
    fn malformed_chord_text_is_refused() {
        for text in [
            "",
            "+",
            "CTRL+",
            "+B",
            "CTRL+ALT",       // modifiers and no key
            "B+CTRL",         // key before a modifier
            "CTRL+B+C",       // two keys
            "CTRL+CTRL+B",    // a repeated modifier
            "CTRL+ALT+ESCAPE", // deliberately not in KEYS
            "HYPER+B",
            "CTRL+ALT+BrowserBack",
        ] {
            assert_eq!(Chord::parse(text), None, "{text:?} was read as a chord");
        }
    }

    /// A bare key is representable and is *known* to be one, which is what the
    /// capture widget refuses on.
    #[test]
    fn a_chord_knows_whether_it_has_a_modifier() {
        assert!(!Chord::new(Modifiers::empty(), Code::KeyP).has_a_modifier());
        assert!(conflicting_chord().has_a_modifier());
        // `parse` will produce one, deliberately: the refusal belongs to the
        // capture widget and to nothing else, so that a `settings.json` a user
        // hand-edited into a bare key is read faithfully and then reported.
        assert_eq!(Chord::parse("P"), Some(Chord::new(Modifiers::empty(), Code::KeyP)));
    }

    /// **A cleared row answers to no press at all**, including a press of the
    /// one chord whose id is zero.
    ///
    /// [`Binding::id`] used to be a `u32` with `0` meaning "cleared", on the
    /// stated reasoning that no chord this app can build has id 0. That
    /// reasoning was wrong: `HotKey::id` is `mods.bits() << 16 | key as u32`
    /// and `keyboard_types::Code`'s first variant is `Backquote`, so a bare
    /// backtick -- which [`KEYS`] allows -- has id exactly 0, and every press
    /// of it would have been delivered to whichever shortcut the user had
    /// turned off. The field is an `Option` now; this pins both halves of why.
    #[test]
    fn a_cleared_row_answers_to_no_press_at_all() {
        // The chord that broke the old sentinel, still reachable and still 0.
        assert_eq!(
            Chord::new(Modifiers::empty(), Code::Backquote).id(),
            0,
            "control: the id that made a `0` sentinel unsafe is no longer 0, so this test is \
             no longer about the hazard it was written for"
        );
        let mut chords = shipped();
        chords[FillShortcut::Sequence.index()] = None;
        let fh = register_fill_hotkeys_with(chords, succeeds, unpublished, Instant::now());
        assert_eq!(
            fh.id(FillShortcut::Sequence),
            None,
            "a cleared row holds an id, so some press somewhere is delivered to a shortcut \
             the user turned off"
        );
        // And every bound row's id is distinct, so no press can be attributed
        // to two shortcuts at once.
        let mut ids: Vec<Option<u32>> = FillShortcut::ALL.into_iter().map(|w| fh.id(w)).collect();
        ids.retain(Option::is_some);
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(before, 4, "control: four rows should still be bound");
        assert_eq!(ids.len(), before, "two shortcuts are listening on one id");
    }

    /// Modifiers `RegisterHotKey` has no concept of are dropped at
    /// construction, so what is stored is what is registered.
    #[test]
    fn an_unregistrable_modifier_never_reaches_the_stored_chord() {
        let chord = Chord::new(Modifiers::CONTROL | Modifiers::META, Code::KeyB);
        assert_eq!(chord.mods, Modifiers::CONTROL);
        // ...and the reason it matters: `HotKey::new` rewrites META to SUPER,
        // so a chord that kept it would register as WIN+CTRL+B while the page
        // said something else.
        assert_eq!(chord.to_hotkey().mods, Modifiers::CONTROL);
    }

    // -- registration ------------------------------------------------------

    /// **The ordinary case, asserted positively.**
    ///
    /// Without this every other test here would pass with the feature
    /// deleted: "does not panic" is satisfied by a function that registers
    /// nothing and reports nothing. So this pins that a successful pass really
    /// does arm all five, and that the ids they will listen on are the ids of
    /// the chords the settings actually hold and not of some other chords.
    #[test]
    fn a_successful_registration_arms_every_shortcut_on_its_own_chord() {
        let chords = shipped();
        let fh = register_fill_hotkeys_with(chords, succeeds, unpublished, Instant::now());
        for which in FillShortcut::ALL {
            assert_eq!(fh.availability(which), HotkeyStatus::Armed, "{which:?}");
            assert_eq!(
                fh.id(which),
                Some(chords[which.index()].expect("every shipped default is bound").id()),
                "{which:?} is armed on an id that is not its own chord's, so every press would \
                 arrive under an id `fill_hotkey_pressed` attributes to something else"
            );
        }
        // And an armed hotkey never re-attempts, however long it runs.
        let mut fh = fh;
        assert!(!retry_if_unavailable(&mut fh, Instant::now() + Duration::from_secs(60 * 60 * 24)));
    }

    /// **The picker chord is still `CTRL+ALT+B`, and it is still the one the
    /// wild crash was about.**
    #[test]
    fn the_picker_is_registered_on_the_chord_the_crash_was_over() {
        let fh = register_fill_hotkeys_with(shipped(), succeeds, unpublished, Instant::now());
        assert_eq!(fh.chord(FillShortcut::Picker), Some(conflicting_chord()));
        assert_eq!(fh.id(FillShortcut::Picker), Some(conflicting_chord().id()));
    }

    /// **The reported crash, as a decision.** The run that produced
    /// `AlreadyRegistered` must come back as a live `FillHotkeys` reporting
    /// why, not as a process that has ended.
    #[test]
    fn already_registered_leaves_a_live_app_that_knows_why() {
        let fh =
            register_fill_hotkeys_with(shipped(), already_registered, unpublished, Instant::now());
        for which in FillShortcut::ALL {
            assert_eq!(
                fh.availability(which),
                HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram),
                "the case that killed a real run is not classified as a conflict for \
                 {which:?}, so the user is told the wrong thing about the one failure they \
                 can act on"
            );
        }
        // The chord is still known, so a later retry re-attempts the right
        // one and a press that arrives after it succeeds is still recognised.
        assert_eq!(fh.id(FillShortcut::Picker), Some(conflicting_chord().id()));
    }

    /// **One chord lost does not cost the other four.**
    ///
    /// The property the single-chord version had nothing to say about and the
    /// one a five-chord pass can most easily get wrong: a `?` on the first
    /// refusal, or a manager dropped when one registration failed, would take
    /// every shortcut down over one conflict. Both of those are live hazards
    /// here -- the manager is shared -- so this is asserted on the states
    /// AND on the shared manager surviving.
    #[test]
    fn one_shortcut_another_program_took_does_not_take_the_others_down() {
        let fh = register_fill_hotkeys_with(
            shipped(),
            only_the_password_chord_is_taken,
            unpublished,
            Instant::now(),
        );
        assert_eq!(
            fh.availability(FillShortcut::Password),
            HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram),
            "the chord that was refused is not reported as refused"
        );
        for which in FillShortcut::ALL {
            if which == FillShortcut::Password {
                continue;
            }
            assert_eq!(
                fh.availability(which),
                HotkeyStatus::Armed,
                "{which:?} was given up on because a DIFFERENT shortcut could not be \
                 registered; one conflict has cost the user four working shortcuts"
            );
        }
        // ...and the one that failed is the only one that will be retried.
        let mut fh = fh;
        assert!(retry_if_unavailable(&mut fh, Instant::now() + RETRY_EVERY));
        assert_eq!(
            fh.availability(FillShortcut::Password),
            HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram)
        );
        assert_eq!(fh.availability(FillShortcut::Username), HotkeyStatus::Armed);
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
            let fh = register_fill_hotkeys_with(shipped(), attempt, unpublished, Instant::now());
            for which in FillShortcut::ALL {
                assert_eq!(fh.availability(which), HotkeyStatus::Unavailable(expected));
            }
        }
        // And each of them says something to the user rather than nothing.
        for reason in [
            Unavailable::TakenByAnotherProgram,
            Unavailable::NoManager,
            Unavailable::Refused,
            Unavailable::NotYetAttempted,
        ] {
            let message = reason.message("CTRL+ALT+B");
            assert!(
                message.contains("Everything else works normally"),
                "{reason:?} tells the user the shortcut is off without telling them the rest \
                 of the app is not: {message:?}"
            );
            assert!(
                message.contains("CTRL+ALT+B"),
                "{reason:?} does not name the chord it is about, so a user with five \
                 shortcuts cannot tell which row is broken: {message:?}"
            );
        }
    }

    /// **A cleared shortcut is not a broken one.**
    ///
    /// It registers nothing, reports `Unbound` rather than a failure, is never
    /// retried, and can never claim a press -- which is the whole of what
    /// "clear this binding" has to mean.
    #[test]
    fn a_cleared_shortcut_registers_nothing_and_is_never_retried() {
        let mut chords = shipped();
        chords[FillShortcut::Totp.index()] = None;
        let mut fh = register_fill_hotkeys_with(chords, succeeds, unpublished, Instant::now());
        assert_eq!(fh.availability(FillShortcut::Totp), HotkeyStatus::Unbound);
        assert_eq!(fh.id(FillShortcut::Totp), None);
        assert!(!should_retry(HotkeyStatus::Unbound, RETRY_EVERY * 100));
        assert!(!retry_if_unavailable(&mut fh, Instant::now() + RETRY_EVERY * 100));
        // The other four are untouched by it.
        assert_eq!(fh.availability(FillShortcut::Picker), HotkeyStatus::Armed);
    }

    /// The pacing rule, in all three directions.
    #[test]
    fn only_an_unavailable_hotkey_retries_and_only_after_the_interval() {
        let taken = HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram);
        assert!(!should_retry(taken, RETRY_EVERY - Duration::from_millis(1)));
        assert!(should_retry(taken, RETRY_EVERY));
        assert!(!should_retry(HotkeyStatus::Armed, RETRY_EVERY * 100));
        assert!(!should_retry(HotkeyStatus::Unbound, RETRY_EVERY * 100));
    }

    /// **The 3,416-line report, as a rule.**
    ///
    /// `deskwarden.log` carried one sentence about `Ctrl+Alt+S` 3,416 times
    /// because the retry wrote a `warn!` on every pass. The retry is right and
    /// is untouched; what is pinned here is that the *sentence* is written only
    /// when something a reader did not already know has happened. See
    /// [`REMIND_EVERY`].
    #[test]
    fn an_unchanged_failure_is_logged_once_and_then_once_an_hour() {
        let taken = HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram);
        let refused = HotkeyStatus::Unavailable(Unavailable::Refused);
        let untried = HotkeyStatus::Unavailable(Unavailable::NotYetAttempted);

        // Nothing said yet: the first failure of a launch or a rebind.
        assert!(should_log_failure(untried, taken, None));
        // ...and it is the ONLY one, for the next hour of half-minute retries.
        assert!(!should_log_failure(taken, taken, Some(Duration::ZERO)));
        assert!(!should_log_failure(taken, taken, Some(RETRY_EVERY)));
        assert!(!should_log_failure(
            taken,
            taken,
            Some(REMIND_EVERY - Duration::from_millis(1))
        ));
        // One line an hour, so a long session still says it never recovered.
        assert!(should_log_failure(taken, taken, Some(REMIND_EVERY)));
        // A different reason is a different fact, however recently the last
        // one was written.
        assert!(should_log_failure(taken, refused, Some(Duration::ZERO)));

        // The arithmetic the report is actually about: at `RETRY_EVERY` the
        // old rule wrote a line per attempt, the new one writes a line per
        // `REMIND_EVERY`.
        let a_day = Duration::from_secs(60 * 60 * 24);
        assert_eq!(a_day.as_secs() / RETRY_EVERY.as_secs(), 2_880);
        assert_eq!(a_day.as_secs() / REMIND_EVERY.as_secs(), 24);
    }

    /// **Quiet is earned per outage, not per process.**
    ///
    /// A chord that fails, is claimed when the other program exits, and is
    /// lost again when it restarts has had two outages, and the second one is
    /// news however little time has passed since the first was written about.
    /// The recovery in the middle is what resets it -- see `register_all`.
    #[test]
    fn a_failure_after_a_recovery_is_news_again() {
        let taken = HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram);
        let start = Instant::now();
        let mut fh = register_fill_hotkeys_with(shipped(), already_registered, unpublished, start);
        for binding in &fh.bindings {
            assert!(
                binding.complained_at.is_some(),
                "the first failure was not written about at all"
            );
        }

        // The other program lets go. Nothing is outstanding any more.
        fh.attempt = succeeds;
        assert!(retry_if_unavailable(&mut fh, start + RETRY_EVERY));
        for binding in &fh.bindings {
            assert_eq!(binding.status, HotkeyStatus::Armed);
            assert!(
                binding.complained_at.is_none(),
                "a shortcut that is working is still holding an hour of earned quiet, so the \
                 next time it breaks the log will not say so"
            );
        }

        // ...and it comes back. `should_log_failure` sees `None` again.
        fh.attempt = already_registered;
        fh.bindings[0].status = taken;
        assert!(should_log_failure(
            HotkeyStatus::Armed,
            taken,
            fh.bindings[0].complained_at.map(|at| start - at)
        ));
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
        let mut fh =
            register_fill_hotkeys_with(shipped(), already_registered, unpublished, start);
        let taken = HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram);
        assert_eq!(fh.availability(FillShortcut::Picker), taken);

        // Too soon: nothing is attempted and nothing changes.
        assert!(!retry_if_unavailable(&mut fh, start + Duration::from_secs(5)));
        assert_eq!(fh.availability(FillShortcut::Picker), taken);

        // The conflicting program exits. The next due attempt arms them.
        fh.attempt = succeeds;
        assert!(retry_if_unavailable(&mut fh, start + RETRY_EVERY));
        for which in FillShortcut::ALL {
            assert_eq!(
                fh.availability(which),
                HotkeyStatus::Armed,
                "the retry ran and succeeded but {which:?} is still reported as unavailable, \
                 so the user is told to close a program they have already closed"
            );
        }
        // ... and having armed, it stops re-attempting for good.
        assert!(!retry_if_unavailable(&mut fh, start + RETRY_EVERY * 10));
    }

    /// A retry that fails again leaves the reason it failed *this* time, not
    /// a stale one.
    #[test]
    fn a_retry_that_fails_again_reports_the_new_reason() {
        let start = Instant::now();
        let mut fh =
            register_fill_hotkeys_with(shipped(), already_registered, unpublished, start);
        fh.attempt = no_manager;
        assert!(retry_if_unavailable(&mut fh, start + RETRY_EVERY));
        assert_eq!(
            fh.availability(FillShortcut::Picker),
            HotkeyStatus::Unavailable(Unavailable::NoManager)
        );
    }

    /// **A rebind takes the new chords and forgets the old ones.**
    ///
    /// The forgetting is the half that has no other witness: a rebind that
    /// registered the new chord without dropping the manager would leave the
    /// old combination still claimed by this process and still firing, and
    /// every assertion about the new one would pass.
    #[test]
    fn rebinding_moves_a_shortcut_and_leaves_nothing_behind() {
        let start = Instant::now();
        let mut fh = register_fill_hotkeys_with(shipped(), succeeds, unpublished, start);
        let was = fh.id(FillShortcut::Username);

        let mut chords = shipped();
        let moved = Chord::new(Modifiers::CONTROL | Modifiers::SHIFT, Code::F9);
        chords[FillShortcut::Username.index()] = Some(moved);
        assert!(rebind_if_changed(&mut fh, chords, start));
        assert_eq!(fh.chord(FillShortcut::Username), Some(moved));
        assert_eq!(fh.id(FillShortcut::Username), Some(moved.id()));
        assert_ne!(fh.id(FillShortcut::Username), was, "the old id is still being listened for");
        assert_eq!(fh.availability(FillShortcut::Username), HotkeyStatus::Armed);
        // Everything else came back armed too, rather than being left in
        // whatever state the teardown put it in.
        for which in FillShortcut::ALL {
            assert_eq!(fh.availability(which), HotkeyStatus::Armed, "{which:?}");
        }
        // An unchanged set is not a rebind, so the ordinary loop iteration
        // makes no user32 calls at all.
        assert!(!rebind_if_changed(&mut fh, chords, start));
    }

    /// **Two shortcuts can swap chords**, which is the case a per-row rebind
    /// gets wrong: registering the incoming chord while the outgoing row still
    /// holds it is a conflict with ourselves, reported to the user as though
    /// another program had taken it.
    #[test]
    fn two_shortcuts_can_swap_their_chords() {
        let start = Instant::now();
        let mut fh = register_fill_hotkeys_with(shipped(), succeeds, unpublished, start);
        let mut chords = shipped();
        chords.swap(FillShortcut::Username.index(), FillShortcut::Password.index());
        assert!(rebind_if_changed(&mut fh, chords, start));
        assert_eq!(fh.availability(FillShortcut::Username), HotkeyStatus::Armed);
        assert_eq!(fh.availability(FillShortcut::Password), HotkeyStatus::Armed);
        assert_eq!(fh.chord(FillShortcut::Username), shipped()[FillShortcut::Password.index()]);
    }

    /// `classify` is the one decision, so it is pinned directly as well --
    /// including the `Ok` arm, which is what makes "armed" mean anything.
    #[test]
    fn classify_maps_every_outcome_to_a_survivable_one() {
        assert_eq!(classify(Ok(())), HotkeyStatus::Armed);
        let already = global_hotkey::Error::AlreadyRegistered(conflicting_chord().to_hotkey());
        assert_eq!(
            classify(Err(&already)),
            HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram)
        );
        let os = global_hotkey::Error::OsError(std::io::Error::other("x"));
        assert_eq!(classify(Err(&os)), HotkeyStatus::Unavailable(Unavailable::NoManager));
        let other = global_hotkey::Error::FailedToWatchMediaKeyEvent;
        assert_eq!(classify(Err(&other)), HotkeyStatus::Unavailable(Unavailable::Refused));
        // And it never invents the two states that are not decisions.
        assert_ne!(classify(Ok(())), HotkeyStatus::Unbound);
        assert_ne!(
            classify(Err(&already)),
            HotkeyStatus::Unavailable(Unavailable::NotYetAttempted)
        );
    }

    /// **Before anything has tried, the page says nothing has tried -- not
    /// that the shortcut works.**
    ///
    /// This is the defect this variant was added for. Registration runs after
    /// `main` opens the startup vault window; that call blocks for the
    /// window's whole life and the Preferences modal inside it is drawn in
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
    /// `register_fill_hotkeys_with` publishes through `announce` on every path
    /// above, and the *reading* of a published value is
    /// `prefs_ui::draw_section`'s one line.
    #[test]
    fn a_process_that_has_not_tried_yet_says_so_rather_than_claiming_the_shortcut_works() {
        for which in FillShortcut::ALL {
            assert_eq!(
                availability(which),
                HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
                "the published default for {which:?} is a claim again: the Shortcuts page \
                 inside the startup vault window reads this before anything has called \
                 RegisterHotKey, so this is what it would show the user"
            );
        }
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
        let message = Unavailable::NotYetAttempted.message("CTRL+ALT+B");
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
            assert_ne!(
                other.message("CTRL+ALT+B"),
                message,
                "{other:?} and NotYetAttempted read the same"
            );
        }
    }

    /// **`NotYetAttempted` is never a *decided* status.**
    ///
    /// It is what [`availability`] synthesises for "nothing published", and
    /// nothing else may produce it: a registration that came back as
    /// "not attempted" would be a `FillHotkeys` that reported it had never run
    /// the attempt it had just run.
    #[test]
    fn no_attempt_can_ever_report_that_it_was_not_attempted() {
        let not_attempted = HotkeyStatus::Unavailable(Unavailable::NotYetAttempted);
        for attempt in [succeeds as RegisterAttempt, already_registered, no_manager, refused] {
            let fh = register_fill_hotkeys_with(shipped(), attempt, unpublished, Instant::now());
            for which in FillShortcut::ALL {
                assert_ne!(fh.availability(which), not_attempted);
            }
        }
        assert_ne!(classify(Ok(())), not_attempted);
        let already = global_hotkey::Error::AlreadyRegistered(conflicting_chord().to_hotkey());
        assert_ne!(classify(Err(&already)), not_attempted);
        // It does, however, retry like any other unavailable state, so a
        // status that somehow reached the main loop would go and find out
        // rather than sit there being not-yet-attempted forever.
        assert!(should_retry(not_attempted, RETRY_EVERY));
    }
}

#[cfg(test)]
mod status_file_tests {
    //! The daemon -> UI-process hop for the Shortcuts page.
    //!
    //! **The defect these exist for**, reported against a running build:
    //! the page said *"Deskwarden has not tried to claim CTRL+ALT+B yet"* on
    //! a machine whose log, seconds earlier, said `the global shortcut for
    //! Fill the focused app is registered`. Both were true of their own
    //! process. `STATUS` is process-wide, the daemon is the only process that
    //! registers a chord, and the vault window -- which draws the page --
    //! registers none, so it read an empty status and rendered the honest
    //! empty answer forever.
    //!
    //! Everything here drives the pure encode/parse pair rather than
    //! `STATUS` itself. That static is shared by every test in this process,
    //! so a test that wrote it would change what the rest of the run
    //! observes -- the same reason `register_all`'s tests publish nowhere.

    use super::*;

    fn armed(chord: &str) -> Published {
        Published { chord: Chord::parse(chord), status: HotkeyStatus::Armed }
    }

    fn five() -> [Published; FillShortcut::COUNT] {
        [
            armed("CTRL+ALT+B"),
            armed("CTRL+ALT+U"),
            Published {
                chord: Chord::parse("CTRL+ALT+P"),
                status: HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram),
            },
            Published { chord: None, status: HotkeyStatus::Unbound },
            Published {
                chord: Chord::parse("CTRL+ALT+S"),
                status: HotkeyStatus::Unavailable(Unavailable::Refused),
            },
        ]
    }

    /// **Everything the daemon knows survives the trip**, chord and status
    /// alike, for every row including the two that are not simply armed.
    ///
    /// The round trip is what the feature IS: the page on the other side
    /// renders exactly these two fields, so a chord that arrived without its
    /// status -- or a status against the wrong chord -- is the whole bug back
    /// in a new shape.
    #[test]
    fn every_row_round_trips_through_the_file_format() {
        let written = encode_status(&five());
        let read = parse_status_report(&written);
        assert_eq!(read.len(), FillShortcut::COUNT, "read back {read:?}");
        for which in FillShortcut::ALL {
            let (_, got) = read
                .iter()
                .find(|(w, _)| *w == which)
                .unwrap_or_else(|| panic!("{which:?} is missing from {written:?}"));
            assert_eq!(*got, five()[which.index()], "{which:?} came back changed");
        }
    }

    /// **A cleared row keeps its `None`**, and does not come back as a chord
    /// this build invented. `Unbound` with a chord beside it would be the
    /// page drawing a shortcut the user deliberately cleared.
    #[test]
    fn a_cleared_row_comes_back_cleared() {
        let read = parse_status_report(&encode_status(&five()));
        let (_, totp) = read.iter().find(|(w, _)| *w == FillShortcut::Totp).expect("the row");
        assert_eq!(totp.chord, None);
        assert_eq!(totp.status, HotkeyStatus::Unbound);
    }

    /// **The rows are keyed, not positional.** A file whose lines arrive in
    /// another order -- or with a row missing, which is what a newer daemon
    /// writing a sixth shortcut looks like from here -- must not shift every
    /// answer onto its neighbour.
    #[test]
    fn the_rows_are_read_by_name_and_not_by_position() {
        let text = "sequence\tCTRL+ALT+S\trefused\npicker\tCTRL+ALT+B\tarmed\n";
        let read = parse_status_report(text);
        assert_eq!(read.len(), 2);
        let (_, picker) =
            read.iter().find(|(w, _)| *w == FillShortcut::Picker).expect("the picker row");
        assert_eq!(picker.status, HotkeyStatus::Armed);
        assert_eq!(picker.chord, Chord::parse("CTRL+ALT+B"));
    }

    /// **A line this build cannot read costs that line and nothing else.**
    ///
    /// Three shapes, one rule: a status word from a newer daemon, a chord
    /// spelling this build does not know, and a line with a field missing.
    /// The alternative -- dropping the file on the first bad line -- would
    /// turn one unknown word from a later version into a page that reports
    /// nothing at all, which is the exact failure being fixed.
    #[test]
    fn one_unreadable_line_does_not_cost_the_others() {
        let text = "picker\tCTRL+ALT+B\tarmed\n\
                    username\tCTRL+ALT+U\tsomething-new\n\
                    password\tCTRL+ALT+NOPE\tarmed\n\
                    totp\tarmed\n\
                    sequence\t\tunbound\n";
        let read = parse_status_report(text);
        let names: Vec<FillShortcut> = read.iter().map(|(w, _)| *w).collect();
        assert_eq!(
            names,
            vec![FillShortcut::Picker, FillShortcut::Sequence],
            "read {read:?}"
        );
    }

    /// **A status is never shown against a chord it does not belong to.** An
    /// unparseable chord drops its whole row rather than becoming `None`:
    /// `None` means *cleared*, and reporting a row the user has bound as
    /// cleared is a different lie, not a smaller one.
    #[test]
    fn an_unreadable_chord_drops_its_row_rather_than_clearing_it() {
        let read = parse_status_report("picker\tCTRL+ALT+NOPE\tarmed\n");
        assert!(read.is_empty(), "read {read:?}");
    }

    /// **An empty file adopts nothing**, so a UI process keeps the honest
    /// default rather than being handed five rows of silence.
    #[test]
    fn an_empty_report_says_nothing() {
        assert!(parse_status_report("").is_empty());
        assert!(parse_status_report("\n\n").is_empty());
        assert!(parse_status_report("nonsense").is_empty());
    }

    /// **Every status word survives its own round trip**, including the two
    /// nothing writes today.
    ///
    /// `not-yet` is written when a rebind has published one row and not the
    /// others, and `no-manager` when Windows gave the daemon no hook at all.
    /// Both are states the page has a sentence for, so both have to arrive
    /// intact -- and a `Debug` rendering, which is what this pair replaced,
    /// is a thing a rename changes silently.
    #[test]
    fn every_status_word_round_trips() {
        for status in [
            HotkeyStatus::Armed,
            HotkeyStatus::Unbound,
            HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram),
            HotkeyStatus::Unavailable(Unavailable::NoManager),
            HotkeyStatus::Unavailable(Unavailable::Refused),
            HotkeyStatus::Unavailable(Unavailable::NotYetAttempted),
        ] {
            assert_eq!(
                status_from_word(status_word(status)),
                Some(status),
                "{status:?} did not survive its own word"
            );
        }
    }

    /// **The five keys are distinct**, so no two rows can overwrite each
    /// other. Trivially true today and the kind of thing a copy-pasted arm
    /// breaks.
    #[test]
    fn the_five_keys_are_five_different_words() {
        let mut keys: Vec<&str> = FillShortcut::ALL.into_iter().map(file_key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), FillShortcut::COUNT);
    }

    /// **The path is in the config directory and is not keyed on a pid.**
    ///
    /// Every `ui-*.json` beside it carries the child's pid, because those
    /// travel child -> daemon and there may be several children. This one
    /// travels daemon -> child and there is exactly one daemon; a pid in the
    /// name would be a filename the reader cannot construct.
    #[test]
    fn the_status_file_sits_in_the_config_directory_under_a_fixed_name() {
        let path = status_file_path(std::path::Path::new("C:\\cfg"));
        assert_eq!(path.parent(), Some(std::path::Path::new("C:\\cfg")));
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some(STATUS_FILE_NAME));
        assert!(
            !STATUS_FILE_NAME.contains("{pid}") && !STATUS_FILE_NAME.contains('%'),
            "the status file's name is not fixed, so a UI process cannot find it"
        );
    }

    /// **Only the daemon writes it, and only a UI process reads it.**
    ///
    /// A source pin because the rule is about which process calls what, and
    /// no type can carry that. `publish_status_file` in the window would have
    /// it overwrite the daemon's answers with its own empty ones;
    /// `adopt_status_file` in the daemon would replace what that process
    /// actually knows with a copy of what it just wrote -- and, in the
    /// startup window that runs inside it before the first registration pass,
    /// with a stale answer from the previous run.
    ///
    /// Needles split with `concat!` so they cannot match their own
    /// declaration. Do not re-join them.
    #[test]
    fn the_writer_is_the_daemon_and_the_reader_is_the_window() {
        let main_rs = include_str!("main.rs");
        let window = include_str!("vault_window/mod.rs");
        let publish = concat!("publish_status_", "file(");
        let adopt = concat!("adopt_status_", "file(");
        assert!(
            main_rs.contains(publish),
            "the daemon no longer writes the shortcut status, so the Shortcuts page in a UI \
             process has nothing to read and is back to saying nothing was attempted"
        );
        assert!(
            !main_rs.contains(adopt),
            "the daemon adopts its own status file, which overwrites what it knows with what \
             it wrote -- and in the startup window, which runs inside it before the first \
             registration pass, with the previous run's answers"
        );
        assert!(
            window.contains(adopt),
            "the vault window no longer adopts the daemon's status, so its Shortcuts page \
             reads a status this process never publishes"
        );
        assert!(
            !window.contains(publish),
            "the vault window writes the status file, overwriting the daemon's real answers \
             with the empty ones of a process that registers no chords"
        );
    }
}

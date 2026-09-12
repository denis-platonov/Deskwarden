//! **The record you just filled, offered back at the same window for five
//! minutes -- and nothing else, anywhere, ever.**
//!
//! # What the owner asked for
//!
//! > also save the previous entry for tray app and offer it next time if
//! > within 5 minutes and same window
//! >
//! > example, I want to enter my creds to a browser and they mostly use
//! > multipage flow - email, next, password, next, TOTP - this way I don't
//! > need to search for the same record 3 times - do it once, next time for
//! > password when ctrl alt b - it shows same record and prompts what I want
//! > to send - email\pass\totp already, so I can do 2 and enter, add back kind
//! > of thing if user needs to pick another records instead of the one
//! > prepopulated
//!
//! A modern sign-in is three pages: the address, then the password, then the
//! challenge. Each page is one `CTRL+ALT+B`, and each press today opens
//! [`crate::picker_prompt`]'s account picker at a list the user has to search
//! *again* for the item they used ninety seconds ago. This module is the one
//! fact that removes the second and third search: which vault item the last
//! fill used, at which window, and when.
//!
//! # The identity, and the trap it exists to avoid
//!
//! **The window title is not part of "same window", and that is the whole
//! feature.** In exactly the flow this was written for the title changes
//! between the steps -- `Sign in - Contoso` becomes `Enter your password -
//! Contoso` becomes `Verify it's you - Contoso`, and a Chromium tab's title is
//! whatever the page's `<title>` currently says. A title-sensitive identity
//! would therefore expire this memory at precisely the three moments the user
//! needs it, and would look *correct* in every test that did not navigate.
//! [`WindowKey::of`] reads `hwnd`, `pid` and `exe_name` off a
//! [`crate::window_watch::ForegroundEvent`] and deliberately leaves `title`
//! untouched; `the_key_ignores_the_title_because_a_sign_in_flow_retitles_itself`
//! is the test that holds it, and it is the first one written.
//!
//! ## Why not the `HWND` alone
//!
//! Windows recycles window handles. A handle is an index into a table with a
//! reuse counter in its high bits, and while the reuse is not *immediate* it
//! is certainly possible inside five minutes on a desktop where windows open
//! and close constantly. So the question is what a collision would cost.
//!
//! It would cost a **credential typed into the wrong application**. The whole
//! point of this feature is that the user does not read the card: they press
//! `2`, then Enter, from muscle memory built over the previous two pages. A
//! card that named the right item at the wrong window would be read as the
//! right card, and the password that follows goes wherever the keyboard focus
//! is. Between "visible enough that the user would catch it" and "a card this
//! design explicitly trains people not to read", the honest answer is the
//! second -- so the guard has to be in the state and not on the card.
//!
//! `pid` closes it, and closes it structurally rather than probabilistically.
//! Windows does not reuse a live window's handle: for the handle to come back
//! the original window must have been destroyed. A destroyed window whose
//! process is still running gives a *new* window in the *same* process, which
//! is the ordinary case -- a dialog closing and reopening -- and which the
//! `pid` check accepts, correctly: it is the same application, in front of the
//! same user, doing the same sign-in. For the offer to land in a *different*
//! application, that application must also have inherited the original's
//! process id, and a process id is only reissued after the original process
//! exits. So the pair is stable together or gone together.
//!
//! `exe_name` is the third belt and it is nearly free: it is already on the
//! event, it is stable across every navigation (the process does not change
//! when a page does), and it reduces the residual failure -- a recycled handle
//! *and* a recycled pid -- from "the wrong application" to "the same
//! executable". It is emphatically not a substitute for either of the other
//! two: `ApplicationFrameHost.exe` owns the top-level window of *every*
//! Microsoft Store app on the machine (see [`crate::window_watch`]'s
//! `WINDOW_HOSTS`, where that case is documented from a real report), so an
//! identity that leaned on the executable name would treat every Store app as
//! the same window.
//!
//! # Five minutes from the **last** fill, not the first
//!
//! The owner said "within 5 minutes". Read as five minutes from the *first*
//! fill it is a budget for the whole sign-in, and a three-page flow with a
//! slow identity provider, an SMS that takes a while, or a user who reads the
//! consent screen will exceed it -- so the memory would expire at step three,
//! the TOTP step, which is the single step this feature exists for. Read as
//! five minutes from the *last* fill it is an idle timeout: "it has been five
//! minutes since you last used this", which is also what the phrase means in
//! ordinary speech.
//!
//! The second reading is taken. It does not let the memory drift indefinitely,
//! which is the objection to it: the clock is only pushed forward by
//! [`FillRecall::remember`], which only runs when a fill actually happened, so
//! every extension is an act the user performed at that same window a moment
//! ago. A memory that survives half an hour survived it by being used six
//! times, which is a user in a flow, not a stale record.
//!
//! # What invalidates it early, and what deliberately does not
//!
//! * **The vault locking does.** [`Forgotten::VaultLocked`]. The memory names
//!   a vault item id and nothing else, so while locked it cannot even be
//!   resolved to a name -- but the real reason is that locking is the user
//!   saying stop. Coming back to an unlocked vault and being handed a
//!   prepopulated record from before the lock would be this app deciding that
//!   a lock was about the vault file rather than about the person.
//! * **Going back does.** [`Forgotten::UserWentBack`]. The card's last row is
//!   *Pick a different record*, and a user who takes it has said "not that
//!   one". Offering it again on the very next press is how a convenience
//!   becomes something the user cannot get out of -- which is worse than
//!   having no memory at all, because at least the picker opens.
//! * **A different window in between does not.** Step three of the motivating
//!   flow is a one-time code, and fetching one means alt-tabbing to a phone
//!   app, an authenticator window or a mail client and coming back. An
//!   identity check that fired on every foreground change would break the step
//!   it was written for. Nothing in this module watches the foreground at all:
//!   the key is compared at the moment of the press, and an excursion leaves
//!   no trace.
//! * **A fill somewhere else does**, implicitly, and that is the reason there
//!   is exactly one slot rather than a map keyed by window.
//!   [`crate::app::PasswordFieldProbe`] keeps eight entries because its
//!   question ("does this window have a password box") is per-window and
//!   cheap to be wrong about. This question is not: a map would offer a record
//!   at a window the user filled twenty minutes ago and has since forgotten
//!   about, which is not the flow that was described and is a great deal more
//!   surprising. One slot means the memory is always *the thing the user is
//!   demonstrably doing right now*, and a fill at another window replaces it
//!   because the user just showed us what they moved on to.
//! * **The item leaving the vault does**, but not here: the offer is only
//!   built if the id still resolves in [`crate::vault_cache::VaultCache`], and
//!   a miss falls through to the ordinary picker. This module stores an id,
//!   not an item, so there is nothing here to go stale.
//!
//! # Why this is in memory and **not on disk**
//!
//! [`crate::receive_history`] and [`crate::scan_history`] are the precedent
//! for state that persists, and both of their module docs argue the case for
//! a file: a *record*, written after the fact, that the user is later shown
//! and that must survive a restart, because a row on a screen that counts
//! something has to have something to count. Every one of those arguments
//! runs the other way here.
//!
//! * **Nothing is ever shown from it after the fact.** There is no history
//!   screen for this, no count, and no row. Its entire readership is one
//!   `CTRL+ALT+B` in the next five minutes.
//! * **A restart is exactly when it should be gone.** The memory means "you
//!   are in the middle of signing in to something"; a process that has just
//!   started is definitionally not in the middle of anything.
//! * **The file would be a tracking log.** A record on disk saying *at
//!   14:12 the user filled vault item `9f2c…` into a window of `chrome.exe`*,
//!   appended to all day, is a timeline of which accounts this person used and
//!   when -- readable by anything running as the user, surviving every lock,
//!   every sign-out and every reboot. `scan_history` refuses to write item
//!   *names* for a narrower version of this reason, and `receive_history`
//!   refuses to write access URLs for a sharper one. A five-minute convenience
//!   does not get to be the feature that writes the user's day down.
//!
//! So the state is a field on `main`'s event loop, beside `pending_hotkey_fill`
//! and the field probe's memo, and it dies with the process. There is no
//! `save`, no `load`, no path, and no [`crate::settings::config_dir`] in this
//! file -- `nothing_in_this_module_reaches_the_filesystem` asserts that by
//! reading this file's own source, because a promise in a comment is not a
//! guard.
//!
//! # The window census
//!
//! [`crate::foreground`] requires every module in this crate to be classified
//! as one that opens windows or one that does not, and this one **opens no
//! window**. It holds three scalars and a `String` and answers questions about
//! them; it creates nothing, paints nothing, and has no title for
//! `foreground::pick` to match. The card that the answer causes to appear is
//! [`crate::prompt_card`]'s, which is classified in its own right and is
//! raised by its own site. The classification is in `OPENS_NO_WINDOW` for the
//! same reason `app_candidates` and `app_match` are: deciding what to show is
//! not showing it.
//!
//! # The clock is a parameter
//!
//! Every function here that can expire something takes `now` rather than
//! calling [`std::time::Instant::now`], which is [`crate::app::PROBE_TTL`]'s
//! shape and [`crate::send::SendClock`]'s argument: a five-minute rule tested
//! by waiting five minutes is a rule that is not tested. Production passes
//! `Instant::now()` at the one call site in `main`; the tests below move time
//! by hand and cover the boundary from both sides.

use std::time::{Duration, Instant};

/// **How long a remembered record stays on offer**, measured from the last
/// fill that used it.
///
/// Five minutes, because that is the number the owner gave. It is not tuned
/// against anything measurable -- unlike [`crate::app::PROBE_TTL`], which is
/// chosen against a UI Automation walk whose cost was measured -- so it is
/// written here once, named, and read by the one function that expires
/// anything, rather than being a literal `300` somewhere in `main`.
///
/// The trade either way is mild and worth stating so a later change has the
/// argument rather than the number. Too short and the third page of a slow
/// sign-in searches again, which is the defect this feature removes. Too long
/// and a user who wandered off comes back to a card offering a record from a
/// context they have forgotten -- which costs one keystroke, because the card
/// has a way back, and no secret, because the card names an item and never a
/// value.
pub const RECALL_TTL: Duration = Duration::from_secs(5 * 60);

/// **What this module means by "the same window".**
///
/// Three fields, and the one that is missing is the point of the type -- see
/// the module header for why a title cannot be in here and what it would cost
/// if it were.
///
/// `Clone` because `main` builds one per press and one per fill and the two
/// are compared; `PartialEq` because that comparison *is* the identity, and a
/// hand-written `same()` would be a second definition of it that could drift
/// from the derived one. `Debug` is safe: an `hwnd`, a `pid` and an executable
/// name are not secrets, and there is no item id in this type at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowKey {
    /// The top-level window handle. Stable across the navigations that retitle
    /// a page, and recycled only after the window is destroyed.
    pub hwnd: isize,
    /// The owning process. **The recycling guard** -- see the module header.
    pub pid: u32,
    /// The owning process's executable file name, as
    /// [`crate::window_watch::ForegroundEvent`] reports it.
    ///
    /// The third belt and the weakest one: it is the *host's* name for a
    /// Microsoft Store app, so it distinguishes nothing among them. It is kept
    /// because it costs nothing and because it narrows the residual
    /// double-recycle failure from "another application" to "another window of
    /// the same executable".
    pub exe_name: String,
}

impl WindowKey {
    /// The key for the window this event describes.
    ///
    /// **It reads three of the event's four fields.** `title` is not one of
    /// them, and that omission is the feature: see the module header, and
    /// `the_key_ignores_the_title_because_a_sign_in_flow_retitles_itself`,
    /// which builds two events differing only in their title and asserts the
    /// keys are equal.
    pub fn of(event: &crate::window_watch::ForegroundEvent) -> Self {
        Self { hwnd: event.hwnd, pid: event.pid, exe_name: event.exe_name.clone() }
    }
}

/// **Why a memory was dropped before it expired.**
///
/// An enum rather than a bare `forget()` so that the reasons are enumerable,
/// so each one can be named by a test, and so the log line says which happened.
/// Expiry is deliberately not one of them: expiry is not an event, it is the
/// absence of a renewal, and it is answered by [`still_offered`] at read time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forgotten {
    /// The vault locked. See the module header: this is about the person, not
    /// about whether an id could still be resolved.
    VaultLocked,
    /// The user took the card's *Pick a different record* row. They have said
    /// "not that one", and a memory that survived it would be a
    /// prepopulation the user cannot get past.
    UserWentBack,
}

impl Forgotten {
    /// What the log says happened. One spelling per reason, beside the reason.
    pub fn reason(self) -> &'static str {
        match self {
            Self::VaultLocked => "the vault locked",
            Self::UserWentBack => "the user asked for a different record",
        }
    }
}

/// **Why there is nothing to offer.**
///
/// [`Recall::Nothing`] carries one of these rather than being a bare variant,
/// because the four ways to have nothing are the four cases this feature has
/// to get right and a test that could only assert "nothing" would pass for the
/// wrong reason. It is also what the log line says, which is the difference
/// between diagnosing "the memory expired" and "you were at a different
/// window" from a user's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    /// Nothing has been filled yet this session, or the memory was dropped --
    /// by a lock, or by the user going back.
    Empty,
    /// Something is remembered, but it was filled at a different window. Note
    /// that this is the answer for a **recycled** handle too, when the process
    /// no longer matches.
    DifferentWindow,
    /// The right window, but the last fill there was more than [`RECALL_TTL`]
    /// ago.
    Expired,
}

/// **What one press of the fill hotkey should be offered**, before the
/// ordinary picker is considered.
///
/// Borrowed rather than owned: the id lives in the [`FillRecall`] the caller
/// already holds, and a clone here would be a second copy of the one thing
/// this module stores for no reason but the shape of the return type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recall<'a> {
    /// Offer this vault item straight away, with its fill choices numbered and
    /// a way back to the picker. See [`crate::app::recall_rows`].
    Offer(&'a str),
    /// Open the ordinary account picker, exactly as this app always has.
    Nothing(Miss),
}

impl Recall<'_> {
    /// Whether this is an offer, for callers that need only the yes-or-no.
    ///
    /// A name rather than a `matches!` spelled out at each site: the reason a
    /// caller does not care *which* [`Miss`] it was is worth being able to
    /// read, and [`Miss`] exists so that the ones that do care can say so.
    pub fn is_offer(&self) -> bool {
        matches!(self, Self::Offer(_))
    }
}

/// One remembered fill.
///
/// Private: the only ways to make one and to read one are
/// [`FillRecall::remember`] and [`FillRecall::recall`], so `at` cannot be set
/// to anything but the clock reading of a real fill.
#[derive(Clone, Debug)]
struct Remembered {
    key: WindowKey,
    /// **A vault item id and never an item.** The same rule
    /// [`crate::app::NoMatchFollowUp::Fill`] and
    /// [`crate::receive_history::ReceivedRecord`] are written to: an id names
    /// a row in a vault an attacker would still have to open, while a name, a
    /// username or a password would be a second home for something the
    /// [`crate::vault_cache::VaultCache`] already holds under a `Zeroizing`.
    item_id: String,
    /// When the fill that put this here happened. **Pushed forward by every
    /// subsequent fill of the same record at the same window** -- see the
    /// module header on which reading of "five minutes" this is.
    at: Instant,
}

/// **Whether a memory taken at `at` is still on offer at `now`.**
///
/// A free function rather than a method so that the expiry rule is a thing a
/// test can drive with two `Instant`s and no state at all, and so that the
/// boundary is written exactly once. Strictly *less than*, matching
/// [`crate::app::PasswordFieldProbe::ask`]: at exactly [`RECALL_TTL`] the
/// memory is gone, so the rule reads "for five minutes" rather than "for five
/// minutes and one instant".
///
/// `Instant::duration_since` saturates to zero for a `now` before `at` rather
/// than panicking, so a clock that went backwards -- which
/// `Instant` promises cannot happen, and which this code therefore does not
/// try to detect -- answers "still live" rather than aborting a fill.
pub fn still_offered(at: Instant, now: Instant) -> bool {
    now.duration_since(at) < RECALL_TTL
}

/// **The record the last fill used, the window it was used at, and when.**
///
/// One slot. See the module header for why this is not a map keyed by window,
/// and why one slot is what makes "a fill somewhere else replaces it" fall out
/// rather than being a rule someone has to remember to write.
///
/// `Default` is "nothing has been filled yet", which is the state a freshly
/// started process is in and the state a lock returns it to.
#[derive(Debug, Default)]
pub struct FillRecall {
    last: Option<Remembered>,
}

impl FillRecall {
    /// A memory with nothing in it.
    pub fn new() -> Self {
        Self::default()
    }

    /// **Records that `item_id` was just filled at `key`.**
    ///
    /// Called after *every* fill, including the ones that came from this
    /// memory itself -- that is what makes [`RECALL_TTL`] an idle timeout
    /// rather than a budget for the whole sign-in, and it is the reading of
    /// the owner's "within 5 minutes" this module takes.
    ///
    /// It **replaces** rather than accumulating, and the replacement is total:
    /// a different item at the same window, or the same item at a different
    /// window, both overwrite. Neither is a special case worth writing,
    /// because both are the user telling us what they are doing now.
    pub fn remember(&mut self, key: WindowKey, item_id: &str, now: Instant) {
        // An empty id is not a memory. `app_candidates` skips items with no
        // id for the same reason: there is nothing to fill from later,
        // however well the card would read.
        if item_id.is_empty() {
            return;
        }
        self.last = Some(Remembered { key, item_id: item_id.to_string(), at: now });
    }

    /// **What to offer at `key`, at `now`.**
    ///
    /// Pure, total, and the whole of the decision. The three misses are
    /// distinguished rather than collapsed -- see [`Miss`] -- and the order
    /// they are checked in is the order they are cheapest to answer, not a
    /// precedence that means anything: an empty memory has no window and no
    /// time, and a memory for a different window is not worth asking the clock
    /// about.
    ///
    /// **It does not expire anything.** A stale entry is left in place and
    /// simply not offered, exactly as [`crate::app::PasswordFieldProbe`]
    /// leaves a stale probe answer in its list: this takes `&self`, so a read
    /// cannot be the thing that mutates, and the next [`Self::remember`]
    /// overwrites the slot anyway. Sweeping here would buy one `Option` of
    /// memory and cost the caller a `&mut` on a path that only asks a
    /// question.
    pub fn recall(&self, key: &WindowKey, now: Instant) -> Recall<'_> {
        let Some(last) = self.last.as_ref() else {
            return Recall::Nothing(Miss::Empty);
        };
        if last.key != *key {
            return Recall::Nothing(Miss::DifferentWindow);
        }
        if !still_offered(last.at, now) {
            return Recall::Nothing(Miss::Expired);
        }
        Recall::Offer(&last.item_id)
    }

    /// **Drops the memory before it expires**, for one of the two reasons that
    /// warrant it.
    ///
    /// Takes the reason rather than being a bare `clear()` so that every call
    /// site has to say which of [`Forgotten`]'s cases it is, and so the log
    /// line can too. A third call site with a third reason is then a change to
    /// this enum and to the tests that enumerate it, rather than one more
    /// `clear()` nobody notices.
    pub fn forget(&mut self, why: Forgotten) {
        if self.last.take().is_some() {
            log::info!(
                "the remembered record was dropped: {} -- the next fill hotkey press opens the \
                 account picker",
                why.reason()
            );
        }
    }

    /// Whether anything is remembered at all, regardless of window or age.
    ///
    /// For `main`'s logging and for the tests that assert [`Self::forget`]
    /// really emptied the slot rather than merely making [`Self::recall`]
    /// answer `Nothing` for some other reason.
    pub fn is_empty(&self) -> bool {
        self.last.is_none()
    }
}

/// **The running app's one memory.**
///
/// # Why a static, when everything above it is a value
///
/// Every decision in this module and in [`crate::app`] is a pure function over
/// a [`FillRecall`] that a test hands in, and that is not weakened here: this
/// is the *instance*, not a second implementation. The three wrappers below
/// are the whole of what touches it and none of them decides anything.
///
/// It is a static because of where the memory must be written. The first fill
/// of a sign-in -- the one the user searched for, and the only one worth
/// remembering, since it is what removes the second and third search -- is
/// carried out inside `main::process_foreground_event`'s account-picker arm:
/// an eleven-parameter function, reached through two seams, with nineteen call
/// sites of which seventeen are tests. Threading a twelfth `&mut` through all
/// nineteen to carry one word would put the entire risk of this change in the
/// places no test looks at, to buy an ownership story for a single `Option`.
///
/// This crate already answers this exact shape the same way and says so:
/// `app::SEARCH_CORPUS` is a `static Mutex` because the seam that reads it is
/// a bare `fn` pointer that cannot close over a cache, and `picker_prompt`'s
/// `MODE` and `PENDING` are statics because a window procedure is an
/// `extern "system"` function with nowhere to keep state. The reason here is
/// the same kind of reason: the write site cannot be handed the state.
///
/// **A poisoned lock is not a reason to lose a fill.** Every wrapper below
/// recovers with `PoisonError::into_inner`, which is what the rest of this
/// crate does with its own process-wide mutexes: the worst a torn state can
/// cost here is one card that opens the picker instead of the record, and
/// aborting a fill over it would be far worse than that.
static PRODUCTION: std::sync::Mutex<FillRecall> =
    std::sync::Mutex::new(FillRecall { last: None });

/// Runs `f` against the running app's memory.
///
/// Borrowed inside the closure and never out of it, so nothing can hold the
/// lock across a window being put on screen -- which is the whole hazard of a
/// process-wide mutex on a path that opens modal cards.
pub fn with_production<R>(f: impl FnOnce(&FillRecall) -> R) -> R {
    let guard = PRODUCTION.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&guard)
}

/// [`FillRecall::remember`] against the running app's memory.
///
/// Named for what it records rather than being a `with_production_mut`,
/// because the two things `main` does to this state -- record a fill, drop the
/// memory -- are the two call shapes that exist, and a general mutable
/// accessor would be an invitation to a third.
pub fn remember_fill(key: WindowKey, item_id: &str, now: Instant) {
    PRODUCTION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remember(key, item_id, now);
}

/// [`FillRecall::forget`] against the running app's memory.
pub fn forget_production(why: Forgotten) {
    PRODUCTION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .forget(why);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(hwnd: isize, pid: u32, exe: &str, title: &str) -> crate::window_watch::ForegroundEvent {
        crate::window_watch::ForegroundEvent {
            hwnd,
            pid,
            exe_name: exe.to_string(),
            title: title.to_string(),
        }
    }

    fn key(hwnd: isize, pid: u32, exe: &str) -> WindowKey {
        WindowKey::of(&event(hwnd, pid, exe, "whatever"))
    }

    /// **The trap, and the first test written.**
    ///
    /// A multi-page sign-in retitles its window at every step. If the title
    /// were part of the identity the memory would expire at step two and step
    /// three -- the two steps this feature exists for -- and every other test
    /// in this file would still pass.
    #[test]
    fn the_key_ignores_the_title_because_a_sign_in_flow_retitles_itself() {
        let sign_in = WindowKey::of(&event(0x1234, 900, "chrome.exe", "Sign in - Contoso"));
        let password = WindowKey::of(&event(0x1234, 900, "chrome.exe", "Enter your password"));
        let challenge = WindowKey::of(&event(0x1234, 900, "chrome.exe", "Verify it's you"));
        assert_eq!(sign_in, password);
        assert_eq!(password, challenge);
    }

    /// The positive control on the test above: the key is not simply equal to
    /// everything. Each of the three fields it *does* read makes a difference.
    #[test]
    fn the_key_reads_the_handle_the_process_and_the_executable() {
        let base = key(0x1234, 900, "chrome.exe");
        assert_ne!(base, key(0x9999, 900, "chrome.exe"), "the handle is not read");
        assert_ne!(base, key(0x1234, 111, "chrome.exe"), "the process is not read");
        assert_ne!(base, key(0x1234, 900, "firefox.exe"), "the executable is not read");
    }

    #[test]
    fn nothing_is_offered_before_anything_has_been_filled() {
        let recall = FillRecall::new();
        assert!(recall.is_empty());
        assert_eq!(
            recall.recall(&key(1, 1, "a.exe"), Instant::now()),
            Recall::Nothing(Miss::Empty)
        );
    }

    /// The motivating case: the same window, a minute later.
    #[test]
    fn the_same_window_inside_the_window_is_offered_the_same_record() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        recall.remember(key(0x1234, 900, "chrome.exe"), "item-9f2c", start);
        assert_eq!(
            recall.recall(&key(0x1234, 900, "chrome.exe"), start + Duration::from_secs(60)),
            Recall::Offer("item-9f2c")
        );
    }

    /// The boundary, from both sides and exactly on it. Strictly less than, so
    /// the rule reads "for five minutes".
    #[test]
    fn the_offer_ends_exactly_at_the_ttl() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        let window = key(0x1234, 900, "chrome.exe");
        recall.remember(window.clone(), "item-9f2c", start);

        assert_eq!(
            recall.recall(&window, start + RECALL_TTL - Duration::from_millis(1)),
            Recall::Offer("item-9f2c")
        );
        assert_eq!(recall.recall(&window, start + RECALL_TTL), Recall::Nothing(Miss::Expired));
        assert_eq!(
            recall.recall(&window, start + RECALL_TTL + Duration::from_millis(1)),
            Recall::Nothing(Miss::Expired)
        );
    }

    /// **Which reading of "five minutes" this is.** Three fills two hundred
    /// seconds apart span more than eight minutes from the first, and the
    /// third is still offered -- because the clock is reset by each fill.
    ///
    /// Written as the flow it is: a slow identity provider between the email
    /// page, the password page and the challenge page.
    #[test]
    fn the_five_minutes_run_from_the_last_fill_and_not_the_first() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        let window = key(0x1234, 900, "chrome.exe");

        recall.remember(window.clone(), "item-9f2c", start);
        let page_two = start + Duration::from_secs(200);
        assert_eq!(recall.recall(&window, page_two), Recall::Offer("item-9f2c"));

        recall.remember(window.clone(), "item-9f2c", page_two);
        let page_three = page_two + Duration::from_secs(200);
        assert_eq!(recall.recall(&window, page_three), Recall::Offer("item-9f2c"));

        // Eight minutes and twenty seconds after the first fill, which a
        // first-fill reading would have expired two hundred seconds ago.
        assert!(page_three.duration_since(start) > RECALL_TTL);
    }

    /// A different window is not this window, however recently anything was
    /// filled.
    #[test]
    fn a_different_window_is_offered_nothing() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        recall.remember(key(0x1234, 900, "chrome.exe"), "item-9f2c", start);
        assert_eq!(
            recall.recall(&key(0x5678, 901, "notepad.exe"), start + Duration::from_secs(1)),
            Recall::Nothing(Miss::DifferentWindow)
        );
    }

    /// **A recycled handle is a different window**, and this is the test that
    /// says why the process id is in the key at all.
    ///
    /// The handle is byte-for-byte the one that was filled a minute ago; the
    /// window it now names belongs to a process that did not exist then. With
    /// `hwnd` alone this would answer `Offer`, and the user's next `2, Enter`
    /// would type a password into an application the record has nothing to do
    /// with.
    #[test]
    fn a_recycled_handle_in_another_process_is_not_the_same_window() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        recall.remember(key(0x1234, 900, "chrome.exe"), "item-9f2c", start);
        assert_eq!(
            recall.recall(&key(0x1234, 4242, "evil.exe"), start + Duration::from_secs(30)),
            Recall::Nothing(Miss::DifferentWindow)
        );
    }

    /// The same executable, a recycled handle, a new process: still refused.
    /// The `pid` is what refuses it, and this is the case `exe_name` alone
    /// could not catch.
    #[test]
    fn a_recycled_handle_in_a_restarted_process_is_not_the_same_window() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        recall.remember(key(0x1234, 900, "chrome.exe"), "item-9f2c", start);
        assert_eq!(
            recall.recall(&key(0x1234, 901, "chrome.exe"), start + Duration::from_secs(30)),
            Recall::Nothing(Miss::DifferentWindow)
        );
    }

    /// **Locking the vault empties it**, and empties it for real rather than
    /// making the read answer `Nothing` some other way.
    #[test]
    fn locking_the_vault_forgets_the_record() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        let window = key(0x1234, 900, "chrome.exe");
        recall.remember(window.clone(), "item-9f2c", start);
        assert_eq!(recall.recall(&window, start), Recall::Offer("item-9f2c"));

        recall.forget(Forgotten::VaultLocked);
        assert!(recall.is_empty());
        assert_eq!(recall.recall(&window, start), Recall::Nothing(Miss::Empty));
    }

    /// **Going back empties it too**, which is what stops the offer being
    /// something the user cannot get past: they said "not that one", and the
    /// next press opens the picker.
    #[test]
    fn going_back_forgets_the_record_so_the_offer_does_not_return() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        let window = key(0x1234, 900, "chrome.exe");
        recall.remember(window.clone(), "item-9f2c", start);

        recall.forget(Forgotten::UserWentBack);
        assert_eq!(
            recall.recall(&window, start + Duration::from_secs(1)),
            Recall::Nothing(Miss::Empty),
            "the record the user just rejected was offered again"
        );
    }

    /// **An excursion to another window costs nothing.** Step three of the
    /// motivating flow is a one-time code, which means alt-tabbing to fetch
    /// one; this module watches no foreground events, so coming back is the
    /// same window it always was.
    #[test]
    fn fetching_a_code_from_another_window_and_coming_back_keeps_the_offer() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        let browser = key(0x1234, 900, "chrome.exe");
        let authenticator = key(0x5678, 901, "Authenticator.exe");
        recall.remember(browser.clone(), "item-9f2c", start);

        // The excursion. Merely *asking* about another window changes nothing.
        assert_eq!(
            recall.recall(&authenticator, start + Duration::from_secs(10)),
            Recall::Nothing(Miss::DifferentWindow)
        );
        assert_eq!(
            recall.recall(&browser, start + Duration::from_secs(20)),
            Recall::Offer("item-9f2c")
        );
    }

    /// A fill at another window replaces the memory, which is the single
    /// slot's whole behaviour and the reason there is no map.
    #[test]
    fn a_fill_at_another_window_replaces_the_memory() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        let browser = key(0x1234, 900, "chrome.exe");
        let mail = key(0x5678, 901, "olk.exe");
        recall.remember(browser.clone(), "item-9f2c", start);
        recall.remember(mail.clone(), "item-aaaa", start + Duration::from_secs(5));

        assert_eq!(recall.recall(&mail, start + Duration::from_secs(6)), Recall::Offer("item-aaaa"));
        assert_eq!(
            recall.recall(&browser, start + Duration::from_secs(6)),
            Recall::Nothing(Miss::DifferentWindow)
        );
    }

    /// A different record at the same window replaces it too: the user picked
    /// something else here, so that is what "the previous entry" now means.
    #[test]
    fn a_different_record_at_the_same_window_replaces_the_memory() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        let window = key(0x1234, 900, "chrome.exe");
        recall.remember(window.clone(), "item-9f2c", start);
        recall.remember(window.clone(), "item-bbbb", start + Duration::from_secs(5));
        assert_eq!(
            recall.recall(&window, start + Duration::from_secs(6)),
            Recall::Offer("item-bbbb")
        );
    }

    /// An empty id is not a memory: there would be nothing to fill from, and
    /// an `Offer("")` would open a card about an item that cannot resolve.
    #[test]
    fn an_empty_item_id_is_not_remembered() {
        let start = Instant::now();
        let mut recall = FillRecall::new();
        recall.remember(key(0x1234, 900, "chrome.exe"), "", start);
        assert!(recall.is_empty());
    }

    /// Forgetting an already-empty memory is a no-op and not a panic; `main`
    /// clears on every lock, and most locks follow no fill at all.
    #[test]
    fn forgetting_nothing_is_harmless() {
        let mut recall = FillRecall::new();
        recall.forget(Forgotten::VaultLocked);
        recall.forget(Forgotten::UserWentBack);
        assert!(recall.is_empty());
    }

    /// **The running app's instance behaves like the value every other test
    /// here drives**, which is the whole claim the three wrappers make: they
    /// are an instance, not a second implementation.
    ///
    /// It uses a handle and a process id no other test names, and empties the
    /// slot at both ends, because this is process-wide state shared by every
    /// test in the binary -- which is the one cost of the static and is worth
    /// saying out loud rather than leaving for somebody to discover.
    #[test]
    fn the_production_instance_records_and_forgets_like_any_other() {
        let start = Instant::now();
        let mine = key(0x7ACE_0001, 65_001, "production-fixture.exe");
        forget_production(Forgotten::VaultLocked);

        remember_fill(mine.clone(), "item-prod", start);
        assert!(
            with_production(|recall| recall.recall(&mine, start).is_offer()),
            "the production instance did not record a fill"
        );

        forget_production(Forgotten::UserWentBack);
        assert!(
            with_production(|recall| recall.is_empty()),
            "the production instance did not forget"
        );
    }

    /// **The disk claim, checked rather than promised.** The module header
    /// argues at length that this state must not be written down; this reads
    /// the file's own production source back and fails if it ever grows a path,
    /// a file handle or a serialiser.
    ///
    /// `receive_history` and `scan_history` pin their own rules the same way,
    /// and for the reason one of them states outright: a promise in a comment
    /// is not a guard.
    #[test]
    fn nothing_in_this_module_reaches_the_filesystem() {
        // **Production only, and comments stripped.** Both halves matter: the
        // module header argues its case in prose that names `config_dir` and
        // the two history files, and the assertion below spells every needle
        // as a string literal, so a naive `contains` over the whole file would
        // find its own evidence and fail for the one reason that is not a
        // defect. The cut is `#[cfg(test)]`, which in this file is the line
        // this test's own module opens with.
        let source = include_str!("fill_recall.rs");
        let production = source.split(concat!("#[cfg(", "test)]")).next().unwrap_or("");
        let code: String = production
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for needle in [
            "std::fs",
            "File::",
            "config_dir",
            "PathBuf",
            "Serialize",
            "Deserialize",
            "to_json",
            "serde",
        ] {
            assert!(
                !code.contains(needle),
                "`{needle}` appears in this module's production code. This state is transient by \
                 design -- see the module header on why a file naming which item was filled into \
                 which window would be a tracking log of the user's day"
            );
        }
    }
}

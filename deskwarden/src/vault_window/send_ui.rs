//! The vault window's **Sends** screen: the Sends this account has published,
//! with a composer for making a new one.
//!
//! Steps 3, 4 and 5 of five, in that order, and the order was the point. A
//! Send is a public URL: the only outbound publishing action in this app. The
//! design built list, then delete, then create -- **revocation before
//! publication**, so that at no commit could this app make a link it could not
//! then show and take down.
//!
//! As of step 5 the screen lists every Send, copies any row's link, revokes a
//! row in two steps, and creates a TEXT Send from the composer in
//! [`draw_composer`]. File Sends are still made in the web vault only; see
//! below, and [`SCOPE_SUBTEXT`], which says so on screen.
//!
//! ## The one rule this file exists to keep
//!
//! **"You have no Sends" and "we could not ask" must never look the same.**
//! An empty list is a claim about the account; a failed fetch is a claim
//! about this app. [`SendPaneState`] is the type that keeps them apart, and
//! [`pane_state`] the single place the decision is made -- so no drawing
//! branch can reach an empty row list by way of an error. `SendError` already
//! carries [`crate::send::SendError::is_ambiguous`] for the same reason one
//! step earlier: a "could not check" must not render as a success.
//!
//! ## File Sends are SHOWN, never hidden
//!
//! This app cannot create a file Send (`bw send create` takes a file path,
//! and this window has no upload path -- which is why step 5's composer makes
//! text Sends and offers no type switch at all, rather than offering one that
//! refuses). It can list and revoke one. So a file Send made in the web vault appears here with a
//! tag saying what it is. Filtering them out would make "your Sends" a lie in
//! exactly the direction that matters: an unlisted public link is one the
//! user cannot revoke from here and does not know is there.
//!
//! ## The link is shown, always
//!
//! Every row carries a Copy link button. A link shown once, at creation, and
//! never again is a support ticket; the whole point of the list is that the
//! user can get back to a link they published. The create's own banner shows
//! the new link too, but the list is what makes it durable -- which is why a
//! successful create invalidates the list rather than only reporting.

use crate::local_time::LocalOffset;
use crate::send::{SendClock, SendError, SendSummary};
use crate::theme;
use eframe::egui::{self, CornerRadius};

/// The one line of subtext under the heading. **It is what makes the excluded
/// scope honest rather than hidden**: this screen shows every Send, including
/// the kinds this app will never be able to make, and the user is told where
/// the rest of the feature lives instead of discovering its absence.
pub const SCOPE_SUBTEXT: &str =
    "Deskwarden creates and deletes text Sends. Use the web vault for file Sends and for editing.";

/// The tag drawn on a Send this app could not have created.
pub const FILE_TAG: &str = "FILE";

/// What the pane says when the account genuinely has no Sends. A **claim**,
/// and only reachable from a fetch that succeeded -- see [`pane_state`].
pub const EMPTY_HEADLINE: &str = "You have no Sends.";
pub const EMPTY_DETAIL: &str = "Nothing is published from this account.";

/// What the pane says when the fetch failed. Deliberately shares not one word
/// with [`EMPTY_HEADLINE`]: these two blocks are the pair that must never be
/// mistaken for each other, and the tests below assert the separation over
/// the glyphs actually painted rather than over the enum.
pub const FAILED_HEADLINE: &str = "Your Sends could not be listed.";

/// The extra sentence an **ambiguous** failure gets, on top of the error's own
/// message. `SendError::is_ambiguous` exists because "could not check" must
/// never render as success, and the strongest form of that untruth on this
/// screen is a blank pane that reads as "you have none".
pub const AMBIGUOUS_DETAIL: &str =
    "Deskwarden could not check, so this is not a list of nothing -- you may still have Sends.";

/// What the pane says while the CLI has not answered yet.
pub const LOADING_LABEL: &str = "Asking Bitwarden for your Sends\u{2026}";

/// One drawn row: everything the pane paints for one Send, and nothing else.
///
/// Built by [`row_from`] out of a [`SendSummary`], so the wording of the
/// expiry -- the only computed field -- is decided once, in a pure function,
/// rather than inside the drawing closure where no test in this crate could
/// reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRow {
    /// The Send's id. Not painted; carried because step 4's revoke needs it
    /// and a row that cannot name itself is a row that cannot be deleted.
    pub id: String,
    pub name: String,
    /// The expiry **in words**, already relative to a clock. See
    /// [`expiry_words`].
    pub expiry: String,
    /// A Send this app could not have made. Shown, never filtered.
    pub is_file: bool,
    /// The public URL. What Copy link puts on the clipboard.
    pub access_url: String,
}

/// What the pane shows this frame.
///
/// Four states and not three: `Empty` and `Failed` are separate variants
/// precisely so that no drawing branch can arrive at "draw no rows" from an
/// error. A single `Vec<SendRow>` plus an `Option<String>` error would make
/// the empty-vs-failed confusion a matter of remembering to check the second
/// field in the right order, which is the class of bug this window keeps
/// having to un-write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendPaneState {
    /// Nothing has been asked yet, or the answer has not come back.
    /// **Not `Empty`** -- an unanswered question is not an answer.
    Loading,
    /// The CLI answered, and the answer was "none".
    Empty,
    /// The CLI answered with these.
    Rows(Vec<SendRow>),
    /// The CLI could not be asked, or could not be understood.
    Failed {
        /// `SendError::user_message`'s own wording. Not re-written here: a
        /// second copy is a second thing to keep true.
        message: String,
        /// `SendError::is_ambiguous`. Adds [`AMBIGUOUS_DETAIL`].
        ambiguous: bool,
    },
}

impl SendPaneState {
    /// Whether this state is a **claim about the account** rather than about
    /// this app's ability to ask. Only `Empty` and `Rows` are.
    ///
    /// Exists so the badge rule and the pane cannot disagree about what a
    /// failure means -- see [`SendFetch::badge_count`].
    pub fn is_an_answer(&self) -> bool {
        matches!(self, SendPaneState::Empty | SendPaneState::Rows(_))
    }
}

/// What the window holds between frames for this screen.
///
/// Modelled on `vault_window::AuxList` -- the same three questions, one
/// answer each -- with one deliberate difference: the failure is stored
/// *inside* `result` rather than beside it. `AuxList` keeps `items` and
/// `error` as separate `Option`s, and that shape is exactly the one where a
/// caller can read the first and forget the second. Here there is one
/// `Option<Result<..>>`, so "not asked" and "asked and failed" are different
/// values of the same field and every reader must pass through both.
#[derive(Default)]
pub struct SendFetch {
    /// `None` means the question has not been answered in this visit.
    pub result: Option<Result<Vec<SendSummary>, SendError>>,
    /// A background thread is running. Cleared by the drain, **before** any
    /// currency check, for the reason the TOTP and aux drains give: gating
    /// the clear on currency is how a flag like this latches forever.
    pub in_flight: bool,
    /// Which question the current `result` is an answer to.
    ///
    /// **This is what makes `invalidate` real.** Bumped by every invalidate,
    /// carried by every spawn, and compared by [`apply_answer`]. Without it,
    /// a fetch started on one visit and landing after the user has left is
    /// written into `result` anyway -- so the next visit finds `result`
    /// already `Some`, `wants_fetch` false, and shows the *previous* visit's
    /// list with no refetch. That is precisely the stale list the refetch
    /// policy exists to prevent, arrived at through the code that was
    /// supposed to prevent it. See
    /// `a_late_answer_from_the_visit_before_is_dropped_and_the_next_visit_asks_again`.
    ///
    /// Private, and there is no setter: [`invalidate`] is the only thing that
    /// may move it, so "the tag changed" and "the answer is stale" cannot
    /// drift apart.
    ///
    /// [`apply_answer`]: SendFetch::apply_answer
    /// [`invalidate`]: SendFetch::invalidate
    generation: u64,
    /// Whether the Sends screen was up on the PREVIOUS frame.
    ///
    /// **Owned here rather than beside the fetch in the frame closure**, and
    /// private, so that the refetch policy cannot be separated from the state
    /// it acts on. It used to be a `let mut was_on_sends` local, with the
    /// whole decision written out as an `if` in the render closure -- and
    /// logic inside that closure is logic no test in this crate can run. The
    /// measured consequence: replacing the `if`'s body with a log line left
    /// the entire suite green while leaving Sends silently stopped dropping
    /// the list, so a returning user saw the previous visit's Copy links
    /// forever. The decision is [`note_screen`] now, and it is tested
    /// directly.
    ///
    /// [`note_screen`]: SendFetch::note_screen
    was_selected: bool,
}

impl SendFetch {
    /// Whether the window should start a fetch this frame.
    ///
    /// A pure predicate rather than three conditions inside the render
    /// closure, for the reason `AuxList::wants_fetch` gives: every failure
    /// mode here is "reachable but wrong" rather than "does not compile". A
    /// missing `in_flight` check spawns a `bw` child per frame -- sixty
    /// processes a second -- and a missing `result` check refetches a list it
    /// already holds on every frame the screen is open.
    ///
    /// A **failed** fetch is not retried automatically: `result` is `Some`,
    /// so this is false. The retry is the pane's own Try again button, or
    /// leaving the screen and coming back. Retrying a dead CLI at 60Hz is the
    /// same defect in a different costume.
    pub fn wants_fetch(&self, selected: bool) -> bool {
        selected && self.result.is_none() && !self.in_flight
    }

    /// The tag a fetch started **now** should carry back with its answer.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Forget the answer, so the next frame asks again.
    ///
    /// `in_flight` is deliberately NOT cleared -- a thread is still running,
    /// and clearing it would let a second one start. **That is only safe
    /// because the generation moves at the same instant**: the running
    /// thread's answer is now tagged with a generation this value no longer
    /// holds, so [`apply_answer`] drops it. `AuxList::invalidate` says the
    /// same thing about its own generation check; this line without the bump
    /// is the same comment attached to nothing.
    ///
    /// [`apply_answer`]: SendFetch::apply_answer
    pub fn invalidate(&mut self) {
        self.result = None;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Take -- or drop -- an answer that a background fetch has reported.
    ///
    /// **The whole drain, as one pure function.** The window's drain is a
    /// `try_recv` inside the frame closure, which no test in this crate can
    /// run; keeping the decision here rather than there is what makes the
    /// late-answer rule testable at all instead of pinned in source.
    ///
    /// `in_flight` is cleared whatever the tag says: the thread that set it
    /// has finished, and a currency-gated clear is how a flag like this
    /// latches forever.
    ///
    /// Returns whether the answer was kept, for the caller's log line.
    pub fn apply_answer(
        &mut self,
        tag: u64,
        result: Result<Vec<SendSummary>, SendError>,
    ) -> bool {
        self.in_flight = false;
        if tag != self.generation {
            return false;
        }
        self.result = Some(result);
        true
    }

    /// **The refetch policy, applied.** Tell the fetch which screen the frame
    /// is drawing; if the user has just left Sends, the list it fetched is
    /// dropped so the next visit asks again.
    ///
    /// The whole of the decision, so that the frame closure carries none of
    /// it: the rule ([`should_invalidate_on_leave`]), the action
    /// ([`invalidate`]) and the remembering are one call with one argument.
    /// A frame that calls this has the policy; a frame that does not has no
    /// half of it left behind to look correct. That is the point -- the
    /// previous shape spelled all three out in the render closure, where an
    /// edit that kept the `if` and dropped the `invalidate` was invisible to
    /// every test in this file.
    ///
    /// Must be called BEFORE [`wants_fetch`] in the same frame: the whole
    /// value of dropping the list is that the gate below sees `None` on the
    /// frame the user returns.
    ///
    /// [`invalidate`]: SendFetch::invalidate
    /// [`wants_fetch`]: SendFetch::wants_fetch
    pub fn note_screen(&mut self, now_selected: bool) {
        if should_invalidate_on_leave(self.was_selected, now_selected) {
            self.invalidate();
        }
        self.was_selected = now_selected;
    }

    /// What the sidebar's Sends badge should read, or `None` for "unknown".
    ///
    /// **A failure is `None`, not `Some(0)`.** This is the same rule as the
    /// pane's, applied to the eight pixels most likely to be read at a
    /// glance: a `0` beside Sends is a claim that nothing is published, and a
    /// fetch that failed does not know that. `sidebar::badge_text` already
    /// draws `None` as an en dash for exactly this reason.
    pub fn badge_count(&self) -> Option<usize> {
        match self.result.as_ref()? {
            Ok(sends) => Some(sends.len()),
            Err(_) => None,
        }
    }
}

/// Whether leaving the Sends screen should drop the list it fetched.
///
/// **The refetch policy, as one testable rule.** The list is fetched once per
/// visit: entering the screen asks, leaving it forgets, and the pane's Try
/// again button asks again without leaving.
///
/// Why not "once per window" (the `AuxList` policy): a Send is a public link
/// with a server-side lifetime, and it can be deleted or expire without this
/// app doing anything. A stale Sends list offers Copy link for a URL that
/// 404s and -- once step 4 lands -- a Revoke button for something already
/// gone. That is a correctness problem, not a cosmetic one.
///
/// Why not a timer: a timer spawns a `bw` child on a schedule nobody asked
/// for, including while the window sits idle in the tray, and the staleness it
/// removes is exactly the staleness a deliberate navigation already removes.
/// The cost of this rule is one CLI spawn per visit to one screen.
pub fn should_invalidate_on_leave(was_selected: bool, now_selected: bool) -> bool {
    was_selected && !now_selected
}

/// The **single** place a fetch outcome becomes something drawable.
///
/// `result` is `None` for "not answered yet" -- whether or not a thread has
/// been started -- and that is why `in_flight` is not a parameter: a frame
/// before the spawn and a frame during it are the same thing to the user, and
/// giving them different pane states would mean an "empty" flash on the first
/// frame of every visit.
pub fn pane_state(
    result: Option<&Result<Vec<SendSummary>, SendError>>,
    now: &dyn SendClock,
) -> SendPaneState {
    match result {
        None => SendPaneState::Loading,
        Some(Err(e)) => SendPaneState::Failed {
            message: e.user_message().to_string(),
            ambiguous: e.is_ambiguous(),
        },
        Some(Ok(sends)) if sends.is_empty() => SendPaneState::Empty,
        Some(Ok(sends)) => SendPaneState::Rows(rows_from(sends, now)),
    }
}

/// Every summary becomes a row, **including the file ones**. There is no
/// filter here and there must not be one; see the module docs.
pub fn rows_from(sends: &[SendSummary], now: &dyn SendClock) -> Vec<SendRow> {
    sends.iter().map(|send| row_from(send, now)).collect()
}

/// One summary to one row.
///
/// A Send with no name is drawn as `(no name)` rather than as an empty
/// string: `bw` allows it, and a row with nothing where the name goes reads
/// as a rendering failure rather than as the Send it is.
pub fn row_from(send: &SendSummary, now: &dyn SendClock) -> SendRow {
    SendRow {
        id: send.id.clone(),
        name: if send.name.trim().is_empty() {
            "(no name)".to_string()
        } else {
            send.name.clone()
        },
        expiry: expiry_words(&send.deletion_date, now),
        is_file: send.is_file,
        access_url: send.access_url.clone(),
    }
}

/// Milliseconds in a day. Same constant `send.rs` uses, spelled here because
/// that one is private to it.
const MILLIS_PER_DAY: i64 = 86_400_000;

/// The expiry of a Send **in words**, relative to `now`.
///
/// `bw send list` reports `deletionDate` as `2026-08-18T00:43:17.148Z` -- an
/// absolute UTC instant. A row that showed it verbatim would be asking the
/// user to do date arithmetic against a timestamp with milliseconds in it, on
/// a screen whose whole subject is "how long is this link alive".
///
/// The unparseable case is its own wording and **not** "Expired": this app
/// not understanding a date is not the same as the link being dead, and
/// guessing the safe-sounding one of those two is how a live public link
/// comes to be ignored.
pub fn expiry_words(deletion_date: &str, now: &dyn SendClock) -> String {
    let Some(at) = parse_iso_utc_millis(deletion_date) else {
        return "Expiry unknown".to_string();
    };
    let remaining = at - now.now_unix_millis();
    if remaining <= 0 {
        return "Expired".to_string();
    }
    let days = remaining / MILLIS_PER_DAY;
    match days {
        0 => "Expires today".to_string(),
        1 => "Expires tomorrow".to_string(),
        d => format!("Expires in {d} days"),
    }
}

/// `2026-08-18T00:43:17.148Z` to milliseconds since the Unix epoch, or `None`
/// if it is not that shape.
///
/// Hand-written rather than a new dependency, for the reason `send.rs`'s
/// base64 is: this is the only date this app parses, the format is fixed by
/// the CLI that emits it, and `chrono` is a large surface to add for one
/// field. The fractional part is optional and ignored beyond being skipped --
/// a Send's lifetime is measured in days, and no wording here can turn on a
/// millisecond.
pub fn parse_iso_utc_millis(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    // The fixed prefix is exactly `YYYY-MM-DDTHH:MM:SS`, 19 bytes.
    if bytes.len() < 19 {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { text.get(from..to)?.parse::<i64>().ok() };
    let sep = |at: usize, want: u8| -> Option<()> { (bytes[at] == want).then_some(()) };
    sep(4, b'-')?;
    sep(7, b'-')?;
    sep(10, b'T')?;
    sep(13, b':')?;
    sep(16, b':')?;
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let minute = num(14, 16)?;
    let second = num(17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1_000)
}

/// Days since 1970-01-01 for a proleptic-Gregorian civil date. Howard
/// Hinnant's `days_from_civil`, which is the exact inverse of the
/// `civil_from_days` `send.rs` already carries for the other direction.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// What a frame of this pane reports back to `vault_window::run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendUiAction {
    None,
    /// Put this **row's own** URL on the clipboard. The URL travels in the
    /// action rather than an index, so there is no second lookup on the far
    /// side that could resolve to a different row than the one clicked.
    CopyLink(String),
    /// Ask again. Both the Try again button on a failure and the Refresh
    /// button in the header report this.
    Refresh,
    /// The inline notice band was clicked away.
    DismissNotice,
    /// **Step one of two.** The Delete button on a row was pressed. Nothing
    /// is destroyed by this: it asks the window to put that row -- and only
    /// that row -- into the confirming state, which is what redraws it with
    /// the confirmation.
    ///
    /// The id travels in the action for [`CopyLink`](Self::CopyLink)'s
    /// reason: it is read off the row that was clicked and off nothing else,
    /// so there is no second lookup on the far side that could resolve to a
    /// different Send. For a destructive operation that is not a nicety --
    /// revoking the wrong link is not undoable.
    AskDelete(String),
    /// **Step two of two.** The confirmation's own destructive button was
    /// pressed, on the row named here.
    ///
    /// Carries the name as well as the id, again off the clicked row, because
    /// the report the user is shown afterwards has to say WHICH Send was
    /// revoked -- and by then the list it came from has been thrown away and
    /// refetched.
    ConfirmDelete { id: String, name: String },
    /// The confirmation was declined. Its button occupies the pixels the
    /// Delete button was drawn in -- see [`draw_row`].
    CancelDelete,
    /// The header's New Send button was pressed: put the composer on screen.
    OpenComposer,
    /// The composer's Discard button was pressed: take it off screen and wipe
    /// the draft.
    CancelComposer,
    /// The composer's Create button was pressed.
    ///
    /// **It carries nothing, and that is deliberate.** Every other variant
    /// here carries the value it was read off, because the alternative is a
    /// second lookup on the far side that can resolve to a different row.
    /// The draft is different: it is a secret, and this enum derives `Debug`,
    /// `Clone`, `PartialEq` and `Eq`. A `SubmitSend(SendPlan)` would put the
    /// text of the Send and its share password into every `{:?}` any future
    /// call site writes of an action -- which is exactly the leak `SendPlan`'s
    /// hand-written `Debug` and `SendInvocation`'s exist to refuse. There is
    /// no second-lookup hazard to trade against, either: the draft lives in
    /// exactly one place, the window's own `SendComposer`, and
    /// `vault_window::apply_send_action` is handed that same `&mut` -- so
    /// there is only ever one plan and no way to name another.
    SubmitSend,
}

/// **A [`SendUiAction`] that this pane produced, which cannot be thrown away
/// in silence.**
///
/// Every earlier pin over the seam between [`draw_send_pane`] and
/// `vault_window::apply_send_action` was a statement about the CALL -- its
/// spelling, its arguments, its brace depth -- and each was defeated by a
/// shadow written one layer outside what it pinned:
///
/// ```ignore
/// let send_action = { drop(send_action); send_ui::SendUiAction::None };
/// let send_action = if send_delete.report.is_some() { None } else { send_action };
/// let send_action = if items.is_empty() { send_action } else { None };
/// ```
///
/// The frame click tests answer those by pressing the real controls, but they
/// can only answer a shadow gated on a state the FIXTURE CONSTRUCTS. The
/// third one above was measured green through a click test whose doc claimed
/// it "covers every state a shadow can plausibly be gated on": the harness
/// loads a vault, `items` is in scope at the call, and the harness's vault
/// was empty -- so every real user, who has at least one item, got a wholly
/// dead Sends pane. Enumerating states loses that race by construction,
/// because the mutant picks its gate AFTER reading the fixture.
///
/// It is a linear value: the action lives inside it,
/// [`into_action`](Self::into_action) is the ONLY way out, and that method
/// consumes `self`. A verdict that reaches its `Drop` still holding an
/// action was **abandoned**, and the drop is counted in
/// [`abandoned_in_this_thread`].
///
/// **THE CLAIM THIS DOC USED TO MAKE HERE WAS FALSE, AND WAS MEASURED
/// FALSE.** It said the count "does not depend on which states a fixture
/// happens to build". A drop is only COUNTED when a test executes the
/// discarding branch, so a gated shadow still needs the fixture to build
/// its state -- the requirement was weakened from "build the state AND
/// assert on the action" to "build the state", not removed. The evidence
/// was already in the numbers that shipped the claim: the `items` shadow
/// died in 4 tests, all in the populated fixture, and its mirror died in
/// exactly 1, in the empty one. A state-independent hold would have killed
/// both in every frame test that draws this pane. A sixth state was then
/// found green against the whole suite -- `search`, the vault window's own
/// search box, live at the pane and never made non-empty by any test before
/// reaching the Sends screen.
///
/// **So the SITE was deleted, which is what actually closed the class.**
/// `vault_window::run` has no binding between the pane and
/// `apply_send_action`: the panel closure returns the verdict and that
/// expression is written directly as the applier's first argument, and the
/// pane's model is written inline at its one use rather than bound above
/// the panel. There is no name to shadow, so there is no frame state left
/// to gate on. See
/// `send_delete_wiring::the_applier_takes_the_panel_with_no_binding_between`.
///
/// **PRIVACY IS THE LOAD-BEARING PART OF THIS TYPE, NOT THE DROP COUNTER.**
/// The tuple field and [`seal`](Self::seal) are private to this module, so
/// `vault_window` cannot mint a verdict carrying a different action; the
/// most it can express is dropping a real one for
/// [`no_sends_screen_this_frame`]. The counter was measured contributing
/// nothing on its own: a shadow that consumed the verdict linearly
/// (`into_action()` then `.filter(..)` in the argument list) left the count
/// at zero, fired no `debug_assert`, and was noticed by NO behavioural test
/// -- only by a source-text equality over the call. It is kept because it
/// does catch the drop-and-mint shape, which was measured, and it is
/// written down here with the reach it actually has.
///
/// **IT IS A `debug_assert`, SO IT HOLDS NOTHING IN A RELEASE BUILD.** The
/// only guard in `vault_window::run` is `debug_assert_eq!(
/// abandoned_in_this_thread(), 0, ..)`; the two hard `assert_eq!`s are
/// `#[cfg(test)]`. Under `--release`, which is what CI ships, an abandoned
/// verdict is not detected at all. That is deliberate -- in a paint loop
/// the right answer is a dead pane rather than a dead process -- but it
/// means the count is a TEST-TIME hold. What holds this seam in a shipping
/// build is the shape of the code (no binding, private constructor) and the
/// source pins that keep that shape.
///
/// **Residual, recorded plainly.** `std::mem::forget` -- or leaking the
/// verdict into something that outlives the frame -- suppresses the `Drop`
/// and so suppresses the count. That is not a gate a shadow can hide behind;
/// it is a whole extra statement naming a leak primitive in a paint loop. But
/// it is the one shape this mechanism does not see, which is why the frame
/// click tests are kept alongside it rather than replaced by it.
#[derive(Debug)]
#[must_use = "a Sends verdict must be applied with `into_action`, not dropped"]
pub struct SendUiVerdict(Option<SendUiAction>);

thread_local! {
    /// How many verdicts this thread has dropped while they still held an
    /// action. Thread-local rather than global, so two tests running in
    /// parallel cannot read each other's counts.
    static ABANDONED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many [`SendUiVerdict`]s this thread has dropped without applying.
///
/// Zero is the only correct value anywhere a frame has finished with its
/// pane. See [`SendUiVerdict`].
pub fn abandoned_in_this_thread() -> usize {
    ABANDONED.with(|c| c.get())
}

/// A verdict for a frame on which **the Sends screen was not drawn**.
///
/// This is the only verdict `vault_window` can obtain without calling
/// [`draw_send_pane`], and it carries [`SendUiAction::None`] by
/// construction -- there is no parameter, so it cannot be made to carry
/// anything else. [`SendUiVerdict::seal`] and the field stay private, so
/// substituting an action for the one the pane reported is not expressible
/// outside this module; the most a caller can do is DROP the real verdict
/// in favour of this one, which is what [`abandoned_in_this_thread`]
/// counts. See [`SendUiVerdict`].
pub fn no_sends_screen_this_frame() -> SendUiVerdict {
    SendUiVerdict::seal(SendUiAction::None)
}

impl SendUiVerdict {
    /// Mints a verdict. Private on purpose: only this module decides what the
    /// Sends pane reported.
    fn seal(action: SendUiAction) -> Self {
        Self(Some(action))
    }

    /// The action, consuming the verdict. The only way out.
    pub fn into_action(mut self) -> SendUiAction {
        self.0
            .take()
            .expect("a verdict holds its action until exactly one `into_action`")
    }
}

impl Drop for SendUiVerdict {
    fn drop(&mut self) {
        // Deliberately no panic: a `Drop` that panics during an unwind aborts
        // the process, and this runs in a paint loop. Counting is enough --
        // the frame and the tests read the count.
        if self.0.is_some() {
            ABANDONED.with(|c| c.set(c.get().saturating_add(1)));
        }
    }
}

/// What the window knows about a delete, as the pane needs it in order to
/// draw one.
///
/// Two `Option<&str>` rather than one enum because they answer two
/// independent questions -- "which row is asking?" and "which row is already
/// being deleted?" -- and the window can hold both at once when the user asks
/// about a second row while a first is still in flight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SendDeleteView<'a> {
    /// The id of the row whose confirmation is showing, if any.
    pub confirming: Option<&'a str>,
    /// The id of the row a `bw send delete` is running for, if any.
    pub in_flight: Option<&'a str>,
}

/// Row geometry. Two lines of text and a button, so the row is tall enough
/// for the expiry to sit under the name rather than compete with it for the
/// width the button also wants.
const ROW_HEIGHT: f32 = 54.0;
const ROW_PAD_X: f32 = 14.0;
const COPY_BUTTON_WIDTH: f32 = 92.0;
const COPY_BUTTON_HEIGHT: f32 = 26.0;
/// The Delete button, and -- exactly -- the Cancel button that replaces it.
/// See [`draw_row`] for why those two are the same rectangle.
const DELETE_BUTTON_WIDTH: f32 = 68.0;
/// The confirmation's destructive button. Wider, because it is labelled with
/// the whole of what it does rather than with one verb.
const CONFIRM_BUTTON_WIDTH: f32 = 128.0;
/// The gap between two controls in a row.
const BUTTON_GAP: f32 = 8.0;
/// The row's first, non-destructive button.
pub const DELETE_LABEL: &str = "Delete";
/// The confirmation's destructive button. **Deliberately not "Delete"**: the
/// two steps must not be the same word, or the second click is muscle memory
/// rather than a decision.
pub const CONFIRM_LABEL: &str = "Delete permanently";
/// The confirmation's way out, drawn in the Delete button's own rectangle.
pub const CANCEL_LABEL: &str = "Cancel";
/// The question the row asks while it is confirming, in place of the expiry.
pub const CONFIRM_PROMPT: &str = "Revoke this link for good? It cannot be undone.";
/// What a row says while its `bw send delete` is running. It has no buttons
/// at all in that state, so a second click cannot start a second child.
pub const DELETING_LABEL: &str = "Revoking\u{2026}";
/// The gap between the name and its FILE tag, and between the tag's text and
/// its own outline.
const TAG_PAD_X: f32 = 6.0;

/// The heading the Sends PANE paints at the top of its own screen.
///
/// Named because the matrix test counts it: the pane's heading and
/// `sidebar::SENDS_ROW_LABEL` are the same string, and "two occurrences"
/// is how that test tells "the pane painted" from "only the sidebar row
/// that leads to it painted". While this was a bare literal, rewording it
/// silently turned that step into "the sidebar row exists" -- which is
/// true in every state, including the ones where the window body is
/// blank. The two constants are pinned equal by the matrix itself.
pub const SENDS_HEADING: &str = "Sends";

/// Draws the whole Sends screen and reports what was clicked.
///
/// `notice` is the message the window's single inline band is showing this
/// frame, already chosen by `vault_window::inline_notice` -- this function
/// does not decide which of the window's messages wins, it only paints the
/// one it is handed. That is the same split every other pane in this window
/// uses, and it is why a Sends failure is a `NoticeSource` rather than a
/// widget of its own.
///
/// **Eight parameters, and the eighth is the reason for this attribute.**
/// `zone` joined `now` when the composer's expiry line stopped naming the UTC
/// day and started naming the user's own. The alternative to passing it is
/// reading the machine's timezone inside the draw, which is exactly what
/// `now` is a parameter to avoid: a paint test could then assert the shape of
/// that sentence and never its content, and would say something different on
/// a runner in another timezone. Bundling the two into a struct would move
/// the argument count rather than reduce what this function is handed.
#[allow(clippy::too_many_arguments)]
pub fn draw_send_pane(
    ui: &mut egui::Ui,
    state: &SendPaneState,
    notice: Option<&str>,
    delete: SendDeleteView<'_>,
    // **`&mut`, and the one place the draft lives.** The pane owns no state
    // of its own -- every other pane in this window is the same -- so the
    // half-typed name, body and share password are the window's, survive a
    // trip to another screen, and are wiped by exactly one thing: dropping
    // this value. See [`SendComposer`].
    composer: &mut SendComposer,
    // Whether a `bw send create` started from that draft is still running.
    creating: bool,
    // The clock the composer's expiry line is worded against. A parameter for
    // `crate::send`'s reason: nothing that decides what a date SAYS may read
    // the wall clock for itself, or the paint tests could only assert the
    // shape of the sentence and never its content.
    now: &dyn SendClock,
    // The machine's offset from UTC, for the composer's expiry line. A
    // parameter beside `now`, and for exactly the same reason: the date under
    // the lifetime picker is the user's OWN day, and a paint test that read
    // the offset off the machine running it would assert a different sentence
    // on a runner in another timezone -- or on the same runner in March.
    zone: &dyn LocalOffset,
) -> SendUiVerdict {
    let mut action = SendUiAction::None;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(SENDS_HEADING)
                .size(17.0)
                .color(theme::INK)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Present in EVERY state, including `Failed`. A screen whose only
            // way back from an error is to navigate away and return is a
            // screen that tells the user the error is permanent.
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Refresh").size(12.0).color(theme::INK))
                        .min_size(egui::vec2(76.0, COPY_BUTTON_HEIGHT)),
                )
                .clicked()
            {
                action = SendUiAction::Refresh;
            }
            // **Hidden while the composer is open**, because the form is
            // already the thing on screen and a second way to "open" it would
            // either do nothing or throw the draft away. It is drawn after
            // Refresh in a right-to-left layout, so it sits to Refresh's
            // left.
            if !composer.open {
                ui.add_space(BUTTON_GAP);
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(NEW_SEND_LABEL).size(12.0).color(theme::INK),
                        )
                        .min_size(egui::vec2(88.0, COPY_BUTTON_HEIGHT)),
                    )
                    .clicked()
                {
                    action = SendUiAction::OpenComposer;
                }
            }
        });
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(SCOPE_SUBTEXT)
            .size(12.0)
            .color(theme::TEXT_FAINT),
    );
    ui.add_space(12.0);

    // **The same sentence is never printed twice.** A failed fetch reaches
    // this function through both doors: `vault_window` turns it into the
    // window's inline notice (`NoticeSource::Sends`), and `pane_state` turns
    // the very same `SendError` into `Failed { message }` below -- both from
    // `SendError::user_message`. Handed both, the pane used to paint the
    // identical line in the band and again under the headline, which reads
    // as two failures.
    //
    // The pane's own rendering wins, because it is the richer one: it has
    // the headline, the "could not check" line for an ambiguous failure, and
    // Try again. A notice that is *not* the failure being drawn (a move or
    // generate error arriving while the list is up) is still shown.
    let notice = match (notice, state) {
        (Some(n), SendPaneState::Failed { message, .. }) if n == message => None,
        (n, _) => n,
    };
    if let Some(message) = notice {
        if draw_notice_band(ui, message) {
            action = SendUiAction::DismissNotice;
        }
        ui.add_space(12.0);
    }

    // **Above the list and not instead of it.** The composer is a card on
    // this screen rather than a screen of its own, so every control the Sends
    // pane has -- Refresh, Copy link, Delete, the confirmation -- keeps
    // working while a draft is open. A form that took the pane whole would
    // make "there is a draft open" a state in which the rest of the feature
    // silently does not exist, which is the shape this window keeps having to
    // un-write.
    if composer.open {
        if let Some(reported) = draw_composer(ui, composer, creating, now, zone) {
            action = reported;
        }
        ui.add_space(12.0);
    }

    match state {
        SendPaneState::Loading => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0).color(theme::BLUE));
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(LOADING_LABEL)
                        .size(13.0)
                        .color(theme::TEXT_FAINT),
                );
            });
        }
        SendPaneState::Empty => {
            ui.label(
                egui::RichText::new(EMPTY_HEADLINE)
                    .size(14.0)
                    .color(theme::INK),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(EMPTY_DETAIL)
                    .size(12.0)
                    .color(theme::TEXT_FAINT),
            );
        }
        SendPaneState::Failed { message, ambiguous } => {
            // A failure is drawn in the ERROR colour with its own headline,
            // and it draws NO row area at all. The two things this pane must
            // never do are draw an empty list here and reuse the empty
            // state's words; both are asserted over painted glyphs below.
            ui.label(
                egui::RichText::new(FAILED_HEADLINE)
                    .size(14.0)
                    .color(theme::ERROR)
                    .strong(),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(message.as_str())
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
            );
            if *ambiguous {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(AMBIGUOUS_DETAIL)
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );
            }
            ui.add_space(10.0);
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("Try again").size(12.0).color(theme::INK),
                    )
                    .min_size(egui::vec2(88.0, COPY_BUTTON_HEIGHT)),
                )
                .clicked()
            {
                action = SendUiAction::Refresh;
            }
        }
        SendPaneState::Rows(rows) => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for row in rows {
                        if let Some(clicked) = draw_row(ui, row, delete) {
                            action = clicked;
                        }
                    }
                });
        }
    }

    SendUiVerdict::seal(action)
}

/// One row. Returns the action **this row** reported, if any.
///
/// Everything the action carries is read off the `row` this call was handed
/// and off nothing else -- no index into the list, no lookup by name. The
/// wrong-row copy is the classic form of this bug and the only structural
/// defence against it is not to have a second way of naming the row; for the
/// delete, where the mistake is not undoable, that matters more and not less.
///
/// **The row has three states and they are mutually exclusive**, decided
/// here and in one place:
///
///  * **Revoking.** `delete.in_flight` names this row: no buttons at all are
///    put into the layout, so a second click cannot start a second
///    `bw send delete` for a Send that is already being revoked.
///  * **Confirming.** `delete.confirming` names this row: the destructive
///    button appears, and [`CANCEL_LABEL`] takes over the Delete button's
///    **exact rectangle**. That is the mis-click defence and it is
///    deliberate: a user who double-clicks Delete, or who clicks it twice
///    because the first click seemed not to register, lands the second click
///    on Cancel. The destructive button is a different size, a different
///    label and a different position, so reaching it is a decision.
///  * **Idle.** Copy link at the row's right edge, Delete beside it.
fn draw_row(
    ui: &mut egui::Ui,
    row: &SendRow,
    delete: SendDeleteView<'_>,
) -> Option<SendUiAction> {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(8), theme::CARD);

    let name_font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
    let small_font = egui::FontId::new(11.0, egui::FontFamily::Proportional);

    let name_rect = painter.text(
        egui::pos2(rect.left() + ROW_PAD_X, rect.top() + 12.0),
        egui::Align2::LEFT_TOP,
        &row.name,
        name_font,
        theme::INK,
    );

    if row.is_file {
        // Drawn beside the name, not instead of it, and never as a reason to
        // omit the row.
        let tag_pos = egui::pos2(name_rect.right() + 10.0, name_rect.center().y);
        let tag_rect = painter.text(
            egui::pos2(tag_pos.x + TAG_PAD_X, tag_pos.y),
            egui::Align2::LEFT_CENTER,
            FILE_TAG,
            small_font.clone(),
            theme::TEXT_MUTED,
        );
        painter.rect_stroke(
            tag_rect.expand2(egui::vec2(TAG_PAD_X, 2.0)),
            CornerRadius::same(4),
            egui::Stroke::new(1.0, theme::HAIRLINE),
            egui::StrokeKind::Outside,
        );
    }

    // The three states, decided once. `is` compares the id this row carries
    // with the id the window is holding, so a confirmation shown for one row
    // cannot be answered by a click on another.
    let revoking = delete.in_flight == Some(row.id.as_str());
    let confirming = !revoking && delete.confirming == Some(row.id.as_str());

    // The second line of the row: the expiry normally, the question while
    // confirming, and the progress word while the child runs. **The expiry is
    // replaced rather than joined**, because a row that says both "Expires in
    // 7 days" and "Revoke this link for good?" is a row whose subject is
    // ambiguous at the moment it matters most.
    let (second_line, second_colour) = if revoking {
        (DELETING_LABEL, theme::TEXT_MUTED)
    } else if confirming {
        (CONFIRM_PROMPT, theme::ERROR)
    } else {
        (row.expiry.as_str(), theme::TEXT_FAINT)
    };
    painter.text(
        egui::pos2(rect.left() + ROW_PAD_X, rect.bottom() - 12.0),
        egui::Align2::LEFT_BOTTOM,
        second_line,
        small_font,
        second_colour,
    );

    // Right-aligned against the row's own right edge, so the button stays
    // reachable at every window width the OS will allow -- see
    // `settings::MIN_VAULT_WINDOW_SIZE` and the geometry test below. Placed
    // with `ui.put` into an explicit rect rather than laid out by a nested
    // horizontal: a nested layout is what has repeatedly pushed a control off
    // the pane in this window, and a control drawn at zero size passes both
    // the presence and the in-pane assertions.
    let slot = |right: f32, width: f32| {
        egui::Rect::from_min_size(
            egui::pos2(right - width, rect.center().y - COPY_BUTTON_HEIGHT / 2.0),
            egui::vec2(width, COPY_BUTTON_HEIGHT),
        )
    };
    let first_right = rect.right() - ROW_PAD_X;
    let button_rect = slot(first_right, COPY_BUTTON_WIDTH);
    // **The Delete slot and the Cancel slot are one expression, so they
    // cannot drift apart.** See this function's doc: the whole mis-click
    // defence is that the second of two rapid clicks on Delete lands on
    // Cancel, and that is only true while these two rectangles are equal.
    let delete_rect = slot(button_rect.left() - BUTTON_GAP, DELETE_BUTTON_WIDTH);
    let confirm_rect = slot(delete_rect.left() - BUTTON_GAP, CONFIRM_BUTTON_WIDTH);

    if revoking {
        // No widget of any kind. A disabled button would still be a button
        // the layout has to hold, and `Button::sense`-less controls in this
        // window have a history of coming back to life after a re-layout.
        ui.add_space(6.0);
        return None;
    }

    // **A row with no URL has nothing to copy, and says so by being
    // unclickable.** `send.rs`'s parser rejects a *missing* `accessUrl` but
    // accepts an empty one, so a row can reach here holding `""`; copying
    // that would hand `copy_secret("")` to the clipboard, silently wiping
    // whatever the user had there and reporting success. The button is still
    // drawn -- the row must not lose its shape, and a row that quietly has
    // no control is harder to understand than one that has a dead one.
    //
    // Nothing of the sort guards the delete: an id is what `bw send delete`
    // needs, `parse_send_list` refuses a Send without one, and a row that
    // could not be revoked would be a public link with no way to take it
    // down. The `id.is_empty()` case is still refused below, for the same
    // structural reason the URL one is.
    let has_url = !row.access_url.is_empty();
    let has_id = !row.id.is_empty();
    let copied = ui
        .put(
            button_rect,
            egui::Button::new(egui::RichText::new("Copy link").size(12.0).color(theme::INK))
                .min_size(egui::vec2(COPY_BUTTON_WIDTH, COPY_BUTTON_HEIGHT)),
        )
        .clicked();

    let destructive = if confirming {
        let confirmed = ui
            .put(
                confirm_rect,
                egui::Button::new(
                    egui::RichText::new(CONFIRM_LABEL).size(12.0).color(theme::ERROR),
                )
                .min_size(egui::vec2(CONFIRM_BUTTON_WIDTH, COPY_BUTTON_HEIGHT)),
            )
            .clicked();
        let cancelled = ui
            .put(
                delete_rect,
                egui::Button::new(egui::RichText::new(CANCEL_LABEL).size(12.0).color(theme::INK))
                    .min_size(egui::vec2(DELETE_BUTTON_WIDTH, COPY_BUTTON_HEIGHT)),
            )
            .clicked();
        // Cancel wins if both somehow report in one frame: the safe answer to
        // an ambiguous frame on a destructive control is not to destroy.
        if cancelled {
            Some(SendUiAction::CancelDelete)
        } else if confirmed && has_id {
            Some(SendUiAction::ConfirmDelete {
                id: row.id.clone(),
                name: row.name.clone(),
            })
        } else {
            None
        }
    } else {
        let asked = ui
            .put(
                delete_rect,
                egui::Button::new(egui::RichText::new(DELETE_LABEL).size(12.0).color(theme::ERROR))
                    .min_size(egui::vec2(DELETE_BUTTON_WIDTH, COPY_BUTTON_HEIGHT)),
            )
            .clicked();
        (asked && has_id).then(|| SendUiAction::AskDelete(row.id.clone()))
    };

    ui.add_space(6.0);
    // The destructive control wins over Copy link when both report, so a
    // stray copy cannot swallow a delete the user asked for. Belt and braces
    // on the URL: the guard is on the *returned action* as well as on the
    // widget, so no future re-layout of the button can reopen the path.
    destructive.or_else(|| (copied && has_url).then(|| SendUiAction::CopyLink(row.access_url.clone())))
}

/// The header button that opens the composer. Hidden while the composer is
/// already open: two ways to reach one open form is one way too many, and the
/// form itself is the thing on screen at that point.
pub const NEW_SEND_LABEL: &str = "New Send";

/// The composer's own heading. **The one label that appears nowhere else on
/// this screen**, which is what makes it the witness
/// `send_delete_wiring::ReachableState::ComposerOpen` is checked by.
pub const COMPOSER_HEADING: &str = "New text Send";

/// The name field's placeholder.
pub const NAME_HINT: &str = "Name this Send";
/// The body field's placeholder. **Says "text"**, because that is the only
/// kind this app makes and a blank box invites a file drag that will do
/// nothing.
pub const TEXT_HINT: &str = "The text to share";
/// The share-password switch.
pub const PASSWORD_TOGGLE_LABEL: &str = "Require a password to open the link";
/// The share-password field's placeholder.
pub const PASSWORD_HINT: &str = "Password for the link";
/// The line above the three lifetime buttons.
pub const LIFETIME_PROMPT: &str = "The link stops working after";
/// The composer's own submit. **Not "Create Send"**: what the user gets back
/// is a link, and the noun on the button is the thing that is about to exist
/// in the world.
pub const CREATE_LABEL: &str = "Create link";
/// The composer's way out. **"Discard" and not "Cancel"**, deliberately: the
/// row's confirmation already owns [`CANCEL_LABEL`] on this same screen, and
/// pressing this one throws typed secret text away.
pub const DISCARD_LABEL: &str = "Discard";
/// What the composer says while its `bw send create` is running. Every control
/// in the form is disabled in that state, so a second press cannot start a
/// second child.
pub const CREATING_LABEL: &str = "Publishing\u{2026}";

/// **The draft, and the whole of what the window holds for it between
/// frames.**
///
/// It is a [`crate::send::SendPlan`] and not a parallel set of fields, which
/// is the decision worth writing down. A composer holding its own `name`,
/// `text`, `password` and `days` would need a conversion to a plan, and a
/// conversion is a place where the validated value and the published value
/// can differ -- the form says "7 days" and the JSON says thirty, and no test
/// that checks the form or the JSON alone sees it. There is nothing to
/// convert here: [`crate::send::validate_plan`] reads the same bytes
/// `crate::send::plan_to_invocation` will encode.
///
/// It also means the draft costs no per-frame clone of the plaintext.
/// Validation runs every frame the form is up, and a shape that had to build
/// a `SendPlan` to validate would hand the secret body to the allocator sixty
/// times a second.
///
/// **`Debug` is derived and that is safe here**, unlike everywhere else in
/// this feature: the only field that carries a secret is the plan, whose own
/// `Debug` is hand-written to print lengths rather than contents.
///
/// **There is no separate "wants a password" flag.** `plan.password` is
/// `Option<Zeroizing<String>>` and the switch drives that `Option` directly,
/// so turning the password off wipes the buffer it was typed into rather than
/// leaving a live secret behind a `false`.
#[derive(Debug, Default)]
pub struct SendComposer {
    /// Whether the form is on screen. **Window state, not pane state**: it
    /// survives leaving the Sends screen and coming back, because a half-typed
    /// secret thrown away by a stray navigation is a worse surprise than a
    /// form that is still open.
    pub open: bool,
    /// The draft itself.
    pub plan: crate::send::SendPlan,
}

impl SendComposer {
    /// Whether the share-password switch is on.
    pub fn wants_password(&self) -> bool {
        self.plan.password.is_some()
    }

    /// Turns the share-password switch on or off.
    ///
    /// Turning it **off drops the buffer**, which zeroizes it. Turning it on
    /// starts from empty rather than from whatever was typed before, for the
    /// same reason: there is no hidden copy to come back.
    pub fn set_wants_password(&mut self, wanted: bool) {
        if wanted != self.wants_password() {
            self.plan.password = wanted.then(|| zeroize::Zeroizing::new(String::new()));
        }
    }
}

/// What is wrong with the draft, phrased for the user, or `None`.
///
/// A one-line delegation to [`crate::send::validate_plan`] **and that is the
/// point**: the sentence under the Create button and the refusal inside
/// `plan_to_invocation` are the same function, so there is no draft this form
/// calls acceptable that the encoder then rejects, and none it greys out that
/// would in fact have published.
pub fn composer_problem(composer: &SendComposer) -> Option<&'static str> {
    crate::send::validate_plan(&composer.plan)
}

/// Whether the Create button may be pressed at all.
///
/// **Pulled out as a function of two facts rather than written into the
/// widget**, because it is the rule that stops a second `bw send create`
/// starting while the first is still running -- and a rule written inside an
/// eframe closure is a rule no test in this crate can run.
pub fn composer_can_submit(problem: Option<&str>, in_flight: bool) -> bool {
    problem.is_none() && !in_flight
}

/// The label on one lifetime button.
pub fn lifetime_label(days: u8) -> String {
    if days == 1 {
        "1 day".to_string()
    } else {
        format!("{days} days")
    }
}

/// The composer card. Returns the action **this form** reported, if any.
///
/// `in_flight` is whether a `bw send create` started from this form is still
/// running. Every control is disabled while it is, which is the first of the
/// two locks against a second child; the second is in
/// `vault_window::apply_send_action`, which refuses a submit with one in
/// flight whatever the pane reported.
fn draw_composer(
    ui: &mut egui::Ui,
    composer: &mut SendComposer,
    in_flight: bool,
    now: &dyn SendClock,
    zone: &dyn LocalOffset,
) -> Option<SendUiAction> {
    let mut action = None;
    let enabled = !in_flight;
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(COMPOSER_HEADING)
                    .size(14.0)
                    .color(theme::INK)
                    .strong(),
            );
            ui.add_space(8.0);

            ui.add_enabled(
                enabled,
                egui::TextEdit::singleline(&mut composer.plan.name)
                    .hint_text(NAME_HINT)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(6.0);
            ui.add_enabled(
                enabled,
                egui::TextEdit::multiline(&mut *composer.plan.text)
                    .hint_text(TEXT_HINT)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(LIFETIME_PROMPT)
                    .size(12.0)
                    .color(theme::TEXT_FAINT),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                // **The choices come from `send.rs` and are not spelled
                // here.** `validate_plan` refuses any other value, so a
                // button offering one would be a control that cannot work.
                for days in crate::send::DELETE_IN_DAYS_CHOICES {
                    let chosen = composer.plan.delete_in_days == days;
                    let colour = if chosen { theme::INK } else { theme::TEXT_MUTED };
                    if ui
                        .add_enabled(
                            enabled,
                            egui::Button::new(
                                egui::RichText::new(lifetime_label(days)).size(12.0).color(colour),
                            )
                            .selected(chosen)
                            .min_size(egui::vec2(72.0, COPY_BUTTON_HEIGHT)),
                        )
                        .clicked()
                    {
                        composer.plan.delete_in_days = days;
                    }
                }
            });
            ui.add_space(4.0);
            // **The DATE, not only the number of days.** A publishing action
            // where being wrong about the lifetime is the harm gets the thing
            // the user can check against a calendar. `expiry_wording` is
            // `send.rs`'s own, so this line and the `deletionDate` in the JSON
            // cannot disagree about what the choice means.
            //
            // The date it prints is the user's **local** day: the stored
            // `deletionDate` is UTC and stays UTC, and a Send that dies at
            // 00:30 UTC dies the previous evening in the Americas. See
            // `send::expiry_wording`.
            ui.label(
                egui::RichText::new(crate::send::expiry_wording(
                    composer.plan.delete_in_days,
                    now,
                    zone,
                ))
                .size(11.0)
                .color(theme::TEXT_FAINT),
            );

            ui.add_space(10.0);
            let mut wants_password = composer.wants_password();
            if ui
                .add_enabled(
                    enabled,
                    egui::Checkbox::new(
                        &mut wants_password,
                        egui::RichText::new(PASSWORD_TOGGLE_LABEL)
                            .size(12.0)
                            .color(theme::TEXT_MUTED),
                    ),
                )
                .changed()
            {
                composer.set_wants_password(wants_password);
            }
            if let Some(password) = composer.plan.password.as_mut() {
                ui.add_space(4.0);
                ui.add_enabled(
                    enabled,
                    egui::TextEdit::singleline(&mut **password)
                        .hint_text(PASSWORD_HINT)
                        .password(true)
                        .desired_width(f32::INFINITY),
                );
            }

            ui.add_space(12.0);
            let problem = composer_problem(composer);
            let can_submit = composer_can_submit(problem, in_flight);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_submit,
                        egui::Button::new(
                            egui::RichText::new(CREATE_LABEL).size(12.0).color(theme::INK),
                        )
                        .min_size(egui::vec2(104.0, COPY_BUTTON_HEIGHT)),
                    )
                    .clicked()
                {
                    action = Some(SendUiAction::SubmitSend);
                }
                ui.add_space(BUTTON_GAP);
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(
                            egui::RichText::new(DISCARD_LABEL)
                                .size(12.0)
                                .color(theme::TEXT_MUTED),
                        )
                        .min_size(egui::vec2(DELETE_BUTTON_WIDTH, COPY_BUTTON_HEIGHT)),
                    )
                    .clicked()
                {
                    action = Some(SendUiAction::CancelComposer);
                }
                ui.add_space(BUTTON_GAP);
                if in_flight {
                    ui.label(
                        egui::RichText::new(CREATING_LABEL)
                            .size(12.0)
                            .color(theme::TEXT_MUTED),
                    );
                } else if let Some(problem) = problem {
                    // **The reason the button is grey, beside the button.** A
                    // disabled control with no explanation is a control the
                    // user reads as broken.
                    ui.label(egui::RichText::new(problem).size(11.0).color(theme::TEXT_FAINT));
                }
            });
        });
    action
}

/// The inline band. Same shape the item list's band has -- one line, clicking
/// it anywhere dismisses -- drawn here because the item list is not on screen
/// when this pane is.
fn draw_notice_band(ui: &mut egui::Ui, message: &str) -> bool {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), theme::BLUE_WASH);
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        message,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
        theme::ERROR,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send::{list_sends, FixedClock, RawOutput, SendInvocation, SendRunner};
    use std::cell::RefCell;

    /// 2026-08-10T00:00:00Z, so every expiry wording below is exact rather
    /// than approximate.
    const NOW: i64 = 1_786_320_000_000;

    fn at(days: i64) -> String {
        // Built from the same civil arithmetic the parser inverts, so the
        // fixtures cannot drift from the format.
        let (y, mo, d) = (2026i64, 8i64, 10i64 + days);
        format!("{y:04}-{mo:02}-{d:02}T00:00:00.000Z")
    }

    fn summary(name: &str, is_file: bool, days: i64) -> SendSummary {
        SendSummary {
            id: format!("id-{name}"),
            name: name.to_string(),
            access_url: format!("https://send.bitwarden.com/#/{name}"),
            deletion_date: at(days),
            is_file,
        }
    }

    // ---- the clock the fixtures are pinned to ----------------------------

    /// A control. Every expiry assertion below rests on `NOW` really being
    /// 2026-08-10T00:00:00Z; if it is not, they all still pass and all mean
    /// something else.
    #[test]
    fn the_fixture_clock_is_the_instant_the_fixtures_claim() {
        assert_eq!(
            parse_iso_utc_millis("2026-08-10T00:00:00.000Z"),
            Some(NOW),
            "the fixture clock and the fixture dates are not the same instant"
        );
    }

    // ---- the date parser -------------------------------------------------

    #[test]
    fn the_epoch_and_a_leap_day_round_trip() {
        assert_eq!(parse_iso_utc_millis("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_iso_utc_millis("1970-01-01T00:00:01.500Z"),
            Some(1_000),
            "the fractional part must be ignored, not added"
        );
        // 2024-02-29 is 19782 days after the epoch.
        assert_eq!(
            parse_iso_utc_millis("2024-02-29T00:00:00.000Z"),
            Some(19_782 * MILLIS_PER_DAY)
        );
    }

    #[test]
    fn a_date_that_is_not_the_cli_shape_is_refused_rather_than_guessed() {
        for bad in [
            "",
            "2026-08-10",
            "not a date",
            "2026/08/10T00:00:00.000Z",
            "2026-13-10T00:00:00.000Z",
            "2026-08-10T25:00:00.000Z",
        ] {
            assert_eq!(parse_iso_utc_millis(bad), None, "{bad:?} parsed as a date");
        }
    }

    // ---- expiry in words -------------------------------------------------

    #[test]
    fn an_expiry_is_words_and_never_a_raw_timestamp() {
        let clock = FixedClock(NOW);
        assert_eq!(expiry_words(&at(7), &clock), "Expires in 7 days");
        assert_eq!(expiry_words(&at(2), &clock), "Expires in 2 days");
        assert_eq!(expiry_words(&at(1), &clock), "Expires tomorrow");
        assert_eq!(
            expiry_words("2026-08-10T18:00:00.000Z", &clock),
            "Expires today"
        );
        assert_eq!(expiry_words(&at(-1), &clock), "Expired");
        assert_eq!(expiry_words(&at(0), &clock), "Expired", "the exact instant");
    }

    /// The unreadable date is its own wording. Reporting it as `Expired`
    /// would be this app's own confusion presented as a fact about a link
    /// that may well still be live.
    #[test]
    fn an_unreadable_expiry_says_unknown_and_never_expired() {
        let words = expiry_words("who knows", &FixedClock(NOW));
        assert_eq!(words, "Expiry unknown");
        assert!(!words.contains("Expired"));
    }

    // ---- the row model ---------------------------------------------------

    #[test]
    fn a_row_carries_its_own_url_and_id() {
        let row = row_from(&summary("alpha", false, 3), &FixedClock(NOW));
        assert_eq!(row.id, "id-alpha");
        assert_eq!(row.name, "alpha");
        assert_eq!(row.access_url, "https://send.bitwarden.com/#/alpha");
        assert_eq!(row.expiry, "Expires in 3 days");
        assert!(!row.is_file);
    }

    #[test]
    fn a_send_with_no_name_is_still_a_row_with_something_in_it() {
        let mut send = summary("x", false, 1);
        send.name = "   ".to_string();
        assert_eq!(row_from(&send, &FixedClock(NOW)).name, "(no name)");
    }

    /// **File Sends are shown.** The list is not filtered anywhere, and this
    /// asserts both halves: the count is unchanged and the file row is
    /// tagged.
    #[test]
    fn file_sends_are_listed_and_tagged_rather_than_hidden() {
        let sends = [
            summary("text-one", false, 5),
            summary("a-file", true, 5),
            summary("text-two", false, 5),
        ];
        let rows = rows_from(&sends, &FixedClock(NOW));
        assert_eq!(rows.len(), 3, "a Send was dropped from the list");
        assert_eq!(
            rows.iter().filter(|r| r.is_file).count(),
            1,
            "the file Send lost the tag that says what it is"
        );
        assert_eq!(rows[1].name, "a-file", "the rows are not in the CLI's order");
    }

    // ---- empty is not failed ---------------------------------------------

    #[test]
    fn an_answered_empty_list_is_empty_and_nothing_else() {
        let state = pane_state(Some(&Ok(Vec::new())), &FixedClock(NOW));
        assert_eq!(state, SendPaneState::Empty);
        assert!(state.is_an_answer());
    }

    /// **The single most important assertion in this file.** Every failure
    /// arm `list_sends` can produce must land on `Failed`, and none of them
    /// may land on `Empty` or on `Rows(vec![])`.
    #[test]
    fn no_failure_is_ever_an_empty_list() {
        let failures = [
            SendError::NoVerifiedCli("no bw".to_string()),
            SendError::Locked,
            SendError::Offline,
            SendError::Rejected("nope".to_string()),
            SendError::FailedSilently,
            SendError::CreatedButUnreadable,
            SendError::TimedOut,
            SendError::SpawnFailed("boom".to_string()),
        ];
        for failure in failures {
            let expected_ambiguous = failure.is_ambiguous();
            let expected_message = failure.user_message().to_string();
            let state = pane_state(Some(&Err(failure.clone())), &FixedClock(NOW));
            assert_ne!(state, SendPaneState::Empty, "{failure:?} rendered as empty");
            assert_ne!(
                state,
                SendPaneState::Rows(Vec::new()),
                "{failure:?} rendered as a list of nothing"
            );
            assert!(
                !state.is_an_answer(),
                "{failure:?} rendered as a claim about the account"
            );
            assert_eq!(
                state,
                SendPaneState::Failed {
                    message: expected_message,
                    ambiguous: expected_ambiguous,
                },
                "{failure:?} did not carry its own message and ambiguity through"
            );
        }
    }

    /// The ambiguous arms carry the extra sentence, and the unambiguous ones
    /// do not. Pinned in both directions, exactly as `is_ambiguous` is one
    /// step down.
    #[test]
    fn only_an_ambiguous_failure_says_it_could_not_check() {
        let ambiguous = pane_state(Some(&Err(SendError::TimedOut)), &FixedClock(NOW));
        assert_eq!(
            ambiguous,
            SendPaneState::Failed {
                message: SendError::TimedOut.user_message().to_string(),
                ambiguous: true,
            }
        );
        let plain = pane_state(Some(&Err(SendError::Offline)), &FixedClock(NOW));
        match plain {
            SendPaneState::Failed { ambiguous, .. } => {
                assert!(!ambiguous, "an unambiguous failure claimed it might have missed some")
            }
            other => panic!("an offline failure rendered as {other:?}"),
        }
    }

    #[test]
    fn an_unanswered_fetch_is_loading_and_never_empty() {
        let state = pane_state(None, &FixedClock(NOW));
        assert_eq!(state, SendPaneState::Loading);
        assert!(
            !state.is_an_answer(),
            "a question nobody has answered was reported as an answer"
        );
    }

    // ---- the fetch state machine -----------------------------------------

    #[test]
    fn the_fetch_asks_once_per_visit_and_never_per_frame() {
        let fresh = SendFetch::default();
        assert!(fresh.wants_fetch(true), "the selected screen never asked");
        assert!(!fresh.wants_fetch(false), "an unselected screen asked anyway");

        let running = SendFetch { in_flight: true, ..SendFetch::default() };
        assert!(!running.wants_fetch(true), "a second `bw` child started while one was running");

        let answered = SendFetch { result: Some(Ok(Vec::new())), in_flight: false, ..Default::default() };
        assert!(!answered.wants_fetch(true), "a list already in hand was fetched again");

        let failed = SendFetch {
            result: Some(Err(SendError::Offline)),
            in_flight: false,
            ..Default::default()
        };
        assert!(!failed.wants_fetch(true), "a failed fetch was retried at frame rate");
    }

    /// `invalidate` re-arms the fetch. **The absence this asserts against is
    /// "the list never refreshes"** -- an omitted `invalidate` is invisible
    /// in every other test in this file, because every other test starts from
    /// a fresh `SendFetch`.
    #[test]
    fn invalidating_re_arms_the_fetch_without_letting_a_second_thread_start() {
        let mut fetch = SendFetch {
            result: Some(Ok(vec![summary("a", false, 1)])),
            in_flight: false,
            ..Default::default()
        };
        assert!(!fetch.wants_fetch(true));
        fetch.invalidate();
        assert!(fetch.wants_fetch(true), "the list did not become refetchable");

        let mut running = SendFetch {
            result: Some(Err(SendError::Offline)),
            in_flight: true,
            ..Default::default()
        };
        running.invalidate();
        assert!(
            !running.wants_fetch(true),
            "a refresh while a thread was still running armed a second one"
        );
    }

    /// The refetch policy itself, as a truth table.
    #[test]
    fn leaving_the_screen_is_what_makes_the_next_visit_ask_again() {
        assert!(should_invalidate_on_leave(true, false), "leaving kept a stale list");
        assert!(!should_invalidate_on_leave(true, true), "staying threw the list away");
        assert!(!should_invalidate_on_leave(false, false), "a screen never visited was invalidated");
        assert!(!should_invalidate_on_leave(false, true), "arriving threw away a list being fetched");
    }

    /// The badge is the same rule as the pane, on the eight pixels most
    /// likely to be read at a glance.
    #[test]
    fn a_failed_fetch_never_badges_the_sidebar_with_a_zero() {
        assert_eq!(SendFetch::default().badge_count(), None, "an unfetched list claimed a count");
        assert_eq!(
            SendFetch { result: Some(Ok(Vec::new())), in_flight: false, ..Default::default() }.badge_count(),
            Some(0),
            "an answered empty list must say 0, which is a fact it has"
        );
        assert_eq!(
            SendFetch {
                result: Some(Ok(vec![summary("a", false, 1), summary("b", true, 1)])),
                in_flight: false,
                ..Default::default()
            }
            .badge_count(),
            Some(2)
        );
        assert_eq!(
            SendFetch { result: Some(Err(SendError::Offline)), in_flight: false, ..Default::default() }.badge_count(),
            None,
            "a failed fetch badged a count it does not have -- a 0 here reads as `no Sends`"
        );
    }

    // ---- over the real seam ----------------------------------------------

    /// A fake [`SendRunner`], local to this module.
    ///
    /// `send.rs`'s own `FakeRunner` is private to its `#[cfg(test)]` module
    /// and `send.rs` is not this task's file, so it could not be reused; this
    /// is the same shape. **No test in this crate may spawn `bw`**, and this
    /// is what keeps the whole-path tests below off a real process.
    struct FakeRunner {
        answer: RefCell<Option<Result<RawOutput, SendError>>>,
        seen: RefCell<Vec<Vec<String>>>,
        sessions: RefCell<Vec<Option<String>>>,
    }

    impl FakeRunner {
        fn answering(answer: Result<RawOutput, SendError>) -> Self {
            Self {
                answer: RefCell::new(Some(answer)),
                seen: RefCell::new(Vec::new()),
                sessions: RefCell::new(Vec::new()),
            }
        }
        fn ok(stdout: &str) -> Self {
            Self::answering(Ok(RawOutput {
                exit_code: Some(0),
                stdout: stdout.to_string(),
                stderr: String::new(),
            }))
        }
    }

    impl SendRunner for FakeRunner {
        fn run(&self, inv: &SendInvocation) -> Result<RawOutput, SendError> {
            self.seen.borrow_mut().push(inv.args().to_vec());
            self.sessions
                .borrow_mut()
                .push(inv.session_token().map(str::to_string));
            self.answer
                .borrow_mut()
                .take()
                .unwrap_or(Err(SendError::FailedSilently))
        }
    }

    /// Constructed, not captured. No real Send exists to capture from: making
    /// one publishes a real public link, and this step exists to make that
    /// impossible. Field names and the `type` codes come from step 1's
    /// captured `bw send template` output.
    const LIST_JSON: &str = r#"[
      {"object":"send","id":"aaa","name":"notes","type":0,
       "accessUrl":"https://send.bitwarden.com/#/aaa","deletionDate":"2026-08-17T00:00:00.000Z"},
      {"object":"send","id":"bbb","name":"report.pdf","type":1,
       "accessUrl":"https://send.bitwarden.com/#/bbb","deletionDate":"2026-08-11T00:00:00.000Z"}
    ]"#;

    #[test]
    fn a_real_list_answer_becomes_rows_with_the_file_one_kept() {
        let runner = FakeRunner::ok(LIST_JSON);
        let result = list_sends(&runner);
        let state = pane_state(Some(&result), &FixedClock(NOW));
        let SendPaneState::Rows(rows) = state else {
            panic!("a clean list did not render as rows")
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "notes");
        assert!(!rows[0].is_file);
        assert_eq!(rows[0].expiry, "Expires in 7 days");
        assert_eq!(rows[1].name, "report.pdf");
        assert!(rows[1].is_file, "the file Send lost its tag on the way through");
        assert_eq!(rows[1].access_url, "https://send.bitwarden.com/#/bbb");
    }

    #[test]
    fn an_unreadable_list_is_a_failure_and_not_an_empty_account() {
        let runner = FakeRunner::ok("this is not json");
        let result = list_sends(&runner);
        let state = pane_state(Some(&result), &FixedClock(NOW));
        assert!(
            !state.is_an_answer(),
            "unreadable output from `bw` was painted as `you have no Sends`"
        );
    }

    // ---- The late answer -------------------------------------------------

    /// **The reviewer's reproduction, kept as a test.** Every step is a
    /// thing the window actually does, in the order an ordinary user does
    /// them, and the whole sequence used to end with the previous visit's
    /// list on screen and no refetch.
    ///
    /// 1. On Sends, a fetch is in flight.
    /// 2. The user clicks Cards. `should_invalidate_on_leave` fires and
    ///    `invalidate` runs -- a **no-op** on `result`, which is already
    ///    `None`.
    /// 3. The detached thread lands. Before the generation tag, the drain
    ///    wrote it into `result` unconditionally.
    /// 4. The user comes back to Sends. `was_on_sends` was false so nothing
    ///    invalidates, and `result` is `Some`, so `wants_fetch` is false.
    ///
    /// The result was a list from before the user left, shown and counted in
    /// the badge as current -- the stale Copy link the refetch policy exists
    /// to prevent, reached through the code meant to prevent it.
    #[test]
    fn a_late_answer_from_the_visit_before_is_dropped_and_the_next_visit_asks_again() {
        let mut fetch = SendFetch::default();

        // 1. Arriving on Sends asks, once.
        assert!(fetch.wants_fetch(true));
        fetch.in_flight = true;
        let tag = fetch.generation();
        assert!(!fetch.wants_fetch(true), "a second thread was allowed to start");

        // 2. Leaving for Cards.
        assert!(should_invalidate_on_leave(true, false));
        fetch.invalidate();
        assert!(fetch.result.is_none());
        assert!(fetch.in_flight, "`invalidate` must not let a second thread start");

        // 3. The thread from step 1 lands, late.
        let stale = vec![SendSummary {
            id: "stale".into(),
            name: "from the visit before".into(),
            access_url: "https://send.bitwarden.com/#/stale".into(),
            deletion_date: at(7),
            is_file: false,
        }];
        assert!(
            !fetch.apply_answer(tag, Ok(stale)),
            "an answer to a question the user has navigated away from was kept"
        );
        assert!(!fetch.in_flight, "`in_flight` latched -- no further fetch can ever start");
        assert!(
            fetch.result.is_none(),
            "the late answer was stored, so the next visit will show it without asking"
        );
        assert_eq!(
            fetch.badge_count(),
            None,
            "the sidebar badge counted a list from a visit the user has left"
        );

        // 4. Returning to Sends. Nothing invalidates on arrival...
        assert!(!should_invalidate_on_leave(false, true));
        // ...and the screen asks again anyway, because the stale answer never
        // landed.
        assert!(
            fetch.wants_fetch(true),
            "returning to Sends did not refetch -- the previous visit's list is on screen"
        );
    }

    /// The other half, so the drop rule cannot be satisfied by dropping
    /// *everything*: an answer to the question actually on screen is kept.
    #[test]
    fn an_answer_tagged_with_the_current_question_is_kept() {
        let mut fetch = SendFetch::default();
        assert!(fetch.wants_fetch(true));
        fetch.in_flight = true;
        let tag = fetch.generation();
        assert!(fetch.apply_answer(tag, Ok(vec![summary("alpha", false, 7)])));
        assert!(!fetch.in_flight);
        assert_eq!(fetch.badge_count(), Some(1));
        assert!(!fetch.wants_fetch(true), "a held list was refetched");
    }

    /// A **failure** is current-or-stale on the same terms. A failed fetch
    /// from a previous visit must not be the failure the next visit shows.
    #[test]
    fn a_late_failure_is_dropped_just_as_a_late_success_is() {
        let mut fetch = SendFetch::default();
        fetch.in_flight = true;
        let tag = fetch.generation();
        fetch.invalidate();
        assert!(!fetch.apply_answer(tag, Err(SendError::Offline)));
        assert!(matches!(
            pane_state(fetch.result.as_ref(), &FixedClock(NOW)),
            SendPaneState::Loading
        ));
    }

    /// `invalidate` moves the tag. This is the line that makes the "in_flight
    /// is deliberately NOT cleared" comment true rather than merely copied
    /// from `AuxList`.
    #[test]
    fn invalidating_moves_the_generation_so_the_running_thread_is_disowned() {
        let mut fetch = SendFetch::default();
        let before = fetch.generation();
        fetch.invalidate();
        assert_ne!(fetch.generation(), before);
        fetch.invalidate();
        assert_ne!(fetch.generation(), before);
    }

    // -- the refetch policy, driven rather than pinned ---------------------
    //
    // `should_invalidate_on_leave` was already tested as a standalone
    // predicate, and that proved nothing about the frame: the predicate can
    // be right while its answer is thrown away. Measured on `c14afb2`,
    // replacing the frame's `send_fetch.invalidate();` with a `log::trace!`
    // left 2050 lib + 217 bin green with the policy entirely gone. The
    // decision is `note_screen` now -- rule, action and remembering in one
    // place -- so these run it.

    /// **The property, end to end.** Visit, get an answer, leave, come back:
    /// the list must be asked for again rather than redrawn from the last
    /// visit.
    #[test]
    fn leaving_the_sends_screen_and_returning_asks_the_server_again() {
        let mut fetch = SendFetch::default();

        // Visit one: the gate opens, a fetch runs, an answer lands.
        fetch.note_screen(true);
        assert!(fetch.wants_fetch(true), "the first visit did not ask");
        fetch.in_flight = true;
        let tag = fetch.generation();
        assert!(fetch.apply_answer(tag, Ok(vec![summary("alpha", false, 7)])));
        assert!(!fetch.wants_fetch(true), "the held list was refetched mid-visit");

        // Staying on the screen for further frames changes nothing.
        fetch.note_screen(true);
        assert_eq!(fetch.badge_count(), Some(1), "the list was dropped without leaving");

        // Leaving drops it.
        fetch.note_screen(false);
        assert_eq!(
            fetch.badge_count(),
            None,
            "leaving the Sends screen did not drop the list, so the next visit will show a \
             Copy link for a Send that may have been deleted or expired since"
        );

        // Frames spent elsewhere are not repeated leaves, and do not ask.
        fetch.note_screen(false);
        assert!(!fetch.wants_fetch(false), "a fetch was started for a screen that is not up");

        // Returning asks.
        fetch.note_screen(true);
        assert!(
            fetch.wants_fetch(true),
            "returning to Sends did not ask again -- the previous visit's list would be drawn"
        );
    }

    /// **Arriving is not leaving.** A `note_screen` that invalidated on every
    /// transition, or on every frame, would pass the test above and refetch
    /// on the frame after the answer lands -- a `bw` child per frame.
    #[test]
    fn only_the_leaving_edge_drops_the_list() {
        for (was, now) in [(false, false), (false, true), (true, true)] {
            let mut fetch = SendFetch::default();
            fetch.note_screen(was);
            fetch.in_flight = true;
            let tag = fetch.generation();
            assert!(fetch.apply_answer(tag, Ok(vec![summary("alpha", false, 7)])));
            let before = fetch.generation();

            fetch.note_screen(now);

            assert_eq!(
                fetch.badge_count(),
                Some(1),
                "the list was dropped moving from was_selected={was} to {now}, which is not a \
                 leave"
            );
            assert_eq!(
                fetch.generation(),
                before,
                "the generation moved on a transition that is not a leave, which disowns a \
                 fetch that is still the current question"
            );
        }
    }

    /// The leave **bumps the generation**, so a fetch still running from the
    /// visit being left cannot land as though it answered the next one. This
    /// is the same reason `invalidate` bumps it; routing through `note_screen`
    /// must not lose that.
    #[test]
    fn a_fetch_still_running_when_the_user_leaves_is_disowned() {
        let mut fetch = SendFetch::default();
        fetch.note_screen(true);
        fetch.in_flight = true;
        let tag = fetch.generation();

        fetch.note_screen(false);

        assert!(
            !fetch.apply_answer(tag, Ok(vec![summary("alpha", false, 7)])),
            "an answer to the visit the user has left was accepted as the current one"
        );
        fetch.note_screen(true);
        assert!(fetch.wants_fetch(true), "the next visit did not ask");
    }

    /// A window that opens straight onto some other screen has never been on
    /// Sends, so the first frame must not be treated as a leave.
    #[test]
    fn the_first_frame_is_not_a_leave() {
        let mut fetch = SendFetch::default();
        let before = fetch.generation();
        fetch.note_screen(false);
        assert_eq!(fetch.generation(), before);
        assert!(fetch.result.is_none());
    }

    /// **The in-flight half of [`composer_can_submit`], which nothing ran.**
    ///
    /// The function's own doc calls `!in_flight` "the rule that stops a
    /// second `bw send create` starting", and it was the reason the rule was
    /// lifted out of the eframe closure at all -- but every caller in the
    /// crate is that closure, so deleting `&& !in_flight` outright left 2243
    /// lib / 217 bin tests green with no warning. Measured.
    ///
    /// **What that mutant did and did not cost, stated exactly, because the
    /// two-lock design is easy to mistake for redundancy.** It does NOT
    /// double-publish: the second lock, in `vault_window::apply_send_action`,
    /// holds and is covered. What it costs is the whole point of the FIRST
    /// lock -- during a publish the Create button goes live and clickable, so
    /// the user is invited to press a control that has been quietly disarmed
    /// one layer down. The misleading live button is the defect; the two
    /// locks exist because refusing the work and refusing the INVITATION are
    /// different jobs.
    ///
    /// All four combinations, so neither argument can be ignored: a truth
    /// table is the only thing that pins a two-input `&&`, and either input
    /// dropped makes one of these four rows fail.
    #[test]
    fn the_create_button_is_dead_while_a_create_is_in_flight() {
        let problem: Option<&str> = None;
        let broken: Option<&str> = Some("Give the Send a name.");

        assert!(
            composer_can_submit(problem, false),
            "a valid draft with nothing in flight could not be submitted, so the rest of \
             this test is about a button that never goes live at all"
        );
        assert!(
            !composer_can_submit(problem, true),
            "THE MISSING CASE: a VALID draft offered a live Create button while a \
             `bw send create` was already running. `apply_send_action`'s own lock stops \
             the second child, so this does not double-publish -- what it does is invite \
             the user to press a control that has been disarmed one layer down, which is \
             the misleading state this first lock exists to prevent"
        );
        assert!(
            !composer_can_submit(broken, false),
            "a draft the form itself calls invalid offered a live Create button"
        );
        assert!(
            !composer_can_submit(broken, true),
            "control: both reasons to refuse at once still refuses"
        );
    }

    /// Control for the test above: the `problem` it calls valid really is the
    /// verdict the FORM reaches on a real draft, and the one it calls invalid
    /// really is a refusal -- so neither row above is asserting about a
    /// hand-made `Option` that no composer could ever produce.
    #[test]
    fn the_submit_rule_is_fed_the_forms_own_verdict() {
        let mut composer = SendComposer::default();
        assert!(
            composer_problem(&composer).is_some(),
            "control: a freshly opened composer is empty, so the form must refuse it"
        );
        assert!(
            !composer_can_submit(composer_problem(&composer), false),
            "an empty draft could be submitted, which would publish an empty Send under a \
             public link"
        );

        composer.plan.name.push_str("a name");
        composer.plan.text.push_str("a body");
        assert_eq!(
            composer_problem(&composer),
            None,
            "control: a filled draft is still refused, so the `None` row above is not a \
             verdict this form ever reaches"
        );
        assert!(
            composer_can_submit(composer_problem(&composer), false),
            "a draft the form accepts could not be submitted"
        );
        assert!(
            !composer_can_submit(composer_problem(&composer), true),
            "a draft the form accepts could be submitted DURING a publish"
        );
    }
}

#[cfg(test)]
mod fetch_thread_tests {
    //! **Where the `bw send list` call runs.** Behavioural, not a source pin.
    //!
    //! The property is "the blocking call is made on a thread that is not the
    //! frame's". It was a pin over `spawn_send_list`'s text, and a pin over a
    //! *function* is satisfied by hoisting the call above the spawn -- which
    //! breaks exactly the property the pin was written for, silently. So
    //! `spawn_send_list_with` takes the fetch as a value, and these tests
    //! hand it one that reports where and when it ran.
    //!
    //! No process is spawned and no network is touched: the fetch under test
    //! is a closure, and the production one is reached only through
    //! `spawn_send_list`, which is not called here.

    use eframe::egui;
    use std::sync::mpsc;
    use std::thread::ThreadId;
    use std::time::Duration;

    /// Generous on purpose. Nothing here is a timing measurement -- the
    /// timeouts exist only so that a regression fails loudly instead of
    /// hanging the suite -- so the bound is set well above any plausible
    /// scheduling delay on a loaded machine.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// **The fetch does not run on the caller's thread.**
    ///
    /// Two independent witnesses, because either alone is weak. The thread id
    /// is the direct statement of the property. The gate is the consequence
    /// that actually matters: `spawn_send_list_with` returns *before* the
    /// fetch has finished, which is what stops a sixty-second `bw` cap from
    /// freezing the window on the frame the user clicks Sends. A hoist of the
    /// call above the spawn fails both.
    #[test]
    fn the_fetch_runs_off_the_callers_thread_and_the_caller_does_not_wait_for_it() {
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        let (where_tx, where_rx) = mpsc::channel::<ThreadId>();
        // Opened by this thread only after the call below has returned.
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let caller = std::thread::current().id();

        super::super::send_fetch_thread::spawn_send_list_with(ctx.clone(), tx, 7, move || {
            let _ = where_tx.send(std::thread::current().id());
            // If the fetch were run by the caller, this would still be inside
            // the call below and the gate could not have been opened. A
            // timeout rather than a blocking recv, so a regression fails
            // loudly instead of hanging the suite.
            let released = gate_rx.recv_timeout(PATIENCE).is_ok();
            assert!(
                released,
                "the fetch was never released -- it ran before `spawn_send_list_with` returned, \
                 so the eframe thread is waiting on `bw`"
            );
            Ok(Vec::new())
        });
        // Reached only because the call above did not block on the fetch.
        gate_tx.send(()).expect("the fetch thread was never started");

        let ran_on = where_rx.recv_timeout(PATIENCE).expect("the fetch never ran");
        assert_ne!(
            ran_on, caller,
            "the Sends fetch ran on the calling thread -- in production that is the eframe \
             thread, and `bw` may be waited on for sixty seconds"
        );

        let (tag, answer) = rx.recv_timeout(PATIENCE).expect("no answer was ever sent");
        assert_eq!(tag, 7, "the answer did not carry the generation it was started under");
        assert!(answer.is_ok());
    }

    /// **The window is asked to repaint when the answer lands.** Without it
    /// the answer sits in the channel until some unrelated input provokes a
    /// frame, and the screen shows a spinner over a list it already has.
    #[test]
    fn a_landed_answer_asks_the_window_to_repaint() {
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        // Consume the request the context starts life with, so the assertion
        // below is about this fetch and not about a fresh `Context`.
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        assert!(
            !ctx.has_requested_repaint(),
            "the fixture context already wanted a repaint, so the assertion below is vacuous"
        );

        super::super::send_fetch_thread::spawn_send_list_with(ctx.clone(), tx, 0, || Ok(Vec::new()));
        let _ = rx.recv_timeout(PATIENCE).expect("no answer was ever sent");

        // The send happens just before the repaint request, so poll rather
        // than read once.
        let deadline = std::time::Instant::now() + PATIENCE;
        while !ctx.has_requested_repaint() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            ctx.has_requested_repaint(),
            "a landed Sends answer did not ask for a repaint, so it would wait for an unrelated \
             frame before it was drawn"
        );
    }
}

/// **The abandonment counter really counts** -- so that every assertion
/// written against it, in this file and in `vault_window::run`, is saying
/// something.
///
/// A `debug_assert_eq!(abandoned_in_this_thread(), 0, ..)` that could never be
/// anything but zero is not a hold, it is decoration, and this is the only
/// place that difference can be measured directly. Each test runs on a thread
/// of its own, because the counter is thread-local by design and monotonic:
/// reading it on the test runner's thread would be reading whatever every
/// other test on that thread had already done.
#[cfg(test)]
mod verdict_linearity {
    use super::*;

    /// Runs `body` and answers with how far the count moved across it.
    ///
    /// A DELTA rather than the raw count, and no thread of its own:
    /// `std::thread::spawn` is censused crate-wide by
    /// `job_object::the_thread_spawn_census_is_exact`, and a test helper is not
    /// a reason to widen that census. It does not need one -- libtest already
    /// runs every test on a thread of its own, so the control below really is
    /// measuring a fresh thread-local.
    fn abandoned_by(body: impl FnOnce()) -> usize {
        let before = abandoned_in_this_thread();
        assert_eq!(
            before, 0,
            "control: this test's own thread starts with a non-zero abandonment count, \
             so the counter is not thread-local and every measurement below is somebody \
             else's"
        );
        body();
        abandoned_in_this_thread() - before
    }

    /// **THE LIVENESS CONTROL for every assertion on this counter.** A verdict
    /// dropped while it still holds an action is counted -- which is what a
    /// shadowed Sends action must do, whatever state it is gated on.
    #[test]
    fn a_verdict_dropped_instead_of_applied_is_counted() {
        assert_eq!(
            abandoned_by(|| {
                let verdict = SendUiVerdict::seal(SendUiAction::Refresh);
                drop(verdict);
            }),
            1,
            "a `SendUiVerdict` was dropped still holding its action and nothing counted it, \
             so `abandoned_in_this_thread` reports zero no matter what happens and every \
             assertion on it -- including `vault_window::run`'s own `debug_assert` -- holds \
             nothing at all"
        );
    }

    /// And a verdict that was applied is NOT counted, so the assertion is not
    /// one that fails on a correct frame.
    #[test]
    fn a_verdict_that_was_applied_is_not_counted() {
        assert_eq!(
            abandoned_by(|| {
                let verdict = SendUiVerdict::seal(SendUiAction::Refresh);
                assert_eq!(verdict.into_action(), SendUiAction::Refresh);
            }),
            0,
            "applying a verdict counted it as abandoned, so the count is a count of verdicts \
             and not of LOST ones -- every assertion on it would fail on a correct frame"
        );
    }

    /// The count is per abandonment rather than a flag, so two losses read as
    /// two -- a frame that drops one verdict and a run that drops several are
    /// distinguishable in a failure message.
    #[test]
    fn each_abandoned_verdict_is_counted_separately() {
        assert_eq!(
            abandoned_by(|| {
                drop(SendUiVerdict::seal(SendUiAction::Refresh));
                drop(SendUiVerdict::seal(SendUiAction::CancelDelete));
            }),
            2,
            "two abandoned verdicts read as something other than two"
        );
    }

    /// **And the pane's own product is a verdict that must be consumed**, so
    /// the mechanism is on the real path and not only on hand-built values.
    /// This is what makes `let _ = draw_send_pane(..)` a counted loss rather
    /// than a silent one anywhere in the crate.
    #[test]
    fn the_pane_hands_back_a_verdict_that_counts_when_it_is_dropped() {
        assert_eq!(
            abandoned_by(|| {
                let ctx = egui::Context::default();
                let state = SendPaneState::Loading;
                let _ = ctx.run_ui(Default::default(), |ui| {
                    // Deliberately dropped rather than applied: this is the
                    // shape every measured shadow reduces to.
                    let _ = draw_send_pane(ui, &state, None, SendDeleteView::default(), &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0));
                });
            }),
            1,
            "the Sends pane's own answer can be thrown away without anything counting it, so \
             the linearity `SendUiVerdict` exists for does not reach the real pane"
        );
    }
}

#[cfg(test)]
mod paint_tests {
    //! What this pane **actually paints**, driven through real frames at the
    //! smallest window the OS will let the user make.
    //!
    //! Two hard-won details are baked in. (1) `theme::apply`'s font families
    //! only exist from the frame after it is called, so every fixture runs
    //! two warm-up frames first. (2) **egui culls shapes entirely outside the
    //! screen rect**, so a control pushed off the pane comes back as
    //! *nothing* rather than as a rect out of bounds -- which is why every
    //! geometry assertion here is preceded by a count assertion, and why the
    //! button is measured for a non-zero size rather than merely found.

    use super::*;
    use crate::send::FixedClock;

    const NOW: i64 = 1_786_320_000_000;

    /// The vault window's centre pane at the **minimum window size**: 900x600
    /// is `settings::MIN_VAULT_WINDOW_SIZE`, less the sidebar's 212 and a
    /// generous allowance for the titlebar and chrome above. If the window
    /// floor ever moves, this is measured off the constant and moves with it.
    fn min_pane_size() -> egui::Vec2 {
        let (w, h) = crate::settings::MIN_VAULT_WINDOW_SIZE;
        egui::vec2(
            w as f32 - crate::vault_window::SIDEBAR_WIDTH - 40.0,
            h as f32 - 120.0,
        )
    }

    struct Painted {
        text: Vec<String>,
        rects: Vec<egui::Rect>,
        /// The rect of each painted text run, by its text.
        text_rects: Vec<(String, egui::Rect)>,
    }

    impl Painted {
        fn has(&self, needle: &str) -> bool {
            self.text.iter().any(|t| t.contains(needle))
        }
        fn count(&self, needle: &str) -> usize {
            self.text.iter().filter(|t| t.contains(needle)).count()
        }
        fn rect_of(&self, needle: &str) -> Option<egui::Rect> {
            self.text_rects
                .iter()
                .find(|(t, _)| t.contains(needle))
                .map(|(_, r)| *r)
        }
    }

    fn collect(shape: &egui::Shape, out: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => {
                out.text.push(text.galley.text().to_owned());
                out.text_rects
                    .push((text.galley.text().to_owned(), text.visual_bounding_rect()));
            }
            egui::Shape::Rect(rect) => out.rects.push(rect.rect),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Runs `draw_send_pane` in a pane of `size` and returns everything it
    /// painted, plus the action of the last frame.
    fn paint(state: &SendPaneState, notice: Option<&str>, size: egui::Vec2) -> (Painted, SendUiAction) {
        paint_with(state, notice, size, SendDeleteView::default())
    }

    /// [`paint`], with the window's delete state as the pane would really be
    /// handed it.
    fn paint_with(
        state: &SendPaneState,
        notice: Option<&str>,
        size: egui::Vec2,
        delete: SendDeleteView<'_>,
    ) -> (Painted, SendUiAction) {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut action = SendUiAction::None;
        let output = ctx.run_ui(input(), |ui| {
            action = draw_send_pane(ui, state, notice, delete, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });

        let mut painted = Painted { text: Vec::new(), rects: Vec::new(), text_rects: Vec::new() };
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        assert!(
            !painted.text.is_empty(),
            "the pane painted no text at all, so every assertion over this list would pass \
             against nothing"
        );
        (painted, action)
    }

    fn rows(n: usize) -> SendPaneState {
        let sends: Vec<SendSummary> = (0..n)
            .map(|i| SendSummary {
                id: format!("id{i}"),
                name: format!("send-number-{i}"),
                access_url: format!("https://send.bitwarden.com/#/{i}"),
                deletion_date: "2026-08-17T00:00:00.000Z".to_string(),
                is_file: i % 2 == 1,
            })
            .collect();
        pane_state(Some(&Ok(sends)), &FixedClock(NOW))
    }

    /// The subtext is on screen at the minimum window size. It is the whole
    /// of what makes the excluded scope honest, and a line that only appears
    /// on a wide window is a line most users never read.
    #[test]
    fn the_scope_subtext_is_painted_at_the_minimum_window_size() {
        let (painted, _) = paint(&rows(3), None, min_pane_size());
        assert!(painted.has(SCOPE_SUBTEXT), "the scope line was not painted: {:?}", painted.text);
        let rect = painted.rect_of(SCOPE_SUBTEXT).expect("no rect for the scope line");
        assert!(rect.width() > 1.0 && rect.height() > 1.0, "the scope line was drawn at {rect:?}");
    }

    /// **Six rows, not two.** A pane that draws only the first few rows
    /// passes every assertion written against a two-row fixture, which is a
    /// defect this codebase has already shipped once.
    #[test]
    fn every_row_is_drawn_with_its_own_copy_button_at_the_minimum_window_size() {
        let size = min_pane_size();
        let (painted, _) = paint(&rows(6), None, size);
        // COUNT FIRST. A row pushed off the pane is culled entirely, so it
        // comes back as nothing at all -- reading geometry before counting
        // would read the geometry of the rows that survived.
        assert_eq!(painted.count("send-number-"), 6, "painted names: {:?}", painted.text);
        assert_eq!(
            painted.count("Copy link"),
            6,
            "a row was drawn without the button that makes its link retrievable"
        );
        assert_eq!(painted.count(FILE_TAG), 3, "the file rows lost their tag");

        // Every Copy link glyph is inside the pane AND has a real size. A
        // control drawn at zero size has passed both a presence assertion and
        // an in-pane assertion in this codebase before; only a glyph-level
        // size check caught it.
        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let buttons: Vec<egui::Rect> = painted
            .text_rects
            .iter()
            .filter(|(t, _)| t == "Copy link")
            .map(|(_, r)| *r)
            .collect();
        assert_eq!(buttons.len(), 6);
        for rect in buttons {
            assert!(rect.width() > 4.0 && rect.height() > 4.0, "a Copy link glyph is {rect:?}");
            assert!(
                pane.contains_rect(rect),
                "a Copy link glyph at {rect:?} is outside the {pane:?} pane at the minimum \
                 window size"
            );
        }
    }

    /// Empty and failed do not share a single word on screen.
    #[test]
    fn an_empty_account_and_a_failed_fetch_paint_different_words() {
        let size = min_pane_size();
        let (empty, _) = paint(&SendPaneState::Empty, None, size);
        assert!(empty.has(EMPTY_HEADLINE));
        assert!(!empty.has(FAILED_HEADLINE));
        assert!(!empty.has("could not"), "the empty state hedged: {:?}", empty.text);

        let failed = SendPaneState::Failed {
            message: SendError::Offline.user_message().to_string(),
            ambiguous: false,
        };
        let (failed, _) = paint(&failed, None, size);
        assert!(failed.has(FAILED_HEADLINE));
        assert!(
            !failed.has(EMPTY_HEADLINE),
            "a failed fetch told the user they have no Sends: {:?}",
            failed.text
        );
        assert!(failed.has(SendError::Offline.user_message()));
        assert!(failed.has("Try again"), "a failure with no way to retry");
    }

    /// An ambiguous failure says so, on screen, in words.
    #[test]
    fn an_ambiguous_failure_paints_the_sentence_that_stops_it_reading_as_none() {
        let size = min_pane_size();
        let state = SendPaneState::Failed {
            message: SendError::TimedOut.user_message().to_string(),
            ambiguous: true,
        };
        let (painted, _) = paint(&state, None, size);
        assert!(painted.has(AMBIGUOUS_DETAIL), "painted: {:?}", painted.text);
        assert!(!painted.has(EMPTY_HEADLINE));

        let plain = SendPaneState::Failed {
            message: SendError::Offline.user_message().to_string(),
            ambiguous: false,
        };
        let (plain, _) = paint(&plain, None, size);
        assert!(
            !plain.has(AMBIGUOUS_DETAIL),
            "an unambiguous failure claimed it might have missed some"
        );
    }

    /// Loading is not empty, on screen as well as in the enum.
    #[test]
    fn a_pane_that_has_not_been_answered_yet_says_so() {
        let (painted, _) = paint(&SendPaneState::Loading, None, min_pane_size());
        assert!(painted.has(LOADING_LABEL));
        assert!(!painted.has(EMPTY_HEADLINE));
        assert!(!painted.has(FAILED_HEADLINE));
    }

    #[test]
    fn the_notice_band_paints_the_message_it_is_handed() {
        let (painted, _) = paint(&rows(1), Some("something went wrong"), min_pane_size());
        assert!(painted.has("something went wrong"));
    }

    // ---- clicks ----------------------------------------------------------

    /// Presses the widget whose painted text is `label`, `nth` occurrence,
    /// and returns the action the pane reported.
    ///
    /// A press **and** a release is what egui counts as a click, and the
    /// frame that locates the control cannot be the frame that clicks it --
    /// both learned the hard way elsewhere in this window.
    fn click_nth(state: &SendPaneState, label: &str, nth: usize) -> SendUiAction {
        click_nth_with(state, SendDeleteView::default(), label, nth)
    }

    /// [`click_nth`], with the window's delete state as the pane would really
    /// be handed it -- so a confirmation can be armed and then answered.
    fn click_nth_with(
        state: &SendPaneState,
        delete: SendDeleteView<'_>,
        label: &str,
        nth: usize,
    ) -> SendUiAction {
        let size = min_pane_size();
        let ctx = egui::Context::default();
        let base = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(base(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(base(), |_ui| {});

        let output = ctx.run_ui(base(), |ui| {
            let _ = draw_send_pane(ui, state, None, delete, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        let mut painted = Painted { text: Vec::new(), rects: Vec::new(), text_rects: Vec::new() };
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        let targets: Vec<egui::Rect> = painted
            .text_rects
            .iter()
            .filter(|(t, _)| t == label)
            .map(|(_, r)| *r)
            .collect();
        assert!(
            targets.len() > nth,
            "only {} widgets labelled {label:?} were painted, so clicking the {nth}th would \
             click nothing",
            targets.len()
        );
        let pos = targets[nth].center();

        let press = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            ..base()
        };
        let mut action = SendUiAction::None;
        let _ = ctx.run_ui(press, |ui| {
            let _ = draw_send_pane(ui, state, None, delete, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        let release = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            ..base()
        };
        let _ = ctx.run_ui(release, |ui| {
            action = draw_send_pane(ui, state, None, delete, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        action
    }

    /// **A row with no URL cannot clear the clipboard.** `parse_send_list`
    /// rejects a *missing* `accessUrl` but accepts `""`, so this row shape is
    /// reachable from a real server answer. The button is still painted --
    /// the row keeps its shape -- but pressing it reports nothing, because
    /// `CopyLink("")` reaches `copy_secret("")`, which empties the clipboard
    /// and then tells the user it copied a link.
    #[test]
    fn a_row_whose_url_is_empty_paints_its_button_but_copies_nothing() {
        let state = SendPaneState::Rows(vec![SendRow {
            id: "id-no-url".into(),
            name: "no url".into(),
            expiry: "Expires in 7 days".into(),
            is_file: false,
            access_url: String::new(),
        }]);
        let (painted, _) = paint(&state, None, min_pane_size());
        assert_eq!(
            painted.count("Copy link"),
            1,
            "the row lost its button entirely, so this test would pass against a row that is \
             not drawn at all"
        );
        assert_eq!(
            click_nth(&state, "Copy link", 0),
            SendUiAction::None,
            "pressing Copy link on a row with an empty URL reported a copy -- that call clears \
             the clipboard and reports success"
        );
    }

    /// **A failure is not printed twice.** The window turns a failed fetch
    /// into the inline notice AND `pane_state` turns the same `SendError`
    /// into `Failed { message }`, both from `SendError::user_message`. Handed
    /// both, the pane must show the sentence once.
    #[test]
    fn a_failure_that_is_also_the_notice_is_shown_once_and_not_twice() {
        let message = SendError::Offline.user_message().to_string();
        let state = SendPaneState::Failed { message: message.clone(), ambiguous: false };
        let (painted, _) = paint(&state, Some(message.as_str()), min_pane_size());
        assert!(
            painted.has(FAILED_HEADLINE),
            "the failure was not drawn at all, so the count below would be vacuous"
        );
        assert_eq!(
            painted.count(message.as_str()),
            1,
            "{message:?} was painted {} times -- the notice band and the failure body are \
             printing the same sentence, which reads as two failures",
            painted.count(message.as_str())
        );
    }

    /// ...and the de-duplication is by **content**, not by "the pane is
    /// failed". A move or generate error arriving while the Sends fetch has
    /// failed is a different message and must still be shown.
    #[test]
    fn a_notice_that_is_not_the_failure_is_still_shown_beside_it() {
        let message = SendError::Offline.user_message().to_string();
        let state = SendPaneState::Failed { message: message.clone(), ambiguous: false };
        let other = "Could not move that item.";
        let (painted, _) = paint(&state, Some(other), min_pane_size());
        assert!(
            painted.has(other),
            "an unrelated notice was swallowed by the failure body"
        );
        assert_eq!(painted.count(message.as_str()), 1);
    }

    /// **Copy link copies the row it was clicked on.** Clicked on the *last*
    /// row of six, because a wrong-row bug that reaches for index 0 is
    /// invisible when the test clicks the first one.
    #[test]
    fn copy_link_reports_the_url_of_the_row_it_was_clicked_on() {
        let state = rows(6);
        let SendPaneState::Rows(model) = &state else { panic!("not rows") };
        for index in [0usize, 3, 5] {
            let expected = model[index].access_url.clone();
            assert_eq!(
                click_nth(&state, "Copy link", index),
                SendUiAction::CopyLink(expected.clone()),
                "the Copy link button on row {index} did not report {expected}"
            );
        }
    }

    #[test]
    fn a_failure_can_be_retried_from_the_pane_itself() {
        let state = SendPaneState::Failed {
            message: SendError::Offline.user_message().to_string(),
            ambiguous: false,
        };
        assert_eq!(click_nth(&state, "Try again", 0), SendUiAction::Refresh);
    }

    #[test]
    fn refresh_is_clickable_at_the_minimum_window_size_in_every_state() {
        for state in [
            SendPaneState::Loading,
            SendPaneState::Empty,
            rows(6),
            SendPaneState::Failed { message: "x".to_string(), ambiguous: false },
        ] {
            assert_eq!(
                click_nth(&state, "Refresh", 0),
                SendUiAction::Refresh,
                "Refresh was not reachable in {state:?}"
            );
        }
    }

    #[test]
    fn clicking_the_notice_band_dismisses_it() {
        let size = min_pane_size();
        let ctx = egui::Context::default();
        let base = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(base(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(base(), |_ui| {});
        let state = SendPaneState::Empty;
        let output = ctx.run_ui(base(), |ui| {
            let _ = draw_send_pane(ui, &state, Some("a message"), SendDeleteView::default(), &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        let mut painted = Painted { text: Vec::new(), rects: Vec::new(), text_rects: Vec::new() };
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        let pos = painted.rect_of("a message").expect("the band was not painted").center();
        let press = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            ..base()
        };
        let _ = ctx.run_ui(press, |ui| {
            let _ = draw_send_pane(ui, &state, Some("a message"), SendDeleteView::default(), &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        let release = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            ..base()
        };
        let mut action = SendUiAction::None;
        let _ = ctx.run_ui(release, |ui| {
            action = draw_send_pane(ui, &state, Some("a message"), SendDeleteView::default(), &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        assert_eq!(action, SendUiAction::DismissNotice);
    }

    // ---- the revoke affordance, step 4 -----------------------------------

    /// Presses the pane at an EXACT position and returns what it reported.
    ///
    /// Separate from [`click_nth_with`] because the mis-click test below has
    /// to click a remembered *pixel* rather than a label: the whole question
    /// there is what is under the pointer after the pane has been redrawn,
    /// and looking the target up by name a second time is precisely the step
    /// a mis-clicking user does not take.
    fn click_at_with(
        state: &SendPaneState,
        delete: SendDeleteView<'_>,
        pos: egui::Pos2,
    ) -> SendUiAction {
        let size = min_pane_size();
        let ctx = egui::Context::default();
        let base = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(base(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(base(), |_ui| {});
        let _ = ctx.run_ui(base(), |ui| {
            let _ = draw_send_pane(ui, state, None, delete, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        let press = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            ..base()
        };
        let _ = ctx.run_ui(press, |ui| {
            let _ = draw_send_pane(ui, state, None, delete, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        let release = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            ..base()
        };
        let mut action = SendUiAction::None;
        let _ = ctx.run_ui(release, |ui| {
            action = draw_send_pane(ui, state, None, delete, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        action
    }

    /// Where the `nth` widget labelled `label` was painted, in the pane drawn
    /// with `delete`.
    fn rect_of_nth(state: &SendPaneState, delete: SendDeleteView<'_>, label: &str, nth: usize) -> egui::Rect {
        let (painted, _) = paint_with(state, None, min_pane_size(), delete);
        let targets: Vec<egui::Rect> = painted
            .text_rects
            .iter()
            .filter(|(t, _)| t == label)
            .map(|(_, r)| *r)
            .collect();
        assert!(
            targets.len() > nth,
            "only {} widgets labelled {label:?} were painted, so there is no {nth}th to locate",
            targets.len()
        );
        targets[nth]
    }

    /// **Every row can be revoked, and the first click revokes nothing.**
    ///
    /// Both halves matter and the second is the requirement: `bw send delete`
    /// takes a public link down and there is no undo, so a control that acted
    /// on one click would be a control that destroys on a mis-aim.
    #[test]
    fn every_row_has_a_delete_button_and_one_click_only_asks() {
        let state = rows(6);
        let (painted, _) = paint_with(&state, None, min_pane_size(), SendDeleteView::default());
        assert_eq!(
            painted.text_rects.iter().filter(|(t, _)| t == DELETE_LABEL).count(),
            6,
            "six rows were drawn but not six Delete buttons: {:?}",
            painted.text
        );
        // Nothing destructive is even OFFERED before the first click.
        assert!(
            !painted.has(CONFIRM_LABEL),
            "the destructive button is painted before anything was asked: {:?}",
            painted.text
        );

        for nth in [0usize, 5] {
            let action = click_nth_with(&state, SendDeleteView::default(), DELETE_LABEL, nth);
            assert_eq!(
                action,
                SendUiAction::AskDelete(format!("id{nth}")),
                "the Delete button on row {nth} did not ask about row {nth}"
            );
        }
    }

    /// **The confirmation is shown on exactly one row, and it is the row that
    /// was asked about.**
    #[test]
    fn only_the_row_asked_about_shows_the_confirmation() {
        let state = rows(4);
        let armed = SendDeleteView { confirming: Some("id2"), in_flight: None };
        let (painted, _) = paint_with(&state, None, min_pane_size(), armed);
        assert_eq!(
            painted.text_rects.iter().filter(|(t, _)| t == CONFIRM_LABEL).count(),
            1,
            "the destructive button is on {} rows, not one: {:?}",
            painted.text_rects.iter().filter(|(t, _)| t == CONFIRM_LABEL).count(),
            painted.text
        );
        assert_eq!(
            painted.text_rects.iter().filter(|(t, _)| t == CANCEL_LABEL).count(),
            1,
            "the way out of the confirmation is not on exactly one row"
        );
        assert_eq!(
            painted.text_rects.iter().filter(|(t, _)| t == DELETE_LABEL).count(),
            3,
            "the other three rows lost their Delete button, or the armed row kept its own"
        );
        assert!(
            painted.has(CONFIRM_PROMPT),
            "the row that is about to be revoked does not say what that means: {:?}",
            painted.text
        );

        // And it answers for its own row and no other.
        assert_eq!(
            click_nth_with(&state, armed, CONFIRM_LABEL, 0),
            SendUiAction::ConfirmDelete {
                id: "id2".to_string(),
                name: "send-number-2".to_string(),
            },
            "the confirmation answered for a row other than the one it was asked about"
        );
    }

    /// **THE MIS-CLICK DEFENCE, as a click on a remembered pixel.**
    ///
    /// A user who double-clicks Delete, or who clicks it twice because the
    /// first click did not seem to register, puts the second click at the
    /// same coordinates as the first. Those coordinates must not be a
    /// destructive control on the redrawn frame. They are `Cancel`, and this
    /// asserts it the only way that means anything: by clicking the position
    /// the first click was made at, without looking anything up again.
    #[test]
    fn a_second_click_where_delete_was_cancels_and_never_destroys() {
        let state = rows(3);
        let idle = SendDeleteView::default();
        let where_delete_was = rect_of_nth(&state, idle, DELETE_LABEL, 1).center();

        // The first click arms the confirmation for that row.
        assert_eq!(
            click_at_with(&state, idle, where_delete_was),
            SendUiAction::AskDelete("id1".to_string()),
            "control: the remembered position is not the Delete button of row 1"
        );

        // The second click, at the very same pixel, on the redrawn pane.
        let armed = SendDeleteView { confirming: Some("id1"), in_flight: None };
        let second = click_at_with(&state, armed, where_delete_was);
        assert_eq!(
            second,
            SendUiAction::CancelDelete,
            "the pixel the Delete button occupied does something other than cancel once the \
             confirmation is up -- a double-click on Delete would revoke a public link with \
             no decision taken"
        );
        assert!(
            !matches!(second, SendUiAction::ConfirmDelete { .. }),
            "a second click at the Delete button's own position REVOKED the Send"
        );

        // And the destructive button is really somewhere else, so the
        // assertion above is about geometry and not about a button that was
        // never drawn.
        let confirm = rect_of_nth(&state, armed, CONFIRM_LABEL, 0);
        assert!(
            !confirm.contains(where_delete_was),
            "control: the destructive button covers the Delete button's own position \
             ({confirm:?} contains {where_delete_was:?}), so cancelling there is an accident \
             of hit-testing order rather than a layout decision"
        );
    }

    /// **A row whose revoke is running has no control on it at all**, so a
    /// second click cannot start a second `bw send delete` for a Send that is
    /// already being revoked.
    #[test]
    fn a_row_being_revoked_has_no_buttons_and_says_so() {
        let state = rows(3);
        let busy = SendDeleteView { confirming: None, in_flight: Some("id1") };
        let (painted, _) = paint_with(&state, None, min_pane_size(), busy);

        assert!(
            painted.has(DELETING_LABEL),
            "the row being revoked does not say that anything is happening: {:?}",
            painted.text
        );
        assert_eq!(
            painted.text_rects.iter().filter(|(t, _)| t == DELETE_LABEL).count(),
            2,
            "the revoking row kept a Delete button, or the other two lost theirs"
        );
        assert_eq!(
            painted.text_rects.iter().filter(|(t, _)| t == "Copy link").count(),
            2,
            "the revoking row kept its Copy link button"
        );
        assert!(
            !painted.has(CONFIRM_LABEL),
            "a destructive button is painted on a row already being revoked"
        );

        // Every pixel of the row reports nothing. Swept across the whole row
        // rather than at one point, because "no button" has to be true of the
        // whole strip and a single sample can miss a control by ten pixels.
        let row_line = painted
            .rect_of(DELETING_LABEL)
            .expect("counted above");
        for x in [0.15f32, 0.35, 0.55, 0.75, 0.85, 0.93, 0.98] {
            let pos = egui::pos2(min_pane_size().x * x, row_line.center().y);
            assert_eq!(
                click_at_with(&state, busy, pos),
                SendUiAction::None,
                "a click at {pos:?} on a row that is already being revoked reported an action"
            );
        }
    }

    /// A Send whose id did not survive the parse cannot be revoked, for the
    /// reason a row with no URL cannot be copied: the button keeps the row's
    /// shape, and the action it would report is refused.
    #[test]
    fn a_row_with_no_id_paints_its_button_but_revokes_nothing() {
        let state = SendPaneState::Rows(vec![SendRow {
            id: String::new(),
            name: "no id".into(),
            expiry: "Expires in 7 days".into(),
            is_file: false,
            access_url: "https://send.bitwarden.com/#/x".into(),
        }]);
        assert_eq!(
            click_nth_with(&state, SendDeleteView::default(), DELETE_LABEL, 0),
            SendUiAction::None,
            "a row with no id asked to revoke something `bw` could not name"
        );
    }

    /// The two steps do not share a word, so the second click is a decision
    /// rather than muscle memory.
    #[test]
    fn the_two_steps_are_not_labelled_the_same_thing() {
        assert_ne!(DELETE_LABEL, CONFIRM_LABEL);
        assert_ne!(CANCEL_LABEL, CONFIRM_LABEL);
        assert!(
            CONFIRM_LABEL.len() > DELETE_LABEL.len(),
            "the destructive label says no more than the harmless one does"
        );
    }
}

#[cfg(test)]
mod source_pins {
    //! Facts about `vault_window::mod`'s render closure and its helpers that
    //! no test in this crate can reach, because they are statements inside an
    //! `eframe` frame closure that only a real window runs.
    //!
    //! Each needle is `concat!`-split so it cannot match its own declaration,
    //! and each is a single line, so a CRLF checkout cannot make it vacuous.
    //! Each is *required*, so the assertion is its own evidence that it still
    //! matches live code.
    //!
    //! **What is deliberately NOT here any more.** "The fetch is not on the
    //! eframe thread" used to be pinned, by slicing `spawn_send_list` out of
    //! this file and asserting `std::thread::spawn`, `list_sends` and
    //! `CliSendRunner::new` were somewhere inside it. Every one of those
    //! needles is satisfied by hoisting the blocking call *above* the spawn:
    //!
    //! ```ignore
    //! let answer = crate::send::list_sends(&runner);   // on the eframe thread
    //! std::thread::spawn(move || { let _ = tx.send(answer); ... });
    //! ```
    //!
    //! -- which is the exact defect the pin existed to prevent, passing the
    //! pin. The property is **closure**-wide and the slice was
    //! **function**-wide. It is now held behaviourally instead, by
    //! `fetch_thread_tests`, against `spawn_send_list_with`, whose `fetch` is
    //! a value with nothing to hoist out of.
    //!
    //! What is left here is the seam between that tested function and
    //! production: that `spawn_send_list` is *nothing but* the delegation, and
    //! that the fetch it delegates is the real one.

    /// What the compiler builds into the shipped binary from `mod.rs`.
    ///
    /// **This was a first-occurrence cut** -- `&source[..source.find(gate)]`
    /// -- propped up by a separate shape walk
    /// (`nothing_but_gated_test_modules_lives_below_the_pins_cut`) asserting
    /// that `mod.rs` really is "production, then nothing but test modules to
    /// EOF". That is the exact fragility [`production_region`] was written to
    /// remove one file over, and leaving it standing here meant the two files
    /// this seal covers were read by two different rules, only one of which
    /// had been beaten into shape by measurement. Both go through the same
    /// function now. The shape walk stays: it is a true statement about
    /// `mod.rs` and a useful one, but nothing depends on it any more.
    ///
    /// Note the return type changed with it -- the region is *built*, not
    /// borrowed, and it is already [`sanitized`], so callers that used to
    /// sanitize it themselves no longer need to (doing so anyway is
    /// harmless: `sanitized` preserves byte length and is idempotent over
    /// already-blanked text).
    fn production() -> String {
        production_region_source(include_str!("mod.rs"))
    }

    /// The same idea over **this** file. `production()` reads `mod.rs` only,
    /// which is how a blocking fetch written one file over stayed invisible.
    ///
    /// Not the same *cut*, though: this file has four `#[cfg(test)]` modules
    /// and a first-occurrence cut would hand back 718 lines out of 2808 and
    /// call the other 2090 "test code" without looking. It goes through
    /// [`production_region`], which removes each gated item by brace-matching
    /// and keeps everything between them.
    fn this_files_production() -> String {
        production_region(include_str!("send_ui.rs"))
    }

    /// Every `.rs` file under `deskwarden/src`, walked at test time, as
    /// `(path relative to `src` with `/` separators, contents)`.
    ///
    /// **A directory walk and not an `include_str!` list**, because a list is
    /// a thing somebody has to remember to extend and a file added next month
    /// would simply not be looked at. `CARGO_MANIFEST_DIR` is the crate root
    /// at compile time; this reads from disk, which is the same thing every
    /// source pin in this crate already does through `include_str!`, and
    /// touches nothing but the crate's own sources.
    fn crate_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, String)>) {
            let entries =
                std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {dir:?}: {e}"));
            let mut paths: Vec<std::path::PathBuf> =
                entries.map(|e| e.expect("a directory entry").path()).collect();
            paths.sort();
            for path in paths {
                if path.is_dir() {
                    walk(&path, root, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    let rel = path
                        .strip_prefix(root)
                        .expect("walked below the root")
                        .to_string_lossy()
                        .replace('\\', "/");
                    let text = std::fs::read_to_string(&path)
                        .unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
                    out.push((rel, text));
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        walk(&root, &root, &mut out);
        out
    }

    /// `text` with every comment and the *contents* of every string, raw
    /// string and character literal replaced by spaces, byte-for-byte in
    /// length so that byte offsets still line up with the original, and
    /// newlines preserved so that line-oriented reads still line up too.
    ///
    /// **Why.** Two reasons, and they are the two ends of the same problem.
    ///
    /// A needle counted in a *comment* is a false positive: writing
    /// `list_sends(` into a doc comment in `job_object.rs` would have turned
    /// the crate walk below red for no behavioural reason at all, which is
    /// the fastest way to get a guard deleted. And a `#[cfg(test)]` written
    /// inside a comment or a string -- this file's own pins are full of
    /// `concat!("#[cfg(", "test)]")` precisely to avoid it -- is a false
    /// *gate*, which would make [`production_region`] discard live production
    /// code and go blind exactly where it must not.
    ///
    /// String literal contents are blanked rather than kept: a call written
    /// in a string is not a call. The delimiters are kept, so the text still
    /// tokenises.
    fn sanitized(text: &str) -> String {
        let b = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut i = 0usize;
        // Blank `n` bytes from `i`, keeping any newline in them.
        let blank = |out: &mut String, from: usize, to: usize| {
            for &c in &b[from..to] {
                // Carriage returns are kept as well as newlines: a pin
                // written as `"\r\n        if let .."` must still match a
                // line whose predecessor was a comment.
                out.push(match c {
                    b'\n' => '\n',
                    b'\r' => '\r',
                    _ => ' ',
                });
            }
        };
        while i < b.len() {
            match b[i] {
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    let start = i;
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                    blank(&mut out, start, i);
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    // Rust block comments nest.
                    let start = i;
                    let mut depth = 0usize;
                    while i < b.len() {
                        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                            depth += 1;
                            i += 2;
                        } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                            depth -= 1;
                            i += 2;
                            if depth == 0 {
                                break;
                            }
                        } else {
                            i += 1;
                        }
                    }
                    blank(&mut out, start, i);
                }
                b'r' if matches!(b.get(i + 1), Some(&b'"') | Some(&b'#')) => {
                    let mut j = i + 1;
                    let mut hashes = 0usize;
                    while b.get(j) == Some(&b'#') {
                        hashes += 1;
                        j += 1;
                    }
                    if b.get(j) != Some(&b'"') {
                        // Not a raw string: an identifier like `r#type`, or
                        // `r` followed by something else. Copy one byte.
                        out.push('r');
                        i += 1;
                        continue;
                    }
                    out.push('r');
                    blank(&mut out, i + 1, j + 1);
                    i = j + 1;
                    let close: Vec<u8> =
                        std::iter::once(b'"').chain(std::iter::repeat(b'#').take(hashes)).collect();
                    let start = i;
                    while i < b.len() && !b[i..].starts_with(&close) {
                        i += 1;
                    }
                    blank(&mut out, start, i);
                    let end = (i + close.len()).min(b.len());
                    blank(&mut out, i, end);
                    i = end;
                }
                b'"' => {
                    out.push('"');
                    i += 1;
                    let start = i;
                    while i < b.len() {
                        if b[i] == b'\\' {
                            i = (i + 2).min(b.len());
                        } else if b[i] == b'"' {
                            break;
                        } else {
                            i += 1;
                        }
                    }
                    blank(&mut out, start, i.min(b.len()));
                    if i < b.len() {
                        out.push('"');
                        i += 1;
                    }
                }
                b'\'' => {
                    // A character literal, or a lifetime. `'a'`, `'\n'`,
                    // `'\\'` and `'\u{1f}'` all end at the next unescaped
                    // quote on the same line; a lifetime has none.
                    let mut j = i + 1;
                    if b.get(j) == Some(&b'\\') {
                        j += 1;
                        while j < b.len() && b[j] != b'\'' && b[j] != b'\n' {
                            j += 1;
                        }
                    } else {
                        while j < b.len() && (b[j] & 0xc0) == 0x80 {
                            j += 1;
                        }
                        j += 1;
                    }
                    if b.get(j) == Some(&b'\'') {
                        out.push('\'');
                        blank(&mut out, i + 1, j);
                        out.push('\'');
                        i = j + 1;
                    } else {
                        out.push('\'');
                        i += 1;
                    }
                }
                c => {
                    // Multi-byte UTF-8 is copied whole so the output stays
                    // valid UTF-8 and the same byte length.
                    let width = if c < 0x80 {
                        1
                    } else if c >= 0xf0 {
                        4
                    } else if c >= 0xe0 {
                        3
                    } else {
                        2
                    };
                    let end = (i + width).min(b.len());
                    out.push_str(&text[i..end]);
                    i = end;
                }
            }
        }
        out
    }

    /// Whatever the compiler builds into the shipped binary from `text`:
    /// everything that is not inside a `#[cfg(test)]`-gated item, with
    /// comments and literal contents blanked by [`sanitized`].
    ///
    /// **This used to be `text[..text.find("#[cfg(test)]")]`, and that was a
    /// hole big enough to drive the whole defect through.** A first-occurrence
    /// cut is only correct for a file shaped as "production, then nothing but
    /// test modules to EOF" -- and `mod.rs` is that shape only because a
    /// dedicated walk (`nothing_but_gated_test_modules_lives_below_the_pins_cut`)
    /// asserts it every run. **This** file has FOUR `#[cfg(test)]` modules,
    /// and the crate walk hands this function fifty files whose shape nothing
    /// asserts at all. Measured on `4446e9a`: the reviewer inserted
    ///
    /// ```ignore
    /// pub(super) fn prefetch_now() -> Result<Vec<SendSummary>, SendError> {
    ///     let runner = crate::send::CliSendRunner::new(None, /* data dir */);
    ///     crate::send::list_sends(&runner)
    /// }
    /// ```
    ///
    /// at column zero between `mod tests` and `mod fetch_thread_tests` -- a
    /// position that compiles into the shipped binary and that the old cut
    /// could not see -- plus a call in the frame closure. 2061 lib + 217 bin,
    /// 0 failed. A sixty-second freeze, green.
    ///
    /// So the gated items are *removed*, not cut at: each gate is followed to
    /// its item's extent -- the matching `}` of the item's brace group, the
    /// `;` that ends a braceless item such as `#[cfg(test)] use foo::bar;`,
    /// or the `,` that ends a gated **struct field, enum variant, tuple
    /// element or match arm** -- and everything else is kept, however many
    /// times production and tests interleave.
    ///
    /// **The `,` and the depth tracking are the second round's fix, and the
    /// hole they close was strictly worse than the one above.** The first
    /// version of this scanner knew only two terminators, `;` and `{..}`, and
    /// tracked no enclosing brace at all. A gate on an item that has neither
    /// -- a struct field is the shortest -- therefore ran *past the closing
    /// brace of its own parent* and deleted everything up to the next `;` or
    /// brace group ANYWHERE later in the file. Where the first-occurrence cut
    /// was blind only after a point, this was blind at a point of the
    /// author's choosing. Measured on `cbe915e`, in this file's production
    /// immediately above `mod fetch_thread_tests`:
    ///
    /// ```ignore
    /// struct GateHole {
    ///     #[cfg(test)]
    ///     marker: u32,
    ///     real: u32,
    /// }
    ///
    /// pub(super) fn prefetch_now() -> Result<Vec<SendSummary>, SendError> {
    ///     let runner = crate::send::CliSendRunner::new(None, /* data dir */);
    ///     crate::send::list_sends(&runner)
    /// }
    /// ```
    ///
    /// plus the call in the frame closure: the region ended at
    /// `struct GateHole {` and resumed after `prefetch_now`'s body, so both
    /// banned spellings were written out in full inside a function that ran
    /// on the eframe thread every frame. 2068 lib + 217 bin, 0 failed.
    ///
    /// So the walk now carries its own brace and paren depth and **stops
    /// before any closer it did not open**: a `}` or `)` or `]` at local
    /// depth zero is the parent's, not the item's, and the walk ends there
    /// without consuming it. A gated item can therefore never swallow code
    /// beyond its own syntactic parent, whatever shape it has. The walk may
    /// still *under*-consume -- a gated field whose type is
    /// `HashMap<K, V>` ends at the `,` inside the generics, since `<` and `>`
    /// are not tracked (they are not reliably delimiters in Rust) -- and that
    /// direction is deliberate: under-consuming leaves a fragment of TEST
    /// code inside the region, which can only ever turn a guard red, while
    /// over-consuming deletes PRODUCTION code, which is how a guard goes
    /// blind. Every ambiguity here resolves toward the red.
    ///
    /// **Which attributes count as a gate**, in the same direction. The gate
    /// used to be the literal `#[cfg(test)]`, so `#[cfg(all(test, windows))]`
    /// was not recognised and its test module stayed in the "production"
    /// region -- harmless for hiding, but it reds the crate walk the first
    /// time somebody writes a perfectly normal platform-scoped test module.
    /// The rule is now: the attribute is `cfg(..)` and its predicate
    /// *implies* `test`, i.e. a bare `test` reached through nothing but
    /// `all(..)`. Deliberately NOT gates:
    ///
    ///  * `#[cfg(any(test, ..))]` -- `any` is satisfiable without `test`, so
    ///    the item may well be in the shipped binary. Treating it as a gate
    ///    would make `#[cfg(any(test, all()))]` -- which is unconditionally
    ///    true -- a one-line invisibility cloak over any production item.
    ///  * `#[cfg(not(test))]` -- that is production code by definition.
    ///  * `#[cfg_attr(test, ..)]` -- it adds an *attribute* under test; the
    ///    item itself is production either way, and removing it would be the
    ///    Critical above with a different spelling.
    ///
    /// The alternative considered and rejected was to require every file in
    /// the crate to have `mod.rs`'s "one trailing run of test modules" shape,
    /// enforced by a walk. That is a real property and it is what holds
    /// `mod.rs`, but imposing it on ~50 files reshapes files this round has
    /// no business reshaping, and it fixes the *files* rather than the
    /// *function* -- the next file added is out of shape until somebody
    /// notices. Handling the interleaving is the honest fix, and
    /// `the_production_region_survives_interleaved_test_items` runs it
    /// against interleaving that the old cut got wrong.
    fn production_region(text: &str) -> String {
        sanitized(&production_region_source(text))
    }

    /// [`production_region`] before the blanking: the same spans of `text`,
    /// in the original, with comments and literals still readable.
    ///
    /// The gates are still located over the *sanitized* copy -- that is the
    /// whole of how a `#[cfg(test)]` written in a doc comment stays a piece
    /// of prose -- and [`sanitized`] preserves byte length and character
    /// boundaries exactly, so every offset it yields indexes the original.
    ///
    /// Two callers want the two halves and they are not the same want. A
    /// needle counted for a *behavioural* claim ("nothing calls `.recv()`")
    /// must not see comments, so it takes the blanked form. A needle that
    /// pins a *literal* -- `egui::Panel::left("vault-item-list")` -- is
    /// pinning the string's contents, and blanking them turns that pin into
    /// a match against nothing at all.
    fn production_region_source(text: &str) -> String {
        let clean = sanitized(text);
        let b = clean.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut i = 0usize;
        let mut kept_from = 0usize;
        while i < b.len() {
            if b[i] == b'#' {
                if let Some((body, after)) = attribute_span(b, i) {
                    if attribute_implies_test(&clean[body.0..body.1]) {
                        out.push_str(&text[kept_from..i]);
                        i = gated_item_end(b, after);
                        kept_from = i;
                    } else {
                        i = after;
                    }
                    continue;
                }
            }
            i += 1;
        }
        out.push_str(&text[kept_from..]);
        out
    }

    /// If `b[at]` opens an attribute (`#[..]` or `#![..]`), the byte range of
    /// its *contents* and the offset just past its closing `]`.
    ///
    /// Separated from [`production_region`] so the gate decision is made on
    /// the attribute's own text rather than on a substring match that cannot
    /// tell `cfg(test)` from `cfg(not(test))`.
    fn attribute_span(b: &[u8], at: usize) -> Option<((usize, usize), usize)> {
        let mut i = at + 1;
        if b.get(i) == Some(&b'!') {
            i += 1;
        }
        if b.get(i) != Some(&b'[') {
            return None;
        }
        let body_start = i + 1;
        let mut i = body_start;
        let mut square = 1usize;
        let mut round = 0usize;
        while i < b.len() {
            match b[i] {
                b'[' => square += 1,
                b'(' => round += 1,
                b')' => round = round.saturating_sub(1),
                b']' => {
                    square -= 1;
                    if square == 0 && round == 0 {
                        return Some(((body_start, i), i + 1));
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Whether an attribute's contents mean "this item exists only under
    /// `cfg(test)`", and so may be removed from the production region.
    ///
    /// See [`production_region`]'s doc for the cases this deliberately says
    /// `false` to. The shape of the answer: the head identifier must be
    /// exactly `cfg`, and a bare `test` token must be reachable through
    /// nothing but `all(..)` -- any `not(..)`, `any(..)` or unknown
    /// combinator on the way makes it a maybe, and a maybe is production.
    /// The arms of an `any(..)` that CANNOT hold in any build of this crate,
    /// removed -- and an `any(..)` left with one arm rewritten to `all(..)`,
    /// which is what `any(x)` and `all(x)` both mean.
    ///
    /// **Why this exists.** `#[cfg(any(test, unix))] mod ..` is an item that
    /// provably does not ship: this crate depends on the `windows` crate
    /// unconditionally and contains no `cfg(unix)`, no `target_os` and no
    /// `target_family` anywhere, so it builds for exactly one target family.
    /// **M-G**, measured red before this: the module was kept whole, and the
    /// crate-wide call-site walk reported `list_sends` as called from this
    /// file's production code. Over-reporting is the SAFE direction, which is
    /// why this was Important and not Critical -- but a guard that accuses
    /// production code of something untrue is a guard the next developer
    /// weakens, and that is the expensive failure.
    ///
    /// **What this deliberately does NOT do.** It does not make `any(test,
    /// ..)` a gate. That would be unsound and the reason is exact: `all()`
    /// with zero arguments is unconditionally TRUE in `cfg`, so
    /// `#[cfg(any(test, all()))]` really does ship, and stripping it would
    /// delete production code from the region every guard in this module
    /// reads. Only arms on an explicit, closed list are removed:
    ///
    ///  * `unix`, and `all(unix)`;
    ///  * `not(windows)`.
    ///
    /// Anything else -- a feature, an unknown combinator, a `target_os` key
    /// whose value `sanitized` has blanked -- survives, the `any` keeps two
    /// or more arms, and the item stays in production. If this crate ever
    /// grows a second target family, this list is the one thing to delete.
    fn without_impossible_arms(body: &str) -> String {
        let trimmed = body.trim();
        let Some(open) = trimmed.find('(') else {
            return trimmed.to_string();
        };
        let head = &trimmed[..open];
        if head.is_empty() || !head.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return trimmed.to_string();
        }
        // The `(` at `open` must be closed by the LAST byte, or this is not
        // one combinator applied to a list and nothing here understands it.
        let mut depth = 0usize;
        for (at, c) in trimmed.char_indices().skip(open) {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 && at + 1 != trimmed.len() {
                        return trimmed.to_string();
                    }
                }
                _ => {}
            }
        }
        if depth != 0 {
            return trimmed.to_string();
        }
        let inner = &trimmed[open + 1..trimmed.len() - 1];
        let mut arms: Vec<String> =
            top_level_arms(inner).iter().map(|a| without_impossible_arms(a)).collect();
        if head == "any" {
            arms.retain(|arm| !cannot_hold_in_any_build(arm));
            if arms.len() == 1 {
                return format!("all({})", arms[0]);
            }
        }
        format!("{head}({})", arms.join(", "))
    }

    /// `inner` split on the commas that are not inside a nested list.
    fn top_level_arms(inner: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut depth = 0usize;
        let mut from = 0usize;
        for (at, c) in inner.char_indices() {
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    out.push(inner[from..at].trim().to_string());
                    from = at + 1;
                }
                _ => {}
            }
        }
        let last = inner[from..].trim();
        if !last.is_empty() {
            out.push(last.to_string());
        }
        out
    }

    /// Whether `arm` is false in every build this crate has. The closed list
    /// [`without_impossible_arms`]'s doc names, and nothing else.
    fn cannot_hold_in_any_build(arm: &str) -> bool {
        matches!(arm, "unix" | "all(unix)" | "not(windows)")
    }

    fn attribute_implies_test(body: &str) -> bool {
        // See [`without_impossible_arms`]: this is the ONLY normalisation,
        // and it removes arms on a closed list, never `test` and never an
        // arm it does not recognise.
        let normalised = without_impossible_arms(body);
        let body = normalised.as_str();
        let b = body.as_bytes();
        // The identifier that opened each still-open `(`, outermost first.
        let mut opened: Vec<&str> = Vec::new();
        let mut i = 0usize;
        while i < b.len() {
            let c = b[i];
            if c.is_ascii_alphanumeric() || c == b'_' {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let word = &body[start..i];
                let mut j = i;
                while j < b.len() && (b[j] as char).is_whitespace() {
                    j += 1;
                }
                if b.get(j) == Some(&b'(') {
                    // A combinator (or the head `cfg` itself).
                    if opened.is_empty() && word != "cfg" {
                        // `cfg_attr`, `derive`, `allow`, ... -- not a gate at
                        // all, whatever it contains.
                        return false;
                    }
                    opened.push(word);
                    i = j + 1;
                } else if word == "test" && opened.iter().all(|f| *f == "cfg" || *f == "all") {
                    return !opened.is_empty();
                }
                continue;
            }
            if c == b')' {
                opened.pop();
            }
            i += 1;
        }
        false
    }

    /// The offset just past the end of the item a gate at `from` applies to,
    /// or -- if the item ends by running into a closer this walk did not open
    /// -- the offset of that closer, which is NOT consumed.
    ///
    /// The second half is the whole point; see [`production_region`]'s doc.
    ///
    /// **Generic parameter lists.** `,` terminates a gated item because a
    /// gate can sit on a struct field, an enum variant or a match arm, none
    /// of which is brace-delimited. But a `,` also occurs inside `<T, U>`,
    /// and until this round that ended the walk in the middle of an item
    /// header:
    ///
    /// ```ignore
    /// #[cfg(test)] fn gated_generic<T, U>(_a: T) -> Option<U> { .. }
    /// ```
    ///
    /// **M-D**, measured green at 2072 lib + 217 bin: the walk stopped after
    /// `<T,`, the body stayed in the "production region", and the crate-wide
    /// call-site walk reported that `list_sends` "is called from
    /// `vault_window/send_ui.rs` in the crate's production code". That
    /// sentence is FALSE -- the function is test-only -- and a guard that
    /// accuses production code of something untrue is a guard the next
    /// developer weakens. A gated generic test helper is an ordinary thing to
    /// write.
    ///
    /// So `<`/`>` are tracked, but conservatively, because this walk's
    /// failure modes are not symmetric: under-consuming leaves test code in
    /// the production region and over-reports (safe, and what M-D did),
    /// while OVER-consuming deletes real production code from the region and
    /// blinds every guard that reads it -- which is M-1, the defect the
    /// previous round fixed and this one must not reintroduce. Three rules
    /// bound it:
    ///
    ///  * a `<` counts only when it touches the name in front of it, so
    ///    `if a < b` and `1 < 2` open nothing;
    ///  * `{` and `}` clear the count outright, because no brace can sit
    ///    inside a generic parameter list;
    ///  * `;` terminates the item whatever the count says.
    ///
    /// Together those mean an unbalanced `<` can delay the end of an item at
    /// most to the end of the statement it is in, and never past a brace.
    fn gated_item_end(b: &[u8], from: usize) -> usize {
        let mut i = from;
        // Depths of what THIS walk opened. A closer while the matching depth
        // is zero belongs to the item's parent, and the item ends before it.
        let mut brace = 0usize;
        let mut round = 0usize;
        // Generic-parameter depth -- see this function's doc for M-D, the
        // measured mutant this exists for.
        let mut angle = 0usize;
        while i < b.len() {
            match b[i] {
                b'(' | b'[' => round += 1,
                b')' | b']' => {
                    if round == 0 && brace == 0 {
                        return i;
                    }
                    round = round.saturating_sub(1);
                }
                // A `<` that opens a generic list follows the name it belongs
                // to with nothing between (`fn f<T>`, `struct S<T>`, `impl<T>`,
                // `Foo::<T>`). A `<` with a space in front of it is a
                // comparison, and is deliberately not counted -- see the doc.
                b'<' if round == 0
                    && brace == 0
                    && matches!(
                        i.checked_sub(1).map(|p| b[p]),
                        Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'>' || c == b':'
                    ) =>
                {
                    angle += 1;
                }
                b'>' if angle > 0 => angle -= 1,
                b'{' => {
                    // A brace cannot occur inside a generic parameter list, so
                    // reaching one means whatever opened `angle` was not a
                    // generic after all. Dropping it here is what bounds the
                    // damage a miscount can do to a single item header.
                    angle = 0;
                    brace += 1;
                }
                b'}' => {
                    angle = 0;
                    if brace == 0 {
                        return i;
                    }
                    brace -= 1;
                    if brace == 0 {
                        return i + 1;
                    }
                }
                // A braceless item (`use foo;`), or a gated field, variant,
                // tuple element or match arm.
                //
                // `;` terminates whatever `angle` says, for the same reason
                // `{` clears it: a semicolon cannot sit inside a generic list,
                // so an unbalanced `<` can never carry this walk past the end
                // of the statement it is in.
                b';' if round == 0 && brace == 0 => return i + 1,
                b',' if round == 0 && brace == 0 && angle == 0 => return i + 1,
                _ => {}
            }
            i += 1;
        }
        b.len()
    }

    /// The controls on [`production_region`] and [`sanitized`]. Without them
    /// the function above is a hundred lines of unexercised string handling
    /// standing between every guard in this module and the code it guards.
    ///
    /// Each case is a shape the OLD first-occurrence cut got wrong, or one
    /// the new scanner could plausibly get wrong.
    #[test]
    fn the_production_region_survives_interleaved_test_items() {
        let gate = concat!("#[cfg(", "test)]");

        // 1. The reviewer's mutation, in miniature: production BETWEEN two
        //    gated modules. The old cut returned "keep\n" and lost `sneaked`.
        let interleaved = format!(
            "fn keep_a() {{}}\n{gate}\nmod t1 {{\n    fn inner() {{ let x = 1; }}\n}}\n\
             fn sneaked() {{ danger(); }}\n{gate}\nmod t2 {{\n    fn inner() {{}}\n}}\n\
             fn keep_b() {{}}\n"
        );
        let region = production_region(&interleaved);
        for kept in ["keep_a", "sneaked", "danger()", "keep_b"] {
            assert!(region.contains(kept), "{kept:?} was dropped from {region:?}");
        }
        for dropped in ["mod t1", "mod t2", "fn inner"] {
            assert!(!region.contains(dropped), "{dropped:?} survived in {region:?}");
        }

        // 1b. **M-D**: a gated GENERIC. The `,` inside `<T, U>` used to end
        //     the walk in the middle of the item header, leaving the body in
        //     the production region -- and the crate-wide call-site walk then
        //     reported that `list_sends` is called from this file's
        //     production code, which was not true. See `gated_item_end`.
        let generic = format!(
            "fn keep_c() {{}}\n{gate}\nfn gated_generic<T, U>(_a: T) -> Option<U> \
             {{ list_sends(); None }}\nfn keep_d() {{}}\n"
        );
        let region = production_region(&generic);
        for kept in ["keep_c", "keep_d"] {
            assert!(region.contains(kept), "{kept:?} was dropped from {region:?}");
        }
        for dropped in ["gated_generic", "list_sends", "Option<U>"] {
            assert!(
                !region.contains(dropped),
                "{dropped:?} survived in {region:?} -- a gated generic test helper is still \
                 being reported as production code"
            );
        }

        // 1c. The over-consume direction, which is the dangerous one (M-1).
        //     A `<` used as a comparison must not swallow the production code
        //     after the gated item.
        let compared = format!(
            "fn keep_e() {{}}\n{gate}\nconst SMALL: bool = 1 < 2;\nfn keep_f() \
             {{ danger(); }}\n"
        );
        let region = production_region(&compared);
        for kept in ["keep_e", "keep_f", "danger()"] {
            assert!(
                region.contains(kept),
                "{kept:?} was dropped from {region:?} -- the angle tracking over-consumed, \
                 which deletes production code from the region every guard reads"
            );
        }
        assert!(!region.contains("SMALL"), "the gated const survived in {region:?}");
        // The old rule, stated here so the improvement is measured and not
        // merely asserted in prose.
        let old_cut = &interleaved[..interleaved.find(gate).expect("a gate")];
        assert!(
            !old_cut.contains("sneaked"),
            "control: the first-occurrence cut this replaced already saw the interleaved \
             production item, so this test is not measuring anything"
        );

        // 2. Nested braces inside a gated module do not end it early.
        let nested = format!("{gate}\nmod t {{\n    fn f() {{ if x {{ y(); }} }}\n}}\nfn after() {{}}\n");
        let region = production_region(&nested);
        assert!(region.contains("fn after"), "{region:?}");
        assert!(!region.contains("y()"), "a nested brace ended the gated module early: {region:?}");

        // 3. A braceless gated item ends at its semicolon, and does not eat
        //    the file. A `[u8; 4]` in the way must not end it either.
        let braceless = format!("{gate}\nuse foo::bar;\nfn after() {{}}\n");
        assert!(production_region(&braceless).contains("fn after"));
        let with_array = format!("{gate}\nstatic S: [u8; 4] = [0; 4];\nfn after() {{}}\n");
        let region = production_region(&with_array);
        assert!(region.contains("fn after"), "a `;` inside `[..]` ended the item early: {region:?}");
        assert!(!region.contains("static S"), "{region:?}");

        // 4. A gate written in a COMMENT or a STRING is not a gate. This is
        //    the failure mode that would make the region discard live code.
        let in_prose = format!("// {gate} in prose\nfn kept() {{}}\nlet s = \"{gate}\";\nfn also() {{}}\n");
        let region = production_region(&in_prose);
        assert!(region.contains("fn kept"), "a gate in a comment cut production: {region:?}");
        assert!(region.contains("fn also"), "a gate in a string cut production: {region:?}");

        // 5. Comments and literal contents are blanked, so a needle written
        //    in either is not counted -- and the code around them survives.
        let commented = "fn a() {} // list_sends( in a comment\nfn b() {}\n\
                         /* list_sends( */ fn c() {}\nlet s = \"list_sends(\";\n";
        let region = production_region(commented);
        assert_eq!(
            region.matches(concat!("list_", "sends(")).count(),
            0,
            "a needle in a comment or a string was counted: {region:?}"
        );
        for kept in ["fn a", "fn b", "fn c", "let s"] {
            assert!(region.contains(kept), "{kept:?} was blanked with its comment: {region:?}");
        }

        // 6. `sanitized` preserves byte length and newlines, which is what
        //    lets offsets and line reads taken over it mean anything.
        let messy = "fn a() { /* x\ny */ let c = '\\''; let s = \"q\\\"z\"; }\n// tail\n";
        let clean = sanitized(messy);
        assert_eq!(clean.len(), messy.len(), "sanitized changed the byte length");
        assert_eq!(
            clean.matches('\n').count(),
            messy.matches('\n').count(),
            "sanitized changed the line count"
        );
        // And CRLF survives blanking, or every `"\r\n.."` pin taken over a
        // sanitized region silently matches nothing.
        let crlf = "// a comment\r\nfn kept() {}\r\n";
        assert!(
            sanitized(crlf).contains("\r\nfn kept() {}"),
            "a blanked CRLF comment ate the carriage return: {:?}",
            sanitized(crlf)
        );
        assert!(clean.contains("fn a() {"), "{clean:?}");
        assert!(!clean.contains("tail"), "a line comment survived: {clean:?}");

        // 7. A lifetime is not a character literal: blanking from `'a` to the
        //    next quote would swallow real code.
        let lifetimes = "fn f<'a>(x: &'a str) -> &'a str { list_sends(x) }\n";
        assert!(
            production_region(lifetimes).contains(concat!("list_", "sends(x)")),
            "lifetimes were treated as character literals: {:?}",
            production_region(lifetimes)
        );

        // 8. Raw strings, including hashed ones containing quotes.
        let raws = "let a = r\"list_sends(\"; let b = r#\"a \" list_sends( b\"#; fn kept() {}\n";
        let region = production_region(raws);
        assert_eq!(region.matches(concat!("list_", "sends(")).count(), 0, "{region:?}");
        assert!(region.contains("fn kept"), "{region:?}");

        // 9. **A GATED ITEM MAY NOT CONSUME ITS PARENT.** The measured
        //    survivor this function's second round exists for: a gate on an
        //    item with neither a `;` nor a `{..}` of its own -- a struct
        //    field, an enum variant, a tuple element, a match arm -- used to
        //    run past the enclosing `}` and delete everything up to the next
        //    terminator anywhere later in the file. `victim` stands where
        //    `prefetch_now` stood.
        for (shape, parent) in [
            ("a struct field", format!("struct S {{\n    {gate}\n    marker: u32,\n    real: u32,\n}}\n")),
            ("a trailing struct field", format!("struct S {{\n    real: u32,\n    {gate}\n    marker: u32\n}}\n")),
            ("an enum variant", format!("enum E {{\n    Real,\n    {gate}\n    Marker(u32),\n}}\n")),
            ("a tuple element", format!("struct T(u32, {gate} u32);\n")),
            ("a match arm", format!("fn f(e: E) {{\n    match e {{\n        {gate}\n        E::Marker(_) => probe(),\n        E::Real => {{ real(); }}\n    }}\n}}\n")),
            ("a fn parameter", format!("fn g(a: u32, {gate} b: u32) {{ inner(); }}\n")),
        ] {
            let source = format!("{parent}\nfn victim() {{ list_sends(&runner); }}\n");
            let region = production_region(&source);
            assert!(
                region.contains("fn victim"),
                "{shape} gated with {gate:?} swallowed the production item after its parent: \
                 {region:?}"
            );
            assert!(
                region.contains(concat!("list_", "sends(")),
                "{shape} gated with {gate:?} hid a banned call from every needle in this \
                 module: {region:?}"
            );
            assert!(
                !region.contains("marker") && !region.contains("Marker") && !region.contains("b: u32"),
                "{shape} was not removed at all: {region:?}"
            );
        }

        // 10. The same in the other direction: the gate is a PREDICATE, not a
        //     literal. `all(test, ..)` is test-only and must be removed (it
        //     is a normal thing to write and would otherwise red the crate
        //     walk); `any(..)`, `not(..)` and `cfg_attr` may all be in the
        //     shipped binary and must NOT be, or each becomes a hiding place
        //     as large as the one case 9 closes.
        let removed = "#[cfg(all(test, windows))]\nmod t { fn hidden() {} }\nfn kept() {}\n";
        let region = production_region(removed);
        assert!(region.contains("fn kept"), "{region:?}");
        assert!(!region.contains("hidden"), "`all(test, ..)` was not recognised as test-only: {region:?}");

        //     And **M-G**: an `any(..)` every one of whose other arms is
        //     false in every build this crate has. `unix` on a crate that
        //     depends on `windows` unconditionally is an item that provably
        //     does not ship, and keeping it whole reddened the crate walk
        //     with a sentence that was not true. See
        //     `without_impossible_arms` -- and note the cases just below,
        //     which stay in production, because this is NOT a rule that
        //     `any(test, ..)` is a gate.
        for impossible in ["unix", "not(windows)", "all(unix)"] {
            let source = format!(
                "#[cfg(any(test, {impossible}))]\nmod t {{ fn hidden() {{}} }}\nfn kept() {{}}\n"
            );
            let region = production_region(&source);
            assert!(region.contains("fn kept"), "{region:?}");
            assert!(
                !region.contains("hidden"),
                "`any(test, {impossible})` was kept whole, so a test-only module is still \
                 being reported as production code: {region:?}"
            );
        }
        // Arms that could BOTH hold: still production, whichever way round.
        for shipped in ["#[cfg(any(test, unix, windows))]", "#[cfg(any(test, windows))]"] {
            let source = format!("{shipped}\nmod t {{ fn hidden() {{}} }}\nfn keeper() {{}}\n");
            assert!(
                production_region(&source).contains("hidden"),
                "{shipped} was stripped, and an item that can ship is production"
            );
        }
        for (why, kept) in [
            ("`any(test, ..)` can hold without `test`", "#[cfg(any(test, feature_x))]\nfn shipped() { list_sends(&r); }\n"),
            ("`any(test, all())` is unconditionally true", "#[cfg(any(test, all()))]\nfn shipped() { list_sends(&r); }\n"),
            ("`not(test)` IS production", "#[cfg(not(test))]\nfn shipped() { list_sends(&r); }\n"),
            ("`cfg_attr` gates an attribute, not the item", "#[cfg_attr(test, derive(Debug))]\nfn shipped() { list_sends(&r); }\n"),
        ] {
            let region = production_region(kept);
            assert!(
                region.contains(concat!("list_", "sends(")),
                "{why}, so removing the item hides production code: {region:?}"
            );
        }

        // 11. Non-vacuity: over this crate's own files the region is neither
        //    everything nor nothing.
        let here = include_str!("send_ui.rs");
        let region = this_files_production();
        assert!(
            region.len() > 5_000,
            "control: this file's production region is only {} bytes",
            region.len()
        );
        assert!(
            region.len() < here.len() * 2 / 3,
            "control: this file's production region is {} of {} bytes -- the four test modules \
             are not being removed",
            region.len(),
            here.len()
        );
        assert!(
            !region.contains(concat!("mod fetch_thread_", "tests")),
            "a `#[cfg(test)]` module survived into this file's production region"
        );
        assert!(
            region.contains(concat!("fn draw_send_", "pane")),
            "the pane's own entry point is missing from this file's production region"
        );
    }

    /// A named function's body, from its opening `(` to the first `}` at the
    /// indentation the function itself is written at.
    ///
    /// `indent` is a parameter and not a hardcoded column zero because both
    /// functions this reads now live inside `mod send_fetch_thread`, and a
    /// column-zero terminator there would run to the module's own closing
    /// brace and swallow every sibling. A wrong `indent` is not silent: the
    /// terminator is required to be found rather than defaulted to
    /// end-of-slice, so the slice cannot quietly become "the rest of the
    /// file" -- which is how a body pin turns into a pin on nothing in
    /// particular.
    fn body_of(name: &str, indent: &str) -> String {
        let production = production();
        let opener = format!("fn {name}(");
        let start = production
            .find(&opener)
            .unwrap_or_else(|| panic!("`{name}` is gone from production"));
        let rest = &production[start + opener.len()..];
        let closer = format!("\r\n{indent}}}\r\n");
        let end = rest.find(&closer).unwrap_or_else(|| {
            panic!("`{name}` has no closing brace at the indentation {indent:?}")
        });
        rest[..end].to_string()
    }

    /// Every `mod NAME;` item written in `region` -- the declarations that
    /// name a SECOND FILE -- in source order, deduplicated.
    ///
    /// `mod x { .. }` is deliberately skipped: an inline module's body is in
    /// this very text, so every count taken over `region` already reads it.
    /// `mod y;` written *inside* an inline `mod x { .. }` is NOT harmless and
    /// is NOT resolved here -- it is reported like any other child, and
    /// [`send_module_files`] resolves children against the FILE, so the
    /// lookup for `send/y.rs` fails loudly rather than guessing. That panic
    /// is the guard; this function does nothing for it.
    ///
    /// The shape is lifted from `job_object.rs`'s `production_mod_children`,
    /// which is the crate's existing, transitive, fail-by-default module
    /// discovery. The one difference is that it reads a *glued* `code_only`
    /// view while this reads [`production_region`], which keeps whitespace --
    /// so `mod` must be followed by a space here rather than by the name.
    fn mod_children(region: &str) -> Vec<String> {
        let b = region.as_bytes();
        let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        let mut out: Vec<String> = Vec::new();
        let mut from = 0usize;
        while let Some(at) = region[from..].find("mod") {
            let start = from + at;
            from = start + 3;
            // `mod` must be a token: not the tail of `submod`, not the head
            // of `module_of`. Being generous in either direction costs a
            // false POSITIVE -- a name that resolves to no file, which fails
            // loudly -- never a miss.
            if (start > 0 && is_ident(b[start - 1])) || b.get(from).is_some_and(|c| is_ident(*c)) {
                continue;
            }
            let mut i = from;
            while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
                i += 1;
            }
            let name_start = i;
            while b.get(i).is_some_and(|c| is_ident(*c)) {
                i += 1;
            }
            if i == name_start {
                continue;
            }
            let name = region[name_start..i].to_string();
            while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
                i += 1;
            }
            if b.get(i) != Some(&b';') {
                continue;
            }
            if !out.contains(&name) {
                out.push(name);
            }
            from = i;
        }
        out
    }

    /// **Every file whose module path is under `crate::send`**, the root
    /// first, discovered by walking `mod` items transitively rather than from
    /// a list a new file would not be on.
    ///
    /// **Why this exists.** `crate::send`'s privacy -- the E0603 wall that
    /// `ec71706` put around `CliSendRunner` -- extends to all of that
    /// module's DESCENDANTS, and a descendant lives in a DIFFERENT FILE.
    /// Every per-file count in this module keyed on the literal path string
    /// `"send.rs"`, so the descendant was read by none of them. Measured on
    /// `89d5e8e`, a NEW FILE `src/send/inner.rs`:
    ///
    /// ```ignore
    /// use super::{SendError, SendRunner, SendSummary};
    /// pub fn warm(session: &str) -> Result<Vec<SendSummary>, SendError> {
    ///     let runner = super::CliSendRunner {   // struct literal -- private
    ///         job: None,                        // fields are visible in a
    ///         data_dir: None,                   // descendant module
    ///         session: Some(zeroize::Zeroizing::new(session.to_string())),
    ///     };
    ///     let raw = runner.run(&super::list_invocation(Some(session)))?;
    ///     let _ = raw;
    ///     Ok(Vec::new())
    /// }
    /// ```
    ///
    /// plus `pub mod inner;` in `send.rs` and
    /// `let _ = crate::send::inner::warm(&session_token);` in the frame
    /// closure, SURVIVED TWICE at 2112 lib / 217 bin / 0 failed / 0 warnings,
    /// byte-identical both runs: an unbounded per-frame, up-to-sixty-second
    /// blocking `bw send list` on the eframe thread. Every guard missed it
    /// for a different reason. The per-file counts read `send.rs`, which
    /// gained one line, `pub mod inner;`, spelling no needle. The crate-wide
    /// call-site map counts `list_sends(`, `cli_send_list(` and the two
    /// constructors -- a STRUCT LITERAL spells none of them, and
    /// `runner.run(&list_invocation(..))` bypasses `list_sends` entirely,
    /// which is the very bypass the author had documented for the
    /// in-`send.rs` case, transplanted one file over. And type privacy does
    /// nothing at all: a descendant sees the type, its private fields and the
    /// private `list_invocation` alike.
    ///
    /// The residual the previous round disclosed was understated. It was
    /// framed as "one spelling away"; it was one FILE away, and adding that
    /// file needed no counted spelling.
    ///
    /// So the counts below are taken over this set, not over one path.
    fn send_module_files(files: &[(String, String)]) -> Vec<String> {
        let has = |p: &str| files.iter().any(|(f, _)| f == p);
        let root = if has("send.rs") { "send.rs" } else { "send/mod.rs" };
        assert!(
            has(root),
            "control: the crate walk found neither `send.rs` nor `send/mod.rs`, so the module \
             whose descendants every count below reads does not exist under either name this \
             walk can follow. Resolve that rather than letting the closure fence nothing"
        );
        let mut out = vec![root.to_string()];
        let mut at = 0usize;
        while at < out.len() {
            let file = out[at].clone();
            at += 1;
            let text = files
                .iter()
                .find(|(p, _)| *p == file)
                .map(|(_, t)| t.as_str())
                .unwrap_or_else(|| {
                    panic!(
                        "the `crate::send` closure wants to read `src/{file}`, which the crate \
                         walk did not find. Do not let the closure quietly stop here"
                    )
                });
            let region = production_region(text);
            // A `#[path = ".."]` attribute re-points a `mod` item at an
            // arbitrary file -- possibly outside `src/` -- so both candidates
            // below would be wrong while the real descendant, where
            // `CliSendRunner`'s private fields and `list_invocation` are
            // still in scope, sat outside every count. There is no such
            // attribute in this module and no reason for one, so it is
            // refused outright rather than followed. (Refused the same way,
            // and for the same reason, as in `job_object.rs`'s closure.)
            let glued: String = region.chars().filter(|c| !c.is_whitespace()).collect();
            for spelling in [concat!("#[pa", "th="), concat!(",pa", "th="), concat!("(pa", "th=")] {
                assert!(
                    !glued.contains(spelling),
                    "production `src/{file}` carries a `path = ..` attribute ({spelling:?}). \
                     That re-points a `mod` item at a file this closure would not look at, \
                     which puts a descendant of `crate::send` -- where the private runner, its \
                     private fields and the private `list_invocation` are all in scope -- \
                     outside every count in this module. Put the child where its `mod` name \
                     says it goes"
                );
            }
            for child in mod_children(&region) {
                let dir = file.trim_end_matches(".rs").trim_end_matches("/mod");
                let flat = format!("{dir}/{child}.rs");
                let nested = format!("{dir}/{child}/mod.rs");
                // BOTH existing is refused rather than resolved: rustc itself
                // errors (E0761), so the two would disagree about which file
                // is even compiled and this closure would read one while the
                // other sat unread.
                let present: Vec<String> =
                    [flat, nested].into_iter().filter(|c| has(c)).collect();
                assert!(
                    present.len() < 2,
                    "production `src/{file}` declares `mod {child};` and BOTH {present:?} \
                     exist. rustc refuses that outright (E0761), so this tree does not build \
                     -- and if it somehow did, this closure would read one file and leave the \
                     other entirely uncounted. Delete whichever one is not the module"
                );
                let found = present.into_iter().next().unwrap_or_else(|| {
                    panic!(
                        "production `src/{file}` declares `mod {child};` but neither \
                         `src/{dir}/{child}.rs` nor `src/{dir}/{child}/mod.rs` exists, so this \
                         closure cannot count the file it pulls in. If the `mod` item sits \
                         inside an INLINE `mod` in this file, the real file is a directory \
                         deeper than either name above: this scan finds `mod` items wherever \
                         they are written but resolves them against the FILE, so it stops here \
                         rather than guessing, and the fix is to stop nesting it"
                    )
                });
                if !out.contains(&found) {
                    out.push(found);
                }
            }
        }
        out
    }

    /// The production halves of [`send_module_files`], joined -- the text
    /// every "`send.rs` spells this needle exactly N times" count reads.
    fn send_module_production(files: &[(String, String)]) -> String {
        send_module_files(files)
            .iter()
            .map(|file| {
                files
                    .iter()
                    .find(|(p, _)| p == file)
                    .map(|(_, text)| production_region(text))
                    .unwrap_or_else(|| panic!("`src/{file}` is in the closure but not the walk"))
            })
            .collect::<Vec<_>>()
            .join("\r\n")
    }

    /// The body of `mod send_fetch_thread`, from its opener to the first `}`
    /// at column zero.
    ///
    /// **The privacy boundary the blocking fetch lives behind**, sliced the
    /// way `the_item_list_is_drawn_only_inside_the_not_sends_gate` slices its
    /// gate, so the pins below can say "inside there and nowhere else" rather
    /// than "somewhere in the file".
    fn sealed_module() -> String {
        let production = production();
        let opener = concat!("mod send_fetch_", "thread {\r\n");
        assert_eq!(
            production.matches(opener).count(),
            1,
            "{opener:?} is not in production exactly once -- the module the blocking Sends \
             fetch is sealed inside is gone, or there are two of them"
        );
        let start = production.find(opener).expect("counted just above");
        let rest = &production[start + opener.len()..];
        let end = rest
            .find("\r\n}\r\n")
            .expect("`mod send_fetch_thread` has no closing brace at column zero");
        rest[..end].to_string()
    }

    /// Whitespace-insensitive, so this is a pin on the code and not on the
    /// formatter.
    fn squashed(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// **`spawn_send_list` is the delegation and nothing else.**
    ///
    /// An *equality*, not a set of needles. A needle-based pin over this
    /// function can be satisfied while the property is violated -- that is
    /// how the previous one was defeated -- and there is no hoist, no `let`
    /// binding above the call, and no helper-function spelling that survives
    /// an exact match of the whole body. Anything at all added here fails,
    /// including the one shape that would otherwise slip past a behavioural
    /// test of `spawn_send_list_with`:
    ///
    /// ```ignore
    /// let answer = real_send_list();                       // eframe thread
    /// spawn_send_list_with(ctx, tx, generation, move || answer);
    /// ```
    #[test]
    fn spawn_send_list_only_hands_the_real_fetch_to_the_tested_spawner() {
        let expected = squashed(&format!(
            "ctx_for_sends: egui::Context, tx: SendListSender, generation: u64, \
             session: zeroize::Zeroizing<String>, ) {{ {}(ctx_for_sends, tx, generation, \
             move || {}(&session)); ",
            concat!("spawn_send_list_", "with"),
            concat!("real_send_", "list")
        ));
        let actual = squashed(&body_of(concat!("spawn_send_", "list"), "    "));
        assert_eq!(
            actual, expected,
            "`spawn_send_list` is no longer purely a delegation to the spawner that \
             `fetch_thread_tests` drives. Anything else in this function is work done on \
             whichever thread called it -- and the only caller is the eframe frame closure."
        );
    }

    /// **`real_send_list` is the whole of the fetch, as an equality.**
    ///
    /// This replaces a set of NEEDLES, and the replacement is the point. The
    /// old pin required the body to *contain*
    /// `CliSendRunner::with_session(None, data_dir.as_deref(), session)`,
    /// which pins the SPELLING of an identifier and says nothing about what
    /// that identifier is bound to. Its own doc argued needles were safe here
    /// because "`real_send_list` is a plain function with no closure in it,
    /// so there is no inside and outside to move code between" -- reasoning
    /// about code MOTION, which does not cover REBINDING. Measured on
    /// `49f0f51`, inserting one statement above the pinned line:
    ///
    /// ```ignore
    /// let session = "";
    /// let runner = crate::send::CliSendRunner::with_session(None, .., session);
    /// ```
    ///
    /// left every needle word-perfect and the suite green at 2097 passed / 0
    /// failed, while production set `BW_SESSION=""` on every `bw send list`
    /// and a real vault answered `Locked` -- the exact bug `07f0e09` is named
    /// for fixing.
    ///
    /// So this is the same shape as
    /// [`spawn_send_list_only_hands_the_real_fetch_to_the_tested_spawner`],
    /// which the reviewer attacked and could not defeat: a whole-body
    /// equality, where any inserted statement, any rebinding, any extra
    /// argument and any swapped callee all change the compared string.
    ///
    /// **Comments are blanked first**, so this pins the code and not the
    /// prose around it -- the body carries a long comment about why the job
    /// is there, and an equality that included it would fail on a typo fix.
    ///
    /// **It is not the only guard, and deliberately.** An equality still only
    /// says the text is what it was; it cannot say what that text DOES.
    /// [`the_real_fetch_runs_bw_send_list_in_a_job_with_the_session_it_was_given`]
    /// says that, by running this very function. The two fail to different
    /// mutations -- a body rewritten to something equivalent-but-wrong fails
    /// here; a `sends_job` or `with_session` that quietly stopped carrying
    /// what it names fails there -- and neither subsumes the other.
    #[test]
    fn the_delegated_fetch_is_a_real_bw_send_list_for_the_active_account() {
        let expected = squashed(&format!(
            "session: &str, ) -> Result<Vec<crate::send::SendSummary>, crate::send::SendError> \
             {{ let data_dir = crate::bw_path::active_data_{}(); \
             crate::send::cli_send_{}({}(), data_dir.as_deref(), session)",
            "dir",
            "list",
            concat!("sends_", "job"),
        ));
        let actual = squashed(&sanitized(&body_of(concat!("real_send_", "list"), "    ")));
        assert_eq!(
            actual, expected,
            "`real_send_list` is no longer exactly one `bw send list`, in this window's job, \
             for the active account, with the session it was handed. Anything at all added, \
             removed or rebound here fails -- including a `let session = ..;` above the call, \
             which every needle-shaped pin over this function was measured to allow"
        );
        assert_eq!(
            production().matches(concat!("crate::send::cli_send_", "list(")).count(),
            1,
            "`cli_send_list` is called somewhere other than `real_send_list` -- and every \
             other caller is unproven ground for which thread it runs on"
        );
    }

    /// **The real fetch runs `bw send list`, in this window's job, with the
    /// session it was given.**
    ///
    /// The behavioural half, and the one that makes the pointer -> child leg
    /// of this feature something other than an assertion about text.
    /// `frame_promptness` substitutes `VaultFrameEnv::send_list`, so no test
    /// that drives a frame can see past the pointer; everything below it was
    /// held by source pins alone. This calls the production function ITSELF,
    /// with `job_object`'s spawn probe armed, and reads back what ARRIVED at
    /// `spawn_in_job`. There is no stand-in and no forwarder to be wrong --
    /// `real_send_list` is `pub(super)` for exactly this, and what replaces
    /// the privacy it gave up is spelled out at its definition.
    ///
    /// Three properties, each of which was a real defect or a real hazard:
    ///
    ///  1. **The session reaches the child's environment.** `bw send list` is
    ///     authenticated; a child that inherits no `BW_SESSION`, or an empty
    ///     one, answers `Locked`. The token compared against is the one this
    ///     test passed IN, so a body that substitutes its own constant, or
    ///     empties the parameter, fails here rather than being spelled right.
    ///  2. **The child is in this window's job.** Compared by ADDRESS against
    ///     `sends_job`, so "some job" is not enough: an orphaned `bw send`
    ///     holds the vault key in an environment block that any same-user
    ///     process can read with `PROCESS_VM_READ`.
    ///  3. **The token is in none of argv.** A process's argument vector is
    ///     readable machine-wide by far cheaper means than that.
    ///
    /// **No child is started.** The probe refuses every spawn before
    /// `CreateProcess` and `list_sends` maps the refusal through its ordinary
    /// failure path.
    #[test]
    fn the_real_fetch_runs_bw_send_list_in_a_job_with_the_session_it_was_given() {
        // A token this test owns, ending in `=` because a real `bw` session
        // token is base64 and does: a mutation that trims, splits or
        // percent-decodes the parameter would leave a token without one
        // untouched and pass a test that used one.
        const TOKEN: &str = "fetch-test-session-token/9+x=";

        // The verified CLI path `bw_job_command_in` refuses without. A path
        // that does not exist and never will: nothing is executed, because
        // the probe below refuses every spawn before `CreateProcess`.
        crate::bw_path::remember_verified_bw_exe(std::path::PathBuf::from(
            r"C:\deskwarden-test\first\bw.exe",
        ));

        let expected_job = super::super::send_fetch_thread::sends_job().map(|j| std::ptr::from_ref(j) as usize);
        assert!(
            expected_job.is_some(),
            "control: this window could not create a job object at all, so the job assertion \
             below would be satisfied by a jobless spawn"
        );

        let probe = crate::job_object::spawn_probe::SpawnProbe::arm();
        let refused = super::super::send_fetch_thread::real_send_list(TOKEN);
        // Plain strings, deliberately: what this test is about is what the
        // CHILD would have been given, not the recorder's own shape.
        let attempts: Vec<(Option<usize>, Vec<String>, Vec<(String, Option<String>)>)> = probe
            .attempts()
            .into_iter()
            .map(|a| {
                (
                    a.job,
                    a.args.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
                    a.envs
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.to_string_lossy().into_owned(),
                                v.as_ref().map(|v| v.to_string_lossy().into_owned()),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        drop(probe);

        assert!(
            refused.is_err(),
            "the probe refused the only spawn this read may make, yet it answered {refused:?} \
             -- so a child was started by a route the probe cannot see"
        );
        // **This is a non-vacuity control, NOT a spawn-count control**, and
        // reading it as one would be a mistake. The probe is thread-local
        // and armed on THIS thread, so a spawn issued from a thread
        // `real_send_list` started is not recorded here and this count does
        // not go up. What refuses a second, off-thread `bw` on this path is
        // the whole-body equality in
        // `the_delegated_fetch_is_a_real_bw_send_list_for_the_active_account`,
        // which admits no statement this body does not already have. All
        // this line says is that the one spawn it CAN see happened, so the
        // three assertions below are about something.
        assert_eq!(
            attempts.len(),
            1,
            "the real fetch did not reach the one spawn exactly once, so every assertion below \
             is about nothing: {attempts:?}"
        );
        let (job, args, envs) = attempts.into_iter().next().expect("just counted one");

        assert_eq!(
            args,
            vec!["send".to_string(), "list".to_string()],
            "control: `real_send_list` did not spawn a `bw send list`, so this test is \
             measuring some other command"
        );
        assert_eq!(
            envs.iter()
                .filter(|(k, _)| k == "BW_SESSION")
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>(),
            vec![Some(TOKEN.to_string())],
            "`BW_SESSION` did not arrive at the child set exactly once to the session \
             `real_send_list` was handed, so a real `bw send list` answers `locked`. The \
             overlay that arrived was {envs:?}"
        );
        assert_eq!(
            job, expected_job,
            "the `bw send list` child was not placed in this window's `KillOnCloseJob`. Its \
             environment block carries the token that unlocks the whole vault, and on Windows \
             an environment block is readable by any same-user process holding \
             `PROCESS_VM_READ | PROCESS_QUERY_INFORMATION` -- no elevation. Without the job, \
             an app that dies mid-fetch leaves that child, and that token, alive"
        );
        for arg in &args {
            assert!(
                !arg.contains(TOKEN),
                "the session token is in argv, where every other process on the machine can \
                 read it: {arg}"
            );
        }
    }

    /// **The blocking fetch has exactly one call site in the whole crate.**
    ///
    /// The count just above is over `include_str!("mod.rs")`, and so is every
    /// other containment assertion in this module. A blocking `bw send list`
    /// written in any SIBLING file is invisible to all of them. Measured on
    /// `0eeb749`, adding to `send_ui`'s own production
    ///
    /// ```ignore
    /// pub fn prefetch_now() -> Result<Vec<SendSummary>, SendError> {
    ///     let data_dir = crate::bw_path::active_data_dir();
    ///     let runner = crate::send::CliSendRunner::new(None, data_dir.as_deref());
    ///     crate::send::list_sends(&runner)
    /// }
    /// ```
    ///
    /// plus `let _ = send_ui::prefetch_now();` in the frame closure gave
    /// 2059 lib + 217 bin, 0 failed. Same "third position nobody counted"
    /// shape as the two before it, moved one file over.
    ///
    /// So the count is taken over the crate: every `.rs` file under `src`,
    /// discovered by walking the directory rather than from a list that a new
    /// file would not be on, cut to its production region, minus `send.rs`
    /// where these two are defined and legitimately used. The expected answer
    /// is a *list of call sites*, so deleting the real one fails as loudly as
    /// adding a second.
    #[test]
    fn the_blocking_fetch_has_exactly_one_call_site_in_the_whole_crate() {
        let files = crate_sources();

        // Controls: the walk really walked, and it walked the two files whose
        // presence and absence the assertion below depends on.
        assert!(
            files.len() > 30,
            "control: the crate walk found only {} source files, which is not this crate",
            files.len()
        );
        assert!(
            files.iter().any(|(p, _)| p == "vault_window/mod.rs"),
            "control: the walk never reached `vault_window/mod.rs`, so the one real call site \
             would not be counted and the expectation below could be met by finding nothing"
        );
        assert!(
            files.iter().any(|(p, _)| p == "send.rs"),
            "control: the walk never reached `send.rs`, so excluding it excludes nothing"
        );

        // Bare names, not `crate::send::`-qualified ones: `use
        // crate::send::list_sends;` and a bare call is precisely the spelling
        // the seal's doc comment names as the remaining way round privacy.
        //
        // And not the bare name ALONE either. Measured on `4446e9a`, writing
        // in `send_ui`'s production
        //
        // ```ignore
        // use crate::send::{list_sends as fetch_now, CliSendRunner as Runner};
        // fetch_now(&Runner::new(None, d.as_deref()))
        // ```
        //
        // and calling it from the frame closure gave 2061 lib + 217 bin, 0
        // failed: neither `list_sends(` nor `CliSendRunner::new(` appeared
        // anywhere in the crate. A needle is a pin on a *spelling*, and `use
        // .. as` renames the spelling. So each file's own `use` items are
        // read first and every local name they bind to these two is counted
        // as well -- see `local_names_of`.
        // **Both constructors, not just the one production uses today.** When
        // the pinned call moved from `CliSendRunner::new` to
        // `CliSendRunner::with_session` this list moved with it, and `::new`
        // stopped being counted anywhere in the crate -- so a second, jobless,
        // sessionless runner written on a non-list path was invisible again.
        // The expectation is therefore per-item: `with_session` has exactly
        // one site and `::new` has none, and either a new site or a deleted
        // one fails.
        // **The pinned name moved, because the TYPE stopped being nameable.**
        // `CliSendRunner` and both of its constructors are private to
        // `crate::send` now, so `crate::send::CliSendRunner::with_session(`
        // written in any other file is an `E0603` before this test runs, and
        // so is every alias, `type`, `use` and re-export of it. The one
        // production route out of `send.rs` is `cli_send_list`, and that is
        // what is counted here. The three rows below it are kept as
        // CONTROLS: they must be empty, and if privacy were ever widened back
        // a site would show up in one of them rather than silently in none.
        for (item, expected) in [
            (concat!("cli_send_", "list"), vec!["vault_window/mod.rs"]),
            (concat!("list_", "sends"), Vec::new()),
            (concat!("CliSendRunner::", "with_session"), Vec::new()),
            (concat!("CliSendRunner::", "new"), Vec::new()),
        ] {
            let sites: Vec<&str> = files
                .iter()
                .filter(|(path, _)| path != "send.rs")
                .flat_map(|(path, text)| {
                    let region = production_region(text);
                    let count: usize = local_names_of(&region, item)
                        .iter()
                        .map(|name| region.matches(&format!("{name}(")).count())
                        .sum();
                    std::iter::repeat(path.as_str()).take(count)
                })
                .collect();
            let needle = format!("{item}(");
            assert_eq!(
                sites, expected,
                "{needle:?} is called from {sites:?} in the crate's production code. The one \
                 permitted site is `real_send_list`, inside `mod send_fetch_thread`, whose \
                 caller is proven to be a background thread. Every other site is an up-to-sixty\
                 -second `bw send list` on whatever thread reaches it -- and the eframe thread \
                 reaches every `pub` function in this module"
            );
        }

        // `use` is module-scoped, so `local_names_of` sees a rename only in
        // the file that wrote it. The one way a rename crosses a file is a
        // re-export, so no file but `send.rs` may re-export either name.
        for (path, text) in files.iter().filter(|(path, _)| path != "send.rs") {
            // **`cli_send_list` is on this list.** It was not: when the
            // pinned primary needle moved to `cli_send_list` the call-site
            // rows moved with it and this loop did not, so the ONE name that
            // carries a real `bw` child out of `crate::send` was the one name
            // a `pub use` could rename past `local_names_of`. Not exploitable
            // on its own -- the crate-wide mention equality above counts the
            // token wherever it is written, `pub use` included -- but it is
            // the same drift that produced this round's finding, so the list
            // is kept in step with the rows above.
            for item in [
                concat!("cli_send_", "list"),
                concat!("list_", "sends"),
                concat!("CliSendRunner", ""),
            ] {
                for use_item in use_items(&production_region(text)) {
                    assert!(
                        !(use_item.starts_with("pub use") && use_item.contains(item)),
                        "{path} re-exports {item:?} ({use_item:?}). A re-export carries the \
                         blocking fetch, and any rename of it, into files whose own `use` items \
                         say nothing about it -- which is past every count above"
                    );
                }
            }
        }
    }

    /// **Every mention of `bw_serve::run_bw_sync` in the crate is one of the
    /// three pinned ones.**
    ///
    /// The Sends screen must start no further `bw sync`. That property had a
    /// guard already -- a four-token map over `spawn_sync`, `env.sync`,
    /// `spawn_vault_sync` and `VaultFrameEnv` -- and it guards the
    /// **pointer**, not what the pointer points at. Measured on `2c51b90`,
    /// this in the frame closure:
    ///
    /// ```ignore
    /// if on_sends && session_token.len() > 100_000 {
    ///     let tx = sync_tx.clone();
    ///     let tok = session_token.to_string();
    ///     std::thread::spawn(move || { let _ = tx.send(bw_serve::run_bw_sync(&tok)); });
    /// }
    /// ```
    ///
    /// SURVIVED at 2111 lib / 217 bin / 0 failed / 0 warnings: an unbounded
    /// per-frame stream of `bw sync` children on the Sends screen. It reaches
    /// no seam the behavioural
    /// [`super::frame_promptness::visiting_the_sends_screen_starts_no_further_bw_sync`]
    /// substitutes, so that test cannot see it, and it spells none of the
    /// four map tokens.
    ///
    /// **The real fix is a visibility change in `bw_serve.rs`**, and it is
    /// not available from these three files. `run_bw_sync` is `pub` in
    /// `crate::bw_serve`, so any file may name it. Narrowing it the way
    /// `send.rs`'s `CliSendRunner` was narrowed by this commit -- private to
    /// `crate::bw_serve`, with one narrow `pub` entry point per legitimate
    /// caller -- would make every one of these mutants an `E0603`. That is
    /// recorded as READY TO LAND and is deliberately not attempted here:
    /// `bw_serve.rs` is being edited concurrently.
    ///
    /// So this is a **text rule and it is honestly labelled as one**. What it
    /// does buy over the four-token map:
    ///
    ///  * It counts the token, not `run_bw_sync(`, so
    ///    `let f = bw_serve::run_bw_sync;` and a later `f(&tok)` is counted
    ///    at the `let`. A pin on the call spelling is not.
    ///  * It reads every file under `src` through [`crate_sources`], so a
    ///    forwarder written in `send_ui.rs` (which is where the previous
    ///    generation of this hole lived) is a second site in a second file.
    ///  * `use .. as` renames are followed through [`local_names_of`], and a
    ///    `pub use` of the name is refused outright.
    ///
    /// What it does NOT buy: a differently-named `pub fn` in `bw_serve.rs`
    /// itself that spawns its own `bw sync`. Nothing in these three files can
    /// see that; only `bw_serve.rs`'s own surface can.
    #[test]
    fn every_bw_sync_call_site_in_the_crate_is_one_of_the_three_pinned_ones() {
        let files = crate_sources();
        assert!(
            files.iter().any(|(p, _)| p == "bw_serve.rs"),
            "control: the walk never reached `bw_serve.rs`, so excluding it excludes nothing"
        );
        assert!(
            files.iter().any(|(p, _)| p == "main.rs"),
            "control: the walk never reached `main.rs`, so the two legitimate sites would not \
             be counted and the expectation below could be met by finding nothing"
        );

        let item = concat!("run_bw_", "sync");
        // Word-boundary occurrences of `name`, so a mention that is not a
        // call -- `let f = bw_serve::run_bw_sync;`, a `fn` pointer in a
        // struct literal, a `const` -- is counted too. `matches` alone would
        // also count `run_bw_sync_twice`, which is why the boundaries are
        // checked explicitly.
        fn mentions(region: &str, name: &str) -> usize {
            region
                .match_indices(name)
                .filter(|(at, _)| {
                    let before = region[..*at].chars().next_back();
                    let after = region[at + name.len()..].chars().next();
                    !matches!(before, Some(c) if c.is_alphanumeric() || c == '_')
                        && !matches!(after, Some(c) if c.is_alphanumeric() || c == '_')
                })
                .count()
        }

        let sites: Vec<&str> = files
            .iter()
            .filter(|(path, _)| path != "bw_serve.rs")
            .flat_map(|(path, text)| {
                let region = production_region(text);
                let count: usize = local_names_of(&region, item)
                    .iter()
                    .map(|name| mentions(&region, name))
                    .sum();
                std::iter::repeat(path.as_str()).take(count)
            })
            .collect();

        assert_eq!(
            sites,
            vec!["main.rs", "main.rs", "vault_window/mod.rs"],
            "`run_bw_sync` is mentioned from {sites:?} in the crate's production code. The three \
             permitted mentions are `main.rs`'s two backend-op sites and `spawn_vault_sync` in \
             `vault_window/mod.rs`, which is the ONE thing `VaultFrameEnv::sync` points at. A \
             fourth mention anywhere is a `bw sync` child on a path the four-token pointer map \
             does not cover -- and written in the frame closure it is one child PER FRAME"
        );

        for (path, text) in files.iter().filter(|(path, _)| path != "bw_serve.rs") {
            for use_item in use_items(&production_region(text)) {
                assert!(
                    !(use_item.starts_with("pub use") && use_item.contains(item)),
                    "{path} re-exports {item:?} ({use_item:?}). A re-export carries the sync \
                     spawn, and any rename of it, into files whose own `use` items say nothing \
                     about it -- which is past the count above"
                );
            }
        }
    }

    /// Every way `item` can be *spelled as a call* inside one production
    /// region, given that region's own `use` items.
    ///
    /// `item` is `head` or `head::method`. The answer always contains `item`
    /// itself, plus one entry for each of:
    ///
    /// * `use ..::head as ALIAS;` (in a braced group or not) -> `ALIAS[::method]`
    /// * `use crate::send as M;` / `use super::send as M;` -> `M::item`
    ///
    /// **This is alias resolution, not a longer list of needles.** A needle
    /// is a pin on a spelling and `use .. as` exists to change the spelling;
    /// see the measurement in the caller. It is still only as wide as one
    /// file, because Rust `use` is module-scoped -- which is why the caller
    /// also refuses a `pub use` of either name outside `send.rs`, closing the
    /// re-export hop that would otherwise carry an alias across files.
    ///
    /// **The real fix is not here.** It is to narrow `list_sends`'s and
    /// `CliSendRunner`'s visibility in `send.rs` so that `vault_window`
    /// cannot name them outside `mod send_fetch_thread` at all, at which
    /// point every spelling above is an `E0603` and no scan is needed.
    /// `send.rs` was held by another change this round and could not be
    /// touched; this stands in until it can.
    fn local_names_of(region: &str, item: &str) -> Vec<String> {
        let (head, method) = match item.split_once("::") {
            Some((h, m)) => (h, format!("::{m}")),
            None => (item, String::new()),
        };
        let mut names = vec![item.to_string()];
        for use_item in use_items(region) {
            // `use ..::head as ALIAS`
            for (at, _) in use_item.match_indices(head) {
                let before = use_item[..at].chars().next_back();
                if matches!(before, Some(c) if c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let rest = use_item[at + head.len()..].trim_start();
                if let Some(alias) = rest.strip_prefix("as ") {
                    let alias: String =
                        alias.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    if !alias.is_empty() {
                        names.push(format!("{alias}{method}"));
                    }
                }
            }
            // `use crate::send as M;`
            for prefix in [concat!("crate::", "send"), concat!("super::", "send")] {
                if let Some(at) = use_item.find(prefix) {
                    let rest = use_item[at + prefix.len()..].trim_start();
                    if let Some(alias) = rest.strip_prefix("as ") {
                        let alias: String = alias
                            .trim_start()
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        if !alias.is_empty() {
                            names.push(format!("{alias}::{item}"));
                        }
                    }
                }
            }
        }
        names.sort();
        names.dedup();
        names
    }

    /// Every `use` item in a production region, as the text from `use` to its
    /// terminating `;`, whitespace-squashed so `use crate::send::{\r\n    a as
    /// b,\r\n};` reads the same as the one-line spelling.
    fn use_items(region: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (at, _) in region.match_indices("use ") {
            let before = region[..at].chars().next_back();
            if matches!(before, Some(c) if c.is_alphanumeric() || c == '_' || c == ':') {
                continue;
            }
            let rest = &region[at..];
            let end = rest.find(';').unwrap_or(rest.len());
            out.push(squashed(&rest[..end]));
        }
        out
    }

    /// The controls on [`local_names_of`]: it really does follow a rename.
    #[test]
    fn a_renaming_import_of_the_blocking_fetch_is_still_the_blocking_fetch() {
        let aliased = concat!(
            "use crate::send::{list_", "sends as fetch_now, CliSendRunner as Runner};\r\n",
            "fn go() { fetch_now(&Runner::new(None, None)) }\r\n"
        );
        let names = local_names_of(aliased, concat!("list_", "sends"));
        assert!(
            names.iter().any(|n| n == "fetch_now"),
            "the rename in {aliased:?} was not followed: {names:?}"
        );
        let runner = local_names_of(aliased, concat!("CliSendRunner::", "new"));
        assert!(
            runner.iter().any(|n| n == concat!("Runner::", "new")),
            "the type rename was not followed: {runner:?}"
        );

        // A module rename, and a multi-line braced group.
        let module_aliased = concat!(
            "use crate::",
            "send as s;\r\nfn go() { s::list_",
            "sends(&r) }\r\n"
        );
        let names = local_names_of(module_aliased, concat!("list_", "sends"));
        assert!(
            names.iter().any(|n| n == concat!("s::list_", "sends")),
            "the module rename was not followed: {names:?}"
        );

        // Control: with no `use` at all the answer is just the item, so the
        // caller's count over an ordinary file is not inflated.
        assert_eq!(
            local_names_of("fn go() {}\r\n", concat!("list_", "sends")),
            vec![concat!("list_", "sends").to_string()]
        );

        // Control: this is a real improvement over the plain needle. The
        // aliased text above contains neither banned spelling.
        assert_eq!(aliased.matches(concat!("list_", "sends(")).count(), 0);
        assert_eq!(aliased.matches(concat!("CliSendRunner::", "new(")).count(), 0);
    }

    /// The frame closure is what starts the fetch, exactly once, and it hands
    /// over the **current** generation. The tag is what `apply_answer` uses to
    /// drop a late answer; a spawn that carries a constant instead would make
    /// every answer look current.
    ///
    /// **The call now goes through `VaultFrameEnv`**, as the vault load and
    /// the sync already did, so that `frame_promptness` can DRIVE the Sends
    /// screen without running a real `bw send list` -- see that module's doc
    /// for why an undriveable arm was worth a seam. That turns one pin into
    /// three, because the indirection is one more place the wiring can be
    /// wrong: the frame calls the pointer, the pointer is bound from the env
    /// once, and the env's own field is the real spawner (held next door by
    /// `production_is_the_only_env_a_shipping_build_has`).
    #[test]
    fn the_frame_starts_the_fetch_once_and_tags_it_with_the_question_it_is_asking() {
        // Comments blanked, so no doc comment that SPELLS one of the three
        // needles below can stand in for the code that should carry it.
        let production = sanitized(&production());
        // Squashed on both sides, so this is a pin on the call and not on
        // where rustfmt chose to break its arguments.
        let spawn = squashed(concat!(
            "(spawn_send_",
            "list)( ui.ctx().clone(), send_tx.clone(), send_fetch.generation(), ",
            "session_token.clone(), );"
        ));
        // The `Zeroizing` the argument used to be wrapped in here is now the
        // type of `session_token` ITSELF -- `build_frame` wraps the bare
        // `String` it is handed on its first line, so the window's own copy is
        // wiped when the window closes instead of going back to the allocator
        // with the vault key still in it. So the clone above is already a
        // `Zeroizing<String>`, which `VaultFrameEnv::send_list`'s signature
        // requires, and the type checker holds what this needle used to.
        //
        // TWICE, and the second is the sibling copy. `spawn_vault_sync` takes
        // a bare `String` because `VaultFrameEnv::sync`'s `fn` pointer says
        // so, so it wraps on arrival the same way -- one wipe per Sync
        // instead of one more freed heap block holding the vault key per
        // Sync, for the life of the process.
        let wrapped = concat!("let session_token = zeroize::Zeroizing::new(session_", "token);");
        assert_eq!(
            production.matches(wrapped).count(),
            2,
            "{wrapped:?} is not in production exactly twice -- the window's copy of the vault \
             session is a bare `String` again, which goes back to the allocator with the token \
             still in it, readable by whatever allocates that block next"
        );
        assert_eq!(
            squashed(&production).matches(&spawn).count(),
            1,
            "{spawn:?} is not in production exactly once -- the Sends fetch is started from \
             nowhere, from more than one place, without the generation that lets a stale \
             answer be told from a current one, or without the window's own session, in \
             which case a real `bw send list` answers `locked`"
        );
        // The binding the call reads, exactly once. Without this the call
        // above could be reading a local shadowing `env.send_list` with
        // something else entirely.
        let bound = concat!("let spawn_send_", "list = env.send_", "list;");
        assert_eq!(
            production.matches(bound).count(),
            1,
            "{bound:?} appears in production {} times, not once -- the pointer the frame \
             calls is not the one the caller handed in",
            production.matches(bound).count()
        );
        // And the real spawner is named exactly once in production: in
        // `VaultFrameEnv::production`, which is the only constructor a
        // shipping build has. A second mention is a second way to reach the
        // `bw` spawn.
        let entry = concat!("send_fetch_thread::spawn_send_", "list");
        assert_eq!(
            production.matches(entry).count(),
            1,
            "{entry:?} appears in production {} times, not once",
            production.matches(entry).count()
        );
    }

    /// **The session wipe is where it has to be, not merely present twice.**
    ///
    /// What held this was the COUNT just above --
    /// `production.matches(wrapped).count() == 2` -- and a count says
    /// nothing about PLACEMENT. Plain deletion of `spawn_vault_sync`'s wrap
    /// is caught by it, so the count is not vacuous; RELOCATION is not.
    /// Measured on `c92c00c`, **M-A**: delete the wrap from
    /// `spawn_vault_sync` and add the byte-identical line to `run()`
    /// (wrapping there and passing `session_token.to_string()` into
    /// `build_frame`) gave 2101 lib / 217 bin / 0 failed / 0 warnings. The
    /// count was still exactly two. `spawn_vault_sync` then received a bare
    /// `String`, moved it into the sync thread and dropped it un-wiped --
    /// one freed heap block holding the vault-unlocking session per Sync,
    /// for the life of the process, which is verbatim the hazard the count
    /// claims to prevent.
    ///
    /// So the wrap is pinned by placement, as a whole-body EQUALITY -- the
    /// same shape as
    /// [`the_delegated_fetch_is_a_real_bw_send_list_for_the_active_account`]
    /// and
    /// [`spawn_send_list_only_hands_the_real_fetch_to_the_tested_spawner`],
    /// which the reviewer attacked and could not defeat. Any inserted
    /// statement, any rebinding, any reordering, any extra argument and any
    /// swapped callee changes the compared string; a wrap that moved to a
    /// caller leaves this body without one and fails here whatever the
    /// file-wide count says.
    ///
    /// **Comments are blanked first**, so this pins the code and not the six
    /// lines of prose the wrap carries above it.
    ///
    /// `body_of` with an EMPTY indent: this function is at column zero, so
    /// its terminator is `\r\n}\r\n`, and a terminator that is required to
    /// be found rather than defaulted is what stops the slice quietly
    /// becoming "the rest of the file".
    #[test]
    fn the_sync_thread_wipes_the_session_it_was_handed() {
        let expected = squashed(&format!(
            "tx: mpsc::Sender<Result<(), String>>, session_token: String) {{ \
             let session_token = zeroize::{}(session_token); \
             std::thread::spawn(move || {{ let _ = tx.send(bw_serve::run_bw_{}\
             (&session_token)); }});",
            concat!("Zeroizing::", "new"),
            "sync",
        ));
        let actual = squashed(&sanitized(&body_of(concat!("spawn_vault_", "sync"), "")));
        assert_eq!(
            actual, expected,
            "`spawn_vault_sync` is no longer exactly `wrap the session, then run `bw sync` \
             on a thread with it`. A wrap that moved to a CALLER leaves this function \
             taking a bare `String`, moving it into the thread and dropping it un-wiped -- \
             one freed heap block holding the vault-unlocking token per Sync, for the life \
             of the process -- while the count of that line in the file is unchanged"
        );
    }

    /// **Both Sync call sites hand over the window's own session, and
    /// nothing rebinds that session in between.**
    ///
    /// The behavioural half of the sync path is
    /// `frame_promptness::the_windows_own_session_is_what_reaches_the_bw_sync_child`,
    /// which drives a real frame and reads back what ARRIVED at the
    /// `VaultFrameEnv::sync` pointer -- at both call sites, the auto-sync on
    /// the first real frame and the status pill's own press. That is the
    /// primary hold and it is what kills **M-C** (`String::new()` for the
    /// argument at both sites, measured green at 2101 / 0 / 0 warnings,
    /// every `bw sync` then running with `BW_SESSION=""` against a vault
    /// that answers `Locked`).
    ///
    /// What is left for source to hold is the two things driving a frame
    /// cannot see. First, that there is no THIRD call site spelled some
    /// other way -- the harness observes the sites it reaches, not the ones
    /// it does not. Second, that `session_token` inside the closure is the
    /// binding `build_frame` wrapped on its first line and not a shadow: a
    /// `let session_token = String::new();` written between the two presses
    /// would leave both call sites word-perfect and both arguments empty,
    /// and the whole-body equality one test up cannot see into the closure.
    ///
    /// **A count of one SPELLING is not a count of the call sites.** Measured
    /// on `328996b`, writing in the frame closure right after
    /// `send_fetch.note_screen(on_sends);`
    ///
    /// ```ignore
    /// if on_sends {
    ///     let sync_now = spawn_sync;
    ///     sync_now(sync_tx.clone(), String::new());
    /// }
    /// ```
    ///
    /// gave 2108 / 0 failed / 0 warnings. A local `let` alias changes the
    /// spelling, so `(spawn_sync)(` was still written exactly twice and the
    /// `let session_token` needle was untouched; and the behavioural half
    /// missed it because `press_sync` never visits the Sends screen. That is
    /// an UNBOUNDED per-frame stream of `bw sync` children, each with
    /// `BW_SESSION=""`, for as long as the Sends screen is up.
    ///
    /// So what is counted now is the NAME, not the call: `spawn_sync` may be
    /// written in the closure exactly twice, and both of those are the two
    /// `(spawn_sync)(` sites already counted. A third site must either spell
    /// the name (fails here, whatever the syntax around it -- a `let`, a
    /// struct field, a `match` arm, a closure) or obtain the pointer some
    /// other way, which the crate-wide map below refuses.
    #[test]
    fn both_sync_call_sites_pass_the_windows_own_session() {
        let closure = squashed(&frame_closure());
        let call =
            squashed(concat!("(spawn_", "sync)(sync_tx.clone(), session_token.to_string());"));
        let opener = concat!("(spawn_", "sync)(");
        assert_eq!(
            closure.matches(&call).count(),
            2,
            "{call:?} is not written in the frame closure exactly twice. The two Sync call \
             sites are the auto-sync on the window's first real frame and the status \
             pill's press, and each must hand over the window's own session: a `bw sync` \
             started with an empty `BW_SESSION` is answered `Locked` by a real vault"
        );
        assert_eq!(
            closure.matches(opener).count(),
            2,
            "{opener:?} is called from the frame closure {} times, not twice -- there is a \
             Sync call site spelled some other way, and the frame harness observes only \
             the sites it reaches",
            closure.matches(opener).count()
        );
        let rebind = concat!("let session_", "token");
        assert_eq!(
            closure.matches(rebind).count(),
            0,
            "{rebind:?} appears inside the frame closure. The session the two Sync call \
             sites read is then whatever the nearest shadow bound, not the \
             `Zeroizing<String>` `build_frame` wrapped on its first line -- and both call \
             sites stay word-perfect while both arguments go empty"
        );

        // The NAME, not the call. Substring matching is deliberate: a
        // `spawn_sync2` or a `my_spawn_sync` contains this and is counted
        // too, which is the right answer -- a near-miss name binding the
        // same pointer is a call site.
        let name = concat!("spawn_", "sync");
        assert_eq!(
            closure.matches(name).count(),
            2,
            "{name:?} is NAMED in the frame closure {} times, not twice. The two times it \
             may be named are the two `(spawn_sync)(` call sites counted just above; any \
             further mention -- `let sync_now = spawn_sync;`, a struct field, a `match` \
             arm, a closure that forwards to it -- is a third `bw sync` call site whose \
             spelling no literal count reaches, and one written under a per-frame `if` is \
             an unbounded stream of un-jobbed children",
            closure.matches(name).count()
        );

        // And the pointer cannot be obtained WITHOUT naming `spawn_sync`
        // either -- not by re-reading the field, not by taking the real
        // spawner directly, and not from another file. Every one of these is
        // a whole-crate map with a file on it, so a fourth file that starts
        // naming any of them fails here and says which file.
        //
        // `main.rs`'s three `spawn_sync` are the TRAY's own unrelated
        // `fn spawn_sync(..)` and its TWO calls; they are pinned, not
        // exempted, so they cannot grow either.
        //
        // It was two until the cache-first startup arm landed. A login that
        // restores an encrypted copy from disk reaches the tray without
        // waiting for `bw serve`, so the backend is started and reconciled
        // *behind* the arm rather than above it -- a second call, on a path
        // that has no session to hand the window because no window is open
        // on it. The count moved with a reason; it is still exact, and a
        // fourth still fails.
        for (needle, expected) in [
            (
                concat!("spawn_", "sync"),
                vec![("main.rs", 3usize), ("vault_window/mod.rs", 3)],
            ),
            (concat!("env.", "sync"), vec![("vault_window/mod.rs", 1)]),
            (concat!("spawn_vault_", "sync"), vec![("vault_window/mod.rs", 2)]),
            (
                concat!("VaultFrame", "Env"),
                // **Five in `mod.rs`, not four**, and the fifth is a
                // PARAMETER: `build_frame` forwards to
                // `build_frame_with_search`, so the type is written on both
                // signatures. A forwarded parameter cannot read the pointer
                // out of the env -- only `env.sync` does that, and it is
                // pinned to one, one file down -- so this is still the count
                // of every place that could.
                //
                // **Three in `main.rs`, not four, since the startup door
                // stopped drawing in the daemon.** That branch built a vault
                // frame in this process and named the type to do it; the
                // window now runs in a process of its own and the branch asks
                // `UiWindows` for it instead. One construction fewer is one
                // fewer place that could reach the pointer, which is the
                // direction this pin wants.
                vec![("main.rs", 3), ("vault_window/mod.rs", 5)],
            ),
        ] {
            let files = crate_sources();
            assert!(
                files.len() > 30,
                "control: the crate walk found only {} source files, which is not this crate",
                files.len()
            );
            let seen: Vec<(&str, usize)> = files
                .iter()
                .map(|(path, text)| {
                    (path.as_str(), sanitized(&production_region(text)).matches(needle).count())
                })
                .filter(|(_, n)| *n > 0)
                .collect();
            assert_eq!(
                seen, expected,
                "{needle:?} is written in the crate's production at {seen:?}, not \
                 {expected:?}. `VaultFrameEnv::sync` is a `fn` pointer to a real `bw sync` \
                 spawner, and these counts are every way there is to get hold of one: \
                 the binding and its two calls (`spawn_sync` in `mod.rs`), the one read of \
                 the field (`env.sync`), the real spawner's own definition and the one \
                 place `VaultFrameEnv::production` names it, and the type itself -- which \
                 no file but `mod.rs` and `main.rs`'s four constructions may name, so a \
                 sibling cannot even take an `env` to read the pointer out of"
            );
        }
    }

    /// **The blocking fetch cannot be reached from the frame closure.**
    ///
    /// The property is "the up-to-sixty-second `bw send list` never runs on
    /// the eframe thread", and it has now been defeated twice by hoisting the
    /// call to a position the pin of the day did not look at. The last one
    /// was the frame closure itself -- measured on `c14afb2`, adding
    /// `let _ = real_send_list();` above the spawn gave 2050 lib + 217 bin,
    /// 0 failed, because nothing counted `real_send_list`'s call sites and
    /// the single textual `list_sends` call was still where it had always
    /// been.
    ///
    /// The primary hold is now the compiler: `real_send_list` is private to
    /// `mod send_fetch_thread`, so that line does not build. What is left for
    /// source to hold is the two ways round a privacy boundary -- writing a
    /// *fresh* blocking call in the frame (`use crate::send::list_sends;` and
    /// a bare call), or widening the module's exports until the sealed fetch
    /// leaks out through a wrapper. So: every mention of either name lives in
    /// the block, and the block exports exactly the two spawners.
    #[test]
    fn every_mention_of_the_blocking_fetch_is_sealed_inside_the_spawning_module() {
        // **The region read is the whole crate, not `mod.rs` alone.**
        //
        // `production()` is `production_region_source(include_str!("mod.rs"))`
        // -- ONE FILE. `real_send_list` is `pub(super)` inside `mod
        // send_fetch_thread`, and `pub(super)` there means "visible in
        // `vault_window`" -- which is `vault_window::send_ui` and
        // `vault_window::item_list` every bit as much as it is `mod.rs`. The
        // commit that traded privacy away for the behavioural test argued
        // this seal was the stronger lock because it "also refuses a call
        // written in a SIBLING file, which `pub(super)` never permitted but
        // `pub(crate)` would have". Both halves were false, and the second
        // was measured. On `c92c00c`, writing in THIS file's production
        //
        // ```ignore
        // pub fn blocking_prefetch(session: &str)
        //     -> Result<Vec<SendSummary>, crate::send::SendError> {
        //     super::send_fetch_thread::real_send_list(session)
        // }
        // ```
        //
        // plus `let _ = send_ui::blocking_prefetch(&session_token);` in the
        // frame closure gave 2101 lib / 217 bin / 0 failed / 0 warnings. It
        // COMPILED, which is the proof that `pub(super)` admits the sibling
        // file; the frame line spelled none of the four needles below; and
        // nothing `pub` was added inside the block, so the export list was
        // unchanged too. A sixty-second blocking `bw send list` on the eframe
        // thread, past every guard in this module.
        //
        // So the counts are taken over every `.rs` file under `src`, walked
        // rather than listed so that a file added next month is read. Both
        // sides are `sanitized`, so a mention in another file's prose is not a
        // mention and cannot inflate the total past what the block can account
        // for.
        //
        // **`send.rs` is IN the walk, and pinned rather than excluded.** It
        // used to be subtracted, an exclusion copied verbatim from
        // `the_blocking_fetch_has_exactly_one_call_site_in_the_whole_crate`,
        // where it is right because `list_sends` and `CliSendRunner` are
        // DEFINED there. Copying it here turned a definition-site exemption
        // into a CALL-site exemption, and `send.rs` is the most natural file
        // in the crate in which to write the forwarder. Measured on
        // `328996b`, adding to `send.rs`'s production
        //
        // ```ignore
        // pub fn blocking_prefetch(session: &str)
        //     -> Result<Vec<SendSummary>, SendError> {
        //     list_sends(&CliSendRunner::with_session(None, None, session))
        // }
        // ```
        //
        // plus `let _ = crate::send::blocking_prefetch(&session_token);` in
        // the frame closure gave 2108 lib / 217 bin / 0 failed / 0 warnings,
        // and 2094 passed again with `--skip frame_promptness --skip
        // vault_window::tests`, so it was not a frame-harness accident. Both
        // needles that line spells live in the one file nothing looked at.
        //
        // The fix is not a wider exclusion but a NARROWER one: `send.rs` may
        // spell each needle exactly the number of times its definitions do,
        // and that number is asserted here rather than subtracted silently.
        // A forwarder written there spells `list_sends(` or
        // `CliSendRunner::with_session(` -- it cannot forward without naming
        // what it forwards to -- so its count moves and this fails.
        let files = crate_sources();
        assert!(
            files.len() > 30,
            "control: the crate walk found only {} source files, which is not this crate",
            files.len()
        );
        for required in ["vault_window/mod.rs", "vault_window/send_ui.rs"] {
            assert!(
                files.iter().any(|(p, _)| p == required),
                "control: the crate walk never reached {required:?}, so a blocking fetch \
                 written there would be counted by nothing at all"
            );
        }
        assert!(
            files.iter().any(|(p, _)| p == "send.rs"),
            "control: the crate walk never reached `send.rs`, so the definition counts \
             pinned below are pinned on nothing"
        );
        let production: String = files
            .iter()
            .map(|(_, text)| production_region(text))
            .collect::<Vec<_>>()
            .join("\r\n");
        // **`send.rs` AND EVERY DESCENDANT OF `crate::send`.** The counts
        // below used to read the single path `"send.rs"`, and privacy does
        // not stop at a file: a descendant module lives in another file and
        // sees the private type, its private fields and the private
        // `list_invocation` alike. See [`send_module_files`] for the measured
        // survivor -- a new `src/send/inner.rs` building the runner by struct
        // literal -- and for why the discovery is a transitive, fail-by-
        // default closure rather than a second path string.
        let send_module = send_module_files(&files);
        assert!(
            send_module.contains(&"send.rs".to_string()),
            "control: the `crate::send` closure is {send_module:?}, which does not contain \
             `send.rs`, so the definition counts pinned below are pinned on nothing"
        );
        let send_rs = sanitized(&send_module_production(&files));
        let block = sanitized(&sealed_module());

        // Control: the slice is a slice, not the whole file. Without this the
        // containment assertions below are trivially true.
        assert!(
            block.len() > 200,
            "control: the sealed-module slice is only {} bytes, which is not a module's worth",
            block.len()
        );
        let frame_only = concat!("send_fetch.note_", "screen(on_sends);");
        assert!(
            !block.contains(frame_only),
            "control: the sealed-module slice contains the frame closure, so it is not a slice \
             of the module and every containment assertion here is vacuous"
        );

        // `send.rs` DEFINES two of the four, so it cannot be required to
        // spell them zero times -- but the definitions are all it may spell.
        // `pub fn list_sends<R: SendRunner>` is one mention of `list_sends`
        // and no call; `pub struct CliSendRunner`, `impl<'a> CliSendRunner<'a>`
        // and `impl SendRunner for CliSendRunner<'_>` name the type but never
        // `CliSendRunner::with_session`, which is spelled `fn with_session(`
        // inside the `impl`. So the allowance is one, and it is one for the
        // FIRST needle only.
        //
        // **The last two rows are not seal needles; they are `send.rs`'s
        // CONSTRUCTORS.** Measured on this commit's first draft, with only the
        // four needles pinned, writing in `send.rs`'s production
        //
        // ```ignore
        // pub fn warm_cache(session: &str) -> Result<(), SendError> {
        //     let runner = CliSendRunner::new(None, None);
        //     let _ = runner.run(&list_invocation(Some(session)))?;
        //     Ok(())
        // }
        // ```
        //
        // plus `let _ = crate::send::warm_cache(&session_token);` in the frame
        // closure gave `source_pins` 20 passed / 0 failed. It is the same
        // sixty-second blocking `bw send list` -- built from the OTHER
        // constructor and driven through `SendRunner::run` and the private
        // `list_invocation` directly, so it spells not one of the four needles
        // above nor `list_sends(`. The same shape written in a FOURTH file is
        // an `E0603`, refused by rustc: `list_invocation` is private to
        // `send.rs`, so this route exists in `send.rs` alone -- which is
        // exactly the file the old exclusion made invisible.
        //
        // So the type itself is counted. Every way there is to obtain a
        // `CliSendRunner` names it: `CliSendRunner::new`,
        // `CliSendRunner::with_session`, or the struct literal (whose fields
        // are private to this file, so nowhere else can write it). Three
        // mentions is `pub struct CliSendRunner`, `impl<'a> CliSendRunner<'a>`
        // and `impl SendRunner for CliSendRunner<'_>` -- the definitions, and
        // no construction at all.
        //
        // **Updated for the privacy wall.** `list_sends` and
        // `CliSendRunner::with_session` are no longer spelled outside
        // `send.rs` at all -- `mod.rs` calls `cli_send_list`, the one `pub`
        // route into a real `bw` child that module has -- so requiring them
        // to be "inside the block" would assert nothing: their crate-wide
        // total, less their `send.rs` definitions, is zero, and the `total >
        // 0` control below would be the thing that fired. They stay here as
        // COUNTS over `send.rs` alone, which is the half of this pin that
        // catches a second construction written in that file; `cli_send_list`
        // takes over as the seal needle.
        for (needle, defined_in_send_rs, is_seal_needle) in [
            (concat!("cli_send_", "list"), 1usize, true),
            (concat!("real_send_", "list"), 0, true),
            (concat!("spawn_send_list_", "with"), 0, true),
            (concat!("list_", "sends"), 2, false),
            // **Both of these moved by one when `cli_send_delete` landed**,
            // and the move is the deliberate decision this comment records
            // rather than a number that drifted: `cli_send_delete`'s body
            // hands a `CliSendRunner::with_session(job, data_dir, session)`
            // to the generic revoke, which is one more `CliSendRunner` and one more
            // `CliSendRunner::with_session` than the file had. Five is now
            // `pub struct CliSendRunner`, `impl<'a> CliSendRunner<'a>`,
            // `impl SendRunner for CliSendRunner<'_>` and the two entry
            // points -- the definitions and the two pinned constructions, and
            // no third.
            // **Both of these moved by one AGAIN when `cli_send_create`
            // landed**, on step 4's terms: its body hands a
            // `CliSendRunner::with_session(job, data_dir, session)` to the
            // generic create, which is one more `CliSendRunner` and one more
            // `CliSendRunner::with_session` than the file had. Six is now
            // `pub struct CliSendRunner`, `impl<'a> CliSendRunner<'a>`,
            // `impl SendRunner for CliSendRunner<'_>` and the THREE entry
            // points -- the definitions and the three pinned constructions,
            // and no fourth.
            (concat!("CliSendRunner", "::with_session"), 3, false),
            // **Moved by one AGAIN when `cli_send_receive` landed**, and this
            // time on the OTHER constructor, which is the whole of what makes
            // the move worth recording rather than a number that drifted: a
            // receive is anonymous -- the link is the credential -- so
            // `cli_send_receive` builds `CliSendRunner::new(job, data_dir)`
            // and hands the child no `BW_SESSION` at all. Seven is now
            // `pub struct CliSendRunner`, `impl<'a> CliSendRunner<'a>`,
            // `impl SendRunner for CliSendRunner<'_>` and the FOUR entry
            // points, and no fifth.
            (concat!("CliSendRunner", ""), 7, false),
            // **One, and it was zero until the receive landed.** This row used
            // to be the whole of what refused the measured `warm_cache`
            // survivor -- a blocking `bw send list` built from the constructor
            // nothing in production used. It is not zero any more, so it no
            // longer refuses that shape by itself; what refuses it now is that
            // this row and the `CliSendRunner` row above must BOTH hold, and
            // the survivor spelled one more of each than the file accounts
            // for. The single production use is `cli_send_receive`'s, and a
            // second one is a second sessionless blocking child.
            (concat!("CliSendRunner", "::new"), 1, false),
            // The revoke's two, on the same terms as `list_sends` and
            // `list_invocation` above. `delete_send` is its definition plus
            // the one call in `cli_send_delete`; `delete_invocation` is its
            // definition plus the one use in `delete_send`. A third mention
            // of either is a second blocking `bw send delete` written inside
            // the privacy boundary, where the private runner and the private
            // `delete_invocation` are both still in scope.
            (concat!("delete_", "send"), 2, false),
            (concat!("delete_", "invocation"), 2, false),
            // **The bypass, counted.** `runner.run(&list_invocation(..))` is
            // a complete blocking `bw send list` that spells neither
            // `list_sends` nor `cli_send_list`; the author documented it for
            // the in-`send.rs` case and the measured `send/inner.rs` survivor
            // used exactly it, one file over. `list_invocation` is private to
            // `crate::send`, so this row and the closure above are together
            // the whole of what refuses a second use of it.
            (concat!("list_", "invocation"), 2, false),
            // **The create's two, on the same terms as the revoke's pair
            // above.** `create_send` is its definition plus the one call in
            // `cli_send_create`; `plan_to_invocation` is its definition plus
            // the one use in `create_send`. A third mention of either is a
            // second blocking `bw send create` written inside the privacy
            // boundary, where the private runner is still in scope -- and
            // `plan_to_invocation` is the create's exact counterpart of
            // `list_invocation`: `runner.run(&plan_to_invocation(..))` is a
            // whole blocking publish that spells neither `create_send` nor
            // `cli_send_create`.
            (concat!("create_", "send"), 2, false),
            (concat!("plan_to_", "invocation"), 2, false),
            // **The receive's three, on the same terms as the three families
            // above**, and they are counted HERE rather than sealed here: the
            // seal for the receive is
            // `vault_window::send_create_wiring::every_mention_of_the_blocking_receive_is_sealed_inside_its_own_module`,
            // which holds `mod send_receive_thread` the way this test holds
            // `mod send_fetch_thread`. What these rows add is the half that
            // seal cannot see, which is the half `send.rs` itself is the most
            // natural place to write: a SECOND blocking `bw send receive`
            // written inside the privacy boundary.
            //
            // `cli_send_receive` is its definition alone -- every call to it
            // is outside this module. `receive_send` is its definition plus
            // the one call in `cli_send_receive`. `receive_invocation` is its
            // definition plus the one use in `receive_send`, and it is the
            // receive's exact counterpart of `list_invocation`:
            // `runner.run(&receive_invocation(..))` is a whole blocking fetch
            // that spells neither of the other two -- and unlike
            // `list_invocation` this builder is `pub`, so the count is the
            // only thing standing between it and a second runner built beside
            // it.
            (concat!("cli_send_", "receive"), 1, false),
            (concat!("receive_", "send"), 2, false),
            (concat!("receive_", "invocation"), 2, false),
        ] {
            assert_eq!(
                send_rs.matches(needle).count(),
                defined_in_send_rs,
                "the `crate::send` module ({send_module:?}) spells {needle:?} {} times, not the \
                 {defined_in_send_rs} its DEFINITIONS account for. The extra mention is a \
                 blocking `bw send list` written inside the privacy boundary -- in `send.rs` \
                 itself or in a DESCENDANT file, where the private runner, its private fields \
                 and the private `list_invocation` are all still in scope -- and a `pub fn` \
                 there is callable from the frame closure by a line that spells none of these \
                 needles at all",
                send_rs.matches(needle).count()
            );
            if !is_seal_needle {
                continue;
            }
            let total = production.matches(needle).count() - defined_in_send_rs;
            let inside = block.matches(needle).count();
            assert!(
                total > 0,
                "control: {needle:?} is not in production at all, so requiring it to be inside \
                 the sealed module asserts nothing"
            );
            assert_eq!(
                inside, total,
                "{needle:?} occurs {total} times in the CRATE's production (every file under \
                 `src`, less the {defined_in_send_rs} definition(s) in `send.rs`) but only \
                 {inside} of them are \
                 inside `mod send_fetch_thread`. Every mention outside that block -- in \
                 `mod.rs` or in any sibling file, which `pub(super)` admits -- is a blocking \
                 `bw send list` reachable from the eframe frame closure, where it freezes the \
                 window -- titlebar included -- for up to sixty seconds"
            );
        }

        // The exports. A `pub(super) fn blocking_send_list()` added here that
        // merely forwards to the private fetch would keep every count above
        // unchanged and hand the frame closure the call back.
        //
        // **Read from EVERY `pub` in the block, not from `pub(super) fn `.**
        // The previous shape collected function headers only, and blacklisted
        // four wider spellings (`pub fn `, `pub(crate) `, `pub struct `,
        // `pub use `) -- none of which is a `pub(super)` non-`fn`. Measured on
        // `0eeb749`, adding
        //
        // ```ignore
        //     pub(super) const PREFETCH: fn() -> Result<..> = real_send_list;
        // ```
        //
        // to the block and `let _ = send_fetch_thread::PREFETCH();` to the
        // frame closure gave 2059 lib + 217 bin, 0 failed: the collected vec
        // was unchanged, every containment count above was unchanged (the
        // mention is inside the block, and the frame line names no needle),
        // and the eframe thread ran a sixty-second `bw send list`. A value
        // export needs no wrapper. So the list below is every `pub` token in
        // the block, whatever item it sits on, matched as whole lines.
        //
        // **The separator is any Rust whitespace, not a literal space.**
        // Measured on `4446e9a`: the previous filter required the character
        // after `pub` to be `(` or `' '`, and writing the same
        // function-pointer export with a TAB --
        //
        // ```ignore
        // \tpub\tconst PREFETCH: fn() -> Result<..> = real_send_list;
        // ```
        //
        // -- plus `let _ = send_fetch_thread::PREFETCH();` in the frame
        // closure gave 2061 lib + 217 bin, 0 failed. One space swapped for
        // one tab resurrected the exact previous-round survivor. `pub\r\nfn`
        // and `pub/*x*/fn` are the same hole. Comments are blanked by
        // `sanitized` first, which turns the third into whitespace too.
        let block = sanitized(&block);
        let block = block.as_str();
        let exported: Vec<String> = block
            .match_indices("pub")
            .filter(|(at, _)| {
                let before = block[..*at].chars().next_back();
                let after = block[at + "pub".len()..].chars().next();
                // A visibility keyword on a word boundary -- `pub(` or `pub`
                // followed by ANY whitespace -- and not the middle of
                // "published" in prose.
                !matches!(before, Some(c) if c.is_alphanumeric() || c == '_')
                    && matches!(after, Some(c) if c == '(' || c.is_whitespace())
            })
            .map(|(at, _)| block[at..].lines().next().unwrap_or_default().trim_end().to_string())
            .collect();
        assert_eq!(
            exported,
            vec![
                concat!("pub(super) fn real_send_", "list("),
                concat!(
                    "pub(super) fn sends_", "job() -> ",
                    "Option<&'static crate::job_object::KillOnCloseJob> {"
                ),
                concat!("pub(super) fn spawn_send_", "list("),
                concat!("pub(super) fn spawn_send_list_", "with<F>("),
            ],
            "`mod send_fetch_thread` no longer declares exactly the two spawners, the fetch and the job, and nothing \
             else. Every `pub` in the block is listed here whatever item it is on: a \
             `pub(super) const`, `static`, `type`, `use`, `mod` or `trait` is as good a handle \
             on the blocking fetch as a `pub(super) fn` wrapper is, and a function-pointer \
             constant is one with no wrapper at all"
        );

        // A trait method's visibility is the *trait's*, not the impl's, so an
        // `impl super::SomeTrait for ()` written in here would carry
        // `real_send_list` out to the frame closure without the token `pub`
        // appearing anywhere for the scan above to see, and without adding a
        // mention of any needle outside the block. A macro defined here and
        // expanded there is the same hole. These two are named needles, said
        // plainly: what closes them is that the block has no reason to hold
        // either, so the assertion is cheap and the mutant is loud.
        //
        // **As tokens, not as `"impl "`.** `block.contains("impl ")` misses
        // `impl<T: Trait> Foo for X`, `impl\tTrait for ()` and `impl\r\nTrait`
        // -- three spellings of the same leak, and the reason the previous
        // shape's KILLED verdict was good for one spelling only. `impl` is a
        // keyword, so a word-boundary scan is exact: nothing else can be
        // spelled `impl` on both boundaries.
        for leak in ["impl", concat!("macro_", "rules!")] {
            let found = block.match_indices(leak).any(|(at, _)| {
                let before = block[..at].chars().next_back();
                let after = block[at + leak.len()..].chars().next();
                let starts = !matches!(before, Some(c) if c.is_alphanumeric() || c == '_');
                // `!` already ends `macro_rules!`; `impl` needs a boundary of
                // its own, and `<` (a generic impl) is one.
                let ends = leak.ends_with('!')
                    || !matches!(after, Some(c) if c.is_alphanumeric() || c == '_');
                starts && ends
            });
            assert!(
                !found,
                "`mod send_fetch_thread` contains the token {leak:?}. The module is two spawners \
                 and one private fetch; an impl or a macro here is a way to name the fetch from \
                 outside that the visibility scan above cannot see. A trait method's visibility \
                 is the trait's, not the impl's, so it carries no `pub` for that scan to find"
            );
        }
    }

    /// **`crate::send`'s public surface is exactly these items -- an
    /// equality, over the whole module including its descendants.**
    ///
    /// This is the shape the previous round designed and did not write. Its
    /// doc comment said so in as many words: the counts above are counts, so
    /// they are "one spelling away from a survivor", and "the shape that
    /// would end the argument is an EQUALITY over this module's whole public
    /// surface, the way `mod send_fetch_thread`'s export list is an
    /// equality." Then a survivor arrived that was not one spelling away but
    /// one FILE away -- see [`send_module_files`] -- and adding that file
    /// needed no counted spelling at all.
    ///
    /// The counts above now read the whole closure, which kills that mutant
    /// on `CliSendRunner` and on `list_invocation`. This test is the half
    /// that does not depend on guessing which token the next one will spell.
    /// **Every `pub` in `crate::send` is listed here**, at any nesting depth
    /// -- module items, struct fields, inherent methods, trait methods -- so
    /// that a new door out of the module fails whether it is written at
    /// column zero of `send.rs`, inside an `impl` on an already-`pub` type,
    /// or in a brand-new descendant file. `pub mod inner;` is a new entry.
    /// So is `pub fn warm`, wherever it is put. So is a `pub use`.
    ///
    /// The list is deliberately literal rather than summarised: what makes it
    /// a wall instead of a count is that ADDING anything fails, and a
    /// summary is a thing an addition can be made to fit.
    ///
    /// **What still gets past it, said plainly.** A door does not have to be
    /// `pub`. `list_sends` is `pub(crate)`, `real_send_list` is
    /// `pub(super)`, and a `pub(crate) fn` written in `send.rs` carries the
    /// blocking fetch to every file in the crate while spelling no `pub `
    /// this scan collects. That is not an open hole today, because such a
    /// function cannot reach a `bw` child without spelling one of the needles
    /// counted above -- `CliSendRunner`, `list_invocation` or `list_sends`
    /// -- inside the closure, where all three are pinned to their definition
    /// counts. The two halves are load-bearing together and neither is
    /// sufficient alone.
    #[test]
    fn the_public_surface_of_the_send_module_is_exactly_these_items() {
        let files = crate_sources();
        let send_module = send_module_files(&files);
        assert!(
            send_module.contains(&"send.rs".to_string()),
            "control: the `crate::send` closure is {send_module:?}, which does not contain \
             `send.rs`"
        );

        // Every line of the module's production whose first token is `pub`,
        // squashed so this pins the code and not the formatter. The region is
        // `production_region`, so a `pub fn` written in a doc comment is
        // blanked and is not a door -- and a `#[cfg(test)]` item is not one
        // either.
        let mut surface: Vec<String> = Vec::new();
        for file in &send_module {
            let text = files
                .iter()
                .find(|(p, _)| p == file)
                .map(|(_, t)| production_region(t))
                .unwrap_or_else(|| panic!("`src/{file}` is in the closure but not the walk"));
            for line in text.lines() {
                let line = line.trim();
                if line == "pub" || line.starts_with("pub ") || line.starts_with("pub(") {
                    surface.push(format!("{file}: {}", squashed(line)));
                }
            }
        }

        // Control: the scan really found the module's declarations. Without
        // it an empty `surface` would match an empty expectation and this
        // test would pass over a module that had been deleted.
        assert!(
            surface.len() > 30,
            "control: the `pub` scan over {send_module:?} found only {} items, which is not \
             this module -- the region cut or the line test is wrong, and an equality against \
             nothing is not an equality",
            surface.len()
        );

        let expected: Vec<String> = SEND_PUBLIC_SURFACE.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            surface, expected,
            "`crate::send`'s public surface is not the pinned one. An ADDITION here is a new \
             door out of the module the frame closure can call -- `pub mod inner;`, a \
             `pub fn warm`, a `pub use`, a `pub fn` bolted onto an already-`pub` type's \
             `impl`, in `send.rs` or in any descendant file -- and behind that door sit the \
             private runner, its private fields and the private `list_invocation`, which \
             together are an up-to-sixty-second blocking `bw send list` on the eframe thread. \
             A DELETION here is a route the counts elsewhere in this module are still pinned \
             to. Either way this list is the deliberate decision, so change it deliberately"
        );
    }

    /// Every `pub` declaration in `crate::send`'s production, file by file,
    /// in source order. See
    /// [`the_public_surface_of_the_send_module_is_exactly_these_items`].
    const SEND_PUBLIC_SURFACE: &[&str] = &[
        "send.rs: pub const DELETE_IN_DAYS_CHOICES: [u8; 3] = [1, 7, 30];",
        "send.rs: pub const DEFAULT_DELETE_IN_DAYS: u8 = 7;",
        "send.rs: pub struct SendPlan {",
        "send.rs: pub name: String,",
        "send.rs: pub text: Zeroizing<String>,",
        "send.rs: pub hidden: bool,",
        "send.rs: pub delete_in_days: u8,",
        "send.rs: pub password: Option<Zeroizing<String>>,",
        "send.rs: pub max_access_count: Option<u32>,",
        "send.rs: pub fn validate_plan(plan: &SendPlan) -> Option<&'static str> {",
        "send.rs: pub trait SendClock {",
        "send.rs: pub struct FixedClock(pub i64);",
        "send.rs: pub struct SystemClock;",
        // `zone` was added beside `now` when the sentence stopped naming the
        // UTC day and started naming the user's own. Both are injected, and
        // that is the point of both: nothing in `send.rs` reads the machine's
        // clock or the machine's timezone for itself.
        "send.rs: pub fn expiry_wording(days: u8, now: &dyn SendClock, zone: &dyn LocalOffset) -> String {",
        "send.rs: pub struct SendInvocation {",
        "send.rs: pub fn args(&self) -> &[String] {",
        "send.rs: pub fn stdin_json_b64(&self) -> &str {",
        "send.rs: pub fn session_token(&self) -> Option<&str> {",
        "send.rs: pub fn plan_to_invocation(",
        // **Added by the record-import plan's task 7, deliberately.** It is a
        // `pub fn` where `list_invocation` and `delete_invocation` beside it
        // are private, and that difference is the whole of the decision: those
        // two each have a `pub` entry point that RUNS them, so the builder
        // itself never had to be reachable. The fetch path has no runner yet,
        // so the import surface being built on top of it needs the builder.
        // Nothing behind this door spawns anything -- building a
        // `SendInvocation` starts no process, and there is still no `pub`
        // implementation of `SendRunner` in this crate to hand one to.
        "send.rs: pub fn receive_invocation(url: &str, password: Option<&str>) -> SendInvocation {",
        "send.rs: pub struct CreatedSend {",
        "send.rs: pub id: String,",
        "send.rs: pub name: String,",
        "send.rs: pub access_url: String,",
        "send.rs: pub deletion_date: String,",
        "send.rs: pub struct SendSummary {",
        "send.rs: pub id: String,",
        "send.rs: pub name: String,",
        "send.rs: pub access_url: String,",
        "send.rs: pub deletion_date: String,",
        "send.rs: pub is_file: bool,",
        // Re-pinned deliberately. `ElidedAccessUrl` is a zero-sized `Debug`
        // stand-in and NOT a door out of the module in the sense this wall
        // guards: it carries no data, reaches no `bw` child, and its whole
        // behaviour is to write one sentence saying a URL was withheld. It
        // was widened from private to `pub(crate)` so the vault window's
        // `SendCreateReport` -- which holds a copy of an access URL and used
        // to derive `Debug` over it -- elides that URL through the same type,
        // rather than growing a second stand-in beside a second copy of the
        // "do not split on `#`" reasoning.
        "send.rs: pub(crate) struct ElidedAccessUrl;",
        "send.rs: pub struct RawOutput {",
        "send.rs: pub exit_code: Option<i32>,",
        "send.rs: pub stdout: String,",
        "send.rs: pub stderr: String,",
        "send.rs: pub enum SendError {",
        "send.rs: pub fn user_message(&self) -> &str {",
        "send.rs: pub fn is_ambiguous(&self) -> bool {",
        "send.rs: pub fn classify_failure(exit_code: Option<i32>, stdout: &str, stderr: &str) -> SendError {",
        "send.rs: pub fn parse_created_send(stdout: &str) -> Result<CreatedSend, SendError> {",
        "send.rs: pub fn parse_send_list(stdout: &str) -> Result<Vec<SendSummary>, SendError> {",
        "send.rs: pub trait SendRunner {",
        "send.rs: pub fn create_send<R: SendRunner>(",
        "send.rs: pub(crate) fn list_sends<R: SendRunner>(runner: &R) -> Result<Vec<SendSummary>, SendError> {",
        "send.rs: pub fn delete_send<R: SendRunner>(runner: &R, id: &str) -> Result<(), SendError> {",
        "send.rs: pub const SEND_TIMEOUT: Duration = Duration::from_secs(60);",
        "send.rs: pub enum WaitDecision {",
        "send.rs: pub fn wait_decision(exited: bool, elapsed: Duration, cap: Duration) -> WaitDecision {",
        "send.rs: pub fn raw_output_from(exit_code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> RawOutput {",
        "send.rs: pub fn cli_send_list(",
        // **Added by Sends step 4, deliberately.** `delete_send` is generic
        // over `SendRunner` and this crate has no `pub` implementation of
        // that trait, so a revoke wired from `vault_window` had a choice
        // between one new door here and making the runner nameable from
        // outside -- and the second is the wall itself. It is the exact
        // counterpart of the line above it and carries the same three
        // parameters plus the id.
        "send.rs: pub fn cli_send_delete(",
        // **Added by Sends step 5, deliberately**, and for the line above's
        // reason word for word: `create_send` is generic over `SendRunner`,
        // this crate has no `pub` implementation of that trait, and the only
        // alternative to one new door here was making the runner nameable
        // from outside -- which is the wall itself. It carries the same three
        // parameters the other two do, plus the plan and the clock.
        "send.rs: pub fn cli_send_create(",
        // **Added by the record import's wiring, deliberately, and it is the
        // only item that work adds to this wall.** `receive_send` -- the
        // generic half beside `create_send`, `list_sends` and `delete_send` --
        // is deliberately PRIVATE, so it is not on this list at all: nothing
        // outside `crate::send` needs to drive a receive against a substituted
        // runner, and this module's own tests are the ones that do. That left
        // exactly the choice the three rows above it record: one `pub` entry
        // point that builds the private runner itself, or making
        // `CliSendRunner` nameable from outside -- which is the wall itself.
        //
        // **It takes no session, and it is the only one of the four that does
        // not.** Fetching a Send is anonymous -- the link is the credential --
        // so this door builds its runner with `CliSendRunner::new` rather than
        // `with_session`, and `BW_SESSION`, which unlocks the whole vault, is
        // never handed to a child that has no use for it. That is why the
        // `CliSendRunner::new` row in the seal above moves from 0 to 1 with
        // this commit while `CliSendRunner::with_session` does not move at all.
        "send.rs: pub fn cli_send_receive(",
    ];

    /// **Nothing waits on the Sends channel from the frame's own thread.**
    ///
    /// The freeze has two ends. The fetch end is held by the seal above; this
    /// is the channel end, and it was held by a single literal `contains` of
    /// `send_rx.recv()`. Measured on `0eeb749`, replacing the drain with
    ///
    /// ```ignore
    /// if let Ok((tag, result)) = send_rx.recv_timeout(Duration::from_secs(60)) {
    /// ```
    ///
    /// gave 2059 lib + 217 bin, 0 failed -- the identical sixty-second freeze
    /// the banned spelling describes, under a name nobody had banned.
    /// `recv_deadline`, `send_rx.iter()`, `for .. in send_rx` and
    /// `into_iter()` were all unbanned too.
    ///
    /// Needles, said plainly. Two of them, and they are complementary: the
    /// blocking method names are banned outright in both files' production
    /// (so renaming the binding does not help), and every occurrence of the
    /// receiver itself must be followed by `try_recv` or by nothing at all
    /// (so an iterator over it, which has no `.recv` in its text, is caught
    /// as well).
    ///
    /// **And both of them were beaten, so the primary hold is now a type.**
    /// Measured on `4446e9a`: because `this_files_production()` cut this file
    /// at line 718, a `pub(super) struct RxHolder(Receiver<Answer>)` with
    /// `fn wait(&self) { self.0.recv().ok() }` written BELOW that cut -- still
    /// compiled production -- plus, in the frame closure,
    ///
    /// ```ignore
    /// let waiter = send_ui::RxHolder::new(send_rx);
    /// if let Some((tag, result)) = waiter.wait() { .. }
    /// ```
    ///
    /// gave 2061 lib + 217 bin, 0 failed: every `send_rx` token still passed
    /// the follow-character rule, `seen` was still 2, and the frame closure
    /// blocked on `recv()` for sixty seconds. Both ends of that are fixed --
    /// the region is no longer cut (see [`production_region`]) and, more to
    /// the point, `send_rx` is a `send_channel::SendListReceiver` now, which
    /// has no blocking drain to move anywhere. The needles below stay as the
    /// second line: they are what catches a *new* raw `mpsc::Receiver` being
    /// introduced beside the sealed one.
    #[test]
    fn the_sends_answer_is_never_waited_for_on_the_frames_own_thread() {
        // Sanitized on both sides: a `.recv()` written inside a doc comment
        // explaining why `.recv()` is banned is not a call, and a guard that
        // cannot survive its own explanation gets deleted.
        for (file, text) in
            [("mod.rs", sanitized(&production())), ("send_ui.rs", this_files_production())]
        {
            for banned in
                [concat!(".recv", "()"), concat!(".recv_", "timeout("), concat!(".recv_", "deadline(")]
            {
                assert!(
                    !text.contains(banned),
                    "{file}'s production contains {banned:?} -- something on the eframe thread \
                     waits for a channel instead of draining it with `try_recv`, which is the \
                     window freeze the whole off-thread fetch exists to prevent"
                );
            }
        }

        let production = sanitized(&production());
        let production = production.as_str();
        let name = concat!("send_", "rx");
        let mut seen = 0usize;
        for (at, _) in production.match_indices(name) {
            let before = production[..at].chars().next_back();
            if matches!(before, Some(c) if c.is_alphanumeric() || c == '_') {
                continue;
            }
            seen += 1;
            let after = &production[at + name.len()..];
            if after.starts_with('.') {
                assert!(
                    after.starts_with(concat!(".try_", "recv()")),
                    "the Sends receiver is used as {:?} -- the only non-blocking drain is \
                     `try_recv`, and everything else waits on the eframe thread",
                    after.chars().take(32).collect::<String>()
                );
            }
            let head = production[..at].trim_end();
            assert!(
                !head.ends_with(" in") && !head.ends_with('&'),
                "the Sends receiver is iterated ({:?}). A `for` over a receiver blocks on every \
                 step exactly as `recv()` does, and has no `.recv` in its text",
                head.chars().rev().take(24).collect::<String>().chars().rev().collect::<String>()
            );
        }
        // **Exactly two, and the second one is the drain, at the frame
        // closure's own indentation.**
        //
        // `>= 2` was a control against the channel being deleted; it is a
        // requirement now, because the last freeze left is one this file
        // cannot see any other way. Measured on this change before this
        // paragraph existed:
        //
        // ```ignore
        // let blocked = loop {
        //     if let Ok(v) = send_rx.try_recv() { break Some(v); }
        // };
        // ```
        //
        // gave 2068 lib + 217 bin, 0 failed. Every guard was satisfied --
        // `try_recv` is the only drain, the receiver has no blocking method,
        // nothing is aliased -- and the eframe thread spun until an answer
        // came, which is the same sixty seconds with the CPU pinned. A
        // non-blocking drain in a loop is a blocking drain.
        //
        // Needle, said plainly: two occurrences, and the drain is the exact
        // line, at exactly the closure's top-level indentation. A `loop`
        // wrapped round it indents it and adds a third mention. What beats
        // this is writing the loop *without* re-indenting; what catches that
        // is that nothing else in this file is written that way and rustfmt
        // does not produce it. The receiver's type is the primary hold; this
        // is the second line.
        assert_eq!(
            seen, 2,
            "{name:?} occurs {seen} times in production as its own token, not twice. It is \
             bound once and drained once; a third mention is the receiver being read from a \
             second place, and the shape that matters is a spin loop -- `try_recv` called \
             round and round on the eframe thread is a blocking wait with the CPU pinned"
        );
        let drain = concat!("\r\n        if let Ok((tag, result)) = send_rx.try_", "recv() {\r\n");
        assert_eq!(
            production.matches(drain).count(),
            1,
            "{drain:?} is not in production exactly once at the frame closure's top level. \
             Indented further, the drain is inside a conditional or a loop; the loop is the \
             freeze this whole design exists to prevent, spelled with the permitted method"
        );
    }

    /// The frame closure's whole text, from the `{` of
    /// `let vault_frame_fn = move |ui: &mut egui::Ui ..` to its matching `}`,
    /// sanitized.
    ///
    /// Sanitized, and the braces counted over the sanitized text, because a
    /// `{` inside a doc comment or a string would otherwise shift every depth
    /// below it -- and this file's comments are full of both.
    fn frame_closure() -> String {
        let production = sanitized(&production());
        let head = concat!("let vault_frame_fn = ", "move |ui: &mut egui::Ui");
        assert_eq!(
            production.matches(head).count(),
            1,
            "{head:?} is not in production exactly once -- the frame closure has been renamed \
             or duplicated, and every assertion taken over its body below is reading nothing"
        );
        let at = production.find(head).expect("counted just above");
        let rest = &production[at..];
        let open = rest.find('{').expect("the frame closure has no body");
        let b = rest.as_bytes();
        let mut depth = 0usize;
        let mut i = open;
        let end = loop {
            assert!(i < b.len(), "the frame closure's body is never closed");
            match b[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break i + 1;
                    }
                }
                _ => {}
            }
            i += 1;
        };
        rest[open..end].to_string()
    }

    /// **The Sends drain sits at the closure's own statement level.**
    ///
    /// The property that the frame does not WAIT is no longer held here. It
    /// is held behaviourally, by running the real closure and timing it --
    /// see `frame_promptness::the_loaded_vault_returns_promptly` at the
    /// bottom of this file. What used to be here beside this assertion was a
    /// ban on `loop {` and a ten-word list of `std::sync::mpsc` vocabulary,
    /// and both were DELETED in the same commit that added the harness,
    /// because they were measured not to hold the property they claimed:
    ///
    ///  * **M-A** -- `let t = Instant::now(); while t.elapsed().as_secs() <
    ///    60 { std::hint::spin_loop(); }` at the closure's own statement
    ///    level. `while`, not `loop`; none of the ten words; drain still at
    ///    depth 1. Sixty seconds per frame and a core burnt.
    ///  * **M-I** -- a plain `fn settle_before_paint()` defined one line
    ///    ABOVE the closure and called from inside it. The scan has no reach
    ///    past the closure at all.
    ///  * **M-B**, **M-C**, **M-13** -- `Mutex::lock` on a mutex this thread
    ///    already holds, `JoinHandle::join` on a sixty-second thread, and
    ///    `stdin().lock().read_line(..)`. `lock`, `join` and `read_line` are
    ///    not `mpsc` vocabulary and were never on any list.
    ///
    /// All five were measured green at 2072 lib + 217 bin, 0 failed. The
    /// class of ways to make a frame wait is unbounded and text scanning
    /// cannot enumerate it, so the list was theatre and the maintenance cost
    /// of theatre is real. What survives here is the ONE thing the harness
    /// does not subsume, and it earns its place because **M-E** -- the drain
    /// moved to brace depth 2, i.e. wrapped in a `loop`/`while`/`if` -- was
    /// measured to die on it:
    ///
    ///  * The drain sits at **brace depth 1**, counted, not inferred from
    ///    leading spaces. **M-2** was a spin loop written without
    ///    re-indenting the drain; the indentation pin it beat held nothing,
    ///    because indentation is a convention and a mutation is not obliged
    ///    to follow it.
    ///
    /// A drain wrapped in a `loop` is also a frame that never returns, so
    /// the harness kills it too. This is the cheap, precise half, and it
    /// names the defect instead of reporting a stopwatch.
    #[test]
    fn the_sends_drain_is_at_the_closures_own_statement_level() {
        let closure = frame_closure();
        assert!(
            closure.len() > 50_000,
            "control: the frame closure slice is only {} bytes, so the assertions below are \
             reading a fragment",
            closure.len()
        );

        let drain = concat!("if let Ok((tag, result)) = send_rx.try_", "recv() {");
        assert_eq!(
            closure.matches(drain).count(),
            1,
            "{drain:?} is not in the frame closure exactly once"
        );
        let before = &closure[..closure.find(drain).expect("counted just above")];
        let depth = before.matches('{').count() as isize - before.matches('}').count() as isize;
        assert_eq!(
            depth, 1,
            "the Sends drain sits at brace depth {depth} inside the frame closure, not at the \
             closure's own statement level. Anything deeper is a `loop`, a `while` or an `if` \
             wrapped round it -- and a `try_recv` called round and round on the eframe thread \
             is a blocking wait with a core burnt as well. Indentation is not what is measured \
             here, precisely because a mutation is free to ignore it"
        );

    }

    /// **The Sends receiver has no blocking drain, by type.**
    ///
    /// This is the structural half of the test above, and the part that does
    /// not depend on any needle: the frame closure never holds an
    /// `mpsc::Receiver` for this channel at all. It holds a
    /// `send_channel::SendListReceiver`, whose only method is `try_recv` and
    /// whose wrapped receiver is private to `mod send_channel` -- a module
    /// with no descendants, so unlike a private field of `vault_window`
    /// itself, `vault_window::send_ui` cannot reach it either. Wrapping the
    /// value in a holder struct, in this file or any other, carries nothing
    /// to wait on; reaching past it is an `E0616`.
    ///
    /// What source has to hold is the shape of the boundary: that the channel
    /// really is built through it, and that the module does not grow a way
    /// out.
    #[test]
    fn the_sends_receiver_is_a_type_with_no_blocking_drain() {
        let production = sanitized(&production());
        let production = production.as_str();

        let build = concat!("send_channel::send_list_", "channel()");
        assert_eq!(
            production.matches(build).count(),
            1,
            "{build:?} is not in production exactly once -- the Sends channel is built somewhere \
             other than behind its sealing constructor, which means a raw `mpsc::Receiver` for \
             it exists for somebody to keep and block on"
        );

        // The module's block, sliced the way `sealed_module` slices the other
        // one, and every `pub` in it listed whatever item it sits on.
        let opener = concat!("mod send_", "channel {\r\n");
        assert_eq!(
            production.matches(opener).count(),
            1,
            "{opener:?} is not in production exactly once"
        );
        let start = production.find(opener).expect("counted just above");
        let rest = &production[start + opener.len()..];
        let end = rest.find("\r\n}\r\n").expect("`mod send_channel` has no column-zero close");
        let block = &rest[..end];
        assert!(block.len() > 200, "control: the slice is only {} bytes", block.len());

        let exported: Vec<String> = block
            .match_indices("pub")
            .filter(|(at, _)| {
                let before = block[..*at].chars().next_back();
                let after = block[at + "pub".len()..].chars().next();
                !matches!(before, Some(c) if c.is_alphanumeric() || c == '_')
                    && matches!(after, Some(c) if c == '(' || c.is_whitespace())
            })
            .map(|(at, _)| block[at..].lines().next().unwrap_or_default().trim_end().to_string())
            .collect();
        assert_eq!(
            exported,
            vec![
                concat!("pub(super) type SendList", "Answer ="),
                concat!("pub(super) struct SendList", "Receiver(mpsc::Receiver<SendListAnswer>);"),
                concat!("pub(super) fn try_", "recv(&self) -> Result<SendListAnswer, mpsc::TryRecvError> {"),
                concat!("pub(super) fn send_list_", "channel() -> (super::SendListSender, SendListReceiver) {"),
            ],
            "`mod send_channel` no longer exports exactly the answer type, the sealed receiver, \
             its one non-blocking drain and the constructor. Anything else here -- an \
             `into_inner`, a `pub(super)` field, a `Deref`, a `pub(super) use` of \
             `mpsc::Receiver` -- hands the raw receiver back out, and a raw receiver in the \
             frame closure is a sixty-second freeze one `.recv()` away"
        );
        // A trait impl carries methods out without a `pub` for the scan above
        // to see; `Deref<Target = mpsc::Receiver<_>>` would hand back every
        // blocking method at once.
        for leak in ["impl", concat!("macro_", "rules!")] {
            let found = block.match_indices(leak).any(|(at, _)| {
                let before = block[..at].chars().next_back();
                let after = block[at + leak.len()..].chars().next();
                !matches!(before, Some(c) if c.is_alphanumeric() || c == '_')
                    && (leak.ends_with('!')
                        || !matches!(after, Some(c) if c.is_alphanumeric() || c == '_'))
            });
            // The one permitted `impl` is the inherent block holding
            // `try_recv`, so this is a count and not an absence.
            if leak == "impl" {
                let opener = concat!("impl SendList", "Receiver {");
                assert_eq!(
                    block.matches("impl").count(),
                    1,
                    "`mod send_channel` has more than one `impl`. The only one permitted is \
                     {opener:?}; a trait impl here carries methods out with no `pub` to see"
                );
                assert!(block.contains(opener), "the inherent impl is not {opener:?}");
            } else {
                assert!(!found, "`mod send_channel` contains {leak:?}");
            }
        }
    }

    /// **The drain goes through `apply_answer`.** That function is where the
    /// late-answer rule and the `in_flight` clear are both tested; a drain
    /// that writes `send_fetch.result` itself is a drain that has neither.
    #[test]
    fn the_sends_drain_applies_the_answer_rather_than_storing_it_directly() {
        let production = production();
        let apply = concat!("send_fetch.apply_", "answer(tag, result)");
        assert_eq!(
            production.matches(apply).count(),
            1,
            "{apply:?} is not in production exactly once -- the Sends drain is not going \
             through the tested rule, so an answer from a visit the user has left can be \
             written in as though it were current"
        );
        let direct = concat!("send_fetch.result = ", "Some(");
        assert!(
            !production.contains(direct),
            "production writes {direct:?} directly, bypassing the generation check"
        );
    }

    /// Leaving the screen drops the list. This is the refetch policy, and it
    /// is an **absence** in the wrong shape: with it gone, every pure test in
    /// this file still passes and the only symptom is a Sends list that
    /// silently stops matching the server.
    ///
    /// **What changed, and why this pin is now thin.** It used to be an `if`
    /// in the frame closure, guarded by a predicate pinned once and an
    /// `invalidate();` that only had to exist *somewhere*. Measured on
    /// `c14afb2`, replacing the `if`'s body with a `log::trace!` and adding a
    /// semicolon to the unrelated notice-arm `invalidate()` gave 2050 lib +
    /// 217 bin, 0 failed: the refetch policy entirely deleted, green. The
    /// decision lives in `SendFetch::note_screen` now, where
    /// `leaving_the_sends_screen_and_returning_asks_the_server_again` and its
    /// three siblings run it for real. The frame's whole part is one call
    /// with one argument, so there is no body left to hollow out and nothing
    /// this pin has to describe except that the call is made, once, at the
    /// closure's top level, and made *before* the fetch gate reads the state
    /// it clears.
    #[test]
    fn leaving_the_sends_screen_invalidates_the_list() {
        let production = production();

        // The leading newline and eight spaces put this at the frame
        // closure's own top level -- the same indentation `if !show_sends {`
        // sits at. A call moved inside any conditional is indented further
        // and fails here, which matters because "invalidate only sometimes"
        // is exactly the mutation this file has already been beaten by.
        let call = concat!("\r\n        send_fetch.note_", "screen(on_sends);\r\n");
        assert_eq!(
            production.matches(call).count(),
            1,
            "{call:?} is not in production exactly once at the frame closure's top level -- the \
             Sends list is never refreshed, so a Send deleted or expired elsewhere keeps its \
             Copy link button forever"
        );

        // Ordering. The call above and the gate below both pass, in either
        // order, but only one order works: `note_screen` after the gate means
        // the frame the user returns on still sees the previous visit's
        // `Some(..)`, `wants_fetch` is false, and the stale list is drawn
        // with no refetch ever -- the very defect the policy exists for.
        let gate = concat!("send_fetch.wants_", "fetch(show_sends)");
        assert_eq!(
            production.matches(gate).count(),
            1,
            "{gate:?} is not in production exactly once, so the ordering assertion below reads \
             the wrong gate"
        );
        assert!(
            production.find(call).expect("counted above")
                < production.find(gate).expect("counted above"),
            "the Sends fetch gate is consulted BEFORE the leave rule is applied, so a visit \
             returning to Sends reads the previous visit's list as current and never refetches"
        );

        // The predicate is no longer consulted from the frame at all: it is
        // `note_screen`'s, in a file with tests that can run it.
        let old = concat!("should_invalidate_on_", "leave");
        assert!(
            !production.contains(old),
            "the frame closure consults {old:?} directly again. The point of `note_screen` is \
             that the rule, the action and the remembering cannot be separated -- spelled out \
             in the closure they can be, and were"
        );
    }

    /// The Sends screen replaces the item list rather than being drawn beside
    /// it, and the item list is not asked to render Sends.
    ///
    /// **This pins the coverage, not the gate.** Counting the gate alone
    /// says only that it exists somewhere; a second, ungated
    /// `draw_item_list` leaves that count at one and puts the item list back
    /// on the Sends screen. So the gate's own block is sliced out -- to the
    /// next `}` at the gate's indentation -- and the item list panel and its
    /// draw call are required to be inside *it*, and to exist nowhere else.
    ///
    /// **The gate is `!show_sends && !on_health`**, because the item-list
    /// column now has a second screen that takes it over: the Password
    /// health report (`vault_window::password_health`). Nothing here was
    /// weakened to let it in -- that screen draws its own panel under its own
    /// id, so the `Panel::left("vault-item-list")` count below still says
    /// "the item list is drawn exactly once" and the block slice still says
    /// "and that once is under this gate". `password_health` carries the
    /// mirror of this test for its own pane.
    #[test]
    fn the_item_list_is_drawn_only_inside_the_not_sends_gate() {
        let production = production();
        let gate = concat!("        if !show_", "sends && !on_health {\r\n");
        assert_eq!(
            production.matches(gate).count(),
            1,
            "{gate:?} is not in production exactly once -- the item list has been given a \
             second gate, or the gate has moved out of the frame closure's top level"
        );
        let start = production.find(gate).expect("gate");
        let rest = &production[start + gate.len()..];
        let end = rest
            .find("\r\n        }\r\n")
            .expect("the `!show_sends` block has no closing brace at its own indentation");
        let block = &rest[..end];

        for needle in [
            concat!("egui::Panel::", "left(\"vault-item-list\")"),
            concat!("draw_item_", "list("),
        ] {
            assert_eq!(
                production.matches(needle).count(),
                1,
                "{needle:?} appears in production {} times, not once -- a second one is an \
                 item list drawn on the Sends screen",
                production.matches(needle).count()
            );
            assert!(
                block.contains(needle),
                "{needle:?} is outside the `!show_sends` block, so the item list is drawn on \
                 the Sends screen"
            );
        }

        // Squashed, because step 4 wrapped this call over five lines when it
        // grew the delete-state argument. The needle is still the WHOLE call
        // -- the argument list included -- so a second draw site, or a site
        // handed a delete state that is not the window's own, fails here.
        let pane = squashed(concat!(
            "send_ui::draw_send_", "pane( ui, state, notice_message.as_deref(), \
             send_delete.view(), &mut send_create.composer, send_create.in_flight, \
             &crate::send::SystemClock, &crate::local_time::SystemZone, )"
        ));
        assert_eq!(
            squashed(&production).matches(pane.as_str()).count(),
            1,
            "{pane:?} is not in production exactly once -- the Sends pane is drawn from more \
             than one place, from none, or with a delete state that is not the window's own"
        );
    }

    /// **Nothing outside `send.rs` builds a Send invocation of its own.**
    ///
    /// Step 4 landed the revoke, so the old wording -- "this step is the
    /// read-only one; a call site appearing here is the whole ordering
    /// undone" -- is no longer what this holds. It is narrowed rather than
    /// deleted, because what is left is still load-bearing: `delete_send` is
    /// the GENERIC entry point, over any `SendRunner`, and the window may not
    /// reach it. The window's one route is `crate::send::cli_send_delete`,
    /// which is the only one that carries the job, the profile directory and
    /// the session together. `create_send` is step 5 and still has no call
    /// site anywhere.
    #[test]
    fn this_window_can_neither_create_nor_delete_a_send() {
        let source = include_str!("mod.rs");
        for forbidden in [concat!("create_", "send("), concat!("delete_", "send(")] {
            assert!(
                !source.contains(forbidden),
                "{forbidden:?} has a call site in the vault window -- this step is read-only, \
                 and revoke is step 4"
            );
        }
        let here = include_str!("send_ui.rs");
        for forbidden in [concat!("create_", "send("), concat!("delete_", "send(")] {
            assert!(!here.contains(forbidden), "{forbidden:?} has a call site in `send_ui`");
        }
    }

    // -----------------------------------------------------------------
    // The region of `mod.rs` BELOW the cut -- the half no pin here reads.
    //
    // `production()` is `mod.rs` up to its first `#[cfg(test)]` and nothing
    // else, and every pin in this module -- plus the ten identical
    // `production()` copies inside `mod.rs`'s own test modules -- is blind to
    // everything past it. Nothing asserted that the region past it was only
    // test modules. It is today, but "is" is not "stays": a real `pub fn`
    // appended at the end of `mod.rs` could spawn a process, or duplicate a
    // call site pinned at exactly one here, with the suite green. Measured in
    // exactly that shape on `send.rs` at `708a34d`. Same walk `breach.rs`,
    // `send.rs` and `vault_export.rs` carry.
    //
    // `mod.rs` carries a sibling of this walk of its own
    // (`nothing_but_gated_test_modules_lives_below_the_guards_cut`) and today
    // the two cut at the same byte. That is a fact, not a guarantee: it lives
    // in another module, over a marker of its own, and this file's pins would
    // go blind without a word if it were renamed, retargeted or deleted. So
    // this one is written independently and cut from THIS module's
    // `production()` -- assertion 1 below is what ties them. The exact
    // module count is left to the sibling; what is asserted here is the
    // shape, plus the self-controls (LF/CRLF agreement, and three mutants fed
    // to the walk) that the sibling does not carry.
    // -----------------------------------------------------------------

    /// The `cfg` attribute that makes a module test-only, split so this
    /// constant is not itself one and cannot be found by a search for the
    /// real attribute.
    const CUT_GATE: &str = concat!("#[cfg(", "test)]");

    /// Column-0 lines below the cut that are the CONTENTS OF A STRING LITERAL
    /// rather than source. Empty today, and controlled by the walk.
    const BELOW_CUT_STRING_LINES: &[&str] = &[];

    /// Where [`production`] cuts `mod.rs`: its FIRST [`CUT_GATE`].
    ///
    /// Deliberately the same rule -- a plain first-occurrence find -- rather
    /// than a better one, because the point of this walk is to inspect the
    /// region the pins are actually blind to, not a region a smarter cut
    /// would have given them. The equality control below ties the two.
    fn cut_index(source: &str) -> usize {
        source.find(CUT_GATE).expect("no `cfg(test)` attribute in `mod.rs`")
    }

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. Exact rather than a `starts_with`: a whole
    /// module written on one line is not a module opener here and must fail.
    fn below_cut_is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let rest = match t.strip_prefix("mod ") {
            Some(rest) => rest,
            None => return false,
        };
        let name = match rest.strip_suffix(" {") {
            Some(name) => name,
            None => return false,
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// What the region below `mod.rs`'s cut is walked under.
    ///
    /// The walk itself is [`crate::below_cut::walk`] and is NOT written here.
    /// It used to be, in fifteen near-identical copies, which is how the
    /// escaped-quote off-by-one in the brace matcher reached three files at
    /// once and how every fix since has had to be applied N times or silently
    /// fail to propagate. What the copies really disagreed about is this
    /// struct's worth of text, so that is what stayed local.
    ///
    /// Every knob here is the one the inline walk this replaced actually had,
    /// field by field: `gated_at_start: false` is its `let mut gated = false`,
    /// so the region begins with the gate itself and nothing outside it is
    /// taken on trust; `gate_at_column_zero: false` is its `trimmed ==
    /// CUT_GATE`; `string_lines` is the same [`BELOW_CUT_STRING_LINES`]; and
    /// the `line == "}"` close rule is the shared walk's, with the byte-offset
    /// check the inline copy did not have added on top. Nothing the old walk
    /// caught stopped being caught.
    ///
    /// `is_module_opener` is this module's OWN [`below_cut_is_module_opener`]
    /// and not [`crate::below_cut::is_module_opener`], deliberately: the
    /// `modules == column_zero_module_openers(..)` cross-check compares the
    /// walk's count against that OTHER instance, so a one-edit widening of
    /// either predicate desynchronizes the two and reds the suite. Pointing
    /// the walk at the shared predicate would make both sides move together
    /// and throw that property away.
    const BELOW_CUT_RULES: crate::below_cut::WalkRules = crate::below_cut::WalkRules {
        gate: CUT_GATE,
        gated_at_start: false,
        gate_at_column_zero: false,
        is_module_opener: below_cut_is_module_opener,
        string_lines: BELOW_CUT_STRING_LINES,
        top_level_item_note:
            "Every pin in this module reads only the half of `mod.rs` ABOVE the cut, so an \
             item down here is read by none of them: it can spawn a process on the eframe \
             thread, reintroduce a blocking `list_sends`, or duplicate a call site pinned at \
             exactly one -- and the suite stays green.",
        ungated_module_note:
            "A `pub(crate) mod ext { .. }` written down there is the same escape, one `mod` \
             deep.",
    };

    /// `(visited, modules, closes, depth)` for the region below `mod.rs`'s
    /// cut, by the one shared walk, so the caller can control it for
    /// non-vacuity.
    ///
    /// **Line-ending agnostic on purpose**: the shared walk strips a trailing
    /// carriage return from every line, so every comparison is against the
    /// line's real text on a CRLF tree and on an LF one alike.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = cut_index(source);
        crate::below_cut::walk(&source[cut..], &BELOW_CUT_RULES)
    }

    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_pins_cut() {
        let source = include_str!("mod.rs");
        let lf = source.replace("\r\n", "\n");

        // 1. The cut this control walks from agrees with what `production`
        //    actually returns, or the walk proves nothing about the region
        //    the pins can see.
        //
        //    **Not equality any more, and the difference is the point.**
        //    `production` used to BE this cut; it goes through
        //    `production_region_source` now, which removes each gated item
        //    and keeps everything between them -- so below the cut it keeps
        //    the blank lines separating the test modules, and above the cut
        //    it ignores a gate spelled inside a comment or a string, which a
        //    raw `find` cannot. What must still hold is the containment:
        //    everything before the cut is in the region, and everything the
        //    region has after it is whitespace. That is the same claim, said
        //    in the shape the region can answer -- and it fails loudly if a
        //    live production item ever appears below the first test module.
        let cut = cut_index(&lf);
        let region = production().replace("\r\n", "\n");
        assert!(
            region.starts_with(&lf[..cut]),
            "the production region does not begin with everything above the first test gate, \
             so `production_region_source` is dropping code the pins are supposed to read"
        );
        //    "Whitespace" is measured after [`sanitized`], and that is not a
        //    dodge: a doc comment written ABOVE a `#[cfg(test)] mod ..` sits
        //    before the attribute, so the region keeps it, and it is prose
        //    either way. What must not be down there is *code*, and blanking
        //    the comments is exactly how this control says so.
        let below = sanitized(&region[lf[..cut].len()..]);
        assert!(
            below.trim().is_empty(),
            "the production region contains {} bytes of live code BELOW the first test gate: \
             {:?}. Every pin in this module reads that region, so an item down there is an \
             item nothing above has looked at",
            below.trim().len(),
            below.trim().chars().take(200).collect::<String>()
        );

        // 2. Positive control on WHERE the cut is: the production half still
        //    reaches the last production item in the file. Were the cut to
        //    move UP -- into a doc comment or a string that happened to spell
        //    the gate -- this anchor would fall below it and every pin
        //    downstream would be reading a truncated file.
        const LAST_PRODUCTION_ITEM: &str =
            concat!("windows::Win32::UI::WindowsAndMessaging::SW_", "SHOWNORMAL,");
        assert_eq!(
            lf.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: the anchor is not in `mod.rs` exactly once, so it pins nothing -- \
             repoint it at the last production item above the first test module"
        );
        let anchor = lf.find(LAST_PRODUCTION_ITEM).expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, so the cut \
             moved up and the production half every pin reads is truncated"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk, over an LF copy and a CRLF copy of the same text,
        //    which must agree. Built both ways rather than compared against
        //    the bytes on disk: this repository stores LF blobs and only
        //    `core.autocrlf=true` makes a working tree CRLF, so a control
        //    that asserted "this file is CRLF" would pass here and fail on
        //    Linux.
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        assert_eq!(
            walk_below_the_cut(&lf),
            walk_below_the_cut(&crlf),
            "the walk gives a different answer on an LF copy of `mod.rs` than on a CRLF one"
        );
        let on_disk = walk_below_the_cut(source);
        assert!(
            on_disk == walk_below_the_cut(&lf) || on_disk == walk_below_the_cut(&crlf),
            "`mod.rs`'s line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) = on_disk;
        assert!(
            visited > 1_000,
            "control: the walk visited only {visited} lines below the cut, which is not this \
             file's worth of test modules -- the slice is empty and this test proves nothing"
        );
        assert_eq!(
            modules, closes,
            "below the cut {modules} test modules are opened and {closes} closed"
        );
        assert_eq!(depth, 0, "the walk ended inside a module, at depth {depth}");
        assert!(
            modules > 40,
            "control: only {modules} test modules were found below the cut, far fewer than \
             `mod.rs` has -- the walk stopped early"
        );

        // 5. Control on the walk itself: it really refuses production code
        //    down there. Without this the walk could be a no-op that visits
        //    lines and asserts nothing.
        let with_an_appended_item = format!("{lf}\npub fn sneaked() {{}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_appended_item)).is_err(),
            "control: the walk accepted a `pub fn` appended below the test modules, which is \
             the exact mutation it exists to catch"
        );
        // And an INDENTED one, which a column-0-only filter would miss.
        let with_an_indented_item = format!("{lf}\n    struct Sneaked(u8);\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_indented_item)).is_err(),
            "control: the walk accepted an INDENTED top-level item appended below the test \
             modules"
        );
        // And an ungated module, which ships.
        let with_an_ungated_module = format!("{lf}\nmod shipped {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_the_cut(&with_an_ungated_module)).is_err(),
            "control: the walk accepted an UNGATED module below the cut, which ships"
        );
    }

    // -----------------------------------------------------------------
    // The region of THIS FILE below ITS OWN cut -- which nothing read.
    //
    // The walk above reads `mod.rs`, and `mod.rs`'s own sibling guard reads
    // `mod.rs` too. Nothing in the crate read `send_ui.rs`'s tail: this file
    // has no guard over itself at all, and `job_object`'s crate-wide tail
    // fence covers `bw_path.rs` and `job_object.rs` only.
    //
    // Measured at `8906835`: a column-0
    // `pub fn shipped(x: u64) -> u64 { x.wrapping_mul(97) }` appended at EOF
    // SURVIVED the whole suite at 2224 lib / 217 bin / 0 failed / 0 warnings
    // in BOTH profiles and shipped three times over in the lib's DEBUG LLVM
    // IR. One edit, zero guard edits -- the cheapest surviving route to a
    // shipping `pub fn` below a cut anywhere in the crate. The identical
    // payload is KILLED in `signature.rs` by that file's guard and in
    // `send.rs` by its `runner_tests` sibling; the difference was that those
    // files are read and this one was not. This is the guard that reads it.
    // -----------------------------------------------------------------

    /// The FIRST [`CUT_GATE`] in `source` that begins a line.
    ///
    /// **Not a plain `find`, and that is the whole reason this helper exists.**
    /// This file spells the gate inside a doc comment some five hundred lines
    /// above its first test module -- the paragraph about the abandonment
    /// counter's two hard assertions being test-only -- so
    /// [`cut_index`]'s first-occurrence rule would cut there and hand the walk
    /// a region beginning in the middle of production code. Every line of that
    /// production half would then be refused as top-level source, which is a
    /// guard that fails for a reason that is not the one it names, and the tail
    /// it exists to read would never be reached.
    ///
    /// The line-start rule is also, by itself, the protection the anchor
    /// control below gives after the fact: a gate spelled inside a string or a
    /// comment cannot satisfy it. Both are kept -- the anchor also catches the
    /// cut moving DOWN, which the line-start rule cannot see.
    fn own_cut_index(source: &str) -> usize {
        let bytes = source.as_bytes();
        source
            .match_indices(CUT_GATE)
            .map(|(at, _)| at)
            .find(|&at| at == 0 || bytes[at - 1] == b'\n')
            .expect("no column-0 test gate in `send_ui.rs`, so this file has no cut to walk from")
    }

    /// What the region below THIS file's cut is walked under.
    ///
    /// A second [`crate::below_cut::WalkRules`] rather than a reuse of
    /// [`BELOW_CUT_RULES`]: the notes name the concrete damage, and the damage
    /// an item below THIS file's cut does is not the damage an item below
    /// `mod.rs`'s cut does. The predicate is the same local
    /// [`below_cut_is_module_opener`], which is what keeps the
    /// `column_zero_module_openers` cross-check below comparing two genuinely
    /// different instances.
    const OWN_BELOW_CUT_RULES: crate::below_cut::WalkRules = crate::below_cut::WalkRules {
        gate: CUT_GATE,
        gated_at_start: false,
        gate_at_column_zero: false,
        is_module_opener: below_cut_is_module_opener,
        string_lines: BELOW_CUT_STRING_LINES,
        top_level_item_note:
            "Nothing in this crate reads the tail of `send_ui.rs`: the pins in this module \
             read `mod.rs`, the source pins above slice this file at its first test gate and \
             read only what is ABOVE it, and the crate-wide tail fence in `job_object` names \
             `bw_path.rs` and `job_object.rs`. An item down here ships and no guard has \
             looked at it.",
        ungated_module_note:
            "A `pub(crate) mod ext { .. }` written down there is the same escape, one `mod` \
             deep, and its contents are read by nothing either.",
    };

    /// `(visited, modules, closes, depth)` for the region below THIS file's
    /// own cut.
    ///
    /// The cut is recomputed INSIDE the string being sliced, every time. A
    /// byte offset taken in [`include_str!`]'s CRLF working-tree bytes and
    /// used to slice an LF copy is off by one byte per line -- measured at
    /// 7355 bytes in `below_cut.rs`, which landed a control's slice in the
    /// middle of a function body and made it pass forever for the wrong
    /// reason.
    fn walk_below_this_files_own_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = own_cut_index(source);
        crate::below_cut::walk(&source[cut..], &OWN_BELOW_CUT_RULES)
    }

    /// **Below THIS file's own cut there is nothing but test-only modules.**
    ///
    /// See the block comment above for what was measured shipping through the
    /// hole this closes.
    #[test]
    fn nothing_but_gated_test_modules_lives_below_this_files_own_cut() {
        let source = include_str!("send_ui.rs");

        // 1. The cut lands at the start of a line, so the gate was matched at
        //    a real attribute and not inside a comment or a string.
        let cut = own_cut_index(source);
        assert!(
            cut > 0 && source.as_bytes()[cut - 1] == b'\n',
            "the cut landed in the MIDDLE of a line, so the gate was matched inside a comment \
             or a string literal rather than at a real attribute"
        );

        // 2. Positive control on WHERE the cut is: the last production item in
        //    this file is still above it, and close to it. Were the cut to
        //    move UP -- into the doc comment that spells the gate, say -- this
        //    anchor would fall below it instead.
        const LAST_PRODUCTION_ITEM: &str =
            concat!("CornerRadius::same(6), ", "theme::BLUE_WASH");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "control: {LAST_PRODUCTION_ITEM:?} is not in this file exactly once, so it pins \
             nothing -- repoint it at the last production item above the first test module"
        );
        let anchor = source.find(LAST_PRODUCTION_ITEM).expect("counted just above");
        assert!(
            anchor < cut,
            "the last production item this control knows about is BELOW the cut, so the cut \
             moved up and the region walked below it is not this file's test half"
        );
        assert!(
            cut - anchor < 4_000,
            "the cut is more than 4000 bytes past the last production item this control knows \
             about: either production was appended below the anchor (repoint the anchor) or \
             the cut moved down"
        );

        // 3. The walk, over an LF copy and a CRLF copy of the same text, which
        //    must agree. Built both ways rather than compared against the bytes
        //    on disk: this repository stores LF blobs and only
        //    `core.autocrlf=true` makes a working tree CRLF, so a control that
        //    asserted "this file is CRLF" would pass here and fail on Linux.
        let lf = source.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_ne!(
            lf, crlf,
            "control: the two copies are the same string, so comparing the walk over them \
             compares it with itself -- this file has no line endings at all"
        );
        let as_lf = walk_below_this_files_own_cut(&lf);
        let as_crlf = walk_below_this_files_own_cut(&crlf);
        assert_eq!(
            as_lf, as_crlf,
            "the walk gives a different answer on an LF copy of this file than on a CRLF one, \
             so something in it is sensitive to line endings"
        );
        let on_disk = walk_below_this_files_own_cut(source);
        assert!(
            on_disk == as_lf || on_disk == as_crlf,
            "this file's line endings are mixed: the walk over it agrees with neither the \
             all-LF nor the all-CRLF copy of its own text"
        );

        // 4. The walk is not vacuous, and it finished.
        let (visited, modules, closes, depth) = on_disk;
        assert!(
            visited > 1_000,
            "control: the walk visited only {visited} lines below this file's cut, which is \
             not this file's worth of test modules -- the slice is empty and this test proves \
             nothing"
        );
        assert_eq!(
            closes, modules,
            "below this file's cut {modules} test modules are opened and {closes} closed"
        );
        assert_eq!(depth, 0, "the walk ended inside a module, at depth {depth}");
        assert_eq!(
            modules, 6,
            "the number of top-level test modules below this file's cut changed. That is fine \
             -- but this count is the control that proves the walk really visited them, so \
             update it deliberately rather than loosening it"
        );

        // The opener count, cross-checked against a SECOND instance of the
        // opener predicate. `column_zero_module_openers` uses
        // `below_cut::is_module_opener`; the walk used this module's own
        // `below_cut_is_module_opener`. Widening either one alone
        // desynchronizes them and fails here, which is the property that
        // sharing a single predicate would have cost.
        assert_eq!(
            modules,
            crate::below_cut::column_zero_module_openers(&source[cut..]),
            "the walk opened {modules} modules but there are {} column-0 gated module openers \
             below this file's cut -- the walk's opener predicate and \
             `below_cut::is_module_opener` no longer agree",
            crate::below_cut::column_zero_module_openers(&source[cut..])
        );

        // 5. Controls on the walk itself. Without these it could be a no-op
        //    that visits lines and asserts nothing.
        //
        //    THE measured survivor, at its measured site: a column-0 `pub fn`
        //    appended at EOF.
        let appended = format!("{source}\npub fn sneaked(x: u64) -> u64 {{ x }}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_this_files_own_cut(&appended)).is_err(),
            "control: the walk accepted a `pub fn` appended below this file's test modules -- \
             the exact payload measured surviving the whole suite green and shipping in the \
             lib's DEBUG LLVM IR"
        );
        // An INDENTED top-level item, which a column-0-only filter would miss.
        // The payload is an indented, GATED module opener and not a `struct`:
        // a struct is refused whether or not indentation is checked, because
        // it is not a module opener either way, so it leaves the indentation
        // rule unmeasured. This shape the opener predicate ACCEPTS, so only
        // the indentation rule can refuse it -- and the trailing column-0 `}`
        // makes it a payload the walk would otherwise take.
        let indented = format!("{source}\n{CUT_GATE}\n    mod sneaked_indented {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_this_files_own_cut(&indented)).is_err(),
            "control: the walk accepted an INDENTED, gated module opener appended below this \
             file's test modules, which a column-0-only filter would miss"
        );
        // An ungated module, which ships.
        let ungated = format!("{source}\nmod shipped {{\n}}\n");
        assert!(
            std::panic::catch_unwind(|| walk_below_this_files_own_cut(&ungated)).is_err(),
            "control: the walk accepted an UNGATED module below this file's cut, which ships"
        );
        // And the one a LINE walk cannot catch: this file's own text with its
        // last test module closed by an INDENTED brace, a `pub fn` at file
        // scope after it, and a column-0 `}` further down to rebalance the
        // count. Perfectly balanced source, no lexer trick -- every payload
        // line is indented, so a line walk's `depth == 1` branch skips all of
        // it and ends with `closes == modules` and `depth == 0`. Only the
        // byte-offset close check in the shared walk kills it.
        let balanced = format!(
            "{}    }}\n    pub fn sneaked(x: u64) -> u64 {{ x }}\n    \
             #[allow(dead_code)]\n    mod filler {{\n}}\n",
            lf.strip_suffix("}\n").expect("this file ends with a column-0 closing brace")
        );
        assert!(
            std::panic::catch_unwind(|| walk_below_this_files_own_cut(&balanced)).is_err(),
            "control: the walk accepted this file's last test module closed by an INDENTED \
             brace with a `pub fn` at file scope after it"
        );
        // Liveness for all four at the identical site: the file as it really
        // is must still be ACCEPTED, or a walk that refuses everything would
        // pass every control above.
        assert!(
            crate::below_cut::try_walk(&lf[own_cut_index(&lf)..], &OWN_BELOW_CUT_RULES).is_ok(),
            "control: the walk refuses this file as it actually is, so the four refusals above \
             measure nothing"
        );
    }

    /// **The frame closure behaves the same way in a test build as in the
    /// shipped one.**
    ///
    /// The one thing a behavioural harness cannot measure about itself.
    /// `frame_promptness::the_loaded_vault_returns_promptly` drives the real
    /// closure and times it, which kills every wait it can REACH -- but a
    /// frame that asked which build it was in could return promptly for the
    /// harness and freeze for the user:
    ///
    /// ```ignore
    /// if !cfg!(test) {
    ///     let t = Instant::now();
    ///     while t.elapsed().as_secs() < 60 { std::hint::spin_loop(); }
    /// }
    /// ```
    ///
    /// **M-N2**, measured green against the harness at 2075 lib + 217 bin,
    /// and green it would stay however long the wall clock were given,
    /// because the test binary cannot execute the branch the user gets.
    ///
    /// This is a needle, and the previous round's needles were deleted this
    /// same commit for being theatre -- so the difference matters. Those
    /// enumerated an OPEN class (the ways to wait, of which there are
    /// unboundedly many). This closes a CLOSED one: the whole vocabulary for
    /// asking about the build configuration from inside an expression is
    /// `cfg!`, and the whole vocabulary for asking about it from an item is
    /// `#[cfg`. Both are here. Neither has a synonym, because the compiler
    /// defines them, and there is nothing else for the frame to key on --
    /// every value it holds is one the harness supplies deliberately.
    ///
    /// **Kept, though it is redundant today, and the redundancy is stated
    /// here rather than discovered again.** The crate-wide
    /// `job_object::tests::nothing_in_this_crate_is_compiled_differently_when_it_is_tested`
    /// already kills M-N2, and kills a hoisted variant
    /// (`fn shipping_build() -> bool { !cfg!(test) }` written ABOVE the
    /// closure) that this test cannot see, because this one only reads the
    /// closure's own body. So it is strictly weaker and buys nothing while
    /// that test stands. It stays for one reason: this is the only guard of
    /// the property that lives in the file the property is about. A crate-wide
    /// scan in another module is exactly the kind of thing that gets narrowed
    /// by an unrelated change, and the failure mode when it is -- a frame that
    /// freezes only for the user -- is the one this whole harness exists for.
    /// Two cheap guards for a sixty-second dead window is the right trade; if
    /// it is ever deleted, delete it for being wrong, not for being second.
    #[test]
    fn the_frame_closure_behaves_the_same_way_in_both_builds() {
        let closure = frame_closure();
        assert!(
            closure.len() > 50_000,
            "control: the frame closure slice is only {} bytes",
            closure.len()
        );
        for asking in [concat!("cfg", "!("), concat!("#[cfg", "(")] {
            assert!(
                !closure.contains(asking),
                "the frame closure contains {asking:?}, so what it does depends on which \
                 build it is. `the_loaded_vault_returns_promptly` drives it in the TEST \
                 build and can only ever measure that half; a frame that waits in the other \
                 half is a frozen window for the user and a green suite for whoever ships \
                 it. The window's behaviour is decided by the values `build_frame` is \
                 handed, never by the configuration it was compiled under"
            );
        }
    }

    /// **A shipping build has exactly one `VaultFrameEnv`, and it is the real
    /// one.**
    ///
    /// The seam `build_frame` grew this round is what lets
    /// `frame_promptness::the_loaded_vault_returns_promptly` drive the real
    /// frame closure without a `bw` spawn, an HTTP call or a read of the real
    /// `settings.json`. The objection a seam like that has to answer is that
    /// it might ALSO be a new way for production to reach a spawn nothing
    /// guards. It is not, and the reason is in the source rather than in this
    /// paragraph: the substitute constructor lives in a module gated to the
    /// test configuration, so it is not compiled into the binary the user
    /// runs, and **every field is private**, so nothing outside
    /// `mod vault_window` can build one any other way. What is left is one
    /// constructor whose body names the same spawn functions the call sites
    /// used to name directly, plus the settings path they used to compute
    /// inline -- and, since a needle list is only ever as long as somebody
    /// remembered to make it, a count of the assignments it really makes
    /// against `vault_window::export_wiring::VAULT_FRAME_ENV_FIELDS`.
    ///
    /// **What this holds is SPELLING, and that is not the whole seam.** Every
    /// needle below is a name. A wrapper written at module level --
    /// `fn export_when_enabled(..) { if ENABLED { export_thread::spawn_export(..) } }`
    /// with `ENABLED` false, handed to the field instead -- still spells
    /// `export_thread::spawn_export` inside the constructor's body region,
    /// still leaves the constructor defining nothing of its own, and still
    /// draws no warning, while the Export row is inert for every user
    /// forever. That mutant was measured green against the whole suite. What
    /// catches it is `vault_window::export_wiring::
    /// production_hands_the_window_the_real_functions`, which compares each
    /// field of the value `production()` really builds against the real
    /// function BY ADDRESS. This test and that one answer different
    /// questions: this one that the constructor is the only one and that it
    /// invents nothing, that one that what it hands over is real.
    #[test]
    fn production_is_the_only_env_a_shipping_build_has() {
        let production = sanitized(&production());
        assert_eq!(
            production.matches(concat!("impl VaultFrame", "Env {")).count(),
            1,
            "`VaultFrameEnv` has more than one impl block in production, so the constructors \
             are not all where this test is looking"
        );
        assert_eq!(
            production.matches(concat!("fn stub", "bed(")).count(),
            0,
            "`frame_env_seam::stubbed` is in the PRODUCTION region -- its gate has been \
             removed or weakened, so the binary the user runs now contains a way to hand \
             this window any spawn at all"
        );
        let opener = concat!("pub fn produc", "tion() -> Self {");
        assert_eq!(
            production.matches(opener).count(),
            1,
            "`VaultFrameEnv::production` is not in production exactly once"
        );
        let at = production.find(opener).expect("counted just above");
        let body = &production[at..];
        let body = &body[..body.find(concat!("\r\n", "    }", "\r\n")).expect("unterminated")];
        // Past the opener, so the constructor's own signature is not
        // counted as a definition made inside it.
        let inside = &body[opener.len()..];
        // **Whole-identifier matching, not substring.** These needles used to
        // be counted with `matches(..).count() == 1`, so an HONEST new field
        // whose spawn is called `spawn_aux_load_2` made the `spawn_aux_load`
        // needle match twice and reddened this test for doing the right
        // thing. The reviewer who hit it renamed the honest function to get
        // around it. A guard that reds on legitimate work gets weakened by
        // whoever hits it next, and two of the holes in this wiring were made
        // exactly that way -- so the guard is fixed instead of the name.
        fn whole_identifier_matches(hay: &str, needle: &str) -> usize {
            let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
            let mut count = 0;
            let mut from = 0;
            while let Some(at) = hay[from..].find(needle) {
                let start = from + at;
                let end = start + needle.len();
                let before = hay[..start].chars().next_back();
                let after = hay[end..].chars().next();
                if before.map_or(true, |c| !ident(c)) && after.map_or(true, |c| !ident(c)) {
                    count += 1;
                }
                from = start + 1;
            }
            count
        }
        // CONTROL: the anchoring really discriminates, both ways. A needle
        // that is a strict prefix of a longer identifier does not match it,
        // and the same needle standing alone does -- otherwise this could be
        // anchored so tightly that it matches nothing and every assertion
        // below passes vacuously.
        assert_eq!(
            whole_identifier_matches("a spawn_aux_load_2(x)", concat!("spawn_aux_", "load")),
            0,
            "control: whole-identifier matching still matches a strict prefix of a longer \
             name, so an honest sibling field reds the needle for the field it is a sibling of"
        );
        assert_eq!(
            whole_identifier_matches("a spawn_aux_load(x)", concat!("spawn_aux_", "load")),
            1,
            "control: whole-identifier matching does not match the identifier itself, so \
             every needle assertion below is vacuous"
        );
        for named in [
            "spawn_vault_sync",
            "spawn_vault_load",
            concat!("send_fetch_thread::spawn_send_", "list"),
            concat!("export_thread::spawn_", "export"),
            concat!("send_delete_thread::spawn_send_", "delete"),
            // The spawner that PUBLISHES. It was absent from this list from
            // the day the field landed, and absent from the address pin too,
            // so a forwarder in its slot -- `if plan.password.is_none() {
            // real(..) }` -- was green across the whole suite with no warning
            // while every password-protected Send silently failed to start.
            concat!("send_create_thread::spawn_send_", "create"),
            concat!("spawn_aux_", "load"),
        ] {
            assert_eq!(
                whole_identifier_matches(body, named),
                1,
                "`VaultFrameEnv::production` does not name {named:?} exactly once: {body}"
            );
        }
        // **DERIVED, not enumerated.** The list above is a list of names, and
        // a list of names cannot notice a name nobody added to it -- which is
        // precisely how `aux_load` and then `send_create` each shipped with
        // nothing pinning them. This counts the field assignments the
        // constructor actually makes and requires the number `VaultFrameEnv`
        // really has, so a ninth field is red HERE even if its author never
        // touches this list.
        //
        // **This is now the WEAKEST of the three walls over that struct, and
        // it is written down as such.** It still reads TEXT, and the previous
        // spelling of this filter required the line to contain `": "` -- so
        // mutation `m14` wrote the ninth assignment in field-init shorthand
        // (`aux_load_2,`, fed by a `let` above the literal) and this counter
        // saw 8, silently. `aux_load_2:fake,` with no space did the same. Both
        // spellings are accepted below, which closes those two, but the
        // lesson is that a text counter is a list of the spellings its author
        // thought of. What actually stops a ninth field now is
        // `vault_window::export_wiring::a_ninth_field_cannot_be_added_to_the_frame_env_without_being_named`,
        // which compares `size_of::<VaultFrameEnv>()` against the fields that
        // module pins: rustc's layout of the real struct, which has no
        // spelling to get wrong. This stays because it answers a question
        // that one does not -- whether the CONSTRUCTOR assigns the fields the
        // struct has -- and because two cheap guards over the field that
        // publishes public links is the right trade.
        let assigned = body
            .lines()
            .filter(|line| {
                let line = line.trim_end();
                if !(line.starts_with("            ") && line.ends_with(',')) {
                    return false;
                }
                let line = line.trim_start();
                if !line.starts_with(|c: char| c.is_ascii_lowercase()) {
                    return false;
                }
                let name_len = line
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(0);
                let rest = &line[name_len..];
                // `field: value,` in ANY spacing -- including none at all --
                // or field-init SHORTHAND, where the whole line is the name
                // and a comma. Each of the last two is a measured survivor of
                // the spelling this filter used to have.
                rest.starts_with(':') || rest == ","
            })
            .count();
        assert_eq!(
            assigned,
            super::super::export_wiring::VAULT_FRAME_ENV_FIELDS,
            "`VaultFrameEnv::production` assigns {assigned} fields but `VaultFrameEnv` has \
             {} of them. Every field of that struct is a way for the frame closure to reach \
             outside this process, so one that appears in the constructor without appearing \
             in the needle list above -- or in the address pin -- is a field a wrapper can \
             occupy with the whole suite green: {body}",
            super::super::export_wiring::VAULT_FRAME_ENV_FIELDS
        );
        assert_eq!(
            inside.matches("fn ").count(),
            0,
            "`VaultFrameEnv::production` defines something of its own, so what it hands the \
             window is no longer just the module-level spawns named above: {body}"
        );
    }
}

/// **The vault frame closure returns promptly -- measured by running it, on
/// every screen the window has.**
///
/// See `the_sends_drain_is_at_the_closures_own_statement_level` for the seven
/// rounds of source scanning this replaces and the five mutants that walked
/// through them. The move is from asking what the closure is SPELLED with to
/// asking what the user experiences: the frame came back, or it did not.
///
/// **How a frame that never returns FAILS instead of hanging the suite.**
/// The whole vault window is built and driven on a worker thread, and the
/// test thread waits on a bounded `recv_timeout`. The `Rc<RefCell<_>>` in
/// the closure never crosses a thread boundary -- `build_frame` is CALLED on
/// the worker, so every cell it makes is born there and dies there; the only
/// things that cross are a `Duration` and a `Vec<String>` of painted labels.
/// A frame that spins, deadlocks, joins or reads stdin leaves that worker
/// thread stuck, the `recv_timeout` expires, and the test panics with the
/// budget it blew. The stuck thread is deliberately LEAKED rather than joined
/// -- joining it is exactly the hang this design exists to avoid -- and the
/// test process reaps it on exit.
///
/// **What was missing until this round, and why it mattered.** The first
/// version of this harness drove ONE scenario: `pre_styled: true` over a
/// loader that never answered. That leaves the window on `VaultBodyState::
/// Loading`, whose arm `return`s about a third of the way down the closure --
/// so roughly two thirds of the frame, including *both screens a user
/// actually looks at*, was never executed by any test and was covered by
/// source scanning alone. Two mutants were measured green against it:
///
///  * a sixty-second spin in the first-frame `if !styled { .. }` block, which
///    no test entered because every test passed `pre_styled: true` while
///    `vault_window::run` and `main.rs` both pass `false`. That block is the
///    window's very first painted frame: a freeze there IS the reported
///    dead-window symptom.
///  * a sixty-second spin in the `Vault | Sends` arm -- the loaded window,
///    where the user spends all of their time.
///
/// Both are killed now, by [`the_first_painted_frame_returns_promptly`] and
/// [`the_loaded_vault_returns_promptly`] respectively.
///
/// **Why none of this is vacuous.** A harness that quietly failed to enter
/// the screen it claims to drive would pass against anything, so every
/// scenario carries a POSITIVE CONTROL that the arm really ran, and each one
/// is a fact about the arm that no other arm produces:
///
///  * every scenario -- the load spawn was reached (`build_frame`'s own body)
///    and the sync spawn was reached (inside the closure, past `styled`, past
///    `draw_resize_handles`, past the repaint schedule).
///  * `Loading` -- the spinner's own label is on screen.
///  * the first painted frame -- the harness does NOT call `theme::apply`, so
///    the ONLY way any later frame can lay out a label in the bundled Archivo
///    faces is that the `!styled` block called it. Additionally the
///    single-frame run must NOT reach the auto-sync, which is what proves the
///    early `return` inside that block was taken.
///  * `Unavailable` -- the error page's heading and *the loader's own reason
///    string*, which this test chose, so a heading painted over some other
///    reason is not accepted.
///  * `Vault` -- the fixture item's name (the item list ran) AND a label only
///    the read detail pane paints (the detail pane ran).
///  * `Sends` -- the Sends fetch was spawned, which only the Sends screen
///    does; the Send the stub answered with is on screen, which is the pane's
///    `Ok` arm (the previous round's disclosed survivor M-N3); and the fixture
///    item's name is ABSENT, which is the item list being replaced rather than
///    merely coexisting.
///
/// **Nothing here leaves the process.** The three spawns are stubbed through
/// [`VaultFrameEnv`](super::super::VaultFrameEnv)'s seam, `server_url` is
/// `None` so no favicon is fetched, `check_breaches` defaults to `false` so
/// no password is ever looked up, the fixture carries no TOTP seed so no code
/// is polled, and the settings path points inside a per-process scratch
/// directory that [`Scratch`] deletes on the way in and on the way out.
#[cfg(test)]
mod frame_promptness {
    use super::super::frame_env_seam::stubbed;
    use super::super::{
        build_frame, AccountDetails, SendListSender, VaultLoadFailure, VaultLoadRequest,
    };
    use crate::login_ui::{BwStatus, BwStatusDetails};
    use crate::vault_bridge::VaultItem;
    use crate::vault_cache::{VaultCache, VaultSnapshot};
    use eframe::egui;
    use std::cell::{Cell, RefCell};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// How long a scenario is allowed to take, end to end, including building
    /// the window and egui's font atlas.
    ///
    /// Generous on purpose. The number that matters is the gap between this
    /// and a frame that WAITS: every mutant this harness was measured against
    /// waits sixty seconds, and the slowest scenario here measures well under
    /// a second on the machine this was written on. A budget in that gap
    /// fails the mutants without turning a slow machine red.
    const BUDGET: Duration = Duration::from_secs(20);

    /// How many frames a scenario drives before it looks at the result. More
    /// than one because the closure's first frame and its later ones are
    /// different code: `styled` and `auto_synced` both flip on the first, the
    /// drains run on all of them, and a load answered during `build_frame` is
    /// not applied until the frame after the one that started it.
    const FRAMES: usize = 3;

    /// The window the frames are laid out in. Wide enough for the sidebar,
    /// the item list and the detail pane to all have room: a viewport too
    /// narrow for a panel makes egui cull it, and a culled pane is an arm
    /// that "ran" and painted nothing -- exactly the vacuity the controls
    /// below exist to catch, arrived at by accident.
    const VIEWPORT: egui::Vec2 = egui::vec2(1280.0, 860.0);

    /// The reason [`load_that_fails`] hands back, and the reason
    /// [`the_unavailable_body_returns_promptly`] then requires to be on
    /// screen. Distinctive so that the error page cannot satisfy it with a
    /// canned string of its own.
    const HARNESS_FAILURE: &str = "the harness refused this load on purpose";

    /// The fixture vault's one login. Its name is the item list's positive
    /// control.
    const LOGIN_NAME: &str = "Harness Login";
    /// Its username. **Not a control**, and the reason the two below exist.
    ///
    /// It was one, on the claim that "the item-list row does not paint it".
    /// The row DOES paint it -- `item_list.rs`'s row subtitle -- and the
    /// harness's own painted dump shows this string twice. Measured: with
    /// `draw_read_arm` returning `DetailAction::None` at its top, so the read
    /// pane paints nothing at all, the old control SURVIVED; with
    /// `draw_item_list` returning at its top, it SURVIVED again. Only killing
    /// both together turned it red, which is the whole of "at least one of
    /// the two panes drew something". Kept as the fixture's username because
    /// the item list still needs a username to put in a row.
    const LOGIN_USERNAME: &str = "harness-detail-username";
    /// **The read detail pane's control.** The heading of its LOGIN
    /// CREDENTIALS card, painted by `detail::draw_detail_read` and by nothing
    /// else in this window -- not the item list, not the sidebar, not the
    /// titlebar. See [`the_loaded_vault_returns_promptly`].
    const DETAIL_PANE_ONLY: &str = "LOGIN CREDENTIALS";
    /// **The item list's control**: its search field's hint, which
    /// `item_list::search_hint` produces and `draw_item_list` is the sole
    /// caller of. The count is the fixture's own two items, so a list that
    /// drew its chrome over an empty vault does not satisfy it either.
    const ITEM_LIST_ONLY: &str = "Search 2 items";
    /// **The `DetailMode::Edit` arm's control**: `detail_edit::form_title`'s
    /// answer for an existing login. `draw_detail_edit` is its only caller,
    /// and the read pane's Edit *button* reads "Edit" alone, so this cannot
    /// be satisfied by the pane the click started from.
    const EDITOR_EDIT_TITLE: &str = "Edit login";
    /// **The `DetailMode::Create` arm's control**, the same title's other
    /// half. `creating` is the only thing that chooses between them, and
    /// `DetailMode::Create` is the only arm that passes `true`.
    const EDITOR_CREATE_TITLE: &str = "New login";
    /// **The item row context menu's control.** `item_list::menu_entries`
    /// produces it and `response.context_menu`'s closure is the only thing
    /// that draws it; no pane on this screen has a "Move to folder" of its
    /// own.
    const ROW_MENU_ONLY: &str = super::super::item_list::MOVE_TO_FOLDER_LABEL;
    /// **The preferences modal's control**: the General section's subtitle,
    /// which `prefs_ui::section_heading` paints and nothing in this window
    /// does. Deliberately NOT `prefs_ui::MODAL_TITLE` ("Preferences"), which
    /// is also the tune button's hover text -- the pointer is resting on that
    /// button when the modal opens, so its tooltip would satisfy a control
    /// that meant to be about the modal.
    const PREFS_MODAL_ONLY: &str =
        "How Deskwarden runs in the background, and when it locks itself.";
    /// The one Send [`counted_send_list`] hands back, and the Sends screen's
    /// second positive control: a row with this name on it can only have been
    /// painted by `draw_send_pane`'s `Ok` arm.
    const SEND_NAME: &str = "Harness Send";

    /// The vault session this harness's window is opened with -- the one
    /// value `build_frame` is handed for it, so a scenario cannot be
    /// measuring one token while asserting about another.
    ///
    /// Long enough to be an unmistakable substring, and distinct from every
    /// other fixture string here, so
    /// [`the_windows_own_session_is_what_reaches_the_bw_child`] cannot be
    /// satisfied by some other value that happens to travel the same path.
    const HARNESS_SESSION: &str = "harness-session-token";

    /// The failure [`counted_sync`] answers with, and so the reason the sync pill
    /// carries when [`SYNC_PILL_LABEL`] is on screen.
    ///
    /// A FAILED sync, deliberately, and this is the whole of the flake fix -- see
    /// [`SYNC_ANSWERS`] for the defect it closes. Nothing this scenario asserts is
    /// about the sync's OUTCOME: it is about the session token that reached both
    /// `(spawn_sync)` call sites, which is recorded by the stub before it answers
    /// anything at all.
    const HARNESS_SYNC_FAILURE: &str = "the harness failed this sync on purpose";

    /// What the toolbar pill reads once [`counted_sync`]'s answer has been drained:
    /// `sync_pill`'s `(Some(Err(_)), _)` arm, whose label is a CONSTANT.
    ///
    /// Not `sync_pill`'s success arm, whose label is `format!("Synced {}", synced_ago_text(..))`
    /// -- a string built out of `last_sync_at.elapsed()`. Nothing about this
    /// harness's `press_sync` scenario has any wall clock in it now, which is the
    /// point.
    const SYNC_PILL_LABEL: &str = "Sync failed";

    // Counted per THREAD, not per process. Each scenario runs on its own
    // freshly spawned worker and every stub below is called synchronously on
    // that worker (the "spawns" are stubs; they spawn nothing), so a
    // thread-local is exactly the right scope: two scenarios running in
    // parallel under `cargo test` cannot see each other's counts, and a
    // scenario reads its own totals inside `drive` before they cross the
    // channel. A process-wide `AtomicUsize` could only ever support `>= 1`.
    thread_local! {
        static SYNC_SPAWNS: Cell<usize> = const { Cell::new(0) };
        static LOAD_SPAWNS: Cell<usize> = const { Cell::new(0) };
        static SEND_LIST_SPAWNS: Cell<usize> = const { Cell::new(0) };
        /// The session each Sends spawn ARRIVED with, in order. Same scope
        /// and same reasoning as the counters above.
        static SEND_LIST_SESSIONS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// The session each `bw sync` spawn ARRIVED with, in order. Same
        /// scope and same reasoning as the counters above, and the reason
        /// it exists is `source_pins::both_sync_call_sites_pass_the_windows_own_session`'s
        /// measured survivor **M-C**.
        static SYNC_SESSIONS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// Whether [`counted_sync`] ANSWERS on the channel it is handed.
        ///
        /// Off for every scenario but the one that presses the Sync pill,
        /// and off by default deliberately: an answered sync clears
        /// `sync_in_progress` and relabels the pill, so switching it on for
        /// everybody would change what every other scenario in this module
        /// measures. The pill refuses its own click while
        /// `sync_in_progress`, so the one scenario that presses it cannot
        /// reach that call site without this.
        ///
        /// **The answer is a FAILURE, and that is what un-flaked this** (it was
        /// `Ok(())`, and `the_windows_own_session_is_what_reaches_the_bw_sync_child`
        /// redded intermittently under a parallel suite for about a day). A
        /// SUCCESSFUL sync makes `run`'s sync drain start a forced vault reload --
        /// and it starts it through `spawn_vault_load` DIRECTLY, not through the
        /// `spawn_load` seam this harness stubs, so that one load is real. It dials
        /// the deliberately dead `http://127.0.0.1:1` bridge this harness builds the
        /// cache on, spends `vault_bridge::CONNECT_TIMEOUT` (3s) getting nowhere, and
        /// reports `VaultLoadFailure::Refresh`; `apply_vault_load_result` then
        /// OVERRIDES `sync_status` from `Some(Ok(()))` to `Some(Err(..))`, because a
        /// "Synced just now" pill over a vault that was never refreshed is a lie it
        /// exists to correct. So the pill turned from "Synced just now" into
        /// "Sync failed" partway through the scenario, and whether that happened
        /// before or after the frame the pill is located on was a pure race between
        /// this module's frame loop and a three-second network timeout on a detached
        /// thread. Idle, the frames cost ~0.8s and won; under `cargo test -j 8` they
        /// cost seconds and lost. Measured on `0fec1e9`: 11 reds in 40 runs of this
        /// module under CPU contention, every one of them
        /// `the toolbar's sync pill painted no "Synced just now" to press ... ["Sync
        /// failed", ..]`; and with a 5s settle forced in before the locate, 5 reds
        /// in 5, which is the same race decided the other way.
        ///
        /// A failed sync takes the `else` of that `if result.is_ok()`, so **no load
        /// is spawned at all** and no thread outlives the frame that made it. The
        /// pill's label is then a constant ([`SYNC_PILL_LABEL`]) rather than one
        /// built out of `last_sync_at.elapsed()`. Neither the count nor the sessions
        /// this scenario asserts is about the sync's outcome -- `counted_sync`
        /// records the token it was handed BEFORE it answers anything -- and the
        /// pill's click gate is `!sync_in_progress`, which a failure clears exactly
        /// as a success does. So the second `(spawn_sync)` call site is still
        /// reached, by the one control that reaches it, and is still asserted on.
        static SYNC_ANSWERS: Cell<bool> = const { Cell::new(false) };
    }

    fn bump(counter: &'static std::thread::LocalKey<Cell<usize>>) {
        counter.with(|c| c.set(c.get() + 1));
    }

    fn read(counter: &'static std::thread::LocalKey<Cell<usize>>) -> usize {
        counter.with(Cell::get)
    }

    /// Stands in for `spawn_vault_sync`, which shells out to `bw sync`.
    ///
    /// It RECORDS the session it was handed, for the same reason
    /// [`counted_send_list`] does: `(spawn_sync)(sync_tx.clone(),
    /// String::new())` at both call sites was measured green on `c92c00c`
    /// at 2101 lib / 217 bin / 0 failed, and every `bw sync` then ran with
    /// `BW_SESSION=""`, which a real vault answers `Locked`. Recorded and
    /// not asserted here: this runs inside the frame, where a panic is an
    /// eframe panic rather than a test failure with a message. See
    /// [`the_windows_own_session_is_what_reaches_the_bw_sync_child`].
    fn counted_sync(tx: mpsc::Sender<Result<(), String>>, session_token: String) {
        bump(&SYNC_SPAWNS);
        SYNC_SESSIONS.with(|s| s.borrow_mut().push(session_token));
        // Synchronous, not spawned, so the answer is on the channel before
        // the drain three hundred lines further down the same frame reads
        // it -- which is what makes the pill clickable on a fixed frame
        // rather than on a race. Off unless the scenario asks, and a
        // FAILURE when it is asked -- see [`SYNC_ANSWERS`] for the race a
        // successful answer started, on a thread that outlived the frame.
        if SYNC_ANSWERS.with(Cell::get) {
            let _ = tx.send(Err(HARNESS_SYNC_FAILURE.to_string()));
        }
    }

    /// Stands in for `send_fetch_thread::spawn_send_list`, which runs a real
    /// `bw send list` on a background thread.
    ///
    /// It **answers**, for the same reason [`load_that_answers`] does: a
    /// silent stub leaves `send_fetch.result` at `None`, `pane_state` on its
    /// "still asking" branch, and the pane's `Ok` arm -- the rows a user
    /// actually reads -- unreachable by any test. That was the previous
    /// round's disclosed survivor **M-N3**, a sleep in exactly that arm.
    ///
    /// Synchronous, not spawned, so the answer is on the channel before the
    /// next frame drains it and the frame count is fixed rather than a race.
    fn counted_send_list(
        _ctx: egui::Context,
        tx: SendListSender,
        generation: u64,
        session: zeroize::Zeroizing<String>,
    ) {
        bump(&SEND_LIST_SPAWNS);
        // Recorded, not asserted here: this stub runs inside the frame, and
        // a panic in there is an eframe panic rather than a test failure with
        // a message. See `the_windows_own_session_is_what_reaches_the_bw_child`.
        SEND_LIST_SESSIONS.with(|s| s.borrow_mut().push(session.to_string()));
        let _ = tx.send((
            generation,
            Ok(vec![crate::send::SendSummary {
                id: "harness-send".to_string(),
                name: SEND_NAME.to_string(),
                access_url: "https://send.example.invalid/harness".to_string(),
                // Far enough out that no clock this test could run under
                // makes it expired, which would be a different row.
                deletion_date: "2999-01-01T00:00:00.000Z".to_string(),
                is_file: false,
            }]),
        ));
    }

    /// Stands in for `spawn_vault_load`, which talks to `bw serve` over HTTP.
    /// Sends nothing back, so the window stays on its loading branch -- which
    /// is the state that reaches the auto-sync and every drain, and reaches
    /// no row, no favicon fetch and no breach lookup.
    fn load_that_never_answers(
        _cache: Arc<VaultCache>,
        _tx: mpsc::Sender<(u64, Result<VaultSnapshot, VaultLoadFailure>)>,
        _request: VaultLoadRequest,
    ) {
        bump(&LOAD_SPAWNS);
    }

    /// A loader that ANSWERS, which is what it takes to get the window off
    /// its spinner and onto the two screens a user looks at.
    ///
    /// Synchronous on the caller's thread -- deliberately. The real one
    /// spawns; this one does not need to, and not spawning means the answer
    /// is on the channel before the first frame runs, so the frame count a
    /// scenario needs is fixed rather than a race. It carries the request's
    /// own `generation`, because `apply_vault_load_result` drops anything
    /// else as superseded and the window would sit on the spinner forever --
    /// which the controls would then catch, but as a confusing failure.
    fn load_that_answers(
        _cache: Arc<VaultCache>,
        tx: mpsc::Sender<(u64, Result<VaultSnapshot, VaultLoadFailure>)>,
        request: VaultLoadRequest,
    ) {
        bump(&LOAD_SPAWNS);
        let _ = tx.send((request.generation, Ok(harness_vault())));
    }

    /// The same, for a load that gave up. `Refresh` and not `Superseded`:
    /// the latter is the vault-session-is-gone path, which tears the window
    /// down rather than drawing the error page this scenario is about.
    fn load_that_fails(
        _cache: Arc<VaultCache>,
        tx: mpsc::Sender<(u64, Result<VaultSnapshot, VaultLoadFailure>)>,
        request: VaultLoadRequest,
    ) {
        bump(&LOAD_SPAWNS);
        let _ = tx.send((
            request.generation,
            Err(VaultLoadFailure::Refresh(HARNESS_FAILURE.to_string())),
        ));
    }

    /// Two items and no folders.
    ///
    /// Built through `serde` rather than by naming fields, so a `VaultItem`
    /// that grows one does not break this fixture -- and so the fixture is
    /// the same shape `bw serve` actually sends. No `totp`, so no code is
    /// polled; no `uris`, so nothing has a host a favicon could be fetched
    /// for even if `server_url` were set.
    fn harness_vault() -> VaultSnapshot {
        let item = |value: serde_json::Value| -> VaultItem {
            serde_json::from_value(value).expect("the harness fixture is not a `VaultItem`")
        };
        VaultSnapshot {
            items: vec![
                item(serde_json::json!({
                    "id": "harness-login",
                    "name": LOGIN_NAME,
                    "type": 1,
                    "login": { "username": LOGIN_USERNAME, "password": "harness-password" },
                })),
                item(serde_json::json!({
                    "id": "harness-note",
                    "name": "Harness Note",
                    "type": 2,
                    "notes": "nothing secret lives in a test fixture",
                })),
            ],
            folders: Vec::new(),
        }
    }

    /// A per-process scratch directory, empty on entry and **removed on the
    /// way out**.
    ///
    /// The removal is the point. `FillStats::new` and the icons path both
    /// create this directory, and the first version of this harness never
    /// deleted it -- one abandoned `%TEMP%\deskwarden-frame-harness-<pid>`
    /// per test process, forever. It is created and destroyed by the TEST
    /// thread, around the worker, so a worker left stuck by a mutant does not
    /// leave the directory behind either.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("deskwarden-frame-harness-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One end-to-end run of the real window: what it is handed, and how many
    /// frames it is driven for.
    #[derive(Clone, Copy)]
    struct Scenario {
        /// What `build_frame`'s `pre_styled` is handed. `false` is what
        /// production passes, and it is the *only* way into the `!styled`
        /// block -- see [`the_first_painted_frame_returns_promptly`].
        pre_styled: bool,
        /// The stub `VaultFrameEnv::load` is built from.
        load: fn(
            Arc<VaultCache>,
            mpsc::Sender<(u64, Result<VaultSnapshot, VaultLoadFailure>)>,
            VaultLoadRequest,
        ),
        /// How many frames to drive before reading the result.
        frames: usize,
        /// Whether to press the sidebar's **Sends** row partway through. The
        /// only way into `VaultBodyState::Sends`: `sends_selected` starts
        /// `false` and `draw_sidebar` is the sole thing that writes it, so
        /// this arm cannot be reached except by clicking.
        press_sends: bool,
        /// Whether to press the read detail pane's **Edit** button once the
        /// vault is up. The only producer of `DetailAction::Edit`, which is
        /// the only thing that writes `DetailMode::Edit` -- so, like the
        /// Sends row, there is no back door and this drives the sequence of
        /// frames a user's click produces.
        press_edit: bool,
        /// Whether to press the titlebar's **Preferences** control (the tune
        /// mark), the only writer of the `prefs` cell and so the only way
        /// into the modal block at the very end of the closure.
        press_prefs: bool,
        /// Whether to send **Ctrl+N**, the only other producer of
        /// `DetailMode::Create` besides the sidebar's own new-item menu, and
        /// the one that needs no control located on screen first.
        press_ctrl_n: bool,
        /// Whether to press the toolbar's **Sync** status pill, the second
        /// of the two `VaultFrameEnv::sync` call sites -- the first being
        /// the auto-sync every scenario already runs on its first real
        /// frame. Turning this on also turns [`SYNC_ANSWERS`] on, because
        /// `theme::status_pill_button(..).clicked() && !sync_in_progress`
        /// is the production gate and an unanswered auto-sync never lets
        /// go of `sync_in_progress`. What that answer SAYS, and why it is a
        /// failure rather than a success, is [`SYNC_ANSWERS`]'s own doc.
        press_sync: bool,
        /// Whether to RIGHT-click the fixture login's row, which is the only
        /// way into `item_list.rs`'s `response.context_menu` closure --
        /// measured, a sixty-second spin in it survived the whole previous
        /// harness.
        press_row_menu: bool,
    }

    impl Scenario {
        /// The shape every scenario starts from: production's `pre_styled`,
        /// a loader that answers, [`FRAMES`] frames, no click.
        fn new() -> Self {
            Self {
                pre_styled: false,
                load: load_that_answers,
                frames: FRAMES,
                press_sends: false,
                press_edit: false,
                press_prefs: false,
                press_ctrl_n: false,
                press_sync: false,
                press_row_menu: false,
            }
        }
    }

    /// What crosses back from the worker. Every field is `Send`; not one
    /// `Rc` or `egui::Context` is in here, which is what keeps the closure's
    /// cells on the thread that made them.
    struct Outcome {
        elapsed: Duration,
        /// Every label painted on the LAST frame driven.
        painted: Vec<String>,
        sync_spawns: usize,
        load_spawns: usize,
        send_list_spawns: usize,
        /// The session each Sends spawn was handed, in order.
        send_list_sessions: Vec<String>,
        /// The session each `bw sync` spawn was handed, in order.
        sync_sessions: Vec<String>,
    }

    impl Outcome {
        fn painted(&self, needle: &str) -> bool {
            self.painted.iter().any(|label| label.contains(needle))
        }

        /// Asserts `needle` is on screen, printing what WAS on screen if it
        /// is not -- a positive control that fails silently is worth nothing.
        #[track_caller]
        fn expect_painted(&self, needle: &str, why: &str) {
            assert!(
                self.painted(needle),
                "{why}: nothing on the last frame painted {needle:?}. What was painted: {:?}",
                self.painted
            );
        }
    }

    /// The two ways [`within`] fails, kept apart.
    ///
    /// They used to be one `Err(())`, and every failure therefore read "did
    /// not come back within 20s -- either one frame is WAITING ... or the
    /// frame panicked". A panic *arrived* as a timeout because the only
    /// signal was the sender being dropped, so an epaint panic in the frame
    /// burned the whole budget and then reported the wrong diagnosis first.
    /// The panic is caught on the worker now, so the report is immediate and
    /// says which of the two happened.
    #[derive(Debug)]
    enum Halt {
        /// The budget ran out. The worker is still running; see this module's
        /// doc for why it is left that way.
        Timeout,
        /// The body unwound, and this is what it said.
        Panicked(String),
    }

    /// What a caught panic payload actually said, for the two payload types
    /// `panic!` produces. Anything else is a type this harness cannot name,
    /// and saying so beats printing nothing.
    fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
        if let Some(message) = payload.downcast_ref::<&'static str>() {
            (*message).to_string()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            "a panic payload that is neither `&str` nor `String`".to_string()
        }
    }

    /// Runs `work` on its own thread and gives it `budget` to answer.
    ///
    /// `Err(Halt::Timeout)` is "it did not answer in time", and the thread is
    /// left running -- see this module's doc. `Err(Halt::Panicked(_))` is the
    /// body unwinding, caught on the worker so it is reported the moment it
    /// happens rather than at the end of the budget.
    fn within<T: Send + 'static>(
        budget: Duration,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, Halt> {
        let (tx, rx) = mpsc::channel::<Result<T, String>>();
        std::thread::Builder::new()
            .name("vault-frame-harness".to_string())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                // `AssertUnwindSafe` because `work` is a closure this module
                // wrote, run on a thread that is thrown away either way:
                // nothing it could leave half-written outlives the catch.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                    .map_err(panic_message);
                let _ = tx.send(outcome);
            })
            .expect("could not start the harness thread");
        match rx.recv_timeout(budget) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(message)) => Err(Halt::Panicked(message)),
            // The sender dropped without sending is only reachable if the
            // catch itself failed to run; reported as the timeout it is
            // indistinguishable from.
            Err(_) => Err(Halt::Timeout),
        }
    }

    fn collect_labelled_rects(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                out.push((text.galley.text().to_string(), text.visual_bounding_rect()))
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_labelled_rects(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Everything painted, as one shape, which is what `theme::icon_probe`'s
    /// finders take: the two controls this harness has to press paint no text
    /// and cannot be found any other way.
    fn all_shapes(output: &egui::FullOutput) -> egui::Shape {
        egui::Shape::Vec(output.shapes.iter().map(|clipped| clipped.shape.clone()).collect())
    }

    /// The centre of the label reading exactly `needle` on `output`, or a
    /// failure naming everything that WAS painted.
    ///
    /// Exact equality, not `contains`: "Edit" as a substring matches the
    /// editor's own "Edit login" heading, and a scenario that clicked the
    /// heading it was supposed to produce would be a click on nothing dressed
    /// up as a driven arm.
    #[track_caller]
    fn locate_label(output: &egui::FullOutput, needle: &str, owner: &str) -> egui::Pos2 {
        let labels = labels_of(output);
        labels
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, rect)| rect.center())
            .unwrap_or_else(|| {
                panic!(
                    "{owner} painted no {needle:?} to press, so this scenario cannot reach \
                     what it is about at all. What was painted: {:?}",
                    labels.iter().map(|(t, _)| t).collect::<Vec<_>>()
                )
            })
    }

    fn labels_of(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        let mut out = Vec::new();
        for clipped in &output.shapes {
            collect_labelled_rects(&clipped.shape, &mut out);
        }
        out
    }

    /// Builds the real vault window and drives `scenario` through a headless
    /// `egui::Context`, reporting how long that took and what ended up on
    /// screen.
    ///
    /// Everything is built HERE, on whatever thread this runs on, because the
    /// frame closure is full of `Rc` and cannot be moved between threads.
    /// `VaultFrameHandles::finish` is deliberately NOT called: it writes the
    /// window geometry to `settings.json`, and this test owns no real one.
    ///
    /// **`theme::apply` is never called by the harness.** The closure's own
    /// `!styled` block calls it, and letting that be the only call is what
    /// turns "the first painted frame ran" into something a test can
    /// observe rather than assume: without it, the bundled Archivo families
    /// do not exist and the first label this window lays out panics inside
    /// epaint. Scenarios that hand `pre_styled: true` therefore run one
    /// throwaway frame against an empty `Ui` first, exactly as `sidebar.rs`
    /// and `detail.rs` do, and call it themselves.
    fn drive(scenario: Scenario, scratch: &std::path::Path) -> Outcome {
        // Before the first frame, because the auto-sync fires inside it.
        SYNC_ANSWERS.with(|c| c.set(scenario.press_sync));
        let (_options, mut frame_fn, handles) = build_frame(
            // A base URL nothing listens on. It is never dialled anyway --
            // the only thing that would dial it is the load spawn, and that
            // is one of the stubs above.
            Arc::new(VaultCache::new(crate::vault_bridge::VaultBridge::new(
                "http://127.0.0.1:1",
            ))),
            crate::fill_stats::FillStats::new(scratch.join("fill-stats.json")),
            // `Ready`, so no `bw status` channel and no drain waiting on one.
            AccountDetails::Ready(BwStatusDetails {
                status: BwStatus::Unlocked,
                user_email: Some("harness@example.invalid".to_string()),
                // `None`, so no favicon is fetched for any host.
                server_url: None,
            }),
            HARNESS_SESSION.to_string(),
            scratch.join("icons"),
            // `Never`, so the auto-lock countdown cannot end the session
            // underneath the measurement.
            crate::settings::AutoLock::Never,
            // The load spawn is stubbed, so this only says the readiness wait
            // would have been skipped.
            true,
            None,
            scenario.pre_styled,
            stubbed(
                counted_sync,
                scenario.load,
                counted_send_list,
                // Under the scratch directory, so no frame reads or writes
                // the real `%APPDATA%\Deskwarden`. Absent on disk, so
                // `Settings::load` returns the default -- whose
                // `check_breaches` is `false`, which is why a fixture with a
                // real password looks nothing up.
                Some(scratch.join("settings.json")),
            ),
        );
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, VIEWPORT)),
            ..Default::default()
        };
        if scenario.pre_styled {
            // Somebody else owns this window's first frame, so somebody else
            // called `theme::apply` -- three stages before handing the frame
            // over. Modelled the way the other painted-output tests in this
            // crate model it: a throwaway frame, then the call, then another
            // throwaway so the families exist from the next frame on.
            let _ = ctx.run_ui(input(), |_ui| {});
            crate::theme::apply(&ctx);
            let _ = ctx.run_ui(input(), |_ui| {});
        }
        let started = Instant::now();
        let mut output = ctx.run_ui(input(), |ui| frame_fn(ui));
        for _ in 1..scenario.frames {
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
        }
        // A press AND a release is what egui counts as a click, and the frame
        // that locates a control cannot be the frame that clicks it.
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let click = |ctx: &egui::Context,
                     frame_fn: &mut dyn FnMut(&mut egui::Ui),
                     pos: egui::Pos2| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::PointerMoved(pos), button(pos, true)],
                    ..input()
                },
                |ui| frame_fn(ui),
            );
            let _ = ctx.run_ui(
                egui::RawInput { events: vec![button(pos, false)], ..input() },
                |ui| frame_fn(ui),
            );
        };
        if scenario.press_sync {
            // The pill is the only control that reaches the SECOND
            // `(spawn_sync)` call site. It is found by the words it paints
            // once the auto-sync has been answered and drained --
            // `sync_pill`'s `(Some(Err(_)), _)` arm, which is the arm
            // [`SYNC_ANSWERS`] puts the window in and whose label is a
            // constant. This line USED to look for the success arm's
            // "Synced just now", and that arm's label is not stable for the
            // life of the scenario: see [`SYNC_ANSWERS`].
            let pos = locate_label(&output, SYNC_PILL_LABEL, "the toolbar's sync pill");
            click(&ctx, &mut *frame_fn, pos);
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
        }
        if scenario.press_sends {
            let pos = locate_label(&output, super::super::sidebar::SENDS_ROW_LABEL, "the sidebar");
            click(&ctx, &mut *frame_fn, pos);
            // Two more settled frames, so what is read below is the Sends
            // screen at rest rather than the frame the click landed on: the
            // fetch is started by the frame that first draws the screen, and
            // its answer is not drained until the frame after that.
            let _ = ctx.run_ui(input(), |ui| frame_fn(ui));
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
        }
        if scenario.press_edit {
            // Edit lives inside the read pane's OVERFLOW MENU, at the user's
            // direction -- so reaching it is two clicks, exactly as it is for
            // a user. The kebab paints no text either; `kebab_dots` finds it
            // the same way `tune_icons` finds the tune mark, and
            // `theme::kebab_button` has exactly one production call site (the
            // one being pressed here), so "the first mark found" is not a
            // guess.
            let dots = crate::theme::icon_probe::kebab_dots(&all_shapes(&output));
            let (rect, _) = *dots.first().unwrap_or_else(|| {
                panic!(
                    "the read pane painted no overflow menu to open, so this scenario cannot \
                     reach the editor. What was painted: {:?}",
                    labels_of(&output).into_iter().map(|(t, _)| t).collect::<Vec<_>>()
                )
            });
            click(&ctx, &mut *frame_fn, rect.center());
            // The menu is open from the frame after the release; this is the
            // frame that paints its entries, and so the one "Edit" is found
            // on.
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
            let pos = locate_label(&output, "Edit", "the read pane's overflow menu");
            click(&ctx, &mut *frame_fn, pos);
            // The click is applied by the frame after the release, and the
            // editor is drawn by the frame after that.
            let _ = ctx.run_ui(input(), |ui| frame_fn(ui));
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
        }
        if scenario.press_ctrl_n {
            let key = |pressed| egui::Event::Key {
                key: egui::Key::N,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            };
            // `RawInput::modifiers` as well as the event's: the closure asks
            // `i.modifiers.ctrl && i.key_pressed(N)`, and `modifiers` is the
            // frame's own held-key state rather than anything derived from
            // the event list. Setting only the event's leaves Ctrl+N reading
            // as a bare N, which is nothing at all.
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: vec![key(true)],
                    modifiers: egui::Modifiers::CTRL,
                    ..input()
                },
                |ui| frame_fn(ui),
            );
            let _ = ctx.run_ui(
                egui::RawInput { events: vec![key(false)], ..input() },
                |ui| frame_fn(ui),
            );
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
        }
        if scenario.press_row_menu {
            let pos = locate_label(&output, LOGIN_NAME, "the item list");
            let secondary = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Default::default(),
            };
            let _ = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::PointerMoved(pos), secondary(true)],
                    ..input()
                },
                |ui| frame_fn(ui),
            );
            let _ = ctx.run_ui(
                egui::RawInput { events: vec![secondary(false)], ..input() },
                |ui| frame_fn(ui),
            );
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
        }
        if scenario.press_prefs {
            // The tune mark paints no text, so it cannot be found the way
            // every other control here is. `theme::icon_probe::tune_icons` is
            // the crate's existing answer to "where is that mark" -- it finds
            // the knobs by the radius that draws them, which is the same two
            // numbers `tune_button` uses, so a retuned mark moves this
            // scenario with it instead of stranding it.
            let marks = crate::theme::icon_probe::tune_icons(&all_shapes(&output));
            let (rect, _) = *marks.first().unwrap_or_else(|| {
                panic!(
                    "the titlebar painted no tune mark to press, so this scenario cannot open \
                     the preferences modal at all. What was painted: {:?}",
                    labels_of(&output).into_iter().map(|(t, _)| t).collect::<Vec<_>>()
                )
            });
            click(&ctx, &mut *frame_fn, rect.center());
            let _ = ctx.run_ui(input(), |ui| frame_fn(ui));
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
        }
        let elapsed = started.elapsed();
        let outcome = Outcome {
            elapsed,
            painted: labels_of(&output).into_iter().map(|(text, _)| text).collect(),
            sync_spawns: read(&SYNC_SPAWNS),
            load_spawns: read(&LOAD_SPAWNS),
            send_list_spawns: read(&SEND_LIST_SPAWNS),
            send_list_sessions: SEND_LIST_SESSIONS.with(|s| s.borrow().clone()),
            sync_sessions: SYNC_SESSIONS.with(|s| s.borrow().clone()),
        };
        drop(frame_fn);
        drop(handles);
        outcome
    }

    /// Runs `scenario` under [`BUDGET`] and reports a timeout as the failure
    /// this whole design exists to produce instead of a hang.
    ///
    /// `tag` names the scratch directory so two scenarios running in parallel
    /// cannot delete each other's, and appears in the failure message.
    fn measured(tag: &'static str, scenario: Scenario) -> Outcome {
        let scratch = Scratch::new(tag);
        let path = scratch.0.clone();
        let outcome = within(BUDGET, move || drive(scenario, &path)).unwrap_or_else(|halt| match halt
        {
            Halt::Panicked(message) => panic!(
                "the {tag} scenario's {} frames of the vault window PANICKED: {message}. Not a \
                 timeout -- this is reported the moment it happens rather than after \
                 {BUDGET:?}, which is what it used to cost to find out",
                scenario.frames
            ),
            Halt::Timeout => panic!(
                "the {tag} scenario's {} frames of the vault window did not come back within \
                 {BUDGET:?}. One frame is WAITING -- on a channel, a lock, a thread, a \
                 process, a file handle, a busy spin, anything at all; the user cannot tell \
                 those apart and neither does this test. (It is NOT a panic: a panic is \
                 caught and reported as one.) This is the whole property: the eframe thread \
                 is the only thread this window has, and a frame that does not return is a \
                 frozen window, no repaint and no input, for as long as it takes",
                scenario.frames
            ),
        });
        assert!(
            outcome.elapsed < BUDGET,
            "control: the {tag} scenario reported {:?}, which is not inside the budget it was \
             admitted under",
            outcome.elapsed
        );
        assert!(
            outcome.load_spawns >= 1,
            "the {tag} scenario built the window without reaching the initial vault load, so \
             it is not measuring `build_frame`'s body at all"
        );
        outcome
    }

    /// The control every scenario that means to run a whole frame shares: the
    /// auto-sync sits inside the closure past the `styled` guard, past
    /// `draw_resize_handles` and past the repaint schedule, so reaching it is
    /// proof the frame body was entered rather than returned out of.
    #[track_caller]
    fn assert_the_body_was_entered(tag: &str, outcome: &Outcome) {
        assert!(
            outcome.sync_spawns >= 1,
            "the {tag} scenario ran its frames without reaching the auto-sync, which sits \
             inside the closure past the `styled` guard, `draw_resize_handles` and the \
             repaint schedule. Whatever those frames measured, it was not the closure's body \
             -- this test would be green against a closure that waited for a minute further \
             down"
        );
    }

    /// **The loading screen returns promptly.** The original scenario, kept
    /// byte-for-byte in behaviour: a loader that never answers leaves the
    /// window on `VaultBodyState::Loading`, which is the state that reaches
    /// the auto-sync and every drain.
    ///
    /// Positive control: the spinner's own label is on screen, so the frame
    /// really did take the `Loading` arm and not one of the other three.
    #[test]
    fn the_loading_screen_returns_promptly() {
        let outcome = measured(
            "loading",
            Scenario { pre_styled: true, load: load_that_never_answers, ..Scenario::new() },
        );
        assert_the_body_was_entered("loading", &outcome);
        outcome.expect_painted(
            "Loading your vault",
            "the loading scenario did not reach the `VaultBodyState::Loading` arm",
        );
    }

    /// **The window's FIRST PAINTED FRAME returns promptly** -- the one
    /// production actually gets, and the one no test entered until now.
    ///
    /// `vault_window::run` and `main.rs`'s `RealVaultOps` path both pass
    /// `pre_styled: false`, so on a fresh window the very first frame runs
    /// the `if !styled { .. }` block: it paints the background, applies the
    /// theme, rounds the window's corners, raises it, and returns. **M-X1**,
    /// a sixty-second spin inserted into that block, was measured green
    /// against the previous harness at 2076 lib + 217 bin, because every
    /// scenario there handed `pre_styled: true` and the block was dead code
    /// to the whole suite. A freeze there is the reported dead-window
    /// symptom exactly: the window appears and never draws anything else.
    ///
    /// **Two positive controls, and they point in opposite directions.**
    ///
    ///  * ONE frame must NOT reach the auto-sync. The `!styled` block ends in
    ///    a `return`, so a single frame that got past it would mean the block
    ///    was skipped and this test is not driving what it says it is.
    ///  * FOUR frames must reach it, and must paint the vault. The harness
    ///    never calls `theme::apply` on this path, so the only call is the
    ///    one inside the block -- and without it, the first label this window
    ///    lays out panics inside epaint. A green run is therefore a statement
    ///    that the block's body executed, not merely that the branch was
    ///    taken.
    #[test]
    fn the_first_painted_frame_returns_promptly() {
        let styling = measured("first-frame", Scenario { frames: 1, ..Scenario::new() });
        assert_eq!(
            styling.sync_spawns, 0,
            "the window's first frame with `pre_styled: false` reached the auto-sync, so it \
             did not take the `if !styled` branch and return -- this test is no longer \
             driving the first painted frame, and M-X1 has nothing looking at it"
        );

        let settled = measured("first-frame-settled", Scenario { frames: 4, ..Scenario::new() });
        assert_the_body_was_entered("first-frame-settled", &settled);
        settled.expect_painted(
            LOGIN_NAME,
            "the frames after the styling frame painted no vault, so the `!styled` block \
             either never ran or never handed over",
        );
    }

    /// **The loaded vault window returns promptly** -- item list AND detail
    /// pane, the screen the user spends all their time on.
    ///
    /// This is the `VaultBodyState::Vault | VaultBodyState::Sends => {}` arm
    /// and everything below it, which is the majority of the closure by line
    /// count and was unreachable while the harness's only loader stayed
    /// silent. **M-X2**, a sixty-second spin in that arm, was measured green
    /// against the previous harness at 2076 lib + 217 bin.
    ///
    /// Positive controls, one per pane, because "the arm ran" and "both panes
    /// drew" are different claims and only the second is worth having:
    ///
    ///  * [`ITEM_LIST_ONLY`], the search field's hint -- `draw_item_list` ran.
    ///  * [`DETAIL_PANE_ONLY`], the LOGIN CREDENTIALS card's heading -- the
    ///    read detail pane ran.
    ///
    /// **Both were vacuous before.** They were the fixture's name and its
    /// USERNAME, on the claim that the list row does not paint the username.
    /// It does, as its subtitle. Measured: an early return at the top of
    /// `draw_read_arm` SURVIVED, an early return at the top of
    /// `draw_item_list` SURVIVED, and only both together were caught -- so
    /// this test asserted nothing more than "at least one of the two panes
    /// drew the fixture item". The two strings above are each produced by
    /// exactly one of the panes, and each mutant now fails ALONE.
    #[test]
    fn the_loaded_vault_returns_promptly() {
        let outcome = measured("vault", Scenario::new());
        assert_the_body_was_entered("vault", &outcome);
        outcome.expect_painted(
            ITEM_LIST_ONLY,
            "the loaded window painted no search hint, so `draw_item_list` never drew -- and \
             the hint is the one thing on that half of the screen the detail pane cannot \
             produce",
        );
        outcome.expect_painted(
            DETAIL_PANE_ONLY,
            "the loaded window painted no LOGIN CREDENTIALS card, so the READ DETAIL PANE \
             never drew -- the list alone is half this screen, and a frame that waits in the \
             pane beside it is the same frozen window",
        );
    }

    /// **The item EDITOR returns promptly** -- `DetailMode::Edit`, reached
    /// the only way a user can reach it: by pressing the read pane's Edit
    /// button.
    ///
    /// This arm and the one below sit in `mod.rs` 2716..3241, the largest
    /// blind region the coverage instrumentation found: 525 consecutive
    /// lines, 22% of the closure, not one statement of which any scenario
    /// executed. Measured against the previous harness, a sixty-second spin
    /// at the top of each of the two editor arms SURVIVED -- 7 passed in
    /// 0.45s.
    ///
    /// Two positive controls:
    ///
    ///  * [`EDITOR_EDIT_TITLE`] is on screen. Only `draw_detail_edit` with
    ///    `creating: false` paints it, and the button that was pressed reads
    ///    "Edit" alone, so the click really did change the mode.
    ///  * [`DETAIL_PANE_ONLY`] is NOT. The editor REPLACES the read pane; a
    ///    window showing both would mean the press landed somewhere else and
    ///    this test measured the read arm over again.
    #[test]
    fn the_item_editor_returns_promptly() {
        let outcome = measured("edit", Scenario { press_edit: true, ..Scenario::new() });
        assert_the_body_was_entered("edit", &outcome);
        outcome.expect_painted(
            EDITOR_EDIT_TITLE,
            "the Edit button was pressed and no edit form is on screen, so `DetailMode::Edit` \
             was never entered and this test is measuring the read pane again",
        );
        assert!(
            !outcome.painted(DETAIL_PANE_ONLY),
            "the edit form is up and the read pane's {DETAIL_PANE_ONLY:?} card is still \
             painted, so the pane was not replaced -- the click did not take. What was \
             painted: {:?}",
            outcome.painted
        );
    }

    /// **The new-item form returns promptly** -- `DetailMode::Create`,
    /// reached by Ctrl+N, which `mod.rs`'s keyboard block is the only
    /// producer of besides the sidebar's own menu.
    ///
    /// Controls mirror the editor's, in the other direction:
    ///
    ///  * [`EDITOR_CREATE_TITLE`] is on screen -- `draw_detail_edit` with
    ///    `creating: true`, which only the `Create` arm passes.
    ///  * [`EDITOR_EDIT_TITLE`] is NOT, which is what separates this arm from
    ///    the one above. The two differ by a single `bool` and every other
    ///    label on the form is shared, so without this the two scenarios
    ///    would be the same test written twice.
    #[test]
    fn the_new_item_form_returns_promptly() {
        let outcome = measured("create", Scenario { press_ctrl_n: true, ..Scenario::new() });
        assert_the_body_was_entered("create", &outcome);
        outcome.expect_painted(
            EDITOR_CREATE_TITLE,
            "Ctrl+N was pressed and no new-item form is on screen, so `DetailMode::Create` \
             was never entered",
        );
        assert!(
            !outcome.painted(EDITOR_EDIT_TITLE),
            "the form on screen is the EDIT form, not the create form -- this scenario is \
             measuring the arm the test above already measures. What was painted: {:?}",
            outcome.painted
        );
    }

    /// **The item row's context menu returns promptly.**
    ///
    /// `item_list.rs`'s `response.context_menu` closure, which builds and
    /// draws every per-row command. Nothing right-clicked a row until now, so
    /// a sixty-second spin at the top of that closure was measured green
    /// against the whole previous suite -- and a menu that never comes back
    /// freezes the window under it, because it is drawn by the same frame.
    ///
    /// Positive control: [`ROW_MENU_ONLY`], an entry that exists nowhere but
    /// in that menu. The row itself, the detail pane and the sidebar all stay
    /// on screen while a context menu is up, so nothing weaker would
    /// distinguish "the menu opened" from "the right-click did nothing".
    #[test]
    fn the_item_row_menu_returns_promptly() {
        let outcome = measured("row-menu", Scenario { press_row_menu: true, ..Scenario::new() });
        assert_the_body_was_entered("row-menu", &outcome);
        outcome.expect_painted(
            ROW_MENU_ONLY,
            "the item row was right-clicked and no context menu is on screen, so \
             `response.context_menu`'s closure never ran",
        );
    }

    /// **The preferences modal returns promptly.**
    ///
    /// The block at the very end of the closure, past every `return` in the
    /// body match. It is drawn over this window rather than in a window of
    /// its own, so a frame that waits inside `draw_prefs_modal` freezes the
    /// vault window behind it too -- and a sixty-second spin there was
    /// measured green against the previous harness.
    ///
    /// Reached by pressing the titlebar's tune mark, the only writer of the
    /// `prefs` cell. Two positive controls:
    ///
    ///  * [`PREFS_MODAL_ONLY`] is on screen -- the General section's
    ///    subtitle, which only `prefs_ui` paints.
    ///  * the vault behind it is STILL painted. The modal is an overlay, not
    ///    a screen: if the item list vanished, the click did something other
    ///    than open the modal and this scenario is not measuring an overlay
    ///    at all.
    #[test]
    fn the_preferences_modal_returns_promptly() {
        let outcome = measured("prefs", Scenario { press_prefs: true, ..Scenario::new() });
        assert_the_body_was_entered("prefs", &outcome);
        outcome.expect_painted(
            PREFS_MODAL_ONLY,
            "the tune mark was pressed and the preferences modal is not on screen, so the \
             block at the end of the closure never ran",
        );
        outcome.expect_painted(
            ITEM_LIST_ONLY,
            "the preferences modal is up and the vault window behind it is gone, so this is \
             not the overlay this test is about",
        );
    }

    /// **The Sends screen returns promptly.**
    ///
    /// Reached the only way it can be: by pressing the sidebar's Sends row.
    /// `sends_selected` starts `false` and `draw_sidebar` is the sole writer,
    /// so there is no back door and this scenario drives the same sequence of
    /// frames a user's click produces.
    ///
    /// Two positive controls:
    ///
    ///  * the Sends fetch was spawned. Only `send_fetch.wants_fetch(
    ///    show_sends)` does that, and `show_sends` is `matches!(body,
    ///    VaultBodyState::Sends)` -- so a spawn is the body state itself,
    ///    observed rather than assumed.
    ///  * the fixture item's name is NOT on screen. The Sends screen REPLACES
    ///    the item list and the detail pane; a window showing both would mean
    ///    the click selected nothing and this test measured the Vault arm
    ///    over again.
    #[test]
    fn the_sends_screen_returns_promptly() {
        let outcome = measured("sends", Scenario { press_sends: true, ..Scenario::new() });
        assert_the_body_was_entered("sends", &outcome);
        assert!(
            outcome.send_list_spawns >= 1,
            "the Sends row was pressed and no Sends fetch was started, so the window is not \
             on `VaultBodyState::Sends` and this test is measuring some other screen. What \
             was painted: {:?}",
            outcome.painted
        );
        outcome.expect_painted(
            SEND_NAME,
            "the Sends screen is up and the fetch answered, but the Send it answered with is              not on screen -- `draw_send_pane`'s `Ok` arm, which is the rows a user reads,              was not executed",
        );
        assert!(
            !outcome.painted(LOGIN_NAME),
            "the Sends screen is up and the vault item {LOGIN_NAME:?} is still painted, so \
             the item list was not replaced -- the click did not take. What was painted: \
             {:?}",
            outcome.painted
        );
    }

    /// **The window's own session is what a real `bw send list` runs with.**
    ///
    /// This replaces `tests::the_list_invocation_still_carries_no_session_token`,
    /// which asserted the opposite -- that the list invocation carried no
    /// session at all -- and which is deleted this commit. That test drove a
    /// FAKE runner the test itself constructed, so under the design this
    /// commit lands its assertion can never fire again whatever production
    /// does: it documented a gap that no longer exists.
    ///
    /// The chain has two links and both are measured here.
    ///
    ///  1. **Window -> spawn.** A real frame is driven to the Sends screen
    ///     the only way there is (the sidebar row), and the stub records the
    ///     token that ARRIVED at the `send_list` pointer. Not the token this
    ///     harness passed in -- the one the frame closure chose to hand over
    ///     -- so a session dropped, defaulted, or read from anywhere but
    ///     `build_frame`'s own `session_token` fails right here.
    ///  2. **Spawn -> child.** That very value, and not a constant written
    ///     out a second time, is then given to the runner `real_send_list`
    ///     builds and driven through the real `list_sends` and the real
    ///     `spawn_in_job` with the spawn probe armed. What is read back is
    ///     the environment overlay that reached the spawn, and every element
    ///     of argv: a process's argument vector is readable by every other
    ///     process on the machine, so the token that unlocks the whole vault
    ///     must appear in none of it.
    ///
    /// **No child is started.** The probe refuses the spawn and `list_sends`
    /// maps the refusal through the ordinary failure path, which is the same
    /// path a real spawn error takes.
    ///
    /// What this does NOT assert, said plainly: that `real_send_list` is the
    /// function on the far side of the pointer in a shipping build, and that
    /// it hands its `session` parameter to the runner rather than dropping
    /// it. The seam substitutes the pointer, so no test that drives the frame
    /// can see that. It is held from the source by
    /// [`source_pins::the_delegated_fetch_is_a_real_bw_send_list_for_the_active_account`]
    /// and by `production_is_the_only_env_a_shipping_build_has`.
    #[test]
    fn the_windows_own_session_is_what_reaches_the_bw_child() {
        let outcome = measured("session", Scenario { press_sends: true, ..Scenario::new() });
        assert!(
            outcome.send_list_spawns >= 1,
            "control: the Sends row was pressed and no Sends fetch was started at all, so \
             there is no session to be right or wrong about. What was painted: {:?}",
            outcome.painted
        );
        assert!(
            HARNESS_SESSION.len() > 8,
            "control: the token this test compares against is too short to be an \
             unmistakable match, so the assertions below could be satisfied by an accident"
        );
        assert_eq!(
            outcome.send_list_sessions,
            vec![HARNESS_SESSION.to_string(); outcome.send_list_spawns],
            "the Sends fetch was not handed the session this window was opened with. A real \
             `bw send list` started without it inherits no BW_SESSION and answers `locked` -- \
             which is exactly what this screen used to show against a real vault"
        );

        // Link 2. From here on the value under test is the one that came back
        // out of the frame, so nothing below can pass on a token the window
        // did not actually hand over.
        let arrived = outcome.send_list_sessions.into_iter().next().expect("counted above");

        // The verified CLI path `bw_job_command_in` refuses without. A path
        // that does not exist and never will: nothing is executed, because
        // the probe below refuses every spawn before `CreateProcess`.
        crate::bw_path::remember_verified_bw_exe(std::path::PathBuf::from(
            r"C:\deskwarden-test\first\bw.exe",
        ));
        let probe = crate::job_object::spawn_probe::SpawnProbe::arm();
        let refused = crate::send::cli_send_list(None, None, &arrived);
        // Plain strings, deliberately: the recorded type belongs to
        // `job_object`, and what this test is about is what the CHILD would
        // have been given, not the recorder's shape.
        let attempts: Vec<(Vec<String>, Vec<(String, Option<String>)>)> = probe
            .attempts()
            .into_iter()
            .map(|a| {
                (
                    a.args.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
                    a.envs
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.to_string_lossy().into_owned(),
                                v.as_ref().map(|v| v.to_string_lossy().into_owned()),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        drop(probe);

        assert!(
            refused.is_err(),
            "the probe refused the only spawn this read may make, yet it answered {refused:?} \
             -- so a child was started by a route the probe cannot see"
        );
        assert_eq!(
            attempts.len(),
            1,
            "the read path did not reach the one spawn exactly once, so any assertion about \
             what it carried is about nothing: {attempts:?}"
        );
        let (args, envs) = attempts.into_iter().next().expect("just counted one");

        assert_eq!(
            envs.iter()
                .filter(|(k, _)| k == "BW_SESSION")
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>(),
            vec![Some(HARNESS_SESSION.to_string())],
            "`BW_SESSION` did not arrive at the child set exactly once to the session the \
             window holds, so a real `bw send list` answers `locked`. The overlay that \
             arrived was {envs:?}"
        );
        assert_eq!(
            args,
            vec!["send".to_string(), "list".to_string()],
            "control: the recorded spawn does not carry this list's arguments, so the argv \
             check below is about some other command"
        );
        for arg in &args {
            assert!(
                !arg.contains(HARNESS_SESSION),
                "the session token is in argv, where every other process on the machine can \
                 read it: {arg}"
            );
        }
    }

    /// **The window's own session is what a real `bw sync` runs with.**
    ///
    /// The sync path's counterpart to
    /// [`the_windows_own_session_is_what_reaches_the_bw_child`], and it
    /// exists because the sync path had the identical defect one function
    /// over. Measured on `c92c00c`, **M-C**: `(spawn_sync)(sync_tx.clone(),
    /// String::new())` at BOTH call sites -- the auto-sync on the first
    /// real frame and the status pill's press -- gave 2101 lib / 217 bin /
    /// 0 failed / 0 warnings. Every `bw sync` then ran with
    /// `BW_SESSION=""`, which a real vault answers `Locked`: Sync silently
    /// stops syncing and the window keeps painting whatever it already had.
    ///
    /// Nothing here is a spelling. A real frame is driven, and what is read
    /// back is the value that ARRIVED at the `VaultFrameEnv::sync` pointer
    /// -- so a session dropped, emptied, defaulted or read from anywhere
    /// but `build_frame`'s own `session_token` fails here whatever the
    /// source says.
    ///
    /// **Both call sites, not one.** The auto-sync is reached by every
    /// scenario in this module; the pill is reached by this one alone, and
    /// pressing it is the only way there is (`sync_in_progress` gates the
    /// click and `draw_toolbar`'s pill is its sole producer). A mutation
    /// that empties only the button's argument therefore dies here too,
    /// which a source count over one call site could not manage.
    ///
    /// **No `bw` child is started.** `counted_sync` is a stub;
    /// `spawn_vault_sync` is behind the seam and is never called. What
    /// happens to the token BELOW the pointer -- that `bw_serve::run_bw_sync`
    /// puts it in the child's environment rather than argv -- is not
    /// asserted here, and is recorded as open in this round's notes.
    ///
    /// **The stub answers a FAILED sync**, and that is not incidental: a
    /// successful one made this test a known flake for about a day. The
    /// mechanism, the measurements and why answering a failure takes nothing
    /// away from what is asserted below are all on [`SYNC_ANSWERS`]. Nothing
    /// in this scenario reads a clock now; the two assertions below are a
    /// count and an equality on recorded tokens.
    #[test]
    fn the_windows_own_session_is_what_reaches_the_bw_sync_child() {
        let outcome = measured("sync-session", Scenario { press_sync: true, ..Scenario::new() });
        assert!(
            HARNESS_SESSION.len() > 8,
            "control: the token this test compares against is too short to be an \
             unmistakable match, so the assertions below could be satisfied by an accident"
        );
        assert_eq!(
            outcome.sync_spawns, 2,
            "this scenario did not reach BOTH `VaultFrameEnv::sync` call sites exactly once \
             each -- the auto-sync on the window's first real frame and the toolbar pill's \
             press -- so whichever it missed is unmeasured here. What was painted: {:?}",
            outcome.painted
        );
        assert_eq!(
            outcome.sync_sessions,
            vec![HARNESS_SESSION.to_string(); 2],
            "a `bw sync` was started without the session this window was opened with. A \
             `bw sync` whose `BW_SESSION` is missing or empty is answered `Locked` by a \
             real vault, so Sync silently stops syncing while the pill still says it \
             worked"
        );
    }

    /// **Visiting the Sends screen starts no further `bw sync`.**
    ///
    /// [`the_windows_own_session_is_what_reaches_the_bw_sync_child`] pins
    /// `sync_spawns == 2` -- but under `press_sync`, a scenario that never
    /// leaves the Vault screen. A third `(spawn_sync)` call site written on
    /// the SENDS arm is therefore never entered there, and the scenario that
    /// does reach Sends read `send_list_sessions` and never looked at the
    /// sync counters at all. Measured on `328996b`: an aliased
    /// `sync_now(sync_tx.clone(), String::new())` under `if on_sends` in the
    /// frame closure was green at 2108 / 0 failed.
    ///
    /// This is the half that does not care how the third site is SPELLED.
    /// `press_sends` drives three further frames on the Sends screen, so a
    /// per-frame `bw sync` there is not one extra spawn but several -- and
    /// in the shipping window it is one per frame for as long as the screen
    /// is up, every one of them an un-jobbed child.
    ///
    /// The expectation is the auto-sync on the window's first real frame and
    /// nothing else, and it is an equality on the SESSIONS as well as the
    /// count, so the one permitted sync still has to carry the window's own
    /// token. `send_list_spawns` is the positive control that the click
    /// landed and the Sends arm really was drawn; without it a scenario that
    /// never left the Vault screen would satisfy this trivially.
    #[test]
    fn visiting_the_sends_screen_starts_no_further_bw_sync() {
        let outcome = measured("sends-sync", Scenario { press_sends: true, ..Scenario::new() });
        assert!(
            outcome.send_list_spawns >= 1,
            "control: the Sends row was pressed and no Sends fetch was started, so this \
             window never reached the Sends screen and a sync call site written on that \
             arm would go unentered here too. What was painted: {:?}",
            outcome.painted
        );
        assert_eq!(
            outcome.sync_sessions,
            vec![HARNESS_SESSION.to_string()],
            "driving the window to the Sends screen started {} `bw sync` children, not the \
             one auto-sync of the window's first real frame. Every frame the Sends screen \
             is up would start another, un-jobbed, and any of them handed a session other \
             than the window's own is answered `Locked` by a real vault",
            outcome.sync_spawns
        );
    }

    /// **The "your vault could not be loaded" page returns promptly.**
    ///
    /// The third early return, added after the loading arm and the one that
    /// once shipped without a repaint schedule of its own. A frame that waits
    /// here is a window that failed to load AND froze.
    ///
    /// Positive control: the page's heading and **the loader's own reason**,
    /// which this test chose ([`HARNESS_FAILURE`]). Requiring both is what
    /// separates "the `Unavailable` arm ran" from "some centred label
    /// happened to be on screen".
    #[test]
    fn the_unavailable_body_returns_promptly() {
        let outcome = measured("unavailable", Scenario { load: load_that_fails, ..Scenario::new() });
        assert_the_body_was_entered("unavailable", &outcome);
        outcome.expect_painted(
            "could not be loaded",
            "the failed load did not reach the `VaultBodyState::Unavailable` arm",
        );
        outcome.expect_painted(
            HARNESS_FAILURE,
            "the error page is up but it is not showing the reason this test handed the \
             loader, so it is not this load's failure being reported",
        );
    }

    /// **The bound is real**, and this is the test that says so.
    ///
    /// Without it, `within` could return `Ok` unconditionally -- or the
    /// budget could be raised past any wall clock -- and every test above
    /// would stay green while holding nothing. So: a body that waits well
    /// past its budget must come back `Err`, and it must come back at all,
    /// which is the other half (a bound that hangs is not a bound).
    #[test]
    fn a_body_that_waits_past_its_budget_is_reported_rather_than_waited_for() {
        let asked_at = Instant::now();
        let answer = within(Duration::from_millis(200), || {
            std::thread::sleep(Duration::from_secs(5));
            Duration::from_secs(5)
        });
        assert!(answer.is_err(), "a five-second body was admitted under a 200ms budget");
        assert!(
            asked_at.elapsed() < Duration::from_secs(4),
            "the bound took {:?} to report -- it waited for the body instead of giving up on \
             it, which is the suite-hanging shape this whole design exists to avoid",
            asked_at.elapsed()
        );
        // And the other direction: a body that answers is not reported as a
        // timeout, or the test above would be green for the wrong reason.
        assert!(
            within(Duration::from_secs(10), || Duration::from_millis(1)).is_ok(),
            "control: an instant body was reported as an overrun"
        );
    }

    /// **A body that PANICS is reported as a panic, not as a timeout.**
    ///
    /// It used to be the other way round: the only signal was the sender
    /// being dropped, so an epaint panic in the frame -- much the commonest
    /// way one of these scenarios goes wrong -- burned the whole twenty
    /// seconds and then produced a message whose first sentence was the wrong
    /// diagnosis. Both halves are asserted: the reason must be the panic's
    /// own words, and it must arrive nowhere near the budget.
    #[test]
    fn a_body_that_panics_is_reported_as_a_panic_rather_than_as_a_timeout() {
        // The default hook would print this deliberate panic and make a green
        // run look like a failed one; restored before the assertions so a
        // real failure below still reports normally.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let asked_at = Instant::now();
        let answer = within(Duration::from_secs(20), || -> u8 {
            panic!("the harness panicked on purpose")
        });
        std::panic::set_hook(hook);
        match answer {
            Err(Halt::Panicked(message)) => assert!(
                message.contains("on purpose"),
                "the panic was reported without what it said: {message:?}"
            ),
            other => panic!(
                "a body that panicked was reported as {other:?} rather than as a panic, so \
                 every panicking scenario spends its whole budget and then blames a wait"
            ),
        }
        assert!(
            asked_at.elapsed() < Duration::from_secs(5),
            "the panic took {:?} to report -- it was waited out rather than caught, which is \
             the twenty-second silence this catch exists to remove",
            asked_at.elapsed()
        );
    }

    /// **The scratch directory does not survive the run.**
    ///
    /// The reported hygiene bug was that `FillStats::new` and the icons path
    /// create `%TEMP%\deskwarden-frame-harness-<pid>` and nothing removes it,
    /// leaving one directory per test process. **Measured, and it does not
    /// reproduce**: `FillStats::new` stores a `PathBuf` and creates nothing,
    /// and the icons directory is created by `favicon.rs` only when a favicon
    /// is actually written -- which needs a `server_url`, and this harness
    /// hands `None`. No such directory exists in `%TEMP%` on this machine
    /// after any number of suite runs.
    ///
    /// [`Scratch`] is kept anyway, and so is this test, because "nothing
    /// under that path is written today" is a property of the arms currently
    /// driven, not of the window: a scenario added later that reaches the
    /// favicon cache or a `record_fill` would start leaving one behind, and
    /// the guard means nobody has to notice.
    ///
    /// **Both halves are asserted**, because a `Drop` that deleted a
    /// directory nothing ever created would pass the second assertion alone
    /// -- and since the run genuinely creates nothing, the control makes the
    /// directory itself rather than pretending the window did.
    #[test]
    fn the_harness_leaves_no_scratch_directory_behind() {
        let path = {
            let scratch = Scratch::new("hygiene");
            let path = scratch.0.clone();
            let outcome = within(BUDGET, {
                let path = path.clone();
                move || drive(Scenario::new(), &path)
            })
            .unwrap_or_else(|halt| panic!("the hygiene scenario did not come back: {halt:?}"));
            assert!(outcome.load_spawns >= 1, "control: the hygiene scenario built no window");
            // Control: there IS something to delete when the guard drops.
            std::fs::create_dir_all(path.join("icons"))
                .expect("could not create the control directory");
            std::fs::write(path.join("icons").join("probe"), b"probe")
                .expect("could not create the control file");
            assert!(path.exists(), "control: the control directory was not created");
            path
        };
        assert!(
            !path.exists(),
            "the harness left {path:?} behind, contents and all. One abandoned directory per \
             test process is what this drop guard exists to stop"
        );
    }
}

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
//!
//! ## Design §5b and §5c, and what this screen is not
//!
//! §5a — "Compose a Send" — is **not this file**. It is
//! [`super::record_ui::draw_export_form`], whose card header is §5a's own
//! `Send a record` and whose tick list is §5a's field-level opt-in; the
//! composer below makes a plain text Send and has no counterpart anywhere on
//! the design page. The lifetime run and the footer here nevertheless take
//! their treatment from §5a, because this app has one segmented control and
//! one pair of footer buttons and the Send screens were the last surface
//! still drawing its own.
//!
//! **§5b, "Shared folder", is a full-window frame and not a screen to build.**
//! It wears the page's dark badge, which across the whole design marks the
//! frames drawn at window width (2b "Vault window", 3f, 4e, 7b) rather than
//! the panel-sized mockups, and its caption says so outright: SAME LIST +
//! DETAIL AS THE VAULT. It is the existing vault window with the Sends screen
//! in the centre column — a wider framing of surfaces that exist — plus three
//! things that do not: a `SHARING` sidebar section with `Waiting` / `Used` /
//! `Ended` sub-filters under it, a `Shared with me` row, and a per-Send detail
//! pane. The titlebar `Send a record CTRL+⇧+S` pill it draws was built, then
//! **removed at the owner's direction** and replaced by an envelope in the
//! detail pane's header strip; see [`super::record_ui::SEND_RECORD_LABEL`],
//! where that departure is argued in place. So §5b is not evidence of an
//! unbuilt screen so much as of an unbuilt *state model* — see §5c.
//!
//! **§5c, "History & states", is a feature and not a treatment.** It wants
//! four named states (`Waiting`, `Used`, `Expired`, `Revoked`) and a per-Send
//! timeline reading "Password revealed · 15:01 · Edge on Windows · Berlin, DE
//! · 84.13.…". [`SendSummary`] carries an id, a name, an access URL, a
//! deletion date and `is_file`, and [`SendRow`] paints those. The states are
//! *derivable* from fields Bitwarden's Send API does have and this crate's
//! parser does not read (`accessCount`, `maxAccessCount`, `disabled`,
//! `expirationDate`); the timeline is not, at any price — there is no
//! per-access audit in a Bitwarden Send, no recipient, no user agent and no
//! geolocation, and nothing here could invent them. Painting the four state
//! pills over an inferred state, while the card above them promises an
//! opened-at time this app cannot know, would make the richer half of §5c a
//! blank that reads as a bug. Both halves need the owner's call on what the
//! feature is before either is drawn.

use crate::local_time::LocalOffset;
use crate::send::{SendClock, SendError, SendSummary};
use crate::theme;
use eframe::egui::{self, CornerRadius, Margin};

/// The one line of subtext under the heading. **It is what makes the excluded
/// scope honest rather than hidden**: this screen shows every Send, including
/// the kinds this app will never be able to make, and the user is told where
/// the rest of the feature lives instead of discovering its absence.
pub const SCOPE_SUBTEXT: &str =
    "Deskwarden creates and deletes text Sends. Use the web vault for file Sends and for editing.";

/// The tag drawn on a Send this app could not have created.
pub const FILE_TAG: &str = "FILE";

/// What the tag means, spelled out, when the list actually contains one.
///
/// **A three-letter tag is a label, not an explanation.** A user who sees
/// `FILE` beside a Send learns that this row differs from the others and
/// nothing about how -- in particular not that the two buttons on it still
/// work. The sentence is the design's own
/// (`2026-08-30-sends-without-the-cli-design.md`, section 2), and it says the
/// limit and the remedy in that order: this app sends text, and a file Send
/// made elsewhere is still yours to copy or revoke from here.
///
/// Painted only when a file Send is present, so a list of text Sends is not
/// told about a restriction it has not met.
pub const FILE_SEND_EXPLANATION: &str = "Deskwarden can send text, not files. This Send holds a \
                                         file, so you can copy its link or delete it here, but it \
                                         was made somewhere else.";

/// What a Send **read from a link** needs, when reading one fails for want of
/// the CLI.
///
/// The one genuine subtraction in dropping `bw.exe` for Sends, and it is said
/// as a missing tool rather than left to surface as a bad link: a user told
/// "that link could not be read" goes and checks the link, which is fine.
/// The second sentence is there because without it the first reads as "Sends
/// are broken", which is false in three operations out of four.
pub const RECEIVE_NEEDS_THE_CLI: &str = "Reading a Send from a link needs Bitwarden's command-line \
                                         tool. Publishing, listing and revoking your own Sends do \
                                         not.";

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
    /// Design §5c's state, as [`crate::send::send_state`] derived it from the
    /// summary and the same clock the expiry was worded against.
    ///
    /// Held on the row rather than re-derived in the painter for
    /// [`expiry`](SendRow::expiry)'s reason: one clock read per list, decided
    /// in a pure function, so two cells of one row cannot disagree about what
    /// time it is.
    pub state: crate::send::SendState,
    /// How many times the link has been opened, and the cap it is counted
    /// against -- `None` for "as often as the recipient likes".
    ///
    /// Carried raw rather than pre-worded because the detail pane draws them
    /// as a fraction AND as a bar, and a string could only be one of the two.
    /// The words the SUBTITLE uses are still decided once, in
    /// [`row_subtitle`].
    pub access_count: u32,
    pub max_access_count: Option<u32>,
    /// Whether opening the link needs the share password. 5b's `link only`
    /// row, in the one direction worth printing; see [`row_subtitle`].
    pub has_password: bool,
    /// **5c's first line, as much of it as this client can actually know.**
    ///
    /// 5c opens "Opened once, 40 minutes after it was sent", because whether
    /// it was used is the question people actually have. The *when* half of
    /// that is not derivable -- there is no per-access record in a Bitwarden
    /// Send -- and the *whether* half is. So the sentence says what was
    /// opened and what is left, and says nothing about when, rather than
    /// leaving 5c's premise unanswered on a screen whose whole subject it is.
    /// See [`activity_sentence`].
    pub activity: String,
    /// When the link stops answering, in words -- the detail pane's `Expires`
    /// row.
    ///
    /// **Its own field beside [`expiry`](SendRow::expiry), which is NOT the
    /// same date.** That one is `deletion_date`, when the record goes;
    /// Bitwarden lets a link stop answering some time before that, and the
    /// two being one field is precisely how a dead link comes to be described
    /// as live. See [`crate::send::SendSummary::expiration_date`].
    pub expires: String,
    /// When the record itself is removed, in words -- the detail pane's
    /// `Deleted` row, and the same date [`expiry`](SendRow::expiry) is worded
    /// from.
    pub deletes: String,
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
        selected && self.wants_fetch_now()
    }

    /// **The same question with the screen taken out of it**, for the readers
    /// that are not the Sends screen.
    ///
    /// Design 5d put two of them on the vault screen -- the `shared` pill on
    /// every row and the read pane's `SHARING` card -- and a list fetched only
    /// while the Sends screen is up is a pill that appears after a detour
    /// through a screen the user had no reason to visit.
    ///
    /// Still one fetch per visit and not a poll: the only thing that clears
    /// `result` is [`invalidate`](Self::invalidate). `wants_fetch` is this
    /// with the screen gate in front of it, so the Sends screen's own policy
    /// and its tests are unchanged.
    pub fn wants_fetch_now(&self) -> bool {
        self.result.is_none() && !self.in_flight
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

    /// [`badge_count`](Self::badge_count) and design 5b's three sub-row
    /// counts, **in one value derived in one pass against one clock**.
    ///
    /// The clock is a parameter for `crate::send`'s standing reason -- three
    /// of the four states are readings of a date against now, so a function
    /// that reached for the wall clock could only be tested for the shape of
    /// its answer and never for its content.
    ///
    /// It is one call and not four because the four badges are four cells of
    /// one row of arithmetic: `waiting + used + ended == all` is a property
    /// of the SAME list read at the SAME instant, and two calls a frame apart
    /// (or against two clocks) would draw a rail whose sub-rows do not add up
    /// to their parent. `sidebar::SendCounts::over` is where the tallying
    /// happens; this is only the "did the fetch answer at all" half, which is
    /// `badge_count`'s rule unchanged: a failure is `None`, never a set of
    /// zeroes.
    pub fn counts(&self, now: &dyn SendClock) -> Option<crate::vault_window::sidebar::SendCounts> {
        match self.result.as_ref()? {
            Ok(sends) => Some(crate::vault_window::sidebar::SendCounts::over(
                sends.iter().map(|send| crate::send::send_state(send, now)),
            )),
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
    zone: &dyn LocalOffset,
) -> SendPaneState {
    match result {
        None => SendPaneState::Loading,
        Some(Err(e)) => SendPaneState::Failed {
            message: e.user_message().to_string(),
            ambiguous: e.is_ambiguous(),
        },
        Some(Ok(sends)) if sends.is_empty() => SendPaneState::Empty,
        Some(Ok(sends)) => SendPaneState::Rows(rows_from(sends, now, zone)),
    }
}

/// Every summary becomes a row, **including the file ones**. There is no
/// filter here and there must not be one; see the module docs.
pub fn rows_from(
    sends: &[SendSummary],
    now: &dyn SendClock,
    zone: &dyn LocalOffset,
) -> Vec<SendRow> {
    sends.iter().map(|send| row_from(send, now, zone)).collect()
}

/// One summary to one row.
///
/// A Send with no name is drawn as `(no name)` rather than as an empty
/// string: `bw` allows it, and a row with nothing where the name goes reads
/// as a rendering failure rather than as the Send it is.
pub fn row_from(send: &SendSummary, now: &dyn SendClock, zone: &dyn LocalOffset) -> SendRow {
    SendRow {
        id: send.id.clone(),
        name: if send.name.trim().is_empty() {
            "(no name)".to_string()
        } else {
            send.name.clone()
        },
        expiry: row_subtitle(send, now),
        is_file: send.is_file,
        access_url: send.access_url.clone(),
        state: crate::send::send_state(send, now),
        access_count: send.access_count,
        max_access_count: send.max_access_count,
        has_password: send.has_password,
        activity: activity_sentence(send, now),
        expires: date_words(&send.expiration_date, now, zone, NO_EXPIRY_OF_ITS_OWN),
        deletes: date_words(&send.deletion_date, now, zone, DELETION_DATE_UNKNOWN),
    }
}

/// **Design 5c's headline, reduced to what this client can stand behind.**
///
/// # What 5c wanted and what is here
///
/// 5c's card reads "Opened once, 40 minutes after it was sent", and under it
/// a per-access timeline: `Password revealed - 15:01 - Edge on Windows -
/// Berlin, DE - 84.13.…`. **None of the second half exists at any price.** A
/// Bitwarden Send carries `accessCount` and nothing else about access: no
/// per-open record, no user agent, no address, no geolocation, and no time of
/// any particular open. Building it would mean the owner's server logging
/// every access with an IP, which is their decision and not a gap in this
/// screen.
///
/// So the detail pane draws **no Activity card at all** rather than a
/// labelled empty box waiting for one, and this sentence is what takes its
/// place: the one fact behind 5c's premise, stated in words, on the first
/// line of the pane. "Opened once" is here. "40 minutes after it was sent" is
/// not, and is not alluded to.
///
/// # Why the state is not simply re-printed
///
/// The pill beside the title already says `Used`. This says the *arithmetic*
/// the pill is a summary of -- how many opens happened, how many are left,
/// and for `Revoked`, what would undo it. A sentence that only restated the
/// pill would be the labelled empty box in prose.
pub fn activity_sentence(send: &SendSummary, now: &dyn SendClock) -> String {
    let opened = match send.access_count {
        0 => "Nobody has opened this link".to_string(),
        1 => "Opened once".to_string(),
        n => format!("Opened {n} times"),
    };
    match crate::send::send_state(send, now) {
        // The one state a person caused, and the only one with an undo, so
        // the sentence names the undo. `SWITCH_ON_LABEL` rather than the word
        // "revoke": see that constant for why this screen does not spend that
        // word here.
        crate::send::SendState::Revoked => {
            format!("{opened}. You turned the link off -- {SWITCH_ON_LABEL} to let it work again.")
        }
        crate::send::SendState::Used => {
            format!("{opened}. Every permitted view is spent, so the link is dead.")
        }
        crate::send::SendState::Expired => {
            format!("{opened}. The link ran out of time, so it no longer answers.")
        }
        crate::send::SendState::Waiting => match (send.max_access_count, send.access_count) {
            // The remaining budget, which is the fact a live capped link is
            // actually about. Saturating rather than wrapping: a server that
            // reported more opens than the cap while still calling the Send
            // live would otherwise print four billion views left.
            (Some(cap), used) => {
                let left = cap.saturating_sub(used);
                let views = if left == 1 { "view" } else { "views" };
                format!("{opened}. {left} {views} left before the link stops working.")
            }
            (None, _) => format!("{opened}. The link works, as often as they like."),
        },
    }
}

/// What the detail pane says where a Send has no expiry of its own -- the
/// ordinary case for a Send made by this app's composer, which sets a
/// lifetime rather than a separate expiry.
///
/// **Not "never".** The record still goes on its deletion date and the link
/// goes with it, so "never" would be the one wrong word available here; the
/// pane prints this beside a `Deleted` row that gives the actual date.
pub const NO_EXPIRY_OF_ITS_OWN: &str = "None of its own -- it lasts until the Send is deleted";

/// What the detail pane says for a deletion date it could not read.
///
/// `expiry_words` has refused to guess "Expired" from an unparseable date
/// since this screen shipped, for the reason it gives: this app failing to
/// understand a date is not the same fact as the link being dead. Same rule,
/// same wording style.
pub const DELETION_DATE_UNKNOWN: &str = "Unknown";

/// One stored date as the detail pane prints it: the user's own day and time,
/// then how far off it is, or `absent` when the field is empty or
/// unparseable.
///
/// **Relative AND absolute, not one or the other.** `expiry_words` gives the
/// list rows "Expires in 3 days", which is the right answer for a row that is
/// scanned; a detail pane is what somebody opens when that is not precise
/// enough, and an absolute instant with no "in 3 days" beside it puts the
/// arithmetic back on the reader. The absolute half is deliberately NOT
/// printed by the list rows, so the two surfaces stay different rather than
/// redundant.
pub fn date_words(
    date: &str,
    now: &dyn SendClock,
    zone: &dyn LocalOffset,
    absent: &'static str,
) -> String {
    let Some(at) = parse_iso_utc_millis(date) else {
        return absent.to_string();
    };
    let relative = relative_words(at - now.now_unix_millis());
    format!("{when} \u{00b7} {relative}", when = at_words(at, zone))
}

/// An instant as `17 Aug 2026, 14:20`, in the user's **own** timezone.
///
/// The zone is a parameter, and threading one all the way down through
/// [`pane_state`] and [`rows_from`] to get it here is the point rather than
/// the cost. This file's standing rule is that nothing which decides what a
/// date SAYS may read the machine for itself -- `now` has been a parameter
/// since the screen shipped, and `zone` joined it when the composer's expiry
/// line stopped naming the UTC day. A detail pane printing an absolute
/// wall-clock instant is the strongest case for that rule, not an exception
/// to it: read off `SystemZone` here, its every assertion would say something
/// different on a runner in another timezone, or on the same runner in March.
fn at_words(millis: i64, zone: &dyn LocalOffset) -> String {
    crate::local_time::format_day_time(crate::local_time::local_parts(millis, zone))
}

/// `in 3 days` / `23 hours ago`, from a signed millisecond delta.
///
/// Hours below a day and days above, which is what 5b prints on both sides of
/// its own list ("expires in 3 h", "2 d ago"): a link with four hours left
/// described as "in 0 days" is the arithmetic slip that makes a countdown
/// useless exactly when it matters.
fn relative_words(remaining: i64) -> String {
    let past = remaining < 0;
    let magnitude = remaining.unsigned_abs();
    let hours = magnitude / crate::local_time::MILLIS_PER_HOUR.unsigned_abs();
    let days = magnitude / crate::local_time::MILLIS_PER_DAY.unsigned_abs();
    let amount = if days >= 1 {
        format!("{days} day{}", if days == 1 { "" } else { "s" })
    } else if hours >= 1 {
        format!("{hours} hour{}", if hours == 1 { "" } else { "s" })
    } else {
        "less than an hour".to_string()
    };
    if past {
        format!("{amount} ago")
    } else {
        format!("in {amount}")
    }
}

/// The row's second line, as design §5b writes it: what the link needs, how
/// many views are left, and when it ends, separated by the design's own
/// middot.
///
/// # What is here, what is not, and why
///
/// 5b's five rows read "to m.reyes - 40 min ago", "link only - 3 of 10 views
/// - 6 d left", "to j.abara - expires in 3 h", "to contractor@vantage.io -
/// never opened" and "revoked by you - 2 d ago". **Only the middle one is
/// derivable today.** A recipient address is `emails` on the wire and this
/// client neither sets it nor reads it; "40 min ago" and "2 d ago" are a
/// `revisionDate` relative to now, which is available but says nothing until
/// there is a per-access record to date -- see the `5c` timeline, which this
/// pass deliberately did not build.
///
/// **"link only" is omitted and its opposite is not.** The design prints it
/// on the one row that has no password, and printing it on every unprotected
/// row here would put four words of nothing on the majority of Sends this app
/// creates (the composer's password is off by default). A password, on the
/// other hand, is the notable fact -- it is the difference between a link
/// that is the whole credential and one that is not -- so it is said and its
/// absence is not.
///
/// # Why a dead link has no expiry on it
///
/// The expiry is [`expiry_words`] unchanged and it is last, for 5b's reason:
/// it is the only segment that keeps moving. But it is **omitted entirely
/// unless the link is still live**, and that is not tidiness -- it is the one
/// thing this line could say that would be false.
///
/// `expiry_words` reads `deletion_date`, and a Send can be dead for two
/// reasons that date knows nothing about: its views can be spent, and its
/// `expiration_date` can pass while the record lives on until deletion. Both
/// leave a row whose pill says `Used` or `Expired` beside words that say
/// "Expires in 7 days". The pill is right and the words are wrong, so the
/// words go: a state that has ended is fully described by the pill that names
/// it, and this line goes back to carrying only what the pill cannot say.
pub fn row_subtitle(send: &SendSummary, now: &dyn SendClock) -> String {
    let mut parts: Vec<String> = Vec::new();
    if send.has_password {
        parts.push(PASSWORD_SEGMENT.to_string());
    }
    match (send.max_access_count, send.access_count) {
        // Capped: the design's own "3 of 10 views", which says the remaining
        // budget and the spent one in one phrase.
        (Some(cap), used) => parts.push(format!("{used} of {cap} views")),
        // Uncapped and untouched: nothing. "0 views" on a link nobody has
        // opened yet reads as a failure rather than as a beginning, and the
        // state pill beside it already says `Waiting`.
        (None, 0) => {}
        (None, 1) => parts.push("opened once".to_string()),
        (None, used) => parts.push(format!("opened {used} times")),
    }
    if crate::send::send_state(send, now) == crate::send::SendState::Waiting {
        parts.push(expiry_words(&send.deletion_date, now));
    }
    parts.join(" \u{00b7} ")
}

/// What the subtitle says about a Send whose link is not the whole
/// credential.
pub const PASSWORD_SEGMENT: &str = "Password required";

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

/// **`crate::send`'s parser, re-exported rather than owned.**
///
/// This function was written here, and it moved when the states pass needed
/// the same bytes read on the other side of the seam: `send::send_state`
/// compares `expiration_date` and `deletion_date` against a clock, and it
/// lives beside the type those are fields of. Two parsers for one wire format
/// agree only by coincidence, and this file already carries the scar of that
/// -- `send::lifetime_label` is here in re-export form for the same reason,
/// after a duration was spelled twice and the two spellings drifted.
///
/// The name is kept and the path is kept, so `record_ui`'s certificate dates
/// -- the one caller outside this file -- still read
/// `send_ui::parse_iso_utc_millis` and are unaffected by where the arithmetic
/// now lives.
pub use crate::send::parse_iso_utc_millis;

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
    /// **Design §5c's Revoke, and its undo**: switch this row's link off
    /// (`disabled: true`) or back on.
    ///
    /// `disabled` is what the row ASKED for rather than what it currently is,
    /// which is the same rule the id and the name follow here and for the
    /// same reason: everything the far side needs is read off the row that
    /// was clicked, so no second lookup can resolve to a different Send or to
    /// a stale reading of this one.
    ///
    /// **One step, where a delete is two.** See
    /// `vault_window::apply_send_action`: a confirmation exists because a
    /// delete cannot be undone, and this can -- by pressing the button again.
    SetDisabled { id: String, name: String, disabled: bool },
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

/// **Design 5d's Revoke, minted for a frame on which the Sends screen was not
/// drawn.**
///
/// The second and last thing `vault_window` can obtain without calling
/// [`draw_send_pane`], and it exists because the read pane's `SHARING` card
/// has a Revoke on it while the Sends screen is nowhere on the glass.
///
/// **Why a mint here rather than a second `apply_send_action` call there.**
/// The frame closure applies exactly one Sends action, unconditionally, and
/// `send_delete_wiring::the_frame_applies_the_sends_action_unconditionally`
/// counts the call sites to keep it that way -- four separate ways of making
/// the whole Sends screen inert were measured green before that count existed,
/// every one of them a gate or a shadowed binding in front of the applier. A
/// conditional second call is that shape again. So the read pane's request
/// travels as the frame's ONE action instead, through the one applier, taking
/// the same `in_flight` lock and reporting through the same band as a Revoke
/// pressed on the Sends screen itself.
///
/// It carries exactly one action, named here, with the id and the name the
/// caller found; `SendUiVerdict::seal` and the field stay private, so this is
/// still not a door to substituting an arbitrary action for a drawn pane's.
pub fn revoke_from_the_read_pane(id: String, name: String) -> SendUiVerdict {
    SendUiVerdict::seal(SendUiAction::SetDisabled { id, name, disabled: true })
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

/// **Design §5c's Revoke, under a name this screen can use.**
///
/// The design calls the `disabled` flag Revoke, and this screen cannot: the
/// word is already spoken for. `DELETE_LABEL`'s own confirmation reads
/// "Revoke this link for good? It cannot be undone", and every sentence,
/// constant and test around the delete path in this crate calls it the
/// revoke. Two controls on one row both called Revoke, one of which is
/// permanent and one of which is not, is the worst outcome available here.
///
/// So the two say what they do to the link, which is also the one thing that
/// distinguishes them from Delete: this takes the link down and keeps the
/// Send, and it can be pressed again. **This is a departure from the design
/// and it is the owner's to settle** -- the other way to resolve it is to
/// rename the destructive path, which is a bigger change than a label and
/// touches wording this screen's tests are pinned to.
pub const SWITCH_OFF_LABEL: &str = "Switch off";
/// The undo of [`SWITCH_OFF_LABEL`]. Drawn only on a row whose state is
/// already `Revoked`, so the two are never offered at once.
pub const SWITCH_ON_LABEL: &str = "Switch on";
/// The detail header's **primary** control, and the one the screen is for: a
/// Send exists so that a link can be handed to somebody.
///
/// Named now that [`action_labels`] has to measure the row before the row is
/// drawn -- a literal in two places is one rewording away from a header
/// measured for a label it does not carry.
pub const COPY_LINK_LABEL: &str = "Copy link";
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
/// Design §5c's four state words, exactly as the design spells them.
///
/// Public because the paint tests press on them and because the words are the
/// whole of what a pill says -- a state pill whose label drifted from the
/// design would be a pill nobody could look up.
pub const WAITING_LABEL: &str = "Waiting";
pub const USED_LABEL: &str = "Used";
pub const EXPIRED_LABEL: &str = "Expired";
pub const REVOKED_LABEL: &str = "Revoked";

/// One [`crate::send::SendState`] as the word the design prints for it.
pub fn state_label(state: crate::send::SendState) -> &'static str {
    match state {
        crate::send::SendState::Waiting => WAITING_LABEL,
        crate::send::SendState::Used => USED_LABEL,
        crate::send::SendState::Expired => EXPIRED_LABEL,
        crate::send::SendState::Revoked => REVOKED_LABEL,
    }
}

/// One [`crate::send::SendState`] as the colours and mark design §5c draws it
/// in.
///
/// **A pure function returning a value, rather than four call sites into
/// `theme::state_pill`.** The mapping is the thing worth testing -- that
/// `Used` is the only one with a tick, that `Expired` and `Revoked` carry no
/// mark at all, that `Waiting` is the only blue one -- and a test can read a
/// returned `PillTone` without a frame.
///
/// Every colour is a `theme` constant. The three greens and three reds the
/// design uses here were added to `theme` by this pass rather than written
/// locally, because 5b draws this pill in the list and again in the detail
/// header and 5d draws it on a record; see `theme::state_pill`.
pub fn state_tone(state: crate::send::SendState) -> theme::PillTone {
    match state {
        crate::send::SendState::Waiting => theme::PillTone {
            fill: theme::BLUE_WASH,
            edge: theme::BLUE_EDGE,
            ink: theme::BLUE_DEEP,
            mark: theme::PillMark::Dot(theme::BLUE),
        },
        crate::send::SendState::Used => theme::PillTone {
            fill: theme::DONE_WASH,
            edge: theme::DONE_EDGE,
            ink: theme::DONE_INK,
            mark: theme::PillMark::Check(theme::DONE_MARK),
        },
        // **No mark, and that is the design's own distinction**, not an
        // omission: a dot on these two would say the link is still doing
        // something. See `theme::PillMark`.
        crate::send::SendState::Expired => theme::PillTone {
            fill: theme::CANVAS,
            edge: theme::BORDER,
            ink: theme::TEXT_FAINT,
            mark: theme::PillMark::None,
        },
        crate::send::SendState::Revoked => theme::PillTone {
            fill: theme::DANGER_WASH,
            edge: theme::DANGER_EDGE,
            ink: theme::DANGER_INK,
            mark: theme::PillMark::None,
        },
    }
}

/// The heading the Sends PANE paints in its list column's strip.
///
/// **It is no longer the same string as `sidebar::SENDS_ROW_LABEL`, and that
/// is the point of this rewording rather than a side effect of it.** Design
/// 5b gives the list column an uppercase eyebrow (`SHARED`) over a rail row
/// in sentence case, so the two differ in the design; they used to be equal
/// here, and the matrix test exploited that by counting "two occurrences of
/// `Sends`" to tell "the pane painted" from "only the rail row that leads to
/// it painted".
///
/// A count of a SHARED string is the weakest form of that check: it fails
/// open the moment any third surface prints the same word, and it says
/// nothing about which of the two occurrences is which. A string only the
/// pane paints is a direct witness, so the matrix now asserts THIS constant
/// is on screen and separately asserts the two constants are not equal -- so
/// the witness cannot quietly degenerate back into the rail row by somebody
/// making them match again. See
/// `vault_window::send_delete_wiring::drive_the_sends_screen_in`.
pub const SENDS_HEADING: &str = "SENDS";

// ---------------------------------------------------------------------------
// Design 5d -- which record a live Send belongs to
// ---------------------------------------------------------------------------

/// **The Send a record has out right now, or `None`.**
///
/// Design 5d: "A record with a live Send carries a filled pill, so sharing is
/// visible from the list", and the pill "always means *someone can open this
/// right now*, never 'was shared once'". That sentence is the whole
/// specification of this function: `Waiting` is the only state in which a link
/// works, so it is the only state that answers `Some`.
///
/// # The link is the NAME, and that is a decision with a cost
///
/// Nothing on a Bitwarden Send records which record it was made from. There
/// were two ways to supply that:
///
///   * **A local file** keyed send id to item id, written when a record Send
///     is created. Exact, and this crate has three precedents for such a file
///     (`fill_stats`, `scan_history`, `receive_history`). Rejected on two
///     counts. It is another on-disk record of what the user has been doing --
///     `fill_recall` argues at length against persisting a timeline of which
///     accounts were used, and "which records I have shared" is the same kind
///     of fact. And it is per-machine: a Send published from a laptop would
///     leave no pill on the desktop, which is exactly the case where a user
///     most needs to be told that a record is out there.
///
///   * **The name**, which is what this does. `record_ui::send_plan_from`
///     names every record Send after the record -- `name: record.name.clone()`
///     -- so the name IS the link, it needs no new state anywhere, and it
///     works from any machine the account is signed in on.
///
/// **What the name cannot do**, stated because it is a real cost and not a
/// footnote: two records with the same name share a pill, and a Send made by
/// hand and named after a record lights that record's pill. Both are wrong in
/// the same direction and it is the safe one -- the pill says something named
/// after this record can be opened right now, which in each of those cases is
/// true -- but neither is precise, and a user who renames a record loses the
/// pill on a Send that is still live. The exact answer stays available: it is
/// the local file above, and this doc is where the argument for it is if the
/// trade ever stops paying.
///
/// The first match wins. A record with two live Sends has one pill either way;
/// the card beside it names the one it found.
pub fn live_send_named<'a>(
    sends: &'a [SendSummary],
    name: &str,
    now: &dyn SendClock,
) -> Option<&'a SendSummary> {
    sends
        .iter()
        .find(|send| send.name == name && crate::send::send_state(send, now) == crate::send::SendState::Waiting)
}

/// [`live_send_named`] against the fetch's own answer, which is three states
/// and not one: not asked yet, failed, or a list.
///
/// **A failure answers `None`, and that is the same rule `badge_count` makes
/// for the sidebar's number**: a fetch that did not happen does not know that
/// nothing is shared, and a pill is a claim. No pill is the honest reading of
/// "we do not know", because the pill's meaning is positive -- it says
/// something can be opened, never that nothing can.
pub fn live_send_in<'a>(
    fetched: Option<&'a Result<Vec<SendSummary>, SendError>>,
    name: &str,
    now: &dyn SendClock,
) -> Option<&'a SendSummary> {
    match fetched? {
        Ok(sends) => live_send_named(sends, name, now),
        Err(_) => None,
    }
}

/// The names of every record with a live Send, for the item list.
///
/// A set rather than a lookup per row for the reason the list is virtualized
/// at all: `item_row` is called for every visible row on every frame, and a
/// linear walk of the Sends inside it would be a walk per row per frame.
pub fn shared_names(
    fetched: Option<&Result<Vec<SendSummary>, SendError>>,
    now: &dyn SendClock,
) -> std::collections::HashSet<String> {
    let Some(Ok(sends)) = fetched else {
        return std::collections::HashSet::new();
    };
    sends
        .iter()
        .filter(|send| crate::send::send_state(send, now) == crate::send::SendState::Waiting)
        .map(|send| send.name.clone())
        .collect()
}

/// The eyebrow over the `Shared with me` list. 5b's own row label, in the
/// case the design's list strips are drawn in.
pub const RECEIVED_HEADING: &str = "SHARED WITH ME";

/// What the `Shared with me` screen says when nothing has ever been imported.
///
/// A **claim**, and a true one in every state: unlike the Sends list there is
/// no fetch behind this row and therefore no "could not check" -- see
/// [`crate::receive_history`], whose `load` folds every failure into an empty
/// history for reasons argued there.
pub const RECEIVED_EMPTY_HEADLINE: &str = "Nothing has been shared with you yet.";
pub const RECEIVED_EMPTY_DETAIL: &str =
    "A Send somebody else gives you is read from its link, and what you import is recorded here.";

/// The detail pane's standing line while no row is picked.
///
/// **Words, not a blank pane.** The vault's own detail pane says "Select an
/// item." for the same reason: an empty right-hand column reads as a load
/// that failed, and this screen's whole history is about not letting "we do
/// not know" and "there is nothing" look alike.
pub const NOTHING_PICKED: &str = "Pick a Send to see its link, its views and its dates.";
pub const NOTHING_PICKED_RECEIVED: &str = "Pick a row to see when it arrived and where it went.";

/// Which of the two SHARING screens this pane is drawing, and -- for the
/// account's own Sends -- which of design 5b's sub-filters is in force.
///
/// **One enum rather than a `SendScope` plus a `bool`**, because the two
/// screens do not overlap: a scope is meaningless for `Shared with me`, and
/// the received rows are meaningless for the account's own Sends. Passed as
/// two parameters, every reader would have to remember which combinations are
/// real, and the combination that is not (`Received` with a scope) is exactly
/// the one that would silently draw a filtered list of the wrong things.
///
/// The received rows ride inside the variant that uses them for the same
/// reason: there is no way to be handed them and draw the other screen.
pub enum SendView<'a> {
    /// The account's own published Sends, cut to this scope.
    Mine(crate::vault_window::sidebar::SendScope),
    /// Design 5b's `Shared with me`: what other people sent this user, off
    /// the local record in [`crate::receive_history`].
    Received(&'a [ReceivedRow]),
}

impl SendView<'_> {
    /// The uppercase word over the list column.
    pub fn heading(&self) -> &'static str {
        match self {
            SendView::Mine(scope) => scope.eyebrow(),
            SendView::Received(_) => RECEIVED_HEADING,
        }
    }

    /// The sentence under the strip: what this app can do with a Send on the
    /// unfiltered screen, and what the sub-filter selects on a filtered one.
    fn gloss(&self) -> &'static str {
        match self {
            SendView::Mine(scope) => scope.gloss().unwrap_or(SCOPE_SUBTEXT),
            SendView::Received(_) => RECEIVED_SCOPE_SUBTEXT,
        }
    }
}

/// The `Shared with me` screen's own scope line, in [`SCOPE_SUBTEXT`]'s
/// spirit: it says where the feature lives rather than leaving its edge to be
/// discovered.
pub const RECEIVED_SCOPE_SUBTEXT: &str =
    "Read a Send from its link with Import a record. What you import is listed here; the record \
     itself lives in your vault.";

/// One line of the `Shared with me` list: an import that happened, as this
/// app is allowed to remember it.
///
/// Built by [`received_rows`] out of a [`crate::receive_history::ReceivedRecord`]
/// and the live vault, so the one derived fact -- whether the item is still
/// there -- is decided in a pure function rather than inside a drawing
/// closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedRow {
    /// What the detail pane selects on. The vault item's id where there is
    /// one, and a synthetic key off the timestamp where there is not -- see
    /// [`received_rows`].
    pub key: String,
    pub name: String,
    /// When it arrived, in the user's own day and time, with how long ago
    /// beside it.
    pub when: String,
    /// Whether the item this import created is still in the vault.
    pub still_in_vault: bool,
}

/// What the detail pane says about a received record whose item has gone.
///
/// **The history outlives the item, so this state is ordinary rather than
/// exceptional**, and saying nothing would leave a row that quietly points at
/// nothing. It names both innocent explanations, because both are common and
/// neither is a fault.
pub const RECEIVED_ITEM_GONE: &str =
    "The item this created is no longer in your vault -- it was deleted, or it is in the trash.";
/// Its opposite, said just as plainly so the two rows read as one question
/// answered two ways rather than as a warning and a silence.
pub const RECEIVED_ITEM_PRESENT: &str = "The item this created is still in your vault.";

/// Every recorded import, newest first, as rows.
///
/// `items` is the LIVE vault list, which is what makes `still_in_vault` mean
/// what it says: an item in the trash is not in that list, and the wording
/// [`RECEIVED_ITEM_GONE`] uses names that case rather than claiming the item
/// was destroyed.
///
/// A record written by a version that did not store an item id -- or by a
/// hand-edited file -- has an empty `item_id`, and gets a key off its
/// timestamp so the row is still selectable. It is reported as gone, which is
/// the honest reading: this app cannot find the item, and saying "still in
/// your vault" about an item it cannot name would be a guess in the
/// reassuring direction.
pub fn received_rows(
    history: &crate::receive_history::ReceiveHistory,
    items: &[crate::vault_bridge::VaultItem],
    now: &dyn SendClock,
    zone: &dyn LocalOffset,
) -> Vec<ReceivedRow> {
    history
        .entries
        .iter()
        .map(|record| ReceivedRow {
            key: if record.item_id.is_empty() {
                format!("received-{}", record.received_at_unix_millis)
            } else {
                record.item_id.clone()
            },
            name: if record.name.trim().is_empty() {
                "(no name)".to_string()
            } else {
                record.name.clone()
            },
            when: date_words(
                &iso_from_millis(record.received_at_unix_millis),
                now,
                zone,
                RECEIVED_WHEN_UNKNOWN,
            ),
            still_in_vault: !record.item_id.is_empty()
                && items.iter().any(|item| item.id == record.item_id),
        })
        .collect()
}

/// What a row says when its timestamp could not be read.
pub const RECEIVED_WHEN_UNKNOWN: &str = "Unknown";

/// A stored millisecond instant back into the one date shape this crate
/// parses.
///
/// **A round trip rather than a second formatter**, and that is the decision
/// worth writing down. `date_words` already turns the CLI's
/// `2026-08-18T00:43:17.148Z` into the sentence the detail pane prints, and a
/// second path from "a number" to that same sentence is a second thing that
/// can word a date differently. So the history's `i64` is rendered into the
/// wire shape and handed to the one reader, which is one conversion in a
/// direction that cannot lose anything: `parse_iso_utc_millis` is its exact
/// inverse, and `the_epoch_and_a_leap_day_round_trip` already pins that.
fn iso_from_millis(millis: i64) -> String {
    let parts = crate::local_time::civil_parts(millis);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        parts.year,
        parts.month,
        parts.day,
        parts.hour,
        parts.minute,
        parts.second,
        parts.millis,
    )
}

/// The rows of `state` that belong on `scope`, in the list's own order.
///
/// **A borrowing filter and not a second list**, so the pane cannot end up
/// holding rows that were derived at a different instant from the ones the
/// rail counted. The predicate is [`crate::vault_window::sidebar::SendScope::admits`]
/// -- the SAME function the badge counts through -- which is what stops a
/// sub-row saying 2 over a list of three.
pub fn rows_in_scope<'a>(
    rows: &'a [SendRow],
    scope: crate::vault_window::sidebar::SendScope,
) -> Vec<&'a SendRow> {
    rows.iter().filter(|row| scope.admits(row.state)).collect()
}

/// What the list column says when the ACCOUNT has Sends but none of them is
/// on the sub-row in force.
///
/// **Not [`EMPTY_HEADLINE`]**, and the separation is this file's oldest rule
/// applied to a third case: "you have no Sends" is a claim about the account,
/// "we could not ask" is a claim about this app, and this is a claim about
/// the filter the user themselves just chose. Printing the account sentence
/// here would tell somebody who clicked `Used` that they have published
/// nothing, which is false and alarming in the direction that matters.
pub fn scope_empty_headline(scope: crate::vault_window::sidebar::SendScope) -> String {
    format!("No Sends are {}.", scope.label().to_lowercase())
}

/// The one way back out of an empty sub-row, said rather than left to be
/// worked out.
pub const SCOPE_EMPTY_DETAIL: &str = "Your other Sends are still on the Sends row above.";

/// Draws the whole Sends screen -- design 5b's list column and detail pane --
/// and reports what was clicked.
///
/// # The layout, and how much of it is the vault's own
///
/// 5b's caption is `SAME LIST + DETAIL AS THE VAULT`, and the shape here is
/// literally that: an `egui::Panel::left` of `vault_window::LIST_WIDTH`
/// against a `theme::CANVAS` frame with no inner margin, exactly as the item
/// list's own panel is built, and the detail drawn on what is left. The panel
/// is nested inside the central panel rather than declared beside the item
/// list's, which is what keeps `vault_window::run`'s own panel census -- and
/// the source pin that requires `Panel::left("vault-item-list")` to appear
/// exactly once -- saying what they say today.
///
/// What is NOT shared is the drawing of a row: an item row is an icon, a
/// name, a username and a kebab over a `VaultItem`, and a Send row is an
/// initials tile, a name, a subtitle and a state pill over something that is
/// not a `VaultItem` and shares no id with one. Threading a second kind of
/// subject through `draw_item_list` would put a `match` on "which of two
/// unrelated things is this" inside every one of its cells, which is the
/// forcing-Sends-through-the-item-pane that this screen's rail flags exist to
/// prevent. The GEOMETRY is shared, by constant, and the contents are not.
///
/// # `notice` spans both columns
///
/// It is drawn above the panel, at the pane's full width, because it is a
/// message about the SCREEN and not about either column. Putting it inside
/// the list column would have squeezed a sentence into 390pt beside the
/// failure headline that is already there; putting it in the detail would
/// have made it disappear behind the composer.
///
/// `notice` is the message the window's single inline band is showing this
/// frame, already chosen by `vault_window::inline_notice` -- this function
/// does not decide which of the window's messages wins, it only paints the
/// one it is handed.
///
/// # Ten parameters, and the reason for the attribute
///
/// `view` and `selected` joined when the screen grew a detail pane: which of
/// the two SHARING screens is up and which of its rows the detail is
/// describing. Both are the window's state and not this pane's, for
/// `composer`'s reason -- a selection that reset every time the user glanced
/// at Password health would be a detail pane that could not be left and
/// returned to. Bundling them into a struct would move the argument count
/// rather than reduce what this function is handed.
#[allow(clippy::too_many_arguments)]
pub fn draw_send_pane(
    ui: &mut egui::Ui,
    state: &SendPaneState,
    notice: Option<&str>,
    delete: SendDeleteView<'_>,
    // Which SHARING screen, and which sub-filter. See [`SendView`].
    view: SendView<'_>,
    // **Which row the detail pane is describing.** `&mut` and the window's,
    // exactly like the item list's `selected_id`: the list column writes it,
    // the detail column resolves it, and a selection naming a row that is no
    // longer in the list draws the "nothing picked" pane rather than a stale
    // one.
    selected: &mut Option<String>,
    // **`&mut`, and the one place the draft lives.** The pane owns no state
    // of its own -- every other pane in this window is the same -- so the
    // half-typed name, body and share password are the window's, survive a
    // trip to another screen, and are wiped by exactly one thing: dropping
    // this value. See [`SendComposer`].
    composer: &mut SendComposer,
    // Whether a `bw send create` started from that draft is still running.
    creating: bool,
    // The clock every dated sentence on this screen is worded against. A
    // parameter for `crate::send`'s reason: nothing that decides what a date
    // SAYS may read the wall clock for itself, or the paint tests could only
    // assert the shape of the sentence and never its content.
    now: &dyn SendClock,
    // The machine's offset from UTC. A parameter beside `now`, and for
    // exactly the same reason: the detail pane's dates are the user's OWN
    // day, and a paint test that read the offset off the machine running it
    // would assert a different sentence on a runner in another timezone -- or
    // on the same runner in March.
    zone: &dyn LocalOffset,
) -> SendUiVerdict {
    let mut action = SendUiAction::None;

    // **The same sentence is never printed twice.** A failed fetch reaches
    // this function through both doors: `vault_window` turns it into the
    // window's inline notice (`NoticeSource::Sends`), and `pane_state` turns
    // the very same `SendError` into `Failed { message }` below -- both from
    // `SendError::user_message`. Handed both, the pane used to paint the
    // identical line in the band and again under the headline, which reads
    // as two failures.
    //
    // The list column's own rendering wins, because it is the richer one: it
    // has the headline, the "could not check" line for an ambiguous failure,
    // and Try again. A notice that is *not* the failure being drawn (a move
    // or generate error arriving while the list is up) is still shown.
    let notice = match (notice, state) {
        (Some(n), SendPaneState::Failed { message, .. }) if n == message => None,
        (n, _) => n,
    };
    if let Some(message) = notice {
        if draw_notice_band(ui, message) {
            action = SendUiAction::DismissNotice;
        }
    }

    egui::Panel::left("vault-send-list")
        .exact_size(crate::vault_window::LIST_WIDTH)
        .resizable(false)
        // NO INNER MARGIN, for the item list's own reason: this column is a
        // white strip spanning its full width over a list area with a
        // different padding beneath, and one panel margin cannot be both.
        .frame(egui::Frame::new().fill(theme::CANVAS))
        .show(ui, |ui| {
            if let Some(reported) = draw_send_list(ui, state, &view, delete, selected) {
                action = reported;
            }
        });

    if let Some(reported) =
        draw_send_detail(ui, state, &view, delete, selected, composer, creating, now, zone)
    {
        // **The detail column wins an ambiguous frame.** Every destructive
        // control on this screen is in it, and the list column's own
        // controls -- Refresh, New Send, and picking a row -- are the ones a
        // user cannot mean by accident to have swallowed. Same rule as
        // `draw_row`'s old ordering, one layer out.
        action = reported;
    }

    SendUiVerdict::seal(action)
}

/// Design 5b's list column: a strip that names the cut in force, the sentence
/// under it, and the rows.
///
/// Returns the action **the strip** reported. Picking a row is not an action:
/// it is a write to `selected`, exactly as an item row writes `selected_id`,
/// because there is nothing for `vault_window` to do about it.
fn draw_send_list(
    ui: &mut egui::Ui,
    state: &SendPaneState,
    view: &SendView<'_>,
    delete: SendDeleteView<'_>,
    selected: &mut Option<String>,
) -> Option<SendUiAction> {
    let mut action: Option<SendUiAction> = None;
    let width = ui.available_width();

    // How many rows this column is about to draw, decided BEFORE the strip so
    // the strip can say it. `None` where the count is not a fact this app has
    // -- an unanswered or failed fetch -- which is the same rule the rail's
    // badge follows, on the same list.
    let counted: Option<usize> = match (view, state) {
        (SendView::Received(rows), _) => Some(rows.len()),
        (SendView::Mine(scope), SendPaneState::Rows(rows)) => {
            Some(rows_in_scope(rows, *scope).len())
        }
        (SendView::Mine(_), SendPaneState::Empty) => Some(0),
        (SendView::Mine(_), _) => None,
    };

    // --- the strip ------------------------------------------------------
    let (strip, _) =
        ui.allocate_exact_size(egui::vec2(width, LIST_STRIP_HEIGHT), egui::Sense::hover());
    ui.painter().rect_filled(strip, CornerRadius::ZERO, theme::CARD);
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(strip.left(), strip.bottom() - 1.0),
            egui::pos2(strip.right(), strip.bottom()),
        ),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );
    let eyebrow = ui.painter().layout_job(theme::letterspaced(
        view.heading(),
        theme::EYEBROW_PX,
        theme::BOLD,
        theme::EYEBROW_TRACKING,
        theme::TEXT_MUTED,
    ));
    let eyebrow_size = eyebrow.size();
    ui.painter().galley(
        egui::pos2(strip.left() + STRIP_PAD_X, strip.center().y - eyebrow_size.y / 2.0),
        eyebrow,
        theme::TEXT_MUTED,
    );
    // The count, beside the word it counts. `badge_text` rather than a local
    // `match`, so an unanswered list draws the rail's own en dash here too
    // and the two readouts cannot disagree about what "not known" looks like.
    //
    // **`Shared with me` counts RECORDS, not sends.** Those rows are imports
    // this app wrote down; calling them "3 sends" beside the heading SHARED
    // WITH ME says the user published three, which is the opposite of what
    // that screen is about. It is the kind of slip a picture catches and a
    // rect assertion never can.
    let (one, many) = match view {
        SendView::Received(_) => (RECORD_NOUN, RECORDS_NOUN),
        SendView::Mine(_) => (SEND_NOUN, SENDS_NOUN),
    };
    let noun = match counted {
        Some(1) => format!("1 {one}"),
        Some(n) => format!("{n} {many}"),
        None => format!("{} {many}", crate::vault_window::sidebar::UNKNOWN_COUNT),
    };
    ui.painter().text(
        egui::pos2(strip.left() + STRIP_PAD_X + eyebrow_size.x + STRIP_GAP, strip.center().y),
        egui::Align2::LEFT_CENTER,
        &noun,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
        theme::TEXT_GHOST,
    );
    // Refresh and New Send live in the strip, and **Refresh is drawn in every
    // state including `Failed`**: a screen whose only way back from an error
    // is to navigate away and return is a screen that tells the user the
    // error is permanent.
    //
    // Neither is drawn on `Shared with me`. Refresh would re-ask a question
    // that screen does not ask -- its rows come from a local file, not a
    // fetch -- and a control that visibly does nothing is worse than one that
    // is not there.
    //
    // **One of them is the primary and the other is not, which is the whole
    // of what changed here.** Both used to be bare `egui::Button`s at 12px in
    // egui's own grey, and laid beside the item list -- the column this one
    // swaps places with -- the difference is not subtle: that column's strip
    // carries a search field and ONE solid blue `+ New`, and this one carried
    // two identical grey boxes taking up half its width and outweighing every
    // row beneath them. A strip with two equal emphases has no emphasis.
    //
    // New Send is the primary because it is what the screen is for. Refresh
    // is `Secondary` and not quieter still, because it is the only way back
    // from a stale list and 5b's own strip keeps a control in that corner.
    if let SendView::Mine(_) = view {
        let slot = |right: f32, w: f32| {
            egui::Rect::from_min_size(
                egui::pos2(right - w, strip.center().y - theme::ACTION_BUTTON_HEIGHT / 2.0),
                egui::vec2(w, theme::ACTION_BUTTON_HEIGHT),
            )
        };
        let refresh_w =
            theme::action_button_width(ui.painter(), REFRESH_LABEL, STRIP_BUTTON_PAD_X);
        let new_w = theme::action_button_width(ui.painter(), NEW_SEND_LABEL, STRIP_BUTTON_PAD_X);
        let refresh_rect = slot(strip.right() - STRIP_PAD_X, refresh_w);
        if theme::action_button(ui, refresh_rect, REFRESH_LABEL, theme::ActionTone::Secondary)
            .clicked()
        {
            action = Some(SendUiAction::Refresh);
        }
        let new_rect = slot(refresh_rect.left() - BUTTON_GAP, new_w);
        if theme::action_button(ui, new_rect, NEW_SEND_LABEL, theme::ActionTone::Primary)
            .clicked()
        {
            action = Some(SendUiAction::OpenComposer);
        }
    }

    // The strip's two controls are `put` INSIDE the band allocated above, and
    // `Ui::put` advances the cursor to the rect it drew at -- backwards, in
    // this case, from the strip's foot to the buttons'. Everything below is
    // laid out from the cursor, so it is put back where the strip left it.
    // See the same note in `draw_send_card`.
    ui.advance_cursor_after_rect(strip);

    // --- the standing explanations, UNDER the column --------------------
    //
    // **They used to sit between the strip and the first row, and that is the
    // change here.** Two wrapped grey paragraphs above the list came to some
    // seventy points of prose at the top of a 390pt column: the first thing
    // the eye met on this screen was a help note, and the rows -- the whole
    // subject of the column -- began a third of the way down it. Design 5b's
    // list column goes strip, then rows, with nothing in between, and the
    // item list this column swaps places with does the same.
    //
    // They are not deleted, because both are true and both are things a user
    // has to be told: this app cannot make a file Send, and it cannot edit
    // one. They are demoted to a footnote, which is what a standing statement
    // about a feature's edges is. The rail already does exactly this with
    // "Locks in 11:42" -- a line the column always carries, at the bottom,
    // at 11px in `TEXT_GHOST` -- so this is the window's own existing rule for
    // "true, and not what you came here for", not a new one invented to get
    // the prose out of the way.
    let text_width = (width - LIST_PAD * 2.0).max(0.0);
    // The `FILE` explanation only when there is a tag on screen to explain.
    // The gloss above says what this app does with Sends; this says what the
    // tag on a row in front of the user means, and a list with no such row
    // has no tag to explain.
    let file_note = matches!(view, SendView::Mine(scope)
        if matches!(state, SendPaneState::Rows(rows)
            if rows_in_scope(rows, *scope).iter().any(|r| r.is_file)));
    //
    // **The notes are LAID OUT before the panel is opened, and the panel is
    // given their exact height.** A `Panel::bottom` left to size itself takes
    // its height from what it drew, which is a frame late: on the first frame
    // of a screen it uses a default and clips whatever does not fit. That is
    // not a test artefact -- it is what a user sees the first time they open
    // this screen -- and it is why the second note went missing under a paint
    // harness that draws one frame. Measuring first is this file's standing
    // discipline for exactly this class of bug.
    let note_font = egui::FontId::new(NOTE_PX, egui::FontFamily::Proportional);
    let mut notes: Vec<std::sync::Arc<egui::Galley>> = vec![ui.painter().layout(
        view.gloss().to_string(),
        note_font.clone(),
        theme::TEXT_GHOST,
        text_width,
    )];
    if file_note {
        notes.push(ui.painter().layout(
            FILE_SEND_EXPLANATION.to_string(),
            note_font,
            theme::TEXT_GHOST,
            text_width,
        ));
    }
    let notes_height: f32 = notes.iter().map(|g| g.size().y).sum::<f32>()
        + NOTE_GAP * (notes.len() - 1) as f32;
    egui::Panel::bottom("send-list-note")
        .exact_size(LIST_PAD * 3.0 + NOTE_RULE + notes_height)
        .resizable(false)
        .frame(egui::Frame::new().fill(theme::CANVAS))
        .show_separator_line(false)
        .show(ui, |ui| {
            let area = ui.available_rect_before_wrap();
            let p = ui.painter();
            // A hairline over the footnote, so it reads as a note under the
            // column rather than as a sentence that has come adrift from the
            // last row. The rail's own footer does not need one because the
            // rail's rows end in a flexed gap; a scrolled list ends wherever
            // the scrolling stopped.
            p.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(area.left() + LIST_PAD, area.top() + LIST_PAD),
                    egui::vec2((area.width() - LIST_PAD * 2.0).max(0.0), NOTE_RULE),
                ),
                CornerRadius::ZERO,
                theme::HAIRLINE,
            );
            let mut y = area.top() + LIST_PAD + NOTE_RULE + LIST_PAD;
            for galley in notes {
                let height = galley.size().y;
                p.galley(egui::pos2(area.left() + LIST_PAD, y), galley, theme::TEXT_GHOST);
                y += height + NOTE_GAP;
            }
        });

    // --- the rows -------------------------------------------------------
    ui.add_space(LIST_PAD);
    let row_width = (width - LIST_PAD * 2.0).max(0.0);
    let rows_area = |ui: &mut egui::Ui, body: &mut dyn FnMut(&mut egui::Ui)| {
        egui::ScrollArea::vertical()
            .id_salt("send-list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = LIST_ROW_GAP;
                ui.add_space(0.0);
                body(ui);
            });
    };
    let headline = |ui: &mut egui::Ui, head: &str, detail: &str, colour: egui::Color32| {
        ui.add_space(LIST_PAD);
        ui.indent("send-empty", |ui| {
            ui.set_width(text_width);
            // [`ANSWER_LINE_PX`] through `theme::semibold`, not `size(14).
            // strong()`. `RichText::strong` is an EGUI treatment -- it swaps
            // in `visuals.strong_text_color` and leaves the face alone -- so
            // this line was the app's body face in a slightly darker grey
            // while every other headline in the window is Archivo SemiBold.
            // It is the same class of drift as the row's plain-face name.
            ui.label(theme::semibold(head, ANSWER_LINE_PX).color(colour));
            ui.add_space(4.0);
            ui.label(egui::RichText::new(detail).size(12.0).color(theme::TEXT_MUTED));
        });
    };

    match view {
        SendView::Received(rows) => {
            if rows.is_empty() {
                headline(ui, RECEIVED_EMPTY_HEADLINE, RECEIVED_EMPTY_DETAIL, theme::INK);
            } else {
                rows_area(ui, &mut |ui| {
                    for row in rows.iter() {
                        draw_received_row(ui, row, row_width, selected);
                    }
                });
            }
        }
        SendView::Mine(scope) => match state {
            SendPaneState::Loading => {
                ui.add_space(LIST_PAD);
                ui.indent("send-loading", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0).color(theme::BLUE));
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(LOADING_LABEL).size(13.0).color(theme::TEXT_FAINT),
                        );
                    });
                });
            }
            SendPaneState::Empty => headline(ui, EMPTY_HEADLINE, EMPTY_DETAIL, theme::INK),
            SendPaneState::Failed { message, ambiguous } => {
                // A failure draws NO row area at all, and never reuses the
                // empty state's words. Both are asserted over painted glyphs.
                ui.add_space(LIST_PAD);
                ui.indent("send-failed", |ui| {
                    ui.set_width(text_width);
                    ui.label(
                        egui::RichText::new(FAILED_HEADLINE)
                            .size(14.0)
                            .color(theme::ERROR)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(message.as_str()).size(12.0).color(theme::TEXT_MUTED),
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
                    // The design system's secondary, like every other control
                    // on this screen now. It used to be a bare `egui::Button`
                    // at 12px -- one more place where a screen built out of
                    // egui defaults looked like it came from a different
                    // application than the one around it.
                    if theme::secondary_button(ui, TRY_AGAIN_LABEL).clicked() {
                        action = Some(SendUiAction::Refresh);
                    }
                });
            }
            SendPaneState::Rows(rows) => {
                let in_scope = rows_in_scope(rows, *scope);
                if in_scope.is_empty() {
                    // The account has Sends; this sub-row has none. A third
                    // sentence, deliberately -- see `scope_empty_headline`.
                    let head = scope_empty_headline(*scope);
                    headline(ui, &head, SCOPE_EMPTY_DETAIL, theme::INK);
                } else {
                    rows_area(ui, &mut |ui| {
                        for row in &in_scope {
                            draw_row(ui, row, row_width, delete, selected);
                        }
                    });
                }
            }
        },
    }

    action
}

/// The list column's strip: design 5b's `padding: 12px` on a 12px line, with
/// room for the 26pt controls that sit in it.
///
/// **56 and not 38**, and the arithmetic is the page's content box. `padding:
/// 12px` around a 12px line box (~15pt drawn) is 12 + 15 + 12 = 39 plus the
/// 1px bottom rule, which is what a strip of TEXT would be; this strip also
/// holds two `theme::ACTION_BUTTON_HEIGHT` controls, and 12 + 32 + 12 = 56 is
/// the same padding around the taller thing in it. Sizing it to the text and
/// letting the buttons overhang is the slip this file has paid for before.
///
/// It was 50 while those controls were 26pt boxes of egui's own making. They
/// come from the design system now -- see `theme::action_button` -- and the
/// design system's action is 32, so the strip around them grew by the same
/// six points rather than cropping them.
const LIST_STRIP_HEIGHT: f32 = 56.0;
/// Design 5b's `padding: 12px` on the list strip.
const STRIP_PAD_X: f32 = 12.0;
/// The gap between the strip's eyebrow and the count beside it: 5b's own
/// `gap: 10px`.
const STRIP_GAP: f32 = 10.0;
/// Design 5b's `padding: 10px` on the list column's row container.
const LIST_PAD: f32 = 10.0;
/// Design 5b's `gap: 6px` between two rows.
const LIST_ROW_GAP: f32 = 6.0;
/// The list column's footnote: the rail's own footer size, because it is the
/// same kind of line in the same kind of place. See the footnote's own
/// argument in [`draw_send_list`].
const NOTE_PX: f32 = 11.0;
/// The gap between the two footnotes, and the hairline above them.
const NOTE_GAP: f32 = 3.0;
const NOTE_RULE: f32 = 1.0;

/// What the strip's count counts, on each of the two screens this pane draws.
///
/// Four constants rather than two `format!`s with the word inlined, because
/// the pair that is easy to get wrong is the SINGULAR: a count line reading
/// "1 sends" is the defect this shape exists to make impossible to write.
pub const SEND_NOUN: &str = "send";
pub const SENDS_NOUN: &str = "sends";
pub const RECORD_NOUN: &str = "record";
pub const RECORDS_NOUN: &str = "records";
/// The padding inside the strip's two controls.
///
/// **12 and not `theme::ACTION_BUTTON_PAD_X`'s 14**, deliberately: this strip
/// carries a heading and a count as well as the pair, and the two buttons in
/// 5b's own detail header have a whole row to themselves. The height, the
/// radius and the face are the design system's; only the padding is tightened
/// for the narrower home, which is the one dimension
/// `theme::action_button_width` takes as a parameter and argues about.
const STRIP_BUTTON_PAD_X: f32 = 12.0;
/// The list column's Refresh. Named because the whole-window matrix presses
/// it, and a bare literal there was one rewording away from pressing nothing.
pub const REFRESH_LABEL: &str = "Refresh";
/// The failure's own retry, which is a different control in a different place
/// from [`REFRESH_LABEL`] and deliberately not the same word: this one sits
/// under a sentence explaining what went wrong, and "Refresh" under an error
/// reads as a suggestion that nothing did.
pub const TRY_AGAIN_LABEL: &str = "Try again";

/// One Send in the list column. **No controls at all**, which is design 5b's
/// own row and the resolution of a departure this file used to carry.
///
/// # The pill is right-aligned now, and the departure is gone
///
/// The note that used to sit here said the pill led the second line "and the
/// design right-aligns it", because Copy link and Delete already owned this
/// row's right edge and a pill placed in that column would be pushed off the
/// pane at `MIN_VAULT_WINDOW_SIZE` or would move every time the row changed
/// mode. Both halves of that argument were about controls that are no longer
/// here: 5b puts Revoke in the detail pane, this screen now has one, and the
/// row's right edge is free. So the pill is where the design puts it, the
/// departure is deleted rather than restated, and the row has nothing in it
/// that can be pushed off by a narrower window -- the name and subtitle are
/// clipped against the pill's left edge instead.
///
/// # Picking is the row's whole interaction
///
/// The entire band is one click target, like an item row, and what it does is
/// write `selected`. Nothing destructive is reachable from here at all, which
/// is worth more than the click it saves: the row that is under the pointer
/// while a list refreshes is not the row the user was reading a moment ago,
/// and this screen's one unrecoverable action used to be two pixels from it.
///
/// The one state the row still shows for itself is a revoke in flight, in
/// place of its subtitle. A Send being deleted while the user looks at
/// another one would otherwise change nothing anywhere they can see.
fn draw_row(
    ui: &mut egui::Ui,
    row: &SendRow,
    width: f32,
    delete: SendDeleteView<'_>,
    selected: &mut Option<String>,
) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let picked = selected.as_deref() == Some(row.id.as_str()) && !row.id.is_empty();
    if response.clicked() && !row.id.is_empty() {
        *selected = Some(row.id.clone());
    }

    // **Design 5b's `opacity: 0.72`, and it is a property of the ROW.**
    //
    // 5b draws its Expired and Revoked rows at 72% of everything -- tile,
    // name, subtitle, border and pill together -- and that is what makes its
    // list read as a list rather than as five identical bands: the links that
    // are still live come forward because the ones that are over have
    // stepped back. Drawn at full strength, as this row was, "Remote Desktop
    // -- Bastion" has exactly the same weight as the Send somebody is about
    // to open, and the column has no shape at all.
    //
    // **Except when it is picked**, which 5b never had to decide because 5b's
    // picked row happens to be live. A receded row whose detail pane is up is
    // a row the user is looking AT, and fading the one thing they selected
    // reads as the app having lost track of it. So a selection restores the
    // row to full strength; nothing else does.
    let ended = matches!(
        row.state,
        crate::send::SendState::Expired | crate::send::SendState::Revoked
    );
    let fade = if ended && !picked { theme::ENDED_ROW_OPACITY } else { 1.0 };
    let dim = |colour: egui::Color32| theme::faded(colour, fade);

    let painter = ui.painter();
    // **The design's selected-row shadow, from `theme` and not from a second
    // spelling.** The item list has drawn one under its picked row since
    // design 2b; this row, rebuilt by hand, had none -- so two rows in the
    // same column of the same window lifted differently under the same
    // gesture. See `theme::SELECTED_ROW_SHADOW`.
    if picked {
        painter.add(
            theme::SELECTED_ROW_SHADOW.as_shape(rect, CornerRadius::same(ROW_RADIUS)),
        );
    }
    painter.rect_filled(rect, CornerRadius::same(ROW_RADIUS), theme::CARD);
    painter.rect_stroke(
        rect,
        CornerRadius::same(ROW_RADIUS),
        egui::Stroke::new(1.0, dim(if picked { theme::BLUE } else { theme::HAIRLINE })),
        egui::StrokeKind::Inside,
    );

    let small_font = egui::FontId::new(11.0, egui::FontFamily::Proportional);

    // The pill first: it is right-aligned, and its measured width is what
    // bounds how far the name and the subtitle may run. Measured through
    // `state_pill_width` before anything is drawn, which is the same
    // reserve-then-fill order every control on this row used to follow and
    // the reason none of them was ever pushed off the pane.
    //
    // The tone is faded with the rest of the row, through `theme::faded_pill`
    // rather than by picking paler colours: an `Expired` pill in a fifth set
    // of greys would be a fifth thing to keep true, and the design's rule is
    // one opacity over the whole band.
    let tone = theme::faded_pill(state_tone(row.state), fade);
    let label = state_label(row.state);
    let pill_width = theme::state_pill_width(painter, tone, label);
    theme::state_pill(
        painter,
        egui::pos2(rect.right() - ROW_PAD_X - pill_width, rect.center().y),
        tone,
        label,
    );
    let text_right = rect.right() - ROW_PAD_X - pill_width - ROW_TEXT_GAP;

    // The initials tile. Painted rather than `theme::avatar`ed, for this
    // file's standing reason: every cell of this row goes into an explicit
    // rectangle against a painter, because a nested layout is what has
    // repeatedly pushed something off this pane.
    let tile = egui::Rect::from_min_size(
        egui::pos2(rect.left() + ROW_PAD_X, rect.center().y - ROW_TILE / 2.0),
        egui::Vec2::splat(ROW_TILE),
    );
    paint_tile_faded(painter, tile, &row.name, picked, 12.0, fade);

    let text_left = tile.right() + ROW_TEXT_GAP;
    let text_area = egui::Rect::from_min_max(
        egui::pos2(text_left, rect.top()),
        egui::pos2(text_right.max(text_left), rect.bottom()),
    );
    let clip = painter.with_clip_rect(text_area.intersect(ui.clip_rect()));

    let name_rect = clip.text(
        egui::pos2(text_area.left(), rect.center().y - 2.0),
        egui::Align2::LEFT_BOTTOM,
        &row.name,
        // **`SEMIBOLD` unselected, not the plain proportional face**, which is
        // the single most visible way this row differed from the item row it
        // shares a column with. 5b's row name is `font-weight: 600` and
        // `item_list`'s is `theme::semibold` at the same 13px; this one was
        // `FontFamily::Proportional`, i.e. Regular. Laid side by side, the
        // vault's names read as the subjects of their rows and the Sends'
        // read as captions -- with every rectangle on both rows measured
        // identically. A weight is not a measurement, which is exactly why
        // no rect assertion ever saw it.
        egui::FontId::new(
            13.0,
            egui::FontFamily::Name(if picked { theme::BOLD } else { theme::SEMIBOLD }.into()),
        ),
        dim(if picked { theme::BLUE_DEEP } else { theme::INK }),
    );
    if row.is_file {
        // Beside the name, never instead of the row. See the module docs: an
        // unlisted public link is one the user cannot revoke from here and
        // does not know is there.
        let tag_rect = clip.text(
            egui::pos2(name_rect.right() + 10.0 + TAG_PAD_X, name_rect.center().y),
            egui::Align2::LEFT_CENTER,
            FILE_TAG,
            small_font.clone(),
            dim(theme::TEXT_MUTED),
        );
        clip.rect_stroke(
            tag_rect.expand2(egui::vec2(TAG_PAD_X, 2.0)),
            CornerRadius::same(4),
            egui::Stroke::new(1.0, dim(theme::HAIRLINE)),
            egui::StrokeKind::Outside,
        );
    }

    // The subtitle, or the progress word while this row's own `bw send
    // delete` runs. Replaced rather than joined, for the reason the row's
    // second line has always been replaced: a row that says both "3 of 10
    // views" and "Revoking..." is a row whose subject is ambiguous at the
    // moment it matters most.
    let revoking = delete.in_flight == Some(row.id.as_str()) && !row.id.is_empty();
    // **`TEXT_FAINT`, not `TEXT_GHOST`.** 5b's row subtitle is `#7d7979`,
    // which is `TEXT_FAINT`; `TEXT_GHOST` (`#9b9797`) is the design's colour
    // for a COUNT beside a rail row, not for the line that says what a Send
    // is. The item row beside this one has always used `TEXT_FAINT` for its
    // username, so this was a second way the two columns disagreed about what
    // a secondary line looks like -- one step fainter, on every row, with the
    // name one weight lighter above it.
    let (second, colour) = if revoking {
        (DELETING_LABEL, theme::TEXT_MUTED)
    } else {
        (row.expiry.as_str(), theme::TEXT_FAINT)
    };
    clip.text(
        egui::pos2(text_area.left(), rect.center().y + 3.0),
        egui::Align2::LEFT_TOP,
        second,
        small_font,
        dim(colour),
    );
}

/// One `Shared with me` row. The same band, the same tile and the same
/// picking as [`draw_row`], with no state pill: a received record has no
/// lifetime this app can read -- it is a note that an import happened -- and
/// a pill here would be a state invented to fill the column.
fn draw_received_row(
    ui: &mut egui::Ui,
    row: &ReceivedRow,
    width: f32,
    selected: &mut Option<String>,
) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let picked = selected.as_deref() == Some(row.key.as_str());
    if response.clicked() {
        *selected = Some(row.key.clone());
    }
    let painter = ui.painter();
    // The same selection treatment the Send row and the item row wear; see
    // `theme::SELECTED_ROW_SHADOW`.
    if picked {
        painter.add(
            theme::SELECTED_ROW_SHADOW.as_shape(rect, CornerRadius::same(ROW_RADIUS)),
        );
    }
    painter.rect_filled(rect, CornerRadius::same(ROW_RADIUS), theme::CARD);
    painter.rect_stroke(
        rect,
        CornerRadius::same(ROW_RADIUS),
        egui::Stroke::new(1.0, if picked { theme::BLUE } else { theme::HAIRLINE }),
        egui::StrokeKind::Inside,
    );
    let tile = egui::Rect::from_min_size(
        egui::pos2(rect.left() + ROW_PAD_X, rect.center().y - ROW_TILE / 2.0),
        egui::Vec2::splat(ROW_TILE),
    );
    paint_tile(painter, tile, &row.name, picked, 12.0);
    let text_area = egui::Rect::from_min_max(
        egui::pos2(tile.right() + ROW_TEXT_GAP, rect.top()),
        egui::pos2(rect.right() - ROW_PAD_X, rect.bottom()),
    );
    let clip = painter.with_clip_rect(text_area.intersect(ui.clip_rect()));
    // `SEMIBOLD`/`TEXT_FAINT`, exactly as `draw_row` above and as the item
    // row beside both: a received record is a row in the same column, and
    // there is no reason for it to be typeset a third way.
    clip.text(
        egui::pos2(text_area.left(), rect.center().y - 2.0),
        egui::Align2::LEFT_BOTTOM,
        &row.name,
        egui::FontId::new(
            13.0,
            egui::FontFamily::Name(if picked { theme::BOLD } else { theme::SEMIBOLD }.into()),
        ),
        if picked { theme::BLUE_DEEP } else { theme::INK },
    );
    clip.text(
        egui::pos2(text_area.left(), rect.center().y + 3.0),
        egui::Align2::LEFT_TOP,
        &row.when,
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
        theme::TEXT_FAINT,
    );
}

/// The initials tile design 5b puts in front of every row and at the head of
/// the detail pane.
///
/// `theme::initials` is the app's own letter pair -- the same one the picker
/// and the item list fall back to -- so a Send called "Remote Desktop —
/// Bastion" reads `RD` here exactly as it would anywhere else.
fn paint_tile(
    painter: &egui::Painter,
    tile: egui::Rect,
    name: &str,
    emphasised: bool,
    text_px: f32,
) {
    paint_tile_faded(painter, tile, name, emphasised, text_px, 1.0);
}

/// [`paint_tile`], at design 5b's row opacity.
///
/// A separate entry point rather than a sixth argument on every call site,
/// because exactly one caller has a faded row and the other three are the
/// detail headers, which never do. See [`theme::ENDED_ROW_OPACITY`].
fn paint_tile_faded(
    painter: &egui::Painter,
    tile: egui::Rect,
    name: &str,
    emphasised: bool,
    text_px: f32,
    opacity: f32,
) {
    let (fill, edge, ink) = if emphasised {
        (theme::BLUE_WASH, theme::BLUE_EDGE, theme::BLUE)
    } else {
        (theme::CANVAS, theme::HAIRLINE, theme::TEXT_MUTED)
    };
    let dim = |colour: egui::Color32| theme::faded(colour, opacity);
    let radius = CornerRadius::same((tile.width() / 4.0).round() as u8);
    painter.rect_filled(tile, radius, dim(fill));
    painter.rect_stroke(
        tile,
        radius,
        egui::Stroke::new(1.0, dim(edge)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        tile.center(),
        egui::Align2::CENTER_CENTER,
        theme::initials(name),
        egui::FontId::new(text_px, egui::FontFamily::Name(theme::BOLD.into())),
        dim(ink),
    );
}

/// Row geometry, from design 5b's own CSS and measured as a border box.
///
/// The row declares `padding: 10px 12px` inside a `1px` border around a
/// 32pt tile, so 1 + 10 + 32 + 10 + 1 = 54. The two text lines it holds
/// beside the tile come to the same 32 (a 13px name's ~16pt line box, the
/// design's 3px gap, an 11px subtitle's ~13pt box), which is why the tile and
/// the text block are both simply centred on the band.
const ROW_HEIGHT: f32 = 54.0;
/// 5b's `padding: ... 12px`, and `border-radius: 10px`.
const ROW_PAD_X: f32 = 12.0;
const ROW_RADIUS: u8 = 10;
/// The tile, and 5b's `gap: 11px` beside it -- also the gap between the text
/// block and the state pill, so the row has one rhythm rather than two.
const ROW_TILE: f32 = 32.0;
const ROW_TEXT_GAP: f32 = 11.0;
/// The gap between two controls in a row.
const BUTTON_GAP: f32 = 8.0;
/// The gap between the row's name and its FILE tag, and between the tag's
/// text and its own outline.
const TAG_PAD_X: f32 = 6.0;

/// Design 5b's detail pane, and the half of 5c that this client can stand
/// behind.
///
/// # What is here
///
/// A header strip -- the initials tile, the name, what kind of Send it is and
/// its state pill -- over an action row, over the link's own facts: the
/// address, what opening it needs, the views against their cap, and BOTH
/// dates. 5c's opening claim, "whether it was used is the question people
/// actually have", is answered on the first line of the body in words rather
/// than left to a pill.
///
/// # What is deliberately NOT here, and why it is not an empty box
///
/// **5c's per-access timeline.** Its entries read `Password revealed - 15:01
/// - Edge on Windows - Berlin, DE - 84.13....` and there is no per-access
/// record in a Bitwarden Send at all: no time of any particular open, no user
/// agent, no address, no geolocation. Having them would mean the owner's
/// server logging every access with an IP, which is their decision about
/// their service and not a gap in this screen.
///
/// So there is **no Activity card**. A card headed `ACTIVITY` with nothing
/// under it is worse than its absence twice over: it reads as a load that
/// failed, and it promises a feature that no amount of work on this screen
/// can deliver. What stands in its place is [`SendRow::activity`], one
/// sentence carrying the part of 5c's premise that IS derivable -- how many
/// opens happened and what is left -- positioned where 5c put its headline.
///
/// **5b's `Recipient` row and its `Extend` button** are gone for the same
/// kind of reason and are likewise not stubbed: this client neither sets nor
/// reads a Send's `emails`, so there is no recipient to name, and it has no
/// edit path, so an `Extend` control would be a button that cannot do its
/// job. The `Opens with` row takes the recipient row's place because it
/// answers the same question the design was really asking there -- what does
/// somebody need in order to open this -- out of a field this client does
/// hold.
///
/// **5b's `created` date** is not in `SendSummary` and is not invented. The
/// subtitle says what kind of Send it is instead.
fn draw_send_detail(
    ui: &mut egui::Ui,
    state: &SendPaneState,
    view: &SendView<'_>,
    delete: SendDeleteView<'_>,
    selected: &mut Option<String>,
    composer: &mut SendComposer,
    creating: bool,
    now: &dyn SendClock,
    zone: &dyn LocalOffset,
) -> Option<SendUiAction> {
    let mut action: Option<SendUiAction> = None;

    // **The composer takes this column, exactly as the vault's edit form
    // takes its detail pane.**
    //
    // It used to be a card stacked ABOVE the rows, and the argument for that
    // was written down and was right at the time: with one column, a form
    // that took the pane would have made "there is a draft open" a state in
    // which the rest of the feature silently did not exist. 5b's caption says
    // SAME LIST + DETAIL AS THE VAULT, and the vault answers this exact
    // question already -- `DetailMode::Edit` replaces the read pane and
    // leaves the item list whole -- so the shape is taken from there.
    //
    // **What that costs, stated rather than glossed.** While a draft is open
    // the per-Send controls are behind it: the list is fully live, every row
    // is still pickable and Refresh is still in the strip, but Copy link, the
    // switch and Delete are one Discard away rather than on screen. That is a
    // real subtraction from the old arrangement and it is accepted for two
    // reasons. The draft is a secret being typed into three fields and a
    // dropdown, and 390pt is not a form column -- crowding is what made the
    // lifetime picker need a fixed box in the first place. And the old
    // sentence's real worry does not apply: nothing is INERT here. Every
    // control the list column has answers every press it ever did, and the
    // way back to the others is the form's own Discard.
    //
    // The whole-window matrix drives that rather than skipping it: its
    // `ComposerOpen` state presses Discard and requires the form to go before
    // it presses the per-Send controls -- one control more than it drove
    // before, and precisely the one a gate on `composer.open` would kill.
    if composer.open {
        return egui::Frame::new()
            .inner_margin(Margin::symmetric(DETAIL_PAD_X, DETAIL_PAD_Y))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("send-composer")
                    .auto_shrink([false, false])
                    .show(ui, |ui| draw_composer(ui, composer, creating, now, zone))
                    .inner
            })
            .inner;
    }

    match view {
        SendView::Mine(scope) => {
            let rows: &[SendRow] = match state {
                SendPaneState::Rows(rows) => rows,
                _ => &[],
            };
            let in_scope = rows_in_scope(rows, *scope);
            // Resolved against the rows IN SCOPE rather than against the
            // whole list, so a selection made on one sub-row and still held
            // after the user clicks another cannot describe a Send that is
            // not in the column beside it. The pane then reads as "nothing
            // picked", which is what is true.
            let picked = selected
                .as_deref()
                .and_then(|id| in_scope.iter().copied().find(|row| row.id == id));
            match picked {
                // **An instruction is only drawn when it can be followed.**
                // "Pick a Send to see its link" beside a column that says
                // "You have no Sends" is a direction to do something the
                // screen has just said is impossible, and it was the loudest
                // thing on that screen. With no rows in scope the list
                // column's own sentence is the whole answer and this column
                // stays out of its way. It is not a blank pane for a blank
                // pane's sake -- the reason `detail_prompt` exists at all is
                // that a blank column beside a FULL list reads as a load that
                // failed, and that case is unchanged.
                None if in_scope.is_empty() => {}
                None => detail_prompt(ui, NOTHING_PICKED),
                Some(row) => action = draw_send_card(ui, row, delete),
            }
        }
        SendView::Received(rows) => {
            let picked =
                selected.as_deref().and_then(|key| rows.iter().find(|row| row.key == key));
            match picked {
                // The same rule as the SHARING screen's: nothing to pick,
                // nothing telling you to pick it.
                None if rows.is_empty() => {}
                None => detail_prompt(ui, NOTHING_PICKED_RECEIVED),
                Some(row) => draw_received_detail(ui, row),
            }
        }
    }
    action
}

/// The detail column with nothing picked. **Words on the canvas, not a blank
/// column**; see [`NOTHING_PICKED`].
fn detail_prompt(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(DETAIL_PAD_X, DETAIL_PAD_Y))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(13.0).color(theme::TEXT_FAINT));
        });
}

/// The whole detail for one Send: header strip, actions, and the link's
/// facts. Returns the action the actions row reported.
fn draw_send_card(
    ui: &mut egui::Ui,
    row: &SendRow,
    delete: SendDeleteView<'_>,
) -> Option<SendUiAction> {
    let mut action: Option<SendUiAction> = None;
    let width = ui.available_width();

    // The three states, decided ONCE and here, exactly as the row used to
    // decide them: `is` compares the id this pane is describing with the id
    // the window is holding, so a confirmation raised for one Send cannot be
    // answered while another is on screen.
    let has_id = !row.id.is_empty();
    let revoking = has_id && delete.in_flight == Some(row.id.as_str());
    let confirming = has_id && !revoking && delete.confirming == Some(row.id.as_str());

    // --- how the header is laid out, decided before anything is painted ---
    //
    // **5b puts the actions ON the header line, right-aligned beside the
    // title**, and this pane put them on a row of their own underneath. That
    // one difference is most of why the detail column read as a stack of
    // fields rather than as a page: 5b's header is one band -- who this is,
    // what state it is in, and what you can do about it, left to right --
    // and ours was a band, then a toolbar, then a sentence, then a card.
    //
    // It was not an arbitrary choice. At `MIN_VAULT_WINDOW_SIZE` this column
    // is 298pt, its content box is 250, and 5b's own arrangement does not fit
    // in it at any padding: a 42pt tile plus a title worth reading plus three
    // controls is well past 250. So the answer is both, chosen by measurement
    // rather than by picking one and living with it at the other end -- which
    // is the same rule the `Views` meter and the card's label column already
    // follow on this screen.
    let labels = action_labels(row);
    let content = (width - DETAIL_PAD_X_F * 2.0).max(0.0);
    let action_pad = action_pad_for(ui, &labels, content);
    let actions_width = actions_total(ui, &labels, action_pad);
    // Inline only if the title still gets a readable run after the tile, the
    // actions and the gap between the two columns. `TITLE_MIN_ROOM` is what
    // "readable" means here and is argued at its own definition.
    //
    // Never while a revoke is running: that state draws a sentence where the
    // controls were (see below), and a sentence is not a right-aligned row.
    let inline = !revoking
        && HEADER_TILE + ROW_TEXT_GAP + TITLE_MIN_ROOM + HEADER_ACTION_GAP + actions_width
            <= content;

    // --- header strip ---------------------------------------------------
    let strip_height = if inline {
        DETAIL_PAD_Y_F * 2.0 + HEADER_BLOCK
    } else {
        DETAIL_PAD_Y_F * 2.0 + HEADER_BLOCK + ACTION_ROW_GAP + theme::ACTION_BUTTON_HEIGHT
    };
    let (strip, _) = ui.allocate_exact_size(egui::vec2(width, strip_height), egui::Sense::hover());
    {
        let p = ui.painter();
        p.rect_filled(strip, CornerRadius::ZERO, theme::CARD);
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(strip.left(), strip.bottom() - 1.0),
                egui::pos2(strip.right(), strip.bottom()),
            ),
            CornerRadius::ZERO,
            theme::HAIRLINE,
        );
        let block_top = strip.top() + DETAIL_PAD_Y_F;
        let tile = egui::Rect::from_min_size(
            egui::pos2(
                strip.left() + DETAIL_PAD_X_F,
                block_top + (HEADER_BLOCK - HEADER_TILE) / 2.0,
            ),
            egui::Vec2::splat(HEADER_TILE),
        );
        paint_tile(p, tile, &row.name, true, 14.0);

        let text_left = tile.right() + ROW_TEXT_GAP;
        // The text column stops where the actions begin when they are on this
        // line, and at the pane's padding when they are not.
        let text_right = strip.right()
            - DETAIL_PAD_X_F
            - if inline { actions_width + HEADER_ACTION_GAP } else { 0.0 };
        let clip = p.with_clip_rect(
            egui::Rect::from_min_max(
                egui::pos2(text_left, strip.top()),
                egui::pos2(text_right.max(text_left), strip.bottom()),
            )
            .intersect(ui.clip_rect()),
        );
        // **Elided, not clipped**, which is `elided`'s whole argument applied
        // to the one run on this screen that had escaped it. A clipped galley
        // keeps its full width, so at 298pt "Office WiFi -- Guest" was drawn
        // at its natural size and simply stopped at the pane edge, with no
        // ellipsis and no way for the reader to know a word had gone. The
        // card rows below have been elided since they were written; the
        // 21px title, the biggest run in the pane, was not.
        let title = elided(
            &clip,
            &row.name,
            egui::FontId::new(TITLE_PX, egui::FontFamily::Name(theme::BOLD.into())),
            theme::INK,
            (text_right - text_left).max(0.0),
        );
        clip.galley(egui::pos2(text_left, block_top), title, theme::INK);
        // The subtitle line: what kind of Send this is, then its pill. 5b
        // reads "Send - created 16 Aug, 14:20" here and this client has no
        // creation date, so the slot says the one thing it does know -- and
        // it is the same fact the list row's `FILE` tag carries, spelled out
        // where there is room for it.
        let sub_y = block_top + TITLE_LINE + SUBTITLE_GAP + theme::PILL_HEIGHT / 2.0;
        let kind = if row.is_file { FILE_SEND_KIND } else { TEXT_SEND_KIND };
        let kind_rect = clip.text(
            egui::pos2(text_left, sub_y),
            egui::Align2::LEFT_CENTER,
            kind,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            theme::TEXT_FAINT,
        );
        theme::state_pill(
            &clip,
            egui::pos2(kind_rect.right() + STRIP_GAP, sub_y),
            state_tone(row.state),
            state_label(row.state),
        );
    }

    // --- the actions -----------------------------------------------------
    //
    // Inline: 5b's own composition, right-aligned on the header line and
    // centred on the header block. Stacked: their own row under it, at the
    // pane's left padding, which is the only arrangement that fits 250pt.
    let action_y = if inline {
        strip.top() + DETAIL_PAD_Y_F + (HEADER_BLOCK - theme::ACTION_BUTTON_HEIGHT) / 2.0
    } else {
        strip.top() + DETAIL_PAD_Y_F + HEADER_BLOCK + ACTION_ROW_GAP
    };
    let slot_at = |x: f32, w: f32| {
        egui::Rect::from_min_size(
            egui::pos2(x, action_y),
            egui::vec2(w, theme::ACTION_BUTTON_HEIGHT),
        )
    };
    let x0 = if inline {
        strip.right() - DETAIL_PAD_X_F - actions_width
    } else {
        strip.left() + DETAIL_PAD_X_F
    };
    if revoking {
        // **No widget of any kind**, which is the rule this screen has always
        // followed here: a disabled button is still a button the layout has
        // to hold, and sense-less controls in this window have a history of
        // coming back to life after a re-layout.
        //
        // `inline` is false whenever this is true -- see its definition --
        // so this sentence always has the second row to itself and can never
        // be laid over the title.
        ui.painter().text(
            egui::pos2(strip.left() + DETAIL_PAD_X_F, action_y + theme::ACTION_BUTTON_HEIGHT / 2.0),
            egui::Align2::LEFT_CENTER,
            DELETING_LABEL,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            theme::TEXT_MUTED,
        );
    } else {
        let copy_w = theme::action_button_width(ui.painter(), COPY_LINK_LABEL, action_pad);
        let switch_on = row.state == crate::send::SendState::Revoked;
        let switch_label = if switch_on { SWITCH_ON_LABEL } else { SWITCH_OFF_LABEL };
        let switch_w = theme::action_button_width(ui.painter(), switch_label, action_pad);
        let delete_w = theme::action_button_width(ui.painter(), DELETE_LABEL, action_pad);
        let copy_rect = slot_at(x0, copy_w);
        let switch_rect = slot_at(copy_rect.right() + BUTTON_GAP, switch_w);
        // **The Delete slot and the Cancel slot are ONE expression, so they
        // cannot drift apart.** The whole mis-click defence is that a second
        // rapid click where Delete was lands on Cancel, and that is only true
        // while these two rectangles are equal. The switch's slot is reserved
        // whether or not it is drawn, for the same reason: hiding it must not
        // slide the destructive control under the pointer.
        let delete_rect = slot_at(switch_rect.right() + BUTTON_GAP, delete_w);

        // **A Send with no URL has nothing to copy, and says so by being
        // unclickable.** `send.rs`'s parser rejects a *missing* `accessUrl`
        // and accepts an empty one, so a row can reach here holding `""`;
        // copying that would hand `copy_secret("")` to the clipboard,
        // silently wiping whatever the user had there and reporting success.
        // The button is still drawn -- the pane must not lose its shape, and
        // a header that quietly has no control is harder to understand than
        // one that has a dead one.
        //
        // **The PRIMARY**, and the only one on this screen. 5b's header has
        // an outlined control and a solid one, which is a hierarchy; this
        // pane had three interchangeable grey boxes, which is a toolbar. What
        // the screen is *for* is the link -- a Send exists to be handed to
        // somebody -- so Copy link is the thing the eye should find, and the
        // other two step back behind it.
        let has_url = !row.access_url.is_empty();
        let copied =
            theme::action_button(ui, copy_rect, COPY_LINK_LABEL, theme::ActionTone::Primary)
                .clicked();

        // Hidden while confirming, and not disabled: the confirmation is
        // about exactly one thing, and a fourth control beside a destructive
        // question is a fourth thing to mis-click. Drawn only for a Send with
        // an id, on Delete's own rule -- an id is what names the Send to the
        // server, and a control that cannot name its subject must not report
        // an action.
        let switched = if confirming || !has_id {
            None
        } else {
            theme::action_button(ui, switch_rect, switch_label, theme::ActionTone::Secondary)
            .clicked()
            .then(|| SendUiAction::SetDisabled {
                id: row.id.clone(),
                name: row.name.clone(),
                // What the press ASKED for: a revoked Send asks to come back
                // on, and every other one asks to go off.
                disabled: !switch_on,
            })
        };

        let destructive = if confirming {
            // Cancel, in Delete's exact rectangle. The destructive half of
            // the confirmation is NOT here -- it is in the body, under the
            // question it answers; see below for why that is a stronger
            // defence than a wider button on this row.
            theme::action_button(ui, delete_rect, CANCEL_LABEL, theme::ActionTone::Secondary)
                .clicked()
                .then_some(SendUiAction::CancelDelete)
        } else {
            // **`DestructiveQuiet` and not `Destructive`**: red words in an
            // outlined button, not a solid red one. 5b's solid red is its
            // `Revoke`, the one destructive control on its header; this
            // header carries a destructive control beside two ordinary ones,
            // and a filled red among them would be the loudest thing on a
            // screen about a link. The solid red is spent one step later, on
            // the confirmation this raises. See `theme::ActionTone`.
            let asked =
                theme::action_button(ui, delete_rect, DELETE_LABEL, theme::ActionTone::DestructiveQuiet)
                    .clicked();
            (asked && has_id).then(|| SendUiAction::AskDelete(row.id.clone()))
        };

        // The destructive control wins, then the switch, then Copy link. One
        // rule, twice: the answer to an ambiguous frame is the one the user
        // cannot have meant by accident, and a stray copy must never swallow
        // a press on either of the other two. Belt and braces on the URL --
        // the guard is on the returned ACTION as well as on the widget, so no
        // future re-layout can reopen the path.
        action = destructive
            .or(switched)
            .or_else(|| (copied && has_url).then(|| SendUiAction::CopyLink(row.access_url.clone())));
    }

    // --- the body --------------------------------------------------------
    //
    // **The cursor is put back at the foot of the strip first.** `Ui::put`
    // does not merely draw at a rect, it advances the cursor to that rect's
    // bottom -- and every control above is put INSIDE a band this function
    // already allocated, so the last one moves the cursor BACKWARDS, from the
    // strip's foot to the action row's. The body then opened flush against
    // the header's hairline with its own 18pt top margin swallowed, which is
    // visible in a picture and invisible in every rect assertion (the margin
    // was applied; it was applied from the wrong place).
    ui.advance_cursor_after_rect(strip);
    egui::Frame::new()
        .inner_margin(Margin::symmetric(DETAIL_PAD_X, DETAIL_PAD_Y))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("send-detail")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // **The first line, and 5c's own subject.** Replaced by
                    // the confirmation's question while one is up rather than
                    // joined to it, for the reason this screen's second line
                    // has always been replaced: a pane that says both "1 view
                    // left" and "Revoke this link for good?" is a pane whose
                    // subject is ambiguous at the moment it matters most.
                    if confirming {
                        ui.label(
                            theme::semibold(CONFIRM_PROMPT, ANSWER_LINE_PX).color(theme::ERROR),
                        );
                        ui.add_space(10.0);
                        // **The destructive button, here rather than on the
                        // action row.** Three things separate it from the
                        // Cancel that now occupies Delete's pixels: a
                        // different rectangle, a different row, and a
                        // different container. It also sits directly under
                        // the question it answers, which the old layout could
                        // not manage -- and it is what lets the whole
                        // confirmation fit at `MIN_VAULT_WINDOW_SIZE`, where
                        // four controls on one 250pt row do not.
                        //
                        // **`theme::destructive_button`, the app's own solid
                        // red.** It was a bare `egui::Button` with red text,
                        // which is the same look Delete wears one step
                        // earlier -- so the escalation from "are you sure?"
                        // to "yes, destroy it" was carried entirely by the
                        // wording. This module already spends the filled red
                        // on exactly this meaning; see `destructive_button`,
                        // whose own doc argues why the second step must not
                        // be blue and must not be quiet.
                        let confirmed = theme::destructive_button(ui, CONFIRM_LABEL).clicked();
                        if confirmed && has_id {
                            // Cancel wins if both somehow report in one
                            // frame: the safe answer to an ambiguous frame on
                            // a destructive control is not to destroy.
                            if action != Some(SendUiAction::CancelDelete) {
                                action = Some(SendUiAction::ConfirmDelete {
                                    id: row.id.clone(),
                                    name: row.name.clone(),
                                });
                            }
                        }
                    } else {
                        // **5c's own treatment for 5c's own line.** 5c opens
                        // "Whether it was used is the question people
                        // actually have -- so it is the first line, not a log
                        // entry", and it draws that line at `font-size: 14px;
                        // font-weight: 700`. This pane drew it at 13px
                        // Regular, which is the size and weight of a caption:
                        // the biggest claim on the screen was typeset as the
                        // smallest thing on it, sitting under a header and
                        // above a card that both outweighed it. One step up
                        // in size and one in weight is the whole change, and
                        // it is what makes the eye land on the answer rather
                        // than on the `LINK` slab below.
                        ui.label(
                            theme::semibold(row.activity.as_str(), ANSWER_LINE_PX)
                                .color(theme::INK),
                        );
                    }
                    ui.add_space(14.0);
                    draw_link_card(ui, row);
                });
        });

    action
}

/// The detail for one received record.
///
/// Two facts and no invented third. What a `ReceivedRow` knows is its name,
/// when it arrived and whether the item it created survives -- see
/// [`crate::receive_history`], which argues at length why it must not know
/// the link it came from.
fn draw_received_detail(ui: &mut egui::Ui, row: &ReceivedRow) {
    let width = ui.available_width();
    let strip_height = DETAIL_PAD_Y_F * 2.0 + HEADER_BLOCK;
    let (strip, _) = ui.allocate_exact_size(egui::vec2(width, strip_height), egui::Sense::hover());
    {
        let p = ui.painter();
        p.rect_filled(strip, CornerRadius::ZERO, theme::CARD);
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(strip.left(), strip.bottom() - 1.0),
                egui::pos2(strip.right(), strip.bottom()),
            ),
            CornerRadius::ZERO,
            theme::HAIRLINE,
        );
        let block_top = strip.top() + DETAIL_PAD_Y_F;
        let tile = egui::Rect::from_min_size(
            egui::pos2(
                strip.left() + DETAIL_PAD_X_F,
                block_top + (HEADER_BLOCK - HEADER_TILE) / 2.0,
            ),
            egui::Vec2::splat(HEADER_TILE),
        );
        paint_tile(p, tile, &row.name, true, 14.0);
        let text_left = tile.right() + ROW_TEXT_GAP;
        let clip = p.with_clip_rect(
            egui::Rect::from_min_max(
                egui::pos2(text_left, strip.top()),
                egui::pos2((strip.right() - DETAIL_PAD_X_F).max(text_left), strip.bottom()),
            )
            .intersect(ui.clip_rect()),
        );
        clip.text(
            egui::pos2(text_left, block_top),
            egui::Align2::LEFT_TOP,
            &row.name,
            egui::FontId::new(TITLE_PX, egui::FontFamily::Name(theme::BOLD.into())),
            theme::INK,
        );
        clip.text(
            egui::pos2(text_left, block_top + TITLE_LINE + SUBTITLE_GAP + 8.0),
            egui::Align2::LEFT_CENTER,
            RECEIVED_KIND,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            theme::TEXT_FAINT,
        );
    }
    egui::Frame::new()
        .inner_margin(Margin::symmetric(DETAIL_PAD_X, DETAIL_PAD_Y))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("received-detail")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // The one derived fact, first and in words, for the same
                    // reason a Send's activity sentence is first: a row that
                    // silently points at nothing is the failure this screen
                    // has to avoid.
                    let (sentence, colour) = if row.still_in_vault {
                        (RECEIVED_ITEM_PRESENT, theme::INK)
                    } else {
                        (RECEIVED_ITEM_GONE, theme::TEXT_MUTED)
                    };
                    // [`ANSWER_LINE_PX`], like the Send pane's own first
                    // line: the two panes swap places in this column and a
                    // reader should not be able to tell which one they are
                    // looking at from the type alone.
                    ui.label(theme::semibold(sentence, ANSWER_LINE_PX).color(colour));
                    ui.add_space(14.0);
                    let short = if row.still_in_vault { IN_VAULT_YES } else { IN_VAULT_NO };
                    draw_fact_card(
                        ui,
                        RECEIVED_CARD_TITLE,
                        &[(ARRIVED_ROW, CardValue::Plain(&row.when)), (IN_VAULT_ROW, CardValue::Plain(short))],
                        // No caution band: a record somebody shared WITH this
                        // user carries no claim about who else has seen it.
                        // See [`draw_link_card`], where there is one.
                        false,
                    );
                });
        });
}

/// What one row of a fact card shows in its value column.
///
/// An enum rather than three functions, because the three differ only in what
/// goes in one rectangle and the rectangle is computed identically for all of
/// them -- which is the property that keeps every card on this screen on one
/// label column.
enum CardValue<'a> {
    /// Ordinary text.
    Plain(&'a str),
    /// A URL: monospace, and struck through with a marker beside it once the
    /// link is dead. 5b draws exactly this.
    Address { url: &'a str, dead: bool },
    /// A count against a cap, with the design's 90x4 bar. `None` for a cap is
    /// "as often as they like", which has no denominator and therefore no
    /// bar: a full bar would say the opposite of what it means.
    Views { used: u32, cap: Option<u32> },
}

/// Design 5b's `Link` card: everything about the link itself, one fact per
/// row, on one label column.
fn draw_link_card(ui: &mut egui::Ui, row: &SendRow) {
    let opens_with = if row.has_password { PASSWORD_SEGMENT } else { LINK_ONLY };
    let address = if row.access_url.is_empty() {
        CardValue::Plain(ADDRESS_MISSING)
    } else {
        CardValue::Address {
            url: row.access_url.as_str(),
            // Dead is the state, not the date: a Send whose views are spent
            // is as unopenable as one that expired, and 5b strikes the
            // address through for exactly that reason.
            dead: row.state != crate::send::SendState::Waiting,
        }
    };
    draw_fact_card(
        ui,
        LINK_CARD_TITLE,
        &[
            (ADDRESS_ROW, address),
            (OPENS_WITH_ROW, CardValue::Plain(opens_with)),
            (
                VIEWS_ROW,
                CardValue::Views { used: row.access_count, cap: row.max_access_count },
            ),
            // **Both dates, and they are not the same date.** `expiration_date`
            // is when the link stops answering; `deletion_date` is when the
            // record goes. Bitwarden allows a gap between them, and a pane
            // that showed only the second would describe a link that already
            // 404s as live for the whole of that gap -- which is the defect
            // `send_state` reads both dates to avoid, said out loud here.
            (EXPIRES_ROW, CardValue::Plain(row.expires.as_str())),
            (DELETED_ROW, CardValue::Plain(row.deletes.as_str())),
        ],
        // **Design 5c's caution band, and the one part of 5c's Activity block
        // this client can stand behind.**
        //
        // 5c ends its history with an amber strip -- "This password has been
        // seen by someone else / Rotate it now" -- and that strip is not part
        // of the per-access timeline this screen deliberately does not build.
        // It is a reading of one number: `accessCount`. If somebody opened
        // the link, whatever was behind it is out, and the useful thing to
        // say is what to do about it. Every other line in 5c's block names a
        // time, a browser and a city that no Bitwarden Send records; this one
        // names none of them.
        //
        // It is also what stops this pane being a header and one card on a
        // field of empty canvas. That is a happy consequence rather than the
        // reason -- a band invented to fill the space would be worse than the
        // space -- but it is worth saying, because the emptiness was real and
        // is most of what "not even close to UI" was about.
        row.access_count > 0,
    );
}

/// One card: an eyebrow band over a run of label/value rows.
///
/// Painted into explicitly-allocated bands rather than built out of nested
/// layouts, for this file's standing reason -- and because the label column
/// has to be ONE width down the whole card, which a per-row layout can only
/// achieve by every row agreeing to measure the same thing.
/// `caution` adds design 5c's amber band along the card's bottom edge; see
/// [`draw_link_card`], which is the one caller that ever passes `true`.
fn draw_fact_card(
    ui: &mut egui::Ui,
    title: &str,
    rows: &[(&str, CardValue<'_>)],
    caution: bool,
) {
    let width = ui.available_width();
    // The band is LAID OUT before the card is allocated, for the list
    // column's footnote's reason one screen over: its two lines wrap, so its
    // height is a measurement and not a constant, and a card allocated to a
    // guess would either crop the sentence or leave a gap under it.
    let band = caution.then(|| caution_lines(ui, width));
    let band_height = band.as_ref().map_or(0.0, |lines| caution_height(lines));
    let height = CARD_HEADER_HEIGHT + CARD_ROW_HEIGHT * rows.len() as f32 + band_height;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(CARD_RADIUS), theme::CARD);
    let header_bottom = rect.top() + CARD_HEADER_HEIGHT;
    let eyebrow = p.layout_job(theme::letterspaced(
        title,
        theme::EYEBROW_PX,
        theme::BOLD,
        theme::EYEBROW_TRACKING,
        theme::TEXT_MUTED,
    ));
    let eyebrow_height = eyebrow.size().y;
    p.galley(
        egui::pos2(
            rect.left() + CARD_PAD_X,
            rect.top() + (CARD_HEADER_HEIGHT - eyebrow_height) / 2.0,
        ),
        eyebrow,
        theme::TEXT_MUTED,
    );
    p.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(rect.left(), header_bottom - 1.0),
            egui::pos2(rect.right(), header_bottom),
        ),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );

    // **One label column for the whole card, and it shrinks with the pane
    // rather than pushing the value off it.** 5b's own 120 is a third of its
    // card; at `MIN_VAULT_WINDOW_SIZE` this card is 250 wide, where a fixed
    // 120 would leave 116 for an address. The clamp keeps it readable at both
    // ends instead of picking one.
    let label_width = (width * CARD_LABEL_SHARE).clamp(CARD_LABEL_MIN, CARD_LABEL_MAX);
    let value_left = rect.left() + CARD_PAD_X + label_width + CARD_GAP;
    let value_right = rect.right() - CARD_PAD_X;
    let label_font = egui::FontId::new(12.0, egui::FontFamily::Proportional);
    let value_font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
    for (index, (label, value)) in rows.iter().enumerate() {
        let top = header_bottom + CARD_ROW_HEIGHT * index as f32;
        let middle = top + CARD_ROW_HEIGHT / 2.0;
        if index > 0 {
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + CARD_PAD_X, top),
                    egui::pos2(rect.right() - CARD_PAD_X, top + 1.0),
                ),
                CornerRadius::ZERO,
                theme::CARD_TINT,
            );
        }
        // Every run in this card is ELIDED to its own column rather than
        // clipped by a rect. See `elided`: a clipped galley keeps its full
        // width, so the pane looks identical whether it fits or not, and the
        // reader is shown a string that has quietly lost its end.
        let label_galley =
            elided(&p, label, label_font.clone(), theme::TEXT_FAINT, label_width);
        p.galley(
            egui::pos2(rect.left() + CARD_PAD_X, middle - label_galley.size().y / 2.0),
            label_galley,
            theme::TEXT_FAINT,
        );
        let cell = egui::Rect::from_min_max(
            egui::pos2(value_left, top),
            egui::pos2(value_right.max(value_left), top + CARD_ROW_HEIGHT),
        );
        let clip = p.with_clip_rect(cell.intersect(ui.clip_rect()));
        match value {
            CardValue::Plain(text) => {
                let galley =
                    elided(&clip, text, value_font.clone(), theme::INK, cell.width());
                clip.galley(
                    egui::pos2(cell.left(), middle - galley.size().y / 2.0),
                    galley,
                    theme::INK,
                );
            }
            CardValue::Address { url, dead } => {
                // The marker is placed FIRST and the address is clipped
                // against it, so a long URL is cut off rather than run
                // underneath the word that says it no longer works.
                let mut right = cell.right();
                if *dead {
                    let marker = clip.text(
                        egui::pos2(cell.right(), middle),
                        egui::Align2::RIGHT_CENTER,
                        DEAD_MARKER,
                        egui::FontId::new(12.0, egui::FontFamily::Proportional),
                        theme::TEXT_GHOST,
                    );
                    right = marker.left() - CARD_GAP;
                }
                let colour = if *dead { theme::TEXT_GHOST } else { theme::INK };
                // **Elided with the design's own ellipsis, not clipped.** 5b
                // prints `send.deskwarden.app/g7HqK2...`, and the difference
                // matters more here than it would anywhere else on this
                // screen: a URL cut off by a clip rect is a complete-looking
                // address that is not the address, and the whole subject of
                // this row is which link this is. The ellipsis says the
                // reader is not seeing all of it; Copy link is what gets the
                // rest.
                let galley = elided(
                    &clip,
                    *url,
                    egui::FontId::new(12.0, egui::FontFamily::Monospace),
                    colour,
                    (right - cell.left()).max(0.0),
                );
                let size = galley.size();
                let at = egui::pos2(cell.left(), middle - size.y / 2.0);
                clip.galley(at, galley, colour);
                if *dead {
                    clip.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(at.x, middle - 0.5),
                            egui::pos2(at.x + size.x, middle + 0.5),
                        ),
                        CornerRadius::ZERO,
                        theme::TEXT_GHOST,
                    );
                }
            }
            CardValue::Views { used, cap } => {
                let text = match cap {
                    Some(cap) => format!("{used} of {cap}"),
                    None if *used == 0 => NO_VIEW_LIMIT.to_string(),
                    None => format!("{used} \u{00b7} {}", NO_VIEW_LIMIT.to_lowercase()),
                };
                let galley = elided(
                    &clip,
                    &text,
                    egui::FontId::new(13.0, egui::FontFamily::Name(theme::BOLD.into())),
                    theme::INK,
                    cell.width(),
                );
                let size = galley.size();
                let at = egui::pos2(cell.left(), middle - size.y / 2.0);
                clip.galley(at, galley, theme::INK);
                let drawn = egui::Rect::from_min_size(at, size);
                // The bar only where there is a denominator to fill, and only
                // where it fits. A bar squeezed to nothing at the minimum
                // window size would be a rectangle that says a fraction it is
                // too small to show.
                if let Some(cap) = cap {
                    let room = cell.right() - drawn.right() - CARD_GAP;
                    if room >= VIEW_BAR_MIN {
                        let bar_width = room.min(VIEW_BAR_WIDTH);
                        let track = egui::Rect::from_min_size(
                            egui::pos2(drawn.right() + CARD_GAP, middle - VIEW_BAR_HEIGHT / 2.0),
                            egui::vec2(bar_width, VIEW_BAR_HEIGHT),
                        );
                        clip.rect_filled(track, CornerRadius::same(2), theme::HAIRLINE);
                        // Saturating, and clamped: a server reporting more
                        // opens than the cap must not paint a bar wider than
                        // its own track.
                        let fraction = if *cap == 0 {
                            1.0
                        } else {
                            (f64::from(*used) / f64::from(*cap)).clamp(0.0, 1.0) as f32
                        };
                        clip.rect_filled(
                            egui::Rect::from_min_size(
                                track.min,
                                egui::vec2(track.width() * fraction, VIEW_BAR_HEIGHT),
                            ),
                            CornerRadius::same(2),
                            theme::BLUE,
                        );
                    }
                }
            }
        }
    }

    // --- design 5c's caution band, along the card's bottom edge ----------
    if let Some(lines) = band {
        let top = header_bottom + CARD_ROW_HEIGHT * rows.len() as f32;
        let band_rect =
            egui::Rect::from_min_max(egui::pos2(rect.left(), top), rect.right_bottom());
        // Only the BOTTOM corners are rounded: this band is the foot of the
        // card, not a card of its own, and a fully rounded tint inside a
        // rounded card leaves two crescents of white in the corners.
        p.rect_filled(
            band_rect,
            CornerRadius { nw: 0, ne: 0, sw: CARD_RADIUS, se: CARD_RADIUS },
            theme::CAUTION_WASH,
        );
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), top),
                egui::pos2(rect.right(), top + 1.0),
            ),
            CornerRadius::ZERO,
            theme::HAIRLINE,
        );
        // 5c's `width: 15; height: 15` triangle, through the theme's own
        // glyph rather than a second drawing of one.
        theme::paint_warning_glyph(
            &p,
            egui::Rect::from_min_size(
                egui::pos2(band_rect.left() + CARD_PAD_X, top + CAUTION_PAD_Y + 1.0),
                egui::Vec2::splat(CAUTION_GLYPH),
            ),
            theme::CAUTION_MARK,
        );
        let mut y = top + CAUTION_PAD_Y;
        let x = band_rect.left() + CARD_PAD_X + CAUTION_GLYPH + CAUTION_GLYPH_GAP;
        for line in lines {
            let height = line.size().y;
            p.galley(egui::pos2(x, y), line, theme::CAUTION_INK);
            y += height + CAUTION_LINE_GAP;
        }
    }

    // **The border last, over everything.** It used to be stroked straight
    // after the fill, which was fine while nothing else reached the card's
    // edge; the caution band does, and a band painted over its own card's
    // outline is a tint that runs off the rounded corner.
    p.rect_stroke(
        rect,
        CornerRadius::same(CARD_RADIUS),
        egui::Stroke::new(1.0, theme::HAIRLINE),
        egui::StrokeKind::Inside,
    );
}

/// Design 5c's caution band, laid out to `width`.
///
/// Separate from the drawing for the reason every measurement on this screen
/// is: the card that holds the band has to be allocated at its full height
/// before anything is painted into it, and these two lines WRAP -- at 298pt
/// the detail column leaves them about 190pt, which is three lines rather
/// than two.
fn caution_lines(ui: &egui::Ui, width: f32) -> Vec<std::sync::Arc<egui::Galley>> {
    let text_width =
        (width - CARD_PAD_X * 2.0 - CAUTION_GLYPH - CAUTION_GLYPH_GAP).max(1.0);
    let p = ui.painter();
    vec![
        p.layout(
            CAUTION_HEADLINE.to_string(),
            egui::FontId::new(CAUTION_PX, egui::FontFamily::Name(theme::SEMIBOLD.into())),
            theme::CAUTION_INK,
            text_width,
        ),
        p.layout(
            CAUTION_ADVICE.to_string(),
            egui::FontId::new(CAUTION_PX, egui::FontFamily::Proportional),
            theme::CAUTION_INK,
            text_width,
        ),
    ]
}

/// How tall the band those lines go in has to be.
fn caution_height(lines: &[std::sync::Arc<egui::Galley>]) -> f32 {
    let text: f32 = lines.iter().map(|g| g.size().y).sum::<f32>()
        + CAUTION_LINE_GAP * (lines.len().saturating_sub(1)) as f32;
    // The glyph is never the taller of the two, but saying so out loud costs
    // nothing and is what stops a one-line band cropping its own icon.
    CAUTION_PAD_Y * 2.0 + text.max(CAUTION_GLYPH)
}

/// One line of `text` laid out to fit `max_width`, with an ellipsis where it
/// does not.
///
/// **A real layout and not a clip rect.** A clipped galley still measures its
/// full width, so a pane that lets one run overflow looks identical to one
/// that fits until something reads the rects -- and what the reader sees is a
/// string that has silently lost its end with nothing to say so. This returns
/// a galley whose OWN width is the space available, which is what makes "no
/// run is painted outside this pane" an assertion worth making.
///
/// `break_anywhere`, because a URL has no spaces to break at.
fn elided(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    colour: egui::Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::text::TextFormat { font_id: font, color: colour, ..Default::default() },
    );
    job.wrap = egui::text::TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    painter.layout_job(job)
}

/// The **slots** the detail header's action row occupies, in the order it
/// draws them.
///
/// **Its own function, because the row's WIDTH has to be known before the
/// strip that holds the row is allocated**, and the labels are what the width
/// is measured from. It is the same reserve-then-fill discipline every cell
/// on this screen follows, one level further out: here the thing being
/// reserved is not a slot but the shape of the whole header -- see
/// `draw_send_card` on why the actions sit on the title's line when they fit
/// and under it when they do not.
///
/// # It measures three slots always, and that is the whole contract
///
/// Two of the three are sometimes not DRAWN -- the switch leaves while a
/// confirmation is up or when the Send has no id, and Delete is replaced by
/// Cancel -- and the row still reserves every one of them. That rule is not
/// new and it is not cosmetic: the mis-click defence on this screen is that a
/// second rapid click where Delete was lands on **Cancel**, which is true
/// only while those two rectangles are equal, and equal rectangles require an
/// equal row width on both frames.
///
/// So this returns the same three widths in every state, measured from
/// `DELETE_LABEL` rather than from whichever of Delete/Cancel is showing.
/// Returning only the visible labels is a real bug and was briefly written:
/// it moved the right-aligned row's origin between the two frames and pushed
/// Cancel clean off the pane.
fn action_labels(row: &SendRow) -> [&'static str; 3] {
    [
        COPY_LINK_LABEL,
        if row.state == crate::send::SendState::Revoked {
            SWITCH_ON_LABEL
        } else {
            SWITCH_OFF_LABEL
        },
        DELETE_LABEL,
    ]
}

/// What a row of `labels` occupies at `pad`, gaps included.
fn actions_total(ui: &egui::Ui, labels: &[&str], pad: f32) -> f32 {
    if labels.is_empty() {
        return 0.0;
    }
    let sum: f32 =
        labels.iter().map(|l| theme::action_button_width(ui.painter(), l, pad)).sum();
    sum + BUTTON_GAP * (labels.len() - 1) as f32
}

/// The widest padding at which this row of controls still fits `room`.
///
/// **Padding is the first thing given up, and the only thing**, which is
/// `theme::action_button_width`'s own argument applied here: the detail
/// column is 250pt of content at `MIN_VAULT_WINDOW_SIZE`, three controls at
/// 5b's `padding: 0 14px` come to 258, and something has to go. Shrinking a
/// LABEL hides what the control does. Shrinking the HEIGHT would make these
/// a different component from the identical controls on a wider window, which
/// is precisely the drift this screen is being repaired for. Shrinking the
/// padding leaves a button that is visibly the same button, slightly tighter
/// -- and only on the window sizes that cannot hold the comfortable one.
///
/// The floor is returned even when it does not fit, because a row that does
/// not fit must still be drawn at its smallest rather than vanish: this pane
/// has no narrower window to fall back to, and a missing Delete is worse than
/// a cramped one. Nothing is painted outside the pane either way -- the
/// labels are laid out inside their own rects.
fn action_pad_for(ui: &egui::Ui, labels: &[&str], room: f32) -> f32 {
    const LADDER: [f32; 4] = [theme::ACTION_BUTTON_PAD_X, 12.0, 10.0, ACTION_PAD_FLOOR];
    for pad in LADDER {
        if actions_total(ui, labels, pad) <= room {
            return pad;
        }
    }
    ACTION_PAD_FLOOR
}

/// The tightest an action button is ever drawn; see [`action_pad_for`].
const ACTION_PAD_FLOOR: f32 = 8.0;

/// Design 5b's `padding: 18px 24px` on the detail pane's header strip and on
/// its body.
///
/// `i8` because that is what `egui::Margin`'s fields are; the painted
/// geometry wants the `f32`, so both spellings are here and derived from one
/// another rather than written twice.
const DETAIL_PAD_X: i8 = 24;
const DETAIL_PAD_Y: i8 = 18;
const DETAIL_PAD_X_F: f32 = DETAIL_PAD_X as f32;
const DETAIL_PAD_Y_F: f32 = DETAIL_PAD_Y as f32;
/// The header's initials tile: 5b's `width: 42px; height: 42px`.
const HEADER_TILE: f32 = 42.0;
/// The title's own line box at [`TITLE_PX`], and the gap under it before the
/// subtitle -- 5b's `gap: 4px` on the header's text column.
const TITLE_PX: f32 = 21.0;
const TITLE_LINE: f32 = 26.0;
const SUBTITLE_GAP: f32 = 4.0;
/// The header's text block: a title line, the gap, and a pill. The tile is
/// centred on it, which is why the block and not the tile is the height the
/// strip is built from -- at 42 the tile is shorter than its own text column,
/// and hanging the column off the tile is what left one of them floating in
/// the item detail's header before this window learnt the lesson.
const HEADER_BLOCK: f32 = TITLE_LINE + SUBTITLE_GAP + theme::PILL_HEIGHT;
/// The gap between the header's text block and the action row under it, on
/// the windows too narrow to put the actions on the title's own line.
const ACTION_ROW_GAP: f32 = 12.0;
/// 5b's `gap: 14px` between the header's text column and the controls at its
/// right edge.
const HEADER_ACTION_GAP: f32 = 14.0;
/// The narrowest run the title may be left with before the actions are moved
/// off its line and onto their own.
///
/// **160 is roughly twenty characters at [`TITLE_PX`]**, which is where a
/// name stops being a name and becomes an ellipsis with a hint in front of
/// it. The number is a judgement and is written here rather than inlined so
/// it can be argued with: below it, 5b's one-band header is worse than the
/// two-row arrangement it replaces, because the whole point of that band is
/// that the thing on the left is identifiable.
const TITLE_MIN_ROOM: f32 = 160.0;

/// The size and weight of the **one sentence** each detail pane opens with --
/// a Send's activity, a received record's whereabouts, and the destructive
/// confirmation that replaces the first of them.
///
/// 5c's own `font-size: 14px; font-weight: 700` for the line it argues is the
/// question people actually have. Named, and shared by the three, because
/// they occupy the same slot in the same column and a reader should not be
/// able to tell which pane they are looking at from the type alone. It is
/// [`theme::semibold`] rather than 5c's 700 for the reason every heading in
/// this app is: 700 is the weight this design system spends on a SELECTED
/// row's name and on a pill, and a sentence in it would outweigh the header
/// above it.
const ANSWER_LINE_PX: f32 = 14.0;

/// Card geometry, measured as a border box from 5b's own CSS.
///
/// The header declares `padding: 11px 16px` on a 12px line (~15pt box) inside
/// a 1px border: 1 + 11 + 15 + 11 = 38. A row declares `padding: 13px 16px`
/// on a 13px line (~16pt box): 13 + 16 + 13 = 42. **The page is content-box**,
/// so the padding does not eat the line and the border is extra -- which is
/// the arithmetic this screen has been re-measured for before.
const CARD_HEADER_HEIGHT: f32 = 38.0;
const CARD_ROW_HEIGHT: f32 = 42.0;
const CARD_PAD_X: f32 = 16.0;
const CARD_RADIUS: u8 = 10;
/// 5b's `gap: 14px` between a card row's label and its value.
const CARD_GAP: f32 = 14.0;
/// The label column: 5b's 120 on a 470pt card is just over a quarter, and
/// this pane is 250 wide at the app's minimum. See [`draw_fact_card`].
const CARD_LABEL_SHARE: f32 = 0.30;
const CARD_LABEL_MIN: f32 = 68.0;
const CARD_LABEL_MAX: f32 = 120.0;
/// Design 5c's caution band: `padding: 13px 16px`, a 15px warning triangle
/// with the design's `gap: 9px` beside it, two 12px lines `gap: 6px` apart.
/// The horizontal padding is [`CARD_PAD_X`], because the band is a foot of
/// the card and its text has to stand on the same column as the labels above.
const CAUTION_PAD_Y: f32 = 13.0;
const CAUTION_GLYPH: f32 = 15.0;
const CAUTION_GLYPH_GAP: f32 = 9.0;
const CAUTION_LINE_GAP: f32 = 6.0;
const CAUTION_PX: f32 = 12.0;

/// What design 5c's band says, in the one form this client can support.
///
/// **5c says "This password has been seen by someone else" and this cannot**,
/// because a Send made here is arbitrary text: it may be a password, a
/// connection string, a one-off token or a note. Naming it a password would
/// be a guess, and a guess in the reassuring direction on the half of the
/// cases where it is something worse. So the headline says what is certainly
/// true -- somebody opened the link, and whatever was behind it is out -- and
/// the advice is conditional in the one place where it has to be.
pub const CAUTION_HEADLINE: &str = "Somebody has opened this link.";
pub const CAUTION_ADVICE: &str =
    "Treat whatever it held as known to them. If it was a password, change it.";

/// 5b's view meter: `width: 90px; height: 4px; border-radius: 2px`.
const VIEW_BAR_WIDTH: f32 = 90.0;
const VIEW_BAR_HEIGHT: f32 = 4.0;
/// Below this there is no bar at all; see [`draw_fact_card`].
const VIEW_BAR_MIN: f32 = 32.0;

/// The detail header's subtitle: what kind of Send this is. 5b's slot for
/// "Send - created ...", carrying the one half of that this client holds.
pub const TEXT_SEND_KIND: &str = "Text Send";
pub const FILE_SEND_KIND: &str = "File Send";
/// Its counterpart on the `Shared with me` detail.
pub const RECEIVED_KIND: &str = "Shared with you";

/// Design 5b's `Link` card and its rows.
pub const LINK_CARD_TITLE: &str = "LINK";
pub const ADDRESS_ROW: &str = "Address";
pub const OPENS_WITH_ROW: &str = "Opens with";
pub const VIEWS_ROW: &str = "Views";
pub const EXPIRES_ROW: &str = "Expires";
pub const DELETED_ROW: &str = "Deleted";
/// 5b's own word beside a struck-through address.
pub const DEAD_MARKER: &str = "dead";
/// The `Opens with` row for a Send that has no share password -- 5b's "link
/// only", said as what the recipient needs rather than as what is missing.
pub const LINK_ONLY: &str = "The link alone";
/// The `Views` row for an uncapped Send.
pub const NO_VIEW_LIMIT: &str = "No limit";
/// The `Address` row for a Send the server described without one. See
/// `draw_send_card` on why the control beside it is drawn and dead rather
/// than absent.
pub const ADDRESS_MISSING: &str = "This Send reported no link.";

/// The `Shared with me` detail's card and its rows.
pub const RECEIVED_CARD_TITLE: &str = "RECEIVED";
pub const ARRIVED_ROW: &str = "Arrived";
pub const IN_VAULT_ROW: &str = "In your vault";
pub const IN_VAULT_YES: &str = "Yes";
pub const IN_VAULT_NO: &str = "No longer";

/// The header button that opens the composer. Hidden while the composer is
/// already open: two ways to reach one open form is one way too many, and the
/// form itself is the thing on screen at that point.
pub const NEW_SEND_LABEL: &str = "New Send";

/// The composer's own heading. **The one label that appears nowhere else on
/// this screen**, which is what makes it the witness
/// `send_delete_wiring::ReachableState::ComposerOpen` is checked by.
pub const COMPOSER_HEADING: &str = "New text Send";

/// The eyebrow over the name field -- this composer's counterpart of §5a's
/// `Record`. See `draw_composer` for why the two inputs are eyebrowed at all.
pub const NAME_EYEBROW: &str = "NAME";
/// The eyebrow over the body field -- this composer's counterpart of §5a's
/// `Include`. **The design's own vocabulary for "what travels"**, said as the
/// noun rather than as the verb, because a text Send's body is one thing and
/// there is nothing to include or leave out.
pub const TEXT_EYEBROW: &str = "TEXT";

/// How many lines of the body the box shows before it scrolls. The four it
/// has always shown -- enough for a host, a user and a note, which is what
/// this field is used for, and short enough that the Access block below it is
/// on screen at the window floor.
pub const BODY_ROWS: usize = 4;

/// The standing note at the footer's right-hand end: **§5a's `Appears in
/// Shared`, in this app's own vocabulary.**
///
/// §5a says `Shared` because the rail row its design puts a new Send into is
/// called that. This app's is called `Sends` -- `sidebar::SENDS_ROW_LABEL` --
/// under a `SHARING` section, so `Appears in Shared` would name a row the
/// user cannot find. The note's job is to answer "where does this go when I
/// press the blue button", and an answer that does not match the rail is
/// worse than none.
///
/// It is `pub` and shared with `record_ui` deliberately: both composers
/// publish a Send into the same list, so a second spelling of the same fact
/// on the other card is exactly the defect the shared Access block and the
/// shared footer buttons were each introduced to remove.
pub const APPEARS_IN_SENDS: &str = "Appears in Sends";

/// The name field's placeholder.
pub const NAME_HINT: &str = "Name this Send";
/// The body field's placeholder. **Says "text"**, because that is the only
/// kind this app makes and a blank box invites a file drag that will do
/// nothing.
pub const TEXT_HINT: &str = "The text to share";
/// The share-password field's placeholder. **It says what an empty box
/// means**, because empty is both the default and the off position, and a
/// blank box under the words "Open with" otherwise reads as something the
/// user has forgotten to fill in. See [`draw_access_block`].
pub const PASSWORD_HINT: &str = "No password";

// `PASSWORD_TOGGLE_LABEL` -- "Require a password to open the link" -- and
// `LIFETIME_PROMPT` -- "The link stops working after" -- are DELETED here
// rather than parked, and the deletion is the decision. Both were the
// composer's own captions for two questions that are now asked once, in
// [`ACCESS_EYEBROW`]'s block, in the row labels [`EXPIRES_LABEL`],
// [`VIEWS_LABEL`] and [`OPEN_WITH_LABEL`]. Keeping them would have left two
// spellings of one question in one file, which is the defect this pass
// exists to remove; and a `pub const` raises no dead-code warning in a lib
// crate, so an unused one sits indefinitely looking like copy somebody still
// paints. The password switch went with its label -- see
// [`draw_access_block`] on why an empty box is the off position now.
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
/// `Option<Zeroizing<String>>` and the Access block's field drives that
/// `Option` directly, so emptying the box wipes the buffer it was typed into
/// rather than leaving a live secret behind a `false`. It used to be driven
/// by a tick-box through `wants_password` / `set_wants_password`; those are
/// gone with the tick-box, and [`draw_access_block`] argues why.
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

/// What is wrong with the draft, phrased for the user, or `None`.
///
/// A one-line delegation to [`crate::send::validate_plan`] **and that is the
/// point**: the sentence under the Create button and the refusal inside
/// `plan_to_invocation` are the same function, so there is no draft this form
/// calls acceptable that the encoder then rejects, and none it greys out that
/// would in fact have published.
/// It takes a clock for `crate::send::validate_plan`'s reason: one of the
/// rules is that a picked date has not gone by, and the past is not a
/// property of a draft.
pub fn composer_problem(
    composer: &SendComposer,
    now: &dyn crate::send::SendClock,
) -> Option<&'static str> {
    crate::send::validate_plan(&composer.plan, now)
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

// ---------------------------------------------------------------------------
// Design section 5a's ACCESS block
// ---------------------------------------------------------------------------

/// Design §5a's third section eyebrow, over the Access rows. The design's own
/// word, in the design's own case, for `record_ui::RECORD_EYEBROW`'s reason:
/// [`theme::eyebrow`] deliberately does not uppercase for the caller, so the
/// constant carries the case the glyphs are painted in and a paint test can
/// look for it.
pub const ACCESS_EYEBROW: &str = "ACCESS";

/// §5a's first Access row label.
pub const EXPIRES_LABEL: &str = "Expires";
/// §5a's second Access row label. **The design's own word**, and the right
/// one: the server counts *accesses*, and "Opens" would promise that a
/// recipient who loads the page twice has spent two.
pub const VIEWS_LABEL: &str = "Views";
/// §5a's third Access row label.
pub const OPEN_WITH_LABEL: &str = "Open with";

/// §5a's own word on the link beside [`OPEN_WITH_LABEL`]'s box.
pub const GENERATE_LABEL: &str = "Generate";

/// §5a's `font-size: 12px` on that link, and the gap before it.
const ACCESS_LINK_PX: f32 = 12.0;
/// See [`ACCESS_LINK_PX`] -- §5a's `gap: 10px`.
const ACCESS_LINK_GAP: f32 = 10.0;

/// The view-limit field's placeholder, and the whole of how the row says its
/// off position. See [`views_note`].
pub const VIEWS_HINT: &str = "Any";

/// What the Access block says beside the view-limit box when there is no
/// limit. **Not an empty space**: a blank box with nothing after it is a
/// field the user is being asked to fill in, and this one is optional.
pub const NO_VIEW_LIMIT_NOTE: &str = "the link works until it expires";

/// What it says when there is one. §5a's own phrase, and it is the sentence
/// that makes the number worth typing: a cap is not "how many people may
/// read this", it is "after this many, the link is dead".
pub const VIEW_LIMIT_NOTE: &str = "then the link dies";

/// The most digits the view-limit box will take.
///
/// **A guard and not a style rule.** The box parses to a `u32`, and a `u32`
/// runs out at ten digits -- so an eleven-digit entry would fail to parse,
/// and a `parse().ok()` on a failure is `None`, which in this control means
/// *no limit at all*. A user holding a key down would have silently turned a
/// cap into an uncapped link. Six digits is far more views than any Send has,
/// and it cannot overflow.
pub const MAX_VIEW_LIMIT_DIGITS: usize = 6;

/// The view limit a box holding `text` means, or `None` for no limit.
///
/// **A pure function, and the whole of the box's rule**, so the states this
/// control can be in are assertable without running a frame -- which is this
/// file's standing rule and is worth more here than usual, because the
/// dangerous state ("the user meant a cap and got none") is invisible on
/// screen.
///
/// Non-digits are dropped rather than refused. The box is a number and the
/// only things a user types into it that are not digits are a stray letter
/// and a pasted `"3 views"`; refusing the keystroke and dropping it look the
/// same to a person typing, and dropping it means the paste does the obvious
/// thing.
///
/// An empty box is `None`, which is the off position: see
/// [`draw_access_block`] on why neither optional control has a switch in
/// front of it. A `0` is `Some(0)`, deliberately -- it is a real thing to
/// have typed and `crate::send::validate_plan` already has the sentence for
/// it ("A limit of zero views would make the link useless."). Swallowing it
/// to `None` here would turn a typo into an uncapped link with no refusal
/// anywhere.
pub fn view_limit_from(text: &str) -> Option<u32> {
    let digits: String =
        text.chars().filter(char::is_ascii_digit).take(MAX_VIEW_LIMIT_DIGITS).collect();
    digits.parse::<u32>().ok()
}

/// The note beside the view-limit box, for the limit in force.
pub fn views_note(limit: Option<u32>) -> &'static str {
    match limit {
        Some(_) => VIEW_LIMIT_NOTE,
        None => NO_VIEW_LIMIT_NOTE,
    }
}

/// The three fields of a [`crate::send::SendPlan`] design §5a's Access block
/// is a control for, borrowed rather than copied.
///
/// # Why a borrow bundle and not a struct either screen owns
///
/// The Sends screen's composer holds a whole `SendPlan` between frames and
/// the record composer builds one at submit time out of a
/// `record_ui::RecordDraft`. A shared block needs to drive both without
/// either screen converting to the other's shape -- and a conversion is
/// exactly where the validated value and the published value come apart,
/// which `SendComposer`'s own doc records as the reason it holds a plan at
/// all. Three `&mut`s cost nothing, convert nothing, and mean the block
/// writes into the buffer that will be published.
///
/// Named fields rather than three positional arguments because two of the
/// three are `Option`s of different types today and could stop being so
/// tomorrow; `AccessControls { password, .. }` cannot be got wrong at a call
/// site, which is [`theme::Segment`]'s stated reason for the same shape.
pub struct AccessControls<'a> {
    /// One of `crate::send::LIFETIME_CHOICES`, or a date the user picked.
    pub lifetime: &'a mut crate::send::SendLifetime,
    /// The share password. `None` **is** the off position -- see
    /// [`draw_access_block`].
    pub password: &'a mut Option<zeroize::Zeroizing<String>>,
    /// The view cap. `None` is no cap.
    pub max_access_count: &'a mut Option<u32>,
}

/// The width of the label column §5a's Access rows line their controls up
/// against: the design's own `width: 96px`.
const ACCESS_LABEL_WIDTH: f32 = 96.0;

/// The design's `gap: 14px` between an Access row's label and its control,
/// and between the control and the note after it.
const ACCESS_ROW_GAP: f32 = 14.0;

/// The vertical gap between one Access row and the next: the design's
/// `flex-direction: column; gap: 10px`.
const ACCESS_ROW_SPACING: f32 = 10.0;

/// The height of the two boxed controls in the Access block.
///
/// **§5a's `height: 30px` measured as a BORDER-BOX, which is 32.** The design
/// page is content-box -- `box-sizing` is never set on it -- so a `div` with
/// `height: 30px` inside `border: 1px solid #d7d3d3` occupies 32 points on
/// screen, and 30 is what is left inside the border. Reading those numbers as
/// border-box has cost this project rework more than once, which is why the
/// arithmetic is written down here rather than the answer.
///
/// 32 is [`theme::BUTTON_HEIGHT`] exactly, which is not a coincidence worth
/// hiding: it is the height every small action control in this app already
/// is, and a row whose box matched the design to the point and matched
/// nothing else in the app would be the odd one out in a card full of this
/// design system's controls.
const ACCESS_FIELD_HEIGHT: f32 = 32.0;

/// The view-limit box's width: §5a's `width: 58px` plus its two 1-point
/// borders -- [`ACCESS_FIELD_HEIGHT`]'s border-box arithmetic applied
/// sideways.
const VIEW_LIMIT_FIELD_WIDTH: f32 = 60.0;

/// The expiry dropdown's width.
///
/// A fixed box, which is the whole point of moving this row off a segmented
/// run: it no longer changes size when a label is reworded or a choice is
/// added. 150 is comfortably over the widest thing it ever has to hold -- the
/// `A date you pick…` row, which is the longest of the nine at 12px semibold
/// -- and comfortably under the **226 points** §5a's Access block actually has
/// for a control inside `record_ui`'s 360-point export modal (12-point margins
/// each side, then a 96-point label column and a 14-point gap). That budget is
/// the constraint this row failed against, and
/// `the_access_block_fits_inside_the_modal_it_is_drawn_in` is what holds it.
pub const EXPIRY_FIELD_WIDTH: f32 = 150.0;

/// The salt `theme::date_picker` remembers its month under.
///
/// A constant rather than a literal at the call site because both composers
/// draw the same block through [`draw_access_block`], and two screens that
/// each invented a salt would each remember a different month for what the
/// user experiences as one control.
const EXPIRY_CALENDAR_ID: &str = "send-expiry-calendar";

/// One row of the expiry dropdown, named.
///
/// The eight fixed rows are `crate::send::lifetime_label`'s job. The ninth has
/// no fixed value, so it says what pressing it does -- until a date has
/// actually been picked, at which point the row says that date, because a row
/// that still read "a date you pick" beside a calendar showing 14 March would
/// be the control refusing to admit it has an answer.
fn lifetime_row_label(
    choice: Option<crate::send::SendLifetime>,
    current: crate::send::SendLifetime,
    zone: &dyn crate::local_time::LocalOffset,
) -> String {
    match choice {
        Some(lifetime) => crate::send::lifetime_label(lifetime, zone),
        None if matches!(current, crate::send::SendLifetime::Until(_)) => {
            crate::send::lifetime_label(current, zone)
        }
        None => crate::send::PICK_A_DATE_LABEL.to_string(),
    }
}

/// What choosing row `index` means, given what was already chosen.
///
/// # The picked-date row keeps the deadline it inherited
///
/// Eight of the nine rows are their own answer. The ninth is "I want to name a
/// day", and it has to become *some* day the moment it is pressed, because the
/// calendar under it opens on one and the sentence above it names one.
///
/// It takes the day the current answer already lands on. Switching from
/// `30 days` to a picked date therefore changes nothing about when the link
/// dies until the user clicks a cell -- it only changes how the deadline is
/// expressed. The alternative, seeding it with today or with some invented
/// default, would silently move the deadline as a side effect of opening a
/// calendar to look at it.
///
/// `Never` has no day to inherit, so that one case falls back to the day
/// `crate::send::DEFAULT_LIFETIME` lands on. Either way the seed is clamped
/// into the window the calendar offers, so the picker never opens on a day it
/// would refuse.
fn lifetime_for_row(
    index: usize,
    current: crate::send::SendLifetime,
    now: &dyn crate::send::SendClock,
    zone: &dyn crate::local_time::LocalOffset,
) -> Option<crate::send::SendLifetime> {
    match crate::send::LIFETIME_CHOICES.get(index)? {
        Some(lifetime) => Some(*lifetime),
        None => {
            if matches!(current, crate::send::SendLifetime::Until(_)) {
                return Some(current);
            }
            let (first, last) = crate::send::pickable_window(now, zone);
            let seed = current
                .local_day(now, zone)
                .or_else(|| crate::send::DEFAULT_LIFETIME.local_day(now, zone))
                .unwrap_or(first)
                .clamp(first, last);
            Some(crate::send::SendLifetime::on_local_day(seed.0, seed.1, seed.2, zone))
        }
    }
}

/// `crate::send::pickable_window` in the shape the calendar takes it.
fn pickable_window(
    now: &dyn crate::send::SendClock,
    zone: &dyn crate::local_time::LocalOffset,
) -> (theme::Day, theme::Day) {
    let (first, last) = crate::send::pickable_window(now, zone);
    (
        theme::Day { year: first.0, month: first.1, day: first.2 },
        theme::Day { year: last.0, month: last.1, day: last.2 },
    )
}

/// The three Access row labels, in the order the block draws them, so
/// [`access_label_width`] measures exactly the set that is on screen.
const ACCESS_LABELS: [&str; 3] = [EXPIRES_LABEL, VIEWS_LABEL, OPEN_WITH_LABEL];

/// §5a's Access label type size.
const ACCESS_LABEL_PX: f32 = 13.0;

/// **How wide the label column is, for the width this block was actually
/// given.**
///
/// §5a's answer is 96 and that is the first answer this returns. It is not
/// the only one it can return, and the reason is the detail column: 250
/// points of card at `settings::MIN_VAULT_WINDOW_SIZE` is 226 inside
/// `theme::FORM_CARD_PAD_X`, and `96 + 14 + 150` is 260. Something has to
/// give, and it is worth being exact about what:
///
///   * **Not the control.** [`EXPIRY_FIELD_WIDTH`] is 150 because the widest
///     row it has to state is `A date you pick…`; a narrower box elides the
///     answer in force, which is the one thing that control exists to show.
///   * **Not the block's shape.** Stacking each label above its control at
///     narrow widths would make the same form two different forms depending
///     on how the window is dragged.
///   * **The column's WIDTH, not its existence.** §5a's claim here is that
///     three questions have their answers on one vertical line -- which is a
///     claim about the three agreeing with each other, not about the number
///     96. Measuring the widest of the three labels and giving all three that
///     keeps the claim exactly and costs the block nothing it can see.
///
/// It is a function of the room rather than a constant for the same reason
/// `action_pad_for` is: the pane is resizable, so the answer has to be too.
fn access_label_width(ui: &egui::Ui) -> f32 {
    let room = ui.available_width();
    if ACCESS_LABEL_WIDTH + ACCESS_ROW_GAP + EXPIRY_FIELD_WIDTH <= room {
        return ACCESS_LABEL_WIDTH;
    }
    ACCESS_LABELS
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(
                    (*label).to_string(),
                    egui::FontId::new(ACCESS_LABEL_PX, egui::FontFamily::Proportional),
                    theme::TEXT_SECONDARY,
                )
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
        .ceil()
}

/// One Access row: a fixed-width label, then whatever the caller draws.
///
/// The label column is fixed rather than laid out naturally because that is
/// the whole visual claim of §5a's block -- three questions whose answers
/// start on one vertical line. Laid out naturally, "Expires", "Views" and
/// "Open with" are three different widths and the three controls step
/// rightwards down the card.
///
/// **And that is what it did**, for as long as the column was drawn with
/// `allocate_ui_with_layout`. That function allocates what its CHILD ended up
/// occupying, not the size it was asked for, so the "fixed" 96-point column
/// was in fact each label's own width -- the three controls stepped rightwards
/// exactly as this doc says they must not, on both composers, in every
/// screenshot ever taken of either. The slot is now allocated first and the
/// label placed into it, which is `theme::modal_answer`'s idiom and is the
/// only one in this codebase that actually reserves a rectangle.
fn access_row<R>(
    ui: &mut egui::Ui,
    label: &str,
    label_width: f32,
    enabled: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        // The design's gap, rather than egui's default item spacing, applied
        // once for the whole row so the note after a control sits the same
        // distance from it as the control sits from its label.
        ui.spacing_mut().item_spacing.x = ACCESS_ROW_GAP;
        let (slot, _) = ui.allocate_exact_size(
            egui::vec2(label_width, ACCESS_FIELD_HEIGHT),
            egui::Sense::hover(),
        );
        let mut cell = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(slot)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        cell.label(
            egui::RichText::new(label)
                .size(ACCESS_LABEL_PX)
                .color(if enabled { theme::TEXT_SECONDARY } else { theme::TEXT_GHOST }),
        );
        body(ui)
    })
    .inner
}

/// **Design §5a's `ACCESS` block, and the one copy of it this app has.**
///
/// Both of this app's Send composers call it: `draw_composer` on the Sends
/// screen and `record_ui::draw_export_form` in the record modal. That is the
/// point of its existing. Two screens that publish a Send and answer "how
/// long does this link live, who can open it, how many times" differently is
/// the same defect this app just finished fixing in those two screens'
/// footers, and it was already halfway present: the Sends composer had a
/// lifetime row and a password tick-box, the record composer had neither, and
/// neither had ever offered the view cap that `SendPlan::max_access_count`
/// has carried and published since the feature was written.
///
/// # Three of §5a's six controls are built, and three are not
///
/// §5a's Access block draws six things. The three below are properties of a
/// Bitwarden Send and reach the wire; the other three are argued, by name, at
/// the bottom of this doc.
///
/// # Both optional controls are OFF by default, and neither has a switch
///
/// A password field that is empty means there is no password; a view box that
/// is empty means there is no cap. The composer used to put a tick-box in
/// front of the password ("Require a password to open the link") and that is
/// now gone -- a switch in front of a single text box is a second way to say
/// what the box already says, and it has a failure mode the box does not: a
/// ticked switch over an empty field is a state the user reads as "password
/// on", which `validate_plan` then has to refuse with a sentence about
/// turning the password off. The box is its own switch, and emptying it drops
/// the `Zeroizing` buffer, which wipes it -- the same guarantee
/// `set_wants_password` gave when it took the tick-box's `false`.
///
/// # What the block does NOT draw, and why each one is absent
///
/// **A recipient e-mail address**, **an "only this address can open it"
/// switch**, and **"tell me when it is opened"**. §5a draws all three, and
/// none of them is a property of a Bitwarden Send.
///
/// *The recipient and the address lock are one thing on the wire, and it does
/// not work here.* A Send's object does have an `emails` array and an
/// `authType` of `Email`, so the field is nameable -- but the mechanism
/// behind it is a one-time code the SERVER mails to that address before it
/// will hand over the content, and the server this app is built against
/// answers `501 Not Implemented` to every send whose `authType` is `Email`,
/// on create, on update and on access alike. Drawing the box would therefore
/// produce, for anyone who typed in it, a publish that fails -- or, worse,
/// against a server that accepted the field and did nothing with it, a link
/// the user believes is locked to one person and which anyone holding it can
/// open. That second outcome is the reason this is a refusal and not a
/// to-do: a lock that does not lock is worse than a visibly absent one.
///
/// *An open notification is not a Send concept at all.* There is no field on
/// the object, no endpoint that subscribes to one, and nothing in the
/// protocol that pushes to a client. It could only be built as polling --
/// this app asking the server for `accessCount` on a timer and comparing --
/// which is a different feature (a background job, a notification surface, a
/// thing that must run while the app is closed to be worth anything) wearing
/// a switch's clothes. A switch that silently means "if the app happens to be
/// open, and you happen to look" is not the switch the design drew.
///
/// **§5a's `Generate` beside the password** is also absent, and for a smaller
/// reason. This app masks a typed secret and offers no reveal on either of
/// these two forms -- `record_ui`'s seed passphrase and the composer's own
/// password are both plain masked fields. A generated share password that the
/// sender cannot read is a password they cannot tell the recipient, so
/// `Generate` is not one control but two, and the second one (a reveal) would
/// be this file inventing a third treatment for secrets on a form that has
/// two already. It is a coherent small feature and it is not this one.
///
/// `now` and `zone` are injected for `crate::send::expiry_wording`'s reason,
/// which is this whole feature's rule: nothing here reads the machine's clock
/// or its timezone, so every assertion about the sentence under the Expires
/// row is exact wherever the suite runs.
pub fn draw_access_block(
    ui: &mut egui::Ui,
    controls: AccessControls<'_>,
    enabled: bool,
    now: &dyn SendClock,
    zone: &dyn LocalOffset,
) {
    theme::eyebrow(ui, ACCESS_EYEBROW);
    ui.add_space(theme::EYEBROW_GAP);

    // Measured ONCE, at the top, and handed to all three rows: the property
    // §5a's label column is for is that the three agree, and three rows each
    // measuring for themselves is three chances to disagree by a point.
    let label_width = access_label_width(ui);

    // ---- Expires ---------------------------------------------------------
    //
    // **The segmented run is gone from this row, and the reason is that nine
    // choices are not a segmented control's question.**
    //
    // This row held `theme::segmented_control` for as long as the answer was a
    // short duration: four cells, `1 hour · 1 day · 7 days · 30 days`, one
    // control with four positions. Then the choices became nine -- months, a
    // date the user picks, and no end date at all.
    //
    // Nine will not fit, and the measurement is not close. The binding
    // container is `record_ui`'s export modal, not the composer: 360 points
    // wide with a 12-point margin each side, minus §5a's 96-point label column
    // and its 14-point gap, leaves **226 points** for the control. The four
    // cells that are there today come to a little over 200 of it. Adding
    // `3 months`, `6 months`, `12 months`, the date row and `Never` roughly
    // triples that -- a run sizes every cell to its own label, so there is no
    // setting of any constant that recovers it.
    //
    // Three shapes were considered and two were rejected:
    //
    //   * **Wrapping the run onto two or three lines.** Cheapest, and it keeps
    //     the pixels while throwing away the property that justified them. A
    //     segmented control earns its joined cells by being *one run with one
    //     lit position*; stacked into three pills that is three controls with
    //     one lit between them, which is a thing the reader has to work out
    //     rather than see. It is also 90 points of vertical space in a modal
    //     that is already scrolling.
    //
    //   * **A short run of the common choices plus a way into the rest.** Two
    //     controls for one question, and the answer in force moves between
    //     them depending on what it is -- so "what does this say right now"
    //     has two places to look. It would also have put `Never` behind a
    //     secondary surface, and `Never` is a common answer, not an advanced
    //     one: a password shared with someone in the same house is wanted
    //     again next month.
    //
    //   * **One dropdown holding all nine.** Chosen. It is a fixed box that
    //     does not grow with the set, it states the answer in force in the
    //     space the run's widest cell took, every choice including `Never` is
    //     one press from every other, and it lines up with the view-limit
    //     field below it instead of running past it. It costs one extra click
    //     on a form that already costs a name, a body and a button.
    //
    // The dropdown is `theme::dropdown` and not a widget built here, for this
    // codebase's standing rule: a control drawn privately on one screen is a
    // second design system. See that function on when a dropdown is right and
    // when a segmented run still is -- this row is the first caller, and the
    // §5a Views row's own `1 ▾` is the obvious second.
    //
    // The rows come from `send.rs` and are not spelled here: `validate_access`
    // refuses any other answer, so a row offering one would be a control that
    // cannot work.
    let labels: Vec<String> = crate::send::LIFETIME_CHOICES
        .iter()
        .map(|choice| lifetime_row_label(*choice, *controls.lifetime, zone))
        .collect();
    let choices: Vec<theme::Choice<'_>> = labels
        .iter()
        .zip(crate::send::LIFETIME_CHOICES)
        .map(|(label, choice)| theme::Choice {
            label: label.as_str(),
            // The picked-date row is the answer in force whenever the answer
            // IS a picked date, whatever date that is -- it is one row, not a
            // row per day.
            selected: match choice {
                Some(lifetime) => *controls.lifetime == lifetime,
                None => matches!(*controls.lifetime, crate::send::SendLifetime::Until(_)),
            },
        })
        .collect();
    let current = crate::send::lifetime_label(*controls.lifetime, zone);
    access_row(ui, EXPIRES_LABEL, label_width, enabled, |ui| {
        // `dropdown_disabled` and not `add_enabled_ui`, for the reason that
        // split exists in this design system (see `toggle_pill` /
        // `toggle_pill_disabled`, and the segmented run before it): the inert
        // box senses nothing, so while a publish is in flight there is no path
        // by which it can be opened at all -- and the answer in force stays at
        // full strength rather than fading with the box round it.
        if enabled {
            if let Some(index) = theme::dropdown(ui, EXPIRY_FIELD_WIDTH, &current, &choices) {
                if let Some(next) = lifetime_for_row(index, *controls.lifetime, now, zone) {
                    *controls.lifetime = next;
                }
            }
        } else {
            theme::dropdown_disabled(ui, EXPIRY_FIELD_WIDTH, &current);
        }
    });
    // **The DATE, not only the duration**, and for a sub-day lifetime the
    // clock time with it. A publishing action where being wrong about the
    // lifetime is the harm gets the thing the user can check against a
    // calendar. `expiry_wording` is `send.rs`'s own, so this line and the
    // `deletionDate` in the built JSON cannot disagree about what the choice
    // means, and the day it prints is the user's LOCAL day -- the stored
    // instant is UTC and stays UTC. For `Never` it is the one sentence that
    // says what "no end date" actually means; it does not caution, because
    // `Never` is a choice this form offers rather than one it tolerates.
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(crate::send::expiry_wording(*controls.lifetime, now, zone))
            .size(11.0)
            .color(theme::TEXT_FAINT),
    );
    // ---- the calendar, only while the answer is a date -------------------
    //
    // **A disclosure under the row, not a second popup layer.** The dropdown
    // is already a floating layer; opening a calendar inside it would mean a
    // menu whose contents change shape when one of its rows is pressed, and
    // `PopupCloseBehavior` has no reading of that which is not surprising. A
    // block that appears under the row is the pattern `detail_edit` already
    // chose for the same question, it paints in one frame so a test can read
    // it, and it leaves the answer and the means of changing it on screen
    // together.
    if matches!(*controls.lifetime, crate::send::SendLifetime::Until(_)) {
        ui.add_space(6.0);
        let (first, last) = pickable_window(now, zone);
        let selected = controls
            .lifetime
            .local_day(now, zone)
            .map(|(year, month, day)| theme::Day { year, month, day });
        // The calendar is drawn while a publish is in flight too, and it is
        // simply not asked for its answer: the chosen day stays legible, which
        // is what the row above it does, and a grid that vanished for the
        // duration of a publish would take the reader's own answer off the
        // card at the moment they are watching it be used.
        if let Some(day) = theme::date_picker(ui, EXPIRY_CALENDAR_ID, selected, first, last) {
            if enabled {
                *controls.lifetime =
                    crate::send::SendLifetime::on_local_day(day.year, day.month, day.day, zone);
            }
        }
    }
    ui.add_space(ACCESS_ROW_SPACING);

    // ---- Views -----------------------------------------------------------
    //
    // **A typed number and not §5a's `1 ▾` dropdown.** A dropdown is a fixed
    // set of answers, and there is no fixed set here: the field is a `u32` on
    // the wire and the useful values run from 1 to whatever the sender's
    // situation is. It also cannot express the off position without a "no
    // limit" row that reads as one of the numbers, where an empty box says it
    // by being empty. Everything the box means is `view_limit_from`, which is
    // a pure function and not a rule inside this closure.
    let mut typed = controls.max_access_count.map(|n| n.to_string()).unwrap_or_default();
    access_row(ui, VIEWS_LABEL, label_width, enabled, |ui| {
        // `theme::inline_field` and not a bare `egui::TextEdit`, which is what
        // stood here: the bare one wears egui's frame, egui's radius and
        // egui's one-line height, so this row's box and the `Open with` box
        // below it and the two boxes on the composer above were four
        // different-looking controls on one card. See that function.
        let changed = ui
            .add_enabled_ui(enabled, |ui| {
                theme::inline_field(ui, &mut typed, VIEWS_HINT, VIEW_LIMIT_FIELD_WIDTH, false)
            })
            .inner
            .changed();
        if changed {
            *controls.max_access_count = view_limit_from(&typed);
        }
        ui.label(
            egui::RichText::new(views_note(*controls.max_access_count))
                .size(12.0)
                .color(theme::TEXT_FAINT),
        );
    });
    ui.add_space(ACCESS_ROW_SPACING);

    // ---- Open with -------------------------------------------------------
    //
    // **The buffer is taken out of the plan and put back**, rather than a
    // `get_or_insert_with(...)` that would leave an empty `Some` behind the
    // moment the field is touched. Taken, typed into, and put back only if it
    // still holds something: an emptied box drops the `Zeroizing` here, which
    // wipes it, and leaves the plan's `password` at `None` -- which is
    // exactly "there is no password on this Send" rather than "there is an
    // empty one", the state `validate_plan` has to refuse.
    //
    // Masked with `.password(true)`, which is what BOTH of this app's forms
    // already do for a typed secret -- the composer's own password field was
    // one, and `record_ui`'s seed passphrase is another. `totp_add`'s
    // Reveal/Hide is a different job: it unmasks a secret the app is SHOWING
    // the user, not one they are typing.
    let mut buffer = controls
        .password
        .take()
        .unwrap_or_else(|| zeroize::Zeroizing::new(String::new()));
    access_row(ui, OPEN_WITH_LABEL, label_width, enabled, |ui| {
        // **§5a's `Generate`**, which this row did not have.
        //
        // The design puts it at the right end of this row and it is the one
        // control on the card that makes the share password a realistic
        // answer rather than an aspiration: a link password typed by hand is
        // a link password the recipient can guess, and the alternative on
        // offer -- opening the vault's own generator in another window,
        // copying, coming back -- is why this box was usually left empty.
        //
        // Measured first and the field given what is left, for the reason
        // every row in this app that ends in a control is laid out that way:
        // a field taking `available_width` pushes whatever follows it off the
        // card.
        let link = ui
            .painter()
            .layout_no_wrap(
                GENERATE_LABEL.to_string(),
                egui::FontId::proportional(ACCESS_LINK_PX),
                theme::BLUE,
            )
            .size()
            .x;
        let room = (ui.available_width() - link - ACCESS_LINK_GAP).max(0.0);
        ui.add_enabled_ui(enabled, |ui| {
            theme::inline_field(ui, &mut buffer, PASSWORD_HINT, room, true);
        });
        ui.add_space(ACCESS_LINK_GAP);
        if enabled && theme::link_label(ui, GENERATE_LABEL, ACCESS_LINK_PX).clicked() {
            // The vault's own generator at its own defaults -- one generator
            // in this app, not a second recipe written here. A refusal (the
            // recipe cannot produce nothing at these defaults) leaves the box
            // as it was rather than clearing a password the user had typed.
            if let Ok(made) = crate::password_gen::generate_password(
                &crate::vault_bridge::PasswordRecipe::default(),
            ) {
                buffer = made;
            }
        }
    });
    *controls.password = (!buffer.is_empty()).then_some(buffer);
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
    // **§5a's card, in §5a's three bands**, drawn by the one set of functions
    // in this app that draws them -- `record_ui`'s record composer calls the
    // same three. See `theme::form_card` for what they are and why they are
    // there rather than here.
    theme::form_card(ui, |ui| {
        theme::form_card_header(ui, COMPOSER_HEADING);
        theme::form_card_body(ui, |ui| {
            // **Both inputs get an eyebrow, and the decision is worth
            // stating because the easy answer was to leave them bare.**
            //
            // §5a's card is three eyebrowed blocks -- `Record`, `Include`,
            // `Access` -- and the eyebrows are not decoration: they are what
            // makes the card read as a sequence of decisions ("which record",
            // "what travels", "who can open it") rather than as a stack of
            // widgets. This composer had exactly one of the three, over the
            // Access block, because that block was lifted from §5a wholesale.
            // One eyebrow is the worst of the three available answers: it
            // says the card has sections and then labels only the last one,
            // so the top half reads as preamble to the part that is properly
            // designed.
            //
            // The two inputs ARE this screen's counterparts of §5a's first
            // two blocks. A text Send has no record to pick and no field list
            // to narrow; what it has is a name (which identifies the Send the
            // way §5a's chip identifies its record) and a body (which is what
            // travels, the way §5a's tick list is). Same two questions, two
            // boxes instead of a chip and a list.
            //
            // The eyebrow and the placeholder overlap while the box is empty
            // -- `NAME` over `Name this Send` -- and that is accepted rather
            // than fixed by deleting one. The placeholder is the only thing
            // in the box for the few seconds before the user types, and the
            // eyebrow is the only thing left once they have; neither covers
            // the other's half of the field's life.
            theme::eyebrow(ui, NAME_EYEBROW);
            ui.add_space(theme::EYEBROW_GAP);
            ui.add_enabled_ui(enabled, |ui| {
                theme::hinted_field(ui, &mut composer.plan.name, NAME_HINT, false);
            });
            ui.add_space(theme::BLOCK_GAP);

            theme::eyebrow(ui, TEXT_EYEBROW);
            ui.add_space(theme::EYEBROW_GAP);
            // `theme::text_area` and not `egui::TextEdit::multiline`, which
            // is what stood here. The body is the one field on this card that
            // the design system had no box for, so it was drawn as a bare
            // multiline -- egui's frame, egui's radius, no focus halo -- and
            // sat directly under a name field that was drawn the same bare
            // way. Two boxes, two chromes, neither of them this app's. The
            // fix is a primitive in `theme.rs`, not a private one here: that
            // rule has already paid for itself twice on this screen.
            ui.add_enabled_ui(enabled, |ui| {
                theme::text_area(ui, &mut composer.plan.text, TEXT_HINT, BODY_ROWS);
            });

            ui.add_space(theme::BLOCK_GAP);
            // **Design §5a's ACCESS block, drawn by the one function that
            // draws it.**
            //
            // What stood here was this screen's own arrangement of two of
            // §5a's six Access controls: a `field_label` reading "The link
            // stops working after" over a segmented run, then a tick-box
            // reading "Require a password to open the link" over a masked
            // field. The record composer in `record_ui` had neither, and
            // neither screen had ever offered the view cap that
            // `SendPlan::max_access_count` has published since the feature
            // was written.
            //
            // **The composer gets the block too, and the argument is the one
            // the footers were just fixed under.** The design page has no
            // counterpart for this screen at all, so "§5a says so" is not
            // available here and something else has to carry the decision. It
            // is this: "how long does this link live", "who can open it" and
            // "how many times" are questions about a Bitwarden Send, not
            // about a screen, and this app has two screens that publish one.
            // Two answers to one question is exactly what the matched pair of
            // footer buttons was -- and the harm here is not cosmetic: a user
            // who finds the view cap on the record composer and comes looking
            // for it here would conclude the app cannot cap a text Send,
            // which is false, and has been false since the field was plumbed.
            //
            // The one thing lost is this screen's own wording, and it was
            // worth losing. "The link stops working after" is a better
            // sentence than "Expires" and a worse LABEL: it reads as a
            // sentence only because it sat alone above its control, and there
            // is no arrangement of three such sentences that lines three
            // controls up on one left edge, which is §5a's whole visual claim
            // for this block. The sentence is not gone -- `expiry_wording`
            // still prints "The link stops working after 7 days -- on ..."
            // under the row, where it now carries the date as well.
            draw_access_block(
                ui,
                AccessControls {
                    lifetime: &mut composer.plan.lifetime,
                    password: &mut composer.plan.password,
                    max_access_count: &mut composer.plan.max_access_count,
                },
                enabled,
                now,
                zone,
            );
        });

        let problem = composer_problem(composer, now);
        let can_submit = composer_can_submit(problem, in_flight);
        // **The footer's two answers are the design system's two
        // buttons**, in the design's own footer BAND.
        //
        // §5a's composer ends in a filled blue `Create & copy link`
        // beside an outlined white `Cancel`, on a tinted strip closed off
        // from the body by a hairline -- a primary and a secondary, which
        // is the whole of what a form footer says about which of its two
        // answers is the one it is for. The buttons were fixed an earlier
        // pass ago; the strip is new, and `theme::form_card_footer` argues
        // why a row of buttons that is simply the last thing in the body
        // is not a footer.
        //
        // **32 points and a 7px radius, not §5a's 34 and 8.** The design
        // draws this card's buttons two points taller than every other
        // action button in the app (3h's Continue, the detail pane's
        // Save, "Fill in app"), all of which are `theme::BUTTON_HEIGHT`.
        // `theme.rs` already refused to move those five to match one
        // outlier and gave the outlier its own function instead
        // (`primary_button_matching_field`, for 2b's `+ New`, which is
        // 34/8 because it matches the *search box* it sits beside). There
        // is no box beside this one to match: the Send composer's footer
        // is an action footer like every other, so it takes the action
        // footer's metrics rather than becoming the app's second
        // almost-32.
        theme::form_card_footer(ui, |ui| {
            // **§5a's footer has ONE slot at its right-hand end, and three
            // things in this app want it.** The design puts a standing note
            // there (`Appears in Shared`); this form also has a sentence
            // saying why the primary is off, and a word saying a publish is
            // running. Until this pass the first did not exist, and the
            // other two were loose `ui.label`s sitting after the buttons in
            // the body -- prose at the end of a control row, which is what
            // the owner saw as `Give the Send a name.` floating beside
            // `Discard`.
            //
            // They share the slot rather than stacking, and the order is
            // the order of how much the reader needs each one. A publish in
            // flight is the only thing worth saying while it runs. A
            // refusal is a stronger claim on the slot than the standing
            // note is: while the form cannot be submitted, "why not" is
            // what the reader is looking for, and `APPEARS_IN_SENDS` is a
            // fact about a Send that does not exist yet. Once the form is
            // submittable there is no refusal to print, and the note is
            // what is left to say. The three are never wanted together, so
            // one slot is not a compromise.
            let note = if in_flight {
                CREATING_LABEL
            } else {
                problem.unwrap_or(APPEARS_IN_SENDS)
            };
            // **Beside the answers when it fits, on its own line when it
            // does not**, measured rather than assumed; see
            // `theme::form_footer_note_width` for why §5a's right-hand
            // placement cannot simply be taken at the window floor.
            let mut beside = false;
            ui.horizontal(|ui| {
                if theme::primary_button_enabled(ui, CREATE_LABEL, None, can_submit).clicked()
                {
                    action = Some(SendUiAction::SubmitSend);
                }
                ui.add_space(theme::FORM_FOOTER_GAP);
                // `add_enabled_ui` rather than `add_enabled`, because
                // `secondary_button` is a `ui.add` with no enabled twin and
                // the whole control -- fill, outline and label -- has to
                // fade together.
                if ui
                    .add_enabled_ui(enabled, |ui| theme::secondary_button(ui, DISCARD_LABEL))
                    .inner
                    .clicked()
                {
                    action = Some(SendUiAction::CancelComposer);
                }
                if theme::form_footer_note_width(ui, note) + theme::FORM_FOOTER_GAP
                    <= ui.available_width()
                {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::form_footer_note(ui, note);
                    });
                    beside = true;
                }
            });
            if !beside {
                ui.add_space(theme::FORM_FOOTER_GAP);
                theme::form_footer_note(ui, note);
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

    /// The offset every dated assertion in this module stands at. Injected
    /// for `send::expiry_wording`'s stated reason: no test in this crate reads
    /// the machine's timezone, so no assertion here says something different
    /// on a runner in another one.
    const UTC: crate::local_time::FixedOffset = crate::local_time::FixedOffset(0);

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
            ..live()
        }
    }

    /// The five state fields of a Send nobody has touched: no cap, never
    /// opened, not disabled, no separate expiry, no share password.
    ///
    /// **A base for `..`, not a `Default` on the type.** A defaulted
    /// `SendSummary` would have no id and no link, which `parse_send_list`
    /// refuses to produce and the screen refuses to act on; giving the
    /// production type a constructor for that shape so that tests could be
    /// three lines shorter is the trade this file has declined before.
    fn live() -> SendSummary {
        SendSummary {
            id: String::new(),
            name: String::new(),
            access_url: String::new(),
            deletion_date: String::new(),
            is_file: false,
            max_access_count: None,
            access_count: 0,
            disabled: false,
            expiration_date: String::new(),
            has_password: false,
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
        let row = row_from(&summary("alpha", false, 3), &FixedClock(NOW), &UTC);
        assert_eq!(row.id, "id-alpha");
        assert_eq!(row.name, "alpha");
        assert_eq!(row.access_url, "https://send.bitwarden.com/#/alpha");
        assert_eq!(row.expiry, "Expires in 3 days");
        assert!(!row.is_file);
        assert_eq!(row.state, crate::send::SendState::Waiting);
    }

    /// **The second line says only what there is to say.**
    ///
    /// A plain Send is its expiry and nothing else; every other segment
    /// appears because a fact exists to report. The case that would be easy
    /// to get wrong is the last one: an uncapped link nobody has opened must
    /// not say "0 views", which reads as a failure rather than as a
    /// beginning.
    #[test]
    fn the_subtitle_adds_a_segment_only_where_there_is_a_fact_to_add() {
        let clock = FixedClock(NOW);
        let plain = summary("alpha", false, 7);
        assert_eq!(row_subtitle(&plain, &clock), "Expires in 7 days");

        assert_eq!(
            row_subtitle(
                &SendSummary { max_access_count: Some(10), access_count: 3, ..plain.clone() },
                &clock
            ),
            "3 of 10 views \u{00b7} Expires in 7 days"
        );
        assert_eq!(
            row_subtitle(&SendSummary { access_count: 1, ..plain.clone() }, &clock),
            "opened once \u{00b7} Expires in 7 days"
        );
        assert_eq!(
            row_subtitle(&SendSummary { access_count: 4, ..plain.clone() }, &clock),
            "opened 4 times \u{00b7} Expires in 7 days"
        );
        assert_eq!(
            row_subtitle(
                &SendSummary {
                    has_password: true,
                    max_access_count: Some(1),
                    ..plain.clone()
                },
                &clock
            ),
            "Password required \u{00b7} 0 of 1 views \u{00b7} Expires in 7 days",
            "the segments are in the design's order: what it needs, how many are left, when \
             it ends"
        );
        // **The absence of a password is not a segment.** See `row_subtitle`:
        // the composer's password is off by default, so "link only" on every
        // row is four words of nothing on the majority of this app's Sends.
        assert!(
            !row_subtitle(&plain, &clock).contains("link"),
            "an unprotected Send announced that it is unprotected"
        );

        // **A dead link makes no promise about when it ends.** Each of the
        // three ways to die, and none of them may carry a future expiry
        // beside a pill that says the link is already gone.
        for (why, send) in [
            (
                "out of views",
                SendSummary { max_access_count: Some(1), access_count: 1, ..plain.clone() },
            ),
            (
                "expired while the record lives on",
                SendSummary {
                    expiration_date: "2026-08-01T00:00:00.000Z".to_string(),
                    ..plain.clone()
                },
            ),
            ("revoked", SendSummary { disabled: true, ..plain.clone() }),
        ] {
            let line = row_subtitle(&send, &clock);
            assert!(
                !line.contains("Expires"),
                "a Send that is {why} still says {line:?} -- the pill beside it says the link \
                 is gone, so those words are the only false thing on the row"
            );
        }
        assert_eq!(
            row_subtitle(
                &SendSummary { max_access_count: Some(1), access_count: 1, ..plain },
                &clock
            ),
            "1 of 1 views",
            "a spent Send lost the one fact that is still true about it"
        );
    }

    #[test]
    fn a_send_with_no_name_is_still_a_row_with_something_in_it() {
        let mut send = summary("x", false, 1);
        send.name = "   ".to_string();
        assert_eq!(row_from(&send, &FixedClock(NOW), &UTC).name, "(no name)");
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
        let rows = rows_from(&sends, &FixedClock(NOW), &UTC);
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
        let state = pane_state(Some(&Ok(Vec::new())), &FixedClock(NOW), &UTC);
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
            let state = pane_state(Some(&Err(failure.clone())), &FixedClock(NOW), &UTC);
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
        let ambiguous = pane_state(Some(&Err(SendError::TimedOut)), &FixedClock(NOW), &UTC);
        assert_eq!(
            ambiguous,
            SendPaneState::Failed {
                message: SendError::TimedOut.user_message().to_string(),
                ambiguous: true,
            }
        );
        let plain = pane_state(Some(&Err(SendError::Offline)), &FixedClock(NOW), &UTC);
        match plain {
            SendPaneState::Failed { ambiguous, .. } => {
                assert!(!ambiguous, "an unambiguous failure claimed it might have missed some")
            }
            other => panic!("an offline failure rendered as {other:?}"),
        }
    }

    #[test]
    fn an_unanswered_fetch_is_loading_and_never_empty() {
        let state = pane_state(None, &FixedClock(NOW), &UTC);
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
        let state = pane_state(Some(&result), &FixedClock(NOW), &UTC);
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
        let state = pane_state(Some(&result), &FixedClock(NOW), &UTC);
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
            ..live()
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
            pane_state(fetch.result.as_ref(), &FixedClock(NOW), &UTC),
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
            composer_problem(&composer, &FixedClock(NOW)).is_some(),
            "control: a freshly opened composer is empty, so the form must refuse it"
        );
        assert!(
            !composer_can_submit(composer_problem(&composer, &FixedClock(NOW)), false),
            "an empty draft could be submitted, which would publish an empty Send under a \
             public link"
        );

        composer.plan.name.push_str("a name");
        composer.plan.text.push_str("a body");
        assert_eq!(
            composer_problem(&composer, &FixedClock(NOW)),
            None,
            "control: a filled draft is still refused, so the `None` row above is not a \
             verdict this form ever reaches"
        );
        assert!(
            composer_can_submit(composer_problem(&composer, &FixedClock(NOW)), false),
            "a draft the form accepts could not be submitted"
        );
        assert!(
            !composer_can_submit(composer_problem(&composer, &FixedClock(NOW)), true),
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
                // **Two warm-up frames, because the pane now paints in this
                // app's own faces.** `theme::apply`'s families only exist
                // from the frame AFTER it is called, and the list column's
                // strip draws a letterspaced eyebrow in `theme::BOLD`;
                // without this the draw panics inside epaint with
                // "FontFamily::Name(..) is not bound to any fonts" and the
                // count below is never reached. That would make this test
                // fail loudly rather than silently, which is why it is a
                // fixture fix and not a hole -- but a panic is not the
                // property being asserted.
                let _ = ctx.run_ui(Default::default(), |_ui| {});
                theme::apply(&ctx);
                let _ = ctx.run_ui(Default::default(), |_ui| {});
                let _ = ctx.run_ui(Default::default(), |ui| {
                    // Deliberately dropped rather than applied: this is the
                    // shape every measured shadow reduces to.
                    let _ = draw_send_pane(ui, &state, None, SendDeleteView::default(), SendView::Mine(crate::vault_window::sidebar::SendScope::All), &mut None, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0));
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

    /// The offset every assertion in this module stands at. Injected for
    /// `send::expiry_wording`'s stated reason: no test in this crate reads
    /// the machine's timezone.
    const UTC: crate::local_time::FixedOffset = crate::local_time::FixedOffset(0);

    /// The vault window's centre pane at the **minimum window size**: 900x600
    /// is `settings::MIN_VAULT_WINDOW_SIZE`, less the sidebar's 212. If the
    /// window floor ever moves, this is measured off the constant and moves
    /// with it.
    ///
    /// **The width is now exact, and it used to carry a 40pt "generous
    /// allowance for the titlebar and chrome".** That allowance was harmless
    /// while this pane was one column -- it only ever made the assertions
    /// stricter than reality -- and it stopped being harmless the moment the
    /// pane became a fixed 390pt list beside a detail that takes what is
    /// left: 40pt off the width is 40pt off the DETAIL alone, which is a
    /// sixth of its content box at the floor, and a control row tuned against
    /// that number would have been tuned against a pane narrower than any
    /// user can make. The height keeps its allowance, where it is still only
    /// a conservative guess at the chrome above.
    fn min_pane_size() -> egui::Vec2 {
        let (w, h) = crate::settings::MIN_VAULT_WINDOW_SIZE;
        egui::vec2(w as f32 - crate::vault_window::SIDEBAR_WIDTH, h as f32 - 120.0)
    }

    /// The width the DETAIL column has at [`min_pane_size`], which is the
    /// number every control in that column has to fit inside.
    fn min_detail_width() -> f32 {
        min_pane_size().x - crate::vault_window::LIST_WIDTH
    }

    #[derive(Default)]
    struct Painted {
        text: Vec<String>,
        rects: Vec<egui::Rect>,
        /// The rect of each painted text run, by its text.
        text_rects: Vec<(String, egui::Rect)>,
        /// Each filled rectangle **with the colour it was filled in**.
        ///
        /// `rects` above drops the fill, which is fine for the geometry
        /// assertions it was added for and useless for the only question a
        /// test can ask about a button: what colour is it. A segmented
        /// control's chosen cell and a primary button are both "a rectangle
        /// somewhere behind a label" until the fill is read.
        fills: Vec<(egui::Rect, egui::Color32)>,
        /// Each rectangle's **outline**, with the colour it was stroked in.
        ///
        /// `fills` cannot answer for the composer's card: `theme::form_card`
        /// paints its edge as a separate transparent-filled `rect_stroke`
        /// after the contents (see that function for why), so the card's
        /// border arrives as a shape with no fill at all and would be
        /// invisible to every assertion in this module.
        strokes: Vec<(egui::Rect, egui::Color32)>,
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

        /// Every rect a run of **exactly** `label` was painted in.
        ///
        /// **Exact and not `contains`, which is what the two above do.** That
        /// is right for a sentence -- "is this paragraph on screen" -- and
        /// wrong for a control, and the difference is now load-bearing: the
        /// detail pane's activity line for a revoked Send reads "You turned
        /// the link off -- Switch on to let it work again", so a substring
        /// count of `Switch on` cannot tell the sentence from the button it
        /// names. Naming the button in prose is good writing and the test
        /// has to be the thing that adapts.
        fn rects_of_exact(&self, label: &str) -> Vec<egui::Rect> {
            self.text_rects
                .iter()
                .filter(|(t, _)| t == label)
                .map(|(_, r)| *r)
                .collect()
        }

        fn count_exact(&self, label: &str) -> usize {
            self.rects_of_exact(label).len()
        }

        /// Runs of exactly `label` painted in the LIST column, and in the
        /// DETAIL pane, told apart by which side of `LIST_WIDTH` they fall.
        ///
        /// **The two columns draw some of the same words** -- 5b puts the
        /// state pill in the list and again in the detail header -- so a
        /// whole-pane count can no longer say where a thing is. These are
        /// what the layout assertions ask instead, and they are more precise
        /// than the counts they replaced rather than less: "the pill is in
        /// the list" and "the pill is in the detail" are two claims where
        /// there used to be one.
        fn in_list(&self, label: &str) -> usize {
            self.rects_of_exact(label)
                .iter()
                .filter(|r| r.center().x < crate::vault_window::LIST_WIDTH)
                .count()
        }

        fn in_detail(&self, label: &str) -> usize {
            self.rects_of_exact(label)
                .iter()
                .filter(|r| r.center().x >= crate::vault_window::LIST_WIDTH)
                .count()
        }

        /// The **smallest** filled rectangle containing `inner`, and its fill
        /// -- the control a label sits on rather than the card the control
        /// sits on. Smallest and not first, because the card, the pane and
        /// any wrapping frame all contain the label too.
        fn fill_behind(&self, inner: egui::Rect) -> Option<(egui::Rect, egui::Color32)> {
            self.fills
                .iter()
                .filter(|(r, _)| r.contains_rect(inner))
                .min_by(|a, b| {
                    let area = |r: &egui::Rect| r.width() * r.height();
                    area(&a.0).partial_cmp(&area(&b.0)).expect("finite rects")
                })
                .copied()
        }

        /// [`Self::fill_behind`], found from a control's own words.
        fn control_under(&self, label: &str) -> (egui::Rect, egui::Color32) {
            let text = self
                .rect_of(label)
                .unwrap_or_else(|| panic!("{label:?} was not painted at all: {:?}", self.text));
            self.fill_behind(text).unwrap_or_else(|| {
                panic!("{label:?} was painted with no filled rectangle behind it at all")
            })
        }
    }

    fn collect(shape: &egui::Shape, out: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => {
                out.text.push(text.galley.text().to_owned());
                out.text_rects
                    .push((text.galley.text().to_owned(), text.visual_bounding_rect()));
            }
            egui::Shape::Rect(rect) => {
                out.rects.push(rect.rect);
                out.fills.push((rect.rect, rect.fill));
                out.strokes.push((rect.rect, rect.stroke.color));
            }
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

    /// The row every fixture describes in the detail pane unless it says
    /// otherwise: the FIRST one.
    ///
    /// **Every control on this screen except Refresh and New Send lives in
    /// the detail pane now**, so a fixture that picked nothing would paint a
    /// list beside the words "Pick a Send..." and every assertion about a
    /// control would be an assertion about a pane that is not showing one.
    /// Picking the first row is what a user does before they can press
    /// anything, so it is what the default fixture does; [`paint_picking`]
    /// and [`click_nth_picking`] take a different row where the test is about
    /// WHICH row the control acted on.
    fn first_row_id(state: &SendPaneState) -> Option<String> {
        match state {
            SendPaneState::Rows(rows) => rows.first().map(|row| row.id.clone()),
            _ => None,
        }
    }

    /// The unfiltered SHARING screen, which is what every fixture here draws
    /// unless it is about the sub-filters.
    const ALL_SENDS: crate::vault_window::sidebar::SendScope =
        crate::vault_window::sidebar::SendScope::All;

    /// [`paint`], with the window's delete state as the pane would really be
    /// handed it.
    fn paint_with(
        state: &SendPaneState,
        notice: Option<&str>,
        size: egui::Vec2,
        delete: SendDeleteView<'_>,
    ) -> (Painted, SendUiAction) {
        paint_picking(state, notice, size, delete, first_row_id(state))
    }

    /// [`paint_with`], describing a NAMED row in the detail pane.
    fn paint_picking(
        state: &SendPaneState,
        notice: Option<&str>,
        size: egui::Vec2,
        delete: SendDeleteView<'_>,
        picked: Option<String>,
    ) -> (Painted, SendUiAction) {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut selected = picked;
        let mut action = SendUiAction::None;
        let output = ctx.run_ui(input(), |ui| {
            action = draw_send_pane(ui, state, notice, delete, SendView::Mine(ALL_SENDS), &mut selected, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });

        let mut painted = Painted::default();
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

    /// [`paint_with`], with the composer OPEN and in whatever draft the
    /// caller wants.
    ///
    /// A fixture of its own because every other one in this module builds a
    /// `SendComposer::default()`, which is closed -- so nothing the form
    /// draws has ever been reachable from here, and the composer's own
    /// controls have until now been asserted only through `vault_window`'s
    /// whole-window matrix.
    fn paint_composer(composer: &mut SendComposer, in_flight: bool) -> Painted {
        // Wider than [`min_pane_size`] on purpose: the composer now lives in
        // the DETAIL column, so a 720pt pane leaves it 330 -- and every
        // assertion below is about the form's own internal geometry, which
        // this file already pins against the pane floor in
        // `every_row_of_the_lifetime_picker_fits_the_box`.
        paint_composer_at(composer, in_flight, ROOMY_COMPOSER_PANE)
    }

    /// The comfortable pane [`paint_composer`] draws in.
    const ROOMY_COMPOSER_PANE: egui::Vec2 =
        egui::vec2(720.0 + crate::vault_window::LIST_WIDTH, 900.0);

    /// **The two panes every assertion about the composer's CARD is made
    /// against**, named together so a test cannot quietly be written at one
    /// of them.
    ///
    /// The first is [`ROOMY_COMPOSER_PANE`]; the second is the pane floor's
    /// own width, where the detail column is 298 points and the card is 250.
    /// That second number is the one this form has only ever been in danger
    /// at -- §5a's Access block does not fit inside it at the design's own
    /// measurements, which is why `access_label_width` exists -- and it is
    /// this file's standing rule that every new paint test on this screen is
    /// run at it.
    ///
    /// The narrow pane is made TALL rather than 600 high, and that is not a
    /// cheat: egui culls shapes entirely outside the screen rect, so a form
    /// longer than the window would come back as missing widgets rather than
    /// as a scroll bar, and every assertion below would be measuring the top
    /// of the card. The axis under test is the width.
    fn composer_panes() -> [(&'static str, egui::Vec2); 2] {
        [
            ("the roomy pane", ROOMY_COMPOSER_PANE),
            ("the window floor", egui::vec2(min_pane_size().x, 1600.0)),
        ]
    }

    /// [`paint_composer`], in a pane of the caller's choosing.
    fn paint_composer_at(
        composer: &mut SendComposer,
        in_flight: bool,
        size: egui::Vec2,
    ) -> Painted {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let state = SendPaneState::Empty;
        let output = ctx.run_ui(input(), |ui| {
            let _ = draw_send_pane(
                ui,
                &state,
                None,
                SendDeleteView::default(),
                SendView::Mine(ALL_SENDS),
                &mut None,
                composer,
                in_flight,
                &crate::send::FixedClock(NOW),
                &crate::local_time::FixedOffset(0),
            )
            .into_action();
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        assert!(
            painted.has(COMPOSER_HEADING),
            "the fixture did not open the composer at all, so every assertion below would be \
             about the empty pane: {:?}",
            painted.text
        );
        painted
    }

    /// A draft with **no name**, so the composer refuses to submit: the state
    /// the owner screenshotted, and the one every assertion about the off
    /// look and the refusal sentence is made in.
    fn unfinished_composer() -> SendComposer {
        let composer = SendComposer { open: true, ..SendComposer::default() };
        assert!(
            composer_problem(&composer, &FixedClock(NOW)).is_some(),
            "the fixture wants a draft the form refuses, and this one is submittable"
        );
        composer
    }

    /// **The design-system BOX a label sits in**, and not whatever rectangle
    /// happens to be smallest around it.
    ///
    /// [`Painted::control_under`] takes the smallest filled rect containing
    /// the text, which is right for a button and wrong for a field: every box
    /// in `theme.rs` puts a frameless `egui::TextEdit` inside itself, and a
    /// frameless `TextEdit` still paints its own transparent frame at the
    /// text's inset rect. That rect is smaller than the box and ten points to
    /// the right of it, so a test written against `control_under` measures a
    /// 15-point-tall field starting inside its own padding -- which is how
    /// this pair of assertions failed the first time they were run against a
    /// form that was in fact correct.
    fn box_under(painted: &Painted, label: &str) -> egui::Rect {
        let text = painted
            .rect_of(label)
            .unwrap_or_else(|| panic!("{label:?} was not painted at all: {:?}", painted.text));
        let area = |r: &egui::Rect| r.width() * r.height();
        painted
            .fills
            .iter()
            .filter(|(rect, colour)| *colour == theme::CARD && rect.contains_rect(text))
            .map(|(rect, _)| *rect)
            .min_by(|a, b| area(a).total_cmp(&area(b)))
            .unwrap_or_else(|| {
                panic!("{label:?} is not inside any box this design system painted")
            })
    }

    /// The bottom-most rectangle painted in `fill`, which for the composer's
    /// footer band is the band: nothing else on the card is that colour, and
    /// the modal frame's own tinted footer is not on this screen.
    fn band_of(painted: &Painted, fill: egui::Color32) -> egui::Rect {
        painted
            .fills
            .iter()
            .filter(|(_, colour)| *colour == fill)
            .map(|(rect, _)| *rect)
            .max_by(|a, b| a.bottom().total_cmp(&b.bottom()))
            .unwrap_or_else(|| panic!("nothing at all was painted in {fill:?}"))
    }

    /// **The composer is a CARD: an edge, a rounded corner and a footer band
    /// -- not a form laid flat on the pane.**
    ///
    /// The defect this pins is the one the owner opened with. The form was
    /// `Frame::new().fill(theme::CARD)` on `theme::CANVAS`, which is white on
    /// a two-value-off grey with no border and no shadow, and its two answers
    /// were simply the last widgets in the body. Design §5a draws a bordered,
    /// rounded, shadowed card whose footer is a tinted strip closed off by a
    /// hairline, and none of those four things was present.
    ///
    /// Asserted on the STROKE and not only on a fill, because that is what
    /// the card's edge is: `theme::form_card` paints it last as a
    /// transparent-filled outline, so a test that read fills alone would pass
    /// against a card with no border at all -- which is exactly the state
    /// being fixed.
    #[test]
    fn the_composer_is_a_card_with_an_edge_and_a_tinted_footer() {
        for (where_, pane) in composer_panes() {
            let painted = paint_composer_at(&mut open_composer(), false, pane);

            let heading = painted
                .rect_of(COMPOSER_HEADING)
                .unwrap_or_else(|| panic!("{where_}: the heading was not painted"));
            let edge = painted
                .strokes
                .iter()
                .filter(|(rect, colour)| {
                    *colour == theme::BORDER_STRONG && rect.contains_rect(heading)
                })
                .map(|(rect, _)| *rect)
                .max_by(|a, b| {
                    let area = |r: &egui::Rect| r.width() * r.height();
                    area(a).total_cmp(&area(b))
                })
                .unwrap_or_else(|| {
                    panic!(
                        "{where_}: nothing outlined in the design's card border encloses the \
                         composer's heading -- the form is drawn flat on the pane, which is \
                         the report this pass was opened over"
                    )
                });

            let band = band_of(&painted, theme::CARD_TINT);
            assert!(
                edge.contains_rect(band),
                "{where_}: the tinted footer band at {band:?} is not inside the card at \
                 {edge:?}"
            );
            assert!(
                (band.bottom() - edge.bottom()).abs() <= 1.5,
                "{where_}: the footer band stops {}pt above the card's own bottom edge -- a \
                 band that does not reach the corner is a stripe",
                edge.bottom() - band.bottom()
            );
            assert!(
                (band.width() - edge.width()).abs() <= 2.5,
                "{where_}: the footer band is {}pt wide inside a {}pt card, so it reads as a \
                 box in the footer rather than as the footer",
                band.width(),
                edge.width()
            );

            let create = painted
                .rect_of(CREATE_LABEL)
                .unwrap_or_else(|| panic!("{where_}: the submit was not painted"));
            assert!(
                band.contains_rect(create),
                "{where_}: the submit is at {create:?}, outside the footer band at {band:?} \
                 -- the answers are still the last things in the body"
            );
        }
    }

    /// **§5a's Access block is three questions whose answers start on one
    /// vertical line, and for the whole life of this block they did not.**
    ///
    /// `access_row` reserved its label column with
    /// `Ui::allocate_ui_with_layout`, which allocates what the child ended up
    /// occupying rather than the size it was asked for -- so the "fixed" 96
    /// points was in fact each label's own width, and `Expires`, `Views` and
    /// `Open with` being three different widths stepped their three controls
    /// rightwards down the card. It was in every screenshot ever taken of
    /// either composer and no test could see it, because every assertion on
    /// this block was about one row at a time.
    ///
    /// The floor pane is the half of this that matters most: 96 + 14 + 150 is
    /// 260 against 226 of card, so the design's own column does not fit and
    /// `access_label_width` narrows it. What must survive that is not the 96
    /// -- it is the three agreeing.
    #[test]
    fn the_access_rows_start_their_controls_on_one_line() {
        for (where_, pane) in composer_panes() {
            let mut composer = open_composer();
            let painted = paint_composer_at(&mut composer, false, pane);
            let chosen = crate::send::lifetime_label(composer.plan.lifetime, &UTC);

            let expires = box_under(&painted, &chosen);
            let views = box_under(&painted, VIEWS_HINT);
            let open_with = box_under(&painted, PASSWORD_HINT);

            for (name, rect) in [("Views", views), ("Open with", open_with)] {
                assert!(
                    (rect.left() - expires.left()).abs() <= 1.0,
                    "{where_}: the {name} control starts at {} against the Expires control's \
                     {} -- §5a's Access block is a label column, and controls that step \
                     rightwards are what having one prevents",
                    rect.left(),
                    expires.left()
                );
            }
        }
    }

    /// **The footer's right-hand slot carries the refusal while there is one
    /// and the standing note once there is not.**
    ///
    /// §5a ends its footer with a standing note (`Appears in Shared`); this
    /// form had none, and instead left the sentence explaining the grey
    /// button as loose prose after the buttons -- `Give the Send a name.`
    /// floating beside `Discard`, which is what the owner saw. Both now go in
    /// the one place §5a defines for a note about the card, and the order
    /// between them is argued at the call site.
    #[test]
    fn the_footers_note_says_the_refusal_first_and_then_where_the_send_goes() {
        for (where_, pane) in composer_panes() {
            let mut unfinished = unfinished_composer();
            let refusing = paint_composer_at(&mut unfinished, false, pane);
            let problem = composer_problem(&unfinished, &FixedClock(NOW))
                .expect("the fixture is a refusing draft");
            assert!(
                refusing.has(problem),
                "{where_}: the form refuses to submit and does not say why: {:?}",
                refusing.text
            );
            assert!(
                !refusing.has(APPEARS_IN_SENDS),
                "{where_}: the standing note is printed beside a refusal, so the footer says \
                 two things at once in a slot that holds one"
            );

            let ready = paint_composer_at(&mut open_composer(), false, pane);
            assert!(
                ready.has(APPEARS_IN_SENDS),
                "{where_}: a submittable draft's footer says nothing about where the Send \
                 ends up -- §5a's standing note is the whole of what that slot is for: {:?}",
                ready.text
            );

            let band = band_of(&ready, theme::CARD_TINT);
            let note = ready
                .rect_of(APPEARS_IN_SENDS)
                .unwrap_or_else(|| panic!("{where_}: the note was not painted"));
            assert!(
                band.contains_rect(note),
                "{where_}: the standing note at {note:?} is outside the footer band at \
                 {band:?} -- it is prose after the card again"
            );
        }
    }

    /// **A `Create link` that cannot be pressed is not a pale blue one.**
    ///
    /// The owner's second report on this screenshot. `primary_button_enabled`
    /// used to run its button inside a disabled `Ui`, which fades
    /// `theme::BLUE` toward the window colour -- a washed-out version of the
    /// live fill, which reads as the button rendered oddly rather than as an
    /// action that is not available. The design system already had a
    /// vocabulary for "switched off" one row up the same card
    /// (`theme::disabled_text_field`), and this asserts the button now speaks
    /// it: `theme::OFF_FILL`, not a blend of `BLUE`.
    ///
    /// The live case is asserted beside it, because "the disabled button is
    /// grey" passes just as well on a form that has no live state at all.
    #[test]
    fn a_create_link_that_cannot_be_pressed_is_not_a_pale_blue_one() {
        for (where_, pane) in composer_panes() {
            let live = paint_composer_at(&mut open_composer(), false, pane);
            let (_, on) = live.control_under(CREATE_LABEL);
            assert_eq!(
                on,
                theme::BLUE,
                "{where_}: a submittable composer's primary is not the design's blue"
            );

            let refusing = paint_composer_at(&mut unfinished_composer(), false, pane);
            let (_, off) = refusing.control_under(CREATE_LABEL);
            assert_eq!(
                off,
                theme::OFF_FILL,
                "{where_}: the unpressable submit is painted {off:?} rather than the design \
                 system's switched-off fill -- a wash of the live colour says \"this button, \
                 dimmed\" where the user needs \"not a control right now\""
            );
            assert_ne!(
                on, off,
                "{where_}: the submit paints the same fill whether it can be pressed or not"
            );
        }
    }

    /// **Both of the composer's own inputs are the design system's boxes**,
    /// under §5a's eyebrows.
    ///
    /// What they were: two bare `egui::TextEdit`s, one single-line and one
    /// multi-line, wearing egui's frame and egui's radius on a card whose
    /// every other control came from `theme.rs`. The owner's words were that
    /// the fields are drawn as thin outlines. The name box is now
    /// `theme::FIELD_HEIGHT` exactly -- the measurement every input box in
    /// this app shares, live or greyed -- and the body is
    /// `theme::text_area`'s taller one in the same treatment.
    ///
    /// The eyebrows are asserted here rather than in a test of their own
    /// because they and the boxes are one decision: the blocks are §5a's
    /// first two, and `draw_composer` argues why this form's two inputs are
    /// eyebrowed at all.
    #[test]
    fn both_of_the_composers_inputs_are_the_design_systems_boxes() {
        for (where_, pane) in composer_panes() {
            let painted = paint_composer_at(&mut open_composer(), false, pane);

            for eyebrow in [NAME_EYEBROW, TEXT_EYEBROW, ACCESS_EYEBROW] {
                assert!(
                    painted.has(eyebrow),
                    "{where_}: the {eyebrow:?} eyebrow is not on the card, so the block above \
                     the Access rows is the only one the card admits to having: {:?}",
                    painted.text
                );
            }

            let name = box_under(&painted, "SAP Production");
            assert!(
                (name.height() - theme::FIELD_HEIGHT).abs() <= 0.5,
                "{where_}: the name field's box is {}pt tall against the design system's \
                 {}pt -- it is not one of this app's fields",
                name.height(),
                theme::FIELD_HEIGHT
            );

            let body = box_under(&painted, "hunter2");
            assert!(
                body.height() > name.height(),
                "{where_}: the body box is {}pt against the name's {}pt, so the paragraph \
                 field is not taller than the one-line one",
                body.height(),
                name.height()
            );
            assert!(
                (body.width() - name.width()).abs() <= 1.0,
                "{where_}: the two boxes are {}pt and {}pt wide -- they are stacked on one \
                 card and have to share an edge",
                name.width(),
                body.width()
            );
        }
    }

    /// An open composer with a valid draft, so the submit below is LIVE: a
    /// disabled primary is faded toward the window colour, and a fill read
    /// off one would be the fade rather than the design's blue.
    fn open_composer() -> SendComposer {
        SendComposer {
            open: true,
            plan: crate::send::SendPlan {
                name: "SAP Production".to_string(),
                text: zeroize::Zeroizing::new("hunter2".to_string()),
                ..crate::send::SendPlan::default()
            },
        }
    }

    /// **The lifetime picker is ONE control, it states the answer in force,
    /// and it is a fixed box rather than a run that grows with the set.**
    ///
    /// This test's ancestor asserted the shape of a four-cell segmented run:
    /// every cell [`theme::SEGMENT_HEIGHT`] tall, consecutive cells joined
    /// within [`theme::SEGMENT_SEAM`], the one in force filled blue. The
    /// control moved to `theme::dropdown` when the set reached nine (see
    /// [`draw_access_block`] for the measurement that forced it), so the run's
    /// geometry is gone -- but the property those three assertions were
    /// protecting is not, and this asserts it at the new shape:
    ///
    ///  * there is **one** control for the question, [`theme::DROPDOWN_HEIGHT`]
    ///    tall -- not a row of buttons and not two controls between which the
    ///    answer moves;
    ///  * it **says the answer in force** when shut, so the form does not
    ///    require a press to be readable;
    ///  * it is exactly [`EXPIRY_FIELD_WIDTH`] wide whatever the answer is, so
    ///    a longer label cannot push it past the card. The old run's width was
    ///    the sum of its labels, and that is precisely what stopped working.
    #[test]
    fn the_lifetime_picker_is_one_fixed_box_that_states_the_answer() {
        let mut composer = open_composer();
        let chosen = crate::send::lifetime_label(composer.plan.lifetime, &UTC);
        let painted = paint_composer(&mut composer, false);

        let (shut, _) = painted.control_under(&chosen);
        assert_eq!(
            shut.height(),
            theme::DROPDOWN_HEIGHT,
            "the lifetime control is {}pt tall against the design system's dropdown box of {}",
            shut.height(),
            theme::DROPDOWN_HEIGHT
        );
        assert!(
            (shut.width() - EXPIRY_FIELD_WIDTH).abs() < 0.5,
            "the lifetime control is {}pt wide rather than its fixed {EXPIRY_FIELD_WIDTH} -- a \
             control whose width follows its label is the shape that overflowed the card",
            shut.width()
        );

        // The eight other answers are behind a press and must NOT be on the
        // card: a dropdown that painted its whole list shut would be a
        // segmented run with extra steps, and would overflow for the same
        // reason.
        for other in crate::send::LIFETIME_CHOICES.into_iter().flatten() {
            let label = crate::send::lifetime_label(other, &UTC);
            if label == chosen {
                continue;
            }
            assert!(
                !painted.has(&label),
                "the {label:?} row is on the card with the control shut: {:?}",
                painted.text
            );
        }

        // And the one that is NOT a duration is reachable in the same list,
        // rather than behind some other surface -- the point of the dropdown
        // was that `Never` costs exactly what `7 days` costs.
        assert!(
            crate::send::LIFETIME_CHOICES.contains(&Some(crate::send::SendLifetime::Never)),
            "`Never` is not one of the picker's own rows"
        );
    }

    /// **Pressing row N gives the answer row N is labelled with**, for all
    /// nine, and the picked-date row inherits rather than invents.
    ///
    /// A pure function over an index, which is what the dropdown hands back --
    /// so the one place this feature could silently publish a different
    /// lifetime from the one pressed is tested without running a frame. An
    /// off-by-one here would be invisible on screen until a link died on the
    /// wrong day.
    #[test]
    fn pressing_a_row_gives_the_answer_that_row_is_labelled_with() {
        let now = FixedClock(NOW);
        for (index, row) in crate::send::LIFETIME_CHOICES.into_iter().enumerate() {
            let got = lifetime_for_row(index, crate::send::DEFAULT_LIFETIME, &now, &UTC)
                .unwrap_or_else(|| panic!("row {index} gave no answer"));
            match row {
                Some(expected) => assert_eq!(
                    got, expected,
                    "row {index} is labelled {:?} and produces {got:?}",
                    lifetime_row_label(row, crate::send::DEFAULT_LIFETIME, &UTC)
                ),
                // The picked-date row keeps the deadline it inherited: the
                // default is seven days out, so pressing it must land on that
                // same day and not on today, on the first of a month, or on
                // anything else invented.
                None => {
                    let day = got.local_day(&now, &UTC).expect("a picked date has a day");
                    assert_eq!(
                        Some(day),
                        crate::send::DEFAULT_LIFETIME.local_day(&now, &UTC),
                        "the picked-date row moved the deadline as a side effect of opening \
                         a calendar to look at it"
                    );
                    assert!(matches!(got, crate::send::SendLifetime::Until(_)));
                }
            }
            // Whatever it produced, the encoder must accept it -- a row the
            // picker offers and the validator refuses is a control whose every
            // press is a refusal.
            assert_eq!(
                crate::send::validate_access(got, None, None, &now),
                None,
                "row {index} produces an answer the encoder refuses"
            );
        }

        // `Never` has no day to inherit, and the fallback is the default's
        // rather than a panic or a zero.
        let from_never =
            lifetime_for_row(7, crate::send::SendLifetime::Never, &now, &UTC).expect("an answer");
        assert_eq!(
            from_never.local_day(&now, &UTC),
            crate::send::DEFAULT_LIFETIME.local_day(&now, &UTC)
        );

        // An index off the end is `None` and not a panic: the dropdown and the
        // list are built from the same array, but a draw closure is not the
        // place to find out that they were not.
        assert_eq!(
            lifetime_for_row(99, crate::send::DEFAULT_LIFETIME, &now, &UTC),
            None
        );
    }

    /// **The picked-date row says what pressing it does, until it has an
    /// answer -- and then it says the answer.**
    #[test]
    fn the_picked_date_row_names_itself_and_then_names_the_date() {
        assert_eq!(
            lifetime_row_label(None, crate::send::DEFAULT_LIFETIME, &UTC),
            crate::send::PICK_A_DATE_LABEL
        );
        let picked = crate::send::SendLifetime::on_local_day(2027, 3, 14, &UTC);
        assert_eq!(lifetime_row_label(None, picked, &UTC), "14 Mar 2027");
        // And a fixed row is never affected by what is in force.
        assert_eq!(
            lifetime_row_label(Some(crate::send::SendLifetime::Never), picked, &UTC),
            crate::send::NEVER_LABEL
        );
    }

    /// **Every row of the picker fits inside the box that holds it.**
    ///
    /// The dropdown does not grow to its widest row -- that is the whole
    /// reason it replaced the run -- so a reworded or added choice that is
    /// wider than [`EXPIRY_FIELD_WIDTH`] would be silently clipped rather than
    /// visibly overflowing, which is the failure mode a fixed box trades for
    /// the one it fixes. This is the assertion that catches it.
    #[test]
    fn every_row_of_the_lifetime_picker_fits_the_box() {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(720.0, 900.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let mut widest = 0.0f32;
        let mut name = String::new();
        let _ = ctx.run_ui(input(), |ui| {
            for row in crate::send::LIFETIME_CHOICES {
                let label = lifetime_row_label(row, crate::send::DEFAULT_LIFETIME, &UTC);
                let width = theme::dropdown_text_width(ui, &label);
                if width > widest {
                    widest = width;
                    name = label;
                }
            }
        });
        assert!(widest > 0.0, "control: no row was measured at all");
        let room = EXPIRY_FIELD_WIDTH - theme::DROPDOWN_TEXT_BUDGET;
        assert!(
            widest <= room,
            "the {name:?} row is {widest}pt wide and the box leaves {room}pt for text, \
             so it is clipped"
        );
    }

    /// **The composer's two answers are a primary and a secondary**, measured
    /// on the fills.
    ///
    /// They used to be two bare `egui::Button`s differing only in the colour
    /// of their text -- so on screen the publish and the throw-away were a
    /// matched pair, and no test in this crate could say so. See
    /// `record_ui`'s twin of this test: the same defect stood on both of this
    /// app's Send composers.
    #[test]
    fn the_composers_two_answers_are_a_primary_and_a_secondary() {
        let mut composer = open_composer();
        assert!(
            composer_problem(&composer, &FixedClock(NOW)).is_none(),
            "the fixture wants a submittable draft, or the fill below is a disabled one"
        );
        let painted = paint_composer(&mut composer, false);

        let (create, create_fill) = painted.control_under(CREATE_LABEL);
        assert_eq!(
            create_fill,
            theme::BLUE,
            "the composer's submit is not the design system's filled primary"
        );
        assert_eq!(create.height(), theme::BUTTON_HEIGHT);

        let (discard, discard_fill) = painted.control_under(DISCARD_LABEL);
        assert_eq!(
            discard_fill,
            theme::CARD,
            "the composer's way out is not the design system's outlined secondary"
        );
        assert_eq!(discard.height(), theme::BUTTON_HEIGHT);
        assert_ne!(
            create_fill, discard_fill,
            "the two answers paint the same fill, which is the exact defect this pass fixed"
        );
    }

    /// **While a publish is in flight the lifetime control is inert, and the
    /// answer in force is still legible.**
    ///
    /// `theme::dropdown_disabled` is what this form draws in that state, and
    /// it exists for the reason its segmented-run predecessor did: the reader
    /// needs "you have this answer and cannot change it", not "you have no
    /// answer". Pinned because the alternative -- an `add_enabled_ui(false)`
    /// round the live control -- looks nearly the same in a screenshot while
    /// fading the answer with the box, and leaves a control egui will still
    /// let through on a re-layout.
    #[test]
    fn a_publish_in_flight_leaves_the_lifetime_control_inert_and_readable() {
        let mut composer = open_composer();
        let chosen = crate::send::lifetime_label(composer.plan.lifetime, &UTC);
        let live = paint_composer(&mut composer, false);
        let (_, live_fill) = live.control_under(&chosen);
        let painted = paint_composer(&mut composer, true);

        assert!(painted.has(CREATING_LABEL), "the in-flight word is not on the form");
        assert!(
            painted.has(&chosen),
            "the lifetime in force is not on the card while a publish is running, so the \
             user cannot read back what they are publishing: {:?}",
            painted.text
        );
        let (_, fill) = painted.control_under(&chosen);
        assert_eq!(
            fill,
            theme::CARD_TINT,
            "the lifetime control is not the design system's inert box while a publish is \
             running -- either it is still live, or it has been faded out with everything \
             the user still needs to read on it"
        );
        assert_ne!(
            fill, live_fill,
            "control: the inert box and the live one paint the same, so this test would \
             pass over a control that was never disabled at all"
        );
    }

    /// **The whole of what an empty view-limit box means, and what a filled
    /// one does.**
    ///
    /// A pure function, so the states this control can be in are assertable
    /// without running a frame -- which matters more here than usual, because
    /// the dangerous state is invisible on screen: a box that looked capped
    /// and parsed to `None` would publish an uncapped link with nothing
    /// anywhere to say so.
    #[test]
    fn the_view_limit_box_reads_empty_as_no_limit_and_digits_as_a_cap() {
        assert_eq!(view_limit_from(""), None, "an empty box is the off position");
        assert_eq!(view_limit_from("   "), None, "whitespace is an empty box");
        assert_eq!(view_limit_from("1"), Some(1));
        assert_eq!(view_limit_from("42"), Some(42));

        // A zero is kept and NOT swallowed to `None`. It is a real thing to
        // have typed, and `validate_plan` has the sentence for it; turning it
        // into "no limit" here would make a typo into an uncapped link with
        // no refusal anywhere in the app.
        assert_eq!(view_limit_from("0"), Some(0));
        assert_eq!(
            crate::send::validate_access(
                crate::send::DEFAULT_LIFETIME,
                None,
                Some(0),
                &FixedClock(NOW),
            ),
            Some("A limit of zero views would make the link useless."),
            "a zero cap is accepted by the encoder, so the box may not produce one silently"
        );

        // Non-digits are dropped, so a pasted "3 views" does the obvious
        // thing rather than being refused with nothing on screen to say why.
        assert_eq!(view_limit_from("3 views"), Some(3));
        assert_eq!(view_limit_from("abc"), None);
        assert_eq!(view_limit_from("-5"), Some(5), "a minus sign is not a digit");

        // **The overflow guard, which is the reason the cap exists.** Eleven
        // digits do not fit a `u32`; an ungated `parse().ok()` would answer
        // `None`, and `None` in this control means NO LIMIT AT ALL. A user
        // holding a key down would have quietly uncapped their own link.
        let held_down = "9".repeat(20);
        assert_eq!(
            view_limit_from(&held_down),
            Some(999_999),
            "a long run of digits did not stop at {MAX_VIEW_LIMIT_DIGITS} -- if this is `None`, \
             leaning on a key turns a cap into an uncapped Send"
        );
        assert!(
            "9".repeat(MAX_VIEW_LIMIT_DIGITS).parse::<u32>().is_ok(),
            "control: {MAX_VIEW_LIMIT_DIGITS} nines do not fit a u32, so the cap is too high"
        );

        // The note beside the box says which of the two states it is in.
        assert_eq!(views_note(None), NO_VIEW_LIMIT_NOTE);
        assert_eq!(views_note(Some(1)), VIEW_LIMIT_NOTE);
        assert_ne!(NO_VIEW_LIMIT_NOTE, VIEW_LIMIT_NOTE);
    }

    /// **§5a's Access block is on the text composer, with all three of its
    /// buildable rows.**
    ///
    /// The composer had a lifetime row and a password tick-box and no view
    /// cap at all. This says the block arrived whole -- and the three
    /// absences at the bottom say it did not bring with it the three controls
    /// no server behind this app can honour.
    #[test]
    fn the_composer_wears_the_whole_access_block() {
        let mut composer = open_composer();
        let painted = paint_composer(&mut composer, false);

        assert!(painted.has(ACCESS_EYEBROW), "the ACCESS eyebrow is missing: {:?}", painted.text);
        for label in [EXPIRES_LABEL, VIEWS_LABEL, OPEN_WITH_LABEL] {
            assert!(painted.has(label), "the {label:?} row is missing: {:?}", painted.text);
        }
        assert!(
            painted.has(NO_VIEW_LIMIT_NOTE),
            "the view box is empty and says nothing about what that means: {:?}",
            painted.text
        );
        assert!(
            painted.has(PASSWORD_HINT),
            "the password box is empty and says nothing about what that means: {:?}",
            painted.text
        );

        // **The tick-box is gone.** It was "Require a password to open the
        // link" over a field that appeared only once ticked; the box is its
        // own switch now, and a switch left in front of it would be a second
        // way to say what the box says.
        assert!(
            !painted.has("Require a password"),
            "the password switch is still drawn in front of a box that is its own switch"
        );

        // The three §5a rows that are absences and not to-dos. Named here so
        // that adding one reds a test rather than shipping a control that
        // cannot do what its label says.
        for absent in ["Recipient", "Only this address can open it", "Tell me when it is opened"] {
            assert!(
                !painted.has(absent),
                "{absent:?} is drawn on the composer, and there is no server behind it"
            );
        }
    }

    /// **What is typed into the Access block reaches the plan that is
    /// published**, by the route a user actually takes.
    ///
    /// `draw_access_block` writes through three `&mut`s into the composer's
    /// own `SendPlan`, which is the buffer `plan_to_invocation` will encode --
    /// there is no conversion step in between and this says so. The password
    /// half is the one that matters most: a control that drew, validated and
    /// then published `None` would ship a Send anyone with the link can open,
    /// with the user's own password visible in the box they typed it into.
    #[test]
    fn what_the_access_block_holds_is_what_the_composer_would_publish() {
        let mut composer = open_composer();
        composer.plan.lifetime = crate::send::SendLifetime::Hours(1);
        composer.plan.password = Some(zeroize::Zeroizing::new("share-pw-9271".to_string()));
        composer.plan.max_access_count = Some(3);

        // A frame of the real form, which is where a block that dropped what
        // it was handed would do the dropping.
        let painted = paint_composer(&mut composer, false);
        assert!(painted.has(ACCESS_EYEBROW), "control: the block did not draw");

        assert_eq!(
            composer.plan.lifetime,
            crate::send::SendLifetime::Hours(1),
            "a frame reset the lifetime"
        );
        assert_eq!(
            composer.plan.password.as_deref().map(String::as_str),
            Some("share-pw-9271"),
            "a frame dropped the share password, so the composer would publish an open link"
        );
        assert_eq!(composer.plan.max_access_count, Some(3), "a frame dropped the view cap");
        assert_eq!(
            composer_problem(&composer, &FixedClock(NOW)),
            None,
            "the draft the user typed is refused"
        );

        // And the sub-day lifetime is the one now on screen, spelled by the
        // one function that spells lifetimes.
        assert!(
            painted.has(&crate::send::lifetime_label(crate::send::SendLifetime::Hours(1), &UTC)),
            "the one-hour answer is not the one in the shut box: {:?}",
            painted.text
        );
        assert!(
            painted.has("The link stops working after 1 hour"),
            "the sentence under the run does not name the sub-day lifetime: {:?}",
            painted.text
        );
    }

    /// **A masked password box never paints what was typed into it.**
    ///
    /// The block writes a `Zeroizing` buffer through a `&mut` and hands it to
    /// a `TextEdit`; `.password(true)` is one call away from being lost in an
    /// edit, and losing it puts a share password on screen in a window the
    /// user may well be screen-sharing. Nothing else in this file would
    /// notice.
    #[test]
    fn the_share_password_is_never_painted_in_the_clear() {
        let mut composer = open_composer();
        composer.plan.password = Some(zeroize::Zeroizing::new("share-pw-9271".to_string()));
        let painted = paint_composer(&mut composer, false);
        assert!(painted.has(OPEN_WITH_LABEL), "control: the password row did not draw");
        assert!(
            !painted.has("share-pw-9271"),
            "the share password was painted in the clear: {:?}",
            painted.text
        );
    }

    fn rows(n: usize) -> SendPaneState {
        let sends: Vec<SendSummary> = (0..n)
            .map(|i| SendSummary {
                id: format!("id{i}"),
                name: format!("send-number-{i}"),
                access_url: format!("https://send.bitwarden.com/#/{i}"),
                deletion_date: "2026-08-17T00:00:00.000Z".to_string(),
                is_file: i % 2 == 1,
                // Live and untouched, so every row here derives to `Waiting`
                // and the geometry assertions below are measuring one pill
                // rather than four different widths.
                max_access_count: None,
                access_count: 0,
                disabled: false,
                expiration_date: String::new(),
                has_password: false,
            })
            .collect();
        pane_state(Some(&Ok(sends)), &FixedClock(NOW), &UTC)
    }

    /// A one-row pane whose single Send really is in `state`.
    ///
    /// **The control is inside the fixture**: `send_state` is asserted on the
    /// summary before it is handed to the pane, so a test below that finds no
    /// `Revoked` pill is telling you the pane did not paint one, not that the
    /// fixture was never revoked in the first place.
    fn one_row(state: crate::send::SendState) -> SendPaneState {
        use crate::send::SendState;
        let base = SendSummary {
            id: "id0".to_string(),
            name: "SAP Production".to_string(),
            access_url: "https://send.bitwarden.com/#/x".to_string(),
            deletion_date: "2026-08-17T00:00:00.000Z".to_string(),
            is_file: false,
            max_access_count: None,
            access_count: 0,
            disabled: false,
            expiration_date: String::new(),
            has_password: false,
        };
        let send = match state {
            SendState::Waiting => base,
            SendState::Used => SendSummary {
                max_access_count: Some(1),
                access_count: 1,
                ..base
            },
            SendState::Expired => SendSummary {
                deletion_date: "2026-08-01T00:00:00.000Z".to_string(),
                ..base
            },
            SendState::Revoked => SendSummary { disabled: true, ..base },
        };
        assert_eq!(
            crate::send::send_state(&send, &FixedClock(NOW)),
            state,
            "control: the fixture for {state:?} does not derive to {state:?}"
        );
        pane_state(Some(&Ok(vec![send])), &FixedClock(NOW), &UTC)
    }

    /// **Every one of design §5c's four states paints its own pill, in its
    /// own ground, at the design's size.**
    ///
    /// Measured rather than merely found, for this module's stated reason: a
    /// label at zero size satisfies "the word is on screen" and is invisible.
    /// So each case reads the filled rectangle BEHIND the word -- which is
    /// the pill itself -- and checks its height, its fill and that it is
    /// wider than the word it wraps.
    #[test]
    fn each_state_paints_the_designs_own_pill_and_not_the_toolbar_readout() {
        use crate::send::SendState;
        for state in [
            SendState::Waiting,
            SendState::Used,
            SendState::Expired,
            SendState::Revoked,
        ] {
            let word = state_label(state);
            let tone = state_tone(state);
            let (painted, _) = paint(&one_row(state), None, min_pane_size());
            // **Twice, and that is 5b's own arrangement**: the pill is in the
            // list row and again in the detail header of the Send that row
            // describes. Asserted as one in each column rather than as "two
            // somewhere", so a pane that drew both in the same place would
            // fail here instead of passing a total.
            assert_eq!(
                painted.in_list(word),
                1,
                "{state:?} painted its word {} times in the list column, not once: {:?}",
                painted.in_list(word),
                painted.text
            );
            assert_eq!(
                painted.in_detail(word),
                1,
                "{state:?} is not on the pill in its own detail header: {:?}",
                painted.text
            );
            let (rect, fill) = painted.control_under(word);
            assert_eq!(
                fill, tone.fill,
                "the {state:?} pill is filled {fill:?}, not the design's {:?}",
                tone.fill
            );
            assert!(
                (rect.height() - theme::PILL_HEIGHT).abs() < 0.51,
                "the {state:?} pill is {} tall, not the design's {}. The design declares \
                 `padding: 3px 8px` inside a `1px` border on a content-box page, so the box \
                 is the line plus six plus two -- writing the padding sum alone is the \
                 measurement slip this screen keeps being re-measured for",
                rect.height(),
                theme::PILL_HEIGHT
            );
            let word_rect = painted.rect_of(word).expect("the word was counted above");
            assert!(
                rect.width() > word_rect.width() + theme::PILL_PAD_X,
                "the {state:?} pill is {} wide around a {} word -- it is not wrapping it",
                rect.width(),
                word_rect.width()
            );
            // **And it is not `theme::status_pill`.** That widget is 28 tall
            // and unfilled; either property alone would be a pill the design
            // does not have.
            assert_ne!(
                fill,
                egui::Color32::TRANSPARENT,
                "the {state:?} pill has no ground, which is `status_pill`'s treatment and not \
                 this one's"
            );
        }
    }

    /// **`Used` is the only state with a mark that is not a dot, and two of
    /// the four have no mark at all.**
    ///
    /// The distinction is the design's own and it carries meaning -- see
    /// `theme::PillMark` -- so it is asserted on the mapping rather than
    /// left to whoever next edits the four arms.
    #[test]
    fn only_the_two_live_states_carry_a_mark_and_only_one_of_them_is_a_tick() {
        use crate::send::SendState;
        assert!(matches!(
            state_tone(SendState::Waiting).mark,
            theme::PillMark::Dot(_)
        ));
        assert!(matches!(
            state_tone(SendState::Used).mark,
            theme::PillMark::Check(_)
        ));
        assert_eq!(state_tone(SendState::Expired).mark, theme::PillMark::None);
        assert_eq!(state_tone(SendState::Revoked).mark, theme::PillMark::None);
        // The four words are four words. A mapping that returned one label
        // twice would leave two states indistinguishable on screen while
        // every assertion above still passed.
        let words = [
            state_label(SendState::Waiting),
            state_label(SendState::Used),
            state_label(SendState::Expired),
            state_label(SendState::Revoked),
        ];
        let mut sorted = words.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "two states are drawn with the same word: {words:?}");
    }

    /// **The pill is at the row's own right edge, clear of the words, and
    /// still inside the pane at the minimum window size.**
    ///
    /// # This test replaces a departure rather than restating it
    ///
    /// Its ancestor asserted the opposite arrangement -- the pill LEADING the
    /// second line, with the expiry after it -- and its doc said so: "the one
    /// place this screen departs from 5b, which right-aligns the pill". That
    /// departure had one argument and it was entirely about controls: Copy
    /// link and Delete owned this row's right edge, and a pill placed in that
    /// column would be pushed off the pane at `MIN_VAULT_WINDOW_SIZE` or
    /// would move every time the row changed mode.
    ///
    /// 5b puts those controls in the detail pane, this screen now has one,
    /// and the row has no controls at all -- so the argument is spent and the
    /// design's own placement is taken. What is pinned here is the property
    /// the departure was protecting, at the design's geometry: the pill is
    /// right-aligned, it does not collide with the words, and the column it
    /// sits in is not one a narrow window can push it out of.
    #[test]
    fn the_row_puts_its_pill_at_its_own_right_edge() {
        let state = one_row(crate::send::SendState::Waiting);
        let size = min_pane_size();
        let (painted, _) = paint(&state, None, size);
        let (pill, _) = painted.control_under(WAITING_LABEL);
        // The LIST column's copy of the name -- it is painted again in the
        // detail header, and the pill this test is about is the row's.
        let name = *painted
            .rects_of_exact("SAP Production")
            .iter()
            .find(|r| r.center().x < crate::vault_window::LIST_WIDTH)
            .expect("the row painted no name at all, so there is nothing for the pill to clear");
        assert!(
            pill.left() >= name.right(),
            "the name ends at {} and the pill starts at {} -- they overlap",
            name.right(),
            pill.left()
        );
        // Right-aligned: the pill's right edge sits exactly one `ROW_PAD_X`
        // in from the row's own right edge.
        //
        // **Measured off the row's painted card, not off a chain of
        // constants.** `LIST_WIDTH` minus this padding minus that one is
        // arithmetic a test can get right while the pane gets it wrong -- and
        // it did: the panel keeps a hairline of its own, so the row is 10pt
        // narrower than the constants predict. What the design actually says
        // is "the pill is `padding` in from the row", and that is a statement
        // about the row.
        let card = painted
            .fills
            .iter()
            .filter(|(rect, fill)| {
                *fill == theme::CARD
                    && (rect.height() - ROW_HEIGHT).abs() < 0.51
                    && rect.center().x < crate::vault_window::LIST_WIDTH
            })
            .map(|(rect, _)| *rect)
            .next()
            .expect("the list column painted no row card at all");
        assert!(
            (card.right() - pill.right() - ROW_PAD_X).abs() < 2.0,
            "the pill ends at {} on a row that ends at {} -- that is {}pt of padding, not the \
             design's {ROW_PAD_X}",
            pill.right(),
            card.right(),
            card.right() - pill.right()
        );
        // ...and it is inside the pane at the floor, which is the assertion
        // the old left-leading placement was bought to guarantee.
        assert!(
            pill.right() <= size.x,
            "the pill ends at {} on a {}pt pane -- it is off the screen",
            pill.right(),
            size.x
        );
    }

    /// **The pill STAYS while the row's own revoke runs, and the subtitle is
    /// what changes.**
    ///
    /// Its ancestor asserted the opposite -- that the pill left while the row
    /// was confirming or working -- because the confirmation used to be ON
    /// the row, and a row saying "Revoke this link for good?" and "Waiting"
    /// at once had two subjects. The confirmation is in the detail pane now,
    /// so the row has no second subject to be ambiguous with, and hiding the
    /// state of a Send at the moment it is being destroyed would remove the
    /// one thing that tells the user which row they are losing.
    ///
    /// What is unchanged is the rule underneath: the row's second line is
    /// REPLACED rather than joined. `DELETING_LABEL` takes the subtitle's
    /// place, and the expiry is not printed beside it.
    #[test]
    fn the_row_keeps_its_pill_while_its_own_revoke_runs() {
        let state = one_row(crate::send::SendState::Waiting);
        // Control: the resting row has its pill and its subtitle.
        let (resting, _) = paint(&state, None, min_pane_size());
        assert_eq!(resting.in_list(WAITING_LABEL), 1, "control: the resting row has no pill");
        assert!(!resting.has(DELETING_LABEL), "control: a resting row says it is being revoked");

        let revoking = SendDeleteView { confirming: None, in_flight: Some("id0") };
        let (painted, _) = paint_with(&state, None, min_pane_size(), revoking);
        assert_eq!(
            painted.in_list(WAITING_LABEL),
            1,
            "the row lost the pill that says which Send is being revoked: {:?}",
            painted.text
        );
        assert!(
            painted.has(DELETING_LABEL),
            "the row does not say a revoke is running: {:?}",
            painted.text
        );
        assert!(
            !painted.has("Expires in"),
            "the row printed its expiry BESIDE the progress word rather than instead of it: \
             {:?}",
            painted.text
        );
    }

    /// **The `FILE` tag is explained, and only where there is one.**
    ///
    /// A three-letter tag beside a name says a row is different and not how,
    /// and the difference that matters is that the row's two buttons still
    /// work. The control is on the second half: a list of text Sends must NOT
    /// carry the sentence, or this test would pass over a pane that printed
    /// it unconditionally and told every user about a restriction they had
    /// not met.
    #[test]
    fn a_file_send_in_the_list_is_explained_and_a_list_without_one_is_not() {
        let with_a_file = SendPaneState::Rows(vec![
            summary_row("a text Send", false),
            summary_row("report.pdf", true),
        ]);
        let (painted, _) = paint(&with_a_file, None, min_pane_size());
        assert!(
            painted.has(FILE_SEND_EXPLANATION),
            "a file Send was listed with nothing saying what its tag means: {:?}",
            painted.text
        );
        // The control: the tag itself is there too, so the sentence is
        // explaining something the user can actually see.
        assert!(painted.has(FILE_TAG), "the row carried no FILE tag to explain");

        let text_only = SendPaneState::Rows(vec![summary_row("a text Send", false)]);
        let (painted, _) = paint(&text_only, None, min_pane_size());
        assert!(
            !painted.has(FILE_SEND_EXPLANATION),
            "a list of text Sends was told about a restriction it has not met"
        );
        // The control for THAT: the pane really painted, so the absence above
        // is about this sentence and not about an empty screen.
        assert!(painted.has(SCOPE_SUBTEXT), "nothing was painted at all: {:?}", painted.text);
    }

    /// A `SendRow` for the two assertions above.
    fn summary_row(name: &str, is_file: bool) -> SendRow {
        SendRow {
            id: format!("id-of-{name}"),
            name: name.to_string(),
            access_url: "https://vault.example.com/#/send/acc/AAAAAAAAAAAAAAAAAAAAAA".to_string(),
            expiry: "Expires in 7 days".to_string(),
            is_file,
            state: crate::send::SendState::Waiting,
            // The facts the detail pane draws, at the shape a live, untouched
            // Send really has. Spelled out rather than defaulted: a `Default`
            // on `SendRow` would be a row that describes no Send at all, and
            // this file's whole subject is states that must not be
            // mistakable for one another.
            access_count: 0,
            max_access_count: None,
            has_password: false,
            activity: "Nobody has opened this link. The link works, as often as they like."
                .to_string(),
            expires: NO_EXPIRY_OF_ITS_OWN.to_string(),
            deletes: "17 Aug 2026, 00:00 \u{00b7} in 7 days".to_string(),
        }
    }

    /// **The receive sentence names the tool and then limits the claim.**
    ///
    /// Two properties, and the second is the one that is easy to lose: it
    /// must not read as "Sends need the CLI", which is false for three of the
    /// four operations and is exactly the impression a user would take from
    /// the first sentence alone.
    #[test]
    fn the_receive_sentence_says_what_is_missing_without_condemning_what_is_not() {
        assert!(
            RECEIVE_NEEDS_THE_CLI.contains("command-line tool"),
            "the sentence does not name the thing that is missing: {RECEIVE_NEEDS_THE_CLI}"
        );
        for kept in ["Publishing", "listing", "revoking"] {
            assert!(
                RECEIVE_NEEDS_THE_CLI.contains(kept),
                "{kept:?} is not named as still working, so the sentence reads as `Sends need the \
                 CLI`: {RECEIVE_NEEDS_THE_CLI}"
            );
        }
        // The control: the two sentences are not the same sentence, so an
        // assertion satisfied by either is satisfied by the right one.
        assert!(
            !FILE_SEND_EXPLANATION.contains("command-line"),
            "the file sentence and the receive sentence have merged"
        );
        assert!(
            !RECEIVE_NEEDS_THE_CLI.contains("file"),
            "the receive sentence has picked up the file restriction"
        );
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

    /// **Six rows, not two, and every one of them can be picked into a
    /// detail pane whose three controls all fit at the minimum window size.**
    ///
    /// # What this asserts now, and why it is the same property
    ///
    /// Its ancestor counted six Copy link buttons, one per row, because that
    /// is where the control was. Design 5b puts the controls in the detail
    /// pane, so there is one Copy link on screen -- and the property that
    /// mattered has not moved: **no row is drawn without a reachable way to
    /// get its link back**. That is now two claims instead of one, and both
    /// are made here: every row is really painted (a pane that drew only the
    /// first few passed every assertion written against a two-row fixture,
    /// which this codebase has shipped once), and picking ANY of them puts
    /// that row's controls in the detail pane, inside the pane, at a real
    /// size.
    ///
    /// The loop over all six is what makes the second claim worth anything. A
    /// detail pane that only worked for the first row would satisfy a
    /// single-pick test exactly as a list that drew only the first few rows
    /// satisfied the old two-row one.
    #[test]
    fn every_row_can_be_picked_into_a_working_detail_pane_at_the_minimum_window_size() {
        let size = min_pane_size();
        let state = rows(6);
        let (painted, _) = paint(&state, None, size);
        // COUNT FIRST. A row pushed off the pane is culled entirely, so it
        // comes back as nothing at all -- reading geometry before counting
        // would read the geometry of the rows that survived. The seventh
        // occurrence is the picked row's name in the detail header.
        assert_eq!(painted.count("send-number-"), 7, "painted names: {:?}", painted.text);
        assert_eq!(painted.count(FILE_TAG), 3, "the file rows lost their tag");

        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        for index in 0..6 {
            let (picked, _) = paint_picking(
                &state,
                None,
                size,
                SendDeleteView::default(),
                Some(format!("id{index}")),
            );
            for label in ["Copy link", SWITCH_OFF_LABEL, DELETE_LABEL] {
                let found = picked.rects_of_exact(label);
                assert_eq!(
                    found.len(),
                    1,
                    "row {index} was picked and {label:?} was painted {} times: {:?}",
                    found.len(),
                    picked.text
                );
                let rect = found[0];
                // A control drawn at zero size has passed both a presence
                // assertion and an in-pane assertion in this codebase before;
                // only a glyph-level size check caught it.
                assert!(
                    rect.width() > 4.0 && rect.height() > 4.0,
                    "{label:?} is {rect:?} on row {index}"
                );
                assert!(
                    pane.contains_rect(rect),
                    "{label:?} at {rect:?} is outside the {pane:?} pane at the minimum window \
                     size -- the detail column is {}pt wide there, which is what the action \
                     row has to fit in",
                    min_detail_width()
                );
                // ...and inside the DETAIL column, not spilling back over the
                // list. The three are laid out left to right from the
                // detail's own content edge, and a regression that measured
                // them against the whole pane would put the first one under
                // the rows.
                assert!(
                    rect.left() >= crate::vault_window::LIST_WIDTH,
                    "{label:?} at {rect:?} starts left of the detail column's edge at {}",
                    crate::vault_window::LIST_WIDTH
                );
            }
            // The detail really is describing THAT row, not the first one.
            assert!(
                picked.in_detail(&format!("send-number-{index}")) == 1,
                "row {index} was picked and the detail header names something else: {:?}",
                picked.text
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

    /// **A revoked row offers to switch ON and every other row offers to
    /// switch OFF, and the two are never on screen together.**
    ///
    /// The rule is stated once, in `draw_row`, off the row's derived state --
    /// so a row that came back from the server already disabled shows the way
    /// back rather than offering to disable it again, which is a button that
    /// would do nothing and report success.
    #[test]
    fn the_switch_offers_the_direction_the_row_is_not_already_in() {
        use crate::send::SendState;
        for (state, offered, hidden) in [
            (SendState::Waiting, SWITCH_OFF_LABEL, SWITCH_ON_LABEL),
            (SendState::Used, SWITCH_OFF_LABEL, SWITCH_ON_LABEL),
            (SendState::Expired, SWITCH_OFF_LABEL, SWITCH_ON_LABEL),
            (SendState::Revoked, SWITCH_ON_LABEL, SWITCH_OFF_LABEL),
        ] {
            let (painted, _) = paint(&one_row(state), None, min_pane_size());
            // `count_exact` and not `count`: the detail's own activity line
            // for a revoked Send NAMES the button ("Switch on to let it work
            // again"), which is the sentence doing its job, and a substring
            // count cannot tell it from the control.
            assert_eq!(
                painted.count_exact(offered),
                1,
                "a {state:?} Send does not offer {offered:?}: {:?}",
                painted.text
            );
            assert_eq!(
                painted.count_exact(hidden),
                0,
                "a {state:?} Send offers {hidden:?} as well, so one of the two buttons does \
                 nothing and reports that it worked"
            );
            // Measured, not merely found: a control at zero size satisfies
            // every "the word is on screen" assertion and is unclickable.
            let rect = painted.rects_of_exact(offered)[0];
            assert!(
                rect.width() > 1.0 && rect.height() > 1.0,
                "{offered:?} was painted at {rect:?} on a {state:?} row, which is not a \
                 control a user can press"
            );
        }
    }

    /// **Pressing it reports the row's own id and name and the direction the
    /// press asked for** -- read off the row that was clicked and off nothing
    /// else, so no lookup on the far side can resolve to a different Send.
    #[test]
    fn pressing_the_switch_reports_the_row_and_the_direction() {
        use crate::send::SendState;
        assert_eq!(
            click_nth(&one_row(SendState::Waiting), SWITCH_OFF_LABEL, 0),
            SendUiAction::SetDisabled {
                id: "id0".to_string(),
                name: "SAP Production".to_string(),
                disabled: true,
            },
        );
        assert_eq!(
            click_nth(&one_row(SendState::Revoked), SWITCH_ON_LABEL, 0),
            SendUiAction::SetDisabled {
                id: "id0".to_string(),
                name: "SAP Production".to_string(),
                disabled: false,
            },
            "a revoked row's button asked to revoke it again instead of asking to put it back"
        );
    }

    /// **The row that is asking to be deleted asks about one thing.**
    ///
    /// The switch is not drawn while the confirmation is up, and it is not
    /// drawn on a row whose worker is running. Both are geometry as much as
    /// wording: the confirmation widens this row to three controls, and the
    /// switch occupies the very rectangle the destructive button takes.
    #[test]
    fn the_switch_leaves_while_the_row_is_confirming_or_busy() {
        let state = one_row(crate::send::SendState::Waiting);
        let (resting, _) = paint(&state, None, min_pane_size());
        assert_eq!(
            resting.count_exact(SWITCH_OFF_LABEL),
            1,
            "control: the resting detail has no switch"
        );

        for (why, view) in [
            ("asking", SendDeleteView { confirming: Some("id0"), in_flight: None }),
            ("busy", SendDeleteView { confirming: None, in_flight: Some("id0") }),
        ] {
            let (painted, _) = paint_with(&state, None, min_pane_size(), view);
            assert_eq!(
                painted.count_exact(SWITCH_OFF_LABEL) + painted.count_exact(SWITCH_ON_LABEL),
                0,
                "the detail kept its switch while {why}: {:?}",
                painted.text
            );
        }

        // **The switch's slot is RESERVED while it is hidden**, and that is
        // what stops hiding it sliding the destructive control under the
        // pointer. The Cancel that takes Delete's pixels is the mis-click
        // defence, and it holds only while Delete's rectangle does not move
        // between the two states. Measured rather than asserted in prose.
        let (confirming, _) = paint_with(
            &state,
            None,
            min_pane_size(),
            SendDeleteView { confirming: Some("id0"), in_flight: None },
        );
        let cancel = confirming
            .rects_of_exact(CANCEL_LABEL)
            .first()
            .copied()
            .expect("the confirming detail painted no way out");
        let delete = resting
            .rects_of_exact(DELETE_LABEL)
            .first()
            .copied()
            .expect("counted above");
        assert!(
            (cancel.center().x - delete.center().x).abs() < 6.0,
            "Cancel is centred at x={} and Delete at x={} -- the second of two rapid clicks \
             where Delete was no longer lands on Cancel, which is the whole mis-click defence",
            cancel.center().x,
            delete.center().x
        );
        // ...and the confirmation's own destructive button is somewhere else
        // entirely: a different row, in a different container, under the
        // question it answers.
        let confirm = confirming
            .rects_of_exact(CONFIRM_LABEL)
            .first()
            .copied()
            .expect("the confirming detail painted no destructive button");
        assert!(
            confirm.top() > cancel.bottom(),
            "the destructive button at {confirm:?} is on the same row as Cancel at {cancel:?} \
             -- reaching it is meant to be a decision, not a second click in the same place"
        );
    }

    /// **A row with no id offers no switch**, on the Delete button's own
    /// rule: an id is what names the Send to the server, and a control that
    /// cannot name its subject must not report an action.
    #[test]
    fn a_row_with_no_id_is_offered_no_switch() {
        let state = SendPaneState::Rows(vec![SendRow {
            id: String::new(),
            name: "no id".into(),
            expiry: "Expires in 7 days".into(),
            is_file: false,
            access_url: "https://send.bitwarden.com/#/x".into(),
            state: crate::send::SendState::Waiting,
            // The facts the detail pane draws, at the shape a live, untouched
            // Send really has. Spelled out rather than defaulted: a `Default`
            // on `SendRow` would be a row that describes no Send at all, and
            // this file's whole subject is states that must not be
            // mistakable for one another.
            access_count: 0,
            max_access_count: None,
            has_password: false,
            activity: "Nobody has opened this link. The link works, as often as they like."
                .to_string(),
            expires: NO_EXPIRY_OF_ITS_OWN.to_string(),
            deletes: "17 Aug 2026, 00:00 \u{00b7} in 7 days".to_string(),
        }]);
        let (painted, _) = paint(&state, None, min_pane_size());
        assert_eq!(
            painted.count(SWITCH_OFF_LABEL),
            0,
            "a row with no id was offered a switch it could not name a Send for"
        );
        // Control: the row is really drawn, so the absence above is about the
        // switch and not about a pane that painted nothing.
        assert_eq!(painted.count("Copy link"), 1, "control: the row was not drawn at all");
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
        click_nth_picking(state, delete, first_row_id(state), label, nth)
    }

    /// [`click_nth_with`], with a NAMED row described in the detail pane --
    /// which is where every control this presses now lives. See
    /// [`first_row_id`].
    fn click_nth_picking(
        state: &SendPaneState,
        delete: SendDeleteView<'_>,
        picked: Option<String>,
        label: &str,
        nth: usize,
    ) -> SendUiAction {
        // The selection is held ACROSS the three frames, exactly as the
        // window holds it: a fixture that re-picked per frame would be
        // re-opening the detail pane under the pointer between the press and
        // the release.
        let mut selected = picked;
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
            let _ = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), &mut selected, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        let mut painted = Painted::default();
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
            let _ = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), &mut selected, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
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
            action = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), &mut selected, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
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
            state: crate::send::SendState::Waiting,
            // The facts the detail pane draws, at the shape a live, untouched
            // Send really has. Spelled out rather than defaulted: a `Default`
            // on `SendRow` would be a row that describes no Send at all, and
            // this file's whole subject is states that must not be
            // mistakable for one another.
            access_count: 0,
            max_access_count: None,
            has_password: false,
            activity: "Nobody has opened this link. The link works, as often as they like."
                .to_string(),
            expires: NO_EXPIRY_OF_ITS_OWN.to_string(),
            deletes: "17 Aug 2026, 00:00 \u{00b7} in 7 days".to_string(),
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

    /// **Copy link copies the row the detail pane is describing.** Tried on
    /// the *last* row of six as well as the first, because a wrong-row bug
    /// that reaches for index 0 is invisible when the test only picks the
    /// first one.
    ///
    /// The control moved -- there is one Copy link now, in the detail header,
    /// where 5b puts it -- and the property is unchanged and is what is
    /// asserted: everything the action carries is read off the Send the pane
    /// is showing and off nothing else, so no lookup on the far side can
    /// resolve to a different one.
    #[test]
    fn copy_link_reports_the_url_of_the_send_the_pane_is_showing() {
        let state = rows(6);
        let SendPaneState::Rows(model) = &state else { panic!("not rows") };
        for index in [0usize, 3, 5] {
            let expected = model[index].access_url.clone();
            assert_eq!(
                click_nth_picking(
                    &state,
                    SendDeleteView::default(),
                    Some(model[index].id.clone()),
                    "Copy link",
                    0,
                ),
                SendUiAction::CopyLink(expected.clone()),
                "Copy link did not report {expected} while row {index} was picked"
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
            let _ = draw_send_pane(ui, &state, Some("a message"), SendDeleteView::default(), SendView::Mine(ALL_SENDS), &mut None, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
        });
        let mut painted = Painted::default();
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
            let _ = draw_send_pane(ui, &state, Some("a message"), SendDeleteView::default(), SendView::Mine(ALL_SENDS), &mut None, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
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
            action = draw_send_pane(ui, &state, Some("a message"), SendDeleteView::default(), SendView::Mine(ALL_SENDS), &mut None, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
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
        let mut selected = first_row_id(state);
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
            let _ = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), &mut selected, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
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
            let _ = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), &mut selected, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
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
            action = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), &mut selected, &mut SendComposer::default(), false, &crate::send::FixedClock(0), &crate::local_time::FixedOffset(0)).into_action();
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
    ///
    /// Its ancestor counted six Delete buttons because the control was on the
    /// row; 5b puts it in the detail pane, so the first half is now "every
    /// row leads to one" -- checked by picking each of the two ends and
    /// pressing what appears. What did not change at all is the second half.
    #[test]
    fn every_send_can_be_revoked_and_the_first_click_only_asks() {
        let state = rows(6);
        let (painted, _) = paint_with(&state, None, min_pane_size(), SendDeleteView::default());
        assert_eq!(
            painted.count_exact(DELETE_LABEL),
            1,
            "the picked Send has no Delete button, or the rows grew one each: {:?}",
            painted.text
        );
        // Nothing destructive is even OFFERED before the first click.
        assert!(
            !painted.has(CONFIRM_LABEL),
            "the destructive button is painted before anything was asked: {:?}",
            painted.text
        );

        for nth in [0usize, 5] {
            let action = click_nth_picking(
                &state,
                SendDeleteView::default(),
                Some(format!("id{nth}")),
                DELETE_LABEL,
                0,
            );
            assert_eq!(
                action,
                SendUiAction::AskDelete(format!("id{nth}")),
                "Delete did not ask about row {nth} while row {nth} was picked"
            );
        }
    }

    /// **The confirmation belongs to exactly one Send, and it is the one that
    /// was asked about.**
    ///
    /// Two claims, and the second is the one that moved. It used to be "the
    /// confirmation is on one ROW and the other three keep their Delete
    /// buttons", which was a statement about a list of controls this screen
    /// no longer has. What replaces it is stronger: the confirmation is
    /// raised against an id, so **picking a different Send while one is armed
    /// shows that Send's ordinary controls and no confirmation at all** --
    /// which is the property the row-level count was standing in for.
    #[test]
    fn only_the_send_asked_about_shows_the_confirmation() {
        let state = rows(4);
        let armed = SendDeleteView { confirming: Some("id2"), in_flight: None };
        let (painted, _) =
            paint_picking(&state, None, min_pane_size(), armed, Some("id2".to_string()));
        assert_eq!(
            painted.count_exact(CONFIRM_LABEL),
            1,
            "the destructive button is painted {} times, not once: {:?}",
            painted.count_exact(CONFIRM_LABEL),
            painted.text
        );
        assert_eq!(
            painted.count_exact(CANCEL_LABEL),
            1,
            "the way out of the confirmation is not painted exactly once"
        );
        assert_eq!(
            painted.count_exact(DELETE_LABEL),
            0,
            "the armed Send kept its Delete button beside the confirmation, so the two steps \
             are one click apart again"
        );
        assert!(
            painted.has(CONFIRM_PROMPT),
            "the Send that is about to be revoked does not say what that means: {:?}",
            painted.text
        );

        // A DIFFERENT Send, with the same confirmation still armed on `id2`:
        // ordinary controls, no question, nothing destructive offered.
        let (other, _) =
            paint_picking(&state, None, min_pane_size(), armed, Some("id0".to_string()));
        assert_eq!(
            other.count_exact(CONFIRM_LABEL) + other.count_exact(CANCEL_LABEL),
            0,
            "a confirmation raised on id2 is showing over id0: {:?}",
            other.text
        );
        assert_eq!(
            other.count_exact(DELETE_LABEL),
            1,
            "the Send that is NOT being confirmed lost its own controls: {:?}",
            other.text
        );
        assert!(!other.has(CONFIRM_PROMPT), "the question followed the selection: {:?}", other.text);

        // And it answers for its own Send and no other.
        assert_eq!(
            click_nth_picking(&state, armed, Some("id2".to_string()), CONFIRM_LABEL, 0),
            SendUiAction::ConfirmDelete {
                id: "id2".to_string(),
                name: "send-number-2".to_string(),
            },
            "the confirmation answered for a Send other than the one it was asked about"
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
        // The pane describes the FIRST row, which is what `click_at_with`
        // picks, so the button this remembers is that Send's own.
        let where_delete_was = rect_of_nth(&state, idle, DELETE_LABEL, 0).center();

        // The first click arms the confirmation for that Send.
        assert_eq!(
            click_at_with(&state, idle, where_delete_was),
            SendUiAction::AskDelete("id0".to_string()),
            "control: the remembered position is not the Delete button of the Send on screen"
        );

        // The second click, at the very same pixel, on the redrawn pane.
        let armed = SendDeleteView { confirming: Some("id0"), in_flight: None };
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

    /// **A Send whose revoke is running has no control on it at all**, so a
    /// second click cannot start a second `bw send delete` for a Send that is
    /// already being revoked.
    ///
    /// The absence is asserted twice over, and the second is what closes it:
    /// no widget is painted in the action row, AND every pixel across that
    /// row reports nothing. A single sample can miss a control by ten pixels,
    /// and "no button" has to be true of the whole strip.
    ///
    /// The progress word is in **two** places now, and both are deliberate:
    /// the detail's action row, where the controls were, and the list row's
    /// own second line -- because a user who picks a different Send while one
    /// is being destroyed would otherwise see nothing anywhere about it.
    #[test]
    fn a_send_being_revoked_has_no_buttons_and_says_so() {
        let state = rows(3);
        let busy = SendDeleteView { confirming: None, in_flight: Some("id0") };
        let (painted, _) = paint_with(&state, None, min_pane_size(), busy);

        assert_eq!(
            painted.in_detail(DELETING_LABEL),
            1,
            "the detail pane does not say that anything is happening: {:?}",
            painted.text
        );
        assert_eq!(
            painted.in_list(DELETING_LABEL),
            1,
            "the row of the Send being revoked does not say so, so a user looking at another \
             Send sees nothing at all: {:?}",
            painted.text
        );
        for label in [DELETE_LABEL, "Copy link", SWITCH_OFF_LABEL, SWITCH_ON_LABEL, CONFIRM_LABEL]
        {
            assert_eq!(
                painted.count_exact(label),
                0,
                "{label:?} is still painted for a Send that is already being revoked: {:?}",
                painted.text
            );
        }

        // Every pixel of the action row reports nothing. Swept across the
        // detail column rather than at one point.
        let line = painted
            .rects_of_exact(DELETING_LABEL)
            .into_iter()
            .find(|r| r.center().x >= crate::vault_window::LIST_WIDTH)
            .expect("counted above");
        let detail_left = crate::vault_window::LIST_WIDTH;
        let span = min_pane_size().x - detail_left;
        for x in [0.05f32, 0.2, 0.4, 0.6, 0.8, 0.95] {
            let pos = egui::pos2(detail_left + span * x, line.center().y);
            assert_eq!(
                click_at_with(&state, busy, pos),
                SendUiAction::None,
                "a click at {pos:?} on a Send that is already being revoked reported an action"
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
            state: crate::send::SendState::Waiting,
            // The facts the detail pane draws, at the shape a live, untouched
            // Send really has. Spelled out rather than defaulted: a `Default`
            // on `SendRow` would be a row that describes no Send at all, and
            // this file's whole subject is states that must not be
            // mistakable for one another.
            access_count: 0,
            max_access_count: None,
            has_password: false,
            activity: "Nobody has opened this link. The link works, as often as they like."
                .to_string(),
            expires: NO_EXPIRY_OF_ITS_OWN.to_string(),
            deletes: "17 Aug 2026, 00:00 \u{00b7} in 7 days".to_string(),
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

    // ---- design 5b's detail pane, and the half of 5c that is derivable ----

    /// A Send with every fact the detail pane can show: a password, a view
    /// cap partly spent, an expiry of its own, and a deletion date after it.
    fn a_fully_described_send() -> SendSummary {
        SendSummary {
            id: "id-detail".to_string(),
            name: "SAP Production".to_string(),
            access_url: "https://send.bitwarden.com/#/g7HqK2".to_string(),
            // 2026-08-17 and 2026-08-20: the link stops answering three days
            // before the record goes, which is the gap `send_state` reads
            // both dates for and the reason the pane prints both.
            expiration_date: "2026-08-17T00:00:00.000Z".to_string(),
            deletion_date: "2026-08-20T00:00:00.000Z".to_string(),
            is_file: false,
            max_access_count: Some(10),
            access_count: 3,
            disabled: false,
            has_password: true,
        }
    }

    fn detail_of(send: SendSummary) -> (Painted, SendUiAction) {
        let id = send.id.clone();
        let state = pane_state(Some(&Ok(vec![send])), &FixedClock(NOW), &UTC);
        paint_picking(&state, None, min_pane_size(), SendDeleteView::default(), Some(id))
    }

    /// **Design 5b's `Link` card, every row of it, at the minimum window
    /// size.**
    ///
    /// The card is what the detail pane is FOR, so its rows are pinned by
    /// name and by value rather than by "a card was drawn": a pane that lost
    /// one row would otherwise pass every assertion about the others.
    ///
    /// **Both dates, and they are different dates.** `expiration_date` is
    /// when the link stops answering and `deletion_date` is when the record
    /// goes; the fixture puts three days between them precisely so a pane
    /// that printed one date twice fails here.
    #[test]
    fn the_detail_pane_draws_the_link_card_at_the_minimum_window_size() {
        let (painted, _) = detail_of(a_fully_described_send());
        for label in [LINK_CARD_TITLE, ADDRESS_ROW, OPENS_WITH_ROW, VIEWS_ROW, EXPIRES_ROW, DELETED_ROW]
        {
            assert_eq!(
                painted.in_detail(label),
                1,
                "the Link card has no {label:?} row: {:?}",
                painted.text
            );
        }
        // The address itself, and the two facts beside it.
        assert!(
            painted.has("https://send.bitwarden.com/#/g7HqK2"),
            "the detail pane does not show the link it is about: {:?}",
            painted.text
        );
        assert_eq!(
            painted.count_exact(PASSWORD_SEGMENT),
            1,
            "a Send that needs a password does not say so in its `Opens with` row: {:?}",
            painted.text
        );
        assert!(
            painted.has("3 of 10"),
            "the views row does not say the count against its cap: {:?}",
            painted.text
        );

        // **Two different dates, in the two rows that mean two different
        // things.** Asserted as painted strings rather than as fields, so a
        // pane that wired both rows to one date fails.
        let expires = painted
            .text
            .iter()
            .find(|t| t.contains("17 Aug 2026"))
            .unwrap_or_else(|| panic!("no expiry date was painted: {:?}", painted.text));
        let deletes = painted
            .text
            .iter()
            .find(|t| t.contains("20 Aug 2026"))
            .unwrap_or_else(|| panic!("no deletion date was painted: {:?}", painted.text));
        assert_ne!(
            expires, deletes,
            "the Expires and Deleted rows print the same string, so one of the two dates is \
             not being read -- which is how a link that already 404s comes to be described \
             as live"
        );

        // Nothing spilled out of the pane at the floor.
        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, min_pane_size());
        for (text, rect) in &painted.text_rects {
            assert!(
                rect.left() >= -0.5 && rect.right() <= pane.right() + 0.5,
                "{text:?} was painted at {rect:?}, outside the {pane:?} pane at the minimum \
                 window size"
            );
        }
    }

    /// **A Send with no password says what opening it needs, in the
    /// affirmative.**
    ///
    /// The row is never blank and never says "no password": what the reader
    /// wants to know is what the recipient has to have, and for most Sends
    /// this app makes the answer is "the link, and that is all".
    #[test]
    fn the_opens_with_row_says_what_the_recipient_needs() {
        let (with, _) = detail_of(a_fully_described_send());
        assert_eq!(with.count_exact(PASSWORD_SEGMENT), 1);
        assert_eq!(with.count_exact(LINK_ONLY), 0);

        let (without, _) =
            detail_of(SendSummary { has_password: false, ..a_fully_described_send() });
        assert_eq!(
            without.count_exact(LINK_ONLY),
            1,
            "a Send whose link is the whole credential leaves the row blank: {:?}",
            without.text
        );
        assert_eq!(without.count_exact(PASSWORD_SEGMENT), 0);
    }

    /// **A dead link is struck through and said to be dead**, which is 5b's
    /// own treatment -- and it is the STATE that decides, not the date: a
    /// Send whose views are spent is exactly as unopenable as one that
    /// expired.
    #[test]
    fn a_dead_links_address_is_struck_through_and_marked() {
        // Live: no marker.
        let (live, _) = detail_of(a_fully_described_send());
        assert_eq!(
            live.count_exact(DEAD_MARKER),
            0,
            "a live link is marked dead: {:?}",
            live.text
        );

        // Views spent. Nothing about the DATES changed.
        let used = SendSummary { access_count: 10, ..a_fully_described_send() };
        assert_eq!(
            crate::send::send_state(&used, &FixedClock(NOW)),
            crate::send::SendState::Used,
            "control: the fixture is not in the state this test is about"
        );
        let (painted, _) = detail_of(used);
        assert_eq!(
            painted.count_exact(DEAD_MARKER),
            1,
            "a Send whose views are spent still presents its link as working: {:?}",
            painted.text
        );
        // The strike is a thin filled rect over the address, so it is found
        // by geometry: a line no taller than two points, lying across the
        // middle of the URL that was painted.
        let url = painted
            .rect_of("https://send.bitwarden.com")
            .expect("the address was not painted at all");
        assert!(
            painted.fills.iter().any(|(rect, _)| {
                rect.height() <= 2.0
                    && rect.width() > 10.0
                    && (rect.center().y - url.center().y).abs() < 4.0
                    && rect.left() >= url.left() - 2.0
            }),
            "nothing is struck through the dead address at {url:?}: the pane says `dead` \
             beside a link that still reads as ordinary text"
        );
    }

    /// **There is NO Activity card, and its absence is not a hole.**
    ///
    /// 5c wants a per-access timeline -- "Password revealed - 15:01 - Edge on
    /// Windows - Berlin, DE" -- and a Bitwarden Send carries no per-access
    /// record at all. A card headed `ACTIVITY` with nothing under it would
    /// read as a load that failed AND promise a feature no work on this
    /// screen can deliver, so there is none.
    ///
    /// What stands in its place is one sentence carrying the half of 5c's
    /// premise that IS derivable, in the position 5c put its headline. Both
    /// halves are asserted: the empty box is absent, and the sentence is
    /// there.
    #[test]
    fn the_detail_pane_answers_5cs_question_without_an_empty_activity_card() {
        let (painted, _) = detail_of(a_fully_described_send());
        for absent in ["ACTIVITY", "Activity", "Berlin", "Edge"] {
            assert!(
                !painted.has(absent),
                "the detail pane paints {absent:?} -- there is no per-access record in a \
                 Bitwarden Send, so this is either an empty box or an invention: {:?}",
                absent
            );
        }
        // 5c's own question, answered in words.
        assert!(
            painted.has("Opened 3 times"),
            "the pane does not say whether the link was used, which 5c's own subtitle calls \
             the question people actually have: {:?}",
            painted.text
        );
        assert!(
            painted.has("7 views left"),
            "the pane does not say what is left of the budget: {:?}",
            painted.text
        );
    }

    /// **Nothing is picked, and the pane says so in words.**
    #[test]
    fn a_detail_pane_with_nothing_picked_says_so() {
        let state = rows(3);
        let (painted, _) =
            paint_picking(&state, None, min_pane_size(), SendDeleteView::default(), None);
        assert!(
            painted.has(NOTHING_PICKED),
            "the detail column is blank with nothing picked, which reads as a load that \
             failed: {:?}",
            painted.text
        );
        for control in ["Copy link", DELETE_LABEL, SWITCH_OFF_LABEL] {
            assert_eq!(
                painted.count_exact(control),
                0,
                "{control:?} is drawn with no Send picked, so it would act on nothing"
            );
        }
        // A selection naming a row that is not in the list is the same state,
        // not a stale pane.
        let (stale, _) = paint_picking(
            &state,
            None,
            min_pane_size(),
            SendDeleteView::default(),
            Some("id-that-is-gone".to_string()),
        );
        assert!(
            stale.has(NOTHING_PICKED),
            "a selection naming a Send that is no longer in the list draws something other \
             than the nothing-picked pane: {:?}",
            stale.text
        );
    }

    // ---- design 5b's SHARING sub-filters, on the pane ----

    /// Four Sends, one in each state, so a scope test can see what it cut.
    fn one_of_each_state() -> SendPaneState {
        use crate::send::SendState;
        let base = SendSummary {
            id: String::new(),
            name: String::new(),
            access_url: "https://send.bitwarden.com/#/x".to_string(),
            deletion_date: "2026-08-17T00:00:00.000Z".to_string(),
            is_file: false,
            max_access_count: None,
            access_count: 0,
            disabled: false,
            expiration_date: String::new(),
            has_password: false,
        };
        let sends: Vec<SendSummary> = [
            SendState::Waiting,
            SendState::Used,
            SendState::Expired,
            SendState::Revoked,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, state)| {
            let send = match state {
                SendState::Waiting => base.clone(),
                SendState::Used => SendSummary {
                    max_access_count: Some(1),
                    access_count: 1,
                    ..base.clone()
                },
                SendState::Expired => SendSummary {
                    deletion_date: "2026-08-01T00:00:00.000Z".to_string(),
                    ..base.clone()
                },
                SendState::Revoked => SendSummary { disabled: true, ..base.clone() },
            };
            let send = SendSummary {
                id: format!("id-{i}"),
                name: format!("send-{}", state_label(state).to_lowercase()),
                ..send
            };
            assert_eq!(
                crate::send::send_state(&send, &FixedClock(NOW)),
                state,
                "control: the fixture for {state:?} does not derive to {state:?}"
            );
            send
        })
        .collect();
        pane_state(Some(&Ok(sends)), &FixedClock(NOW), &UTC)
    }

    /// [`paint_picking`] on a named SHARING sub-filter.
    fn paint_scope(
        state: &SendPaneState,
        scope: crate::vault_window::sidebar::SendScope,
    ) -> Painted {
        let size = min_pane_size();
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});
        let mut selected = None;
        let output = ctx.run_ui(input(), |ui| {
            draw_send_pane(
                ui,
                state,
                None,
                SendDeleteView::default(),
                SendView::Mine(scope),
                &mut selected,
                &mut SendComposer::default(),
                false,
                &FixedClock(NOW),
                &UTC,
            )
            .into_action();
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        painted
    }

    /// **Each sub-row lists exactly the Sends its badge counts, and `Ended`
    /// really is the two states merged.**
    ///
    /// The rows the pane lists and the number the rail prints go through ONE
    /// predicate -- `SendScope::admits` -- and this is what says so from the
    /// painted side: a scope that listed more than it counted would show a
    /// name here that its own sub-row does not admit.
    #[test]
    fn each_sharing_sub_filter_lists_exactly_what_its_badge_counts() {
        use crate::send::SendState;
        use crate::vault_window::sidebar::SendScope;
        let state = one_of_each_state();
        for (scope, expected) in [
            (SendScope::All, vec!["send-waiting", "send-used", "send-expired", "send-revoked"]),
            (SendScope::Waiting, vec!["send-waiting"]),
            (SendScope::Used, vec!["send-used"]),
            // The merge, and the whole of what makes `Ended` a row rather
            // than a rename of either state.
            (SendScope::Ended, vec!["send-expired", "send-revoked"]),
        ] {
            let painted = paint_scope(&state, scope);
            for name in ["send-waiting", "send-used", "send-expired", "send-revoked"] {
                let wanted = expected.contains(&name);
                assert_eq!(
                    painted.in_list(name) == 1,
                    wanted,
                    "under {scope:?} the list {} {name:?}, and it should {}: {:?}",
                    if wanted { "does not show" } else { "shows" },
                    if wanted { "" } else { "not" },
                    painted.text
                );
            }
            // The strip names the cut in force, so a user who clicked `Ended`
            // and then reads `Revoked` on a pill has the bridge on screen.
            assert!(
                painted.has(scope.eyebrow()),
                "the list strip does not name the {scope:?} cut it is showing: {:?}",
                painted.text
            );
        }

        // The counts the rail would draw, over the same four Sends and the
        // same predicate.
        let counts = crate::vault_window::sidebar::SendCounts::over([
            SendState::Waiting,
            SendState::Used,
            SendState::Expired,
            SendState::Revoked,
        ]);
        assert_eq!((counts.waiting, counts.used, counts.ended, counts.all), (1, 1, 2, 4));
    }

    /// **An empty sub-row does NOT say "you have no Sends".**
    ///
    /// Three claims this screen has always had to keep apart, and this is the
    /// third: "you have none" is about the account, "we could not check" is
    /// about this app, and "none of yours is used" is about the filter the
    /// user themselves just chose. Telling somebody who clicked `Used` that
    /// they have published nothing is false in the direction that matters.
    #[test]
    fn an_empty_sub_filter_says_which_filter_is_empty_and_not_that_the_account_is() {
        use crate::vault_window::sidebar::SendScope;
        // One Send, waiting. `Used` and `Ended` are therefore empty cuts of a
        // non-empty account.
        let state = one_row(crate::send::SendState::Waiting);
        for scope in [SendScope::Used, SendScope::Ended] {
            let painted = paint_scope(&state, scope);
            assert!(
                painted.has(&scope_empty_headline(scope)),
                "the {scope:?} row is empty and does not say so in its own words: {:?}",
                painted.text
            );
            assert!(
                !painted.has(EMPTY_HEADLINE),
                "an empty {scope:?} row told the user their ACCOUNT has no Sends, which is \
                 false: {:?}",
                painted.text
            );
            assert!(
                painted.has(SCOPE_EMPTY_DETAIL),
                "the empty row does not say where the rest of the Sends are: {:?}",
                painted.text
            );
        }
        // The control: the same account, unfiltered, has a Send.
        let all = paint_scope(&state, SendScope::All);
        assert!(!all.has(EMPTY_HEADLINE), "control: the fixture account really is empty");
        assert_eq!(all.in_list("SAP Production"), 1);
    }

    // ---- design 5b's `Shared with me` ----

    fn a_received_record(at: i64, name: &str, item: &str) -> crate::receive_history::ReceivedRecord {
        crate::receive_history::ReceivedRecord {
            received_at_unix_millis: at,
            name: name.to_string(),
            item_id: item.to_string(),
        }
    }

    fn vault_item(id: &str) -> crate::vault_bridge::VaultItem {
        crate::vault_bridge::VaultItem {
            id: id.to_string(),
            name: id.to_string(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// **A received row knows whether the item it made is still there**, and
    /// a record whose item id never existed is reported as gone rather than
    /// guessed at.
    ///
    /// The history outlives the item, so this is ordinary rather than
    /// exceptional -- and a row that silently points at nothing is the
    /// failure this screen has to avoid.
    #[test]
    fn a_received_row_says_whether_its_item_survived() {
        let history = crate::receive_history::ReceiveHistory {
            entries: vec![
                a_received_record(NOW - 3_600_000, "SAP Production", "item-here"),
                a_received_record(NOW - 7_200_000, "Office WiFi", "item-gone"),
                // A record from a version that did not store an id, or a
                // hand-edited file. Still a row, still selectable, and
                // reported as gone: this app cannot find the item, and
                // "still in your vault" about an item it cannot name would
                // be a guess in the reassuring direction.
                a_received_record(NOW - 10_800_000, "Legacy", ""),
            ],
        };
        let items = vec![vault_item("item-here")];
        let built = received_rows(&history, &items, &FixedClock(NOW), &UTC);
        assert_eq!(built.len(), 3);
        assert!(built[0].still_in_vault, "the item that IS in the vault is reported as gone");
        assert!(!built[1].still_in_vault, "an item that is not in the vault is reported present");
        assert!(!built[2].still_in_vault, "a record with no item id claimed the item survives");
        // Newest first, and every row has a key to be picked by.
        assert_eq!(built[0].name, "SAP Production");
        for row in &built {
            assert!(!row.key.is_empty(), "a row has no key, so it cannot be picked: {row:?}");
        }
        assert_ne!(built[2].key, built[1].key, "two rows share one key");
        // The times are the user's own day, and "1 hour ago" rather than a
        // raw instant alone.
        assert!(
            built[0].when.contains("ago"),
            "a received row does not say how long ago it arrived: {:?}",
            built[0].when
        );
    }

    /// The `Shared with me` screen, drawn.
    fn paint_received(rows: &[ReceivedRow], picked: Option<String>) -> Painted {
        let size = min_pane_size();
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});
        let mut selected = picked;
        let state = SendPaneState::Loading;
        let output = ctx.run_ui(input(), |ui| {
            draw_send_pane(
                ui,
                // **`Loading`, deliberately.** The received screen asks no
                // question of the server, so the Sends fetch is untouched
                // while it is up -- and nothing it draws may come from that
                // state. A pane that leaked `Loading` onto this screen would
                // paint the spinner, which is asserted against below.
                &state,
                None,
                SendDeleteView::default(),
                SendView::Received(rows),
                &mut selected,
                &mut SendComposer::default(),
                false,
                &FixedClock(NOW),
                &UTC,
            )
            .into_action();
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        painted
    }

    /// **`Shared with me` lists what was imported, and its detail says where
    /// the item went.**
    ///
    /// Including the one thing this screen must never do: draw the Sends
    /// list's own states. It shares a pane with them and reads a local file
    /// instead of a fetch, so a `Loading` spinner or a `Refresh` button here
    /// would be the account's Sends leaking into a screen that is not about
    /// them.
    #[test]
    fn the_shared_with_me_screen_lists_imports_and_says_where_each_went() {
        let history = crate::receive_history::ReceiveHistory {
            entries: vec![
                a_received_record(NOW - 3_600_000, "SAP Production", "item-here"),
                a_received_record(NOW - 7_200_000, "Office WiFi", "item-gone"),
            ],
        };
        let items = vec![vault_item("item-here")];
        let rows = received_rows(&history, &items, &FixedClock(NOW), &UTC);

        let listed = paint_received(&rows, None);
        assert!(listed.has(RECEIVED_HEADING), "the strip does not name this screen: {:?}", listed.text);
        assert_eq!(listed.in_list("SAP Production"), 1);
        assert_eq!(listed.in_list("Office WiFi"), 1);
        assert!(listed.has(NOTHING_PICKED_RECEIVED), "nothing picked and nothing said");
        // The Sends list's own machinery is nowhere on this screen.
        for leaked in [LOADING_LABEL, REFRESH_LABEL, NEW_SEND_LABEL, SCOPE_SUBTEXT] {
            assert!(
                !listed.has(leaked),
                "{leaked:?} is drawn on the `Shared with me` screen, which asks the server \
                 nothing: {:?}",
                listed.text
            );
        }

        // The item that survived, and the one that did not.
        let present = paint_received(&rows, Some("item-here".to_string()));
        assert!(
            present.has(RECEIVED_ITEM_PRESENT),
            "a record whose item is still in the vault does not say so: {:?}",
            present.text
        );
        assert_eq!(present.in_detail(IN_VAULT_YES), 1);
        assert!(present.has(ARRIVED_ROW), "the detail does not say when it arrived");

        let gone = paint_received(&rows, Some("item-gone".to_string()));
        assert!(
            gone.has(RECEIVED_ITEM_GONE),
            "a record whose item has been deleted points at nothing and says nothing: {:?}",
            gone.text
        );
        assert_eq!(gone.in_detail(IN_VAULT_NO), 1);
    }

    /// **An empty `Shared with me` is a claim, and it is a true one.**
    ///
    /// Unlike the Sends list there is no fetch behind this screen, so there
    /// is no "we could not check" to be confused with "there is nothing" --
    /// `ReceiveHistory::load` folds every failure into an empty history, for
    /// the reasons argued there. So the empty state says so outright.
    #[test]
    fn an_empty_shared_with_me_says_nothing_has_arrived() {
        let painted = paint_received(&[], None);
        assert!(painted.has(RECEIVED_EMPTY_HEADLINE), "{:?}", painted.text);
        assert!(painted.has(RECEIVED_EMPTY_DETAIL), "{:?}", painted.text);
        assert!(
            !painted.has(EMPTY_HEADLINE),
            "the received screen borrowed the Sends list's own empty sentence, which is a \
             claim about a different thing: {:?}",
            painted.text
        );
    }

    /// **The activity sentence says what happened and what is left, and never
    /// invents a time.**
    ///
    /// Pure, so every state is checked rather than the one a fixture happens
    /// to build. The forbidden half is asserted too: 5c's timeline wants
    /// clock times and places, and none of them is derivable, so none of them
    /// may appear.
    #[test]
    fn the_activity_sentence_answers_whether_it_was_used_and_invents_no_when() {
        use crate::send::SendState;
        let base = a_fully_described_send();
        let cases = [
            (SendSummary { access_count: 0, max_access_count: None, ..base.clone() }, SendState::Waiting, "Nobody has opened this link"),
            (SendSummary { access_count: 1, max_access_count: None, ..base.clone() }, SendState::Waiting, "Opened once"),
            (SendSummary { access_count: 3, max_access_count: Some(10), ..base.clone() }, SendState::Waiting, "7 views left"),
            (SendSummary { access_count: 1, max_access_count: Some(1), ..base.clone() }, SendState::Used, "spent"),
            (SendSummary { disabled: true, ..base.clone() }, SendState::Revoked, SWITCH_ON_LABEL),
            (
                SendSummary {
                    deletion_date: "2026-08-01T00:00:00.000Z".to_string(),
                    expiration_date: String::new(),
                    max_access_count: None,
                    ..base.clone()
                },
                SendState::Expired,
                "ran out of time",
            ),
        ];
        for (send, state, needle) in cases {
            assert_eq!(
                crate::send::send_state(&send, &FixedClock(NOW)),
                state,
                "control: the fixture for {state:?} does not derive to {state:?}"
            );
            let sentence = activity_sentence(&send, &FixedClock(NOW));
            assert!(
                sentence.contains(needle),
                "the {state:?} sentence does not contain {needle:?}: {sentence:?}"
            );
            // Nothing 5c wanted and this client cannot know.
            for forbidden in ["15:0", "minutes after", "Edge", "Berlin", "84."] {
                assert!(
                    !sentence.contains(forbidden),
                    "the {state:?} sentence claims {forbidden:?}, which no Bitwarden Send \
                     records: {sentence:?}"
                );
            }
        }
        // A server that reports more opens than the cap must not print four
        // billion views left.
        let over = SendSummary { access_count: 5, max_access_count: Some(10), ..a_fully_described_send() };
        assert!(activity_sentence(&over, &FixedClock(NOW)).contains("5 views left"));
    }

    /// **A stored instant round-trips into the one date sentence this screen
    /// has**, so the received rows and the Send rows word a date the same
    /// way. See `iso_from_millis`, which exists so there is not a second
    /// formatter.
    #[test]
    fn a_stored_instant_is_worded_by_the_same_reader_as_a_wire_date() {
        // 2026-08-10T00:00:00Z is `NOW`, so an instant one day earlier is
        // "1 day ago" and the day itself is the ninth.
        let a_day_ago = NOW - crate::local_time::MILLIS_PER_DAY;
        let round_tripped = iso_from_millis(a_day_ago);
        assert_eq!(
            parse_iso_utc_millis(&round_tripped),
            Some(a_day_ago),
            "the instant did not survive the trip through the wire shape: {round_tripped:?}"
        );
        let words = date_words(&round_tripped, &FixedClock(NOW), &UTC, "absent");
        assert!(words.contains("9 Aug 2026"), "{words:?}");
        assert!(words.contains("1 day ago"), "{words:?}");

        // Hours below a day, which is what 5b prints on both sides of its own
        // list ("expires in 3 h"): a link with four hours left described as
        // "in 0 days" is the arithmetic slip that makes a countdown useless
        // exactly when it matters.
        let in_four_hours = NOW + 4 * crate::local_time::MILLIS_PER_HOUR;
        let soon = date_words(&iso_from_millis(in_four_hours), &FixedClock(NOW), &UTC, "absent");
        assert!(soon.contains("in 4 hours"), "{soon:?}");
        assert!(!soon.contains("0 days"), "{soon:?}");

        // An unreadable date is the caller's own word, never a guess.
        assert_eq!(date_words("not a date", &FixedClock(NOW), &UTC, "absent"), "absent");
    }

    // -----------------------------------------------------------------
    // The pass that put this screen beside design 5b
    // -----------------------------------------------------------------
    //
    // Every assertion below is run at BOTH pane widths, which is this
    // screen's standing rule and is more than a formality here: three of
    // these four properties are about a layout that deliberately differs
    // between the two, and a test at one size would pin half of it.

    /// The vault window's centre pane at the SHIPPED window size -- 1240 wide
    /// less the rail -- which is the comfortable end of the range
    /// [`min_pane_size`] is the floor of.
    ///
    /// Named beside its twin because almost nothing on this screen is
    /// interesting at one width alone: what a reviewer is looking for is the
    /// difference, and what a test is looking for is that BOTH ends behave.
    fn roomy_pane_size() -> egui::Vec2 {
        egui::vec2(1240.0 - crate::vault_window::SIDEBAR_WIDTH, 740.0)
    }

    /// The rect of a run of exactly `label` in the DETAIL column.
    fn detail_rect(painted: &Painted, label: &str) -> egui::Rect {
        painted
            .rects_of_exact(label)
            .into_iter()
            .find(|r| r.center().x >= crate::vault_window::LIST_WIDTH)
            .unwrap_or_else(|| {
                panic!("{label:?} was not painted in the detail column: {:?}", painted.text)
            })
    }

    /// **Design 5b's header is ONE band, and it is one band whenever it can
    /// be.**
    ///
    /// 5b draws the detail pane's controls on the title's own line, right
    /// aligned; this pane drew them on a row of their own underneath, which
    /// is most of why the column read as a stack of parts rather than as a
    /// page. It cannot always: at `MIN_VAULT_WINDOW_SIZE` the column is 298pt
    /// and 5b's arrangement does not fit at any padding. So the rule is
    /// measured, and this is the test that says the measurement really
    /// switches -- a `let inline = true` would pass half of it and a
    /// `let inline = false` the other half.
    #[test]
    fn the_detail_actions_share_the_title_line_when_there_is_room_and_take_their_own_when_there_is_not()
    {
        let state = one_row(crate::send::SendState::Waiting);

        let (roomy, _) = paint(&state, None, roomy_pane_size());
        let copy = detail_rect(&roomy, COPY_LINK_LABEL);
        let title = detail_rect(&roomy, "SAP Production");
        let pill = detail_rect(&roomy, WAITING_LABEL);
        assert!(
            copy.left() > title.right(),
            "with room to spare the actions are not beside the title: Copy link at {copy:?}, \
             title at {title:?}"
        );
        assert!(
            copy.center().y < pill.bottom(),
            "with room to spare the actions still sit BELOW the header block rather than on \
             its line: Copy link centred at y={}, the pill ends at y={}",
            copy.center().y,
            pill.bottom()
        );

        let (tight, _) = paint(&state, None, min_pane_size());
        let copy = detail_rect(&tight, COPY_LINK_LABEL);
        let pill = detail_rect(&tight, WAITING_LABEL);
        assert!(
            copy.center().y > pill.bottom(),
            "at the minimum window size the actions were squeezed onto the title's line, \
             where 5b's own arrangement does not fit: Copy link centred at y={}, the pill \
             ends at y={}",
            copy.center().y,
            pill.bottom()
        );
        // And the row is still INSIDE the pane, which is the whole reason the
        // padding shrinks rather than the labels.
        let right = min_pane_size().x;
        for label in [COPY_LINK_LABEL, SWITCH_OFF_LABEL, DELETE_LABEL] {
            let rect = detail_rect(&tight, label);
            assert!(
                rect.right() <= right,
                "{label:?} runs past the right edge of a {right}pt pane: {rect:?}"
            );
        }
    }

    /// **Cancel lands in Delete's exact rectangle at BOTH widths.**
    ///
    /// The mis-click defence on this screen is that a second rapid click
    /// where Delete was lands on Cancel, and that holds only while the two
    /// rectangles are equal. It is asserted here at both ends because the
    /// header now measures its own row: a measurement that counted only the
    /// VISIBLE controls -- Cancel replaces Delete and the switch leaves --
    /// made the right-aligned row narrower on the confirming frame and slid
    /// Cancel away from Delete's pixels. That bug was written, and this is
    /// what found it. See [`action_labels`].
    #[test]
    fn cancel_stands_exactly_where_delete_was_at_both_pane_widths() {
        let state = one_row(crate::send::SendState::Waiting);
        for size in [roomy_pane_size(), min_pane_size()] {
            let (before, _) = paint_with(&state, None, size, SendDeleteView::default());
            let (during, _) = paint_with(
                &state,
                None,
                size,
                SendDeleteView { confirming: Some("id0"), in_flight: None },
            );
            let delete = detail_rect(&before, DELETE_LABEL);
            let cancel = detail_rect(&during, CANCEL_LABEL);
            let delete_box = before
                .fill_behind(delete)
                .expect("Delete has no button behind it")
                .0;
            let cancel_box = during
                .fill_behind(cancel)
                .expect("Cancel has no button behind it")
                .0;
            // **The whole rectangle, not its centre.** A button eight points
            // taller than the one it replaces has the same centre and is not
            // the same target -- which is exactly the defect this test found
            // on its first run: egui wrapped `Cancel` inside a slot measured
            // for `Delete` and grew the button to fit. See
            // `theme::action_button`'s wrap mode.
            assert!(
                (delete_box.min - cancel_box.min).length() < 0.5
                    && (delete_box.max - cancel_box.max).length() < 0.5,
                "at a {}pt pane Delete's button is at {:?} and Cancel's at {:?} -- the \
                 second of two rapid clicks where Delete was no longer lands on Cancel",
                size.x,
                delete_box,
                cancel_box
            );
        }
    }

    /// **Design 5b's `opacity: 0.72`: a Send whose link is over steps back,
    /// and a live one does not.**
    ///
    /// Read off the PILL's own fill rather than off the words, because that
    /// is the one cell of the row whose colour a test can see -- and because
    /// it is the cell where a half-applied fade would be most obvious: a
    /// receded row with a full-strength pill is a row whose loudest element
    /// is its least important one.
    #[test]
    fn an_ended_row_is_drawn_back_and_a_live_one_is_not() {
        use crate::send::{SendState, SendSummary};
        let sends = vec![
            SendSummary {
                id: "id-live".to_string(),
                name: "Office WiFi".to_string(),
                access_url: "https://send.bitwarden.com/#/a".to_string(),
                deletion_date: "2026-08-17T00:00:00.000Z".to_string(),
                is_file: false,
                max_access_count: None,
                access_count: 0,
                disabled: false,
                expiration_date: String::new(),
                has_password: false,
            },
            SendSummary {
                id: "id-ended".to_string(),
                name: "Atlas Studio".to_string(),
                access_url: "https://send.bitwarden.com/#/b".to_string(),
                deletion_date: "2026-08-17T00:00:00.000Z".to_string(),
                is_file: false,
                max_access_count: None,
                access_count: 0,
                // Revoked, and NOT the picked row: a selection restores a
                // receded row to full strength, so a fixture that picked this
                // one would be measuring the exception.
                disabled: true,
                expiration_date: String::new(),
                has_password: false,
            },
        ];
        // **The control is inside the fixture**, as `one_row`'s is: a test
        // below that finds no faded pill should be telling you the pane did
        // not fade one, never that the fixture was never revoked.
        assert_eq!(
            sends.iter().map(|s| crate::send::send_state(s, &FixedClock(NOW))).collect::<Vec<_>>(),
            vec![SendState::Waiting, SendState::Revoked],
            "control: the two fixtures do not derive to a live row and an ended one"
        );
        let state = pane_state(Some(&Ok(sends)), &FixedClock(NOW), &UTC);
        for size in [roomy_pane_size(), min_pane_size()] {
            let (painted, _) = paint(&state, None, size);
            let live = painted.control_under(WAITING_LABEL).1;
            let ended = painted.control_under(REVOKED_LABEL).1;
            assert_eq!(
                live.a(),
                255,
                "at a {}pt pane the LIVE row's pill is faded, so nothing in the column comes \
                 forward",
                size.x
            );
            assert!(
                ended.a() < 255,
                "at a {}pt pane the ended row's pill is drawn at full strength, so a revoked \
                 link has exactly the weight of one somebody is about to open: alpha {}",
                size.x,
                ended.a()
            );
        }
    }

    /// **The standing notes are a footnote, under the rows.**
    ///
    /// They used to sit between the strip and the first row -- some seventy
    /// points of grey prose at the top of the column, so the first thing the
    /// eye met on this screen was a help note and the rows began a third of
    /// the way down. 5b's list column goes strip, then rows. The sentences
    /// are still here, because both are true and both are things a user has
    /// to be told; they are simply no longer the headline.
    #[test]
    fn the_standing_notes_sit_below_the_rows_rather_than_above_them() {
        let state = rows(3);
        for size in [roomy_pane_size(), min_pane_size()] {
            let (painted, _) = paint(&state, None, size);
            let note = painted
                .rect_of(SCOPE_SUBTEXT)
                .unwrap_or_else(|| panic!("the standing note vanished: {:?}", painted.text));
            let first = painted
                .rect_of("send-number-0")
                .expect("the first row was not painted");
            assert!(
                note.top() > first.bottom(),
                "at a {}pt pane the standing note is still above the first row: note at \
                 {note:?}, first row at {first:?}",
                size.x
            );
            // And it really is at the FOOT of the column rather than merely
            // after the rows: nothing of the list is drawn below it.
            assert!(
                note.bottom() <= size.y + 1.0,
                "the note is drawn off the bottom of the pane: {note:?} in {size:?}"
            );
        }
    }

    /// **Design 5c's caution band, on exactly the Sends that have been
    /// opened.**
    ///
    /// The band is the one part of 5c's history block this client can stand
    /// behind -- it is a reading of `accessCount` and names no time, browser
    /// or city. The half that is easy to lose is the second: a pane that drew
    /// it unconditionally would warn every user about every link, including
    /// the ones nobody has touched, which is how a warning stops being read.
    #[test]
    fn a_send_that_has_been_opened_is_cautioned_and_an_untouched_one_is_not() {
        let opened = one_row(crate::send::SendState::Used);
        let untouched = one_row(crate::send::SendState::Waiting);
        for size in [roomy_pane_size(), min_pane_size()] {
            let (painted, _) = paint(&opened, None, size);
            assert!(
                painted.has(CAUTION_HEADLINE),
                "at a {}pt pane a Send somebody has opened says nothing about it: {:?}",
                size.x,
                painted.text
            );
            let text = painted.rect_of(CAUTION_HEADLINE).expect("just asserted");
            let (band, fill) = painted
                .fill_behind(text)
                .expect("the caution sentence has no band behind it");
            assert_eq!(
                fill,
                theme::CAUTION_WASH,
                "the caution sentence is on something other than the design's amber ground"
            );
            assert!(
                band.right() <= size.x + 1.0 && band.left() >= crate::vault_window::LIST_WIDTH,
                "the caution band is not inside the detail column: {band:?} in {size:?}"
            );

            let (painted, _) = paint(&untouched, None, size);
            assert!(
                !painted.has(CAUTION_HEADLINE),
                "at a {}pt pane a link nobody has opened is warned about anyway: {:?}",
                size.x,
                painted.text
            );
            // The control for that absence: the pane really drew, so what is
            // missing is the band and not the screen.
            assert!(painted.has(LINK_CARD_TITLE), "nothing was painted at all");
        }
    }

    /// **The detail title is elided, not clipped.**
    ///
    /// `elided`'s whole argument, applied to the one run on this screen that
    /// had escaped it: a clipped galley keeps its full width, so a 21px name
    /// at 298pt was laid out at its natural size and simply stopped at the
    /// pane edge -- a complete-looking title that is not the title, with
    /// nothing to say a word had gone.
    #[test]
    fn a_long_detail_title_ends_in_an_ellipsis_inside_the_pane() {
        use crate::send::SendSummary;
        let send = SendSummary {
            id: "id0".to_string(),
            name: "Remote Desktop \u{2014} Bastion \u{2014} contractor access, third quarter"
                .to_string(),
            access_url: "https://send.bitwarden.com/#/x".to_string(),
            deletion_date: "2026-08-17T00:00:00.000Z".to_string(),
            is_file: false,
            max_access_count: None,
            access_count: 0,
            disabled: false,
            expiration_date: String::new(),
            has_password: false,
        };
        let state = pane_state(Some(&Ok(vec![send])), &FixedClock(NOW), &UTC);
        for size in [roomy_pane_size(), min_pane_size()] {
            let (painted, _) = paint(&state, None, size);
            // The DETAIL header's copy of the name. Found by its column
            // rather than by its size, because the list row draws the very
            // same string and the two are told apart by where they are.
            let (text, rect) = painted
                .text_rects
                .iter()
                .filter(|(t, r)| {
                    t.starts_with("Remote Desktop")
                        && r.center().x >= crate::vault_window::LIST_WIDTH
                })
                .max_by(|a, b| a.1.width().partial_cmp(&b.1.width()).expect("finite"))
                .cloned()
                .unwrap_or_else(|| panic!("no title was painted: {:?}", painted.text));
            assert!(
                rect.right() <= size.x + 1.0,
                "at a {}pt pane the title runs past the edge: {text:?} at {rect:?}",
                size.x
            );
            // At the minimum size this name cannot fit at 21px, so what is
            // being asserted there is that the reader is TOLD it was cut. At
            // the roomy size it may well fit, and a test that demanded an
            // ellipsis would be demanding a defect.
            if !text.ends_with("quarter") {
                assert!(
                    text.ends_with('\u{2026}'),
                    "at a {}pt pane the title was cut short with no ellipsis to say so: \
                     {text:?}",
                    size.x
                );
            }
        }
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
    /// `mod backend_tasks`'s text -- the module that chooses between `bw send`
    /// and the built-in client, and holds both implementations.
    ///
    /// [`sealed_module`]'s body with one opener changed, and a separate
    /// function rather than a parameterised one for that function's reason: a
    /// helper taking the opener as an argument is a helper a caller can be
    /// handed the wrong opener for, which would slice the other module and
    /// assert about it under this one's name.
    fn backend_tasks_module() -> String {
        let production = production();
        let opener = concat!("mod backend_", "tasks {\r\n");
        assert_eq!(
            production.matches(opener).count(),
            1,
            "{opener:?} is not in production exactly once -- the module the four Sends \
             backends are chosen in is gone, or there are two of them"
        );
        let start = production.find(opener).expect("counted just above");
        let rest = &production[start + opener.len()..];
        let end = rest
            .find("\r\n}\r\n")
            .expect("`mod backend_tasks` has no closing brace at column zero");
        rest[..end].to_string()
    }

    /// One `impl .. {` block inside [`backend_tasks_module`], by its header line.
    ///
    /// The header is matched whole and must occur exactly once, and the block
    /// ends at the first `}` written at four spaces -- which inside that
    /// module is the impl's own close and nothing else, because every method
    /// and every `match` inside one is indented further.
    fn tasks_impl(header: &str) -> String {
        // A trailing newline, because the module slice stops at its own last
        // `}` -- so without one the LAST impl block in it has no closer to
        // find and this panics on the block it is most often asked for.
        let module = format!("{}\r\n", backend_tasks_module());
        let opener = format!("    {header} {{\r\n");
        assert_eq!(
            module.matches(&opener).count(),
            1,
            "{opener:?} is not in `mod backend_tasks` exactly once"
        );
        let start = module.find(&opener).expect("counted just above");
        let rest = &module[start + opener.len()..];
        let end = rest
            .find("\r\n    }\r\n")
            .expect("that impl block has no closing brace at four spaces");
        rest[..end].to_string()
    }

    /// [`body_of`], but reading a REGION rather than the whole production --
    /// which is what makes it usable on `fn list(`, a name three items in
    /// `mod backend_tasks` share (the trait's declaration and the two impls').
    fn body_in(region: &str, name: &str, indent: &str) -> String {
        // A trailing newline, for [`tasks_impl`]'s reason: a region slice
        // stops at its own last `}`, so the LAST item in one would have no
        // closer to find and this would panic on it.
        let region = &format!("{region}\r\n");
        let opener = format!("fn {name}(");
        let start = region
            .find(&opener)
            .unwrap_or_else(|| panic!("`{name}` is gone from that region"));
        let rest = &region[start + opener.len()..];
        let closer = format!("\r\n{indent}}}\r\n");
        let end = rest.find(&closer).unwrap_or_else(|| {
            panic!("`{name}` has no closing brace at the indentation {indent:?}")
        });
        rest[..end].to_string()
    }

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

    /// **The fetch is four equalities now, and that is the point.**
    ///
    /// This used to be ONE whole-body equality over `real_send_list`, whose
    /// body was a `backend_policy::selected()` branch with a `bw send list`
    /// under it. Three more functions in three other modules carried the same
    /// branch, written out longhand, and six defects shipped in a day from
    /// exactly that duplication. The branch lives once now, in
    /// `backend_tasks::tasks_for`, and the two arms are the two implementations
    /// of `backend_tasks::BackendTasks`.
    ///
    /// So the single equality becomes four, and together they assert strictly
    /// more than it did:
    ///
    ///  1. `real_send_list` is the delegation and nothing else -- no branch of
    ///     its own, nothing hoisted above the call, and the session it was
    ///     handed is the session that goes in.
    ///  2. `tasks_for` is exactly one `selected()` gate over the two
    ///     implementations. This is the only place the choice is made, for
    ///     all four Sends operations.
    ///  3. The CLI implementation is exactly one `bw send list`, in this
    ///     window's job, for the active account, with the session the value
    ///     was constructed with.
    ///  4. The built-in implementation is exactly one call to the REST
    ///     client, and it cannot reach a `bw` because it does not name one.
    ///
    /// Each is a whole-body equality for the reason the one it replaces
    /// records: a needle pins the SPELLING of an identifier and says nothing
    /// about what it is bound to, and one inserted `let session = "";` above
    /// a pinned line was measured to leave every needle word-perfect while
    /// production set `BW_SESSION=""` on every `bw send list`.
    ///
    /// **Comments are blanked first**, so this pins the code and not the
    /// prose around it.
    ///
    /// **It is not the only guard, and deliberately.** An equality still only
    /// says the text is what it was; it cannot say what that text DOES.
    /// [`the_real_fetch_runs_bw_send_list_in_a_job_with_the_session_it_was_given`]
    /// says that, by running the production function itself.
    #[test]
    fn the_delegated_fetch_is_a_real_bw_send_list_for_the_active_account() {
        // 1. The worker: the delegation and nothing else.
        assert_eq!(
            squashed(&sanitized(&body_of(concat!("real_send_", "list"), "    "))),
            squashed(
                "session: &str, ) -> Result<Vec<crate::send::SendSummary>, \
                 crate::send::SendError> { super::backend_tasks::tasks_for(Some(session)).send_list()"
            ),
            "`real_send_list` is no longer exactly the delegation to the chosen backend with \
             the session it was handed. A branch written back into this function is a fifth \
             copy of the decision this refactor exists to hold in one place; a `let session = \
             ..` above the call is the measured defect that retired the needle-shaped pin \
             this equality replaced"
        );

        // 2. The gate: one `selected()` read, and the whole of the choice.
        assert_eq!(
            squashed(&sanitized(&body_in(&backend_tasks_module(), "tasks_for", "    "))),
            squashed(
                "session: Option<&str>) -> Box<dyn BackendTasks + '_> { if \
                 crate::backend_policy::selected() == \
                 crate::backend_policy::VaultBackendChoice::DirectRest { return \
                 Box::new(DirectRestTasks); } Box::new(CliTasks { session })"
            ),
            "`tasks_for` is no longer exactly one `selected()` gate over the two \
             implementations. Either a `bw serve` account can now be sent down the REST arm, \
             or a REST account can reach `bw` -- and this is the ONE place either could \
             happen for any of the four Sends operations"
        );

        // 3. The CLI implementation.
        assert_eq!(
            squashed(&sanitized(&body_in(
                &tasks_impl("impl BackendTasks for CliTasks<'_>"),
                "send_list",
                "        ",
            ))),
            squashed(&format!(
                "&self) -> Result<Vec<crate::send::SendSummary>, crate::send::SendError> {{ \
                 let data_dir = Self::data_{}(); crate::send::cli_send_{}(Self::job(), \
                 data_dir.as_deref(), self.session())",
                "dir", "list",
            )),
            "the CLI Sends fetch is no longer exactly one `bw send list`, in this window's \
             job, for the active account, with the session the value was constructed with"
        );

        // 4. The built-in implementation, which names no `bw` at all.
        assert_eq!(
            squashed(&sanitized(&body_in(
                &tasks_impl("impl BackendTasks for DirectRestTasks"),
                "send_list",
                "        ",
            ))),
            squashed(
                "&self) -> Result<Vec<crate::send::SendSummary>, crate::send::SendError> { \
                 crate::rest::send::list_on_active_account()"
            ),
            "the built-in Sends fetch is no longer exactly one call to the REST client"
        );

        assert_eq!(
            production().matches(concat!("crate::send::cli_send_", "list(")).count(),
            1,
            "`cli_send_list` is called somewhere other than the CLI implementation of \
             `BackendTasks` -- and every other caller is unproven ground for which thread it \
             runs on and which backend the account is on"
        );
    }

    /// **The receive is four equalities**, on
    /// [`the_delegated_fetch_is_a_real_bw_send_list_for_the_active_account`]'s
    /// terms and for its reasons.
    ///
    /// The three properties the single equality this replaces held are all
    /// still held, and each is now held by a smaller, more specific string:
    ///
    ///  1. **A `DirectRest` account never reaches the CLI arm.** It is not a
    ///     `return` inside a branch any more; the two arms are two types, and
    ///     the built-in one does not name `cli_send_receive` at all.
    ///  2. **The `bw serve` arm is textually what it was.** The CLI
    ///     implementation's body is the old CLI half, word for word, less the
    ///     `active_data_dir()` call that moved to `Self::data_dir()`.
    ///  3. **Neither arm is handed anything of the account's.** A receive is
    ///     addressed by a link; the link is the credential. `real_send_receive`
    ///     passes `None` for the session and that is pinned here, so a
    ///     credential brought into either arm changes one of these strings.
    #[test]
    fn the_delegated_receive_reads_the_link_on_the_backend_the_policy_chose() {
        assert_eq!(
            squashed(&sanitized(&body_of(concat!("real_send_", "receive"), "    "))),
            squashed(
                "link: &str) -> RecordImportReport { \
                 super::backend_tasks::tasks_for(None).send_receive(link)"
            ),
            "`real_send_receive` is no longer exactly the delegation to the chosen backend \
             with NO session. Anything at all added, removed or rebound here fails -- \
             including a credential brought in, which a receive has no use for on either \
             backend"
        );

        assert_eq!(
            squashed(&sanitized(&body_in(
                &tasks_impl("impl BackendTasks for CliTasks<'_>"),
                "send_receive",
                "        ",
            ))),
            squashed(&format!(
                "&self, link: &str) -> RecordImportReport {{ let data_dir = Self::data_{}(); \
                 match crate::send::cli_send_{}(Self::job(), data_dir.as_deref(), link, None) \
                 {{ Ok(body) => RecordImportReport::Read(Box::new( \
                 crate::record::payload::read_json(&body), )), Err( error @ \
                 (crate::send::SendError::NoVerifiedCli(_) | \
                 crate::send::SendError::SpawnFailed(_)), ) => \
                 RecordImportReport::Failed(format!( \" \", \
                 crate::vault_window::send_ui::RECEIVE_NEEDS_THE_CLI, error.user_message() \
                 )), Err(error) => RecordImportReport::Failed(error.user_message().to_string()), }}",
                "dir", "receive",
            )),
            "the CLI receive is no longer exactly one `bw send receive`, in this window's \
             job, for the active account, with the link and no session"
        );

        assert_eq!(
            squashed(&sanitized(&body_in(
                &tasks_impl("impl BackendTasks for DirectRestTasks"),
                "send_receive",
                "        ",
            ))),
            squashed(&format!(
                "&self, link: &str) -> RecordImportReport {{ match crate::rest::send::{}(link, \
                 None) {{ Ok(body) => RecordImportReport::Read(Box::new( \
                 crate::record::payload::read_json(&body), )), Err(error) => \
                 RecordImportReport::Failed(error.user_message().to_string()), }}",
                concat!("receive_on_active_", "account"),
            )),
            "the built-in receive is no longer exactly one link read against the REST client"
        );

        // The two arms are reached from nowhere else. A second caller of
        // either is unproven ground for which backend it runs on -- and for
        // the CLI one, for which thread.
        assert_eq!(
            production().matches(concat!("crate::send::cli_send_", "receive(")).count(),
            1,
            "`cli_send_receive` is called somewhere other than the CLI implementation of \
             `BackendTasks`"
        );
        assert_eq!(
            production()
                .matches(concat!("crate::rest::send::receive_on_active_", "account("))
                .count(),
            1,
            "`receive_on_active_account` is called somewhere other than the built-in \
             implementation of `BackendTasks`"
        );
    }

    /// **The switch is three equalities**, on
    /// [`the_delegated_fetch_is_a_real_bw_send_list_for_the_active_account`]'s
    /// terms and for its reasons, plus the census that keeps the REST entry
    /// point to one caller.
    ///
    /// What each one holds:
    ///
    ///  1. **The worker asks `tasks_for` ONCE and then branches on what the
    ///     ROW wanted.** This is the property that makes a seventh operation
    ///     free: `the_backend_is_chosen_in_exactly_one_place` counts
    ///     `tasks_for(` at seven, and a switch that asked again would be an
    ///     eighth -- a second reading of the backend, which is the shape all
    ///     six of this window's original defects had. A branch on
    ///     `backend_policy::selected()` written in here fails this equality
    ///     and that census at once.
    ///  2. **The built-in arm is one call to the REST client**, and it reports
    ///     what was ASKED for rather than what the server used to hold --
    ///     `rest::send::set_disabled` already refuses to return `Ok` unless
    ///     the server echoed the flag back, so `disabled` here is confirmed
    ///     and not assumed.
    ///  3. **The CLI arm is a refusal and nothing else.** It names no `bw`
    ///     entry point, starts no child, and cannot: `bw send` has no verb
    ///     for `disabled`. The equality is what stops that becoming a quiet
    ///     `Ok(())` -- which would tell a `bw serve` user their link is off
    ///     while it stayed live, the exact lie `rest::send::set_disabled`'s
    ///     own doc is written against.
    #[test]
    fn the_switch_is_the_built_in_clients_and_the_cli_refuses_it_in_words() {
        // 1. One backend read, then a branch on the row's own verb.
        assert_eq!(
            squashed(&sanitized(&body_of(concat!("real_send_", "delete"), "    "))),
            squashed(
                "id: &str, name: &str, session: &str, op: SendRowOp, ) -> SendDeleteReport { \
                 let tasks = super::backend_tasks::tasks_for(Some(session)); match op { \
                 SendRowOp::Delete => tasks.send_delete(id, name), SendRowOp::Disable => \
                 tasks.send_set_disabled(id, name, true), SendRowOp::Enable => \
                 tasks.send_set_disabled(id, name, false), }"
            ),
            "the Sends worker is no longer one backend choice followed by a branch on what \
             the row asked for. A second `tasks_for` here is a second reading of the backend \
             between the frame and the thread, and an account switch happens in that gap"
        );

        // 2. The built-in arm.
        assert_eq!(
            squashed(&sanitized(&body_in(
                &tasks_impl("impl BackendTasks for DirectRestTasks"),
                "send_set_disabled",
                "        ",
            ))),
            squashed(&format!(
                "&self, id: &str, name: &str, disabled: bool) -> SendDeleteReport {{ match \
                 crate::rest::send::{}(id, disabled) {{ Ok(()) => \
                 SendDeleteReport::Switched {{ name: name.to_string(), disabled }}, \
                 Err(error) => {{ SendDeleteReport::SwitchFailed {{ name: name.to_string(), \
                 disabled, error }} }} }}",
                concat!("set_disabled_on_active_", "account"),
            )),
            "the built-in switch is no longer exactly one call to the REST client reporting \
             the flag it asked for"
        );

        // 3. The CLI arm: a refusal, with no `bw` in it.
        let cli = sanitized(&tasks_impl("impl BackendTasks for CliTasks<'_>"));
        let body = squashed(&sanitized(&body_in(
            &tasks_impl("impl BackendTasks for CliTasks<'_>"),
            "send_set_disabled",
            "        ",
        )));
        assert_eq!(
            body,
            squashed(
                "&self, _id: &str, name: &str, disabled: bool) -> SendDeleteReport { \
                 SendDeleteReport::SwitchFailed { name: name.to_string(), disabled, error: \
                 crate::send::SendError::Rejected( \" \" .to_string(), ), }"
            ),
            "the CLI switch is no longer exactly a refusal. `bw send` has no verb that sets \
             `disabled`, so anything here that is not a refusal either starts a child that \
             cannot do the job or reports a success that did not happen"
        );
        // It takes the id by `_id` because there is nothing to name it TO --
        // a body that started using it is a body that found something to run.
        assert!(
            body.contains("_id: &str"),
            "the CLI switch has found a use for the Send's id, which means it is doing \
             something with it: {body}"
        );
        assert!(
            !body.contains(concat!("cli_send_", "")) && !body.contains(concat!("Self::", "job")),
            "the CLI switch names a `bw send` entry point or this window's job: {body}"
        );
        // Control: the same impl really does spawn for its other operations,
        // so the absence above is about this method and not about a slice
        // that found nothing.
        assert!(
            cli.contains(concat!("cli_send_", "delete")),
            "control: the `bw serve` implementation no longer spawns anything at all"
        );

        // 4. The REST entry point has exactly one caller. A second is
        // unproven ground for which thread it runs on and which backend the
        // account is on.
        assert_eq!(
            production()
                .matches(concat!("crate::rest::send::set_disabled_on_active_", "account("))
                .count(),
            1,
            "`set_disabled_on_active_account` is called somewhere other than the built-in \
             implementation of `BackendTasks`"
        );
    }

    /// **`RECEIVE_NEEDS_THE_CLI` survives for the CLI and is unreachable for
    /// the built-in client.**
    ///
    /// The sentence itself does not move -- a user on `bw serve` is not
    /// affected by this at all, and
    /// [`the_receive_sentence_says_what_is_missing_without_condemning_what_is_not`]
    /// still holds its wording. What is asserted here is where it can be
    /// produced from.
    ///
    /// **The cut is a type boundary now, not a `split_once` on the first
    /// statement of the second arm.** The two arms were two halves of one
    /// function body and had to be separated by finding a line inside one of
    /// them; they are two `impl` blocks now, so the slice cannot land in the
    /// wrong place. The control -- that the sentence IS still produced on the
    /// CLI side -- is kept exactly as it was, because a slice that found
    /// nothing would otherwise report both halves as satisfied.
    #[test]
    fn the_cli_sentence_survives_for_the_cli_and_is_gone_for_the_built_in_client() {
        let cli = sanitized(&tasks_impl("impl BackendTasks for CliTasks<'_>"));
        let direct = sanitized(&tasks_impl("impl BackendTasks for DirectRestTasks"));

        // **The control.** The sentence is produced in the CLI half, so the
        // absence below is a statement about live code and not about a needle
        // that matches nothing anywhere.
        assert!(
            cli.contains("RECEIVE_NEEDS_THE_CLI"),
            "control: the `bw serve` implementation no longer names RECEIVE_NEEDS_THE_CLI at \
             all, so the assertion below is searching text it could never have found"
        );
        assert!(
            !direct.contains("RECEIVE_NEEDS_THE_CLI"),
            "the built-in client can reach the sentence that tells a user to install \
             Bitwarden's command-line tool. It does not need one: it reads the link itself"
        );
        // And the built-in implementation reaches no `bw` at all -- the
        // sentence is the symptom, the spawn is the thing.
        assert!(
            !direct.contains(concat!("cli_send_", "")),
            "the built-in implementation of `BackendTasks` names a `bw send` entry point"
        );
        assert!(
            cli.contains(concat!("cli_send_", "receive")),
            "control: the `bw serve` implementation no longer spawns a `bw send receive`"
        );
    }

    /// **The backend is chosen in exactly one place, and these are all the
    /// places that can ask.**
    ///
    /// `backend_tasks::tasks_for` is the only way to obtain a `BackendTasks`,
    /// so this census over the whole crate's production is what makes the
    /// "one swap point" claim checkable rather than asserted. Seven mentions
    /// in `mod.rs`: the definition, and the six workers -- the Sends list,
    /// create, revoke and receive, the vault export, and the Sync button.
    /// Nowhere else at all, and in particular not in the frame closure, where
    /// four of the six would block the eframe thread on a `bw` child for up
    /// to sixty seconds.
    ///
    /// An EIGHTH mention is either a seventh caller, which is unproven ground
    /// for which thread it runs on and which backend it reaches, or a second
    /// definition. A seventh OPERATION is meant to be added to the trait,
    /// where both implementations must answer it or the crate does not
    /// compile -- not by asking here again.
    #[test]
    fn the_backend_is_chosen_in_exactly_one_place() {
        let files = crate_sources();
        assert!(
            files.len() > 30,
            "control: the crate walk found only {} source files, which is not this crate",
            files.len()
        );
        let needle = concat!("tasks_", "for(");
        let seen: Vec<(&str, usize)> = files
            .iter()
            .map(|(path, text)| {
                (path.as_str(), sanitized(&production_region(text)).matches(needle).count())
            })
            .filter(|(_, n)| *n > 0)
            .collect();
        assert_eq!(
            seen,
            vec![("vault_window/mod.rs", 7)],
            "{needle:?} is written in the crate's production at {seen:?}, not the one \
             definition plus the six workers that are the whole of how a `BackendTasks` can \
             be obtained. A seventh caller is one of this window's backend-sensitive \
             actions on unproven ground -- which thread it runs on, and which backend it \
             reaches"
        );

        // **And the window READS the choice exactly once**, which is the
        // half the census above cannot see. `tasks_for` being the only route
        // to a `BackendTasks` says nothing about a SEVENTH action written
        // next month with its own `if selected() == DirectRest` -- which is
        // exactly the shape all six of these had, and exactly how six defects
        // shipped in a day. So the read itself is counted, over every file
        // this window is made of: once, in `tasks_for`, and nowhere else.
        //
        // Deliberately scoped to `vault_window/`. `main.rs` legitimately
        // reads the choice several times -- it is the process that PUBLISHES
        // it -- and folding those into this number would make it a total that
        // drifts for reasons this test has no opinion about.
        let read = concat!("backend_policy::selected", "()");
        let reads: Vec<(&str, usize)> = files
            .iter()
            .filter(|(path, _)| path.starts_with("vault_window/"))
            .map(|(path, text)| {
                (path.as_str(), sanitized(&production_region(text)).matches(read).count())
            })
            .filter(|(_, n)| *n > 0)
            .collect();
        assert_eq!(
            reads,
            vec![("vault_window/mod.rs", 1)],
            "this window reads {read:?} at {reads:?}, not once in `tasks_for`. A second read \
             is a seventh hand-branch -- the shape all six of these actions had, and the one \
             that put an `os error 2` on every Sends screen, export and Sync of an account \
             with no `bw.exe` on it"
        );

        // And the module exports nothing but the trait and the gate. A
        // `pub(super) fn blocking_send_list()` added inside it would keep the
        // count above unchanged and hand the frame closure a `bw` child back.
        let module = sanitized(&backend_tasks_module());
        let exports: Vec<String> = module
            .split_whitespace()
            .filter(|t| t.starts_with("pub"))
            .map(str::to_string)
            .collect();
        assert_eq!(
            exports,
            vec!["pub(super)".to_string(), "pub(super)".to_string()],
            "`mod backend_tasks` exports {exports:?}, not exactly the trait and the gate. \
             Everything else in that module is a blocking `bw` child or the value that \
             holds the vault session, and neither is anybody else's to name"
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
             std::thread::spawn(move || {{ let _ = tx.send(sync_the_selected_{}\
             (&session_token)); }});",
            concat!("Zeroizing::", "new"),
            "backend",
        ));
        let actual = squashed(&sanitized(&body_of(concat!("spawn_vault_", "sync"), "")));
        assert_eq!(
            actual, expected,
            "`spawn_vault_sync` is no longer exactly `wrap the session, then run the \
             SELECTED backend's sync on a thread with it`. A wrap that moved to a CALLER leaves this function \
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
        // on it.
        //
        // **It is four in `main.rs` since the `bw serve` sign-in moved into
        // the `--ui` child**, and the fourth is that move's whole mechanism.
        // A child that signs in stores its token and rings the daemon; the
        // daemon answers by starting the backend, and it does that through
        // this same spawner rather than a second one, because doing it
        // inline would block the loop that answers the tray for up to ~30 s
        // -- the exact freeze the child was moved out to remove. The count
        // moved with a reason; it is still exact, and a fifth still fails.
        for (needle, expected) in [
            (
                concat!("spawn_", "sync"),
                vec![("main.rs", 4usize), ("vault_window/mod.rs", 3)],
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
                // **Three in `main.rs`.** Two of them are the daemon's, and
                // it went four -> three -> two as the daemon stopped drawing:
                // first the startup door, which built a vault frame in this
                // process and named the type to do it and now asks
                // `UiWindows` for a process instead, and then
                // `RealVaultOps::open_window`, whose in-daemon host came out
                // whole -- see `no_route_left_in_the_daemon_draws_a_vault_
                // window`. What is left on this side is the startup branch's
                // own frame and the rebuild the in-window lock runs.
                //
                // **The third is the `--ui` child's own, and it is on the
                // other side of the boundary this pin is drawn around.** A
                // direct-REST child that opens on a sign-in card hosts the
                // vault stage itself, through `app_window::run`, and builds
                // the frame for it -- so it names the type exactly as the
                // daemon's remaining three do. It is a `production()` like the
                // others, which is the only constructor there is; nothing here
                // reads `env.sync`, and that read is still pinned to one, one
                // file down. So this is a fourth place the pointer EXISTS and
                // not a fourth place it can be reached from, which is the
                // property this count stands for.
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
                 no file but `mod.rs` and `main.rs`'s three constructions may name, so a \
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
        // **`cli_send_list` is sealed in a DIFFERENT block from the other
        // two, and that split is the structural change this pin records.**
        // The blocking call is the CLI implementation of
        // `backend_tasks::BackendTasks` -- the one module in which this window
        // chooses between `bw send` and the built-in client for all four
        // Sends operations -- while `real_send_list` and
        // `spawn_send_list_with`, which are the thread boundary and nothing
        // else, stayed in `mod send_fetch_thread`. Every needle is still
        // confined to exactly ONE named module, which is the whole of what
        // this test ever asserted; the fourth column says which.
        let tasks = sanitized(&backend_tasks_module());
        assert!(
            tasks.len() > 200,
            "control: the `mod backend_tasks` slice is only {} bytes, which is not a module's \
             worth",
            tasks.len()
        );
        for (needle, defined_in_send_rs, is_seal_needle, in_backend_tasks) in [
            (concat!("cli_send_", "list"), 1usize, true, true),
            (concat!("real_send_", "list"), 0, true, false),
            (concat!("spawn_send_list_", "with"), 0, true, false),
            (concat!("list_", "sends"), 2, false, false),
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
            (concat!("CliSendRunner", "::with_session"), 3, false, false),
            // **Moved by one AGAIN when `cli_send_receive` landed**, and this
            // time on the OTHER constructor, which is the whole of what makes
            // the move worth recording rather than a number that drifted: a
            // receive is anonymous -- the link is the credential -- so
            // `cli_send_receive` builds `CliSendRunner::new(job, data_dir)`
            // and hands the child no `BW_SESSION` at all. Seven is now
            // `pub struct CliSendRunner`, `impl<'a> CliSendRunner<'a>`,
            // `impl SendRunner for CliSendRunner<'_>` and the FOUR entry
            // points, and no fifth.
            (concat!("CliSendRunner", ""), 7, false, false),
            // **One, and it was zero until the receive landed.** This row used
            // to be the whole of what refused the measured `warm_cache`
            // survivor -- a blocking `bw send list` built from the constructor
            // nothing in production used. It is not zero any more, so it no
            // longer refuses that shape by itself; what refuses it now is that
            // this row and the `CliSendRunner` row above must BOTH hold, and
            // the survivor spelled one more of each than the file accounts
            // for. The single production use is `cli_send_receive`'s, and a
            // second one is a second sessionless blocking child.
            (concat!("CliSendRunner", "::new"), 1, false, false),
            // The revoke's two, on the same terms as `list_sends` and
            // `list_invocation` above. `delete_send` is its definition plus
            // the one call in `cli_send_delete`; `delete_invocation` is its
            // definition plus the one use in `delete_send`. A third mention
            // of either is a second blocking `bw send delete` written inside
            // the privacy boundary, where the private runner and the private
            // `delete_invocation` are both still in scope.
            (concat!("delete_", "send"), 2, false, false),
            (concat!("delete_", "invocation"), 2, false, false),
            // **The bypass, counted.** `runner.run(&list_invocation(..))` is
            // a complete blocking `bw send list` that spells neither
            // `list_sends` nor `cli_send_list`; the author documented it for
            // the in-`send.rs` case and the measured `send/inner.rs` survivor
            // used exactly it, one file over. `list_invocation` is private to
            // `crate::send`, so this row and the closure above are together
            // the whole of what refuses a second use of it.
            (concat!("list_", "invocation"), 2, false, false),
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
            (concat!("create_", "send"), 2, false, false),
            (concat!("plan_to_", "invocation"), 2, false, false),
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
            (concat!("cli_send_", "receive"), 1, false, false),
            (concat!("receive_", "send"), 2, false, false),
            (concat!("receive_", "invocation"), 2, false, false),
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
            let (home, inside) = if in_backend_tasks {
                ("mod backend_tasks", tasks.matches(needle).count())
            } else {
                ("mod send_fetch_thread", block.matches(needle).count())
            };
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
                 inside `{home}`. Every mention outside that block -- in \
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
        // **Re-pinned by the nine-choice lifetime pass, and a widening of
        // nothing this wall guards.** The lifetime stopped being a `u32` of
        // hours -- a duration cannot say "3 months" without lying about how
        // long a month is, cannot say "the 14th of March" at all, and cannot
        // say "never" except as a sentinel wearing a duration's clothes -- so
        // it is an enum of the four shapes that answer is given in, with the
        // choices, the default, the wire sentinel, the picker's bound and the
        // two special labels beside it.
        //
        // Not one of these reaches a runner, an invocation or a child. They
        // are read by `validate_access`, by `deletion_date`'s arithmetic and
        // by the control that draws them; the enum's two `impl` methods below
        // are civil-date arithmetic and nothing else.
        "send.rs: pub enum SendLifetime {",
        "send.rs: pub const LIFETIME_CHOICES: [Option<SendLifetime>; 9] = [",
        "send.rs: pub const PICK_A_DATE_LABEL: &str = \" \";",
        "send.rs: pub const DEFAULT_LIFETIME: SendLifetime = SendLifetime::Hours(24 * 7);",
        "send.rs: pub const NEVER_DELETION_DATE: &str = \" \";",
        "send.rs: pub const MAX_PICKED_MONTHS: u32 = 12;",
        "send.rs: pub struct SendPlan {",
        "send.rs: pub name: String,",
        "send.rs: pub text: Zeroizing<String>,",
        "send.rs: pub hidden: bool,",
        "send.rs: pub lifetime: SendLifetime,",
        "send.rs: pub password: Option<Zeroizing<String>>,",
        "send.rs: pub max_access_count: Option<u32>,",
        "send.rs: pub fn validate_plan(plan: &SendPlan, now: &dyn SendClock) -> Option<&'static str> {",
        // **Added by the design §5a Access block, deliberately, and it is the
        // second and last item that work adds to this wall.** It is
        // `validate_plan`'s own last three rules, split out so that
        // `record_ui`'s composer -- which has an Access block and no name or
        // body yet, because the record is re-resolved by id at submit time --
        // can run exactly the rules that apply to it. The two alternatives
        // were both worse and are written out on the function: restate the
        // three sentences in `record_ui`, or hand `validate_plan` an invented
        // name and body, which meant cloning a share password out of a
        // `Zeroizing` buffer on every frame that validated.
        //
        // It is not a door in the sense this list guards. It takes three
        // scalars and a `&str`, returns a `&'static str`, reaches no runner,
        // no invocation and no child, and `validate_plan` -- already on this
        // list, one line up -- is now written in terms of it.
        "send.rs: pub fn validate_access(",
        "send.rs: pub trait SendClock {",
        "send.rs: pub struct FixedClock(pub i64);",
        "send.rs: pub struct SystemClock;",
        // Re-pinned by `2026-08-30-sends-without-the-cli`, and a
        // widening of exactly nothing that this wall guards: it computes
        // a date string and reaches no runner, no invocation and no
        // child. It went from private to `pub(crate)` so that
        // `crate::rest::send` stamps the SAME instant in the SAME format
        // as the CLI path, rather than growing a second copy of the
        // arithmetic that the two backends could then disagree over.
        "send.rs: pub(crate) fn deletion_date(lifetime: SendLifetime, now: &dyn SendClock) -> String {",
        // The enum's own two methods, and the window the calendar offers.
        // `on_local_day` is the single place a picked `(year, month, day)`
        // becomes an instant; `local_day` is the inverse, for a calendar that
        // has to open on the answer already in force; `pickable_window` is the
        // bound `validate_access` enforces, stated once so the control and the
        // validator cannot disagree. All three are integer arithmetic over a
        // clock and an offset that are both injected.
        "send.rs: pub fn on_local_day(year: i64, month: u32, day: u32, zone: &dyn LocalOffset) -> Self {",
        "send.rs: pub fn local_day(self, now: &dyn SendClock, zone: &dyn LocalOffset) -> Option<(i64, u32, u32)> {",
        "send.rs: pub fn pickable_window(",
        // **Moved here from `send_ui`, deliberately, and it is the only item
        // the §5a Access block adds to this wall.** It was the Sends screen's
        // own `pub fn lifetime_label(days: u8)` while `expiry_wording`, four
        // lines below, built the same phrase out of its own `format!` -- two
        // spellings of one duration, agreeing by coincidence. It is now one
        // function that both screens' cells and that sentence all read.
        //
        // It is a door in the sense this list counts and in no other: it
        // takes an enum and an offset and returns a `String`, reaches no runner, no
        // invocation and no child, and there is nothing behind it but a
        // `format!`.
        "send.rs: pub fn lifetime_label(lifetime: SendLifetime, zone: &dyn LocalOffset) -> String {",
        // The one word `Never` is named by, beside the function that names
        // it. A `&str` constant, read by the label and by nothing else.
        "send.rs: pub const NEVER_LABEL: &str = \" \";",
        // `zone` was added beside `now` when the sentence stopped naming the
        // UTC day and started naming the user's own. Both are injected, and
        // that is the point of both: nothing in `send.rs` reads the machine's
        // clock or the machine's timezone for itself.
        "send.rs: pub fn expiry_wording(lifetime: SendLifetime, now: &dyn SendClock, zone: &dyn LocalOffset) -> String {",
        // And the one sentence that has no date in it, for the same reason:
        // a `&str` beside the function that returns it.
        "send.rs: pub const NEVER_WORDING: &str =",
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
        // **The five fields and two functions the §5b/§5c states pass adds,
        // deliberately, and it is the whole of what that work adds to this
        // wall.**
        //
        // The five fields are the server's own answer, which this client
        // already received on every `bw send list` and every `GET /api/sends`
        // and then dropped on the floor. They are read by
        // `send_state` below them and by the row's subtitle, and not one of
        // them reaches a runner, an invocation or a child -- they are two
        // integers, two flags and a date string.
        //
        // **`has_password` is a `bool` where the wire carries a hash**, and
        // that is a narrowing rather than a widening: the server's `password`
        // field is a credential verifier, so the choice was between a new
        // `String` on a type whose `Debug` is hand-written precisely because
        // it already carries one secret, and the single bit the screen
        // actually draws. It is the bit.
        "send.rs: pub max_access_count: Option<u32>,",
        "send.rs: pub access_count: u32,",
        "send.rs: pub disabled: bool,",
        "send.rs: pub expiration_date: String,",
        "send.rs: pub has_password: bool,",
        // The derivation and its four answers. A pure function of a summary
        // and a clock, returning a fieldless enum: no runner, no invocation,
        // no child, and nothing behind the door but four comparisons. It is
        // `pub` rather than `pub(crate)` for the reason `lifetime_label`
        // above it is -- the screen that draws the pill is a different module
        // -- and it is in THIS module rather than in `send_ui` so that the
        // rule lives beside the fields it reads.
        "send.rs: pub enum SendState {",
        "send.rs: pub fn send_state(send: &SendSummary, now: &dyn SendClock) -> SendState {",
        // **Moved here from `send_ui`, and the move is the point.** It was
        // the Sends screen's own date parser, and `send_state` needs the same
        // bytes read the same way against the same clock. Two parsers for one
        // wire format agree only by coincidence, which is the exact reason
        // `lifetime_label` is on this list a few rows up. `send_ui`
        // re-exports this one under its old name, so `record_ui` -- the only
        // caller outside that file -- is untouched.
        //
        // Integer arithmetic over a `&str` and one call to
        // `crate::local_time::days_from_civil`, which this file already made
        // for the other direction.
        "send.rs: pub fn parse_iso_utc_millis(text: &str) -> Option<i64> {",
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
        // `Some(..)`, and the stale list is drawn with no refetch ever -- the
        // very defect the policy exists for.
        //
        // **The gate is `wants_fetch_now()` and no longer takes the screen**,
        // and the ordering this test is about is unchanged by that. Design 5d
        // gave the list two readers on the VAULT screen -- the `shared` pill
        // on every row and the read pane's `SHARING` card -- so a fetch gated
        // on the Sends screen is a pill that only appears after a detour
        // through a screen the user had no reason to visit.
        //
        // What that changes, and it is worth being plain about: leaving the
        // Sends screen invalidates, and the very next frame is on the vault
        // screen and refetches. That is one extra `bw send list` per exit --
        // and it is the right one, because the user may have just revoked
        // something, and the pills behind them have to stop claiming it can
        // be opened.
        //
        // The narrowing this comment used to record still stands where it
        // was made: `note_screen(on_sends)` above takes `on_sends` and not
        // `show_sends`, so `Shared with me` -- which reads a local file --
        // still does not invalidate a list it has no question about.
        let gate = concat!("send_fetch.wants_", "fetch_now()");
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

    // -----------------------------------------------------------------
    // Design 5d -- the link between a record and a live Send
    // -----------------------------------------------------------------

    use super::{live_send_in, live_send_named, shared_names};
    use crate::send::{SendError, SendSummary};

    /// **The pill means "someone can open this right now", and nothing else.**
    ///
    /// 5d's own sentence, and the whole specification of `live_send_named`:
    /// "The pill disappears on its own when the Send expires or is revoked --
    /// it always means someone can open this right now, never 'was shared
    /// once'." So every state but `Waiting` answers `None`, and this walks all
    /// four rather than asserting the one -- a match arm that let `Revoked`
    /// through would be a pill on a link the owner has already turned off.
    #[test]
    fn only_a_live_send_lights_a_record() {
        let now = crate::send::FixedClock(1_700_000_000_000);
        let live = SendSummary {
            id: "s-1".into(),
            name: "SAP Production".into(),
            access_url: "https://example.test/#k".into(),
            deletion_date: "2099-01-01T00:00:00.000Z".into(),
            is_file: false,
            max_access_count: None,
            access_count: 0,
            disabled: false,
            expiration_date: String::new(),
            has_password: false,
        };
        assert_eq!(
            live_send_named(std::slice::from_ref(&live), "SAP Production", &now).map(|s| &s.id),
            Some(&"s-1".to_string()),
            "a live Send does not light its record"
        );

        // Revoked, spent and expired: three ways to be dead, none of them a
        // pill.
        let revoked = SendSummary { disabled: true, ..live.clone() };
        let spent = SendSummary { max_access_count: Some(1), access_count: 1, ..live.clone() };
        let expired = SendSummary {
            deletion_date: "2000-01-01T00:00:00.000Z".into(),
            ..live.clone()
        };
        for (what, send) in [("revoked", revoked), ("spent", spent), ("expired", expired)] {
            assert!(
                live_send_named(std::slice::from_ref(&send), "SAP Production", &now).is_none(),
                "a {what} Send still lights its record, so the pill says a dead link can be \
                 opened"
            );
        }

        // A different record's Send is not this record's.
        assert!(live_send_named(std::slice::from_ref(&live), "Ledgerline", &now).is_none());
    }

    /// **A fetch that has not happened, and one that failed, light nothing.**
    ///
    /// The pill is a positive claim -- it says something can be opened -- so
    /// "we do not know" has to look like no pill. This is the same rule
    /// `badge_count` makes for the sidebar's number, asserted here for the
    /// three-state answer the pill reads.
    #[test]
    fn an_unknown_sends_list_lights_nothing() {
        let now = crate::send::FixedClock(1_700_000_000_000);
        assert!(live_send_in(None, "SAP Production", &now).is_none(), "not asked yet");
        let failed: Result<Vec<SendSummary>, SendError> = Err(SendError::Offline);
        assert!(
            live_send_in(Some(&failed), "SAP Production", &now).is_none(),
            "a failed fetch claims a record is shared"
        );
        assert!(shared_names(Some(&failed), &now).is_empty());
        assert!(shared_names(None, &now).is_empty());
    }

    /// The set the item list reads and the lookup the read pane reads agree.
    ///
    /// Two functions answering one question is two chances to disagree, and
    /// the disagreement would be visible: a pill on a row whose detail pane
    /// shows no card.
    #[test]
    fn the_pill_and_the_card_agree_about_what_is_shared() {
        let now = crate::send::FixedClock(1_700_000_000_000);
        let send = |name: &str, disabled: bool| SendSummary {
            id: format!("s-{name}"),
            name: name.into(),
            access_url: "https://example.test/#k".into(),
            deletion_date: "2099-01-01T00:00:00.000Z".into(),
            is_file: false,
            max_access_count: None,
            access_count: 0,
            disabled,
            expiration_date: String::new(),
            has_password: false,
        };
        let sends = vec![send("SAP Production", false), send("Ledgerline", true)];
        let fetched: Result<Vec<SendSummary>, SendError> = Ok(sends);
        let names = shared_names(Some(&fetched), &now);
        for name in ["SAP Production", "Ledgerline", "Nothing"] {
            assert_eq!(
                names.contains(name),
                live_send_in(Some(&fetched), name, &now).is_some(),
                "the row's pill and the pane's card disagree about {name:?}"
            );
        }
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
    /// **The gate is `!show_sends && !on_health && !on_sequence`**, because
    /// the item-list column now has two more screens that take it over: the
    /// Password health report (`vault_window::password_health`) and design
    /// 4a's sequence builder (`vault_window::sequence_builder`). Nothing here
    /// was weakened to let either in -- neither draws an item list, so the
    /// `Panel::left("vault-item-list")` count below still says "the item list
    /// is drawn exactly once" and the block slice still says "and that once is
    /// under this gate". `password_health` carries the mirror of this test for
    /// its own pane.
    ///
    /// The builder is a `DetailMode` rather than a screen flag, so its term is
    /// read off `mode` a few lines above the gate; the needle is still the
    /// whole line, which is what makes a term quietly dropped from it a
    /// failure here.
    #[test]
    fn the_item_list_is_drawn_only_inside_the_not_sends_gate() {
        let production = production();
        let gate =
            concat!("        if !show_", "sends && !on_health && !on_sequence {\r\n");
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

        // Squashed, because this call is wrapped over a dozen lines. The
        // needle is still the WHOLE call -- the argument list included -- so
        // a second draw site, or a site handed a delete state, a view or a
        // selection that is not the window's own, fails here.
        //
        // **The view and the selection joined it with the detail pane**, and
        // pinning them is the point rather than an overhead: the
        // `if on_received` written inline is what makes `Shared with me` and
        // the account's own Sends one pane, and a `let` for it above the
        // panel would be a name upstream of the pane -- the exact shape
        // `the_applier_takes_the_panel_with_no_binding_between` exists to
        // refuse. A comment cannot keep it inline; this can.
        let pane = squashed(concat!(
            "send_ui::draw_send_", "pane( ui, state, notice_message.as_deref(), \
             send_delete.view(), if on_received { send_ui::SendView::Received(&received) } \
             else { send_ui::SendView::Mine(send_scope) }, &mut selected_send, \
             &mut send_create.composer, send_create.in_flight, &crate::send::SystemClock, \
             &crate::local_time::SystemZone, )"
        ));
        assert_eq!(
            squashed(&production).matches(pane.as_str()).count(),
            1,
            "{pane:?} is not in production exactly once -- the Sends pane is drawn from more \
             than one place, from none, or with a delete state that is not the window's own"
        );
    }

    /// **A receive is recorded in exactly one place, and that place is AFTER
    /// the item exists.**
    ///
    /// Design 5b's `Shared with me` counts imports, so the count is only
    /// worth anything if the thing counted really happened. A record written
    /// when the LINK was fetched would claim an import that the passphrase
    /// step, the collision answer or `create_item` itself can still refuse,
    /// and the row would count Sends this vault never received.
    ///
    /// Pinned three ways, because each alone is weak. The record is
    /// constructed once; it is constructed inside the block that runs after a
    /// successful `create_item`, which is sliced out by the line that pushes
    /// the created item into the vault; and the whole file mentions the
    /// history's writer once, so a second append somewhere else is refused
    /// rather than merely unobserved.
    #[test]
    fn a_receive_is_recorded_once_and_only_after_the_item_exists() {
        let production = production();
        let record = concat!("crate::receive_history::Received", "Record {");
        assert_eq!(
            production.matches(record).count(),
            1,
            "{record:?} is built {} times in production, not once -- a second construction is \
             a second definition of what `Shared with me` counts",
            production.matches(record).count()
        );
        let append = concat!("crate::receive_history::", "append(");
        assert_eq!(
            production.matches(append).count(),
            1,
            "{append:?} is called {} times in production, not once",
            production.matches(append).count()
        );

        // The construction is BELOW the create's own success line and ABOVE
        // the push that ends the arm, which is what puts it inside the
        // branch a failed create never reaches.
        let pushed = concat!("items.push(", "created);");
        let created_at = production.find(record).expect("counted above");
        let push_at = production[created_at..]
            .find(pushed)
            .map(|at| created_at + at)
            .expect("the import arm no longer pushes the created item after recording");
        let earlier_failure = concat!("state.failure = Some(item_write_", "failure_message(");
        assert!(
            production[created_at..push_at].find(earlier_failure).is_none(),
            "the receive record is built on a path that can still report a write failure, so \
             `Shared with me` counts imports that did not happen"
        );

        // And nothing else in the window reads or writes the history file.
        let module = concat!("crate::receive_", "history::");
        let mentions = production.matches(module).count();
        assert_eq!(
            mentions, 4,
            "`receive_history` is named {mentions} times in production, not the four this \
             window has: the path, the load, the record and the append. A fifth is a second \
             route to a file whose whole point is that it holds no link"
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
        build_frame, SendListSender, VaultLoadFailure, VaultLoadRequest,
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
    /// **The read detail pane's control**: the copy chord drawn beside its
    /// username row, painted by `detail::draw_detail_read` and by nothing
    /// else in this window -- not the item list, not the sidebar, not the
    /// titlebar, and not the edit form, which has no copy affordances at
    /// all. See [`the_loaded_vault_returns_promptly`].
    ///
    /// **It used to be `LOGIN CREDENTIALS`**, the card's heading, and that
    /// stopped being exclusive the day the EDIT form's cards took §8a's
    /// `text-transform: uppercase` and started drawing the same words. The
    /// tests here are about WHICH PANE is on screen, so a control both panes
    /// paint answers nothing -- which is what
    /// [`the_item_editor_returns_promptly`] caught.
    ///
    /// Not the sidebar's `CTRL+K` and not the composer's: this is `CTRL+U`,
    /// and `detail::copy_shortcut_label` is the only thing in this window
    /// that produces it.
    fn detail_pane_only() -> &'static str {
        super::super::detail::copy_shortcut_chord(
            super::super::detail::CopyShortcut::Username,
        )
    }
    /// **The item list's control**: its search field's hint, which
    /// `item_list::search_hint` produces and `draw_item_list` is the sole
    /// caller of. The count is the fixture's own two items, so a list that
    /// drew its chrome over an empty vault does not satisfy it either.
    const ITEM_LIST_ONLY: &str = "Search 2 items";
    /// **The `DetailMode::Edit` arm's control**: the footer's own Save, which
    /// `draw_detail_edit` is the only thing in this crate that draws.
    ///
    /// It was `detail_edit::form_title`'s `Edit login`, which the form no
    /// longer says: the owner asked for that subtitle to read exactly what the
    /// READ pane's does -- "should be just regular same as on details page
    /// label along with folder" -- so the one string this test used to tell
    /// the two panes apart became a string they share. `Save changes` is the
    /// better sentinel anyway: it is the control that makes this an editor,
    /// and the read pane has no button by that name.
    const EDITOR_EDIT_TITLE: &str = "Save changes";
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
    // **Read from the page, not transcribed from it.** This was a copy of
    // the General subtitle and it went stale the day that subtitle gained
    // "the shortcut it answers" -- leaving a permanently red test that was
    // about a sentence rather than about the modal it exists to detect.
    // Sourced from `Section::subtitle`, the drift cannot happen again.
    fn prefs_modal_only() -> &'static str {
        crate::prefs_ui::Section::General.subtitle()
    }
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
                // Untouched, so the harness row is `Waiting` under any clock.
                max_access_count: None,
                access_count: 0,
                disabled: false,
                expiration_date: String::new(),
                has_password: false,
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
            BwStatusDetails {
                status: BwStatus::Unlocked,
                user_email: Some("harness@example.invalid".to_string()),
                // `None`, so no favicon is fetched for any host.
                server_url: None,
            },
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
    ///  * [`detail_pane_only`], the LOGIN CREDENTIALS card's heading -- the
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
            detail_pane_only(),
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
    ///  * [`detail_pane_only`] is NOT. The editor REPLACES the read pane; a
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
            !outcome.painted(detail_pane_only()),
            "the edit form is up and the read pane's {:?} control is still \
             painted, so the pane was not replaced -- the click did not take. What was \
             painted: {:?}",
            detail_pane_only(),
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
    ///  * [`prefs_modal_only()`] is on screen -- the General section's
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
            prefs_modal_only(),
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
            "the Sends screen is up and the fetch answered, but the Send it answered with is not \
             on screen -- `draw_send_pane`'s `Ok` arm, which is the rows a user reads, was not \
             executed",
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

//! The vault window's **Sent** screen: the Sends this account has published.
//!
//! Steps 3, 4 and 5 of five, in that order, and the order was the point. A
//! Send is a public URL: the only outbound publishing action in this app. The
//! design built list, then delete, then create -- **revocation before
//! publication**, so that at no commit could this app make a link it could not
//! then show and take down.
//!
//! The screen lists every Send, copies any Send's link, extends one, and
//! revokes one in two steps. **It makes none.** A Send is made from a record
//! -- `record_ui::draw_export_form`, behind the envelope on a vault item --
//! and the text composer this screen used to carry went with its `New Send`
//! button; see [`scope_subtext()`], which says where Sends come from.
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
//! and this window has no upload path -- which is why a Send made from a
//! record is always a text Send, and nothing offers a type switch that would
//! refuse). It can list and revoke one. So a file Send made in the web vault appears here with a
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
//! `Send a record` and whose tick list is §5a's field-level opt-in. What this
//! file does keep for it is §5a's ACCESS block, [`draw_access_block`], and
//! the recipient row: they are drawn by that card, and live here because the
//! lifetime, the view cap and the share password are questions about a Send.
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
use eframe::egui::{self, CornerRadius};

mod shared_page;
pub mod words;

/// The Shared page's own title. The rail row now says the same word (the
/// design has one name for the screen), so it no longer proves the PANE drew.
pub fn shared_title() -> &'static str {
    shared_page::title()
}

/// The Shared page's sub-line: a sentence only the pane paints, which the
/// window's reachability tests look for to know the PANE drew and not just the
/// row that leads to it.
pub fn shared_sub() -> &'static str {
    shared_page::sub()
}

/// The one line of subtext under the heading. **It is what makes the excluded
/// scope honest rather than hidden**: this screen shows every Send, including
/// the kinds this app will never be able to make, and the user is told where
/// the rest of the feature lives instead of discovering its absence.
///
/// **A Send is made from a record now**, and this line says where: the
/// owner, "for simplicity let's just send from the record itself".
pub fn scope_subtext() -> &'static str {
    crate::i18n::t("sends.scope_subtext")
}

/// [`scope_subtext()`] once person shares are in the list: design 6c's `.lnote`,
/// word for word.
fn shares_subtext() -> &'static str {
    crate::i18n::t("sends.shares_subtext")
}

/// The list's sort, drawn where design 5b draws it: the server lists Sends
/// newest first, and so does this column.
pub fn newest_label() -> &'static str {
    crate::i18n::t("sends.newest_label")
}

/// The tag drawn on a Send this app could not have created.
pub fn file_tag() -> &'static str {
    crate::i18n::t("sends.file_tag")
}

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
pub fn file_send_explanation() -> &'static str {
    crate::i18n::t("sends.file_send_explanation")
}

/// What a Send **read from a link** needs, when reading one fails for want of
/// the CLI.
///
/// The one genuine subtraction in dropping `bw.exe` for Sends, and it is said
/// as a missing tool rather than left to surface as a bad link: a user told
/// "that link could not be read" goes and checks the link, which is fine.
/// The second sentence is there because without it the first reads as "Sends
/// are broken", which is false in three operations out of four.
pub fn receive_needs_the_cli() -> &'static str {
    crate::i18n::t("sends.receive_needs_the_cli")
}

/// What the pane says when the account genuinely has no Sends. A **claim**,
/// and only reachable from a fetch that succeeded -- see [`pane_state`].
pub fn empty_headline() -> &'static str {
    crate::i18n::t("sends.empty_headline")
}
pub fn empty_detail() -> &'static str {
    crate::i18n::t("sends.empty_detail")
}

/// What the pane says when the fetch failed. Deliberately shares not one word
/// with [`empty_headline()`]: these two blocks are the pair that must never be
/// mistaken for each other, and the tests below assert the separation over
/// the glyphs actually painted rather than over the enum.
pub fn failed_headline() -> &'static str {
    crate::i18n::t("sends.failed_headline")
}

/// The extra sentence an **ambiguous** failure gets, on top of the error's own
/// message. `SendError::is_ambiguous` exists because "could not check" must
/// never render as success, and the strongest form of that untruth on this
/// screen is a blank pane that reads as "you have none".
pub fn ambiguous_detail() -> &'static str {
    crate::i18n::t("sends.ambiguous_detail")
}

/// What the pane says while the CLI has not answered yet.
pub fn loading_label() -> &'static str {
    crate::i18n::t("sends.loading_label")
}

/// One drawn row: everything the pane paints for one Send, and nothing else.
///
/// Built by [`row_from`] out of a [`SendSummary`], so the wording of the
/// expiry -- the only computed field -- is decided once, in a pure function,
/// rather than inside the drawing closure where no test in this crate could
/// reach it.
/// **What this PC filed about a Send it made from a record** -- see
/// `crate::sent_history`. A Send made anywhere else has none, and its row and
/// detail say only what the server says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SentFacts {
    /// Who it went to, as typed; empty when nobody was named.
    pub recipient: String,
    /// The vault item it was made from -- what `Open record` opens.
    pub item_id: String,
    /// The names of the fields that went.
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRow {
    /// See [`SentFacts`]. `None` for a Send this PC has no entry for.
    pub sent: Option<SentFacts>,
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
    /// Whether the detail pane offers **Extend** for this Send: it has an
    /// end this app can read and move. See [`can_extend`].
    pub extendable: bool,
}

/// Whether a Send has an end an Extend could move: a deletion date this app
/// can read that is not `send::NEVER_DELETION_DATE`, or an expiry of its own.
///
/// **A Send that lasts forever is not offered one**, rather than offered a
/// button that changes nothing: `send::extend_dates` leaves "never" as it is,
/// so the press would come back reading "now lasts 7 more days" over dates
/// that did not move. A deletion date this app cannot read is not offered one
/// either, for `extend_dates`' own rule -- it refuses to move a date it
/// cannot read, and a button that can only fail is not a button to draw.
pub fn can_extend(send: &SendSummary) -> bool {
    let never = parse_iso_utc_millis(crate::send::NEVER_DELETION_DATE);
    let Some(deletion) = parse_iso_utc_millis(&send.deletion_date) else {
        return false;
    };
    let expires = !send.expiration_date.is_empty();
    Some(deletion) != never || expires
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
        /// `SendError::is_ambiguous`. Adds [`ambiguous_detail()`].
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

/// **The rows, with what this PC filed about each** -- the recipient on the
/// row's second line (`to m.reyes · ...`, design 5b's own opening), and the
/// record and fields the detail pane's `Open record` and `WHAT WAS SHARED`
/// need. Anything that is not rows passes through untouched.
pub fn with_sent_facts(
    state: SendPaneState,
    history: &crate::sent_history::SentHistory,
) -> SendPaneState {
    match state {
        SendPaneState::Rows(rows) => SendPaneState::Rows(
            rows.into_iter()
                .map(|mut row| {
                    if let Some(entry) = history.for_send(&row.id) {
                        let recipient = entry.recipient.trim().to_string();
                        if !recipient.is_empty() {
                            let short = recipient.split('@').next().unwrap_or(&recipient);
                            row.expiry = crate::i18n::tf("sends.to_recipient_expiry", &[("short", short), ("expiry", &row.expiry.to_string())]);
                        }
                        row.sent = Some(SentFacts {
                            recipient,
                            item_id: entry.item_id.clone(),
                            fields: entry.fields.clone(),
                        });
                    }
                    row
                })
                .collect(),
        ),
        other => other,
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
        sent: None,
        id: send.id.clone(),
        name: if send.name.trim().is_empty() {
            crate::i18n::t("sends.no_name").to_string()
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
        expires: date_words(&send.expiration_date, now, zone, no_expiry_of_its_own()),
        deletes: date_words(&send.deletion_date, now, zone, deletion_date_unknown()),
        extendable: can_extend(send),
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
        0 => crate::i18n::t("sends.nobody_opened").to_string(),
        n => crate::i18n::tn("sends.opened_times", n as u64, &[]),
    };
    match crate::send::send_state(send, now) {
        // Somebody switched the link off -- in another Bitwarden app, since
        // this one no longer does: its Revoke deletes the Send. There is no
        // undo to name here, so the sentence says what is true of the link
        // and stops. Not the word "revoke", which on this screen means the
        // Send is gone rather than switched off.
        crate::send::SendState::Revoked => {
            crate::i18n::tf("sends.activity_revoked", &[("opened", &opened)])
        }
        crate::send::SendState::Used => {
            crate::i18n::tf("sends.activity_used", &[("opened", &opened)])
        }
        crate::send::SendState::Expired => {
            crate::i18n::tf("sends.activity_expired", &[("opened", &opened)])
        }
        // Both live states say the same kind of thing -- what has been
        // opened and what is left -- because the first half of the sentence
        // is already the difference between them ("Nobody has opened this
        // link" against "Opened twice").
        crate::send::SendState::Waiting | crate::send::SendState::Opened => match (
            send.max_access_count,
            send.access_count,
        ) {
            // The remaining budget, which is the fact a live capped link is
            // actually about. Saturating rather than wrapping: a server that
            // reported more opens than the cap while still calling the Send
            // live would otherwise print four billion views left.
            (Some(cap), used) => {
                let left = cap.saturating_sub(used);
                crate::i18n::tn("sends.activity_views_left", left as u64, &[("opened", &opened)])
            }
            (None, _) => crate::i18n::tf("sends.activity_unlimited", &[("opened", &opened)]),
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
pub fn no_expiry_of_its_own() -> &'static str {
    crate::i18n::t("sends.no_expiry_of_its_own")
}

/// What the detail pane says for a deletion date it could not read.
///
/// `expiry_words` has refused to guess "Expired" from an unparseable date
/// since this screen shipped, for the reason it gives: this app failing to
/// understand a date is not the same fact as the link being dead. Same rule,
/// same wording style.
pub fn deletion_date_unknown() -> &'static str {
    crate::i18n::t("sends.deletion_date_unknown")
}

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
        crate::i18n::tn("sends.rel_days", days, &[])
    } else if hours >= 1 {
        crate::i18n::tn("sends.rel_hours", hours, &[])
    } else {
        crate::i18n::t("sends.rel_under_an_hour").to_string()
    };
    if past {
        crate::i18n::tf("sends.rel_ago", &[("amount", &amount)])
    } else {
        crate::i18n::tf("sends.rel_in", &[("amount", &amount)])
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
///   never opened" and "revoked by you - 2 d ago". **Only the middle one is
///   derivable today.** A recipient address is `emails` on the wire and this
///   client neither sets it nor reads it; "40 min ago" and "2 d ago" are a
///   `revisionDate` relative to now, which is available but says nothing until
///   there is a per-access record to date -- see the `5c` timeline, which this
///   pass deliberately did not build.
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
        parts.push(password_segment().to_string());
    }
    match (send.max_access_count, send.access_count) {
        // Capped: the design's own "3 of 10 views", which says the remaining
        // budget and the spent one in one phrase.
        (Some(cap), used) => parts.push(crate::i18n::tf(
            "sends.subtitle_views_of_cap",
            &[("used", &used.to_string()), ("cap", &cap.to_string())],
        )),
        // Uncapped and untouched: nothing. "0 views" on a link nobody has
        // opened yet reads as a failure rather than as a beginning, and the
        // state pill beside it already says `Waiting`.
        (None, 0) => {}
        (None, used) => parts.push(crate::i18n::tn("sends.subtitle_opened_times", used as u64, &[])),
    }
    if crate::send::send_state(send, now).is_live() {
        parts.push(expiry_words(&send.deletion_date, now));
    }
    parts.join(" \u{00b7} ")
}

/// What the subtitle says about a Send whose link is not the whole
/// credential.
pub fn password_segment() -> &'static str {
    crate::i18n::t("sends.password_segment")
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
        return crate::i18n::t("sends.expiry_unknown").to_string();
    };
    let remaining = at - now.now_unix_millis();
    if remaining <= 0 {
        return expired_label().to_string();
    }
    let days = remaining / MILLIS_PER_DAY;
    match days {
        0 => crate::i18n::t("sends.expires_today").to_string(),
        1 => crate::i18n::t("sends.expires_tomorrow").to_string(),
        d => crate::i18n::tn("sends.expires_in_days", d as u64, &[]),
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
    /// `Open record`: show the vault item this Send was made from.
    OpenRecord(String),
    /// Spec B §7's "Change password": open the item this Send was made
    /// from in its editor.
    ChangePassword(String),
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
    /// **Design 5b's Extend**: push this Send's dates out by
    /// `send::EXTEND_BY_DAYS`. One step -- it destroys nothing -- and the id
    /// and the name travel in it for [`ConfirmDelete`](Self::ConfirmDelete)'s
    /// reasons: read off the Send that was on screen, and named again in the
    /// report after the list has been thrown away.
    Extend { id: String, name: String },
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
/// It carries exactly one action, chosen here from the three the card can ask
/// for, with the id and the name the caller found; `SendUiVerdict::seal` and
/// the field stay private, so this is still not a door to substituting an
/// arbitrary action for a drawn pane's.
///
/// **Revoke is a delete, and asks first**, exactly as on the Sent screen: the
/// three requests map onto that screen's own three actions, so the card arms,
/// answers and disarms the same confirmation the Sent screen does.
pub fn revoke_from_the_read_pane(asked: ReadPaneRevoke) -> SendUiVerdict {
    SendUiVerdict::seal(match asked {
        ReadPaneRevoke::Ask(id) => SendUiAction::AskDelete(id),
        ReadPaneRevoke::Confirm { id, name } => SendUiAction::ConfirmDelete { id, name },
        ReadPaneRevoke::Cancel => SendUiAction::CancelDelete,
    })
}

/// What the read pane's `SHARING` card asked of its Send this frame -- the
/// Sent screen's two steps and their way out, and nothing else. See
/// [`revoke_from_the_read_pane`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadPaneRevoke {
    /// Revoke was pressed: arm the confirmation on this Send.
    Ask(String),
    /// `Delete permanently` was pressed: delete this Send.
    Confirm { id: String, name: String },
    /// The confirmation was declined.
    Cancel,
}

/// **One share of a person being forgotten, deleted.** The owner confirmed
/// it ("deleting will remove all shares yes\no"), so this is the confirmed
/// delete itself -- the same action the Sends screen's confirmation sends --
/// sealed like every verdict so it reaches the frame's one
/// `apply_send_action` rather than a second call beside it.
pub fn delete_for_forgetting(id: String, name: String) -> SendUiVerdict {
    SendUiVerdict::seal(SendUiAction::ConfirmDelete { id, name })
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
    /// Whether the work running for `in_flight` is an **Extend** and not a
    /// delete. The lock is the same; only the words differ.
    pub extending: bool,
}

impl SendDeleteView<'_> {
    /// What the Send `in_flight` names says while its work runs, in place
    /// of its controls and of its row's subtitle.
    fn busy_label(&self) -> &'static str {
        if self.extending {
            extending_label()
        } else {
            deleting_label()
        }
    }
}

/// The detail header's **primary** control, and the one the screen is for: a
/// Send exists so that a link can be handed to somebody.
///
/// Named now that [`action_labels`] has to measure the row before the row is
/// drawn -- a literal in two places is one rewording away from a header
/// measured for a label it does not carry.
pub fn copy_link_label() -> &'static str {
    crate::i18n::t("sends.copy_link_label")
}
/// The row's first, non-destructive button.
/// The confirmation's destructive button. **Deliberately not "Delete"**: the
/// two steps must not be the same word, or the second click is muscle memory
/// rather than a decision.
pub fn confirm_label() -> &'static str {
    crate::i18n::t("sends.confirm_label")
}
/// The confirmation's way out, drawn in the Delete button's own rectangle.
pub fn cancel_label() -> &'static str {
    crate::i18n::t("sends.cancel_label")
}
/// The question the row asks while it is confirming, in place of the expiry.
pub fn confirm_prompt() -> &'static str {
    crate::i18n::t("sends.confirm_prompt")
}
/// What a row says while its `bw send delete` is running. It has no buttons
/// at all in that state, so a second click cannot start a second child.
pub fn deleting_label() -> &'static str {
    crate::i18n::t("sends.deleting_label")
}
/// What a Send says while its Extend runs, on the same terms as
/// [`deleting_label()`]: no buttons at all in that state.
pub fn extending_label() -> &'static str {
    crate::i18n::t("sends.extending_label")
}
/// Design §5c's four state words, exactly as the design spells them.
///
/// Public because the paint tests press on them and because the words are the
/// whole of what a pill says -- a state pill whose label drifted from the
/// design would be a pill nobody could look up.
pub fn waiting_label() -> &'static str {
    crate::i18n::t("sends.waiting_label")
}
/// **Not one of §5c's four.** A live link somebody has opened; see
/// `crate::send::SendState`, where why the owner needed a fifth word is
/// argued.
pub fn opened_label() -> &'static str {
    crate::i18n::t("sends.opened_label")
}
pub fn used_label() -> &'static str {
    crate::i18n::t("sends.used_label")
}
pub fn expired_label() -> &'static str {
    crate::i18n::t("sends.expired_label")
}
pub fn revoked_label() -> &'static str {
    crate::i18n::t("sends.revoked_label")
}

/// One [`crate::send::SendState`] as the word the design prints for it.
pub fn state_label(state: crate::send::SendState) -> &'static str {
    match state {
        crate::send::SendState::Waiting => waiting_label(),
        crate::send::SendState::Opened => opened_label(),
        crate::send::SendState::Used => used_label(),
        crate::send::SendState::Expired => expired_label(),
        crate::send::SendState::Revoked => revoked_label(),
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
/// Every colour is the palette's, so the pills go dark with the window: the
/// greens and reds are `done_*` and `danger_*`. 5b draws this pill in the list
/// and again in the detail header and 5d draws it on a record.
pub fn state_tone(p: &crate::kit::tokens::Palette, state: crate::send::SendState) -> theme::PillTone {
    // Spec section 4's pills, borderless: the edge is the fill.
    let pill = |fill, ink, mark| theme::PillTone { fill, edge: fill, ink, mark };
    match state {
        crate::send::SendState::Waiting => pill(p.blue_wash, p.blue_deep, theme::PillMark::Dot(p.blue)),
        crate::send::SendState::Opened => pill(p.done_wash, p.done_ink, theme::PillMark::Dot(p.done_mark)),
        crate::send::SendState::Used => pill(p.done_wash, p.done_ink, theme::PillMark::Check(p.done_mark)),
        // **No mark, and that is the design's own distinction**, not an
        // omission: a dot on these two would say the link is still doing
        // something. See `theme::PillMark`.
        crate::send::SendState::Expired => pill(p.hover, p.text_faint, theme::PillMark::None),
        crate::send::SendState::Revoked => pill(p.danger_wash, p.error, theme::PillMark::None),
    }
}

/// What copying a Send's link says (spec section 4).
pub fn link_copied_toast() -> &'static str {
    crate::i18n::t("sends.link_copied_toast")
}

/// **A record's fields as a row of pills** -- design 5b's `WHAT WAS SHARED`
/// and the whole-record line of 5a's card both draw one. Blue for an ordinary
/// field, red for a secret one (the password), as the design draws them.
/// Wraps when the row runs out of width.
pub fn field_pills(ui: &mut egui::Ui, fields: &[(&str, bool)]) {
    let p = crate::kit::tokens::palette(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
        for (label, secret) in fields {
            let tone = if *secret { secret_field_tone(&p) } else { field_tone(&p) };
            let width = crate::kit::rows::state_pill_width(ui.painter(), tone, label);
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(width, theme::PILL_HEIGHT), egui::Sense::hover());
            crate::kit::rows::state_pill(ui.painter(), egui::pos2(rect.left(), rect.center().y), tone, label);
        }
    });
}

/// An ordinary field's pill: the blue the `Waiting` pill wears, without its dot.
pub fn field_tone(p: &crate::kit::tokens::Palette) -> theme::PillTone {
    theme::PillTone { fill: p.blue_wash, edge: p.blue_edge, ink: p.blue_deep, mark: theme::PillMark::None }
}

/// A secret field's pill: the red the `Revoked` pill wears.
pub fn secret_field_tone(p: &crate::kit::tokens::Palette) -> theme::PillTone {
    theme::PillTone { fill: p.danger_wash, edge: p.danger_edge, ink: p.danger_ink, mark: theme::PillMark::None }
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
pub fn sends_heading() -> &'static str {
    crate::i18n::t("sends.sends_heading")
}

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
        .find(|send| send.name == name && crate::send::send_state(send, now).is_live())
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
        .filter(|send| crate::send::send_state(send, now).is_live())
        .map(|send| send.name.clone())
        .collect()
}

/// The eyebrow over the `Shared with me` list. 5b's own row label, in the
/// case the design's list strips are drawn in.
pub fn received_heading() -> &'static str {
    crate::i18n::t("sends.received_heading")
}

/// What the `Shared with me` screen says when nothing has ever been imported.
///
/// A **claim**, and a true one in every state: unlike the Sends list there is
/// no fetch behind this row and therefore no "could not check" -- see
/// [`crate::receive_history`], whose `load` folds every failure into an empty
/// history for reasons argued there.
pub fn received_empty_headline() -> &'static str {
    crate::i18n::t("sends.received_empty_headline")
}
pub fn received_empty_detail() -> &'static str {
    crate::i18n::t("sends.received_empty_detail")
}

/// The detail pane's standing line while no row is picked.
///
/// **Words, not a blank pane.** The vault's own detail pane says "Select an
/// item." for the same reason: an empty right-hand column reads as a load
/// that failed, and this screen's whole history is about not letting "we do
/// not know" and "there is nothing" look alike.
pub fn nothing_picked() -> &'static str {
    crate::i18n::t("sends.nothing_picked")
}
pub fn nothing_picked_received() -> &'static str {
    crate::i18n::t("sends.nothing_picked_received")
}
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
            SendView::Received(_) => received_heading(),
        }
    }

}

/// The `Shared with me` screen's own scope line, in [`scope_subtext()`]'s
/// spirit: it says where the feature lives rather than leaving its edge to be
/// discovered.
pub fn received_scope_subtext() -> &'static str {
    crate::i18n::t("sends.received_scope_subtext")
}

/// [`received_scope_subtext()`] once records from people are in the list:
/// design 6d's `.lnote`, word for word -- [`shares_subtext()`]'s rule on the
/// other screen.
pub fn received_people_subtext() -> &'static str {
    crate::i18n::t("sends.received_people_subtext")
}

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
pub fn received_item_gone() -> &'static str {
    crate::i18n::t("sends.received_item_gone")
}
/// Its opposite, said just as plainly so the two rows read as one question
/// answered two ways rather than as a warning and a silence.
pub fn received_item_present() -> &'static str {
    crate::i18n::t("sends.received_item_present")
}

/// Every recorded import, newest first, as rows.
///
/// `items` is the LIVE vault list, which is what makes `still_in_vault` mean
/// what it says: an item in the trash is not in that list, and the wording
/// [`received_item_gone()`] uses names that case rather than claiming the item
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
                crate::i18n::t("sends.no_name").to_string()
            } else {
                record.name.clone()
            },
            when: date_words(
                &iso_from_millis(record.received_at_unix_millis),
                now,
                zone,
                received_when_unknown(),
            ),
            still_in_vault: !record.item_id.is_empty()
                && items.iter().any(|item| item.id == record.item_id),
        })
        .collect()
}

/// What a row says when its timestamp could not be read.
pub fn received_when_unknown() -> &'static str {
    crate::i18n::t("sends.received_when_unknown")
}

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
pub fn rows_in_scope(
    rows: &[SendRow],
    scope: crate::vault_window::sidebar::SendScope,
) -> Vec<&SendRow> {
    rows.iter().filter(|row| scope.admits(row.state)).collect()
}

/// What the list column says when the ACCOUNT has Sends but none of them is
/// on the sub-row in force.
///
/// **Not [`empty_headline()`]**, and the separation is this file's oldest rule
/// applied to a third case: "you have no Sends" is a claim about the account,
/// "we could not ask" is a claim about this app, and this is a claim about
/// the filter the user themselves just chose. Printing the account sentence
/// here would tell somebody who clicked `Used` that they have published
/// nothing, which is false and alarming in the direction that matters.
pub fn scope_empty_headline(scope: crate::vault_window::sidebar::SendScope) -> String {
    crate::i18n::tf("sends.scope_empty_headline", &[("scope", &(scope.label().to_lowercase()).to_string())])
}

/// The one way back out of an empty sub-row, said rather than left to be
/// worked out.
pub fn scope_empty_detail() -> &'static str {
    crate::i18n::t("sends.scope_empty_detail")
}

/// Draws the whole Sends screen -- spec A 4's **Shared page**, a grid of cards,
/// and spec B 7's **Send detail modal** over it -- and reports what was
/// pressed. The drawing is `shared_page`'s; this is the screen's door.
///
/// # What replaced what
///
/// 0.15 drew two columns: a list of rows beside a detail pane, the second
/// painted into hand-measured rectangles. The v2 design has one page of cards
/// (two to a row, one on a narrow window) and a modal a card opens, which is
/// how every other detail in the redesign works, and `shared_page` is that.
/// Everything the old screen could do is still done from the same
/// [`SendUiAction`]s -- see its module doc.
///
/// # `notice` spans the page
///
/// It is drawn above it, at full width, because it is a message about the
/// SCREEN. `notice` is the message the window's single inline band is showing
/// this frame, already chosen by `vault_window::inline_notice` -- this
/// function does not decide which of the window's messages wins, it only
/// paints the one it is handed.
///
/// # Eight parameters, and no clock among them
///
/// `view` and `selected` are the window's state and not this pane's: which of
/// the two SHARING screens is up and which card's modal is open. A selection
/// that reset every time the user glanced at Password health would be a modal
/// that could not be left and returned to. `people` joined with design 6c, for
/// the same reason: the shares and the stop the user is in the middle of are
/// the window's `share_ui::SharingPane`, and what a press here asks of it goes
/// back out through `people.asked` -- the verdict stays the Sends' own, one
/// action, for `apply_send_action`.
///
/// Every date the page and the modal print is already worded by
/// [`pane_state`], which is handed the clock and the zone before this function
/// sees a row, so nothing drawn here reads the time at all.
// Draw entry point: its arguments are the state it paints; a bundle would only move them.
#[allow(clippy::too_many_arguments)]
pub fn draw_send_pane(
    ui: &mut egui::Ui,
    state: &SendPaneState,
    notice: Option<&str>,
    delete: SendDeleteView<'_>,
    // Which SHARING screen, and which sub-filter. See [`SendView`].
    view: SendView<'_>,
    // **Which card's modal is open.** `&mut` and the window's, exactly like
    // the item list's `selected_id`: a card writes it, the modal resolves it
    // and clears it when closed, and a selection naming a row that is no
    // longer listed draws no modal rather than a stale one.
    selected: &mut Option<String>,
    // The vault's records and their icons, for the tiles: a Send made from a
    // record wears that record's tile. See [`RecordTiles`].
    records: RecordTiles<'_>,
    // **Design 6c's person shares**, listed before the Sends on the
    // unfiltered row, and the Stop sharing two-step on a share's own detail:
    // the rows in, what was pressed out (`asked`). The window's, like
    // `selected` -- see `share_ui::PeopleRows`. `PeopleRows::none()` for a
    // caller that is not about sharing.
    people: &mut super::share_ui::PeopleRows<'_>,
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
    // The page's own rendering wins, because it is the richer one: it has the
    // headline, the "could not check" line for an ambiguous failure, and Try
    // again. A notice that is *not* the failure being drawn (a move or
    // generate error arriving while the cards are up) is still shown.
    let notice = match (notice, state) {
        (Some(n), SendPaneState::Failed { message, .. }) if n == message => None,
        (n, _) => n,
    };
    if let Some(message) = notice {
        if draw_notice_band(ui, message) {
            action = SendUiAction::DismissNotice;
        }
    }

    // **The Shared page** (spec A 4) and, over it, the Send detail modal
    // (spec B 7): one column of cards where 0.15 had a list and a detail
    // pane. Whatever the page or its modal reports wins the frame.
    if let Some(reported) = shared_page::draw(ui, state, &view, delete, selected, records, people) {
        action = reported;
    }

    SendUiVerdict::seal(action)
}

/// What the strip's count counts, on each of the two screens this pane draws.
///
/// Four constants rather than two `format!`s with the word inlined, because
/// the pair that is easy to get wrong is the SINGULAR: a count line reading
/// "1 sends" is the defect this shape exists to make impossible to write.
pub const SEND_NOUN: &str = "send";
pub const SENDS_NOUN: &str = "sends";
pub const RECORD_NOUN: &str = "record";
pub const RECORDS_NOUN: &str = "records";
/// The failure's own retry, and the one way to ask for the list again without
/// leaving the screen. **Not "Refresh"**, which is what the strip's own button
/// said before it went: this one sits under a sentence explaining what went
/// wrong, and "Refresh" under an error reads as a suggestion that nothing did.
pub fn try_again_label() -> &'static str {
    crate::i18n::t("sends.try_again_label")
}

/// **The records this screen's tiles are drawn from**: the vault's items and
/// the icon cache's textures, both the window's, both borrowed.
///
/// A Send made from a record knows that record through the sent history
/// ([`SentFacts::item_id`]), and the owner's report was that its row showed
/// the monogram `2X` where the records list shows the same record's favicon.
/// So the row asks this for the record, and draws what the records list would
/// draw for it: its icon when the cache has one, its kind's tile when it does
/// not. A Send made anywhere else, or whose record has since gone, has no
/// record here and wears the monogram of its own name.
///
/// `Default` is the answer for a caller with no vault to hand -- a paint test
/// about something else, or `Shared with me`, whose rows are imports and not
/// Sends -- and it draws every tile as a monogram.
#[derive(Clone, Copy, Default)]
pub struct RecordTiles<'a> {
    pub items: &'a [crate::vault_bridge::VaultItem],
    pub icons: Option<&'a std::collections::HashMap<String, egui::TextureHandle>>,
}

impl<'a> RecordTiles<'a> {
    /// The record with this id, while it is still in the vault: the one a
    /// Send was made from (`SentFacts::item_id`, when this PC filed one), or
    /// the one a person share is of.
    fn record(&self, item_id: Option<&str>) -> Option<&'a crate::vault_bridge::VaultItem> {
        let id = item_id?;
        self.items.iter().find(|item| item.id == id)
    }
}

/// One tile, in `tile`, **drawn by `item_list::row_tile`** -- the records
/// list's own tile, so a Send's tile and a record's are one thing: the same
/// unfilled box, the same monogram face and size, the same favicon treatment.
/// This screen used to paint its own: a filled box with a bold 12px pair of
/// initials, visibly not the tile beside it.
///
/// Drawn by a child `Ui` placed exactly on `tile`, because the records tile is
/// a widget that allocates, and this screen lays its rows out against explicit
/// rectangles. `opacity` is design 5b's faded row (see
/// [`theme::ENDED_ROW_OPACITY`]), applied to the child as a whole so the tile
/// recedes with its row whatever is drawn in it.
fn paint_send_tile(
    ui: &mut egui::Ui,
    tile: egui::Rect,
    records: RecordTiles<'_>,
    // The record the row is of or was made from, by id -- see
    // [`RecordTiles::record`].
    record: Option<&str>,
    name: &str,
    emphasised: bool,
    opacity: f32,
) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(tile)
            .layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    child.multiply_opacity(opacity);
    match records.record(record) {
        Some(item) => super::item_list::row_tile(
            &mut child,
            records.icons.and_then(|icons| icons.get(&item.id)),
            crate::vault_bridge::ItemKind::of(item),
            &item.name,
            tile.width(),
            emphasised,
        ),
        None => super::item_list::row_tile(
            &mut child,
            None,
            crate::vault_bridge::ItemKind::Login,
            name,
            tile.width(),
            emphasised,
        ),
    }
}

/// Spec B 7's caution band: what it says, and its button.
pub fn seen_title() -> &'static str {
    crate::i18n::t("sends.seen_title")
}
pub fn seen_body() -> &'static str {
    crate::i18n::t("sends.seen_body")
}
pub fn change_password() -> &'static str {
    crate::i18n::t("sends.change_password")
}

/// Whether `row` was opened and carried the item's password.
pub fn password_was_seen(row: &SendRow) -> bool {
    row.state == crate::send::SendState::Opened
        && row.sent.as_ref().is_some_and(|sent| {
            !sent.item_id.is_empty() && sent.fields.iter().any(|f| f == super::record_ui::STORED_PASSWORD)
        })
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

/// Design 5b's `Extend`: the Send's dates, `send::EXTEND_BY_DAYS` later.
pub fn extend_label() -> &'static str {
    crate::i18n::t("sends.extend_label")
}

/// What hovering Extend says, with the period in it -- read off the same
/// constant the arithmetic uses, so the promise and the date cannot differ.
pub fn extend_hint() -> String {
    crate::i18n::tn("sends.extend_hint", crate::send::EXTEND_BY_DAYS as u64, &[])
}

/// Design 5b's first header button: the vault item the Send was made from.
pub fn open_record_label() -> &'static str {
    crate::i18n::t("sends.open_record_label")
}
/// Design 5b's red header button. **It deletes the Send**, after the
/// confirmation -- the owner's choice between switching a link off and
/// taking it down: "Revoke = delete".
pub fn revoke_label() -> &'static str {
    crate::i18n::t("sends.revoke_label")
}
/// The header's kind line for a Send this PC made from a record.
pub fn record_send_kind() -> &'static str {
    crate::i18n::t("sends.record_send_kind")
}
/// The link card's recipient row.
pub fn recipient_row() -> &'static str {
    crate::i18n::t("sends.recipient_row")
}
/// Design 5b's second card.
pub fn shared_card_title() -> &'static str {
    crate::i18n::t("sends.shared_card_title")
}
/// Design 5b's third card.
pub fn activity_card_title() -> &'static str {
    crate::i18n::t("sends.activity_card_title")
}
/// The activity card's views row.
pub fn opened_row() -> &'static str {
    crate::i18n::t("sends.opened_row")
}
/// Drawn at the right of a live address: press to copy the link.
pub fn copy_marker() -> &'static str {
    crate::i18n::t("sends.copy_marker")
}
/// Every field a record Send can carry, in the order the card names them.
const ALL_SHARED_FIELDS: [&str; 5] = [
    super::record_ui::STORED_USERNAME,
    super::record_ui::STORED_PASSWORD,
    super::record_ui::STORED_WEBSITES,
    super::record_ui::STORED_NOTES,
    super::record_ui::STORED_TOTP,
];

/// The sentence after the pills: what did NOT go.
pub fn not_included(fields: &[String]) -> Option<String> {
    let left: Vec<&str> = ALL_SHARED_FIELDS
        .iter()
        .copied()
        .filter(|f| !fields.iter().any(|sent| sent == f))
        .collect();
    match left.as_slice() {
        [] => None,
        [one] => Some(crate::i18n::tf("sends.not_included_one", &[("field", &super::record_ui::field_label(one))])),
        [init @ .., last] => {
            let shown: Vec<String> = init.iter().map(|f| super::record_ui::field_label(f).into_owned()).collect();
            Some(crate::i18n::tf(
                "sends.not_included_many",
                &[
                    ("fields", &shown.join(", ")),
                    ("last", &super::record_ui::field_label(last).to_lowercase()),
                ],
            ))
        }
    }
}

/// What design 5c's band says, in the one form this client can support.
///
/// **5c says "This password has been seen by someone else" and this cannot**,
/// because a Send made here is arbitrary text: it may be a password, a
/// connection string, a one-off token or a note. Naming it a password would
/// be a guess, and a guess in the reassuring direction on the half of the
/// cases where it is something worse. So the headline says what is certainly
/// true -- somebody opened the link, and whatever was behind it is out -- and
/// the advice is conditional in the one place where it has to be.
pub fn caution_headline() -> &'static str {
    crate::i18n::t("sends.caution_headline")
}
pub fn caution_advice() -> &'static str {
    crate::i18n::t("sends.caution_advice")
}

/// The detail header's subtitle: what kind of Send this is. 5b's slot for
/// "Send - created ...", carrying the one half of that this client holds.
pub fn text_send_kind() -> &'static str {
    crate::i18n::t("sends.text_send_kind")
}
pub fn file_send_kind() -> &'static str {
    crate::i18n::t("sends.file_send_kind")
}
/// Its counterpart on the `Shared with me` detail.
pub fn received_kind() -> &'static str {
    crate::i18n::t("sends.received_kind")
}

/// Design 5b's `Link` card and its rows.
pub fn link_card_title() -> &'static str {
    crate::i18n::t("sends.link_card_title")
}
pub fn address_row() -> &'static str {
    crate::i18n::t("sends.address_row")
}
pub fn opens_with_row() -> &'static str {
    crate::i18n::t("sends.opens_with_row")
}
pub fn views_row() -> &'static str {
    crate::i18n::t("sends.views_row")
}
pub fn expires_row() -> &'static str {
    crate::i18n::t("sends.expires_row")
}
pub fn deleted_row() -> &'static str {
    crate::i18n::t("sends.deleted_row")
}
/// 5b's own word beside a struck-through address.
pub fn dead_marker() -> &'static str {
    crate::i18n::t("sends.dead_marker")
}
/// The `Opens with` row for a Send that has no share password -- 5b's "link
/// only", said as what the recipient needs rather than as what is missing.
pub fn link_only() -> &'static str {
    crate::i18n::t("sends.link_only")
}
/// The `Views` row for an uncapped Send.
pub fn no_view_limit() -> &'static str {
    crate::i18n::t("sends.no_view_limit")
}
/// The `Address` row for a Send the server described without one. See
/// `draw_send_card` on why the control beside it is drawn and dead rather
/// than absent.
pub fn address_missing() -> &'static str {
    crate::i18n::t("sends.address_missing")
}

/// The `Shared with me` detail's card and its rows.
pub fn received_card_title() -> &'static str {
    crate::i18n::t("sends.received_card_title")
}
pub fn arrived_row() -> &'static str {
    crate::i18n::t("sends.arrived_row")
}
pub fn in_vault_row() -> &'static str {
    crate::i18n::t("sends.in_vault_row")
}
pub fn in_vault_yes() -> &'static str {
    crate::i18n::t("sends.in_vault_yes")
}
pub fn in_vault_no() -> &'static str {
    crate::i18n::t("sends.in_vault_no")
}

/// The standing note at the record composer's footer's right-hand end:
/// **§5a's `Appears in Shared`, in this app's own vocabulary.**
///
/// §5a says `Shared` because the rail row its design puts a new Send into is
/// called that. This app's is called `Sends` -- `sidebar::SENDS_ROW_LABEL` --
/// under a `SHARING` section, so `Appears in Shared` would name a row the
/// user cannot find. The note's job is to answer "where does this go when I
/// press the blue button", and an answer that does not match the rail is
/// worse than none.
///
/// It is `pub`, and here beside the Access block rather than in `record_ui`,
/// because it is a fact about where a Send goes -- the list this file draws --
/// and not about the card that makes one.
pub fn appears_in_sends() -> &'static str {
    crate::i18n::t("sends.appears_in_sends")
}

/// The share-password field's placeholder. **It says what an empty box
/// means**, because empty is both the default and the off position, and a
/// blank box under the words "Open with" otherwise reads as something the
/// user has forgotten to fill in. See [`draw_access_block`].
pub fn password_hint() -> &'static str {
    crate::i18n::t("sends.password_hint")
}

// `PASSWORD_TOGGLE_LABEL` -- "Require a password to open the link" -- and
// `LIFETIME_PROMPT` -- "The link stops working after" -- are DELETED here
// rather than parked, and the deletion is the decision. Both were the old
// text composer's own captions for two questions that are now asked once, in
// [`access_eyebrow()`]'s block, in the row labels [`expires_label()`],
// [`views_label()`] and [`open_with_label()`]. Keeping them would have left two
// spellings of one question in one file, which is the defect this pass
// exists to remove; and a `pub const` raises no dead-code warning in a lib
// crate, so an unused one sits indefinitely looking like copy somebody still
// paints. The password switch went with its label -- see
// [`draw_access_block`] on why an empty box is the off position now.
// ---------------------------------------------------------------------------
// Design section 5a's ACCESS block
// ---------------------------------------------------------------------------

/// Design §5a's third section eyebrow, over the Access rows. The design's own
/// word, in the design's own case, for `record_ui::RECORD_EYEBROW`'s reason:
/// [`theme::eyebrow`] deliberately does not uppercase for the caller, so the
/// constant carries the case the glyphs are painted in and a paint test can
/// look for it.
pub fn access_eyebrow() -> &'static str {
    crate::i18n::t("sends.access_eyebrow")
}

/// §5a's first Access row label.
pub fn expires_label() -> &'static str {
    crate::i18n::t("sends.expires_label")
}
/// §5a's second Access row label. **The design's own word**, and the right
/// one: the server counts *accesses*, and "Opens" would promise that a
/// recipient who loads the page twice has spent two.
pub fn views_label() -> &'static str {
    crate::i18n::t("sends.views_label")
}
/// §5a's third Access row label.
pub fn open_with_label() -> &'static str {
    crate::i18n::t("sends.open_with_label")
}

/// §5a's own word on the link beside [`open_with_label()`]'s box.
pub fn generate_label() -> &'static str {
    crate::i18n::t("sends.generate_label")
}

/// §5a's `font-size: 12px` on that link, and the gap before it.
const ACCESS_LINK_PX: f32 = 12.0;
/// See [`ACCESS_LINK_PX`] -- §5a's `gap: 10px`.
const ACCESS_LINK_GAP: f32 = 10.0;

/// The view-limit field's placeholder, and the whole of how the row says its
/// off position. See [`views_note`].
pub fn views_hint() -> &'static str {
    crate::i18n::t("sends.views_hint")
}

/// What the Access block says beside the view-limit box when there is no
/// limit. **Not an empty space**: a blank box with nothing after it is a
/// field the user is being asked to fill in, and this one is optional.
pub fn no_view_limit_note() -> &'static str {
    crate::i18n::t("sends.no_view_limit_note")
}

/// What it says when there is one. §5a's own phrase, and it is the sentence
/// that makes the number worth typing: a cap is not "how many people may
/// read this", it is "after this many, the link is dead".
pub fn view_limit_note() -> &'static str {
    crate::i18n::t("sends.view_limit_note")
}

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
        Some(_) => view_limit_note(),
        None => no_view_limit_note(),
    }
}

/// The three fields of a [`crate::send::SendPlan`] design §5a's Access block
/// is a control for, borrowed rather than copied.
///
/// # Why a borrow bundle and not a struct the screen owns
///
/// The record composer builds its plan at submit time out of a
/// `record_ui::RecordDraft`, and this block writes three of that draft's
/// fields as the user answers. Borrowing them is what keeps the block from
/// owning a copy -- and a copy is exactly where the validated value and the
/// published value come apart. Three `&mut`s cost nothing, convert nothing,
/// and mean the block writes into the buffer that will be published. (It
/// was shaped this way when a second screen, the Sent screen's text
/// composer, drove it from a whole `SendPlan`; that screen went, and the
/// shape is still the right one for the one that is left.)
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
pub const EXPIRY_FIELD_WIDTH: f32 = 180.0;

/// The salt `theme::date_picker` remembers its month under.
///
/// A constant rather than a literal at the call site because both composers
/// draw the same block through [`draw_access_block`], and two screens that
/// each invented a salt would each remember a different month for what the
/// user experiences as one control.
const EXPIRY_CALENDAR_ID: &str = "send-expiry-calendar";

/// One row of the expiry dropdown, named.
///
/// The eight fixed rows are `crate::vault_window::send_ui::words::lifetime_label`'s job. The ninth has
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
        Some(lifetime) => crate::vault_window::send_ui::words::lifetime_label(lifetime, zone),
        None if matches!(current, crate::send::SendLifetime::Until(_)) => {
            crate::vault_window::send_ui::words::lifetime_label(current, zone)
        }
        None => crate::vault_window::send_ui::words::pick_a_date_label().to_string(),
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
) -> (crate::kit::pills::Day, crate::kit::pills::Day) {
    let (first, last) = crate::send::pickable_window(now, zone);
    (
        crate::kit::pills::Day { year: first.0, month: first.1, day: first.2 },
        crate::kit::pills::Day { year: last.0, month: last.1, day: last.2 },
    )
}

/// The three Access row labels, in the order the block draws them, so
/// [`access_label_width`] measures exactly the set that is on screen.
fn access_labels() -> [&'static str; 3] {
    [expires_label(), views_label(), open_with_label()]
}

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
    let ink = crate::kit::tokens::palette(ui.ctx()).text_nav;
    let room = ui.available_width();
    if ACCESS_LABEL_WIDTH + ACCESS_ROW_GAP + EXPIRY_FIELD_WIDTH <= room {
        return ACCESS_LABEL_WIDTH;
    }
    access_labels()
        .iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(
                    (*label).to_string(),
                    egui::FontId::new(ACCESS_LABEL_PX, egui::FontFamily::Proportional),
                    ink,
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
    let p = crate::kit::tokens::palette(ui.ctx());
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
                .color(if enabled { p.text_nav } else { p.text_ghost }),
        );
        body(ui)
    })
    .inner
}

/// The Recipient row's label, 5a's own word.
pub fn recipient_label() -> &'static str {
    crate::i18n::t("sends.recipient_label")
}
/// What the empty box says. Optional, and it says so: a Send needs no
/// recipient, and naming one only files the Send under that person.
pub fn recipient_hint() -> &'static str {
    crate::i18n::t("sends.recipient_hint")
}
/// The eyebrow over the suggestions.
pub fn suggestions_eyebrow() -> &'static str {
    crate::i18n::t("sends.suggestions_eyebrow")
}
/// The forget mark at the end of each suggestion.
pub const FORGET_MARK: &str = "\u{00D7}";

/// A suggestion's hover wash, rounded inside the list's own 9pt corner.
const SUGGESTION_ROW_RADIUS: CornerRadius = CornerRadius::same(10);

/// A suggestion row (the v2 popover): a 30 tile with 7 above and below.
const SUGGESTION_ROW_HEIGHT: f32 = 44.0;

/// How many Sends this file holds for a person, beside their address.
pub fn sent_count_words(count: usize) -> String {
    crate::i18n::tf("sends.sent_count", &[("count", &count.to_string())])
}

/// What the Recipient row reports: nothing, or that a suggestion's
/// [`FORGET_MARK`] was pressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipientAnswer {
    Nothing,
    Forget(String),
}

/// **5a's Recipient row, and the people sent to before.**
///
/// The owner: "if sent once - we should save the email\recepient and suggest
/// those from the list next time, person can also click x in the list to
/// delete a person". A box in the Access block's label column; while it has
/// focus -- or the pointer is over the list, so a click on a suggestion is
/// not lost to the box giving up focus on the press -- the people in
/// `crate::sent_history` whose address contains what is typed, most recently
/// sent first. A click fills the box; [`FORGET_MARK`] asks to forget the
/// person, which the window confirms first when they still hold shares.
///
/// It is a label kept on this PC, not a lock on the link: see
/// `crate::sent_history`.
pub fn draw_recipient_row(
    ui: &mut egui::Ui,
    recipient: &mut String,
    history: &crate::sent_history::SentHistory,
    enabled: bool,
) -> RecipientAnswer {
    let label_width = access_label_width(ui);
    ui.add_space(ACCESS_ROW_SPACING);
    let field = access_row(ui, recipient_label(), label_width, enabled, |ui| {
        let width = ui.available_width();
        ui.add_enabled_ui(enabled, |ui| crate::kit::input::inline(ui, recipient, recipient_hint(), width, false))
            .inner
    });
    match draw_people_suggestions(
        ui,
        egui::Id::new("record-recipient-suggestions"),
        &field,
        recipient,
        history,
        enabled,
        suggestions_eyebrow(),
        &|person| sent_count_words(person.send_ids.len()),
    ) {
        Some(email) => RecipientAnswer::Forget(email),
        None => RecipientAnswer::Nothing,
    }
}

/// **The people suggested under an address box** -- 5a's `SENT TO BEFORE`
/// list, and 6a's `SHARED WITH BEFORE`, which is the same list fed by both
/// kinds of entry. Answers the address whose [`FORGET_MARK`] was pressed.
///
/// One function for the two boxes that suggest people, because 6a's own note
/// is that the Share dialog's list is the Send composer's list, renamed: the
/// same rows, the same hover wash, the same ✕ at the same inset. The two
/// differ in their eyebrow and in what the count beside each address says --
/// a Send composer counts Sends, the Share dialog counts both kinds -- so
/// those two are what a caller passes.
///
/// While `field` has focus -- or the pointer is over the list, so a click on
/// a suggestion is not lost to the box giving up focus on the press -- the
/// people in `crate::sent_history` whose address contains what is typed, most
/// recently sent first. A click fills the box.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_people_suggestions(
    ui: &mut egui::Ui,
    popup_id: egui::Id,
    field: &egui::Response,
    typed: &mut String,
    history: &crate::sent_history::SentHistory,
    enabled: bool,
    eyebrow: &str,
    count: &dyn Fn(&crate::sent_history::Person) -> String,
) -> Option<String> {
    let recipient = typed;
    let mut answer = None;
    let last: Option<egui::Rect> = ui.data(|d| d.get_temp(popup_id));
    let pointer_in = last.is_some_and(|rect| {
        ui.input(|i| i.pointer.hover_pos().is_some_and(|at| rect.contains(at)))
    });
    let people = history.suggestions(recipient);
    if !(enabled && !people.is_empty() && (field.has_focus() || pointer_in)) {
        ui.data_mut(|d| d.remove::<egui::Rect>(popup_id));
        return answer;
    }
    let width = field.rect.width();
    let shown = egui::Area::new(popup_id)
        .order(egui::Order::Foreground)
        .fixed_pos(field.rect.left_bottom() + egui::vec2(0.0, 4.0))
        .show(ui.ctx(), |ui| {
            // The v2 popover: a white card at radius 14 with the design's ring
            // and soft shadow, 8 of padding, tracked capitals over rows with
            // an initials tile.
            let palette = crate::kit::tokens::palette(ui.ctx());
            crate::kit::menu::RICH
                .frame(&palette)
                .show(ui, |ui| {
                    let inner = (width - 16.0).max(160.0);
                    ui.set_width(inner);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        crate::kit::card::section_label(ui, eyebrow, palette.text_ghost);
                    });
                    ui.add_space(4.0);
                    for person in &people {
                        let (row, pressed) =
                            ui.allocate_exact_size(egui::vec2(inner, SUGGESTION_ROW_HEIGHT), egui::Sense::click());
                        let mark = egui::Rect::from_min_size(
                            egui::pos2(row.right() - 32.0, row.top()),
                            egui::vec2(32.0, row.height()),
                        );
                        let forget = ui.interact(
                            mark,
                            popup_id.with(&person.email).with("forget"),
                            egui::Sense::click(),
                        );
                        let p = ui.painter();
                        if pressed.hovered() || forget.hovered() {
                            p.rect_filled(row, SUGGESTION_ROW_RADIUS, palette.row_hover_blue);
                        }
                        let tile = egui::Rect::from_min_size(
                            egui::pos2(row.left() + 6.0, row.center().y - 15.0),
                            egui::Vec2::splat(30.0),
                        );
                        let (bg, fg) = crate::kit::avatar::initials_colours(&person.email);
                        let initials: String = person.email.chars().take(2).flat_map(char::to_uppercase).collect();
                        crate::kit::avatar::paint(p, tile, &crate::kit::avatar::Look::Initials { text: initials, bg, fg }, 9);
                        let text_left = tile.right() + 10.0;
                        let count = p.text(
                            egui::pos2(mark.left() - 6.0, row.center().y),
                            egui::Align2::RIGHT_CENTER,
                            count(person),
                            egui::FontId::new(13.0, egui::FontFamily::Proportional),
                            palette.text_faint,
                        );
                        let email = elided(
                            p,
                            &person.email,
                            egui::FontId::new(15.0, egui::FontFamily::Name(theme::SEMIBOLD.into())),
                            palette.ink,
                            (count.left() - 10.0 - text_left).max(0.0),
                        );
                        p.galley(egui::pos2(text_left, row.center().y - email.size().y / 2.0), email, palette.ink);
                        p.text(
                            mark.center(),
                            egui::Align2::CENTER_CENTER,
                            FORGET_MARK,
                            egui::FontId::new(15.0, egui::FontFamily::Proportional),
                            if forget.hovered() { palette.error } else { palette.text_faint },
                        );
                        if forget.hovered() || pressed.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if forget.clicked() {
                            answer = Some(person.email.clone());
                        } else if pressed.clicked() {
                            *recipient = person.email.clone();
                            ui.memory_mut(|m| m.surrender_focus(field.id));
                        }
                    }
                });
        });
    ui.data_mut(|d| d.insert_temp(popup_id, shown.response.rect));
    answer
}

/// **Design §5a's `ACCESS` block, and the one copy of it this app has.**
///
/// `record_ui::draw_export_form` calls it, in the record modal, and it is the
/// only Send composer this app has: the Sent screen's text composer went with
/// its `New Send` button, because a Send is made from a record. The block was
/// shared while there were two, so that two screens publishing one kind of
/// Send could not answer "how long does this link live, who can open it, how
/// many times" differently; it is kept here, beside the Sends it describes.
/// Before it, one composer had a lifetime row and a password tick-box, the
/// other had neither, and neither offered the view cap that
/// `SendPlan::max_access_count` has carried and published since the feature
/// was written.
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
/// `now` and `zone` are injected for `crate::vault_window::send_ui::words::expiry_wording`'s reason,
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
    let p = crate::kit::tokens::palette(ui.ctx());
    crate::kit::card::section_label(ui, access_eyebrow(), p.text_ghost);
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
    let choices: Vec<crate::kit::menu::Choice<'_>> = labels
        .iter()
        .zip(crate::send::LIFETIME_CHOICES)
        .map(|(label, choice)| {
            // The picked-date row is the answer in force whenever the answer
            // IS a picked date, whatever date that is -- it is one row, not a
            // row per day.
            crate::kit::menu::Choice::new(
                label.as_str(),
                match choice {
                    Some(lifetime) => *controls.lifetime == lifetime,
                    None => matches!(*controls.lifetime, crate::send::SendLifetime::Until(_)),
                },
            )
        })
        .collect();
    let current = crate::vault_window::send_ui::words::lifetime_label(*controls.lifetime, zone);
    access_row(ui, expires_label(), label_width, enabled, |ui| {
        // `dropdown_disabled` and not `add_enabled_ui`, for the reason that
        // split exists in this design system (see `toggle_pill` /
        // `toggle_pill_disabled`, and the segmented run before it): the inert
        // box senses nothing, so while a publish is in flight there is no path
        // by which it can be opened at all -- and the answer in force stays at
        // full strength rather than fading with the box round it.
        if enabled {
            if let Some(index) =
                crate::kit::menu::select(ui, "send-expiry", EXPIRY_FIELD_WIDTH, crate::kit::menu::ROW, &current, &choices)
            {
                if let Some(next) = lifetime_for_row(index, *controls.lifetime, now, zone) {
                    *controls.lifetime = next;
                }
            }
        } else {
            crate::kit::menu::select_disabled(ui, crate::kit::menu::ROW, EXPIRY_FIELD_WIDTH, &current);
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
        egui::RichText::new(crate::vault_window::send_ui::words::expiry_wording(*controls.lifetime, now, zone))
            .size(11.0)
            .color(p.text_faint),
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
            .map(|(year, month, day)| crate::kit::pills::Day { year, month, day });
        // The calendar is drawn while a publish is in flight too, and it is
        // simply not asked for its answer: the chosen day stays legible, which
        // is what the row above it does, and a grid that vanished for the
        // duration of a publish would take the reader's own answer off the
        // card at the moment they are watching it be used.
        if let Some(day) = crate::kit::pills::date_picker(ui, EXPIRY_CALENDAR_ID, selected, first, last) {
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
    access_row(ui, views_label(), label_width, enabled, |ui| {
        // `theme::inline_field` and not a bare `egui::TextEdit`, which is what
        // stood here: the bare one wears egui's frame, egui's radius and
        // egui's one-line height, so this row's box and the `Open with` box
        // below it and the two boxes on the composer above were four
        // different-looking controls on one card. See that function.
        let changed = ui
            .add_enabled_ui(enabled, |ui| {
                crate::kit::input::inline(ui, &mut typed, views_hint(), VIEW_LIMIT_FIELD_WIDTH, false)
            })
            .inner
            .changed();
        if changed {
            *controls.max_access_count = view_limit_from(&typed);
        }
        ui.label(
            egui::RichText::new(views_note(*controls.max_access_count))
                .size(12.0)
                .color(p.text_faint),
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
    access_row(ui, open_with_label(), label_width, enabled, |ui| {
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
                generate_label().to_string(),
                egui::FontId::proportional(ACCESS_LINK_PX),
                p.blue_deep,
            )
            .size()
            .x;
        let room = (ui.available_width() - link - ACCESS_LINK_GAP).max(0.0);
        ui.add_enabled_ui(enabled, |ui| {
            crate::kit::input::inline(ui, &mut buffer, password_hint(), room, true);
        });
        ui.add_space(ACCESS_LINK_GAP);
        if enabled && crate::kit::controls::link(ui, generate_label(), ACCESS_LINK_PX).clicked() {
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

/// The inline band. Same shape the item list's band has -- one line, clicking
/// it anywhere dismisses -- drawn here because the item list is not on screen
/// when this pane is.
fn draw_notice_band(ui: &mut egui::Ui, message: &str) -> bool {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
    let p = crate::kit::tokens::palette(ui.ctx());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), p.blue_wash);
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        message,
        egui::FontId::new(12.0, egui::FontFamily::Proportional),
        p.error,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

#[cfg(test)]
mod tests {
    /// Spec section 4's state pills: Waiting blue, Opened and Used green,
    /// Expired grey, Revoked red -- each fill with its own ink.
    #[test]
    fn state_pills_wear_the_designs_colours() {
        use crate::kit::tokens::Palette;
        use crate::send::SendState;
        let p = Palette::LIGHT;
        for (state, fill, ink) in [
            (SendState::Waiting, p.blue_wash, p.blue_deep),
            (SendState::Opened, p.done_wash, p.done_ink),
            (SendState::Used, p.done_wash, p.done_ink),
            (SendState::Expired, p.hover, p.text_faint),
            (SendState::Revoked, p.danger_wash, p.error),
        ] {
            let tone = super::state_tone(&p, state);
            assert_eq!((tone.fill, tone.ink), (fill, ink), "{state:?}");
        }
        assert_eq!(super::link_copied_toast(), "Link copied");
    }

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
        let mut fetch = SendFetch {
            in_flight: true,
            ..SendFetch::default()
        };
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
                    let _ = draw_send_pane(ui, &state, None, SendDeleteView::default(), SendView::Mine(crate::vault_window::sidebar::SendScope::All), &mut None, RecordTiles::default(), &mut crate::vault_window::share_ui::PeopleRows::none());
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
        /// Every textured rectangle -- a favicon is the only picture this
        /// screen paints, and it paints no string and no plain fill.
        images: Vec<egui::Rect>,
        /// Runs painted with a line through them.
        struck: Vec<String>,
        /// Where in `text_rects` the Send detail modal's own words begin: the
        /// scrim is painted after the page and before the dialog, so what
        /// follows it is the modal's. `None` when no modal was up.
        dialog_from: Option<usize>,
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
        /// wrong for a control: a sentence that names a button contains the
        /// button's label, so a substring count cannot tell the sentence from
        /// the button it names. Naming the button in prose is good writing
        /// and the test has to be the thing that adapts.
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

        /// Runs of exactly `label` painted on the PAGE (the cards), and in
        /// the Send detail MODAL over it, told apart by where the scrim fell
        /// in the paint order.
        ///
        /// **The page and the modal draw some of the same words** -- a card
        /// and the modal of the same Send both carry its pill -- so a
        /// whole-screen count cannot say where a thing is. "The pill is on
        /// the card" and "the pill is on the modal" are two claims.
        fn in_list(&self, label: &str) -> usize {
            let end = self.dialog_from.unwrap_or(self.text_rects.len());
            self.text_rects[..end].iter().filter(|(t, _)| t == label).count()
        }

        fn in_detail(&self, label: &str) -> usize {
            let from = self.dialog_from.unwrap_or(self.text_rects.len());
            self.text_rects[from..].iter().filter(|(t, _)| t == label).count()
        }

        /// Every run the modal painted.
        fn dialog_text_rects(&self) -> impl Iterator<Item = &(String, egui::Rect)> {
            let from = self.dialog_from.unwrap_or(self.text_rects.len());
            self.text_rects[from..].iter()
        }

        /// The rect of the modal's run of exactly `label`.
        fn dialog_rect_of(&self, label: &str) -> egui::Rect {
            self.dialog_text_rects()
                .find(|(t, _)| t == label)
                .map(|(_, r)| *r)
                .unwrap_or_else(|| panic!("{label:?} was not painted in the modal: {:?}", self.text))
        }

        /// Where a click on `label` lands: the modal's runs when it has any
        /// (the scrim is over the page), the page's otherwise.
        fn rects_clickable(&self, label: &str) -> Vec<egui::Rect> {
            let modal: Vec<egui::Rect> =
                self.dialog_text_rects().filter(|(t, _)| t == label).map(|(_, r)| *r).collect();
            if modal.is_empty() {
                self.text_rects[..self.dialog_from.unwrap_or(self.text_rects.len())]
                    .iter()
                    .filter(|(t, _)| t == label)
                    .map(|(_, r)| *r)
                    .collect()
            } else {
                modal
            }
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
                if text.galley.job.sections.iter().any(|s| s.format.strikethrough.width > 0.0) {
                    out.struck.push(text.galley.text().to_owned());
                }
                out.text.push(text.galley.text().to_owned());
                out.text_rects
                    .push((text.galley.text().to_owned(), text.visual_bounding_rect()));
            }
            egui::Shape::Rect(rect) => {
                // The scrim: a window-sized rectangle in the dialog's blue-grey
                // at some strength up to its own (it fades in, so the first
                // frames are fainter than `modal::SCRIM`).
                let scrim = rect.rect.min == egui::Pos2::ZERO
                    && rect.rect.width() >= 600.0
                    && rect.fill.a() > 0
                    && rect.fill.a() <= crate::kit::modal::SCRIM.a();
                if scrim && out.dialog_from.is_none() {
                    out.dialog_from = Some(out.text_rects.len());
                }
                out.rects.push(rect.rect);
                out.fills.push((rect.rect, rect.fill));
                out.strokes.push((rect.rect, rect.stroke.color));
                if rect.brush.is_some() {
                    out.images.push(rect.rect);
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Runs `f` for three frames and answers the last one's output. **A
    /// dialog's card is an anchored `Area`, invisible on its first pass**, so
    /// a harness that draws once never sees the modal the page opened.
    fn warm_run(
        ctx: &egui::Context,
        input: impl Fn() -> egui::RawInput,
        mut f: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        for _ in 0..2 {
            let _ = ctx.run_ui(input(), |ui| f(ui));
        }
        ctx.run_ui(input(), |ui| f(ui))
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
        let output = warm_run(&ctx, input, |ui| {
            action = draw_send_pane(ui, state, notice, delete, SendView::Mine(ALL_SENDS), &mut selected, RecordTiles::default(), &mut crate::vault_window::share_ui::PeopleRows::none()).into_action();
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
            .filter(|(rect, colour)| *colour == crate::kit::tokens::Palette::LIGHT.card && rect.contains_rect(text))
            .map(|(rect, _)| *rect)
            .min_by(|a, b| area(a).total_cmp(&area(b)))
            .unwrap_or_else(|| {
                panic!("{label:?} is not inside any box this design system painted")
            })
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
    /// the block and no test could see it, because every assertion on
    /// this block was about one row at a time.
    ///
    /// The floor pane is the half of this that matters most: 96 + 14 + 150 is
    /// 260 against 226 of card, so the design's own column does not fit and
    /// `access_label_width` narrows it. What must survive that is not the 96
    /// -- it is the three agreeing.
    #[test]
    fn the_access_rows_start_their_controls_on_one_line() {
        for (where_, pane) in access_cards() {
            let mut plan = a_plan();
            let painted = paint_access_at(&mut plan, true, pane);
            let chosen = crate::vault_window::send_ui::words::lifetime_label(plan.lifetime, &UTC);

            let expires = box_under(&painted, &chosen);
            let views = box_under(&painted, views_hint());
            let open_with = box_under(&painted, password_hint());

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

    /// **§5a's ACCESS block, painted on its own card**, with whatever plan
    /// the caller wants and the block live or inert.
    ///
    /// The block used to be reached from here only through the Sent screen's
    /// text composer, which drew it on the same card the record composer
    /// does; the composer went with its `New Send` button, and the block did
    /// not -- `record_ui::draw_export_form` draws it for every Send made from
    /// a record. So these tests draw it the way that card does, on
    /// `theme::form_card`, through the same three `&mut`s into a
    /// `SendPlan`, and lose none of what they measured.
    ///
    /// `enabled: false` is the card while a publish is running, which is the
    /// state the record composer draws it in.
    fn paint_access_at(
        plan: &mut crate::send::SendPlan,
        enabled: bool,
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

        let output = ctx.run_ui(input(), |ui| {
            crate::kit::form::section_card(ui, |ui| {
                crate::kit::form::section_card_body_at(ui, crate::kit::form::section_card_pad_x(ui.available_width()), |ui| {
                    draw_access_block(
                        ui,
                        AccessControls {
                            lifetime: &mut plan.lifetime,
                            password: &mut plan.password,
                            max_access_count: &mut plan.max_access_count,
                        },
                        enabled,
                        &crate::send::FixedClock(NOW),
                        &UTC,
                    );
                });
            });
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        assert!(
            painted.has(access_eyebrow()),
            "the fixture did not draw the Access block at all, so every assertion below would \
             be about an empty card: {:?}",
            painted.text
        );
        painted
    }

    /// [`paint_access_at`] on a comfortable card.
    fn paint_access(plan: &mut crate::send::SendPlan, enabled: bool) -> Painted {
        paint_access_at(plan, enabled, ROOMY_ACCESS_CARD)
    }

    /// A card with room to spare, for the assertions about the block's own
    /// internal geometry.
    const ROOMY_ACCESS_CARD: egui::Vec2 = egui::vec2(720.0, 900.0);

    /// **The two cards every assertion about the block's GEOMETRY is made
    /// against**, named together so a test cannot quietly be written at one of
    /// them.
    ///
    /// The second is the narrowest this block has ever been drawn at: the
    /// Sent screen's detail column at the pane floor, less its padding -- the
    /// 250 points §5a's Access block does not fit at the design's own
    /// measurements, which is why `access_label_width` exists. Made TALL
    /// rather than 600 high, because egui culls shapes outside the screen
    /// rect and the axis under test is the width.
    fn access_cards() -> [(&'static str, egui::Vec2); 2] {
        [
            ("the roomy card", ROOMY_ACCESS_CARD),
            ("the narrowest card", egui::vec2(min_detail_width() - 48.0, 1600.0)),
        ]
    }

    /// A plan with a name and a body, at every default -- what the record
    /// composer hands the block before the user touches it.
    fn a_plan() -> crate::send::SendPlan {
        crate::send::SendPlan {
            name: "SAP Production".to_string(),
            text: zeroize::Zeroizing::new("hunter2".to_string()),
            ..crate::send::SendPlan::default()
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
        let mut plan = a_plan();
        let chosen = crate::vault_window::send_ui::words::lifetime_label(plan.lifetime, &UTC);
        let painted = paint_access(&mut plan, true);

        let (shut, _) = painted.control_under(&chosen);
        assert_eq!(
            shut.height(),
            crate::kit::menu::ROW.height,
            "the lifetime control is {}pt tall against the design system's dropdown box of {}",
            shut.height(),
            crate::kit::menu::ROW.height
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
            let label = crate::vault_window::send_ui::words::lifetime_label(other, &UTC);
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
            crate::vault_window::send_ui::words::pick_a_date_label()
        );
        let picked = crate::send::SendLifetime::on_local_day(2027, 3, 14, &UTC);
        assert_eq!(lifetime_row_label(None, picked, &UTC), "14 Mar 2027");
        // And a fixed row is never affected by what is in force.
        assert_eq!(
            lifetime_row_label(Some(crate::send::SendLifetime::Never), picked, &UTC),
            crate::vault_window::send_ui::words::never_label()
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
                let width = {
                    let job = crate::kit::tokens::job(crate::kit::menu::ROW.role, &label, egui::Color32::BLACK);
                    ui.fonts_mut(|f| f.layout_job(job)).size().x
                };
                if width > widest {
                    widest = width;
                    name = label;
                }
            }
        });
        assert!(widest > 0.0, "control: no row was measured at all");
        let room = EXPIRY_FIELD_WIDTH
            - (crate::kit::menu::ROW.pad_x * 2.0 + crate::kit::menu::CHEVRON_WIDTH + crate::kit::menu::ROW_GAP);
        assert!(
            widest <= room,
            "the {name:?} row is {widest}pt wide and the box leaves {room}pt for text, \
             so it is clipped"
        );
    }

    /// **While a publish is in flight the lifetime control is inert, and the
    /// answer in force is still legible.**
    ///
    /// The record composer draws the block with `enabled: false` while its
    /// `bw send create` runs. `theme::dropdown_disabled` is what the block
    /// draws in that state, and
    /// it exists for the reason its segmented-run predecessor did: the reader
    /// needs "you have this answer and cannot change it", not "you have no
    /// answer". Pinned because the alternative -- an `add_enabled_ui(false)`
    /// round the live control -- looks nearly the same in a screenshot while
    /// fading the answer with the box, and leaves a control egui will still
    /// let through on a re-layout.
    #[test]
    fn a_publish_in_flight_leaves_the_lifetime_control_inert_and_readable() {
        let mut plan = a_plan();
        let chosen = crate::vault_window::send_ui::words::lifetime_label(plan.lifetime, &UTC);
        let live = paint_access(&mut plan, true);
        let (_, live_fill) = live.control_under(&chosen);
        let painted = paint_access(&mut plan, false);

        assert!(
            painted.has(&chosen),
            "the lifetime in force is not on the card while a publish is running, so the \
             user cannot read back what they are publishing: {:?}",
            painted.text
        );
        let (_, fill) = painted.control_under(&chosen);
        assert_eq!(
            fill,
            crate::kit::tokens::Palette::LIGHT.list_bg,
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
        assert_eq!(views_note(None), no_view_limit_note());
        assert_eq!(views_note(Some(1)), view_limit_note());
        assert_ne!(no_view_limit_note(), view_limit_note());
    }

    /// **§5a's Access block is whole, with all three of its buildable
    /// rows.**
    ///
    /// The old text composer had a lifetime row and a password tick-box and
    /// no view cap at all; the block replaced all three. This says it is
    /// whole -- and the three absences at the bottom say it did not bring with
    /// it the three controls no server behind this app can honour.
    #[test]
    fn the_access_block_is_whole() {
        let mut plan = a_plan();
        let painted = paint_access(&mut plan, true);

        assert!(painted.has(access_eyebrow()), "the ACCESS eyebrow is missing: {:?}", painted.text);
        for label in [expires_label(), views_label(), open_with_label()] {
            assert!(painted.has(label), "the {label:?} row is missing: {:?}", painted.text);
        }
        assert!(
            painted.has(no_view_limit_note()),
            "the view box is empty and says nothing about what that means: {:?}",
            painted.text
        );
        assert!(
            painted.has(password_hint()),
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
                "{absent:?} is drawn in the Access block, and there is no server behind it"
            );
        }
    }

    /// **What is typed into the Access block reaches the plan that is
    /// published**, by the route a user actually takes.
    ///
    /// `draw_access_block` writes through three `&mut`s into the draft's own
    /// `SendPlan` fields, which `record_ui::send_plan_from` carries into the
    /// plan that is published -- there is no conversion step in between and
    /// this says so. The password
    /// half is the one that matters most: a control that drew, validated and
    /// then published `None` would ship a Send anyone with the link can open,
    /// with the user's own password visible in the box they typed it into.
    #[test]
    fn what_the_access_block_holds_is_what_would_be_published() {
        let mut plan = a_plan();
        plan.lifetime = crate::send::SendLifetime::Hours(1);
        plan.password = Some(zeroize::Zeroizing::new("share-pw-9271".to_string()));
        plan.max_access_count = Some(3);

        // A frame of the real block, which is where a block that dropped what
        // it was handed would do the dropping.
        let painted = paint_access(&mut plan, true);

        assert_eq!(plan.lifetime, crate::send::SendLifetime::Hours(1), "a frame reset the lifetime");
        assert_eq!(
            plan.password.as_deref().map(String::as_str),
            Some("share-pw-9271"),
            "a frame dropped the share password, so an open link would be published"
        );
        assert_eq!(plan.max_access_count, Some(3), "a frame dropped the view cap");
        assert_eq!(
            crate::send::validate_plan(&plan, &FixedClock(NOW)),
            None,
            "the draft the user typed is refused"
        );

        // And the sub-day lifetime is the one now on screen, spelled by the
        // one function that spells lifetimes.
        assert!(
            painted.has(&crate::vault_window::send_ui::words::lifetime_label(crate::send::SendLifetime::Hours(1), &UTC)),
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
        let mut plan = a_plan();
        plan.password = Some(zeroize::Zeroizing::new("share-pw-9271".to_string()));
        let painted = paint_access(&mut plan, true);
        assert!(painted.has(open_with_label()), "control: the password row did not draw");
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
            SendState::Opened => SendSummary { access_count: 2, ..base },
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
            state_tone(&crate::kit::tokens::Palette::LIGHT, SendState::Waiting).mark,
            theme::PillMark::Dot(_)
        ));
        assert!(matches!(
            state_tone(&crate::kit::tokens::Palette::LIGHT, SendState::Used).mark,
            theme::PillMark::Check(_)
        ));
        assert_eq!(state_tone(&crate::kit::tokens::Palette::LIGHT, SendState::Expired).mark, theme::PillMark::None);
        assert_eq!(state_tone(&crate::kit::tokens::Palette::LIGHT, SendState::Revoked).mark, theme::PillMark::None);
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

    /// A `SendRow` for the two assertions above.
    fn summary_row(name: &str, is_file: bool) -> SendRow {
        SendRow {
            sent: None,
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
            expires: no_expiry_of_its_own().to_string(),
            deletes: "17 Aug 2026, 00:00 \u{00b7} in 7 days".to_string(),
            extendable: true,
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
            receive_needs_the_cli().contains("command-line tool"),
            "the sentence does not name the thing that is missing: {receive_needs_the_cli}", receive_needs_the_cli = receive_needs_the_cli()
        );
        for kept in ["Publishing", "listing", "revoking"] {
            assert!(
                receive_needs_the_cli().contains(kept),
                "{kept:?} is not named as still working, so the sentence reads as `Sends need the \
                 CLI`: {receive_needs_the_cli}", receive_needs_the_cli = receive_needs_the_cli()
            );
        }
        // The control: the two sentences are not the same sentence, so an
        // assertion satisfied by either is satisfied by the right one.
        assert!(
            !file_send_explanation().contains("command-line"),
            "the file sentence and the receive sentence have merged"
        );
        assert!(
            !receive_needs_the_cli().contains("file"),
            "the receive sentence has picked up the file restriction"
        );
    }

    /// Empty and failed do not share a single word on screen.
    #[test]
    fn an_empty_account_and_a_failed_fetch_paint_different_words() {
        let size = min_pane_size();
        let (empty, _) = paint(&SendPaneState::Empty, None, size);
        assert!(empty.has(empty_headline()));
        assert!(!empty.has(failed_headline()));
        assert!(!empty.has("could not"), "the empty state hedged: {:?}", empty.text);

        let failed = SendPaneState::Failed {
            message: SendError::Offline.user_message().to_string(),
            ambiguous: false,
        };
        let (failed, _) = paint(&failed, None, size);
        assert!(failed.has(failed_headline()));
        assert!(
            !failed.has(empty_headline()),
            "a failed fetch told the user they have no Sends: {:?}",
            failed.text
        );
        assert!(failed.has(&SendError::Offline.user_message()));
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
        assert!(painted.has(ambiguous_detail()), "painted: {:?}", painted.text);
        assert!(!painted.has(empty_headline()));

        let plain = SendPaneState::Failed {
            message: SendError::Offline.user_message().to_string(),
            ambiguous: false,
        };
        let (plain, _) = paint(&plain, None, size);
        assert!(
            !plain.has(ambiguous_detail()),
            "an unambiguous failure claimed it might have missed some"
        );
    }

    /// Loading is not empty, on screen as well as in the enum.
    #[test]
    fn a_pane_that_has_not_been_answered_yet_says_so() {
        let (painted, _) = paint(&SendPaneState::Loading, None, min_pane_size());
        assert!(painted.has(loading_label()));
        assert!(!painted.has(empty_headline()));
        assert!(!painted.has(failed_headline()));
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

        let output = warm_run(&ctx, base, |ui| {
            let _ = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), &mut selected, RecordTiles::default(), &mut crate::vault_window::share_ui::PeopleRows::none()).into_action();
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        let targets: Vec<egui::Rect> = painted.rects_clickable(label);
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
            let _ = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), &mut selected, RecordTiles::default(), &mut crate::vault_window::share_ui::PeopleRows::none()).into_action();
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
            action = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), &mut selected, RecordTiles::default(), &mut crate::vault_window::share_ui::PeopleRows::none()).into_action();
        });
        action
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
            painted.has(failed_headline()),
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

    #[test]
    fn a_failure_can_be_retried_from_the_pane_itself() {
        let state = SendPaneState::Failed {
            message: SendError::Offline.user_message().to_string(),
            ambiguous: false,
        };
        assert_eq!(click_nth(&state, "Try again", 0), SendUiAction::Refresh);
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
        let output = warm_run(&ctx, base, |ui| {
            let _ = draw_send_pane(ui, &state, Some("a message"), SendDeleteView::default(), SendView::Mine(ALL_SENDS), &mut None, RecordTiles::default(), &mut crate::vault_window::share_ui::PeopleRows::none()).into_action();
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
            let _ = draw_send_pane(ui, &state, Some("a message"), SendDeleteView::default(), SendView::Mine(ALL_SENDS), &mut None, RecordTiles::default(), &mut crate::vault_window::share_ui::PeopleRows::none()).into_action();
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
            action = draw_send_pane(ui, &state, Some("a message"), SendDeleteView::default(), SendView::Mine(ALL_SENDS), &mut None, RecordTiles::default(), &mut crate::vault_window::share_ui::PeopleRows::none()).into_action();
        });
        assert_eq!(action, SendUiAction::DismissNotice);
    }

    // ---- the revoke affordance, step 4 -----------------------------------

    // ---- design 5b's Extend ------------------------------------------------

    /// **A Send that lasts forever is not offered Extend** -- the arithmetic
    /// leaves "never" where it is, so the press would report a week that
    /// never arrived -- and neither is one whose date this app cannot read.
    /// One with an expiry of its own is, whatever its deletion date says.
    #[test]
    fn only_a_send_with_an_end_this_app_can_read_is_offered_extend() {
        let send = |deletion: &str, expiration: &str| SendSummary {
            id: "id0".to_string(),
            name: "forever".to_string(),
            access_url: "https://send.bitwarden.com/#/0".to_string(),
            deletion_date: deletion.to_string(),
            is_file: false,
            max_access_count: None,
            access_count: 0,
            disabled: false,
            expiration_date: expiration.to_string(),
            has_password: false,
        };
        let never = crate::send::NEVER_DELETION_DATE;
        assert!(can_extend(&send("2026-08-17T00:00:00.000Z", "")));
        assert!(!can_extend(&send(never, "")), "a Send with no end was offered Extend");
        assert!(can_extend(&send(never, "2026-08-17T00:00:00.000Z")), "its expiry can move");
        assert!(!can_extend(&send("", "")), "an unreadable date was offered Extend");

        let forever = pane_state(Some(&Ok(vec![send(never, "")])), &FixedClock(NOW), &UTC);
        let (painted, _) = paint_with(&forever, None, min_pane_size(), SendDeleteView::default());
        assert_eq!(painted.count_exact(extend_label()), 0, "{:?}", painted.text);
        assert_eq!(painted.in_detail(revoke_label()), 1, "control: the modal is not up");
    }

    // ---- the records list's shape ----------------------------------------------

    /// A record as the vault holds one: a login with a username, so its row
    /// has two lines like a Send's, and its tile with no icon is its name's
    /// monogram.
    fn a_record(id: &str, name: &str) -> crate::vault_bridge::VaultItem {
        crate::vault_bridge::VaultItem {
            id: id.to_string(),
            name: name.to_string(),
            fields: Vec::new(),
            login: Some(crate::vault_bridge::LoginData {
                username: Some("dancefront".to_string()),
                password: None,
                totp: None,
                uris: Vec::new(),
                other: serde_json::Map::new(),
            }),
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

    /// One live Send, made from `record` and filed as such, as the Sent
    /// list's single row.
    fn a_send_made_from(record: &crate::vault_bridge::VaultItem) -> SendPaneState {
        let send = SendSummary {
            id: "send-of-the-record".to_string(),
            name: record.name.clone(),
            access_url: "https://send.bitwarden.com/#/r".to_string(),
            deletion_date: "2026-08-17T00:00:00.000Z".to_string(),
            is_file: false,
            max_access_count: None,
            access_count: 0,
            disabled: false,
            expiration_date: String::new(),
            has_password: false,
        };
        let mut history = crate::sent_history::SentHistory::default();
        history.record(crate::sent_history::SentRecord {
            kind: crate::sent_history::SentKind::Send,
            sent_at_unix_millis: NOW,
            send_id: send.id.clone(),
            recipient: "dancefront".to_string(),
            item_id: record.id.clone(),
            name: record.name.clone(),
            fields: Vec::new(),
        });
        with_sent_facts(pane_state(Some(&Ok(vec![send])), &FixedClock(NOW), &UTC), &history)
    }

    /// A 2x2 texture, because what is asserted is that the record's picture
    /// is painted and where, never what is in it.
    fn a_texture(ctx: &egui::Context) -> egui::TextureHandle {
        ctx.load_texture(
            "the-records-icon",
            egui::ColorImage::from_rgba_unmultiplied([2, 2], &[255u8; 16]),
            egui::TextureOptions::default(),
        )
    }

    /// The Sent screen, with the first row picked, drawn with `records` for
    /// its tiles.
    fn paint_sent_with(
        state: &SendPaneState,
        size: egui::Vec2,
        records: impl Fn(&egui::Context) -> (Vec<crate::vault_bridge::VaultItem>, Option<egui::TextureHandle>),
    ) -> Painted {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});
        let (items, texture) = records(&ctx);
        let icons: std::collections::HashMap<String, egui::TextureHandle> = texture
            .into_iter()
            .flat_map(|t| items.iter().map(move |item| (item.id.clone(), t.clone())))
            .collect();
        let mut selected = first_row_id(state);
        let output = warm_run(&ctx, input, |ui| {
            let _ = draw_send_pane(
                ui,
                state,
                None,
                SendDeleteView::default(),
                SendView::Mine(ALL_SENDS),
                &mut selected,
                RecordTiles { items: &items, icons: Some(&icons) },
                &mut crate::vault_window::share_ui::PeopleRows::none(),
            )
            .into_action();
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        painted
    }

    /// The two steps do not share a word, so the second click is a decision
    /// rather than muscle memory.
    #[test]
    fn the_two_steps_are_not_labelled_the_same_thing() {
        assert_ne!(revoke_label(), confirm_label());
        assert_ne!(cancel_label(), confirm_label());
        assert!(
            confirm_label().len() > revoke_label().len(),
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
        paint_picking(&state, None, TALL_WINDOW, SendDeleteView::default(), Some(id))
    }

    /// A window as wide as the app's minimum and tall enough for the whole
    /// modal, for the assertions about what the modal SAYS rather than where
    /// it scrolls.
    const TALL_WINDOW: egui::Vec2 = egui::vec2(688.0, 1300.0);

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
                SendState::Opened => SendSummary { access_count: 2, ..base.clone() },
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
        let output = warm_run(&ctx, input, |ui| {
            draw_send_pane(
                ui,
                state,
                None,
                SendDeleteView::default(),
                SendView::Mine(scope),
                &mut selected,
                RecordTiles::default(),
                &mut crate::vault_window::share_ui::PeopleRows::none(),
            )
            .into_action();
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        painted
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
                !painted.has(empty_headline()),
                "an empty {scope:?} row told the user their ACCOUNT has no Sends, which is \
                 false: {:?}",
                painted.text
            );
            assert!(
                painted.has(scope_empty_detail()),
                "the empty row does not say where the rest of the Sends are: {:?}",
                painted.text
            );
        }
        // The control: the same account, unfiltered, has a Send.
        let all = paint_scope(&state, SendScope::All);
        assert!(!all.has(empty_headline()), "control: the fixture account really is empty");
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
        paint_received_with(rows, &[], picked)
    }

    /// [`paint_received`] with records people share with this account
    /// (mock 6d's FROM PEOPLE).
    fn paint_received_with(
        rows: &[ReceivedRow],
        from_people: &[crate::vault_window::share_ui::ReceivedShareRow],
        picked: Option<String>,
    ) -> Painted {
        let mut people =
            crate::vault_window::share_ui::PeopleRows { received: from_people, ..crate::vault_window::share_ui::PeopleRows::none() };
        paint_received_people(rows, &mut people, picked).0
    }

    /// [`paint_received`] with every sharing input the screen takes -- 6d's
    /// invitations and waiting sharers too -- and the selection it ended on.
    fn paint_received_people(
        rows: &[ReceivedRow],
        people: &mut crate::vault_window::share_ui::PeopleRows<'_>,
        picked: Option<String>,
    ) -> (Painted, Option<String>) {
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
        let output = warm_run(&ctx, input, |ui| {
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
                RecordTiles::default(),
                people,
            )
            .into_action();
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        (painted, selected)
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
        assert!(painted.has(received_empty_headline()), "{:?}", painted.text);
        assert!(painted.has(received_empty_detail()), "{:?}", painted.text);
        assert!(
            !painted.has(empty_headline()),
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
            (SendSummary { access_count: 1, max_access_count: None, ..base.clone() }, SendState::Opened, "Opened once"),
            (SendSummary { access_count: 3, max_access_count: Some(10), ..base.clone() }, SendState::Opened, "7 views left"),
            (SendSummary { access_count: 1, max_access_count: Some(1), ..base.clone() }, SendState::Used, "spent"),
            (SendSummary { disabled: true, ..base.clone() }, SendState::Revoked, "switched off"),
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
            let live = painted.control_under(waiting_label()).1;
            let ended = painted.control_under(revoked_label()).1;
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

    // ---- person shares in the Sent list (design 6c) -------------------------------

    use crate::rest::sharing::PersonState;
    use crate::vault_window::share_ui::{self, PeopleAsk, PeopleRows, ShareRow};

    /// `record`'s share with `email`, as `share_ui::share_rows` would list it.
    fn a_share_of(record: &crate::vault_bridge::VaultItem, email: &str, state: PersonState) -> ShareRow {
        ShareRow {
            key: share_ui::share_key(&record.id, email),
            item_id: record.id.clone(),
            name: record.name.clone(),
            email: email.to_string(),
            state,
            subtitle: format!("to {email} \u{b7} Shared"),
            since: Some("14 Sep".to_string()),
        }
    }

    /// The Sent screen with `people` handed to it and `selected` picked, drawn
    /// with `records` for its tiles, over a warm frame and a plain one before
    /// the frame that carries `events` -- a press is hit-tested against the
    /// widgets of the frame before it. `people.asked` is what that last frame
    /// asked.
    fn paint_sharing(
        state: &SendPaneState,
        people: &mut PeopleRows<'_>,
        selected: &mut Option<String>,
        records: &[crate::vault_bridge::VaultItem],
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> (Painted, SendUiAction) {
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let tiles = RecordTiles { items: records, icons: None };
        let _ = warm_run(&ctx, || input(Vec::new()), |ui| {
            let _ = draw_send_pane(ui, state, None, SendDeleteView::default(), SendView::Mine(ALL_SENDS), selected, tiles, people)
                .into_action();
        });
        people.asked = None;
        let mut action = SendUiAction::None;
        let output = ctx.run_ui(input(events), |ui| {
            action = draw_send_pane(ui, state, None, SendDeleteView::default(), SendView::Mine(ALL_SENDS), selected, tiles, people)
                .into_action();
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        (painted, action)
    }

    fn press_at(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ]
    }


    // ====================================================================
    // The Shared page (spec A 4) and the Send detail modal (spec B 7)
    //
    // The page is a grid of cards; a card opens a modal. `in_list` counts
    // what the PAGE painted and `in_detail` what the MODAL painted (see
    // `Painted`), so the old column vocabulary still reads true: a picked
    // Send's words are in "the detail".
    // ====================================================================

    /// The card (a white 18-radius rect) a name sits on, on the page.
    fn card_of(painted: &Painted, name: &str) -> egui::Rect {
        let text = painted
            .rects_of_exact(name)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("{name:?} was not painted: {:?}", painted.text));
        painted
            .fills
            .iter()
            .filter(|(r, fill)| *fill == crate::kit::tokens::Palette::LIGHT.card && r.width() > 200.0 && r.contains_rect(text))
            .map(|(r, _)| *r)
            .min_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
            .unwrap_or_else(|| panic!("{name:?} is on no card"))
    }

    /// [`click_nth_picking`], also answering the selection the click left.
    fn click_picking_sel(
        state: &SendPaneState,
        delete: SendDeleteView<'_>,
        picked: Option<String>,
        label: &str,
        nth: usize,
    ) -> (SendUiAction, Option<String>) {
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
        let draw = |ui: &mut egui::Ui, selected: &mut Option<String>| {
            draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), selected, RecordTiles::default(), &mut crate::vault_window::share_ui::PeopleRows::none()).into_action()
        };
        let output = warm_run(&ctx, base, |ui| {
            let _ = draw(ui, &mut selected);
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        let targets = painted.rects_clickable(label);
        assert!(targets.len() > nth, "only {} widgets labelled {label:?} were painted: {:?}", targets.len(), painted.text);
        let pos = targets[nth].center();
        let events = press_at(pos);
        let mut action = SendUiAction::None;
        let _ = ctx.run_ui(egui::RawInput { events: events[..2].to_vec(), ..base() }, |ui| {
            let _ = draw(ui, &mut selected);
        });
        let _ = ctx.run_ui(egui::RawInput { events: events[2..].to_vec(), ..base() }, |ui| {
            action = draw(ui, &mut selected);
        });
        (action, selected)
    }

    #[test]
    fn each_state_paints_its_own_pill_on_its_card_and_on_its_modal() {
        use crate::send::SendState;
        for state in [SendState::Waiting, SendState::Used, SendState::Expired, SendState::Revoked] {
            let word = state_label(state);
            let tone = state_tone(&crate::kit::tokens::Palette::LIGHT, state);
            let (painted, _) = paint(&one_row(state), None, min_pane_size());
            assert_eq!(painted.in_list(word), 1, "{state:?}: the card's pill: {:?}", painted.text);
            assert_eq!(painted.in_detail(word), 1, "{state:?}: the modal header's pill: {:?}", painted.text);
            let (rect, fill) = painted.control_under(word);
            assert_eq!(fill.a(), tone.fill.a().min(fill.a()), "control: {fill:?}");
            let word_rect = painted.rects_of_exact(word)[0];
            // Spec A 4: 13 bold, padding `5 11`, fully round.
            assert!(
                (rect.width() - word_rect.width() - 22.0).abs() < 4.0,
                "the {state:?} pill is {} wide around a {} word: not the design's 11 either side",
                rect.width(),
                word_rect.width()
            );
            assert!((24.0..=28.0).contains(&rect.height()), "the {state:?} pill is {} tall", rect.height());
        }
        // Opened is its own state and its own colours (a live link, seen).
        let painted = paint(&one_row(SendState::Opened), None, min_pane_size()).0;
        assert_eq!(painted.in_list(opened_label()), 1, "{:?}", painted.text);
        assert_eq!(painted.in_list(waiting_label()), 0, "an opened link still says Waiting");
        assert_ne!(
            state_tone(&crate::kit::tokens::Palette::LIGHT, SendState::Opened).fill,
            state_tone(&crate::kit::tokens::Palette::LIGHT, SendState::Waiting).fill
        );
    }

    #[test]
    fn a_cards_pill_sits_inside_its_card_and_clear_of_its_name() {
        let state = one_row(crate::send::SendState::Waiting);
        let (painted, _) = paint(&state, None, min_pane_size());
        let card = card_of(&painted, "SAP Production");
        let (pill, _) = painted.control_under(waiting_label());
        let name = painted.rects_of_exact("SAP Production")[0];
        assert!(card.contains_rect(pill), "the pill leaves its card: {pill:?} in {card:?}");
        assert!(pill.left() >= name.right(), "the name ends at {} and the pill starts at {}", name.right(), pill.left());
        assert!(
            card.right() - pill.right() >= 20.0,
            "the pill is {}pt from the card's edge: it shares the line with the chevron, 20pt in",
            card.right() - pill.right()
        );
    }

    #[test]
    fn the_page_lists_every_send_as_a_card_and_says_which_are_files() {
        let state = rows(6);
        let (painted, _) = paint_picking(&state, None, min_pane_size(), SendDeleteView::default(), None);
        for i in 0..6 {
            assert_eq!(painted.in_list(&format!("send-number-{i}")), 1, "card {i}: {:?}", painted.text);
        }
        assert_eq!(painted.count("File Send"), 3, "the file Sends say so on their card: {:?}", painted.text);
        // Two to a row at this width, and the cards of one row share a top.
        let a = card_of(&painted, "send-number-0");
        let b = card_of(&painted, "send-number-1");
        let c = card_of(&painted, "send-number-2");
        assert!((a.top() - b.top()).abs() < 0.5 && b.left() > a.right(), "{a:?} {b:?}");
        assert!(c.top() > a.bottom(), "the third card starts a new row: {a:?} {c:?}");
        assert!((b.left() - a.right() - 16.0).abs() < 0.5, "the grid's gap is 16: {a:?} {b:?}");
        assert!(painted.has(shared_page::title()), "the page's title");
        assert!(painted.has(shared_page::sub()), "the page's sub-line");
    }

    #[test]
    fn a_live_card_offers_copy_link_and_revoke_and_an_ended_one_offers_neither() {
        let live = paint_picking(&one_row(crate::send::SendState::Waiting), None, min_pane_size(), SendDeleteView::default(), None).0;
        assert_eq!(live.count_exact(copy_link_label()), 1, "{:?}", live.text);
        assert_eq!(live.count_exact(revoke_label()), 1, "{:?}", live.text);
        for state in [crate::send::SendState::Used, crate::send::SendState::Expired, crate::send::SendState::Revoked] {
            let ended = paint_picking(&one_row(state), None, min_pane_size(), SendDeleteView::default(), None).0;
            assert_eq!(ended.count_exact(copy_link_label()), 0, "{state:?} offers Copy link on its card");
            assert_eq!(ended.count_exact(revoke_label()), 0, "{state:?} offers Revoke on its card");
        }
    }

    #[test]
    fn a_cards_copy_link_reports_the_url_of_its_own_send() {
        let state = rows(6);
        let SendPaneState::Rows(model) = &state else { panic!("not rows") };
        for index in [0usize, 1] {
            let (action, selected) =
                click_picking_sel(&state, SendDeleteView::default(), None, copy_link_label(), index);
            assert_eq!(action, SendUiAction::CopyLink(model[index].access_url.clone()), "card {index}");
            assert_eq!(selected, None, "Copy link does not also open the modal");
        }
    }

    #[test]
    fn a_cards_revoke_asks_about_that_send_and_opens_its_modal_for_the_answer() {
        let state = rows(4);
        for index in [0usize, 1] {
            let (action, selected) = click_picking_sel(&state, SendDeleteView::default(), None, revoke_label(), index);
            assert_eq!(action, SendUiAction::AskDelete(format!("id{index}")));
            assert_eq!(selected, Some(format!("id{index}")), "the question is asked in the modal");
        }
    }

    #[test]
    fn pressing_a_card_opens_the_modal_of_that_send() {
        let state = rows(4);
        let (action, selected) = click_picking_sel(&state, SendDeleteView::default(), None, "send-number-2", 0);
        assert_eq!(action, SendUiAction::None);
        assert_eq!(selected.as_deref(), Some("id2"));
        let (painted, _) = paint_picking(&state, None, min_pane_size(), SendDeleteView::default(), selected);
        assert_eq!(painted.in_detail("send-number-2"), 1, "the modal names the Send: {:?}", painted.text);
        assert_eq!(painted.in_detail("send-number-0"), 0);
    }

    #[test]
    fn a_card_whose_url_is_empty_copies_nothing() {
        let state = SendPaneState::Rows(vec![SendRow {
            sent: None,
            id: "id-no-url".into(),
            name: "no url".into(),
            expiry: "Expires in 7 days".into(),
            is_file: false,
            access_url: String::new(),
            state: crate::send::SendState::Waiting,
            access_count: 0,
            max_access_count: None,
            has_password: false,
            activity: "Nobody has opened this link. The link works, as often as they like.".to_string(),
            expires: no_expiry_of_its_own().to_string(),
            deletes: "17 Aug 2026, 00:00 \u{00b7} in 7 days".to_string(),
            extendable: true,
        }]);
        assert_eq!(
            click_picking_sel(&state, SendDeleteView::default(), None, copy_link_label(), 0).0,
            SendUiAction::None,
            "a Send with no link put an empty string on the clipboard"
        );
        let (painted, _) = paint(&state, None, min_pane_size());
        assert!(painted.has(address_missing()), "the missing link is not said: {:?}", painted.text);
        assert_eq!(painted.in_detail(copy_link_label()), 1, "control: the modal was not drawn");
    }

    #[test]
    fn a_send_with_no_id_asks_to_revoke_nothing() {
        let state = SendPaneState::Rows(vec![SendRow {
            sent: None,
            id: String::new(),
            name: "no id".into(),
            expiry: "Expires in 7 days".into(),
            is_file: false,
            access_url: "https://send.bitwarden.com/#/x".into(),
            state: crate::send::SendState::Waiting,
            access_count: 0,
            max_access_count: None,
            has_password: false,
            activity: "Nobody has opened this link. The link works, as often as they like.".to_string(),
            expires: no_expiry_of_its_own().to_string(),
            deletes: "17 Aug 2026, 00:00 \u{00b7} in 7 days".to_string(),
            extendable: true,
        }]);
        let (painted, _) = paint_picking(&state, None, min_pane_size(), SendDeleteView::default(), None);
        assert_eq!(painted.count_exact(revoke_label()), 0, "a card with no id offered Revoke: {:?}", painted.text);
        assert_eq!(painted.count_exact(copy_link_label()), 0, "a card with no id offered actions");
    }

    #[test]
    fn the_modal_copies_the_link_of_the_send_it_is_showing() {
        let state = rows(6);
        let SendPaneState::Rows(model) = &state else { panic!("not rows") };
        for index in [0usize, 3, 5] {
            let expected = model[index].access_url.clone();
            assert_eq!(
                click_nth_picking(&state, SendDeleteView::default(), Some(model[index].id.clone()), copy_link_label(), 0),
                SendUiAction::CopyLink(expected.clone()),
                "Copy link did not report {expected} while row {index} was picked"
            );
        }
    }

    #[test]
    fn a_picked_send_can_be_revoked_and_the_first_click_only_asks() {
        let state = rows(6);
        let (painted, _) = paint_with(&state, None, min_pane_size(), SendDeleteView::default());
        assert_eq!(painted.in_detail(revoke_label()), 1, "the picked Send's modal has no Revoke: {:?}", painted.text);
        assert!(!painted.has(confirm_label()), "the destructive button is painted before anything was asked");
        for nth in [0usize, 5] {
            let action =
                click_nth_picking(&state, SendDeleteView::default(), Some(format!("id{nth}")), revoke_label(), 0);
            assert_eq!(action, SendUiAction::AskDelete(format!("id{nth}")), "row {nth}");
        }
    }

    #[test]
    fn only_the_send_asked_about_shows_the_confirmation() {
        let state = rows(4);
        let armed = SendDeleteView { confirming: Some("id2"), in_flight: None, extending: false };
        let (painted, _) = paint_picking(&state, None, min_pane_size(), armed, Some("id2".to_string()));
        assert_eq!(painted.count_exact(confirm_label()), 1, "{:?}", painted.text);
        assert_eq!(painted.count_exact(cancel_label()), 1, "the way out of the confirmation is not painted once");
        assert_eq!(painted.in_detail(revoke_label()), 0, "the armed Send kept its Revoke beside the question");
        assert!(painted.has(confirm_prompt()), "the question is not asked: {:?}", painted.text);

        let (other, _) = paint_picking(&state, None, min_pane_size(), armed, Some("id0".to_string()));
        assert_eq!(other.count_exact(confirm_label()) + other.count_exact(cancel_label()), 0, "id2's question is over id0");
        assert_eq!(other.in_detail(revoke_label()), 1, "the Send that is NOT being asked about lost its Revoke");
        assert!(!other.has(confirm_prompt()), "the question followed the selection");

        assert_eq!(
            click_nth_picking(&state, armed, Some("id2".to_string()), confirm_label(), 0),
            SendUiAction::ConfirmDelete { id: "id2".to_string(), name: "send-number-2".to_string() },
            "the confirmation answered for a Send other than the one it was asked about"
        );
    }

    #[test]
    fn a_send_being_revoked_says_so_on_its_card_and_its_modal_and_has_no_buttons() {
        let state = rows(3);
        let busy = SendDeleteView { confirming: None, in_flight: Some("id0"), extending: false };
        let (painted, _) = paint_with(&state, None, min_pane_size(), busy);
        assert_eq!(painted.in_detail(deleting_label()), 1, "the modal does not say anything is happening: {:?}", painted.text);
        assert_eq!(painted.in_list(deleting_label()), 1, "the card of the Send being revoked does not say so: {:?}", painted.text);
        for label in [revoke_label(), open_record_label(), confirm_label(), extend_label()] {
            assert_eq!(painted.in_detail(label), 0, "{label:?} is offered on a Send already being revoked");
        }
        let card = card_of(&painted, "send-number-0");
        assert!(
            !painted.rects_of_exact(revoke_label()).iter().any(|r| card.contains_rect(*r)),
            "the busy card kept its Revoke"
        );
        assert_eq!(painted.in_list("Expires in"), 0, "the card printed its expiry BESIDE the progress word");
    }

    #[test]
    fn extend_reports_the_send_it_was_pressed_on_in_one_step() {
        let state = rows(3);
        let (painted, _) = paint_with(&state, None, min_pane_size(), SendDeleteView::default());
        assert_eq!(painted.in_detail(extend_label()), 1, "no Extend on the modal: {:?}", painted.text);
        assert_eq!(painted.in_list(extend_label()), 0, "Extend leaked onto the cards");
        assert_eq!(
            click_nth_with(&state, SendDeleteView::default(), extend_label(), 0),
            SendUiAction::Extend { id: "id0".to_string(), name: "send-number-0".to_string() }
        );
    }

    #[test]
    fn extend_steps_back_for_the_question_and_a_busy_send_says_extending() {
        let state = rows(3);
        let armed = SendDeleteView { confirming: Some("id0"), in_flight: None, extending: false };
        let (asking, _) = paint_with(&state, None, min_pane_size(), armed);
        assert_eq!(asking.in_detail(extend_label()), 0, "Extend beside a question about destroying");
        assert_eq!(asking.count_exact(cancel_label()), 1, "control: the question is not up");

        let busy = SendDeleteView { confirming: None, in_flight: Some("id0"), extending: true };
        let (painted, _) = paint_with(&state, None, min_pane_size(), busy);
        assert_eq!(painted.in_detail(extending_label()), 1, "{:?}", painted.text);
        assert_eq!(painted.in_list(extending_label()), 1, "{:?}", painted.text);
        assert_eq!(painted.count_exact(deleting_label()), 0, "an Extend is described as a revoke");
        for label in [extend_label(), revoke_label(), open_record_label()] {
            assert_eq!(painted.in_detail(label), 0, "{label:?} is offered while the Extend runs");
        }
    }

    #[test]
    fn the_modal_says_what_the_link_is_and_what_has_happened_to_it() {
        let (painted, _) = detail_of(a_fully_described_send());
        for (label, times) in [(address_row(), 1), (opens_with_row(), 1), (views_row(), 1), (expires_row(), 1), (deleted_row(), 1), ("Opens", 1), ("Link", 1), ("Includes", 1), ("Activity", 1)] {
            assert_eq!(painted.in_detail(label), times, "the modal does not draw {label:?} {times} time(s): {:?}", painted.text);
        }
        assert!(painted.has("https://send.bitwarden.com/#/g7HqK2"), "the link: {:?}", painted.text);
        assert_eq!(painted.count_exact(password_segment()), 1, "a password Send does not say so: {:?}", painted.text);
        assert!(painted.has("3 of 10"), "the count against its cap: {:?}", painted.text);
        let expires = painted.text.iter().find(|t| t.contains("17 Aug 2026")).expect("an expiry date");
        let deletes = painted.text.iter().find(|t| t.contains("20 Aug 2026")).expect("a deletion date");
        assert_ne!(expires, deletes, "the Expires and Deleted rows print one date, so one of the two is not being read");
        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, TALL_WINDOW);
        for (text, rect) in painted.dialog_text_rects() {
            assert!(
                pane.contains_rect(rect.shrink(0.5)),
                "{text:?} was painted at {rect:?}, outside the {pane:?} window at the minimum size"
            );
        }
    }

    #[test]
    fn the_opens_with_row_says_what_the_recipient_needs() {
        let (with, _) = detail_of(a_fully_described_send());
        assert_eq!(with.count_exact(password_segment()), 1);
        assert_eq!(with.count_exact(link_only()), 0);
        let (without, _) = detail_of(SendSummary { has_password: false, ..a_fully_described_send() });
        assert_eq!(without.count_exact(link_only()), 1, "{:?}", without.text);
        assert_eq!(without.count_exact(password_segment()), 0);
    }

    #[test]
    fn a_dead_links_address_is_struck_through_and_marked() {
        let (live, _) = detail_of(a_fully_described_send());
        assert_eq!(live.count_exact(dead_marker()), 0, "a live link is marked dead: {:?}", live.text);
        let used = SendSummary { access_count: 10, ..a_fully_described_send() };
        assert_eq!(crate::send::send_state(&used, &FixedClock(NOW)), crate::send::SendState::Used, "control");
        let (painted, _) = detail_of(used);
        assert_eq!(painted.count_exact(dead_marker()), 1, "a spent Send still presents its link as working: {:?}", painted.text);
        assert!(
            painted.struck.iter().any(|t| t.starts_with("https://send.bitwarden.com")),
            "nothing is struck through the dead address: {:?}",
            painted.struck
        );
        assert!(live.struck.is_empty(), "a live link is struck through: {:?}", live.struck);
    }

    #[test]
    fn the_activity_answers_whether_it_was_used_and_invents_no_timeline() {
        let (painted, _) = detail_of(a_fully_described_send());
        assert_eq!(painted.in_detail("Activity"), 1, "no Activity: {:?}", painted.text);
        for absent in ["Berlin", "Edge", "Password revealed"] {
            assert!(!painted.has(absent), "the modal paints {absent:?}: a Bitwarden Send keeps no per-open record");
        }
        assert!(painted.has("Opened 3 times"), "{:?}", painted.text);
        assert!(painted.has("7 views left"), "{:?}", painted.text);
        let SendPaneState::Rows(rows) = pane_state(Some(&Ok(vec![a_fully_described_send()])), &FixedClock(NOW), &UTC) else {
            panic!("not rows")
        };
        let events = shared_page::activity_events(&rows[0]);
        assert_eq!(events.first().map(|e| e.0.as_str()), Some("Created by you"));
        assert!(events.iter().any(|e| e.0 == "Opened 3 times" && e.1 == "3 of 10 opens"), "{events:?}");
        assert!(events.last().is_some_and(|e| e.0 == "The link stops working"), "{events:?}");
    }

    #[test]
    fn an_opened_password_send_is_cautioned_in_the_modal_and_an_untouched_one_is_not() {
        let with_password = |state| match one_row(state) {
            SendPaneState::Rows(mut rows) => {
                rows[0].sent = Some(SentFacts { recipient: String::new(), item_id: "item-1".into(), fields: vec!["Password".into()] });
                SendPaneState::Rows(rows)
            }
            other => other,
        };
        for size in [roomy_pane_size(), min_pane_size()] {
            let mut window = Session::new(size, Some("id0".to_string()));
            let painted = window.settle(&with_password(crate::send::SendState::Opened), SendDeleteView::default(), &mut PeopleRows::none(), &[]);
            assert_eq!(painted.in_detail(seen_title()), 1, "at {}pt: {:?}", size.x, painted.text);
            let text = painted.dialog_rect_of(seen_title());
            let (band, fill) = painted.fill_behind(text).expect("the caution has no band behind it");
            assert_eq!(fill, crate::kit::tokens::Palette::LIGHT.caution_wash, "the caution is not on the amber ground");
            assert!(band.right() <= size.x + 1.0 && band.left() >= 0.0, "{band:?}");
            assert_eq!(
                click_nth_picking(&with_password(crate::send::SendState::Opened), SendDeleteView::default(), Some("id0".into()), change_password(), 0),
                SendUiAction::ChangePassword("item-1".into()),
                "Change password does not open the record's edit"
            );
            let (quiet, _) = paint(&with_password(crate::send::SendState::Waiting), None, size);
            assert!(!quiet.has(seen_title()), "a link nobody has opened is warned about anyway");
        }
    }

    #[test]
    fn a_long_name_ends_in_an_ellipsis_on_the_card_and_in_the_modal() {
        let send = SendSummary {
            id: "id0".to_string(),
            name: "Remote Desktop \u{2014} Bastion \u{2014} contractor access, third quarter, and then some more words".to_string(),
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
            let names: Vec<_> = painted.text_rects.iter().filter(|(t, _)| t.starts_with("Remote Desktop")).collect();
            assert!(names.len() >= 2, "the card and the modal both name it: {:?}", painted.text);
            for (text, rect) in names {
                assert!(rect.right() <= size.x + 1.0, "at {}pt the name runs past the edge: {text:?} at {rect:?}", size.x);
                // Laid out to fit (with its ellipsis), not painted full width
                // and clipped: the whole name at 17pt bold is wider than this.
                assert!(rect.width() < 520.0, "at {}pt the name was not shortened: {text:?} at {rect:?}", size.x);
            }
        }
    }

    #[test]
    fn nothing_picked_draws_no_modal_and_a_stale_pick_draws_none_either() {
        let state = rows(3);
        let (painted, _) = paint_picking(&state, None, min_pane_size(), SendDeleteView::default(), None);
        assert_eq!(painted.in_detail(revoke_label()) + painted.in_detail(extend_label()), 0, "a modal with nothing picked");
        assert_eq!(painted.dialog_text_rects().count(), 0, "{:?}", painted.text);
        let (stale, _) = paint_picking(&state, None, min_pane_size(), SendDeleteView::default(), Some("id-that-is-gone".to_string()));
        assert_eq!(stale.dialog_text_rects().count(), 0, "a selection naming a Send that is gone draws a modal: {:?}", stale.text);
    }

    #[test]
    fn a_modal_that_is_taller_than_the_window_scrolls_and_keeps_its_buttons_on_screen() {
        let send = SendSummary { access_count: 3, ..a_fully_described_send() };
        let state = pane_state(Some(&Ok(vec![send])), &FixedClock(NOW), &UTC);
        let size = egui::vec2(688.0, 430.0);
        let (painted, _) = paint_picking(&state, None, size, SendDeleteView::default(), Some("id-detail".to_string()));
        let pane = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        for label in [revoke_label(), copy_link_label(), extend_label()] {
            let at = painted.dialog_rect_of(label);
            assert!(pane.contains_rect(at), "{label:?} at {at:?} is off a {size:?} window");
        }
    }

    #[test]
    fn a_send_made_from_a_record_with_an_icon_wears_it_on_its_card_and_its_modal() {
        let size = min_pane_size();
        let record = a_record("rec-1", "23000268.xyz");
        let state = a_send_made_from(&record);
        let with_icon = paint_sent_with(&state, size, |ctx| (vec![record.clone()], Some(a_texture(ctx))));
        assert!(with_icon.images.len() >= 2, "the card's and the modal's tiles hold no picture: {:?}", with_icon.images);
        assert!(
            !with_icon.text.iter().any(|t| t == &crate::vault_window::item_list::initials(&record.name)),
            "the monogram was painted as well as the icon: {:?}",
            with_icon.text
        );
        let without = paint_sent_with(&state, size, |_| (vec![record.clone()], None));
        assert!(without.images.is_empty(), "a picture with no icon: {:?}", without.images);
        assert!(
            without.text.iter().any(|t| t == &crate::vault_window::item_list::initials(&record.name)),
            "no monogram when the record has no icon: {:?}",
            without.text
        );
    }

    #[test]
    fn the_standing_notes_sit_below_the_cards_and_a_file_is_explained_only_when_listed() {
        let state = SendPaneState::Rows(vec![summary_row("a text Send", false), summary_row("report.pdf", true)]);
        let size = egui::vec2(688.0, 900.0);
        let (painted, _) = paint_picking(&state, None, size, SendDeleteView::default(), None);
        let note = painted.rect_of(scope_subtext()).unwrap_or_else(|| panic!("the standing note vanished: {:?}", painted.text));
        let card = card_of(&painted, "report.pdf");
        assert!(note.top() > card.bottom(), "the note is above the cards: {note:?} {card:?}");
        assert!(painted.has(file_send_explanation()), "a file Send was listed with nothing saying what it means");
        let text_only = SendPaneState::Rows(vec![summary_row("a text Send", false)]);
        let (painted, _) = paint_picking(&text_only, None, size, SendDeleteView::default(), None);
        assert!(!painted.has(file_send_explanation()), "a list of text Sends was told about a restriction it has not met");
        assert!(painted.has(scope_subtext()));
    }

    #[test]
    fn each_sharing_sub_filter_lists_exactly_what_its_badge_counts() {
        use crate::send::SendState;
        use crate::vault_window::sidebar::SendScope;
        let state = one_of_each_state();
        for (scope, expected) in [
            (SendScope::All, vec!["send-waiting", "send-used", "send-expired", "send-revoked"]),
            (SendScope::Waiting, vec!["send-waiting"]),
            (SendScope::Used, vec!["send-used"]),
            (SendScope::Ended, vec!["send-expired", "send-revoked"]),
        ] {
            let painted = paint_scope(&state, scope);
            for name in ["send-waiting", "send-used", "send-expired", "send-revoked"] {
                let wanted = expected.contains(&name);
                assert_eq!(painted.in_list(name) == 1, wanted, "under {scope:?} {name:?}: {:?}", painted.text);
            }
            let title = if scope == SendScope::All { shared_page::title() } else { scope.label() };
            assert!(painted.has(title), "the page does not name the {scope:?} cut it shows: {:?}", painted.text);
        }
        let counts = crate::vault_window::sidebar::SendCounts::over([SendState::Waiting, SendState::Used, SendState::Expired, SendState::Revoked]);
        assert_eq!((counts.waiting, counts.used, counts.ended, counts.all), (1, 1, 2, 4));
    }

    // ---- person shares -----------------------------------------------------

    #[test]
    fn a_share_is_a_card_listed_before_the_sends_and_wears_a_share_pill() {
        let record = a_record("rec-1", "SAP Production");
        let shares = [a_share_of(&record, "anna@x.io", PersonState::Shared)];
        let mut people = PeopleRows { shares: &shares, ..PeopleRows::none() };
        let size = min_pane_size();
        let (painted, _) = paint_sharing(&a_send_made_from(&record), &mut people, &mut None, std::slice::from_ref(&record), size, Vec::new());
        let cards: Vec<egui::Rect> = painted
            .text_rects
            .iter()
            .filter(|(t, _)| t == &record.name)
            .map(|(_, r)| *r)
            .collect();
        assert_eq!(cards.len(), 2, "one share and one Send: {:?}", painted.text);
        assert!(cards[0].top() <= cards[1].top() && cards[0].left() < cards[1].left(), "the share leads: {cards:?}");
        assert_eq!(painted.in_list("to anna@x.io \u{b7} Shared"), 1, "{:?}", painted.text);
        let pill = crate::kit::rows::share_tone(&crate::kit::tokens::Palette::LIGHT, theme::SharePillKind::Shared).0;
        let share_card = card_of(&painted, &record.name);
        assert!(
            painted.fills.iter().any(|(r, f)| *f == pill.fill && share_card.contains_rect(*r)),
            "the share card wears no share pill"
        );
        assert!(painted.has("Since 14 Sep"), "{:?}", painted.text);
    }

    #[test]
    fn a_shares_modal_shows_the_person_and_stops_in_two_steps() {
        let size = min_pane_size();
        let record = a_record("rec-1", "Acme VPN");
        let state = a_send_made_from(&record);
        let shares = [a_share_of(&record, "anna@x.io", PersonState::Shared)];
        let key = shares[0].key.clone();
        let records = std::slice::from_ref(&record);
        let mut people = PeopleRows { shares: &shares, ..PeopleRows::none() };

        let mut selected = None;
        let (painted, _) = paint_sharing(&state, &mut people, &mut selected, records, size, Vec::new());
        let card = card_of(&painted, "Acme VPN");
        let _ = paint_sharing(&state, &mut people, &mut selected, records, size, press_at(card.center_top() + egui::vec2(0.0, 30.0)));
        assert_eq!(selected.as_deref(), Some(key.as_str()), "the share card did not pick itself");

        let (painted, _) = paint_sharing(&state, &mut people, &mut selected, records, size, Vec::new());
        for words in [
            share_ui::shared_with_a_person(),
            share_ui::view_only(),
            share_ui::follows_edits(),
            share_ui::whole_record(),
            "anna@x.io",
            "14 Sep",
            open_record_label(),
            share_ui::stop_label(),
        ] {
            assert_eq!(painted.in_detail(words), 1, "{words:?} in the modal: {:?}", painted.text);
        }
        assert_eq!(painted.in_detail(revoke_label()), 0, "a share has no link to revoke");

        let open = painted.dialog_rect_of(open_record_label()).center();
        let (_, action) = paint_sharing(&state, &mut people, &mut selected, records, size, press_at(open));
        assert_eq!(action, SendUiAction::OpenRecord(record.id.clone()));
        assert_eq!(people.asked, None);

        let stop = painted.dialog_rect_of(share_ui::stop_label()).center();
        let (_, action) = paint_sharing(&state, &mut people, &mut selected, records, size, press_at(stop));
        assert_eq!(people.asked, Some(PeopleAsk::Stop(key.clone())), "step one asks, and nothing else");
        assert_eq!(action, SendUiAction::None);
    }

    #[test]
    fn a_shares_modal_says_what_went_wrong_at_its_foot() {
        let record = a_record("rec-1", "Acme VPN");
        let shares = [a_share_of(&record, "anna@x.io", PersonState::WaitingAccept)];
        let mut people = PeopleRows { shares: &shares, notice: Some(share_ui::busy().to_string()), ..PeopleRows::none() };
        let mut selected = Some(shares[0].key.clone());
        let (painted, _) =
            paint_sharing(&a_send_made_from(&record), &mut people, &mut selected, std::slice::from_ref(&record), TALL_WINDOW, Vec::new());
        assert_eq!(painted.in_detail(share_ui::busy()), 1, "no notice: {:?}", painted.text);
        let notice = painted.dialog_rect_of(share_ui::busy());
        let facts = painted.dialog_rect_of(share_ui::can_row());
        assert!(notice.top() > facts.top(), "the sentence is not under the facts");
        assert_eq!(painted.in_detail(share_ui::follows_edits()), 0);
        assert_eq!(painted.in_detail(share_ui::status_row()), 1, "{:?}", painted.text);
    }

    #[test]
    fn a_gone_share_draws_no_modal_and_shares_list_only_on_all() {
        let record = a_record("rec-1", "Acme VPN");
        let records = std::slice::from_ref(&record);
        let size = min_pane_size();
        let mut people = PeopleRows::none();
        let mut selected = Some(share_ui::share_key("the-old-id", "anna@x.io"));
        let (painted, _) = paint_sharing(&a_send_made_from(&record), &mut people, &mut selected, records, size, Vec::new());
        assert_eq!(painted.dialog_text_rects().count(), 0, "{:?}", painted.text);

        let shares = [a_share_of(&record, "anna@x.io", PersonState::Shared)];
        let mut people = PeopleRows { shares: &shares, ..PeopleRows::none() };
        let (painted, _) = paint_sharing(&SendPaneState::Empty, &mut people, &mut None, records, size, Vec::new());
        assert_eq!(painted.in_list("to anna@x.io \u{b7} Shared"), 1, "an account with no Sends still lists its share");
        assert!(!painted.has(empty_headline()), "{:?}", painted.text);
        assert!(painted.has(shares_subtext()) || painted.has(scope_subtext()), "{:?}", painted.text);

        let waiting_only = paint_scope_with(&a_send_made_from(&record), crate::vault_window::sidebar::SendScope::Waiting, &mut people);
        assert_eq!(waiting_only.in_list("to anna@x.io \u{b7} Shared"), 0, "a share on a Send-state sub-row");
    }

    /// [`paint_scope`] with person shares handed to the pane.
    fn paint_scope_with(
        state: &SendPaneState,
        scope: crate::vault_window::sidebar::SendScope,
        people: &mut PeopleRows<'_>,
    ) -> Painted {
        let size = min_pane_size();
        let ctx = egui::Context::default();
        let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});
        let output = warm_run(&ctx, input, |ui| {
            let _ = draw_send_pane(ui, state, None, SendDeleteView::default(), SendView::Mine(scope), &mut None, RecordTiles::default(), people).into_action();
        });
        let mut painted = Painted::default();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        painted
    }

    #[test]
    fn shares_are_listed_while_the_sends_load_and_after_they_fail() {
        let record = a_record("rec-1", "Acme VPN");
        let records = std::slice::from_ref(&record);
        let shares = [a_share_of(&record, "anna@x.io", PersonState::Shared)];
        let failed = SendPaneState::Failed { message: "bw is not signed in.".to_string(), ambiguous: true };
        for size in [egui::vec2(1028.0, 740.0), min_pane_size()] {
            for (state, status) in [(&SendPaneState::Loading, loading_label()), (&failed, try_again_label())] {
                let mut people = PeopleRows { shares: &shares, ..PeopleRows::none() };
                let (painted, _) = paint_sharing(state, &mut people, &mut None, records, size, Vec::new());
                assert_eq!(painted.in_list("to anna@x.io \u{b7} Shared"), 1, "{status:?}: the share is not listed: {:?}", painted.text);
                let card = card_of(&painted, "Acme VPN");
                let words = painted.rect_of(status).unwrap_or_else(|| panic!("no {status:?}: {:?}", painted.text));
                assert!(words.top() > card.bottom(), "{status:?} is not under the share");
                assert!(words.bottom() <= size.y, "{status:?} is pushed off a {}pt window", size.y);

                let mut none = PeopleRows::none();
                let (bare, _) = paint_sharing(state, &mut none, &mut None, records, size, Vec::new());
                assert_eq!(bare.in_list("to anna@x.io \u{b7} Shared"), 0);
                assert!(bare.has(status));
            }
        }
    }

    // ---- Shared with me ------------------------------------------------------

    #[test]
    fn shared_with_me_lists_imports_as_cards_and_says_where_each_went() {
        let history = crate::receive_history::ReceiveHistory {
            entries: vec![
                a_received_record(NOW - 3_600_000, "SAP Production", "item-here"),
                a_received_record(NOW - 7_200_000, "Office WiFi", "item-gone"),
            ],
        };
        let items = vec![vault_item("item-here")];
        let rows = received_rows(&history, &items, &FixedClock(NOW), &UTC);

        let listed = paint_received(&rows, None);
        assert!(listed.has(shared_page::received_title()), "the page does not name itself: {:?}", listed.text);
        assert_eq!(listed.in_list("SAP Production"), 1);
        assert_eq!(listed.in_list("Office WiFi"), 1);
        assert_eq!(listed.in_list("In your vault"), 1);
        assert_eq!(listed.in_list("Gone"), 1);
        assert_eq!(listed.dialog_text_rects().count(), 0, "a modal with nothing picked");
        for leaked in [loading_label(), scope_subtext()] {
            assert!(!listed.has(leaked), "{leaked:?} is drawn on `Shared with me`, which asks the server nothing: {:?}", listed.text);
        }

        let present = paint_received(&rows, Some("item-here".to_string()));
        assert!(present.has(received_item_present()), "{:?}", present.text);
        assert_eq!(present.in_detail(in_vault_yes()), 1);
        assert!(present.has(arrived_row()));

        let gone = paint_received(&rows, Some("item-gone".to_string()));
        assert!(gone.has(received_item_gone()), "{:?}", gone.text);
        assert_eq!(gone.in_detail(in_vault_no()), 1);
    }

    #[test]
    fn an_empty_shared_with_me_says_nothing_has_arrived_and_the_scope_line_is_its_own() {
        let painted = paint_received(&[], None);
        assert!(painted.has(received_empty_headline()), "{:?}", painted.text);
        assert!(painted.has(received_scope_subtext()));
    }

    #[test]
    fn shared_with_me_puts_records_from_people_above_the_imports_and_opens_them() {
        use crate::vault_window::share_ui::{ReceivedShareRow, from_people_eyebrow, from_sends_eyebrow};
        let history = crate::receive_history::ReceiveHistory {
            entries: vec![a_received_record(NOW - 3_600_000, "SAP Production", "item-here")],
        };
        let rows = received_rows(&history, &[vault_item("item-here")], &FixedClock(NOW), &UTC);
        let people = [ReceivedShareRow { key: "recv:r9".into(), item_id: "r9".into(), name: "Acme VPN".into(), from: "denis@x.io".into(), verified: true, organization_id: "org-x".into() }];
        let painted = paint_received_with(&rows, &people, None);
        let top = |label: &str| painted.rect_of(label).unwrap_or_else(|| panic!("no {label:?} in {:?}", painted.text)).top();
        assert!(top(from_people_eyebrow()) < top("Acme VPN"));
        assert!(top("Acme VPN") < top(from_sends_eyebrow()));
        assert!(top(from_sends_eyebrow()) < top("SAP Production"));
        assert!(painted.has("from denis@x.io \u{b7} view-only"), "{:?}", painted.text);
        assert!(painted.has(received_people_subtext()) && !painted.has(received_scope_subtext()), "{:?}", painted.text);
        let imports_only = paint_received_with(&rows, &[], None);
        assert!(imports_only.has(received_scope_subtext()) && !imports_only.has(received_people_subtext()));

        let alone = paint_received_with(&[], &people, None);
        assert!(alone.has("Acme VPN"));
        assert!(!alone.has(received_empty_headline()), "{:?}", alone.text);
        assert!(!alone.has(from_sends_eyebrow()), "an eyebrow over no rows: {:?}", alone.text);

        // A press on a record from a person opens it in the vault, as the
        // 0.15 row did, and picks nothing (the vault is where it is read).
        let at = card_of(&painted, "Acme VPN").center_top() + egui::vec2(0.0, 30.0);
        let mut people_rows = PeopleRows { received: &people, ..PeopleRows::none() };
        let (action, selected) = press_received(&rows, &mut people_rows, at);
        assert_eq!(action, SendUiAction::OpenRecord("r9".into()));
        assert_eq!(selected, None);
    }

    /// Presses `at` on the `Shared with me` page and answers the action and
    /// the selection it left.
    fn press_received(rows: &[ReceivedRow], people: &mut PeopleRows<'_>, at: egui::Pos2) -> (SendUiAction, Option<String>) {
        let size = min_pane_size();
        let ctx = egui::Context::default();
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let mut selected = None;
        let state = SendPaneState::Loading;
        let mut run = |events: Vec<egui::Event>, selected: &mut Option<String>| {
            let mut action = SendUiAction::None;
            let _ = ctx.run_ui(input(events), |ui| {
                action = draw_send_pane(ui, &state, None, SendDeleteView::default(), SendView::Received(rows), selected, RecordTiles::default(), people).into_action();
            });
            action
        };
        let _ = run(Vec::new(), &mut selected);
        let events = press_at(at);
        let _ = run(events[..2].to_vec(), &mut selected);
        let action = run(events[2..].to_vec(), &mut selected);
        (action, selected)
    }

    #[test]
    fn organisations_that_share_with_you_are_told_apart_by_a_hairline() {
        use crate::vault_window::share_ui::ReceivedShareRow;
        let row = |id: &str, org: &str| ReceivedShareRow { key: format!("recv:{id}"), item_id: id.into(), name: id.into(), from: "boss@x.io".into(), verified: false, organization_id: org.into() };
        let hairlines = |rows: &[ReceivedShareRow]| {
            paint_received_with(&[], rows, None)
                .fills
                .iter()
                .filter(|(r, f)| *f == crate::kit::tokens::Palette::LIGHT.hairline && r.height() <= 1.5 && r.width() > 100.0)
                .count()
        };
        let one = hairlines(&[row("a", "org-1"), row("b", "org-1")]);
        let two = hairlines(&[row("a", "org-1"), row("b", "org-2")]);
        assert_eq!(two, one + 1, "one hairline between the two organisations' records");
    }

    #[test]
    fn paste_an_invite_link_leads_shared_with_me() {
        use crate::vault_window::share_ui::{self as sharing, PeopleRows, ReceivedShareRow};
        let history = crate::receive_history::ReceiveHistory {
            entries: vec![a_received_record(NOW - 3_600_000, "SAP Production", "item-here")],
        };
        let rows = received_rows(&history, &[vault_item("item-here")], &FixedClock(NOW), &UTC);
        let from_people = [ReceivedShareRow { key: "recv:r9".into(), item_id: "r9".into(), name: "Acme VPN".into(), from: "denis@x.io".into(), verified: true, organization_id: "org-x".into() }];
        let mut people = PeopleRows { received: &from_people, paste: Some(zeroize::Zeroizing::new(String::new())), ..PeopleRows::none() };
        let (painted, selected) = paint_received_people(&rows, &mut people, None);
        assert_eq!(selected, None, "nothing is picked for the user");
        let top = |label: &str| painted.rect_of(label).unwrap_or_else(|| panic!("no {label:?} in {:?}", painted.text)).top();
        assert!(top(sharing::paste_accept_label()) < top(sharing::from_people_eyebrow()));

        let mut box_only = PeopleRows { paste: Some(zeroize::Zeroizing::new(String::new())), ..PeopleRows::none() };
        let (alone, _) = paint_received_people(&[], &mut box_only, None);
        assert!(alone.has(sharing::paste_accept_label()), "{:?}", alone.text);
        assert!(alone.has(received_empty_headline()), "{:?}", alone.text);

        let waiting = [crate::rest::sharing::WaitingEntry { organization_id: "org-x".into(), from: "denis@x.io".into(), verified: true }];
        let mut only_waiting = PeopleRows { waiting: &waiting, ..PeopleRows::none() };
        let (painted, _) = paint_received_people(&[], &mut only_waiting, None);
        assert!(painted.has(&sharing::waiting_line("denis@x.io")), "{:?}", painted.text);
        assert!(!painted.has(received_empty_headline()), "{:?}", painted.text);
    }

    /// **One window across frames**, for what depends on the frame before: a
    /// dialog's card shows on its second pass, and a question that joins the
    /// modal keeps the height the modal had at rest.
    struct Session {
        ctx: egui::Context,
        size: egui::Vec2,
        selected: Option<String>,
        /// Seconds; a second a frame, so a fade or a pop is over by the next.
        clock: f64,
    }

    impl Session {
        fn new(size: egui::Vec2, selected: Option<String>) -> Self {
            let ctx = egui::Context::default();
            let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
            let _ = ctx.run_ui(input(), |_ui| {});
            theme::apply(&ctx);
            Self { ctx, size, selected, clock: 0.0 }
        }

        /// One frame with `events`.
        fn frame(
            &mut self,
            state: &SendPaneState,
            delete: SendDeleteView<'_>,
            people: &mut PeopleRows<'_>,
            records: &[crate::vault_bridge::VaultItem],
            events: Vec<egui::Event>,
        ) -> (Painted, SendUiAction) {
            self.clock += 1.0;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
                time: Some(self.clock),
                events,
                ..Default::default()
            };
            let mut action = SendUiAction::None;
            let selected = &mut self.selected;
            let tiles = RecordTiles { items: records, icons: None };
            let output = self.ctx.run_ui(input, |ui| {
                action = draw_send_pane(ui, state, None, delete, SendView::Mine(ALL_SENDS), selected, tiles, people).into_action();
            });
            let mut painted = Painted::default();
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut painted);
            }
            (painted, action)
        }

        /// Six quiet frames (an anchored card is placed by the size it had the
        /// frame before, and a scrolled body takes a few to find its own), the
        /// last one's paint.
        fn settle(
            &mut self,
            state: &SendPaneState,
            delete: SendDeleteView<'_>,
            people: &mut PeopleRows<'_>,
            records: &[crate::vault_bridge::VaultItem],
        ) -> Painted {
            for _ in 0..5 {
                let _ = self.frame(state, delete, people, records, Vec::new());
            }
            self.frame(state, delete, people, records, Vec::new()).0
        }
    }

    #[test]
    fn cancel_stands_exactly_where_revoke_was_at_both_window_sizes() {
        let state = one_row(crate::send::SendState::Waiting);
        for size in [roomy_pane_size(), min_pane_size()] {
            let mut window = Session::new(size, Some("id0".to_string()));
            let mut none = PeopleRows::none();
            let before = window.settle(&state, SendDeleteView::default(), &mut none, &[]);
            let revoke = before.fill_behind(before.dialog_rect_of(revoke_label())).expect("Revoke has no button").0;
            let armed = SendDeleteView { confirming: Some("id0"), in_flight: None, extending: false };
            let during = window.settle(&state, armed, &mut none, &[]);
            let cancel = during.fill_behind(during.dialog_rect_of(cancel_label())).expect("Cancel has no button").0;
            assert!(
                (revoke.min - cancel.min).length() < 0.5 && (revoke.max - cancel.max).length() < 0.5,
                "at {}pt Revoke is {revoke:?} and Cancel {cancel:?}: the question grew the dialog and moved the buttons",
                size.x
            );
        }
    }

    #[test]
    fn a_second_click_where_revoke_was_cancels_and_never_destroys() {
        let state = rows(3);
        let mut window = Session::new(min_pane_size(), Some("id0".to_string()));
        let mut none = PeopleRows::none();
        let before = window.settle(&state, SendDeleteView::default(), &mut none, &[]);
        let where_revoke_was = before.dialog_rect_of(revoke_label()).center();
        let armed = SendDeleteView { confirming: Some("id0"), in_flight: None, extending: false };
        let during = window.settle(&state, armed, &mut none, &[]);
        assert!(
            !during.dialog_rect_of(confirm_label()).contains(where_revoke_was),
            "control: the destructive button covers Revoke's place"
        );
        let (_, action) = window.frame(&state, armed, &mut none, &[], press_at(where_revoke_was));
        assert_eq!(
            action,
            SendUiAction::CancelDelete,
            "the pixel Revoke occupied does something other than cancel once the question is up -- a \
             double-click on Revoke would delete a public link with no decision taken"
        );
        assert!(!matches!(action, SendUiAction::ConfirmDelete { .. }));
    }

    #[test]
    fn stopping_a_share_asks_first_and_cancel_takes_the_stops_place() {
        let record = a_record("rec-1", "Acme VPN");
        let state = a_send_made_from(&record);
        let shares = [a_share_of(&record, "anna@x.io", PersonState::Shared)];
        let key = shares[0].key.clone();
        let records = std::slice::from_ref(&record);
        for size in [egui::vec2(1028.0, 740.0), min_pane_size()] {
            let mut window = Session::new(size, Some(key.clone()));
            let mut people = PeopleRows { shares: &shares, ..PeopleRows::none() };
            let at_rest = window.settle(&state, SendDeleteView::default(), &mut people, records);
            let stop_rect = at_rest.dialog_rect_of(share_ui::stop_label());
            assert!(stop_rect.right() <= size.x + 0.5, "Stop sharing runs off a {}pt window", size.x);

            people.confirming = Some(key.clone());
            let asking = window.settle(&state, SendDeleteView::default(), &mut people, records);
            assert!(asking.has(&share_ui::stop_question("Acme VPN", "anna@x.io")), "{:?}", asking.text);
            assert_eq!(asking.in_detail(share_ui::remove_label()), 1);
            assert_eq!(asking.in_detail(open_record_label()), 0, "Open record steps back while the question is up");
            assert_eq!(asking.in_detail(share_ui::stop_label()), 0);
            let cancel_button = asking.fill_behind(asking.dialog_rect_of(share_ui::cancel_button())).expect("Cancel has no button").0;
            let stop_button = at_rest.fill_behind(stop_rect).expect("Stop has no button").0;
            assert!(
                (cancel_button.min - stop_button.min).length() < 0.51 && (cancel_button.max - stop_button.max).length() < 0.51,
                "Cancel is not in Stop sharing's slot at {}pt: {cancel_button:?} vs {stop_button:?}",
                size.x
            );

            let remove = asking.dialog_rect_of(share_ui::remove_label()).center();
            people.asked = None;
            let _ = window.frame(&state, SendDeleteView::default(), &mut people, records, press_at(remove));
            assert_eq!(people.asked, Some(PeopleAsk::ConfirmStop(key.clone())));
            let cancel = asking.dialog_rect_of(share_ui::cancel_button()).center();
            people.asked = None;
            let _ = window.frame(&state, SendDeleteView::default(), &mut people, records, press_at(cancel));
            assert_eq!(people.asked, Some(PeopleAsk::CancelStop));

            people.confirming = None;
            people.stopping = Some(key.clone());
            let running = window.settle(&state, SendDeleteView::default(), &mut people, records);
            assert_eq!(running.in_detail(share_ui::stopping_label()), 1, "{:?}", running.text);
            for control in [share_ui::stop_label(), share_ui::cancel_button(), open_record_label(), share_ui::remove_label()] {
                assert_eq!(running.in_detail(control), 0, "{control:?} is offered while the stop runs");
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
                        std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
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
                 crate::vault_window::send_ui::receive_needs_the_cli(), error.user_message() \
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

    /// **The Sends worker asks `tasks_for` ONCE and then branches on what the
    /// ROW wanted** -- on
    /// [`the_delegated_fetch_is_a_real_bw_send_list_for_the_active_account`]'s
    /// terms and for its reasons.
    ///
    /// This is the property that makes another operation free:
    /// `the_backend_is_chosen_in_exactly_one_place` counts `tasks_for(`, and a
    /// verb that asked again would be one more -- a second reading of the
    /// backend, which is the shape all six of this window's original defects
    /// had. A branch on `backend_policy::selected()` written in here fails
    /// this equality and that census at once.
    ///
    /// **It used to pin a switch as well**, a link turned off and on through
    /// the built-in client and refused in words by the CLI. Revoke deletes
    /// now, on the Sent screen and on a record's SHARING card alike, and the
    /// switch went with everything that reached it.
    #[test]
    fn the_sends_worker_reads_the_backend_once_and_branches_on_the_rows_verb() {
        assert_eq!(
            squashed(&sanitized(&body_of(concat!("real_send_", "delete"), "    "))),
            squashed(
                "id: &str, name: &str, session: &str, op: SendRowOp, ) -> SendDeleteReport { \
                 let tasks = super::backend_tasks::tasks_for(Some(session)); match op { \
                 SendRowOp::Delete => tasks.send_delete(id, name), SendRowOp::Extend => \
                 tasks.send_extend(id, name), }"
            ),
            "the Sends worker is no longer one backend choice followed by a branch on what \
             the row asked for. A second `tasks_for` here is a second reading of the backend \
             between the frame and the thread, and an account switch happens in that gap"
        );
        // Nothing in production reaches for the switch any more, on either
        // side of the trait.
        assert!(
            !production().contains(concat!("send_set_", "disabled")),
            "a switch is back on the Sends backend, and nothing on screen asks for one"
        );
    }

    /// **Extend is one worker verb and one door on each backend**, on
    /// [`the_sends_worker_reads_the_backend_once_and_branches_on_the_rows_verb`]'s
    /// terms: equalities over the two implementations, and a census of the
    /// two doors.
    ///
    ///  1. **The CLI arm is one `cli_send_extend`**, in this window's job, for
    ///     the active account, with the session and the real clock -- and a
    ///     report of what it asked for, confirmed or not.
    ///  2. **The built-in arm is one call to the REST client**, with the same
    ///     clock and the same two reports.
    ///  3. **Each door has exactly one caller.** `cli_send_extend` blocks for
    ///     two children; a second caller is unproven ground for which thread
    ///     it runs on. The seal that keeps `real_send_delete` off the frame
    ///     thread holds the worker, and this holds the doors to the worker.
    #[test]
    fn extend_is_one_worker_verb_and_one_door_on_each_backend() {
        assert_eq!(
            squashed(&sanitized(&body_in(
                &tasks_impl("impl BackendTasks for CliTasks<'_>"),
                "send_extend",
                "        ",
            ))),
            squashed(&format!(
                "&self, id: &str, name: &str) -> SendDeleteReport {{ let data_dir = \
                 Self::data_dir(); match crate::send::{}( Self::job(), data_dir.as_deref(), \
                 self.session(), id, &crate::send::SystemClock, ) {{ Ok(()) => \
                 SendDeleteReport::Extended {{ name: name.to_string() }}, Err(error) => \
                 SendDeleteReport::ExtendFailed {{ name: name.to_string(), error }}, }}",
                concat!("cli_send_", "extend"),
            )),
            "the CLI Extend is no longer exactly one `bw send get` and `bw send edit` pair, in \
             this window's job, for the active account, reporting what it asked for"
        );
        assert_eq!(
            squashed(&sanitized(&body_in(
                &tasks_impl("impl BackendTasks for DirectRestTasks"),
                "send_extend",
                "        ",
            ))),
            squashed(&format!(
                "&self, id: &str, name: &str) -> SendDeleteReport {{ match \
                 crate::rest::send::{}(id, &crate::send::SystemClock) {{ Ok(()) => \
                 SendDeleteReport::Extended {{ name: name.to_string() }}, Err(error) => \
                 SendDeleteReport::ExtendFailed {{ name: name.to_string(), error }}, }}",
                concat!("extend_on_active_", "account"),
            )),
            "the built-in Extend is no longer exactly one call to the REST client"
        );
        for door in [
            concat!("crate::send::cli_send_", "extend("),
            concat!("crate::rest::send::extend_on_active_", "account("),
        ] {
            assert_eq!(
                production().matches(door).count(),
                1,
                "{door} is called somewhere other than its implementation of `BackendTasks`"
            );
        }
    }

    /// **`receive_needs_the_cli()` survives for the CLI and is unreachable for
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
            cli.contains("receive_needs_the_cli"),
            "control: the `bw serve` implementation no longer names receive_needs_the_cli at \
             all, so the assertion below is searching text it could never have found"
        );
        assert!(
            !direct.contains("receive_needs_the_cli"),
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
        // one-off test table; an alias would only be read here
        #[allow(clippy::type_complexity)]
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
                    std::iter::repeat_n(path.as_str(), count)
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
                std::iter::repeat_n(path.as_str(), count)
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
    ///
    /// **Three, since the window's heartbeat.** The third site is
    /// `heartbeat_sync_due`'s, and it IS a `bw sync` under a per-frame `if`
    /// -- the exact shape the paragraph above calls an unbounded stream. It
    /// is counted here on purpose rather than routed round this pin, and
    /// what bounds it is not its spelling: it never fires while a sync is
    /// out, never within `HEARTBEAT_FOCUS_GAP` of the last attempt by any
    /// door, and never within `HEARTBEAT_AFTER_FAILURE` of a failed one.
    /// Those floors are pinned where they are decided, in
    /// `heartbeat_sync_due`'s own tests; and the multi-frame scenarios in
    /// this module still count exactly the syncs they expect with the
    /// heartbeat compiled in, which is the behavioural half of "not a
    /// stream".
    #[test]
    fn both_sync_call_sites_pass_the_windows_own_session() {
        let closure = squashed(&frame_closure());
        let call =
            squashed(concat!("(spawn_", "sync)(sync_tx.clone(), session_token.to_string());"));
        let opener = concat!("(spawn_", "sync)(");
        assert_eq!(
            closure.matches(&call).count(),
            4,
            "{call:?} is not written in the frame closure exactly four times. The Sync \
             call sites are the auto-sync on the window's first real frame, the window's \
             heartbeat, the status pill's press and the list's offline Retry, and each must \
             hand over the window's \
             own session: a `bw sync` started with an empty `BW_SESSION` is answered \
             `Locked` by a real vault"
        );
        assert_eq!(
            closure.matches(opener).count(),
            4,
            "{opener:?} is called from the frame closure {} times, not four -- there is a \
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
            4,
            "{name:?} is NAMED in the frame closure {} times, not four. The four times it \
             may be named are the four `(spawn_sync)(` call sites counted just above; any \
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
        //
        // **And four in `vault_window/mod.rs` since the window's heartbeat**
        // -- the binding and now THREE calls, the third being
        // `heartbeat_sync_due`'s. See the doc on this test for what bounds
        // it; the count moved with the same kind of reason as `main.rs`'s.
        //
        // **Five since the list's offline banner** (spec 3.1): its Retry is
        // the pill's own sync, called from the same closure, a fourth call.
        for (needle, expected) in [
            (
                concat!("spawn_", "sync"),
                vec![("main.rs", 4usize), ("vault_window/mod.rs", 5)],
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
            // **And AGAIN when `cli_send_extend` landed**, on step 4's terms
            // exactly: an Extend is authenticated at both of its halves, so
            // its body hands a `CliSendRunner::with_session(job, data_dir,
            // session)` to the generic extend -- one more of each. Eight is
            // now the three definitions and the FIVE entry points, and no
            // sixth; `CliSendRunner::new` below does not move.
            (concat!("CliSendRunner", "::with_session"), 4, false, false),
            // **Moved by one AGAIN when `cli_send_receive` landed**, and this
            // time on the OTHER constructor, which is the whole of what makes
            // the move worth recording rather than a number that drifted: a
            // receive is anonymous -- the link is the credential -- so
            // `cli_send_receive` builds `CliSendRunner::new(job, data_dir)`
            // and hands the child no `BW_SESSION` at all. Seven is now
            // `pub struct CliSendRunner`, `impl<'a> CliSendRunner<'a>`,
            // `impl SendRunner for CliSendRunner<'_>` and the FOUR entry
            // points, and no fifth.
            (concat!("CliSendRunner", ""), 8, false, false),
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
            // **The Extend's four, on the receive's terms.** `cli_send_extend`
            // is its definition alone; `extend_send` is its definition plus the
            // one call in `cli_send_extend`; `get_invocation` and
            // `edit_invocation` are each their definition plus the one use in
            // `extend_send`. Both builders are private, so a third mention of
            // either is a second blocking `bw send get` or `bw send edit`
            // written inside the privacy boundary -- and `edit_invocation` is
            // the one that carries a whole decrypted Send on its stdin.
            (concat!("cli_send_", "extend"), 1, false, false),
            (concat!("extend_", "send"), 2, false, false),
            (concat!("get_", "invocation"), 2, false, false),
            (concat!("edit_", "invocation"), 2, false, false),
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
        // The instant behind `deletion_date`, which the Send words file reads to say the day.
        "send.rs: pub(crate) fn deletion_instant(lifetime: SendLifetime, now: &dyn SendClock) -> Option<i64> {",
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
        // **`is_live`, added with `Opened`, deliberately.** A `bool` read off
        // a fieldless enum: "does this link still answer" asked once, rather
        // than `== Waiting` in five places that would each have called an
        // opened link dead. No runner, no invocation, no child.
        "send.rs: pub fn is_live(self) -> bool {",
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
        "send.rs: pub fn is_ambiguous(&self) -> bool {",
        "send.rs: pub fn classify_failure(exit_code: Option<i32>, stdout: &str, stderr: &str) -> SendError {",
        "send.rs: pub fn parse_created_send(stdout: &str) -> Result<CreatedSend, SendError> {",
        "send.rs: pub fn parse_send_list(stdout: &str) -> Result<Vec<SendSummary>, SendError> {",
        "send.rs: pub trait SendRunner {",
        "send.rs: pub fn create_send<R: SendRunner>(",
        "send.rs: pub(crate) fn list_sends<R: SendRunner>(runner: &R) -> Result<Vec<SendSummary>, SendError> {",
        "send.rs: pub fn delete_send<R: SendRunner>(runner: &R, id: &str) -> Result<(), SendError> {",
        // **Added by design 5b's Extend, deliberately, and none of the three
        // is a door in the sense this wall guards.** The period is a number.
        // `extend_dates` moves two date strings on a JSON object it is handed
        // and `extend_confirmed` reads one back; both are `pub(crate)` for
        // `deletion_date`'s reason, so `crate::rest::send` extends through the
        // same arithmetic as the CLI path rather than a second copy of it.
        // None reaches a runner, an invocation or a child -- the generic
        // `extend_send` beside them is private, like `receive_send`.
        "send.rs: pub const EXTEND_BY_DAYS: i64 = 7;",
        "send.rs: pub(crate) fn extend_dates(",
        "send.rs: pub(crate) fn extend_confirmed(answer: &serde_json::Value, asked: i64) -> bool {",
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
        // **Added by design 5b's Extend, deliberately**, for the four rows
        // above's reason word for word: `extend_send` needs a `SendRunner`,
        // there is no `pub` implementation of that trait in this crate, and
        // the only alternative to one new door was making the runner nameable
        // from outside -- which is the wall itself. It carries the session,
        // like the delete and unlike the receive: both of its children are
        // authenticated. Its one caller is sealed by
        // `extend_is_one_worker_verb_and_one_door_on_each_backend`.
        "send.rs: pub fn cli_send_extend(",
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
    /// **4a's term has gone from this gate**, and that is the change rather
    /// than a term quietly dropped: the sequence builder was a screen that
    /// took the window, and it is a modal now -- the panels behind it lay out
    /// exactly as they do in Read or Edit, and what disables them is the
    /// scrim. The needle is still the whole line, which is what makes a term
    /// dropped by accident a failure here.
    #[test]
    fn the_item_list_is_drawn_only_inside_the_not_sends_gate() {
        let production = production();
        let gate = concat!("        if !show_", "sends && !on_health && !editing && !generator_selected && !settings_selected {\r\n");
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
        //
        // **Design 6c's `&mut people_rows` is the last argument**: the frame's
        // share rows and the stop two-step, built above the applier and read
        // back after it. It is not a gate on what the pane is asked for -- the
        // Sends model is still the inline `state` -- and pinning it here keeps
        // the pane handed the window's one `PeopleRows` rather than a
        // `none()` that would quietly list no share.
        let pane = squashed(concat!(
            "send_ui::draw_send_", "pane( ui, state, notice_message.as_deref(), \
             send_delete.view(), if on_received { send_ui::SendView::Received(&received) } \
             else { send_ui::SendView::Mine(send_scope) }, &mut selected_send, \
             send_ui::RecordTiles { items: &items, icons: Some(&icons.textures) }, \
             &mut people_rows, )"
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
            concat!("CornerRadius::same(6), ", "p.blue_wash");
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
            // 7 with `seen_password_tests` (spec B §7).
            modules, 7,
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
                if before.is_none_or(|c| !ident(c)) && after.is_none_or(|c| !ident(c)) {
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
        // v2: the read pane's footer ("Updated recently · ..."), which
        // neither the list nor the editor paints.
        "Updated "
    }
    /// **The item list's control**: its header's `+ New`, which
    /// `draw_item_list` is the only thing in this window that paints. (It was
    /// the search field's hint until the v2 top bar took the search out of
    /// the list.)
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
    // v2: both forms' Save says "Save changes"; the header's kind line
    // tells an edit from a create.
    const EDITOR_EDIT_TITLE: &str = "Editing login";
    /// **The `DetailMode::Create` arm's control.**
    ///
    /// It was `detail_edit::form_title`'s `New login`, drawn under the name
    /// box; that line is gone entirely -- the owner: "Login under title -
    /// remove, it will go in a separate block" -- so the string that told the
    /// two arms apart no longer exists on either.
    ///
    /// `detail_edit::TOTP_CREATE_NOTICE` replaces it, and it is a better
    /// sentinel than the heading was: the form draws it exactly when there is
    /// no saved item to hang a one-time code on, which is `creating` itself
    /// rather than a label that happened to follow it. The EDIT arm draws a
    /// real TOTP row there instead, so the two arms still differ by this
    /// string alone.
    const EDITOR_CREATE_TITLE: &str = super::super::detail_edit::TOTP_CREATE_NOTICE;
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
    //
    // Slice 7: Settings is a screen now, so this is the v2 General page's
    // subtitle (and the old modal's subtitle is no longer on screen at all).
    fn prefs_modal_only() -> String {
        crate::prefs_ui::settings_v2::Page::General.subtitle().expect("General has a subtitle")
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
        load: crate::vault_window::VaultLoadSpawner,
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
        /// The item list's header `+` was drawn on the last frame: the list
        /// alone draws it, so this is "`draw_item_list` ran".
        list_drew: bool,
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
            // The Sends screen is the gear menu's first row: two clicks, the
            // gear and then the row, exactly as it is for a user.
            let gear = ctx
                .read_response(crate::vault_window::top_bar::gear_id())
                .map(|r| r.rect.center())
                .unwrap_or_else(|| panic!("the top bar drew no Settings gear to open the menu with"));
            click(&ctx, &mut *frame_fn, gear);
            let _ = ctx.run_ui(input(), |ui| frame_fn(ui));
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
            let pos = locate_label(&output, super::super::sidebar::SENDS_ROW_LABEL, "the gear menu");
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
            // v2: Edit is a button on the header's action row.
            let pos = locate_label(&output, "Edit", "the read pane's action row");
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
            // The top bar's gear menu's Settings row: the gear first, then the
            // row, found by its label like every other control here. Its
            // click is answered on the next frame (a shell request), hence
            // the extra frame below.
            let gear = ctx
                .read_response(crate::vault_window::top_bar::gear_id())
                .map(|r| r.rect.center())
                .unwrap_or_else(|| panic!("the top bar drew no Settings gear to open the menu with"));
            click(&ctx, &mut *frame_fn, gear);
            let _ = ctx.run_ui(input(), |ui| frame_fn(ui));
            output = ctx.run_ui(input(), |ui| frame_fn(ui));
            let label = crate::vault_window::sidebar::SETTINGS_ROW_LABEL;
            let rect = labels_of(&output)
                .into_iter()
                .find(|(text, _)| text == label)
                .map(|(_, rect)| rect)
                .unwrap_or_else(|| {
                    panic!(
                        "the gear menu painted no {label:?} row to press, so this scenario cannot \
                         open the preferences modal at all. What was painted: {:?}",
                        labels_of(&output).into_iter().map(|(t, _)| t).collect::<Vec<_>>()
                    )
                });
            click(&ctx, &mut *frame_fn, rect.center());
            let _ = ctx.run_ui(input(), |ui| frame_fn(ui));
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
            list_drew: ctx.read_response(crate::vault_window::item_list::new_button_id()).is_some(),
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
    ///  * `Outcome::list_drew`, the list header's `+` -- `draw_item_list` ran.
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
        assert!(
            outcome.list_drew,
            "the loaded window drew no list header +, so `draw_item_list` never drew -- the + \
             is the one thing on that half of the screen the detail pane cannot produce"
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
    fn the_settings_screen_returns_promptly() {
        let outcome = measured("prefs", Scenario { press_prefs: true, ..Scenario::new() });
        assert_the_body_was_entered("prefs", &outcome);
        outcome.expect_painted(
            &prefs_modal_only(),
            "the Settings row was pressed and the Settings screen is not on screen, so the \
             block that draws it never ran",
        );
        assert!(
            !outcome.list_drew,
            "the Settings screen is up and the item list is still drawn beside it, so this is \
             not the screen-in-place-of-the-list this test is about"
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
        // one-off test table; an alias would only be read here
        #[allow(clippy::type_complexity)]
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

#[cfg(test)]
mod seen_password_tests {
    use super::*;

    fn row(state: crate::send::SendState, fields: &[&str]) -> SendRow {
        SendRow {
            sent: Some(SentFacts { recipient: String::new(), item_id: "item-1".into(), fields: fields.iter().map(|f| f.to_string()).collect() }),
            state,
            id: "send-1".into(),
            name: "Ledgerline".into(),
            expiry: String::new(),
            is_file: false,
            access_url: String::new(),
            access_count: 1,
            max_access_count: Some(1),
            has_password: false,
            activity: String::new(),
            expires: String::new(),
            deletes: String::new(),
            extendable: false,
        }
    }

    #[test]
    fn the_warning_shows_for_an_opened_password_send() {
        assert!(password_was_seen(&row(crate::send::SendState::Opened, &["Username", "Password"])));
        assert!(!password_was_seen(&row(crate::send::SendState::Waiting, &["Password"])), "not before it is opened");
        assert!(!password_was_seen(&row(crate::send::SendState::Opened, &["Username"])), "not without the password");
    }
}

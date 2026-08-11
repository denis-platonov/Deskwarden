//! The vault window's **Sends** screen: a read-only list of the Sends this
//! account has published.
//!
//! This is step 3 of five, and it is deliberately the *third* thing built and
//! the *first* thing visible. A Send is a public URL: the only outbound
//! publishing action in this app. The design's order is list, then delete,
//! then create -- revocation before publication -- so this screen can show a
//! Send and copy its link, and there is no way in the app to make one or to
//! revoke one yet. Steps 4 and 5 add those.
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
//! and this window has no upload path). It can list and -- from step 4 -- it
//! can revoke one. So a file Send made in the web vault appears here with a
//! tag saying what it is. Filtering them out would make "your Sends" a lie in
//! exactly the direction that matters: an unlisted public link is one the
//! user cannot revoke from here and does not know is there.
//!
//! ## The link is shown, always
//!
//! Every row carries a Copy link button. A link shown once, at creation, and
//! never again is a support ticket; the whole point of the list is that the
//! user can get back to a link they published.

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
}

/// Row geometry. Two lines of text and a button, so the row is tall enough
/// for the expiry to sit under the name rather than compete with it for the
/// width the button also wants.
const ROW_HEIGHT: f32 = 54.0;
const ROW_PAD_X: f32 = 14.0;
const COPY_BUTTON_WIDTH: f32 = 92.0;
const COPY_BUTTON_HEIGHT: f32 = 26.0;
/// The gap between the name and its FILE tag, and between the tag's text and
/// its own outline.
const TAG_PAD_X: f32 = 6.0;

/// Draws the whole Sends screen and reports what was clicked.
///
/// `notice` is the message the window's single inline band is showing this
/// frame, already chosen by `vault_window::inline_notice` -- this function
/// does not decide which of the window's messages wins, it only paints the
/// one it is handed. That is the same split every other pane in this window
/// uses, and it is why a Sends failure is a `NoticeSource` rather than a
/// widget of its own.
pub fn draw_send_pane(
    ui: &mut egui::Ui,
    state: &SendPaneState,
    notice: Option<&str>,
) -> SendUiAction {
    let mut action = SendUiAction::None;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Sends")
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
                        if let Some(url) = draw_row(ui, row) {
                            action = SendUiAction::CopyLink(url);
                        }
                    }
                });
        }
    }

    action
}

/// One row. Returns **this row's** URL when its Copy link button was clicked.
///
/// The URL is read off the `row` this call was handed and off nothing else --
/// no index into the list, no lookup by name. The wrong-row copy is the
/// classic form of this bug and the only structural defence against it is not
/// to have a second way of naming the row.
fn draw_row(ui: &mut egui::Ui, row: &SendRow) -> Option<String> {
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

    painter.text(
        egui::pos2(rect.left() + ROW_PAD_X, rect.bottom() - 12.0),
        egui::Align2::LEFT_BOTTOM,
        &row.expiry,
        small_font,
        theme::TEXT_FAINT,
    );

    // Right-aligned against the row's own right edge, so the button stays
    // reachable at every window width the OS will allow -- see
    // `settings::MIN_VAULT_WINDOW_SIZE` and the geometry test below. Placed
    // with `ui.put` into an explicit rect rather than laid out by a nested
    // horizontal: a nested layout is what has repeatedly pushed a control off
    // the pane in this window, and a control drawn at zero size passes both
    // the presence and the in-pane assertions.
    let button_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.right() - ROW_PAD_X - COPY_BUTTON_WIDTH,
            rect.center().y - COPY_BUTTON_HEIGHT / 2.0,
        ),
        egui::vec2(COPY_BUTTON_WIDTH, COPY_BUTTON_HEIGHT),
    );
    // **A row with no URL has nothing to copy, and says so by being
    // unclickable.** `send.rs`'s parser rejects a *missing* `accessUrl` but
    // accepts an empty one, so a row can reach here holding `""`; copying
    // that would hand `copy_text("")` to the clipboard, silently wiping
    // whatever the user had there and reporting success. The button is still
    // drawn -- the row must not lose its shape, and a row that quietly has
    // no control is harder to understand than one that has a dead one.
    let has_url = !row.access_url.is_empty();
    let clicked = ui
        .put(
            button_rect,
            egui::Button::new(egui::RichText::new("Copy link").size(12.0).color(theme::INK))
                .min_size(egui::vec2(COPY_BUTTON_WIDTH, COPY_BUTTON_HEIGHT)),
        )
        .clicked();
    ui.add_space(6.0);
    // Belt and braces: the guard is on the *returned action* as well as on
    // the widget, so no future re-layout of the button can reopen the path.
    (clicked && has_url).then(|| row.access_url.clone())
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

    /// **The gap this step could not close, asserted rather than left
    /// latent.** `list_invocation` carries no session token -- step 1 defined
    /// `None` as "the runner uses the session it was configured with", and
    /// step 2's `CliSendRunner` has no session to be configured with. So the
    /// child inherits whatever `BW_SESSION` this process has, which is none,
    /// and a real `bw send list` answers `Locked`.
    ///
    /// This test is the evidence for the report, and it will fail the moment
    /// somebody fixes it -- which is the point: the fix is a change to
    /// `send.rs`, which this task does not own.
    #[test]
    fn the_list_invocation_still_carries_no_session_token() {
        let runner = FakeRunner::ok("[]");
        let _ = list_sends(&runner);
        assert_eq!(runner.seen.borrow().as_slice(), [vec!["send".to_string(), "list".to_string()]]);
        assert_eq!(
            runner.sessions.borrow().as_slice(),
            [None],
            "the list invocation now carries a session -- delete this test and the note in \
             `spawn_send_list` with it"
        );
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
            action = draw_send_pane(ui, state, notice);
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
            let _ = draw_send_pane(ui, state, None);
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
            let _ = draw_send_pane(ui, state, None);
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
            action = draw_send_pane(ui, state, None);
        });
        action
    }

    /// **A row with no URL cannot clear the clipboard.** `parse_send_list`
    /// rejects a *missing* `accessUrl` but accepts `""`, so this row shape is
    /// reachable from a real server answer. The button is still painted --
    /// the row keeps its shape -- but pressing it reports nothing, because
    /// `CopyLink("")` reaches `copy_text("")`, which wipes the clipboard and
    /// then tells the user it copied a link.
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
            let _ = draw_send_pane(ui, &state, Some("a message"));
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
            let _ = draw_send_pane(ui, &state, Some("a message"));
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
            action = draw_send_pane(ui, &state, Some("a message"));
        });
        assert_eq!(action, SendUiAction::DismissNotice);
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

    fn production() -> &'static str {
        let source = include_str!("mod.rs");
        let end = source.find(concat!("#[cfg(", "test)]")).expect("no test marker");
        &source[..end]
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

    /// The body of `mod send_fetch_thread`, from its opener to the first `}`
    /// at column zero.
    ///
    /// **The privacy boundary the blocking fetch lives behind**, sliced the
    /// way `the_item_list_is_drawn_only_inside_the_not_sends_gate` slices its
    /// gate, so the pins below can say "inside there and nowhere else" rather
    /// than "somewhere in the file".
    fn sealed_module() -> &'static str {
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
        &rest[..end]
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
            "ctx_for_sends: egui::Context, tx: SendListSender, generation: u64, ) {{ {}(\
             ctx_for_sends, tx, generation, {}); ",
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

    /// The delegated fetch really is a real `bw send list`. A fake here, or a
    /// runner built for some other account, would be a screen that lists
    /// nothing whatever the account holds -- and `fetch_thread_tests` cannot
    /// see it, because it deliberately supplies its own fetch.
    ///
    /// Needles are safe *here*: `real_send_list` is a plain function with no
    /// closure in it, so there is no inside and outside to move code between.
    #[test]
    fn the_delegated_fetch_is_a_real_bw_send_list_for_the_active_account() {
        let body = body_of(concat!("real_send_", "list"), "    ");
        let runner = concat!("crate::send::CliSendRunner::", "new(None, data_dir.as_deref())");
        assert!(
            body.contains(runner),
            "the Sends fetch no longer runs through {runner:?}. Body was: {body}"
        );
        let dir = concat!("crate::bw_path::active_data_", "dir()");
        assert!(
            body.contains(dir),
            "the fetch no longer reads the active account's profile directory, so it can \
             list a different account's Sends than the window is showing"
        );
        let call = concat!("crate::send::list_", "sends(&runner)");
        assert!(
            body.contains(call),
            "`real_send_list` does not call {call:?} -- it is not the thing fetching the list"
        );
        assert_eq!(
            production().matches(concat!("crate::send::list_", "sends(")).count(),
            1,
            "`list_sends` is called somewhere other than `real_send_list` -- and every other \
             caller is unproven ground for which thread it runs on"
        );
    }

    /// The frame closure is what starts the fetch, exactly once, and it hands
    /// over the **current** generation. The tag is what `apply_answer` uses to
    /// drop a late answer; a spawn that carries a constant instead would make
    /// every answer look current.
    #[test]
    fn the_frame_starts_the_fetch_once_and_tags_it_with_the_question_it_is_asking() {
        let production = production();
        // Squashed on both sides, so this is a pin on the call and not on
        // where rustfmt chose to break its arguments.
        let spawn = squashed(concat!(
            "send_fetch_thread::spawn_send_",
            "list( ui.ctx().clone(), send_tx.clone(), send_fetch.generation(), );"
        ));
        assert_eq!(
            squashed(production).matches(&spawn).count(),
            1,
            "{spawn:?} is not in production exactly once -- the Sends fetch is started from \
             nowhere, from more than one place, or without the generation that lets a stale \
             answer be told from a current one"
        );
        // And the entry point itself is named exactly once, so the squashed
        // match above cannot be satisfied by a doc comment that spells the
        // call out while the frame does something else.
        let entry = concat!("send_fetch_thread::spawn_send_", "list(");
        assert_eq!(
            production.matches(entry).count(),
            1,
            "{entry:?} appears in production {} times, not once",
            production.matches(entry).count()
        );
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
        let production = production();
        let block = sealed_module();

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

        for needle in [
            concat!("list_", "sends"),
            concat!("real_send_", "list"),
            concat!("spawn_send_list_", "with"),
            concat!("CliSendRunner", "::new"),
        ] {
            let total = production.matches(needle).count();
            let inside = block.matches(needle).count();
            assert!(
                total > 0,
                "control: {needle:?} is not in production at all, so requiring it to be inside \
                 the sealed module asserts nothing"
            );
            assert_eq!(
                inside, total,
                "{needle:?} occurs {total} times in production but only {inside} of them are \
                 inside `mod send_fetch_thread`. Every mention outside that block is a blocking \
                 `bw send list` reachable from the eframe frame closure, where it freezes the \
                 window -- titlebar included -- for up to sixty seconds"
            );
        }

        // The exports. A `pub(super) fn blocking_send_list()` added here that
        // merely forwards to the private fetch would keep every count above
        // unchanged and hand the frame closure the call back.
        let exported: Vec<&str> = block
            .match_indices("pub(super) fn ")
            .map(|(at, tag)| {
                let rest = &block[at + tag.len()..];
                let end = rest.find(['(', '<', ' ']).expect("a function name ends somewhere");
                &rest[..end]
            })
            .collect();
        assert_eq!(
            exported,
            vec![concat!("spawn_send_", "list"), concat!("spawn_send_list_", "with")],
            "`mod send_fetch_thread` no longer exports exactly the two spawners. Anything else \
             reachable from outside is a way for the frame closure to obtain the blocking fetch \
             the module exists to keep away from it"
        );
        for wider in ["\n    pub fn ", "\n    pub(crate) ", "\n    pub struct ", "\n    pub use "] {
            assert!(
                !block.contains(wider),
                "`mod send_fetch_thread` contains {wider:?} -- an export wider than \
                 `pub(super)`, which is the visibility the seal is made of"
            );
        }

        // And the answer is never waited for on the frame's own thread: a
        // blocking `recv` on the Sends channel is the same sixty-second
        // freeze arrived at from the other end.
        let blocking_drain = concat!("send_rx.", "recv()");
        assert!(
            !production.contains(blocking_drain),
            "production contains {blocking_drain:?} -- the frame waits on the Sends answer \
             instead of draining it with `try_recv`, which is the window freeze the whole \
             off-thread fetch exists to prevent"
        );
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
    /// **This pins the coverage, not the gate.** Counting `if !show_sends {`
    /// alone says only that the gate exists somewhere; a second, ungated
    /// `draw_item_list` leaves that count at one and puts the item list back
    /// on the Sends screen. So the gate's own block is sliced out -- to the
    /// next `}` at the gate's indentation -- and the item list panel and its
    /// draw call are required to be inside *it*, and to exist nowhere else.
    #[test]
    fn the_item_list_is_drawn_only_inside_the_not_sends_gate() {
        let production = production();
        let gate = concat!("        if !show_", "sends {\r\n");
        assert_eq!(
            production.matches(gate).count(),
            1,
            "{gate:?} is not in production exactly once -- the Sends screen has been given a \
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

        let pane = concat!("send_ui::draw_send_", "pane(ui, state, notice_message.as_deref())");
        assert_eq!(
            production.matches(pane).count(),
            1,
            "{pane:?} is not in production exactly once -- the Sends pane is drawn from more \
             than one place, or from none"
        );
    }

    /// **Nothing outside `send.rs` can publish or revoke a Send.** The design
    /// puts revocation before publication and this step is the read-only one;
    /// a call site appearing here is the whole ordering undone.
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

    /// The two-state walk from the cut to EOF over whatever text it is handed.
    /// Returns `(visited, modules, closes, depth)` so the caller can control
    /// it for non-vacuity.
    ///
    /// **Line-ending agnostic on purpose**: `lines()` strips a trailing
    /// carriage return, so every comparison is against the line's real text on
    /// a CRLF tree and on an LF one alike.
    fn walk_below_the_cut(source: &str) -> (usize, usize, usize, usize) {
        let cut = cut_index(source);
        let mut depth = 0usize;
        // The walked region BEGINS with the gate, so nothing outside it is
        // taken on trust: the first line seen is the attribute itself.
        let mut gated = false;
        let (mut modules, mut closes, mut visited) = (0usize, 0usize, 0usize);
        for line in source[cut..].lines() {
            visited += 1;
            if depth == 0 {
                // Between modules NOTHING is allowed but blanks, comments,
                // the gate and a module opener -- at ANY indentation, because
                // an indented `fn` at file scope is still a top-level item
                // and a column-0-only filter would walk straight past it.
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with("//") {
                    continue;
                }
                if trimmed == CUT_GATE {
                    gated = true;
                    continue;
                }
                assert!(
                    !line.starts_with(char::is_whitespace) && below_cut_is_module_opener(trimmed),
                    "top-level source below the cut: {line:?}. Every pin in this module reads \
                     only the half of `mod.rs` ABOVE the cut, so an item down here is read by \
                     none of them: it can spawn a process on the eframe thread, reintroduce a \
                     blocking `list_sends`, or duplicate a call site pinned at exactly one -- \
                     and the suite stays green. Move it above the first test module."
                );
                assert!(
                    gated,
                    "the module {line:?} below the cut is not test-gated, so it SHIPS -- and it \
                     ships in the half of the file no pin here reads"
                );
                gated = false;
                depth = 1;
                modules += 1;
            } else if !line.is_empty() && !line.starts_with(char::is_whitespace) {
                // Inside a test module every item is indented, so the only
                // column-0 line is the module's own closing brace.
                if line == "}" {
                    depth = 0;
                    closes += 1;
                    continue;
                }
                assert!(
                    BELOW_CUT_STRING_LINES.contains(&line),
                    "a column-0 line inside a test module below the cut: {line:?}. Either a \
                     top-level item escaped the brace count, or this is the contents of a \
                     string literal and belongs in BELOW_CUT_STRING_LINES"
                );
            }
        }
        (visited, modules, closes, depth)
    }

    #[test]
    fn nothing_but_gated_test_modules_lives_below_the_pins_cut() {
        let source = include_str!("mod.rs");
        let lf = source.replace("\r\n", "\n");

        // 1. The cut this control walks from is the SAME byte `production`
        //    cuts at, or the walk proves nothing about the region the pins
        //    cannot see.
        let cut = cut_index(&lf);
        assert_eq!(
            lf[..cut],
            production().replace("\r\n", "\n"),
            "this control walks from a different byte than `production` cuts at, so the region \
             it inspects is not the region the pins are blind to"
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
}

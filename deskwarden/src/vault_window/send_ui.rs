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
    fn attribute_implies_test(body: &str) -> bool {
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
    fn gated_item_end(b: &[u8], from: usize) -> usize {
        let mut i = from;
        // Depths of what THIS walk opened. A closer while the matching depth
        // is zero belongs to the item's parent, and the item ends before it.
        let mut brace = 0usize;
        let mut round = 0usize;
        while i < b.len() {
            match b[i] {
                b'(' | b'[' => round += 1,
                b')' | b']' => {
                    if round == 0 && brace == 0 {
                        return i;
                    }
                    round = round.saturating_sub(1);
                }
                b'{' => brace += 1,
                b'}' => {
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
                b';' | b',' if round == 0 && brace == 0 => return i + 1,
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
        for item in [concat!("list_", "sends"), concat!("CliSendRunner::", "new")] {
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
                sites,
                vec!["vault_window/mod.rs"],
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
            for item in [concat!("list_", "sends"), concat!("CliSendRunner", "")] {
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
            squashed(&production).matches(&spawn).count(),
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
                concat!("pub(super) fn spawn_send_", "list("),
                concat!("pub(super) fn spawn_send_list_", "with<F>("),
            ],
            "`mod send_fetch_thread` no longer declares exactly the two spawners and nothing \
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

    /// **The frame closure has nothing to wait on, and no shape to wait in.**
    ///
    /// The stated property is that the frame does not wait, and until this
    /// round nothing enforced it. What enforced it was two needles aimed at
    /// the token `send_rx` and at the drain line's *indentation*, and both
    /// were beaten by measurement on `cbe915e`, at 2068 lib + 217 bin, 0
    /// failed:
    ///
    ///  * **M-2**, a spin loop written without re-indenting the drain:
    ///
    ///    ```ignore
    ///    loop {
    ///    if let Ok((tag, result)) = send_rx.try_recv() {
    ///        ..
    ///        break;
    ///    }
    ///    }
    ///    ```
    ///
    ///    `send_rx` still occurred exactly twice and the drain line still
    ///    matched the pinned string, spaces and all. The indentation pin held
    ///    nothing, because indentation is a convention and a mutation is not
    ///    obliged to follow it.
    ///
    ///  * **M-7**, which shows the drain was the wrong target altogether. The
    ///    guards constrain the token `send_rx`; nothing stopped the closure
    ///    making its own channel and blocking on that, under a spelling with
    ///    no `.recv` in it to ban:
    ///
    ///    ```ignore
    ///    let (park_tx, park_rx) = std::sync::mpsc::channel::<u8>();
    ///    let _park = park_tx;
    ///    let _ = std::sync::mpsc::Receiver::recv_timeout(
    ///        &park_rx, std::time::Duration::from_secs(60));
    ///    ```
    ///
    ///    Sixty seconds per frame. `SendListReceiver` removes the blocking
    ///    API from *one* channel; it says nothing about a second one.
    ///
    /// So this asserts over the closure's own body rather than over a line:
    ///
    ///  1. The drain sits at **brace depth 1** -- the closure's own statement
    ///     level -- counted, not inferred from leading spaces. A `loop`, a
    ///     `while` or an `if` wrapped round it puts it at 2 whatever it is
    ///     indented to, which is M-2 dead at the source.
    ///  2. The closure contains **no unbounded `loop {`** at all. A frame
    ///     that spins is a frame that waits with the CPU pinned, and the
    ///     difference between that and `recv()` is invisible to the user.
    ///  3. The closure **names no waiting primitive**: no `Receiver`, no
    ///     `mpsc`, no `recv`, no `park`, no `sleep`, no `Condvar`, no
    ///     `Barrier`. Every channel this window has is created *above* the
    ///     closure and moved in, so this is an absence that is true today for
    ///     structural reasons and not a coincidence -- and M-7 needs all
    ///     three of `mpsc`, `Receiver` and `recv` to spell itself, under any
    ///     path syntax, UFCS included.
    ///
    /// **What this is not.** It is not the behavioural hold the property
    /// deserves -- driving the real closure in a headless `Context::run_ui`
    /// harness and bounding the wall clock. That was designed and rejected
    /// for this round for two reasons, both recorded rather than hidden:
    /// `build_frame`'s second frame unconditionally calls `spawn_vault_sync`,
    /// which shells out to the `bw` CLI, so the harness cannot run without a
    /// spawn seam through `build_frame` that this round has no mandate to
    /// cut; and a frame that *spins* (M-2) never returns at all, so a
    /// wall-clock bound taken on the test's own thread would hang the suite
    /// rather than fail it, and the closure is full of `Rc<RefCell<_>>` so it
    /// cannot be watchdogged from another thread. The harness wants an
    /// injectable spawner and a cooperative deadline the closure itself
    /// checks; both are real work and belong with that seam.
    #[test]
    fn the_frame_closure_has_nothing_to_wait_on() {
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

        for shape in [concat!("loop", " {"), concat!("loop", "\r\n")] {
            assert!(
                !closure.contains(shape),
                "the frame closure contains {shape:?}. An unbounded loop inside one frame is \
                 the freeze this whole off-thread design exists to prevent, whether it spins \
                 on `try_recv` or blocks outright; the user cannot tell the two apart"
            );
        }

        // Every channel this window owns is created above the closure and
        // moved in, and the one Sends receiver is a `SendListReceiver` bound
        // out there too -- so the closure needs none of these words, and each
        // of them is a way to wait. M-7 needed three of them at once.
        //
        // Matched as a PREFIX and not as a whole word, deliberately: the
        // suffix is where the spellings hide. `recv` alone would miss
        // `recv_timeout` and `recv_deadline`; `mpsc` alone is dodged by
        // `use std::sync::mpsc as chan` written above the closure, which is
        // why `channel` and `Receiver` are here beside it. Three of these
        // words have to be absent at once for a second channel to exist at
        // all, and any one of them is enough to fail.
        for word in [
            "Receiver", "Sender", "mpsc", "channel", "recv", "park", "sleep", "Condvar",
            "Barrier", "wait",
        ] {
            let found = closure.match_indices(word).any(|(at, _)| {
                let prev = closure[..at].chars().next_back();
                !matches!(prev, Some(c) if c.is_alphanumeric() || c == '_')
            });
            assert!(
                !found,
                "the frame closure names {word:?}. Every channel this window has is built \
                 above the closure and moved in, so there is no honest reason for the word to \
                 be in here -- and a channel created *inside* the frame is one no type in \
                 `mod send_channel` constrains: `std::sync::mpsc::Receiver::recv_timeout(&rx, \
                 Duration::from_secs(60))` is a sixty-second freeze per frame that every \
                 needle aimed at `send_rx` and at `.recv(` misses"
            );
        }
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
}

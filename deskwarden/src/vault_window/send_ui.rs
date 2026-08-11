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

    /// Forget the answer, so the next frame asks again.
    ///
    /// `in_flight` is deliberately NOT cleared -- a thread is still running,
    /// and clearing it would let a second one start.
    pub fn invalidate(&mut self) {
        self.result = None;
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
    let clicked = ui
        .put(
            button_rect,
            egui::Button::new(egui::RichText::new("Copy link").size(12.0).color(theme::INK))
                .min_size(egui::vec2(COPY_BUTTON_WIDTH, COPY_BUTTON_HEIGHT)),
        )
        .clicked();
    ui.add_space(6.0);
    clicked.then(|| row.access_url.clone())
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

        let answered = SendFetch { result: Some(Ok(Vec::new())), in_flight: false };
        assert!(!answered.wants_fetch(true), "a list already in hand was fetched again");

        let failed = SendFetch {
            result: Some(Err(SendError::Offline)),
            in_flight: false,
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
        };
        assert!(!fetch.wants_fetch(true));
        fetch.invalidate();
        assert!(fetch.wants_fetch(true), "the list did not become refetchable");

        let mut running = SendFetch {
            result: Some(Err(SendError::Offline)),
            in_flight: true,
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
            SendFetch { result: Some(Ok(Vec::new())), in_flight: false }.badge_count(),
            Some(0),
            "an answered empty list must say 0, which is a fact it has"
        );
        assert_eq!(
            SendFetch {
                result: Some(Ok(vec![summary("a", false, 1), summary("b", true, 1)])),
                in_flight: false,
            }
            .badge_count(),
            Some(2)
        );
        assert_eq!(
            SendFetch { result: Some(Err(SendError::Offline)), in_flight: false }.badge_count(),
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
    //! `eframe` frame closure that only a real window runs, or inside a
    //! thread body that only a real `bw` child gives meaning to.
    //!
    //! Each needle is `concat!`-split so it cannot match its own declaration,
    //! and each is a single line, so a CRLF checkout cannot make it vacuous.
    //! Each is *required*, so the assertion is its own evidence that it still
    //! matches live code.

    fn production() -> &'static str {
        let source = include_str!("mod.rs");
        let end = source.find(concat!("#[cfg(", "test)]")).expect("no test marker");
        &source[..end]
    }

    /// `spawn_send_list`'s own body, and nothing else.
    ///
    /// **The slice matters more than the needles.** `mod.rs` already contains
    /// several `std::thread::spawn` calls and several `request_repaint`s, so
    /// a crate-wide "does this file spawn a thread" assertion is satisfied by
    /// somebody else's thread -- and a fetch moved onto the eframe thread
    /// would walk straight past it. Everything below is asserted *within this
    /// function*.
    fn spawn_send_list_body() -> &'static str {
        let production = production();
        let opener = concat!("fn spawn_send_", "list(");
        let start = production
            .find(opener)
            .expect("`spawn_send_list` is gone from production -- the Sends fetch has no home");
        let rest = &production[start + opener.len()..];
        // To the next item at column zero, which is the end of this function.
        let end = rest.find("\r\n}\r\n").unwrap_or(rest.len());
        &rest[..end]
    }

    /// **The fetch is never on the eframe thread.** `.output()` blocks, and
    /// `send.rs` gives a `bw` child a sixty-second cap, so a synchronous call
    /// would freeze the whole window -- titlebar, drag, close -- for a minute
    /// on the frame the user clicks Sends.
    #[test]
    fn the_send_list_fetch_is_spawned_and_never_run_in_the_frame() {
        let production = production();
        let spawn = concat!("spawn_send_", "list(ui.ctx().clone(), send_tx.clone())");
        assert_eq!(
            production.matches(spawn).count(),
            1,
            "{spawn:?} is not in production exactly once -- the Sends fetch must be started \
             from the frame closure and run off it"
        );

        let body = spawn_send_list_body();
        let thread = concat!("std::thread::", "spawn(move || {");
        assert!(
            body.contains(thread),
            "`spawn_send_list` no longer opens with {thread:?}, so the `bw` child is waited on \
             by whichever thread called it -- and the only caller is the frame closure. Body \
             was: {body}"
        );
        let call = concat!("crate::send::list_", "sends(&runner)");
        assert!(
            body.contains(call),
            "`spawn_send_list` does not call {call:?} -- it is not the thing fetching the list"
        );
        // ...and the blocking call is made nowhere else in the file.
        assert_eq!(
            production.matches(concat!("crate::send::list_", "sends(")).count(),
            1,
            "`list_sends` is called somewhere other than inside the spawned thread"
        );
        // The runner really is the production one. A fake here would be a
        // screen that lists nothing whatever the account holds.
        let runner = concat!("crate::send::CliSendRunner::", "new(None, data_dir.as_deref())");
        assert!(
            body.contains(runner),
            "the Sends fetch no longer runs through {runner:?}"
        );
    }

    /// The window repaints when the answer lands. Without it the result sits
    /// in the channel until some unrelated input provokes a frame, and the
    /// screen shows a spinner over a list it already has.
    #[test]
    fn the_send_fetch_asks_for_a_repaint_when_it_lands() {
        let repaint = concat!("ctx_for_sends.request_", "repaint();");
        let body = spawn_send_list_body();
        assert!(
            body.contains(repaint),
            "{repaint:?} is not in `spawn_send_list` -- a landed Sends answer would wait for an \
             unrelated frame before it was drawn"
        );
    }

    /// Leaving the screen drops the list. This is the refetch policy, and it
    /// is an **absence** in the wrong shape: with the call deleted, every
    /// pure test in this file still passes and the only symptom is a Sends
    /// list that silently stops matching the server.
    #[test]
    fn leaving_the_sends_screen_invalidates_the_list() {
        let production = production();
        let call = concat!("send_ui::should_invalidate_on_", "leave(was_on_sends, on_sends)");
        assert_eq!(
            production.matches(call).count(),
            1,
            "{call:?} is not in production exactly once -- the Sends list is never refreshed, so \
             a Send deleted or expired elsewhere keeps its Copy link button forever"
        );
        let apply = concat!("send_fetch.invalidate", "();");
        assert!(
            production.contains(apply),
            "the leave rule is consulted and its answer thrown away"
        );
    }

    /// The Sends screen replaces the item list rather than being drawn beside
    /// it, and the item list is not asked to render Sends.
    #[test]
    fn the_item_list_is_not_drawn_on_the_sends_screen() {
        let production = production();
        let gate = concat!("if !show_", "sends {");
        assert_eq!(
            production.matches(gate).count(),
            1,
            "{gate:?} is not in production exactly once -- the item list panel is drawn on the \
             Sends screen, or the Sends screen has been given a second gate"
        );
        let pane = concat!("send_ui::draw_send_", "pane(ui, state, notice_message.as_deref())");
        assert_eq!(
            production.matches(pane).count(),
            1,
            "{pane:?} is not in production exactly once -- the Sends pane is drawn from more \
             than one place, or from none"
        );
    }
}

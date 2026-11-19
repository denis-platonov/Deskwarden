//! Spec A section 4 and spec B section 7: the **Shared** page -- a header, a
//! two-column grid of cards -- and the **Send detail** modal a card opens.
//!
//! This replaced 0.15's two columns (a list of rows, a detail pane painted
//! into hand-measured rectangles). Everything those could do is still done,
//! from the same [`super::SendUiAction`]s: a card copies the link or asks to
//! revoke it; the modal copies, extends, asks and confirms a revoke, opens the
//! record and changes a seen password; a person share still has its
//! two-step Stop sharing; a received import still says whether its item
//! survived; a record somebody shares still opens in the vault.
//!
//! What the design draws that a Bitwarden Send cannot supply stays out: there
//! is no per-open timeline (a Send keeps a count and nothing about who or
//! when), so the Activity list is built from what is known -- see
//! [`activity_events`].

use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, Margin, Rect, Sense, Stroke, StrokeKind};

use super::{
    elided, paint_send_tile, ReceivedRow, RecordTiles, SendDeleteView, SendPaneState, SendRow, SendUiAction, SendView,
};
use crate::kit::controls::{self, ButtonKind, ButtonSize};
use crate::kit::modal::{self, Dialog, ModalOutcome, Tone};
use crate::kit::tips::{self, Placement};
use crate::kit::tokens::{self, Palette, Role};
use crate::theme;
use crate::vault_window::share_ui::{self as sharing, PeopleAsk, PeopleRows, ReceivedShareRow, ShareRow};

/// The Send detail modal's id; its scrim is `"send-detail-scrim"`.
pub const ID: &str = "send-detail";

/// Spec A 4: the page's title, its sub-line and the `?` bubble's words.
pub const TITLE: &str = "Shared";
pub const RECEIVED_TITLE: &str = "Shared with me";
pub const SUB: &str = "Links you've sent. Revoke any of them instantly.";
pub const STATUS_TIP: &str = "Waiting \u{2014} nobody has opened it yet. Opened \u{2014} the person has seen it. Links stop working when they expire or you revoke them.";
/// The page's padding and the grid's gap (spec A 4).
const PAGE_PAD_X: i8 = 40;
const PAGE_PAD_Y: i8 = 32;
const GRID_GAP: f32 = 16.0;
/// Below this much room the grid is one column.
const TWO_COLUMNS_FROM: f32 = 560.0;

// ---- a card (spec A 4) -----------------------------------------------------

const CARD_RADIUS: u8 = 18;
const CARD_PAD: f32 = 20.0;
const AVATAR: f32 = 46.0;
const CARD_GAP: f32 = 16.0;
const META_HEIGHT: f32 = 20.0;
const ACTION_HEIGHT: f32 = 42.0;
/// An ended card is drawn back (spec A 4: opacity .62).
const ENDED_OPACITY: f32 = 0.62;

/// What a card shows.
struct Face<'a> {
    key: &'a str,
    record: Option<&'a str>,
    name: &'a str,
    line: String,
    pill: Option<(String, Color32, Color32)>,
    clock: Option<String>,
    eye: Option<String>,
    ended: bool,
    /// Copy link and Revoke along the foot (a live Send only).
    actions: bool,
    /// What hovering the card says, if anything.
    hover: Option<&'a str>,
    /// A revoke or an extend is running on it: said in place of its meta
    /// row, and no button is drawn (a button here is one more thing to press
    /// while it runs).
    busy: Option<&'static str>,
}

#[derive(Default)]
struct Hit {
    opened: bool,
    copy: bool,
    revoke: bool,
}

fn card_height(face: &Face<'_>) -> f32 {
    let meta = face.clock.is_some() || face.eye.is_some() || face.busy.is_some();
    CARD_PAD * 2.0
        + AVATAR
        + if meta { CARD_GAP + META_HEIGHT } else { 0.0 }
        + if face.actions { CARD_GAP + ACTION_HEIGHT } else { 0.0 }
}

fn bold(px: f32) -> egui::FontId {
    egui::FontId::new(px, egui::FontFamily::Name(theme::BOLD.into()))
}

fn semibold(px: f32) -> egui::FontId {
    egui::FontId::new(px, egui::FontFamily::Name(theme::SEMIBOLD.into()))
}

fn regular(px: f32) -> egui::FontId {
    egui::FontId::new(px, egui::FontFamily::Proportional)
}

/// A status pill: 13 bold, `5 11` padding, fully round (spec A 4). Right edge
/// at `right`, centred on `y`; answers the left edge.
fn pill(painter: &egui::Painter, right: f32, y: f32, label: &str, fill: Color32, ink: Color32) -> f32 {
    let galley = painter.layout_no_wrap(label.to_string(), bold(13.0), ink);
    let size = vec2(galley.size().x + 22.0, galley.size().y + 10.0);
    let rect = Rect::from_min_size(pos2(right - size.x, y - size.y / 2.0), size);
    painter.rect_filled(rect, CornerRadius::same(255), fill);
    painter.galley(pos2(rect.left() + 11.0, rect.top() + 5.0), galley, ink);
    rect.left()
}

/// A 16 px clock: a ring and its two hands, drawn by hand (the kit has no
/// clock glyph).
fn paint_clock(painter: &egui::Painter, rect: Rect, ink: Color32) {
    let c = rect.center();
    let r = rect.width() / 2.0 - 1.0;
    let stroke = Stroke::new(1.6, ink);
    painter.circle_stroke(c, r, stroke);
    painter.line_segment([c, pos2(c.x, c.y - r * 0.55)], stroke);
    painter.line_segment([c, pos2(c.x + r * 0.45, c.y + r * 0.3)], stroke);
}

fn paint_eye(painter: &egui::Painter, rect: Rect, ink: Color32) {
    crate::kit::icon::stroke(painter, rect, crate::kit::icon::glyph::EYE, 1.8, ink);
}

fn paint_chevron(painter: &egui::Painter, rect: Rect, ink: Color32) {
    crate::kit::icon::stroke(painter, rect, crate::kit::icon::glyph::CHEVRON_RIGHT, 2.0, ink);
}

/// One of the card's two foot buttons: a 42 high slab, 15 type. `danger`
/// is the red-edged one.
fn foot_button(ui: &mut egui::Ui, rect: Rect, id: egui::Id, label: &str, danger: bool) -> bool {
    let p = tokens::palette(ui.ctx());
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = response.hovered();
    let radius = CornerRadius::same(12);
    let (face, ink) = if danger {
        let fill = if hovered { p.danger_wash } else { p.card };
        ui.painter().rect(rect, radius, fill, Stroke::new(1.0, p.danger_edge), StrokeKind::Inside);
        (bold(15.0), p.error)
    } else {
        let (fill, ink) = if hovered { (p.blue_wash, p.blue_deep) } else { (p.field_bg, p.ink) };
        ui.painter().rect_filled(rect, radius, fill);
        (semibold(15.0), ink)
    };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, face, ink);
    response.clicked()
}

/// Paints one card in `rect` and reports what was pressed.
fn paint_card(ui: &mut egui::Ui, rect: Rect, face: &Face<'_>, records: RecordTiles<'_>) -> Hit {
    let p = tokens::palette(ui.ctx());
    let mut hit = Hit::default();
    let response = ui.interact(rect, egui::Id::new(("shared-card", face.key)), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let response = match face.hover {
        Some(tip) => response.on_hover_text(tip),
        None => response,
    };
    hit.opened = response.clicked();

    let opacity = if face.ended { ENDED_OPACITY } else { 1.0 };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.multiply_opacity(opacity);
    let ui = &mut child;
    let painter = ui.painter().clone();
    painter.add(tokens::card_shadow().as_shape(rect, CornerRadius::same(CARD_RADIUS)));
    painter.rect_filled(rect, CornerRadius::same(CARD_RADIUS), p.card);
    painter.rect_stroke(rect, CornerRadius::same(CARD_RADIUS), Stroke::new(1.0, p.card_ring), StrokeKind::Inside);

    // The top row: avatar, name over recipient, pill, chevron.
    let top = rect.top() + CARD_PAD;
    let tile = Rect::from_min_size(pos2(rect.left() + CARD_PAD, top), vec2(AVATAR, AVATAR));
    paint_send_tile(ui, tile, records, face.record, face.name, true, 1.0);
    let chevron = Rect::from_min_size(pos2(rect.right() - CARD_PAD - 18.0, top + (AVATAR - 18.0) / 2.0), vec2(18.0, 18.0));
    paint_chevron(&painter, chevron, p.text_ghost);
    let mut text_right = chevron.left() - 14.0;
    if let Some((label, fill, ink)) = &face.pill {
        text_right = pill(&painter, text_right, top + AVATAR / 2.0, label, *fill, *ink) - 14.0;
    }
    let text_left = tile.right() + 14.0;
    let clip = painter.with_clip_rect(
        Rect::from_min_max(pos2(text_left, rect.top()), pos2(text_right.max(text_left), rect.bottom())).intersect(ui.clip_rect()),
    );
    let width = (text_right - text_left).max(0.0);
    let name = elided(&clip, face.name, bold(17.0), p.ink, width);
    let line = elided(&clip, &face.line, regular(14.0), p.text_faint, width);
    let block = name.size().y + 2.0 + line.size().y;
    let y = top + (AVATAR - block) / 2.0;
    let line_y = y + name.size().y + 2.0;
    clip.galley(pos2(text_left, y), name, p.ink);
    clip.galley(pos2(text_left, line_y), line, p.text_faint);

    // The meta row: when it ends, and how often it was opened.
    let meta_y = top + AVATAR + CARD_GAP + META_HEIGHT / 2.0;
    let mut x = rect.left() + CARD_PAD;
    if let Some(busy) = face.busy {
        painter.text(pos2(x, meta_y), egui::Align2::LEFT_CENTER, busy, semibold(14.0), p.text_faint);
    } else if let Some(text) = &face.clock {
        paint_clock(&painter, Rect::from_center_size(pos2(x + 8.0, meta_y), vec2(16.0, 16.0)), p.text_label);
        let at = painter.text(pos2(x + 24.0, meta_y), egui::Align2::LEFT_CENTER, text, regular(14.0), p.text_nav);
        x = at.right() + 24.0;
    }
    if let (None, Some(text)) = (face.busy, &face.eye) {
        paint_eye(&painter, Rect::from_center_size(pos2(x + 8.0, meta_y), vec2(16.0, 16.0)), p.text_label);
        painter.text(pos2(x + 24.0, meta_y), egui::Align2::LEFT_CENTER, text, regular(14.0), p.text_nav);
    }

    // The foot: Copy link and Revoke, over the card's own click.
    if face.actions {
        let top = rect.bottom() - CARD_PAD - ACTION_HEIGHT;
        let w = (rect.width() - CARD_PAD * 2.0 - 8.0) / 2.0;
        let copy = Rect::from_min_size(pos2(rect.left() + CARD_PAD, top), vec2(w, ACTION_HEIGHT));
        let revoke = Rect::from_min_size(pos2(copy.right() + 8.0, top), vec2(w, ACTION_HEIGHT));
        hit.copy = foot_button(ui, copy, egui::Id::new(("shared-card-copy", face.key)), super::COPY_LINK_LABEL, false);
        hit.revoke = foot_button(ui, revoke, egui::Id::new(("shared-card-revoke", face.key)), super::REVOKE_LABEL, true);
        hit.opened = hit.opened && !hit.copy && !hit.revoke;
    }
    hit
}

// ---- words ---------------------------------------------------------------

/// "Not opened yet", "Opened once", "3 of 10 opens", "Opened 3 times" (spec
/// A 4's `views`).
pub fn views_words(row: &SendRow) -> String {
    match (row.max_access_count, row.access_count) {
        (_, 0) => "Not opened yet".to_string(),
        (Some(cap), used) => format!("{used} of {cap} opens"),
        (None, 1) => "Opened once".to_string(),
        (None, n) => format!("Opened {n} times"),
    }
}

/// "Ends in 6 days", "Ended 2 days ago", "Switched off": spec A 4's `exp`.
/// The relative half of the dates the row already carries, so the card and
/// the detail say one thing.
pub fn ends_words(row: &SendRow) -> String {
    use crate::send::SendState;
    match row.state {
        SendState::Revoked => return "Switched off".to_string(),
        SendState::Used => return "All views used".to_string(),
        _ => {}
    }
    let relative = |text: &str| text.split_once(" \u{b7} ").map(|(_, rel)| rel.to_string());
    let rel = relative(&row.expires).or_else(|| relative(&row.deletes));
    match (rel, row.state.is_live()) {
        (Some(rel), true) => format!("Ends {rel}"),
        (Some(rel), false) => format!("Ended {rel}"),
        (None, true) => "Lasts until deleted".to_string(),
        (None, false) => "Ended".to_string(),
    }
}

/// Who a Send went to: the name it was sent to, else "Anyone with the link".
fn recipient_line(row: &SendRow) -> String {
    let who = row
        .sent
        .as_ref()
        .map(|s| s.recipient.as_str())
        .filter(|r| !r.is_empty())
        .unwrap_or("Anyone with the link");
    if row.is_file {
        format!("{who} \u{b7} File Send")
    } else {
        who.to_string()
    }
}

/// What the Includes tile says: the record's fields, or the kind of Send.
fn includes_words(row: &SendRow) -> String {
    match &row.sent {
        Some(sent) if !sent.fields.is_empty() => sent.fields.join(", "),
        Some(_) => "\u{2014}".to_string(),
        None if row.is_file => "A file".to_string(),
        None => "Text".to_string(),
    }
}

fn send_face<'a>(row: &'a SendRow, delete: SendDeleteView<'_>) -> Face<'a> {
    let tone = super::state_tone(row.state);
    let busy = (!row.id.is_empty() && delete.in_flight == Some(row.id.as_str())).then(|| delete.busy_label());
    Face {
        key: &row.id,
        record: row.sent.as_ref().map(|s| s.item_id.as_str()),
        name: &row.name,
        line: recipient_line(row),
        pill: Some((super::state_label(row.state).to_string(), tone.fill, tone.ink)),
        clock: Some(ends_words(row)),
        eye: Some(views_words(row)),
        ended: !row.state.is_live(),
        actions: row.state.is_live() && !row.id.is_empty() && busy.is_none(),
        hover: None,
        busy,
    }
}

fn share_face<'a>(share: &'a ShareRow) -> Face<'a> {
    let kind = share.pill();
    let tone = kind.tone();
    Face {
        key: &share.key,
        record: Some(&share.item_id),
        name: &share.name,
        line: share.subtitle.clone(),
        pill: Some((kind.label().to_string(), tone.fill, tone.ink)),
        clock: share.since.as_ref().map(|s| format!("Since {s}")),
        eye: None,
        ended: kind == theme::SharePillKind::Gone,
        actions: false,
        hover: None,
        busy: None,
    }
}

fn person_face<'a>(row: &'a ReceivedShareRow, subtitle: &str) -> Face<'a> {
    Face {
        key: &row.key,
        record: Some(&row.item_id),
        name: &row.name,
        line: subtitle.to_string(),
        pill: None,
        clock: None,
        eye: None,
        ended: false,
        actions: false,
        hover: (!row.verified).then_some(crate::vault_window::detail::UNVERIFIED_SHARER_TOOLTIP),
        busy: None,
    }
}

fn received_face(row: &ReceivedRow) -> Face<'_> {
    let (word, fill, ink) = if row.still_in_vault {
        ("In your vault", Palette::LIGHT.done_wash, Palette::LIGHT.done_ink)
    } else {
        ("Gone", Palette::LIGHT.hover, Palette::LIGHT.text_faint)
    };
    Face {
        key: &row.key,
        record: None,
        name: &row.name,
        line: super::RECEIVED_KIND.to_string(),
        pill: Some((word.to_string(), fill, ink)),
        clock: Some(row.when.clone()),
        eye: None,
        ended: !row.still_in_vault,
        actions: false,
        hover: None,
        busy: None,
    }
}

// ---- the page ----------------------------------------------------------------

/// One card to draw, and what its press means.
enum Entry<'a> {
    Send(&'a SendRow),
    Share(&'a ShareRow),
    Person(&'a ReceivedShareRow),
    Received(&'a ReceivedRow),
}

/// Draws the Shared page and its modal; answers what was pressed.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    ui: &mut egui::Ui,
    state: &SendPaneState,
    view: &SendView<'_>,
    delete: SendDeleteView<'_>,
    selected: &mut Option<String>,
    records: RecordTiles<'_>,
    people: &mut PeopleRows<'_>,
) -> Option<SendUiAction> {
    let mut action: Option<SendUiAction> = None;
    let p = tokens::palette(ui.ctx());

    egui::ScrollArea::vertical().id_salt("shared-page").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(Margin::symmetric(PAGE_PAD_X, PAGE_PAD_Y)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            header(ui, view, people);
            ui.add_space(24.0);
            match view {
                SendView::Mine(scope) => {
                    let shares: &[ShareRow] =
                        if *scope == crate::vault_window::sidebar::SendScope::All { people.shares } else { &[] };
                    let mut entries: Vec<Entry<'_>> = shares.iter().map(Entry::Share).collect();
                    match state {
                        SendPaneState::Rows(rows) => {
                            let in_scope = super::rows_in_scope(rows, *scope);
                            if in_scope.is_empty() && shares.is_empty() {
                                let head = super::scope_empty_headline(*scope);
                                placeholder(ui, &head, super::SCOPE_EMPTY_DETAIL, p.ink);
                            }
                            entries.extend(in_scope.into_iter().map(Entry::Send));
                            grid(ui, &entries, delete, selected, records, people, &mut action);
                        }
                        SendPaneState::Empty => {
                            if shares.is_empty() {
                                placeholder(ui, super::EMPTY_HEADLINE, super::EMPTY_DETAIL, p.ink);
                            } else {
                                grid(ui, &entries, delete, selected, records, people, &mut action);
                            }
                        }
                        SendPaneState::Loading => {
                            grid(ui, &entries, delete, selected, records, people, &mut action);
                            ui.add_space(16.0);
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(16.0).color(p.blue));
                                ui.add_space(8.0);
                                ui.label(tokens::job(
                                    Role { size: 14.0, face: theme::REGULAR, tracking_em: 0.0 },
                                    super::LOADING_LABEL,
                                    p.text_faint,
                                ));
                            });
                        }
                        SendPaneState::Failed { message, ambiguous } => {
                            grid(ui, &entries, delete, selected, records, people, &mut action);
                            ui.add_space(16.0);
                            if let Some(retry) = failure(ui, message, *ambiguous) {
                                action = Some(retry);
                            }
                        }
                    }
                    // What the screen cannot do, said once and quietly.
                    let file_note = matches!(state, SendPaneState::Rows(rows)
                        if super::rows_in_scope(rows, *scope).iter().any(|r| r.is_file));
                    footnotes(ui, view, shares.is_empty(), file_note);
                }
                SendView::Received(rows) => {
                    received_page(ui, rows, delete, selected, records, people, &mut action);
                }
            }
        });
    });

    // The modal, over the page, for whichever card is picked.
    if let Some(key) = selected.clone() {
        let (outcome, pressed, asking) = match view {
            SendView::Mine(scope) => {
                let shares: &[ShareRow] =
                    if *scope == crate::vault_window::sidebar::SendScope::All { people.shares } else { &[] };
                let send = match state {
                    SendPaneState::Rows(rows) => rows.iter().find(|r| !r.id.is_empty() && r.id == key),
                    _ => None,
                };
                if let Some(share) = shares.iter().find(|s| s.key == key) {
                    let (outcome, pressed) = share_modal(ui.ctx(), share, people, records);
                    (outcome, pressed, false)
                } else if let Some(row) = send {
                    let (outcome, pressed) = send_modal(ui.ctx(), row, delete, records);
                    (outcome, pressed, delete.confirming == Some(row.id.as_str()))
                } else {
                    // Nothing this key names is listed (yet, or any more).
                    (ModalOutcome::Open, None, false)
                }
            }
            SendView::Received(rows) => match rows.iter().find(|r| r.key == key) {
                Some(row) => (received_modal(ui.ctx(), row), None, false),
                None => (ModalOutcome::Open, None, false),
            },
        };
        if pressed.is_some() {
            action = pressed;
        }
        if outcome == ModalOutcome::Closed {
            *selected = None;
            if asking {
                action = Some(SendUiAction::CancelDelete);
            }
        }
    }
    action
}

fn header(ui: &mut egui::Ui, view: &SendView<'_>, _people: &PeopleRows<'_>) {
    let p = tokens::palette(ui.ctx());
    let (title, sub) = match view {
        SendView::Mine(scope) => {
            (if *scope == crate::vault_window::sidebar::SendScope::All { TITLE } else { scope.label() }, scope.gloss().unwrap_or(SUB))
        }
        SendView::Received(_) if !_people.received.is_empty() || !_people.waiting.is_empty() => {
            (RECEIVED_TITLE, super::RECEIVED_PEOPLE_SUBTEXT)
        }
        SendView::Received(_) => (RECEIVED_TITLE, super::RECEIVED_SCOPE_SUBTEXT),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(tokens::job(tokens::PAGE_TITLE, title, p.ink));
        if matches!(view, SendView::Mine(_)) {
            let _ = tips::help_dot_sized(ui, egui::Id::new("shared-status-tip"), STATUS_TIP, 280.0, Placement::Below, 20.0);
        }
    });
    ui.add_space(6.0);
    ui.add(egui::Label::new(tokens::job(Role { size: 16.0, face: theme::REGULAR, tracking_em: 0.0 }, sub, p.text_faint)).wrap());
}

/// The centred empty-state words: 17 bold over 15 regular.
fn placeholder(ui: &mut egui::Ui, head: &str, detail: &str, ink: Color32) {
    let p = tokens::palette(ui.ctx());
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(theme::bold(head, 17.0).color(ink));
        ui.add(egui::Label::new(egui::RichText::new(detail).size(15.0).color(p.text_faint)).wrap());
    });
}

/// The failed fetch: its headline, the engine's sentence, and Try again.
fn failure(ui: &mut egui::Ui, message: &str, ambiguous: bool) -> Option<SendUiAction> {
    let p = tokens::palette(ui.ctx());
    let mut action = None;
    ui.label(theme::bold(super::FAILED_HEADLINE, 15.0).color(p.error));
    ui.add_space(4.0);
    ui.add(egui::Label::new(egui::RichText::new(message).size(14.0).color(p.text_muted)).wrap());
    if ambiguous {
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(super::AMBIGUOUS_DETAIL).size(14.0).color(p.text_muted)).wrap());
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if controls::button(ui, ButtonKind::Outline, controls::SMALL, super::TRY_AGAIN_LABEL, None).clicked() {
            action = Some(SendUiAction::Refresh);
        }
    });
    action
}

/// The two standing statements the 0.15 list kept under its column.
fn footnotes(ui: &mut egui::Ui, view: &SendView<'_>, no_shares: bool, file_note: bool) {
    let p = tokens::palette(ui.ctx());
    let gloss = if no_shares { super::SCOPE_SUBTEXT } else { super::SHARES_SUBTEXT };
    let _ = view;
    ui.add_space(24.0);
    ui.add(egui::Label::new(egui::RichText::new(gloss).size(14.0).color(p.text_ghost)).wrap());
    if file_note {
        ui.add_space(6.0);
        ui.add(egui::Label::new(egui::RichText::new(super::FILE_SEND_EXPLANATION).size(14.0).color(p.text_ghost)).wrap());
    }
}

/// Lays `entries` out two to a row (one on a narrow window) and draws them.
fn grid(
    ui: &mut egui::Ui,
    entries: &[Entry<'_>],
    delete: SendDeleteView<'_>,
    selected: &mut Option<String>,
    records: RecordTiles<'_>,
    people: &mut PeopleRows<'_>,
    action: &mut Option<SendUiAction>,
) {
    let _ = people;
    let width = ui.available_width();
    let cols = if width >= TWO_COLUMNS_FROM { 2 } else { 1 };
    let card_w = (width - GRID_GAP * (cols as f32 - 1.0)) / cols as f32;
    let subtitles: Vec<String> = entries
        .iter()
        .map(|e| match e {
            Entry::Person(row) => sharing::received_subtitle(row),
            _ => String::new(),
        })
        .collect();
    for (r, chunk) in entries.chunks(cols).enumerate() {
        let faces: Vec<Face<'_>> = chunk
            .iter()
            .enumerate()
            .map(|(i, e)| match e {
                Entry::Send(row) => send_face(row, delete),
                Entry::Share(share) => share_face(share),
                Entry::Person(row) => person_face(row, &subtitles[r * cols + i]),
                Entry::Received(row) => received_face(row),
            })
            .collect();
        let height = faces.iter().map(card_height).fold(0.0, f32::max);
        let (row_rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        for (i, (face, entry)) in faces.iter().zip(chunk).enumerate() {
            let rect = Rect::from_min_size(pos2(row_rect.left() + i as f32 * (card_w + GRID_GAP), row_rect.top()), vec2(card_w, height));
            let hit = paint_card(ui, rect, face, records);
            match entry {
                Entry::Send(row) => {
                    if hit.copy && !row.access_url.is_empty() {
                        *action = Some(SendUiAction::CopyLink(row.access_url.clone()));
                    } else if hit.revoke && !row.id.is_empty() {
                        // The question is asked in the modal, so the card
                        // that asked it is the one the modal opens on.
                        *selected = Some(row.id.clone());
                        *action = Some(SendUiAction::AskDelete(row.id.clone()));
                    } else if hit.opened {
                        *selected = Some(row.id.clone());
                    }
                }
                Entry::Share(share) => {
                    if hit.opened {
                        *selected = Some(share.key.clone());
                    }
                }
                Entry::Person(row) => {
                    if hit.opened {
                        *action = Some(SendUiAction::OpenRecord(row.item_id.clone()));
                    }
                }
                Entry::Received(row) => {
                    if hit.opened {
                        *selected = Some(row.key.clone());
                    }
                }
            }
        }
        ui.add_space(GRID_GAP);
    }
}

/// An eyebrow over a group of cards (spec 6d's FROM PEOPLE and IMPORTED).
fn group_label(ui: &mut egui::Ui, text: &str) {
    let p = tokens::palette(ui.ctx());
    ui.label(tokens::job(tokens::SECTION, text, p.text_ghost));
    ui.add_space(10.0);
}

fn received_page(
    ui: &mut egui::Ui,
    rows: &[ReceivedRow],
    delete: SendDeleteView<'_>,
    selected: &mut Option<String>,
    records: RecordTiles<'_>,
    people: &mut PeopleRows<'_>,
    action: &mut Option<SendUiAction>,
) {
    let p = tokens::palette(ui.ctx());
    let width = ui.available_width();
    // Paste an invite link, and the sharers still being waited on.
    if people.paste.is_some() || !people.waiting.is_empty() {
        sharing::draw_paste_and_waiting(ui, people, width);
        ui.add_space(16.0);
    }
    let from_people = !people.received.is_empty() || !people.waiting.is_empty();
    if rows.is_empty() && people.received.is_empty() && people.waiting.is_empty() {
        placeholder(ui, super::RECEIVED_EMPTY_HEADLINE, super::RECEIVED_EMPTY_DETAIL, p.ink);
        return;
    }
    let received = people.received;
    if !received.is_empty() {
        group_label(ui, sharing::FROM_PEOPLE_EYEBROW);
        // Each organisation's records stand together, and a hairline parts
        // one organisation's from the next: two that claim one sharer are two
        // groups, not one sharer's.
        for (i, group) in received.chunk_by(|a, b| a.organization_id == b.organization_id).enumerate() {
            if i > 0 {
                ui.add_space(4.0);
                let (rule, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(rule, CornerRadius::ZERO, p.hairline);
                ui.add_space(GRID_GAP + 4.0);
            }
            let entries: Vec<Entry<'_>> = group.iter().map(Entry::Person).collect();
            grid(ui, &entries, delete, selected, records, people, action);
        }
    }
    if !rows.is_empty() {
        if from_people {
            group_label(ui, sharing::FROM_SENDS_EYEBROW);
        }
        let entries: Vec<Entry<'_>> = rows.iter().map(Entry::Received).collect();
        grid(ui, &entries, delete, selected, records, people, action);
    }
}

// ---- the modal's parts ---------------------------------------------------------

/// The dialog's header: a 56 tile, the name at 24/800, a line under it and
/// a pill at the right, clear of the close mark.
fn modal_header(
    ui: &mut egui::Ui,
    records: RecordTiles<'_>,
    record: Option<&str>,
    name: &str,
    line: &str,
    pill_face: Option<(String, Color32, Color32)>,
) {
    let p = tokens::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
    let tile = Rect::from_min_size(rect.min, vec2(56.0, 56.0));
    paint_send_tile(ui, tile, records, record, name, true, 1.0);
    let mut right = rect.right() - 44.0;
    let painter = ui.painter().clone();
    if let Some((label, fill, ink)) = &pill_face {
        right = pill(&painter, right, rect.center().y, label, *fill, *ink) - 14.0;
    }
    let left = tile.right() + 16.0;
    let clip = painter.with_clip_rect(Rect::from_min_max(pos2(left, rect.top()), pos2(right.max(left), rect.bottom())));
    let name_g = elided(&clip, name, egui::FontId::new(24.0, egui::FontFamily::Name(theme::EXTRABOLD.into())), p.ink, (right - left).max(0.0));
    let line_g = elided(&clip, line, regular(15.0), p.text_faint, (right - left).max(0.0));
    let block = name_g.size().y + 2.0 + line_g.size().y;
    let y = rect.center().y - block / 2.0;
    let line_y = y + name_g.size().y + 2.0;
    clip.galley(pos2(left, y), name_g, p.ink);
    clip.galley(pos2(left, line_y), line_g, p.text_faint);
}

/// Spec B 7's three fact tiles: a label over a value, on `#f7f6f5`.
fn fact_tiles(ui: &mut egui::Ui, facts: &[(&str, String)]) {
    let p = tokens::palette(ui.ctx());
    let width = ui.available_width();
    let gap = 10.0;
    // Each tile is as wide as its words want (one line), the spare room split
    // evenly; where the words do not fit one line each, the tiles are equal
    // and the values wrap.
    let wants: Vec<f32> = facts
        .iter()
        .map(|(label, value)| {
            let one = |text: &str, font: egui::FontId| ui.painter().layout_no_wrap(text.to_string(), font, p.ink).size().x;
            one(label, semibold(13.0)).max(one(value, bold(16.0))) + 32.0
        })
        .collect();
    let room = width - gap * (facts.len() as f32 - 1.0);
    let widths: Vec<f32> = if wants.iter().sum::<f32>() <= room {
        let spare = (room - wants.iter().sum::<f32>()) / facts.len() as f32;
        wants.iter().map(|w| w + spare).collect()
    } else {
        vec![room / facts.len() as f32; facts.len()]
    };
    let galleys: Vec<_> = facts
        .iter()
        .zip(&widths)
        .map(|((label, value), w)| {
            (
                ui.painter().layout_no_wrap(label.to_string(), semibold(13.0), p.text_label),
                ui.painter().layout(value.clone(), bold(16.0), p.ink, (w - 32.0).max(0.0)),
            )
        })
        .collect();
    let height = galleys.iter().map(|(l, v)| 28.0 + l.size().y + 4.0 + v.size().y).fold(0.0, f32::max);
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let mut left = row.left();
    for ((label, value), w) in galleys.into_iter().zip(widths) {
        let rect = Rect::from_min_size(pos2(left, row.top()), vec2(w, height));
        left += w + gap;
        ui.painter().rect_filled(rect, CornerRadius::same(14), p.secondary_hover);
        let label_h = label.size().y;
        ui.painter().galley(pos2(rect.left() + 16.0, rect.top() + 14.0), label, p.text_label);
        ui.painter().galley(pos2(rect.left() + 16.0, rect.top() + 14.0 + label_h + 4.0), value, p.ink);
    }
}

/// What a detail row's value is.
enum Value<'a> {
    Plain(&'a str),
    /// A URL: mono, struck through once the link is dead.
    Address { url: &'a str, dead: bool },
    /// A count against a cap, with the 90x4 meter.
    Views { used: u32, cap: Option<u32> },
}

/// Label / value rows in a white card with the design's ring, a rule between.
fn detail_rows(ui: &mut egui::Ui, rows: &[(&str, Value<'_>)]) {
    let p = tokens::palette(ui.ctx());
    let width = ui.available_width();
    let label_w = 96.0;
    let pad = 16.0;
    let mut layout: Vec<(std::sync::Arc<egui::Galley>, std::sync::Arc<egui::Galley>, f32)> = Vec::new();
    for (label, value) in rows {
        let l = ui.painter().layout_no_wrap(label.to_string(), semibold(14.0), p.text_label);
        let room = width - pad * 2.0 - label_w;
        let v = match value {
            Value::Plain(text) => ui.painter().layout(text.to_string(), semibold(15.0), p.ink, room),
            Value::Address { url, dead } => {
                let mut job = egui::text::LayoutJob::simple(
                    url.to_string(),
                    egui::FontId::new(13.0, egui::FontFamily::Monospace),
                    if *dead { p.text_ghost } else { p.ink },
                    room - if *dead { 40.0 } else { 0.0 },
                );
                if *dead {
                    job.sections[0].format.strikethrough = Stroke::new(1.0, p.text_ghost);
                }
                ui.painter().layout_job(job)
            }
            Value::Views { used, cap } => {
                let text = match cap {
                    Some(cap) => format!("{used} of {cap}"),
                    None => format!("{used}"),
                };
                ui.painter().layout_no_wrap(text, bold(15.0), p.ink)
            }
        };
        let h = v.size().y.max(l.size().y) + 24.0;
        layout.push((l, v, h));
    }
    let height: f32 = layout.iter().map(|(_, _, h)| *h).sum();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(tokens::R_CARD), p.card, Stroke::new(1.0, p.card_ring), StrokeKind::Inside);
    let mut y = rect.top();
    for (i, ((l, v, h), (_, value))) in layout.into_iter().zip(rows).enumerate() {
        if i > 0 {
            ui.painter().hline(rect.left() + pad..=rect.right() - pad, y, Stroke::new(1.0, p.row_rule));
        }
        let mid = y + h / 2.0;
        let (lh, vh) = (l.size().y, v.size().y);
        let value_x = rect.left() + pad + label_w;
        ui.painter().galley(pos2(rect.left() + pad, mid - lh / 2.0), l, p.text_label);
        let v_right = value_x + v.size().x;
        ui.painter().galley(pos2(value_x, mid - vh / 2.0), v, p.ink);
        match value {
            Value::Address { dead: true, .. } => {
                ui.painter().text(pos2(rect.right() - pad, mid), egui::Align2::RIGHT_CENTER, "dead", regular(13.0), p.text_ghost);
            }
            Value::Views { used, cap: Some(cap) } if *cap > 0 => {
                let track = Rect::from_min_size(pos2(v_right + 12.0, mid - 2.0), vec2(90.0, 4.0));
                ui.painter().rect_filled(track, CornerRadius::same(2), p.meter_track);
                let share = (*used as f32 / *cap as f32).clamp(0.0, 1.0);
                ui.painter().rect_filled(
                    Rect::from_min_size(track.min, vec2(track.width() * share, 4.0)),
                    CornerRadius::same(2),
                    p.blue,
                );
            }
            _ => {}
        }
        y += h;
    }
}

/// The events a Send's Activity lists, oldest first: `(title, detail)`. A
/// Bitwarden Send keeps no per-open log, so this is what is known -- who it
/// went to, how often it was opened, and how it ends -- not an invented
/// history; the last entry is the one the blue dot marks.
pub fn activity_events(row: &SendRow) -> Vec<(String, String)> {
    use crate::send::SendState;
    let mut events = vec![("Created by you".to_string(), String::new())];
    if let Some(who) = row.sent.as_ref().map(|s| s.recipient.as_str()).filter(|r| !r.is_empty()) {
        events.push((format!("Link sent to {who}"), String::new()));
    }
    let opened = match row.access_count {
        0 => "Not opened yet".to_string(),
        1 => "Opened once".to_string(),
        n => format!("Opened {n} times"),
    };
    let cap = row.max_access_count.map(|cap| format!("{} of {cap} opens", row.access_count)).unwrap_or_default();
    events.push((opened, cap));
    events.push(match row.state {
        SendState::Revoked => ("Switched off".to_string(), "The link no longer answers".to_string()),
        SendState::Used => ("Every permitted view is spent".to_string(), "The link no longer answers".to_string()),
        SendState::Expired => ("The link ran out of time".to_string(), row.expires.clone()),
        _ if row.expires != super::NO_EXPIRY_OF_ITS_OWN && !row.expires.is_empty() => {
            ("The link stops working".to_string(), row.expires.clone())
        }
        _ => ("Removed automatically".to_string(), row.deletes.clone()),
    });
    events
}

/// Spec B 7's Activity: a heading, then each event on a rail.
fn activity(ui: &mut egui::Ui, row: &SendRow) {
    let p = tokens::palette(ui.ctx());
    ui.label(tokens::job(Role { size: 15.0, face: theme::EXTRABOLD, tracking_em: 0.0 }, "Activity", p.ink));
    ui.add_space(6.0);
    ui.add(egui::Label::new(egui::RichText::new(row.activity.as_str()).size(14.0).color(p.text_nav)).wrap());
    ui.add_space(12.0);
    let events = activity_events(row);
    let width = ui.available_width();
    let last = events.len() - 1;
    for (i, (title, detail)) in events.iter().enumerate() {
        let t = ui.painter().layout(title.clone(), semibold(15.0), p.ink, width - 26.0);
        let d = (!detail.is_empty()).then(|| ui.painter().layout(detail.clone(), regular(14.0), p.text_label, width - 26.0));
        let text_h = t.size().y + d.as_ref().map_or(0.0, |d| d.size().y + 1.0);
        let height = (text_h + if i < last { 14.0 } else { 0.0 }).max(22.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let dot_c = pos2(rect.left() + 6.0, rect.top() + 5.0 + 5.5);
        if i < last {
            ui.painter().rect_filled(
                Rect::from_min_max(pos2(rect.left() + 5.0, dot_c.y + 5.5), pos2(rect.left() + 7.0, rect.bottom() + 1.0)),
                CornerRadius::ZERO,
                p.card_ring,
            );
        }
        ui.painter().circle_filled(dot_c, 5.5, if i == last { p.blue } else { p.switch_off });
        ui.painter().galley(pos2(rect.left() + 26.0, rect.top()), t.clone(), p.ink);
        if let Some(d) = d {
            ui.painter().galley(pos2(rect.left() + 26.0, rect.top() + t.size().y + 1.0), d, p.text_label);
        }
    }
}

/// How tall a dialog's scrolling body may be: the window, less the room a
/// dialog keeps from its edges, its padding and what stays fixed around the
/// body (`reserved`). A dialog taller than the window scrolls its body and
/// keeps its header and buttons where they can be pressed.
fn body_budget(ctx: &egui::Context, reserved: f32) -> f32 {
    (ctx.content_rect().height() - 2.0 * modal::GUTTER - 2.0 * f32::from(modal::PADDING) - reserved).max(120.0)
}

/// A dialog's scrolling body. **While a question is up the body keeps the
/// height it had at rest**, and the question joins it at the top, so the
/// dialog does not grow and its buttons stay where they were -- which is what
/// lets Cancel stand exactly where Revoke was. (A dialog that was never seen
/// at rest, opened already asking, has no height to keep and sizes itself.)
fn pinned_body<R>(ui: &mut egui::Ui, scope: &str, confirming: bool, budget: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    let height_key = egui::Id::new((ID, scope, "rest-height"));
    let asked_key = egui::Id::new((ID, scope, "was-asking"));
    let pinned = if confirming { ctx.data(|d| d.get_temp::<f32>(height_key)) } else { None };
    let was_asking = ctx.data(|d| d.get_temp::<bool>(asked_key)).unwrap_or(false);
    ctx.data_mut(|d| d.insert_temp(asked_key, confirming));
    let mut area = egui::ScrollArea::vertical().id_salt((scope, "body")).max_height(budget).auto_shrink([false, true]);
    if let Some(h) = pinned {
        area = area.max_height(h).min_scrolled_height(h);
    }
    if confirming && !was_asking {
        // The question is at the top: show it, whatever was scrolled.
        area = area.vertical_scroll_offset(0.0);
    }
    let out = area.show(ui, add);
    if !confirming {
        ctx.data_mut(|d| d.insert_temp(height_key, out.content_size.y.min(budget)));
    }
    out.inner
}

const BUTTON: ButtonSize = ButtonSize { height: 48.0, pad_x: 16.0, radius: 14, font_px: 16.0 };

/// One slot of [`button_row`]: what is drawn in it (nothing, while a
/// question holds its neighbours back) and what it is measured as.
struct Slot<'a> {
    draw: Option<(ButtonKind, &'a str)>,
    measure: (ButtonKind, &'a str),
}

/// A row of buttons sharing the width: each at least its own natural width,
/// the spare room split evenly (spec B 7's `flex 1`). A slot measures what it
/// would show at rest, drawn or not, so the button after a held-back one is
/// where it always is. `Some(i)` when slot `i` was pressed.
fn button_row(ui: &mut egui::Ui, slots: &[Slot<'_>]) -> Option<usize> {
    let gap = 10.0;
    let width = ui.available_width();
    let natural: Vec<f32> =
        slots.iter().map(|s| controls::button_width(ui.ctx(), s.measure.0, BUTTON, s.measure.1, false)).collect();
    let spare = (width - natural.iter().sum::<f32>() - gap * (slots.len() as f32 - 1.0)).max(0.0) / slots.len() as f32;
    let (row, _) = ui.allocate_exact_size(vec2(width, BUTTON.height), Sense::hover());
    let mut x = row.left();
    let mut pressed = None;
    for (i, slot) in slots.iter().enumerate() {
        let w = natural[i] + spare;
        if let Some((kind, label)) = slot.draw {
            let rect = Rect::from_min_size(pos2(x, row.top()), vec2(w, BUTTON.height));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            if controls::button_filling(&mut child, kind, BUTTON, label).clicked() {
                pressed = Some(i);
            }
        }
        x += w + gap;
    }
    pressed
}

/// The caution band for an opened Send that carried the password (spec B 7).
fn seen_band(ui: &mut egui::Ui, row: &SendRow) -> Option<String> {
    let p = tokens::palette(ui.ctx());
    let mut pressed = false;
    egui::Frame::new()
        .fill(p.caution_wash)
        .stroke(Stroke::new(1.0, p.caution_edge))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let (slot, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                crate::kit::icon::stroke(ui.painter(), slot, modal::WARNING_GLYPH, 2.0, p.caution_mark);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(theme::bold(super::SEEN_TITLE, 15.0).color(p.caution_ink));
                    ui.label(egui::RichText::new(super::SEEN_BODY).size(14.0).color(p.caution_ink));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let size = ButtonSize { height: 40.0, pad_x: 14.0, radius: 11, font_px: 14.0 };
                    let (rect, response) = ui.allocate_exact_size(
                        vec2(controls::button_width(ui.ctx(), ButtonKind::Outline, size, super::CHANGE_PASSWORD, false), 40.0),
                        Sense::click(),
                    );
                    ui.painter().rect(rect, CornerRadius::same(11), p.card, Stroke::new(1.0, p.caution_edge), StrokeKind::Inside);
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, super::CHANGE_PASSWORD, bold(14.0), p.caution_ink);
                    pressed = response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                });
            });
        });
    pressed.then(|| row.sent.as_ref().map(|s| s.item_id.clone())).flatten()
}

// ---- the Send detail modal -------------------------------------------------------

/// Spec B 7: the Send detail. `ModalOutcome::Closed` is the scrim, the mark
/// or Esc.
fn send_modal(
    ctx: &egui::Context,
    row: &SendRow,
    delete: SendDeleteView<'_>,
    records: RecordTiles<'_>,
) -> (ModalOutcome, Option<SendUiAction>) {
    let has_id = !row.id.is_empty();
    let revoking = has_id && delete.in_flight == Some(row.id.as_str());
    let confirming = has_id && !revoking && delete.confirming == Some(row.id.as_str());
    let tone = super::state_tone(row.state);
    Dialog::new(ID, 600.0).show(ctx, |ui| {
        let p = tokens::palette(ui.ctx());
        let mut action: Option<SendUiAction> = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        let kind = if row.sent.is_some() {
            super::RECORD_SEND_KIND
        } else if row.is_file {
            super::FILE_SEND_KIND
        } else {
            super::TEXT_SEND_KIND
        };
        modal_header(
            ui,
            records,
            row.sent.as_ref().map(|s| s.item_id.as_str()),
            &row.name,
            &recipient_line_with(row, kind),
            Some((super::state_label(row.state).to_string(), tone.fill, tone.ink)),
        );
        ui.add_space(22.0);
        let seen = super::password_was_seen(row);
        let budget = body_budget(ui.ctx(), 56.0 + 22.0 + 22.0 + BUTTON.height + if seen { 22.0 + 76.0 } else { 0.0 });
        pinned_body(ui, &row.id, confirming, budget, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if confirming {
                modal::note(ui, Tone::Danger, super::CONFIRM_PROMPT);
                ui.add_space(12.0);
                if controls::button_filling(ui, ButtonKind::Danger, BUTTON, super::CONFIRM_LABEL).clicked() && has_id {
                    action = Some(SendUiAction::ConfirmDelete { id: row.id.clone(), name: row.name.clone() });
                }
                ui.add_space(22.0);
            }
            fact_tiles(ui, &[("Opens", views_words(row)), ("Link", ends_words(row)), ("Includes", includes_words(row))]);
            ui.add_space(22.0);
            let opens_with = if row.has_password { super::PASSWORD_SEGMENT } else { super::LINK_ONLY };
            let address = if row.access_url.is_empty() {
                Value::Plain(super::ADDRESS_MISSING)
            } else {
                Value::Address { url: row.access_url.as_str(), dead: !row.state.is_live() }
            };
            let mut rows: Vec<(&str, Value<'_>)> = vec![(super::ADDRESS_ROW, address)];
            if let Some(who) = row.sent.as_ref().map(|s| s.recipient.as_str()).filter(|r| !r.is_empty()) {
                rows.push((super::RECIPIENT_ROW, Value::Plain(who)));
            }
            rows.push((super::OPENS_WITH_ROW, Value::Plain(opens_with)));
            rows.push((super::VIEWS_ROW, Value::Views { used: row.access_count, cap: row.max_access_count }));
            rows.push((super::EXPIRES_ROW, Value::Plain(row.expires.as_str())));
            rows.push((super::DELETED_ROW, Value::Plain(row.deletes.as_str())));
            detail_rows(ui, &rows);
            ui.add_space(22.0);
            activity(ui, row);
        });
        // Spec B 7's caution sits with the buttons, not in the scrolled
        // body: it asks for something to be done, and it is the one thing a
        // tall dialog must not hide below its fold.
        if seen {
            ui.add_space(22.0);
            if let Some(item_id) = seen_band(ui, row) {
                action = Some(SendUiAction::ChangePassword(item_id));
            }
        }
        ui.add_space(22.0);
        if revoking {
            // No widget while the revoke runs: a button here would be one
            // more thing to press during it.
            ui.label(tokens::job(
                Role { size: 15.0, face: theme::SEMIBOLD, tracking_em: 0.0 },
                delete.busy_label(),
                p.text_faint,
            ));
        } else {
            // Copy link, Extend, Open record, Revoke -- in the same slots
            // whether or not a question is up, so a second quick click where
            // Revoke was lands on Cancel and destroys nothing.
            #[derive(Clone, Copy)]
            enum Which {
                Copy,
                Extend,
                Open,
                Revoke,
            }
            let live = row.state.is_live();
            let held = |kind, label| Slot { draw: (!confirming).then_some((kind, label)), measure: (kind, label) };
            let mut plan: Vec<(Which, Slot<'_>)> = Vec::new();
            if live {
                plan.push((Which::Copy, held(ButtonKind::Primary, super::COPY_LINK_LABEL)));
            }
            if row.extendable {
                plan.push((Which::Extend, held(ButtonKind::Outline, super::EXTEND_LABEL)));
            }
            if row.sent.is_some() {
                plan.push((Which::Open, held(ButtonKind::Outline, super::OPEN_RECORD_LABEL)));
            }
            plan.push((
                Which::Revoke,
                Slot {
                    draw: Some(if confirming {
                        (ButtonKind::Outline, super::CANCEL_LABEL)
                    } else {
                        (ButtonKind::Danger, super::REVOKE_LABEL)
                    }),
                    measure: (ButtonKind::Danger, super::REVOKE_LABEL),
                },
            ));
            let slots: Vec<Slot<'_>> = plan.iter().map(|(_, s)| Slot { draw: s.draw, measure: s.measure }).collect();
            if let Some(i) = button_row(ui, &slots) {
                action = match plan[i].0 {
                    Which::Copy if !row.access_url.is_empty() => Some(SendUiAction::CopyLink(row.access_url.clone())),
                    Which::Extend if has_id => Some(SendUiAction::Extend { id: row.id.clone(), name: row.name.clone() }),
                    Which::Open => row.sent.as_ref().map(|s| SendUiAction::OpenRecord(s.item_id.clone())),
                    Which::Revoke if confirming => Some(SendUiAction::CancelDelete),
                    Which::Revoke if has_id => Some(SendUiAction::AskDelete(row.id.clone())),
                    _ => action,
                };
            }
        }
        action
    })
}

fn recipient_line_with(row: &SendRow, kind: &str) -> String {
    format!("{} \u{b7} {kind}", recipient_line(row))
}

// ---- a person share ----------------------------------------------------------------

/// A person share's modal: who has the record and the two-step Stop sharing.
/// `Some(false)` when it closed, `None` while it is up.
fn share_modal(
    ctx: &egui::Context,
    share: &ShareRow,
    people: &mut PeopleRows<'_>,
    records: RecordTiles<'_>,
) -> (ModalOutcome, Option<SendUiAction>) {
    let key = share.key.as_str();
    let stopping = people.stopping.as_deref() == Some(key);
    let confirming = !stopping && people.confirming.as_deref() == Some(key);
    let kind = share.pill();
    let tone = kind.tone();
    let mut open_record = false;
    let (outcome, ()) = Dialog::new(ID, 600.0).show(ctx, |ui| {
        let p = tokens::palette(ui.ctx());
        ui.spacing_mut().item_spacing.y = 0.0;
        modal_header(
            ui,
            records,
            Some(&share.item_id),
            &share.name,
            sharing::SHARED_WITH_A_PERSON,
            Some((kind.label().to_string(), tone.fill, tone.ink)),
        );
        ui.add_space(22.0);
        let budget = body_budget(ui.ctx(), 56.0 + 22.0 + 22.0 + BUTTON.height);
        pinned_body(ui, &share.key, confirming, budget, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if confirming {
                modal::note(ui, Tone::Danger, &sharing::stop_question(&share.name, &share.email));
                ui.add_space(6.0);
                ui.add(egui::Label::new(egui::RichText::new(sharing::STOP_HINT).size(13.0).color(p.text_faint)).wrap());
                ui.add_space(12.0);
                if controls::button_filling(ui, ButtonKind::Danger, BUTTON, sharing::REMOVE_LABEL).clicked() {
                    people.asked = Some(PeopleAsk::ConfirmStop(key.to_string()));
                }
                ui.add_space(22.0);
            }
            let since = share.since.as_deref().unwrap_or(sharing::SINCE_UNKNOWN);
            fact_tiles(
                ui,
                &[
                    (sharing::SHARED_WITH_ROW, share.email.clone()),
                    (sharing::SINCE_ROW, since.to_string()),
                    (sharing::ACCESS_ROW, sharing::VIEW_ONLY.to_string()),
                ],
            );
            ui.add_space(22.0);
            let copy = if share.state == crate::rest::sharing::PersonState::Shared {
                (sharing::THEIR_COPY_ROW, Value::Plain(sharing::FOLLOWS_EDITS))
            } else {
                (sharing::STATUS_ROW, Value::Plain(share.state_words()))
            };
            detail_rows(
                ui,
                &[
                    (sharing::CAN_ROW, Value::Plain(sharing::CAN_VALUE)),
                    (sharing::CANNOT_ROW, Value::Plain(sharing::CANNOT_VALUE)),
                    copy,
                ],
            );
            ui.add_space(10.0);
            ui.add(egui::Label::new(egui::RichText::new(sharing::WHOLE_RECORD).size(14.0).color(p.text_faint)).wrap());
            if let Some(notice) = people.notice.as_deref() {
                ui.add_space(12.0);
                modal::note(ui, Tone::Danger, notice);
            }
        });
        ui.add_space(22.0);
        if stopping {
            ui.label(tokens::job(
                Role { size: 15.0, face: theme::SEMIBOLD, tracking_em: 0.0 },
                sharing::STOPPING_LABEL,
                p.text_faint,
            ));
        } else {
            let slots = [
                Slot {
                    draw: (!confirming).then_some((ButtonKind::Outline, super::OPEN_RECORD_LABEL)),
                    measure: (ButtonKind::Outline, super::OPEN_RECORD_LABEL),
                },
                Slot {
                    draw: Some(if confirming {
                        (ButtonKind::Outline, sharing::CANCEL_BUTTON)
                    } else {
                        (ButtonKind::Danger, sharing::STOP_LABEL)
                    }),
                    measure: (ButtonKind::Danger, sharing::STOP_LABEL),
                },
            ];
            if let Some(i) = button_row(ui, &slots) {
                match (i, confirming) {
                    (0, _) => open_record = true,
                    (_, true) => people.asked = Some(PeopleAsk::CancelStop),
                    (_, false) => people.asked = Some(PeopleAsk::Stop(key.to_string())),
                }
            }
        }
    });
    let opened = open_record.then(|| SendUiAction::OpenRecord(share.item_id.clone()));
    (outcome, opened)
}

// ---- a received import ----------------------------------------------------------------

fn received_modal(ctx: &egui::Context, row: &ReceivedRow) -> ModalOutcome {
    let (outcome, ()) = Dialog::new(ID, 600.0).show(ctx, |ui| {
        let p = tokens::palette(ui.ctx());
        ui.spacing_mut().item_spacing.y = 0.0;
        modal_header(ui, RecordTiles::default(), None, &row.name, super::RECEIVED_KIND, None);
        ui.add_space(22.0);
        let (sentence, ink) = if row.still_in_vault {
            (super::RECEIVED_ITEM_PRESENT, p.ink)
        } else {
            (super::RECEIVED_ITEM_GONE, p.text_faint)
        };
        ui.add(egui::Label::new(tokens::job(Role { size: 16.0, face: theme::SEMIBOLD, tracking_em: 0.0 }, sentence, ink)).wrap());
        ui.add_space(22.0);
        let short = if row.still_in_vault { super::IN_VAULT_YES } else { super::IN_VAULT_NO };
        detail_rows(ui, &[(super::ARRIVED_ROW, Value::Plain(&row.when)), (super::IN_VAULT_ROW, Value::Plain(short))]);
    });
    outcome
}

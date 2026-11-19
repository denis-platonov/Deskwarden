//! The two full-window sign-in states (spec part C section 5.2 and 5.3): the
//! signed-out split screen, and the navy lock screen.
//!
//! Both are painted across the whole window, caption buttons included, because
//! the design has no title bar here. The screens that use them
//! (`login_ui`'s frame, `second_factor_ui`, `api_key_ui`) pass in the Ui's
//! rectangle and a closure that lays out the form or the stage; everything
//! about where it goes is decided here.

use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, Rect, Sense, Shadow, Stroke, Ui, UiBuilder, Vec2};

use crate::i18n;
use crate::kit::icon::{self, glyph};
use crate::kit::motion;
use crate::kit::tokens::{self, palette, Role, PAGE_TITLE};
use crate::login_ui::{ChromeAction, CloseControl};
use crate::theme::{self, BOLD, EXTRABOLD, SEMIBOLD};

/// The brand half's headline (spec C 5.2, verbatim).
pub fn headline() -> &'static str {
    i18n::t("auth.headline")
}
/// Under the headline.
pub fn brand_sub() -> &'static str {
    i18n::t("auth.brand_sub")
}
/// Pinned to the brand half's foot.
pub fn brand_foot() -> &'static str {
    i18n::t("auth.brand_foot")
}
/// The wordmark beside the mark.
pub const WORDMARK: &str = "Deskwarden";

/// The sign-in form's width.
pub const FORM_WIDTH: f32 = 420.0;
/// The lock screen's password row.
pub const LOCK_ROW_WIDTH: f32 = 440.0;
/// A window narrower than this drops the brand half and gives the form the
/// whole window (the design is drawn at 1280; the form needs 420 and a margin).
pub const COMPACT_BELOW: f32 = 760.0;
/// One caption button's cell.
pub const CAPTION_CELL: Vec2 = vec2(46.0, 34.0);

/// Gap between the stacked parts of a form (spec: column gap 18).
pub const FORM_GAP: f32 = 18.0;
/// Between a label and its field.
pub const LABEL_GAP: f32 = 8.0;
/// Between the stacked parts of the lock screen.
pub const LOCK_GAP: f32 = 26.0;

/// What the lock screen's Hello button says before and during a check.
pub fn hello_label() -> &'static str {
    i18n::t("auth.hello_label")
}
pub fn hello_checking() -> &'static str {
    i18n::t("auth.hello_checking")
}
/// The link from the Hello stage to the password stage, and back.
pub fn use_password_label() -> &'static str {
    i18n::t("auth.use_password")
}
pub fn lock_heading() -> &'static str {
    i18n::t("auth.lock_heading")
}
pub fn lock_sub() -> &'static str {
    i18n::t("auth.lock_sub")
}

// ---------------------------------------------------------------- caption

/// The window's three caption buttons in the top right corner, over whatever
/// the screen painted, and a drag band along the top. `ink` is their colour (the
/// design's grey on the white half, light blue on navy). The maximise cell is
/// the design's ghost: this window cannot be maximised.
pub fn caption_buttons(ui: &mut Ui, full: Rect, ink: Color32, close: CloseControl) -> ChromeAction {
    let mut action = ChromeAction::None;
    let cell = |i: f32| Rect::from_min_size(pos2(full.right() - CAPTION_CELL.x * (3.0 - i), full.top()), CAPTION_CELL);
    let band = Rect::from_min_max(full.min, pos2(cell(0.0).left(), full.top() + CAPTION_CELL.y));
    let drag = ui.interact(band, ui.id().with("auth-drag"), Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    let wash = ink.gamma_multiply(0.14);
    let stroke = Stroke::new(1.2, ink);

    let min = cell(0.0);
    let min_response = ui.interact(min, ui.id().with("auth-min"), Sense::click());
    if min_response.hovered() {
        ui.painter().rect_filled(min, CornerRadius::ZERO, wash);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let c = min.center();
    ui.painter().line_segment([pos2(c.x - 5.0, c.y), pos2(c.x + 5.0, c.y)], stroke);
    if min_response.clicked() {
        action = ChromeAction::Minimize;
    }

    ui.painter().rect_stroke(
        Rect::from_center_size(cell(1.0).center(), Vec2::splat(10.0)),
        CornerRadius::ZERO,
        Stroke::new(1.0, ink.gamma_multiply(0.4)),
        egui::StrokeKind::Inside,
    );

    let x = cell(2.0);
    let close_stroke = match close {
        CloseControl::Active => {
            let response = ui.interact(x, ui.id().with("auth-close"), Sense::click());
            if response.hovered() {
                ui.painter().rect_filled(x, CornerRadius::ZERO, wash);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                action = ChromeAction::Close;
            }
            stroke
        }
        CloseControl::Disabled => Stroke::new(1.2, ink.gamma_multiply(0.4)),
    };
    let c = x.center();
    ui.painter().line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], close_stroke);
    ui.painter().line_segment([c + vec2(5.0, -5.0), c + vec2(-5.0, 5.0)], close_stroke);
    action
}

/// What a caption action does to the window, for hosts that draw no title
/// bar of their own.
pub fn apply_caption(ctx: &egui::Context, action: ChromeAction) {
    match action {
        ChromeAction::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        ChromeAction::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
        ChromeAction::None => {}
    }
}

// --------------------------------------------------------------- motion

/// `dw-up` for a screen part: eased progress, restarted when the part has
/// been off screen for a moment, so a lock screen shown a second time rises
/// again.
fn rise(ctx: &egui::Context, key: &str, delay: f32, duration: f32) -> motion::Pose {
    let id = egui::Id::new(("kit-auth-rise", key));
    let seen = id.with("seen");
    let now = ctx.input(|i| i.time);
    let last = ctx.data(|d| d.get_temp::<f64>(seen));
    if last.is_none_or(|last| now - last > 0.35) {
        motion::restart(ctx, id);
    }
    ctx.data_mut(|d| d.insert_temp(seen, now));
    motion::up(motion::progress(ctx, id, delay, duration, motion::EASE_OUT))
}

/// Runs `add` in a column at `rect`'s top left, `width` wide, risen into place
/// by `pose`, and reports how tall it came out.
fn column(ui: &mut Ui, rect: Rect, pose: motion::Pose, add: impl FnOnce(&mut Ui)) -> f32 {
    let mut child = ui.new_child(
        UiBuilder::new().max_rect(rect.translate(vec2(0.0, pose.dy))).layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.multiply_opacity(pose.opacity);
    child.set_width(rect.width());
    add(&mut child);
    child.min_rect().height()
}

/// The height a column took last frame, so this frame can centre it. A change
/// asks for one more frame.
fn remembered_height(ctx: &egui::Context, id: egui::Id, now: f32) -> f32 {
    let before = ctx.data(|d| d.get_temp::<f32>(id)).unwrap_or(now);
    if (before - now).abs() > 0.5 {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(id, now));
    before
}

// -------------------------------------------------------------- text help

fn galley(ctx: &egui::Context, role: Role, text: &str, color: Color32, width: Option<f32>, line: Option<f32>) -> std::sync::Arc<egui::Galley> {
    let mut job = tokens::job(role, text, color);
    if let Some(width) = width {
        job.wrap.max_width = width;
    }
    if let Some(line) = line {
        for section in &mut job.sections {
            section.format.line_height = Some(line);
        }
    }
    ctx.fonts_mut(|f| f.layout_job(job))
}

/// The label over a field (14, bold).
pub fn label(ui: &mut Ui, text: &str) {
    label_dim(ui, text, false);
}

/// [`label`], greyed with its field while an attempt is in flight.
pub fn label_dim(ui: &mut Ui, text: &str, dim: bool) {
    let p = palette(ui.ctx());
    let colour = if dim { p.text_ghost } else { p.text_nav };
    let role = Role { size: 14.0, face: BOLD, tracking_em: 0.0 };
    let g = galley(ui.ctx(), role, text, colour, None, None);
    let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, colour);
}

/// A form's title (32, extra bold).
pub fn title(ui: &mut Ui, text: &str) {
    let p = palette(ui.ctx());
    let g = galley(ui.ctx(), PAGE_TITLE, text, p.ink, Some(ui.available_width()), None);
    let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, p.ink);
}

/// A line of explanation under a title (15, grey), wrapped to the column.
pub fn explain(ui: &mut Ui, text: &str) {
    let p = palette(ui.ctx());
    let role = Role { size: 15.0, face: theme::REGULAR, tracking_em: 0.0 };
    let g = galley(ui.ctx(), role, text, p.text_muted, Some(ui.available_width()), None);
    let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, p.text_muted);
}

/// An error under a field on the white half (spec: errors use `ERROR`).
pub fn error_line(ui: &mut Ui, text: &str) {
    let p = palette(ui.ctx());
    let role = Role { size: 14.0, face: SEMIBOLD, tracking_em: 0.0 };
    let g = galley(ui.ctx(), role, text, p.error, Some(ui.available_width()), None);
    let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, p.error);
}

/// An error on the navy lock screen: red on navy does not read, so it sits on
/// a white chip, still in `ERROR`.
pub fn error_chip(ui: &mut Ui, text: &str, width: f32) {
    let p = palette(ui.ctx());
    let role = Role { size: 14.0, face: SEMIBOLD, tracking_em: 0.0 };
    let g = galley(ui.ctx(), role, text, p.error, Some(width - 28.0), None);
    let size = g.size() + vec2(28.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(vec2(size.x.max(0.0), size.y), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(12), p.card);
    ui.painter().galley(rect.min + vec2(14.0, 10.0), g, p.error);
}

/// A full-width button (the sign-in screen's Log in is the form's width).
// Draw entry point: its arguments are the state it paints; a bundle would only move them.
#[allow(clippy::too_many_arguments)]
pub fn wide_button(
    ui: &mut Ui,
    kind: crate::kit::controls::ButtonKind,
    height: f32,
    radius: u8,
    font_px: f32,
    label: &str,
    icon: Option<&str>,
    width: f32,
) -> egui::Response {
    use crate::kit::controls;
    let size = controls::ButtonSize { height, pad_x: 0.0, radius, font_px };
    let natural = controls::button_width(ui.ctx(), kind, size, label, icon.is_some());
    let size = controls::ButtonSize { pad_x: ((width - natural) / 2.0).max(0.0), ..size };
    controls::button(ui, kind, size, label, icon)
}

// -------------------------------------------------------------- the split

/// The signed-out screen (spec C 5.2): the navy brand half on the left, the
/// form on the right. `add_form` lays out the form in a 420-wide column
/// centred on the right half. Below [`COMPACT_BELOW`] the brand half is
/// dropped. Returns what the caption buttons were asked.
pub fn split(ui: &mut Ui, full: Rect, close: CloseControl, add_form: impl FnOnce(&mut Ui)) -> ChromeAction {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    ui.painter().rect_filled(full, CornerRadius::ZERO, p.card);
    let compact = full.width() < COMPACT_BELOW;
    let form_half = if compact {
        full
    } else {
        let mid = full.center().x;
        brand_panel(ui, Rect::from_min_max(full.min, pos2(mid, full.bottom())));
        Rect::from_min_max(pos2(mid, full.top()), full.max)
    };
    let action = caption_buttons(ui, full, p.text_icon, close);

    let width = FORM_WIDTH.min((form_half.width() - 48.0).max(0.0));
    let id = egui::Id::new("kit-auth-form-height");
    let before = remembered_height(&ctx, id, 0.0);
    let top = form_half.top() + ((form_half.height() - before) / 2.0).max(CAPTION_CELL.y);
    let rect = Rect::from_min_size(pos2(form_half.center().x - width / 2.0, top), vec2(width, form_half.bottom() - top));
    let pose = rise(&ctx, "split-form", 0.08, 0.45);
    let height = column(ui, rect, pose, add_form);
    remembered_height(&ctx, id, height);
    action
}

/// The brand half: the mark and wordmark at the top, the headline and its
/// line in the middle, the reassurance at the foot.
fn brand_panel(ui: &mut Ui, rect: Rect) {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    ui.painter().rect_filled(rect, CornerRadius::ZERO, p.blue_deep);
    let inner = rect.shrink2(vec2(56.0, 64.0));

    // Top: the mark (34 by 39) and the wordmark, 12 apart.
    let mark = Rect::from_min_size(inner.min, vec2(34.0, 39.0));
    theme::paint_mark_colours(ui.painter(), mark, theme::MARK_REVERSED);
    let word_role = Role { size: 22.0, face: EXTRABOLD, tracking_em: -0.02 };
    let word = galley(&ctx, word_role, WORDMARK, Color32::WHITE, None, None);
    ui.painter().galley(pos2(mark.right() + 12.0, mark.center().y - word.size().y / 2.0), word, Color32::WHITE);

    // Foot.
    let foot_role = Role { size: 14.0, face: theme::REGULAR, tracking_em: 0.0 };
    let foot = galley(&ctx, foot_role, brand_foot(), p.blue_soft, None, None);
    let foot_at = pos2(inner.left(), inner.bottom() - foot.size().y);
    ui.painter().galley(foot_at, foot, p.blue_soft);

    // Middle: centred between the two, headline 48 over a 30ch line of 18.
    let head_role = Role { size: 48.0, face: EXTRABOLD, tracking_em: -0.035 };
    let sub_role = Role { size: 18.0, face: theme::REGULAR, tracking_em: 0.0 };
    let head = galley(&ctx, head_role, headline(), Color32::WHITE, Some(inner.width()), Some(48.0 * 1.04));
    let sub = galley(&ctx, sub_role, brand_sub(), p.blue_edge, Some(30.0 * 18.0 * 0.5), Some(18.0 * 1.5));
    let block = head.size().y + 18.0 + sub.size().y;
    let top = (mark.bottom() + foot_at.y) / 2.0 - block / 2.0;
    let pose = rise(&ctx, "split-brand", 0.0, 0.5);
    let painter = ui.painter().clone();
    let tint = |c: Color32| c.gamma_multiply(pose.opacity);
    let head_at = pos2(inner.left(), top + pose.dy);
    let sub_at = pos2(inner.left(), top + head.size().y + 18.0 + pose.dy);
    painter.galley_with_override_text_color(head_at, head, tint(Color32::WHITE));
    painter.galley_with_override_text_color(sub_at, sub, tint(p.blue_edge));
}

// --------------------------------------------------------------- the lock

/// The lock screen (spec C 5.3): navy, with a column centred in it. `add`
/// lays out the column: [`lock_header`] and then the stage.
pub fn lock_surface(ui: &mut Ui, full: Rect, close: CloseControl, add: impl FnOnce(&mut Ui)) -> ChromeAction {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    ui.painter().rect_filled(full, CornerRadius::ZERO, p.blue_deep);
    let action = caption_buttons(ui, full, p.blue_edge, close);

    let width = LOCK_ROW_WIDTH.min((full.width() - 48.0).max(0.0));
    let id = egui::Id::new("kit-auth-lock-height");
    let before = remembered_height(&ctx, id, 0.0);
    let top = full.top() + ((full.height() - before) / 2.0).max(CAPTION_CELL.y);
    let rect = Rect::from_min_size(pos2(full.center().x - width / 2.0, top), vec2(width, full.bottom() - top));
    let pose = rise(&ctx, "lock", 0.0, 0.3);
    let height = column(ui, rect, pose, |ui| {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = LOCK_GAP;
            add(ui);
        });
    });
    remembered_height(&ctx, id, height);
    action
}

/// The lock screen's head: the white shield (64 by 75), the heading and the
/// line under it, 8 apart.
pub fn lock_header(ui: &mut Ui, heading: &str, sub: &str) {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    let (mark, _) = ui.allocate_exact_size(vec2(64.0, 75.0), Sense::hover());
    theme::paint_mark_colours(ui.painter(), mark, theme::MARK_REVERSED);
    let head_role = Role { size: 32.0, face: EXTRABOLD, tracking_em: -0.02 };
    let sub_role = Role { size: 17.0, face: theme::REGULAR, tracking_em: 0.0 };
    let head = galley(&ctx, head_role, heading, Color32::WHITE, Some(ui.available_width()), None);
    let sub = galley(&ctx, sub_role, sub, p.blue_edge, Some(ui.available_width()), None);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), head.size().y + 8.0 + sub.size().y), Sense::hover());
    let head_at = pos2(rect.center().x - head.size().x / 2.0, rect.top());
    let sub_at = pos2(rect.center().x - sub.size().x / 2.0, rect.top() + head.size().y + 8.0);
    ui.painter().galley(head_at, head, Color32::WHITE);
    ui.painter().galley(sub_at, sub, p.blue_edge);
}

/// The Windows Hello button (62 high, white, navy type, a fingerprint). While
/// `checking` it reads "Checking\u{2026}" and a white ring spreads from it.
pub fn hello_button(ui: &mut Ui, label: &str, checking: bool) -> egui::Response {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    let role = Role { size: 17.0, face: BOLD, tracking_em: 0.0 };
    let text = galley(&ctx, role, label, p.blue_deep, None, None);
    let size = vec2(30.0 * 2.0 + 24.0 + 12.0 + text.size().x, 62.0);
    let (slot, response) = ui.allocate_exact_size(size, Sense::click());
    let lift = if response.hovered() && !checking { -2.0 } else { 0.0 };
    let rect = slot.translate(vec2(0.0, lift));
    let radius = CornerRadius::same(18);
    if checking {
        let (spread, alpha) = motion::pulse(ctx.input(|i| i.time), 22.0);
        ui.painter().rect_stroke(
            rect.expand(spread),
            CornerRadius::same(18 + spread as u8),
            Stroke::new(2.0, Color32::WHITE.gamma_multiply(alpha * 0.6)),
            egui::StrokeKind::Outside,
        );
        ctx.request_repaint();
    }
    ui.painter().add(Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(46) }.as_shape(rect, radius));
    ui.painter().rect_filled(rect, radius, Color32::WHITE);
    let icon_rect = Rect::from_min_size(pos2(rect.left() + 30.0, rect.center().y - 12.0), Vec2::splat(24.0));
    icon::stroke(ui.painter(), icon_rect, glyph::FINGERPRINT, 2.0, p.blue_deep);
    ui.painter().galley(pos2(icon_rect.right() + 12.0, rect.center().y - text.size().y / 2.0), text, p.blue_deep);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The text link under a lock stage (15, light blue, white on hover).
pub fn lock_link(ui: &mut Ui, text: &str) -> egui::Response {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    let role = Role { size: 15.0, face: SEMIBOLD, tracking_em: 0.0 };
    let g = galley(&ctx, role, text, p.blue_edge, None, None);
    let (rect, response) = ui.allocate_exact_size(g.size() + vec2(8.0, 6.0), Sense::click());
    let colour = if response.hovered() { Color32::WHITE } else { p.blue_edge };
    ui.painter().galley_with_override_text_color(rect.center() - g.size() / 2.0, g, colour);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// How wide [`lock_button`] draws `label`.
pub fn lock_button_width(ctx: &egui::Context, label: &str) -> f32 {
    let role = Role { size: 16.0, face: BOLD, tracking_em: 0.0 };
    galley(ctx, role, label, Color32::WHITE, None, None).size().x + 48.0
}

/// A text link on the white half (14, blue, deeper blue on hover).
pub fn link(ui: &mut Ui, text: &str) -> egui::Response {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    let role = Role { size: 14.0, face: SEMIBOLD, tracking_em: 0.0 };
    let g = galley(&ctx, role, text, p.blue, None, None);
    let (rect, response) = ui.allocate_exact_size(g.size() + vec2(0.0, 8.0), Sense::click());
    let colour = if response.hovered() { p.blue_deep } else { p.blue };
    ui.painter().galley_with_override_text_color(pos2(rect.left(), rect.center().y - g.size().y / 2.0), g, colour);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The indeterminate bar of the splash (spec C 5.4), at `width`: while a
/// sign-in or an unlock is with the server. `on_dark` is the lock screen's
/// white-on-navy; otherwise blue on a grey track.
pub fn progress(ui: &mut Ui, width: f32, on_dark: bool) {
    let p = palette(ui.ctx());
    let (track, _) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
    let (back, fore) = if on_dark {
        (Color32::from_rgba_unmultiplied(255, 255, 255, 41), Color32::WHITE)
    } else {
        (p.meter_track, p.blue)
    };
    ui.painter().rect_filled(track, CornerRadius::same(2), back);
    let bar = width * 0.34;
    let t = ui.input(|i| i.time);
    let rect = Rect::from_min_size(pos2(track.left() + motion::line(t) * bar, track.top()), vec2(bar, 4.0)).intersect(track);
    if rect.width() > 0.0 {
        ui.painter().rect_filled(rect, CornerRadius::same(2), fore);
    }
    ui.ctx().request_repaint();
}

/// A wrapped note on the navy lock screen, centred (the first-run notice).
pub fn lock_note(ui: &mut Ui, text: &str, width: f32) {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    let role = Role { size: 14.0, face: theme::REGULAR, tracking_em: 0.0 };
    let mut job = tokens::job(role, text, p.blue_edge);
    job.wrap.max_width = width;
    job.halign = egui::Align::Center;
    let g = ctx.fonts_mut(|f| f.layout_job(job));
    let (rect, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
    ui.painter().galley(pos2(rect.center().x, rect.top()), g, p.blue_edge);
}

/// The lock screen's Unlock button (58 high, bright blue).
pub fn lock_button(ui: &mut Ui, label: &str) -> egui::Response {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    let role = Role { size: 16.0, face: BOLD, tracking_em: 0.0 };
    let g = galley(&ctx, role, label, Color32::WHITE, None, None);
    let (rect, response) = ui.allocate_exact_size(vec2(g.size().x + 48.0, 58.0), Sense::click());
    let fill = if response.hovered() { p.blue_bright_hover } else { p.blue_bright };
    ui.painter().rect_filled(rect, CornerRadius::same(16), fill);
    ui.painter().galley_with_override_text_color(rect.center() - g.size() / 2.0, g, Color32::WHITE);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small line under the lock stage in a quieter blue (the server, Log out).
pub fn lock_quiet(ui: &mut Ui, text: &str) {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    let role = Role { size: 13.0, face: theme::REGULAR, tracking_em: 0.0 };
    let g = galley(&ctx, role, text, p.blue_soft, None, None);
    let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, p.blue_soft);
}

// ------------------------------------------------- the sign-in gallery's parts

/// `circle-alert`: the mark in front of a field's error line.
const ALERT_CIRCLE: &str = "M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zM12 8v4M12 16h.01";
/// A phone with its home dot, and with two lines on its screen (the code).
pub const PHONE_CODE: &str = "M7 2h10a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2zM12 18h.01M9 7h6M9 11h6";
/// `triangle-alert`: the server banner's mark.
const TRIANGLE_ALERT: &str = "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z";
/// `lock`, with the rounded body the stage tiles use.
pub const LOCK_TILE: &str = "M6 11h12a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-6a2 2 0 0 1 2-2zM8 11V7a4 4 0 0 1 8 0v4";
/// The done stage's tick.
const DONE_TICK: &str = "M5 12.5 10 17 19 7";

/// Between a label and its field on the white half (spec gallery: gap 7).
pub const FIELD_LABEL_GAP: f32 = 7.0;
/// A field's border (the design's `1.5px`).
const FIELD_EDGE: f32 = 1.5;
/// How tall the in-button bar is, and how wide.
const BUTTON_BAR: Vec2 = vec2(120.0, 3.0);

/// The sub line under a form's title (16, grey).
pub fn subtitle(ui: &mut Ui, text: &str) {
    let p = palette(ui.ctx());
    let role = Role { size: 16.0, face: theme::REGULAR, tracking_em: 0.0 };
    let g = galley(ui.ctx(), role, text, p.text_faint, Some(ui.available_width()), Some(16.0 * 1.45));
    let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, p.text_faint);
}

/// The title of a stage that follows the form (2FA, too many tries, done):
/// 30, extra bold.
pub fn stage_title(ui: &mut Ui, text: &str) {
    let p = palette(ui.ctx());
    let role = Role { size: 30.0, face: EXTRABOLD, tracking_em: -0.03 };
    let g = galley(ui.ctx(), role, text, p.ink, Some(ui.available_width()), None);
    let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(rect.min, g, p.ink);
}

/// The wash an errored field sits on (the design's `#fff8f7`).
fn error_wash(ctx: &egui::Context) -> Color32 {
    if ctx.global_style().visuals.dark_mode {
        palette(ctx).danger_wash
    } else {
        Color32::from_rgb(0xff, 0xf8, 0xf7)
    }
}

/// A field's box: `1.5` border, red and washed while `error`, blue while it
/// has the keyboard.
fn field_box(ui: &Ui, rect: Rect, radius: u8, focused: bool, error: bool) {
    let p = palette(ui.ctx());
    let (edge, fill) = if error {
        (p.error, error_wash(ui.ctx()))
    } else if focused {
        (p.blue, p.card)
    } else {
        (p.border, p.card)
    };
    ui.painter().rect(rect, CornerRadius::same(radius), fill, Stroke::new(FIELD_EDGE, edge), egui::StrokeKind::Inside);
}

/// A text field on the sign-in screen: 50 high, radius 14, `error` paints it
/// red. `prefix` is the grey word in front of the typing (`https://`).
pub fn field_text(
    ui: &mut Ui,
    id: egui::Id,
    value: &mut String,
    placeholder: &str,
    width: f32,
    error: bool,
    prefix: Option<&str>,
) -> egui::Response {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, 50.0), Sense::hover());
    field_box(ui, rect, 14, ui.memory(|m| m.has_focus(id)), error);
    let mut left = rect.left() + 16.0;
    if let Some(prefix) = prefix {
        let role = Role { size: 16.0, face: theme::REGULAR, tracking_em: 0.0 };
        let g = galley(ui.ctx(), role, prefix, p.text_ghost, None, None);
        let at = pos2(left, rect.center().y - g.size().y / 2.0);
        left += g.size().x + 4.0;
        ui.painter().galley(at, g, p.text_ghost);
    }
    let inner = Rect::from_min_max(pos2(left, rect.top()), pos2(rect.right() - 16.0, rect.bottom()));
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .id(id)
            .frame(egui::Frame::NONE)
            .font(egui::FontId::proportional(16.0))
            .hint_text(egui::RichText::new(placeholder).font(egui::FontId::proportional(16.0)))
            .desired_width(inner.width())
            .vertical_align(egui::Align::Center),
    );
    ui.advance_cursor_after_rect(rect);
    response
}

/// The sign-in screen's master-password field: [`field_text`] hidden unless
/// `revealed`, with the eye (38 square, radius 11) at its right.
pub fn field_secret(
    ui: &mut Ui,
    id: egui::Id,
    value: &mut String,
    revealed: &mut bool,
    placeholder: &str,
    width: f32,
    error: bool,
) -> egui::Response {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, 50.0), Sense::hover());
    field_box(ui, rect, 14, ui.memory(|m| m.has_focus(id)), error);
    let eye = Rect::from_center_size(pos2(rect.right() - 6.0 - 19.0, rect.center().y), Vec2::splat(38.0));
    let eye_response = ui.interact(eye, id.with("eye"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if eye_response.hovered() {
        ui.painter().rect_filled(eye, CornerRadius::same(11), p.hover);
    }
    let d = if *revealed { glyph::EYE_OFF } else { glyph::EYE };
    icon::stroke(ui.painter(), Rect::from_center_size(eye.center(), Vec2::splat(19.0)), d, 2.0, p.text_muted);
    crate::kit::tips::hint(ui, &eye_response, if *revealed { i18n::t("auth.hide_password") } else { i18n::t("auth.show_password") }, &[]);
    if eye_response.clicked() {
        *revealed = !*revealed;
    }
    let inner = Rect::from_min_max(pos2(rect.left() + 16.0, rect.top()), pos2(eye.left() - 6.0, rect.bottom()));
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .id(id)
            .password(!*revealed)
            .frame(egui::Frame::NONE)
            .font(egui::FontId::proportional(16.0))
            .hint_text(egui::RichText::new(placeholder).font(egui::FontId::proportional(16.0)))
            .desired_width(inner.width())
            .vertical_align(egui::Align::Center),
    );
    ui.advance_cursor_after_rect(rect);
    response
}

/// The line under an errored field: the alert mark and the message in `ERROR`,
/// 14 semibold, wrapped.
pub fn field_error(ui: &mut Ui, text: &str) {
    let p = palette(ui.ctx());
    let role = Role { size: 14.0, face: SEMIBOLD, tracking_em: 0.0 };
    let width = ui.available_width();
    let g = galley(ui.ctx(), role, text, p.error, Some(width - 23.0), Some(14.0 * 1.4));
    let (rect, _) = ui.allocate_exact_size(vec2(width, g.size().y.max(18.0)), Sense::hover());
    icon::stroke(ui.painter(), Rect::from_min_size(pos2(rect.left(), rect.top() + 1.0), Vec2::splat(16.0)), ALERT_CIRCLE, 2.2, p.error);
    ui.painter().galley(pos2(rect.left() + 23.0, rect.top()), g, p.error);
}

/// Which of the two banners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerTone {
    /// Amber: "You're offline".
    Caution,
    /// Red: "Can't reach your server".
    Danger,
}

/// The banner's background, its type and its button's ring.
fn banner_colours(ctx: &egui::Context, tone: BannerTone) -> (Color32, Color32, Color32) {
    let p = palette(ctx);
    match (tone, ctx.global_style().visuals.dark_mode) {
        (BannerTone::Caution, _) => (p.caution_wash, p.caution_mark, p.caution_edge),
        (BannerTone::Danger, false) => (Color32::from_rgb(0xfd, 0xec, 0xea), Color32::from_rgb(0x8f, 0x1d, 0x13), Color32::from_rgb(0xf4, 0xc7, 0xc1)),
        (BannerTone::Danger, true) => (p.danger_wash, p.error, p.danger_edge),
    }
}

/// The banner above the form when the server could not be reached: mark,
/// title, a line, and one button (`label`) on the right. True when the
/// button was pressed.
pub fn banner(ui: &mut Ui, tone: BannerTone, title: &str, text: &str, label: &str) -> bool {
    let ctx = ui.ctx().clone();
    let (bg, fg, ring) = banner_colours(&ctx, tone);
    let width = ui.available_width();
    let button_role = Role { size: 14.0, face: BOLD, tracking_em: 0.0 };
    let button = galley(&ctx, button_role, label, fg, None, None);
    let button_w = button.size().x + 28.0;
    let copy_w = (width - 16.0 - 14.0 - 22.0 - 12.0 - 12.0 - button_w).max(40.0);
    let head = galley(&ctx, Role { size: 15.0, face: EXTRABOLD, tracking_em: 0.0 }, title, fg, Some(copy_w), None);
    let body = galley(&ctx, Role { size: 14.0, face: theme::REGULAR, tracking_em: 0.0 }, text, fg.gamma_multiply(0.9), Some(copy_w), Some(14.0 * 1.45));
    let copy_h = head.size().y + 3.0 + body.size().y;
    let height = copy_h.max(36.0) + 28.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(16), bg);
    let mark = if tone == BannerTone::Caution { glyph::WIFI_OFF } else { TRIANGLE_ALERT };
    icon::stroke(ui.painter(), Rect::from_min_size(pos2(rect.left() + 16.0, rect.top() + 15.0), Vec2::splat(22.0)), mark, 2.0, fg);
    let copy_x = rect.left() + 16.0 + 22.0 + 12.0;
    let copy_y = rect.center().y - copy_h / 2.0;
    let head_h = head.size().y;
    ui.painter().galley(pos2(copy_x, copy_y), head, fg);
    ui.painter().galley(pos2(copy_x, copy_y + head_h + 3.0), body, fg);
    let slot = Rect::from_min_size(pos2(rect.right() - 14.0 - button_w, rect.center().y - 18.0), vec2(button_w, 36.0));
    let response = ui.interact(slot, ui.id().with(("auth-banner", label)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let face = if response.hovered() { Color32::from_rgb(0xfb, 0xfa, 0xf9) } else { Color32::WHITE };
    ui.painter().rect(slot, CornerRadius::same(11), face, Stroke::new(1.0, ring), egui::StrokeKind::Inside);
    ui.painter().galley(slot.center() - button.size() / 2.0, button, fg);
    response.clicked()
}

/// A bar of `fraction` of `track`'s width sliding along it (`dw-line`).
fn slide_bar(ui: &Ui, track: Rect, back: Color32, fore: Color32, fraction: f32) {
    let radius = CornerRadius::same((track.height() / 2.0).ceil() as u8);
    ui.painter().rect_filled(track, radius, back);
    let bar = track.width() * fraction;
    let t = ui.input(|i| i.time);
    let rect = Rect::from_min_size(pos2(track.left() + motion::line(t) * bar, track.top()), vec2(bar, track.height())).intersect(track);
    if rect.width() > 0.0 {
        ui.painter().rect_filled(rect, radius, fore);
    }
    ui.ctx().request_repaint();
}

/// The sign-in screen's big blue button: 56 high, radius 16. While `busy` it
/// reads `label` over a 120 by 3 bar and sits at 85%.
pub fn submit_button(ui: &mut Ui, label: &str, busy: bool, width: f32) -> egui::Response {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 56.0), Sense::click());
    let radius = CornerRadius::same(16);
    let alpha = if busy { 0.85 } else { 1.0 };
    let hovered = response.hovered() && !busy;
    let shadow = Shadow { offset: [0, 8], blur: 20, spread: 0, color: Color32::from_rgba_unmultiplied(27, 63, 160, 77) };
    ui.painter().add(shadow.as_shape(rect, radius));
    ui.painter().rect_filled(rect, radius, (if hovered { p.blue_deep } else { p.blue }).gamma_multiply(alpha));
    let role = Role { size: 17.0, face: BOLD, tracking_em: 0.0 };
    let g = galley(&ctx, role, label, Color32::WHITE, None, None);
    if busy {
        let block = g.size().y + 8.0 + BUTTON_BAR.y;
        let top = rect.center().y - block / 2.0;
        let label_h = g.size().y;
        ui.painter().galley_with_override_text_color(pos2(rect.center().x - g.size().x / 2.0, top), g, Color32::WHITE.gamma_multiply(alpha));
        let track = Rect::from_min_size(pos2(rect.center().x - BUTTON_BAR.x / 2.0, top + label_h + 8.0), BUTTON_BAR);
        slide_bar(ui, track, Color32::from_white_alpha(56), Color32::WHITE, 0.3);
    } else {
        ui.painter().galley_with_override_text_color(rect.center() - g.size() / 2.0, g, Color32::WHITE);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A text link at the larger size the 2FA and device stages use (15).
pub fn link_big(ui: &mut Ui, text: &str) -> egui::Response {
    let ctx = ui.ctx().clone();
    let p = palette(&ctx);
    let role = Role { size: 15.0, face: SEMIBOLD, tracking_em: 0.0 };
    let g = galley(&ctx, role, text, p.blue, None, None);
    let (rect, response) = ui.allocate_exact_size(g.size() + vec2(0.0, 8.0), Sense::click());
    let colour = if response.hovered() { p.blue_deep } else { p.blue };
    ui.painter().galley_with_override_text_color(pos2(rect.left(), rect.center().y - g.size().y / 2.0), g, colour);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A row of [`link_big`]s, 22 apart. Returns the index of the one pressed.
pub fn link_row(ui: &mut Ui, labels: &[&str]) -> Option<usize> {
    let mut pressed = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 22.0;
        for (i, label) in labels.iter().enumerate() {
            if link_big(ui, label).clicked() {
                pressed = Some(i);
            }
        }
    });
    pressed
}

/// The 56 square tile that heads a stage: `d` in blue on blue wash, or in red
/// on a red wash when `danger`.
pub fn stage_tile(ui: &mut Ui, d: &str, danger: bool) {
    let p = palette(ui.ctx());
    let (bg, fg) = if danger { (banner_colours(ui.ctx(), BannerTone::Danger).0, p.error) } else { (p.blue_wash, p.blue) };
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(16), bg);
    icon::stroke(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(28.0)), d, 2.0, fg);
}

/// The six boxes of a one-time code (64 high, radius 14, mono 26). `code`
/// takes only digits, at most six; the box the next digit goes into is blue
/// and an `error` paints all six red. One invisible text field lies over them
/// so typing, pasting and an authenticator's autofill all land in `code`.
pub fn code_boxes(ui: &mut Ui, id: egui::Id, code: &mut String, error: bool, width: f32, enabled: bool) -> egui::Response {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, 64.0), Sense::hover());
    let cell = (width - 8.0 * 5.0) / 6.0;
    let digits: Vec<char> = code.chars().collect();
    for i in 0..6 {
        let slot = Rect::from_min_size(pos2(rect.left() + i as f32 * (cell + 8.0), rect.top()), vec2(cell, 64.0));
        let next = enabled && i == digits.len();
        let (edge, fill) = if error {
            (p.error, error_wash(ui.ctx()))
        } else if next {
            (p.blue, p.card)
        } else {
            (p.border, p.card)
        };
        ui.painter().rect(slot, CornerRadius::same(14), fill, Stroke::new(FIELD_EDGE, edge), egui::StrokeKind::Inside);
        if let Some(ch) = digits.get(i) {
            let font = egui::FontId::new(26.0, egui::FontFamily::Monospace);
            ui.painter().text(slot.center() + vec2(0.0, 2.0), egui::Align2::CENTER_CENTER, ch, font, p.ink);
        }
    }
    // A child at the boxes' rectangle, so the field lies over them without
    // taking room of its own.
    let mut over = ui.new_child(UiBuilder::new().max_rect(rect));
    let visuals = over.visuals_mut();
    visuals.text_cursor.stroke = Stroke::NONE;
    visuals.selection.bg_fill = Color32::TRANSPARENT;
    visuals.selection.stroke = Stroke::NONE;
    let response = over.add_enabled(
        enabled,
        egui::TextEdit::singleline(code)
            .id(id)
            .frame(egui::Frame::NONE)
            .text_color(Color32::TRANSPARENT)
            .desired_width(rect.width()),
    );
    code.retain(|c| c.is_ascii_digit());
    if code.len() > 6 {
        code.truncate(6);
    }
    response
}

/// The stage shown for the moment between a sign-in that worked and the
/// window that follows it: a green ring with a tick, "You're in", and the bar.
pub fn done_stage(ui: &mut Ui) {
    let p = palette(ui.ctx());
    ui.spacing_mut().item_spacing.y = 16.0;
    let (ring, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
    ui.painter().circle_filled(ring.center(), 32.0, p.done_wash);
    icon::stroke(ui.painter(), Rect::from_center_size(ring.center(), Vec2::splat(30.0)), DONE_TICK, 2.6, p.done_ink);
    stage_title(ui, i18n::t("auth.done_title"));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let (track, _) = ui.allocate_exact_size(BUTTON_BAR, Sense::hover());
        slide_bar(ui, track, Color32::from_rgb(0xe3, 0xe8, 0xf6), p.blue, 0.3);
        let role = Role { size: 15.0, face: SEMIBOLD, tracking_em: 0.0 };
        let g = galley(ui.ctx(), role, i18n::t("auth.done_opening"), p.text_muted, None, None);
        let (rect, _) = ui.allocate_exact_size(g.size(), Sense::hover());
        ui.painter().galley(rect.min, g, p.text_muted);
    });
}

/// The panel in the too-many-tries stage: a grey card with a `label` and the
/// `value` beside it.
pub fn wait_panel(ui: &mut Ui, label: &str, value: &str) {
    let p = palette(ui.ctx());
    let width = ui.available_width();
    let l = galley(ui.ctx(), Role { size: 15.0, face: SEMIBOLD, tracking_em: 0.0 }, label, p.text_muted, None, None);
    let v = galley(ui.ctx(), Role { size: 24.0, face: EXTRABOLD, tracking_em: 0.0 }, value, p.ink, Some(width - 40.0 - l.size().x - 10.0), None);
    let height = v.size().y + 36.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(16), p.window_bg);
    let base = rect.bottom() - 18.0;
    let label_x = rect.left() + 20.0;
    let value_x = label_x + l.size().x + 10.0;
    ui.painter().galley(pos2(label_x, base - l.size().y - 2.0), l, p.text_muted);
    ui.painter().galley(pos2(value_x, base - v.size().y), v, p.ink);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at};

    const WIDE: Vec2 = vec2(1280.0, 820.0);

    fn paint(ctx: &egui::Context, size: Vec2, events: Vec<egui::Event>, add: impl FnOnce(&mut Ui)) -> testing::Painted {
        let mut add = Some(add);
        testing::frame(ctx, ctx.input(|i| i.time) + 0.2, size, events, |ui| {
            let full = ui.max_rect();
            let _ = split(ui, full, CloseControl::Active, |ui| (add.take().unwrap())(ui));
        })
    }

    #[test]
    fn the_split_is_a_navy_half_and_a_white_half_with_the_brand_copy() {
        let ctx = testing::ctx();
        let mut painted = testing::Painted::default();
        for _ in 0..8 {
            painted = paint(&ctx, WIDE, vec![], |ui| title(ui, "Log in"));
        }
        let navy = painted.rects.iter().find(|r| r.fill == palette(&ctx).blue_deep).expect("the brand half");
        assert_eq!(navy.rect.width(), 640.0);
        assert_eq!(navy.rect.height(), 820.0);
        for copy in [headline().split_whitespace().next().unwrap(), "Sign in once", brand_foot(), WORDMARK, "Log in"] {
            assert!(painted.text(copy).is_some(), "{copy} is not on the screen");
        }
    }

    #[test]
    fn the_form_column_is_420_wide_and_centred_on_the_right_half() {
        let ctx = testing::ctx();
        let mut seen = Rect::NOTHING;
        for _ in 0..8 {
            paint(&ctx, WIDE, vec![], |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::hover());
                seen = rect;
            });
        }
        assert_eq!(seen.width(), 420.0);
        assert!((seen.center().x - 960.0).abs() < 0.5, "centred on x 960, got {}", seen.center().x);
    }

    #[test]
    fn a_narrow_window_drops_the_brand_half() {
        let ctx = testing::ctx();
        let mut painted = testing::Painted::default();
        let mut seen = Rect::NOTHING;
        for _ in 0..8 {
            painted = paint(&ctx, vec2(600.0, 700.0), vec![], |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::hover());
                seen = rect;
            });
        }
        assert!(painted.text(brand_foot()).is_none());
        assert_eq!(seen.width(), 420.0, "the form keeps its width when it fits");
        let ctx = testing::ctx();
        for _ in 0..8 {
            paint(&ctx, vec2(400.0, 700.0), vec![], |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::hover());
                seen = rect;
            });
        }
        assert_eq!(seen.width(), 352.0, "a window under 468 wide leaves the form 24 either side");
    }

    #[test]
    fn the_caption_buttons_minimise_and_close_and_the_close_can_be_ghosted() {
        let ctx = testing::ctx();
        let full = Rect::from_min_size(pos2(0.0, 0.0), WIDE);
        let run = |ctx: &egui::Context, at: Option<egui::Pos2>, close: CloseControl| {
            let mut got = ChromeAction::None;
            let events = at.map(click_at).unwrap_or_default();
            testing::frame(ctx, 1.0, WIDE, events, |ui| got = caption_buttons(ui, full, Color32::GRAY, close));
            got
        };
        for _ in 0..3 {
            run(&ctx, None, CloseControl::Active);
        }
        assert_eq!(run(&ctx, Some(pos2(1165.0, 17.0)), CloseControl::Active), ChromeAction::Minimize);
        assert_eq!(run(&ctx, Some(pos2(1280.0 - 23.0, 17.0)), CloseControl::Active), ChromeAction::Close);
        let ctx = testing::ctx();
        for _ in 0..3 {
            run(&ctx, None, CloseControl::Disabled);
        }
        assert_eq!(run(&ctx, Some(pos2(1280.0 - 23.0, 17.0)), CloseControl::Disabled), ChromeAction::None, "a ghosted close cannot be pressed");
    }

    #[test]
    fn the_lock_surface_is_navy_with_the_shield_heading_and_line() {
        let ctx = testing::ctx();
        let mut painted = testing::Painted::default();
        for i in 0..8 {
            painted = testing::frame(&ctx, 1.0 + i as f64 * 0.2, WIDE, vec![], |ui| {
                let full = ui.max_rect();
                let _ = lock_surface(ui, full, CloseControl::Active, |ui| lock_header(ui, lock_heading(), lock_sub()));
            });
        }
        assert!(painted.rects.iter().any(|r| r.fill == palette(&ctx).blue_deep && r.rect.size() == WIDE));
        let heading = painted.text(lock_heading()).expect("the heading");
        assert_eq!(heading.1, Color32::WHITE);
        assert_eq!(painted.text(lock_sub()).expect("the line").1, palette(&ctx).blue_edge);
        assert!((heading.2.center().x - 640.0).abs() < 1.0, "centred");
    }

    #[test]
    fn the_hello_button_is_62_high_and_says_checking_while_it_checks() {
        let ctx = testing::ctx();
        let mut size = Vec2::ZERO;
        let p = testing::frame(&ctx, 1.0, WIDE, vec![], |ui| {
            size = hello_button(ui, hello_label(), false).rect.size();
        });
        assert_eq!(size.y, 62.0);
        assert!(p.text(hello_label()).is_some());
        let p = testing::frame(&ctx, 1.1, WIDE, vec![], |ui| {
            hello_button(ui, hello_checking(), true);
        });
        assert!(p.text("Checking").is_some());
    }

    #[test]
    fn the_lock_button_and_link_answer_clicks() {
        let ctx = testing::ctx();
        let mut button = Rect::NOTHING;
        let mut link = Rect::NOTHING;
        for t in 0..3 {
            testing::frame(&ctx, t as f64, WIDE, vec![], |ui| {
                button = lock_button(ui, "Unlock").rect;
                link = lock_link(ui, use_password_label()).rect;
            });
        }
        assert_eq!(button.height(), 58.0);
        let mut clicked = (false, false);
        testing::frame(&ctx, 3.0, WIDE, click_at(button.center()), |ui| {
            clicked.0 = lock_button(ui, "Unlock").clicked();
            lock_link(ui, use_password_label());
        });
        testing::frame(&ctx, 4.0, WIDE, vec![egui::Event::PointerGone], |ui| {
            lock_button(ui, "Unlock");
            lock_link(ui, use_password_label());
        });
        testing::frame(&ctx, 5.0, WIDE, click_at(link.center()), |ui| {
            lock_button(ui, "Unlock");
            clicked.1 = lock_link(ui, use_password_label()).clicked();
        });
        assert_eq!(clicked, (true, true));
    }

    #[test]
    fn an_error_on_navy_sits_on_a_white_chip_in_the_error_colour() {
        let ctx = testing::ctx();
        let p = testing::frame(&ctx, 1.0, WIDE, vec![], |ui| error_chip(ui, "That master password is not right.", 440.0));
        let text = p.text("master password").expect("the message");
        assert_eq!(text.1, palette(&ctx).error);
        assert!(p.rects.iter().any(|r| r.fill == palette(&ctx).card && r.rect.contains_rect(text.2)));
    }

    // ---- the sign-in gallery's parts (slice 15a) ---------------------------

    fn one(ctx: &egui::Context, time: f64, events: Vec<egui::Event>, add: impl FnMut(&mut Ui)) -> testing::Painted {
        testing::frame(ctx, time, vec2(420.0, 600.0), events, add)
    }

    #[test]
    fn a_field_error_is_the_error_colour_with_the_alert_mark_in_front() {
        let ctx = testing::ctx();
        let p = one(&ctx, 1.0, vec![], |ui| field_error(ui, "Enter your email."));
        let text = p.text("Enter your email.").expect("the message");
        assert_eq!(text.1, palette(&ctx).error);
        assert!(text.2.left() >= 23.0, "the message starts after the 16 mark and its 7 gap");
    }

    #[test]
    fn an_errored_field_is_red_on_a_wash_and_a_good_one_is_neither() {
        let ctx = testing::ctx();
        let mut value = String::new();
        let bad = one(&ctx, 1.0, vec![], |ui| {
            field_text(ui, egui::Id::new("f"), &mut value, "you@company.com", 420.0, true, None);
        });
        let red = bad.rects.iter().find(|r| r.stroke.color == palette(&ctx).error).expect("a red box");
        assert_eq!(red.stroke.width, 1.5);
        assert_eq!(red.rect.height(), 50.0);
        assert_ne!(red.fill, palette(&ctx).card);
        let good = one(&ctx, 2.0, vec![], |ui| {
            field_text(ui, egui::Id::new("f"), &mut value, "you@company.com", 420.0, false, None);
        });
        assert!(good.rects.iter().all(|r| r.stroke.color != palette(&ctx).error));
    }

    #[test]
    fn a_field_can_carry_a_grey_prefix() {
        let ctx = testing::ctx();
        let mut value = "vault.example.com".to_string();
        let p = one(&ctx, 1.0, vec![], |ui| {
            field_text(ui, egui::Id::new("u"), &mut value, "", 420.0, false, Some("https://"));
        });
        let prefix = p.text("https://").expect("the prefix");
        assert_eq!(prefix.1, palette(&ctx).text_ghost);
        assert!(p.text("vault.example.com").unwrap().2.left() > prefix.2.right());
    }

    #[test]
    fn the_banner_has_its_words_and_one_button_that_answers() {
        let ctx = testing::ctx();
        let p = one(&ctx, 1.0, vec![], |ui| {
            banner(ui, BannerTone::Caution, "You're offline", "Connect to the internet.", "Try again");
        });
        for word in ["You're offline", "Connect to the internet.", "Try again"] {
            assert!(p.text(word).is_some(), "{word}");
        }
        let at = p.text("Try again").unwrap().2.center();
        let mut pressed = false;
        one(&ctx, 2.0, click_at(at), |ui| {
            pressed = banner(ui, BannerTone::Caution, "You're offline", "Connect to the internet.", "Try again");
        });
        assert!(pressed);
    }

    #[test]
    fn the_big_button_shows_a_bar_inside_it_only_while_busy() {
        let ctx = testing::ctx();
        let idle = one(&ctx, 1.0, vec![], |ui| {
            submit_button(ui, "Log in", false, 420.0);
        });
        let busy = one(&ctx, 2.0, vec![], |ui| {
            submit_button(ui, "Checking", true, 420.0);
        });
        let bars = |p: &testing::Painted| p.rects.iter().filter(|r| r.rect.height() == 3.0).count();
        assert_eq!(bars(&idle), 0);
        assert!(bars(&busy) >= 1, "the 3 high track");
        assert!(busy.text("Checking").is_some());
        assert!(busy.repaint_delay < std::time::Duration::from_secs(1), "the bar keeps moving");
    }

    #[test]
    fn the_code_boxes_take_digits_only_and_at_most_six() {
        let ctx = testing::ctx();
        let mut code = "12a3 456789".to_string();
        let mut painted = testing::Painted::default();
        for i in 0..4 {
            painted = one(&ctx, 1.0 + f64::from(i) * 0.1, vec![], |ui| {
                code_boxes(ui, egui::Id::new("c"), &mut code, false, 420.0, true);
            });
        }
        assert_eq!(code, "123456");
        let boxes = painted.rects.iter().filter(|r| r.rect.height() == 64.0).count();
        assert_eq!(boxes, 6);
        for digit in ["1", "2", "3", "4", "5", "6"] {
            assert!(painted.text(digit).is_some(), "{digit}");
        }
    }

    #[test]
    fn the_next_box_is_blue_and_an_error_makes_all_six_red() {
        let ctx = testing::ctx();
        let p = palette(&ctx);
        let mut code = "48".to_string();
        let painted = one(&ctx, 1.0, vec![], |ui| {
            code_boxes(ui, egui::Id::new("c"), &mut code, false, 420.0, true);
        });
        let blue = painted.rects.iter().filter(|r| r.rect.height() == 64.0 && r.stroke.color == p.blue).count();
        assert_eq!(blue, 1);
        let painted = one(&ctx, 2.0, vec![], |ui| {
            code_boxes(ui, egui::Id::new("c"), &mut code, true, 420.0, true);
        });
        let red = painted.rects.iter().filter(|r| r.rect.height() == 64.0 && r.stroke.color == p.error).count();
        assert_eq!(red, 6);
    }

    #[test]
    fn the_done_stage_says_you_are_in_and_what_is_next() {
        let ctx = testing::ctx();
        let p = one(&ctx, 1.0, vec![], done_stage);
        assert!(p.text("You're in").is_some());
        assert!(p.text("Opening your vault").is_some());
        assert!(p.circles.iter().any(|c| c.radius == 32.0 && c.fill == palette(&ctx).done_wash));
    }

    #[test]
    fn the_wait_panel_sets_its_label_and_value_on_one_grey_card() {
        let ctx = testing::ctx();
        let p = one(&ctx, 1.0, vec![], |ui| wait_panel(ui, "Try again in", "a few minutes"));
        let label = p.text("Try again in").unwrap().2;
        let value = p.text("a few minutes").unwrap().2;
        assert!(value.left() > label.right());
        assert!(p.rects.iter().any(|r| r.fill == palette(&ctx).window_bg && r.rect.contains_rect(value)));
    }

    #[test]
    fn every_new_glyph_parses() {
        let ctx = testing::ctx();
        one(&ctx, 1.0, vec![], |ui| {
            for d in [ALERT_CIRCLE, PHONE_CODE, TRIANGLE_ALERT, DONE_TICK, LOCK_TILE] {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                icon::stroke(ui.painter(), rect, d, 2.0, Color32::BLACK);
            }
        });
    }
}

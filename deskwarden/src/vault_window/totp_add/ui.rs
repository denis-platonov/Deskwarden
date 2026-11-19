//! Spec B section 8: the Add 2FA modal in the v2 look, every stage of it.
//!
//! One frame for all of them: `kit::modal::Dialog` (580 wide) titled "Add 2FA
//! code" with the 20 px `?` tip under it and "For {item}" as its sub-line.
//! What changes from stage to stage is only the body:
//!
//! * the picker: the design's two cards, with 0.15's other two routes (an
//!   image file, a webcam) as two quiet rows under them;
//! * scanning: the dark panel with the decorative code, its brackets and the
//!   moving line;
//! * the camera: the same dark panel holding the picture;
//! * the by-hand and the read-from-a-code form: the key field, the live
//!   code, the Digits and Period controls, and the table of what was read.
//!
//! The state machine, the readers and the actions are the parent module's and
//! did not change; this file only draws them.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Vec2};

use super::*;
use crate::kit::controls::{self, ButtonKind, ButtonSize};
use crate::kit::icon::{self, glyph};
use crate::kit::modal::{self, Dialog, ModalOutcome};
use crate::kit::tokens::{self, palette, Role};

/// The scrim's area name is `"totp-add-scrim"` (listed in
/// `item_list::MODAL_SCRIM_AREAS`).
pub const ID: &str = "totp-add";

/// The id of the Save code / Replace code answer, for tests.
pub fn save_id() -> egui::Id {
    egui::Id::new((ID, "save"))
}

/// The id of the Back / Scan again answer, for tests.
pub fn back_id() -> egui::Id {
    egui::Id::new((ID, "back"))
}

/// The id of the live code (the preview row, or the found card), for tests.
pub fn code_id() -> egui::Id {
    egui::Id::new((ID, "code"))
}

/// The id of the Reveal / Hide control in the table, for tests.
pub fn reveal_id() -> egui::Id {
    egui::Id::new((ID, "reveal"))
}

/// The id of a table row, by its label, for tests.
pub fn table_row_id(label: &str) -> egui::Id {
    egui::Id::new((ID, "row", label.to_string()))
}

/// Spec B section 2: the QR modal is 580 wide.
const WIDTH: f32 = 580.0;

/// Spec B 0.4: the QR tip's bubble is 280 wide.
const TIP_WIDTH: f32 = 280.0;

/// Spec B section 8: the tip under the title, verbatim.
pub const TIP: &str = "When you turn on two-step sign-in, the site shows a QR code. Scan it here and you won't need a phone app.";

/// The caption on the scan panel, verbatim.
pub const LOOKING_CAPTION: &str = "Looking for a code\u{2026}";

/// What the scanned card says when the code carries neither an issuer nor an
/// account (0.15's heading for the same card).
pub const FOUND_TITLE: &str = "Found it";

/// The question under the found card, verbatim from the design.
pub const MATCH_PROMPT: &str = "Does this match the code on the site?";

/// The preview row's label on the by-hand form, verbatim.
pub const CODE_NOW: &str = "Your code right now";

/// The two answers' button: 52 high, radius 15, 16 px (spec B section 8).
const ANSWER: ButtonSize = ButtonSize { height: 52.0, pad_x: 18.0, radius: 15, font_px: 16.0 };

/// The dark panel behind the scan and the camera: `#1d1f2b`, radius 20.
const PANEL: Color32 = Color32::from_rgb(0x1d, 0x1f, 0x2b);
const PANEL_HEIGHT: f32 = 300.0;
const PANEL_RADIUS: u8 = 20;
const CAPTION_INK: Color32 = Color32::from_rgb(0xe8, 0xea, 0xf2);
const PANEL_NOTE_INK: Color32 = Color32::from_rgb(0xa4, 0xa9, 0xc0);
const SCAN_LINE: Color32 = Color32::from_rgb(0x3b, 0x74, 0xe8);
const BRACKET: Color32 = Color32::from_rgb(0x7f, 0xa4, 0xef);

/// Spacing between a body's blocks (spec B 0.6 cards use 16).
const GAP: f32 = 16.0;

fn bold(text: &str, px: f32, ink: Color32) -> egui::RichText {
    egui::RichText::new(text)
        .font(egui::FontId::new(px, egui::FontFamily::Name(theme::BOLD.into())))
        .color(ink)
}

fn plain(text: &str, px: f32, ink: Color32) -> egui::RichText {
    egui::RichText::new(text).size(px).color(ink)
}

/// The design's `rgba(...)` tints do not need the palette: they are the same
/// in both themes because the panel they sit on is.
fn layout(ui: &egui::Ui, role: Role, text: &str, ink: Color32) -> std::sync::Arc<egui::Galley> {
    ui.fonts_mut(|f| f.layout_job(tokens::job(role, text, ink)))
}

// ---------------------------------------------------------------------------
// The frame
// ---------------------------------------------------------------------------

/// **The whole modal**: the dialog, its header, and the stage it is at.
///
/// Esc belongs to the dialog at every stage but one: while the region overlay
/// is up it is a full-screen window with the keyboard, its own Escape cancels
/// the scan, and a stray one landing here would throw the form away on the
/// keystroke the user meant as "stop scanning".
pub fn draw_add_modal(ctx: &egui::Context, state: &mut TotpAdd, now_unix: u64) -> TotpAddAction {
    let sub = format!("For {}", state.item_name);
    // A body taller than the window scrolls inside the card instead of running
    // off it: the title block and the margins stay put.
    let room = (ctx.content_rect().height() - 2.0 * modal::GUTTER - 2.0 * f32::from(modal::PADDING) - 110.0).max(240.0);
    let (outcome, action) = Dialog::new(ID, WIDTH)
        .title(PICKER_TITLE)
        .sub(&sub)
        .title_tip(TIP, TIP_WIDTH)
        .esc_closes(state.stage != Stage::Scanning)
        .show(ctx, |ui| {
            // The body is drawn as it is, and measured. Only a window too short
            // to hold it puts it in a scroll area, sized to the room there is
            // -- `ScrollArea`'s own `auto_shrink` sized it from what it
            // measured while the card was still being laid out, and left the
            // card the height of its first pass.
            let measured = egui::Id::new((ID, "body-height"));
            let too_tall = ctx.data(|d| d.get_temp::<f32>(measured)).is_some_and(|height| height > room);
            let (action, height) = if too_tall {
                egui::ScrollArea::vertical()
                    .id_salt((ID, "body"))
                    .max_height(room)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let top = ui.min_rect().top();
                        let action = draw_stage(ui, state, now_unix);
                        (action, ui.min_rect().bottom() - top)
                    })
                    .inner
            } else {
                let top = ui.cursor().top();
                let action = draw_stage(ui, state, now_unix);
                (action, ui.min_rect().bottom() - top)
            };
            ctx.data_mut(|d| d.insert_temp(measured, height));
            action
        });
    // The card is as tall as its stage, and egui anchors it by the height it
    // had last pass: on the frame a stage changes the new one would appear at
    // the old one's corner and then move. `settle_reshaped_modal` throws that
    // pass away before it is painted.
    theme::settle_reshaped_modal(ctx, modal::card_id(ID), state.stage);
    match (action, outcome) {
        (TotpAddAction::None, ModalOutcome::Closed) => TotpAddAction::Cancel,
        (action, _) => action,
    }
}

/// **The one place that decides which half of this surface is on screen**, so
/// no caller has to know there are four.
///
/// The camera is advanced BEFORE the match, and that is load-bearing:
/// [`advance_webcam`] can change the stage (a code read on this frame moves
/// the form on, a camera that has just refused moves it back to the picker),
/// and running it first means the frame that learns either of those paints
/// the stage it learned about. Its clock is an `Instant` and not `now_unix`
/// because the only thing measured against a camera is an elapsed duration,
/// and a wall clock moved by a time sync would make it negative.
pub fn draw_stage(ui: &mut egui::Ui, state: &mut TotpAdd, now_unix: u64) -> TotpAddAction {
    let frame = (state.stage == Stage::Webcam)
        .then(|| advance_webcam(state, std::time::Instant::now()))
        .flatten();
    ui.spacing_mut().item_spacing.y = GAP;
    match state.stage {
        Stage::Picker => draw_picker(ui, state).action,
        Stage::Scanning => draw_scanning(ui),
        Stage::Webcam => draw_webcam(ui, state, frame),
        Stage::Manual => draw_add_form(ui, state, now_unix),
    }
}

// ---------------------------------------------------------------------------
// Small shared pieces
// ---------------------------------------------------------------------------

/// The refusal sentence, as the design's danger tint.
fn refusal_note(ui: &mut egui::Ui, text: &str) {
    modal::note(ui, modal::Tone::Danger, text);
}

/// The amber band over the buttons when saving would replace a code.
fn caution_note(ui: &mut egui::Ui) {
    modal::note(ui, modal::Tone::Caution, REPLACE_WARNING);
}

/// A quiet line under the body: the info mark and a sentence.
fn quiet_line(ui: &mut egui::Ui, text: &str) {
    let p = palette(ui.ctx());
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (slot, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        paint_svg(ui.painter(), slot, INFO_MARK, 2.0, p.text_label);
        ui.add(egui::Label::new(plain(text, 13.0, p.text_icon)).wrap());
    });
}

// ---------------------------------------------------------------------------
// The picker
// ---------------------------------------------------------------------------

/// What [`draw_picker`] drew and what was pressed.
pub struct PickerFrame {
    pub action: TotpAddAction,
    /// Where each route's card or row was drawn, for the tests.
    pub rows: Vec<(Route, egui::Rect)>,
}

/// Spec B section 8 `choose`: the two cards, then the other two routes as
/// quiet rows, the last refusal if there is one, and the privacy sentence.
pub fn draw_picker(ui: &mut egui::Ui, state: &mut TotpAdd) -> PickerFrame {
    let mut action = TotpAddAction::None;
    let mut rows = Vec::with_capacity(ROUTES.len());
    let mut chosen = None;

    ui.spacing_mut().item_spacing.x = 12.0;
    ui.columns(2, |columns| {
        for (column, card) in columns.iter_mut().zip(ROUTE_CARDS.iter()) {
            let (response, rect) = route_card(column, card);
            rows.push((card.route, rect));
            if response.clicked() {
                chosen = Some(card.route);
            }
        }
    });
    // 0.15's other two routes, in the design's quiet row.
    ui.spacing_mut().item_spacing.y = 10.0;
    for row in ROUTES.iter().filter(|row| !ROUTE_CARDS.iter().any(|card| card.route == row.route)) {
        let (response, rect) = quiet_row(ui, row);
        rows.push((row.route, rect));
        if response.clicked() {
            chosen = Some(row.route);
        }
    }
    ui.spacing_mut().item_spacing.y = GAP;

    // The keys 0.15 answered (Enter and 1-4) still do, and only here: this
    // function is the picker's and nowhere else's, so a bare `3` cannot reach
    // the secret field. A pressed row beats a key on the same frame.
    if chosen.is_none() {
        chosen = ui.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::Key { key, pressed: true, modifiers, .. } => route_for_key(&ROUTES, *key, *modifiers),
                _ => None,
            })
        });
    }
    if let Some(route) = chosen {
        if route == Route::ByHand {
            state.stage = Stage::Manual;
            state.refusal = None;
        } else {
            action = action_for(route);
        }
    }

    if let Some(refusal) = state.refusal {
        refusal_note(ui, &refusal.sentence());
    }
    quiet_line(ui, PRIVACY_LINE);
    PickerFrame { action, rows }
}

/// Spec B section 8's two cards: radius 18, a 2 px edge, a 52 px tile, title
/// 18/800 and sub 15.
#[derive(Clone, Copy)]
pub struct RouteCard {
    pub route: Route,
    pub title: &'static str,
    pub sub: &'static str,
    primary: bool,
}

pub const ROUTE_CARDS: [RouteCard; 2] = [
    RouteCard { route: Route::ScanRegion, title: "Scan from screen", sub: "Draw a box around the code", primary: true },
    RouteCard { route: Route::ByHand, title: "Type the key", sub: "The code of letters under the QR", primary: false },
];

fn route_card(ui: &mut egui::Ui, card: &RouteCard) -> (egui::Response, egui::Rect) {
    let p = palette(ui.ctx());
    let width = ui.available_width();
    let (slot, response) = ui.allocate_exact_size(egui::vec2(width, 172.0), Sense::click());
    let hovered = response.hovered();
    let rect = if hovered { slot.translate(egui::vec2(0.0, -3.0)) } else { slot };
    let (edge, fill, tile, glyph_ink, title_ink) = if card.primary {
        (p.blue, p.chip_on, p.blue, Color32::WHITE, p.blue_deep)
    } else {
        let edge = if hovered { Color32::from_rgb(0xcf, 0xcc, 0xc8) } else { p.hairline };
        (edge, p.card, p.hover, p.text_nav, p.ink)
    };
    if hovered {
        let tint = if card.primary { Color32::from_rgba_unmultiplied(27, 63, 160, 41) } else { Color32::from_rgba_unmultiplied(32, 30, 29, 26) };
        let lift = egui::Shadow { offset: [0, 12], blur: 26, spread: 0, color: tint };
        ui.painter().add(lift.as_shape(rect, CornerRadius::same(18)));
    }
    ui.painter().rect(rect, CornerRadius::same(18), fill, Stroke::new(2.0, edge), StrokeKind::Inside);
    let tile_rect = Rect::from_min_size(rect.min + egui::vec2(22.0, 22.0), Vec2::splat(52.0));
    ui.painter().rect_filled(tile_rect, CornerRadius::same(15), tile);
    // The design's keyboard glyph for "Type the key", the scan frame for the
    // other.
    let mark = if card.primary { glyph::SCAN } else { KEYBOARD };
    icon::stroke(ui.painter(), Rect::from_center_size(tile_rect.center(), Vec2::splat(26.0)), mark, 2.0, glyph_ink);
    let text_w = rect.width() - 44.0;
    let title = layout_wrapped(ui, Role { size: 18.0, face: theme::EXTRABOLD, tracking_em: 0.0 }, card.title, title_ink, text_w);
    let at = egui::pos2(rect.left() + 22.0, tile_rect.bottom() + 14.0);
    let title_h = title.size().y;
    ui.painter().galley(at, title, title_ink);
    let sub = layout_wrapped(ui, Role { size: 15.0, face: theme::REGULAR, tracking_em: 0.0 }, card.sub, p.text_nav, text_w);
    ui.painter().galley(at + egui::vec2(0.0, title_h + 4.0), sub, p.text_nav);
    (response.on_hover_cursor(egui::CursorIcon::PointingHand), rect)
}

/// The keyboard glyph the design draws on "Type the key".
const KEYBOARD: &str = "M4 6h16a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2zM6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10";

fn layout_wrapped(ui: &egui::Ui, role: Role, text: &str, ink: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = tokens::job(role, text, ink);
    job.wrap.max_width = width;
    ui.fonts_mut(|f| f.layout_job(job))
}

/// 0.15's image-file and webcam routes: an outline row, 15/700 title and a
/// 13 px line under it, in the cards' own radius and edge.
fn quiet_row(ui: &mut egui::Ui, row: &RouteRow) -> (egui::Response, egui::Rect) {
    let p = palette(ui.ctx());
    let width = ui.available_width();
    let text_left = 14.0 + 40.0 + 14.0;
    let title = layout_wrapped(ui, Role { size: 15.0, face: theme::BOLD, tracking_em: 0.0 }, row.title, p.ink, width - text_left - 14.0);
    let sub = layout_wrapped(ui, Role { size: 13.0, face: theme::REGULAR, tracking_em: 0.0 }, row.subtitle, p.text_faint, width - text_left - 14.0);
    let text_h = title.size().y + 2.0 + sub.size().y;
    let height = (text_h + 24.0).max(40.0 + 24.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), Sense::click());
    let hovered = response.hovered();
    let edge = if hovered { Color32::from_rgb(0xcf, 0xcc, 0xc8) } else { p.hairline };
    ui.painter().rect(rect, CornerRadius::same(14), if hovered { p.secondary_hover } else { p.card }, Stroke::new(1.0, edge), StrokeKind::Inside);
    let tile = Rect::from_min_size(egui::pos2(rect.left() + 14.0, rect.center().y - 20.0), Vec2::splat(40.0));
    ui.painter().rect_filled(tile, CornerRadius::same(12), p.hover);
    paint_svg(ui.painter(), Rect::from_center_size(tile.center(), Vec2::splat(20.0)), mark_for(row.route), 2.0, p.text_nav);
    let top = rect.center().y - text_h / 2.0;
    let title_h = title.size().y;
    ui.painter().galley(egui::pos2(rect.left() + text_left, top), title, p.ink);
    ui.painter().galley(egui::pos2(rect.left() + text_left, top + title_h + 2.0), sub, p.text_faint);
    (response.on_hover_cursor(egui::CursorIcon::PointingHand), rect)
}

// ---------------------------------------------------------------------------
// Scanning the screen
// ---------------------------------------------------------------------------

/// The decorative code on the scan tile: spec B section 8's 25 by 25 module
/// grid, three 7 by 7 finder patterns and the design's pseudo-random rest.
pub fn scan_modules() -> Vec<(u8, u8)> {
    let mut on = Vec::new();
    for y in 0..25u8 {
        for x in 0..25u8 {
            let finder = [(0u8, 0u8), (18, 0), (0, 18)]
                .iter()
                .find(|(ox, oy)| x >= *ox && x < ox + 7 && y >= *oy && y < oy + 7);
            let lit = match finder {
                // A finder is a 7x7 ring, a gap, and a 3x3 eye.
                Some((ox, oy)) => {
                    let (fx, fy) = (x - ox, y - oy);
                    fx == 0 || fx == 6 || fy == 0 || fy == 6 || ((2..=4).contains(&fx) && (2..=4).contains(&fy))
                }
                None => (u32::from(x) * 73 + u32::from(y) * 151 + u32::from(x) * u32::from(y) * 7) % 11 < 5,
            };
            if lit {
                on.push((x, y));
            }
        }
    }
    on
}

/// The panel behind the scan and the camera.
fn dark_panel(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui, Rect)) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), PANEL_HEIGHT), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(PANEL_RADIUS), PANEL);
    add(ui, rect);
}

/// Spec B section 8 `scan`: the 200 tile with the 170 code, four bracket
/// corners 8 px outside it and a 3 px line sliding down it, then "Looking for
/// a code..." under it. 0.15's waiting note goes under that.
pub fn draw_scanning(ui: &mut egui::Ui) -> TotpAddAction {
    dark_panel(ui, |ui, panel| {
        let time = ui.input(|i| i.time) as f32;
        // `dw-scan .9s ease-in-out infinite alternate`: top 6 <-> 190.
        let phase = (time / 0.9) % 2.0;
        let t = if phase > 1.0 { 2.0 - phase } else { phase };
        let eased = t * t * (3.0 - 2.0 * t);
        let tile = Rect::from_center_size(egui::pos2(panel.center().x, panel.top() + 28.0 + 100.0), Vec2::splat(200.0));
        ui.painter().rect_filled(tile, CornerRadius::same(10), Color32::WHITE);
        let code = Rect::from_center_size(tile.center(), Vec2::splat(170.0));
        let module = 170.0 / 25.0;
        for (x, y) in scan_modules() {
            let at = code.min + egui::vec2(f32::from(x) * module, f32::from(y) * module);
            ui.painter().rect_filled(Rect::from_min_size(at, Vec2::splat(module)), CornerRadius::ZERO, Color32::from_rgb(0x20, 0x1e, 0x1d));
        }
        for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            bracket(ui.painter(), tile, dx, dy);
        }
        let line_top = tile.top() + 6.0 + (190.0 - 6.0) * eased;
        let line = Rect::from_min_size(egui::pos2(tile.left() + 6.0, line_top), egui::vec2(tile.width() - 12.0, 3.0));
        let glow = egui::Shadow { offset: [0, 0], blur: 14, spread: 4, color: Color32::from_rgba_unmultiplied(59, 116, 232, 140) };
        ui.painter().add(glow.as_shape(line, CornerRadius::same(3)));
        ui.painter().rect_filled(line, CornerRadius::same(3), SCAN_LINE);
        let caption = layout(ui, Role { size: 16.0, face: theme::SEMIBOLD, tracking_em: 0.0 }, LOOKING_CAPTION, CAPTION_INK);
        let caption_at = egui::pos2(panel.center().x - caption.size().x / 2.0, tile.bottom() + 18.0);
        let caption_h = caption.size().y;
        ui.painter().galley(caption_at, caption, CAPTION_INK);
        let note = layout(ui, Role { size: 13.0, face: theme::REGULAR, tracking_em: 0.0 }, SCANNING_NOTE, PANEL_NOTE_INK);
        ui.painter().galley(egui::pos2(panel.center().x - note.size().x / 2.0, caption_at.y + caption_h + 2.0), note, PANEL_NOTE_INK);
    });
    // The line is moving: keep drawing.
    ui.ctx().request_repaint();
    TotpAddAction::None
}

/// One 28 px bracket, 4 px thick, 8 px outside the tile's corner.
fn bracket(painter: &egui::Painter, tile: Rect, dx: f32, dy: f32) {
    let corner = egui::pos2(
        if dx < 0.0 { tile.left() - 8.0 } else { tile.right() + 8.0 },
        if dy < 0.0 { tile.top() - 8.0 } else { tile.bottom() + 8.0 },
    );
    let stroke = Stroke::new(4.0, BRACKET);
    let r = 10.0;
    let (sx, sy) = (-dx, -dy); // direction from the corner into the tile
    let mut points = vec![corner + egui::vec2(sx * 28.0, 0.0), corner + egui::vec2(sx * r, 0.0)];
    for k in 0..=6 {
        let a = std::f32::consts::FRAC_PI_2 * k as f32 / 6.0;
        points.push(corner + egui::vec2(sx * (r - r * a.sin()), sy * (r - r * a.cos())));
    }
    points.push(corner + egui::vec2(0.0, sy * 28.0));
    painter.add(egui::Shape::line(points, stroke));
}

// ---------------------------------------------------------------------------
// The camera
// ---------------------------------------------------------------------------

/// The camera stage: a camera list while none is on, then the picture in the
/// dark panel with what it is doing under it.
pub fn draw_webcam(ui: &mut egui::Ui, state: &mut TotpAdd, frame: Option<crate::webcam::Frame>) -> TotpAddAction {
    let mut action = TotpAddAction::None;
    let mut go_back = false;
    let mut chosen: Option<usize> = None;
    let mut back_to_list = false;

    if let (Some(stage), Some(frame)) = (state.webcam.as_mut(), frame) {
        let image = egui::ColorImage::from_rgba_unmultiplied([frame.width(), frame.height()], frame.pixels());
        match &mut stage.texture {
            Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
            None => {
                stage.texture = Some(ui.ctx().load_texture("totp-add-webcam-preview", image, egui::TextureOptions::LINEAR))
            }
        }
    }

    let Some(stage) = state.webcam.as_ref() else {
        // No camera stage to draw: the caller is mid-transition.
        state.stage = Stage::Picker;
        return action;
    };
    let showing = stage.showing();
    let several = stage.devices.len() > 1;
    let picking = stage.chosen.is_none();
    let texture = stage.texture.clone();
    let size = stage.size;
    let devices: Vec<String> = stage.devices.iter().map(|d| d.name.clone()).collect();
    let p = palette(ui.ctx());

    if picking {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(bold(WEBCAM_CHOOSE, 18.0, p.ink));
            ui.label(plain(WEBCAM_CHOOSE_HINT, 14.0, p.text_faint));
        });
        ui.spacing_mut().item_spacing.y = 10.0;
        for (index, camera) in devices.iter().enumerate() {
            if camera_row(ui, camera).clicked() {
                chosen = Some(index);
            }
        }
        ui.spacing_mut().item_spacing.y = GAP;
    } else {
        dark_panel(ui, |ui, panel| {
            if let (Some(texture), Some(size), true) = (texture.as_ref(), size, showing) {
                let fit = preview_fit(panel.shrink(8.0), size);
                ui.painter().image(
                    texture.id(),
                    fit,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                let caption = layout(ui, Role { size: 16.0, face: theme::SEMIBOLD, tracking_em: 0.0 }, WEBCAM_STARTING, CAPTION_INK);
                ui.painter().galley(panel.center() - caption.size() / 2.0, caption, CAPTION_INK);
            }
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(bold(WEBCAM_HEADING, 18.0, p.ink));
            ui.label(plain(if showing { WEBCAM_HINT } else { WEBCAM_STARTING }, 14.0, p.text_faint));
        });
    }

    quiet_line(ui, WEBCAM_PRIVACY_LINE);

    ui.horizontal(|ui| {
        let both = several && !picking;
        let each = if both { (ui.available_width() - 10.0) / 2.0 } else { ui.available_width() };
        ui.spacing_mut().item_spacing.x = 10.0;
        if both {
            ui.allocate_ui(egui::vec2(each, ANSWER.height), |ui| {
                if controls::button_filling(ui, ButtonKind::Outline, ANSWER, WEBCAM_ANOTHER_LABEL).clicked() {
                    back_to_list = true;
                }
            });
        }
        ui.allocate_ui(egui::vec2(each, ANSWER.height), |ui| {
            if controls::button_filling(ui, ButtonKind::Outline, ANSWER, OTHER_WAYS_LABEL).clicked() {
                go_back = true;
            }
        });
    });

    if go_back {
        state.back_to_picker();
    } else if back_to_list {
        if let Some(stage) = state.webcam.as_mut() {
            stage.session = None;
            stage.chosen = None;
            stage.texture = None;
            stage.size = None;
        }
    } else if let Some(index) = chosen {
        action = TotpAddAction::UseCamera(index);
    }
    action
}

/// One camera in the list: a quiet row with the camera glyph.
fn camera_row(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let p = palette(ui.ctx());
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 64.0), Sense::click());
    let hovered = response.hovered();
    let edge = if hovered { Color32::from_rgb(0xcf, 0xcc, 0xc8) } else { p.hairline };
    ui.painter().rect(rect, CornerRadius::same(14), if hovered { p.secondary_hover } else { p.card }, Stroke::new(1.0, edge), StrokeKind::Inside);
    let tile = Rect::from_min_size(egui::pos2(rect.left() + 14.0, rect.center().y - 20.0), Vec2::splat(40.0));
    ui.painter().rect_filled(tile, CornerRadius::same(12), p.hover);
    paint_svg(ui.painter(), Rect::from_center_size(tile.center(), Vec2::splat(20.0)), WEBCAM_MARK, 2.0, p.text_nav);
    let title = layout_wrapped(ui, Role { size: 15.0, face: theme::BOLD, tracking_em: 0.0 }, name, p.ink, width - 14.0 - 40.0 - 14.0 - 14.0);
    ui.painter().galley(egui::pos2(tile.right() + 14.0, rect.center().y - title.size().y / 2.0), title, p.ink);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------
// The form: by hand, or read from a code
// ---------------------------------------------------------------------------

/// Spec B section 8 `manual` and `found`, on one body: the by-hand form is a
/// key field, its live code and the two parameters; the read-from-a-code form
/// is the found card, its parameters (set by the link, so inert) and the
/// table of what was read. Both end in the same answers.
pub fn draw_add_form(ui: &mut egui::Ui, state: &mut TotpAdd, now_unix: u64) -> TotpAddAction {
    let mut action = TotpAddAction::None;
    let mut copy_asked = TotpAddAction::None;
    let p = palette(ui.ctx());

    // One reading drives the whole body, taken before anything is drawn.
    let reading = if state.scanned {
        let reading = read_field(&state.typed, state.digits, state.period);
        if let Reading::Refused(refusal) = &reading {
            refusal_note(ui, &refusal_sentence(refusal));
        }
        reading
    } else {
        let valid = matches!(read_field(&state.typed, state.digits, state.period), Reading::Ok(_));
        secret_field(ui, &mut state.typed, valid);
        let reading = read_field(&state.typed, state.digits, state.period);
        match &reading {
            Reading::Refused(_) => {
                if let Some(line) = validity_line(&reading) {
                    refusal_note(ui, &line);
                }
            }
            Reading::Ok(_) => {
                if let Some(line) = validity_line(&reading) {
                    ui.label(plain(&line, 13.0, p.done_ink));
                }
            }
            Reading::Empty => {}
        }
        reading
    };

    let (shown_digits, shown_period, live) = controls_for(&reading, &state.typed, state.digits, state.period);
    if !matches!(reading, Reading::Refused(_)) || !state.scanned {
        let (digits, period) = parameter_controls(ui, shown_digits, shown_period, live);
        if let Some(digits) = digits {
            state.digits = digits;
        }
        if let Some(period) = period {
            state.period = period;
        }
        if !live {
            ui.label(plain(PARAMETERS_FROM_URI, 13.0, p.text_label));
        }
    }

    if let Reading::Ok(auth) = &reading {
        let asked = draw_confirmation(ui, auth, state, now_unix);
        if asked != TotpAddAction::None {
            copy_asked = asked;
        }
    }

    if state.already_has_code {
        caution_note(ui);
    }

    // The answers: Back / Save code by hand, Scan again / Save code when the
    // code was read. Save dims to .4 until the field reads as a code.
    let enabled = can_save(&reading);
    let label = submit_label(state.already_has_code);
    let mut save = false;
    let mut back = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let each = (ui.available_width() - 10.0) / 2.0;
        ui.allocate_ui(egui::vec2(each, ANSWER.height), |ui| {
            let back_label = if state.scanned { SCAN_AGAIN_LABEL } else { BACK_LABEL };
            let button = controls::button_filling(ui, ButtonKind::Outline, ANSWER, back_label);
            let _ = ui.interact(button.rect, back_id(), Sense::hover());
            back = button.clicked();
        });
        ui.allocate_ui(egui::vec2(each, ANSWER.height), |ui| {
            ui.scope(|ui| {
                if !enabled {
                    ui.set_opacity(0.4);
                }
                let button = controls::button_filling(ui, ButtonKind::Primary, ANSWER, label);
                let _ = ui.interact(button.rect, save_id(), Sense::hover());
                save = button.clicked() && enabled;
            });
        });
    });
    if save {
        action = TotpAddAction::Save;
    }
    if back {
        state.back_to_picker();
    }
    if copy_asked != TotpAddAction::None && action == TotpAddAction::None {
        action = copy_asked;
    }
    action
}

/// The Back answer on the by-hand form (spec B section 8).
pub const BACK_LABEL: &str = "Back";

/// The first answer on the read-from-a-code form (spec B section 8 `found`).
/// It returns to the picker, where Scan from screen is one press away and so
/// are the image and the camera.
pub const SCAN_AGAIN_LABEL: &str = "Scan again";

/// Spec B section 8: the key field is 58 high, radius 16, a 2 px edge that is
/// blue once the field reads as a code, mono 18 with `.1em` tracking, and its
/// placeholder is `JBSW Y3DP EHPK 3PXP`.
///
/// Not upper-cased as the design's CSS does: the field also takes a whole
/// `otpauth://` link, which is case-sensitive, and a field that rewrote what
/// was pasted would corrupt it.
fn secret_field(ui: &mut egui::Ui, typed: &mut String, valid: bool) -> egui::Response {
    let p = palette(ui.ctx());
    let id = ui.id().with("totp-secret");
    let (outer, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 58.0), Sense::hover());
    let edge = if valid { p.blue } else { p.border };
    ui.painter().rect(outer, CornerRadius::same(16), p.card, Stroke::new(2.0, edge), StrokeKind::Inside);
    let font = egui::FontId::new(18.0, egui::FontFamily::Monospace);
    let inner = outer.shrink2(egui::vec2(18.0, 0.0));
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _wrap: f32| {
        let mut job = egui::text::LayoutJob { wrap: egui::text::TextWrapping::no_max_width(), ..Default::default() };
        job.append(
            buffer.as_str(),
            0.0,
            egui::TextFormat { extra_letter_spacing: 1.8, font_id: font.clone(), color: p.ink, ..Default::default() },
        );
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(typed)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .hint_text(egui::RichText::new("JBSW Y3DP EHPK 3PXP").font(font.clone()).color(p.text_ghost))
            .desired_width(inner.width())
            .vertical_align(egui::Align::Center)
            .layouter(&mut layouter),
    );
    ui.advance_cursor_after_rect(outer);
    response
}

/// Digits and Period: a label over a full-width segmented control each, side
/// by side. When the link that was read set them they are shown and inert.
fn parameter_controls(ui: &mut egui::Ui, digits: u8, period: u16, live: bool) -> (Option<u8>, Option<u16>) {
    let p = palette(ui.ctx());
    let mut chosen_digits = None;
    let mut chosen_period = None;
    ui.add_enabled_ui(live, |ui| {
        ui.columns(2, |columns| {
            columns[0].vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 7.0;
                ui.label(bold(DIGITS_LABEL, 14.0, p.text_nav));
                let faces: Vec<String> = DIGITS_CHOICES.iter().map(|d| d.to_string()).collect();
                let faces: Vec<&str> = faces.iter().map(String::as_str).collect();
                let selected = DIGITS_CHOICES.iter().position(|d| *d == digits).unwrap_or(0);
                if let Some(at) = controls::segmented(ui, &faces, selected, controls::SEG_EXPIRY) {
                    chosen_digits = Some(DIGITS_CHOICES[at]);
                }
            });
            columns[1].vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 7.0;
                ui.label(bold(PERIOD_LABEL, 14.0, p.text_nav));
                let faces: Vec<String> = PERIOD_CHOICES.iter().map(|s| format!("{s} s")).collect();
                let faces: Vec<&str> = faces.iter().map(String::as_str).collect();
                let selected = PERIOD_CHOICES.iter().position(|s| *s == period).unwrap_or(0);
                if let Some(at) = controls::segmented(ui, &faces, selected, controls::SEG_EXPIRY) {
                    chosen_period = Some(PERIOD_CHOICES[at]);
                }
            });
        });
    });
    (chosen_digits, chosen_period)
}

/// What a good reading looks like under the field. By hand it is the design's
/// "Your code right now" row; read from a code it is the found card and the
/// table of what the link carried.
fn draw_confirmation(ui: &mut egui::Ui, auth: &OtpAuth, state: &mut TotpAdd, now_unix: u64) -> TotpAddAction {
    let mut action = TotpAddAction::None;
    let Some(code) = code_at(auth, now_unix) else {
        return action;
    };
    if state.scanned {
        if found_card(ui, auth, &code, now_unix) {
            action = TotpAddAction::CopyCode;
        }
        ui.vertical_centered(|ui| {
            ui.label(plain(MATCH_PROMPT, 15.0, palette(ui.ctx()).text_icon));
        });
        if let Some(field) = draw_field_table(ui, auth, state) {
            if action == TotpAddAction::None {
                action = TotpAddAction::CopyField(field);
            }
        }
    } else if code_now_row(ui, auth, &code, now_unix) {
        action = TotpAddAction::CopyCode;
    }
    action
}

/// The tip shown when a live code is hovered.
pub const CODE_COPY_HINT: &str = "Click to copy this code";

fn code_font(size: f32) -> Role {
    Role { size, face: theme::BOLD, tracking_em: 0.12 }
}

/// Spec B section 8 `manual`'s preview row: padding 16 20, radius 16, the
/// check, "Your code right now", and the live code in 24/700. A 3 px line along
/// its foot drains with the period (0.15's countdown).
fn code_now_row(ui: &mut egui::Ui, auth: &OtpAuth, code: &str, now_unix: u64) -> bool {
    let p = palette(ui.ctx());
    let left = seconds_left(auth, now_unix);
    let digits = layout(ui, code_font(24.0), &grouped_code(code), p.blue_deep);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0 + digits.size().y.max(24.0)), Sense::hover());
    let response = ui.interact(rect, code_id(), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(16), if response.hovered() { p.blue_wash } else { p.chip_on });
    let mark = Rect::from_center_size(egui::pos2(rect.left() + 20.0 + 10.0, rect.center().y), Vec2::splat(20.0));
    icon::stroke(ui.painter(), mark, glyph::TICK, 2.8, p.done_mark);
    let label = layout(ui, Role { size: 15.0, face: theme::REGULAR, tracking_em: 0.0 }, CODE_NOW, p.text_nav);
    let label_w = label.size().x;
    ui.painter().galley(egui::pos2(mark.right() + 14.0, rect.center().y - label.size().y / 2.0), label, p.text_nav);
    let refresh = layout(ui, Role { size: 13.0, face: theme::REGULAR, tracking_em: 0.0 }, &format!("\u{b7} {}", refresh_line(left)), p.text_label);
    let refresh_x = mark.right() + 14.0 + label_w + 8.0;
    let right_edge = rect.right() - 20.0;
    let digits_x = right_edge - digits.size().x;
    if refresh_x + refresh.size().x + 12.0 < digits_x {
        ui.painter().galley(egui::pos2(refresh_x, rect.center().y - refresh.size().y / 2.0), refresh, p.text_label);
    }
    ui.painter().galley(egui::pos2(digits_x, rect.center().y - digits.size().y / 2.0), digits, p.blue_deep);
    let track = Rect::from_min_max(egui::pos2(rect.left() + 20.0, rect.bottom() - 7.0), egui::pos2(rect.right() - 20.0, rect.bottom() - 4.0));
    ui.painter().rect_filled(track, CornerRadius::same(2), p.focus);
    let filled = Rect::from_min_size(track.min, egui::vec2(track.width() * countdown_fraction(auth, now_unix).clamp(0.0, 1.0), track.height()));
    if filled.width() > 0.0 {
        ui.painter().rect_filled(filled, CornerRadius::same(2), p.blue);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(CODE_COPY_HINT).clicked()
}

/// Spec B section 8 `found`: padding 18 20, radius 18, bg `#f5f7fd`, a 1 px
/// `#dbe4f7` edge; a 48 px success circle, "Found it" 18/800 over the issuer
/// and account, the live code 26/700 and "new code in Ns" at the right.
fn found_card(ui: &mut egui::Ui, auth: &OtpAuth, code: &str, now_unix: u64) -> bool {
    let p = palette(ui.ctx());
    let left = seconds_left(auth, now_unix);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0 * 2.0 + 48.0), Sense::hover());
    let response = ui.interact(rect, code_id(), Sense::click());
    ui.painter().rect(
        rect,
        CornerRadius::same(18),
        if response.hovered() { p.blue_wash } else { p.chip_on },
        Stroke::new(1.0, p.focus),
        StrokeKind::Inside,
    );
    let circle = egui::pos2(rect.left() + 20.0 + 24.0, rect.center().y);
    ui.painter().circle_filled(circle, 24.0, p.done_wash);
    icon::stroke(ui.painter(), Rect::from_center_size(circle, Vec2::splat(24.0)), glyph::TICK, 2.8, p.done_mark);

    let code_galley = layout(ui, code_font(26.0), &grouped_code(code), p.blue_deep);
    let secs = layout(ui, Role { size: 13.0, face: theme::REGULAR, tracking_em: 0.0 }, &format!("new code in {left}s"), p.text_icon);
    let right_w = code_galley.size().x.max(secs.size().x);
    let right = rect.right() - 20.0;
    let block_h = code_galley.size().y + 2.0 + secs.size().y;
    let top = rect.center().y - block_h / 2.0;
    let secs_h = secs.size().y;
    let code_h = code_galley.size().y;
    ui.painter().galley(egui::pos2(right - code_galley.size().x, top), code_galley, p.blue_deep);
    ui.painter().galley(egui::pos2(right - secs.size().x, top + code_h + 2.0), secs, p.text_icon);

    let text_left = circle.x + 24.0 + 14.0;
    let title = layout(ui, Role { size: 18.0, face: theme::EXTRABOLD, tracking_em: 0.0 }, FOUND_TITLE, p.ink);
    let who = match (auth.issuer.as_deref(), auth.account.as_deref()) {
        (Some(issuer), Some(account)) => format!("{issuer} \u{b7} {account}"),
        (Some(only), None) | (None, Some(only)) => only.to_string(),
        (None, None) => CODE_READ_KIND.to_string(),
    };
    let who = layout_wrapped(ui, Role { size: 15.0, face: theme::REGULAR, tracking_em: 0.0 }, &who, p.text_nav, (rect.right() - 20.0 - right_w - 14.0 - text_left).max(40.0));
    let who_h = who.size().y.min(secs_h + code_h);
    let text_h = title.size().y + 3.0 + who_h;
    let text_top = rect.center().y - text_h / 2.0;
    let title_h = title.size().y;
    ui.painter().galley(egui::pos2(text_left, text_top), title, p.ink);
    ui.painter().galley(egui::pos2(text_left, text_top + title_h + 3.0), who, p.text_nav);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(CODE_COPY_HINT).clicked()
}

// ---------------------------------------------------------------------------
// What the link carried
// ---------------------------------------------------------------------------

/// Issuer, account, secret (with its Reveal) and the four parameters, as one
/// quiet card: rows of 13/600 label over 15 value, a hairline between them.
fn draw_field_table(ui: &mut egui::Ui, auth: &OtpAuth, state: &mut TotpAdd) -> Option<TableField> {
    let p = palette(ui.ctx());
    let mut pressed = None;
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.card_ring))
        .corner_radius(CornerRadius::same(16))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let width = ui.available_width();
            ui.set_min_width(width);
            let issuer = auth.issuer.as_deref().unwrap_or("\u{2014}").to_string();
            if table_row(ui, ISSUER_ROW_LABEL, field_to_copy(state, TableField::Issuer).is_some(), |ui| {
                ui.label(bold(&issuer, 15.0, p.ink));
            }, |_| {}) {
                pressed = Some(TableField::Issuer);
            }
            table_seam(ui);
            let account = auth.account.as_deref().unwrap_or("\u{2014}").to_string();
            if table_row(ui, ACCOUNT_ROW_LABEL, field_to_copy(state, TableField::Account).is_some(), |ui| {
                ui.label(plain(&account, 15.0, p.ink));
            }, |_| {}) {
                pressed = Some(TableField::Account);
            }
            table_seam(ui);
            let mut toggle = false;
            let shown = if state.revealed { secret_as_typed(&auth.secret) } else { masked(&auth.secret) };
            if table_row(ui, SECRET_ROW_LABEL, true, |ui| {
                ui.add(egui::Label::new(
                    egui::RichText::new(&shown).font(egui::FontId::new(14.0, egui::FontFamily::Monospace)).color(p.ink),
                ).truncate());
            }, |ui| {
                toggle = row_action(ui, if state.revealed { HIDE_LABEL } else { REVEAL_LABEL }).clicked();
            }) {
                pressed = Some(TableField::Secret);
            }
            if toggle {
                state.revealed = !state.revealed;
                pressed = None;
            }
            table_seam(ui);
            let chips = parameter_chips(auth);
            let _ = table_row(ui, PARAMETERS_ROW_LABEL, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                    for chip in &chips {
                        param_chip(ui, chip);
                    }
                });
            }, |_| {});
        });
    pressed
}

/// One row of the table: padding 12 16, a 96 px label column, the value, and
/// a trailing control. Answers whether a copyable row was pressed.
fn table_row(
    ui: &mut egui::Ui,
    label: &str,
    copyable: bool,
    value: impl FnOnce(&mut egui::Ui),
    trailing: impl FnOnce(&mut egui::Ui),
) -> bool {
    let p = palette(ui.ctx());
    let full = ui.available_width();
    let (row, _) = ui.allocate_exact_size(egui::vec2(full, 44.0), Sense::hover());
    let response = ui.interact(row, table_row_id(label), if copyable { Sense::click() } else { Sense::hover() });
    if copyable && response.hovered() {
        ui.painter().rect_filled(row.shrink(1.0), CornerRadius::same(12), p.secondary_hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let galley = layout(ui, Role { size: 13.0, face: theme::SEMIBOLD, tracking_em: 0.0 }, label, p.text_label);
    ui.painter().galley(egui::pos2(row.left() + 16.0, row.center().y - galley.size().y / 2.0), galley, p.text_label);
    let value_rect = Rect::from_min_max(egui::pos2(row.left() + 16.0 + 96.0, row.top() + 4.0), egui::pos2(row.right() - 16.0, row.bottom() - 4.0));
    let mut value_ui = ui.new_child(egui::UiBuilder::new().max_rect(value_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    value_ui.spacing_mut().item_spacing.x = 8.0;
    // The trailing control (Reveal) sits at the right edge; the value takes the rest.
    value_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), trailing);
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(value_rect.min, egui::pos2(value_rect.right() - 70.0, value_rect.bottom()))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    value(&mut inner);
    response.clicked() && copyable
}

fn table_seam(ui: &mut egui::Ui) {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, p.row_rule);
}

fn row_action(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let p = palette(ui.ctx());
    let galley = layout(ui, Role { size: 14.0, face: theme::BOLD, tracking_em: 0.0 }, text, p.blue_deep);
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(8.0, 8.0), Sense::hover());
    let response = ui.interact(rect, reveal_id(), Sense::click());
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, p.blue_deep);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn param_chip(ui: &mut egui::Ui, text: &str) {
    let p = palette(ui.ctx());
    let galley = ui.fonts_mut(|f| {
        f.layout_no_wrap(text.to_string(), egui::FontId::new(12.0, egui::FontFamily::Monospace), p.text_nav)
    });
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(16.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(8), p.field_bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, p.text_nav);
}

/// Where a frame goes inside the preview rectangle, preserving its aspect
/// ratio and never enlarging past the rectangle.
fn preview_fit(into: Rect, frame: (usize, usize)) -> Rect {
    let (w, h) = (frame.0.max(1) as f32, frame.1.max(1) as f32);
    let scale = (into.width() / w).min(into.height() / h);
    Rect::from_center_size(into.center(), egui::vec2(w * scale, h * scale))
}

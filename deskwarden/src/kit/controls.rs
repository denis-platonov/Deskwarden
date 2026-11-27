//! The v2 controls: the switch, the segmented control, the four buttons and
//! the square icon button.
//!
//! Each paints itself rather than going through `egui::Button`, because the
//! design's shapes (a 50x30 pill switch, a white cell in a grey track, shadows
//! under primary buttons) are not what egui's widgets draw.

use eframe::egui::{self, Color32, CornerRadius, Rect, Response, Sense, Shadow, Stroke, Ui, Vec2};

use crate::kit::motion::SWITCH;
use crate::kit::tokens::{self, palette, Role, R_PILL};
use crate::theme::{BOLD, SEMIBOLD};

fn switch_ease(t: f32) -> f32 {
    SWITCH.at(t)
}

/// The design's switch: a 50x30 pill, blue when on, with a white knob that
/// overshoots a little as it travels.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let p = palette(ui.ctx());
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(50.0, 30.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let travel = ui.ctx().animate_bool_with_time_and_easing(response.id, *on, 0.22, switch_ease);
    let tint = ui.ctx().animate_bool_with_time(response.id.with("track"), *on, 0.2);
    let track = lerp_color(p.switch_off, p.blue, tint);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(R_PILL), track);
    let knob = egui::pos2(rect.left() + 3.0 + 12.0 + 20.0 * travel, rect.center().y);
    let knob_shadow = Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_rgba_unmultiplied(0, 0, 0, 51) };
    painter.add(knob_shadow.as_shape(Rect::from_center_size(knob, Vec2::splat(24.0)), CornerRadius::same(12)));
    painter.circle_filled(knob, 12.0, Color32::WHITE);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), mix(a.a(), b.a()))
}

/// One segmented control's measurements (spec part C section 0.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentStyle {
    /// Padding inside each cell.
    pub pad: Vec2,
    pub font_px: f32,
    pub cell_radius: u8,
    pub track_radius: u8,
    /// Cells share the full width equally (the Send modal's expiry).
    pub full_width: bool,
}

pub const SEG_SETTINGS: SegmentStyle =
    SegmentStyle { pad: Vec2::new(13.0, 8.0), font_px: 14.0, cell_radius: 10, track_radius: 13, full_width: false };
pub const SEG_APPS: SegmentStyle =
    SegmentStyle { pad: Vec2::new(14.0, 8.0), font_px: 14.0, cell_radius: 10, track_radius: 13, full_width: false };
pub const SEG_TABS: SegmentStyle =
    SegmentStyle { pad: Vec2::new(15.0, 9.0), font_px: 14.0, cell_radius: 10, track_radius: 13, full_width: false };
pub const SEG_GENERATOR: SegmentStyle =
    SegmentStyle { pad: Vec2::new(20.0, 10.0), font_px: 15.0, cell_radius: 11, track_radius: 13, full_width: false };
pub const SEG_EXPIRY: SegmentStyle =
    SegmentStyle { pad: Vec2::new(0.0, 10.0), font_px: 15.0, cell_radius: 11, track_radius: 14, full_width: true };

/// The width [`segmented`] takes for `options` (not full-width styles).
pub fn segmented_width(ctx: &egui::Context, options: &[&str], style: SegmentStyle) -> f32 {
    let role = Role { size: style.font_px, face: BOLD, tracking_em: 0.0 };
    let p = palette(ctx);
    let inner: f32 = options
        .iter()
        .map(|label| ctx.fonts_mut(|f| f.layout_job(tokens::job(role, label, p.ink))).size().x + style.pad.x * 2.0)
        .sum();
    inner + 8.0
}

/// A grey track of options with the selected one lifted onto a white cell.
/// Returns the index clicked this frame, if any.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: usize, style: SegmentStyle) -> Option<usize> {
    let p = palette(ui.ctx());
    let role = Role { size: style.font_px, face: BOLD, tracking_em: 0.0 };
    let galleys: Vec<_> = options
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let color = if i == selected { p.ink } else { p.text_icon };
            ui.fonts_mut(|f| f.layout_job(tokens::job(role, label, color)))
        })
        .collect();
    let text_h = galleys.iter().map(|g| g.size().y).fold(0.0, f32::max);
    let cell_h = text_h + style.pad.y * 2.0;
    let natural: Vec<f32> = galleys.iter().map(|g| g.size().x + style.pad.x * 2.0).collect();
    let inner_w = if style.full_width {
        ui.available_width() - 8.0
    } else {
        natural.iter().sum::<f32>()
    };
    // The track's own id is unique per control, so two controls with the
    // same labels in one Ui keep their cells' clicks apart.
    let (track, track_response) = ui.allocate_exact_size(Vec2::new(inner_w + 8.0, cell_h + 8.0), Sense::hover());
    ui.painter().rect_filled(track, CornerRadius::same(style.track_radius), p.hover);
    let mut clicked = None;
    let mut x = track.left() + 4.0;
    for (i, galley) in galleys.into_iter().enumerate() {
        let w = if style.full_width { inner_w / options.len() as f32 } else { natural[i] };
        let cell = Rect::from_min_size(egui::pos2(x, track.top() + 4.0), Vec2::new(w, cell_h));
        x += w;
        let response = ui.interact(cell, track_response.id.with(("segment", i)), Sense::click());
        if response.clicked() && i != selected {
            clicked = Some(i);
        }
        if i == selected {
            let lift = Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_rgba_unmultiplied(32, 30, 29, 36) };
            ui.painter().add(lift.as_shape(cell, CornerRadius::same(style.cell_radius)));
            ui.painter().rect_filled(cell, CornerRadius::same(style.cell_radius), p.card);
        } else {
            response.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        let pos = cell.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, p.ink);
    }
    clicked
}

/// The design's four buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// Brand blue, white type, a blue shadow under it.
    Primary,
    /// White with a grey border.
    Outline,
    /// Blue wash with deep-blue type ("Generate").
    Tinted,
    /// A red-edged outline for what cannot be undone.
    Danger,
}

/// A button's height, side padding, corner radius and type size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonSize {
    pub height: f32,
    pub pad_x: f32,
    pub radius: u8,
    pub font_px: f32,
}

/// Header buttons (Save, Cancel), the Fill row.
pub const LARGE: ButtonSize = ButtonSize { height: 48.0, pad_x: 22.0, radius: 14, font_px: 16.0 };
/// Settings rows' buttons.
pub const ROW: ButtonSize = ButtonSize { height: 42.0, pad_x: 16.0, radius: 12, font_px: 15.0 };
/// Buttons inside fields (Generate).
pub const SMALL: ButtonSize = ButtonSize { height: 40.0, pad_x: 14.0, radius: 11, font_px: 14.0 };

/// A design button with an optional leading glyph (spec part B section 0.6).
/// A button's face and ink.
fn button_look(p: &tokens::Palette, kind: ButtonKind) -> (&'static str, Color32) {
    match kind {
        ButtonKind::Primary => (BOLD, Color32::WHITE),
        ButtonKind::Outline => (SEMIBOLD, p.ink),
        ButtonKind::Tinted => (BOLD, p.blue_deep),
        ButtonKind::Danger => (SEMIBOLD, p.error),
    }
}

/// How wide [`button`] will draw `label`: for rows that must decide, before
/// drawing, whether their buttons fit.
pub fn button_width(ctx: &egui::Context, kind: ButtonKind, size: ButtonSize, label: &str, icon: bool) -> f32 {
    let (face, ink) = button_look(&palette(ctx), kind);
    let role = Role { size: size.font_px, face, tracking_em: 0.0 };
    let text = ctx.fonts_mut(|f| f.layout_job(tokens::job(role, label, ink)).size().x);
    let icon_px = (size.font_px + 2.0).round();
    size.pad_x * 2.0 + if icon { icon_px + 7.0 } else { 0.0 } + text
}

pub fn button(ui: &mut Ui, kind: ButtonKind, size: ButtonSize, label: &str, icon: Option<&str>) -> Response {
    let width = button_width(ui.ctx(), kind, size, label, icon.is_some());
    paint_button(ui, kind, size, width, label, icon)
}

/// The edit form's small row buttons (Copy, Remove field, Add a field...,
/// Add a hidden field..., the sign-in steps' Edit and the like): the kit's
/// outline button at [`SMALL`], so they read as the Replace / Remove beside
/// the 2FA field and not as a 0.x boxed caption.
pub fn row_button(ui: &mut Ui, label: &str) -> Response {
    button(ui, ButtonKind::Outline, SMALL, label, None)
}

/// [`row_button`] for what cannot be undone: the red-edged outline.
pub fn row_button_danger(ui: &mut Ui, label: &str) -> Response {
    button(ui, ButtonKind::Danger, SMALL, label, None)
}

/// How wide [`row_button`] draws `label`, for the rows that reserve room for
/// their buttons before drawing them.
pub fn row_button_width(ui: &Ui, label: &str) -> f32 {
    button_width(ui.ctx(), ButtonKind::Outline, SMALL, label, false)
}

/// A text link: `size` pt semibold in the deep blue, the pointing hand under
/// the cursor ("More options", the Send composer's quiet links).
pub fn link(ui: &mut Ui, text: &str, size: f32) -> Response {
    let p = palette(ui.ctx());
    let role = Role { size, face: SEMIBOLD, tracking_em: 0.0 };
    // Laid here, no-wrap, and handed over finished: a `Label` that lays its own
    // text inside a `horizontal_wrapped` row takes the rest of the line as its
    // box, and two links side by side would both answer a click on either.
    let mut job = tokens::job(role, text, p.blue_deep);
    job.wrap = egui::text::TextWrapping::no_max_width();
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let response = ui.add(egui::Label::new(galley).selectable(false).sense(Sense::click()));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The small caption chips of the key-sequence editor (`<`, `>`, `x`, and the
/// palette's field and key names): the outline button one step under
/// [`SMALL`].
pub const CHIP: ButtonSize = ButtonSize { height: 30.0, pad_x: 10.0, radius: 9, font_px: 13.0 };

/// An outline [`CHIP`] button.
pub fn chip_button(ui: &mut Ui, label: &str) -> Response {
    button(ui, ButtonKind::Outline, CHIP, label, None)
}

/// The height of [`dashed_button`] (spec B 1.4 F's "+ Add field").
pub const DASHED_HEIGHT: f32 = 44.0;

/// How wide [`dashed_button`] draws `label`.
pub fn dashed_button_width(ctx: &egui::Context, label: &str) -> f32 {
    let p = palette(ctx);
    let role = Role { size: 15.0, face: SEMIBOLD, tracking_em: 0.0 };
    let text = ctx.fonts_mut(|f| f.layout_job(tokens::job(role, label, p.text_nav)).size().x);
    text + 32.0
}

/// The "add one more" button (spec B 1.4 F): [`DASHED_HEIGHT`] high at radius
/// 12 with a 1 pt dashed edge, 15/600 type; the edge and the type turn blue
/// under the pointer.
pub fn dashed_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.ctx());
    let role = Role { size: 15.0, face: SEMIBOLD, tracking_em: 0.0 };
    let width = dashed_button_width(ui.ctx(), label);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, DASHED_HEIGHT), Sense::click());
    let hovered = response.hovered();
    let ink = if hovered { p.blue } else { p.text_nav };
    let edge = if hovered { p.blue } else { p.border.lerp_to_gamma(p.text_ghost, 0.5) };
    let painter = ui.painter();
    for shape in egui::Shape::dashed_line(&dash_path(rect.shrink(0.5), 12.0), Stroke::new(1.0, edge), 5.0, 4.0) {
        painter.add(shape);
    }
    let galley = ui.fonts_mut(|f| f.layout_job(tokens::job(role, label, ink)));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, ink);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A rounded rectangle walked as a closed polyline, for the dashes.
fn dash_path(rect: Rect, radius: f32) -> Vec<egui::Pos2> {
    const STEPS: usize = 6;
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let quarter = std::f32::consts::FRAC_PI_2;
    let corners = [
        (egui::pos2(rect.left() + radius, rect.top() + radius), 2.0 * quarter),
        (egui::pos2(rect.right() - radius, rect.top() + radius), 3.0 * quarter),
        (egui::pos2(rect.right() - radius, rect.bottom() - radius), 0.0),
        (egui::pos2(rect.left() + radius, rect.bottom() - radius), quarter),
    ];
    let mut path = Vec::with_capacity(4 * (STEPS + 1) + 1);
    for (centre, from) in corners {
        for step in 0..=STEPS {
            let angle = from + quarter * step as f32 / STEPS as f32;
            path.push(egui::pos2(centre.x + radius * angle.cos(), centre.y + radius * angle.sin()));
        }
    }
    if let Some(&first) = path.first() {
        path.push(first);
    }
    path
}

/// A button as wide as the room it is given, its label centred (the modals'
/// `flex: 1` pairs: Back and Save code).
pub fn button_filling(ui: &mut Ui, kind: ButtonKind, size: ButtonSize, label: &str) -> Response {
    let width = ui.available_width();
    paint_button(ui, kind, size, width, label, None)
}

fn paint_button(ui: &mut Ui, kind: ButtonKind, size: ButtonSize, width: f32, label: &str, icon: Option<&str>) -> Response {
    let p = palette(ui.ctx());
    let (face, ink) = button_look(&p, kind);
    let role = Role { size: size.font_px, face, tracking_em: 0.0 };
    let galley = ui.fonts_mut(|f| f.layout_job(tokens::job(role, label, ink)));
    let icon_px = (size.font_px + 2.0).round();
    let icon_w = if icon.is_some() { icon_px + 7.0 } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, size.height), Sense::click());
    let hovered = response.hovered();
    let radius = CornerRadius::same(size.radius);
    let painter = ui.painter();
    match kind {
        ButtonKind::Primary => {
            let shadow = if hovered {
                Shadow { offset: [0, 10], blur: 22, spread: 0, color: Color32::from_rgba_unmultiplied(27, 63, 160, 87) }
            } else {
                Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_rgba_unmultiplied(27, 63, 160, 71) }
            };
            painter.add(shadow.as_shape(rect, radius));
            painter.rect_filled(rect, radius, if hovered { p.blue_deep } else { p.blue });
        }
        ButtonKind::Outline => {
            let fill = if hovered { p.secondary_hover } else { p.card };
            painter.rect(rect, radius, fill, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        }
        ButtonKind::Tinted => {
            painter.rect_filled(rect, radius, if hovered { p.focus } else { p.blue_wash });
        }
        ButtonKind::Danger => {
            let fill = if hovered { p.danger_wash } else { p.card };
            painter.rect(rect, radius, fill, Stroke::new(1.0, p.danger_edge), egui::StrokeKind::Inside);
        }
    }
    // Centred in whatever width it was given; a natural-width button is
    // already exactly its content and padding, so this is the same place.
    let content = icon_w + galley.size().x;
    let mut x = rect.left() + (rect.width() - content) / 2.0;
    if let Some(d) = icon {
        let glyph = Rect::from_min_size(egui::pos2(x, rect.center().y - icon_px / 2.0), Vec2::splat(icon_px));
        crate::kit::icon::stroke(painter, glyph, d, 2.2, ink);
        x += icon_w;
    }
    painter.galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, ink);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A square glyph button: 42 (radius 12) or 38 (radius 11), grey on hover.
pub fn icon_button(ui: &mut Ui, size: f32, glyph: &str, color: Color32) -> Response {
    let p = palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let radius = CornerRadius::same(if size >= 42.0 { 12 } else { 11 });
    if response.hovered() {
        ui.painter().rect_filled(rect, radius, p.hover);
    }
    crate::kit::icon::stroke(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(19.0)), glyph, 2.0, color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{click_at, frame};
    use crate::kit::tokens::Palette;
    use eframe::egui::{self, Color32, Rect, Vec2};

    const SCREEN: Vec2 = Vec2::new(600.0, 400.0);
    const P: Palette = Palette::LIGHT;

    #[test]
    fn a_switch_is_fifty_by_thirty_and_blue_when_on() {
        let ctx = crate::kit::testing::ctx();
        let mut rect = Rect::NOTHING;
        let mut on = true;
        let mut painted = None;
        for time in [0.0, 0.1, 0.3, 0.5] {
            painted = Some(frame(&ctx, time, SCREEN, vec![], |ui| rect = switch(ui, &mut on).rect));
        }
        let painted = painted.unwrap();
        assert_eq!(rect.size(), Vec2::new(50.0, 30.0));
        assert!(painted.rects.iter().any(|r| r.fill == P.blue && r.rect == rect));
        let knob = painted.circles.iter().find(|c| c.fill == Color32::WHITE).expect("a knob");
        assert!((knob.center.x - (rect.left() + 3.0 + 12.0 + 20.0)).abs() < 0.5, "{knob:?} in {rect:?}");
        assert!((knob.radius - 12.0).abs() < 0.01);
    }

    #[test]
    fn an_off_switch_is_grey_with_its_knob_at_the_left() {
        let ctx = crate::kit::testing::ctx();
        let mut rect = Rect::NOTHING;
        let mut on = false;
        let painted = frame(&ctx, 0.0, SCREEN, vec![], |ui| rect = switch(ui, &mut on).rect);
        assert!(painted.rects.iter().any(|r| r.fill == P.switch_off && r.rect == rect));
        let knob = painted.circles.iter().find(|c| c.fill == Color32::WHITE).expect("a knob");
        assert!((knob.center.x - (rect.left() + 15.0)).abs() < 0.5);
    }

    #[test]
    fn clicking_a_switch_flips_it() {
        let ctx = crate::kit::testing::ctx();
        let mut on = false;
        let mut rect = Rect::NOTHING;
        frame(&ctx, 0.0, SCREEN, vec![], |ui| rect = switch(ui, &mut on).rect);
        let mut changed = false;
        frame(&ctx, 0.1, SCREEN, click_at(rect.center()), |ui| changed = switch(ui, &mut on).changed());
        assert!(on);
        assert!(changed);
    }

    const THEMES: [&str; 3] = ["Light", "Dark", "System"];

    #[test]
    fn the_selected_segment_is_white_and_the_rest_are_not() {
        let ctx = crate::kit::testing::ctx();
        let painted = frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            segmented(ui, &THEMES, 2, SEG_SETTINGS);
        });
        let white: Vec<_> = painted.rects.iter().filter(|r| r.fill == P.card).collect();
        assert_eq!(white.len(), 1, "exactly one selected cell");
        assert_eq!(painted.text("System").unwrap().1, P.ink);
        assert_eq!(painted.text("Light").unwrap().1, P.text_icon);
        assert_eq!(painted.text("Dark").unwrap().1, P.text_icon);
        assert!(painted.rects.iter().any(|r| r.fill == P.hover), "the track");
    }

    /// Two settings rows can both offer "Off / On" in one card: a click on
    /// the second must reach the second, not the first.
    #[test]
    fn two_segmented_controls_with_the_same_labels_keep_their_own_clicks() {
        let ctx = crate::kit::testing::ctx();
        const OFF_ON: [&str; 2] = ["Off", "On"];
        let painted = frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            segmented(ui, &OFF_ON, 0, SEG_SETTINGS);
            segmented(ui, &OFF_ON, 0, SEG_SETTINGS);
        });
        let second_on = painted.texts.iter().filter(|(t, _, _)| t == "On").nth(1).unwrap().2.center();
        let (mut first, mut second) = (None, None);
        frame(&ctx, 0.1, SCREEN, click_at(second_on), |ui| {
            first = segmented(ui, &OFF_ON, 0, SEG_SETTINGS);
            second = segmented(ui, &OFF_ON, 0, SEG_SETTINGS);
        });
        assert_eq!((first, second), (None, Some(1)));
    }

    #[test]
    fn clicking_a_segment_returns_its_index() {
        let ctx = crate::kit::testing::ctx();
        let painted = frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            segmented(ui, &THEMES, 0, SEG_SETTINGS);
        });
        let dark = painted.text("Dark").unwrap().2.center();
        let mut picked = None;
        frame(&ctx, 0.1, SCREEN, click_at(dark), |ui| picked = segmented(ui, &THEMES, 0, SEG_SETTINGS));
        assert_eq!(picked, Some(1));
    }

    #[test]
    fn button_sizes_and_colours_match_the_design() {
        let ctx = crate::kit::testing::ctx();
        let (mut large, mut row) = (Rect::NOTHING, Rect::NOTHING);
        let painted = frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            large = button(ui, ButtonKind::Primary, LARGE, "Save", None).rect;
            row = button(ui, ButtonKind::Danger, ROW, "Delete", None).rect;
        });
        assert_eq!(large.height(), 48.0);
        assert_eq!(row.height(), 42.0);
        let primary = painted.rects.iter().find(|r| r.rect == large).expect("primary fill");
        assert_eq!(primary.fill, P.blue);
        assert_eq!(primary.corner_radius, egui::CornerRadius::same(14));
        let danger = painted.rects.iter().find(|r| r.rect == row).expect("danger box");
        assert_eq!(danger.corner_radius, egui::CornerRadius::same(12));
        assert_eq!(danger.stroke.color, P.danger_edge);
        assert_eq!(painted.text("Delete").unwrap().1, P.error);
        assert_eq!(painted.text("Save").unwrap().1, Color32::WHITE);
    }

    #[test]
    fn outline_and_tinted_buttons_use_their_colours() {
        let ctx = crate::kit::testing::ctx();
        let (mut outline, mut tinted) = (Rect::NOTHING, Rect::NOTHING);
        let painted = frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            outline = button(ui, ButtonKind::Outline, LARGE, "Cancel", None).rect;
            tinted = button(ui, ButtonKind::Tinted, SMALL, "Generate", Some(crate::kit::icon::glyph::GEN)).rect;
        });
        let o = painted.rects.iter().find(|r| r.rect == outline).unwrap();
        assert_eq!(o.stroke.color, P.border);
        assert_eq!(painted.text("Cancel").unwrap().1, P.ink);
        let t = painted.rects.iter().find(|r| r.rect == tinted).unwrap();
        assert_eq!(t.fill, P.blue_wash);
        assert_eq!(tinted.height(), 40.0);
        assert_eq!(painted.text("Generate").unwrap().1, P.blue_deep);
    }

    #[test]
    fn a_buttons_width_is_known_before_it_is_drawn() {
        let ctx = crate::kit::testing::ctx();
        for (kind, label, icon) in [
            (ButtonKind::Primary, "Fill", Some(crate::kit::icon::glyph::FILL)),
            (ButtonKind::Outline, "Share", Some(crate::kit::icon::glyph::SHARED)),
            (ButtonKind::Danger, "Delete forever", None),
        ] {
            let mut drawn = Rect::NOTHING;
            frame(&ctx, 0.0, SCREEN, vec![], |ui| drawn = button(ui, kind, LARGE, label, icon).rect);
            assert_eq!(button_width(&ctx, kind, LARGE, label, icon.is_some()), drawn.width(), "{label}");
        }
    }

    #[test]
    fn an_icon_button_is_square_and_rounds_by_its_size() {
        let ctx = crate::kit::testing::ctx();
        let (mut big, mut small) = (Rect::NOTHING, Rect::NOTHING);
        frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            big = icon_button(ui, 42.0, crate::kit::icon::glyph::LOCK, P.text_muted).rect;
            small = icon_button(ui, 38.0, crate::kit::icon::glyph::LOCK, P.text_muted).rect;
        });
        assert_eq!(big.size(), Vec2::splat(42.0));
        assert_eq!(small.size(), Vec2::splat(38.0));
    }

    #[test]
    fn a_dashed_button_is_forty_four_high_and_turns_blue_under_the_pointer() {
        let ctx = crate::kit::testing::ctx();
        let mut drawn = Rect::NOTHING;
        let idle = frame(&ctx, 0.0, SCREEN, vec![], |ui| drawn = dashed_button(ui, "+ Add field").rect);
        assert_eq!(drawn.height(), DASHED_HEIGHT);
        assert_eq!(drawn.width(), dashed_button_width(&ctx, "+ Add field"));
        assert_eq!(idle.text("+ Add field").unwrap().1, P.text_nav);
        frame(&ctx, 0.1, SCREEN, crate::kit::testing::hover_at(drawn.center()), |ui| {
            dashed_button(ui, "+ Add field");
        });
        let hover = frame(&ctx, 0.2, SCREEN, vec![], |ui| {
            dashed_button(ui, "+ Add field");
        })
        .text("+ Add field")
        .unwrap()
        .1;
        assert_eq!(hover, P.blue);
    }

    #[test]
    fn a_chip_button_is_an_outline_button_thirty_high_and_answers_a_click() {
        let ctx = crate::kit::testing::ctx();
        let mut drawn = Rect::NOTHING;
        frame(&ctx, 0.0, SCREEN, vec![], |ui| drawn = chip_button(ui, "<").rect);
        assert_eq!(drawn.height(), CHIP.height);
        let mut clicked = false;
        frame(&ctx, 0.1, SCREEN, click_at(drawn.center()), |ui| clicked = chip_button(ui, "<").clicked());
        assert!(clicked);
    }

    #[test]
    fn a_link_is_deep_blue_text_that_answers_a_click() {
        let ctx = crate::kit::testing::ctx();
        let mut at = Rect::NOTHING;
        let p = frame(&ctx, 0.0, SCREEN, vec![], |ui| at = link(ui, "More options", 14.0).rect);
        assert_eq!(p.text("More options").unwrap().1, P.blue_deep);
        let mut clicked = false;
        frame(&ctx, 0.1, SCREEN, click_at(at.center()), |ui| clicked = link(ui, "More options", 14.0).clicked());
        assert!(clicked);
    }
}

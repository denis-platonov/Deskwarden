//! The v2 versions of the read pane's, the sequence builder's and the sharing
//! screens' last v1 helpers: the reveal eye, the link galley, the scroll
//! gutter, the secret band, and the share and state pills.
//!
//! Every colour is read from [`palette`], so each follows the dark theme.
//! Sizes, geometry and behaviour are the v1 helpers' own.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Frame, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use super::tokens::{palette, Palette};
use crate::theme::{self, PillMark, PillTone, SharePillKind};

/// The square [`eye_toggle`] allocates for itself. A masked row budgets its
/// controls against this.
pub const EYE_TOGGLE_SIZE: f32 = 28.0;
/// The scroll bar's width when it floats in the gutter.
pub const SCROLLBAR_WIDTH: f32 = 6.0;

const EYE_HALF_W: f32 = 8.5;
const EYE_HALF_H: f32 = 6.4;
const EYE_LID_SEGMENTS: usize = 12;
const EYE_LID_FULLNESS: f32 = 0.72;
const EYE_PUPIL_RADIUS: f32 = 2.9;

fn eye_outline(center: Pos2, half_w: f32, half_h: f32) -> Vec<Pos2> {
    let lid = |i: usize, sign: f32| {
        let sweep = std::f32::consts::PI * i as f32 / EYE_LID_SEGMENTS as f32;
        let t = (sweep - std::f32::consts::FRAC_PI_2).sin();
        let height = (1.0 - t * t).max(0.0).powf(EYE_LID_FULLNESS);
        center + Vec2::new(t * half_w, sign * half_h * height)
    };
    let mut points: Vec<Pos2> = (0..=EYE_LID_SEGMENTS).map(|i| lid(i, -1.0)).collect();
    points.extend((1..EYE_LID_SEGMENTS).rev().map(|i| lid(i, 1.0)));
    points
}

/// A masked row's reveal control: an open eye while the value is hidden,
/// struck through once it is showing. Faint, and ink under the pointer.
pub fn eye_toggle(ui: &mut Ui, revealed: bool) -> Response {
    let p = palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(EYE_TOGGLE_SIZE), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() { p.ink } else { p.text_faint };
    let center = rect.center();
    let painter = ui.painter();
    painter.add(egui::Shape::closed_line(
        eye_outline(center, EYE_HALF_W, EYE_HALF_H),
        Stroke::new(theme::ICON_STROKE, color),
    ));
    painter.circle_filled(center, EYE_PUPIL_RADIUS, color);
    if revealed {
        let arm = Vec2::new(EYE_HALF_W - 0.5, EYE_HALF_H + 2.5);
        painter.line_segment([center - arm, center + arm], Stroke::new(1.5, color));
    }
    response
}

/// A link from text the caller has already laid out (in `palette.blue`): the
/// pointing hand and the un-selectable click sense.
pub fn link_galley(ui: &mut Ui, galley: std::sync::Arc<egui::Galley>) -> Response {
    let response = ui.add(egui::Label::new(galley).selectable(false).sense(Sense::click()));
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Reserves `gutter` points at the right of a scroll area for a floating bar
/// of [`SCROLLBAR_WIDTH`], flush to the pane's edge.
pub fn scrollbar_in_gutter(ui: &mut Ui, gutter: f32) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating_allocated_width = gutter;
    scroll.bar_width = SCROLLBAR_WIDTH;
    scroll.floating_width = SCROLLBAR_WIDTH;
    scroll.bar_outer_margin = 0.0;
}

/// Paints nothing for a floating bar while leaving [`scrollbar_in_gutter`]'s
/// measurements alone: for the body that fits.
pub fn hide_scrollbar(ui: &mut Ui) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.dormant_background_opacity = 0.0;
    scroll.active_background_opacity = 0.0;
    scroll.interact_background_opacity = 0.0;
    scroll.dormant_handle_opacity = 0.0;
    scroll.active_handle_opacity = 0.0;
    scroll.interact_handle_opacity = 0.0;
}

/// The danger wash with its edge: the ground of everything that carries a
/// secret (a masked step, the typed-in-the-clear band).
pub fn secret_band(p: &Palette) -> Frame {
    Frame::new().fill(p.danger_wash).stroke(Stroke::new(1.0, p.danger_edge))
}

const PILL_GAP: f32 = 6.0;
const PILL_DOT: f32 = 5.0;
const PILL_CHECK: f32 = 10.0;

fn pill_font() -> FontId {
    FontId::new(theme::PILL_TEXT_PX, FontFamily::Name(theme::BOLD.into()))
}

/// Width a [`state_pill`] will occupy, without drawing it.
pub fn state_pill_width(painter: &egui::Painter, tone: PillTone, text: &str) -> f32 {
    let galley = painter.layout_no_wrap(text.to_string(), pill_font(), tone.ink);
    let lead = match tone.mark {
        PillMark::None => 0.0,
        PillMark::Dot(_) => PILL_DOT + PILL_GAP,
        PillMark::Check(_) => PILL_CHECK + PILL_GAP,
    };
    lead + galley.size().x + theme::PILL_PAD_X * 2.0 + 2.0
}

/// The Sends' 20-point stadium pill, its left edge at `left_center`.
pub fn state_pill(painter: &egui::Painter, left_center: Pos2, tone: PillTone, text: &str) -> Rect {
    let font = pill_font();
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), tone.ink);
    let width = state_pill_width(painter, tone, text);
    let rect = Rect::from_min_size(
        Pos2::new(left_center.x, left_center.y - theme::PILL_HEIGHT / 2.0),
        Vec2::new(width, theme::PILL_HEIGHT),
    );
    let rounding = CornerRadius::same((theme::PILL_HEIGHT / 2.0) as u8);
    painter.rect_filled(rect, rounding, tone.fill);
    painter.rect_stroke(rect, rounding, Stroke::new(1.0, tone.edge), StrokeKind::Inside);
    let mut x = rect.min.x + 1.0 + theme::PILL_PAD_X;
    match tone.mark {
        PillMark::None => {}
        PillMark::Dot(colour) => {
            painter.circle_filled(Pos2::new(x + PILL_DOT / 2.0, rect.center().y), PILL_DOT / 2.0, colour);
            x += PILL_DOT + PILL_GAP;
        }
        PillMark::Check(colour) => {
            let unit = PILL_CHECK / 24.0;
            let at = |ux: f32, uy: f32| Pos2::new(x + ux * unit, rect.center().y - PILL_CHECK / 2.0 + uy * unit);
            painter.add(egui::Shape::line(
                vec![at(20.0, 6.0), at(9.0, 17.0), at(4.0, 12.0)],
                Stroke::new(2.4 * unit, colour),
            ));
            x += PILL_CHECK + PILL_GAP;
        }
    }
    painter.galley(theme::centred_galley_top(painter.ctx(), rect, &galley, &font, x), galley, tone.ink);
    rect
}

const SHARE_PILL_PAD_X: f32 = 7.0;
const SHARE_PILL_MARK: f32 = 10.0;
const SHARE_PILL_GAP: f32 = 5.0;
const SHARE_PILL_RADIUS: u8 = 5;

/// A share pill's tone and the person mark's ink, from the palette: Shared is
/// filled blue, the waits are the blue wash, a changed key the danger wash,
/// and a person no membership reaches the quiet grey.
pub fn share_tone(p: &Palette, kind: SharePillKind) -> (PillTone, Color32) {
    let tone = |fill, edge, ink| PillTone { fill, edge, ink, mark: PillMark::None };
    match kind {
        SharePillKind::Shared => (tone(p.blue, p.blue, p.card), p.card),
        SharePillKind::Invited => (tone(p.blue_wash, p.blue_edge, p.blue_deep), p.blue),
        SharePillKind::Gone => (tone(p.field_bg, p.border, p.text_faint), p.text_faint),
        SharePillKind::KeyChanged => (tone(p.danger_wash, p.danger_edge, p.danger_ink), p.danger_ink),
    }
}

/// A share pill's width, for right-aligning it.
pub fn share_pill_width(painter: &egui::Painter, kind: SharePillKind) -> f32 {
    let galley = painter.layout_no_wrap(kind.label().to_string(), pill_font(), Color32::BLACK);
    SHARE_PILL_PAD_X * 2.0 + SHARE_PILL_MARK + SHARE_PILL_GAP + galley.size().x + 2.0
}

/// A share pill: 20 tall, 5-point corners and a person mark.
pub fn share_pill(painter: &egui::Painter, left_center: Pos2, kind: SharePillKind) -> Rect {
    let (tone, mark_ink) = share_tone(&palette(painter.ctx()), kind);
    let width = share_pill_width(painter, kind);
    let rect = Rect::from_min_size(
        Pos2::new(left_center.x, left_center.y - theme::PILL_HEIGHT / 2.0),
        Vec2::new(width, theme::PILL_HEIGHT),
    );
    let rounding = CornerRadius::same(SHARE_PILL_RADIUS);
    painter.rect_filled(rect, rounding, tone.fill);
    painter.rect_stroke(rect, rounding, Stroke::new(1.0, tone.edge), StrokeKind::Inside);
    let mark = Rect::from_min_size(
        Pos2::new(rect.min.x + 1.0 + SHARE_PILL_PAD_X, rect.center().y - SHARE_PILL_MARK / 2.0),
        Vec2::splat(SHARE_PILL_MARK),
    );
    theme::paint_person_mark(painter, mark, mark_ink);
    let font = pill_font();
    let galley = painter.layout_no_wrap(kind.label().to_string(), font.clone(), tone.ink);
    let x = mark.right() + SHARE_PILL_GAP;
    painter.galley(theme::centred_galley_top(painter.ctx(), rect, &galley, &font, x), galley, tone.ink);
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_eye_keeps_the_probe_s_vertex_count() {
        assert_eq!(eye_outline(Pos2::ZERO, 1.0, 1.0).len(), EYE_LID_SEGMENTS * 2);
    }

    #[test]
    fn the_share_tones_follow_the_palette() {
        for p in [Palette::LIGHT, Palette::DARK] {
            let (shared, _) = share_tone(&p, SharePillKind::Shared);
            assert_eq!(shared.fill, p.blue);
            let (key, _) = share_tone(&p, SharePillKind::KeyChanged);
            assert_eq!(key.fill, p.danger_wash);
        }
    }
}

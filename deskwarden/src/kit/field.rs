//! The design's field rows (spec part A section 3.3): a label over a value,
//! with an eye for secrets and a copy button, alone or in a pair.

use crate::kit::icon::{self, glyph};
use crate::kit::tokens::{self, palette, Palette};
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, CursorIcon, Rect, Sense, Ui, Vec2};

/// What a field row reported this frame.
#[derive(Clone, Copy, Debug)]
pub struct FieldResponse {
    /// The copy button was clicked.
    pub copy: bool,
    /// The eye was clicked: the caller flips the secret's reveal.
    pub reveal: bool,
    /// Where the eye button sits, when the field is a secret.
    pub eye_rect: Option<Rect>,
    /// Where the copy button sits.
    pub copy_rect: Rect,
}

/// One half of a [`pair`].
#[derive(Clone, Copy, Debug)]
pub struct Cell<'a> {
    pub label: &'a str,
    pub value: &'a str,
    /// The value's letter-spacing in em (spec 1.2: secrets track wider).
    pub tracking_em: f32,
    /// `Some(revealed)` for a secret, which gets the eye.
    pub secret: Option<bool>,
}

/// Padding (top, right, bottom, left), button side and button radius.
struct Shape {
    pad: [f32; 4],
    button: f32,
    radius: u8,
}

const SINGLE: Shape = Shape { pad: [16.0, 20.0, 16.0, 20.0], button: 42.0, radius: 12 };
const PAIRED: Shape = Shape { pad: [16.0, 12.0, 16.0, 20.0], button: 38.0, radius: 11 };
/// Between the text column and the buttons.
const GAP: f32 = 14.0;
/// Between the label and the value.
const LABEL_GAP: f32 = 4.0;

fn row_height(ctx: &egui::Context, shape: &Shape) -> f32 {
    let text = tokens::font_height(ctx, tokens::LABEL) + LABEL_GAP + tokens::font_height(ctx, tokens::FIELD_VALUE);
    shape.pad[0] + text.max(shape.button) + shape.pad[2]
}

/// One labelled value with copy (and, for a secret, the eye), full width.
pub fn single(ui: &mut Ui, label: &str, value: &str, tracking_em: f32, secret: Option<bool>) -> FieldResponse {
    let height = row_height(ui.ctx(), &SINGLE);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let cell = Cell { label, value, tracking_em, secret };
    paint_cell(ui, rect, &cell, &SINGLE, ui.id().with(("field", label)))
}

/// Two labelled values side by side in equal columns, a rule between them.
pub fn pair(ui: &mut Ui, a: Cell<'_>, b: Cell<'_>) -> (FieldResponse, FieldResponse) {
    let height = row_height(ui.ctx(), &PAIRED);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let mid = rect.center().x;
    let left = Rect::from_min_max(rect.min, pos2(mid, rect.bottom()));
    let right = Rect::from_min_max(pos2(mid, rect.top()), rect.max);
    let p = palette(ui.ctx());
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(mid - 0.5, rect.top()), pos2(mid + 0.5, rect.bottom())),
        0.0,
        p.row_rule,
    );
    let id = ui.id().with(("pair", a.label, b.label));
    (paint_cell(ui, left, &a, &PAIRED, id.with(0)), paint_cell(ui, right, &b, &PAIRED, id.with(1)))
}

fn paint_cell(ui: &mut Ui, rect: Rect, cell: &Cell<'_>, shape: &Shape, id: egui::Id) -> FieldResponse {
    let p = palette(ui.ctx());
    let [top, right, bottom, left] = shape.pad;
    let inner = Rect::from_min_max(pos2(rect.left() + left, rect.top() + top), pos2(rect.right() - right, rect.bottom() - bottom));
    let size = Vec2::splat(shape.button);

    let copy_rect = Rect::from_min_size(pos2(inner.right() - shape.button, inner.center().y - shape.button / 2.0), size);
    let copy = button(ui, copy_rect, id.with("copy"), glyph::COPY, 19.0, shape.radius, &p, true);
    let (eye_rect, reveal) = match cell.secret {
        Some(revealed) => {
            let eye_rect = copy_rect.translate(vec2(-shape.button, 0.0));
            let d = if revealed { glyph::EYE_OFF } else { glyph::EYE };
            (Some(eye_rect), button(ui, eye_rect, id.with("eye"), d, 20.0, shape.radius, &p, false))
        }
        None => (None, false),
    };

    let text_right = eye_rect.unwrap_or(copy_rect).left() - GAP;
    let width = (text_right - inner.left()).max(0.0);
    let label_h = tokens::font_height(ui.ctx(), tokens::LABEL);
    let text_h = label_h + LABEL_GAP + tokens::font_height(ui.ctx(), tokens::FIELD_VALUE);
    let y = inner.center().y - text_h / 2.0;

    let painter = ui.painter();
    let mut label_job = tokens::job(tokens::LABEL, cell.label, p.text_label);
    label_job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    painter.galley(pos2(inner.left(), y), painter.layout_job(label_job), p.text_label);

    let mut value_job = tokens::job(tokens::Role { tracking_em: cell.tracking_em, ..tokens::FIELD_VALUE }, cell.value, p.ink);
    value_job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    painter.galley(pos2(inner.left(), y + label_h + LABEL_GAP), painter.layout_job(value_job), p.ink);

    FieldResponse { copy, reveal, eye_rect, copy_rect }
}

/// A square icon button: `hover` behind it under the pointer, or for copy
/// the blue wash with the glyph in blue.
#[allow(clippy::too_many_arguments)]
fn button(ui: &mut Ui, rect: Rect, id: egui::Id, d: &str, glyph_size: f32, radius: u8, p: &Palette, copy: bool) -> bool {
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let mut ink: Color32 = p.text_icon;
    if response.hovered() {
        let bg = if copy { p.blue_wash } else { p.hover };
        if copy {
            ink = p.blue;
        }
        ui.painter().rect_filled(rect, CornerRadius::same(radius), bg);
    }
    icon::stroke(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(glyph_size)), d, 2.0, ink);
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at};
    use crate::kit::tokens::Palette;
    use eframe::egui::{self, vec2};

    const SCREEN: egui::Vec2 = egui::vec2(600.0, 400.0);
    const P: Palette = Palette::LIGHT;

    #[test]
    fn a_single_field_paints_label_value_and_its_two_buttons() {
        let ctx = testing::ctx();
        let mut got = None;
        let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            got = Some(single(ui, "Password", "hunter2", 0.02, Some(true)));
        });
        let r = got.unwrap();
        assert_eq!(p.text("Password").unwrap().1, P.text_label);
        assert_eq!(p.text("hunter2").unwrap().1, P.ink);
        let (eye, copy) = (r.eye_rect.expect("a secret has an eye"), r.copy_rect);
        assert_eq!(eye.size(), vec2(42.0, 42.0));
        assert_eq!(copy.size(), vec2(42.0, 42.0));
        assert!(eye.right() <= copy.left(), "the eye comes before copy");
    }

    #[test]
    fn a_plain_field_has_copy_and_no_eye() {
        let ctx = testing::ctx();
        let mut got = None;
        testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| got = Some(single(ui, "Username", "a.novak", 0.0, None)));
        assert!(got.unwrap().eye_rect.is_none());
    }

    #[test]
    fn clicking_copy_and_eye_reports_them() {
        let ctx = testing::ctx();
        let mut got = None;
        testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| got = Some(single(ui, "Password", "••••", 0.12, Some(false))));
        let r = got.unwrap();
        let (eye, copy) = (r.eye_rect, r.copy_rect);
        let mut clicked = None;
        testing::frame(&ctx, 0.1, SCREEN, click_at(copy.center()), |ui| {
            clicked = Some(single(ui, "Password", "••••", 0.12, Some(false)))
        });
        let c = clicked.unwrap();
        assert!(c.copy && !c.reveal);
        let mut revealed = None;
        testing::frame(&ctx, 0.2, SCREEN, click_at(eye.unwrap().center()), |ui| {
            revealed = Some(single(ui, "Password", "••••", 0.12, Some(false)))
        });
        let r = revealed.unwrap();
        assert!(r.reveal && !r.copy);
    }

    #[test]
    fn a_long_value_ellipsises_before_the_buttons() {
        let ctx = testing::ctx();
        let long = "a.very.long.username.that.does.not.fit@an-even-longer-domain.example";
        let mut got = None;
        let p = testing::frame(&ctx, 0.0, vec2(320.0, 300.0), vec![], |ui| {
            got = Some(single(ui, "Username", long, 0.0, None))
        });
        let (drawn, _, rect) = p.texts.iter().find(|(t, _, _)| t.starts_with("a.very")).unwrap();
        assert!(drawn.ends_with('…'), "{drawn}");
        assert!(rect.right() <= got.unwrap().copy_rect.left());
    }

    #[test]
    fn a_pair_splits_the_row_in_two_with_a_rule_between() {
        let ctx = testing::ctx();
        let mut got = None;
        let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            got = Some(pair(
                ui,
                Cell { label: "Expires", value: "08 / 29", tracking_em: 0.0, secret: None },
                Cell { label: "Security code", value: "•••", tracking_em: 0.12, secret: Some(false) },
            ));
        });
        let (a, b) = got.unwrap();
        let (_, _, expires) = p.text("Expires").unwrap();
        let (_, _, code) = p.text("Security code").unwrap();
        assert!(expires.center().x < SCREEN.x / 2.0 && code.center().x > SCREEN.x / 2.0 - 10.0);
        assert_eq!(a.copy_rect.size(), vec2(38.0, 38.0));
        assert!(b.eye_rect.is_some() && a.eye_rect.is_none());
        assert!(p.rects.iter().any(|r| r.fill == P.row_rule && r.rect.width() <= 1.0), "no rule between the cells");
    }
}

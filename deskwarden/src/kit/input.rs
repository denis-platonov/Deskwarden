//! The editor's inputs (spec B 0.6): a text field 50 high at radius 14 with
//! a 1pt border that turns blue with the keyboard, the password field with
//! its eye and Generate, the strength meter, and the card number's grouping.

use crate::kit::{controls, icon, tokens};
use crate::generator_mode::GeneratorLevel;
use crate::password_strength::Strength;
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Rect, Response, Sense, Stroke, Ui, Vec2};

/// Paints a field's box: white, radius 14, a 1pt border that is blue while
/// the field has the keyboard (spec B 0.6: no focus ring in v2).
fn field_box(ui: &Ui, rect: Rect, focused: bool) {
    let p = tokens::palette(ui.ctx());
    let edge = if focused { p.blue } else { p.border };
    ui.painter().rect(rect, CornerRadius::same(14), p.card, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
}

/// A text field 50 high, `width` wide: padding 0 16, 16pt type (mono for
/// numbers), `placeholder` while empty.
pub fn text(ui: &mut Ui, id: egui::Id, value: &mut String, placeholder: &str, mono: bool, width: f32) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 50.0), Sense::hover());
    field_box(ui, rect, ui.memory(|m| m.has_focus(id)));
    let font = if mono { FontId::new(16.0, FontFamily::Monospace) } else { FontId::proportional(16.0) };
    let inner = rect.shrink2(Vec2::new(16.0, 0.0));
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .id(id)
            .frame(egui::Frame::NONE)
            .font(font)
            .hint_text(placeholder)
            .desired_width(inner.width())
            .vertical_align(egui::Align::Center),
    );
    // `put` leaves the cursor at the text's edge, short of the field's.
    ui.advance_cursor_after_rect(rect);
    response
}

/// What the password field did this frame, and where its parts are.
pub struct PasswordResponse {
    pub response: Response,
    pub field: Rect,
    pub eye: Rect,
    pub generate: Rect,
    pub generate_clicked: bool,
}

/// The password field (spec B 1.4 A): 52 high, mono 17, hidden unless
/// `revealed`, with the eye (40, radius 11) and the tinted Generate.
pub fn password(ui: &mut Ui, id: egui::Id, value: &mut String, revealed: &mut bool, width: f32) -> PasswordResponse {
    let p = tokens::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::hover());
    field_box(ui, rect, ui.memory(|m| m.has_focus(id)));
    let generate_w = controls::button_width(ui.ctx(), controls::ButtonKind::Tinted, controls::SMALL, generate_label(), true);
    let generate = Rect::from_min_size(egui::pos2(rect.right() - 6.0 - generate_w, rect.center().y - 20.0), Vec2::new(generate_w, 40.0));
    let eye = Rect::from_min_size(egui::pos2(generate.left() - 6.0 - 40.0, rect.center().y - 20.0), Vec2::splat(40.0));
    let mut generate_ui = ui.new_child(egui::UiBuilder::new().max_rect(generate));
    let generate_clicked = controls::button(&mut generate_ui, controls::ButtonKind::Tinted, controls::SMALL, generate_label(), Some(icon::glyph::GEN)).clicked();
    let eye_response = ui.interact(eye, id.with("eye"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if eye_response.hovered() {
        ui.painter().rect_filled(eye, CornerRadius::same(11), p.hover);
    }
    let d = if *revealed { icon::glyph::EYE_OFF } else { icon::glyph::EYE };
    icon::stroke(ui.painter(), Rect::from_center_size(eye.center(), Vec2::splat(19.0)), d, 2.0, p.text_icon);
    crate::kit::tips::hint(ui, &eye_response, crate::i18n::t("input.show_hide"), &[]);
    if eye_response.clicked() {
        *revealed = !*revealed;
    }
    let inner = Rect::from_min_max(egui::pos2(rect.left() + 16.0, rect.top()), egui::pos2(eye.left() - 6.0, rect.bottom()));
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .id(id)
            .password(!*revealed)
            .frame(egui::Frame::NONE)
            .font(FontId::new(17.0, FontFamily::Monospace))
            .hint_text(crate::i18n::t("input.password_hint"))
            .desired_width(inner.width())
            .vertical_align(egui::Align::Center),
    );
    ui.advance_cursor_after_rect(rect);
    PasswordResponse { response, field: rect, eye, generate, generate_clicked }
}

/// Generate's word.
pub fn generate_label() -> &'static str {
    crate::i18n::t("input.generate")
}

/// The size of a sign-in or lock field: its height, corner radius and type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxLook {
    pub height: f32,
    pub radius: u8,
    pub font_px: f32,
}

/// The sign-in screen's fields (spec C 5.2): 50 high, radius 14, 16 pt.
pub const SIGN_IN: BoxLook = BoxLook { height: 50.0, radius: 14, font_px: 16.0 };
/// The lock screen's master-password field (spec C 5.3): 58 high, radius 16.
pub const LOCK: BoxLook = BoxLook { height: 58.0, radius: 16, font_px: 17.0 };

/// The eye's square, and the gap it keeps from the field's right edge.
const EYE: f32 = 36.0;
const EYE_INSET: f32 = 8.0;

fn look_box(ui: &Ui, rect: Rect, look: BoxLook, edge: Color32, fill: Color32) {
    ui.painter().rect(rect, CornerRadius::same(look.radius), fill, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
}

/// A password field with an eye, hidden unless `revealed`: the box turns blue
/// with the keyboard, like [`text`]. `placeholder` shows while it is empty.
pub fn secret(
    ui: &mut Ui,
    id: egui::Id,
    value: &mut String,
    revealed: &mut bool,
    placeholder: &str,
    width: f32,
    look: BoxLook,
) -> Response {
    let p = tokens::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, look.height), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(id));
    look_box(ui, rect, look, if focused { p.blue } else { p.border }, p.card);
    let eye = Rect::from_center_size(egui::pos2(rect.right() - EYE_INSET - EYE / 2.0, rect.center().y), Vec2::splat(EYE));
    let eye_response = ui.interact(eye, id.with("eye"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if eye_response.hovered() {
        ui.painter().rect_filled(eye, CornerRadius::same(10), p.hover);
    }
    let d = if *revealed { icon::glyph::EYE_OFF } else { icon::glyph::EYE };
    icon::stroke(ui.painter(), Rect::from_center_size(eye.center(), Vec2::splat(19.0)), d, 2.0, p.text_icon);
    crate::kit::tips::hint(ui, &eye_response, crate::i18n::t("input.show_hide"), &[]);
    if eye_response.clicked() {
        *revealed = !*revealed;
    }
    let inner = Rect::from_min_max(egui::pos2(rect.left() + 16.0, rect.top()), egui::pos2(eye.left() - 4.0, rect.bottom()));
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .id(id)
            .password(!*revealed)
            .frame(egui::Frame::NONE)
            .font(FontId::proportional(look.font_px))
            .hint_text(egui::RichText::new(placeholder).font(FontId::proportional(look.font_px)))
            .desired_width(inner.width())
            .vertical_align(egui::Align::Center),
    );
    ui.advance_cursor_after_rect(rect);
    response
}

/// A text field while an attempt is in flight: the box greyed and `value`
/// painted as text, with **no `TextEdit` in it**, so it takes no focus, draws
/// no caret and eats no click.
pub fn text_disabled(ui: &mut Ui, value: &str, width: f32, look: BoxLook) -> Rect {
    let p = tokens::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, look.height), Sense::hover());
    look_box(ui, rect, look, p.border, p.field_bg);
    let galley = ui.painter().layout_no_wrap(value.to_owned(), FontId::proportional(look.font_px), p.text_faint);
    let at = egui::pos2(rect.left() + 16.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().with_clip_rect(rect.shrink2(Vec2::new(16.0, 0.0))).galley(at, galley, p.text_faint);
    rect
}

/// A password field while an attempt is in flight: the same ten bullets
/// whatever the buffer holds (so the mask neither shows the length nor
/// announces an emptied buffer), the eye greyed and inert.
pub fn secret_disabled(ui: &mut Ui, width: f32, look: BoxLook) -> Rect {
    let p = tokens::palette(ui.ctx());
    let rect = text_disabled(ui, &crate::theme::masked_readout(), width, look);
    let eye = Rect::from_center_size(egui::pos2(rect.right() - EYE_INSET - EYE / 2.0, rect.center().y), Vec2::splat(EYE));
    icon::stroke(ui.painter(), Rect::from_center_size(eye.center(), Vec2::splat(19.0)), icon::glyph::EYE, 2.0, p.text_ghost);
    rect
}

/// One step of the strength meter: its word, colour and how full the track is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrengthLevel {
    pub label: &'static str,
    pub colour: Color32,
    pub fraction: f32,
}

/// The app's four ratings in the design's four words and colours (spec B
/// 1.4 A): Weak red, Fair amber, Strong blue, Very strong green.
pub fn strength_level(rating: Strength, p: &tokens::Palette) -> StrengthLevel {
    match rating {
        Strength::Weak => StrengthLevel { label: GeneratorLevel::Weak.label(), colour: p.error, fraction: 0.25 },
        Strength::Fair => StrengthLevel { label: GeneratorLevel::Fair.label(), colour: p.caution_dot, fraction: 0.5 },
        Strength::Good => StrengthLevel { label: GeneratorLevel::Strong.label(), colour: p.blue, fraction: 0.75 },
        Strength::Strong => StrengthLevel { label: GeneratorLevel::VeryStrong.label(), colour: p.done_mark, fraction: 1.0 },
    }
}

/// The meter under the password: a 6pt track, filled to the rating, and the
/// word right-aligned in an 86pt column beside it. `None` (no password) is
/// the empty track and no word.
pub fn strength_meter(ui: &mut Ui, rating: Option<Strength>, width: f32) {
    let p = tokens::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::hover());
    let track = Rect::from_min_max(egui::pos2(rect.left(), rect.center().y - 3.0), egui::pos2(rect.right() - 86.0 - 12.0, rect.center().y + 3.0));
    ui.painter().rect_filled(track, CornerRadius::same(255), p.meter_track);
    if let Some(level) = rating.map(|r| strength_level(r, &p)) {
        let fill = Rect::from_min_size(track.min, Vec2::new(track.width() * level.fraction, track.height()));
        ui.painter().rect_filled(fill, CornerRadius::same(255), level.colour);
        let role = tokens::Role { size: 13.0, face: crate::theme::BOLD, tracking_em: 0.0 };
        let word = ui.fonts_mut(|f| f.layout_job(tokens::job(role, level.label, level.colour)));
        let size = word.size();
        ui.painter().galley(egui::pos2(rect.right() - size.x, rect.center().y - size.y / 2.0), word, level.colour);
    }
}

/// A card number as it is typed (spec B 1.4 A): digits only, at most 19,
/// in groups of four.
pub fn grouped_card_number(input: &str) -> String {
    let digits: Vec<char> = input.chars().filter(char::is_ascii_digit).take(19).collect();
    digits.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at};
    use crate::kit::tokens::Palette;
    use crate::password_strength::Strength;
    use eframe::egui::{self, vec2};

    const P: Palette = Palette::LIGHT;
    const SCREEN: egui::Vec2 = egui::vec2(500.0, 300.0);

    #[test]
    fn a_text_field_is_fifty_high_and_turns_blue_when_focused() {
        let ctx = testing::ctx();
        let id = egui::Id::new("t");
        let mut value = String::from("a.novak");
        let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            text(ui, id, &mut value, "Email or username", false, 300.0);
        });
        let field = p.rects.iter().find(|r| r.stroke.color == P.border).expect("the field's box");
        assert_eq!(field.rect.size(), vec2(300.0, 50.0));
        assert_eq!(field.corner_radius, egui::CornerRadius::same(14));
        ctx.memory_mut(|m| m.request_focus(id));
        let p = testing::frame(&ctx, 0.1, SCREEN, vec![], |ui| {
            text(ui, id, &mut value, "", false, 300.0);
        });
        assert!(p.rects.iter().any(|r| r.stroke.color == P.blue && r.rect.height() == 50.0), "focused, the border is blue");
    }

    #[test]
    fn a_password_field_hides_until_revealed_and_offers_generate() {
        let ctx = testing::ctx();
        let id = egui::Id::new("pw");
        let mut value = String::from("hunter2");
        let mut revealed = false;
        let mut got = None;
        let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            got = Some(password(ui, id, &mut value, &mut revealed, 400.0));
        });
        assert!(!p.texts.iter().any(|(t, _, _)| t.contains("hunter2")), "the password is shown in clear");
        let r = got.unwrap();
        assert_eq!(r.field.height(), 52.0);
        let _ = testing::frame(&ctx, 0.1, SCREEN, click_at(r.eye.center()), |ui| {
            password(ui, id, &mut value, &mut revealed, 400.0);
        });
        assert!(revealed, "the eye reveals");
        let mut generated = false;
        let _ = testing::frame(&ctx, 0.2, SCREEN, click_at(r.generate.center()), |ui| {
            generated = password(ui, id, &mut value, &mut revealed, 400.0).generate_clicked;
        });
        assert!(generated, "Generate reports its click");
    }

    #[test]
    fn the_secret_field_has_the_sign_in_and_lock_sizes_and_hides_until_revealed() {
        let ctx = testing::ctx();
        let id = egui::Id::new("s");
        let mut value = String::from("hunter2");
        let mut revealed = false;
        let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            secret(ui, id, &mut value, &mut revealed, "pw", 400.0, SIGN_IN);
            secret(ui, id.with(1), &mut value, &mut revealed, "pw", 400.0, LOCK);
        });
        assert!(!p.texts.iter().any(|(t, _, _)| t.contains("hunter2")));
        assert_eq!(p.rects.iter().filter(|r| r.rect.height() == 50.0 && r.fill == P.card).count(), 1);
        assert_eq!(p.rects.iter().filter(|r| r.rect.height() == 58.0 && r.fill == P.card).count(), 1);
    }

    #[test]
    fn a_disabled_field_shows_the_same_mask_whatever_the_buffer_and_takes_no_focus() {
        let ctx = testing::ctx();
        let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            secret_disabled(ui, 300.0, SIGN_IN);
            text_disabled(ui, "a.novak", 300.0, SIGN_IN);
        });
        assert!(p.texts.iter().any(|(t, _, _)| t == &crate::theme::masked_readout()));
        assert!(p.texts.iter().any(|(t, _, _)| t == "a.novak"));
        assert!(p.rects.iter().any(|r| r.fill == P.field_bg), "greyed");
        assert!(ctx.memory(|m| m.focused()).is_none());
    }

    #[test]
    fn the_strength_meter_names_and_colours_the_four_levels() {
        for (rating, label, colour) in [
            (Strength::Weak, "Weak", P.error),
            (Strength::Fair, "Fair", P.caution_dot),
            (Strength::Good, "Strong", P.blue),
            (Strength::Strong, "Very strong", P.done_mark),
        ] {
            let level = strength_level(rating, &P);
            assert_eq!((level.label, level.colour), (label, colour), "{rating:?}");
        }
        let ctx = testing::ctx();
        let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| strength_meter(ui, Some(Strength::Good), 400.0));
        assert_eq!(p.text("Strong").expect("the label").1, P.blue);
    }

    #[test]
    fn typing_a_card_number_groups_it() {
        assert_eq!(grouped_card_number("4111-1111 1111 1111"), "4111 1111 1111 1111");
        assert_eq!(grouped_card_number("41111111111111111112"), "4111 1111 1111 1111 111", "capped at 19 digits");
        assert_eq!(grouped_card_number("4111 1"), "4111 1");
        assert_eq!(grouped_card_number(""), "");
    }
}

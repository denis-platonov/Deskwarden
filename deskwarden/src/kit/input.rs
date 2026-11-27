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

/// [`text`] for a form row that does not name its own id: the id is the one
/// egui would give the next widget, so the field keeps its keyboard focus from
/// frame to frame the way the bare `TextEdit` it replaces did. The edit
/// form's rows (Cardholder name, Path, Arguments, Public key...) are this.
pub fn field(ui: &mut Ui, value: &mut String, width: f32) -> Response {
    let id = ui.next_auto_id().with("field");
    text(ui, id, value, "", false, width)
}

/// [`secret`] for a form row, at [`SIGN_IN`]'s size with no placeholder and an
/// auto id, like [`field`]: the card's security code and PIN, a hidden custom
/// field, the SSH private key.
pub fn field_secret(ui: &mut Ui, value: &mut String, revealed: &mut bool, width: f32) -> Response {
    let id = ui.next_auto_id().with("field");
    secret(ui, id, value, revealed, "", width, SIGN_IN)
}

/// [`field`] across the room the row has left.
pub fn field_fill(ui: &mut Ui, value: &mut String) -> Response {
    let width = ui.available_width();
    field(ui, value, width)
}

/// [`field_secret`] across the room the row has left.
pub fn field_secret_fill(ui: &mut Ui, value: &mut String, revealed: &mut bool) -> Response {
    let width = ui.available_width();
    field_secret(ui, value, revealed, width)
}

/// The inset of a field's text from its edge, which the rows that paint a
/// chip inside a [`field_disabled`] box have to keep clear of.
pub const FIELD_INSET: f32 = 16.0;

/// A form row's greyed read-out (the `Type` row, a website on a create form,
/// the one-time code's "added after saving"): a box `room` wide, [`text`]'s
/// 50 high, `text` elided to fit and `reserve` points kept clear at the
/// right-hand end for something the caller paints there. It senses nothing, so
/// the pointer passes over it. Answers the box so a caller can hang a hover on
/// it.
pub fn field_disabled(ui: &mut Ui, text: &str, room: f32, reserve: f32) -> Rect {
    let p = tokens::palette(ui.ctx());
    let width = room.min(ui.available_width()).max(0.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 50.0), Sense::hover());
    look_box(ui, rect, SIGN_IN, p.border, p.field_bg);
    let room_for_text = (rect.width() - 2.0 * FIELD_INSET - reserve.max(0.0)).max(1.0);
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(16.0), p.text_faint);
    job.wrap = egui::text::TextWrapping::truncate_at_width(room_for_text);
    let galley = ui.painter().layout_job(job);
    let at = egui::pos2(rect.left() + FIELD_INSET, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(at, galley, p.text_faint);
    rect
}

/// [`field_disabled`] across the room the row has left.
pub fn field_disabled_fill(ui: &mut Ui, text: &str) -> Rect {
    let width = ui.available_width();
    field_disabled(ui, text, width, 0.0)
}

/// How tall the compact field is: the same 42 as [`controls::ROW`]'s buttons,
/// so it can share a line with them.
pub const INLINE_HEIGHT: f32 = 42.0;

/// The compact field of the Send composer's access rows (Share with, Views,
/// Password): [`INLINE_HEIGHT`] high at radius 12, 15 pt type, `hint` while it
/// is empty, bullets when `password` (no eye: Generate sits beside it).
pub fn inline(ui: &mut Ui, value: &mut String, hint: &str, width: f32, password: bool) -> Response {
    let id = ui.next_auto_id().with("field");
    let p = tokens::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, INLINE_HEIGHT), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(id));
    ui.painter().rect(
        rect,
        CornerRadius::same(12),
        p.card,
        Stroke::new(1.0, if focused { p.blue } else { p.border }),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(Vec2::new(14.0, 0.0));
    let response = ui.put(
        inner,
        egui::TextEdit::singleline(value)
            .id(id)
            .password(password)
            .frame(egui::Frame::NONE)
            .font(FontId::proportional(15.0))
            .hint_text(hint)
            .desired_width(inner.width())
            .vertical_align(egui::Align::Center),
    );
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
/// word right-aligned in a column beside it -- 86pt, the design's width, or
/// wider when the word is (a translated "Very strong" often is), so the two
/// never overlap. `None` (no password) is the empty track and no word.
pub fn strength_meter(ui: &mut Ui, rating: Option<Strength>, width: f32) {
    const WORD_COLUMN: f32 = 86.0;
    const GAP: f32 = 12.0;
    let p = tokens::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::hover());
    let level = rating.map(|r| strength_level(r, &p));
    let word = level.as_ref().map(|level| {
        let role = tokens::Role { size: 13.0, face: crate::theme::BOLD, tracking_em: 0.0 };
        ui.fonts_mut(|f| f.layout_job(tokens::job(role, level.label, level.colour)))
    });
    let column = word.as_ref().map_or(WORD_COLUMN, |w| w.size().x.max(WORD_COLUMN));
    let track_right = (rect.right() - column - GAP).max(rect.left());
    let track = Rect::from_min_max(egui::pos2(rect.left(), rect.center().y - 3.0), egui::pos2(track_right, rect.center().y + 3.0));
    ui.painter().rect_filled(track, CornerRadius::same(255), p.meter_track);
    if let (Some(level), Some(word)) = (level, word) {
        let fill = Rect::from_min_size(track.min, Vec2::new(track.width() * level.fraction, track.height()));
        ui.painter().rect_filled(fill, CornerRadius::same(255), level.colour);
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

    /// The word sits clear of the track in every language: "Очень надёжный"
    /// is wider than the English column, and the track gives way to it.
    #[test]
    fn the_strength_word_never_overlaps_its_track_in_any_language() {
        let ctx = testing::ctx();
        for locale in crate::i18n::available() {
            crate::i18n::with_language(locale.tag, || {
                for rating in [Strength::Weak, Strength::Fair, Strength::Good, Strength::Strong] {
                    let level = strength_level(rating, &P);
                    let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| strength_meter(ui, Some(rating), 400.0));
                    let word = p.text(level.label).expect("the word").2;
                    let track = p.rects.iter().find(|r| r.fill == P.meter_track).expect("the track").rect;
                    assert!(
                        track.right() + 8.0 <= word.left(),
                        "{} {rating:?}: track ends at {} but the word starts at {}",
                        locale.tag,
                        track.right(),
                        word.left()
                    );
                }
            });
        }
    }

    #[test]
    fn typing_a_card_number_groups_it() {
        assert_eq!(grouped_card_number("4111-1111 1111 1111"), "4111 1111 1111 1111");
        assert_eq!(grouped_card_number("41111111111111111112"), "4111 1111 1111 1111 111", "capped at 19 digits");
        assert_eq!(grouped_card_number("4111 1"), "4111 1");
        assert_eq!(grouped_card_number(""), "");
    }

    #[test]
    fn a_form_field_keeps_its_focus_on_the_id_egui_gave_it() {
        let ctx = testing::ctx();
        let mut value = String::new();
        let mut id = egui::Id::NULL;
        testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            id = ui.next_auto_id().with("field");
            let r = field(ui, &mut value, 300.0);
            assert_eq!(r.rect.height(), 50.0);
        });
        ctx.memory_mut(|m| m.request_focus(id));
        let p = testing::frame(&ctx, 0.1, SCREEN, vec![], |ui| {
            field(ui, &mut value, 300.0);
        });
        assert!(p.rects.iter().any(|r| r.stroke.color == P.blue && r.rect.height() == 50.0), "focused, blue");
    }

    #[test]
    fn a_greyed_form_field_elides_its_text_and_leaves_room_for_a_chip() {
        let ctx = testing::ctx();
        let long = "a very long application name that cannot possibly fit in the box";
        let mut rect = egui::Rect::NOTHING;
        let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| rect = field_disabled(ui, long, 300.0, 80.0));
        assert_eq!(rect.size(), vec2(300.0, 50.0));
        let text = p.texts.iter().find(|t| t.0.starts_with("a very")).expect("the text");
        assert!(text.2.right() <= rect.right() - FIELD_INSET - 80.0 + 1.0, "{:?} runs into the chip's room in {rect:?}", text.2);
        assert!(p.rects.iter().any(|r| r.fill == P.field_bg), "greyed");
    }

    #[test]
    fn the_inline_field_is_forty_two_high_and_masks_a_password() {
        let ctx = testing::ctx();
        let mut value = String::from("hunter2");
        let mut rect = egui::Rect::NOTHING;
        let p = testing::frame(&ctx, 0.0, SCREEN, vec![], |ui| rect = inline(ui, &mut value, "Password", 200.0, true).rect);
        assert_eq!(rect.height(), INLINE_HEIGHT.min(rect.height()));
        assert!(p.text("hunter2").is_none(), "the value is not painted");
    }
}

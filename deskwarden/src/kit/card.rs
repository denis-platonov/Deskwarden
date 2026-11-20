//! Cards, the rules between their rows, field and section labels, and the
//! App / 2FA / Shared tags.

use eframe::egui::{Color32, CornerRadius, Margin, Painter, Pos2, Rect, Response, Sense, Ui, Vec2};

use crate::kit::tokens::{self, card_frame, palette, Role, LABEL, SECTION, TAG_DETAIL, TAG_LIST};

/// A white card with the design's ring and soft shadow around `add`.
pub fn card<R>(ui: &mut Ui, radius: u8, inner_margin: Margin, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui.ctx());
    card_frame(&p, radius).inner_margin(inner_margin).show(ui, add).inner
}

/// The 1 px rule between a card's rows, across the available width.
pub fn row_rule(ui: &mut Ui) {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, p.row_rule);
}

/// "Username", "2FA code", "Fills in": the small grey label over a value.
pub fn field_label(ui: &mut Ui, text: &str) -> Response {
    let p = palette(ui.ctx());
    ui.label(tokens::job(LABEL, text, p.text_label))
}

/// VAULT / TOOLS / FOLDERS: tracked capitals in `color`.
pub fn section_label(ui: &mut Ui, text: &str, color: Color32) -> Response {
    ui.label(tokens::job(SECTION, text, color))
}

/// Which tag: what it says and how it is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    /// The login fills a desktop app.
    App,
    /// The login carries a one-time code.
    TwoFa,
    /// The item is shared.
    Shared,
    /// Someone shares the item with this account and it cannot be changed
    /// here: 0.15's `view-only` chip, in the neutral tag's colours.
    ViewOnly,
}

/// The list's small tags or the detail header's larger ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagSize {
    List,
    Detail,
}

/// The type a tag of `size` is set in.
pub fn tag_role(size: TagSize) -> Role {
    match size {
        TagSize::List => TAG_LIST,
        TagSize::Detail => TAG_DETAIL,
    }
}

/// A tag's word and colours.
fn tag_look(p: &tokens::Palette, kind: TagKind) -> (&'static str, Color32, Color32) {
    match kind {
        TagKind::App => (crate::i18n::t("detail.row_app"), p.tag_neutral_bg, p.tag_neutral_ink),
        TagKind::TwoFa => ("2FA", p.tag_2fa_bg, p.done_ink),
        TagKind::Shared => (crate::i18n::t("list.shared_pill"), p.blue, Color32::WHITE),
        TagKind::ViewOnly => (crate::i18n::t("list.view_only"), p.tag_neutral_bg, p.tag_neutral_ink),
    }
}

/// A tag's padding and corner radius at `size`.
fn tag_box(size: TagSize) -> (Vec2, u8) {
    match size {
        TagSize::List => (Vec2::new(7.0, 2.0), 6),
        TagSize::Detail => (Vec2::new(9.0, 3.0), 7),
    }
}

fn tag_galley(ctx: &eframe::egui::Context, kind: TagKind, size: TagSize) -> std::sync::Arc<eframe::egui::Galley> {
    let (label, _, ink) = tag_look(&palette(ctx), kind);
    ctx.fonts_mut(|f| f.layout_job(tokens::job(tag_role(size), label, ink)))
}

/// How big a tag of `kind` at `size` is, for laying one out by hand.
pub fn tag_size(ctx: &eframe::egui::Context, kind: TagKind, size: TagSize) -> Vec2 {
    tag_galley(ctx, kind, size).size() + tag_box(size).0 * 2.0
}

/// Paints a tag with its top-left at `at`, allocating nothing: for rows that
/// are painted by hand. Returns where it went.
pub fn paint_tag(painter: &Painter, at: Pos2, kind: TagKind, size: TagSize) -> Rect {
    let (_, bg, ink) = tag_look(&palette(painter.ctx()), kind);
    let (pad, radius) = tag_box(size);
    let galley = tag_galley(painter.ctx(), kind, size);
    let rect = Rect::from_min_size(at, galley.size() + pad * 2.0);
    painter.rect_filled(rect, CornerRadius::same(radius), bg);
    painter.galley(rect.min + pad, galley, ink);
    rect
}

/// One tag: padding 2x7 at radius 6 in lists, 3x9 at radius 7 in the detail
/// header.
pub fn tag(ui: &mut Ui, kind: TagKind, size: TagSize) -> Response {
    let (rect, response) = ui.allocate_exact_size(tag_size(ui.ctx(), kind, size), Sense::hover());
    paint_tag(ui.painter(), rect.min, kind, size);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{ctx, frame};
    use crate::kit::tokens::{Palette, R_CARD};
    use eframe::egui::{Color32, CornerRadius, Margin, Rect, Stroke, Vec2};

    const SCREEN: Vec2 = Vec2::new(600.0, 400.0);
    const P: Palette = Palette::LIGHT;

    #[test]
    fn a_card_paints_one_rounded_white_rect_with_its_ring() {
        let ctx = ctx();
        let painted = frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            card(ui, R_CARD, Margin::same(20), |ui| {
                ui.label("inside");
            });
        });
        let cards: Vec<_> = painted.rects.iter().filter(|r| r.fill == Color32::WHITE).collect();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].corner_radius, CornerRadius::same(18));
        assert_eq!(cards[0].stroke, Stroke::new(1.0, P.card_ring));
    }

    #[test]
    fn tags_use_the_design_colours_and_padding() {
        let ctx = ctx();
        for (kind, label, bg, ink) in [
            (TagKind::App, "App", P.tag_neutral_bg, P.tag_neutral_ink),
            (TagKind::TwoFa, "2FA", P.tag_2fa_bg, P.done_ink),
            (TagKind::Shared, "Shared", P.blue, Color32::WHITE),
            (TagKind::ViewOnly, "View only", P.tag_neutral_bg, P.tag_neutral_ink),
        ] {
            for (size, pad_y, radius) in [(TagSize::List, 2.0, 6), (TagSize::Detail, 3.0, 7)] {
                let mut rect = Rect::NOTHING;
                let painted = frame(&ctx, 0.0, SCREEN, vec![], |ui| rect = tag(ui, kind, size).rect);
                let pill = painted.rects.iter().find(|r| r.rect == rect).expect("a tag background");
                assert_eq!(pill.fill, bg, "{label} {size:?}");
                assert_eq!(pill.corner_radius, CornerRadius::same(radius));
                let (_, color, _) = painted.text(label).expect("the tag's label");
                assert_eq!(*color, ink, "{label} {size:?}");
                let text_h = crate::kit::tokens::font_height(&ctx, tag_role(size));
                assert!((rect.height() - (text_h + 2.0 * pad_y)).abs() < 0.5, "{label} {size:?}: {rect:?}");
            }
        }
    }

    #[test]
    fn a_painted_tag_is_the_allocated_tag_at_a_point() {
        let ctx = ctx();
        let mut placed = Rect::NOTHING;
        let mut painted_at = Rect::NOTHING;
        frame(&ctx, 0.0, SCREEN, vec![], |ui| placed = tag(ui, TagKind::TwoFa, TagSize::List).rect);
        let painted = frame(&ctx, 0.1, SCREEN, vec![], |ui| {
            painted_at = paint_tag(ui.painter(), placed.min, TagKind::TwoFa, TagSize::List)
        });
        assert_eq!(painted_at, placed);
        assert_eq!(tag_size(&ctx, TagKind::TwoFa, TagSize::List), placed.size());
        assert!(painted.rects.iter().any(|r| r.rect == placed && r.fill == P.tag_2fa_bg));
    }

    #[test]
    fn the_row_rule_is_one_pixel_of_row_rule() {
        let ctx = ctx();
        let painted = frame(&ctx, 0.0, SCREEN, vec![], row_rule);
        let rule = painted.rects.iter().find(|r| r.fill == P.row_rule).expect("a rule");
        assert_eq!(rule.rect.height(), 1.0);
    }

    #[test]
    fn labels_are_set_in_their_roles() {
        let ctx = ctx();
        let painted = frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            field_label(ui, "Username");
            section_label(ui, "Folders", P.text_ghost);
        });
        assert_eq!(painted.text("Username").unwrap().1, P.text_label);
        assert_eq!(painted.text("FOLDERS").unwrap().1, P.text_ghost);
    }
}

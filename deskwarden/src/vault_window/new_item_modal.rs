//! Spec B §5: "What are you adding?" -- the New item chooser.
//!
//! The spec's three kinds first, in its words; the app's other creatable
//! kinds (identity, SSH key) follow in the same cards (slice 5, decision 2).

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::kit::icon::{self, glyph};
use crate::kit::modal::{Dialog, ModalOutcome};
use crate::theme;
use crate::vault_bridge::ItemKind;

/// The scrim's area name (listed in `item_list::MODAL_SCRIM_AREAS`).
pub const ID: &str = "new-item";
pub fn title() -> &'static str {
    crate::i18n::t("new_item.title")
}
const WIDTH: f32 = 660.0;
const CARD_HEIGHT: f32 = 168.0;
const GAP: f32 = 12.0;
const SUB_INK: Color32 = Color32::from_rgb(0x6b, 0x68, 0x66);
const EDGE: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xe3);

/// What the chooser did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChooserAnswer {
    Open,
    Closed,
    Picked(ItemKind),
}

/// One card: kind, title, sub, glyph, tile and its ink.
pub struct Choice {
    pub kind: ItemKind,
    pub title: &'static str,
    pub sub: &'static str,
    glyph: &'static str,
    tile: Color32,
    ink: Color32,
}

/// Every card, in order.
pub fn choices() -> [Choice; 5] {
    let rgb = Color32::from_rgb;
    [
        Choice { kind: ItemKind::Login, title: crate::i18n::t("new_item.login"), sub: crate::i18n::t("new_item.login_sub"), glyph: glyph::LOGIN, tile: rgb(0xee, 0xf2, 0xfc), ink: rgb(0x1b, 0x3f, 0xa0) },
        Choice { kind: ItemKind::Card, title: crate::i18n::t("new_item.card"), sub: crate::i18n::t("new_item.card_sub"), glyph: glyph::CARD, tile: rgb(0xe8, 0xf3, 0xec), ink: rgb(0x17, 0x67, 0x3a) },
        Choice { kind: ItemKind::SecureNote, title: crate::i18n::t("new_item.note"), sub: crate::i18n::t("new_item.note_sub"), glyph: glyph::NOTE, tile: rgb(0xfe, 0xf6, 0xe7), ink: rgb(0x8a, 0x5a, 0x06) },
        Choice { kind: ItemKind::Identity, title: crate::i18n::t("new_item.identity"), sub: crate::i18n::t("new_item.identity_sub"), glyph: glyph::IDENTITY, tile: rgb(0xf1, 0xf0, 0xee), ink: rgb(0x4a, 0x47, 0x45) },
        Choice { kind: ItemKind::SshKey, title: crate::i18n::t("new_item.ssh"), sub: crate::i18n::t("new_item.ssh_sub"), glyph: glyph::SSH, tile: rgb(0xf1, 0xf0, 0xee), ink: rgb(0x4a, 0x47, 0x45) },
    ]
}

/// A card's id, for tests and the window.
pub fn card_id(kind: ItemKind) -> egui::Id {
    egui::Id::new(("new-item-card", format!("{kind:?}")))
}

pub fn draw(ctx: &egui::Context) -> ChooserAnswer {
    let (outcome, picked) = Dialog::new(ID, WIDTH).title(title()).show(ctx, |ui| {
        let mut picked = None;
        let width = ui.available_width();
        let card_w = ((width - 2.0 * GAP) / 3.0).floor();
        for row in choices().chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for choice in row {
                    if card(ui, choice, card_w) {
                        picked = Some(choice.kind);
                    }
                }
            });
            ui.add_space(GAP - ui.spacing().item_spacing.y);
        }
        picked
    });
    match (outcome, picked) {
        (_, Some(kind)) => ChooserAnswer::Picked(kind),
        (ModalOutcome::Closed, None) => ChooserAnswer::Closed,
        (ModalOutcome::Open, None) => ChooserAnswer::Open,
    }
}

/// One kind's card; true when clicked.
fn card(ui: &mut egui::Ui, choice: &Choice, width: f32) -> bool {
    let (slot, response) = ui.allocate_exact_size(Vec2::new(width, CARD_HEIGHT), Sense::click());
    let _ = ui.interact(slot, card_id(choice.kind), Sense::hover());
    let hovered = response.hovered();
    let rect = if hovered { slot.translate(Vec2::new(0.0, -3.0)) } else { slot };
    if hovered {
        let lift = egui::Shadow { offset: [0, 12], blur: 26, spread: 0, color: Color32::from_rgba_unmultiplied(27, 63, 160, 36) };
        ui.painter().add(lift.as_shape(rect, CornerRadius::same(18)));
    }
    let edge = if hovered { theme::BLUE } else { EDGE };
    ui.painter().rect(rect, CornerRadius::same(18), theme::CARD, Stroke::new(2.0, edge), StrokeKind::Inside);
    let tile = Rect::from_min_size(rect.min + Vec2::splat(20.0), Vec2::splat(52.0));
    ui.painter().rect_filled(tile, CornerRadius::same(15), choice.tile);
    icon::stroke(ui.painter(), Rect::from_center_size(tile.center(), Vec2::splat(24.0)), choice.glyph, 2.0, choice.ink);
    let text_w = rect.width() - 40.0;
    let title = ui.painter().layout(
        choice.title.to_string(),
        egui::FontId::new(18.0, egui::FontFamily::Name(theme::EXTRABOLD.into())),
        theme::INK,
        text_w,
    );
    let title_at = egui::pos2(rect.left() + 20.0, tile.bottom() + 16.0);
    let title_h = title.size().y;
    ui.painter().galley(title_at, title, theme::INK);
    let sub = ui.painter().layout(choice.sub.to_string(), egui::FontId::proportional(14.0), SUB_INK, text_w);
    ui.painter().galley(title_at + Vec2::new(0.0, title_h + 4.0), sub, SUB_INK);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at, Painted};

    const SCREEN: Vec2 = egui::vec2(1280.0, 820.0);

    fn frame(ctx: &egui::Context, time: f64, events: Vec<egui::Event>) -> (ChooserAnswer, Painted) {
        let mut answer = ChooserAnswer::Open;
        let p = testing::frame(ctx, time, SCREEN, events, |ui| {
            answer = draw(ui.ctx());
        });
        (answer, p)
    }

    fn settled() -> (egui::Context, Painted) {
        let ctx = testing::ctx();
        let _ = frame(&ctx, 0.0, vec![]);
        let _ = frame(&ctx, 0.1, vec![]);
        let (_, p) = frame(&ctx, 0.2, vec![]);
        (ctx, p)
    }

    fn at(p: &Painted, text: &str) -> Rect {
        p.texts.iter().find(|(t, _, _)| t == text).unwrap_or_else(|| panic!("no {text}")).2
    }

    #[test]
    fn the_chooser_offers_login_card_and_note_first() {
        let (_, p) = settled();
        assert!(p.texts.iter().any(|(t, _, _)| t == title()));
        let (login, card, note) = (at(&p, "Login"), at(&p, "Card"), at(&p, "Secure note"));
        assert!(login.left() < card.left() && card.left() < note.left(), "the first row is login, card, note");
        assert!((login.top() - note.top()).abs() < 1.0);
        for sub in ["For an app or a website", "Pay without hunting for your wallet", "Text only you can read"] {
            assert!(p.texts.iter().any(|(t, _, _)| t == sub), "no {sub}");
        }
        assert!(at(&p, "Identity").top() > login.bottom(), "the app's other kinds follow");
    }

    #[test]
    fn picking_a_kind_opens_its_editor() {
        let (ctx, _) = settled();
        let card = ctx.read_response(card_id(ItemKind::Card)).expect("the Card card").rect;
        let (answer, _) = frame(&ctx, 0.3, click_at(card.center()));
        assert_eq!(answer, ChooserAnswer::Picked(ItemKind::Card));
    }
}

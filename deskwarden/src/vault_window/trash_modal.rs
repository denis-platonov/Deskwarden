//! Spec C §3: "Empty trash?" -- the one confirmation before every trashed
//! item is deleted for good.

use eframe::egui;

use crate::kit::controls::{self, ButtonKind, ButtonSize};
use crate::kit::modal::{Dialog, ModalOutcome};

/// The scrim's area name (listed in `item_list::MODAL_SCRIM_AREAS`).
pub const ID: &str = "empty-trash";
pub const TITLE: &str = "Empty trash?";
pub const CONFIRM: &str = "Delete forever";
pub const CANCEL: &str = "Cancel";
pub const EMPTIED_TOAST: &str = "Trash emptied";

/// What the dialog says will go.
pub fn body(count: usize) -> String {
    let noun = if count == 1 { "item" } else { "items" };
    format!("{count} {noun} will be deleted for good. This can't be undone.")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyTrashAnswer {
    Open,
    Cancel,
    Confirm,
}

pub fn confirm_id() -> egui::Id {
    egui::Id::new("empty-trash-confirm")
}

pub fn draw(ctx: &egui::Context, count: usize) -> EmptyTrashAnswer {
    let text = body(count);
    let (outcome, answer) = Dialog::new(ID, 480.0).title(TITLE).sub(&text).show(ctx, |ui| {
        let mut answer = EmptyTrashAnswer::Open;
        let size = ButtonSize { height: 48.0, pad_x: 18.0, radius: 14, font_px: 16.0 };
        ui.columns(2, |columns| {
            if controls::button_filling(&mut columns[0], ButtonKind::Outline, size, CANCEL).clicked() {
                answer = EmptyTrashAnswer::Cancel;
            }
            let confirm = controls::button_filling(&mut columns[1], ButtonKind::Danger, size, CONFIRM);
            let _ = columns[1].interact(confirm.rect, confirm_id(), egui::Sense::hover());
            if confirm.clicked() {
                answer = EmptyTrashAnswer::Confirm;
            }
        });
        answer
    });
    match (answer, outcome) {
        (EmptyTrashAnswer::Open, ModalOutcome::Closed) => EmptyTrashAnswer::Cancel,
        (answer, _) => answer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at};

    #[test]
    fn empty_trash_asks_then_purges_every_item() {
        assert_eq!(body(3), "3 items will be deleted for good. This can't be undone.");
        assert_eq!(body(1), "1 item will be deleted for good. This can't be undone.");
        let ctx = testing::ctx();
        let screen = egui::vec2(1280.0, 820.0);
        let mut answer = EmptyTrashAnswer::Open;
        for t in [0.0, 0.1, 0.2] {
            let _ = testing::frame(&ctx, t, screen, vec![], |ui| answer = draw(ui.ctx(), 3));
            assert_eq!(answer, EmptyTrashAnswer::Open, "nothing goes until it is confirmed");
        }
        let at = ctx.read_response(confirm_id()).expect("the confirm button").rect.center();
        let _ = testing::frame(&ctx, 0.3, screen, click_at(at), |ui| answer = draw(ui.ctx(), 3));
        assert_eq!(answer, EmptyTrashAnswer::Confirm);
    }
}

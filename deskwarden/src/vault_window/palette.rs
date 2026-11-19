//! Spec B §4: the Ctrl+K command palette -- search the vault's items and the
//! window's actions from one box, with Up/Down/Enter as well as the mouse.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Vec2};

use crate::kit::icon::{self, glyph};
use crate::kit::modal::{Dialog, ModalOutcome};
use crate::theme;
use crate::vault_bridge::VaultItem;

/// The scrim's area name (listed in `item_list::MODAL_SCRIM_AREAS`).
pub const ID: &str = "palette";
pub const PLACEHOLDER: &str = "Search or jump to\u{2026}";
pub const NOTHING_FOUND: &str = "Nothing found";
pub const ITEMS_HEADING: &str = "ITEMS";
pub const ACTIONS_HEADING: &str = "ACTIONS";
const WIDTH: f32 = 640.0;
const TOP: f32 = 90.0;
const MAX_ITEMS: usize = 5;
const FAINT: Color32 = Color32::from_rgb(0x9b, 0x97, 0x97);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x87, 0x85);
const ACTIVE: Color32 = Color32::from_rgb(0xee, 0xf2, 0xfc);

/// What a palette action does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaletteAction {
    NewLogin,
    /// Slice 6: the Generator screen.
    Generate,
    Share,
    Settings,
    Lock,
}

/// What the palette did this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAnswer {
    Open,
    Closed,
    Item(String),
    Action(PaletteAction),
}

/// The palette's own state while it is up.
#[derive(Clone, Debug, Default)]
pub struct PaletteState {
    pub query: String,
    /// The highlighted row, counting items first and then actions.
    pub active: usize,
}

/// One action row: what it does, its label, its key hint and glyph.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionRow {
    pub action: PaletteAction,
    pub label: String,
    pub hint: &'static str,
    glyph: &'static str,
}

/// The items whose name and user name hold the query, at most five.
pub fn matching_items<'a>(items: &'a [VaultItem], query: &str) -> Vec<&'a VaultItem> {
    let query = query.trim().to_lowercase();
    items
        .iter()
        .filter(|item| {
            let user = item.login.as_ref().and_then(|l| l.username.as_deref()).unwrap_or("");
            format!("{} {}", item.name, user).to_lowercase().contains(&query)
        })
        .take(MAX_ITEMS)
        .collect()
}

/// The actions whose label holds the query. "Share" is offered only with an
/// item selected. (Import joins with its wizard, slice 9.)
pub fn matching_actions(selected: Option<&str>, query: &str) -> Vec<ActionRow> {
    let query = query.trim().to_lowercase();
    let mut rows = vec![
        ActionRow { action: PaletteAction::NewLogin, label: "New login".into(), hint: "Ctrl N", glyph: glyph::LOGIN },
        ActionRow { action: PaletteAction::Generate, label: "Generate a password".into(), hint: "", glyph: glyph::GEN },
    ];
    if let Some(name) = selected {
        rows.push(ActionRow { action: PaletteAction::Share, label: format!("Share {name}"), hint: "", glyph: glyph::SHARED });
    }
    rows.push(ActionRow { action: PaletteAction::Settings, label: "Open settings".into(), hint: "", glyph: glyph::SETTINGS });
    rows.push(ActionRow { action: PaletteAction::Lock, label: "Lock now".into(), hint: "Ctrl L", glyph: glyph::LOCK });
    rows.retain(|row| row.label.to_lowercase().contains(&query));
    rows
}

pub fn input_id() -> egui::Id {
    egui::Id::new("palette-input")
}

pub fn row_id(index: usize) -> egui::Id {
    egui::Id::new(("palette-row", index))
}

pub fn draw(ctx: &egui::Context, state: &mut PaletteState, items: &[VaultItem], selected: Option<&str>) -> PaletteAnswer {
    let found = matching_items(items, &state.query);
    let actions = matching_actions(selected, &state.query);
    let total = found.len() + actions.len();
    // The keyboard: Up/Down move the highlight, Enter takes it.
    let (up, down, enter) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
        )
    });
    if total == 0 {
        state.active = 0;
    } else {
        state.active = state.active.min(total - 1);
        if down {
            state.active = (state.active + 1) % total;
        }
        if up {
            state.active = (state.active + total - 1) % total;
        }
    }
    let pick = |index: usize| -> PaletteAnswer {
        if index < found.len() {
            PaletteAnswer::Item(found[index].id.clone())
        } else {
            PaletteAnswer::Action(actions[index - found.len()].action)
        }
    };
    let mut answer = if enter && total > 0 { Some(pick(state.active)) } else { None };

    let (outcome, clicked) = Dialog::new(ID, WIDTH).top(TOP).without_close().padding(0).radius(22).show(ctx, |ui| {
        let mut clicked = None;
        // The search row.
        egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 18)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (glass, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                icon::stroke(ui.painter(), glass, glyph::SEARCH, 2.2, MUTED);
                let width = ui.available_width() - 48.0;
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut state.query)
                        .id(input_id())
                        .frame(egui::Frame::NONE)
                        .hint_text(PLACEHOLDER)
                        .font(egui::FontId::proportional(19.0))
                        .desired_width(width),
                );
                if !edit.has_focus() {
                    edit.request_focus();
                }
                if edit.changed() {
                    state.active = 0;
                }
                let chip = ui.painter().layout_no_wrap("Esc".into(), egui::FontId::proportional(12.0), FAINT);
                let (rect, _) = ui.allocate_exact_size(chip.size() + Vec2::new(14.0, 4.0), Sense::hover());
                ui.painter().rect_stroke(rect, CornerRadius::same(6), egui::Stroke::new(1.0, Color32::from_rgb(0xdc, 0xda, 0xd7)), egui::StrokeKind::Inside);
                ui.painter().galley(rect.center() - chip.size() / 2.0, chip, FAINT);
            });
        });
        let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(line, 0.0, Color32::from_rgb(0xee, 0xec, 0xe9));

        egui::Frame::new().inner_margin(8).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                if total == 0 {
                    ui.add_space(32.0);
                    ui.vertical_centered(|ui| ui.label(egui::RichText::new(NOTHING_FOUND).size(15.0).color(MUTED)));
                    ui.add_space(32.0);
                    return;
                }
                if !found.is_empty() {
                    heading(ui, ITEMS_HEADING, 10.0);
                }
                for (i, item) in found.iter().enumerate() {
                    let user = item.login.as_ref().and_then(|l| l.username.clone()).unwrap_or_default();
                    let look = super::item_list::row_look(item, None);
                    if row(ui, i, state.active == i, |ui, rect| {
                        let tile = Rect::from_min_size(rect.left_center() + Vec2::new(12.0, -18.0), Vec2::splat(36.0));
                        crate::kit::avatar::paint(ui.painter(), tile, &look, 10);
                        let x = tile.right() + 12.0;
                        ui.painter().text(egui::pos2(x, rect.center().y - 1.0), egui::Align2::LEFT_BOTTOM, &item.name, egui::FontId::new(15.0, egui::FontFamily::Name(theme::BOLD.into())), theme::INK);
                        ui.painter().text(egui::pos2(x, rect.center().y + 1.0), egui::Align2::LEFT_TOP, &user, egui::FontId::proportional(13.0), MUTED);
                    }) {
                        clicked = Some(i);
                    }
                }
                if !actions.is_empty() {
                    heading(ui, ACTIONS_HEADING, 12.0);
                }
                for (j, action) in actions.iter().enumerate() {
                    let index = found.len() + j;
                    if row(ui, index, state.active == index, |ui, rect| {
                        let tile = Rect::from_min_size(rect.left_center() + Vec2::new(12.0, -18.0), Vec2::splat(36.0));
                        ui.painter().rect_filled(tile, CornerRadius::same(10), Color32::from_rgb(0xf1, 0xf0, 0xee));
                        icon::stroke(ui.painter(), Rect::from_center_size(tile.center(), Vec2::splat(18.0)), action.glyph, 2.0, Color32::from_rgb(0x4a, 0x47, 0x45));
                        ui.painter().text(egui::pos2(tile.right() + 12.0, rect.center().y), egui::Align2::LEFT_CENTER, &action.label, egui::FontId::new(15.0, egui::FontFamily::Name(theme::SEMIBOLD.into())), theme::INK);
                        ui.painter().text(rect.right_center() - Vec2::new(12.0, 0.0), egui::Align2::RIGHT_CENTER, action.hint, egui::FontId::proportional(12.0), FAINT);
                    }) {
                        clicked = Some(index);
                    }
                }
            });
        });
        clicked
    });
    if let Some(index) = clicked {
        answer = Some(pick(index));
    }
    match (answer, outcome) {
        (Some(answer), _) => answer,
        (None, ModalOutcome::Closed) => PaletteAnswer::Closed,
        (None, ModalOutcome::Open) => PaletteAnswer::Open,
    }
}

fn heading(ui: &mut egui::Ui, text: &str, top: f32) {
    ui.add_space(top - 6.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(egui::RichText::new(text).size(12.0).color(FAINT).family(egui::FontFamily::Name(theme::EXTRABOLD.into())));
    });
}

/// One 54pt row, highlighted when active or hovered; true when clicked.
fn row(ui: &mut egui::Ui, index: usize, active: bool, paint: impl FnOnce(&egui::Ui, Rect)) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 54.0), Sense::click());
    let _ = ui.interact(rect, row_id(index), Sense::hover());
    if active || response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(12), ACTIVE);
    }
    paint(ui, rect);
    response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at, Painted};

    const SCREEN: Vec2 = egui::vec2(1280.0, 820.0);

    fn item(id: &str, name: &str, user: &str) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "type": 1,
            "login": { "username": user, "uris": [] }
        }))
        .expect("a login")
    }

    fn vault() -> Vec<VaultItem> {
        vec![
            item("1", "Ledgerline", "a.novak"),
            item("2", "Git Host", "anovak"),
            item("3", "Contoso 365", "ada@contoso.test"),
            item("4", "Ledger archive", "x"),
            item("5", "Ledgerline staging", "y"),
            item("6", "Ledgerline corporate", "z"),
            item("7", "Ledge", "w"),
        ]
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::default() }
    }

    fn frame(ctx: &egui::Context, time: f64, state: &mut PaletteState, events: Vec<egui::Event>) -> (PaletteAnswer, Painted) {
        let items = vault();
        let mut answer = PaletteAnswer::Open;
        let p = testing::frame(ctx, time, SCREEN, events, |ui| {
            answer = draw(ui.ctx(), state, &items, Some("Ledgerline"));
        });
        (answer, p)
    }

    #[test]
    fn items_match_name_and_user_and_stop_at_five() {
        let items = vault();
        let names = |q: &str| matching_items(&items, q).iter().map(|i| i.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("anovak"), vec!["Git Host"], "the user name is searched");
        assert_eq!(names("  LEDGE ").len(), 5, "at most five");
        assert_eq!(names("").len(), 5);
    }

    #[test]
    fn actions_filter_by_label() {
        let labels = |q: &str| matching_actions(Some("Ledgerline"), q).into_iter().map(|a| a.label).collect::<Vec<_>>();
        assert_eq!(labels(""), vec!["New login", "Generate a password", "Share Ledgerline", "Open settings", "Lock now"]);
        assert_eq!(labels("lock"), vec!["Lock now"]);
        assert!(matching_actions(None, "share").is_empty(), "no Share without an item");
    }

    #[test]
    fn arrows_and_enter_choose_a_row() {
        let ctx = testing::ctx();
        let mut state = PaletteState { query: "lo".into(), active: 0 };
        // "lo" is no item's; the actions it leaves are New login and Lock now.
        let _ = frame(&ctx, 0.0, &mut state, vec![]);
        let _ = frame(&ctx, 0.1, &mut state, vec![key(egui::Key::ArrowDown)]);
        let (answer, _) = frame(&ctx, 0.2, &mut state, vec![key(egui::Key::Enter)]);
        assert_eq!(answer, PaletteAnswer::Action(PaletteAction::Lock));
        let mut state = PaletteState { query: "git".into(), active: 0 };
        let (answer, _) = frame(&ctx, 0.3, &mut state, vec![key(egui::Key::Enter)]);
        assert_eq!(answer, PaletteAnswer::Item("2".into()));
    }

    #[test]
    fn nothing_found_when_nothing_matches() {
        let ctx = testing::ctx();
        let mut state = PaletteState { query: "zzzz".into(), active: 0 };
        let _ = frame(&ctx, 0.0, &mut state, vec![]);
        let (answer, p) = frame(&ctx, 0.1, &mut state, vec![key(egui::Key::Enter)]);
        assert_eq!(answer, PaletteAnswer::Open, "Enter with nothing to take does nothing");
        assert!(p.texts.iter().any(|(t, _, _)| t == NOTHING_FOUND));
    }

    #[test]
    fn a_click_takes_its_row() {
        let ctx = testing::ctx();
        let mut state = PaletteState::default();
        for t in [0.0, 0.1, 0.2] {
            let _ = frame(&ctx, t, &mut state, vec![]);
        }
        let first = ctx.read_response(row_id(0)).expect("the first row").rect;
        let (answer, _) = frame(&ctx, 0.3, &mut state, click_at(first.center()));
        assert_eq!(answer, PaletteAnswer::Item("1".into()));
    }
}

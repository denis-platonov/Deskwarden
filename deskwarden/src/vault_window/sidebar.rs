//! The vault window's left pane (design 4.8 "Sidebar"): the VAULT section
//! (All items / Favorites / Logins / Cards / Secure notes / Trash, each with
//! a live count) and the FOLDERS section (one row per real vault folder,
//! also counted), plus the auto-lock countdown pinned to the bottom.

use crate::theme;
use crate::vault_bridge::{Folder, VaultItem};
use eframe::egui::{self, RichText};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarFilter {
    All,
    Favorites,
    Logins,
    Cards,
    SecureNotes,
    Trash,
    Folder(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    None,
    NewFolder,
    DeleteFolder(String),
}

/// How many of `items` fall under `filter`. Pure and separate from drawing
/// so the sidebar's counts are testable without an egui context.
pub fn count_for(items: &[VaultItem], filter: &SidebarFilter) -> usize {
    items
        .iter()
        .filter(|item| match filter {
            SidebarFilter::All => true,
            SidebarFilter::Favorites => item.favorite,
            SidebarFilter::Logins => item.item_type == Some(1),
            SidebarFilter::Cards => item.item_type == Some(3),
            SidebarFilter::SecureNotes => item.item_type == Some(2),
            SidebarFilter::Trash => false, // wired to real trash state in Task 6
            SidebarFilter::Folder(id) => item.folder_id.as_deref() == Some(id.as_str()),
        })
        .count()
}

pub fn draw_sidebar(
    ui: &mut egui::Ui,
    items: &[VaultItem],
    folders: &[Folder],
    selected: &mut SidebarFilter,
    lock_countdown: &str,
) -> SidebarAction {
    let mut action = SidebarAction::None;

    ui.vertical(|ui| {
        ui.set_width(ui.available_width());
        ui.add_space(4.0);
        section_label(ui, "VAULT");
        for (label, filter) in [
            ("All items", SidebarFilter::All),
            ("Favorites", SidebarFilter::Favorites),
            ("Logins", SidebarFilter::Logins),
            ("Cards", SidebarFilter::Cards),
            ("Secure notes", SidebarFilter::SecureNotes),
            ("Trash", SidebarFilter::Trash),
        ] {
            let count = count_for(items, &filter);
            if sidebar_row(ui, label, count, *selected == filter) {
                *selected = filter;
            }
        }

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            section_label(ui, "FOLDERS");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("+").frame(false)).clicked() {
                    action = SidebarAction::NewFolder;
                }
            });
        });
        for folder in folders {
            let filter = SidebarFilter::Folder(folder.id.clone());
            let count = count_for(items, &filter);
            ui.horizontal(|ui| {
                if sidebar_row(ui, &folder.name, count, *selected == filter) {
                    *selected = filter.clone();
                }
                if ui.small_button("×").on_hover_text("Delete folder").clicked() {
                    action = SidebarAction::DeleteFolder(folder.id.clone());
                }
            });
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(lock_countdown).size(11.0).color(theme::TEXT_GHOST));
        });
    });

    action
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(theme::letterspaced(text, 10.0, theme::SEMIBOLD, 1.2, theme::TEXT_GHOST));
    ui.add_space(4.0);
}

/// One VAULT/FOLDERS row: label left, right-aligned count. Returns true when
/// clicked.
fn sidebar_row(ui: &mut egui::Ui, label: &str, count: usize, selected: bool) -> bool {
    let response = ui.add(
        egui::Button::new("")
            .frame(false)
            .min_size(egui::vec2(ui.available_width(), 26.0)),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(response.rect), |ui| {
        ui.horizontal(|ui| {
            ui.label(theme::semibold(label, 13.0).color(if selected {
                theme::BLUE_DEEP
            } else {
                theme::TEXT_SECONDARY
            }));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(count.to_string()).size(12.0).color(theme::TEXT_GHOST));
            });
        });
    });
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_bridge::VaultField;

    fn item(item_type: Option<i64>, favorite: bool, folder_id: Option<&str>) -> VaultItem {
        VaultItem {
            id: "1".into(),
            name: "x".into(),
            fields: vec![],
            login: None,
            item_type,
            folder_id: folder_id.map(str::to_string),
            favorite,
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn all_counts_every_item() {
        let items = vec![item(Some(1), false, None), item(Some(3), true, None)];
        assert_eq!(count_for(&items, &SidebarFilter::All), 2);
    }

    #[test]
    fn favorites_counts_only_favorited_items() {
        let items = vec![item(Some(1), true, None), item(Some(1), false, None)];
        assert_eq!(count_for(&items, &SidebarFilter::Favorites), 1);
    }

    #[test]
    fn logins_and_cards_are_disjoint() {
        let items = vec![item(Some(1), false, None), item(Some(3), false, None)];
        assert_eq!(count_for(&items, &SidebarFilter::Logins), 1);
        assert_eq!(count_for(&items, &SidebarFilter::Cards), 1);
    }

    #[test]
    fn folder_counts_only_items_in_that_folder() {
        let items = vec![
            item(Some(1), false, Some("f1")),
            item(Some(1), false, Some("f2")),
            item(Some(1), false, None),
        ];
        assert_eq!(count_for(&items, &SidebarFilter::Folder("f1".to_string())), 1);
    }
}

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

impl SidebarFilter {
    /// Whether `item` falls under this filter. The single place that
    /// encodes "what does each filter variant mean" -- both `count_for`
    /// (this file) and `item_list::matches_filter` delegate to it, rather
    /// than each hand-duplicating the same per-variant scoping logic (which
    /// had drifted into two copies that happened to still agree, but had no
    /// mechanism keeping them that way).
    ///
    /// `Trash` always returns `false`: this codebase has no confirmed
    /// knowledge of `bw serve`'s trash/deletedDate JSON shape, so rather
    /// than guess at it (and risk silently misclassifying real data), Trash
    /// is left as an explicit "not implemented" no-op. There is no prior
    /// task that wired this up to real trash state -- if you're looking for
    /// one, it doesn't exist yet.
    pub(crate) fn scope_contains(&self, item: &VaultItem) -> bool {
        match self {
            SidebarFilter::All => true,
            SidebarFilter::Favorites => item.favorite,
            SidebarFilter::Logins => item.item_type == Some(1),
            SidebarFilter::Cards => item.item_type == Some(3),
            SidebarFilter::SecureNotes => item.item_type == Some(2),
            SidebarFilter::Trash => false,
            SidebarFilter::Folder(id) => item.folder_id.as_deref() == Some(id.as_str()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    None,
    NewFolder,
    /// A click on a folder's × button -- the id, not yet confirmed as an
    /// actual delete. `vault_window::mod`'s `confirm_click` decides whether
    /// this particular click arms or confirms the delete; only a confirming
    /// click results in `VaultBridge::delete_folder` actually being called.
    DeleteFolder(String),
}

/// How many of `items` fall under `filter`. Pure and separate from drawing
/// so the sidebar's counts are testable without an egui context.
pub fn count_for(items: &[VaultItem], filter: &SidebarFilter) -> usize {
    items.iter().filter(|item| filter.scope_contains(item)).count()
}

/// Reserved width for a folder row's × delete button, subtracted from
/// `sidebar_row`'s width *before* the row itself is laid out. `sidebar_row`
/// claims `min_size(width, 26.0)` -- if it were handed the full
/// `ui.available_width()` (as it used to be, implicitly, by being laid out
/// before the × in the same `ui.horizontal`), there would be nothing left
/// for the × button, which is what put it outside the panel's clickable
/// area in the first place.
const FOLDER_DELETE_BUTTON_WIDTH: f32 = 28.0;

pub fn draw_sidebar(
    ui: &mut egui::Ui,
    items: &[VaultItem],
    folders: &[Folder],
    selected: &mut SidebarFilter,
    lock_countdown: &str,
    // The folder id (if any) whose × delete button is currently armed --
    // i.e. its first click already happened and the confirm window (see
    // `vault_window::mod::DELETE_CONFIRM_WINDOW`) hasn't expired yet. Purely
    // for what that row's button shows; `vault_window::mod::confirm_click`
    // is what actually decides whether a click here arms or confirms.
    pending_delete_id: Option<&str>,
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
            let width = ui.available_width();
            if sidebar_row(ui, label, count, *selected == filter, width) {
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
            let confirming = pending_delete_id == Some(folder.id.as_str());
            ui.horizontal(|ui| {
                // Reserve the ×'s width *before* the row claims the rest of
                // the available width -- see `FOLDER_DELETE_BUTTON_WIDTH`.
                let row_width = (ui.available_width() - FOLDER_DELETE_BUTTON_WIDTH).max(0.0);
                if sidebar_row(ui, &folder.name, count, *selected == filter, row_width) {
                    *selected = filter.clone();
                }
                let hover = if confirming { "Click again to permanently delete this folder" } else { "Delete folder" };
                let label = RichText::new("×").color(if confirming { theme::ERROR } else { theme::TEXT_SECONDARY });
                if ui.small_button(label).on_hover_text(hover).clicked() {
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

/// One VAULT/FOLDERS row: label left, right-aligned count, allocated at
/// exactly `width` wide (not necessarily all of `ui.available_width()` --
/// see `FOLDER_DELETE_BUTTON_WIDTH`). Returns true when clicked.
fn sidebar_row(ui: &mut egui::Ui, label: &str, count: usize, selected: bool, width: f32) -> bool {
    let response = ui.add(
        egui::Button::new("")
            .frame(false)
            .min_size(egui::vec2(width, 26.0)),
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

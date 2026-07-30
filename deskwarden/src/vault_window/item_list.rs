//! The vault window's middle pane: search box, `+ New`, and the virtualized
//! item list (design 4.8 "Item list"). Virtualized the same way
//! `picker_ui`'s lists are (`ScrollArea::show_rows`) -- a real vault can be
//! in the thousands, and laying out every row on every repaint was already
//! a confirmed source of a laggy picker before that fix.

use super::sidebar::SidebarFilter;
use crate::theme;
use crate::vault_bridge::VaultItem;
use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke};
use std::collections::HashMap;

/// Holds loaded favicon textures, keyed by item id. Owned by
/// `vault_window::mod` (Task 9), which populates it from the background
/// favicon loader; this module only ever reads it.
#[derive(Default)]
pub struct IconCache {
    pub textures: HashMap<String, egui::TextureHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemListAction {
    None,
    NewItem,
}

/// True when `item` is both in `filter`'s scope (delegates to
/// `SidebarFilter::scope_contains` -- the one place that logic lives, so
/// this and `sidebar::count_for` can't drift apart) and matches
/// `search_lower` against its name or username.
pub fn matches_filter(item: &VaultItem, filter: &SidebarFilter, search_lower: &str) -> bool {
    if !filter.scope_contains(item) {
        return false;
    }
    if search_lower.is_empty() {
        return true;
    }
    let username = item
        .login
        .as_ref()
        .and_then(|l| l.username.as_deref())
        .unwrap_or("");
    item.name.to_lowercase().contains(search_lower) || username.to_lowercase().contains(search_lower)
}

const ROW_HEIGHT: f32 = 50.0;

pub fn draw_item_list(
    ui: &mut egui::Ui,
    items: &[VaultItem],
    filter: &SidebarFilter,
    search: &mut String,
    selected_id: &mut Option<String>,
    icons: &IconCache,
) -> ItemListAction {
    let mut action = ItemListAction::None;

    ui.horizontal(|ui| {
        let width = (ui.available_width() - 70.0).max(40.0);
        ui.add(
            egui::TextEdit::singleline(search)
                // Stable id so `Ctrl+K` (wired in `vault_window::mod`) can
                // request focus on this field from outside this function.
                .id(egui::Id::new("vault-search"))
                .hint_text(RichText::new("Search").color(theme::TEXT_GHOST))
                .desired_width(width)
                .margin(Margin::symmetric(10, 8)),
        );
        if theme::primary_button(ui, "New", None).clicked() {
            action = ItemListAction::NewItem;
        }
    });
    ui.add_space(8.0);

    let search_lower = search.to_lowercase();
    let filtered: Vec<&VaultItem> = items
        .iter()
        .filter(|item| matches_filter(item, filter, &search_lower))
        .collect();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, filtered.len(), |ui, row_range| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for row in row_range {
                let item = filtered[row];
                let selected = selected_id.as_deref() == Some(item.id.as_str());
                if item_row(ui, item, selected, icons.textures.get(&item.id)) {
                    *selected_id = Some(item.id.clone());
                }
            }
        });

    action
}

fn item_row(
    ui: &mut egui::Ui,
    item: &VaultItem,
    selected: bool,
    icon: Option<&egui::TextureHandle>,
) -> bool {
    let username = item.login.as_ref().and_then(|l| l.username.as_deref()).unwrap_or("");
    let frame = egui::Frame::new()
        .fill(if selected { theme::CARD } else { theme::CANVAS })
        .stroke(if selected {
            Stroke::new(1.0, theme::BLUE)
        } else {
            Stroke::NONE
        })
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match icon {
                    Some(tex) => {
                        ui.add(egui::Image::new((tex.id(), tex.size_vec2())).fit_to_exact_size(egui::Vec2::splat(32.0)));
                    }
                    None => theme::avatar(ui, &theme::initials(&item.name), 32.0, selected),
                }
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(theme::semibold(&item.name, 13.0).color(if selected {
                        theme::BLUE_DEEP
                    } else {
                        theme::INK
                    }));
                    if !username.is_empty() {
                        ui.label(RichText::new(username).size(11.0).color(theme::TEXT_FAINT));
                    }
                });
            });
        });
    frame.response.interact(Sense::click()).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, username: Option<&str>, item_type: Option<i64>) -> VaultItem {
        VaultItem {
            id: "1".into(),
            name: name.into(),
            fields: vec![],
            login: username.map(|u| crate::vault_bridge::LoginData {
                username: Some(u.to_string()),
                password: None,
                totp: None,
                uris: vec![],
                other: serde_json::Map::new(),
            }),
            item_type,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_search_matches_everything_in_scope() {
        assert!(matches_filter(&item("Ledgerline", None, Some(1)), &SidebarFilter::All, ""));
    }

    #[test]
    fn search_matches_name_case_insensitively() {
        assert!(matches_filter(&item("Ledgerline", None, Some(1)), &SidebarFilter::All, "ledger"));
        assert!(!matches_filter(&item("Ledgerline", None, Some(1)), &SidebarFilter::All, "vantage"));
    }

    #[test]
    fn search_matches_username_too() {
        let it = item("Ledgerline", Some("a.novak@ledgerline.com"), Some(1));
        assert!(matches_filter(&it, &SidebarFilter::All, "novak"));
    }

    #[test]
    fn out_of_scope_items_never_match_regardless_of_search() {
        let it = item("Ledgerline", None, Some(3)); // a Card
        assert!(!matches_filter(&it, &SidebarFilter::Logins, ""));
        assert!(!matches_filter(&it, &SidebarFilter::Logins, "ledgerline"));
    }
}

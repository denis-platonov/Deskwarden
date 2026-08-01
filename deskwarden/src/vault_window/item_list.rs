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

/// Design 2b's search placeholder: "Search 180 logins" -- a count of what is
/// actually in scope, and a noun naming that scope.
///
/// Both halves are load-bearing and neither could be hardcoded. The count is
/// the caller's, taken from `sidebar::count_for` so the placeholder and the
/// sidebar's own badge cannot disagree. The NOUN has to follow the active
/// filter: a Cards scope reading "Search 12 logins" would be a new untruth of
/// exactly the kind this window keeps having to un-write, and the design's
/// literal "logins" is only correct for the one screenshot it appears in.
///
/// A pure function so every variant can be asserted, including the two the
/// grammar breaks on: 1 (no plural "s") and 0 (which takes the plural, as
/// English does).
///
/// `Trash` and `Folder` deliberately keep the neutral "item": both hold a
/// mixture of kinds, so any specific noun would be wrong for most of their
/// contents, and the sidebar already shows which scope is selected.
pub fn search_hint(count: usize, filter: &SidebarFilter) -> String {
    let (singular, plural) = match filter {
        SidebarFilter::All => ("item", "items"),
        SidebarFilter::Favorites => ("favorite", "favorites"),
        SidebarFilter::Logins => ("login", "logins"),
        SidebarFilter::Passkeys => ("passkey", "passkeys"),
        SidebarFilter::Cards => ("card", "cards"),
        SidebarFilter::Identities => ("identity", "identities"),
        SidebarFilter::SecureNotes => ("secure note", "secure notes"),
        SidebarFilter::SshKeys => ("SSH key", "SSH keys"),
        SidebarFilter::Trash => ("item", "items"),
        SidebarFilter::Folder(_) => ("item", "items"),
    };
    format!("Search {count} {}", if count == 1 { singular } else { plural })
}

const ROW_HEIGHT: f32 = 50.0;

/// Draws the search box, `+ New` button, and the virtualized item list.
///
/// `visible_ids` is cleared at the top of this call and then filled with the
/// id of every item row actually rendered this frame (i.e. within
/// `show_rows`'s returned range) -- `vault_window::mod` uses this to know
/// which items are currently on screen so it can trigger favicon fetches for
/// exactly those, matching official Bitwarden clients' "load icons for
/// what's visible" behavior instead of only the single selected item. This
/// module stays otherwise unaware of favicons/threads/caching -- it just
/// reports what it drew.
pub fn draw_item_list(
    ui: &mut egui::Ui,
    items: &[VaultItem],
    filter: &SidebarFilter,
    search: &mut String,
    selected_id: &mut Option<String>,
    icons: &IconCache,
    visible_ids: &mut Vec<String>,
) -> ItemListAction {
    let mut action = ItemListAction::None;
    visible_ids.clear();

    // Design 4.8/2b's toolbar strip: `padding: 12px; gap: 8px; border-bottom:
    // 1px solid #eae7e7; background: #ffffff` -- a WHITE tile spanning the
    // full width of this pane, with the search box and `+ New` on it. This is
    // the "search field should be on white tile as per design" report: the
    // search box used to be drawn straight onto the pane's grey canvas with
    // no strip at all, no border, no icon and no shortcut hint.
    //
    // The strip has to reach the pane's own edges to read as a tile rather
    // than a floating card, which is why `vault_window::mod`'s panel frame
    // for this pane carries NO inner margin and the padding is applied here
    // instead -- 12 for this header, 10 for the list below it (design 2b's
    // two different paddings, which a single panel margin could not express).
    egui::Frame::new()
        .fill(theme::CARD)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                // `+ New` is added FIRST, right-to-left, so the search box
                // gets `flex: 1`: laid out left-to-right the button would
                // have to be given a width up front, and the search box the
                // remainder minus a guess at it -- which is exactly the
                // `available_width() - 70.0` guess this replaces.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary_button_matching_field(ui, "+ New").clicked() {
                        action = ItemListAction::NewItem;
                    }
                    // "Search 180 logins" -- see `search_hint`, which owns
                    // both the count's source and the per-filter noun.
                    let hint = search_hint(super::sidebar::count_for(items, filter), filter);
                    theme::search_field(
                        ui,
                        search,
                        &hint,
                        "CTRL+K",
                        // Stable id so `Ctrl+K` (wired in
                        // `vault_window::mod`) can request focus on this
                        // field from outside this function. MUST NOT CHANGE.
                        egui::Id::new("vault-search"),
                    );
                });
            });
        });
    // The design's `border-bottom` under the strip. Painted here rather than
    // as a `Frame` stroke so it is only on the bottom edge -- a full-box
    // stroke would draw a second line down the pane's own right edge, on top
    // of `Panel`'s separator, which is the exact doubling the sidebar's frame
    // comment warns about.
    let strip_bottom = ui.min_rect().bottom();
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::Pos2::new(ui.min_rect().left(), strip_bottom - 1.0),
            egui::Pos2::new(ui.min_rect().right(), strip_bottom),
        ),
        CornerRadius::ZERO,
        theme::HAIRLINE,
    );

    let search_lower = search.to_lowercase();
    let filtered: Vec<&VaultItem> = items
        .iter()
        .filter(|item| matches_filter(item, filter, &search_lower))
        .collect();

    // Design 2b's list padding (`padding: 10px`), applied here now that the
    // pane's panel frame has none -- see the header strip's comment above.
    egui::Frame::new()
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, ROW_HEIGHT, filtered.len(), |ui, row_range| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for row in row_range {
                        let item = filtered[row];
                        visible_ids.push(item.id.clone());
                        let selected = selected_id.as_deref() == Some(item.id.as_str());
                        if item_row(ui, item, selected, icons.textures.get(&item.id)) {
                            *selected_id = Some(item.id.clone());
                        }
                    }
                });
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
                        // Rounded to match `theme::avatar`'s initials-tile
                        // treatment (same `size * 0.25` formula) -- a sharp-
                        // cornered square in the identical box read as
                        // visually heavier/bigger than the monogram fallback
                        // even at the same pixel dimensions.
                        const SIZE: f32 = 32.0;
                        ui.add(
                            egui::Image::new((tex.id(), tex.size_vec2()))
                                .fit_to_exact_size(egui::Vec2::splat(SIZE))
                                .corner_radius(CornerRadius::same((SIZE * 0.25) as u8)),
                        );
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
    let response = frame.response.interact(Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
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
            card: None,
            identity: None,
            notes: None,
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

#[cfg(test)]
mod search_hint_tests {
    use super::{search_hint, SidebarFilter};

    #[test]
    fn the_noun_follows_the_active_filter() {
        // The whole reason this is a function. Hardcoding the design's
        // "logins" would put "Search 12 logins" over a list of cards.
        assert_eq!(search_hint(180, &SidebarFilter::Logins), "Search 180 logins");
        assert_eq!(search_hint(4, &SidebarFilter::Cards), "Search 4 cards");
        assert_eq!(search_hint(21, &SidebarFilter::SecureNotes), "Search 21 secure notes");
        assert_eq!(search_hint(9, &SidebarFilter::Passkeys), "Search 9 passkeys");
        assert_eq!(search_hint(12, &SidebarFilter::Favorites), "Search 12 favorites");
        assert_eq!(search_hint(3, &SidebarFilter::Identities), "Search 3 identities");
        assert_eq!(search_hint(2, &SidebarFilter::SshKeys), "Search 2 SSH keys");
        assert_eq!(search_hint(214, &SidebarFilter::All), "Search 214 items");
    }

    #[test]
    fn the_mixed_scopes_keep_the_neutral_noun() {
        // Trash and a folder both hold a mixture of kinds, so any specific
        // noun is wrong for most of what is in them.
        assert_eq!(search_hint(6, &SidebarFilter::Trash), "Search 6 items");
        assert_eq!(
            search_hint(64, &SidebarFilter::Folder("f-1".to_string())),
            "Search 64 items"
        );
    }

    #[test]
    fn one_item_is_singular_in_every_scope() {
        // The case a naive `format!("{n} {plural}")` gets wrong, and the one
        // a user with a small vault sees constantly.
        assert_eq!(search_hint(1, &SidebarFilter::Logins), "Search 1 login");
        assert_eq!(search_hint(1, &SidebarFilter::Identities), "Search 1 identity");
        assert_eq!(search_hint(1, &SidebarFilter::SshKeys), "Search 1 SSH key");
        assert_eq!(search_hint(1, &SidebarFilter::SecureNotes), "Search 1 secure note");
        assert_eq!(search_hint(1, &SidebarFilter::All), "Search 1 item");
    }

    #[test]
    fn zero_takes_the_plural_the_way_english_does() {
        // "Search 0 login" is the other half of the same off-by-one.
        assert_eq!(search_hint(0, &SidebarFilter::Logins), "Search 0 logins");
        assert_eq!(search_hint(0, &SidebarFilter::All), "Search 0 items");
    }
}

#[cfg(test)]
mod toolbar_strip_tests {
    //! The user-reported defect: "Search field should be on white tile as per
    //! design".
    //!
    //! Design 2b puts the search box and `+ New` in a strip with
    //! `background: #ffffff` spanning the whole item pane, and gives the box
    //! a border, a magnifier and a `CTRL+K` hint. The implementation drew a
    //! bare `TextEdit` on the pane's grey canvas. These drive real frames of
    //! `draw_item_list` and read back what was actually painted, because
    //! every part of that is invisible to a unit test over `matches_filter`
    //! -- which is the only thing this module used to have.
    use super::*;
    use crate::theme;

    const PANE_WIDTH: f32 = 390.0;

    fn an_item(name: &str) -> VaultItem {
        VaultItem {
            id: name.to_string(),
            name: name.into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// One real frame of `draw_item_list` at the item pane's own width.
    /// `before_frame` runs between the settling frames and the measured one,
    /// which is how the Ctrl+K test asks for focus the way `vault_window::run`
    /// does.
    fn run_item_list(
        items: &[VaultItem],
        search: &mut String,
        before_frame: impl FnOnce(&egui::Context),
    ) -> (egui::FullOutput, egui::Context) {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(PANE_WIDTH, 700.0),
            )),
            ..Default::default()
        };
        // Two throwaway frames so `theme::apply`'s font set is live -- the
        // same reason `detail.rs`'s `painted_text` harness runs them.
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut selected = None;
        let icons = IconCache::default();
        let mut visible = Vec::new();
        let mut draw = |ctx: &egui::Context, search: &mut String| {
            ctx.run_ui(input(), |ui| {
                draw_item_list(
                    ui,
                    items,
                    &SidebarFilter::All,
                    search,
                    &mut selected,
                    &icons,
                    &mut visible,
                );
            })
        };
        let _ = draw(&ctx, search);
        before_frame(&ctx);
        let output = draw(&ctx, search);
        (output, ctx)
    }

    fn collect_text(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.text().to_string(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Every filled rectangle painted in `fill`, in paint order.
    fn collect_fills(shape: &egui::Shape, fill: egui::Color32, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.fill == fill => out.push(rect.rect),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_fills(shape, fill, out);
                }
            }
            _ => {}
        }
    }

    fn painted(output: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut texts);
        }
        texts
    }

    fn fills(output: &egui::FullOutput, fill: egui::Color32) -> Vec<egui::Rect> {
        let mut rects = Vec::new();
        for clipped in &output.shapes {
            collect_fills(&clipped.shape, fill, &mut rects);
        }
        rects
    }

    #[test]
    fn the_search_box_and_new_button_sit_on_a_white_tile_spanning_the_pane() {
        // The report, stated as an assertion. Two things have to be true and
        // neither was: there IS a white tile, and it reaches BOTH edges of
        // the pane. A tile inset by a panel margin reads as a card floating
        // on grey, which is what the design does not draw.
        let mut search = String::new();
        let (output, _) = run_item_list(&[an_item("Ledgerline")], &mut search, |_| {});
        let texts = painted(&output);

        let new_button = texts
            .iter()
            .find(|(t, _)| t.contains("New"))
            .unwrap_or_else(|| panic!("nothing resembling a New button was painted: {texts:?}"));
        let shortcut = texts
            .iter()
            .find(|(t, _)| t == "CTRL+K")
            .unwrap_or_else(|| panic!("no CTRL+K hint in the search box: {texts:?}"));

        let tile = fills(&output, theme::CARD)
            .into_iter()
            // The search box's own interior is also `CARD`; the strip is the
            // one that spans the pane.
            .find(|r| r.width() >= PANE_WIDTH - 0.5)
            .unwrap_or_else(|| {
                panic!("no white fill spans the pane -- the search field is not on a tile")
            });
        assert!(tile.min.y <= 0.5, "the tile must start at the top of the pane, not float: {tile:?}");
        for (label, rect) in [("+ New", new_button.1), ("CTRL+K", shortcut.1)] {
            assert!(
                tile.contains_rect(rect),
                "{label} at {rect:?} is outside the white tile {tile:?} -- it is being drawn on \
                 the pane's grey canvas, which is the reported defect"
            );
        }
    }

    #[test]
    fn the_search_box_shows_the_designs_hint_and_shortcut() {
        // The two things the old bare `TextEdit` had no way to show. The
        // count is the design's ("Search 180 logins"), taken from the filter's
        // own scope so it cannot drift from the sidebar's badge.
        let mut search = String::new();
        let (output, _) = run_item_list(
            &[an_item("Ledgerline"), an_item("Atlas"), an_item("Vantage")],
            &mut search,
            |_| {},
        );
        let texts: Vec<String> = painted(&output).into_iter().map(|(t, _)| t).collect();
        assert!(
            texts.iter().any(|t| t == "Search 3 items"),
            "the hint must count what is in scope; painted: {texts:?}"
        );
        assert!(texts.iter().any(|t| t == "CTRL+K"), "painted: {texts:?}");
    }

    #[test]
    fn a_single_item_in_scope_is_not_described_as_1_items() {
        let mut search = String::new();
        let (output, _) = run_item_list(&[an_item("Ledgerline")], &mut search, |_| {});
        let texts: Vec<String> = painted(&output).into_iter().map(|(t, _)| t).collect();
        assert!(texts.iter().any(|t| t == "Search 1 item"), "painted: {texts:?}");
    }

    #[test]
    fn a_typed_query_replaces_the_hint_rather_than_sitting_beside_it() {
        // `hint_text` only shows while the field is empty; this is what says
        // the string above really is the hint and not a label painted next
        // to the box forever.
        let mut search = "ledger".to_string();
        let (output, _) = run_item_list(&[an_item("Ledgerline")], &mut search, |_| {});
        let texts: Vec<String> = painted(&output).into_iter().map(|(t, _)| t).collect();
        assert!(texts.iter().any(|t| t == "ledger"), "painted: {texts:?}");
        assert!(!texts.iter().any(|t| t.starts_with("Search ")), "painted: {texts:?}");
    }

    #[test]
    fn ctrl_k_still_focuses_the_search_field_after_the_move() {
        // THE ONE THING THE REDESIGN COULD BREAK SILENTLY. `vault_window::run`
        // focuses this field by id, from outside this function, with exactly
        // the call below. Moving the field into a new `Frame`, a new layout,
        // and a hand-painted box would compile and look right while the
        // shortcut quietly stopped working -- the id is now handed to
        // `theme::search_field`, which passes it to the `TextEdit`, and
        // nothing but this test says it arrives.
        let mut search = String::new();
        let id = egui::Id::new("vault-search");
        let (_, ctx) = run_item_list(&[an_item("Ledgerline")], &mut search, |ctx| {
            ctx.memory_mut(|m| m.request_focus(id));
        });
        assert!(
            ctx.memory(|m| m.has_focus(id)),
            "Ctrl+K's `request_focus(Id::new(\"vault-search\"))` no longer lands on the search \
             field -- the id was renamed, or it is no longer the TextEdit's own id"
        );
    }

    #[test]
    fn the_item_rows_are_still_below_the_tile_and_not_under_it() {
        // The strip is drawn before the list and both are in the same pane;
        // getting the order or the padding wrong hides the first row behind
        // the tile rather than producing any error.
        let mut search = String::new();
        let (output, _) = run_item_list(&[an_item("Ledgerline")], &mut search, |_| {});
        let texts = painted(&output);
        let tile = fills(&output, theme::CARD)
            .into_iter()
            .find(|r| r.width() >= PANE_WIDTH - 0.5)
            .expect("the white tile");
        let row = texts
            .iter()
            .find(|(t, _)| t == "Ledgerline")
            .expect("the item row's name")
            .1;
        assert!(
            row.min.y >= tile.max.y,
            "the first item row at {row:?} overlaps the toolbar tile {tile:?}"
        );
    }
}

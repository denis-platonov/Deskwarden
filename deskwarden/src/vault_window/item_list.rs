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
/// `Trash`, `Folder` and `Unfiled` deliberately keep the neutral "item": all
/// three hold a mixture of kinds, so any specific noun would be wrong for
/// most of their contents, and the sidebar already shows which scope is
/// selected.
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
        SidebarFilter::Unfiled => ("item", "items"),
    };
    format!("Search {count} {}", if count == 1 { singular } else { plural })
}

/// The design's avatar/favicon tile: `width: 32px; height: 32px`.
const AVATAR_SIZE: f32 = 32.0;

/// Design 2b's row box, in full: `padding: 10px 12px` around a 32px avatar,
/// plus the 1px border every row carries. egui's `Frame` sizes itself
/// `content + inner_margin + 2 * stroke.width`, which is the same box CSS's
/// content-box model produces -- 32 + 10 + 10 + 1 + 1.
///
/// This is what `ScrollArea::show_rows` is virtualized against, so it has to
/// be the height the rows really paint at; `consecutive_row_tiles_sit_exactly_
/// one_design_gap_apart_and_span_the_pane` asserts that from painted output
/// rather than trusting the arithmetic above.
const ROW_TILE_HEIGHT: f32 = AVATAR_SIZE + 2.0 * ROW_PAD_Y + 2.0 * ROW_BORDER;
const ROW_PAD_Y: f32 = 10.0;
const ROW_PAD_X: f32 = 12.0;
const ROW_BORDER: f32 = 1.0;
/// The row's `gap: 11px`, between the avatar, the title column and the badge.
const ROW_GAP_X: f32 = 11.0;
/// The list container's `gap: 6px`.
const ROW_GAP: f32 = 6.0;
/// The list container's `padding: 10px`.
const LIST_PADDING: f32 = 10.0;

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

    // Design 2b's list container butts STRAIGHT against the header strip:
    // there is no gap between the two, and the list's own `padding: 10px` is
    // the only space above the first tile. egui would otherwise insert its
    // ambient `item_spacing.y` between the two frames -- 8, from
    // `theme::apply` -- putting 18pt above the first tile against 10 at the
    // sides, which is the reported defect.
    //
    // Zeroed HERE, before the strip is drawn, and not between the two frames:
    // egui's placer advances its cursor by `rect + item_spacing` as each
    // widget is ALLOCATED, so by the time the strip has been shown the gap is
    // already committed and a later change to the spacing cannot retract it.
    //
    // This is the only vertical spacing this function relies on egui for --
    // the strip's padding, the list's padding and the rows' `gap: 6px` are
    // all set explicitly -- so zeroing it costs nothing else. The list frame
    // below re-sets `item_spacing.y` to `ROW_GAP` on its own ui before
    // `show_rows` reads it, so the scroll pitch is untouched by this.
    ui.spacing_mut().item_spacing.y = 0.0;

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
    //
    // The RIGHT padding is 0 because the scroll bar is given that lane
    // instead: see `theme::scrollbar_in_gutter` below, which reserves exactly
    // `LIST_PADDING` for itself. The row tiles therefore still end at
    // `pane_right - LIST_PADDING`, unchanged, and the bar is centred in the
    // padding rather than drawn hard against the tiles.
    egui::Frame::new()
        .inner_margin(Margin {
            left: LIST_PADDING as i8,
            right: 0,
            top: LIST_PADDING as i8,
            bottom: LIST_PADDING as i8,
        })
        .show(ui, |ui| {
            // The design's `gap: 6px`, set on THIS ui rather than inside the
            // closure below. `show_rows` reads `item_spacing.y` from the ui it
            // is given, BEFORE the closure runs, and virtualizes against
            // `row_height + that spacing` -- a gap set inside the closure
            // changes where the rows actually paint while leaving the scroll
            // maths on the old pitch, which puts the list out of register
            // with its own scrollbar.
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            theme::scrollbar_in_gutter(ui, LIST_PADDING);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                // Required by `scrollbar_in_gutter`: the reserved lane is
                // what keeps the tiles one width, and egui only reserves it
                // for a bar it is actually showing.
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
                .show_rows(ui, ROW_TILE_HEIGHT, filtered.len(), |ui, row_range| {
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

/// Design 2b's trailing row chip: `font-size: 10px; border-radius: 5px;
/// padding: 2px 6px`, `#605d5d on #f3f2f2` unselected and `#14307a on
/// #dbe4f7` selected.
///
/// Not [`theme::kbd_chip`]: that one is a fixed-height MONOSPACE keyboard
/// hint (10px in an 18px box, radius 4), which is a different element of the
/// design that happens to be a similar size. Kept private here rather than
/// added to `theme` because the badge exists in exactly one place and its
/// colours all come from constants `theme` already owns.
fn row_badge(ui: &mut egui::Ui, text: &str, selected: bool) {
    const PAD_X: f32 = 6.0;
    const PAD_Y: f32 = 2.0;
    let (bg, fg) = if selected {
        // `#dbe4f7`. The design uses one value for the focus halo and for
        // this chip; see `theme::FOCUS_RING`.
        (theme::FOCUS_RING, theme::BLUE_DEEP)
    } else {
        (theme::CANVAS, theme::TEXT_MUTED)
    };
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::new(10.0, egui::FontFamily::Proportional),
        fg,
    );
    let (rect, _) = ui.allocate_exact_size(
        galley.size() + egui::Vec2::new(PAD_X * 2.0, PAD_Y * 2.0),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, CornerRadius::same(5), bg);
    ui.painter()
        .galley(rect.min + egui::Vec2::new(PAD_X, PAD_Y), galley, fg);
}

fn item_row(
    ui: &mut egui::Ui,
    item: &VaultItem,
    selected: bool,
    icon: Option<&egui::TextureHandle>,
) -> bool {
    let username = item.login.as_ref().and_then(|l| l.username.as_deref()).unwrap_or("");
    // The design's "app" chip. It is not decorative and it is not invented:
    // `deskwarden:app-match` is the custom field that makes an item fillable
    // into a native window, and `extract_app_match` answers it from the item
    // already in hand -- no extra lookup, and only for the handful of rows
    // `show_rows` actually hands us.
    let badged = crate::vault_bridge::extract_app_match(item).is_some();
    let frame = egui::Frame::new()
        // Design 2b: EVERY row is `background: #ffffff`, selected or not.
        // Filling unselected rows with the pane's own `CANVAS` is what made
        // them read as flat bands instead of tiles -- the reported defect.
        .fill(theme::CARD)
        .stroke(Stroke::new(
            ROW_BORDER,
            if selected { theme::BLUE } else { theme::HAIRLINE },
        ))
        // `box-shadow: 0 1px 2px rgba(45, 43, 43, 0.06)`, selected only.
        .shadow(if selected { SELECTED_ROW_SHADOW } else { egui::Shadow::NONE })
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(ROW_PAD_X as i8, ROW_PAD_Y as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ROW_GAP_X;
                match icon {
                    Some(tex) => {
                        // Rounded to match `theme::avatar`'s initials-tile
                        // treatment (same `size * 0.25` formula) -- a sharp-
                        // cornered square in the identical box read as
                        // visually heavier/bigger than the monogram fallback
                        // even at the same pixel dimensions.
                        ui.add(
                            egui::Image::new((tex.id(), tex.size_vec2()))
                                .fit_to_exact_size(egui::Vec2::splat(AVATAR_SIZE))
                                .corner_radius(CornerRadius::same((AVATAR_SIZE * 0.25) as u8)),
                        );
                    }
                    None => theme::avatar(ui, &theme::initials(&item.name), AVATAR_SIZE, selected),
                }
                // The design's title column is `flex: 1` with the badge
                // trailing it. Laid out right-to-left so the badge takes its
                // own width off the right edge and the column gets the
                // remainder -- the same trick the toolbar strip above uses
                // for `+ New` and the search field, and the reason neither
                // needs a guessed width.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if badged {
                        row_badge(ui, "app", selected);
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let title = if selected {
                            // `font-weight: 700; color: #14307a`.
                            theme::bold(&item.name, 13.0).color(theme::BLUE_DEEP)
                        } else {
                            theme::semibold(&item.name, 13.0).color(theme::INK)
                        };
                        // Truncated, not wrapped: a name long enough to wrap
                        // ("Remote Desktop — Bastion" is already close) would
                        // make one row taller than every other and slide the
                        // whole virtualized list out of register with the
                        // fixed pitch `show_rows` scrolls by.
                        ui.add(egui::Label::new(title).truncate());
                        if !username.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(username).size(11.0).color(theme::TEXT_FAINT),
                                )
                                .truncate(),
                            );
                        }
                    });
                });
            });
        });
    let response = frame.response.interact(Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// `box-shadow: 0 1px 2px rgba(45, 43, 43, 0.06)` -- the design's selected
/// row. Alpha is `0.06 * 255`, rounded.
const SELECTED_ROW_SHADOW: egui::Shadow = egui::Shadow {
    offset: [0, 1],
    blur: 2,
    spread: 0,
    color: egui::Color32::from_rgba_unmultiplied_const(45, 43, 43, 15),
};

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
mod row_tile_tests {
    //! The user-reported defect: "those result set tiles should have white
    //! color -- check win design as well".
    //!
    //! Design 2b (the WINDOWS vault window -- its shortcut hints read
    //! `CTRL+K`/`CTRL+L` and it draws the —/▢/✕ window controls, unlike the
    //! macOS `3f` block) draws EVERY item row as a white tile:
    //! `background: #ffffff; border-radius: 10px; padding: 10px 12px;
    //! gap: 11px`, `border: 1px solid #eae7e7` unselected and
    //! `1px solid #1b3fa0` selected. The implementation filled unselected rows
    //! with the pane's own grey canvas and gave them no border at all, so they
    //! read as flat bands rather than tiles -- exactly the report.
    //!
    //! These drive real frames of `draw_item_list` and read back the painted
    //! `RectShape`s and galleys, so fills, stroke colours, corner radii and
    //! geometry are all asserted from what egui actually emitted.
    use super::*;
    use crate::app_match::{AppMatch, TriggerMode, APP_MATCH_FIELD_NAME};
    use crate::theme;
    use crate::vault_bridge::VaultField;
    use eframe::egui::epaint::RectShape;

    const PANE_WIDTH: f32 = 390.0;
    const PANE_HEIGHT: f32 = 700.0;
    /// A row tile spans the pane minus the list frame's `padding: 10px`.
    const TILE_WIDTH: f32 = PANE_WIDTH - 2.0 * LIST_PADDING;

    fn login(name: &str, username: &str) -> VaultItem {
        VaultItem {
            id: name.to_string(),
            name: name.into(),
            fields: vec![],
            login: Some(crate::vault_bridge::LoginData {
                username: Some(username.to_string()),
                password: None,
                totp: None,
                uris: vec![],
                other: serde_json::Map::new(),
            }),
            card: None,
            identity: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// The same custom field `app::save_app_match` writes and
    /// `vault_bridge::extract_app_match` reads -- built through `AppMatch`'s
    /// own serializer so this cannot drift from the real format.
    fn with_app_match(mut item: VaultItem) -> VaultItem {
        item.fields.push(VaultField {
            name: Some(APP_MATCH_FIELD_NAME.to_string()),
            value: Some(
                AppMatch {
                    process: "ledgerline.exe".to_string(),
                    trigger: TriggerMode::Prompt,
                }
                .to_field_value(),
            ),
            other: serde_json::Map::new(),
        });
        item
    }

    struct Painted {
        rects: Vec<RectShape>,
        texts: Vec<(String, egui::Rect, egui::Color32)>,
        fonts: Vec<(String, egui::FontId)>,
        visible: Vec<String>,
    }

    fn walk(shape: &egui::Shape, p: &mut Painted) {
        match shape {
            egui::Shape::Rect(rect) => p.rects.push(rect.clone()),
            egui::Shape::Text(text) => {
                if let Some(section) = text.galley.job.sections.first() {
                    p.fonts
                        .push((text.galley.text().to_string(), section.format.font_id.clone()));
                }
                // The colour egui will actually render with: an explicit
                // override wins, then the layout job's own section colour,
                // and only an unset (`PLACEHOLDER`) section falls back.
                let color = text
                    .override_text_color
                    .or_else(|| {
                        text.galley
                            .job
                            .sections
                            .first()
                            .map(|s| s.format.color)
                            .filter(|c| *c != egui::Color32::PLACEHOLDER)
                    })
                    .unwrap_or(text.fallback_color);
                p.texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                    color,
                ));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, p);
                }
            }
            _ => {}
        }
    }

    /// One real frame of `draw_item_list` at the item pane's own width, with
    /// `selected` already chosen, returning everything it painted.
    fn paint(items: &[VaultItem], selected: Option<&str>) -> Painted {
        paint_with(items, selected, 0)
    }

    /// One real frame of `draw_item_list` at the item pane's own width, with
    /// `selected` already chosen, returning everything it painted.
    ///
    /// `wheel_frames` frames of downward mouse wheel are pumped in first (and
    /// then settled), which is how the scrolled test drives the list to its
    /// end without reaching into `ScrollArea`'s private state.
    fn paint_with(items: &[VaultItem], selected: Option<&str>, wheel_frames: usize) -> Painted {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(PANE_WIDTH, PANE_HEIGHT),
        );
        let input = || egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        // Two throwaway frames so `theme::apply`'s font set is live -- the
        // same reason `detail.rs`'s and `sidebar.rs`'s harnesses run them.
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut selected_id = selected.map(str::to_string);
        let mut search = String::new();
        let icons = IconCache::default();
        let mut draw = |ctx: &egui::Context, input: egui::RawInput, visible: &mut Vec<String>| {
            ctx.run_ui(input, |ui| {
                draw_item_list(
                    ui,
                    items,
                    &SidebarFilter::All,
                    &mut search,
                    &mut selected_id,
                    &icons,
                    visible,
                );
            })
        };
        let mut visible = Vec::new();
        let _ = draw(&ctx, input(), &mut visible);
        for frame in 0..wheel_frames {
            let mut raw = input();
            raw.events.push(egui::Event::PointerMoved(screen.center()));
            // Only the first half of the frames scroll; the rest let egui's
            // scroll smoothing settle, so the measured frame is at rest.
            if frame < wheel_frames / 2 {
                raw.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -2000.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let _ = draw(&ctx, raw, &mut visible);
        }
        let output = draw(&ctx, input(), &mut visible);

        let mut painted = Painted {
            rects: Vec::new(),
            texts: Vec::new(),
            fonts: Vec::new(),
            visible,
        };
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    /// The row tiles, in paint order: full-width boxes exactly one row high.
    /// Deliberately keyed on GEOMETRY rather than on fill, so a test that
    /// asserts the fill cannot be the thing that found the rect. The
    /// selected row's drop shadow occupies the same box, so it is excluded by
    /// its blur -- and asserted on separately.
    fn row_tiles(p: &Painted) -> Vec<RectShape> {
        p.rects
            .iter()
            .filter(|r| {
                // A rect that paints neither a fill nor a stroke is not a
                // tile -- egui emits one per row for the layout itself.
                !(r.fill == egui::Color32::TRANSPARENT && r.stroke.width == 0.0)
                    && r.blur_width == 0.0
                    && (r.rect.width() - TILE_WIDTH).abs() < 0.5
                    && (r.rect.height() - ROW_TILE_HEIGHT).abs() < 0.5
            })
            .cloned()
            .collect()
    }

    fn one_tile(p: &Painted) -> RectShape {
        let tiles = row_tiles(p);
        assert_eq!(
            tiles.len(),
            1,
            "expected exactly one {TILE_WIDTH}x{ROW_TILE_HEIGHT} row tile; every painted rect \
             was: {:?}",
            p.rects.iter().map(|r| (r.rect, r.fill)).collect::<Vec<_>>()
        );
        tiles[0].clone()
    }

    /// A square of exactly `size`, by geometry alone -- the avatar tile.
    fn square(p: &Painted, size: f32) -> RectShape {
        p.rects
            .iter()
            .find(|r| {
                (r.rect.width() - size).abs() < 0.5 && (r.rect.height() - size).abs() < 0.5
            })
            .cloned()
            .unwrap_or_else(|| panic!("no {size}x{size} tile was painted"))
    }

    fn text_color(p: &Painted, needle: &str) -> egui::Color32 {
        p.texts
            .iter()
            .find(|(t, _, _)| t == needle)
            .unwrap_or_else(|| panic!("{needle:?} was never painted; painted: {:?}", p.texts))
            .2
    }

    /// The `FontId` a painted string was laid out with -- size and family,
    /// i.e. the design's `font-size` and `font-weight`.
    fn text_font(p: &Painted, needle: &str) -> egui::FontId {
        p.fonts
            .iter()
            .find(|(t, _)| t == needle)
            .unwrap_or_else(|| panic!("{needle:?} was never painted; painted: {:?}", p.texts))
            .1
            .clone()
    }

    /// Design 2b's header strip: `padding: 12px` around a `height: 34px`
    /// search box, i.e. 12 + 34 + 12. Written out rather than derived from
    /// `theme::SEARCH_FIELD_HEIGHT` so this stays an INDEPENDENT statement of
    /// the geometry -- a test that recomputed the strip from the same
    /// constants the code uses would stay green if both moved together.
    const STRIP_HEIGHT: f32 = 58.0;

    #[test]
    fn the_first_tile_sits_exactly_the_lists_own_padding_below_the_header_strip() {
        // THE REPORT: "top padding above the first tile is too big; should
        // match left/right". Design 2b's list container butts straight against
        // the header strip and carries `padding: 10px` of its own, so at this
        // pane the first tile's top edge is at 58 + 10 = 68 and its left edge
        // at 10 -- the SAME 10.
        //
        // ABSOLUTE on both axes against a pinned pane geometry, deliberately:
        // asserting `top - strip_bottom == left` would have stayed green while
        // egui's ambient `item_spacing.y` (8, from `theme::apply`) pushed the
        // whole list down, because both sides are measured off the same list.
        let p = paint(&[login("Ledgerline", "a.novak@ledgerline.com")], None);
        let tile = one_tile(&p);
        assert!(
            (tile.rect.top() - (STRIP_HEIGHT + LIST_PADDING)).abs() < 0.5,
            "the first row tile's top edge is at y={}, expected {} (the {STRIP_HEIGHT}pt header \
             strip plus the list's own {LIST_PADDING}pt padding and NOTHING else -- egui's \
             ambient item_spacing is sitting between the strip and the list)",
            tile.rect.top(),
            STRIP_HEIGHT + LIST_PADDING
        );
        assert!(
            (tile.rect.left() - LIST_PADDING).abs() < 0.5,
            "the first row tile's left edge is at x={}, expected {LIST_PADDING}",
            tile.rect.left()
        );
    }

    #[test]
    fn the_scrollbar_is_centred_in_the_lists_right_padding_and_the_tiles_keep_their_width() {
        // THE REPORT: "the scrollbar sits against the tiles' right edge;
        // centre it in the right padding".
        //
        // The gutter is the list's own `padding: 10px` on the right, i.e.
        // x in [380, 390] at this pane. A 6pt bar centred in it occupies
        // [382, 388]. ABSOLUTE numbers, not "the bar is right of the tiles".
        //
        // Paired with the tile geometry, which must NOT move: giving the
        // gutter to the scroll area only works if the bar takes its space
        // from the content, so the tiles still span 10..380.
        const GUTTER: std::ops::Range<f32> = 380.0..390.0;
        let items: Vec<VaultItem> = (0..40)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let p = paint(&items, None);

        for tile in row_tiles(&p) {
            assert!(
                (tile.rect.left() - LIST_PADDING).abs() < 0.5
                    && (tile.rect.right() - (PANE_WIDTH - LIST_PADDING)).abs() < 0.5,
                "a row tile spans {}..{}, expected {LIST_PADDING}..{} -- the tiles must keep \
                 their exact width when the scrollbar is given the gutter",
                tile.rect.left(),
                tile.rect.right(),
                PANE_WIDTH - LIST_PADDING
            );
        }

        // The scroll bar's own two rects (track and handle) are the only
        // things painted in the gutter at all. Found by geometry -- they are
        // the rects that lie strictly right of the tiles.
        let in_gutter: Vec<egui::Rect> = p
            .rects
            .iter()
            .map(|r| r.rect)
            .filter(|r| r.right() > PANE_WIDTH - LIST_PADDING + 0.5 && r.width() < LIST_PADDING)
            .collect();
        assert!(
            !in_gutter.is_empty(),
            "nothing at all was painted in the list's right padding, so there is no scrollbar \
             there to centre; painted: {:?}",
            p.rects.iter().map(|r| r.rect).collect::<Vec<_>>()
        );
        for bar in &in_gutter {
            assert!(
                bar.left() >= GUTTER.start - 0.01 && bar.right() <= GUTTER.end + 0.01,
                "the scrollbar spans x={}..{}, which leaves the {GUTTER:?} gutter -- it is being \
                 drawn over the tiles",
                bar.left(),
                bar.right()
            );
            let slack_left = bar.left() - GUTTER.start;
            let slack_right = GUTTER.end - bar.right();
            assert!(
                (slack_left - slack_right).abs() < 0.51,
                "the scrollbar has {slack_left}pt of gutter to its left and {slack_right}pt to \
                 its right -- it is not centred"
            );
        }
    }

    #[test]
    fn an_unselected_row_is_a_white_tile_with_the_designs_hairline_border() {
        // THE REPORT, stated as an assertion. `background: #ffffff` on EVERY
        // row -- the implementation filled unselected rows with `CANVAS`,
        // the pane's own grey, which is why they did not read as tiles.
        let p = paint(&[login("Atlas Studio", "a.novak@studio.atlas.com")], None);
        let tile = one_tile(&p);
        assert_eq!(
            tile.fill,
            theme::CARD,
            "design 2b fills every row with #ffffff; this row is filled with {:?}",
            tile.fill
        );
        assert_eq!(
            tile.stroke.color,
            theme::HAIRLINE,
            "an unselected row's border is `1px solid #eae7e7`"
        );
        assert!(
            (tile.stroke.width - 1.0).abs() < 0.01,
            "border width {} , expected 1",
            tile.stroke.width
        );
        assert_eq!(
            tile.corner_radius,
            CornerRadius::same(10),
            "design 2b's rows are `border-radius: 10px`"
        );
    }

    #[test]
    fn a_selected_row_is_white_too_and_differs_only_in_its_blue_border() {
        // The half of the design that WAS implemented (selected rows were
        // already white) must survive the fix, and the difference between the
        // two states must be the border colour -- not the fill.
        let items = [login("Ledgerline", "a.novak@ledgerline.com")];
        let selected = one_tile(&paint(&items, Some("Ledgerline")));
        let unselected = one_tile(&paint(&items, None));
        assert_eq!(selected.fill, theme::CARD);
        assert_eq!(unselected.fill, theme::CARD);
        assert_eq!(
            selected.stroke.color,
            theme::BLUE,
            "a selected row's border is `1px solid #1b3fa0`"
        );
        assert_ne!(
            selected.stroke.color, unselected.stroke.color,
            "selected and unselected rows must be distinguishable"
        );
    }

    #[test]
    fn the_selected_rows_title_is_the_designs_deep_blue_and_the_unselected_ones_is_ink() {
        // `font-size: 13px; font-weight: 700; color: #14307a` when selected,
        // the default ink at 600 otherwise. Absolute on both sides: asserting
        // only that they differ would stay green if both drifted together.
        let items = [login("Ledgerline", "a.novak@ledgerline.com")];
        let selected = paint(&items, Some("Ledgerline"));
        let unselected = paint(&items, None);
        assert_eq!(text_color(&selected, "Ledgerline"), theme::BLUE_DEEP);
        assert_eq!(text_color(&unselected, "Ledgerline"), theme::INK);
        // ...and the WEIGHT, which the colours alone would not have pinned:
        // 700 selected, 600 otherwise, both at 13px.
        for (state, font, family) in [
            (&selected, text_font(&selected, "Ledgerline"), theme::BOLD),
            (
                &unselected,
                text_font(&unselected, "Ledgerline"),
                theme::SEMIBOLD,
            ),
        ] {
            let _ = state;
            assert_eq!(font.size, 13.0, "the design's `font-size: 13px`");
            assert_eq!(
                font.family,
                egui::FontFamily::Name(family.into()),
                "expected the {family} face"
            );
        }
    }

    #[test]
    fn the_subtitle_is_the_designs_faint_grey_in_both_states() {
        let items = [login("Ledgerline", "a.novak@ledgerline.com")];
        for selected in [None, Some("Ledgerline")] {
            assert_eq!(
                text_color(&paint(&items, selected), "a.novak@ledgerline.com"),
                theme::TEXT_FAINT,
                "the subtitle is `font-size: 11px; color: #7d7979` regardless of selection"
            );
        }
    }

    #[test]
    fn the_avatar_tile_is_actually_filled_and_bordered_in_both_states() {
        // "the tile is actually filled rather than transparent", asserted on
        // the 32x32 monogram box: `background: #f3f2f2; border: 1px solid
        // #eae7e7` unselected, `#eef2fc` / `#b8c7ea` selected.
        let items = [login("Ledgerline", "a.novak@ledgerline.com")];
        let unselected = square(&paint(&items, None), 32.0);
        assert_eq!(unselected.fill, theme::CANVAS);
        let selected = square(&paint(&items, Some("Ledgerline")), 32.0);
        assert_eq!(selected.fill, theme::BLUE_WASH);
        assert_ne!(selected.fill, egui::Color32::TRANSPARENT);
    }

    #[test]
    fn consecutive_row_tiles_sit_exactly_one_design_gap_apart_and_span_the_pane() {
        // ABSOLUTE geometry, not "row 2 is below row 1": the list is
        // `padding: 10px; gap: 6px`, so tiles start at x=10, are
        // 390-20 wide, and leave exactly 6 between them. This is also what
        // pins `ROW_TILE_HEIGHT + ROW_GAP` to the pitch `show_rows`
        // virtualizes against -- if the painted rows and that pitch disagree,
        // the list scrolls out of register.
        let items = [
            login("Ledgerline", "a.novak@ledgerline.com"),
            login("Atlas Studio", "a.novak@studio.atlas.com"),
            login("Vantage VPN", "a.novak@vantage.io"),
        ];
        let p = paint(&items, None);
        let tiles = row_tiles(&p);
        assert_eq!(tiles.len(), 3, "three items, three tiles");
        for tile in &tiles {
            assert!(
                (tile.rect.left() - LIST_PADDING).abs() < 0.5,
                "a row tile starts at x={}, expected {LIST_PADDING} (the list's own padding)",
                tile.rect.left()
            );
            assert!(
                (tile.rect.width() - TILE_WIDTH).abs() < 0.5,
                "a row tile is {} wide, expected {TILE_WIDTH}",
                tile.rect.width()
            );
            assert!(
                (tile.rect.height() - ROW_TILE_HEIGHT).abs() < 0.5,
                "a row tile is {} high, expected {ROW_TILE_HEIGHT} (32px avatar + 10px padding \
                 top and bottom)",
                tile.rect.height()
            );
        }
        for pair in tiles.windows(2) {
            let gap = pair[1].rect.top() - pair[0].rect.bottom();
            assert!(
                (gap - ROW_GAP).abs() < 0.5,
                "consecutive tiles are {gap} apart, expected the design's {ROW_GAP}"
            );
        }
    }

    #[test]
    fn the_app_badge_marks_exactly_the_items_that_carry_an_app_match_field() {
        // The design's trailing "app" chip. Its meaning is not decorative:
        // it is `deskwarden:app-match`, the field that makes an item fillable
        // into a native window, which `extract_app_match` answers from the
        // item already in hand.
        let with = paint(&[with_app_match(login("Ledgerline", "a@b.c"))], None);
        assert!(
            with.texts.iter().any(|(t, _, _)| t == "app"),
            "an item with a `deskwarden:app-match` field must carry the badge; painted: {:?}",
            with.texts
        );
        let without = paint(&[login("Vantage VPN", "a@b.c")], None);
        assert!(
            !without.texts.iter().any(|(t, _, _)| t == "app"),
            "an item with no app match must NOT be badged -- a badge that means nothing is worse \
             than no badge; painted: {:?}",
            without.texts
        );
    }

    #[test]
    fn the_app_badge_takes_the_designs_two_colour_treatments() {
        // `color: #605d5d; background: #f3f2f2` unselected;
        // `color: #14307a; background: #dbe4f7` selected.
        let items = [with_app_match(login("Ledgerline", "a@b.c"))];
        let unselected = paint(&items, None);
        assert_eq!(text_color(&unselected, "app"), theme::TEXT_MUTED);
        let selected = paint(&items, Some("Ledgerline"));
        assert_eq!(text_color(&selected, "app"), theme::BLUE_DEEP);

        let badge_of = |p: &Painted| {
            let label = p
                .texts
                .iter()
                .find(|(t, _, _)| t == "app")
                .expect("the badge")
                .1;
            p.rects
                .iter()
                .find(|r| r.rect.contains_rect(label) && r.rect.width() < 60.0)
                .unwrap_or_else(|| panic!("the badge's own filled chip was never painted"))
                .clone()
        };
        assert_eq!(badge_of(&unselected).fill, theme::CANVAS);
        assert_eq!(badge_of(&selected).fill, theme::FOCUS_RING);
        assert_eq!(badge_of(&unselected).corner_radius, CornerRadius::same(5));
    }

    #[test]
    fn a_vault_sized_list_still_lays_out_only_the_visible_rows() {
        // NON-NEGOTIABLE. The picker's list shipped un-virtualized once and
        // was visibly laggy; painting a filled, bordered, rounded tile per row
        // must not turn `draw` into an O(N) layout. 1656 is the user's real
        // vault size. `visible_ids` is filled with exactly the rows
        // `show_rows` handed back, so it is a direct readout of how many rows
        // were laid out -- and the tile count proves the painting followed it.
        let items: Vec<VaultItem> = (0..1656)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let p = paint(&items, None);
        let ceiling = (PANE_HEIGHT / (ROW_TILE_HEIGHT + ROW_GAP)).ceil() as usize + 4;
        assert!(
            p.visible.len() <= ceiling,
            "{} of 1656 rows were laid out; at most {ceiling} fit the {PANE_HEIGHT}pt pane, so \
             the list is no longer virtualized",
            p.visible.len()
        );
        // And the tiles followed the windowing rather than being painted for
        // every item: never more than the rows that were laid out, and at
        // least all but the one `show_rows` deliberately overshoots by (it
        // hands back `ceil + 1` rows, and egui skips painting the one that
        // falls outside the clip rect).
        let tiles = row_tiles(&p).len();
        assert!(
            tiles <= p.visible.len() && tiles + 1 >= p.visible.len(),
            "{tiles} tiles were painted for {} laid-out rows",
            p.visible.len()
        );
        assert!(tiles > 0, "nothing was painted at all");
    }

    #[test]
    fn the_rows_stay_in_register_with_the_scrollbar_all_the_way_to_the_end() {
        // The trap the design's `gap: 6px` walks straight into.
        // `ScrollArea::show_rows` reads `item_spacing.y` from the ui it is
        // GIVEN, before its closure runs, and virtualizes against
        // `row_height + that spacing`. Setting the gap inside the closure
        // instead -- which is where the old 2pt gap was set, and the obvious
        // place to put a new one -- leaves the scroll maths on the theme's
        // default 8pt pitch while the rows paint 6pt apart. Nothing looks
        // wrong until you scroll: the error is 2pt per row, so at the bottom
        // of a 100-item list the last row sits ~200pt above where the
        // scrollbar says the end is.
        //
        // Scrolled to the very end, the last row's tile must therefore end
        // exactly on the list's own bottom padding edge. That is an ABSOLUTE
        // position, not "below the one before it".
        const N: usize = 100;
        let items: Vec<VaultItem> = (0..N)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let p = paint_with(&items, None, 24);
        let last = format!("Item {:04}", N - 1);
        assert!(
            p.visible.contains(&last),
            "scrolled to the end, {last:?} was not among the laid-out rows: {:?}",
            p.visible
        );
        let bottom = row_tiles(&p)
            .iter()
            .map(|t| t.rect.bottom())
            .fold(f32::MIN, f32::max);
        let expected = PANE_HEIGHT - LIST_PADDING;
        assert!(
            (bottom - expected).abs() < 1.5,
            "at the end of the list the last row tile ends at y={bottom}, but the list's own \
             bottom padding edge is at y={expected} -- the painted rows and the pitch \
             `show_rows` scrolls by have drifted apart"
        );
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

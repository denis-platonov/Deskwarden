//! The vault window's left pane (design 4.8 "Sidebar"): the VAULT section
//! (All items / Favorites / Logins / Cards / Secure notes / Trash, each with
//! a live count) and the FOLDERS section (one row per real vault folder,
//! also counted), plus the auto-lock countdown pinned to the bottom.

use crate::theme;
use crate::vault_bridge::{Folder, VaultItem};
use eframe::egui::{self, CornerRadius, RichText};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarFilter {
    All,
    Favorites,
    Logins,
    Passkeys,
    Cards,
    Identities,
    SecureNotes,
    SshKeys,
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
            // Passkeys are not their own item type -- Bitwarden stores them
            // as `fido2Credentials` on ordinary login items, so this is a
            // filter over logins rather than a type match.
            //
            // Read out of `LoginData::other` rather than being given a typed
            // field: `bw serve` sends `fido2Credentials: []` on logins that
            // have none, and a typed `Vec` with the `skip_serializing_if =
            // "Vec::is_empty"` that the neighbouring fields use would drop
            // that key on write, changing the item's shape server-side --
            // exactly the class of round-trip bug `vault_bridge`'s
            // "a_partial_login_object..." tests exist to prevent. `other`
            // already round-trips it untouched.
            SidebarFilter::Passkeys => item
                .login
                .as_ref()
                .and_then(|login| login.other.get("fido2Credentials"))
                .and_then(|value| value.as_array())
                .is_some_and(|credentials| !credentials.is_empty()),
            SidebarFilter::Cards => item.item_type == Some(3),
            SidebarFilter::Identities => item.item_type == Some(4),
            SidebarFilter::SecureNotes => item.item_type == Some(2),
            SidebarFilter::SshKeys => item.item_type == Some(5),
            // Not implementable from the endpoint this window reads:
            // `/list/object/items` returns no trashed items and carries no
            // `deletedDate` field to filter on (verified against a real
            // 1657-item vault). Kept as a visible-but-empty row because both
            // the design and the official client list it; it is not a bug
            // that its count is always 0, and no filtering here can change
            // that without a different endpoint.
            SidebarFilter::Trash => false,
            SidebarFilter::Folder(id) => item.folder_id.as_deref() == Some(id.as_str()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    None,
    NewFolder,
    /// A click on a folder row's edit-pencil icon -- the folder id.
    /// `vault_window::mod` opens the "Edit folder" modal in response (see
    /// `folder_modal`), which is where rename and delete actually happen.
    EditFolder(String),
}

/// Whether `folder` is `bw serve`'s virtual "No Folder" bucket rather than
/// a real, server-side folder.
///
/// The CLI reports it alongside genuine folders but with an empty id, which
/// is the only thing distinguishing the two (its *name* is user-facing text
/// a real folder could equally be called, so matching on that would let
/// someone lock themselves out of a folder they actually named "No
/// Folder"). Nothing about it can be renamed or deleted.
pub fn is_virtual_folder(folder: &Folder) -> bool {
    folder.id.is_empty()
}

/// How many of `items` fall under `filter`. Pure and separate from drawing
/// so the sidebar's counts are testable without an egui context.
pub fn count_for(items: &[VaultItem], filter: &SidebarFilter) -> usize {
    items.iter().filter(|item| filter.scope_contains(item)).count()
}

/// Reserved width for a folder row's edit-pencil icon, subtracted from
/// `sidebar_row`'s width *before* the row itself is laid out -- otherwise the
/// row's own click target would extend under the icon and the two would
/// double-fire on the same click. Positioned against `sidebar_row`'s
/// returned response rect (see the FOLDERS loop below) rather than a nested
/// `ui.horizontal`: a horizontal layout's row height comes from its tallest
/// child, which previously made FOLDERS rows a different height than plain
/// VAULT rows despite both using the same `sidebar_row` allocation.
const FOLDER_EDIT_BUTTON_WIDTH: f32 = 24.0;

/// Row height and horizontal text inset, from design 4.8's exact CSS for
/// each sidebar row: `padding: 8px 10px` on a single 13px text line (line
/// box ~16px at this size) -- 8 + 16 + 8 rounds to 32px tall, and both the
/// label and the count sit 10px in from the row's own edge.
const ROW_HEIGHT: f32 = 32.0;
const ROW_INSET_X: f32 = 10.0;
/// Gap between consecutive rows within a section (`gap: 2px` on the
/// design's row list) -- deliberately much smaller than the ambient
/// `item_spacing.y` this app's global style otherwise uses (8px, see
/// `theme::apply`), so it's set explicitly around each row loop rather than
/// inherited.
const ROW_GAP: f32 = 2.0;
/// Horizontal inset for a section label ("VAULT"/"FOLDERS") -- 8px per the
/// design, two pixels tighter than a row's own 10px `ROW_INSET_X`, which is
/// exactly what the design's CSS specifies for these two elements.
const SECTION_LABEL_INSET: f32 = 8.0;
/// Height of the band a section header occupies. Taller than the 11px text
/// itself so the FOLDERS header's trailing "+" has room to sit centered on
/// the same line.
const SECTION_HEADER_HEIGHT: f32 = 20.0;

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
        // Zeroed for this whole block: egui inserts `item_spacing` between
        // every pair of sequential widgets automatically, including
        // `add_space` calls, so leaving the ambient 8px default in place
        // here would silently add 8px on top of every explicit gap this
        // function writes below (an explicit 14px gap would render as
        // 22px). Every gap in this sidebar is deliberate and spelled out
        // below instead -- `ROW_GAP` is re-applied just around each row
        // loop, where a uniform inter-row gap actually is wanted.
        ui.spacing_mut().item_spacing.y = 0.0;

        section_label(ui, "VAULT");
        ui.add_space(SECTION_LABEL_INSET);
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        for (label, filter) in [
            ("All items", SidebarFilter::All),
            ("Favorites", SidebarFilter::Favorites),
            ("Logins", SidebarFilter::Logins),
            ("Passkeys", SidebarFilter::Passkeys),
            ("Cards", SidebarFilter::Cards),
            ("Identities", SidebarFilter::Identities),
            ("Secure notes", SidebarFilter::SecureNotes),
            ("SSH keys", SidebarFilter::SshKeys),
            ("Trash", SidebarFilter::Trash),
        ] {
            let count = count_for(items, &filter);
            let width = ui.available_width();
            if sidebar_row(ui, label, count, *selected == filter, width).clicked() {
                *selected = filter;
            }
        }
        ui.spacing_mut().item_spacing.y = 0.0;

        // Divider (design: `height: 1px; background: #eae7e7; margin: 14px
        // 8px`) -- 14px above and below, inset 8px from each side rather
        // than spanning the panel's full width.
        ui.add_space(14.0);
        inset_hairline(ui, 8.0);
        ui.add_space(14.0);

        // The FOLDERS header carries a trailing "+" (new folder). Both are
        // placed against one explicitly-allocated header rect rather than a
        // `ui.horizontal` + nested `right_to_left`, for the same reason
        // `sidebar_row` paints directly: nested layout containers each
        // re-advance the parent cursor by their own content height, which
        // is what made these rows overlap.
        let header_rect = section_label(ui, "FOLDERS");
        let plus_rect = egui::Rect::from_center_size(
            egui::Pos2::new(header_rect.right() - SECTION_LABEL_INSET - 8.0, header_rect.center().y),
            egui::Vec2::splat(18.0),
        );
        let plus = ui.interact(plus_rect, ui.id().with("new-folder"), egui::Sense::click());
        if plus.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.painter().text(
            plus_rect.center(),
            egui::Align2::CENTER_CENTER,
            "+",
            egui::FontId::new(15.0, egui::FontFamily::Proportional),
            if plus.hovered() { theme::INK } else { theme::TEXT_GHOST },
        );
        if plus.clicked() {
            action = SidebarAction::NewFolder;
        }
        ui.add_space(SECTION_LABEL_INSET);
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        for folder in folders {
            let filter = SidebarFilter::Folder(folder.id.clone());
            let count = count_for(items, &filter);
            // Reserve the edit icon's width *before* the row claims the
            // rest of the available width -- see `FOLDER_EDIT_BUTTON_WIDTH`.
            let row_width = (ui.available_width() - FOLDER_EDIT_BUTTON_WIDTH).max(0.0);
            let response = sidebar_row(ui, &folder.name, count, *selected == filter, row_width);
            if response.clicked() {
                *selected = filter.clone();
            }
            // Positioned against the row's own returned rect, in the same
            // vertical span -- not a nested `ui.horizontal`, which is what
            // caused this row to be taller (and differently spaced) than
            // the plain VAULT rows above.
            let edit_rect = egui::Rect::from_min_max(
                egui::Pos2::new(response.rect.right(), response.rect.top()),
                egui::Pos2::new(response.rect.right() + FOLDER_EDIT_BUTTON_WIDTH, response.rect.bottom()),
            );
            // `bw serve` reports a virtual "No Folder" bucket -- the items
            // that are in no folder at all -- as a folder with an *empty
            // id*. It is a view, not a real folder: there is nothing on the
            // server to rename or delete, and `DELETE /object/folder/` with
            // no id matches nothing. It stays listed (clicking it filters
            // to unfiled items, which is useful), but offering an edit
            // affordance on it promises an action that cannot exist.
            if !is_virtual_folder(folder) {
                let edit_id = egui::Id::new(("folder-edit", folder.id.as_str()));
                if theme::pencil_glyph_at(ui, edit_rect, edit_id).clicked() {
                    action = SidebarAction::EditFolder(folder.id.clone());
                }
            }
        }
        ui.spacing_mut().item_spacing.y = 0.0;

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(10.0);
            ui.label(RichText::new(lock_countdown).size(11.0).color(theme::TEXT_GHOST));
        });
    });

    action
}

/// A section header ("VAULT"/"FOLDERS"): 11px Bold, letterspaced, ghost
/// text, inset `SECTION_LABEL_INSET` from the panel edge. Allocates one
/// full-width band of `SECTION_HEADER_HEIGHT` and paints into it, returning
/// that band's rect so a caller can position a trailing control (the
/// FOLDERS "+") against it without a second layout container.
///
/// The caller adds whatever vertical gap belongs after it
/// (`SECTION_LABEL_INSET`, matching the design's own 8px).
fn section_label(ui: &mut egui::Ui, text: &str) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), SECTION_HEADER_HEIGHT),
        egui::Sense::hover(),
    );
    let job = theme::letterspaced(text, 11.0, theme::BOLD, 1.2, theme::TEXT_GHOST);
    let galley = ui.painter().layout_job(job);
    let pos = egui::Pos2::new(
        rect.left() + SECTION_LABEL_INSET,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(pos, galley, theme::TEXT_GHOST);
    rect
}

/// A hairline inset `inset` from both sides of the available width.
fn inset_hairline(ui: &mut egui::Ui, inset: f32) {
    let full_width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(full_width, 1.0), egui::Sense::hover());
    let line = egui::Rect::from_min_max(
        egui::Pos2::new(rect.min.x + inset, rect.min.y),
        egui::Pos2::new(rect.max.x - inset, rect.max.y),
    );
    ui.painter().rect_filled(line, CornerRadius::ZERO, theme::HAIRLINE);
}

/// One VAULT/FOLDERS row: label left, right-aligned count, allocated at
/// exactly `width` wide (not necessarily all of `ui.available_width()` --
/// see `FOLDER_EDIT_BUTTON_WIDTH`) and `ROW_HEIGHT` tall. Returns the row's
/// `Response` so callers can both check `.clicked()` and (FOLDERS rows)
/// position a trailing icon relative to `.rect`.
///
/// Selected rows get the design's blue wash background plus Bold text in
/// `BLUE_DEEP`; unselected rows are plain-weight `INK` text (the design
/// specifies no `font-weight` on the unselected row divs, which means the
/// inherited default -- 400/regular, not the 600/SemiBold this used
/// everywhere before). A hover tint gives non-selected rows the same
/// "this reacted to me" feedback selected ones get from their wash.
///
/// Both texts are *painted*, not added as child widgets inside a nested
/// `scope_builder`/`horizontal`. That nesting is what made these rows
/// overlap: `Ui::scope_builder` ends with
/// `advance_cursor_after_rect(child.min_rect())`, and the child's min_rect
/// only spans the ~16px of text drawn in it, not the row's full
/// `ROW_HEIGHT`. So every row rewound the parent cursor to 16px below its
/// own top, and the next row started roughly half a row too high. Painting
/// touches no cursor at all, so the single `allocate_exact_size` below is
/// the only thing that moves it -- by exactly `ROW_HEIGHT` + `ROW_GAP`.
fn sidebar_row(ui: &mut egui::Ui, label: &str, count: usize, selected: bool, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(8), theme::BLUE_WASH);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), theme::CARD_TINT);
    }

    // Count first: it's right-aligned, and its measured width is what bounds
    // how far the label may run before it would collide with it.
    let count_text = count.to_string();
    let count_galley = ui.painter().layout_no_wrap(
        count_text,
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
        theme::TEXT_GHOST,
    );
    let count_width = count_galley.size().x;
    ui.painter().galley(
        egui::Pos2::new(
            rect.right() - ROW_INSET_X - count_width,
            rect.center().y - count_galley.size().y / 2.0,
        ),
        count_galley,
        theme::TEXT_GHOST,
    );

    let (font, color) = if selected {
        (
            egui::FontId::new(13.0, egui::FontFamily::Name(theme::BOLD.into())),
            theme::BLUE_DEEP,
        )
    } else {
        (egui::FontId::new(13.0, egui::FontFamily::Proportional), theme::INK)
    };
    // Clipped to the space left of the count, so a long folder name is cut
    // off rather than running underneath its own count.
    let label_area = egui::Rect::from_min_max(
        egui::Pos2::new(rect.left() + ROW_INSET_X, rect.top()),
        egui::Pos2::new(rect.right() - ROW_INSET_X - count_width - 6.0, rect.bottom()),
    );
    ui.painter().with_clip_rect(label_area.intersect(ui.clip_rect())).text(
        egui::Pos2::new(label_area.left(), rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        color,
    );

    response
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

    /// Regression test for rows overlapping each other.
    ///
    /// `sidebar_row` used to paint its label/count inside a nested
    /// `ui.scope_builder(...)`, which ends by calling
    /// `advance_cursor_after_rect(child.min_rect())` -- and that child's
    /// min_rect covers only the text drawn in it (~16px), not the row's
    /// full `ROW_HEIGHT`. Each row therefore rewound the parent cursor to
    /// well above its own bottom edge, and every following row was drawn
    /// overlapping the one before it. Nothing about that is visible by
    /// reading the row's own code, which is why it survived several rounds
    /// of inspection; it is trivially visible in the allocated rects.
    #[test]
    fn consecutive_rows_do_not_overlap_and_sit_exactly_one_gap_apart() {
        let ctx = egui::Context::default();
        let mut rects: Vec<egui::Rect> = Vec::new();

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            for i in 0..5 {
                // `selected: false` keeps this on the default proportional
                // font -- `theme::apply`'s Archivo families are not
                // installed in this bare test context.
                let response = sidebar_row(ui, "Row", i, false, 180.0);
                rects.push(response.rect);
            }
        });

        assert_eq!(rects.len(), 5);
        for (i, rect) in rects.iter().enumerate() {
            assert!(
                (rect.height() - ROW_HEIGHT).abs() < 0.5,
                "row {i} is {}px tall, expected {ROW_HEIGHT}",
                rect.height()
            );
        }
        for pair in rects.windows(2) {
            let (above, below) = (pair[0], pair[1]);
            assert!(
                below.top() >= above.bottom() - 0.5,
                "rows overlap: one ends at y={} but the next starts at y={}",
                above.bottom(),
                below.top()
            );
            let gap = below.top() - above.bottom();
            assert!(
                (gap - ROW_GAP).abs() < 0.5,
                "gap between rows is {gap}px, expected {ROW_GAP}"
            );
        }
    }

    /// A passkey lives on a login item as `login.fido2Credentials`, and
    /// reaches us through `LoginData::other` (see `scope_contains`). An
    /// empty array means "this login has no passkey" and must not count.
    #[test]
    fn passkeys_count_logins_carrying_a_credential_not_every_login() {
        let with_passkey: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"A","fields":[],"type":1,
                "login":{"fido2Credentials":[{"credentialId":"abc"}]}}"#,
        )
        .unwrap();
        // What `bw serve` actually sends for a login without one.
        let empty_array: VaultItem = serde_json::from_str(
            r#"{"id":"2","name":"B","fields":[],"type":1,
                "login":{"fido2Credentials":[]}}"#,
        )
        .unwrap();
        let key_absent: VaultItem =
            serde_json::from_str(r#"{"id":"3","name":"C","fields":[],"type":1,"login":{}}"#)
                .unwrap();
        let not_a_login: VaultItem =
            serde_json::from_str(r#"{"id":"4","name":"D","fields":[],"type":2}"#).unwrap();

        let items = vec![with_passkey, empty_array, key_absent, not_a_login];
        assert_eq!(count_for(&items, &SidebarFilter::Passkeys), 1);
    }

    #[test]
    fn identities_and_ssh_keys_match_their_own_types() {
        let items = vec![
            item(Some(1), false, None), // Login
            item(Some(4), false, None), // Identity
            item(Some(5), false, None), // SSH key
            item(Some(5), false, None),
        ];
        assert_eq!(count_for(&items, &SidebarFilter::Identities), 1);
        assert_eq!(count_for(&items, &SidebarFilter::SshKeys), 2);
        // ...and don't leak into the neighbouring type filters.
        assert_eq!(count_for(&items, &SidebarFilter::Logins), 1);
        assert_eq!(count_for(&items, &SidebarFilter::Cards), 0);
        assert_eq!(count_for(&items, &SidebarFilter::SecureNotes), 0);
    }

    /// `bw serve` lists its virtual "No Folder" bucket with an empty id.
    /// Telling it apart by id rather than by name matters: "No Folder" is
    /// ordinary text a user could genuinely name a real folder, and matching
    /// on that would make their own folder uneditable.
    #[test]
    fn only_the_empty_id_folder_is_treated_as_virtual() {
        let virtual_bucket = Folder {
            id: String::new(),
            name: "No Folder".into(),
        };
        let real_folder_same_name = Folder {
            id: "a4e839ea-252a-4bcf-9ae0-f29f33304ef2".into(),
            name: "No Folder".into(),
        };
        let ordinary = Folder {
            id: "957b860f-1130-42d9-a72c-7814f828b4d5".into(),
            name: "Napps".into(),
        };

        assert!(is_virtual_folder(&virtual_bucket));
        assert!(!is_virtual_folder(&real_folder_same_name));
        assert!(!is_virtual_folder(&ordinary));
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

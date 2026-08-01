//! The vault window's left pane (design 4.8 "Sidebar"): the VAULT section
//! (All items / Favorites / Logins / Cards / Secure notes / Trash, each with
//! a live count) and the FOLDERS section (one row per vault folder, also
//! counted -- including `bw serve`'s virtual "No Folder" bucket, which is
//! reported as a folder but scoped by [`SidebarFilter::Unfiled`] rather than
//! by an id), plus the auto-lock countdown pinned to the bottom.

use crate::theme;
use crate::vault_bridge::{Folder, ItemKind, VaultItem};
use eframe::egui::{self, CornerRadius};

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
    /// A real, server-side folder, by id.
    Folder(String),
    /// The items that are in no folder at all -- `bw serve`'s virtual "No
    /// Folder" bucket.
    ///
    /// Its own variant rather than `Folder("")`, and that distinction is the
    /// whole point. The CLI reports the bucket *as a folder with an empty
    /// id* (see [`is_virtual_folder`]), so the empty string was doing double
    /// duty: it was the marker for "this row is the virtual bucket" and, at
    /// the same time, a folder id to compare items against. The FOLDERS loop
    /// built `Folder("")` for that row, which matches items whose
    /// `folder_id` is `Some("")` -- and unfiled items have `folder_id:
    /// None`. So the row matched nothing and its badge read 0 while 94% of a
    /// real 1654-item vault sat unfiled behind it.
    ///
    /// Splitting the variant, rather than special-casing the empty string
    /// inside the `Folder` arm, is deliberate: the latter leaves `Folder("")`
    /// constructible and meaning something other than what it says, which is
    /// the defect rather than the fix.
    Unfiled,
}

impl SidebarFilter {
    /// Whether `item` falls under this filter. The single place that
    /// encodes "what does each filter variant mean" -- both `count_for`
    /// (this file) and `item_list::matches_filter` delegate to it, rather
    /// than each hand-duplicating the same per-variant scoping logic (which
    /// had drifted into two copies that happened to still agree, but had no
    /// mechanism keeping them that way).
    ///
    /// The type-based variants go through [`ItemKind::of`] rather than
    /// comparing `item.item_type` to a literal, for the same reason: the
    /// mapping from `bw`'s numeric type to a meaning now lives in exactly
    /// one place, so this file and the read pane cannot drift apart about
    /// what a `3` is. One deliberate behaviour change came with that: an
    /// item whose `type` the server omitted is a `Login` to `ItemKind`, so
    /// it now counts under `Logins` where the hand-written `== Some(1)`
    /// left it out of every type filter while the rest of the app was
    /// already treating it as a login.
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
            SidebarFilter::Logins => ItemKind::of(item) == ItemKind::Login,
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
            SidebarFilter::Cards => ItemKind::of(item) == ItemKind::Card,
            SidebarFilter::Identities => ItemKind::of(item) == ItemKind::Identity,
            SidebarFilter::SecureNotes => ItemKind::of(item) == ItemKind::SecureNote,
            SidebarFilter::SshKeys => ItemKind::of(item) == ItemKind::SshKey,
            // Not implementable from the endpoint this window reads:
            // `/list/object/items` returns no trashed items and carries no
            // `deletedDate` field to filter on (verified against a real
            // 1657-item vault). Kept as a visible-but-empty row because both
            // the design and the official client list it; it is not a bug
            // that its count is always 0, and no filtering here can change
            // that without a different endpoint.
            SidebarFilter::Trash => false,
            SidebarFilter::Folder(id) => item.folder_id.as_deref() == Some(id.as_str()),
            // `None`, and *only* `None`. `Some("")` is deliberately not
            // treated as unfiled: nothing produces it. `bw serve` sends
            // `folderId: null` for unfiled items (measured against the real
            // vault -- 1559 nulls, zero empty strings out of 1654), and this
            // app's own unfile path writes an explicit JSON null too (see
            // `vault_bridge::folder_move_body` and
            // `the_unfile_body_carries_a_folder_id_key_that_is_present_and_null`).
            // Accepting `Some("")` here would mean inventing a third state
            // to paper over a case that does not exist -- which is how the
            // empty string came to mean two things in the first place.
            SidebarFilter::Unfiled => item.folder_id.is_none(),
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

/// Where the sidebar's right-hand glyph column is centred, given the
/// sidebar's own right edge -- the single source of truth for *both* glyphs
/// that live in that column: the FOLDERS header's "+" (new folder) and each
/// folder row's edit pencil.
///
/// The column is the `FOLDER_EDIT_BUTTON_WIDTH` lane that every folder row
/// already gives up before it is laid out, so its centre is half that lane
/// in from the edge. That lane is the fixed thing here; the "+" simply joins
/// it.
///
/// It exists as a function rather than as two agreeing literals because that
/// is precisely how these two drifted: the pencil was hung off the row's
/// right edge (`right ..= right + FOLDER_EDIT_BUTTON_WIDTH`, centre at
/// `edge - 12`) while the "+" was placed by unrelated arithmetic
/// (`header_rect.right() - SECTION_LABEL_INSET - 8.0`, centre at `edge -
/// 16`), leaving them 4px apart in one visual column with nothing keeping
/// them together.
fn glyph_column_center_x(sidebar_right: f32) -> f32 {
    sidebar_right - FOLDER_EDIT_BUTTON_WIDTH / 2.0
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
        // ONE right edge for ONE glyph column, read off the header's
        // full-width band and reused by every folder row's pencil below --
        // `section_label` allocates all of `ui.available_width()`, which is
        // the same width the rows then divide up, so this is the same edge
        // the pencil lane is measured from. Deriving both from it is what
        // keeps the "+" and the pencils on one column; see
        // `glyph_column_center_x`.
        let glyph_column_x = glyph_column_center_x(header_rect.right());
        let plus_rect = egui::Rect::from_center_size(
            egui::Pos2::new(glyph_column_x, header_rect.center().y),
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
            // The virtual "No Folder" bucket gets the filter that says what
            // it means. Building `Folder(folder.id.clone())` here for *every*
            // row is what gave that row `Folder("")`, a filter for a folder
            // whose id is the empty string -- which no item has. See
            // `SidebarFilter::Unfiled`.
            let filter = if is_virtual_folder(folder) {
                SidebarFilter::Unfiled
            } else {
                SidebarFilter::Folder(folder.id.clone())
            };
            let count = count_for(items, &filter);
            // Reserve the edit icon's width *before* the row claims the
            // rest of the available width -- see `FOLDER_EDIT_BUTTON_WIDTH`.
            let row_width = (ui.available_width() - FOLDER_EDIT_BUTTON_WIDTH).max(0.0);
            let response = sidebar_row(ui, &folder.name, count, *selected == filter, row_width);
            if response.clicked() {
                *selected = filter.clone();
            }
            // Vertically, the row's own returned rect (same span, same
            // height) -- not a nested `ui.horizontal`, which is what caused
            // this row to be taller (and differently spaced) than the plain
            // VAULT rows above. Horizontally, the shared glyph column, so
            // the pencil and the header's "+" cannot drift apart again.
            let edit_rect = egui::Rect::from_center_size(
                egui::Pos2::new(glyph_column_x, response.rect.center().y),
                egui::Vec2::new(FOLDER_EDIT_BUTTON_WIDTH, response.rect.height()),
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
            // The bottom half of the design's `padding: 10px` on the
            // countdown div -- the same 10px on all four sides that
            // `countdown_label` takes its horizontal inset from. Left as a
            // literal rather than folded into `ROW_INSET_X`: that constant
            // names a *horizontal* text inset, and the two only coincide
            // because this one element's padding happens to be uniform.
            ui.add_space(10.0);
            countdown_label(ui, lock_countdown);
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

/// The auto-lock countdown pinned to the sidebar's bottom.
///
/// Design 4.8 spells it out as
/// `<div style="padding: 10px; font-size: 11px; color: #9b9797">Locks in
/// 11:42</div>`, a sibling of the rows inside the same padded sidebar
/// column. That uniform `padding: 10px` puts its text 10px in from exactly
/// the edge the rows' own `padding: 8px 10px` puts their labels 10px in
/// from -- so this is [`ROW_INSET_X`], deliberately *not* the two-pixels-
/// tighter [`SECTION_LABEL_INSET`] the "VAULT"/"FOLDERS" headers use.
///
/// Painted into one explicitly-allocated band, like [`section_label`] and
/// [`sidebar_row`], rather than added as a `ui.label`: a label inside the
/// enclosing bottom-up layout is placed by that layout against the panel's
/// content edge and has nowhere to take a horizontal inset from, which is
/// precisely how the countdown came to sit 10px left of every label above
/// it.
fn countdown_label(ui: &mut egui::Ui, text: &str) {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
        theme::TEXT_GHOST,
    );
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), galley.size().y),
        egui::Sense::hover(),
    );
    ui.painter().galley(
        egui::Pos2::new(rect.left() + ROW_INSET_X, rect.top()),
        galley,
        theme::TEXT_GHOST,
    );
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
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
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

    /// An item whose `type` the server omitted is a login everywhere else in
    /// the app (`ItemKind::of`, and therefore the read pane and the picker),
    /// so the sidebar must agree. Before `scope_contains` went through
    /// `ItemKind` it compared `item_type == Some(1)` by hand and left such an
    /// item out of Logins while every other surface showed it as one -- the
    /// two-copies-that-happen-to-agree hazard this function's own doc names.
    #[test]
    fn an_item_with_no_type_counts_as_a_login_like_it_does_everywhere_else() {
        let items = vec![item(None, false, None)];
        assert_eq!(count_for(&items, &SidebarFilter::Logins), 1);
        assert_eq!(count_for(&items, &SidebarFilter::Cards), 0);
        assert_eq!(count_for(&items, &SidebarFilter::SecureNotes), 0);
        assert_eq!(count_for(&items, &SidebarFilter::Identities), 0);
        assert_eq!(count_for(&items, &SidebarFilter::SshKeys), 0);
    }

    /// A type this build does not know (`ItemKind::Unknown`) must fall into
    /// no type filter at all rather than into Logins.
    #[test]
    fn an_unknown_item_type_counts_under_no_type_filter() {
        let items = vec![item(Some(6), false, None)];
        assert_eq!(count_for(&items, &SidebarFilter::All), 1);
        assert_eq!(count_for(&items, &SidebarFilter::Logins), 0);
        assert_eq!(count_for(&items, &SidebarFilter::Cards), 0);
        assert_eq!(count_for(&items, &SidebarFilter::SecureNotes), 0);
        assert_eq!(count_for(&items, &SidebarFilter::Identities), 0);
        assert_eq!(count_for(&items, &SidebarFilter::SshKeys), 0);
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
            other: serde_json::Map::new(),
        };
        let real_folder_same_name = Folder {
            id: "a4e839ea-252a-4bcf-9ae0-f29f33304ef2".into(),
            name: "No Folder".into(),
            other: serde_json::Map::new(),
        };
        let ordinary = Folder {
            id: "957b860f-1130-42d9-a72c-7814f828b4d5".into(),
            name: "Napps".into(),
            other: serde_json::Map::new(),
        };

        assert!(is_virtual_folder(&virtual_bucket));
        assert!(!is_virtual_folder(&real_folder_same_name));
        assert!(!is_virtual_folder(&ordinary));
    }

    /// Same walk `detail.rs`'s `collect_text_rects` does: every painted
    /// string plus the rectangle egui laid it out in. This file paints its
    /// labels straight onto `ui.painter()` rather than adding widgets, so
    /// the paint list is the only place their positions exist -- there are
    /// no `Response`s to read them off.
    fn collect_text_rects(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.text().to_string(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text_rects(shape, out);
                }
            }
            // Every other shape is geometry, and this is a test helper, not
            // a decision over a domain enum.
            _ => {}
        }
    }

    /// Draws a whole sidebar headlessly and returns every painted string
    /// with its laid-out rect.
    ///
    /// The two throwaway frames before the real one are the same ones
    /// `detail.rs`'s `painted_text` runs, for the same reason:
    /// `theme::apply`'s font families only exist from the *next* frame on,
    /// so a selected row's `FontFamily::Name(BOLD)` would otherwise resolve
    /// against a family that does not exist yet.
    fn painted_sidebar(lock_countdown: &str) -> Vec<(String, egui::Rect)> {
        painted_sidebar_and_bounds(lock_countdown).0
    }

    /// Every painted convex polygon's bounding rect. The folder pencil is
    /// drawn as two `Shape::Path`s (see `theme::pencil_glyph_at`) rather than
    /// as text, so it is invisible to `collect_text_rects`; this is how its
    /// painted position is read.
    fn collect_path_rects(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Path(path) => out.push(path.visual_bounding_rect()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_path_rects(shape, out);
                }
            }
            _ => {}
        }
    }

    fn painted_sidebar_and_bounds(
        lock_countdown: &str,
    ) -> (Vec<(String, egui::Rect)>, Vec<egui::Rect>, egui::Rect) {
        let items = vec![item(Some(1), false, Some("f1"))];
        let folders = vec![Folder {
            id: "f1".into(),
            name: "Engineering".into(),
            other: serde_json::Map::new(),
        }];
        painted_sidebar_fixture(lock_countdown, items, folders)
    }

    /// [`painted_sidebar_and_bounds`] over a caller-supplied vault, so a test
    /// about the virtual "No Folder" row can hand in one that actually has
    /// unfiled items and that row in it. The default fixture above is left
    /// byte-for-byte as it was, so every test already written against it is
    /// still looking at the same sidebar.
    fn painted_sidebar_fixture(
        lock_countdown: &str,
        items: Vec<VaultItem>,
        folders: Vec<Folder>,
    ) -> (Vec<(String, egui::Rect)>, Vec<egui::Rect>, egui::Rect) {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(212.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut selected = SidebarFilter::All;

        let mut bounds = egui::Rect::NOTHING;
        let output = ctx.run_ui(input(), |ui| {
            bounds = ui.max_rect();
            draw_sidebar(ui, &items, &folders, &mut selected, lock_countdown);
        });

        let mut rects = Vec::new();
        let mut paths = Vec::new();
        for clipped in &output.shapes {
            collect_text_rects(&clipped.shape, &mut rects);
            collect_path_rects(&clipped.shape, &mut paths);
        }
        (rects, paths, bounds)
    }

    /// The pencil is two polygons (body + nib), each with its own bounding
    /// box; the glyph's position is the union of them, which
    /// `theme::pencil_glyph_at` centres on the rect it is given.
    fn union_of(rects: &[egui::Rect]) -> egui::Rect {
        assert!(!rects.is_empty(), "the sidebar painted no polygons at all");
        rects
            .iter()
            .copied()
            .reduce(|a, b| a.union(b))
            .expect("non-empty")
    }

    /// The right-hand glyph column's centre, stated as an absolute number
    /// rather than only "the plus and the pencil agree".
    ///
    /// A relative-only assertion is exactly what let the previous defect in
    /// this file through (see
    /// `the_countdown_and_the_rows_are_both_row_inset_from_the_panel_edge`):
    /// a probe that moved *both* insets together stayed green. So this pins
    /// the function's output for a known right edge: the column is centred
    /// inside the `FOLDER_EDIT_BUTTON_WIDTH` lane that folder rows already
    /// give up, i.e. half that lane in from the edge.
    #[test]
    fn the_glyph_column_is_centred_in_the_lane_folder_rows_give_up() {
        assert_eq!(glyph_column_center_x(212.0), 200.0);
        assert_eq!(glyph_column_center_x(300.0), 288.0);
        assert_eq!(
            glyph_column_center_x(212.0),
            212.0 - FOLDER_EDIT_BUTTON_WIDTH / 2.0
        );
    }

    /// The user-reported defect: "Folder + should be aligned with pencil
    /// icons". The FOLDERS header's "+" was placed by its own arithmetic
    /// (`header_rect.right() - SECTION_LABEL_INSET - 8.0`, centre at
    /// R-16) while the pencils hang off the reserved edit lane (centre at
    /// R-12), so the two sat 4px apart. Both now come from
    /// [`glyph_column_center_x`], and this asserts the painted "+" really
    /// lands there -- against the sidebar's own right edge, absolutely, not
    /// merely level with the pencil.
    #[test]
    fn the_folders_plus_is_painted_on_the_shared_glyph_column() {
        let (painted, _, bounds) = painted_sidebar_and_bounds("Locks in 11:42");

        let plus = painted
            .iter()
            .find(|(text, _)| text == "+")
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("the sidebar painted no \"+\": {painted:?}"));
        let expected = glyph_column_center_x(bounds.right());
        assert!(
            (plus.center().x - expected).abs() < 0.5,
            "the \"+\" is centred at x={}, expected {expected} \
             (the glyph column for a sidebar whose right edge is {})",
            plus.center().x,
            bounds.right()
        );
    }

    /// ...and the pencil is on that same column, read off its real painted
    /// polygons rather than assumed. No fallback source-text guard was
    /// needed: `theme::pencil_glyph_at` emits exactly two
    /// `Shape::Path`s and centres their union on the rect it is handed, and
    /// those are the only paths this sidebar paints (the rows, hairline and
    /// washes are `Shape::Rect`, the labels `Shape::Text`).
    #[test]
    fn the_folder_pencil_sits_on_the_same_column_as_the_plus() {
        let (painted, paths, bounds) = painted_sidebar_and_bounds("Locks in 11:42");

        assert_eq!(
            paths.len(),
            2,
            "expected exactly the pencil's two polygons, got {paths:?}"
        );
        let pencil = union_of(&paths);
        let plus = painted
            .iter()
            .find(|(text, _)| text == "+")
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("the sidebar painted no \"+\": {painted:?}"));

        assert!(
            (pencil.center().x - glyph_column_center_x(bounds.right())).abs() < 0.5,
            "the pencil is centred at x={}, expected {}",
            pencil.center().x,
            glyph_column_center_x(bounds.right())
        );
        assert!(
            (pencil.center().x - plus.center().x).abs() < 0.5,
            "the FOLDERS \"+\" is centred at x={} but the folder pencil at x={} \
             -- they are {}px apart",
            plus.center().x,
            pencil.center().x,
            (pencil.center().x - plus.center().x).abs()
        );
    }

    fn left_edge_of(painted: &[(String, egui::Rect)], needle: &str) -> f32 {
        painted
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, rect)| rect.left())
            .unwrap_or_else(|| panic!("the sidebar painted no {needle:?}: {painted:?}"))
    }

    /// The user-reported defect: the auto-lock countdown sat flush against
    /// the panel's content edge while every row label above it was inset
    /// `ROW_INSET_X`, so it hung ~10px further left than everything else.
    ///
    /// Design 4.8 gives the countdown `padding: 10px` and each row
    /// `padding: 8px 10px`, both inside the same padded sidebar column --
    /// i.e. the same 10px horizontal inset, so their text must start on the
    /// same x. That is what this asserts, against the real painted galleys,
    /// because a pixel offset is invisible to any test that only checks
    /// which strings were drawn.
    #[test]
    fn the_lock_countdown_starts_on_the_same_x_as_the_row_labels() {
        let painted = painted_sidebar("Locks in 11:42");

        let countdown = left_edge_of(&painted, "Locks in 11:42");
        let vault_row = left_edge_of(&painted, "All items");
        let folder_row = left_edge_of(&painted, "Engineering");

        assert!(
            (countdown - vault_row).abs() < 0.5,
            "the countdown starts at x={countdown} but the VAULT rows' labels start at x={vault_row}"
        );
        assert!(
            (countdown - folder_row).abs() < 0.5,
            "the countdown starts at x={countdown} but the FOLDERS rows' labels start at x={folder_row}"
        );
    }

    /// ...and that shared x really is `ROW_INSET_X` in from the sidebar's
    /// own left edge, not merely equal to whatever the rows happen to do.
    /// Without this, insetting *both* by the wrong amount would still pass
    /// the test above -- which is not hypothetical: a probe that moved both
    /// `countdown_label` and `sidebar_row` to `SECTION_LABEL_INSET` left the
    /// test above green and was caught only here.
    #[test]
    fn the_countdown_and_the_rows_are_both_row_inset_from_the_panel_edge() {
        let painted = painted_sidebar("Locks in 11:42");

        // The section headers are the design's 8px `SECTION_LABEL_INSET`
        // from that same edge, which is how this test locates the edge
        // without depending on egui's panel margins.
        let header = left_edge_of(&painted, "VAULT");
        let expected = header - SECTION_LABEL_INSET + ROW_INSET_X;

        let countdown = left_edge_of(&painted, "Locks in 11:42");
        assert!(
            (countdown - expected).abs() < 0.5,
            "the countdown starts at x={countdown}, expected {expected} \
             ({ROW_INSET_X} in from the panel edge the VAULT header sits \
             {SECTION_LABEL_INSET} in from, at x={header})"
        );
    }

    /// A vault of known composition: three items in no folder at all, two in
    /// the real folder `f1`. Used by the tests below so their expected
    /// numbers are absolute (3 and 2), not restatements of whatever the code
    /// under test happens to compute.
    fn three_unfiled_and_two_filed() -> Vec<VaultItem> {
        vec![
            item(Some(1), false, None),
            item(Some(1), false, None),
            item(Some(2), false, None),
            item(Some(1), false, Some("f1")),
            item(Some(3), false, Some("f1")),
        ]
    }

    /// The real folder plus `bw serve`'s virtual "No Folder" bucket, which it
    /// reports as a folder with an empty id.
    fn one_real_folder_and_the_virtual_bucket() -> Vec<Folder> {
        vec![
            Folder {
                id: "f1".into(),
                name: "Engineering".into(),
                other: serde_json::Map::new(),
            },
            Folder {
                id: String::new(),
                name: "No Folder".into(),
                other: serde_json::Map::new(),
            },
        ]
    }

    /// The one painted string sitting on the same row as `label` but not the
    /// label itself -- i.e. that row's right-aligned count badge.
    fn badge_beside(painted: &[(String, egui::Rect)], label: &str) -> String {
        let row_y = painted
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, rect)| rect.center().y)
            .unwrap_or_else(|| panic!("the sidebar painted no {label:?}: {painted:?}"));
        // Half a row: consecutive rows are ROW_HEIGHT + ROW_GAP apart, so
        // nothing from a neighbouring row can fall inside this window.
        let same_row: Vec<&(String, egui::Rect)> = painted
            .iter()
            .filter(|(text, rect)| {
                text != label && (rect.center().y - row_y).abs() < ROW_HEIGHT / 2.0
            })
            .collect();
        match same_row.as_slice() {
            [(badge, _)] => badge.clone(),
            other => panic!("expected exactly one badge beside {label:?}, got {other:?}"),
        }
    }

    /// The user-reported defect, at the level the user reported it: "No
    /// folder should show all records that are not in Folders". Against a
    /// vault where 3 of 5 items are unfiled, that row's badge read **0** --
    /// because the row's filter was `Folder("")` (the virtual bucket's empty
    /// id used as if it were a real folder id) and unfiled items carry
    /// `folder_id: None`, which is not `Some("")`.
    #[test]
    fn the_virtual_no_folder_rows_badge_shows_the_unfiled_count() {
        let (painted, _, _) = painted_sidebar_fixture(
            "Locks in 11:42",
            three_unfiled_and_two_filed(),
            one_real_folder_and_the_virtual_bucket(),
        );

        assert_eq!(
            badge_beside(&painted, "No Folder"),
            "3",
            "3 of the 5 fixture items are in no folder, so the virtual row's \
             badge must read 3"
        );
        // The real folder's badge is asserted too, so a change that made
        // *every* folder row count the unfiled items could not pass.
        assert_eq!(badge_beside(&painted, "Engineering"), "2");
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

    /// `Unfiled` means `folder_id: None`, and nothing else.
    #[test]
    fn unfiled_contains_exactly_the_items_in_no_folder() {
        assert!(SidebarFilter::Unfiled.scope_contains(&item(Some(1), false, None)));
        assert!(!SidebarFilter::Unfiled.scope_contains(&item(Some(1), false, Some("f1"))));
    }

    /// The regression that would have caught the original defect: an unfiled
    /// item is not in `Folder(_)` for *any* id -- least of all the empty one
    /// the virtual bucket is reported with, which is precisely what the
    /// FOLDERS loop used to hand it.
    #[test]
    fn an_unfiled_item_is_in_no_folder_filter_including_the_empty_id() {
        let unfiled = item(Some(1), false, None);
        assert!(!SidebarFilter::Folder(String::new()).scope_contains(&unfiled));
        assert!(!SidebarFilter::Folder("f1".to_string()).scope_contains(&unfiled));
    }

    /// ...and the converse: `Unfiled` is not a synonym for "the empty-id
    /// folder". An item that somehow carried `folder_id: Some("")` belongs to
    /// `Folder("")`, not here -- see the arm's comment for why that case is
    /// left alone rather than folded in.
    #[test]
    fn an_empty_string_folder_id_is_not_unfiled() {
        let empty_id = item(Some(1), false, Some(""));
        assert!(!SidebarFilter::Unfiled.scope_contains(&empty_id));
        assert!(SidebarFilter::Folder(String::new()).scope_contains(&empty_id));
    }

    /// The count behind the badge, over a fixture of known composition:
    /// 3 unfiled and 2 filed. The old `Folder("")` returned 0 here.
    #[test]
    fn unfiled_counts_the_items_in_no_folder_rather_than_none() {
        let items = three_unfiled_and_two_filed();
        assert_eq!(count_for(&items, &SidebarFilter::Unfiled), 3);
        assert_eq!(count_for(&items, &SidebarFilter::Folder("f1".to_string())), 2);
        assert_eq!(count_for(&items, &SidebarFilter::Folder(String::new())), 0);
        assert_eq!(count_for(&items, &SidebarFilter::All), 5);
    }
}

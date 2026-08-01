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
    /// The items this app can fill into a Windows application -- the ones
    /// carrying an app match.
    ///
    /// **Membership is [`crate::vault_bridge::extract_app_match`] and nothing
    /// else.** That function reads one custom field
    /// (`APP_MATCH_FIELD_NAME`) out of `item.fields` and parses it, and it is
    /// already the single definition the picker, the match engine and the
    /// detail pane's AUTOFILL TARGETS card all use. A second notion of "has
    /// an app" here -- "any field whose name looks like ours", "a non-empty
    /// value", "the field exists" -- would be a row that disagreed with the
    /// pane it sends the user to, which is the two-copies-that-happen-to-
    /// agree hazard [`SidebarFilter::scope_contains`]'s own doc names. A
    /// field this build cannot PARSE is deliberately not on this row: the
    /// rest of the app cannot fill from it either, so listing it would
    /// promise an autofill that will not happen.
    Apps,
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
            // The EXISTING definition of "this item has an app", not a
            // second one -- see the variant's doc.
            SidebarFilter::Apps => crate::vault_bridge::extract_app_match(item).is_some(),
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
    /// An item row was dragged out of the item list and dropped on a folder
    /// row that will take it. Both ids are carried; the caller resolves the
    /// item itself.
    MoveItemToFolder { item_id: String, folder_id: String },
    /// An item row was dropped on a folder row that will NOT take it,
    /// carrying the reason for the caller to show inline.
    ///
    /// Its own variant, and not simply `None`, because the whole point of the
    /// refusal is that it is not silent -- see [`DropOutcome`].
    RefusedMove(&'static str),
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
            // Directly after Favorites, by the user's explicit instruction
            // ("it is our main feature"). Design 2b has no such row -- it
            // predates app matching -- so the placement is theirs and the
            // styling is the neighbouring rows'.
            ("Apps", SidebarFilter::Apps),
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
        // What is being dragged over this window right now, if anything, and
        // what each folder row would do with it. Both read ONCE, before the
        // loop: `drop_outcomes` returns one verdict per folder in this exact
        // order, and it is the single place that decision is made -- see its
        // doc for why it cannot live inside the loop.
        let dragged = egui::DragAndDrop::payload::<crate::vault_window::item_list::DraggedItem>(
            ui.ctx(),
        );
        let outcomes = dragged.as_ref().map(|item| drop_outcomes(folders, item));
        for (index, folder) in folders.iter().enumerate() {
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
            // The drop half of the row. Painted AFTER `sidebar_row` and as an
            // OUTLINE rather than a fill, deliberately: the row has already
            // painted its own label and count, and a filled wash here would
            // cover them. Nothing is allocated -- the outline goes onto the
            // rect the row already claimed -- so this cannot change the
            // sidebar's row pitch.
            if let Some(outcome) = outcomes.as_ref().map(|o| &o[index]) {
                let (color, width) = match outcome {
                    // Every folder that would take the item says so for the
                    // whole drag, not only under the pointer: a target the
                    // user has to guess at is a target they will drop next
                    // to. The one under the pointer is drawn heavier.
                    DropOutcome::Accept => {
                        (theme::BLUE, if response.contains_pointer() { 2.0 } else { 1.0 })
                    }
                    // REFUSED, VISIBLY -- not inert. A row that looked
                    // identical to its neighbours and quietly swallowed the
                    // gesture is the silent no-op this window keeps having to
                    // un-write; see `CANNOT_UNFILE`.
                    DropOutcome::Refuse(_) => {
                        (theme::ERROR, if response.contains_pointer() { 2.0 } else { 1.0 })
                    }
                };
                ui.painter().rect_stroke(
                    response.rect,
                    CornerRadius::same(8),
                    egui::Stroke::new(width, color),
                    egui::StrokeKind::Inside,
                );
                if response.contains_pointer() {
                    ui.ctx().set_cursor_icon(match outcome {
                        DropOutcome::Accept => egui::CursorIcon::Grabbing,
                        DropOutcome::Refuse(_) => egui::CursorIcon::NotAllowed,
                    });
                }
            }
            // Consumed on EVERY folder row, accepting or not: the gesture
            // ended here, and leaving the payload on the clipboard for a
            // refused drop would let it be picked up by whatever the pointer
            // crossed next.
            if let Some(item) =
                response.dnd_release_payload::<crate::vault_window::item_list::DraggedItem>()
            {
                // Re-read rather than reused from `outcomes` above, which was
                // computed from the payload as it stood at the top of this
                // function; they agree, and that is exactly why neither has to
                // be trusted to.
                action = match drop_outcomes(folders, &item)[index] {
                    DropOutcome::Accept => SidebarAction::MoveItemToFolder {
                        item_id: item.id.clone(),
                        folder_id: folder.id.clone(),
                    },
                    DropOutcome::Refuse(reason) => SidebarAction::RefusedMove(reason),
                };
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

/// What a folder row does when an item is released on it.
///
/// Two variants, and a refusal that carries its reason, because "nothing
/// happened" and "this cannot happen, here is why" are different states and
/// collapsing them is the failure this window keeps having to un-write. There
/// is deliberately no third "silently ignore" variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropOutcome {
    /// The move will be attempted.
    Accept,
    /// The drop is refused, out loud. The string is shown to the user.
    Refuse(&'static str),
}

/// Why the virtual "No Folder" row refuses every drop.
///
/// **Not a UI decision and not fixable in this crate.** `bw serve` (CLI
/// 2026.7.0) cannot clear a folder assignment: omitting `folderId`, sending
/// `null`, sending `""` and sending a fully round-tripped object were each
/// measured against a control field that DID change in the same request
/// (`.superpowers/sdd/put-semantics-capture.md`). A drop here would return
/// success and leave the item exactly where it was. The row therefore refuses
/// visibly rather than being made inert: an inert row that looks like every
/// other folder row is the same silent no-op with the explanation removed.
///
/// Worth re-testing after a CLI upgrade -- current `bitwarden/clients` main
/// assigns `folderId` unconditionally, so this looks like a version
/// difference rather than a permanent property of the API.
pub const CANNOT_UNFILE: &str =
    "\"No Folder\" isn't a place an item can be moved to -- the Bitwarden CLI this build talks \
     to cannot take an item out of a folder, so the drop was refused rather than looking like \
     it worked.";

/// Why the folder an item already lives in refuses it. The same fact the
/// row menu's "Move to folder" submenu greys that destination for.
pub const ALREADY_IN_THIS_FOLDER: &str = "That item is already in this folder.";

/// What each of `folders` does when `dragged` is released on it -- **one
/// entry per folder, in the order the sidebar draws them**, which is what
/// lets the FOLDERS loop zip this against itself.
///
/// Built on [`assignable_folders`], the EXISTING predicate for "folders an
/// item can be moved into", rather than on a second copy of the same
/// `is_virtual_folder` filter: one definition of "virtual" is what keeps this,
/// the row menu's submenu and the edit form's dropdown from drifting apart.
///
/// Pure, and the single source of what a drop does, because nothing in this
/// crate can call [`draw_sidebar`] with a live drag in flight without a real
/// egui context -- a per-folder decision made inside that loop would be one
/// no unit test could reach.
///
/// [`assignable_folders`]: super::detail_edit::assignable_folders
pub fn drop_outcomes(folders: &[Folder], dragged: &crate::vault_window::item_list::DraggedItem) -> Vec<DropOutcome> {
    let assignable = super::detail_edit::assignable_folders(folders);
    folders
        .iter()
        .map(|folder| {
            if !assignable.iter().any(|f| f.id == folder.id) {
                // The only way `assignable_folders` drops a row is
                // `is_virtual_folder`, so this is the un-file case.
                return DropOutcome::Refuse(CANNOT_UNFILE);
            }
            if dragged.folder_id.as_deref() == Some(folder.id.as_str()) {
                return DropOutcome::Refuse(ALREADY_IN_THIS_FOLDER);
            }
            DropOutcome::Accept
        })
        .collect()
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
mod drop_outcome_tests {
    //! What each folder row does when an item is dragged onto it.
    //!
    //! Every assertion reads the WHOLE outcome list, one entry per folder in
    //! the order the sidebar draws them -- not "is Work an accepting
    //! target". A test that probed only for what it expected would pass just
    //! as happily against a sidebar that ALSO accepted the virtual bucket,
    //! which is the one outcome here that cannot be allowed.
    use super::*;
    use crate::vault_window::item_list::DraggedItem;

    fn folder(id: &str, name: &str) -> Folder {
        Folder { id: id.into(), name: name.into(), other: serde_json::Map::new() }
    }

    fn dragged(folder_id: Option<&str>) -> DraggedItem {
        DraggedItem { id: "i1".into(), folder_id: folder_id.map(str::to_string) }
    }

    /// The list `bw serve` really returns: its virtual "No Folder" bucket
    /// (empty id) alongside real folders.
    fn a_real_looking_vault() -> Vec<Folder> {
        vec![folder("", "No Folder"), folder("f1", "Work"), folder("f2", "Personal")]
    }

    #[test]
    fn an_unfiled_item_may_go_into_any_real_folder_and_nowhere_else() {
        assert_eq!(
            drop_outcomes(&a_real_looking_vault(), &dragged(None)),
            vec![
                DropOutcome::Refuse(CANNOT_UNFILE),
                DropOutcome::Accept,
                DropOutcome::Accept,
            ]
        );
    }

    #[test]
    fn the_virtual_no_folder_bucket_is_never_a_working_target() {
        // The measured fact this whole feature is shaped around: `bw serve`
        // (CLI 2026.7.0) cannot clear a folder assignment. Omitting the key,
        // `null`, `""` and a fully round-tripped object were each proven
        // against a control field that DID change in the same request. A drop
        // there would write successfully and do nothing, which is the exact
        // silent no-op this project keeps finding -- so it is refused, out
        // loud, with a reason.
        for from in [None, Some("f1"), Some("f2")] {
            assert_eq!(
                drop_outcomes(&a_real_looking_vault(), &dragged(from))[0],
                DropOutcome::Refuse(CANNOT_UNFILE),
                "the virtual bucket accepted an item dragged from {from:?}"
            );
        }
    }

    #[test]
    fn the_folder_the_item_already_lives_in_is_refused_for_its_own_reason() {
        // A write that achieves nothing, and a different refusal from the
        // un-file one -- the user needs to be told which of the two happened.
        assert_eq!(
            drop_outcomes(&a_real_looking_vault(), &dragged(Some("f1"))),
            vec![
                DropOutcome::Refuse(CANNOT_UNFILE),
                DropOutcome::Refuse(ALREADY_IN_THIS_FOLDER),
                DropOutcome::Accept,
            ]
        );
    }

    #[test]
    fn there_is_exactly_one_outcome_per_folder_in_the_order_they_are_drawn() {
        // The sidebar zips this list against its own FOLDERS loop, so a
        // shorter or reordered result would attach one folder's verdict to
        // another folder's row.
        let folders = a_real_looking_vault();
        let outcomes = drop_outcomes(&folders, &dragged(Some("f2")));
        assert_eq!(outcomes.len(), folders.len());
        assert_eq!(outcomes[2], DropOutcome::Refuse(ALREADY_IN_THIS_FOLDER));
    }

    #[test]
    fn a_vault_with_no_real_folders_accepts_nothing() {
        assert_eq!(
            drop_outcomes(&[folder("", "No Folder")], &dragged(None)),
            vec![DropOutcome::Refuse(CANNOT_UNFILE)]
        );
        assert_eq!(drop_outcomes(&[], &dragged(None)), Vec::<DropOutcome>::new());
    }

    #[test]
    fn the_accepting_targets_are_exactly_the_assignable_folders_minus_the_current_one() {
        // Tied to `detail_edit::assignable_folders`, the EXISTING predicate
        // for "folders an item can be moved into", rather than to a second
        // copy of the same filter -- one definition of "virtual" is what
        // keeps this, the row menu's submenu and the edit form's dropdown
        // from drifting apart.
        let folders = a_real_looking_vault();
        let accepting: Vec<&str> = drop_outcomes(&folders, &dragged(Some("f1")))
            .iter()
            .zip(&folders)
            .filter(|(outcome, _)| **outcome == DropOutcome::Accept)
            .map(|(_, folder)| folder.id.as_str())
            .collect();
        assert_eq!(accepting, vec!["f2"]);
    }
}

#[cfg(test)]
mod drag_and_drop_tests {
    //! Dragging an item row onto a folder row, driven end to end.
    //!
    //! WHAT THIS HARNESS IS AND IS NOT. It draws the REAL [`draw_sidebar`]
    //! and the REAL `item_list::draw_item_list` side by side in ONE
    //! `egui::Context`, and pushes real pointer events through them, so the
    //! drag payload, the drop detection and the refusals are all exercised as
    //! they ship. What it is not is `vault_window::run` -- that opens an OS
    //! window inside `eframe` and no test in this crate can call it -- so the
    //! two panes are laid out by this module rather than by `Panel::left`.
    //! The thing that could differ is the panes' geometry, and nothing here
    //! depends on it beyond "the sidebar is left of the item list".
    use super::*;
    use crate::vault_window::item_list::{draw_item_list, IconCache};
    use crate::theme;

    const SIDEBAR_WIDTH: f32 = 212.0;
    const LIST_WIDTH: f32 = 390.0;
    const HEIGHT: f32 = 700.0;

    fn folder(id: &str, name: &str) -> Folder {
        Folder { id: id.into(), name: name.into(), other: serde_json::Map::new() }
    }

    fn login(name: &str, folder_id: Option<&str>) -> VaultItem {
        VaultItem {
            id: name.to_string(),
            name: name.into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            notes: None,
            item_type: Some(1),
            folder_id: folder_id.map(str::to_string),
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// What one run of the harness observed.
    struct Run {
        action: SidebarAction,
        /// Every string painted on the measured frame, with its rect.
        texts: Vec<(String, egui::Rect)>,
        /// Every rect painted with a visible stroke, on the measured frame.
        strokes: Vec<(egui::Rect, egui::Stroke)>,
        /// What the item list left `selected_id` at.
        selected: Option<String>,
    }

    fn walk(shape: &egui::Shape, run: &mut Run) {
        match shape {
            egui::Shape::Text(text) => run
                .texts
                .push((text.galley.text().to_string(), egui::Rect::from_min_size(text.pos, text.galley.size()))),
            egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => {
                run.strokes.push((rect.rect, rect.stroke))
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, run);
                }
            }
            _ => {}
        }
    }

    /// Draws both panes for `frames` frames of events and returns what the
    /// LAST one produced -- the same "measure the frame the gesture resolves
    /// on" rule `item_list`'s menu harness follows.
    fn run_frames(
        items: &[VaultItem],
        folders: &[Folder],
        frames: Vec<Vec<egui::Event>>,
    ) -> Run {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(SIDEBAR_WIDTH + LIST_WIDTH, HEIGHT),
        );
        let input = || egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        // Two throwaway frames so `theme::apply`'s font set is live.
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut filter = SidebarFilter::All;
        let mut search = String::new();
        let mut selected_id: Option<String> = None;
        let icons = IconCache::default();
        let mut visible = Vec::new();
        let mut action = SidebarAction::None;
        let mut draw = |ctx: &egui::Context, raw: egui::RawInput| {
            ctx.run_ui(raw, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(SIDEBAR_WIDTH, HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            action = draw_sidebar(ui, items, folders, &mut filter, "Locks in 11:42");
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(LIST_WIDTH, HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            draw_item_list(
                                ui,
                                items,
                                folders,
                                &SidebarFilter::All,
                                &mut search,
                                &mut selected_id,
                                None,
                                &icons,
                                &mut visible,
                                None,
                            );
                        },
                    );
                });
            })
        };
        let _ = draw(&ctx, input());
        let mut output = None;
        for events in frames {
            output = Some(draw(&ctx, egui::RawInput { events, ..input() }));
        }
        let output = output.unwrap_or_else(|| draw(&ctx, input()));

        let mut run = Run {
            action,
            texts: Vec::new(),
            strokes: Vec::new(),
            selected: selected_id,
        };
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut run);
        }
        run
    }

    /// The centre of whatever painted `label` -- an item row's title or a
    /// folder row's name. Read off a real frame rather than computed from the
    /// row-height constants, so a test cannot aim at the wrong place in
    /// exactly the case (a row whose height changed) it exists to catch.
    fn centre_of(items: &[VaultItem], folders: &[Folder], label: &str) -> egui::Pos2 {
        let run = run_frames(items, folders, Vec::new());
        run.texts
            .iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| {
                panic!("{label:?} was never painted; the frame drew {:?}",
                    run.texts.iter().map(|(t, _)| t).collect::<Vec<_>>())
            })
            .1
            .center()
    }

    /// The event frames for picking a row up and letting go over `to`.
    ///
    /// The intermediate moves are not padding: egui only decides a press is a
    /// DRAG once the pointer has travelled past its threshold, and
    /// `dnd_set_drag_payload` puts the payload on the clipboard on the frame
    /// `drag_started` fires. A press-then-release with no travel is a CLICK,
    /// which is the other half of what these tests pin.
    fn drag_frames(from: egui::Pos2, to: egui::Pos2) -> Vec<Vec<egui::Event>> {
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut frames = vec![
            vec![egui::Event::PointerMoved(from)],
            vec![egui::Event::PointerMoved(from), button(from, true)],
        ];
        // Travel in a few steps, so the drag is decided well before the
        // pointer arrives.
        for step in 1..=4 {
            let t = step as f32 / 4.0;
            frames.push(vec![egui::Event::PointerMoved(from + (to - from) * t)]);
        }
        frames.push(vec![egui::Event::PointerMoved(to), button(to, false)]);
        frames
    }

    fn a_vault() -> (Vec<VaultItem>, Vec<Folder>) {
        (
            vec![login("Ledgerline", None), login("Vantage", Some("f1"))],
            vec![folder("", "No Folder"), folder("f1", "Work"), folder("f2", "Personal")],
        )
    }

    #[test]
    fn dropping_an_item_on_a_real_folder_asks_for_that_move() {
        let (items, folders) = a_vault();
        let from = centre_of(&items, &folders, "Ledgerline");
        let to = centre_of(&items, &folders, "Personal");
        let run = run_frames(&items, &folders, drag_frames(from, to));
        assert_eq!(
            run.action,
            SidebarAction::MoveItemToFolder {
                item_id: "Ledgerline".to_string(),
                folder_id: "f2".to_string(),
            }
        );
    }

    #[test]
    fn dropping_an_item_on_the_virtual_no_folder_row_is_refused_out_loud() {
        // THE DECISION THIS FEATURE TURNS ON. `bw serve` cannot un-file an
        // item, so the choice was between an inert row and a visibly refused
        // one. Inert loses: a row that looks exactly like the two below it
        // and swallows the gesture is the silent no-op this project keeps
        // finding. The drop is consumed, refused, and the reason is handed
        // back for the caller to show.
        let (items, folders) = a_vault();
        let from = centre_of(&items, &folders, "Vantage");
        let to = centre_of(&items, &folders, "No Folder");
        let run = run_frames(&items, &folders, drag_frames(from, to));
        assert_eq!(run.action, SidebarAction::RefusedMove(CANNOT_UNFILE));
    }

    #[test]
    fn dropping_an_item_on_the_folder_it_already_lives_in_is_refused_for_its_own_reason() {
        let (items, folders) = a_vault();
        let from = centre_of(&items, &folders, "Vantage"); // already in "f1"
        let to = centre_of(&items, &folders, "Work");
        let run = run_frames(&items, &folders, drag_frames(from, to));
        assert_eq!(run.action, SidebarAction::RefusedMove(ALREADY_IN_THIS_FOLDER));
    }

    #[test]
    fn a_drag_that_ends_on_a_vault_row_moves_nothing() {
        // "All items" is not a folder and must not behave like one. Nothing
        // there paints a drop highlight, so this is not the silent-no-op
        // case: there is no affordance offered and none taken away.
        let (items, folders) = a_vault();
        let from = centre_of(&items, &folders, "Ledgerline");
        let to = centre_of(&items, &folders, "All items");
        let run = run_frames(&items, &folders, drag_frames(from, to));
        assert_eq!(run.action, SidebarAction::None);
    }

    #[test]
    fn dragging_a_row_does_not_select_it() {
        // How drag sensing coexists with row selection: a press that travels
        // is a drag and NOT a click, so the selection (and everything
        // `vault_window::run` resets off it -- the open draft, the revealed
        // password, the TOTP poll) is left alone by a gesture that was only
        // ever about moving the item.
        let (items, folders) = a_vault();
        let from = centre_of(&items, &folders, "Ledgerline");
        let to = centre_of(&items, &folders, "Personal");
        let run = run_frames(&items, &folders, drag_frames(from, to));
        // The move is asserted in the same breath, deliberately: "nothing was
        // selected" is also true of a build where the drag never happened, so
        // on its own this would be a test that cannot fail. Pinned together,
        // it can only pass when the gesture WAS a drag and was NOT a click.
        assert_eq!(
            run.action,
            SidebarAction::MoveItemToFolder {
                item_id: "Ledgerline".to_string(),
                folder_id: "f2".to_string(),
            }
        );
        assert_eq!(run.selected, None, "the drag selected the row it picked up");
    }

    #[test]
    fn a_press_that_does_not_travel_still_selects_the_row() {
        // The other half of the same rule, and the pre-existing behaviour:
        // `Sense::click_and_drag` must not have quietly turned every click
        // into a drag.
        let (items, folders) = a_vault();
        let at = centre_of(&items, &folders, "Ledgerline");
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let run = run_frames(
            &items,
            &folders,
            vec![
                vec![egui::Event::PointerMoved(at), button(true)],
                vec![button(false)],
            ],
        );
        assert_eq!(run.selected.as_deref(), Some("Ledgerline"));
    }

    /// The rect of the folder row whose name is `label`, found from that
    /// name's own galley -- the row is a painted band, not a widget with a
    /// rect this harness can ask for.
    fn row_band(run: &Run, label: &str) -> egui::Rect {
        let text = run
            .texts
            .iter()
            .find(|(t, _)| t == label)
            .unwrap_or_else(|| panic!("{label:?} was never painted"))
            .1;
        run.strokes
            .iter()
            .map(|(rect, _)| *rect)
            .find(|rect| rect.contains_rect(text) && rect.width() < SIDEBAR_WIDTH)
            .unwrap_or_else(|| panic!("no outlined row band around {label:?}"))
    }

    #[test]
    fn a_drag_in_flight_outlines_the_folders_that_would_take_it_and_the_one_that_would_not() {
        // The visible half of "refused, not inert". Mid-drag, every folder
        // row states its verdict: the two that would accept are outlined in
        // the accent colour, the virtual bucket in the error colour. A test
        // that only checked the drop's RESULT would pass against a sidebar
        // that looked completely dead while the item was in the air.
        let (items, folders) = a_vault();
        let from = centre_of(&items, &folders, "Vantage"); // in "f1"
        let to = centre_of(&items, &folders, "Personal");
        // Stop one frame short of the release, so this reads the drag in
        // flight rather than its outcome.
        let mut frames = drag_frames(from, to);
        frames.pop();
        frames.push(vec![egui::Event::PointerMoved(to)]);
        let run = run_frames(&items, &folders, frames);

        let stroke_of = |label: &str| {
            let band = row_band(&run, label);
            run.strokes
                .iter()
                .find(|(rect, _)| *rect == band)
                .expect("band vanished")
                .1
                .color
        };
        assert_eq!(stroke_of("No Folder"), theme::ERROR, "the virtual bucket did not refuse visibly");
        assert_eq!(stroke_of("Work"), theme::ERROR, "the item's own folder did not refuse visibly");
        assert_eq!(stroke_of("Personal"), theme::BLUE, "an accepting folder was not offered");
    }

    #[test]
    fn nothing_is_outlined_when_no_drag_is_in_flight() {
        // Otherwise the assertion above would be about permanent decoration.
        let (items, folders) = a_vault();
        let run = run_frames(&items, &folders, Vec::new());
        for label in ["No Folder", "Work", "Personal"] {
            let text = run.texts.iter().find(|(t, _)| t == label).expect("row missing").1;
            assert!(
                !run.strokes.iter().any(|(rect, _)| rect.contains_rect(text) && rect.width() < SIDEBAR_WIDTH),
                "{label:?} is outlined with no drag in flight"
            );
        }
    }
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

    /// The Apps row, against the storage the rest of the app already uses.
    ///
    /// The three items are built by `vault_bridge::with_app_match` -- the
    /// function the picker writes an app match with -- rather than by hand-
    /// assembling a `VaultField`, so this test fails if the row's predicate
    /// and the writer ever stop agreeing about the field's name or value
    /// format. A hand-built fixture would keep passing against exactly that
    /// break.
    #[test]
    fn apps_counts_the_items_carrying_an_app_match() {
        use crate::app_match::{AppMatch, TriggerMode};
        use crate::vault_bridge::with_app_match;

        let matched = with_app_match(
            &item(Some(1), false, None),
            &AppMatch { process: "Ledgerline.exe".into(), trigger: TriggerMode::Prompt },
        );
        let also_matched = with_app_match(
            &item(Some(3), false, None),
            &AppMatch { process: "Vantage.exe".into(), trigger: TriggerMode::Auto },
        );
        let plain = item(Some(1), false, None);

        let items = vec![matched.clone(), also_matched, plain.clone()];
        // Absolute: 2 of the 3 fixture items were given an app match.
        assert_eq!(count_for(&items, &SidebarFilter::Apps), 2);
        // ...and per item, so a count that happened to come out right for the
        // wrong reason cannot pass.
        assert!(SidebarFilter::Apps.scope_contains(&matched));
        assert!(!SidebarFilter::Apps.scope_contains(&plain));
        // The row cuts across kinds -- a card with an app match is on it --
        // so it must not have quietly become "logins that have a field".
        assert_eq!(count_for(&items, &SidebarFilter::Logins), 2);
        assert_eq!(count_for(&items, &SidebarFilter::All), 3);
    }

    /// A field with the right NAME but a value this build cannot parse is not
    /// on the Apps row.
    ///
    /// This is what pins the row to `extract_app_match` rather than to "does
    /// a field called `deskwarden:app-match` exist": the two differ only
    /// here, and the difference is the point. Nothing else in this app can
    /// fill from an unparseable match, so a row that listed it would send the
    /// user to a pane that offers an autofill which cannot happen.
    #[test]
    fn a_field_with_our_name_but_an_unreadable_value_is_not_an_app() {
        use crate::app_match::APP_MATCH_FIELD_NAME;
        use crate::vault_bridge::VaultField;

        let mut broken = item(Some(1), false, None);
        broken.fields = vec![VaultField {
            name: Some(APP_MATCH_FIELD_NAME.to_string()),
            value: Some("not json".to_string()),
            other: serde_json::Map::new(),
        }];
        assert!(!SidebarFilter::Apps.scope_contains(&broken));
        assert_eq!(crate::vault_bridge::extract_app_match(&broken), None);

        // POSITIVE CONTROL: the same fixture with a value the app CAN read is
        // on the row, so this is not a test that passes against a row which
        // matches nothing at all.
        let readable = crate::vault_bridge::with_app_match(
            &broken,
            &crate::app_match::AppMatch {
                process: "Ledgerline.exe".into(),
                trigger: crate::app_match::TriggerMode::Prompt,
            },
        );
        assert!(SidebarFilter::Apps.scope_contains(&readable));
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

    /// Every VAULT-section row label, in the order they are painted down the
    /// panel -- read off a real frame, so this is the order the user sees and
    /// not a restatement of the array `draw_sidebar` loops over.
    fn vault_row_labels_in_painted_order(painted: &[(String, egui::Rect)]) -> Vec<String> {
        let folders_header_y = painted
            .iter()
            .find(|(text, _)| text == "FOLDERS")
            .map(|(_, rect)| rect.top())
            .expect("the sidebar painted no FOLDERS header");
        let mut rows: Vec<(f32, String)> = painted
            .iter()
            // Above the FOLDERS header (so folder rows are excluded), below
            // the VAULT one, and not a count badge: the badges are the only
            // other strings in that band, and they are digits.
            .filter(|(text, rect)| {
                rect.top() < folders_header_y
                    && text != "VAULT"
                    // The FOLDERS header's "+" is painted centred on that
                    // header's band, and its 18px glyph box starts a hair
                    // above the band's own top -- so it slips under the
                    // `top()` cut. Excluded by name rather than by widening
                    // the cut, which would start swallowing the SSH keys row.
                    && text != "+"
                    && !text.chars().all(|c| c.is_ascii_digit())
            })
            .map(|(text, rect)| (rect.top(), text.clone()))
            .collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        rows.into_iter().map(|(_, text)| text).collect()
    }

    /// The user's explicit instruction: Apps sits **directly after
    /// Favorites**, and it is "our main feature".
    ///
    /// Asserted against the whole painted VAULT column rather than "Apps is
    /// somewhere below Favorites": a relative check passes just as happily
    /// for a row that landed at the bottom of the section, and the whole
    /// point of the instruction was the position.
    #[test]
    fn the_apps_row_is_painted_directly_after_favorites() {
        let (painted, _, _) = painted_sidebar_and_bounds("Locks in 11:42");
        assert_eq!(
            vault_row_labels_in_painted_order(&painted),
            vec![
                "All items",
                "Favorites",
                "Apps",
                "Logins",
                "Passkeys",
                "Cards",
                "Identities",
                "Secure notes",
                "SSH keys",
                "Trash",
            ]
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

    /// Pins `ROW_INSET_X` and `SECTION_LABEL_INSET` to the actual numbers in
    /// design 4.8's CSS, not merely to each other.
    ///
    /// `the_lock_countdown_starts_on_the_same_x_as_the_row_labels` and
    /// `the_countdown_and_the_rows_are_both_row_inset_from_the_panel_edge`
    /// both compute their expected value from these two constants
    /// (`header_left - SECTION_LABEL_INSET + ROW_INSET_X`), so a change that
    /// moves a constant away from the design -- while leaving the countdown
    /// and the rows agreeing with each other -- leaves both of those tests
    /// green. Demonstrated: bumping `ROW_INSET_X` from `10.0` to `16.0`, or
    /// `SECTION_LABEL_INSET` from `8.0` to `20.0`, does not fail either test,
    /// because both quantities cancel out of the comparison. Neither test is
    /// wrong -- they correctly check that the countdown and the rows line up
    /// with each other -- but nothing before this test checked that the
    /// value they line up *on* is the one the design specifies.
    ///
    /// Design 4.8, block `2b`: each sidebar row is `padding: 8px 10px`
    /// (`ROW_INSET_X` is the 10px), and the section header is `padding: 0 8px
    /// 8px` (`SECTION_LABEL_INSET` is the 8px) -- see the doc comments on
    /// both constants, which already cite the same source.
    #[test]
    fn the_row_and_section_insets_match_design_4_8s_padding_not_just_each_other() {
        assert_eq!(ROW_INSET_X, 10.0, "design 4.8 block 2b: row `padding: 8px 10px`");
        assert_eq!(
            SECTION_LABEL_INSET, 8.0,
            "design 4.8 block 2b: section header `padding: 0 8px 8px`"
        );
    }
}

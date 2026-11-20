//! The vault window's middle pane: search box, `+ New`, and the virtualized
//! item list (design 4.8 "Item list"). Virtualized the same way
//! `picker_ui`'s lists are (`ScrollArea::show_rows`) -- a real vault can be
//! in the thousands, and laying out every row on every repaint was already
//! a confirmed source of a laggy picker before that fix.

use super::detail::{kind_offers_edit, kind_offers_fill};
use super::detail_edit::assignable_folders;
use super::sidebar::{FilterSource, OutOfVault, SidebarFilter};
use crate::card_brand::{brand_for_number, CardBrand};
use crate::theme;
use crate::vault_bridge::{Folder, ItemKind, VaultItem};
use crate::kit::logos::{Bank, Mark, Net};
use crate::kit::{self, motion, tokens};
use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke};
use std::collections::HashMap;

/// The window's in-memory icon state. Owned by `vault_window::mod` (Task 9),
/// which populates it from the background favicon loader; this module only
/// ever reads [`Self::textures`], and never touches [`Self::refreshing`] at
/// all.
#[derive(Default)]
pub struct IconCache {
    /// Loaded favicon textures, keyed by item id.
    pub textures: HashMap<String, egui::TextureHandle>,
    /// The ids whose next icon fetch must bypass the icon proxy's own cache
    /// -- "Refresh icon" was chosen on that row.
    ///
    /// **Here rather than as an eighth parameter to `ensure_icon_loaded`,**
    /// which is already at clippy's argument limit (see that function's
    /// `IconFetch` doc). It also belongs here on the merits: a refresh drops
    /// the texture and marks the id, and those are one act on one piece of
    /// per-item icon state rather than two facts kept in two places that can
    /// disagree.
    ///
    /// **Drained, not read.** `ensure_icon_loaded` removes the id at the
    /// moment it dispatches, so a refresh buys exactly one refreshing
    /// request; every later frame asks the ordinary way and the proxy is
    /// allowed to serve what it has just re-fetched. A flag that stayed set
    /// would make one menu click into a permanent cache-buster on that row.
    pub refreshing: std::collections::HashSet<String>,
    /// The websites each item's icon was resolved for -- what
    /// `favicon::icon_authorities_for` answered -- keyed by item id. When an
    /// item's answer changes, its texture and "already asked" mark are
    /// dropped and the icon is looked up again; see `vault_window`'s
    /// `forget_icon_if_websites_changed`.
    pub sites: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemListAction {
    None,
    /// A kind was picked from the `+ New` button's type menu.
    ///
    /// The kind is carried rather than assumed: `+ New` used to create a
    /// login on the spot, and the caller's job is now to open a blank draft
    /// **of this kind** (`EditDraft::empty_of`). Always one of
    /// [`detail_edit::CREATABLE_KINDS`] -- the menu has no other rows to
    /// click, which is why that array is what the rows are built from.
    ///
    /// [`detail_edit::CREATABLE_KINDS`]: super::detail_edit::CREATABLE_KINDS
    NewItem(ItemKind),
    /// The `+ New` menu's "Import from a Send..." row was chosen.
    ///
    /// Carries no kind, unlike [`Self::NewItem`]: the item an import creates
    /// takes its shape from the payload that arrives, and this menu has no
    /// say in it. It is also the one entry here that needs NO SELECTION --
    /// the point of it is to create something that is not there yet.
    ImportFromSend,
    /// The inline "that move did not happen" band was clicked away.
    DismissMoveError,
    /// The offline banner's Retry: sync now.
    RetrySync,
    /// Spec C §3's "Empty trash": the window asks, then purges every item.
    EmptyTrash,
    /// An entry of some row's right-click menu was chosen.
    ///
    /// The item's id is carried rather than left to `selected_id`, even
    /// though a right-click also selects the row (see [`item_row`]): the two
    /// are then belt and braces, and nothing about this action depends on
    /// the caller having applied that selection before it acts.
    Row { id: String, command: RowCommand },
}

/// What choosing an entry in an item row's right-click menu asks
/// `vault_window::mod` to do.
///
/// Its own enum rather than `detail::DetailAction`, which it overlaps on
/// several entries. This menu still has to express things the detail pane
/// cannot -- Archive, Unarchive, Restore and Purge, all of which act on items
/// that are not in the live vault the pane draws -- so folding the two enums
/// together would give `DetailAction`'s exhaustive match at the call site
/// arms the read pane can never produce.
///
/// **"Move this item into that folder" used to be on that list and no longer
/// is**: the detail pane's kebab now offers the same move, reporting
/// `DetailAction::MoveToFolder`. The two enums each keep their own variant,
/// but only one *destination list* exists -- both menus call [`move_menu`],
/// and both effects land in `vault_window::mod`'s `move_item_into_folder`.
/// Two enums naming the same operation is cheap; two implementations of it
/// would not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowCommand {
    CopyUsername,
    CopyPassword,
    CopyTotp,
    /// Carries the URL, resolved from the item when the menu is built by the
    /// same rule the detail pane's AUTOFILL TARGETS card uses.
    OpenWebsite(String),
    /// Forget this item's site icon everywhere it is remembered and fetch it
    /// again -- see `vault_window::mod`'s `refresh_item_icon`.
    ///
    /// Carries nothing. The domain is `favicon::icon_authority_for`'s answer
    /// for the item, and the handler asks it the same way the loader does
    /// rather than being handed a copy taken when the menu was built: the two
    /// are separated by however long the menu stays open, and a stale copy
    /// would delete the wrong file.
    RefreshIcon,
    /// Open the "Select icon" modal on this row -- see
    /// `vault_window::icon_modal`.
    ///
    /// Carries nothing, [`Self::RefreshIcon`]'s rule: the modal is seeded
    /// from the item the handler looks up by id, not from a copy taken while
    /// the menu was open.
    ///
    /// **It only OPENS something.** No dialog is shown and no vault write is
    /// made from the menu: `IFileOpenDialog::Show` is modal and pumps its own
    /// message loop, and this command is produced from inside
    /// `response.context_menu`'s draw closure, so shelling out to the shell
    /// from here would re-enter egui's frame. The modal's own action handler
    /// is where the dialog opens, which is the same rule
    /// `EditAction::PickAppFile` and `TotpAddAction::OpenImage` already
    /// follow.
    SelectIcon,
    /// Take the chosen icon back off this item, so it wears whatever the
    /// automatic path gives it.
    ///
    /// **Its own command rather than an argument to [`Self::SelectIcon`],**
    /// because it is the one icon action that writes to the vault directly
    /// with nothing to ask the user first. Folding it into the modal would
    /// mean opening a modal to press a button that closes it.
    ClearIcon,
    Edit,
    /// The destination folder's id, always a real assignable folder --
    /// see [`move_menu`].
    MoveToFolder(String),
    /// **Take the item out of every folder** -- the "No folder" destination
    /// at the top of the same submenu.
    ///
    /// Its own variant rather than a `MoveToFolder("")`, because an empty
    /// folder id is a real and different thing: `bw serve` reports a virtual
    /// "No Folder" bucket whose id IS the empty string, writing it strands
    /// the item out of every sidebar row, and
    /// `the_move_submenu_never_offers_an_empty_folder_id` exists to keep it
    /// out of that payload. A variant carrying nothing cannot be confused
    /// with a folder that happens to be named nothing.
    ///
    /// **Offered only where it works.** `VaultBackend::can_unfile_items`
    /// answers `false` for `bw serve`, which accepts every spelling of "no
    /// folder" with a 200 and moves nothing -- see [`move_menu`].
    Unfile,
    Delete,
    /// Put a live item into the archive.
    Archive,
    /// Take an archived item back out. The backend route is `restore`, not
    /// `unarchive` -- see `VaultBridge::unarchive_item` -- but the user-facing
    /// action is its own thing and is named for what it does.
    Unarchive,
    /// Take a trashed item out of the trash.
    Restore,
    /// Delete a trashed item for good. Two-click confirmed, like [`Self::Delete`].
    PurgeForever,
}

impl RowCommand {
    /// Whether this command **destroys something**, and so should be drawn in
    /// [`theme::ERROR`] wherever a menu offers it -- see [`menu_command`].
    ///
    /// **Derived from the command rather than carried as a field on
    /// [`MenuCommand`],** which is the whole point of putting it here. Four
    /// separate places build a `MenuCommand` -- [`plain_command`],
    /// [`enabled_command`], [`move_menu`]'s two arms and [`icon_menu`]'s
    /// three rows -- and a `destructive: bool` they each had to set would be
    /// a field one of them eventually got wrong, silently, in the direction
    /// that matters: a Delete that forgot to say so looks like Copy username
    /// and is one misclick from the modal. Deriving it means a new
    /// destructive variant is red in every menu that offers it the moment it
    /// is added to this match, and cannot be red in one surface and plain in
    /// another.
    ///
    /// **Two variants, and the line is "this ends the item", not "this
    /// writes".** [`Self::Archive`] takes an item out of the working vault
    /// and [`Self::ClearIcon`] throws away a chosen picture, and neither is
    /// here: both are one menu click from being undone, and a menu whose
    /// every consequential row is red is a menu with no warning colour left.
    /// The two that are here are the two that go through
    /// `super::delete_modal` -- the app already treats "needs a sentence and
    /// a confirmation" and "is destructive" as the same set, and this keeps
    /// the colour on that same set rather than inventing a second one.
    pub(super) fn is_destructive(&self) -> bool {
        matches!(self, Self::Delete | Self::PurgeForever)
    }

    /// Whether the command writes to the record -- every one a received,
    /// view-only record refuses (spec 6.9). A refreshed favicon is a local
    /// cache, not the record; choosing or clearing one writes a custom field.
    ///
    /// **Move to folder is here although the mock keeps it** on a received
    /// record ("folders are yours, so Move to folder stays"). A folder is
    /// per-user on the wire, but NodeWarden's slice 1 refuses every write to a
    /// read-only collection's cipher, `partial` included (spec 10.3), so the
    /// move would meet a refusal -- the plan's decision 5 hides it until the
    /// server has a per-user folder route.
    ///
    /// **The writers are listed, not the readers.** A new command forgotten
    /// here is offered on a view-only record and the server refuses it -- the
    /// courtesy `vault_bridge::may_edit` describes, lost for one command. A
    /// reader list forgotten the same way would silently take a harmless Copy
    /// off every received record's menu, which nobody would report as a bug.
    ///
    /// Read twice: [`menu_entries`] leaves these out of a view-only record's
    /// menu, and `vault_window::mod`'s row-command gate drops one that
    /// arrives anyway -- from the pane's own ⋮, or a menu drawn a frame
    /// before a sync turned the record view-only.
    #[must_use]
    pub(crate) fn writes_the_record(&self) -> bool {
        matches!(
            self,
            Self::SelectIcon
                | Self::ClearIcon
                | Self::Edit
                | Self::MoveToFolder(_)
                | Self::Unfile
                | Self::Delete
                | Self::Archive
                | Self::Unarchive
                | Self::Restore
                | Self::PurgeForever
        )
    }
}

/// What an item row puts on egui's drag-and-drop clipboard while it is being
/// dragged, and what the sidebar's folder rows read back.
///
/// A named type, not a bare `String`: egui's payload store is keyed by type
/// (`DragAndDrop::payload::<T>`), so this is what makes "is the thing being
/// dragged an item row" answerable at all. A `String` payload would collide
/// with any other string-shaped drag this window ever grows.
///
/// It carries the item's CURRENT folder as well as its id so that a drop
/// target can refuse the folder the item already lives in without looking the
/// item up -- the sidebar has the vault's items but has no business
/// re-deriving which one is under the pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraggedItem {
    pub id: String,
    /// `None` for an unfiled item, matching `VaultItem::folder_id`.
    pub folder_id: Option<String>,
}

/// One clickable line of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuCommand {
    pub label: String,
    pub command: RowCommand,
    /// `false` draws the line greyed and unclickable, with
    /// [`Self::disabled_reason`] on hover.
    ///
    /// **Disabled and absent are different situations and both occur here.**
    /// "Copy TOTP" is ABSENT on an item with no seed -- there is nothing to
    /// explain and no action to want. "Edit" is PRESENT AND DISABLED for the
    /// kinds `detail::kind_offers_edit` rejects, because a user looking for
    /// the obvious action needs to be told why it is not on offer rather
    /// than left to conclude the menu is broken. Collapsing the two onto one
    /// representation is this codebase's most-repeated defect; they are kept
    /// apart deliberately.
    pub enabled: bool,
    pub disabled_reason: Option<&'static str>,
}

/// One entry of an item row's right-click menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Command(MenuCommand),
    /// "Move to folder", which opens a submenu rather than acting.
    MoveToFolder(MoveMenu),
    /// **"Icon"**, which opens a submenu rather than acting. Never empty.
    ///
    /// A bare `Vec<MenuCommand>` rather than a second [`MoveMenu`]-shaped
    /// enum, and the difference is a real one about the two features rather
    /// than a shortcut. "This vault has no folders yet" is an ordinary state
    /// for a user to be in, it is not the item's fault, and the remedy --
    /// make a folder -- is something the submenu can usefully say out loud;
    /// that is what `MoveMenu::Empty` exists for. An item with no icon action
    /// at all is not a state [`icon_menu`] can produce (see its doc: "Select
    /// icon..." is offered unconditionally), and if it ever became one there
    /// would be nothing to tell the user and nothing for them to change --
    /// so that function answers `None` and no **Icon** entry is drawn,
    /// which is `MenuCommand::enabled`'s absent-rather-than-greyed rule one
    /// level up. Either way the renderers never meet an empty list, so
    /// neither of them has to decide what an empty box means.
    Icon(Vec<MenuCommand>),
}

/// The "Move to folder" submenu's contents.
///
/// Two variants rather than a possibly-empty `Vec`, so "this vault has no
/// folders yet" is a state the renderer has to handle explicitly instead of
/// opening an empty box, which reads as a submenu that failed to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveMenu {
    /// At least one assignable folder. Never empty.
    Targets(Vec<MenuCommand>),
    /// No assignable folder exists; the submenu says so and offers nothing.
    Empty(&'static str),
}

/// The submenu's own label. A constant because the draw code and the tests
/// that read painted galleys both need it.
// English only, kept because `send_ui`'s tests alias it in a `const`; everything that
// draws reads [`move_to_folder_label`]. Phase 2 of the translation work removes this
// constant when `send_ui` has moved to the function.
pub const MOVE_TO_FOLDER_LABEL: &str = "Move to folder";

pub fn move_to_folder_label() -> &'static str {
    crate::i18n::t("list.move_to_folder")
}

/// Why "Edit" is greyed for a non-login.
///
/// Why "Edit" is greyed for the kinds the form cannot write back.
///
/// The reason, not just the fact. Since 2026-08-17 that is only an SSH key
/// (creatable, but `EditDraft::apply_to` has no arm that writes its keys, so
/// a form would silently discard them) and an item type this build does not
/// recognise. Logins, secure notes, cards and identities are editable -- see
/// `detail::kind_offers_edit`, which is the one predicate the enabled flag is
/// read straight off.
fn edit_disabled_reason() -> &'static str {
    crate::i18n::t("list.edit_disabled")
}

/// Shown instead of a destination list when the vault has no folder that can
/// be assigned to.
fn no_assignable_folders() -> &'static str {
    crate::i18n::t("list.no_folders")
}

/// **The icon submenu's own label**, alongside [`MOVE_TO_FOLDER_LABEL`] and a
/// constant for its reason: the two draw sites, the entry-list tests and the
/// tests that read painted galleys all name it.
///
/// **No ellipsis, unlike [`SELECT_ICON_LABEL`] one line down.** That
/// convention marks an entry that opens something the user then has to deal
/// with, and this one only unfolds a list -- an "Icon..." promising a dialog
/// and delivering three more menu rows would spend the one piece of
/// punctuation this menu uses to mean something on the entry that does not
/// mean it.
///
/// **A noun and not a verb.** "Change icon" or "Set icon" would be wrong for
/// one of the three entries underneath it however it was worded: "Refresh
/// icon" asks for the same picture again and changes nothing the user chose.
/// The group is everything this app can do with an item's picture, so it is
/// named for the picture.
pub fn icon_label() -> &'static str {
    crate::i18n::t("list.icon")
}

/// The "forget this icon and fetch it again" entry's wording, the owner's own.
///
/// A constant because the draw code, the entry-list tests and the tests that
/// read painted galleys all name it, and a relabelling that reached three of
/// those four is a menu whose test suite is about a string nobody sees.
pub fn refresh_icon_label() -> &'static str {
    crate::i18n::t("list.refresh_icon")
}

/// The "choose this item's picture yourself" entry's wording.
///
/// The trailing ellipsis is the platform convention for an entry that opens
/// something rather than acting, and it is load-bearing here: every other
/// entry on this menu does its thing on the click, and this one puts a modal
/// up. `\u{2026}` rather than three periods, matching every other ellipsis in
/// this app's copy.
///
/// A constant for [`REFRESH_ICON_LABEL`]'s reason: the draw code, the
/// entry-list tests and the tests that read painted galleys all name it.
pub fn select_icon_label() -> &'static str {
    crate::i18n::t("list.select_icon")
}

/// The way back to the automatic icon.
///
/// **"Use the automatic icon", not "Clear icon" or "Remove icon".** The item
/// does not end up with no picture: it ends up with the picture it would have
/// had all along, which for a login is its site's favicon and for a secure
/// note is its monogram. A label promising removal would read as destructive
/// on an item whose visible result is a *different* picture appearing.
pub fn clear_icon_label() -> &'static str {
    crate::i18n::t("list.clear_icon")
}

/// Why the item's own folder is greyed inside the submenu. Kept rather than
/// dropped from the list so the destinations do not reshuffle as items are
/// selected, and so the row doubles as "this is where it lives now".
fn already_in_this_folder() -> &'static str {
    crate::i18n::t("list.already_in_folder")
}

/// The "no folder at all" destination's label, and the reason it is greyed
/// for an item that already has none.
///
/// The wording is Bitwarden's own ("No folder"), so a user who files things
/// in another client meets the phrase they already know rather than this
/// app's invention of one.
fn no_folder_label() -> &'static str {
    crate::i18n::t("list.no_folder")
}
fn already_in_no_folder() -> &'static str {
    crate::i18n::t("list.not_in_folder")
}

/// The Delete entry's label. **One label, because the confirmation is no
/// longer in the menu**: choosing this opens
/// [`super::delete_modal`], which asks the question somewhere a question can
/// actually be asked. It used to have a second, armed label
/// ("Delete? Click to confirm") that this entry re-labelled itself with for
/// three seconds -- see that module's doc for why a menu is the wrong place
/// to hold a state.
///
/// `pub(super)` since the detail pane's kebab builds its own Delete entry
/// from it rather than repeating the word: that pane had `("Delete", "Delete
/// this item")` typed into it, so the row menu and the kebab were two
/// spellings of one label and a rename would have moved only one of them.
pub(super) fn delete_label() -> &'static str {
    crate::i18n::t("list.delete")
}

/// The Trash row's permanent delete. Also one label, through the same modal,
/// which says "forever" in its heading, its sentence and its button.
fn purge_label() -> &'static str {
    crate::i18n::t("list.purge")
}

/// Trimmed text, or `None` when there is nothing worth an entry -- the same
/// rule `detail::non_empty` applies to that pane's rows, restated here
/// because it is private there.
fn menu_non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// Every entry of `item`'s right-click menu, in order, for a vault holding
/// `folders`.
///
/// Pure, and the single source of what the menu contains: `item_row` draws
/// exactly this list and decides nothing itself. That split is not stylistic
/// -- egui renders a context menu into its own popup layer, and a per-kind
/// decision made inside that closure is one no test in this crate could
/// reach.
///
/// `source` is which list the row was drawn from, and it selects between
/// three DISJOINT menus rather than adding or removing a line from one.
/// That is the point: an item in the trash cannot be edited, filled, moved
/// between folders or soft-deleted again, and offering those on it would be
/// six silent no-ops in a row -- the failure this file's own comments keep
/// naming. See [`out_of_vault_entries`].
///
/// **No "No folder" / un-file entry** in any of the three. `bw serve` (CLI 2026.7.0)
/// cannot clear a folder assignment at all -- omitting the key, `null`, `""`
/// and a fully round-tripped object were each proven against a control field
/// that did change in the same request
/// (`.superpowers/sdd/put-semantics-capture.md`). Offering it would write
/// successfully and do nothing.
pub fn menu_entries(
    item: &VaultItem,
    folders: &[Folder],
    source: FilterSource,
    // Whether this window's backend can actually take an item out of a
    // folder -- `VaultBackend::can_unfile_items`, threaded from the window
    // rather than decided here. See [`move_menu`], which is the only thing
    // that reads it.
    may_unfile: bool,
) -> Vec<MenuEntry> {
    if let Some(out) = source.out_of_vault() {
        return out_of_vault_entries_for(item, out);
    }
    let kind = ItemKind::of(item);
    let login = item.login.as_ref();
    let mut entries = Vec::new();

    // Copy username/password are offered only when there is something to
    // copy. A card has no login object at all, and a login may carry an
    // empty one; either way an entry that puts "" on the clipboard is the
    // same untruth as a blank row, which is the rule `detail::non_empty`
    // already applies to that pane.
    if menu_non_empty(login.and_then(|l| l.username.as_deref())).is_some() {
        entries.push(enabled_command(crate::i18n::t("list.copy_username"), RowCommand::CopyUsername));
    }
    if menu_non_empty(login.and_then(|l| l.password.as_deref()).map(|p| p.as_str())).is_some() {
        entries.push(enabled_command(crate::i18n::t("list.copy_password"), RowCommand::CopyPassword));
        // No reveal and no confirmation before this one, by explicit user
        // decision.
    }
    // `is_some`, NOT the non-empty rule above, and that is deliberate: the
    // row's own "2FA" chip (see `item_row`) and `vault_window::mod`'s TOTP
    // poll gate are both `totp.is_some()`, so anything stricter here would
    // let a row show a 2FA chip whose menu then denies the code exists.
    if login.and_then(|l| l.totp.as_ref()).is_some() {
        entries.push(enabled_command(crate::i18n::t("list.copy_totp"), RowCommand::CopyTotp));
    }
    // Login-only, through the existing predicate rather than an inline
    // `item_type == Some(1)`: a website is a login's property, and no other
    // kind carries the `uris` this reads.
    //
    // **This block used to open with a worded "Fill in app" entry, and it is
    // gone at the user's request** ("I'm not sure what it does") -- the same
    // request that took the detail pane's Fill button and its CTRL+SHIFT+F
    // chord out at 7da1bba. It was the last MANUAL fill trigger; Auto,
    // Prompt and the global hotkey are untouched and are how a fill happens.
    if kind_offers_fill(kind) {
        // The same URL the pane's AUTOFILL TARGETS card opens: the first
        // URI, when there is one.
        if let Some(url) =
            menu_non_empty(login.and_then(|l| l.uris.first()).and_then(|u| u.uri.as_deref()))
        {
            entries.push(enabled_command(
                crate::i18n::t("list.open_website"),
                RowCommand::OpenWebsite(url.to_string()),
            ));
        }
    }
    // **The icon group is NOT here any more**, and its absence is the point.
    //
    // It arrived as three top-level entries, was folded into one submenu at
    // the owner's request, and has now moved off this menu entirely -- onto
    // the pencil badge on the edit pane's avatar, which is the picture those
    // entries are about. The owner's direction: "let's actually remove those
    // from result set and details to strictly to Edit screen".
    //
    // [`icon_menu`] itself is untouched and still the one decision about
    // WHICH of the three an item is offered; only its readers changed. See
    // `detail_edit`'s `edit_header`, the single one left.
    // Present for every kind, enabled only for those the edit form can
    // honestly edit -- see `MenuCommand::enabled` for why this one is greyed
    // rather than hidden.
    let editable = kind_offers_edit(kind);
    entries.push(MenuEntry::Command(MenuCommand {
        label: crate::i18n::t("list.edit").to_string(),
        command: RowCommand::Edit,
        enabled: editable,
        disabled_reason: (!editable).then_some(edit_disabled_reason()),
    }));
    entries.push(MenuEntry::MoveToFolder(move_menu(item, folders, may_unfile)));
    // Above Delete, below everything else: Archive is the gentler of the two
    // ways to take an item out of the working vault, and the design lists
    // the two rows in that order too.
    entries.push(enabled_command(crate::i18n::t("list.archive"), RowCommand::Archive));
    entries.push(enabled_command(delete_label(), RowCommand::Delete));
    // **A received record's menu is the read half of this one** (mock 6d:
    // "only what a view-only record allows"). Everything that writes goes --
    // [`RowCommand::writes_the_record`]'s decision, not a second list here --
    // and Move to folder with it, which that function counts as a write on
    // NodeWarden. Left out rather than greyed, like the detail pane's kebab:
    // greyed entries on every received record would be a menu that mostly
    // says no. `vault_window::mod` refuses a write for such an item anyway;
    // this is so the menu does not offer one.
    if !crate::vault_bridge::may_edit(item) {
        entries.retain(|entry| match entry {
            MenuEntry::Command(command) => !command.command.writes_the_record(),
            MenuEntry::MoveToFolder(_) | MenuEntry::Icon(_) => false,
        });
    }
    entries
}

/// The menu for an item that is NOT in the live vault -- one listed under
/// Trash or Archive.
///
/// **Nothing from the live menu appears here, and that is the whole design.**
/// A trashed item cannot be edited (the CLI rejects a PUT of a deleted
/// cipher), cannot be filled (it is not in the list the fill path reads),
/// cannot be moved between folders, and cannot be soft-deleted a second time.
/// Offering any of them would produce an entry that either fails or, worse,
/// succeeds at nothing -- and this file already carries three comments about
/// exactly that shape. Each of these two states has one or two things that
/// genuinely work, and those are what the menu holds.
///
/// Copy username/password are absent too, which is a judgement rather than a
/// technical limit: the values are in hand and copying them would work. They
/// are left out because an archived or trashed credential is one the user has
/// put away, and the two rows exist to get it back or get rid of it. Restore
/// it and every copy action returns.
///
/// Takes [`OutOfVault`] rather than a [`FilterSource`], so it cannot be
/// called for a live item at all.
///
/// **`pub(super)` so the detail pane's own kebab is built from it**, exactly
/// as that kebab's "Move to folder" submenu is built from [`move_menu`] and
/// for the same reason: the two surfaces would otherwise be two lists of
/// what a trashed item can do, and the one that is not the one being read is
/// the one that goes stale. See `detail::draw_out_of_vault_read`, which maps
/// each [`RowCommand`] here to the `DetailAction` it reports.
pub(super) fn out_of_vault_entries_for(item: &VaultItem, out: OutOfVault) -> Vec<MenuEntry> {
    let mut entries = out_of_vault_entries(out);
    // **An `edit: false` record's Trash and Archive menus are empty**: every
    // entry here -- Restore, Delete forever, Unarchive -- writes the record
    // ([`RowCommand::writes_the_record`]), and NodeWarden refuses each on a
    // view-only collection. The row menu then opens on nothing; the read
    // pane's ⋮ says why (`detail::view_only_footer_text()`).
    if !crate::vault_bridge::may_edit(item) {
        entries.retain(|entry| !matches!(entry, MenuEntry::Command(command) if command.command.writes_the_record()));
    }
    entries
}

/// **What an EMPTY row menu says instead of opening on nothing** (final
/// review, item 7): [`super::detail::view_only_footer_text()`], the read pane's own
/// words. Only a view-only record's menu can be empty -- every writable menu
/// holds Edit or a restore -- and one with nothing to copy (a card, a note,
/// a login with no username or password) or anything in the Trash or the
/// Archive (whose every entry writes) used to open as a blank popup, a
/// right-click that looked broken. `None` whenever there is an entry.
///
/// Pure, for [`menu_entries`]' reason: a decision made inside the popup's
/// closure is one no test could reach.
pub(super) fn empty_menu_footer(entries: &[MenuEntry]) -> Option<&'static str> {
    entries.is_empty().then_some(super::detail::view_only_footer_text())
}

/// [`out_of_vault_entries_for`] without the item: the menu for an item this
/// account may write, which is what the tests that compare the two menus
/// compare.
pub(super) fn out_of_vault_entries(out: OutOfVault) -> Vec<MenuEntry> {
    match out {
        OutOfVault::Trash => vec![
            enabled_command(crate::i18n::t("list.restore"), RowCommand::Restore),
            // The only entry in this file that is not undoable. It goes
            // through the SAME `delete_modal` the ordinary Delete does
            // rather than a second confirmation idiom -- the modal tells the
            // two apart by its `DeleteKind`, in every sentence it shows.
            enabled_command(purge_label(), RowCommand::PurgeForever),
        ],
        // One entry, because there is exactly one thing to do with an
        // archived item. "Delete" is deliberately not offered: archiving is
        // the user putting something aside, and the route from aside to gone
        // goes back through the vault, where the ordinary Delete lives with
        // its confirmation.
        OutOfVault::Archive => vec![enabled_command(crate::i18n::t("list.unarchive"), RowCommand::Unarchive)],
    }
}

/// A plain, clickable command: enabled, with nothing to explain.
///
/// Split out of [`enabled_command`] when the icon group became a submenu.
/// A submenu's rows are `MenuCommand`s and not [`MenuEntry`]s -- nothing
/// inside a submenu of this menu opens a further one -- so the two shapes are
/// built by one function each over a single definition of what "plain" means,
/// rather than by two spellings of the same four fields.
///
/// `pub(super)` for [`menu_command_button`]'s reason: the detail pane's kebab
/// has one entry -- the live item's Delete -- that no list in this file
/// produces, and building it here is what puts it under the same
/// [`RowCommand::is_destructive`] rule as every entry that does come from a
/// list, instead of under a hand-rolled red of its own.
pub(super) fn plain_command(label: &str, command: RowCommand) -> MenuCommand {
    MenuCommand {
        label: label.to_string(),
        command,
        enabled: true,
        disabled_reason: None,
    }
}

/// A plain, clickable entry.
fn enabled_command(label: &str, command: RowCommand) -> MenuEntry {
    MenuEntry::Command(plain_command(label, command))
}

/// The destinations "Move to folder" offers for `item`.
///
/// Filtered through `detail_edit::assignable_folders` -- the EXISTING
/// predicate, which drops `bw serve`'s virtual "No Folder" bucket via
/// `sidebar::is_virtual_folder`. Not re-derived inline: offering that bucket
/// writes `folderId: ""`, which strands the item out of every sidebar row,
/// and one definition of "virtual" is what keeps this menu and the edit
/// form's dropdown from drifting apart.
///
/// **`pub(super)` so the detail pane's kebab builds its "Move to folder" from
/// this exact function rather than from its own copy.** The two surfaces offer
/// the same operation on the same item, and what shapes the list -- which
/// folders are assignable, and that the item's own folder stays present but
/// greyed -- are decisions, not rendering. A second implementation of them in
/// `detail.rs` is the drift this crate keeps losing to, and it would be
/// invisible: both menus would look right, and only an item in a folder the
/// backend had stopped reporting would show the two disagreeing.
///
/// **"No folder" is here now, and only where it works.** It used to be
/// absent from this menu unconditionally, because `bw serve` (CLI 2026.7.0)
/// cannot un-file an item at all: `.superpowers/sdd/put-semantics-capture.md`
/// records the controlled run in which omitting `folderId`, sending `null`,
/// sending `""` and PUTting a fully round-tripped object all left the folder
/// unchanged while a name change in the very same request applied. That
/// finding stands, and it is a fact about **one backend**. The direct-REST
/// backend replaces the whole cipher, so an omitted key is an absent folder
/// and un-filing is an ordinary edit -- `RestBackend::move_item_to_folder`
/// says so at its own definition.
///
/// `may_unfile` is `VaultBackend::can_unfile_items`, threaded from the window
/// rather than decided here, so this menu offers the destination exactly when
/// the write behind it does something. Withholding it from everyone was the
/// honest answer while one backend existed; it is the wrong answer now, and
/// offering it to everyone would put back the silent lie the capture found.
///
/// A `false` here also keeps `EditDraft::may_unfile`'s reasoning intact: that
/// form still withholds the option on its own rule, so the two surfaces agree
/// on `bw serve` and the form is simply the stricter of the two on REST.
pub(super) fn move_menu(item: &VaultItem, folders: &[Folder], may_unfile: bool) -> MoveMenu {
    let assignable = assignable_folders(folders);
    if assignable.is_empty() && !may_unfile {
        return MoveMenu::Empty(no_assignable_folders());
    }
    // **First, above the folders**, which is where Bitwarden's own clients
    // put it and where a destination that is not a folder belongs: reading
    // down a list of folder names and meeting "No folder" among them reads as
    // a folder someone named that.
    //
    // Greyed when the item is already unfiled, for the same reason the item's
    // own folder is greyed below: it is a write that would achieve nothing,
    // and dropping the row instead would make the destinations reshuffle from
    // item to item.
    let already_unfiled = item.folder_id.as_deref().unwrap_or("").is_empty();
    let unfile = may_unfile.then(|| MenuCommand {
        label: no_folder_label().to_string(),
        command: RowCommand::Unfile,
        enabled: !already_unfiled,
        disabled_reason: already_unfiled.then_some(already_in_no_folder()),
    });
    MoveMenu::Targets(
        unfile
            .into_iter()
            .chain(assignable.into_iter().map(|folder| {
                // The item's own folder stays in the list, greyed: dropping
                // it would make the destinations reshuffle from item to
                // item, and it is useful as a statement of where the item
                // lives now. Moving an item to the folder it is already in
                // is a write that achieves nothing, so it is not offered as
                // an action.
                let here = item.folder_id.as_deref() == Some(folder.id.as_str());
                MenuCommand {
                    label: folder.name.clone(),
                    command: RowCommand::MoveToFolder(folder.id.clone()),
                    enabled: !here,
                    disabled_reason: here.then_some(already_in_this_folder()),
                }
            }))
            .collect(),
    )
}

/// **The last four digits of a card's number, and the ONE place they are
/// worked out.**
///
/// Both the row's `(*4545)` suffix ([`card_number_suffix`]) and the search
/// arm in [`matches_filter`] read this. They are deliberately not two
/// extractions: a card findable by digits it does not display, or displaying
/// digits that do not find it, is a disagreement no user could diagnose and
/// nothing but a third test comparing the two could catch. One function, and
/// the two behaviours are the same fact by construction.
///
/// **Keyed on [`ItemKind`], not on the presence of a number.** A login whose
/// name is `4545` is not a card and grows neither a suffix nor a digit match.
///
/// **Fewer than four digits stored gives `None`, not a partial.** The rule is
/// `docs/superpowers/specs/2026-08-17-card-art-design.md` §4's, quoted
/// straight: revealing "the last four" of a six-digit fragment discloses a
/// larger fraction of it than the last four of a real number does, and a
/// partial number is a data-entry state rather than a card.
///
/// **Non-digits are skipped** -- a number the user typed as `4242 4242 4242
/// 4242` or with dashes has the same last four as one typed bare.
///
/// The full number is never materialised into a plain `String`. It lives in a
/// `Zeroizing` on the item, and this walks its `chars` twice rather than
/// collecting them, so the only copy this function makes is of the four
/// digits it answers with -- which are the digits already painted on the row.
pub fn card_last_four(item: &VaultItem) -> Option<String> {
    if ItemKind::of(item) != ItemKind::Card {
        return None;
    }
    let number = item.card.as_ref()?.number.as_ref()?;
    let digits = || number.chars().filter(char::is_ascii_digit);
    let count = digits().count();
    if count < 4 {
        return None;
    }
    Some(digits().skip(count - 4).collect())
}

/// The network whose badge belongs on `item`'s tile, or `None` when this app
/// cannot name one.
///
/// **The stored `brand` is the authority, read case-insensitively** -- the
/// vault carries both `"Visa"` and `"visa"`, and it is the field every other
/// Bitwarden client writes and the only one a user can correct by hand.
///
/// **The number is the fallback, not the override.** A card saved by another
/// client with no `brand` at all still reads as what it is, but a hand-picked
/// brand is never second-guessed by the digits -- the same rule the edit
/// form's "a hand-picked brand survives a later edit to the number" holds.
///
/// **Only the leading digits leave the `Zeroizing`.** The longest prefix rule
/// in [`brand_for_number`] is four digits long, so eight is already more than
/// the decision can use; the rest of the number is never copied into a plain
/// `String` here. Those leading digits are the card's issuer prefix, which is
/// what the badge then paints in public anyway.
pub fn card_network(item: &VaultItem) -> Option<CardBrand> {
    if ItemKind::of(item) != ItemKind::Card {
        return None;
    }
    let card = item.card.as_ref()?;
    // A brand that is stored answers on its own, whether or not this app can
    // name it. Naming it draws the badge; failing to draws NOTHING, per the
    // design spec: a card the user has labelled "Ledger Coin" is not secretly
    // a Visa because its digits start with a 4, and a placeholder badge on an
    // item the user already knows the identity of is noise. The number is
    // consulted only where the brand is genuinely absent.
    if let Some(brand) = card.brand.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        return CardBrand::from_canonical(brand);
    }
    let prefix: String =
        card.number.as_ref()?.chars().filter(char::is_ascii_digit).take(8).collect();
    brand_for_number(&prefix)
}

/// The network mark a card shows (design 1e): [`card_network`]'s answer as a
/// [`Net`], with **Mir added**. Mir is not one of Bitwarden's ten brands, so
/// it can be read only off the digits: a card with no stored brand, or the
/// brand "Other" (which says "not one of the ten"), and a Mir number is a Mir.
/// A card whose stored brand names another network keeps it.
pub fn card_net(item: &VaultItem) -> Option<Net> {
    if ItemKind::of(item) != ItemKind::Card {
        return None;
    }
    let card = item.card.as_ref()?;
    let prefix = || -> String {
        card.number.as_ref().map(|n| n.chars().filter(char::is_ascii_digit).take(8).collect()).unwrap_or_default()
    };
    match card.brand.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        None => Net::for_number(&prefix()),
        Some(stored) => match CardBrand::from_canonical(stored) {
            Some(CardBrand::Other) => Net::for_number(&prefix()).filter(|n| *n == Net::Mir),
            Some(brand) => Net::from_brand(brand),
            None => None,
        },
    }
}

/// The bank an item is for (design 1e): a card's `deskwarden:bank-domain`, or
/// a login's website, when either is one of the banks that have a mark.
pub fn item_bank(item: &VaultItem) -> Option<Bank> {
    crate::favicon::icon_domain_for(item).as_deref().and_then(Bank::for_domain)
}

/// The row's title suffix for `item`, exactly as painted: `(*4545)`.
///
/// A pure function of the item so the rendering rule -- one asterisk, four
/// digits, parentheses, and nothing at all when there are not four digits --
/// can be asserted without a frame, and so [`item_row`] has no formatting
/// decision of its own to get wrong.
pub fn card_number_suffix(item: &VaultItem) -> Option<String> {
    card_last_four(item).map(|four| format!("(*{four})"))
}

/// The part of `search_lower` that could be a card's digits, with the
/// punctuation the row paints AROUND them trimmed off.
///
/// The user reads `(*4545)` off the row, so a query typed as `*4545` or
/// `(*4545)` should find the same card `4545` does. This trims those three
/// characters from the ENDS and stops -- it is deliberately not a parser for
/// the suffix's shape, which is why a query that is nothing but punctuation
/// trims to empty and matches no card rather than every one of them.
fn card_digit_query(search_lower: &str) -> Option<&str> {
    let trimmed = search_lower.trim_matches(|c| c == '(' || c == '*' || c == ')');
    (!trimmed.is_empty()).then_some(trimmed)
}

/// True when `item` is both in `filter`'s scope (delegates to
/// `SidebarFilter::scope_contains` -- the one place that logic lives, so
/// this and `sidebar::count_for` can't drift apart) and matches
/// `search_lower` against its name, its username, or -- for a card -- the
/// last four digits of its number.
///
/// **The card arm matches the last four ONLY, never the whole number.** The
/// principle is "you can search for what you can see": the row paints
/// `(*4545)` and nothing else of the number, so a query matching a middle
/// fragment would pull up a card on evidence the user cannot read anywhere on
/// screen. [`card_last_four`] is the single source for both, so the digits
/// that find a card are the digits it shows, by construction.
///
/// Substring and case-insensitive like the two arms beside it: `45` matches,
/// exactly as a two-letter name fragment already does.
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
    item.name.to_lowercase().contains(search_lower)
        || username.to_lowercase().contains(search_lower)
        || card_digit_query(search_lower).is_some_and(|digits| {
            card_last_four(item).is_some_and(|four| four.contains(digits))
        })
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
/// `Archive`, `Trash`, `Folder` and `Unfiled` deliberately keep the neutral
/// "item": all four hold a mixture of kinds, so any specific noun would be wrong for
/// most of their contents, and the sidebar already shows which scope is
/// selected.
///
/// **`None` is "this app does not know yet", and it drops the number instead
/// of printing one.** The Trash and Archive rows are on-demand queries, so
/// between selecting the row and the fetch landing -- and permanently, if it
/// failed -- there is no count to state. This control used to be handed
/// `count_for(shown, filter)` over an empty placeholder list and so read
/// "Search 0 items": the identical `0`-instead-of-unknown claim
/// `sidebar::UNKNOWN_COUNT` was introduced to remove from the badge sitting
/// one control to the left of it, restated by the neighbour. It says
/// "Search items" until it has something true to say.
///
/// The en dash is deliberately NOT reused here. In the badge it occupies a
/// slot the eye reads as a number, and so reads as "unknown"; in the middle
/// of a sentence ("Search - items") it reads as a typo.
pub fn search_hint(count: Option<usize>, filter: &SidebarFilter) -> String {
    let noun = match filter {
        SidebarFilter::Favorites => "favorites",
        SidebarFilter::Apps => "apps",
        SidebarFilter::Logins => "logins",
        SidebarFilter::Passkeys => "passkeys",
        SidebarFilter::Cards => "cards",
        SidebarFilter::Identities => "identities",
        SidebarFilter::SecureNotes => "notes",
        SidebarFilter::SshKeys => "ssh_keys",
        _ => "items",
    };
    match count {
        Some(count) => {
            let count = count as u64;
            match noun {
                "favorites" => crate::i18n::tn("list.search_n_favorites", count, &[]),
                "apps" => crate::i18n::tn("list.search_n_apps", count, &[]),
                "logins" => crate::i18n::tn("list.search_n_logins", count, &[]),
                "passkeys" => crate::i18n::tn("list.search_n_passkeys", count, &[]),
                "cards" => crate::i18n::tn("list.search_n_cards", count, &[]),
                "identities" => crate::i18n::tn("list.search_n_identities", count, &[]),
                "notes" => crate::i18n::tn("list.search_n_notes", count, &[]),
                "ssh_keys" => crate::i18n::tn("list.search_n_ssh_keys", count, &[]),
                _ => crate::i18n::tn("list.search_n_items", count, &[]),
            }
        }
        None => match noun {
            "favorites" => crate::i18n::t("list.search_favorites"),
            "apps" => crate::i18n::t("list.search_apps"),
            "logins" => crate::i18n::t("list.search_logins"),
            "passkeys" => crate::i18n::t("list.search_passkeys"),
            "cards" => crate::i18n::t("list.search_cards"),
            "identities" => crate::i18n::t("list.search_identities"),
            "notes" => crate::i18n::t("list.search_notes"),
            "ssh_keys" => crate::i18n::t("list.search_ssh_keys"),
            _ => crate::i18n::t("list.search_items"),
        }
        .to_string(),
    }
}

/// What this pane draws where the rows would be, when there are no rows.
///
/// **Three distinguishable situations wearing one appearance.** Until this
/// existed, a list with nothing in it painted literally nothing -- an empty
/// grey rectangle under the search box -- whether the answer had not arrived
/// yet, had arrived and was empty, or had been filtered down to nothing. A
/// blank pane is the shape of a broken window, and it is the thing the report
/// behind this work describes: "you need to show a window instantly with
/// spinner and then whatever it takes to show something".
///
/// The live vault's own initial load is NOT one of these. It is handled a
/// level up, by `vault_body_state`, which replaces the whole window body --
/// sidebar included -- with one centred spinner and never reaches this pane at
/// all. That is deliberate and stays: half-drawn chrome around an empty list
/// reads as an empty vault. See `list_unless_unfetched`, whose `LiveVault` arm
/// says the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListPlaceholder {
    /// The list this scope reads has not been answered yet. Reached by the
    /// Trash and Archive rows, which are on-demand queries against `bw serve`
    /// -- measured at 3.46s on a cold backend, which is a long time to look
    /// at an empty box.
    Loading,
    /// Answered, and this scope genuinely holds nothing: an empty Trash, a
    /// folder with nothing filed in it, a type the vault has none of.
    Empty,
    /// This scope has contents; the search matched none of them.
    NoMatches,
}

impl ListPlaceholder {
    /// The one wording for each state. A method rather than three literals at
    /// the draw site so the states can be asserted without a rendered frame,
    /// and so the draw site cannot grow a fourth wording nothing decided on.
    pub fn message(self) -> &'static str {
        match self {
            ListPlaceholder::Loading => crate::i18n::t("list.loading"),
            ListPlaceholder::Empty => crate::i18n::t("list.nothing_yet"),
            ListPlaceholder::NoMatches => nothing_matches(),
        }
    }

    /// The line under the message, where the design draws one.
    pub fn detail(self) -> Option<&'static str> {
        match self {
            ListPlaceholder::NoMatches => Some(nothing_matches_detail()),
            ListPlaceholder::Loading | ListPlaceholder::Empty => None,
        }
    }
}

/// Spec 3.1's no-results title and its line underneath.
pub fn nothing_matches() -> &'static str {
    crate::i18n::t("list.nothing_matches")
}
pub fn nothing_matches_detail() -> &'static str {
    crate::i18n::t("list.nothing_matches_detail")
}

/// Decides between the three (see [`ListPlaceholder`]), or `None` when there
/// are rows to draw and no placeholder belongs on screen at all.
///
/// Every argument is a fact the caller already had; none of them is a new
/// "is a fetch in flight" flag:
///
///  * `fetched` -- `items.is_some()`, the `Option` this function's caller is
///    already handed precisely because "no answer yet" and "no items" are
///    different (see `draw_item_list`'s own doc, and `list_unless_unfetched`).
///  * `fetch_failed` -- the window's `aux_error` for the row that is selected
///    right now, i.e. `AuxList::error`. It is `None` for the live vault.
///  * `matched` -- how many rows survived the filter and the search.
///  * `searching` -- whether the search box has anything in it.
///
/// **A failed fetch draws no placeholder, and that is the important arm.** A
/// spinner that never resolves is worse than the blank pane it replaced, and
/// `!fetched` alone cannot tell "in flight" from "gave up" -- `AuxList` clears
/// neither `items` nor much else on failure. The failure already reaches the
/// user through the inline notice band this pane draws above the list
/// (`inline_notice`/`NoticeSource::Aux`), which also carries the retry, so
/// this returns `None` and lets that band be the answer rather than sitting a
/// second, contradictory message underneath it.
pub fn list_placeholder(
    fetched: bool,
    fetch_failed: bool,
    matched: usize,
    searching: bool,
) -> Option<ListPlaceholder> {
    if matched > 0 {
        return None;
    }
    if fetch_failed {
        return None;
    }
    if !fetched {
        return Some(ListPlaceholder::Loading);
    }
    if searching {
        return Some(ListPlaceholder::NoMatches);
    }
    Some(ListPlaceholder::Empty)
}

/// Paints one [`ListPlaceholder`] where the rows would be.
///
/// **Deliberately the same visual language as `loading_ui::show_while`**, the
/// window this app already shows while `bw serve` starts: a `theme::BLUE`
/// spinner over a `theme::semibold` line in `theme::TEXT_SECONDARY`, at the
/// same sizes. Two spinners invented separately would read as two apps.
///
/// The repaint request is `super::LOADING_FRAME_INTERVAL` -- the same constant
/// the whole-window loading body uses, not a second number -- because the
/// window's ambient cadence is `FRAME_INTERVAL` (500ms), at which a spinner
/// does not animate so much as twitch. It is asked for only in the `Loading`
/// arm; the other two are static text with nothing to drive.
pub fn trash_note() -> &'static str {
    crate::i18n::t("list.trash_note")
}
pub fn empty_trash() -> &'static str {
    crate::i18n::t("list.empty_trash")
}
pub fn trash_empty() -> &'static str {
    crate::i18n::t("list.trash_empty")
}
pub fn trash_empty_detail() -> &'static str {
    crate::i18n::t("list.trash_empty_detail")
}

pub fn empty_trash_id() -> egui::Id {
    egui::Id::new("list-empty-trash")
}

/// The note, and (with something to empty) the button; true when pressed.
fn trash_band(ui: &mut egui::Ui, any: bool) -> bool {
    let p = tokens::palette(ui.ctx());
    let mut pressed = false;
    egui::Frame::new().inner_margin(Margin { left: 24, right: 24, top: 0, bottom: 12 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            let size = kit::controls::ButtonSize { height: 36.0, pad_x: 14.0, radius: 11, font_px: 14.0 };
            let button_w = if any {
                kit::controls::button_width(ui.ctx(), kit::controls::ButtonKind::Danger, size, empty_trash(), false) + 10.0
            } else {
                0.0
            };
            let room = (ui.available_width() - button_w).max(40.0);
            ui.add_sized(
                egui::vec2(room, 36.0),
                egui::Label::new(tokens::job(tokens::BODY, trash_note(), p.text_faint)).wrap(),
            );
            if any {
                let button = kit::controls::button(ui, kit::controls::ButtonKind::Danger, size, empty_trash(), None);
                let _ = ui.interact(button.rect, empty_trash_id(), egui::Sense::hover());
                pressed = button.clicked();
            }
        });
    });
    pressed
}

fn draw_trash_empty(ui: &mut egui::Ui) {
    let p = tokens::palette(ui.ctx());
    ui.vertical_centered(|ui| {
        ui.add_space(48.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(tokens::job(PLACEHOLDER_TITLE, trash_empty(), p.ink));
        ui.label(tokens::job(tokens::BODY, trash_empty_detail(), p.text_faint));
    });
}

fn draw_list_placeholder(ui: &mut egui::Ui, placeholder: ListPlaceholder) {
    let p = tokens::palette(ui.ctx());
    ui.vertical_centered(|ui| {
        // Spec 3.1: `padding: 48 20`, a centred column with a gap of 8.
        ui.add_space(48.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        if placeholder == ListPlaceholder::Loading {
            ui.add(egui::Spinner::new().size(22.0).color(p.blue));
        }
        ui.label(tokens::job(PLACEHOLDER_TITLE, placeholder.message(), p.ink));
        if let Some(detail) = placeholder.detail() {
            ui.label(tokens::job(tokens::BODY, detail, p.text_faint));
        }
    });
    if placeholder == ListPlaceholder::Loading {
        ui.ctx().request_repaint_after(super::LOADING_FRAME_INTERVAL);
    }
}

/// "Nothing matches": 17/700.
const PLACEHOLDER_TITLE: tokens::Role = tokens::Role { size: 17.0, face: theme::BOLD, tracking_em: 0.0 };

/// The design's avatar/favicon tile: `width: 32px; height: 32px`.
///
/// `pub(crate)` only so the detail pane can PIN its own `HEADER_AVATAR` equal
/// to it (`the_header_tile_is_the_list_row_tile`). The two panes draw one tile
/// at one size by the owner's decision, and a number agreed by two private
/// constants is a number that can quietly stop agreeing. Nothing outside this
/// module lays anything out with it.
pub(crate) const AVATAR_SIZE: f32 = 44.0;

/// Design 2b's row box, in full: `padding: 10px 12px` around a 32px avatar,
/// plus the 1px border every row carries. egui's `Frame` sizes itself
/// `content + inner_margin + 2 * stroke.width`, which is the same box CSS's
/// content-box model produces -- 32 + 10 + 10 + 1 + 1.
///
/// This is what `ScrollArea::show_rows` is virtualized against, so it has to
/// be the height the rows really paint at; `consecutive_row_tiles_sit_exactly_
/// one_design_gap_apart_and_span_the_pane` asserts that from painted output
/// rather than trusting the arithmetic above.
pub(crate) const ROW_TILE_HEIGHT: f32 = AVATAR_SIZE + 2.0 * ROW_PAD_Y + 2.0 * ROW_BORDER;
/// The row's `padding: 10px 12px` and its border. **`pub(crate)`, with the
/// three below it, because the Sent list is drawn to
/// this row's shape.** The two columns swap places in one window, and the
/// owner's report was that a Send's row was not a record's row -- a 54pt band
/// with a 32pt tile beside a 62pt band with a 40pt one, its own padding, its
/// own radius. `send_ui` reads these rather than spelling its own, so the two
/// rows cannot drift apart again.
pub(crate) const ROW_PAD_Y: f32 = 12.0;
pub(crate) const ROW_PAD_X: f32 = 12.0;
/// v2 rows carry no border; the selected one wears a ring painted outside it.
pub(crate) const ROW_BORDER: f32 = 0.0;
/// The row's `gap: 11px`, between the avatar, the title column and the badge.
pub(crate) const ROW_GAP_X: f32 = 14.0;
/// The row's `border-radius: 10px`.
pub(crate) const ROW_RADIUS: u8 = 14;
/// The list container's `gap: 6px`.
pub(crate) const ROW_GAP: f32 = 4.0;
/// The list container's `padding: 10px`.
///
/// `pub(crate)` because the **Password health** screen takes this same column
/// over and has to be inset by the same amount: two panes that swap places in
/// one column and each spell their own `10.0` are two numbers that must agree,
/// which is this codebase's standing defect. See
/// `password_health::PANE_PADDING`.
pub(crate) const LIST_PADDING: f32 = 14.0;
/// Spec 3.1's row list padding `4 14 14`: the top is the 4.
pub(crate) const LIST_PADDING_TOP: f32 = 4.0;

/// The search field's own id.
///
/// `theme::search_field` puts this on the `TextEdit` it builds, and
/// `vault_window::mod`'s Ctrl+F asks `request_focus` for it by the same name
/// from outside this file -- so it MUST NOT CHANGE. Written once here rather
/// than at the two places in this module that need it (the field itself, and
/// [`nav_key`]'s Home/End gate), so a rename cannot move one and leave the
/// other reading a dead id that would simply never match.
/// `ctrl_k_still_focuses_the_search_field_after_the_move` is what says the
/// field really carries it.
pub(crate) fn search_field_id() -> egui::Id {
    egui::Id::new("vault-search")
}

/// Where the last frame left the list's vertical scroll offset.
///
/// Kept in `ctx.data` rather than read back out of egui's own `ScrollArea`
/// state, whose id is generated for us and is not ours to reconstruct. The
/// one reader is [`scroll_offset_for_row`]'s "is the row I just selected
/// already on screen" question, and last frame's offset is exactly the offset
/// `show_rows` is about to virtualize against on this one.
fn scroll_offset_id() -> egui::Id {
    egui::Id::new("vault-item-list-scroll-offset")
}

/// Every modal in this window, by the id of the SCRIM it draws under itself.
///
/// This is the list's half of a rule the window already follows and that a
/// scrim cannot enforce on its own: a scrim is a full-window click-catcher on
/// `Order::Foreground`, so it stops the pointer by sitting on a higher layer,
/// but a key read straight off `ctx.input` never consults a layer at all.
/// `vault_window::mod`'s `keyboard_shortcuts_enabled` is that same decision for
/// the window's own Ctrl+K/L/N; this is it for the list's arrow keys, made on
/// this side of the boundary because `draw_item_list` is not told which modals
/// its caller has open.
///
/// **Scrims, not the modal cards.** The scrim is the thing every one of them
/// has and
/// the thing that MEANS "the window behind this is inert" -- `detail.rs`'s
/// copy toast is also a `Foreground` area and has no scrim, which is exactly
/// the difference. `every_modal_scrim_in_the_crate_is_named_here` walks `src/`
/// and fails if a modal is added with a scrim this list does not name.
const MODAL_SCRIM_AREAS: &[&str] = &[
    "app-window-picker-scrim",
    "sequence-builder-scrim",
    "delete-confirm-scrim",
    "detail-edit-discard-scrim",
    "empty-trash-scrim",
    "folder-edit-scrim",
    "icon-pick-scrim",
    "import-scrim",
    "launch-confirm-scrim",
    "new-item-scrim",
    "palette-scrim",
    "password-generator-scrim",
    "prefs-modal-scrim",
    "record-forget-scrim",
    "record-import-scrim",
    "record-join-scrim",
    "record-leave-scrim",
    "record-send-scrim",
    "record-share-scrim",
    "send-detail-scrim",
    "totp-add-scrim",
];

/// Whether any of [`MODAL_SCRIM_AREAS`] is up.
///
/// `Areas::is_visible` is "shown last frame OR already shown this one". The
/// last-frame half is what makes this work at all: the modals are all drawn
/// AFTER the three panels, so on the frame a modal first appears this is
/// asked before that scrim has been shown. That costs one frame, and the
/// keystroke in it is the click or chord that opened the modal -- never an
/// arrow key, because the user cannot yet see the thing they would be
/// arrowing behind.
/// **`pub(crate)` for a second reader, and it is the read pane.**
///
/// The list asks this to keep arrow keys out from behind an open modal. The
/// read pane asks it for a related reason with the same shape: the copy
/// confirmation is ONE toast in the context's memory, and both the pane and
/// the add-a-code card can draw it -- so a copy made on the card was painted
/// twice, once over the card and once over the window behind it. The owner's
/// screenshot: two `Secret copied` boxes.
///
/// The rule is that the toast belongs to the topmost surface. A modal is
/// topmost whenever one is up, so the pane stands down and the modal draws
/// it; with no modal up the pane draws it as it always has.
pub(crate) fn a_modal_is_up(ctx: &egui::Context) -> bool {
    ctx.memory(|m| {
        MODAL_SCRIM_AREAS.iter().any(|name| {
            m.areas()
                // **`theme::SCRIM_ORDER`, which is not `Foreground`.** A
                // `LayerId` is an order AND an id, so this lookup went blind
                // the moment the scrims moved down a layer -- and a blind
                // gate is an arrow key reaching the item list behind an open
                // modal. Read off the constant the scrims are drawn on rather
                // than written out, so the two cannot part again.
                .is_visible(&egui::LayerId::new(
                    crate::theme::SCRIM_ORDER,
                    egui::Id::new(*name),
                ))
        })
    })
}

/// A keystroke that moves the selection through the item list.
///
/// **Enter is deliberately not one of these.** Selecting a row is what opens
/// it in the detail pane -- that already happened, on the arrow key -- so
/// Enter would have nothing left to do that the user did not just get. It is
/// also not free to take: the search field is a `TextEdit` whose `return_key`
/// is Enter, so a meaning bound here would fire every time somebody finished
/// typing a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListNavKey {
    Up,
    Down,
    Home,
    End,
}

/// Given the ids of the rows that are on screen -- **in the order they are
/// drawn**, i.e. after the sidebar filter and the search box, and not the
/// order of the vault's own vector -- the id selected right now, and a
/// navigation key: the index of the row that should be selected instead.
///
/// `None` only when there is nothing to select. It answers with an INDEX and
/// not an id because the caller needs the index anyway, to scroll a row that
/// may not be drawn this frame into view.
///
/// **Past either end it STOPS; it does not wrap.** This list is virtualized
/// because a real vault runs to thousands of rows, and wrapping is the one
/// move in it with no visual continuity at all -- Up on the first row would
/// teleport the viewport to row 4999, which reads as the list having jumped
/// rather than the selection having moved. It is also what every list on this
/// platform does (Explorer, Outlook, the Start menu), and the failure mode of
/// stopping is that a key does nothing, which is recoverable, where the
/// failure mode of wrapping is losing your place.
///
/// A selection that is not in `visible` at all -- the sidebar filter or the
/// search box narrowed it away while the detail pane still shows it -- counts
/// as no selection, which is what puts Down on the first row and Up on the
/// last.
fn next_selection(visible: &[&str], selected: Option<&str>, key: ListNavKey) -> Option<usize> {
    let last = visible.len().checked_sub(1)?;
    let current = selected.and_then(|id| visible.iter().position(|row| *row == id));
    Some(match (key, current) {
        (ListNavKey::Home, _) => 0,
        (ListNavKey::End, _) => last,
        (ListNavKey::Down, None) => 0,
        (ListNavKey::Up, None) => last,
        (ListNavKey::Down, Some(i)) => (i + 1).min(last),
        (ListNavKey::Up, Some(i)) => i.saturating_sub(1),
    })
}

/// Which navigation key this frame carries, if the list is allowed to act on
/// one.
///
/// **What egui actually delivers here, and why the search box is safe.** A
/// `TextEdit` does not take its events out of the queue: it reads
/// `InputState::filtered_events`, which CLONES the events its `EventFilter`
/// matches and leaves every one of them in place. So `key_pressed` below sees
/// Up/Down whether or not the search field has focus, and the field goes on
/// receiving everything it ever received -- typing is untouched, because
/// nothing here looks at a text event.
///
/// That leaves what the FIELD does with the same keystroke:
///
///  * **Up/Down: nothing.** It is a `TextEdit::singleline`, and egui resolves
///    a vertical arrow in one to `cursor_up_one_row` / `cursor_down_one_row`
///    on a one-row galley. So the list may take them even while the field has
///    focus -- which is the whole point of the feature: type to narrow, then
///    arrow down into what is left without reaching for the mouse.
///  * **Home/End: a real caret move**, to the start and the end of the typed
///    query. Those belong to the user, so they are only read here when the
///    field does NOT have focus.
///
/// Nothing is consumed, for the reason `theme::search_field`'s Escape is not
/// consumed either: the modals run later in the frame than this function
/// does, and a key taken out of the queue here is a key their own bindings
/// would never see.
///
/// A held modifier disqualifies all four, so a future Ctrl+Home or Shift+Down
/// cannot fire this as well as itself -- the hazard `vault_window::mod`'s
/// `matches_exact` chords were written for. Read off the KEY EVENT's own
/// modifiers rather than off `InputState::modifiers`, which is `detail.rs`'s
/// `consume_chord` idiom: the latter is the modifier state as of the end of
/// the frame's input, which is not necessarily what was held when the key
/// went down. The first qualifying event in the frame wins; two navigation
/// keys in one frame is a keyboard repeat racing a repaint, and taking one is
/// the honest answer to it.
fn nav_key(ctx: &egui::Context) -> Option<ListNavKey> {
    if a_modal_is_up(ctx) {
        return None;
    }
    let typing = ctx.memory(|m| m.has_focus(search_field_id()));
    // **Any OTHER text field with the keyboard keeps every key** -- the rail's
    // New folder input above all: a folder name being typed owns its caret,
    // and Up/Down/Home/End moving the list (and the detail pane with it)
    // under it is the selection changing while the user types elsewhere.
    // The search field keeps its carve-out below: Up/Down walk the results.
    if ctx.text_edit_focused() && !typing {
        return None;
    }
    ctx.input(|i| {
        i.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if !modifiers.any() => match key {
                egui::Key::Home if !typing => Some(ListNavKey::Home),
                egui::Key::End if !typing => Some(ListNavKey::End),
                egui::Key::ArrowUp => Some(ListNavKey::Up),
                egui::Key::ArrowDown => Some(ListNavKey::Down),
                _ => None,
            },
            _ => None,
        })
    })
}

/// The scroll offset that puts row `row` on screen, or `None` if it already
/// is -- a MINIMAL scroll, so a step onto the next row moves the viewport by
/// one row rather than re-centring the list under the selection.
///
/// **This is what keeps keyboard selection working at all.** The list is
/// `ScrollArea::show_rows`, so a row outside the drawn range is not a widget
/// this frame: it cannot be asked to scroll itself into view, and a selection
/// moved onto it would simply vanish. `show_rows` derives its row range from
/// the offset it is given, so forcing the offset here puts the row in the
/// drawn range on the SAME frame.
///
/// The pitch is `show_rows`' own -- `row_height + item_spacing.y`, which for
/// this list is [`ROW_TILE_HEIGHT`] plus [`ROW_GAP`]. Reading it off those two
/// constants rather than writing a number down is what keeps this in register
/// with the geometry they pin.
fn scroll_offset_for_row(row: usize, offset: f32, viewport_height: f32) -> Option<f32> {
    let pitch = ROW_TILE_HEIGHT + ROW_GAP;
    let top = row as f32 * pitch;
    let bottom = top + ROW_TILE_HEIGHT;
    if top < offset {
        Some(top)
    } else if bottom > offset + viewport_height {
        Some(bottom - viewport_height)
    } else {
        None
    }
}

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
///
/// `folders` is only ever read by the rows' right-click menus, where it
/// supplies "Move to folder"'s destinations. It does not affect what a row
/// paints.
///
/// **`items` is `None` when the list this row reads has not been fetched
/// yet** -- the Trash and Archive rows are on-demand queries, so "no answer
/// yet" is a real state and not a synonym for "empty". It reaches this
/// function at all because the SEARCH PLACEHOLDER has to be able to tell the
/// two apart: it read "Search 0 items" while a Trash fetch was in flight or
/// had failed, which is the same `0`-instead-of-unknown untruth
/// `sidebar::badge_text` exists to keep out of the badge one control to the
/// left. The rows themselves are drawn from `unwrap_or(&[])`, because there
/// is genuinely nothing to draw either way.
// Draw entry point: its arguments are the state it paints; a bundle would only move them.
#[allow(clippy::too_many_arguments)]
pub fn draw_item_list(
    ui: &mut egui::Ui,
    items: Option<&[VaultItem]>,
    folders: &[Folder],
    filter: &SidebarFilter,
    search: &mut str,
    selected_id: &mut Option<String>,
    icons: &IconCache,
    // See `menu_entries`' own parameter. Read by the rows' right-click menus
    // alone; it changes nothing a row paints.
    may_unfile: bool,
    visible_ids: &mut Vec<String>,
    move_error: Option<&str>,
    // Whether the fetch that would have filled `items` gave up -- the window's
    // `aux_error` for the row selected right now, which is `AuxList::error`
    // and nothing new. Read by `list_placeholder` alone, and only so that a
    // list which is empty BECAUSE the fetch failed does not get a spinner that
    // can never resolve. See that function.
    fetch_failed: bool,
    // **Set when a row was OPENED by a primary click this frame** -- the
    // gesture, reported out so `vault_window::mod` can arm the detail pane's
    // slide with it. An out-parameter for `visible_ids`' reason: it is a
    // by-product of drawing, not the list's verdict, and `ItemListAction` is
    // the verdict.
    //
    // NOT "the selection changed": a right-click, a keyboard move, a sync
    // restoring a selection and `Create`'s select-what-was-just-made all
    // change it, and none of them is this gesture. See `RowOutcome::opened`.
    //
    // Written only, never read here; the caller clears it.
    row_opened: &mut bool,
    // **Design 5d**: the names of the records with a live Send. A set of
    // NAMES and not of ids, which is `send_ui::live_send_named`'s decision
    // and is argued there.
    shared: &std::collections::HashSet<String>,
    // **Design 6**: which records are shared with people and who shares the
    // received ones. Asked per row by id -- a share IS a record, unlike a Send.
    people: &RowPeople,
    // The header's name for this scope (`sidebar::scope_title`): the folder's
    // own name, "All items", "Cards"... -- "Results" replaces it while the
    // search box has anything in it.
    scope_title: &str,
    // The window's vault came from the disk cache and the last sync failed:
    // spec 3.1's offline banner, with its Retry.
    offline: bool,
) -> ItemListAction {
    let mut action = ItemListAction::None;
    visible_ids.clear();
    // The rows, once. Everything below this line that draws an item reads
    // `rows`; the one thing that reads `items` itself is the placeholder's
    // count, which is the only control that cares about the difference.
    let rows: &[VaultItem] = items.unwrap_or(&[]);

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

    let search_lower = search.to_lowercase();
    let searching = !search_lower.trim().is_empty();
    let filtered: Vec<&VaultItem> = rows
        .iter()
        .filter(|item| matches_filter(item, filter, &search_lower))
        .collect();

    let title = if searching { results_title() } else { scope_title };
    if let Some(kind) = list_header(ui, title, filtered.len()) {
        action = kind;
    }
    // Spec C §3: the trash's 30-day line, and Empty trash while it holds
    // anything.
    if *filter == SidebarFilter::Trash && trash_band(ui, !filtered.is_empty()) {
        action = ItemListAction::EmptyTrash;
    }
    if offline && offline_banner(ui) {
        action = ItemListAction::RetrySync;
    }

    // A drag-to-folder that did not happen, said out loud. Drawn here --
    // between the toolbar strip and the list -- because this is the pane the
    // item lives in and the pane the gesture started from; the sidebar, where
    // it ENDED, has a bottom-anchored countdown and no band to put this in.
    //
    // ABOVE the list frame, so it takes its height off the scroll area rather
    // than out of it. `show_rows` reads `item_spacing.y` from the ui the list
    // frame gives it, which is set inside that frame, so a band added here
    // cannot reach the scroll pitch -- only how many rows fit. Pinned by
    // `an_inline_move_error_does_not_change_the_row_pitch_beneath_it`.
    if let Some(message) = move_error {
        if move_error_band(ui, message) {
            action = ItemListAction::DismissMoveError;
        }
    }

    // **The keyboard half of this pane.** Every chord that acts on an item --
    // Ctrl+B, Ctrl+U, Ctrl+T and the four card chords -- acts on the SELECTED
    // one, and until this block there was no way to select without the mouse,
    // so each of them needed a click first.
    //
    // Applied to `filtered`, which is the list AS DISPLAYED: the sidebar's
    // scope and the search box have already narrowed it and it is in the
    // on-screen order. Walking `rows` (or `items`) instead would compile and
    // look right on an unfiltered All view and then step onto items that are
    // not on screen the moment either control is used.
    //
    // What the key means is `next_selection`'s decision, not this block's --
    // see it for the stop-at-the-ends argument and for why there is no Enter.
    // Whether a key is even ours to read is `nav_key`'s, which is where the
    // search field and the modals are answered.
    let mut scroll_to_row = None;
    if let Some(key) = nav_key(ui.ctx()) {
        let ids: Vec<&str> = filtered.iter().map(|item| item.id.as_str()).collect();
        if let Some(row) = next_selection(&ids, selected_id.as_deref(), key) {
            *selected_id = Some(ids[row].to_string());
            // Set even when the selection did not actually change -- End on
            // the last row is "show me the end of the list", and a row that
            // is already selected but scrolled off is exactly the case this
            // has to answer.
            scroll_to_row = Some(row);
        }
    }

    // Design 2b's list padding (`padding: 10px`), applied here now that the
    // pane's panel frame has none -- see the header strip's comment above.
    //
    // The RIGHT padding is 0 because the scroll bar's lane IS that padding:
    // `theme::scrollbar_in_gutter` below reserves exactly `LIST_PADDING` and
    // draws a `theme::SCROLLBAR_WIDTH` bar inside it, so the gutter measures
    // `LIST_PADDING` on both sides whether or not a bar is showing.
    egui::Frame::new()
        .inner_margin(Margin {
            left: LIST_PADDING as i8,
            right: 0,
            top: LIST_PADDING_TOP as i8,
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
            // No rows: say which of the three reasons this is, instead of
            // leaving a blank rectangle that reads the same for all of them.
            // Drawn INSTEAD OF the scroll area rather than inside it -- an
            // empty `show_rows` has no rows to hang anything off, and the
            // reserved scrollbar gutter has nothing to scroll.
            if let Some(placeholder) = list_placeholder(
                items.is_some(),
                fetch_failed,
                filtered.len(),
                searching,
            ) {
                if *filter == SidebarFilter::Trash && placeholder == ListPlaceholder::Empty {
                    draw_trash_empty(ui);
                } else {
                    draw_list_placeholder(ui, placeholder);
                }
                // No scroll area ran, so the remembered offset would be last
                // frame's -- from a scope that is not on screen any more.
                // Zeroed, so the first arrow key after this list refills
                // measures against where a fresh list really starts.
                ui.ctx().data_mut(|d| d.insert_temp(scroll_offset_id(), 0.0_f32));
                return;
            }
            // **Does this list actually overflow?** Predicted from the row
            // count rather than read back from last frame's `ScrollArea`
            // output, which would be a frame late and so would flash a bar
            // for one frame whenever the filter changed. The pitch is
            // `show_rows`' own: one tile per row plus a gap BETWEEN rows.
            // Ties go to CAN SCROLL -- being wrong the other way would hide
            // a bar on a list that really can move.
            let content_height = filtered.len() as f32 * ROW_TILE_HEIGHT
                + filtered.len().saturating_sub(1) as f32 * ROW_GAP;
            // The scroll area takes all of it (`auto_shrink([false, false])`),
            // so this one measurement is both the overflow test's denominator
            // and the viewport a keyboard move has to land its row inside.
            let viewport_height = ui.available_height();
            let fits = content_height < viewport_height;
            // **Forced only on a frame a key moved the selection**, so mouse
            // scrolling is never fought: on every other frame the builder
            // below carries no offset at all and egui keeps its own.
            let forced_offset = scroll_to_row.and_then(|row| {
                let current = ui
                    .ctx()
                    .data(|d| d.get_temp::<f32>(scroll_offset_id()))
                    .unwrap_or(0.0);
                scroll_offset_for_row(row, current, viewport_height)
            });
            // **The bar lives INSIDE the 10pt gutter, and the tiles never
            // move.**
            //
            // THE REPORT, fourth time -- against the attempt that gave the bar
            // a lane of its OWN outside the padding: "no again, you made it
            // huge on the right now - scroll should be included in those 10pt
            // and window should not shrink\expand if more or less items".
            // Two constraints, both hard:
            //
            // * the gutter is `LIST_PADDING` on BOTH sides in every state --
            //   no wider lane on the right for the bar to sit beyond;
            // * the tiles keep ONE width, so filtering a list across the
            //   overflow boundary does not resize them.
            //
            // Hold both and the bar cannot ALLOCATE any width, because every
            // point of width it takes has to come out of one of those two. So
            // the lane is a flat `LIST_PADDING` here, unconditionally, and the
            // 6pt bar is drawn flush to its OUTER edge (`scrollbar_in_gutter`
            // pins it there) -- inside the padding, with 4pt of it left clear
            // between the bar and the tiles. `AlwaysVisible` keeps the
            // reservation from coming and going with the row count, which is
            // what holds the tiles at one width.
            //
            // **Measured, all three states, 390pt pane:** tiles 10..380 (370pt
            // wide) whether the list fits or scrolls and whether or not the
            // pointer is over it; gutter 10pt left, 10pt right, 10pt top,
            // 10pt bottom; the bar at 384..390 when it is showing.
            //
            // **The mechanism that was measured and rejected**: setting
            // `floating_allocated_width` to 0 so the bar reserves nothing and
            // is painted OVER the content, with the padding restored to the
            // frame and a negative `bar_outer_margin` pushing the bar back out
            // into the gutter. It paints pixel-for-pixel the same frame -- and
            // egui derives the bar's hit rect from `outer_rect.with_min_x(...)`
            // against an outer rect that now stops at the content's edge, so
            // the rect comes out INVERTED and the bar cannot be hovered or
            // dragged. Measured: a press-and-drag down the bar left the list on
            // its first row, where the same drag under the reservation below
            // scrolled it 23 rows.
            //
            // A list that FITS paints no bar at all -- `AlwaysVisible` would
            // otherwise run a full-height 6pt line down a list with nothing to
            // scroll, which is the original report. Only the PAINT is
            // suppressed; the lane stays reserved, so nothing resizes.
            theme::scrollbar_in_gutter(ui, LIST_PADDING);
            if fits {
                theme::hide_scrollbar(ui);
            }
            // **egui's own fade is left alone.** A floating bar fades out when
            // the pointer leaves the area. The previous attempt had to pin the
            // dormant opacities at the active ones, because a faded bar there
            // left a lane the TILES had paid for standing empty -- 10pt of
            // clear space with the pointer over the list against 16pt without.
            // Nothing is paid for here: the lane is the padding either way, so
            // the fade costs no layout at all and an idle window simply shows
            // the full 10pt clear on both sides. That is ordinary platform
            // behaviour and the best reading of the state a screenshot catches.
            let mut area = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                // Required by `scrollbar_in_gutter`: egui lays a bar's gutter
                // out only for a bar it is actually showing, so on
                // `VisibleWhenNeeded` the offsets that function sets have
                // nothing to apply to on a frame where the pointer is away.
                //
                // **It is NOT here to keep the tiles one width any more.**
                // `41e2e2e` gave that constraint up deliberately: the bar is
                // drawn inside the list's 10pt padding, so it costs no layout
                // at all and the tiles are the same width bar or no bar. The
                // comment that used to be here said otherwise, which would have
                // sent the next reader looking for a width that no longer
                // depends on this. What is still true is that egui has to
                // reserve and handle the lane at all, which is what this asks
                // for; the fade is left to egui (see just above).
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible);
            // **Set BEFORE `show_rows`, which is the whole trick.** `show_rows`
            // computes its row range from the offset the area starts the frame
            // with, so a forced offset applied here changes which rows are
            // drawn on THIS frame -- the row the arrow key just selected is a
            // real widget, painted, on the same frame it was selected. Asking
            // the row to scroll itself into view instead is the version that
            // silently does nothing: a row outside the drawn range never runs.
            if let Some(offset) = forced_offset {
                area = area.vertical_scroll_offset(offset);
            }
            let list_key = entrance_key(filter, &search_lower);
            let list = area
                .show_rows(ui, ROW_TILE_HEIGHT, filtered.len(), |ui, row_range| {
                    let entrance = entrance_for(ui.ctx(), list_key, row_range.clone());
                    for row in row_range {
                        let item = filtered[row];
                        visible_ids.push(item.id.clone());
                        let selected = selected_id.as_deref() == Some(item.id.as_str());
                        // Pushed so the row's widget id -- and therefore the
                        // id of the context-menu popup egui hangs off it --
                        // is derived from the ITEM rather than from its
                        // position in the visible range. Without this the
                        // ids are positional (egui counts widgets within a
                        // ui), so a list that scrolls while a menu is open
                        // would leave that menu attached to whatever item
                        // had slid into that slot. `push_id` allocates
                        // exactly the child's own rect, so it does not
                        // change the row's height or the scroll pitch --
                        // see the ROW_GAP comment above.
                        let outcome = ui
                            .push_id(&item.id, |ui| {
                                item_row(
                                    ui,
                                    item,
                                    folders,
                                    selected,
                                    icons.textures.get(&item.id),
                                    may_unfile,
                                    filter.source(),
                                    RowMarks {
                                        send: shared.contains(&item.name),
                                        person: people.shared.contains(&item.id),
                                        from: people.from(&item.id),
                                    },
                                    entrance.pose(ui.ctx(), row),
                                )
                            })
                            .inner;
                        if outcome.select {
                            *selected_id = Some(item.id.clone());
                        }
                        if outcome.opened {
                            *row_opened = true;
                        }
                        if let Some(command) = outcome.command {
                            // Overwrites a `NewItem` from the header strip in
                            // the same frame. The two cannot both happen: an
                            // open context menu is what a menu command comes
                            // from, and the click that dismisses it is the
                            // same click, so `+ New` is not also being
                            // pressed.
                            action = ItemListAction::Row { id: item.id.clone(), command };
                        }
                    }
                });
            // Remembered for the next frame's `scroll_offset_for_row`. This
            // is the offset AFTER everything this frame did to it -- the
            // wheel, a bar drag, or the forced offset just above -- which is
            // what makes the next keyboard step a step from where the list
            // actually is.
            ui.ctx()
                .data_mut(|d| d.insert_temp(scroll_offset_id(), list.state.offset.y));
        });

    action
}

/// The Shared tag's word (`kit::card::TagKind::Shared`).
pub fn shared_pill() -> &'static str {
    crate::i18n::t("list.shared_pill")
}

/// The tag on a row someone shares with this account: v2's neutral tag
/// (`kit::card::TagKind::ViewOnly`), which reads "View only".
pub fn view_only_chip() -> &'static str {
    crate::i18n::t("list.view_only")
}

/// **What sharing says about each row**: which records this account shares
/// with someone who has them now, and who shares each received record. Built
/// once per frame from the sharing view, and asked per row by id.
///
/// **Ids, where the Send set beside it is names.** A Send is a copy with no
/// link back to its record, which is why `send_ui::live_send_named` matches
/// by name; a person share IS the record, moved under the org key with its
/// id intact, so the id answers exactly and two records with one name are
/// never confused.
#[derive(Debug, Clone, Default)]
pub struct RowPeople {
    pub shared: std::collections::HashSet<String>,
    pub received: std::collections::HashMap<String, String>,
}

impl RowPeople {
    /// From the daemon's view; empty without one.
    ///
    /// `shared` holds only the records someone has NOW (spec 6.1's pill
    /// promise, `SharingView::shared_with_someone`) -- an invitation still
    /// waiting is not a person who can open the record, which is 5d's own
    /// rule for the chip ("someone can open it right now").
    #[must_use]
    pub fn from_view(view: Option<&crate::rest::sharing::SharingView>) -> Self {
        let Some(view) = view else {
            return Self::default();
        };
        Self {
            shared: view
                .records
                .iter()
                .filter(|r| view.shared_with_someone(&r.item_id))
                .map(|r| r.item_id.clone())
                .collect(),
            received: view.received.iter().map(|r| (r.item_id.clone(), r.from.clone())).collect(),
        }
    }

    /// Who shares a received record.
    #[must_use]
    pub fn from(&self, item_id: &str) -> Option<&str> {
        self.received.get(item_id).map(String::as_str)
    }
}

/// A row's trailing marks: a live Send, a person share, a received record.
///
/// A struct rather than three more parameters on `item_row`, which already
/// carries eight, and because the three are one question -- what is
/// happening to this record between people -- asked once per row.
#[derive(Debug, Clone, Copy, Default)]
pub struct RowMarks<'a> {
    /// 5d: a Send somebody can open right now.
    pub send: bool,
    /// 6e: a person has this record in their vault now.
    pub person: bool,
    /// 6e: who shares this record with this account, when someone does.
    pub from: Option<&'a str>,
}

/// Design 2b's row title (`font-size: 13px`) and subtitle
/// (`font-size: 11px`), and the `gap: 2px` between them. Named because
/// [`text_column_height`] has to measure exactly what [`item_row`] then
/// draws; a size stated twice would drift. `pub(crate)` for the row geometry's
/// reason: the Sent list's rows set their two lines in these.
pub(crate) const TITLE_SIZE: f32 = 16.0;
pub(crate) const TITLE_GAP_Y: f32 = 3.0;

/// What one drawn row reports back to [`draw_item_list`].
struct RowOutcome {
    /// The row was clicked and should become the selection.
    ///
    /// **True for a right-click as well as a left one**, which is the whole
    /// reason this is not just `response.clicked()`: the context menu acts
    /// on the row it was opened over, and if that row were not also selected
    /// the menu and the detail pane could be showing two different items
    /// while the user chose "Delete".
    select: bool,
    /// **A PRIMARY click on the row** -- the gesture, not the state change.
    ///
    /// [`select`](Self::select) cannot answer this: it is deliberately true
    /// for a right-click too, so that the context menu and the detail pane
    /// agree about which item they are on. The detail pane's slide-in is
    /// armed by the user OPENING an item and by nothing else, and the owner
    /// ruled a right-click out of that explicitly -- it selects a row on its
    /// way to a menu, which is not the same gesture as opening one.
    ///
    /// Only `vault_window::mod`'s arming site reads this. Selection itself
    /// still runs off `select`.
    opened: bool,
    /// An entry of this row's context menu was chosen this frame.
    command: Option<RowCommand>,
}

/// **The tile at the head of a row: the record's favicon, or what its kind
/// draws when it has none.**
///
/// Its own function because two lists draw it: this one, and the Sent list,
/// whose row for a Send made from a record wears THAT record's tile -- the
/// owner's "it shows 2X when it should show the record's image". One function
/// is one answer to "what does this record look like", rather than a second
/// copy of the branch that could disagree about a card or a note.
///
/// With a favicon, it is the SAME box the monogram fallback draws -- bordered,
/// with the tile's radius -- with the artwork FILLING it, clipped to the
/// tile's own corner radius and with the border re-drawn over the top.
/// `theme::avatar_image` carries the reasoning, and the history: this was
/// edge-to-edge, then inset 4pt, and is edge-to-edge again because the design
/// says the image takes the tile.
///
/// Without one, the item's KIND draws the tile. A login and an unsupported
/// kind still get the name's monogram -- `kind_mark::avatar` is the one place
/// that branch is made, and its module doc carries why a card's tile is the
/// generic card mark while `VISA` stays out on the trailing pill.
pub(crate) fn row_tile(
    ui: &mut egui::Ui,
    icon: Option<&egui::TextureHandle>,
    kind: ItemKind,
    name: &str,
    size: f32,
    selected: bool,
) {
    let _ = (kind, selected);
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), Sense::hover());
    let look = match icon {
        Some(texture) => kit::avatar::Look::Favicon(texture.clone()),
        None => {
            let (bg, fg) = kit::avatar::initials_colours(name);
            kit::avatar::Look::Initials { text: initials(name), bg, fg }
        }
    };
    kit::avatar::paint(ui.painter(), rect, &look, 12);
}

// Draw entry point: its arguments are the state it paints; a bundle would only move them.
#[allow(clippy::too_many_arguments)]
fn item_row(
    ui: &mut egui::Ui,
    item: &VaultItem,
    folders: &[Folder],
    selected: bool,
    icon: Option<&egui::TextureHandle>,
    // See `menu_entries`' own parameter.
    may_unfile: bool,
    // Which list this row was drawn from -- the row's menu is entirely
    // different for a trashed or archived item. See `menu_entries`.
    source: FilterSource,
    // **Designs 5d and 6e**: this record has a Send somebody can open right
    // now, a person who has it, or a person who shares it with this account.
    // Decided by the caller rather than a list searched here, because this
    // function runs per visible row per frame -- see `send_ui::shared_names`
    // and `RowPeople`.
    marks: RowMarks<'_>,
    // Where the row is in its entrance (`Entrance::pose`): at rest, it is
    // `Pose::REST`.
    pose: motion::Pose,
) -> RowOutcome {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_TILE_HEIGHT), Sense::click_and_drag());
    paint_row(ui, rect, item, selected, icon, marks, pose, response.hovered());
    // `click_and_drag`, not `click`: the row is both the selection control it
    // has always been AND the handle for dragging the item onto a folder.
    //
    // `Response::interact` re-registers the rect the frame ALREADY allocated
    // with a wider sense -- it allocates nothing of its own. That is why the
    // drag source is added here rather than by wrapping the row in
    // `Ui::dnd_drag_source`, which opens a `scope` and would put a second
    // allocation inside the `push_id` the context menu depends on. The row's
    // height, and therefore the fixed pitch `show_rows` virtualizes against,
    // is untouched; `a_drag_in_flight_does_not_move_the_rows_underneath_it`
    // asserts that from painted output rather than from this comment.
    //
    // egui distinguishes the two gestures by travel: a press that does not
    // move is `clicked()`, a press that does is `dragged()` and never
    // `clicked()`. So dragging an item does NOT select it, and the
    // right-click that opens the context menu is untouched -- dragging is
    // primary-button only, and `secondary_clicked()` is read below exactly as
    // before.
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // Only acts on the frame the drag actually starts (see
    // `Response::dnd_set_drag_payload`), so this is not a per-frame write.
    //
    // **Not for a received record** (mock 6d): a drop on a folder is a move,
    // which NodeWarden refuses on a view-only collection (see
    // `RowCommand::writes_the_record`), and the sidebar's drop goes straight
    // to the move rather than through the row-command gate. So the drag
    // never starts -- no payload, no ghost -- which is the menu's rule for
    // Move to folder, applied to the gesture that does the same thing.
    if crate::vault_bridge::may_edit(item) {
        response.dnd_set_drag_payload(DraggedItem {
            id: item.id.clone(),
            folder_id: item.folder_id.clone(),
        });
        if response.dragged() {
            drag_ghost(ui, &item.name);
        }
    }
    // The menu is rendered into egui's own popup layer (a separate `Area`),
    // so nothing here allocates in the row's ui and the fixed row pitch
    // `show_rows` virtualizes against is untouched.
    //
    // WHAT it contains is `menu_entries`' decision and not this closure's --
    // see that function's doc for why a per-kind decision must not live in
    // here.
    let mut command = None;
    crate::kit::menu::context(&response, |ui| {
        let entries = menu_entries(item, folders, source, may_unfile);
        // Nothing to offer: say why, as the read pane's kebab does, rather
        // than open a blank popup (`empty_menu_footer`).
        if let Some(footer) = empty_menu_footer(&entries) {
            ui.set_width(super::detail::VIEW_ONLY_MENU_WIDTH);
            crate::kit::menu::note(ui, footer);
        }
        for entry in entries {
            match entry {
                MenuEntry::Command(entry) => {
                    if menu_command(ui, &entry) {
                        command = Some(entry.command);
                    }
                }
                MenuEntry::MoveToFolder(destinations) => {
                    crate::kit::menu::submenu(ui, move_to_folder_label(), |ui| match destinations {
                        MoveMenu::Targets(targets) => {
                            for target in targets {
                                if menu_command(ui, &target) {
                                    command = Some(target.command);
                                }
                            }
                        }
                        // Said out loud rather than left as an empty box,
                        // which reads as a submenu that failed to load.
                        MoveMenu::Empty(note) => {
                            crate::kit::menu::item(ui, crate::kit::menu::Item::new(note).enabled(false));
                        }
                    });
                }
                // The same shape as the arm above and with no empty case to
                // handle: `icon_menu` withholds the whole entry rather than
                // handing back a list with nothing in it, so there is no
                // second variant here to unfold. See `MenuEntry::Icon`.
                MenuEntry::Icon(actions) => {
                    crate::kit::menu::submenu(ui, icon_label(), |ui| {
                        for action in actions {
                            if menu_command(ui, &action) {
                                command = Some(action.command);
                            }
                        }
                    });
                }
            }
        }
    });
    RowOutcome {
        // A right-click selects the row too -- see `RowOutcome::select`.
        select: response.clicked() || response.secondary_clicked(),
        // ...but only the primary click OPENS it -- see `RowOutcome::opened`.
        opened: response.clicked(),
        command,
    }
}

/// The header's title while the search box has anything in it.
pub fn results_title() -> &'static str {
    crate::i18n::t("list.results")
}

/// Spec 3.1's header: padding `24 24 14`, the title 26/800 and the count 15
/// `text_ghost` beside it, and the compact `+` at the right that opens the
/// New menu. Returns what that menu chose.
fn list_header(ui: &mut egui::Ui, title: &str, count: usize) -> Option<ItemListAction> {
    let p = tokens::palette(ui.ctx());
    let mut chosen = None;
    egui::Frame::new()
        .inner_margin(Margin { left: 24, right: 24, top: 24, bottom: 14 })
        .show(ui, |ui| {
            // A fixed band: a right-to-left layout left to size itself would
            // centre in, and claim, the whole pane's height.
            let band = egui::vec2(
                ui.available_width(),
                tokens::font_height(ui.ctx(), tokens::LIST_HEADER).max(NEW_BUTTON_SIZE),
            );
            ui.allocate_ui_with_layout(band, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // ALWAYS opens the menu; the button itself creates nothing.
                // `Popup::menu` draws into its own `Area`, so nothing in it
                // allocates here -- which keeps the list beneath in register
                // (`the_new_menu_does_not_change_the_rows_below_it`).
                let new = new_button(ui, &p);
                crate::kit::menu::menu(&new, |ui| {
                    // From `CREATABLE_KINDS`, never a list written out here:
                    // that array keeps `ItemKind::Unknown` out of creation.
                    for kind in super::detail_edit::CREATABLE_KINDS {
                        let label = super::detail::kind_label(kind);
                        let mut row = crate::kit::menu::Item::new(&label);
                        if let Some(choice) = super::new_item_modal::choices().iter().find(|c| c.kind == kind) {
                            row = row.icon(choice.glyph);
                        }
                        if crate::kit::menu::item(ui, row).clicked() {
                            chosen = Some(ItemListAction::NewItem(kind));
                            ui.close();
                        }
                    }
                    // The import is not a kind (its type comes from the
                    // payload), so it sits below a separator.
                    crate::kit::menu::separator(ui);
                    let import = crate::kit::menu::Item::new(super::record_ui::import_from_send_label())
                        .icon(kit::icon::glyph::RECEIVED);
                    if crate::kit::menu::item(ui, import).clicked() {
                        chosen = Some(ItemListAction::ImportFromSend);
                        ui.close();
                    }
                });
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Max), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let count = tokens::job(tokens::BODY, &count.to_string(), p.text_ghost);
                    let count_width = ui.fonts_mut(|f| f.layout_job(count.clone()).size().x);
                    let room = (ui.available_width() - count_width - 10.0).max(1.0);
                    let mut title_job = tokens::job(tokens::LIST_HEADER, title, p.ink);
                    title_job.wrap = egui::text::TextWrapping::truncate_at_width(room);
                    ui.label(title_job);
                    // Nudged down to the title's baseline: the two sizes share
                    // a bottom edge, not a baseline, in egui's boxes.
                    ui.add_space(0.0);
                    ui.label(count);
                });
            });
        });
    chosen
}

/// The compact `+` beside the list's title (slice 3, decision 1).
const NEW_BUTTON_SIZE: f32 = 34.0;

/// The `+`'s own id: it paints no words to be found by, so the window's
/// tests read its rect back off the context (`ctx.read_response`).
pub(crate) fn new_button_id() -> egui::Id {
    egui::Id::new("vault-list-new")
}

/// The `+`: `kit::controls::icon_button`'s look, under [`new_button_id`].
fn new_button(ui: &mut egui::Ui, p: &tokens::Palette) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(NEW_BUTTON_SIZE), Sense::hover());
    let response = ui.interact(rect, new_button_id(), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(11), p.hover);
    }
    kit::icon::stroke(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(19.0)),
        kit::icon::glyph::PLUS,
        2.0,
        p.text_icon,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(new_item_hint())
}
/// What hovering the `+` says.
pub fn new_item_hint() -> &'static str {
    crate::i18n::t("list.new_item")
}

/// Spec 3.1's offline banner. Returns whether Retry was clicked.
fn offline_banner(ui: &mut egui::Ui) -> bool {
    let p = tokens::palette(ui.ctx());
    let bold = tokens::Role { size: 14.0, face: theme::BOLD, tracking_em: 0.0 };
    let line = tokens::Role { size: 13.0, face: theme::REGULAR, tracking_em: 0.0 };
    let mut retry = false;
    egui::Frame::new()
        .outer_margin(Margin { left: 14, right: 14, top: 0, bottom: 10 })
        .inner_margin(Margin::symmetric(14, 12))
        .corner_radius(CornerRadius::same(14))
        .fill(p.caution_wash)
        .stroke(Stroke::new(1.0, p.caution_edge))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (glyph, _) = ui.allocate_exact_size(egui::Vec2::splat(18.0), Sense::hover());
                kit::icon::stroke(ui.painter(), glyph, kit::icon::glyph::WIFI_OFF, 2.2, p.caution_mark);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(tokens::job(bold, offline_title(), p.caution_ink));
                    ui.label(tokens::job(line, offline_detail(), p.caution_ink));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let button = ui
                        .add(egui::Label::new(tokens::job(bold, offline_retry(), p.caution_ink)).sense(Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    retry = button.clicked();
                });
            });
        });
    retry
}

/// Spec 3.1's banner copy, the apostrophe straight as the design sets it.
pub fn offline_title() -> &'static str {
    crate::i18n::t("list.offline_title")
}
pub fn offline_detail() -> &'static str {
    crate::i18n::t("list.offline_detail")
}
pub fn offline_retry() -> &'static str {
    crate::i18n::t("list.offline_retry")
}

/// What a list is, for its entrance: the scope and the search. A change in
/// either is a new list, and a new list enters again.
fn entrance_key(filter: &SidebarFilter, search_lower: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{filter:?}").hash(&mut hasher);
    search_lower.trim().hash(&mut hasher);
    hasher.finish()
}

/// One list's entrance: when it started, and the rows that were on screen on
/// its first frame. Rows scrolled into view later arrive at rest.
#[derive(Clone)]
struct Entrance {
    key: u64,
    start: f64,
    rows: std::ops::Range<usize>,
}

/// Spec 3.1: `dw-up .4s ease-out`, `i * 0.035s` apart.
const ENTRANCE_DURATION: f32 = 0.4;
const ENTRANCE_STAGGER: f32 = 0.035;

fn entrance_for(ctx: &egui::Context, key: u64, visible: std::ops::Range<usize>) -> Entrance {
    let id = egui::Id::new("vault-list-entrance");
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| {
        let current: Option<Entrance> = d.get_temp(id);
        match current {
            Some(e) if e.key == key => e,
            // The first list this window shows arrives at rest: the splash
            // that covers the opening vault is its entrance.
            None => {
                let first = Entrance { key, start: now, rows: 0..0 };
                d.insert_temp(id, first.clone());
                first
            }
            Some(_) => {
                let fresh = Entrance { key, start: now, rows: visible };
                d.insert_temp(id, fresh.clone());
                fresh
            }
        }
    })
}

impl Entrance {
    /// Where row `row` is in the entrance, asking for the next frame only
    /// while some row is still moving.
    fn pose(&self, ctx: &egui::Context, row: usize) -> motion::Pose {
        if !self.rows.contains(&row) {
            return motion::Pose::REST;
        }
        let delay = motion::stagger(row - self.rows.start, ENTRANCE_STAGGER);
        let elapsed = (ctx.input(|i| i.time) - self.start) as f32 - delay;
        let linear = (elapsed / ENTRANCE_DURATION).clamp(0.0, 1.0);
        if linear >= 1.0 {
            return motion::Pose::REST;
        }
        ctx.request_repaint();
        motion::up(motion::EASE_OUT.at(linear))
    }
}

/// A row's initials (spec 3.2): two words give their first letters, one word
/// its first two, nothing a `?`; upper-cased.
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let text: String = match (words.next(), words.next()) {
        (Some(a), Some(b)) => a.chars().take(1).chain(b.chars().take(1)).collect(),
        (Some(a), None) => a.chars().take(2).collect(),
        _ => "?".to_string(),
    };
    text.to_uppercase()
}

/// What a row's 44pt tile shows: a card's network tile, else the site's
/// favicon, else initials on the name's colour.
pub(crate) fn row_look(item: &VaultItem, icon: Option<&egui::TextureHandle>) -> kit::avatar::Look {
    // Spec B 1.2: a chosen colour wins over everything automatic -- the
    // card's network tile and the website's icon -- as it does in the editor.
    if let Some((bg, fg)) = crate::item_icon::chosen_colours(item) {
        return kit::avatar::Look::Initials { text: initials(&item.name), bg, fg };
    }
    // Design 1e: a bank is the main badge and the card's network a chip on
    // it; with no bank the network is the badge.
    let net = card_net(item);
    if let Some(bank) = item_bank(item) {
        return kit::avatar::Look::Brand { mark: Mark::Bank(bank), chip: net };
    }
    if let Some(net) = net {
        return kit::avatar::Look::Brand { mark: Mark::Net(net), chip: None };
    }
    if let Some(texture) = icon {
        return kit::avatar::Look::Favicon(texture.clone());
    }
    let (bg, fg) = kit::avatar::initials_colours(&item.name);
    kit::avatar::Look::Initials { text: initials(&item.name), bg, fg }
}

/// A row's subtitle: a card's masked last four, or the login's username.
/// Spec C §3: a trashed row says when it went, from `deletedDate`.
pub(crate) fn deleted_line(item: &VaultItem, today: i64) -> Option<String> {
    let deleted = item.other.get("deletedDate")?.as_str()?;
    let days = (today - super::detail::date_days(deleted)?).max(0);
    Some(match days {
        0 => crate::i18n::t("list.deleted_today").to_string(),
        1 => crate::i18n::t("list.deleted_yesterday").to_string(),
        n => crate::i18n::tn("list.deleted_days_ago", n as u64, &[]),
    })
}

pub(crate) fn row_subtitle(item: &VaultItem) -> String {
    if let Some(line) = deleted_line(item, super::detail::today_days()) {
        return line;
    }
    if let Some(last) = card_last_four(item) {
        return format!("\u{2022}\u{2022}\u{2022}\u{2022} {last}");
    }
    item.login.as_ref().and_then(|l| l.username.clone()).unwrap_or_default()
}

/// A row's tags, in spec 3.1's order: App, 2FA, then View only and Shared.
pub(crate) fn row_tags(item: &VaultItem, marks: RowMarks<'_>) -> Vec<kit::card::TagKind> {
    use kit::card::TagKind;
    let mut tags = Vec::with_capacity(4);
    if crate::vault_bridge::extract_app_match(item).is_some() {
        tags.push(TagKind::App);
    }
    if item.login.as_ref().and_then(|l| l.totp.as_ref()).is_some() {
        tags.push(TagKind::TwoFa);
    }
    if marks.from.is_some() {
        tags.push(TagKind::ViewOnly);
    }
    if marks.send || marks.person {
        tags.push(TagKind::Shared);
    }
    tags
}

/// The selected row's lift: `0 2px 8px rgba(32,30,29,.08)`.
const SELECTED_ROW_SHADOW: egui::Shadow =
    egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: egui::Color32::from_rgba_premultiplied(3, 2, 2, 20) };

/// Paints one row into `rect`: spec 3.1's tile, avatar, title and the
/// subtitle line with its tags. Allocates nothing.
#[allow(clippy::too_many_arguments)]
fn paint_row(
    ui: &egui::Ui,
    rect: egui::Rect,
    item: &VaultItem,
    selected: bool,
    icon: Option<&egui::TextureHandle>,
    marks: RowMarks<'_>,
    pose: motion::Pose,
    hovered: bool,
) {
    use egui::epaint::RectShape;
    let p = tokens::palette(ui.ctx());
    let mut painter = ui.painter().clone();
    painter.multiply_opacity(pose.opacity);
    let rect = rect.translate(egui::vec2(pose.dx, pose.dy));
    let radius = CornerRadius::same(ROW_RADIUS);
    if selected {
        painter.add(SELECTED_ROW_SHADOW.as_shape(rect, radius));
        // The ring is painted INSIDE the row: outside it, the scroll area
        // clips the first row's top edge.
        painter.add(RectShape::new(rect, radius, p.card, Stroke::new(2.0, p.blue), egui::StrokeKind::Inside));
    } else if hovered {
        painter.rect_filled(rect, radius, p.hover);
    }

    let tile = egui::Rect::from_min_size(
        egui::pos2(rect.left() + ROW_PAD_X, rect.center().y - AVATAR_SIZE / 2.0),
        egui::Vec2::splat(AVATAR_SIZE),
    );
    kit::avatar::paint(&painter, tile, &row_look(item, icon), 12);

    let left = tile.right() + ROW_GAP_X;
    let width = (rect.right() - ROW_PAD_X - left).max(1.0);
    let ctx = ui.ctx();
    let title_h = tokens::font_height(ctx, tokens::ROW_TITLE);
    let subtitle = row_subtitle(item);
    let tags = row_tags(item, marks);
    let tag_sizes: Vec<egui::Vec2> =
        tags.iter().map(|t| kit::card::tag_size(ctx, *t, kit::card::TagSize::List)).collect();
    let second_line = !subtitle.is_empty() || !tags.is_empty();
    let sub_h = tokens::font_height(ctx, tokens::SUBTITLE);
    let line_h = tag_sizes.iter().map(|s| s.y).fold(sub_h, f32::max);
    let column_h = if second_line { title_h + TITLE_GAP_Y + line_h } else { title_h };
    let top = rect.center().y - column_h / 2.0;

    let mut title = tokens::job(tokens::ROW_TITLE, &item.name, p.ink);
    title.wrap = egui::text::TextWrapping::truncate_at_width(width);
    painter.galley(egui::pos2(left, top), painter.layout_job(title), p.ink);
    if !second_line {
        return;
    }
    let line_top = top + title_h + TITLE_GAP_Y;
    let tags_width: f32 = tag_sizes.iter().map(|s| s.x).sum::<f32>() + TAG_GAP * tags.len() as f32;
    let mut x = left;
    if !subtitle.is_empty() {
        let room = (width - tags_width).max(1.0);
        let mut job = tokens::job(tokens::SUBTITLE, &subtitle, p.text_faint);
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        let galley = painter.layout_job(job);
        let at = egui::pos2(x, line_top + (line_h - galley.size().y) / 2.0);
        x += galley.size().x + TAG_GAP;
        painter.galley(at, galley, p.text_faint);
    }
    for (tag, size) in tags.iter().zip(&tag_sizes) {
        kit::card::paint_tag(&painter, egui::pos2(x, line_top + (line_h - size.y) / 2.0), *tag, kit::card::TagSize::List);
        x += size.x + TAG_GAP;
    }
}

/// Spec 3.1's subtitle row `gap: 6`.
const TAG_GAP: f32 = 6.0;

/// The inline "that move did not happen" band. Returns whether it was
/// clicked, which dismisses it.
///
/// Wrapped rather than truncated: every message this shows is a sentence
/// explaining a refusal or a failure, and a truncated explanation is worse
/// than none. It is outside the virtualized list, so its height is free to
/// vary -- the rows' fixed pitch is not.
fn move_error_band(ui: &mut egui::Ui, message: &str) -> bool {
    const PAD: i8 = 10;
    /// The dismiss glyph's lane, taken off the width before the message is
    /// laid out.
    const GLYPH_LANE: f32 = 14.0;
    const GAP: f32 = 8.0;
    let band = egui::Frame::new()
        .fill(theme::CARD)
        .inner_margin(Margin::same(PAD))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                // LEFT TO RIGHT, deliberately -- NOT the `right_to_left`
                // trick the toolbar strip above uses to give `+ New` its
                // width first. In a right-to-left row a `Label` is handed an
                // effectively unbounded width and never wraps: this sentence
                // was painted COMPLETE and entirely off the right edge of a
                // 390pt pane, and the test that read the painted text agreed
                // it was fine. `the_band_paints_the_whole_message_inside_the_
                // pane` is what actually catches that, and it fails on that
                // layout today.
                //
                // The explicit width reserves the dismiss glyph's lane. It
                // can only ever SHRINK the label (`set_max_width` cannot
                // widen a ui past its parent), which is exactly what is
                // wanted here.
                let text_width = (ui.available_width() - GLYPH_LANE - GAP).max(0.0);
                ui.scope(|ui| {
                    ui.set_max_width(text_width);
                    ui.add(
                        egui::Label::new(
                            RichText::new(message).size(11.0).color(theme::ERROR),
                        )
                        .wrap(),
                    );
                });
                ui.add_space(GAP);
                ui.label(RichText::new("✕").size(11.0).color(theme::TEXT_GHOST));
            });
        })
        .response;
    // A fresh interaction over the band's rect, with an id of its own, rather
    // than `band.interact(Sense::click())`: the `Frame`'s own response id
    // belongs to the labels' container, and widening ITS sense makes the
    // whole band's clickability depend on which child happened to claim the
    // pointer first. This claims the band as one control.
    let response = ui.interact(band.rect, ui.id().with("move-error-band"), Sense::click());
    // The design has no dedicated error tile, so this borrows the pane's own
    // card surface and marks it with a full-width rule in the error colour
    // along its bottom edge -- the same "hairline under a strip" device the
    // toolbar above uses, recoloured.
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::Pos2::new(response.rect.left(), response.rect.bottom() - 1.0),
            egui::Pos2::new(response.rect.right(), response.rect.bottom()),
        ),
        CornerRadius::ZERO,
        theme::ERROR,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// The chip that follows the pointer while an item is being dragged, naming
/// the item so the user can see what is in the air.
///
/// Painted into a `Tooltip`-order layer through `layer_painter`, which
/// allocates NOTHING in the calling ui -- the alternative,
/// `Ui::dnd_drag_source`, re-parents the row's own widgets into a floating
/// layer and would fight both the `push_id` the context menu is anchored to
/// and the fixed row pitch `show_rows` scrolls by.
fn drag_ghost(ui: &egui::Ui, name: &str) {
    let Some(pointer) = ui.ctx().pointer_interact_pos() else {
        return;
    };
    const PAD_X: f32 = 8.0;
    const PAD_Y: f32 = 5.0;
    /// Offset from the cursor's hot spot, so the chip does not sit under the
    /// pointer and hide the row it is about to be dropped on.
    const CURSOR_OFFSET: egui::Vec2 = egui::Vec2::new(14.0, 10.0);
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("vault-drag-ghost")));
    let galley = painter.layout_no_wrap(
        name.to_string(),
        egui::FontId::new(TITLE_SIZE, egui::FontFamily::Name(theme::SEMIBOLD.into())),
        theme::CARD,
    );
    let rect = egui::Rect::from_min_size(
        pointer + CURSOR_OFFSET,
        galley.size() + egui::Vec2::new(PAD_X * 2.0, PAD_Y * 2.0),
    );
    painter.rect_filled(rect, CornerRadius::same(8), theme::BLUE);
    painter.galley(rect.min + egui::Vec2::new(PAD_X, PAD_Y), galley, theme::CARD);
}

/// One line of a row's context menu, drawn from [`MenuCommand`] and deciding
/// nothing itself. Returns whether it was chosen.
///
/// A disabled entry states its reason on hover; that is the entire point of
/// greying it rather than dropping it (see [`MenuCommand::enabled`]), so the
/// two are set together here and cannot be drawn apart.
///
/// A destructive entry is painted in [`theme::ERROR`] -- see
/// [`menu_command_button`], which does the drawing, and
/// [`RowCommand::is_destructive`], which decides. That is here and not at the
/// four call sites that build entries, so this file's Delete and the detail
/// pane's are the same red by construction rather than by two people
/// remembering.
///
/// `pub(super)` for [`move_menu`]'s reason: the detail pane's kebab draws the
/// same [`MenuCommand`]s and must grey them, and state their reason, exactly
/// as this does.
pub(super) fn menu_command(ui: &mut egui::Ui, entry: &MenuCommand) -> bool {
    if menu_command_button(ui, entry).clicked() {
        ui.close();
        return true;
    }
    false
}

/// [`menu_command`] without the click handling: **what a menu entry LOOKS
/// like**, handed back as its `Response` so the caller can still hang an
/// enabled-state tooltip on it.
///
/// It exists because the detail pane's kebab draws two entries this file's
/// lists do not reach -- the live item's own Delete, which is built there and
/// not from [`menu_entries`], and the trashed pane's "Delete forever",
/// which comes from [`out_of_vault_entries`] but wants a hover sentence of
/// its own. Both used to hand-roll `Button::new(RichText::new(..).color(
/// theme::ERROR))`, which made two definitions of "a destructive menu entry
/// is red" that nothing kept in step; folding them onto this one is what lets
/// the row menu's Delete become red by changing one match arm rather than
/// three call sites.
///
/// **The red goes on the WORDS and on nothing else**, which is not a detail
/// but the entire lesson of the entry this replaced: that one carried a
/// `.fill(theme::CARD)`, `Button::fill` is documented to "override any
/// on-hover effects", and the result was the one row in the menu that never
/// lit up under the pointer -- the "delete item doesn't get hovered in menu"
/// report. A [`RichText`] colour touches the galley and leaves
/// `widgets.hovered.bg_fill` alone, so every entry drawn through here, red or
/// not, takes the theme's hover exactly as its siblings do. See
/// `detail::tests::the_pointer_tints_the_kebabs_delete_the_way_it_tints_its_siblings`,
/// which is the assertion that keeps it that way.
pub(super) fn menu_command_button(ui: &mut egui::Ui, entry: &MenuCommand) -> egui::Response {
    // One row of the kit's menu: the red is the row's `danger` and nothing else
    // is touched, so every entry, red or not, takes the same hover wash. A row
    // is one line and the menu widens to it -- an entry is a name to click,
    // never a paragraph.
    let row = crate::kit::menu::Item::new(entry.label.as_str())
        .danger(entry.command.is_destructive())
        .enabled(entry.enabled);
    let response = crate::kit::menu::item(ui, row);
    match entry.disabled_reason {
        Some(reason) if !entry.enabled => response.on_hover_text(reason),
        _ => response,
    }
}

/// `box-shadow: 0 1px 2px rgba(45, 43, 43, 0.06)` -- the design's selected
/// row.
///
/// **The value moved to [`theme::SELECTED_ROW_SHADOW`]; this is the local
/// name for it, not a second copy.** Design 5b's Sends screen takes this same
/// column over and draws its own rows, and while this token was private here
/// that screen's selected row simply had no shadow -- one of three ways the
/// hand-rebuilt Send row came out looking like a different application in the
/// same slot. The name is kept because `item_row` reads far better for it
/// than for a fully-qualified path in the middle of a `Frame` builder.
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
            ssh_key: None,
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

    /// A card (`type: 3`) with `number` stored exactly as the vault stores
    /// it -- a `Zeroizing<String>` on `CardData` -- so nothing here is
    /// asserting against a shape the bridge does not produce.
    fn card_numbered(name: &str, number: &str) -> VaultItem {
        let mut it = item(name, None, Some(3));
        it.card = Some(crate::vault_bridge::CardData {
            number: Some(zeroize::Zeroizing::new(number.to_string())),
            ..Default::default()
        });
        it
    }

    /// The names of everything a query leaves standing, in list order --
    /// which is what the user sees, and what "assert the list, not that it is
    /// non-empty" means.
    fn found(items: &[VaultItem], query: &str) -> Vec<String> {
        items
            .iter()
            .filter(|it| matches_filter(it, &SidebarFilter::All, query))
            .map(|it| it.name.clone())
            .collect()
    }

    #[test]
    fn the_last_four_are_the_last_four_digits_of_a_cards_number() {
        assert_eq!(
            card_last_four(&card_numbered("BoA Credit", "4242424242424545")).as_deref(),
            Some("4545")
        );
    }

    #[test]
    fn separators_in_a_stored_number_do_not_change_its_last_four() {
        // The same card typed two ways. Were the digits not filtered, the
        // spaced form would answer "4545" only by luck of where the space
        // fell -- a dash immediately before the last four would answer
        // "-454".
        assert_eq!(
            card_last_four(&card_numbered("Spaced", "4242 4242 4242 4545")).as_deref(),
            Some("4545")
        );
        assert_eq!(
            card_last_four(&card_numbered("Dashed", "4242-4242-4242-4545")).as_deref(),
            Some("4545")
        );
    }

    #[test]
    fn fewer_than_four_digits_stored_reveals_nothing_at_all() {
        // The card-art spec's rule: a partial number is a data-entry state,
        // and "the last four" of a three-digit fragment is all of it.
        assert_eq!(card_last_four(&card_numbered("Partial", "454")), None);
        assert_eq!(card_number_suffix(&card_numbered("Partial", "454")), None);
        // And exactly four IS a card, so the floor is `< 4` and not `<= 4`.
        assert_eq!(card_last_four(&card_numbered("Four", "4545")).as_deref(), Some("4545"));
    }

    #[test]
    fn a_card_with_no_number_or_an_empty_one_has_no_last_four() {
        assert_eq!(card_last_four(&item("Bare card", None, Some(3))), None);
        assert_eq!(card_last_four(&card_numbered("Empty", "")), None);
    }

    #[test]
    fn the_rule_is_keyed_on_kind_not_on_text() {
        // A login whose NAME is a card-shaped number. Nothing about it is a
        // card, so it grows neither a suffix nor a digit match.
        let login = item("4242424242424545", None, Some(1));
        assert_eq!(card_last_four(&login), None);
        assert_eq!(card_number_suffix(&login), None);

        // **And the case that actually exercises the kind check.** `type` and
        // the `card` payload are two independent fields on `VaultItem`, so an
        // item can carry card DATA while being a login -- a stale payload
        // left behind by a kind change, or anything a server chose to send.
        // The check on `ItemKind` is the only thing standing between that and
        // a login row painting a card's digits; with the check deleted the
        // assertions above still pass, because the login above has no `card`
        // at all.
        let mut disguised = card_numbered("Looks like a login", "4242424242424545");
        disguised.item_type = Some(1);
        assert_eq!(card_last_four(&disguised), None);
        assert_eq!(card_number_suffix(&disguised), None);
        assert!(found(&[disguised], "4545").is_empty());
    }

    #[test]
    fn the_suffix_is_one_asterisk_and_the_four_digits_in_parentheses() {
        assert_eq!(
            card_number_suffix(&card_numbered("BoA Credit", "4242424242424545")).as_deref(),
            Some("(*4545)")
        );
    }

    #[test]
    fn searching_the_last_four_finds_the_card_and_only_the_card() {
        let items = [
            card_numbered("BoA Credit", "4242424242424545"),
            item("Ledgerline", None, Some(1)),
        ];
        assert_eq!(found(&items, "4545"), ["BoA Credit"]);
    }

    #[test]
    fn a_login_is_not_found_by_a_cards_digits() {
        // The live control on the arm above: the query returns nothing at
        // all when the only item is a login, so "finds the card" above is a
        // statement about the card and not about the query matching
        // everything.
        let items = [item("Ledgerline", None, Some(1))];
        assert!(found(&items, "4545").is_empty());
    }

    #[test]
    fn the_name_and_username_arms_still_match_after_the_card_arm_was_added() {
        // The control that the new arm was ADDED and did not replace the two
        // beside it -- the failure a card-only test could not see.
        let items = [
            card_numbered("BoA Credit", "4242424242424545"),
            item("Ledgerline", Some("a.novak@ledgerline.com"), Some(1)),
        ];
        assert_eq!(found(&items, "boa"), ["BoA Credit"]);
        assert_eq!(found(&items, "ledger"), ["Ledgerline"]);
        assert_eq!(found(&items, "novak"), ["Ledgerline"]);
    }

    #[test]
    fn a_card_too_short_to_show_its_digits_is_not_findable_by_them_either() {
        // The two rules are the same fact: the row shows nothing, so nothing
        // can be searched for. A disagreement here is a card findable by
        // digits it does not display.
        let items = [card_numbered("Partial", "454")];
        assert_eq!(card_number_suffix(&items[0]), None);
        assert!(found(&items, "454").is_empty());
        // Still findable by its name, so it has not fallen out of the list.
        assert_eq!(found(&items, "partial"), ["Partial"]);
    }

    #[test]
    fn a_middle_fragment_of_the_number_does_not_find_the_card() {
        // "You can search for what you can see." `4242` is stored and is not
        // painted anywhere, so it must not pull the card up.
        let items = [card_numbered("BoA Credit", "4242424242424545")];
        assert!(found(&items, "4242").is_empty());
        assert_eq!(found(&items, "4545"), ["BoA Credit"]);
    }

    #[test]
    fn a_partial_query_of_the_last_four_matches_like_any_other_fragment() {
        let items = [card_numbered("BoA Credit", "4242424242424545")];
        assert_eq!(found(&items, "45"), ["BoA Credit"]);
        assert_eq!(found(&items, "545"), ["BoA Credit"]);
    }

    #[test]
    fn the_suffixs_own_punctuation_is_tolerated_but_is_not_a_query_by_itself() {
        let items = [card_numbered("BoA Credit", "4242424242424545")];
        // Typed straight off the row.
        assert_eq!(found(&items, "*4545"), ["BoA Credit"]);
        assert_eq!(found(&items, "(*4545)"), ["BoA Credit"]);
        // Punctuation alone trims to nothing and matches no card, rather
        // than matching every card there is.
        assert!(found(&items, "*").is_empty());
        assert!(found(&items, "()").is_empty());
    }

    #[test]
    fn out_of_scope_items_never_match_regardless_of_search() {
        let it = item("Ledgerline", None, Some(3)); // a Card
        assert!(!matches_filter(&it, &SidebarFilter::Logins, ""));
        assert!(!matches_filter(&it, &SidebarFilter::Logins, "ledgerline"));
    }

    /// Under Trash and Archive this pane is handed the list from that row's
    /// own query, which is already exactly the row's contents -- so every
    /// item in it is listed, and the search box still narrows it.
    ///
    /// The second half is the one that can regress unnoticed: a Trash scope
    /// that short-circuited to "everything matches" would leave the search
    /// box on screen doing nothing, which is a silent no-op in a control the
    /// user is actively typing into.
    #[test]
    fn the_trash_and_archive_scopes_list_what_their_query_returned_and_still_search_it() {
        let ledgerline = item("Ledgerline", None, Some(1));
        let vantage = item("Vantage", None, Some(3));
        for scope in [SidebarFilter::Trash, SidebarFilter::Archive] {
            assert!(matches_filter(&ledgerline, &scope, ""));
            // ...including a CARD, which no type row would list -- these two
            // scopes hold a mixture of kinds and must not have quietly
            // acquired a type test.
            assert!(matches_filter(&vantage, &scope, ""));
            assert!(matches_filter(&ledgerline, &scope, "ledger"));
            assert!(!matches_filter(&ledgerline, &scope, "vantage"));
        }
    }
}

#[cfg(test)]
mod menu_entry_tests {
    //! What an item row's right-click menu contains, per kind.
    //!
    //! Every assertion here reads the WHOLE entry list, not "does entry X
    //! appear". A test that only probes for the entries it expects passes
    //! just as happily against a menu that also offers three things it must
    //! not -- "Copy password" on a card, say -- and this file's history is
    //! full of tests that agreed with a broken predicate rather than
    //! checking it.
    use super::*;
    use crate::vault_bridge::{LoginData, UriEntry};
    use zeroize::Zeroizing;

    /// The one place these tests name an entry's on-screen wording, so a
    /// relabelling is a single edit here and the assertions below stay
    /// readable as lists.
    fn labels(entries: &[MenuEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| match entry {
                MenuEntry::Command(c) => c.label.clone(),
                MenuEntry::MoveToFolder(_) => move_to_folder_label().to_string(),
                MenuEntry::Icon(_) => icon_label().to_string(),
            })
            .collect()
    }

    /// The labels of the entries that are actually clickable.
    fn enabled_labels(entries: &[MenuEntry]) -> Vec<String> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Command(c) => c.enabled.then(|| c.label.clone()),
                // Either submenu itself is always openable.
                MenuEntry::MoveToFolder(_) => Some(move_to_folder_label().to_string()),
                MenuEntry::Icon(_) => Some(icon_label().to_string()),
            })
            .collect()
    }

    fn move_menu_of(entries: &[MenuEntry]) -> MoveMenu {
        entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::MoveToFolder(menu) => Some(menu.clone()),
                MenuEntry::Command(_) | MenuEntry::Icon(_) => None,
            })
            .expect("no \"Move to folder\" entry was offered at all")
    }

    fn of_kind(item_type: Option<i64>) -> VaultItem {
        VaultItem {
            id: "i1".into(),
            name: "Ledgerline".into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// A fully-populated login: username, password, TOTP seed and a URI.
    fn full_login() -> VaultItem {
        VaultItem {
            login: Some(LoginData {
                username: Some("a.novak@ledgerline.com".into()),
                password: Some(Zeroizing::new("hunter2".into())),
                totp: Some(Zeroizing::new("JBSWY3DPEHPK3PXP".into())),
                uris: vec![UriEntry {
                    uri: Some("https://ledgerline.com".into()),
                    other: serde_json::Map::new(),
                }],
                other: serde_json::Map::new(),
            }),
            ..of_kind(Some(1))
        }
    }

    fn folder(id: &str, name: &str) -> Folder {
        Folder { id: id.into(), name: name.into(), other: serde_json::Map::new() }
    }

    /// **Ten top-level entries went to nine when the icon group folded into
    /// a submenu**, and this is the fixture that says which nine.
    ///
    /// The submenu's own contents are asserted in the same test rather than
    /// left to the icon tests further down: a fold that dropped a row on its
    /// way in would leave this list looking exactly right.
    #[test]
    fn a_full_login_offers_every_entry_in_the_agreed_order() {
        let entries = menu_entries(&full_login(), &[], FilterSource::LiveVault, false);
        assert_eq!(
            labels(&entries),
            vec![
                "Copy username",
                "Copy password",
                "Copy TOTP",
                "Open website",
                "Edit",
                move_to_folder_label(),
                "Archive",
                "Delete",
            ]
        );
    }

    /// The two menus for an item that is NOT in the live vault, whole.
    ///
    /// Asserted as the COMPLETE list, not as "Restore is present": the point
    /// of these menus is what they leave out. Every live entry -- Edit, Fill,
    /// the copies, Move to folder, Delete -- reads or writes through the live
    /// item list, which by definition does not hold a trashed or archived
    /// item, so any of them appearing here would be a click that does
    /// nothing. A `contains` assertion passes happily against exactly that.
    #[test]
    fn a_trashed_or_archived_item_offers_only_what_actually_works() {
        assert_eq!(
            labels(&menu_entries(
                &full_login(),
                &[folder("f1", "Work")],
                FilterSource::Trash,
                false,
            )),
            vec!["Restore", "Delete forever"]
        );
        assert_eq!(
            labels(&menu_entries(
                &full_login(),
                &[folder("f1", "Work")],
                FilterSource::Archive,
                false,
            )),
            vec!["Unarchive"]
        );
    }

    /// The source, not the item, is what selects the menu.
    ///
    /// The SAME item is passed to all three calls above and here, so nothing
    /// about these menus can be coming from the item's own contents -- which
    /// matters because a trashed item's JSON is an ordinary item's plus one
    /// key, and a menu keyed off that key would silently offer the live menu
    /// for any archived item (they carry no marker at all).
    #[test]
    fn the_menu_follows_the_list_the_row_was_drawn_from_not_the_item() {
        let item = full_login();
        let live = labels(&menu_entries(&item, &[], FilterSource::LiveVault, false));
        let trashed = labels(&menu_entries(&item, &[], FilterSource::Trash, false));
        assert!(live.contains(&"Delete".to_string()));
        assert!(!trashed.contains(&"Delete".to_string()));
        assert!(!live.contains(&"Restore".to_string()));
        assert!(trashed.contains(&"Restore".to_string()));
    }

    /// **A received record's row menu is the read half of the live one**
    /// (mock 6d): the copies and Open website stay; Edit, Move to folder,
    /// Archive and Delete -- each a write NodeWarden refuses on a view-only
    /// collection -- are not offered at all, rather than offered and then
    /// dropped by the window's gate.
    #[test]
    fn a_view_only_records_row_menu_offers_only_what_reads() {
        let mut item = full_login();
        item.other.insert("edit".into(), serde_json::Value::Bool(false));
        assert_eq!(
            labels(&menu_entries(&item, &[folder("f1", "Work")], FilterSource::LiveVault, true)),
            vec!["Copy username", "Copy password", "Copy TOTP", "Open website"]
        );
    }

    /// **And its Trash and Archive menus are empty**: Restore, Delete forever
    /// and Unarchive each write the record. The same item, writable, is the
    /// control.
    #[test]
    fn a_view_only_records_trash_and_archive_menus_offer_no_write() {
        let mut item = full_login();
        item.other.insert("edit".into(), serde_json::Value::Bool(false));
        for source in [FilterSource::Trash, FilterSource::Archive] {
            assert!(!labels(&menu_entries(&full_login(), &[], source, false)).is_empty(), "control: {source:?}");
            assert_eq!(labels(&menu_entries(&item, &[], source, false)), Vec::<String>::new(), "{source:?}");
        }
    }

    /// **"No folder" is offered exactly where the backend can perform it.**
    ///
    /// Both directions, because each is a different defect: withheld on a
    /// backend that CAN un-file is this app hiding something that works, and
    /// offered on one that cannot is the silent no-op
    /// `.superpowers/sdd/put-semantics-capture.md` measured -- a 200 that
    /// moves nothing, with the pane showing the item unfiled until the next
    /// sync puts it back.
    #[test]
    fn no_folder_is_offered_only_where_the_backend_can_unfile() {
        let folders = [folder("f1", "Work")];
        let item = of_folder(Some("f1"));

        let MoveMenu::Targets(with) = move_menu(&item, &folders, true) else {
            panic!("the submenu reported no destinations at all");
        };
        assert_eq!(
            with.first().map(|entry| entry.label.as_str()),
            Some(no_folder_label()),
            "\"No folder\" is not the first destination; it drew {:?}",
            with.iter().map(|e| &e.label).collect::<Vec<_>>()
        );

        let MoveMenu::Targets(without) = move_menu(&item, &folders, false) else {
            panic!("the submenu reported no destinations at all");
        };
        assert!(
            !without.iter().any(|entry| entry.label == no_folder_label()),
            "a backend that cannot un-file was offered \"No folder\" anyway: {:?}",
            without.iter().map(|e| &e.label).collect::<Vec<_>>()
        );
        // And nothing else moved: the folders below it are the same list.
        let names = |menu: &[MenuCommand]| {
            menu.iter()
                .filter(|e| e.label != no_folder_label())
                .map(|e| e.label.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&with), names(&without));
    }

    /// **It carries `Unfile`, never a `MoveToFolder("")`.**
    ///
    /// The empty string is the id of `bw serve`'s virtual "No Folder" bucket,
    /// and writing it is what strands an item out of every sidebar row --
    /// `the_move_submenu_never_offers_an_empty_folder_id` exists for that.
    /// A destination that means "no folder" therefore has to be a command
    /// carrying nothing rather than a folder id that happens to be empty.
    #[test]
    fn the_no_folder_entry_carries_no_folder_id_at_all() {
        let MoveMenu::Targets(targets) =
            move_menu(&of_folder(Some("f1")), &[folder("f1", "Work")], true)
        else {
            panic!("the submenu reported no destinations at all");
        };
        let entry = targets.first().expect("the first destination");
        assert_eq!(entry.command, RowCommand::Unfile, "it carried {:?}", entry.command);
    }

    /// **Greyed for an item that is already out of every folder**, with its
    /// own reason -- the same treatment the item's own folder gets, and for
    /// the same reason: a write that would achieve nothing, on a row that
    /// stays present so the destinations do not reshuffle from item to item.
    #[test]
    fn the_no_folder_entry_is_greyed_for_an_item_that_has_none() {
        let folders = [folder("f1", "Work")];
        for (item, enabled) in
            [(of_folder(Some("f1")), true), (of_folder(None), false), (of_folder(Some("")), false)]
        {
            let MoveMenu::Targets(targets) = move_menu(&item, &folders, true) else {
                panic!("the submenu reported no destinations at all");
            };
            let entry = targets.first().expect("the first destination");
            assert_eq!(
                entry.enabled, enabled,
                "with folder_id {:?} the \"No folder\" row was enabled={}",
                item.folder_id, entry.enabled
            );
            assert_eq!(entry.disabled_reason.is_some(), !enabled);
        }
    }

    /// **A vault with no assignable folders still offers it**, where the
    /// backend can un-file.
    ///
    /// Without this the submenu answers `Empty("no folders yet")` and the one
    /// destination that exists is unreachable -- which is the state an
    /// account that files nothing is permanently in, and exactly the account
    /// most likely to want an item taken back out of a folder created by
    /// another client.
    #[test]
    fn a_vault_with_no_folders_still_offers_no_folder() {
        let MoveMenu::Targets(targets) = move_menu(&of_folder(Some("f1")), &[], true) else {
            panic!("the submenu refused to offer the one destination that exists");
        };
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].command, RowCommand::Unfile);
        // And with the backend unable to, it is still the "no folders yet"
        // note rather than an empty box.
        assert!(matches!(move_menu(&of_folder(Some("f1")), &[], false), MoveMenu::Empty(_)));
    }

    /// An item filed in `folder_id`.
    fn of_folder(folder_id: Option<&str>) -> VaultItem {
        VaultItem { folder_id: folder_id.map(str::to_string), ..full_login() }
    }

    /// **Neither delete asks its question in the menu any more.** Both
    /// entries carry one fixed label and open
    /// [`super::super::delete_modal`], so a menu built twice in a row is the
    /// same menu -- there is no armed state for it to be in.
    ///
    /// Asserted as the ABSENCE of the old wording rather than only as the
    /// presence of the new: an entry that re-labelled itself under some
    /// condition this test does not set would pass a `contains("Delete")`
    /// happily, and the whole point of the modal is that the menu stopped
    /// being where the confirmation lives.
    #[test]
    fn neither_delete_entry_confirms_inside_the_menu() {
        for source in [FilterSource::LiveVault, FilterSource::Trash] {
            let once = labels(&menu_entries(&full_login(), &[], source, false));
            let again = labels(&menu_entries(&full_login(), &[], source, false));
            assert_eq!(once, again, "{source:?}: the menu is not the same twice");
            assert!(
                !once.iter().any(|label| label.contains("Click to confirm")),
                "{source:?}: a delete entry still confirms in the menu: {once:?}"
            );
        }
        assert_eq!(
            labels(&menu_entries(&full_login(), &[], FilterSource::Trash, false)),
            vec!["Restore", "Delete forever"]
        );
    }

    /// The five entries a plain card gets, in order.
    ///
    /// **The submenu holds `SELECT_ICON_LABEL` and not
    /// `REFRESH_ICON_LABEL`**, which is the whole shape of `icon_menu`'s rule
    /// in one fixture: a card with no `deskwarden:bank-domain` has no
    /// automatic icon to refresh, and giving it one by hand is exactly the
    /// thing that is worth offering. Asserted one level down now that the
    /// group is a submenu -- the top-level list only says the group is there.
    #[test]
    fn a_card_offers_no_open_website_but_can_be_edited() {
        // "Open website" is login-only (`detail::kind_offers_fill`) and is
        // ABSENT. Editing a card is offered and enabled -- `apply_to` writes
        // the card object -- which is the user-visible half of the 2026-08-17
        // fix.
        let entries = menu_entries(&of_kind(Some(3)), &[], FilterSource::LiveVault, false);
        assert_eq!(
            labels(&entries),
            vec!["Edit", move_to_folder_label(), "Archive", "Delete"]
        );
        assert_eq!(
            enabled_labels(&entries),
            vec!["Edit", move_to_folder_label(), "Archive", "Delete"]
        );
    }

    /// **Renamed from `..._the_same_four_as_a_card` when "Select icon..."
    /// made it five.** The count moved because the product changed, so the
    /// name moved with it rather than the list being trimmed to keep an old
    /// number true. It is five again for a different reason -- the icon group
    /// collapsed from one entry to one submenu -- and the name is left alone
    /// because what it claims (a note and a card get the same menu) is what
    /// this still checks.
    #[test]
    fn a_secure_note_offers_the_same_five_as_a_card() {
        let entries = menu_entries(&of_kind(Some(2)), &[], FilterSource::LiveVault, false);
        assert_eq!(
            labels(&entries),
            vec!["Edit", move_to_folder_label(), "Archive", "Delete"]
        );
        assert_eq!(
            enabled_labels(&entries),
            vec!["Edit", move_to_folder_label(), "Archive", "Delete"]
        );
    }

    /// An SSH key is the live case for the greyed entry now: it is creatable
    /// but not editable, because `apply_to` has no arm that writes its keys.
    #[test]
    fn the_greyed_edit_entry_says_why() {
        // Greying without a reason is the failure this is guarding: the user
        // sees the action they came for, unavailable, and no explanation.
        let entries = menu_entries(&of_kind(Some(5)), &[], FilterSource::LiveVault, false);
        let edit = entries
            .iter()
            .find_map(|e| match e {
                MenuEntry::Command(c) if c.command == RowCommand::Edit => Some(c.clone()),
                _ => None,
            })
            .expect("no Edit entry");
        assert!(!edit.enabled);
        assert_eq!(edit.disabled_reason, Some(edit_disabled_reason()));
    }

    /// **The login-only gate on "Open website", asked of an item that is
    /// NOT a login but carries a login blob anyway.**
    ///
    /// Until the row menu's "Fill in app" entry was removed at the user's
    /// request, `kind_offers_fill` was pinned by the card tests above: it
    /// guarded an entry that was pushed UNCONDITIONALLY inside its block,
    /// so deleting the gate put "Fill in app" on a card and they failed.
    /// With that entry gone the block holds only "Open website", which
    /// reads `login.uris` and is therefore absent on an ordinary card by
    /// accident of the item having no `login` at all -- and deleting the
    /// gate outright then broke nothing. That is a gate nothing tests.
    ///
    /// `VaultItem`'s deserialisation is lenient (`other` swallows what it
    /// does not name, and `login` is a plain `Option` no `type` agrees
    /// with), so "a card carrying a login blob" is a shape the CLI can
    /// hand us, not a hypothetical. This is the case that tells the two
    /// apart, and it fails against `if true {`.
    #[test]
    fn a_non_login_carrying_a_login_blob_still_offers_no_open_website() {
        let card_with_a_login = VaultItem {
            login: full_login().login,
            ..of_kind(Some(3))
        };
        // The premise: the blob really does hold the URI the entry reads,
        // so its absence below is the KIND being refused and not an empty
        // field.
        assert_eq!(
            card_with_a_login
                .login
                .as_ref()
                .and_then(|l| l.uris.first())
                .and_then(|u| u.uri.as_deref()),
            Some("https://ledgerline.com"),
            "the fixture carries no URI, so nothing here is being refused"
        );
        // And the positive control on the predicate itself: the SAME blob
        // on a login-typed item does offer the entry.
        assert!(labels(&menu_entries(&full_login(), &[], FilterSource::LiveVault, false))
            .contains(&"Open website".to_string()));
        assert_eq!(
            labels(&menu_entries(&card_with_a_login, &[], FilterSource::LiveVault, false)),
            vec![
                "Copy username",
                "Copy password",
                "Copy TOTP",
                "Edit",
                move_to_folder_label(),
                "Archive",
                "Delete",
            ],
            "a card carrying a login blob was offered a login-only entry"
        );
    }

    // ---- "Select icon..." and "Use the automatic icon" -------------------
    //
    // `icon_menu` decides all three icon entries from one read of the
    // item's field. These are its rules; what the chosen icon then DOES is
    // `vault_window::mod`'s half. They read `icon_labels` rather than
    // `labels`, because the three live one level down since the group became
    // a submenu -- and a rule asserted against the outer list would now be a
    // rule asserted against a list that never contains any of them.

    /// `item` with `value` on its `deskwarden:icon` field.
    fn with_icon_field(item: &VaultItem, value: &str) -> VaultItem {
        crate::vault_bridge::with_custom_field(item, crate::item_icon::ICON_FIELD_NAME, value)
    }

    fn chosen_url(item: &VaultItem) -> VaultItem {
        with_icon_field(
            item,
            &crate::item_icon::choice_from_url("https://cdn.example.com/logo.png")
                .expect("a good URL")
                .to_field_value(),
        )
    }

    /// **No row menu offers an icon entry any more, whatever the source.**
    ///
    /// This used to be about the trash and the archive alone: those two menus
    /// are `out_of_vault_entries`' and shared nothing with the live one, so
    /// the group could not leak into them. The group has now left the live
    /// menu too -- it is the edit pane's pencil badge, `detail_edit`'s
    /// `edit_header` -- and the claim widens to every source rather than
    /// being deleted, because "the icon entries are gone from this file's
    /// menus" is exactly what the owner asked for and is worth a test that
    /// fails if one comes back.
    ///
    /// Both halves: the group's own row, and each of the three underneath it,
    /// since a menu could shed the fold while keeping its contents.
    #[test]
    fn no_row_menu_offers_an_icon_entry_whatever_the_source() {
        for source in [FilterSource::LiveVault, FilterSource::Trash, FilterSource::Archive] {
            let entries = menu_entries(&chosen_url(&full_login()), &[], source, false);
            let top = labels(&entries);
            for label in [icon_label(), select_icon_label(), clear_icon_label(), refresh_icon_label()] {
                assert!(
                    !top.contains(&label.to_string()),
                    "{source:?} offered {label:?}: {top:?}"
                );
            }
            assert!(
                !entries.iter().any(|e| matches!(e, MenuEntry::Icon(_))),
                "{source:?} carries an Icon entry that `labels` cannot see"
            );
        }
    }

    #[test]
    fn edit_follows_kind_offers_edit_for_every_kind() {
        // Drives the predicate the menu itself consumes, so relaxing
        // `kind_offers_edit` enables the entry here without anyone having to
        // remember this file exists -- which is exactly what happened when
        // it was widened to cards, notes and identities.
        for item_type in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(9)] {
            let item = of_kind(item_type);
            let entries = menu_entries(&item, &[], FilterSource::LiveVault, false);
            let edit = entries
                .iter()
                .find_map(|e| match e {
                    MenuEntry::Command(c) if c.command == RowCommand::Edit => Some(c.clone()),
                    _ => None,
                })
                .expect("no Edit entry");
            assert_eq!(
                edit.enabled,
                kind_offers_edit(ItemKind::of(&item)),
                "Edit's enabled state disagrees with kind_offers_edit for type {item_type:?}"
            );
        }
    }

    #[test]
    fn copy_totp_appears_only_when_the_item_carries_a_seed() {
        let with_seed = full_login();
        let without = VaultItem {
            login: Some(LoginData { totp: None, ..with_seed.login.clone().unwrap() }),
            ..full_login()
        };
        assert!(labels(&menu_entries(&with_seed, &[], FilterSource::LiveVault, false)).contains(&"Copy TOTP".to_string()));
        assert_eq!(
            labels(&menu_entries(&without, &[], FilterSource::LiveVault, false)),
            vec![
                "Copy username",
                "Copy password",
                "Open website",
                "Edit",
                move_to_folder_label(),
                "Archive",
                "Delete",
            ],
            "removing the seed changed more than the TOTP entry"
        );
    }

    #[test]
    fn the_copy_entries_are_absent_when_there_is_nothing_to_copy() {
        // A login with empty strings where its credentials should be. An
        // entry that copies "" is the same untruth as a blank row.
        let empty = VaultItem {
            login: Some(LoginData {
                username: Some("  ".into()),
                password: Some(Zeroizing::new(String::new())),
                totp: None,
                uris: vec![UriEntry { uri: Some(String::new()), other: serde_json::Map::new() }],
                other: serde_json::Map::new(),
            }),
            ..of_kind(Some(1))
        };
        let entries = menu_entries(&empty, &[], FilterSource::LiveVault, false);
        assert_eq!(
            labels(&entries),
            vec!["Edit", move_to_folder_label(), "Archive", "Delete"]
        );
        // Empty strings where the credentials should be do not make the item
        // iconless: it still carries the URI the automatic icon is read off.
    }

    #[test]
    fn open_website_carries_the_url_the_detail_pane_would_open() {
        let entries = menu_entries(&full_login(), &[], FilterSource::LiveVault, false);
        let opens: Vec<&RowCommand> = entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Command(c) => Some(&c.command),
                MenuEntry::MoveToFolder(_) | MenuEntry::Icon(_) => None,
            })
            .filter(|c| matches!(c, RowCommand::OpenWebsite(_)))
            .collect();
        assert_eq!(
            opens,
            vec![&RowCommand::OpenWebsite("https://ledgerline.com".to_string())]
        );
    }

    #[test]
    fn the_move_submenu_excludes_the_virtual_no_folder_bucket() {
        // `bw serve` reports its "no folder" bucket AS A FOLDER with an
        // empty id. Offering it writes `folderId: ""` and strands the item
        // out of every sidebar row -- a Critical fixed in the edit form, and
        // this menu must not reintroduce it.
        let folders = [folder("", "No Folder"), folder("f1", "Work"), folder("f2", "Personal")];
        let MoveMenu::Targets(targets) = move_menu_of(&menu_entries(&full_login(), &folders, FilterSource::LiveVault, false))
        else {
            panic!("the submenu reported no assignable folders when two exist");
        };
        assert_eq!(
            targets.iter().map(|t| t.command.clone()).collect::<Vec<_>>(),
            vec![
                RowCommand::MoveToFolder("f1".into()),
                RowCommand::MoveToFolder("f2".into()),
            ]
        );
        assert_eq!(
            targets.iter().map(|t| t.label.clone()).collect::<Vec<_>>(),
            vec!["Work", "Personal"]
        );
    }

    #[test]
    fn a_real_folder_named_no_folder_is_still_a_destination() {
        // The bucket is identified by its empty id, never by its name -- a
        // user may own a folder actually called "No Folder", and matching on
        // the name would lock them out of it.
        let folders = [folder("f9", "No Folder")];
        let MoveMenu::Targets(targets) = move_menu_of(&menu_entries(&full_login(), &folders, FilterSource::LiveVault, false))
        else {
            panic!("a real folder named \"No Folder\" was dropped");
        };
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].command, RowCommand::MoveToFolder("f9".into()));
    }

    #[test]
    fn a_vault_with_no_assignable_folder_says_so_instead_of_opening_an_empty_box() {
        let folders = [folder("", "No Folder")];
        assert_eq!(
            move_menu_of(&menu_entries(&full_login(), &folders, FilterSource::LiveVault, false)),
            MoveMenu::Empty(no_assignable_folders())
        );
    }

    #[test]
    fn the_folder_the_item_already_lives_in_is_greyed_not_dropped() {
        let folders = [folder("f1", "Work"), folder("f2", "Personal")];
        let item = VaultItem { folder_id: Some("f1".into()), ..full_login() };
        let MoveMenu::Targets(targets) = move_menu_of(&menu_entries(&item, &folders, FilterSource::LiveVault, false)) else {
            panic!("the submenu reported no assignable folders when two exist");
        };
        assert_eq!(
            targets.iter().map(|t| (t.label.as_str(), t.enabled)).collect::<Vec<_>>(),
            vec![("Work", false), ("Personal", true)]
        );
        assert_eq!(targets[0].disabled_reason, Some(already_in_this_folder()));
    }

    #[test]
    fn there_is_no_un_file_destination() {
        // `bw serve` (CLI 2026.7.0) cannot clear a folder assignment: the
        // write succeeds and does nothing. Every destination this menu
        // offers must therefore name a real folder.
        let folders = [folder("", "No Folder"), folder("f1", "Work")];
        let MoveMenu::Targets(targets) = move_menu_of(&menu_entries(&full_login(), &folders, FilterSource::LiveVault, false))
        else {
            panic!("the submenu reported no assignable folders when one exists");
        };
        for target in &targets {
            let RowCommand::MoveToFolder(id) = &target.command else {
                panic!("the submenu offered {:?}, which is not a move", target.command);
            };
            assert!(!id.is_empty(), "{:?} would write an empty folderId", target.label);
        }
    }

    /// **Delete is the last entry and reads exactly one thing.** The entry
    /// used to carry the confirmation itself; it now opens
    /// [`super::super::delete_modal`], so its label is a constant and its
    /// position is the design's (below Archive, the gentler of the two ways
    /// to take an item out of the vault).
    #[test]
    fn delete_is_the_last_entry_and_wears_one_label() {
        assert_eq!(
            labels(&menu_entries(&of_kind(Some(3)), &[], FilterSource::LiveVault, false)).last().unwrap(),
            delete_label()
        );
    }
}

#[cfg(test)]
mod search_hint_tests {
    use super::{search_hint, SidebarFilter};

    #[test]
    fn the_noun_follows_the_active_filter() {
        // The whole reason this is a function. Hardcoding the design's
        // "logins" would put "Search 12 logins" over a list of cards.
        assert_eq!(search_hint(Some(180), &SidebarFilter::Logins), "Search 180 logins");
        assert_eq!(search_hint(Some(4), &SidebarFilter::Cards), "Search 4 cards");
        assert_eq!(search_hint(Some(21), &SidebarFilter::SecureNotes), "Search 21 secure notes");
        assert_eq!(search_hint(Some(9), &SidebarFilter::Passkeys), "Search 9 passkeys");
        assert_eq!(search_hint(Some(12), &SidebarFilter::Favorites), "Search 12 favorites");
        assert_eq!(search_hint(Some(3), &SidebarFilter::Identities), "Search 3 identities");
        assert_eq!(search_hint(Some(2), &SidebarFilter::SshKeys), "Search 2 SSH keys");
        assert_eq!(search_hint(Some(214), &SidebarFilter::All), "Search 214 items");
    }

    #[test]
    fn the_mixed_scopes_keep_the_neutral_noun() {
        // Trash and a folder both hold a mixture of kinds, so any specific
        // noun is wrong for most of what is in them.
        assert_eq!(search_hint(Some(6), &SidebarFilter::Trash), "Search 6 items");
        assert_eq!(
            search_hint(Some(64), &SidebarFilter::Folder("f-1".to_string())),
            "Search 64 items"
        );
    }

    #[test]
    fn one_item_is_singular_in_every_scope() {
        // The case a naive `format!("{n} {plural}")` gets wrong, and the one
        // a user with a small vault sees constantly.
        assert_eq!(search_hint(Some(1), &SidebarFilter::Logins), "Search 1 login");
        assert_eq!(search_hint(Some(1), &SidebarFilter::Identities), "Search 1 identity");
        assert_eq!(search_hint(Some(1), &SidebarFilter::SshKeys), "Search 1 SSH key");
        assert_eq!(search_hint(Some(1), &SidebarFilter::SecureNotes), "Search 1 secure note");
        assert_eq!(search_hint(Some(1), &SidebarFilter::All), "Search 1 item");
    }

    #[test]
    fn zero_takes_the_plural_the_way_english_does() {
        // "Search 0 login" is the other half of the same off-by-one.
        assert_eq!(search_hint(Some(0), &SidebarFilter::Logins), "Search 0 logins");
        assert_eq!(search_hint(Some(0), &SidebarFilter::All), "Search 0 items");
    }

    #[test]
    fn an_unfetched_list_states_no_count_at_all() {
        // The Trash and Archive rows are on-demand queries. Between the row
        // being selected and its fetch landing -- and permanently, if that
        // fetch failed -- this app does not know how many items are behind
        // it, and `0` is a claim it cannot make. It made it anyway, over an
        // empty placeholder list, one control to the right of the badge that
        // exists specifically to say the en dash instead.
        assert_eq!(search_hint(None, &SidebarFilter::Trash), "Search items");
        assert_eq!(search_hint(None, &SidebarFilter::Archive), "Search items");
    }

    #[test]
    fn an_unfetched_list_never_claims_zero_in_any_scope() {
        // The negative half of the pair above, over every variant: the
        // failure being guarded against is a digit appearing in this string,
        // and asserting the two exact sentences above would not notice a
        // third scope that still printed one.
        for filter in [
            SidebarFilter::All,
            SidebarFilter::Favorites,
            SidebarFilter::Apps,
            SidebarFilter::Logins,
            SidebarFilter::Passkeys,
            SidebarFilter::Cards,
            SidebarFilter::Identities,
            SidebarFilter::SecureNotes,
            SidebarFilter::SshKeys,
            SidebarFilter::Archive,
            SidebarFilter::Trash,
            SidebarFilter::Folder("f-1".to_string()),
            SidebarFilter::Unfiled,
        ] {
            let hint = search_hint(None, &filter);
            assert!(
                !hint.contains(char::is_numeric),
                "{filter:?} prints a count it does not have: {hint:?}"
            );
            // Positive control: an implementation that returned "" would
            // satisfy the assertion above for free.
            assert!(hint.starts_with("Search "), "{filter:?}: {hint:?}");
            assert!(hint.len() > "Search ".len(), "{filter:?} names no scope: {hint:?}");
        }
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
    use crate::theme;
    use eframe::egui::epaint::RectShape;

    const PANE_WIDTH: f32 = 390.0;
    const PANE_HEIGHT: f32 = 700.0;
    /// A row tile on a list that FITS: the pane minus the list frame's
    /// `padding: 10px` on each side.
    const TILE_WIDTH: f32 = PANE_WIDTH - 2.0 * LIST_PADDING;
    // **There is no second tile width.** The bar is drawn inside the
    // padding and reserves nothing extra, so a list that scrolls lays its
    // tiles out exactly like one that fits -- pinned by
    // `the_tiles_keep_one_width_whether_or_not_the_list_can_scroll`.

    // -----------------------------------------------------------------
    // Design 5d -- the `shared` pill
    // -----------------------------------------------------------------

    /// Every painted run, flattened. The harness above has a richer collector
    /// of its own; this is the one fact this pair of assertions needs.
    fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    /// **Each row is asked about its OWN record.**
    ///
    /// The paint test above proves the pill can be drawn; this proves the list
    /// decides per row rather than handing every row one answer. Read off the
    /// source, because a list that passed `true` to all of them would paint a
    /// pill on every row and look, in a screenshot, exactly like a vault in
    /// which everything really is shared.
    #[test]
    fn the_list_asks_the_shared_set_about_each_rows_own_name() {
        let source = include_str!("item_list.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        let needle = concat!("shared.contains(&item.", "name)");
        assert_eq!(
            code.matches(needle).count(),
            1,
            "the item list no longer asks {needle:?} -- every row is being given the same \
             answer, or none"
        );
    }

    /// **A person-shared record wears the `shared` pill with a person mark**,
    /// a received one the `view-only` chip (mock 6e, LIST CHIPS). The person
    /// mark wins when a record has both a person and a live Send.
    #[test]
    fn a_shared_record_wears_the_person_pill_and_a_received_one_says_view_only() {
        let view = crate::rest::sharing::SharingView {
            available: true,
            people: vec![crate::rest::sharing::PersonEntry { email: "anna@x.io".into(), state: crate::rest::sharing::PersonState::Shared, expires_at: None, authenticated: true }],
            records: vec![crate::rest::sharing::RecordEntry { item_id: "mine".into(), people: vec!["anna@x.io".into()] }],
            received: vec![crate::rest::sharing::ReceivedEntry { item_id: "theirs".into(), from: "denis@x.io".into(), organization_id: "org-x".into(), verified: true }],
            ..Default::default()
        };
        let people = RowPeople::from_view(Some(&view));
        assert!(people.shared.contains("mine"));
        assert_eq!(people.from("theirs"), Some("denis@x.io"));
        assert_eq!(people.from("mine"), None);

        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        crate::theme::apply(&ctx);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let icons = IconCache::default();
            item_row(ui, &login("theirs", "u"), &[], false, icons.textures.get("theirs"), false, FilterSource::LiveVault,
                RowMarks { send: false, person: false, from: people.from("theirs") },
                crate::kit::motion::Pose::REST,);
        });
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut texts);
        }
        assert!(texts.iter().any(|t| t == view_only_chip()), "{texts:?}");
    }

    /// Which row commands write to a record -- the ones a received,
    /// view-only record refuses (spec 6.9).
    #[test]
    fn a_view_only_record_refuses_every_command_that_writes() {
        assert!(RowCommand::Edit.writes_the_record());
        assert!(RowCommand::Delete.writes_the_record());
        assert!(RowCommand::MoveToFolder("f".into()).writes_the_record());
        assert!(RowCommand::SelectIcon.writes_the_record());
        assert!(!RowCommand::CopyPassword.writes_the_record());
        assert!(!RowCommand::RefreshIcon.writes_the_record(), "a refresh forgets a cached favicon; the record is untouched");
    }

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
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    struct Painted {
        /// Where the header's `+` is (`new_button_id`).
        new_button: Option<egui::Rect>,
        rects: Vec<RectShape>,
        /// Every stroked circle, straight run and open path the frame drew.
        ///
        /// **Added for `kind_mark`.** The four kind marks are `egui` strokes
        /// -- circles, line segments and one open path -- and this walk threw
        /// all three away, so a test asking "WHICH mark was drawn" could only
        /// have answered off the rects, and three of the four would have
        /// looked alike. That is the house's own defect class: a test that
        /// passes because it never reached the thing it names. With these
        /// collected, `note`, `card`, `identity` and `ssh_key` are told apart
        /// by the shapes they are actually made of.
        circles: Vec<egui::epaint::CircleShape>,
        segments: Vec<([egui::Pos2; 2], Stroke)>,
        paths: Vec<egui::epaint::PathShape>,
        texts: Vec<(String, egui::Rect, egui::Color32)>,
        /// Every painted run, with the y its box was painted at.
        ///
        /// `texts` keeps only the BOX, which is ascent plus descent and not
        /// what a reader sees; the alignment tests need the glyphs themselves.
        galleys: Vec<(std::sync::Arc<egui::Galley>, f32)>,
        fonts: Vec<(String, egui::FontId)>,
        visible: Vec<String>,
        /// What `draw_item_list` left `selected_id` at. Read by the
        /// right-click tests, which are about exactly that.
        selected: Option<String>,
        /// What `draw_item_list` returned on the measured frame.
        action: ItemListAction,
        /// Whether the measured frame reported a row OPENED by a primary
        /// click -- `draw_item_list`'s `row_opened` out-parameter, which is
        /// what arms the detail pane's slide. See `RowOutcome::opened`.
        row_opened: bool,
    }

    fn walk(shape: &egui::Shape, p: &mut Painted) {
        match shape {
            egui::Shape::Rect(rect) => p.rects.push(rect.clone()),
            egui::Shape::Circle(circle) => p.circles.push(*circle),
            egui::Shape::LineSegment { points, stroke } => p.segments.push((*points, *stroke)),
            egui::Shape::Path(path) => p.paths.push(path.clone()),
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
                p.galleys.push((text.galley.clone(), text.pos.y));
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
        paint_core(items, selected, wheel_frames, PANE_WIDTH, |_| IconCache::default(), Menu::none())
    }

    /// The row-menu inputs `draw_item_list` takes, and the pointer events
    /// that drive the menu open and click an entry in it.
    ///
    /// Bundled so the four harness entry points that have nothing to do with
    /// the menu can each pass one [`Menu::none`] instead of four arguments
    /// of padding.
    struct Menu {
        folders: Vec<Folder>,
        /// One `Vec` of events per extra frame to run after the first draw.
        /// Extra frames are needed at all because egui resolves a click only
        /// on the frame the button is released, and a popup only exists from
        /// the frame after the one that opened it.
        frames: Vec<Vec<egui::Event>>,
        /// **Which sidebar row the list is being drawn under**, which is a
        /// menu input and not merely a scope: `draw_item_list` hands
        /// `filter.source()` to every row, and `menu_entries` uses it to pick
        /// between three DISJOINT menus. A reviewer replaced that argument
        /// with a literal `FilterSource::LiveVault` and the whole suite
        /// stayed green while a trashed item offered Copy password, Move to
        /// folder and Delete -- every one of them a click that fails or, in
        /// the case of Delete, silently does nothing. Nothing could catch it
        /// because this harness had the same literal baked in.
        filter: SidebarFilter,
    }

    impl Menu {
        fn none() -> Self {
            Self {
                folders: Vec::new(),
                frames: Vec::new(),
                filter: SidebarFilter::All,
            }
        }
    }

    /// One real frame at an arbitrary pane width. The real pane is fixed at
    /// `LIST_WIDTH` and not resizable, so this exists to squeeze the row well
    /// past anything it can actually meet -- two chips plus an avatar plus a
    /// title is the tightest it ever gets, and what it does when it runs out
    /// of room should be a decision, not an accident.
    fn paint_at_width(items: &[VaultItem], selected: Option<&str>, width: f32) -> Painted {
        paint_core(items, selected, 0, width, |_| IconCache::default(), Menu::none())
    }

    fn paint_core(
        items: &[VaultItem],
        selected: Option<&str>,
        wheel_frames: usize,
        pane_width: f32,
        make_icons: impl FnOnce(&egui::Context) -> IconCache,
        menu: Menu,
    ) -> Painted {
        paint_core_with_marks(items, selected, wheel_frames, pane_width, make_icons, menu, None)
    }

    /// [`paint_core`], plus a directory of brand logo files to draw the
    /// network marks from.
    ///
    /// **`None` -- what every other harness here passes -- installs a policy
    /// that is switched OFF and has nowhere to look, and that is not
    /// tidiness.** With no policy installed at all, `card_mark` resolves the
    /// real `settings.json` and, if the developer running the suite has turned
    /// the preference on, their real Deskwarden `brand-marks` folders: the
    /// row tests below assert on painted WORDS, so the suite would pass or
    /// fail depending on what is in a folder on one machine. Every context
    /// here is told what to think.
    fn paint_core_with_marks(
        items: &[VaultItem],
        selected: Option<&str>,
        wheel_frames: usize,
        pane_width: f32,
        make_icons: impl FnOnce(&egui::Context) -> IconCache,
        menu: Menu,
        marks: Option<std::path::PathBuf>,
    ) -> Painted {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(pane_width, PANE_HEIGHT),
        );
        let input = || egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        // Two throwaway frames so `theme::apply`'s font set is live -- the
        // same reason `detail.rs`'s and `sidebar.rs`'s harnesses run them.
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        crate::card_mark::install_logo_policy(&ctx, crate::card_mark::LogoPolicy::off());
        let _ = ctx.run_ui(input(), |_ui| {});
        crate::card_mark::install_logo_policy(
            &ctx,
            crate::card_mark::LogoPolicy {
                enabled: marks.is_some(),
                dirs: std::sync::Arc::new(marks.into_iter().collect()),
            },
        );

        let mut selected_id = selected.map(str::to_string);
        let mut search = String::new();
        let icons = make_icons(&ctx);
        let mut action = ItemListAction::None;
        // The gesture flag, reset at the top of EVERY frame so what survives
        // to the assertion is the measured frame's own answer and not a
        // click from a set-up frame that came before it.
        let mut row_opened = false;
        let Menu { folders, mut frames, filter } = menu;
        let mut draw = |ctx: &egui::Context, input: egui::RawInput, visible: &mut Vec<String>| {
            row_opened = false;
            ctx.run_ui(input, |ui| {
                action = draw_item_list(
                    ui,
                    Some(items),
                    &folders,
                    &filter,
                    &mut search,
                    &mut selected_id,
                    &icons,
                    false,
                    visible,
                    // This harness predates the inline move-error band and
                    // has no business growing a parameter for it; the band
                    // has its own module below.
                    None,
                    // Nothing failed here, and nothing in this module's
                    // harnesses draws an empty list -- `list_placeholder` is
                    // asserted directly, by `list_placeholder_tests`.
                    false,
                    &mut row_opened,
                                &std::collections::HashSet::new(),
                                &RowPeople::default(),
                                "All items",
                                false,
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
        // The LAST of `Menu::frames` is the measured one, so a test that
        // clicks a menu entry reads back the frame that click resolved on --
        // both what was painted and what `draw_item_list` returned.
        let measured = frames.pop();
        for events in frames {
            let mut raw = input();
            raw.events = events;
            let _ = draw(&ctx, raw, &mut visible);
        }
        let output = draw(
            &ctx,
            match measured {
                Some(events) => egui::RawInput { events, ..input() },
                None => input(),
            },
            &mut visible,
        );

        let mut painted = Painted {
            new_button: ctx.read_response(new_button_id()).map(|r| r.rect),
            rects: Vec::new(),
            circles: Vec::new(),
            segments: Vec::new(),
            paths: Vec::new(),
            texts: Vec::new(),
            galleys: Vec::new(),
            fonts: Vec::new(),
            visible,
            selected: selected_id,
            action,
            row_opened,
        };
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    fn row_tiles(p: &Painted) -> Vec<RectShape> {
        tiles_of_width(p, TILE_WIDTH)
    }

    /// Every row, as the box it occupies. A v2 row paints no tile of its own
    /// until it is hovered or selected, so each is found by the one thing
    /// every row paints, its 44pt avatar, and boxed around it: the row's
    /// padding to the left and its fixed height around the avatar's centre.
    fn tiles_of_width(p: &Painted, width: f32) -> Vec<RectShape> {
        p.rects
            .iter()
            .filter(|r| {
                r.blur_width == 0.0
                    && (r.rect.width() - AVATAR_SIZE).abs() < 0.5
                    && (r.rect.height() - AVATAR_SIZE).abs() < 0.5
            })
            .map(|avatar| {
                let row = egui::Rect::from_min_size(
                    egui::pos2(avatar.rect.left() - ROW_PAD_X, avatar.rect.center().y - ROW_TILE_HEIGHT / 2.0),
                    egui::vec2(width, ROW_TILE_HEIGHT),
                );
                RectShape::filled(row, CornerRadius::same(ROW_RADIUS), avatar.fill)
            })
            .collect()
    }

    fn one_tile(p: &Painted) -> RectShape {
        one_tile_of_width(p, TILE_WIDTH)
    }

    fn one_tile_of_width(p: &Painted, width: f32) -> RectShape {
        let tiles = tiles_of_width(p, width);
        assert_eq!(
            tiles.len(),
            1,
            "expected exactly one {width}x{ROW_TILE_HEIGHT} row tile; every painted rect was: {:?}",
            p.rects.iter().map(|r| (r.rect, r.fill)).collect::<Vec<_>>()
        );
        tiles[0].clone()
    }

    /// Design 2b's header strip: `padding: 12px` around a `height: 34px`
    /// search box, i.e. 12 + 34 + 12. Written out rather than derived from
    /// `theme::SEARCH_FIELD_HEIGHT` so this stays an INDEPENDENT statement of
    /// the geometry -- a test that recomputed the strip from the same
    /// constants the code uses would stay green if both moved together.
    const STRIP_HEIGHT: f32 = 72.0;

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
            (tile.rect.top() - (STRIP_HEIGHT + LIST_PADDING_TOP)).abs() < 0.5,
            "the first row tile's top edge is at y={}, expected {} (the {STRIP_HEIGHT}pt header \
             -- 24 + 34 + 14 -- plus the row list's own {LIST_PADDING_TOP}pt top padding and \
             NOTHING else: egui's ambient item_spacing is sitting between the two)",
            tile.rect.top(),
            STRIP_HEIGHT + LIST_PADDING_TOP
        );
        assert!(
            (tile.rect.left() - LIST_PADDING).abs() < 0.5,
            "the first row tile's left edge is at x={}, expected {LIST_PADDING}",
            tile.rect.left()
        );
    }

    #[test]
    fn the_scrollbar_sits_at_the_outer_edge_of_the_lists_padding_and_the_tiles_keep_their_width() {
        // THE REPORT this began as: "the scrollbar sits against the tiles'
        // right edge; put it in the right padding". That was first answered
        // by CENTRING the bar in the padding, then by giving it a lane of its
        // OWN beyond the padding -- and the latest report rejects that lane:
        // "scroll should be included in those 10pt and window should not
        // shrink\expand if more or less items".
        //
        // So the gutter is the list's `padding: 10px` and NOTHING more, i.e.
        // x in [380, 390] at this pane, and the 6pt bar sits flush to that
        // gutter's OUTER edge, at [384, 390]. ABSOLUTE numbers, not "the bar
        // is right of the tiles", and both slacks pinned to a number each
        // rather than only to each other: 4pt on the tile side, 0 behind the
        // bar. Every point of leftover belongs on the side the reader
        // compares to the left gap.
        //
        // The tiles run 10..380 in BOTH states -- see
        // `the_tiles_keep_one_width_whether_or_not_the_list_can_scroll`.
        const GUTTER: std::ops::Range<f32> = (PANE_WIDTH - LIST_PADDING)..PANE_WIDTH;
        let items: Vec<VaultItem> = (0..40)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        // SETTLED frames, not the first one. egui fades a floating bar in
        // over several frames, so on frame 1 the track and the handle are
        // both emitted at alpha 0 -- an earlier version of this test read
        // that frame and so asserted the placement of a bar that was not yet
        // being drawn at all. `SETTLE_FRAMES` also parks the pointer over the
        // list, which is what a floating bar fades in for.
        let p = paint_with(&items, None, SETTLE_FRAMES);

        // NOT a bare `for` over the filter's output: `row_tiles` keys on a
        // width, and a width that no longer matches finds ZERO tiles and
        // walks the loop below zero times, green. So the count is pinned
        // first, against the rows egui reported visible on this same frame.
        let tiles = row_tiles(&p);
        assert!(
            !tiles.is_empty() && tiles.len() + 1 >= p.visible.len(),
            "the tile filter found {} rects on a frame that reported {} visible rows -- a filter \
             that matches nothing makes every assertion below vacuous. One fewer is allowed: the \
             row at the viewport's edge can be clipped out of the paint list entirely.",
            tiles.len(),
            p.visible.len()
        );
        for tile in tiles {
            assert!(
                (tile.rect.left() - LIST_PADDING).abs() < 0.5
                    && (tile.rect.right() - GUTTER.start).abs() < 0.5,
                "a row tile spans {}..{}, expected {LIST_PADDING}..{} -- a list that scrolls \
                 must lay its tiles out exactly like one that fits; the bar is not allowed to \
                 take width from them",
                tile.rect.left(),
                tile.rect.right(),
                GUTTER.start
            );
        }

        // The scroll bar's own two rects (track and handle) are the only
        // things painted in the gutter at all. Found by geometry -- they are
        // the rects that lie strictly right of the tiles -- and required to
        // be VISIBLE, because an overflowing list must actually show the user
        // where it is: `visibly_painted` rejects the alpha-0 shapes egui
        // still emits for a bar it is not drawing.
        let in_gutter: Vec<egui::Rect> = gutter_marks(&p);
        assert!(
            !in_gutter.is_empty(),
            "nothing VISIBLE was painted in the list's right padding, so a list of {} rows on a \
             {PANE_HEIGHT}pt pane is not showing the user that it can scroll; painted: {:?}",
            items.len(),
            p.rects.iter().map(|r| (r.rect, r.fill)).collect::<Vec<_>>()
        );
        for bar in &in_gutter {
            assert!(
                bar.left() >= GUTTER.start - 0.01 && bar.right() <= GUTTER.end + 0.01,
                "the scrollbar spans x={}..{}, which leaves the {GUTTER:?} gutter -- it is being \
                 drawn over the tiles, or outside the padding it is meant to live in",
                bar.left(),
                bar.right()
            );
            let slack_left = bar.left() - GUTTER.start;
            let slack_right = GUTTER.end - bar.right();
            assert!(
                (slack_left - (LIST_PADDING - theme::SCROLLBAR_WIDTH)).abs() < 0.51
                    && slack_right.abs() < 0.51,
                "the scrollbar has {slack_left}pt of gutter to its left and {slack_right}pt to \
                 its right, expected {}pt and 0pt -- what is left of the padding once the bar \
                 is in it belongs on the TILE side, where the reader compares it to the left \
                 gap, and none of it between the bar and the pane's own outer edge",
                LIST_PADDING - theme::SCROLLBAR_WIDTH
            );
        }
    }

    #[test]
    fn the_scrollbar_can_be_grabbed_and_dragged_where_it_is_drawn() {
        // **A bar nobody can grab is a decoration.** Placement is set by
        // spacing numbers, and one combination that PAINTS the bar in exactly
        // the right place -- `floating_allocated_width = 0` plus a negative
        // `bar_outer_margin`, which was measured against this test -- leaves
        // egui deriving the bar's hit rect from an outer rect that stops at
        // the content's edge, so the rect comes out inverted and the bar is
        // inert. Every geometry assertion in this module passed under it.
        //
        // So: press on the bar where it is DRAWN, drag down, release, and
        // require that the list actually moved. Driven through real pointer
        // events rather than by poking `ScrollArea`'s state.
        let items: Vec<VaultItem> = (0..40)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let unscrolled = paint(&items, None);
        let first_before = unscrolled.visible.first().cloned();

        // The middle of the bar's own 6pt column, taken from the geometry the
        // placement test pins, not from a guess.
        let on_bar = egui::pos2(PANE_WIDTH - theme::SCROLLBAR_WIDTH / 2.0, 300.0);
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut frames: Vec<Vec<egui::Event>> = vec![
            vec![egui::Event::PointerMoved(on_bar)],
            vec![egui::Event::PointerMoved(on_bar), button(on_bar, true)],
        ];
        // Dragged in steps: egui reads a drag from pointer MOVEMENT between
        // frames, so one jump would be a single delta and a poor imitation of
        // a real hand.
        for y in [340.0f32, 400.0, 460.0, 520.0] {
            frames.push(vec![egui::Event::PointerMoved(egui::pos2(on_bar.x, y))]);
        }
        let end = egui::pos2(on_bar.x, 520.0);
        frames.push(vec![egui::Event::PointerMoved(end), button(end, false)]);
        frames.push(vec![egui::Event::PointerMoved(end)]);
        let dragged = paint_core(
            &items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu {
                folders: Vec::new(),
                frames,
                filter: SidebarFilter::All,
            },
        );

        assert!(
            !dragged.visible.is_empty() && dragged.visible.first() != first_before.as_ref(),
            "dragging the scroll bar from y=300 to y=520 left the list showing {:?} first, the \
             same row it started on. The bar is painted where the reader can see it but egui is \
             not sensing it there -- its hit rect and its paint rect have come apart",
            dragged.visible.first()
        );
    }
    /// Frames to run before measuring anything about the scroll bar.
    ///
    /// Long enough for egui's floating-bar fade to finish, and it parks the
    /// pointer over the list, which is the state the bar is most visible in
    /// -- so a test that expects NOTHING there is being asked the hardest
    /// version of the question.
    const SETTLE_FRAMES: usize = 20;

    /// The painted rects that a user can actually SEE: a rect whose fill and
    /// whose stroke are both fully transparent occupies space in the shape
    /// list and none on screen. egui emits exactly such rects for a floating
    /// scroll bar it is holding at opacity 0, so geometry alone cannot tell
    /// "the bar is centred in the gutter" from "there is no bar".
    fn visibly_painted(p: &Painted) -> impl Iterator<Item = egui::Rect> + '_ {
        p.rects
            .iter()
            .filter(|r| {
                r.fill.a() > 0 || (r.stroke.width > 0.0 && r.stroke.color.a() > 0)
            })
            .map(|r| r.rect)
    }

    #[test]
    fn a_list_that_fits_leaves_the_same_clear_space_on_both_sides_of_its_tiles() {
        // THE REPORT: a three-item vault, nothing to scroll, and "the right
        // padding feels smaller".
        //
        // It was not smaller. Both gaps measured exactly `LIST_PADDING`, and
        // the older geometry test above said so and passed. What the reader
        // was seeing is that `AlwaysVisible` -- which the reserved gutter
        // needs, so the tiles do not resize when the bar comes and goes --
        // painted a 6pt bar down the FULL height of a list that could not
        // move, leaving 2pt of clear space on the right against 10pt on the
        // left.
        //
        // So this asserts what the eye measures, not what the layout says:
        // the clear space beside the tiles, with anything visible painted in
        // it counted against that side. A test that only compared tile edges
        // is what let the complaint through.
        let items: Vec<VaultItem> = (0..3)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let p = paint_with(&items, None, SETTLE_FRAMES);

        let tiles = row_tiles(&p);
        assert_eq!(tiles.len(), items.len(), "every row should have drawn a tile");
        let tile = tiles[0].rect;
        assert!(
            (tile.left() - LIST_PADDING).abs() < 0.5
                && (tile.right() - (PANE_WIDTH - LIST_PADDING)).abs() < 0.5,
            "a row tile spans {}..{}, expected {LIST_PADDING}..{} -- a list that FITS must lay \
             its tiles out exactly like one that overflows, or they resize as items are added",
            tile.left(),
            tile.right(),
            PANE_WIDTH - LIST_PADDING
        );

        // Strictly outside the tile on one side or the other -- see
        // `clear_space_beside_tiles`, which the scrolling list's floor test
        // measures with too, so the two cases cannot disagree about what
        // "clear space" means.
        let (clear_left, clear_right) = clear_space_beside_tiles(&p, tile);
        assert!(
            (clear_left - clear_right).abs() < 0.51,
            "the tiles have {clear_left}pt of clear space to their left and {clear_right}pt to \
             their right. The tile EDGES are symmetric ({}..{}); something is being painted in \
             one of the gutters of a list that cannot scroll. Visible rects: {:?}",
            tile.left(),
            tile.right(),
            visibly_painted(&p).collect::<Vec<_>>()
        );
        assert!(
            (clear_right - LIST_PADDING).abs() < 0.51,
            "the clear space beside the tiles is {clear_right}pt, not the design's \
             {LIST_PADDING}pt -- both gutters are occupied"
        );
    }

    /// The clear space beside the tiles, with anything VISIBLE painted in a
    /// gutter counted against that side -- the eye's measurement, not the
    /// layout's. Returns `(left, right)`.
    ///
    /// Shared with the fitting list's test below so both cases ask the
    /// question the SAME way; a second, subtly different measurement in the
    /// scrolling case could have made the two agree by accident.
    fn clear_space_beside_tiles(p: &Painted, tile: egui::Rect) -> (f32, f32) {
        let mut clear_left = tile.left();
        let mut clear_right = PANE_WIDTH - tile.right();
        for r in visibly_painted(p) {
            if r.right() <= tile.left() + 0.01 {
                clear_left = clear_left.min(tile.left() - r.right());
            }
            if r.left() >= tile.right() - 0.01 {
                clear_right = clear_right.min(r.left() - tile.right());
            }
        }
        (clear_left, clear_right)
    }

    #[test]
    fn the_clear_space_beside_a_scrolling_lists_tiles_is_at_its_floor() {
        // **What the bar being INSIDE the padding costs, stated out loud.**
        //
        // The gutter is `LIST_PADDING` on both sides (the placement test pins
        // that) and the tiles never move, so while the bar is showing the
        // clear space between the tiles and the first ink on their right is
        // `LIST_PADDING - SCROLLBAR_WIDTH` = 4pt, against 10 on the left.
        //
        // That is a FLOOR, not an oversight, and the two ways past it are
        // both closed by the report: widen the gutter for the bar to sit
        // beyond ("you made it huge on the right"), or narrow the tiles to
        // pay for it ("window should not shrink\expand if more or less
        // items"). With both sides of the budget fixed, the same strip of
        // pane has to be clear when no bar is showing and ink when one is.
        // `bar_outer_margin = 0` puts every point of the leftover on the tile
        // side, which is the most of it that can reach the gap the reader is
        // actually comparing.
        //
        // The state a still screenshot catches is the DORMANT one, where the
        // bar has faded and both gaps read a full 10 -- see
        // `a_scrolling_lists_bar_fades_when_the_pointer_leaves_the_list`.
        let items: Vec<VaultItem> = (0..40)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let p = paint_with(&items, None, SETTLE_FRAMES);

        // The bar must actually be on screen, or the clear space below is
        // just the fitting list's and this test says nothing. `gutter_marks`
        // rejects the alpha-0 rects egui still emits for a bar it is holding
        // invisible -- the trap the placement test above once fell into.
        let marks = gutter_marks(&p);
        assert!(
            !marks.is_empty(),
            "a {}-row list on a {PANE_HEIGHT}pt pane is painting nothing visible in its \
             gutter, so this is not the bar-showing case at all",
            items.len()
        );

        let tile = row_tiles(&p)[0].rect;
        let (clear_left, clear_right) = clear_space_beside_tiles(&p, tile);
        assert!(
            (clear_left - LIST_PADDING).abs() < 0.51,
            "the tiles have {clear_left}pt of clear space to their left, not the design's \
             {LIST_PADDING}pt -- something is being painted in the LEFT gutter"
        );
        assert!(
            (clear_right - (LIST_PADDING - theme::SCROLLBAR_WIDTH)).abs() < 0.51,
            "with the bar showing the tiles have {clear_right}pt of clear space to their \
             right, expected exactly {}pt. MORE than that means the bar has drifted off the \
             pane's outer edge and is wasting the gutter behind itself; LESS means it has \
             grown or moved inward over the tiles. Visible rects: {:?}",
            LIST_PADDING - theme::SCROLLBAR_WIDTH,
            visibly_painted(&p).collect::<Vec<_>>()
        );

        // **The positive control.** The number above is cheap on its own: a
        // measurement that never saw the bar would report `LIST_PADDING`, and
        // a bar drawn anywhere in the gutter would report SOMETHING. So pin
        // that the tiles stop a full `LIST_PADDING` short of the pane edge --
        // i.e. that the 4pt is the bar eating into an unchanged gutter, and
        // not a gutter that has quietly been resized.
        let to_pane_edge = PANE_WIDTH - tile.right();
        assert!(
            (to_pane_edge - LIST_PADDING).abs() < 0.51,
            "the tiles end {to_pane_edge}pt short of the pane's right edge, expected \
             {LIST_PADDING}pt -- the gutter itself has changed size, so the clear space above \
             is not measuring what it claims to"
        );
    }

    #[test]
    fn the_tiles_keep_one_width_whether_or_not_the_list_can_scroll() {
        // **THE INVERSION.** This test used to assert that the tiles narrow
        // by `SCROLLBAR_WIDTH` the moment a list can scroll -- the price of
        // giving the bar a lane outside the padding. The report rejects that
        // price in as many words: "window should not shrink\expand if more
        // or less items". So it now asserts the opposite, and it is the same
        // test in the sense that matters: it is the one place the tile width
        // is compared ACROSS the overflow boundary, which is the only place
        // the wobble could ever have shown up.
        //
        // A single fixture would not have caught it -- the width is
        // self-consistent within either state. The boundary is FOUND from
        // painted output, not asserted, so the two fixtures really are one
        // row either side of it.
        let overflow_at = (1..=40)
            .find(|n| row_stack_overflows(*n))
            .expect("no list of up to 40 rows overflowed the pane");
        let fitting: Vec<VaultItem> = (0..overflow_at - 1)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let overflowing: Vec<VaultItem> = (0..overflow_at)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let fits_tile = row_tiles(&paint_with(&fitting, None, SETTLE_FRAMES))[0].rect;
        let scrolls_tile = row_tiles(&paint_with(&overflowing, None, SETTLE_FRAMES))[0].rect;

        assert!(
            (fits_tile.left() - scrolls_tile.left()).abs() < 0.5
                && (fits_tile.right() - scrolls_tile.right()).abs() < 0.5,
            "a {}-row list lays its tiles out {}..{} and a {}-row one -- ONE row more, the \
             first that cannot fit -- lays them out {}..{}. Adding an item must not resize \
             them: the bar has to live inside the padding, not take width from the content",
            fitting.len(),
            fits_tile.left(),
            fits_tile.right(),
            overflowing.len(),
            scrolls_tile.left(),
            scrolls_tile.right()
        );
        for (tile, n) in [(fits_tile, fitting.len()), (scrolls_tile, overflowing.len())] {
            assert!(
                (tile.width() - TILE_WIDTH).abs() < 0.5,
                "a {n}-row list lays its tiles out {}pt wide, expected {TILE_WIDTH} -- the \
                 pane minus the design's {LIST_PADDING}pt padding on each side and nothing \
                 else",
                tile.width()
            );
        }
    }

    #[test]
    fn a_scrolling_lists_bar_fades_when_the_pointer_leaves_the_list() {
        // **THE OTHER INVERSION.** This test used to require the bar to stay
        // painted with the pointer away. That was forced: the bar had a lane
        // of its own that the TILES had paid `SCROLLBAR_WIDTH` for, so a
        // faded bar left paid-for pane standing empty and the right-hand gap
        // flicked between 10pt and 16pt with the mouse.
        //
        // The bar reserves nothing now, so the fade costs no layout at all
        // and egui's own floating behaviour is left alone -- a bar that
        // appears when you reach for the list and gets out of the way when
        // you do not is what every platform does. The reading it gives is the
        // best one available for a still screenshot of an idle window: a full
        // `LIST_PADDING` of clear space on BOTH sides.
        //
        // `paint` runs frames with NO pointer at all, which is the dormant
        // state; `paint_with(.., SETTLE_FRAMES)` parks the pointer over the
        // list. Both are asserted here so the two states cannot drift apart.
        let items: Vec<VaultItem> = (0..40)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();

        let dormant = paint(&items, None);
        assert!(
            gutter_marks(&dormant).is_empty(),
            "a {}-row list is painting something in its gutter with the pointer away from it. \
             The bar is meant to fade: it reserves no width, so leaving it up only spends the \
             gutter of an idle window for nothing. Visible rects: {:?}",
            items.len(),
            visibly_painted(&dormant).collect::<Vec<_>>()
        );
        let tile = row_tiles(&dormant)[0].rect;
        let (clear_left, clear_right) = clear_space_beside_tiles(&dormant, tile);
        assert!(
            (clear_left - LIST_PADDING).abs() < 0.51
                && (clear_right - LIST_PADDING).abs() < 0.51,
            "an idle {}-row list shows {clear_left}pt of clear space left of its tiles and \
             {clear_right}pt right of them; both should be the design's {LIST_PADDING}pt",
            items.len()
        );

        // **The positive control, and the point of the fade.** With the
        // pointer over the list the bar IS painted -- otherwise this test
        // would pass just as well on a list that never draws a bar at all,
        // which is the defect `the_scrollbar_appears_on_the_very_first_list_
        // that_does_not_fit` exists to catch.
        let hovered = paint_with(&items, None, SETTLE_FRAMES);
        assert!(
            !gutter_marks(&hovered).is_empty(),
            "a {}-row list paints nothing visible in its gutter even with the pointer over it, \
             so it is never showing the user that it can scroll. Visible rects: {:?}",
            items.len(),
            visibly_painted(&hovered).collect::<Vec<_>>()
        );
        // And the tiles are in the SAME place in both -- the fade is paint
        // only, which is what makes it free.
        let hovered_tile = row_tiles(&hovered)[0].rect;
        assert!(
            (hovered_tile.left() - tile.left()).abs() < 0.5
                && (hovered_tile.right() - tile.right()).abs() < 0.5,
            "the tiles span {}..{} with the pointer away and {}..{} with it over the list -- \
             the bar appearing must not move them",
            tile.left(),
            tile.right(),
            hovered_tile.left(),
            hovered_tile.right()
        );
    }

    /// The bottom of the list's scrolling viewport, from the PANE's geometry
    /// alone: the list frame is flush to the bottom of the pane and carries
    /// `LIST_PADDING` of its own. Deliberately says nothing about rows, tile
    /// heights or gaps -- it is the fixed edge the row stack is measured
    /// AGAINST, and the whole point of the test below is that the two are
    /// arrived at independently.
    const VIEWPORT_BOTTOM: f32 = PANE_HEIGHT - LIST_PADDING;

    /// Anything VISIBLE painted in the list's reserved right-hand gutter,
    /// i.e. the scroll bar's track and handle -- the same "strictly right of
    /// the tiles, narrower than the gutter" test the centring and clear-space
    /// tests use, so all three agree on what "the bar is on screen" means.
    fn gutter_marks(p: &Painted) -> Vec<egui::Rect> {
        visibly_painted(p)
            .filter(|r| r.right() > PANE_WIDTH - LIST_PADDING + 0.5 && r.width() < LIST_PADDING)
            .collect()
    }

    /// Does a list of `n` rows actually OVERFLOW its viewport? Answered by
    /// MEASURING the rows egui painted -- not by any arithmetic on
    /// `ROW_TILE_HEIGHT` and `ROW_GAP`.
    ///
    /// Two independent signs, either of which means the stack did not fit:
    /// the last tile egui laid out reaches past [`VIEWPORT_BOTTOM`], or
    /// `show_rows` virtualized some rows away and painted fewer tiles than
    /// there are items. On an unscrolled frame, so the row stack is measured
    /// where it starts.
    fn row_stack_overflows(n: usize) -> bool {
        let items: Vec<VaultItem> = (0..n)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let p = paint(&items, None);
        let tiles = row_tiles(&p);
        tiles.len() < n
            || tiles.last().is_some_and(|t| t.rect.bottom() > VIEWPORT_BOTTOM + 0.01)
    }

    #[test]
    fn the_scrollbar_appears_on_the_very_first_list_that_does_not_fit() {
        // THE DEFECT this pins: `draw_item_list` PREDICTS overflow from the
        // row count in order to hide the bar on a list that fits, and that
        // prediction has to include the gap BETWEEN rows. Drop the gap term
        // and the prediction under-measures the stack by `(n-1) * ROW_GAP`,
        // so at the boundary the bar is hidden on a list that really does
        // overflow -- exactly what the code's own "ties go to SHOWING the
        // bar" comment says must not happen. The suite tested 3 items and 40
        // and nothing in between, so the boundary itself was never exercised.
        //
        // The boundary is FOUND, not asserted: `row_stack_overflows` reads
        // back the tiles egui actually painted and compares them to the
        // pane's own bottom edge. Recomputing `n * ROW_TILE_HEIGHT +
        // (n-1) * ROW_GAP` here would have been the same sum the production
        // line computes, and a test that agrees with the code by construction
        // cannot disagree with it when it is wrong.
        let overflow_at = (1..=40)
            .find(|n| row_stack_overflows(*n))
            .expect("no list of up to 40 rows overflowed a {PANE_HEIGHT}pt pane");
        let fits_at = overflow_at - 1;
        assert!(
            fits_at > 3 && overflow_at < 40,
            "the boundary is at {fits_at}/{overflow_at} rows, which is no longer strictly \
             between the 3-row and 40-row lists the other scroll tests use -- this test is \
             meant to cover the case NEITHER of those does"
        );

        // The list one row short of overflowing shows no bar at all, even
        // with the pointer parked over it for `SETTLE_FRAMES`.
        let fitting: Vec<VaultItem> = (0..fits_at)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let marks = gutter_marks(&paint_with(&fitting, None, SETTLE_FRAMES));
        assert!(
            marks.is_empty(),
            "a {fits_at}-row list whose tiles all fit above y={VIEWPORT_BOTTOM} is still \
             painting {marks:?} in its right-hand gutter"
        );

        // Add ONE row -- the first list that does not fit -- and the bar must
        // be back. This is the assertion the zeroed gap term fails: it
        // predicts {overflow_at} rows fit, and hides the bar on a list the
        // measurement above just showed overflowing.
        let overflowing: Vec<VaultItem> = (0..overflow_at)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let marks = gutter_marks(&paint_with(&overflowing, None, SETTLE_FRAMES));
        assert!(
            !marks.is_empty(),
            "adding one row to {fits_at} pushes the tiles past y={VIEWPORT_BOTTOM}, so the list \
             CAN scroll -- and nothing visible is painted in its gutter, so the user is not \
             told. The overflow prediction is under-measuring the row stack"
        );
        // ...and it is egui's bar for THIS viewport, not some stray mark that
        // happens to land in the gutter: the track runs the viewport's own
        // height. A second, independent confirmation that the pane geometry
        // the boundary was measured against is the one the bar belongs to.
        assert!(
            marks.iter().any(|r| (r.bottom() - VIEWPORT_BOTTOM).abs() < 0.5),
            "the gutter marks {marks:?} do not reach the viewport's bottom edge at \
             y={VIEWPORT_BOTTOM}, so they are not the scroll bar's track"
        );
    }


    /// A card carrying a stored `brand`, the way every Bitwarden client
    /// writes it.
    fn card_branded(name: &str, brand: &str) -> VaultItem {
        let mut it = card(name);
        it.card = Some(crate::vault_bridge::CardData {
            brand: Some(brand.to_string()),
            ..Default::default()
        });
        it
    }




    // ---- brand logos on the row ----------------------------------------

    fn with_site(mut item: VaultItem, uri: &str) -> VaultItem {
        item.login.get_or_insert_with(Default::default).uris =
            vec![crate::vault_bridge::UriEntry { uri: Some(uri.to_string()), other: serde_json::Map::new() }];
        item
    }

    #[test]
    fn a_mir_number_badges_as_mir_but_a_stored_brand_still_wins() {
        assert_eq!(card_net(&card_numbered("A", "2200 1234 5678 9012")), Some(Net::Mir));
        // "Other" says "none of the ten", which Mir is not.
        let mut other = card_numbered("A", "2200123456789012");
        other.card.as_mut().unwrap().brand = Some("Other".to_string());
        assert_eq!(card_net(&other), Some(Net::Mir));
        let mut other = card_numbered("A", "4111111111111111");
        other.card.as_mut().unwrap().brand = Some("Other".to_string());
        assert_eq!(card_net(&other), None);
        let mut visa = card_numbered("A", "2200123456789012");
        visa.card.as_mut().unwrap().brand = Some("Visa".to_string());
        assert_eq!(card_net(&visa), Some(Net::Visa));
        assert_eq!(card_net(&card_branded("A", "Amex")), Some(Net::Amex));
        assert_eq!(card_net(&login("L", "a@b.c")), None);
    }

    #[test]
    fn a_bank_is_read_from_a_cards_bank_domain_or_a_logins_website() {
        let card = crate::vault_bridge::with_custom_field(&card_branded("Corp", "Visa"), crate::favicon::BANK_DOMAIN_FIELD, "chase.com");
        assert_eq!(item_bank(&card), Some(Bank::Chase));
        assert_eq!(item_bank(&with_site(login("Chase Business", "me"), "https://secure.chase.com/login")), Some(Bank::Chase));
        assert_eq!(item_bank(&with_site(login("Other", "me"), "https://example.com")), None);
        assert_eq!(item_bank(&card_branded("Bare", "Visa")), None);
    }

    #[test]
    fn the_row_look_puts_the_bank_first_with_the_network_as_a_chip() {
        use crate::kit::avatar::Look;
        let card = crate::vault_bridge::with_custom_field(&card_branded("Corp", "Visa"), crate::favicon::BANK_DOMAIN_FIELD, "chase.com");
        match row_look(&card, None) {
            Look::Brand { mark, chip } => assert_eq!((mark, chip), (Mark::Bank(Bank::Chase), Some(Net::Visa))),
            _ => panic!("a bank tile"),
        }
        match row_look(&card_branded("Plain", "Mastercard"), None) {
            Look::Brand { mark, chip } => assert_eq!((mark, chip), (Mark::Net(Net::Mastercard), None)),
            _ => panic!("a network tile"),
        }
        match row_look(&with_site(login("Revolut", "me"), "https://www.revolut.com"), None) {
            Look::Brand { mark, chip } => assert_eq!((mark, chip), (Mark::Bank(Bank::Revolut), None)),
            _ => panic!("a bank tile"),
        }
        assert!(matches!(row_look(&login("Ledgerline", "me"), None), Look::Initials { .. }));
    }

    #[test]
    fn the_network_is_read_from_the_brand_first_and_the_digits_only_after() {
        // The pure decision behind all four tests above, without a frame.
        assert_eq!(card_network(&card_branded("A", "visa")), Some(CardBrand::Visa));
        assert_eq!(card_network(&card_branded("A", "VISA")), Some(CardBrand::Visa));
        // A card written by a client that stores no brand still reads as what
        // it is -- the fallback, and the reason this is not just a field read.
        assert_eq!(
            card_network(&card_numbered("A", "5555555555554444")),
            Some(CardBrand::Mastercard)
        );
        // The brand WINS over the digits, in the direction that matters: a
        // hand-picked Mastercard on a number beginning with 4 stays a
        // Mastercard.
        let mut picked = card_numbered("A", "4111111111111111");
        picked.card.as_mut().unwrap().brand = Some("Mastercard".to_string());
        assert_eq!(card_network(&picked), Some(CardBrand::Mastercard));
        // Blank is not a brand: it falls through to the digits rather than
        // suppressing the badge.
        let mut blank = card_numbered("A", "4111111111111111");
        blank.card.as_mut().unwrap().brand = Some("   ".to_string());
        assert_eq!(card_network(&blank), Some(CardBrand::Visa));
        // And nothing that is not a card ever wears a badge.
        assert_eq!(card_network(&login("Ledgerline", "a@b.c")), None);
        assert_eq!(card_network(&card("Bare")), None);
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

    /// **The same claim at every width the detail pane's slide sweeps the
    /// list through**, which is what makes the slide a REFLOW that a real
    /// vault can afford.
    ///
    /// The list genuinely shrinks as the pane comes in -- it is not a pane
    /// drawn over the top of it -- so every frame of the animation re-lays the
    /// list at a new width. The owner's reasoning for accepting that was "you
    /// usually see only a dozen on the screen", and this is that assumption
    /// stated as an assertion rather than trusted: the row COUNT is a function
    /// of the pane's HEIGHT, which the slide does not touch, so a 1656-item
    /// vault lays out the same handful of rows at 390pt and at 1028pt.
    ///
    /// Without this, "the reflow is cheap" rests on `show_rows` staying in the
    /// list, and a future full-width branch that bypassed it would be an O(N)
    /// layout on every frame of an animation.
    #[test]
    fn the_list_stays_virtualized_at_every_width_the_slide_sweeps_it_through() {
        let items: Vec<VaultItem> = (0..1656)
            .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
            .collect();
        let ceiling = (PANE_HEIGHT / (ROW_TILE_HEIGHT + ROW_GAP)).ceil() as usize + 4;
        // The list's resting width, the full-width state it reaches with no
        // detail pane, and a width in between -- the animation passes through
        // all three.
        for width in [PANE_WIDTH, 700.0, 1028.0] {
            let p = paint_at_width(&items, None, width);
            assert!(
                p.visible.len() <= ceiling,
                "at a {width}pt list, {} of 1656 rows were laid out; at most {ceiling} fit \
                 the {PANE_HEIGHT}pt pane. The row count must follow the pane's HEIGHT, \
                 which the slide does not change -- if it follows the width, every frame \
                 of the animation is an O(N) layout",
                p.visible.len()
            );
            assert!(
                !p.visible.is_empty(),
                "at a {width}pt list nothing was laid out at all, so this width proves \
                 nothing"
            );
        }
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

    // ---- the rows' right-click menu -------------------------------------
    //
    // What `menu_entries` decides is pinned directly, exhaustively, in
    // `menu_entry_tests`. These are the OTHER half, and the half this
    // repository keeps getting wrong: that the draw code actually obeys it.
    // A correct decision inside a closure nothing calls is this file's
    // most-repeated defect shape, and an egui context menu renders into its
    // own popup layer, which is exactly the kind of place such a closure
    // hides.
    //
    // They drive real pointer events -- a secondary press and release over a
    // row tile whose position is read back from the previous frame's painted
    // output -- and then read the galleys the popup painted.

    /// Every label the row menu can paint. The painted assertions below
    /// intersect what was drawn with this vocabulary, so they are ABSOLUTE
    /// ("exactly these entries") rather than "the ones I looked for" -- a
    /// menu that also offered "Copy password" on a card would fail them.
    /// Every label a row menu can paint, live or out-of-vault.
    ///
    /// **This is an assertion input, not a message helper**: `menu_labels`
    /// filters painted galleys through it, so the tests that assert a menu's
    /// entries "exactly" are blind to any label missing from here. It was
    /// missing "Archive" -- so
    /// `a_right_click_on_a_login_paints_exactly_that_login_s_entries` claimed
    /// exactness over a list with a real entry filtered out of both sides,
    /// and would not have noticed that entry disappearing. Adding the four
    /// out-of-vault labels is what lets those same tests state that a LIVE
    /// row offers no Restore or Unarchive, rather than being unable to see
    /// one if it did.
    /// The count went 14 -> 16 when "Select icon..." and "Use the automatic
    /// icon" arrived. Both had to be added here and not merely to the
    /// expected lists: a label missing from this array is one the painted
    /// assertions cannot see, so an entry that stopped being drawn would have
    /// gone unnoticed on both sides -- which is the exact defect the
    /// "Archive" note above records.
    ///
    /// Then 16 -> 14 when the two deletes stopped confirming inside the menu:
    /// the armed wordings ("Delete? Click to confirm", "Delete forever?
    /// Click to confirm") are not labels this menu can draw any more, so
    /// leaving them here would be two entries the painted assertions look
    /// for and never find -- vocabulary that reads as coverage and is not.
    /// The tests that watched for that wording now look for the substring
    /// directly, because its absence is the thing being asserted.
    ///
    /// **And 14 -> 15 when the icon group folded into an `ICON_LABEL`
    /// submenu.** ONE label added and none removed, which is the whole shape
    /// of that change: the three icon wordings are still labels this menu can
    /// paint -- they moved a level down, not away -- so dropping them here
    /// would have blinded every assertion that says a menu offers *no*
    /// refresh, and those are exactly the assertions the icon rules are made
    /// of. What arrived is the group's own row, and it had to arrive here and
    /// not merely in the expected lists: a label missing from this array is
    /// one the painted assertions cannot see, so an `ICON_LABEL` that stopped
    /// being drawn would have gone unnoticed on both sides -- which is the
    /// exact defect the "Archive" note above records.
    ///
    /// A submenu's rows are painted only while it is open, so a menu opened
    /// and not walked into paints the group's row and none of its three. That
    /// is what makes the per-item icon assertions below open the submenu
    /// (see `open_icon_submenu`) rather than reading the outer menu's galleys.
    fn menu_vocabulary() -> [&'static str; 15] {
    [
        "Copy username",
        "Copy password",
        "Copy TOTP",
        "Open website",
        icon_label(),
        refresh_icon_label(),
        select_icon_label(),
        clear_icon_label(),
        "Edit",
        move_to_folder_label(),
        "Archive",
        delete_label(),
        "Restore",
        "Unarchive",
        purge_label(),
    ]
    }

    fn menu_labels(p: &Painted) -> Vec<String> {
        p.texts
            .iter()
            .map(|(text, _, _)| text.clone())
            .filter(|text| menu_vocabulary().contains(&text.as_str()))
            .collect()
    }

    fn click_frames(at: egui::Pos2, button: egui::PointerButton) -> Vec<Vec<egui::Event>> {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        vec![
            vec![egui::Event::PointerMoved(at), press(true)],
            // egui resolves a click on the frame the button is RELEASED.
            vec![press(false)],
            // One settled frame, so the popup that release opened is
            // painted with the pointer resting where it was left.
            vec![egui::Event::PointerMoved(at)],
        ]
    }

    /// The centre of row `row`'s tile, read from a first painted frame
    /// rather than re-derived from the row-height constants -- a test that
    /// computed the position itself could aim at the wrong place in exactly
    /// the case (a row whose height changed) it exists to catch.
    fn row_centre(items: &[VaultItem], row: usize) -> egui::Pos2 {
        let tiles = row_tiles(&paint(items, None));
        assert!(row < tiles.len(), "row {row} was never painted");
        tiles[row].rect.center()
    }

    /// Right-clicks row `row` under the ordinary live vault and returns the
    /// frame the menu is open on.
    fn open_menu(items: &[VaultItem], folders: Vec<Folder>, row: usize) -> Painted {
        open_menu_with(items, folders, row, SidebarFilter::All)
    }

    /// The same, under whichever sidebar row `filter` names. The Trash and
    /// Archive rows get a DIFFERENT menu, not the live one with entries
    /// hidden -- see `out_of_vault_entries` -- so which row the list is drawn
    /// under has to be something this harness can vary.
    fn open_menu_under(
        items: &[VaultItem],
        folders: Vec<Folder>,
        row: usize,
        filter: SidebarFilter,
    ) -> Painted {
        open_menu_with(items, folders, row, filter)
    }

    fn open_menu_with(
        items: &[VaultItem],
        folders: Vec<Folder>,
        row: usize,
        filter: SidebarFilter,
    ) -> Painted {
        let mut at = row_centre(items, row);
        // The Trash draws its 30-day band (36 high, 12 under it) above the
        // rows, which `row_centre`'s live-vault frame does not.
        if filter == SidebarFilter::Trash {
            at.y += 48.0;
        }
        paint_core(
            items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu {
                folders,
                frames: click_frames(at, egui::PointerButton::Secondary),
                filter,
            },
        )
    }

    fn text_centre(p: &Painted, label: &str) -> egui::Pos2 {
        p.texts
            .iter()
            .find(|(text, _, _)| text == label)
            .unwrap_or_else(|| {
                panic!("{label:?} was never painted; the menu drew {:?}", menu_labels(p))
            })
            .1
            .center()
    }

    /// Right-clicks row `row`, then rests the pointer on "Move to folder"
    /// for several frames so its submenu opens (egui opens submenus on
    /// hover, not on click).
    fn open_move_submenu(items: &[VaultItem], folders: Vec<Folder>, row: usize) -> Painted {
        let hover = text_centre(&open_menu(items, folders.clone(), row), move_to_folder_label());
        let at = row_centre(items, row);
        let mut frames = click_frames(at, egui::PointerButton::Secondary);
        frames.extend((0..4).map(|_| vec![egui::Event::PointerMoved(hover)]));
        paint_core(
            items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu { folders, frames, filter: SidebarFilter::All },
        )
    }

    // **`open_icon_submenu`, `icon_submenu_labels`, `ICON_VOCABULARY` and
    // `choose_icon_entry` were here and are gone.** The row menu has no Icon
    // submenu to open: the group moved to the edit pane's pencil badge, and
    // its painted coverage moved with it -- see `detail_edit`'s
    // `every_row_of_the_pencil_menu_reports_its_own_action`. What is left
    // here is the negative, `no_right_click_menu_paints_any_icon_wording`,
    // which is the half this file can still make.

    /// Right-clicks row `row`, then left-clicks the menu entry reading
    /// `label`, and returns the frame that second click resolved on.
    fn choose_entry(items: &[VaultItem], folders: Vec<Folder>, row: usize, label: &str) -> Painted {
        choose_entry_under(items, folders, row, label, SidebarFilter::All)
    }

    /// The same, under whichever sidebar row `filter` names -- the Trash and
    /// Archive rows have their own entries, and clicking one has to come back
    /// out as its own `RowCommand`.
    fn choose_entry_under(
        items: &[VaultItem],
        folders: Vec<Folder>,
        row: usize,
        label: &str,
        filter: SidebarFilter,
    ) -> Painted {
        let entry = text_centre(
            &open_menu_under(items, folders.clone(), row, filter.clone()),
            label,
        );
        let mut at = row_centre(items, row);
        // The Trash's band, as in `open_menu_with`.
        if filter == SidebarFilter::Trash {
            at.y += 48.0;
        }
        let mut frames = click_frames(at, egui::PointerButton::Secondary);
        let mut entry_click = click_frames(entry, egui::PointerButton::Primary);
        // The MEASURED frame has to be the one the click resolves on. egui
        // reports a click on the frame the button is released, and the entry
        // acts (and the menu closes) on that same frame -- measuring the
        // settled frame after it reads back a closed menu and no action,
        // which is what this harness did on its first run.
        entry_click.pop();
        frames.extend(entry_click);
        paint_core(
            items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu { folders, frames, filter },
        )
    }

    fn folder(id: &str, name: &str) -> Folder {
        Folder { id: id.into(), name: name.into(), other: serde_json::Map::new() }
    }

    /// A login with a password, a TOTP seed and a URI, so every entry that
    /// depends on one of those is reachable.
    fn full_login(name: &str) -> VaultItem {
        let mut item = login(name, "a.novak@ledgerline.com");
        let data = item.login.as_mut().unwrap();
        data.password = Some(zeroize::Zeroizing::new("hunter2".into()));
        data.totp = Some(zeroize::Zeroizing::new("JBSWY3DPEHPK3PXP".into()));
        data.uris = vec![crate::vault_bridge::UriEntry {
            uri: Some("https://ledgerline.com".into()),
            other: serde_json::Map::new(),
        }];
        item
    }

    fn card(name: &str) -> VaultItem {
        VaultItem { item_type: Some(3), login: None, ..login(name, "") }
    }

    /// A card with a number stored in the shape the vault bridge produces --
    /// a `Zeroizing<String>` on `CardData`.
    fn card_numbered(name: &str, number: &str) -> VaultItem {
        let mut it = card(name);
        it.card = Some(crate::vault_bridge::CardData {
            number: Some(zeroize::Zeroizing::new(number.to_string())),
            ..Default::default()
        });
        it
    }

    /// The kind whose Edit entry is still greyed -- see
    /// `EDIT_DISABLED_REASON`. A card's no longer is.
    fn ssh_key(name: &str) -> VaultItem {
        VaultItem { item_type: Some(5), login: None, ..login(name, "") }
    }

    #[test]
    fn a_right_click_selects_the_row_it_lands_on() {
        // The whole reason right-click is not just "open a menu": the menu
        // acts on this row, and if the row were not also selected the menu
        // and the detail pane could be showing two different items while the
        // user chose Delete.
        let items = [full_login("Ledgerline"), full_login("Vantage")];
        let p = open_menu(&items, vec![], 1);
        assert_eq!(p.selected.as_deref(), Some("Vantage"));
    }

    #[test]
    fn a_left_click_still_selects_the_row_it_lands_on() {
        // The pre-existing behaviour, re-asserted from the same harness so
        // that adding the right-click path cannot have quietly replaced it.
        let items = [full_login("Ledgerline"), full_login("Vantage")];
        let at = row_centre(&items, 0);
        let p = paint_core(
            &items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu {
                folders: vec![],
                frames: click_frames(at, egui::PointerButton::Primary),
                filter: SidebarFilter::All,
            },
        );
        assert_eq!(p.selected.as_deref(), Some("Ledgerline"));
    }

    /// **The icon group is ONE painted line now**, and this is where that is
    /// visible: the popup draws `ICON_LABEL` and neither of the two wordings
    /// that used to be here. Both halves matter -- `MENU_VOCABULARY` still
    /// carries all three icon wordings, so a build that painted them at the
    /// top level as well would fail this rather than slip past it.
    #[test]
    fn a_right_click_on_a_login_paints_exactly_that_login_s_entries() {
        let items = [full_login("Ledgerline")];
        assert_eq!(
            menu_labels(&open_menu(&items, vec![folder("f1", "Work")], 0)),
            vec![
                "Copy username",
                "Copy password",
                "Copy TOTP",
                "Open website",
                "Edit",
                move_to_folder_label(),
                "Archive",
                delete_label(),
            ]
        );
    }

    #[test]
    fn a_right_click_on_a_card_paints_no_open_website() {
        // The painted half of `menu_entry_tests`' per-kind assertions: the
        // decision being right is worth nothing if the popup draws a fixed
        // list regardless.
        let items = [card("Visa (personal)")];
        assert_eq!(
            menu_labels(&open_menu(&items, vec![folder("f1", "Work")], 0)),
            vec!["Edit", move_to_folder_label(), "Archive", delete_label()]
        );
    }

    /// **No right-click menu paints any icon wording**, for either kind.
    ///
    /// The painted counterpart of `menu_entry_tests`'
    /// `no_row_menu_offers_an_icon_entry_whatever_the_source`, and the whole
    /// of what this file still has to say about the icon group: the three
    /// entries and the fold over them now live on the edit pane's pencil
    /// badge, where `detail_edit` paints and clicks them.
    ///
    /// Read off the painted galleys and not off `menu_entries`, because that
    /// is the half a decision cannot make: a menu could go on drawing a row
    /// it no longer decides to offer.
    #[test]
    fn no_right_click_menu_paints_any_icon_wording() {
        let items = [full_login("Ledgerline"), card("Visa (personal)")];
        for row in 0..items.len() {
            let drawn = menu_labels(&open_menu(&items, vec![folder("f1", "Work")], row));
            for label in [icon_label(), refresh_icon_label(), select_icon_label(), clear_icon_label()] {
                assert!(
                    !drawn.contains(&label.to_string()),
                    "row {row} painted {label:?}: {drawn:?}"
                );
            }
            // The live control: this harness really does paint a menu, so
            // the four absences above are absences and not a popup that
            // never opened.
            assert!(
                drawn.contains(&"Edit".to_string()),
                "row {row} painted no menu at all: {drawn:?}"
            );
        }
    }

    #[test]
    fn a_login_with_no_totp_seed_paints_no_copy_totp_entry() {
        let mut item = full_login("Ledgerline");
        item.login.as_mut().unwrap().totp = None;
        assert_eq!(
            menu_labels(&open_menu(&[item], vec![], 0)),
            vec![
                "Copy username",
                "Copy password",
                "Open website",
                "Edit",
                move_to_folder_label(),
                "Archive",
                delete_label(),
            ]
        );
    }

    /// **The gesture that arms the detail pane's slide, and the three that do
    /// not.**
    ///
    /// `vault_window::mod` arms the slide on `row_opened && !pane_shown_
    /// before_click`. That first half is decided HERE, and the owner's rule
    /// turns entirely on it being the GESTURE rather than the selection
    /// changing: a right-click selects a row on its way to a menu, and a
    /// keyboard move selects one without any click at all. Both change
    /// `selected_id`; neither opens an item.
    ///
    /// So this is the whole of the negative claim, with the positive control
    /// beside it in the same harness -- without the control, "a right-click
    /// does not open" would pass just as well against a flag that was never
    /// set by anything.
    #[test]
    fn only_a_primary_click_reports_a_row_as_opened() {
        let items = [full_login("Ledgerline"), full_login("Vantage")];
        let at = row_centre(&items, 1);

        // The first TWO frames of a click -- press, then release. The
        // harness measures the LAST frame it is given, and `row_opened` is a
        // per-frame gesture flag (the caller reads it on the frame the click
        // lands and clears it), so `click_frames`' third settling frame would
        // measure a frame with no click in it and read `false` for both
        // buttons. `selected_id` persists and would have hidden that.
        let click = |button| {
            let frames: Vec<Vec<egui::Event>> =
                click_frames(at, button).into_iter().take(2).collect();
            paint_core(
                &items,
                None,
                0,
                PANE_WIDTH,
                |_| IconCache::default(),
                Menu {
                    folders: vec![],
                        frames,
                    filter: SidebarFilter::All,
                },
            )
        };

        // THE CONTROL: a primary click opens the row.
        let primary = click(egui::PointerButton::Primary);
        assert!(
            primary.row_opened,
            "a primary click on a row did not report it as opened, so nothing would ever \
             arm the detail pane's slide"
        );
        assert_eq!(
            primary.selected.as_deref(),
            Some("Vantage"),
            "the control did not even select the row, so it is not testing the gesture"
        );

        // THE CLAIM: a right-click selects the row -- deliberately, so the
        // menu and the pane agree -- and does NOT open it.
        let secondary = click(egui::PointerButton::Secondary);
        assert_eq!(
            secondary.selected.as_deref(),
            Some("Vantage"),
            "a right-click must still SELECT the row it was made on; if it does not, this \
             test is no longer telling the gesture from the selection"
        );
        assert!(
            !secondary.row_opened,
            "a right-click reported the row as opened. It selects on its way to a menu, \
             which the owner ruled out of the slide explicitly -- the pane would animate \
             every time a context menu was opened from an empty pane"
        );
    }

    /// A keyboard move is the third way `selected_id` changes inside this
    /// function, and it is not a click either. Same rule, and the same reason
    /// to state it separately: it takes a different code path (`next_
    /// selection`, not `RowOutcome`) and so could break on its own.
    #[test]
    fn a_keyboard_move_selects_without_reporting_a_row_as_opened() {
        let items = [full_login("Ledgerline"), full_login("Vantage")];
        let p = paint_core(
            &items,
            Some("Ledgerline"),
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu {
                folders: vec![],
                frames: vec![vec![egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                }]],
                filter: SidebarFilter::All,
            },
        );
        assert_eq!(
            p.selected.as_deref(),
            Some("Vantage"),
            "the arrow key did not move the selection, so this test never reached the \
             thing it names"
        );
        assert!(
            !p.row_opened,
            "a keyboard move reported a row as opened; the slide is armed by a click, and \
             arrowing through a list would animate the pane on every step"
        );
    }

    /// A frame with no gesture at all reports nothing -- the flag is not
    /// simply stuck on. Cheap, and it is the assertion that would have caught
    /// `row_opened` being set from `outcome.select` instead of
    /// `outcome.opened`, since the selection is non-empty on every one of
    /// these frames.
    #[test]
    fn a_frame_with_no_click_reports_no_row_opened() {
        let p = paint(&[full_login("Ledgerline")], Some("Ledgerline"));
        assert!(
            !p.row_opened,
            "a frame that drew a selected row with no click on it reported the row as \
             opened, so the slide would re-arm on every frame"
        );
    }

    /// **Every row's Delete reads the same thing, whichever row it is.**
    /// The wording used to depend on `delete_pending_id`, which was one id
    /// rather than a flag precisely so an arm belonged to the row it was set
    /// on. There is no arm now, and this pins that the menu two rows apart
    /// is one menu -- a per-row label would be a state that survived the
    /// change to the modal.
    #[test]
    fn every_rows_delete_entry_reads_the_same() {
        let items = [full_login("Ledgerline"), full_login("Vantage")];
        for row in 0..items.len() {
            let p = open_menu_with(&items, vec![], row, SidebarFilter::All);
            assert!(
                menu_labels(&p).contains(&delete_label().to_string()),
                "row {row} drew {:?}",
                menu_labels(&p)
            );
            assert!(
                !menu_labels(&p).iter().any(|label| label.contains("Click to confirm")),
                "row {row} still confirms in the menu: {:?}",
                menu_labels(&p)
            );
        }
    }

    #[test]
    fn choosing_an_entry_reports_it_against_the_row_that_was_right_clicked() {
        let items = [full_login("Ledgerline"), full_login("Vantage")];
        let p = choose_entry(&items, vec![], 1, "Copy password");
        assert_eq!(
            p.action,
            ItemListAction::Row {
                id: "Vantage".to_string(),
                command: RowCommand::CopyPassword,
            }
        );
    }

    #[test]
    fn choosing_open_website_reports_the_items_own_url() {
        let items = [full_login("Ledgerline")];
        let p = choose_entry(&items, vec![], 0, "Open website");
        assert_eq!(
            p.action,
            ItemListAction::Row {
                id: "Ledgerline".to_string(),
                command: RowCommand::OpenWebsite("https://ledgerline.com".to_string()),
            }
        );
    }

    // **Three painted tests were here**: that clicking Refresh two levels
    // down reported it against the row that was right-clicked, that all
    // three rows answered a click, and that a row with no icon domain
    // painted no Refresh. They went with the submenu. The same three claims
    // are made on the edit pane now -- see `detail_edit`'s
    // `every_row_of_the_pencil_menu_reports_its_own_action`.

    #[test]
    fn the_greyed_edit_entry_reports_nothing_when_it_is_clicked() {
        // Greyed has to mean inert. A disabled entry that still fired would
        // hand an SSH key to a form with no boxes for its keys, whose Save
        // would change nothing but the name.
        let items = [ssh_key("deploy key")];
        let p = choose_entry(&items, vec![], 0, "Edit");
        assert_eq!(p.action, ItemListAction::None);
        // The positive control, in the same harness: a card's Edit is NOT
        // greyed now, so the assertion above is the greying and not the
        // menu failing to deliver clicks.
        let cards = [card("Visa (personal)")];
        assert_eq!(
            choose_entry(&cards, vec![], 0, "Edit").action,
            ItemListAction::Row { id: "Visa (personal)".to_string(), command: RowCommand::Edit },
        );
    }

    #[test]
    fn just_opening_the_menu_reports_nothing() {
        // The bite check on the two assertions above: if merely right-
        // clicking produced an action, they would pass without a single
        // menu entry ever being clicked.
        let items = [full_login("Ledgerline")];
        assert_eq!(open_menu(&items, vec![folder("f1", "Work")], 0).action, ItemListAction::None);
    }

    #[test]
    fn the_move_submenu_paints_exactly_the_assignable_folders() {
        // The painted half of `menu_entry_tests`'
        // `the_move_submenu_excludes_the_virtual_no_folder_bucket`. `bw
        // serve` reports its "no folder" bucket AS A FOLDER with an empty
        // id; drawing it as a destination writes `folderId: ""` and strands
        // the item out of every sidebar row.
        let items = [full_login("Ledgerline")];
        let folders = vec![
            folder("", "No Folder"),
            folder("f1", "Work"),
            folder("f2", "Personal"),
        ];
        let p = open_move_submenu(&items, folders, 0);
        let painted: Vec<String> = p
            .texts
            .iter()
            .map(|(text, _, _)| text.clone())
            .filter(|text| ["No Folder", "Work", "Personal"].contains(&text.as_str()))
            .collect();
        assert_eq!(painted, vec!["Work", "Personal"]);
    }

    #[test]
    fn choosing_a_folder_reports_a_move_to_that_folders_id() {
        let items = [full_login("Ledgerline")];
        let folders = vec![folder("", "No Folder"), folder("f2", "Personal")];
        let opened = open_move_submenu(&items, folders.clone(), 0);
        let destination = text_centre(&opened, "Personal");
        let hover = text_centre(&open_menu(&items, folders.clone(), 0), move_to_folder_label());
        let at = row_centre(&items, 0);
        let mut frames = click_frames(at, egui::PointerButton::Secondary);
        // The pointer travels along the parent entry and then onto the
        // destination, resting on each: egui opens a submenu on hover and
        // closes it when the pointer leaves, so a jump straight from the row
        // to a submenu row lands on a submenu that was never open.
        frames.extend((0..4).map(|_| vec![egui::Event::PointerMoved(hover)]));
        frames.extend((0..3).map(|_| vec![egui::Event::PointerMoved(destination)]));
        let mut click = click_frames(destination, egui::PointerButton::Primary);
        // Measured on the release -- see `choose_entry`.
        click.pop();
        frames.extend(click);
        let p = paint_core(
            &items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu { folders, frames, filter: SidebarFilter::All },
        );
        assert_eq!(
            p.action,
            ItemListAction::Row {
                id: "Ledgerline".to_string(),
                command: RowCommand::MoveToFolder("f2".to_string()),
            }
        );
    }

    #[test]
    fn the_menu_does_not_change_the_rows_it_is_drawn_over() {
        // VIRTUALIZATION. `show_rows` reads `item_spacing.y` from the ui it
        // is given before the closure runs and scrolls by a fixed pitch, so
        // anything a row allocates for its menu would slide the list out of
        // register with its own scrollbar. A context menu lives in egui's
        // popup layer and allocates nothing here -- asserted, not assumed,
        // by comparing the painted tiles and the laid-out range against the
        // same list with no menu open.
        let items: Vec<VaultItem> = (0..100).map(|i| full_login(&format!("Item {i:03}"))).collect();
        let closed = paint(&items, None);
        let open = open_menu(&items, vec![folder("f1", "Work")], 2);

        assert_eq!(open.visible, closed.visible, "the laid-out row range changed");
        let closed_tiles: Vec<egui::Rect> = row_tiles(&closed).iter().map(|t| t.rect).collect();
        let open_tiles: Vec<egui::Rect> = row_tiles(&open).iter().map(|t| t.rect).collect();
        assert_eq!(
            open_tiles.len(),
            closed_tiles.len(),
            "a row stopped being exactly one ROW_TILE_HEIGHT tall while a menu was open"
        );
        for (open, closed) in open_tiles.iter().zip(&closed_tiles) {
            assert!(
                (open.top() - closed.top()).abs() < 0.5 && (open.height() - closed.height()).abs() < 0.5,
                "a row tile moved or resized while a menu was open: {open:?} vs {closed:?}"
            );
        }
    }

    #[test]
    fn a_drag_in_flight_does_not_move_the_rows_underneath_it() {
        // VIRTUALIZATION, for the drag source. `show_rows` reads
        // `item_spacing.y` off the ui it is GIVEN, before its closure runs,
        // and scrolls by `row_height + that spacing`; anything the drag
        // source allocated inside a row would put the list out of register
        // with its own scrollbar. `Response::interact` re-registers the rect
        // the row already claimed and the ghost is painted into a `Tooltip`
        // layer, so neither allocates -- asserted here from painted output
        // rather than from the comment at the call site.
        let items: Vec<VaultItem> = (0..100).map(|i| full_login(&format!("Item {i:03}"))).collect();
        let still = paint(&items, None);
        let from = row_centre(&items, 3);
        let to = from + egui::vec2(0.0, 180.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut frames = vec![
            vec![egui::Event::PointerMoved(from)],
            vec![egui::Event::PointerMoved(from), button(true)],
        ];
        // Measured mid-flight: the button is never released, so the payload
        // is on egui's clipboard on the frame this reads back.
        for step in 1..=4 {
            frames.push(vec![egui::Event::PointerMoved(from + (to - from) * (step as f32 / 4.0))]);
        }
        let dragging = paint_core(
            &items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu { folders: vec![], frames, filter: SidebarFilter::All },
        );

        assert_eq!(dragging.visible, still.visible, "the laid-out row range changed mid-drag");
        let still_tiles: Vec<egui::Rect> = row_tiles(&still).iter().map(|t| t.rect).collect();
        let dragging_tiles: Vec<egui::Rect> = row_tiles(&dragging).iter().map(|t| t.rect).collect();
        assert_eq!(
            dragging_tiles.len(),
            still_tiles.len(),
            "a row stopped being exactly one ROW_TILE_HEIGHT tall mid-drag"
        );
        for (moving, still) in dragging_tiles.iter().zip(&still_tiles) {
            assert!(
                (moving.top() - still.top()).abs() < 0.5
                    && (moving.height() - still.height()).abs() < 0.5,
                "a row tile moved or resized mid-drag: {moving:?} vs {still:?}"
            );
        }
    }

    #[test]
    fn a_right_click_still_opens_the_menu_now_that_rows_also_sense_drags() {
        // The interaction the drag source could most plausibly have broken:
        // the row's sense went from `click` to `click_and_drag`, and the
        // context menu hangs off the SAME response, anchored per item by
        // `push_id`. Dragging is primary-button only, so the two do not
        // overlap -- asserted, because "they shouldn't" is not evidence.
        let items = [full_login("Ledgerline")];
        assert_eq!(
            menu_labels(&open_menu(&items, vec![folder("f1", "Work")], 0)),
            vec![
                "Copy username",
                "Copy password",
                "Copy TOTP",
                "Open website",
                "Edit",
                move_to_folder_label(),
                "Archive",
                delete_label(),
            ]
        );
    }

    // ---- The "+ New" type menu ------------------------------------------
    //
    // `+ New` no longer creates a login on the spot: it opens a menu of the
    // kinds this build can create, and nothing exists until one is picked.
    // These drive the real button through real pointer frames, because the
    // menu is drawn into egui's popup layer and nothing about it is visible
    // to a unit test over the action enum.

    /// The five rows the menu must offer, in order, spelled out.
    ///
    /// **Deliberately hardcoded and NOT derived from `CREATABLE_KINDS`.** A
    /// test that mapped the array through `ItemKind::label` would agree with
    /// the menu no matter what the array said, which is exactly the shape of
    /// "coverage" this codebase has already been bitten by. The array is
    /// pinned against this same list separately, by
    /// `the_creatable_kinds_are_exactly_the_five_the_menu_is_pinned_to`, so a
    /// sixth kind fails HERE (the menu grew a row) and THERE (the array grew
    /// an entry) rather than passing quietly in both.
    const NEW_MENU_ROWS: [&str; 5] = ["Login", "Secure note", "Card", "Identity", "SSH key"];

    /// The centre of the header's `+`, read off a drawn frame.
    fn new_button_centre(items: &[VaultItem]) -> egui::Pos2 {
        paint(items, None).new_button.expect("the header's + was never drawn").center()
    }

    /// Every string painted this frame, in paint order. Used as a
    /// before/after pair so the menu's contents are found by DIFFERENCE
    /// rather than by looking for the labels the test expects -- a menu that
    /// also offered "Unsupported item" would show up in the difference.
    fn all_texts(p: &Painted) -> Vec<String> {
        p.texts.iter().map(|(text, _, _)| text.clone()).collect()
    }

    /// The strings `open` painted that `closed` did not, in order.
    fn extra_texts(closed: &Painted, open: &Painted) -> Vec<String> {
        let mut before = all_texts(closed);
        let mut extra = Vec::new();
        for text in all_texts(open) {
            match before.iter().position(|t| *t == text) {
                Some(at) => {
                    before.remove(at);
                }
                None => extra.push(text),
            }
        }
        extra
    }

    /// Left-clicks `+ New` and returns the frame its menu is open on.
    fn open_new_menu(items: &[VaultItem]) -> Painted {
        let at = new_button_centre(items);
        paint_core(
            items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu {
                folders: vec![],
                frames: click_frames(at, egui::PointerButton::Primary),
                filter: SidebarFilter::All,
            },
        )
    }

    #[test]
    fn the_new_button_opens_a_menu_of_exactly_the_creatable_kinds() {
        // Item names chosen so none of them can be mistaken for a kind row.
        let items = [full_login("Ledgerline"), full_login("Vantage")];
        let closed = paint(&items, None);
        let open = open_new_menu(&items);
        // **The five kinds, then the import.** The import row is deliberately
        // NOT in `NEW_MENU_ROWS`: that constant is one half of the pair that
        // pins `CREATABLE_KINDS`, and an entry there that is not a kind would
        // make the OTHER half of the pair
        // (`the_creatable_kinds_are_exactly_the_five_the_menu_is_pinned_to`)
        // demand a sixth `ItemKind` for a row that has none. It is written
        // out here instead, where the claim is about what the menu PAINTS.
        //
        // Order is asserted with it: the import sits below the kinds, behind a
        // separator, because it is the one row that is not a shape this menu
        // chose.
        let expected: Vec<String> = NEW_MENU_ROWS
            .iter()
            .map(|s| s.to_string())
            .chain(std::iter::once(
                super::super::record_ui::import_from_send_label().to_string(),
            ))
            .collect();
        assert_eq!(
            extra_texts(&closed, &open),
            expected,
            "the \"+ New\" menu drew something other than the creatable kinds and the import"
        );
    }

    /// **Picking the import row asks for the import and creates nothing.**
    ///
    /// The row is the whole of how a user reaches the record import: there is
    /// no chord for it and no other control anywhere in this window that
    /// opens it. A row that painted and reported `None` would look identical
    /// on screen and be the same defect the import surface already shipped
    /// with once -- a finished screen nobody can get to.
    #[test]
    fn picking_the_import_row_asks_to_import_a_record() {
        let items = [full_login("Ledgerline")];
        let row = text_centre(
            &open_new_menu(&items),
            super::super::record_ui::import_from_send_label(),
        );
        let at = new_button_centre(&items);
        let mut frames = click_frames(at, egui::PointerButton::Primary);
        let mut pick = click_frames(row, egui::PointerButton::Primary);
        // Measured on the release, which is the frame egui resolves the click
        // on -- `picking_a_kind_asks_for_a_new_item_of_that_kind`'s rule.
        pick.pop();
        frames.extend(pick);
        let p = paint_core(
            &items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu {
                folders: vec![],
                frames,
                filter: SidebarFilter::All,
            },
        );
        assert_eq!(
            p.action,
            ItemListAction::ImportFromSend,
            "clicking the import row reported {:?}, so the one way into the record import \
             does nothing",
            p.action
        );
    }

    #[test]
    fn the_creatable_kinds_are_exactly_the_five_the_menu_is_pinned_to() {
        // The other half of `NEW_MENU_ROWS`' contract: the menu builds its
        // rows FROM `CREATABLE_KINDS`, and that array is one of three doors
        // keeping `ItemKind::Unknown` unreachable from creation. If a kind is
        // added to it, this fails and says so, rather than the painted test
        // above failing on its own with no explanation of where the extra row
        // came from.
        let labels: Vec<String> = super::super::detail_edit::CREATABLE_KINDS
            .iter()
            .map(|kind| kind.label())
            .collect();
        assert_eq!(labels, NEW_MENU_ROWS);
    }

    #[test]
    fn opening_the_new_menu_creates_nothing_by_itself() {
        // The user's explicit choice: the button ALWAYS opens the dropdown,
        // and nothing is created until a kind is picked. A `+ New` that
        // reported `NewItem(Login)` on the way to showing the menu would
        // look identical on screen and be wrong.
        let items = [full_login("Ledgerline")];
        assert_eq!(open_new_menu(&items).action, ItemListAction::None);
    }

    #[test]
    fn picking_a_kind_asks_for_a_new_item_of_that_kind() {
        // Every row, with its kind written out rather than looked up, so a
        // menu that offered the right five labels wired to the wrong five
        // kinds fails here.
        for (label, kind) in [
            ("Login", ItemKind::Login),
            ("Secure note", ItemKind::SecureNote),
            ("Card", ItemKind::Card),
            ("Identity", ItemKind::Identity),
            ("SSH key", ItemKind::SshKey),
        ] {
            let items = [full_login("Ledgerline")];
            let row = text_centre(&open_new_menu(&items), label);
            let at = new_button_centre(&items);
            let mut frames = click_frames(at, egui::PointerButton::Primary);
            let mut pick = click_frames(row, egui::PointerButton::Primary);
            // Measured on the release, which is the frame egui resolves the
            // click on -- see `choose_entry`.
            pick.pop();
            frames.extend(pick);
            let p = paint_core(
                &items,
                None,
                0,
                PANE_WIDTH,
                |_| IconCache::default(),
                Menu { folders: vec![], frames, filter: SidebarFilter::All },
            );
            assert_eq!(
                p.action,
                ItemListAction::NewItem(kind),
                "picking {label:?} asked for the wrong kind"
            );
        }
    }

    #[test]
    fn the_new_menu_does_not_change_the_rows_below_it() {
        // VIRTUALIZATION, the same guard `the_menu_does_not_change_the_rows_
        // it_is_drawn_over` applies to the row menu: `show_rows` reads
        // `item_spacing.y` from the ui it is given BEFORE its closure runs
        // and scrolls by a fixed pitch, so anything the menu allocated in the
        // toolbar strip would move every tile below it. A `Popup` draws into
        // its own `Area`, which allocates nothing here -- asserted rather
        // than assumed.
        let items: Vec<VaultItem> = (0..100).map(|i| full_login(&format!("Item {i:03}"))).collect();
        let closed = paint(&items, None);
        let open = open_new_menu(&items);
        assert_eq!(open.visible, closed.visible, "the laid-out row range changed");
        let closed_tiles: Vec<egui::Rect> = row_tiles(&closed).iter().map(|t| t.rect).collect();
        let open_tiles: Vec<egui::Rect> = row_tiles(&open).iter().map(|t| t.rect).collect();
        assert_eq!(open_tiles.len(), closed_tiles.len(), "a row tile changed height");
        for (open, closed) in open_tiles.iter().zip(&closed_tiles) {
            assert!(
                (open.top() - closed.top()).abs() < 0.5,
                "a row tile moved while the \"+ New\" menu was open: {open:?} vs {closed:?}"
            );
        }
    }

    // ---- The menu a row gets depends on which sidebar row it was drawn
    // under. `menu_entries` decides that from a `FilterSource` and is
    // exhaustively tested; what was NOT tested is that `draw_item_list`
    // passes the LIVE filter's source rather than a constant. A reviewer
    // replaced that one argument with `FilterSource::LiveVault` and the
    // whole suite stayed green while a trashed item offered Copy password,
    // Move to folder, Edit, Archive and Delete -- five entries that are
    // either rejected by the CLI or succeed at nothing.
    //
    // These drive the real right-click through the real widget tree, so they
    // fail on that mutation rather than describing it.

    /// Every entry the LIVE menu can offer. The negative assertions below
    /// are written against this list rather than against a couple of
    /// hand-picked labels, so an entry added to the live menu later cannot
    /// quietly start appearing on a trashed item too.
    /// **`ICON_LABEL` joined this list when the icon group folded into a
    /// submenu and left it when the group left the menu.** It is on the edit
    /// pane's pencil badge now, so it is no longer an entry the LIVE menu can
    /// offer and naming it here would fail every positive assertion below.
    /// `no_right_click_menu_paints_any_icon_wording` is where its absence is
    /// asserted instead, on both sides rather than only on the out-of-vault
    /// one this array guards.
    fn live_only_entries() -> [&'static str; 8] {
    [
        "Copy username",
        "Copy password",
        "Copy TOTP",
        "Open website",
        "Edit",
        move_to_folder_label(),
        "Archive",
        delete_label(),
    ]
    }

    fn painted_strings(p: &Painted) -> Vec<&str> {
        p.texts.iter().map(|(t, _, _)| t.as_str()).collect()
    }

    /// Asserts that none of the live vault's entries were painted, having
    /// first established that a menu was painted at all.
    ///
    /// The positive control is the whole point: "the menu has no Copy
    /// password" passes trivially against a right-click that opened nothing,
    /// which is exactly what a test written only in negatives would have
    /// reported as success.
    fn assert_out_of_vault_menu(p: &Painted, expected: &[&str]) {
        let painted = painted_strings(p);
        for entry in expected {
            assert!(
                painted.contains(entry),
                "the menu did not offer {entry:?} -- so the negative assertions below \
                 would have passed against a menu that painted nothing. Painted: {painted:?}"
            );
        }
        for entry in live_only_entries() {
            assert!(
                !painted.contains(&entry),
                "an out-of-vault row offered the live vault's {entry:?}. Every entry on \
                 that menu reads or writes through the LIVE list, which by construction \
                 does not hold this item. Painted: {painted:?}"
            );
        }
    }

    #[test]
    fn a_trashed_rows_menu_offers_restore_and_purge_and_nothing_from_the_live_vault() {
        let items = vec![full_login("Ledgerline"), full_login("Vantage")];
        let p = open_menu_under(&items, vec![folder("f1", "Work")], 0, SidebarFilter::Trash);
        assert_out_of_vault_menu(&p, &["Restore", purge_label()]);
    }

    /// **A view-only row's menu that would be empty says why** (final
    /// review, item 7): an `edit: false` card has nothing to copy and every
    /// other entry writes, and an `edit: false` record in the Trash has only
    /// writes -- both used to open a blank popup. A view-only login with a
    /// username still offers the copy, and no footer.
    #[test]
    fn an_empty_view_only_menu_says_view_only_instead_of_opening_blank() {
        let view_only = |mut item: VaultItem| {
            item.other.insert("edit".into(), serde_json::Value::Bool(false));
            item
        };
        let cards = [view_only(card("Visa (shared)"))];
        assert!(menu_entries(&cards[0], &[], FilterSource::LiveVault, true).is_empty(), "the control: nothing to offer");
        let p = open_menu(&cards, vec![], 0);
        assert!(painted_strings(&p).contains(&super::super::detail::view_only_footer_text()), "{:?}", painted_strings(&p));

        let trashed = [view_only(full_login("Ledgerline"))];
        let p = open_menu_under(&trashed, vec![], 0, SidebarFilter::Trash);
        assert!(painted_strings(&p).contains(&super::super::detail::view_only_footer_text()), "{:?}", painted_strings(&p));

        let logins = [view_only(full_login("Ledgerline"))];
        let p = open_menu(&logins, vec![], 0);
        let painted = painted_strings(&p);
        assert!(painted.contains(&"Copy username"), "{painted:?}");
        assert!(!painted.contains(&super::super::detail::view_only_footer_text()), "a menu with entries has no footer: {painted:?}");
        assert_eq!(empty_menu_footer(&menu_entries(&logins[0], &[], FilterSource::LiveVault, true)), None);
    }

    #[test]
    fn an_archived_rows_menu_offers_unarchive_and_nothing_from_the_live_vault() {
        let items = vec![full_login("Ledgerline"), full_login("Vantage")];
        let p = open_menu_under(&items, vec![folder("f1", "Work")], 0, SidebarFilter::Archive);
        assert_out_of_vault_menu(&p, &["Unarchive"]);
        // Archive's menu is the one-entry case, so the absence of Trash's own
        // two entries is worth stating: the two rows must not share a menu
        // either.
        let painted = painted_strings(&p);
        for entry in ["Restore", purge_label()] {
            assert!(!painted.contains(&entry), "an archived row offered {entry:?}");
        }
    }

    #[test]
    fn the_same_row_under_the_live_vault_still_gets_the_live_menu() {
        // The control for BOTH tests above: their negatives would also be
        // satisfied by a `draw_item_list` that had stopped drawing the live
        // menu at all, or by a `full_login` that had stopped carrying the
        // fields those entries depend on.
        let items = vec![full_login("Ledgerline"), full_login("Vantage")];
        let p = open_menu_under(&items, vec![folder("f1", "Work")], 0, SidebarFilter::All);
        let painted = painted_strings(&p);
        for entry in live_only_entries() {
            assert!(
                painted.contains(&entry),
                "the LIVE menu is missing {entry:?}; painted: {painted:?}"
            );
        }
        for entry in ["Restore", "Unarchive", purge_label()] {
            assert!(!painted.contains(&entry), "a live row offered {entry:?}");
        }
    }

    /// Every colour the frame painted `label` in, in paint order.
    ///
    /// A `Vec` rather than one colour, and a `panic!` on an empty one: a
    /// colour assertion whose label was never painted is the failure mode
    /// this file's `MENU_VOCABULARY` doc is entirely about -- a test that
    /// reads as coverage while asking nothing -- and an `unwrap_or_default`
    /// here would turn "the menu drew no Delete at all" into a comparison
    /// against `Color32::default()` that could pass or fail for reasons
    /// nobody could read off the message.
    fn painted_colours_of(p: &Painted, label: &str) -> Vec<egui::Color32> {
        let found: Vec<egui::Color32> = p
            .texts
            .iter()
            .filter(|(text, _, _)| text == label)
            .map(|(_, _, colour)| *colour)
            .collect();
        assert!(
            !found.is_empty(),
            "{label:?} was never painted, so nothing about its colour can be asserted; \
             the menu drew {:?}",
            menu_labels(p)
        );
        found
    }

    /// **Delete is red in the row menu, and its neighbours are not.**
    ///
    /// The user's report was that the row menu's Delete looked like every
    /// other entry while the detail pane's kebab had spent the app's whole
    /// life painting the same act in `theme::ERROR` -- one item, two menus,
    /// two answers to "is this the dangerous one".
    ///
    /// **The neighbours are half the assertion.** "Delete is `ERROR`" alone
    /// passes against a menu that painted every line red, which is not a
    /// warning colour but a theme change, and it is a real failure mode here
    /// because the colour is applied inside the shared `menu_command_button`
    /// that draws all of them. Archive is the entry directly above Delete and
    /// the closest thing the menu has to a decoy -- it also takes the item
    /// out of the working vault -- so it is the one worth naming; Edit is
    /// there as an ordinary line well away from the lifecycle pair.
    #[test]
    fn the_row_menus_delete_is_painted_as_an_alarm_and_its_neighbours_are_not() {
        let items = vec![full_login("Ledgerline"), full_login("Vantage")];
        let p = open_menu(&items, vec![folder("f1", "Work")], 0);
        for colour in painted_colours_of(&p, delete_label()) {
            assert_eq!(
                colour,
                theme::ERROR,
                "the row menu's Delete is painted {colour:?}; the kebab's Delete is \
                 {:?}, and one item's two menus disagree about which act is destructive",
                theme::ERROR
            );
        }
        for neighbour in ["Archive", "Edit"] {
            for colour in painted_colours_of(&p, neighbour) {
                assert_ne!(
                    colour, theme::ERROR,
                    "{neighbour:?} is painted in the alarm colour too, so the menu is \
                     uniformly red and says nothing about Delete"
                );
            }
        }
    }

    /// The same rule one level down, on the menu a TRASHED row gets: the
    /// permanent delete is red and Restore, the only other entry that menu
    /// has, is not.
    ///
    /// Worth its own test rather than a second loop in the one above,
    /// because "Delete forever" reaches the painter down a different path --
    /// `out_of_vault_entries`, not `menu_entries` -- and the colour is
    /// derived from `RowCommand::is_destructive` precisely so that both
    /// paths get it without either list saying so. A rule that held only on
    /// the live menu would leave the app's one irreversible act as the
    /// plainest line in its menu.
    #[test]
    fn a_trashed_rows_delete_forever_is_painted_as_an_alarm_and_restore_is_not() {
        let items = vec![full_login("Ledgerline")];
        let p = open_menu_under(&items, vec![], 0, SidebarFilter::Trash);
        for colour in painted_colours_of(&p, purge_label()) {
            assert_eq!(
                colour,
                theme::ERROR,
                "the app's one irreversible entry is painted {colour:?}"
            );
        }
        for colour in painted_colours_of(&p, "Restore") {
            assert_ne!(
                colour, theme::ERROR,
                "Restore is painted in the alarm colour, so the trash menu is uniformly \
                 red and says nothing about Delete forever"
            );
        }
    }

    /// **The trash row's one irreversible entry is painted, with one
    /// wording.** It used to change its words when armed, and this test
    /// asserted that the arm reached the out-of-vault menu at all. The arm
    /// is gone -- the entry opens [`super::super::delete_modal`] with
    /// `DeleteKind::Forever` -- so what is worth pinning is that the entry
    /// really reaches the painted menu for a row drawn under Trash, which is
    /// still the half `draw_item_list` could silently drop by forwarding the
    /// wrong source.
    #[test]
    fn a_trashed_rows_purge_entry_is_painted_with_one_wording() {
        let items = vec![full_login("Ledgerline")];
        let p = open_menu_with(&items, vec![], 0, SidebarFilter::Trash);
        let painted = painted_strings(&p);
        assert!(
            painted.contains(&purge_label()),
            "\"Delete forever\" never reached the trash menu; painted: {painted:?}"
        );
        assert!(
            !painted.iter().any(|text| text.contains("Click to confirm")),
            "the trash menu still confirms inside itself: {painted:?}"
        );
    }

    #[test]
    fn choosing_restore_on_a_trashed_row_returns_the_restore_command() {
        // The entries are not merely painted: clicking one has to come back
        // out of `draw_item_list` as the command `vault_window::mod`'s arm
        // acts on. A menu that painted "Restore" and returned
        // `ItemListAction::None` is the dead-button shape this whole feature
        // exists to close.
        let items = vec![full_login("Ledgerline"), full_login("Vantage")];
        let p = choose_entry_under(&items, vec![], 0, "Restore", SidebarFilter::Trash);
        assert_eq!(
            p.action,
            ItemListAction::Row {
                id: "Ledgerline".to_string(),
                command: RowCommand::Restore
            },
            "clicking Restore did not return the Restore command"
        );
    }

    #[test]
    fn choosing_unarchive_on_an_archived_row_returns_the_unarchive_command() {
        let items = vec![full_login("Ledgerline"), full_login("Vantage")];
        let p = choose_entry_under(&items, vec![], 0, "Unarchive", SidebarFilter::Archive);
        assert_eq!(
            p.action,
            ItemListAction::Row {
                id: "Ledgerline".to_string(),
                command: RowCommand::Unarchive
            },
            "clicking Unarchive did not return the Unarchive command"
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
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// [`run_item_list`], with `events` delivered to the MEASURED frame --
    /// how the Esc hint's click and the Escape key are driven without
    /// reaching into egui's private input state.
    fn run_item_list_with_events(
        items: &[VaultItem],
        search: &mut String,
        before_frame: impl FnOnce(&egui::Context),
        events: Vec<egui::Event>,
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
        crate::card_mark::install_logo_policy(&ctx, crate::card_mark::LogoPolicy::off());
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut selected = None;
        let icons = IconCache::default();
        let mut visible = Vec::new();
        let mut draw = |ctx: &egui::Context, search: &mut String, raw: egui::RawInput| {
            ctx.run_ui(raw, |ui| {
                draw_item_list(
                    ui,
                    Some(items),
                    &[],
                    &SidebarFilter::All,
                    search,
                    &mut selected,
                    &icons,
                    false,
                    &mut visible,
                    None,
                    false,
                    // This harness is not about the open gesture; the slide's arming is
                    // covered by `the_open_gesture_tests` and the pin in `vault_window::mod`.
                    &mut false,
                                &std::collections::HashSet::new(),
                                &RowPeople::default(),
                                "All items",
                                false,
                            );
            })
        };
        let _ = draw(&ctx, search, input());
        before_frame(&ctx);
        let mut raw = input();
        raw.events = events;
        let output = draw(&ctx, search, raw);
        (output, ctx)
    }







    fn escape_key() -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }]
    }






    #[test]
    fn escape_is_ignored_when_the_search_field_never_had_focus() {
        // WHY THIS MATTERS BEYOND THE FIELD. Escape is already bound in this
        // window: `folder_modal::draw_folder_edit_modal` cancels on it, and it
        // runs LATER in the frame than this function does. The field's Escape
        // is therefore gated on the field's own focus, and reads the key
        // without consuming it, so the modal's binding can never be swallowed.
        let items = [an_item("Ledgerline")];
        let mut search = "ledger".to_string();
        let (_, _) = run_item_list_with_events(&items, &mut search, |_| {}, escape_key());
        assert_eq!(
            search, "ledger",
            "Escape cleared the search field without it ever having focus -- the folder modal's \
             own Escape binding runs later in the frame and would be shadowed"
        );
    }


}

#[cfg(test)]
mod move_error_band_tests {
    //! The inline "that move did not happen" band.
    //!
    //! The user's explicit choice for a failed drag-to-folder was "revert in
    //! the UI and show an inline error", the alternative being a row left
    //! looking moved. The revert is `vault_window::mod`'s
    //! `move_item_into_folder`, tested there; this is the "show" half.
    //!
    //! It has its own harness rather than a tenth field on `row_tile_tests`'
    //! `Menu`, so that no pre-existing test's setup had to be touched to add
    //! it.
    use super::*;
    use crate::theme;

    const PANE_WIDTH: f32 = 390.0;
    const PANE_HEIGHT: f32 = 700.0;
    /// Deliberately names an item that is NOT in the list below it: the
    /// assertions find the band's text by matching against this string, and a
    /// row title that was also a substring of it would be counted as part of
    /// the message.
    /// The message under test. It names an item that is deliberately NOT in
    /// the list below it: the assertions recover the band's text by matching
    /// painted galleys against this string, and a row title that was also a
    /// substring of it would be counted as part of the message.
    const MESSAGE: &str = "Couldn't move \"Ledgerline\" -- the vault backend refused the write.";
    /// The one item the list holds, named so it cannot collide with
    /// [`MESSAGE`].
    const ROW: &str = "Vantage";

    fn login(name: &str) -> VaultItem {
        VaultItem {
            id: name.to_string(),
            name: name.into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    struct Painted {
        texts: Vec<(String, egui::Rect)>,
        rects: Vec<(egui::Rect, egui::Color32)>,
        action: ItemListAction,
    }

    fn walk(shape: &egui::Shape, p: &mut Painted) {
        match shape {
            egui::Shape::Text(text) => p.texts.push((
                text.galley.text().to_string(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Rect(rect) => p.rects.push((rect.rect, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, p);
                }
            }
            _ => {}
        }
    }

    /// Real frames of `draw_item_list` with `move_error` set, returning what
    /// the last one painted and returned.
    fn paint(
        items: &[VaultItem],
        move_error: Option<&str>,
        frames: Vec<Vec<egui::Event>>,
    ) -> Painted {
        let ctx = egui::Context::default();
        let screen =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(PANE_WIDTH, PANE_HEIGHT));
        let input = || egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        // Two throwaway frames so `theme::apply`'s font set is live.
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        crate::card_mark::install_logo_policy(&ctx, crate::card_mark::LogoPolicy::off());
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut selected = None;
        let mut search = String::new();
        let icons = IconCache::default();
        let mut visible = Vec::new();
        let mut action = ItemListAction::None;
        let mut draw = |ctx: &egui::Context, raw: egui::RawInput| {
            ctx.run_ui(raw, |ui| {
                action = draw_item_list(
                    ui,
                    Some(items),
                    &[],
                    &SidebarFilter::All,
                    &mut search,
                    &mut selected,
                    &icons,
                    false,
                    &mut visible,
                    move_error,
                    false,
                    // This harness is not about the open gesture; the slide's arming is
                    // covered by `the_open_gesture_tests` and the pin in `vault_window::mod`.
                    &mut false,
                                &std::collections::HashSet::new(),
                                &RowPeople::default(),
                                "All items",
                                false,
                            );
            })
        };
        let _ = draw(&ctx, input());
        let mut output = None;
        for events in frames {
            output = Some(draw(&ctx, egui::RawInput { events, ..input() }));
        }
        let output = output.unwrap_or_else(|| draw(&ctx, input()));

        let mut painted = Painted { texts: Vec::new(), rects: Vec::new(), action };
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        painted
    }

    /// The message as it was actually painted, joined back together -- egui
    /// may lay a wrapped sentence out as several galleys.
    fn painted_message(p: &Painted) -> String {
        p.texts
            .iter()
            .map(|(text, _)| text.as_str())
            .filter(|text| MESSAGE.contains(*text) && !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_band_paints_the_whole_message_and_nothing_shorter() {
        // WRAPPED, NOT TRUNCATED. Every message this shows is a sentence
        // explaining a refusal or a failure; a truncated explanation is worse
        // than none, and truncation is exactly what the rows below it
        // deliberately do.
        let p = paint(&[login(ROW)], Some(MESSAGE), Vec::new());
        assert_eq!(
            painted_message(&p).split_whitespace().collect::<Vec<_>>(),
            MESSAGE.split_whitespace().collect::<Vec<_>>(),
            "the band did not paint the message it was given"
        );
        assert!(
            !p.texts.iter().any(|(text, _)| text.contains('…')),
            "the message was truncated: {:?}",
            p.texts.iter().map(|(t, _)| t).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_band_paints_the_whole_message_inside_the_pane() {
        // The defect the assertion above could not see, and did not: laid out
        // in a right-to-left row a `Label` is handed an unbounded width and
        // never wraps, so the sentence was painted COMPLETE and entirely off
        // the right edge of a 390pt pane. Checking the text alone said it was
        // fine. This checks where it landed.
        let p = paint(&[login(ROW)], Some(MESSAGE), Vec::new());
        let mut lines = 0;
        for (text, rect) in &p.texts {
            if !MESSAGE.contains(text.as_str()) || text.trim().is_empty() {
                continue;
            }
            lines += 1;
            assert!(
                rect.right() <= PANE_WIDTH + 0.5,
                "{text:?} was painted out to x={} on a {PANE_WIDTH}pt pane",
                rect.right()
            );
        }
        assert!(lines > 0, "no part of the message was painted at all");
        // The dismiss glyph rides the same layout and is placed AFTER the
        // message, so a message that took the whole width would push it off
        // the edge -- which is what `GLYPH_LANE` is subtracted for.
        let dismiss = p
            .texts
            .iter()
            .find(|(text, _)| text == "✕")
            .expect("the dismiss glyph was never painted")
            .1;
        assert!(
            dismiss.right() <= PANE_WIDTH + 0.5,
            "the dismiss glyph was pushed out to x={} on a {PANE_WIDTH}pt pane",
            dismiss.right()
        );
    }

    #[test]
    fn no_band_is_painted_when_there_is_nothing_to_say() {
        let p = paint(&[login(ROW)], None, Vec::new());
        assert!(
            !p.texts.iter().any(|(text, _)| text.contains("Couldn't move")),
            "a band was painted with no error set"
        );
        assert_eq!(p.action, ItemListAction::None);
    }

    #[test]
    fn clicking_the_band_dismisses_it() {
        // An explanation the user cannot get rid of ends up sitting over the
        // list describing a gesture three gestures ago.
        let items = [login(ROW)];
        let opened = paint(&items, Some(MESSAGE), Vec::new());
        let at = opened
            .texts
            .iter()
            .find(|(text, _)| MESSAGE.contains(text.as_str()) && text.len() > 5)
            .expect("the message was never painted")
            .1
            .center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let p = paint(
            &items,
            Some(MESSAGE),
            vec![
                vec![egui::Event::PointerMoved(at), button(true)],
                // egui resolves a click on the frame the button is released.
                vec![button(false)],
            ],
        );
        assert_eq!(p.action, ItemListAction::DismissMoveError);
    }

    #[test]
    fn an_inline_move_error_does_not_change_the_row_pitch_beneath_it() {
        // VIRTUALIZATION. The band is allocated ABOVE the list frame, so it
        // takes its height off the scroll area rather than out of it:
        // `show_rows` still reads `item_spacing.y` from the ui that frame
        // gives it. What the band may change is how many rows fit; what it
        // may not change is the PITCH they sit at, because that is what
        // `show_rows` scrolls by.
        let items: Vec<VaultItem> = (0..100).map(|i| login(&format!("Item {i:03}"))).collect();
        let with_band = paint(&items, Some(MESSAGE), Vec::new());
        // Each row found by its 44pt avatar and boxed around it: a v2 row
        // paints no tile of its own until it is hovered or selected.
        let tiles: Vec<egui::Rect> = with_band
            .rects
            .iter()
            .filter(|(rect, _)| (rect.width() - AVATAR_SIZE).abs() < 0.5 && (rect.height() - AVATAR_SIZE).abs() < 0.5)
            .map(|(rect, fill)| {
                let full = PANE_WIDTH - 2.0 * LIST_PADDING;
                (egui::Rect::from_center_size(rect.center(), egui::vec2(full, ROW_TILE_HEIGHT)), *fill)
            })
            .collect::<Vec<_>>()
            .iter()
            .filter(|(rect, _)| {
                // THE ONE tile width. This filter used to accept a second,
                // `SCROLLBAR_WIDTH`-narrower width for the scrolling case --
                // and a 100-row list scrolls, so it was the narrow arm that
                // matched here. The bar no longer takes width from the
                // content (`row_tile_tests::the_tiles_keep_one_width_
                // whether_or_not_the_list_can_scroll`), so the narrow arm now
                // matches NOTHING and is exactly the kind of filter that
                // makes a test unfalsifiable. The `tiles.len() > 3` guard
                // below is what keeps this honest either way.
                let full = PANE_WIDTH - 2.0 * LIST_PADDING;
                (rect.width() - full).abs() < 0.5
                    && (rect.height() - ROW_TILE_HEIGHT).abs() < 0.5
            })
            .map(|(rect, _)| *rect)
            .collect();
        assert!(tiles.len() > 3, "expected a list of rows under the band, got {}", tiles.len());
        for pair in tiles.windows(2) {
            let gap = pair[1].top() - pair[0].bottom();
            assert!(
                (gap - ROW_GAP).abs() < 0.5,
                "rows sit {gap}pt apart under the band, expected {ROW_GAP} -- the virtualized \
                 pitch and the painted pitch have diverged"
            );
        }
    }
}

/// **The three ways this pane can have no rows**, which until now all painted
/// the same blank rectangle.
///
/// The distinction is not academic: a user can reach all three in one session
/// -- select Trash (loading), watch it come back empty (empty), type into the
/// search box (no matches) -- and the pane said nothing at any point.
#[cfg(test)]
mod list_placeholder_tests {
    use super::{list_placeholder, ListPlaceholder};

    /// The state the report is about: an on-demand list that has been asked
    /// for and has not answered yet. `/list/object/items` is 3.46s cold.
    #[test]
    fn an_unanswered_list_says_it_is_loading() {
        assert_eq!(
            list_placeholder(false, false, 0, false),
            Some(ListPlaceholder::Loading),
            "a list that has not been fetched yet and has not failed is still in flight, and \
             must say so rather than look like an empty vault"
        );
    }

    /// The positive control for every negative below, and for the feature as a
    /// whole: rows on screen means no placeholder at all. A build that always
    /// returned `Some(Loading)` satisfies the test above and fails here.
    #[test]
    fn a_list_with_rows_draws_no_placeholder() {
        assert_eq!(
            list_placeholder(true, false, 12, false),
            None,
            "there are rows to draw; a placeholder would be painted over a populated list"
        );
    }

    /// And it stays `None` while a fetch is still in flight, so a list that
    /// already has something on screen is never replaced by a spinner.
    #[test]
    fn rows_on_screen_outrank_a_fetch_still_in_flight() {
        assert_eq!(list_placeholder(false, false, 3, false), None);
    }

    /// A real vault with nothing in this scope -- an empty Trash, a folder
    /// with nothing filed in it.
    #[test]
    fn an_answered_empty_list_says_it_is_empty_rather_than_loading() {
        assert_eq!(
            list_placeholder(true, false, 0, false),
            Some(ListPlaceholder::Empty),
            "the answer arrived and it was \"nothing\"; a spinner here would never resolve"
        );
    }

    /// The search box narrowed a scope that does have contents. Distinct from
    /// `Empty` because the way out is different -- clear the search.
    #[test]
    fn a_search_that_matches_nothing_says_so_rather_than_claiming_the_scope_is_empty() {
        assert_eq!(
            list_placeholder(true, false, 0, true),
            Some(ListPlaceholder::NoMatches),
            "the user typed something; \"Nothing here yet\" would blame the vault for the \
             search box"
        );
    }

    /// **A failed fetch gets no spinner.** `!fetched` alone cannot tell "in
    /// flight" from "gave up", and a spinner that can never resolve is worse
    /// than the blank pane it replaced. The failure has its own channel: the
    /// inline notice band above the list, which also carries the retry.
    #[test]
    fn a_failed_fetch_does_not_spin_forever() {
        assert_eq!(
            list_placeholder(false, true, 0, false),
            None,
            "this list has no answer because the fetch FAILED. A spinner would claim a fetch \
             is running when none is, and nothing would ever take it down -- `AuxList` does \
             not retry on its own. The inline notice band says what happened and carries the \
             retry."
        );
    }

    /// The same, with a search active: the failure still outranks it. There is
    /// nothing that could have matched.
    #[test]
    fn a_failed_fetch_outranks_the_search_wording_too() {
        assert_eq!(list_placeholder(false, true, 0, true), None);
    }

    /// Every state has its own words. Written out rather than derived from the
    /// enum, so a message silently becoming another state's fails here instead
    /// of agreeing with whatever it is handed.
    #[test]
    fn each_state_says_a_different_thing() {
        assert_eq!(ListPlaceholder::Loading.message(), "Loading…");
        assert_eq!(ListPlaceholder::Empty.message(), "Nothing here yet");
        assert_eq!(
            ListPlaceholder::NoMatches.message(),
            super::nothing_matches()
        );
    }
}

/// The placeholder as the user meets it: painted into a real `egui::Context`
/// and read back off the frame, because none of it is observable from
/// `list_placeholder` alone -- a correct decision that the draw site ignores
/// is the blank pane all over again.
#[cfg(test)]
mod list_placeholder_paint_tests {
    use super::*;
    use crate::theme;

    const PANE_WIDTH: f32 = 390.0;

    /// `true` for `mod NAME {`, `pub mod NAME {` and `pub(crate) mod NAME {`,
    /// and for nothing else. The same shape `foreground.rs` walks with,
    /// deliberately exact rather than a `starts_with`: a whole module written
    /// on one line is not a module opener as far as this walk is concerned.
    fn is_module_opener(line: &str) -> bool {
        let t = line.strip_prefix("pub(crate) ").unwrap_or(line);
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("mod ") else {
            return false;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            return false;
        };
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    }

    /// **`vault_window::mod`'s source with its gated test modules cut out**,
    /// and how many were cut. A copy of `foreground.rs`'s walk (`ec74c74`);
    /// there is no test-only module the two could share without adding a `mod`
    /// declaration to a file this change has no business touching.
    ///
    /// A line that is exactly `#[cfg(test)]` followed immediately by a
    /// column-0 module opener starts a skip that runs to the next column-0
    /// `}` -- inside a module every item is indented, so that brace is the
    /// module's own. A walk rather than a split at the first gate -- though
    /// **measured, that makes no difference to `mod.rs` today**: below its
    /// first gate (line 5636 of 14189) the 373 non-blank lines outside a
    /// gated module are all doc comments belonging to those modules, because
    /// `mod.rs`'s own `nothing_but_gated_test_modules_lives_below_the_guards_cut`
    /// forbids anything else. This guard should not rest on that other
    /// guard's continued existence, and this is the shape `foreground.rs` and
    /// `settings.rs` already read other modules with.
    ///
    /// **Line-ending agnostic on purpose.** `lines()` strips a trailing
    /// carriage return, so every comparison here reads the same on this
    /// repository's CRLF working tree and on an LF checkout of its LF blobs.
    fn production_half(source: &str) -> (String, usize) {
        let mut kept: Vec<&str> = Vec::new();
        let mut cut = 0usize;
        let mut gated = false;
        let mut skipping = false;
        for line in source.lines() {
            if skipping {
                if line == "}" {
                    skipping = false;
                }
                continue;
            }
            if gated && is_module_opener(line) {
                // The `#[cfg(test)]` line itself was pushed on the previous
                // turn; it belongs to the module being cut.
                kept.pop();
                skipping = true;
                cut += 1;
                gated = false;
                continue;
            }
            gated = line.trim() == "#[cfg(test)]";
            kept.push(line);
        }
        assert!(
            !skipping,
            "a test module was opened and never closed by a column-0 brace, so the rest of the \
             file was dropped and every needle counted over this reads nothing"
        );
        (kept.join("\n"), cut)
    }

    fn an_item(name: &str) -> VaultItem {
        VaultItem {
            id: name.to_string(),
            name: name.into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// One real frame of `draw_item_list`, at the item pane's own width, with
    /// the two inputs `run_item_list` fixes and this module has to vary: the
    /// unfetched `None` list, and a failed fetch.
    fn painted_pane(items: Option<&[VaultItem]>, search: &str, fetch_failed: bool) -> Vec<String> {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(PANE_WIDTH, 700.0),
            )),
            ..Default::default()
        };
        // Two throwaway frames so `theme::apply`'s font set is live -- the
        // same reason every other painted-output harness in this crate runs
        // them.
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        crate::card_mark::install_logo_policy(&ctx, crate::card_mark::LogoPolicy::off());
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut search = search.to_string();
        let mut selected = None;
        let icons = IconCache::default();
        let mut visible = Vec::new();
        let mut draw = |search: &mut String| {
            ctx.run_ui(input(), |ui| {
                draw_item_list(
                    ui,
                    items,
                    &[],
                    &SidebarFilter::All,
                    search,
                    &mut selected,
                    &icons,
                    false,
                    &mut visible,
                    None,
                    fetch_failed,
                    // This harness is not about the open gesture; the slide's arming is
                    // covered by `the_open_gesture_tests` and the pin in `vault_window::mod`.
                    &mut false,
                                &std::collections::HashSet::new(),
                                &RowPeople::default(),
                                "All items",
                                false,
                            );
            })
        };
        let _ = draw(&mut search);
        let output = draw(&mut search);

        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        texts
    }

    /// **The control every negative below is worth nothing without**: a
    /// populated list paints its row and none of the three messages.
    #[test]
    fn a_populated_list_paints_its_rows_and_no_placeholder() {
        let texts = painted_pane(Some(&[an_item("Ledgerline")]), "", false);
        assert!(
            texts.iter().any(|t| t == "Ledgerline"),
            "control: the row itself is painted. Painted: {texts:?}"
        );
        for state in [
            ListPlaceholder::Loading,
            ListPlaceholder::Empty,
            ListPlaceholder::NoMatches,
        ] {
            assert!(
                !texts.iter().any(|t| t == state.message()),
                "a populated list painted {:?} over its own rows. Painted: {texts:?}",
                state.message()
            );
        }
    }

    /// The report's state, painted: an unfetched list says it is loading
    /// instead of showing an empty box.
    #[test]
    fn an_unfetched_list_paints_the_loading_message() {
        let texts = painted_pane(None, "", false);
        assert!(
            texts
                .iter()
                .any(|t| t == ListPlaceholder::Loading.message()),
            "an unfetched list painted no loading message, so it is the blank pane this work \
             is about. Painted: {texts:?}"
        );
    }

    /// A fetched, empty list says it is empty -- and specifically does NOT say
    /// it is loading, which is the untruth a single "no rows" placeholder
    /// would have shipped.
    #[test]
    fn an_empty_list_paints_the_empty_message_and_not_the_loading_one() {
        let texts = painted_pane(Some(&[]), "", false);
        assert!(
            texts.iter().any(|t| t == ListPlaceholder::Empty.message()),
            "an empty list painted nothing at all. Painted: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|t| t == ListPlaceholder::Loading.message()),
            "an empty list claims to be loading, and nothing would ever take that down. \
             Painted: {texts:?}"
        );
    }

    /// A search that matches nothing, over a list that does have contents.
    #[test]
    fn a_search_matching_nothing_paints_the_no_matches_message() {
        let texts = painted_pane(Some(&[an_item("Ledgerline")]), "zzzzz", false);
        assert!(
            texts
                .iter()
                .any(|t| t == ListPlaceholder::NoMatches.message()),
            "a search matching nothing painted an empty pane. Painted: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t == ListPlaceholder::Empty.message()),
            "a search matching nothing blamed the vault for being empty. Painted: {texts:?}"
        );
    }

    /// The spinner-that-never-resolves guard, painted: a failed fetch leaves
    /// the pane to the inline notice band above it.
    #[test]
    fn a_failed_fetch_paints_no_loading_message() {
        let texts = painted_pane(None, "", true);
        assert!(
            !texts
                .iter()
                .any(|t| t == ListPlaceholder::Loading.message()),
            "a failed fetch is spinning. Nothing is running and nothing will land, so that \
             spinner stays for the rest of the session. Painted: {texts:?}"
        );
    }

    /// **The window must hand this pane the failure fact, and the right one.**
    /// `list_placeholder`'s failure arm is unreachable if `vault_window::mod`
    /// passes a constant, and WRONG if it passes `notice.is_some()` -- that
    /// band also carries Generate and Move failures, neither of which says
    /// anything about whether this list's fetch is still running.
    #[test]
    fn the_window_passes_this_lists_own_failure_and_not_the_bands() {
        let raw = include_str!("mod.rs");

        // **The presence pin reads the production half; the absence pin below
        // reads the whole file.** Deliberately two different sources, because
        // the two pins fail in opposite directions:
        //
        // * `contains(aux_error...)` is a PRESENCE pin, and over a whole file
        //   that is the LOOSE side. `mod.rs` carries fifty gated test modules
        //   (measured), any one of which could spell this needle in a fixture
        //   and hold the guard green after `draw_item_list`'s real call site
        //   had stopped passing the row's own error. That is the drift this
        //   test exists for.
        // * `!contains(notice...)` is a ZERO-count pin, and over a whole file
        //   that is the STRICT side: a fixture spelling it SHOULD fire, since
        //   here a false alarm is cheap and an escape is the defect. Same
        //   judgement `foreground.rs`'s `only_one_window...` was left alone
        //   on. So it is left raw, on purpose, not by omission.
        let (source, cut) = production_half(raw);
        assert!(
            cut > 0,
            "no gated test module was cut out of `vault_window::mod`, so the presence pin below \
             is still satisfiable by a fixture in that file rather than by its code"
        );

        // Positive control on the cut itself -- neither `contains` below
        // proves the walk did anything, and a walk that cut too much would
        // make the presence pin fire for the wrong reason while a walk that
        // cut nothing would leave it as loose as before.
        let interleaved = concat!(
            "fn draw() { list(aux_error", ".is_some(), ..); }\n",
            "#[cfg(test)]\n",
            "mod fixtures {\n",
            "    const SAMPLE: &str = \"aux_error", ".is_some(),\";\n",
            "}\n",
            "fn draw_later() { let _ = SURVIVOR; }\n"
        );
        assert_eq!(
            interleaved.matches(concat!("aux_error", ".is_some(),")).count(),
            2,
            "control: the fixture no longer spells the needle, so the cut below proves nothing"
        );
        let (cut_fixture, cuts) = production_half(interleaved);
        assert_eq!(cuts, 1, "the walk did not find the gated test module");
        assert_eq!(
            cut_fixture.matches(concat!("aux_error", ".is_some(),")).count(),
            1,
            "the walk did not remove the occurrence inside the test module"
        );
        assert!(
            cut_fixture.contains("SURVIVOR"),
            "the walk threw away production below the test module, which is exactly what a \
             split at the first test gate would have done"
        );
        // **Measured, so the shape is not oversold.** A split at `mod.rs`'s
        // first gate would in fact read the same thing TODAY: that file's own
        // `nothing_but_gated_test_modules_lives_below_the_guards_cut` already
        // forbids production below its first gate, and planting some there to
        // prove otherwise fires THAT guard rather than this one. The walk is
        // used anyway because this guard should not be resting on another
        // file's guard continuing to exist, and because it is the one shape
        // `foreground.rs` and `settings.rs` already use -- `main.rs`, which
        // has no such guard, really does interleave.

        let needle = concat!("aux_error", ".is_some(),");
        assert!(
            source.contains(needle),
            "`vault_window::mod` no longer hands `draw_item_list` this row's own \
             `AuxList::error`. If it passes a constant, the failure arm is dead and a failed \
             Trash fetch spins forever; if it passes `notice.is_some()`, an unrelated \
             Generate failure suppresses a spinner for a fetch that really is running."
        );
        assert!(
            !raw.contains(concat!("notice", ".is_some(),")),
            "control: the band's own \"is anything on screen\" is not what is passed"
        );
    }
}

#[cfg(test)]
mod keyboard_selection_tests {
    //! **Arrow-key selection**, and the three things about it that a test
    //! written against a plain `Vec` would miss entirely: that the keys walk
    //! the list AS DISPLAYED, that a row the virtualizer is not drawing this
    //! frame still ends up on screen, and that none of it happens behind a
    //! modal.
    //!
    //! The frame tests below drive real frames of `draw_item_list` and read
    //! back `visible_ids` -- the ids of the rows `show_rows` actually drew --
    //! because that is the only thing in this file that can tell "the
    //! selection moved" apart from "the selection moved somewhere nobody can
    //! see". Each of them first asserts the state it is about to change, so a
    //! test that could not distinguish the two fails here rather than passing
    //! quietly.
    use super::*;
    use crate::theme;

    const PANE_WIDTH: f32 = 390.0;
    const PANE_HEIGHT: f32 = 700.0;

    fn an_item(name: &str) -> VaultItem {
        VaultItem {
            id: name.to_string(),
            name: name.into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    fn raw(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(PANE_WIDTH, PANE_HEIGHT),
            )),
            events,
            ..Default::default()
        }
    }

    fn key(key: egui::Key) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }]
    }

    /// A modal's scrim, drawn by **the function every one of them calls**.
    ///
    /// It used to be a COPY of `folder_modal`'s, "copied rather than called,
    /// so this module needs none of that modal's state" -- and the copy went
    /// stale the day the real scrims moved off `Order::Foreground`, leaving
    /// this test asserting that a gate it had blinded still worked. That is
    /// the same hazard, one level down, that put six hand-written scrims in
    /// this crate on the wrong layer together; see `theme::SCRIM_ORDER`.
    ///
    /// Named with `folder_modal`'s own id, so if the id there changes,
    /// `every_modal_scrim_in_the_crate_is_named_here` is what fails.
    fn draw_a_modal_scrim(ctx: &egui::Context) {
        crate::theme::modal_scrim(ctx, egui::Area::new(egui::Id::new("folder-edit-scrim")));
    }

    /// A live item pane across frames: the selection and the scroll offset
    /// both persist, which is the whole point -- a keyboard step is a step
    /// from wherever the last frame left the list.
    struct Pane {
        ctx: egui::Context,
        items: Vec<VaultItem>,
        search: String,
        selected: Option<String>,
        /// The ids `show_rows` DREW on the last frame -- not the ids that
        /// matched the filter.
        drawn: Vec<String>,
        /// Whether a modal's scrim is drawn over the pane, after it, exactly
        /// where `vault_window::mod` draws its modals.
        modal: bool,
        /// Draw a second text field (the rail's New folder input, say).
        other_field: bool,
    }

    impl Pane {
        fn new(items: Vec<VaultItem>) -> Self {
            let ctx = egui::Context::default();
            // Two throwaway frames so `theme::apply`'s font set is live --
            // the same reason the header-strip harness above runs them.
            let _ = ctx.run_ui(raw(Vec::new()), |_ui| {});
            theme::apply(&ctx);
            crate::card_mark::install_logo_policy(&ctx, crate::card_mark::LogoPolicy::off());
            let _ = ctx.run_ui(raw(Vec::new()), |_ui| {});
            let mut pane = Pane {
                ctx,
                items,
                search: String::new(),
                selected: None,
                drawn: Vec::new(),
                modal: false,
                other_field: false,
            };
            pane.frame(Vec::new());
            pane
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let ctx = self.ctx.clone();
            let icons = IconCache::default();
            let mut drawn = Vec::new();
            let items = &self.items;
            let search = &mut self.search;
            let selected = &mut self.selected;
            let modal = self.modal;
            let other_field = self.other_field;
            let mut other_text = String::new();
            let _ = ctx.run_ui(raw(events), |ui| {
                if other_field {
                    ui.add(egui::TextEdit::singleline(&mut other_text).id(egui::Id::new("other-field")));
                }
                // The search field lives in the v2 top bar now, drawn by the
                // window above the panes; so does it here, with the same
                // `search` string, so focus and typing reach it as they do in
                // the app.
                let bar = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(PANE_WIDTH, crate::vault_window::top_bar::HEIGHT),
                );
                let pill = crate::vault_window::top_bar::SyncPill::from_v1(theme::TEXT_GHOST, "Sync");
                crate::vault_window::top_bar::draw(
                    ui,
                    bar,
                    search,
                    crate::vault_window::top_bar::search_placeholder(),
                    &pill,
                    false,
                    false,
                    |_| {},
                );
                draw_item_list(
                    ui,
                    Some(items),
                    &[],
                    &SidebarFilter::All,
                    search,
                    selected,
                    &icons,
                    false,
                    &mut drawn,
                    None,
                    false,
                    // This harness is not about the open gesture; the slide's arming is
                    // covered by `the_open_gesture_tests` and the pin in `vault_window::mod`.
                    &mut false,
                                &std::collections::HashSet::new(),
                                &RowPeople::default(),
                                "All items",
                                false,
                            );
                // AFTER the pane, where the window really draws them -- so the
                // gate is being tested through the one-frame-late
                // `is_visible` path it actually runs on, and not through a
                // scrim conveniently shown first.
                if modal {
                    draw_a_modal_scrim(ui.ctx());
                }
            });
            self.drawn = drawn;
        }

        fn press(&mut self, k: egui::Key) {
            self.frame(key(k));
        }

        fn drew(&self, id: &str) -> bool {
            self.drawn.iter().any(|drawn| drawn == id)
        }
    }

    fn three() -> Vec<VaultItem> {
        vec![an_item("Alpha"), an_item("Bravo"), an_item("Charlie")]
    }

    // ---- the decision, on its own --------------------------------------

    #[test]
    fn nothing_to_select_is_not_a_selection() {
        for key in [ListNavKey::Up, ListNavKey::Down, ListNavKey::Home, ListNavKey::End] {
            assert_eq!(next_selection(&[], None, key), None, "{key:?}");
            assert_eq!(next_selection(&[], Some("gone"), key), None, "{key:?}");
        }
    }

    #[test]
    fn with_nothing_selected_down_takes_the_first_and_up_takes_the_last() {
        let ids = ["a", "b", "c"];
        assert_eq!(next_selection(&ids, None, ListNavKey::Down), Some(0));
        assert_eq!(next_selection(&ids, None, ListNavKey::Up), Some(2));
    }

    #[test]
    fn the_ends_stop_rather_than_wrap() {
        // The decision this file makes, and the one a reader is most likely
        // to expect the other way round -- see `next_selection`'s doc for the
        // argument. Both ends, so neither direction can quietly grow a wrap.
        let ids = ["a", "b", "c"];
        assert_eq!(next_selection(&ids, Some("c"), ListNavKey::Down), Some(2));
        assert_eq!(next_selection(&ids, Some("a"), ListNavKey::Up), Some(0));
        // ...and the step that is not at an end really does move, so the two
        // assertions above are about the ends and not about a function that
        // never moves anything.
        assert_eq!(next_selection(&ids, Some("a"), ListNavKey::Down), Some(1));
        assert_eq!(next_selection(&ids, Some("c"), ListNavKey::Up), Some(1));
    }

    #[test]
    fn home_and_end_go_to_the_ends_from_anywhere() {
        let ids = ["a", "b", "c"];
        for from in [None, Some("a"), Some("b"), Some("c")] {
            assert_eq!(next_selection(&ids, from, ListNavKey::Home), Some(0), "{from:?}");
            assert_eq!(next_selection(&ids, from, ListNavKey::End), Some(2), "{from:?}");
        }
    }

    #[test]
    fn a_selection_that_is_no_longer_on_screen_counts_as_none() {
        // The detail pane goes on showing an item after the search box has
        // narrowed it out of the list. Treating that as "selected" would make
        // Down a no-op with no row highlighted anywhere.
        let ids = ["a", "b", "c"];
        assert_eq!(next_selection(&ids, Some("z"), ListNavKey::Down), Some(0));
        assert_eq!(next_selection(&ids, Some("z"), ListNavKey::Up), Some(2));
    }

    // ---- the scroll arithmetic -----------------------------------------

    #[test]
    fn a_row_already_in_the_viewport_is_not_scrolled_to() {
        // A minimal scroll: no forced offset at all, so a keyboard step
        // inside the visible window leaves the list exactly where it was.
        let pitch = ROW_TILE_HEIGHT + ROW_GAP;
        assert_eq!(scroll_offset_for_row(0, 0.0, 10.0 * pitch), None);
        assert_eq!(scroll_offset_for_row(5, 0.0, 10.0 * pitch), None);
    }

    #[test]
    fn a_row_above_the_viewport_scrolls_its_top_into_view() {
        let pitch = ROW_TILE_HEIGHT + ROW_GAP;
        assert_eq!(scroll_offset_for_row(3, 10.0 * pitch, 10.0 * pitch), Some(3.0 * pitch));
    }

    #[test]
    fn a_row_below_the_viewport_scrolls_by_exactly_what_it_overhangs() {
        // The next row down from a full viewport moves the list by ONE pitch
        // and no more -- the difference between "the list follows the
        // selection" and "the list jumps".
        let pitch = ROW_TILE_HEIGHT + ROW_GAP;
        let viewport = 10.0 * pitch;
        let offset =
            scroll_offset_for_row(10, 0.0, viewport).expect("row 10 is below a 10-row viewport");
        assert_eq!(offset, 10.0 * pitch + ROW_TILE_HEIGHT - viewport);
        assert!(offset > 0.0 && offset < pitch, "one row's worth, not a jump: {offset}");
    }

    // ---- real frames ----------------------------------------------------

    #[test]
    fn down_selects_the_first_row_as_displayed_and_not_the_first_in_the_vault() {
        // THE OBVIOUS BUG THIS FEATURE HAS. `Bravo` is the second item in the
        // vector and the ONLY row on screen; a step that walked `items`
        // instead of the filtered list would select `Alpha`, which is not
        // drawn at all.
        let mut pane = Pane::new(three());
        pane.search = "bravo".to_string();
        pane.frame(Vec::new());
        assert_eq!(pane.drawn, vec!["Bravo".to_string()], "the search must leave one row");
        assert_eq!(pane.selected, None, "nothing is selected before the key");
        pane.press(egui::Key::ArrowDown);
        assert_eq!(
            pane.selected.as_deref(),
            Some("Bravo"),
            "Down selected an item that is not on screen -- the walk is following the vault's \
             own vector rather than the list as displayed"
        );
    }

    #[test]
    fn down_then_up_walks_the_displayed_list_and_stops_at_the_top() {
        let mut pane = Pane::new(three());
        pane.press(egui::Key::ArrowDown);
        assert_eq!(pane.selected.as_deref(), Some("Alpha"));
        pane.press(egui::Key::ArrowDown);
        assert_eq!(pane.selected.as_deref(), Some("Bravo"));
        pane.press(egui::Key::ArrowUp);
        assert_eq!(pane.selected.as_deref(), Some("Alpha"));
        pane.press(egui::Key::ArrowUp);
        assert_eq!(pane.selected.as_deref(), Some("Alpha"), "Up wrapped off the top of the list");
    }

    #[test]
    fn end_puts_the_last_row_on_screen_although_the_virtualizer_was_not_drawing_it() {
        // **THE REQUIREMENT MOST LIKELY TO HALF-LAND.** `show_rows` only ever
        // draws the rows in the viewport, so moving the selection to row 199
        // without moving the scroll offset leaves it selected and invisible
        // -- and every assertion about `selected_id` alone still passes.
        //
        // The pre-assertion is what makes the post-assertion mean anything:
        // row 199 really is NOT drawn before the key.
        let items: Vec<VaultItem> = (0..200).map(|i| an_item(&format!("Item {i:03}"))).collect();
        let mut pane = Pane::new(items);
        assert!(
            !pane.drew("Item 199"),
            "the last row is already drawn without scrolling, so this test cannot see the \
             difference it exists to check: drawn {:?}",
            pane.drawn
        );
        pane.press(egui::Key::End);
        assert_eq!(pane.selected.as_deref(), Some("Item 199"));
        assert!(
            pane.drew("Item 199"),
            "End selected the last row and left it off screen -- the scroll offset did not \
             follow the selection out of the drawn range. Drawn: {:?}",
            pane.drawn
        );
        // ...and back, the other way, which is the same failure mirrored.
        pane.press(egui::Key::Home);
        assert_eq!(pane.selected.as_deref(), Some("Item 000"));
        assert!(pane.drew("Item 000"), "Home left the first row off screen: {:?}", pane.drawn);
    }

    #[test]
    fn stepping_down_past_the_bottom_of_the_viewport_brings_the_next_row_in() {
        // The one-row-at-a-time version of the test above: the selection is
        // walked down with the Down key alone until it leaves the rows that
        // were drawn on the first frame, and it is still on screen when it
        // gets there.
        let items: Vec<VaultItem> = (0..200).map(|i| an_item(&format!("Item {i:03}"))).collect();
        let mut pane = Pane::new(items);
        let first_frame = pane.drawn.clone();
        assert!(
            first_frame.len() < 200,
            "the list must really be virtualized: {}",
            first_frame.len()
        );
        for _ in 0..first_frame.len() + 3 {
            pane.press(egui::Key::ArrowDown);
        }
        let landed = pane.selected.clone().expect("something is selected after Down");
        assert!(
            !first_frame.contains(&landed),
            "the walk never left the rows that were drawn on the first frame, so this proves \
             nothing about scrolling: landed on {landed}"
        );
        assert!(
            pane.drew(&landed),
            "the selection walked out of the drawn range and was not scrolled back into it: \
             selected {landed}, drawn {:?}",
            pane.drawn
        );
    }

    #[test]
    fn arrow_keys_never_move_the_selection_behind_a_modal() {
        let mut pane = Pane::new(three());
        // Control first: with no modal up, this exact key moves it. Without
        // this the assertions below pass on a build where the key does
        // nothing at all.
        pane.press(egui::Key::ArrowDown);
        assert_eq!(pane.selected.as_deref(), Some("Alpha"));

        pane.modal = true;
        // The frame the scrim first appears; the gate reads it from the frame
        // after, which is `is_visible`'s last-frame half.
        pane.frame(Vec::new());
        pane.press(egui::Key::ArrowDown);
        assert_eq!(
            pane.selected.as_deref(),
            Some("Alpha"),
            "an arrow key moved the item list's selection while a modal was over it -- the \
             scrim stops the pointer and never sees a key"
        );
        pane.press(egui::Key::End);
        assert_eq!(pane.selected.as_deref(), Some("Alpha"), "End reached the list behind a modal");

        // ...and they come back when the modal goes away, so the gate is a
        // gate and not a permanent off switch.
        pane.modal = false;
        pane.frame(Vec::new());
        pane.frame(Vec::new());
        pane.press(egui::Key::ArrowDown);
        assert_eq!(pane.selected.as_deref(), Some("Bravo"), "the keys did not come back");
    }

    #[test]
    fn the_arrows_reach_the_list_while_the_search_field_has_the_caret() {
        // The point of the feature: type to narrow, then arrow into what is
        // left without touching the mouse. Safe because the field is a
        // `singleline` `TextEdit`, where a vertical arrow is a cursor move
        // within one row -- i.e. nothing. See `nav_key`.
        let mut pane = Pane::new(three());
        let id = search_field_id();
        pane.ctx.memory_mut(|m| m.request_focus(id));
        pane.frame(Vec::new());
        assert!(
            pane.ctx.memory(|m| m.has_focus(id)),
            "the search field does not actually have focus, so this test is not testing the \
             case it is named for"
        );
        pane.press(egui::Key::ArrowDown);
        assert!(
            pane.ctx.memory(|m| m.has_focus(id)),
            "the arrow key moved focus off the search field"
        );
        assert_eq!(pane.selected.as_deref(), Some("Alpha"));
    }

    #[test]
    fn home_and_end_are_left_to_the_caret_while_the_search_field_has_focus() {
        // The half that is NOT free to take: in a focused text field Home and
        // End move the caret to the start and the end of what was typed, and
        // those are the user's.
        let mut pane = Pane::new(three());
        let id = search_field_id();
        pane.ctx.memory_mut(|m| m.request_focus(id));
        pane.frame(Vec::new());
        assert!(pane.ctx.memory(|m| m.has_focus(id)), "the field must have focus");
        pane.press(egui::Key::End);
        assert_eq!(
            pane.selected, None,
            "End moved the list selection while the caret was in the search box"
        );
        pane.press(egui::Key::Home);
        assert_eq!(
            pane.selected, None,
            "Home moved the list selection from inside the search box"
        );

        // ...and it is the FOCUS that gates them, not the keys being dead:
        // surrender it and the same End lands.
        pane.ctx.memory_mut(|m| m.surrender_focus(id));
        pane.frame(Vec::new());
        pane.press(egui::Key::End);
        assert_eq!(
            pane.selected.as_deref(),
            Some("Charlie"),
            "End is dead even outside the field"
        );
    }

    /// **A folder name being typed keeps its arrows and Home/End.** The
    /// rail's New folder input is a second text field; while it (or any field
    /// other than the search) has the keyboard, those keys are the caret's.
    #[test]
    fn the_list_leaves_the_keys_alone_while_another_field_has_the_keyboard() {
        let mut pane = Pane::new(three());
        pane.other_field = true;
        pane.frame(Vec::new());
        pane.ctx.memory_mut(|m| m.request_focus(egui::Id::new("other-field")));
        pane.frame(Vec::new());
        for k in [egui::Key::ArrowDown, egui::Key::ArrowUp, egui::Key::Home, egui::Key::End] {
            pane.press(k);
            assert_eq!(pane.selected, None, "{k:?} moved the list while a folder name was being typed");
        }
    }

    #[test]
    fn typing_still_goes_to_the_search_field() {
        // Nothing here consumes an event, so the field receives everything it
        // always received. **Not type-ahead**: the search box already is the
        // type-ahead, and this asserts it still works rather than adding a
        // second one.
        let mut pane = Pane::new(three());
        let id = search_field_id();
        pane.ctx.memory_mut(|m| m.request_focus(id));
        pane.frame(Vec::new());
        pane.frame(vec![egui::Event::Text("bravo".to_string())]);
        assert_eq!(pane.search, "bravo", "typing no longer reaches the search field");
        pane.press(egui::Key::ArrowDown);
        assert_eq!(pane.selected.as_deref(), Some("Bravo"), "type, then arrow into the result");
    }

    #[test]
    fn a_held_modifier_disqualifies_every_one_of_them() {
        // So a future Ctrl+Home or Shift+Down cannot fire this as well as
        // itself -- the hazard `vault_window::mod`'s `matches_exact` chords
        // were written for.
        let mut pane = Pane::new(three());
        for k in [egui::Key::ArrowDown, egui::Key::ArrowUp, egui::Key::Home, egui::Key::End] {
            pane.frame(vec![egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::CTRL,
            }]);
            assert_eq!(pane.selected, None, "CTRL+{k:?} moved the selection");
        }
        // Control: the same keys with no modifier do move it.
        pane.press(egui::Key::ArrowDown);
        assert_eq!(pane.selected.as_deref(), Some("Alpha"));
    }

    #[test]
    fn enter_is_not_bound_here() {
        // A deliberate non-feature: arrowing onto a row already shows it in
        // the detail pane, so Enter has nothing left to do -- and it is the
        // search field's own `return_key`. Asserted rather than left implicit
        // so adding a meaning is a decision somebody has to take on purpose.
        let mut pane = Pane::new(three());
        pane.press(egui::Key::Enter);
        assert_eq!(pane.selected, None, "Enter selected something");
        pane.press(egui::Key::ArrowDown);
        pane.press(egui::Key::Enter);
        assert_eq!(pane.selected.as_deref(), Some("Alpha"), "Enter moved the selection");
    }

    // ---- the modal list stays honest ------------------------------------

    /// Every `.rs` file under `src/`, as (path, contents).
    ///
    /// Walks the tree rather than reading a list, so a modal added in a new
    /// file is scanned without this test changing. The same shape
    /// `debug_leak_guard` and `send_ui` already use.
    fn crate_sources() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
            for entry in entries {
                let path = entry.expect("cannot read a directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path)
                        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
                    out.push((path.display().to_string(), text));
                }
            }
        }
        out
    }

    #[test]
    fn every_modal_scrim_in_the_crate_is_named_here() {
        // **The guard the gate rests on.** `MODAL_SCRIM_AREAS` is a list of
        // strings, so a modal added tomorrow with a scrim this file has never
        // heard of would leave the arrow keys live behind it -- and nothing
        // about that fails to compile. This walks `src/` for the id every
        // scrim is declared with and demands the two sets match exactly.
        //
        // Matched on the area DECLARATION rather than on the bare id, so
        // this file's own `MODAL_SCRIM_AREAS` is not one of the hits. The
        // needle is assembled by `concat!` for the reason every other
        // source-reading guard in this crate does it -- and this comment does
        // not spell it out, because a comment that did would BE a hit: the
        // walk reads this file too, and the first draft of it failed on a
        // scrim that only ever existed in the sentence describing the test.
        let opener = concat!("Area::new(egui", "::Id::new(\"");
        let mut found: Vec<String> = Vec::new();
        for (file, source) in crate_sources() {
            for (index, _) in source.match_indices(opener) {
                let rest = &source[index + opener.len()..];
                let Some(end) = rest.find('"') else { continue };
                let id = &rest[..end];
                if id.ends_with("-scrim") {
                    assert!(
                        MODAL_SCRIM_AREAS.contains(&id),
                        "`{file}` draws a modal scrim `{id}` that `MODAL_SCRIM_AREAS` does not \
                         name, so the item list's arrow keys are live behind that modal"
                    );
                    found.push(id.to_string());
                }
            }
        }
        // `kit::modal::Dialog`s and `kit::modal::confirm`s name their scrim
        // "<id>-scrim" from the id they are opened with: a literal, or a
        // `const` in the same file. Read off the call sites, so a dialog opened
        // with a new id is found without this test changing.
        let dialog = concat!("Dialog::new", "(");
        let confirm = concat!("confirm", "(");
        for (file, source) in crate_sources() {
            if file.replace('\\', "/").ends_with("kit/modal.rs") {
                continue;
            }
            let source = source.replace("\r\n", "\n");
            let mut ids: Vec<String> = Vec::new();
            for (at, _) in source.match_indices(dialog) {
                let rest = &source[at + dialog.len()..];
                // A modeless card draws no scrim, so it names none.
                let chain = &rest[..rest.find(';').unwrap_or(rest.len()).min(400)];
                if chain.contains(concat!(".modeless", "()")) {
                    continue;
                }
                ids.push(rest[..rest.find(',').unwrap_or(0)].trim().to_string());
            }
            for (at, _) in source.match_indices(confirm) {
                if source[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let rest = source[at + confirm.len()..].trim_start();
                let Some(rest) = rest.strip_prefix("ctx,") else { continue };
                let rest = rest.trim_start();
                ids.push(rest[..rest.find(',').unwrap_or(0)].trim().to_string());
            }
            for id in ids {
                let value = if let Some(literal) = id.strip_prefix('"') {
                    literal.trim_end_matches('"').to_string()
                } else {
                    let decl = format!("const {id}: &str = \"");
                    let Some(at) = source.find(&decl) else { continue };
                    let rest = &source[at + decl.len()..];
                    rest[..rest.find('"').expect("a closed literal")].to_string()
                };
                let scrim = format!("{value}-scrim");
                assert!(
                    MODAL_SCRIM_AREAS.contains(&scrim.as_str()),
                    "`{file}` draws a dialog whose scrim `{scrim}` `MODAL_SCRIM_AREAS` does not name"
                );
                found.push(scrim);
            }
        }
        found.sort();
        found.dedup();
        let mut named: Vec<String> = MODAL_SCRIM_AREAS.iter().map(|s| s.to_string()).collect();
        named.sort();
        assert_eq!(
            found, named,
            "`MODAL_SCRIM_AREAS` names a scrim no file draws any more -- a stale entry is a gate \
             that will never fire and a reader's wrong picture of which modals exist"
        );
        // The walk really found something, so an empty `found` cannot make
        // the equality above vacuous on a broken matcher.
        assert!(found.len() >= 5, "the source walk found only {found:?}");
    }
}


#[cfg(test)]
mod v2_list_tests {
    //! Spec 3.1's list pane, as drawn: the header, the rows, their tags, the
    //! empty and offline states, and the entrance.
    use super::*;
    use crate::kit::testing::{self, click_at, Painted};
    use crate::kit::tokens::Palette;
    use crate::vault_bridge::{CardData, LoginData};
    use eframe::egui::{vec2, Color32, Rect};
    use std::collections::HashSet;

    const P: Palette = Palette::LIGHT;
    const SCREEN: egui::Vec2 = egui::vec2(390.0, 2000.0);

    fn login(name: &str, username: &str) -> VaultItem {
        VaultItem {
            id: name.to_string(),
            name: name.into(),
            login: Some(LoginData {
                username: Some(username.to_string()),
                password: None,
                totp: None,
                uris: vec![],
                other: serde_json::Map::new(),
            }),
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            fields: vec![],
            item_type: Some(1),
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    fn card(name: &str, number: &str) -> VaultItem {
        VaultItem {
            item_type: Some(3),
            login: None,
            card: Some(CardData { number: Some(zeroize::Zeroizing::new(number.into())), ..Default::default() }),
            ..login(name, "")
        }
    }

    fn with_app(item: VaultItem) -> VaultItem {
        let m = crate::app_match::AppMatch {
            process: "Ledgerline.exe".to_string(),
            title: String::new(),
            hosted: false,
            path: r"C:\Apps\Ledgerline\Ledgerline.exe".to_string(),
            args: String::new(),
            sequence: String::new(),
            trigger: crate::app_match::TriggerMode::Prompt,
        };
        crate::vault_bridge::with_app_match(&item, &m)
    }

    fn with_totp(mut item: VaultItem) -> VaultItem {
        item.login.as_mut().unwrap().totp = Some(zeroize::Zeroizing::new("JBSWY3DPEHPK3PXP".into()));
        item
    }

    /// What one frame of the list is drawn with.
    struct List<'a> {
        items: &'a [VaultItem],
        filter: SidebarFilter,
        search: &'a str,
        selected: Option<&'a str>,
        title: &'a str,
        offline: bool,
        shared: HashSet<String>,
    }

    impl<'a> List<'a> {
        fn of(items: &'a [VaultItem]) -> Self {
            Self {
                items,
                filter: SidebarFilter::All,
                search: "",
                selected: None,
                title: "All items",
                offline: false,
                shared: HashSet::new(),
            }
        }

        fn draw(&self, ctx: &egui::Context, time: f64, events: Vec<egui::Event>) -> (Painted, ItemListAction) {
            let mut action = ItemListAction::None;
            let mut search = self.search.to_string();
            let mut selected = self.selected.map(str::to_string);
            let painted = testing::frame(ctx, time, SCREEN, events, |ui| {
                action = draw_item_list(
                    ui,
                    Some(self.items),
                    &[],
                    &self.filter,
                    &mut search,
                    &mut selected,
                    &IconCache::default(),
                    false,
                    &mut Vec::new(),
                    None,
                    false,
                    &mut false,
                    &self.shared,
                    &RowPeople::default(),
                    self.title,
                    self.offline,
                );
            });
            (painted, action)
        }
    }

    fn avatars(p: &Painted) -> Vec<Rect> {
        p.rects
            .iter()
            .filter(|r| r.blur_width == 0.0 && (r.rect.width() - 44.0).abs() < 0.5 && (r.rect.height() - 44.0).abs() < 0.5)
            .map(|r| r.rect)
            .collect()
    }

    #[test]
    fn the_header_names_the_scope_and_counts_it() {
        let items = [login("Ledgerline", "a"), login("Atlas", "b"), login("Vantage", "c")];
        let ctx = testing::ctx();
        let (p, _) = List { title: "Work", ..List::of(&items) }.draw(&ctx, 0.0, vec![]);
        let (_, ink, title) = p.text("Work").expect("the scope's title");
        assert_eq!(*ink, P.ink);
        let (_, count_ink, count) = p.texts.iter().find(|(t, _, _)| t == "3").expect("the count");
        assert_eq!(*count_ink, P.text_ghost);
        assert!(count.left() > title.right(), "the count sits after the title");
        assert!(title.left() >= 24.0 - 0.5 && title.top() >= 24.0 - 6.0, "the header's 24pt padding: {title:?}");

        // The titles themselves, one per scope (spec 3.1's map, and the
        // folder's own name).
        let folders = [Folder { id: "f1".into(), name: "Banking".into(), other: Default::default() }];
        let lists = super::super::sidebar::VaultLists::live_only(&items);
        for (filter, want) in [
            (SidebarFilter::All, "All items"),
            (SidebarFilter::Favorites, "Favorites"),
            (SidebarFilter::Logins, "Logins"),
            (SidebarFilter::Cards, "Cards"),
            (SidebarFilter::SecureNotes, "Secure notes"),
            (SidebarFilter::Folder("f1".into()), "Banking"),
        ] {
            assert_eq!(super::super::sidebar::scope_title(&filter, &folders, lists), want);
        }
    }

    #[test]
    fn searching_retitles_the_header_results() {
        let items = [login("Ledgerline", "a"), login("Atlas", "b"), login("Ledger two", "c")];
        let ctx = testing::ctx();
        let (p, _) = List { title: "Work", search: "ledger", ..List::of(&items) }.draw(&ctx, 0.0, vec![]);
        assert!(p.text(results_title()).is_some(), "searching titles the list Results");
        assert!(p.text("Work").is_none(), "the scope's own name gives way to Results");
        assert!(p.texts.iter().any(|(t, c, _)| t == "2" && *c == P.text_ghost), "Results counts the matches");
    }

    #[test]
    fn rows_are_44_avatar_16_title_14_subtitle() {
        let items = [login("Ledgerline", "a.novak@ledgerline.com"), login("Atlas", "b@atlas")];
        let ctx = testing::ctx();
        let (p, _) = List::of(&items).draw(&ctx, 0.0, vec![]);
        let tiles = avatars(&p);
        assert_eq!(tiles.len(), 2, "one 44pt avatar per row");
        assert!((tiles[1].top() - tiles[0].top() - (ROW_TILE_HEIGHT + ROW_GAP)).abs() < 0.5, "pitch is row + 4");
        assert_eq!(ROW_TILE_HEIGHT, 68.0, "padding 12 around the 44pt avatar");
        let (_, title_ink, title) = p.text("Ledgerline").unwrap();
        assert_eq!(*title_ink, P.ink);
        let (_, sub_ink, sub) = p.text("a.novak@ledgerline.com").unwrap();
        assert_eq!(*sub_ink, P.text_faint);
        assert!(sub.top() > title.bottom() - 1.0, "the subtitle is under the title");
        assert!((title.left() - (tiles[0].right() + 14.0)).abs() < 1.5, "a 14pt gap after the avatar");
        // The type sizes, from the roles the row is set in.
        assert_eq!((tokens::ROW_TITLE.size, tokens::SUBTITLE.size), (16.0, 14.0));
    }

    #[test]
    fn the_selected_row_is_white_with_a_blue_ring() {
        let items = [login("Ledgerline", "a"), login("Atlas", "b")];
        let ctx = testing::ctx();
        let (p, _) = List { selected: Some("Atlas"), ..List::of(&items) }.draw(&ctx, 0.0, vec![]);
        let ringed: Vec<_> = p
            .rects
            .iter()
            .filter(|r| r.fill == P.card && r.stroke.width == 2.0 && r.stroke.color == P.blue)
            .collect();
        assert_eq!(ringed.len(), 1, "exactly the selected row is white with the ring");
        let row = ringed[0].rect;
        assert_eq!(row.height(), ROW_TILE_HEIGHT);
        assert_eq!(ringed[0].corner_radius, CornerRadius::same(14));
        let (_, _, atlas) = p.text("Atlas").unwrap();
        assert!(row.contains(atlas.center()), "the ring is on Atlas's row");
        assert!(p.rects.iter().any(|r| r.blur_width > 0.0 && (r.rect.center().x - row.center().x).abs() < 0.5), "the lift's shadow");
    }

    #[test]
    fn card_rows_wear_their_networks_tile_and_masked_last_four() {
        let items = [card("Household card", "4111111111114821")];
        let ctx = testing::ctx();
        let (p, _) = List::of(&items).draw(&ctx, 0.0, vec![]);
        let tile = avatars(&p)[0];
        let visa = p.rects.iter().find(|r| r.rect == tile).unwrap();
        assert_eq!(visa.fill, Color32::from_rgb(0x1a, 0x1f, 0x71), "Visa's own navy");
        // Visa has a bundled mark: a white picture inside the tile, no label.
        assert!(p.text("VISA").is_none(), "the mark replaces the label");
        let glyph = p
            .rects
            .iter()
            .find(|r| r.rect != tile && tile.contains_rect(r.rect) && r.fill == Color32::WHITE)
            .expect("the network's mark on its tile");
        assert!(glyph.rect.width() <= 36.0 + 0.01 && glyph.rect.height() <= 14.0 + 0.01, "{:?}", glyph.rect);
        assert!(p.text("\u{2022}\u{2022}\u{2022}\u{2022} 4821").is_some(), "the masked last four");
    }

    #[test]
    fn a_network_without_a_bundled_mark_keeps_its_text_tile() {
        let items = [card("Mir card", "2200123456789012")];
        let ctx = testing::ctx();
        let (p, _) = List::of(&items).draw(&ctx, 0.0, vec![]);
        let tile = avatars(&p)[0];
        let ground = p.rects.iter().find(|r| r.rect == tile).unwrap();
        assert_eq!(ground.fill, Color32::from_rgb(0x0f, 0x75, 0x4e), "Mir's own green");
        let (_, ink, label) = p.text("MIR").expect("the label on the tile");
        assert_eq!(*ink, Color32::WHITE);
        assert!(tile.contains(label.center()));
    }

    #[test]
    fn tags_follow_app_2fa_shared_order() {
        let items = [with_totp(with_app(login("Ledgerline", "a.novak")))];
        let ctx = testing::ctx();
        let mut list = List::of(&items);
        list.shared.insert("Ledgerline".into());
        let (p, _) = list.draw(&ctx, 0.0, vec![]);
        let x = |word: &str| p.texts.iter().find(|(t, _, _)| t == word).unwrap_or_else(|| panic!("no {word}")).2.left();
        assert!(x("a.novak") < x("App") && x("App") < x("2FA") && x("2FA") < x("Shared"), "App, 2FA, Shared after the username");
        assert_eq!(p.texts.iter().find(|(t, _, _)| t == "Shared").unwrap().1, Color32::WHITE);
        // And none of them on a plain login.
        let plain = [login("Atlas", "b")];
        let (p, _) = List::of(&plain).draw(&testing::ctx(), 0.0, vec![]);
        for word in ["App", "2FA", "Shared", view_only_chip()] {
            assert!(!p.texts.iter().any(|(t, _, _)| t == word), "a plain login wears {word}");
        }
    }

    #[test]
    fn nothing_matches_says_so_in_the_designs_words() {
        let items = [login("Ledgerline", "a")];
        let ctx = testing::ctx();
        let (p, _) = List { search: "zzz", ..List::of(&items) }.draw(&ctx, 0.0, vec![]);
        let (_, ink, _) = p.text("Nothing matches").expect("the title");
        assert_eq!(*ink, P.ink);
        let (_, sub, _) = p.text("Try a site, an app or a username").expect("the line under it");
        assert_eq!(*sub, P.text_faint);
    }

    #[test]
    fn the_offline_banner_offers_retry() {
        let items = [login("Ledgerline", "a")];
        let ctx = testing::ctx();
        let (p, _) = List::of(&items).draw(&ctx, 0.0, vec![]);
        assert!(p.text(offline_title()).is_none(), "online, no banner");

        let offline = List { offline: true, ..List::of(&items) };
        let (p, _) = offline.draw(&ctx, 0.1, vec![]);
        assert_eq!(p.text("You're offline").expect("the banner").1, P.caution_ink);
        assert!(p.text("Filling still works").is_some());
        let retry = p.text("Retry").expect("Retry").2;
        assert!(p.rects.iter().any(|r| r.fill == P.caution_wash && r.stroke.color == P.caution_edge), "the banner's wash");
        let (_, action) = offline.draw(&ctx, 0.2, click_at(retry.center()));
        assert!(matches!(action, ItemListAction::RetrySync), "Retry asks for a sync");
    }

    #[test]
    fn rows_enter_with_a_stagger_once_per_list_and_then_go_quiet() {
        let items: Vec<VaultItem> = (0..4).map(|i| login(&format!("Item {i}"), "u")).collect();
        let ctx = testing::ctx();
        // The first list is at rest: the splash was its entrance.
        let (first, _) = List::of(&items).draw(&ctx, 0.0, vec![]);
        let rest = avatars(&first);
        let (settled, _) = List::of(&items).draw(&ctx, 0.05, vec![]);
        assert_eq!(avatars(&settled), rest, "the window's first list does not move");

        // A new scope is a new list: its rows rise and fade in, one after another.
        let logins = || List { filter: SidebarFilter::Logins, ..List::of(&items) };
        let (start, _) = logins().draw(&ctx, 1.0, vec![]);
        let (moving, _) = logins().draw(&ctx, 1.1, vec![]);
        let alpha = |p: &Painted, i: usize| p.rects.iter().find(|r| r.rect.size() == vec2(44.0, 44.0) && (r.rect.center().y - rest[i].center().y).abs() < 20.0).map_or(0, |r| r.fill.a()); // an invisible row is culled
        assert!(alpha(&start, 0) < 255, "the first row starts faded");
        assert!(alpha(&moving, 0) > alpha(&moving, 3), "row 3 is behind row 0: {} vs {}", alpha(&moving, 0), alpha(&moving, 3));
        assert!(avatars(&moving)[0].top() > rest[0].top(), "rising from below");
        assert_eq!(moving.repaint_delay, std::time::Duration::ZERO, "moving rows ask for the next frame");

        // Then it stops, and stops asking.
        let _ = logins().draw(&ctx, 2.0, vec![]);
        let (done, _) = logins().draw(&ctx, 2.1, vec![]);
        assert_eq!(avatars(&done), rest, "at rest where the rows belong");
        assert!(done.repaint_delay > std::time::Duration::from_secs(1), "a settled list asks for nothing: {:?}", done.repaint_delay);
    }
}

#[cfg(test)]
mod trash_tests {
    //! Spec C §3.
    use super::*;

    fn trashed(deleted: &str) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": "1", "name": "Old router", "type": 2, "deletedDate": deleted
        }))
        .unwrap()
    }

    #[test]
    fn trashed_rows_say_when_they_were_deleted() {
        let today = super::super::detail::date_days("2026-11-19").unwrap();
        assert_eq!(deleted_line(&trashed("2026-11-19T08:00:00Z"), today).as_deref(), Some("Deleted today"));
        assert_eq!(deleted_line(&trashed("2026-11-18T08:00:00Z"), today).as_deref(), Some("Deleted yesterday"));
        assert_eq!(deleted_line(&trashed("2026-11-07T08:00:00Z"), today).as_deref(), Some("Deleted 12 days ago"));
        let live: VaultItem = serde_json::from_value(serde_json::json!({ "id": "2", "name": "x", "type": 2 })).unwrap();
        assert_eq!(deleted_line(&live, today), None, "a live item says nothing of the kind");
    }

    #[test]
    fn an_empty_trash_says_so() {
        let ctx = crate::kit::testing::ctx();
        let p = crate::kit::testing::frame(&ctx, 0.0, egui::vec2(380.0, 400.0), vec![], draw_trash_empty);
        assert!(p.text(trash_empty()).is_some() && p.text(trash_empty_detail()).is_some());
    }
}

//! The vault window's middle pane: search box, `+ New`, and the virtualized
//! item list (design 4.8 "Item list"). Virtualized the same way
//! `picker_ui`'s lists are (`ScrollArea::show_rows`) -- a real vault can be
//! in the thousands, and laying out every row on every repaint was already
//! a confirmed source of a laggy picker before that fix.

use super::detail::{kind_offers_edit, kind_offers_fill};
use super::detail_edit::assignable_folders;
use super::sidebar::{FilterSource, OutOfVault, SidebarFilter};
use crate::card_brand::{brand_for_number, CardBrand};
use crate::card_mark;
use crate::theme;
use crate::vault_bridge::{Folder, ItemKind, VaultItem};
use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke};
use std::collections::HashMap;

/// Holds loaded favicon textures, keyed by item id. Owned by
/// `vault_window::mod` (Task 9), which populates it from the background
/// favicon loader; this module only ever reads it.
#[derive(Default)]
pub struct IconCache {
    pub textures: HashMap<String, egui::TextureHandle>,
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
    Edit,
    /// The destination folder's id, always a real assignable folder --
    /// see [`move_menu`].
    MoveToFolder(String),
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
pub const MOVE_TO_FOLDER_LABEL: &str = "Move to folder";

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
const EDIT_DISABLED_REASON: &str =
    "Deskwarden cannot edit this item type yet. Open it in the Bitwarden web vault \
     or app to edit it.";

/// Shown instead of a destination list when the vault has no folder that can
/// be assigned to.
const NO_ASSIGNABLE_FOLDERS: &str = "No folders yet";

/// Why the item's own folder is greyed inside the submenu. Kept rather than
/// dropped from the list so the destinations do not reshuffle as items are
/// selected, and so the row doubles as "this is where it lives now".
const ALREADY_IN_THIS_FOLDER: &str = "This item is already in this folder";

/// The Delete entry's two labels. The second is the armed state of
/// `vault_window::mod`'s existing `confirm_click` two-click confirmation --
/// the SAME mechanism and the same wording the detail pane's Delete button
/// uses, deliberately, rather than a second confirmation idiom.
const DELETE_LABEL: &str = "Delete";
const DELETE_CONFIRM_LABEL: &str = "Delete? Click to confirm";

/// The Trash row's permanent delete, in the same two-click shape -- the same
/// mechanism, so there is one confirmation idiom in this menu and not two.
/// The wording says "forever" in both states because that is the difference
/// between this entry and the one above it, and it is not recoverable.
const PURGE_LABEL: &str = "Delete forever";
const PURGE_CONFIRM_LABEL: &str = "Delete forever? Click to confirm";

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
/// `delete_pending` is `vault_window::mod`'s `item_delete_pending` for THIS
/// item; it only changes the Delete entry's wording.
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
    delete_pending: bool,
    source: FilterSource,
) -> Vec<MenuEntry> {
    if let Some(out) = source.out_of_vault() {
        return out_of_vault_entries(out, delete_pending);
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
        entries.push(enabled_command("Copy username", RowCommand::CopyUsername));
    }
    if menu_non_empty(login.and_then(|l| l.password.as_deref()).map(|p| p.as_str())).is_some() {
        entries.push(enabled_command("Copy password", RowCommand::CopyPassword));
        // No reveal and no confirmation before this one, by explicit user
        // decision.
    }
    // `is_some`, NOT the non-empty rule above, and that is deliberate: the
    // row's own "2FA" chip (see `item_row`) and `vault_window::mod`'s TOTP
    // poll gate are both `totp.is_some()`, so anything stricter here would
    // let a row show a 2FA chip whose menu then denies the code exists.
    if login.and_then(|l| l.totp.as_ref()).is_some() {
        entries.push(enabled_command("Copy TOTP", RowCommand::CopyTotp));
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
                "Open website",
                RowCommand::OpenWebsite(url.to_string()),
            ));
        }
    }
    // Present for every kind, enabled only for those the edit form can
    // honestly edit -- see `MenuCommand::enabled` for why this one is greyed
    // rather than hidden.
    let editable = kind_offers_edit(kind);
    entries.push(MenuEntry::Command(MenuCommand {
        label: "Edit".to_string(),
        command: RowCommand::Edit,
        enabled: editable,
        disabled_reason: (!editable).then_some(EDIT_DISABLED_REASON),
    }));
    entries.push(MenuEntry::MoveToFolder(move_menu(item, folders)));
    // Above Delete, below everything else: Archive is the gentler of the two
    // ways to take an item out of the working vault, and the design lists
    // the two rows in that order too.
    entries.push(enabled_command("Archive", RowCommand::Archive));
    entries.push(enabled_command(
        if delete_pending { DELETE_CONFIRM_LABEL } else { DELETE_LABEL },
        RowCommand::Delete,
    ));
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
fn out_of_vault_entries(out: OutOfVault, delete_pending: bool) -> Vec<MenuEntry> {
    match out {
        OutOfVault::Trash => vec![
            enabled_command("Restore", RowCommand::Restore),
            // The two-click confirmation, and the only entry in this file
            // that is not undoable. It uses the SAME `confirm_click`
            // mechanism the ordinary Delete does rather than a second
            // confirmation idiom.
            enabled_command(
                if delete_pending { PURGE_CONFIRM_LABEL } else { PURGE_LABEL },
                RowCommand::PurgeForever,
            ),
        ],
        // One entry, because there is exactly one thing to do with an
        // archived item. "Delete" is deliberately not offered: archiving is
        // the user putting something aside, and the route from aside to gone
        // goes back through the vault, where the ordinary Delete lives with
        // its confirmation.
        OutOfVault::Archive => vec![enabled_command("Unarchive", RowCommand::Unarchive)],
    }
}

/// A plain, clickable entry.
fn enabled_command(label: &str, command: RowCommand) -> MenuEntry {
    MenuEntry::Command(MenuCommand {
        label: label.to_string(),
        command,
        enabled: true,
        disabled_reason: None,
    })
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
/// Note what is **not** here and cannot be: a "No folder" destination.
/// `bw serve` (CLI 2026.7.0) cannot un-file an item at all --
/// `.superpowers/sdd/put-semantics-capture.md` records the controlled run, in
/// which omitting `folderId`, sending `null`, sending `""` and PUTting a fully
/// round-tripped object all left the folder unchanged while a name change in
/// the very same request applied. See `EditDraft::may_unfile`, which withholds
/// the same option in the edit form for the same reason.
pub(super) fn move_menu(item: &VaultItem, folders: &[Folder]) -> MoveMenu {
    let assignable = assignable_folders(folders);
    if assignable.is_empty() {
        return MoveMenu::Empty(NO_ASSIGNABLE_FOLDERS);
    }
    MoveMenu::Targets(
        assignable
            .into_iter()
            .map(|folder| {
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
                    disabled_reason: here.then_some(ALREADY_IN_THIS_FOLDER),
                }
            })
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
    let (singular, plural) = match filter {
        SidebarFilter::All => ("item", "items"),
        SidebarFilter::Favorites => ("favorite", "favorites"),
        // Named after the row, like every other specific noun here
        // (Favorites -> favorites, Logins -> logins). "Search 12 apps" is
        // the scope the sidebar shows as selected, so the two read as one
        // thing rather than the placeholder inventing a second name for it.
        SidebarFilter::Apps => ("app", "apps"),
        SidebarFilter::Logins => ("login", "logins"),
        SidebarFilter::Passkeys => ("passkey", "passkeys"),
        SidebarFilter::Cards => ("card", "cards"),
        SidebarFilter::Identities => ("identity", "identities"),
        SidebarFilter::SecureNotes => ("secure note", "secure notes"),
        SidebarFilter::SshKeys => ("SSH key", "SSH keys"),
        SidebarFilter::Archive => ("item", "items"),
        SidebarFilter::Trash => ("item", "items"),
        SidebarFilter::Folder(_) => ("item", "items"),
        SidebarFilter::Unfiled => ("item", "items"),
    };
    match count {
        Some(count) => format!("Search {count} {}", if count == 1 { singular } else { plural }),
        None => format!("Search {plural}"),
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
            ListPlaceholder::Loading => "Loading…",
            ListPlaceholder::Empty => "Nothing here yet",
            ListPlaceholder::NoMatches => "No items match your search",
        }
    }
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
fn draw_list_placeholder(ui: &mut egui::Ui, placeholder: ListPlaceholder) {
    let available = ui.available_height();
    ui.vertical_centered(|ui| {
        // Roughly half the spinner-plus-label block, so the pair sits centred
        // rather than the spinner alone -- the same arithmetic the window
        // body's spinner uses.
        ui.add_space((available / 2.0 - 30.0).max(0.0));
        if placeholder == ListPlaceholder::Loading {
            ui.add(egui::Spinner::new().size(22.0).color(theme::BLUE));
            ui.add_space(10.0);
        }
        ui.label(theme::semibold(placeholder.message(), 13.0).color(theme::TEXT_SECONDARY));
    });
    if placeholder == ListPlaceholder::Loading {
        ui.ctx().request_repaint_after(super::LOADING_FRAME_INTERVAL);
    }
}

/// The design's avatar/favicon tile: `width: 32px; height: 32px`.
const AVATAR_SIZE: f32 = 32.0;

/// The network mark's height, in points, on a list row.
///
/// [`card_mark::MARK_ROW_HEIGHT`] and not a number of this module's own: that
/// constant is pinned to the height at which the mark's type comes out at
/// [`TITLE_SIZE`], the item name's own size, which is what the owner asked
/// for. A number written here instead would be a second place to change it.
const NETWORK_MARK_HEIGHT: f32 = card_mark::MARK_ROW_HEIGHT;

/// The narrowest the item name's column may be squeezed to before the network
/// mark gives up its place on the row.
///
/// **Not taste -- the truncation budget.** The mark's pill is allocated out of
/// the same row width the name and its `(*9988)` suffix are laid into, so on a
/// pane narrow enough the pill would leave the name a single ellipsis and the
/// suffix nowhere to go, which is the overflow `theme::truncated_galley` was
/// added to stop one release ago. 120pt is roughly the suffix (about 47pt at
/// `TITLE_SIZE`), its gap, and enough name to still be a name.
///
/// At the real pane this never binds: the list is
/// `Panel::exact_size(LIST_WIDTH).resizable(false)` at 390pt, which leaves the
/// title column 301pt, and the widest mark this app sets is `MASTERCARD` at
/// 88pt. It binds only under a test pane, and there the answer is the
/// module's standing one -- draw nothing rather than a mark that costs the row
/// its name. `the_network_mark_yields_to_the_name_on_a_pane_too_narrow_for_
/// both` holds it.
const NETWORK_MARK_MIN_TITLE_ROOM: f32 = 120.0;

/// Allocates and paints the network mark's pill on the row, immediately after
/// the avatar tile; returns the rect it took, or `None` when it stood aside.
///
/// **Beside the tile, never inside it.** The badge used to be anchored into
/// the tile's lower-right corner, over the bank's own artwork. The owner asked
/// for it moved -- "maybe not overlap the icon but place to the right ... then
/// name with last digits" -- and moving it is what let the wordmarks become
/// words: the 32pt tile was the four-character cap, and there is no tile
/// around the mark any more (see `CardBrand::wordmark`).
///
/// **It is ALLOCATED, which is the whole point.** `allocate_exact_size` in the
/// row's left-to-right ui advances the cursor by the pill plus the row's
/// `ROW_GAP_X`, so the `ui.available_width()` the title column is then given
/// -- and therefore the `room` `theme::truncated_galley` truncates the name
/// into -- is already net of the pill. Painting it without allocating would
/// have left the name laying out into room the pill was sitting in.
fn paint_network_mark(ui: &mut egui::Ui, brand: CardBrand) -> Option<egui::Rect> {
    let width = card_mark::mark_width(ui, brand, NETWORK_MARK_HEIGHT);
    // The gap is charged twice on purpose: `item_spacing` puts one between the
    // tile and the pill, and another between the pill and the title column.
    if ui.available_width() - width - 2.0 * ROW_GAP_X < NETWORK_MARK_MIN_TITLE_ROOM {
        return None;
    }
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width, NETWORK_MARK_HEIGHT), Sense::hover());
    Some(card_mark::paint_mark(
        ui,
        brand,
        NETWORK_MARK_HEIGHT,
        egui::Align2::LEFT_TOP,
        rect.left_top(),
    ))
}

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
const ROW_PAD_Y: f32 = 10.0;
const ROW_PAD_X: f32 = 12.0;
const ROW_BORDER: f32 = 1.0;
/// The row's `gap: 11px`, between the avatar, the title column and the badge.
const ROW_GAP_X: f32 = 11.0;
/// The list container's `gap: 6px`.
const ROW_GAP: f32 = 6.0;
/// The list container's `padding: 10px`.
///
/// `pub(crate)` because the **Password health** screen takes this same column
/// over and has to be inset by the same amount: two panes that swap places in
/// one column and each spell their own `10.0` are two numbers that must agree,
/// which is this codebase's standing defect. See
/// `password_health::PANE_PADDING`.
pub(crate) const LIST_PADDING: f32 = 10.0;

/// The search field's own id.
///
/// `theme::search_field` puts this on the `TextEdit` it builds, and
/// `vault_window::mod`'s Ctrl+K asks `request_focus` for it by the same name
/// from outside this file -- so it MUST NOT CHANGE. Written once here rather
/// than at the two places in this module that need it (the field itself, and
/// [`nav_key`]'s Home/End gate), so a rename cannot move one and leave the
/// other reading a dead id that would simply never match.
/// `ctrl_k_still_focuses_the_search_field_after_the_move` is what says the
/// field really carries it.
fn search_field_id() -> egui::Id {
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
/// **Scrims, not the modal cards.** The scrim is the thing all seven have and
/// the thing that MEANS "the window behind this is inert" -- `detail.rs`'s
/// copy toast is also a `Foreground` area and has no scrim, which is exactly
/// the difference. `every_modal_scrim_in_the_crate_is_named_here` walks `src/`
/// and fails if a modal is added with a scrim this list does not name.
const MODAL_SCRIM_AREAS: &[&str] = &[
    "detail-edit-discard-scrim",
    "folder-edit-scrim",
    "launch-confirm-scrim",
    "prefs-modal-scrim",
    "record-import-scrim",
    "record-send-scrim",
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
fn a_modal_is_up(ctx: &egui::Context) -> bool {
    ctx.memory(|m| {
        MODAL_SCRIM_AREAS.iter().any(|name| {
            m.areas()
                .is_visible(&egui::LayerId::new(egui::Order::Foreground, egui::Id::new(*name)))
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
/// `folders` and `delete_pending_id` are both only ever read by the rows'
/// right-click menus: the first supplies "Move to folder"'s destinations, the
/// second is `vault_window::mod`'s `item_delete_pending`, which decides
/// whether that row's Delete entry reads "Delete" or the armed
/// "Delete? Click to confirm". Neither affects what a row paints.
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
pub fn draw_item_list(
    ui: &mut egui::Ui,
    items: Option<&[VaultItem]>,
    folders: &[Folder],
    filter: &SidebarFilter,
    search: &mut String,
    selected_id: &mut Option<String>,
    delete_pending_id: Option<&str>,
    icons: &IconCache,
    visible_ids: &mut Vec<String>,
    move_error: Option<&str>,
    // Whether the fetch that would have filled `items` gave up -- the window's
    // `aux_error` for the row selected right now, which is `AuxList::error`
    // and nothing new. Read by `list_placeholder` alone, and only so that a
    // list which is empty BECAUSE the fetch failed does not get a spinner that
    // can never resolve. See that function.
    fetch_failed: bool,
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
                    // ALWAYS opens the menu; the button itself creates
                    // nothing. That is the user's explicit choice over the
                    // alternative ("+ New" makes a login, a separate caret
                    // opens the list), and it is the reason the click is not
                    // also reported as an action here.
                    //
                    // `Popup::menu` draws into its own `Area`, so nothing in
                    // the closure below allocates in this strip -- which is
                    // what keeps the list beneath it in register with the
                    // fixed pitch `show_rows` virtualizes against. Pinned by
                    // `the_new_menu_does_not_change_the_rows_below_it`.
                    let new = theme::primary_button_matching_field(ui, "+ New");
                    egui::Popup::menu(&new).show(|ui| {
                        // Built FROM `CREATABLE_KINDS`, never from a list
                        // written out here. That array is one of the three
                        // doors keeping `ItemKind::Unknown` -- "a type this
                        // build does not understand" -- unreachable from
                        // creation, and a hand-written list is precisely the
                        // door that would leak: it would keep compiling, and
                        // keep offering whatever it said, after the array
                        // moved on.
                        for kind in super::detail_edit::CREATABLE_KINDS {
                            if ui.button(kind.label()).clicked() {
                                action = ItemListAction::NewItem(kind);
                                ui.close();
                            }
                        }
                        // **The import, below a separator, and NOT a row of
                        // the loop above.** It is not a kind: it makes an
                        // item whose type comes from the payload, not from
                        // this menu, so putting it in `CREATABLE_KINDS`
                        // would have added a fake `ItemKind` to the one
                        // array three doors depend on for keeping
                        // `ItemKind::Unknown` out of creation.
                        //
                        // Here rather than in the titlebar beside "Send a
                        // record" -- see `record_ui::IMPORT_FROM_SEND_LABEL`
                        // for the argument. The short of it: this control is
                        // "make me a new item", and the import is one, while
                        // the send narrows an item already selected.
                        ui.separator();
                        if ui.button(super::record_ui::IMPORT_FROM_SEND_LABEL).clicked() {
                            action = ItemListAction::ImportFromSend;
                            ui.close();
                        }
                    });
                    // "Search 180 logins" -- see `search_hint`, which owns
                    // both the count's source and the per-filter noun.
                    let hint = search_hint(
                        items.map(|items| super::sidebar::count_for(items, filter)),
                        filter,
                    );
                    theme::search_field(
                        ui,
                        search,
                        &hint,
                        "CTRL+K",
                        // Stable id so `Ctrl+K` (wired in
                        // `vault_window::mod`) can request focus on this
                        // field from outside this function. MUST NOT CHANGE.
                        // Spelled once, in `search_field_id` -- the arrow-key
                        // block below has to ask about this same field's
                        // focus, and two literals would be two things to
                        // rename.
                        search_field_id(),
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

    let search_lower = search.to_lowercase();
    let filtered: Vec<&VaultItem> = rows
        .iter()
        .filter(|item| matches_filter(item, filter, &search_lower))
        .collect();

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
            // No rows: say which of the three reasons this is, instead of
            // leaving a blank rectangle that reads the same for all of them.
            // Drawn INSTEAD OF the scroll area rather than inside it -- an
            // empty `show_rows` has no rows to hang anything off, and the
            // reserved scrollbar gutter has nothing to scroll.
            if let Some(placeholder) = list_placeholder(
                items.is_some(),
                fetch_failed,
                filtered.len(),
                !search_lower.trim().is_empty(),
            ) {
                draw_list_placeholder(ui, placeholder);
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
            let list = area
                .show_rows(ui, ROW_TILE_HEIGHT, filtered.len(), |ui, row_range| {
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
                                    delete_pending_id == Some(item.id.as_str()),
                                    icons.textures.get(&item.id),
                                    filter.source(),
                                )
                            })
                            .inner;
                        if outcome.select {
                            *selected_id = Some(item.id.clone());
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

/// Design 2b's row title (`font-size: 13px`) and subtitle
/// (`font-size: 11px`), and the `gap: 2px` between them. Named because
/// [`text_column_height`] has to measure exactly what [`item_row`] then
/// draws; a size stated twice would drift.
const TITLE_SIZE: f32 = 13.0;
const SUBTITLE_SIZE: f32 = 11.0;
const TITLE_GAP_Y: f32 = 2.0;

/// The space between a card row's name and its `(*4545)` suffix. A word gap,
/// not a column gap: the suffix qualifies the name it follows, and the row's
/// own `ROW_GAP_X` (which separates the avatar, the text column and the
/// chips) would read as a third element on the line.
const TITLE_SUFFIX_GAP_X: f32 = 4.0;

/// The exact height the row's one- or two-line text column will lay out to,
/// measured from the fonts it is about to be drawn with.
///
/// Needed because that column has to be ALLOCATED at its own height for
/// design 2b's `align-items: center` to have anything to centre against the
/// avatar -- an allocation that took the row's available height would
/// top-align its content inside it instead, which is the reported defect.
///
/// The two font FAMILIES are restated here rather than shared with the draw
/// below (which builds `RichText` through `theme::bold`/`theme::semibold`,
/// not `FontId`s). What keeps the two in step is
/// `the_title_and_email_are_centred_against_the_avatar_not_hung_from_its_top`
/// and its single-line sibling: a family here that did not match the one
/// drawn would mis-measure the column and shift it off centre.
fn text_column_height(ui: &egui::Ui, username: &str, selected: bool) -> f32 {
    let family = if selected { theme::BOLD } else { theme::SEMIBOLD };
    let title = egui::FontId::new(TITLE_SIZE, egui::FontFamily::Name(family.into()));
    let mut height = ui.ctx().fonts_mut(|f| f.row_height(&title));
    if !username.is_empty() {
        let subtitle = egui::FontId::new(SUBTITLE_SIZE, egui::FontFamily::Proportional);
        height += TITLE_GAP_Y + ui.ctx().fonts_mut(|f| f.row_height(&subtitle));
    }
    height
}

/// The y offset that puts `suffix`'s first baseline on `name`'s, measured
/// from the two galleys rather than written down as a constant.
///
/// `detail.rs`'s `digits_baseline_drop` argument, for the one case here: the
/// name is Archivo (SemiBold or Bold) and the suffix is the subtitle's plain
/// proportional face, so the two have different ascents at the same 13pt.
/// Painted at a shared top the suffix would sit visibly high against the name
/// it qualifies.
fn suffix_baseline_drop(name: &egui::Galley, suffix: &egui::Galley) -> f32 {
    fn first_baseline(galley: &egui::Galley) -> f32 {
        galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first().map(|glyph| row.pos.y + glyph.pos.y))
            .unwrap_or(0.0)
    }
    first_baseline(name) - first_baseline(suffix)
}

/// The title line of a CARD row: the item's name, then `(*4545)`.
///
/// **Laid and painted by hand rather than as two `Label`s in a
/// `ui.horizontal`, and both halves of that are load-bearing.**
///
/// *The box is exactly the NAME galley's height.* The text column above was
/// allocated at [`text_column_height`], which measures the title's own font
/// and nothing else; a line that allocated the taller of two faces would grow
/// that column for card rows only, un-centre them against the avatar, and red
/// `the_title_and_email_are_centred_against_the_avatar_not_hung_from_its_top`
/// -- or worse, pass it, since that guard's items are logins. Painting into
/// the name's own box means a suffix can never change a row's height, which
/// is also what keeps every row exactly one `ROW_TILE_HEIGHT` tall for
/// `show_rows` to virtualize against.
///
/// *The NAME is truncated and the suffix is not.* The suffix's width is taken
/// off the available room FIRST and the name is laid into what is left, so a
/// long name loses its tail and the digits survive. That is the whole point
/// of the suffix: two cards from the same bank have the same name and are
/// told apart only by the four digits, so truncating those instead would
/// leave two identical rows. A single `Label` over one `LayoutJob` of both
/// runs would have done exactly that -- egui truncates at the END of the
/// line.
fn paint_title_with_suffix(ui: &mut egui::Ui, title: RichText, suffix: &str) {
    // The subtitle's typography, on the title's line: the plain proportional
    // face at no named weight, in `TEXT_FAINT`. Deliberately NOT
    // `theme::semibold`/`theme::bold` -- the user asked for the suffix "not
    // in bold", and secondary text on this row already has an answer, which
    // is the username line directly below it. Kept at `TITLE_SIZE` rather
    // than `SUBTITLE_SIZE` because it shares the name's line and its
    // baseline; the lighter weight and the faint ink are what separate it
    // from the name, not a second type size on one line.
    let suffix_galley = egui::WidgetText::from(
        RichText::new(suffix).size(TITLE_SIZE).color(theme::TEXT_FAINT),
    )
    .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
    // The suffix's width comes off FIRST -- see the doc comment above.
    // `theme::truncated_galley` is where the `max(1.0)` room clamp and the
    // choice of `Truncate` over `Wrap` live, shared with the finding rows on
    // the Password health screen, which paint a name into a fixed tile for
    // the same reason.
    let room = ui.available_width() - suffix_galley.size().x - TITLE_SUFFIX_GAP_X;
    let name_galley = theme::truncated_galley(ui, title, room, egui::TextStyle::Body);
    let drop = suffix_baseline_drop(&name_galley, &suffix_galley);
    let width = name_galley.size().x + TITLE_SUFFIX_GAP_X + suffix_galley.size().x;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width, name_galley.size().y), Sense::hover());
    let suffix_x = rect.left() + name_galley.size().x + TITLE_SUFFIX_GAP_X;
    // The fallback colours are never reached -- both galleys carry their own
    // section colour -- but they are the right ones if a future `RichText`
    // here ever drops its `.color()`.
    ui.painter().galley(rect.left_top(), name_galley, theme::INK);
    ui.painter().galley(
        egui::pos2(suffix_x, rect.top() + drop),
        suffix_galley,
        theme::TEXT_FAINT,
    );
}

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
    /// An entry of this row's context menu was chosen this frame.
    command: Option<RowCommand>,
}

fn item_row(
    ui: &mut egui::Ui,
    item: &VaultItem,
    folders: &[Folder],
    selected: bool,
    delete_pending: bool,
    icon: Option<&egui::TextureHandle>,
    // Which list this row was drawn from -- the row's menu is entirely
    // different for a trashed or archived item. See `menu_entries`.
    source: FilterSource,
) -> RowOutcome {
    let username = item.login.as_ref().and_then(|l| l.username.as_deref()).unwrap_or("");
    // Design 2b's two trailing chips. Neither is decorative and neither is
    // invented: "app" is `deskwarden:app-match`, the custom field that makes
    // an item fillable into a native window, and "2FA" is the item's own TOTP
    // seed. Both are answered from the item already in hand -- no extra
    // lookup, and only for the handful of rows `show_rows` hands us.
    //
    // BOTH MAY APPEAR AT ONCE. The design never draws two chips on a row and
    // gives no precedence rule, which is why the "2FA" chip was left out
    // originally; the user has since decided they may sit side by side, "app"
    // first. A fixed-size array rather than a `Vec` because this runs per
    // visible row, per frame.
    //
    // Ordered for a RIGHT-TO-LEFT layout below, so the array is walked in
    // reverse: the last chip pushed is the leftmost drawn, and "app first"
    // means app is the leftmost of the pair in reading order.
    let chips: [Option<&str>; 2] = [
        crate::vault_bridge::extract_app_match(item).is_some().then_some("app"),
        item.login
            .as_ref()
            .and_then(|l| l.totp.as_ref())
            .is_some()
            .then_some("2FA"),
    ];
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
                // The tile is now ONE decision and the mark is another, which
                // is what moving the mark off the tile bought. The tile is the
                // bank's icon when there is one and the name's monogram when
                // there is not -- the same two rungs every non-card row has
                // always had, with no card-shaped third. The network's mark
                // then follows the tile, on the row, for any card whose
                // network this app can name.
                let mark = card_network(item);
                match icon {
                    Some(tex) => {
                        // The SAME box the monogram fallback draws -- filled,
                        // bordered, 8px radius -- with the artwork FILLING it,
                        // clipped to the tile's own corner radius and with the
                        // border re-drawn over the top. `theme::avatar_image`
                        // carries the reasoning, and the history: this was
                        // edge-to-edge, then inset 4pt, and is edge-to-edge
                        // again because the design says the image takes the
                        // tile.
                        let tile = theme::avatar_tile(ui, AVATAR_SIZE, selected);
                        theme::avatar_image(ui, tile, tex, selected);
                    }
                    None => {
                        theme::avatar(ui, &theme::initials(&item.name), AVATAR_SIZE, selected)
                    }
                }
                // The mark's pill, after the tile and before the name. It is
                // allocated, so the room the title column is laid into is
                // already net of it -- see `paint_network_mark`.
                if let Some(brand) = mark {
                    paint_network_mark(ui, brand);
                }
                // The design's title column is `flex: 1` with the chips
                // trailing it. Laid out right-to-left so the chips take their
                // own width off the right edge and the column gets the
                // remainder -- the same trick the toolbar strip above uses
                // for `+ New` and the search field, and the reason neither
                // needs a guessed width.
                //
                // Allocated at EXACTLY the avatar's height, which is what
                // makes design 2b's `align-items: center` work. Left to take
                // the row's available height (which `with_layout` does) the
                // cross-axis extent is unbounded, egui falls back to placing
                // at the top, and the `Align::Center` below has nothing to
                // centre within -- the two-line column then hung 2pt high
                // against the avatar and a single-line row 9pt high. Bounding
                // it here is also free: `ROW_TILE_HEIGHT` is already defined
                // as the avatar plus the row's padding, because `show_rows`
                // has to virtualize against a fixed row height.
                let column = egui::Vec2::new(ui.available_width(), AVATAR_SIZE);
                ui.allocate_ui_with_layout(
                    column,
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                    // Reversed: this ui runs right-to-left, so the FIRST chip
                    // drawn lands furthest right. Walking the array backwards
                    // puts "app" to the left of "2FA".
                    //
                    // The chips are allocated before the title column, so on a
                    // pane too narrow for everything they keep their full size
                    // and the title/subtitle absorb the squeeze by truncating
                    // (both are `Label::truncate`). That is deliberate: a chip
                    // that wrapped, or a title that did, would make one row
                    // taller than `ROW_TILE_HEIGHT` and slide the virtualized
                    // list out of register with the pitch `show_rows` scrolls
                    // by. `two_chips_on_a_narrow_pane_stay_inside_the_tile_
                    // and_squeeze_the_title_instead` pins that at 170pt, a
                    // pane less than half the real one: both chips still sit
                    // inside the tile and the row is still exactly one
                    // `ROW_TILE_HEIGHT` tall. Narrower than the chips plus the
                    // avatar the chips would start to overlap it, but this
                    // pane is `Panel::exact_size(LIST_WIDTH).resizable(false)`
                    // in `vault_window::mod` and cannot get there.
                    for chip in chips.iter().rev().flatten() {
                        row_badge(ui, chip, selected);
                    }
                    // Sized to its OWN content height, not to the available
                    // height: `ui.vertical` would take the full 32 and
                    // top-align inside it, which is the same defect one level
                    // down. With an exact height the parent's `Align::Center`
                    // centres the whole column against the avatar.
                    let text_column = egui::Vec2::new(
                        ui.available_width(),
                        text_column_height(ui, username, selected),
                    );
                    ui.allocate_ui_with_layout(
                        text_column,
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                        ui.spacing_mut().item_spacing.y = TITLE_GAP_Y;
                        let title = if selected {
                            // `font-weight: 700; color: #14307a`.
                            theme::bold(&item.name, TITLE_SIZE).color(theme::BLUE_DEEP)
                        } else {
                            theme::semibold(&item.name, TITLE_SIZE).color(theme::INK)
                        };
                        // Truncated, not wrapped: a name long enough to wrap
                        // ("Remote Desktop — Bastion" is already close) would
                        // make one row taller than every other and slide the
                        // whole virtualized list out of register with the
                        // fixed pitch `show_rows` scrolls by.
                        //
                        // A CARD with four or more digits stored takes the
                        // other branch, which draws the same name followed by
                        // `(*4545)`. Every other row -- including a card with
                        // no number, an empty one, or a fragment shorter than
                        // four digits -- takes this one and is byte for byte
                        // the row it was before.
                        match card_number_suffix(item) {
                            Some(suffix) => paint_title_with_suffix(ui, title, &suffix),
                            None => {
                                ui.add(egui::Label::new(title).truncate());
                            }
                        }
                        if !username.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(username)
                                        .size(SUBTITLE_SIZE)
                                        .color(theme::TEXT_FAINT),
                                )
                                .truncate(),
                            );
                        }
                    });
                });
            });
        });
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
    let response = frame.response.interact(Sense::click_and_drag());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // Only acts on the frame the drag actually starts (see
    // `Response::dnd_set_drag_payload`), so this is not a per-frame write.
    response.dnd_set_drag_payload(DraggedItem {
        id: item.id.clone(),
        folder_id: item.folder_id.clone(),
    });
    if response.dragged() {
        drag_ghost(ui, &item.name);
    }
    // The menu is rendered into egui's own popup layer (a separate `Area`),
    // so nothing here allocates in the row's ui and the fixed row pitch
    // `show_rows` virtualizes against is untouched.
    //
    // WHAT it contains is `menu_entries`' decision and not this closure's --
    // see that function's doc for why a per-kind decision must not live in
    // here.
    let mut command = None;
    response.context_menu(|ui| {
        for entry in menu_entries(item, folders, delete_pending, source) {
            match entry {
                MenuEntry::Command(entry) => {
                    if menu_command(ui, &entry) {
                        command = Some(entry.command);
                    }
                }
                MenuEntry::MoveToFolder(destinations) => {
                    ui.menu_button(MOVE_TO_FOLDER_LABEL, |ui| match destinations {
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
                            ui.add_enabled(false, egui::Button::new(note));
                        }
                    });
                }
            }
        }
    });
    RowOutcome {
        // A right-click selects the row too -- see `RowOutcome::select`.
        select: response.clicked() || response.secondary_clicked(),
        command,
    }
}

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
/// `pub(super)` for [`move_menu`]'s reason: the detail pane's kebab draws the
/// same [`MenuCommand`]s and must grey them, and state their reason, exactly
/// as this does.
pub(super) fn menu_command(ui: &mut egui::Ui, entry: &MenuCommand) -> bool {
    let button = ui.add_enabled(entry.enabled, egui::Button::new(entry.label.as_str()));
    let button = match entry.disabled_reason {
        Some(reason) => button.on_disabled_hover_text(reason),
        None => button,
    };
    if button.clicked() {
        ui.close();
        return true;
    }
    false
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
                MenuEntry::MoveToFolder(_) => MOVE_TO_FOLDER_LABEL.to_string(),
            })
            .collect()
    }

    /// The labels of the entries that are actually clickable.
    fn enabled_labels(entries: &[MenuEntry]) -> Vec<String> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Command(c) => c.enabled.then(|| c.label.clone()),
                // The submenu itself is always openable.
                MenuEntry::MoveToFolder(_) => Some(MOVE_TO_FOLDER_LABEL.to_string()),
            })
            .collect()
    }

    fn move_menu_of(entries: &[MenuEntry]) -> MoveMenu {
        entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::MoveToFolder(menu) => Some(menu.clone()),
                MenuEntry::Command(_) => None,
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

    #[test]
    fn a_full_login_offers_every_entry_in_the_agreed_order() {
        assert_eq!(
            labels(&menu_entries(&full_login(), &[], false, FilterSource::LiveVault)),
            vec![
                "Copy username",
                "Copy password",
                "Copy TOTP",
                "Open website",
                "Edit",
                MOVE_TO_FOLDER_LABEL,
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
                false,
                FilterSource::Trash,
            )),
            vec!["Restore", "Delete forever"]
        );
        assert_eq!(
            labels(&menu_entries(
                &full_login(),
                &[folder("f1", "Work")],
                false,
                FilterSource::Archive,
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
        let live = labels(&menu_entries(&item, &[], false, FilterSource::LiveVault));
        let trashed = labels(&menu_entries(&item, &[], false, FilterSource::Trash));
        assert!(live.contains(&"Delete".to_string()));
        assert!(!trashed.contains(&"Delete".to_string()));
        assert!(!live.contains(&"Restore".to_string()));
        assert!(trashed.contains(&"Restore".to_string()));
    }

    /// The permanent delete is armed by the same two-click confirmation the
    /// ordinary Delete uses, and its wording changes to say so.
    ///
    /// Both states are asserted: a label that read "Delete forever? Click to
    /// confirm" unconditionally would pass a test that only looked at the
    /// armed one, and it would mean the menu asking for confirmation of a
    /// click the user has not made yet.
    #[test]
    fn delete_forever_states_when_it_is_armed() {
        let unarmed = labels(&menu_entries(&full_login(), &[], false, FilterSource::Trash));
        let armed = labels(&menu_entries(&full_login(), &[], true, FilterSource::Trash));
        assert_eq!(unarmed, vec!["Restore", "Delete forever"]);
        assert_eq!(armed, vec!["Restore", "Delete forever? Click to confirm"]);
    }

    #[test]
    fn a_card_offers_no_open_website_but_can_be_edited() {
        // "Open website" is login-only (`detail::kind_offers_fill`) and is
        // ABSENT. Editing a card is offered and enabled -- `apply_to` writes
        // the card object -- which is the user-visible half of the 2026-08-17
        // fix.
        let entries = menu_entries(&of_kind(Some(3)), &[], false, FilterSource::LiveVault);
        assert_eq!(labels(&entries), vec!["Edit", MOVE_TO_FOLDER_LABEL, "Archive", "Delete"]);
        assert_eq!(
            enabled_labels(&entries),
            vec!["Edit", MOVE_TO_FOLDER_LABEL, "Archive", "Delete"]
        );
    }

    #[test]
    fn a_secure_note_offers_the_same_four_as_a_card() {
        let entries = menu_entries(&of_kind(Some(2)), &[], false, FilterSource::LiveVault);
        assert_eq!(labels(&entries), vec!["Edit", MOVE_TO_FOLDER_LABEL, "Archive", "Delete"]);
        assert_eq!(
            enabled_labels(&entries),
            vec!["Edit", MOVE_TO_FOLDER_LABEL, "Archive", "Delete"]
        );
    }

    /// An SSH key is the live case for the greyed entry now: it is creatable
    /// but not editable, because `apply_to` has no arm that writes its keys.
    #[test]
    fn the_greyed_edit_entry_says_why() {
        // Greying without a reason is the failure this is guarding: the user
        // sees the action they came for, unavailable, and no explanation.
        let entries = menu_entries(&of_kind(Some(5)), &[], false, FilterSource::LiveVault);
        let edit = entries
            .iter()
            .find_map(|e| match e {
                MenuEntry::Command(c) if c.command == RowCommand::Edit => Some(c.clone()),
                _ => None,
            })
            .expect("no Edit entry");
        assert!(!edit.enabled);
        assert_eq!(edit.disabled_reason, Some(EDIT_DISABLED_REASON));
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
        assert!(labels(&menu_entries(&full_login(), &[], false, FilterSource::LiveVault))
            .contains(&"Open website".to_string()));
        assert_eq!(
            labels(&menu_entries(&card_with_a_login, &[], false, FilterSource::LiveVault)),
            vec!["Copy username", "Copy password", "Copy TOTP", "Edit", MOVE_TO_FOLDER_LABEL, "Archive", "Delete"],
            "a card carrying a login blob was offered a login-only entry"
        );
    }

    #[test]
    fn edit_follows_kind_offers_edit_for_every_kind() {
        // Drives the predicate the menu itself consumes, so relaxing
        // `kind_offers_edit` enables the entry here without anyone having to
        // remember this file exists -- which is exactly what happened when
        // it was widened to cards, notes and identities.
        for item_type in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(9)] {
            let item = of_kind(item_type);
            let entries = menu_entries(&item, &[], false, FilterSource::LiveVault);
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
        assert!(labels(&menu_entries(&with_seed, &[], false, FilterSource::LiveVault)).contains(&"Copy TOTP".to_string()));
        assert_eq!(
            labels(&menu_entries(&without, &[], false, FilterSource::LiveVault)),
            vec![
                "Copy username",
                "Copy password",
                "Open website",
                "Edit",
                MOVE_TO_FOLDER_LABEL,
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
        assert_eq!(
            labels(&menu_entries(&empty, &[], false, FilterSource::LiveVault)),
            vec!["Edit", MOVE_TO_FOLDER_LABEL, "Archive", "Delete"]
        );
    }

    #[test]
    fn open_website_carries_the_url_the_detail_pane_would_open() {
        let entries = menu_entries(&full_login(), &[], false, FilterSource::LiveVault);
        let opens: Vec<&RowCommand> = entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Command(c) => Some(&c.command),
                MenuEntry::MoveToFolder(_) => None,
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
        let MoveMenu::Targets(targets) = move_menu_of(&menu_entries(&full_login(), &folders, false, FilterSource::LiveVault))
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
        let MoveMenu::Targets(targets) = move_menu_of(&menu_entries(&full_login(), &folders, false, FilterSource::LiveVault))
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
            move_menu_of(&menu_entries(&full_login(), &folders, false, FilterSource::LiveVault)),
            MoveMenu::Empty(NO_ASSIGNABLE_FOLDERS)
        );
    }

    #[test]
    fn the_folder_the_item_already_lives_in_is_greyed_not_dropped() {
        let folders = [folder("f1", "Work"), folder("f2", "Personal")];
        let item = VaultItem { folder_id: Some("f1".into()), ..full_login() };
        let MoveMenu::Targets(targets) = move_menu_of(&menu_entries(&item, &folders, false, FilterSource::LiveVault)) else {
            panic!("the submenu reported no assignable folders when two exist");
        };
        assert_eq!(
            targets.iter().map(|t| (t.label.as_str(), t.enabled)).collect::<Vec<_>>(),
            vec![("Work", false), ("Personal", true)]
        );
        assert_eq!(targets[0].disabled_reason, Some(ALREADY_IN_THIS_FOLDER));
    }

    #[test]
    fn there_is_no_un_file_destination() {
        // `bw serve` (CLI 2026.7.0) cannot clear a folder assignment: the
        // write succeeds and does nothing. Every destination this menu
        // offers must therefore name a real folder.
        let folders = [folder("", "No Folder"), folder("f1", "Work")];
        let MoveMenu::Targets(targets) = move_menu_of(&menu_entries(&full_login(), &folders, false, FilterSource::LiveVault))
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

    #[test]
    fn delete_wears_the_armed_label_while_its_confirmation_is_pending() {
        // The SAME two-click confirmation the detail pane's Delete button
        // uses (`vault_window::mod`'s `confirm_click`), not a second idiom:
        // the first click arms, the label changes, the second confirms.
        assert_eq!(
            labels(&menu_entries(&of_kind(Some(3)), &[], false, FilterSource::LiveVault)).last().unwrap(),
            DELETE_LABEL
        );
        assert_eq!(
            labels(&menu_entries(&of_kind(Some(3)), &[], true, FilterSource::LiveVault)).last().unwrap(),
            DELETE_CONFIRM_LABEL
        );
    }

    #[test]
    fn arming_the_delete_changes_nothing_else_about_the_menu() {
        let folders = [folder("f1", "Work")];
        let armed = menu_entries(&full_login(), &folders, true, FilterSource::LiveVault);
        let idle = menu_entries(&full_login(), &folders, false, FilterSource::LiveVault);
        assert_eq!(labels(&armed).len(), labels(&idle).len());
        assert_eq!(labels(&armed)[..labels(&idle).len() - 1], labels(&idle)[..labels(&idle).len() - 1]);
        assert_eq!(move_menu_of(&armed), move_menu_of(&idle));
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
    use crate::app_match::{AppMatch, TriggerMode, APP_MATCH_FIELD_NAME};
    use crate::theme;
    use crate::vault_bridge::VaultField;
    use eframe::egui::epaint::RectShape;

    const PANE_WIDTH: f32 = 390.0;
    const PANE_HEIGHT: f32 = 700.0;
    /// A row tile on a list that FITS: the pane minus the list frame's
    /// `padding: 10px` on each side.
    const TILE_WIDTH: f32 = PANE_WIDTH - 2.0 * LIST_PADDING;
    /// **There is no second tile width.** The bar is drawn inside the
    /// padding and reserves nothing extra, so a list that scrolls lays its
    /// tiles out exactly like one that fits -- pinned by
    /// `the_tiles_keep_one_width_whether_or_not_the_list_can_scroll`.

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

    /// The same custom field `app::save_app_match` writes and
    /// `vault_bridge::extract_app_match` reads -- built through `AppMatch`'s
    /// own serializer so this cannot drift from the real format.
    fn with_app_match(mut item: VaultItem) -> VaultItem {
        item.fields.push(VaultField {
            name: Some(APP_MATCH_FIELD_NAME.to_string()),
            value: Some(zeroize::Zeroizing::new(
                AppMatch::for_process("ledgerline.exe", TriggerMode::Prompt)
                .to_field_value(),
            )),
            other: serde_json::Map::new(),
        });
        item
    }

    struct Painted {
        rects: Vec<RectShape>,
        texts: Vec<(String, egui::Rect, egui::Color32)>,
        fonts: Vec<(String, egui::FontId)>,
        visible: Vec<String>,
        /// What `draw_item_list` left `selected_id` at. Read by the
        /// right-click tests, which are about exactly that.
        selected: Option<String>,
        /// What `draw_item_list` returned on the measured frame.
        action: ItemListAction,
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
        delete_pending: Option<String>,
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
                delete_pending: None,
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

    /// One real frame with a favicon TEXTURE loaded for every id in `ids` --
    /// the branch `IconCache::default()` can never reach, and the one the
    /// "the favicon fills its tile" report is about.
    fn paint_with_icons(items: &[VaultItem], selected: Option<&str>, ids: &[&str]) -> Painted {
        let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        paint_core(items, selected, 0, PANE_WIDTH, |ctx| {
            let mut icons = IconCache::default();
            for id in &ids {
                icons.textures.insert(
                    id.clone(),
                    ctx.load_texture(
                        format!("favicon-{id}"),
                        egui::ColorImage::filled([16, 16], egui::Color32::RED),
                        egui::TextureOptions::LINEAR,
                    ),
                );
            }
            icons
        }, Menu::none())
    }

    fn paint_core(
        items: &[VaultItem],
        selected: Option<&str>,
        wheel_frames: usize,
        pane_width: f32,
        make_icons: impl FnOnce(&egui::Context) -> IconCache,
        menu: Menu,
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
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut selected_id = selected.map(str::to_string);
        let mut search = String::new();
        let icons = make_icons(&ctx);
        let mut action = ItemListAction::None;
        let Menu { folders, delete_pending, mut frames, filter } = menu;
        let mut draw = |ctx: &egui::Context, input: egui::RawInput, visible: &mut Vec<String>| {
            ctx.run_ui(input, |ui| {
                action = draw_item_list(
                    ui,
                    Some(items),
                    &folders,
                    &filter,
                    &mut search,
                    &mut selected_id,
                    delete_pending.as_deref(),
                    &icons,
                    visible,
                    // This harness predates the inline move-error band and
                    // has no business growing a parameter for it; the band
                    // has its own module below.
                    None,
                    // Nothing failed here, and nothing in this module's
                    // harnesses draws an empty list -- `list_placeholder` is
                    // asserted directly, by `list_placeholder_tests`.
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
            rects: Vec::new(),
            texts: Vec::new(),
            fonts: Vec::new(),
            visible,
            selected: selected_id,
            action,
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

    /// A chip's own filled box, found from the label it contains -- the small
    /// rect wrapping the text, never the row tile.
    fn chip_rect(p: &Painted, label: &str) -> egui::Rect {
        let text = p
            .texts
            .iter()
            .find(|(t, _, _)| t == label)
            .unwrap_or_else(|| panic!("the {label:?} chip was never painted; painted: {:?}", p.texts))
            .1;
        p.rects
            .iter()
            .find(|r| r.rect.contains_rect(text) && r.rect.width() < 60.0)
            .unwrap_or_else(|| panic!("the {label:?} chip's own filled box was never painted"))
            .rect
    }

    fn one_tile(p: &Painted) -> RectShape {
        one_tile_of_width(p, TILE_WIDTH)
    }

    fn one_tile_of_width(p: &Painted, width: f32) -> RectShape {
        let tiles: Vec<RectShape> = p
            .rects
            .iter()
            .filter(|r| {
                !(r.fill == egui::Color32::TRANSPARENT && r.stroke.width == 0.0)
                    && r.blur_width == 0.0
                    && (r.rect.width() - width).abs() < 0.5
                    && (r.rect.height() - ROW_TILE_HEIGHT).abs() < 0.5
            })
            .cloned()
            .collect();
        assert_eq!(
            tiles.len(),
            1,
            "expected exactly one {width}x{ROW_TILE_HEIGHT} row tile; every painted rect was: {:?}",
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
            "the tile filter found {} rects on a frame that reported {} visible rows -- a              filter that matches nothing makes every assertion below vacuous. One fewer is              allowed: the row at the viewport's edge can be clipped out of the paint list              entirely.",
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
                delete_pending: None,
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

    #[test]
    fn the_gap_above_the_first_tile_is_the_same_as_the_gap_beside_it() {
        // THE REPORT, the half nobody had ever measured: "also feels on top
        // is smaller as well". Two rounds of work went into the side gaps
        // while the top was only ever asserted as an ABSOLUTE y coordinate
        // (`STRIP_HEIGHT + LIST_PADDING`), which is a statement about the
        // layout, not about what the eye compares. If the header strip's own
        // ink stopped short of `STRIP_HEIGHT`, or something were painted
        // between the strip and the list, the absolute assertion would stay
        // green while the visible gap shrank.
        //
        // So this measures INK to INK, the same way `clear_space_beside_
        // tiles` measures the sides: from the bottom of the lowest thing
        // painted above the first tile down to that tile's top edge, counting
        // only what actually overlaps the tile's own column. Then it compares
        // that to the left gap MEASURED IN THE SAME FRAME rather than to
        // `LIST_PADDING`, so "the top matches the left" cannot pass by both
        // sides drifting together.
        //
        // Run on a list that fits AND on one that overflows, unscrolled --
        // the vertical gap has nothing to do with the scrollbar, and saying
        // so is the point: the answer must not depend on the state the side
        // gaps do.
        for n in [3usize, 40usize] {
            let items: Vec<VaultItem> = (0..n)
                .map(|i| login(&format!("Item {i:04}"), "a@b.c"))
                .collect();
            // NOT `SETTLE_FRAMES`: that pumps the wheel, and a scrolled list
            // has its first tile clipped by the viewport rather than sitting
            // its padding below the strip.
            let p = paint(&items, None);
            let tile = row_tiles(&p)[0].rect;

            let mut gap_above = tile.top();
            for r in visibly_painted(&p) {
                let shares_the_column =
                    r.right() > tile.left() + 0.5 && r.left() < tile.right() - 0.5;
                if shares_the_column && r.bottom() <= tile.top() + 0.01 {
                    gap_above = gap_above.min(tile.top() - r.bottom());
                }
            }
            let (gap_left, _) = clear_space_beside_tiles(&p, tile);

            assert!(
                (gap_above - gap_left).abs() < 0.51,
                "on a {n}-row list there is {gap_above}pt of clear space above the first \
                 tile and {gap_left}pt beside it. The reader compares those two directly, \
                 so they have to be the same number"
            );
            assert!(
                (gap_above - LIST_PADDING).abs() < 0.51,
                "on a {n}-row list the gap above the first tile is {gap_above}pt, not the \
                 design's {LIST_PADDING}pt -- the list frame's TOP margin is no longer the \
                 same padding as its sides, or something is being painted between the \
                 header strip and the list"
            );
        }
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

    /// The vertical centre of the 32px avatar tile in the FIRST row, absolute
    /// at this pane: the tile's top edge (68) plus its 1px border and 10px
    /// padding (79), plus half the avatar (16).
    const FIRST_ROW_AVATAR_CENTRE_Y: f32 = 95.0;

    #[test]
    fn the_title_and_email_are_centred_against_the_avatar_not_hung_from_its_top() {
        // THE REPORT: "title/email are top-aligned against the 32px image".
        // Design 2b's row is `align-items: center`.
        //
        // The two-line column is SHORTER than the 32px avatar (a 13px line, a
        // 2px gap and an 11px line), so top-aligning it leaves it sitting ~2pt
        // high. Asserted as the column's own mid-line, which is independent of
        // the font metrics that set the two line heights -- and against an
        // ABSOLUTE y, as well as against the avatar's, so this cannot stay
        // green by both moving together.
        let p = paint(&[login("Ledgerline", "a.novak@ledgerline.com")], None);
        let avatar = square(&p, AVATAR_SIZE);
        assert!(
            (avatar.rect.center().y - FIRST_ROW_AVATAR_CENTRE_Y).abs() < 0.01,
            "the harness's geometry moved: the avatar's centre is at y={}, expected \
             {FIRST_ROW_AVATAR_CENTRE_Y}",
            avatar.rect.center().y
        );
        let title = p
            .texts
            .iter()
            .find(|(t, _, _)| t == "Ledgerline")
            .expect("the title")
            .1;
        let email = p
            .texts
            .iter()
            .find(|(t, _, _)| t == "a.novak@ledgerline.com")
            .expect("the subtitle")
            .1;
        let column_centre = (title.top() + email.bottom()) / 2.0;
        assert!(
            (column_centre - FIRST_ROW_AVATAR_CENTRE_Y).abs() < 0.51,
            "the title/subtitle column runs {}..{}, so its centre is at y={column_centre}, but \
             the avatar's is at y={FIRST_ROW_AVATAR_CENTRE_Y} -- the column is hung from the top \
             of the avatar rather than centred against it",
            title.top(),
            email.bottom()
        );
    }

    #[test]
    fn a_row_with_no_username_centres_its_single_line_too() {
        // The positive control's other half: one line, not two. A fix that
        // only centred the two-line case (say, by nudging it down a fixed 2pt)
        // would push a single-line row 2pt BELOW centre, and the two-line
        // assertion above could not tell.
        let mut it = login("Vantage VPN", "");
        it.login = None;
        let p = paint(&[it], None);
        let title = p
            .texts
            .iter()
            .find(|(t, _, _)| t == "Vantage VPN")
            .expect("the title")
            .1;
        assert!(
            (title.center().y - FIRST_ROW_AVATAR_CENTRE_Y).abs() < 0.51,
            "a single-line row's title is centred at y={}, expected the avatar's \
             {FIRST_ROW_AVATAR_CENTRE_Y}",
            title.center().y
        );
    }

    #[test]
    fn a_favicon_fills_its_tile_clipped_to_the_tiles_own_corner_radius() {
        // THREE PASSES OVER ONE ROW, and this test has been on both sides of
        // it. The whole history, because the next person here will be holding
        // one of these and not the other two:
        //
        // 1. The favicon was drawn at the full 32pt (`fit_to_exact_size(32)`)
        //    and the report was "the favicon fills its tile edge-to-edge and
        //    feels too big" -- next to a monogram, whose letters cover about a
        //    third of the same tile.
        // 2. So it was inset 4pt a side: a 24pt image in a 32pt bordered box.
        //    The report on THAT was "icon is not fully taking the rounded
        //    rectangle" -- an icon adrift in a frame.
        // 3. The design is the tiebreaker and it says the image takes the
        //    tile. The inset is gone; `theme::avatar_image` is the rule now.
        //    This test is named for that, having previously been named
        //    `a_favicon_is_inset_inside_its_tile_instead_of_filling_it_edge_
        //    to_edge` -- deliberately reversed, not quietly dropped.
        //
        // FULL BLEED IS NOT THE HARD PART; THE CORNERS ARE. The tile is a
        // rounded rectangle, so an image painted at its bounds with square
        // corners would poke four corners out past the rounding -- a worse
        // result than either report. So three things are asserted, and the
        // corner radius is the one that matters most:
        let p = paint_with_icons(&[login("Ledgerline", "a@b.c")], None, &["Ledgerline"]);
        let tile = square(&p, AVATAR_SIZE);
        assert_eq!(
            tile.fill,
            theme::CANVAS,
            "the favicon's tile must still be drawn the way the monogram's is -- filled, so a \\r
             favicon with transparent margins has the same ground behind it"
        );
        let (at, image) = p
            .rects
            .iter()
            .enumerate()
            .find(|(_, r)| r.brush.is_some())
            .map(|(i, r)| (i, r.clone()))
            .unwrap_or_else(|| {
                panic!(
                    "no textured rect was painted at all, so no favicon was drawn; painted: {:?}",
                    p.rects.iter().map(|r| r.rect).collect::<Vec<_>>()
                )
            });

        // 1. EXACTLY the tile. Not inside it, not a fraction of it -- the same
        //    rect to the same corners.
        assert!(
            (image.rect.min - tile.rect.min).length() < 0.01
                && (image.rect.max - tile.rect.max).length() < 0.01,
            "the favicon was painted at {:?} but its tile is {:?} -- the image is meant to take \\r
             the whole tile",
            image.rect,
            tile.rect
        );
        // ...and the negative, so an inset creeping back in reds here rather
        // than drifting: no shrinkage at all, in either dimension.
        assert!(
            (image.rect.width() - tile.rect.width()).abs() < 0.01
                && (image.rect.height() - tile.rect.height()).abs() < 0.01,
            "the favicon is {}x{} inside a {}x{} tile -- something has re-introduced an inset",
            image.rect.width(),
            image.rect.height(),
            tile.rect.width(),
            tile.rect.height()
        );

        // 2. CLIPPED TO THE TILE'S CURVE. A full-bleed image is only right if
        //    its corners are the tile's corners; square ones would overhang the
        //    rounding on all four. Asserted as the tile's own radius, and
        //    separately as non-zero, because zero is exactly the failure.
        assert_eq!(
            image.corner_radius,
            theme::avatar_corner_radius(AVATAR_SIZE),
            "the favicon is not clipped to the tile's corner radius, so its corners overhang \\r
             the rounded tile"
        );
        assert_eq!(
            image.corner_radius, tile.corner_radius,
            "the favicon and its tile are rounded differently"
        );
        assert!(
            image.corner_radius.nw > 0,
            "the favicon was painted with square corners over a rounded tile"
        );

        // 3. THE BORDER SURVIVES, AND IT IS ON TOP. `StrokeKind::Middle`
        //    straddles the tile's edge, so the border painted UNDER a
        //    full-bleed image keeps only its outer half-pixel. The tile's edge
        //    is what a pale favicon needs in order to read as the same tile the
        //    monograms beside it have, so it is drawn again over the artwork --
        //    which means a 32pt bordered rect must appear AFTER the image.
        let border_over = p.rects.iter().skip(at + 1).find(|r| {
            (r.rect.width() - AVATAR_SIZE).abs() < 0.5
                && r.stroke.width > 0.0
                && r.stroke.color.a() > 0
        });
        let border_over = border_over.unwrap_or_else(|| {
            panic!(
                "no bordered {AVATAR_SIZE}pt rect was painted after the favicon, so the tile's \\r
                 edge is buried under it; painted: {:?}",
                p.rects.iter().map(|r| (r.rect, r.fill, r.stroke)).collect::<Vec<_>>()
            )
        });
        assert_eq!(
            border_over.stroke.color,
            theme::HAIRLINE,
            "the border redrawn over the favicon is not the tile's own unselected border"
        );
        assert!(
            (border_over.rect.min - tile.rect.min).length() < 0.01,
            "the border over the favicon is at {:?}, not on the {:?} tile",
            border_over.rect,
            tile.rect
        );
    }

    #[test]
    fn an_item_with_no_favicon_still_gets_the_monogram_and_no_texture() {
        // The positive control for the test above: `paint_with_icons` really
        // is the thing that puts a texture on the row, so "a textured rect was
        // painted, and here is where" is a statement about the favicon branch
        // and not about something egui draws anyway.
        let p = paint(&[login("Ledgerline", "a@b.c")], None);
        assert!(
            !p.rects.iter().any(|r| r.brush.is_some()),
            "an item with no cached favicon must paint no texture at all"
        );
        assert!(
            p.texts.iter().any(|(t, _, _)| t == "LE"),
            "...and must fall back to the monogram; painted: {:?}",
            p.texts
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

    /// Every textured rect a frame painted, in paint order.
    fn textured(p: &Painted) -> Vec<RectShape> {
        p.rects.iter().filter(|r| r.brush.is_some()).cloned().collect()
    }

    /// Every 32px avatar tile a frame painted, top to bottom.
    ///
    /// De-duplicated because `theme::avatar_tile` paints its fill and its
    /// border as two rects over one geometry.
    fn avatar_tiles(p: &Painted) -> Vec<egui::Rect> {
        let mut v: Vec<egui::Rect> = p
            .rects
            .iter()
            .filter(|r| {
                (r.rect.width() - AVATAR_SIZE).abs() < 0.5
                    && (r.rect.height() - AVATAR_SIZE).abs() < 0.5
            })
            .map(|r| r.rect)
            .collect();
        v.sort_by(|a, b| a.top().total_cmp(&b.top()));
        v.dedup();
        v
    }

    /// Every network mark painted inside `area`, with the word on it: a
    /// `theme::BLUE` ground, and whichever painted text sits inside that
    /// ground. Top to bottom.
    ///
    /// **Identified by what a mark IS, not by where it landed.** The marks
    /// used to be textures and were found by being textured; they are drawn
    /// type on a drawn ground now, and a test that looked for "a small rect in
    /// the corner" would pass for any small rect in the corner.
    ///
    /// Handed a ROW's rect by most callers now that the mark sits beside the
    /// avatar tile rather than inside it -- and handed the avatar tile by the
    /// tests whose claim is that the tile is left alone.
    fn marks_in(p: &Painted, area: egui::Rect) -> Vec<(egui::Rect, String)> {
        let mut v: Vec<(egui::Rect, String)> = p
            .rects
            .iter()
            .filter(|r| r.fill == theme::BLUE && area.contains_rect(r.rect))
            .map(|r| {
                let word = p
                    .texts
                    .iter()
                    .find(|(_, rect, _)| r.rect.contains_rect(*rect))
                    .map(|(t, _, _)| t.clone())
                    .unwrap_or_else(|| {
                        panic!("a mark ground was painted at {:?} with no word on it", r.rect)
                    });
                (r.rect, word)
            })
            .collect();
        v.sort_by(|a, b| a.0.top().total_cmp(&b.0.top()));
        v
    }

    #[test]
    fn a_networks_mark_is_the_networks_own_word() {
        // FIRST, because every badge test below leans on it: the mark really
        // does track the brand, and it says the brand's NAME.
        //
        // THE REPORT this replaced: "VISA icon supposed to be visa and not
        // some Play sign". The marks were seven abstract glyphs on one blue
        // square -- a wedge for Visa, a diamond for Mastercard -- which named
        // no network and did not tell each other apart either. The old version
        // of this test asserted only that two networks were DIFFERENT
        // pictures, which those placeholders satisfied perfectly.
        let p = paint(
            &[
                card_branded("Visa One", "Visa"),
                card_branded("MC One", "Mastercard"),
                card_branded("Visa Two", "visa"),
            ],
            None,
        );
        let rows: Vec<egui::Rect> = row_tiles(&p).iter().map(|r| r.rect).collect();
        assert_eq!(rows.len(), 3, "three card rows");
        let word = |i: usize| {
            let marks = marks_in(&p, rows[i]);
            assert_eq!(marks.len(), 1, "row {i} painted {} marks, expected 1", marks.len());
            marks[0].1.clone()
        };
        // The actual claim, and it is about legible content rather than about
        // two ids differing: each row says its own network.
        assert_eq!(word(0), CardBrand::Visa.wordmark());
        assert_eq!(word(1), CardBrand::Mastercard.wordmark());
        assert_ne!(
            word(0),
            word(1),
            "Visa and Mastercard drew the same word, so the mark names no network"
        );
        // ...and the same network spelled either way is the same word, so the
        // inequality is about the brand and not about a per-row accident.
        assert_eq!(word(0), word(2));
    }

    #[test]
    fn a_card_with_a_bank_icon_wears_its_network_mark_beside_the_tile_and_not_over_it() {
        // THE REPORT: a bank favicon with `VISA` sitting over the tile's
        // lower-right corner -- "maybe not overlap the icon but place to the
        // right ... then name with last digits". This is that arrangement,
        // asserted from painted geometry: icon, then pill, then name.
        let p = paint_with_icons(&[card_branded("BoA Credit", "Visa")], None, &["BoA Credit"]);
        let row = row_tiles(&p)[0].rect;
        let tile = square(&p, AVATAR_SIZE).rect;
        let marks = marks_in(&p, row);
        assert_eq!(marks.len(), 1, "expected exactly the mark on the row: {marks:?}");
        let (mark, word) = marks[0].clone();
        assert_eq!(word, CardBrand::Visa.wordmark(), "the mark names the wrong network");

        // The favicon is still there and still fills the tile, so the tile
        // really is the icon's and this is not the monogram rung.
        assert!(
            textured(&p).iter().any(|r| r.rect == tile),
            "no full-tile favicon was painted: {:?}",
            textured(&p).iter().map(|r| r.rect).collect::<Vec<_>>()
        );

        // **Nothing is drawn inside the tile any more.** The negative half of
        // the report, and the one that would silently come back if a future
        // edit re-added the corner anchor: `marks_in` over the TILE finds
        // nothing.
        assert!(
            marks_in(&p, tile).is_empty(),
            "a mark is still being painted inside the {tile:?} tile: {:?}",
            marks_in(&p, tile)
        );

        // To the RIGHT of the tile, clear of it, one row gap away.
        assert!(
            mark.left() >= tile.right(),
            "the mark at {mark:?} overlaps the {tile:?} tile it is supposed to sit beside"
        );
        assert!(
            (mark.left() - tile.right() - ROW_GAP_X).abs() < 0.51,
            "the mark at {mark:?} is {}pt from the tile's right edge, expected the row's \
             {ROW_GAP_X}pt gap",
            mark.left() - tile.right()
        );
        // Vertically centred against the tile: the row is `align-items:
        // center` and a pill hanging off that baseline is the defect the
        // title column already had to be bounded to avoid.
        assert!(
            (mark.center().y - tile.center().y).abs() < 0.51,
            "the mark at {mark:?} is not centred against the {tile:?} tile"
        );
        assert!(
            (mark.height() - NETWORK_MARK_HEIGHT).abs() < 0.01,
            "the mark is {}pt tall, expected {NETWORK_MARK_HEIGHT}",
            mark.height()
        );
        // And the name follows the pill rather than starting under it.
        let name = p
            .texts
            .iter()
            .find(|(t, _, _)| t == "BoA Credit")
            .map(|(_, r, _)| *r)
            .expect("the item's name was painted");
        assert!(
            name.left() >= mark.right(),
            "the name at {name:?} starts before the mark at {mark:?} ends"
        );
    }

    #[test]
    fn a_card_with_no_bank_domain_takes_the_monogram_tile_and_still_wears_its_mark() {
        // With the mark off the tile, a card with no issuer icon is not a
        // special case any more: it gets the SAME monogram tile every other
        // iconless row gets, and the network is named beside it. That is the
        // whole simplification -- the tile answers "who issued this" and the
        // pill answers "which network", and neither has to stand in for the
        // other.
        let p = paint(
            &[card_branded("BoA Credit", "Visa"), card_branded("MC Credit", "Mastercard")],
            None,
        );
        let rows: Vec<egui::Rect> = row_tiles(&p).iter().map(|r| r.rect).collect();
        let tiles = avatar_tiles(&p);
        assert_eq!(rows.len(), 2);
        assert_eq!(tiles.len(), 2);
        let marks: Vec<(egui::Rect, String)> = rows
            .iter()
            .map(|r| {
                let m = marks_in(&p, *r);
                assert_eq!(m.len(), 1, "exactly one mark belongs on a row");
                m.into_iter().next().unwrap()
            })
            .collect();
        assert_eq!(marks[0].1, CardBrand::Visa.wordmark());
        assert_eq!(marks[1].1, CardBrand::Mastercard.wordmark());
        for (tile, (mark, word)) in tiles.iter().zip(&marks) {
            assert!(
                mark.left() >= tile.right(),
                "{word:?} at {mark:?} overlaps its {tile:?} tile"
            );
            assert!(
                (mark.height() - NETWORK_MARK_HEIGHT).abs() < 0.01,
                "{word:?} is {}pt tall, expected {NETWORK_MARK_HEIGHT}",
                mark.height()
            );
        }
        // The monogram IS what the tile shows -- the rung this used to skip.
        assert!(
            p.texts.iter().any(|(t, _, _)| t == "BC"),
            "the monogram was not drawn, so the tile is still card-special: {:?}",
            p.texts
        );
    }

    /// Every network's mark stays inside its ROW at the real pane width --
    /// `MASTERCARD`, the widest word this app sets, included.
    ///
    /// `card_mark` measures the same seven against an arithmetic budget; this
    /// is the same claim from painted output, so a change to the row's padding
    /// or gap that the arithmetic there does not know about still fails.
    #[test]
    fn every_networks_mark_stays_inside_its_row_at_the_real_pane_width() {
        for brand in crate::card_brand::CARD_BRANDS {
            let p = paint(&[card_branded("No Bank", brand.canonical())], None);
            let row = row_tiles(&p)[0].rect;
            let tile = square(&p, AVATAR_SIZE).rect;
            let marks = marks_in(&p, row);
            assert_eq!(
                marks.len(),
                1,
                "{brand:?} painted {} marks inside its row -- a mark that escaped the row is \
                 invisible to `marks_in` and shows up here as zero",
                marks.len()
            );
            assert_eq!(marks[0].1, brand.wordmark());
            assert!(
                marks[0].0.left() >= tile.right(),
                "{brand:?}'s mark at {:?} overlaps its {tile:?} tile",
                marks[0].0
            );
        }
    }

    /// **The truncation guard, in the form the last release's overflow bug
    /// took.** The pill is allocated out of the same width the name is laid
    /// into, so a name long enough to need truncating must truncate SOONER
    /// with a pill present -- and every glyph of both must still land inside
    /// the row.
    #[test]
    fn a_marked_rows_name_truncates_into_the_room_the_pill_left_it() {
        const LONG: &str = "Bank of America Platinum Rewards Signature Debit";
        let mut marked = card_branded(LONG, "Mastercard");
        marked.card.as_mut().unwrap().number =
            Some(zeroize::Zeroizing::new("5555444433332222".to_string()));
        // The control: the same item, on a network this app cannot name, so it
        // gets no pill and the whole column. Same name, same suffix, same
        // everything else.
        let mut unmarked = marked.clone();
        unmarked.card.as_mut().unwrap().brand = Some("Ledger Coin".to_string());

        let with = paint(&[marked], None);
        let without = paint(&[unmarked], None);
        assert_eq!(marks_in(&with, row_tiles(&with)[0].rect).len(), 1);
        assert!(
            marks_in(&without, row_tiles(&without)[0].rect).is_empty(),
            "the control row drew a mark, so it is not a control"
        );

        let name_of = |p: &Painted| {
            p.texts
                .iter()
                .find(|(t, _, _)| t.starts_with("Bank of"))
                .map(|(t, r, _)| (t.clone(), *r))
                .expect("the name was painted")
        };
        // A truncated galley keeps its ORIGINAL text (that is what
        // `Galley::text` returns), so truncation is read off the laid-out
        // WIDTH rather than off a trailing ellipsis in the string.
        let (_, marked_rect) = name_of(&with);
        let (plain_name, plain_rect) = name_of(&without);
        let full = plain_name.chars().count() as f32;
        assert!(
            plain_rect.width() < full * TITLE_SIZE,
            "the control's name is {}pt wide for {full} characters, which is not truncated at \
             all -- the fixture name is too short to prove anything",
            plain_rect.width()
        );
        // And the pill really cost the name room, rather than the name
        // overflowing into the pill's place.
        let pill = marks_in(&with, row_tiles(&with)[0].rect)[0].0.width();
        assert!(
            // Within one glyph: truncation lands on a character boundary, so
            // the name gives up slightly more or less than the pill took.
            (plain_rect.width() - marked_rect.width() - pill - ROW_GAP_X).abs() < TITLE_SIZE,
            "the name is {}pt wide beside a {pill}pt pill and {}pt wide without one -- a \
             difference of {}pt, where the pill plus the row's {ROW_GAP_X}pt gap is {}. The \
             pill took the wrong amount off the truncation budget, which is the overflow bug \
             returning.",
            marked_rect.width(),
            plain_rect.width(),
            plain_rect.width() - marked_rect.width(),
            pill + ROW_GAP_X
        );
    }

    /// **Ink inside the row, positively and negatively, narrow and wide.**
    ///
    /// The pane really is fixed at `LIST_WIDTH`, but a guard that only ever
    /// sees one width is a guard that pins a coincidence.
    #[test]
    fn a_marked_rows_ink_stays_inside_its_row_at_both_pane_widths() {
        const NARROW: f32 = 170.0;
        let item = |brand: &str| {
            let mut it = card_branded("Bank of America Platinum Debit", brand);
            it.card.as_mut().unwrap().number =
                Some(zeroize::Zeroizing::new("5555444433332222".to_string()));
            it
        };
        for width in [NARROW, PANE_WIDTH] {
            let p = paint_at_width(&[item("Mastercard")], None, width);
            // `row_tiles` is written against the real pane's width, so the
            // narrow frame's row is found by the width it actually has.
            let row = one_tile_of_width(&p, width - 2.0 * LIST_PADDING).rect;
            // Everything painted on the row's own band -- the toolbar above it
            // is not this claim -- must land inside the row horizontally.
            let on_row: Vec<&(String, egui::Rect, egui::Color32)> =
                p.texts.iter().filter(|(_, r, _)| row.y_range().contains(r.center().y)).collect();
            assert!(!on_row.is_empty(), "at a {width}pt pane the row painted no text at all");
            for (text, rect, _) in &on_row {
                assert!(
                    row.expand(0.51).contains_rect(*rect),
                    "at a {width}pt pane, {text:?} was painted at {rect:?}, outside the {row:?} row"
                );
            }
            // The row is still exactly one virtualized pitch tall either way.
            assert!(
                (row.height() - ROW_TILE_HEIGHT).abs() < 0.51,
                "at a {width}pt pane the row is {}pt tall, expected {ROW_TILE_HEIGHT}",
                row.height()
            );
        }
        // The negative: the guard above can actually SEE ink outside a row.
        // Measured against a row deliberately given a rect one third its own
        // width -- if `contains_rect` were vacuously true this would pass too.
        let p = paint_at_width(&[item("Mastercard")], None, PANE_WIDTH);
        let row = row_tiles(&p)[0].rect;
        let clipped = egui::Rect::from_min_size(
            row.min,
            egui::vec2(row.width() / 3.0, row.height()),
        );
        assert!(
            p.texts
                .iter()
                .filter(|(_, r, _)| row.y_range().contains(r.center().y))
                .any(|(_, rect, _)| !clipped.contains_rect(*rect)),
            "no text on the row falls outside a third of it, so the containment check above \
             proves nothing"
        );
    }

    /// **The pill yields to the name rather than crushing it.** See
    /// `NETWORK_MARK_MIN_TITLE_ROOM`: at the real pane this never fires, and a
    /// pane narrow enough to fire it gets no pill instead of a one-ellipsis
    /// name.
    #[test]
    fn the_network_mark_yields_to_the_name_on_a_pane_too_narrow_for_both() {
        let card = card_branded("BoA Debit", "Mastercard");
        // Wide: the pill is there.
        let wide = paint_at_width(std::slice::from_ref(&card), None, PANE_WIDTH);
        assert_eq!(
            marks_in(&wide, row_tiles(&wide)[0].rect).len(),
            1,
            "the real pane must draw the mark"
        );
        // Narrow: it stands aside, and the NAME is what survives.
        const NARROW: f32 = 170.0;
        let narrow = paint_at_width(&[card], None, NARROW);
        let row = one_tile_of_width(&narrow, NARROW - 2.0 * LIST_PADDING).rect;
        assert!(
            marks_in(&narrow, row).is_empty(),
            "a 170pt pane drew a pill it has no room for: {:?}",
            marks_in(&narrow, row)
        );
        assert!(
            narrow.texts.iter().any(|(t, _, _)| t.starts_with("BoA")),
            "the name did not survive either: {:?}",
            narrow.texts
        );
    }

    #[test]
    fn a_brand_this_app_cannot_name_falls_all_the_way_back_to_the_monogram() {
        // No mark, no placeholder, no question mark -- the row looks
        // exactly as it did before any of this existed. The fixture also
        // carries a Visa NUMBER, so this pins the decision the spec makes: a
        // card the user labelled "Ledger Coin" is not quietly relabelled a
        // Visa by its leading digits.
        let mut item = card_branded("Bank Coin", "Ledger Coin");
        item.card.as_mut().unwrap().number =
            Some(zeroize::Zeroizing::new("4111111111111111".to_string()));
        let p = paint(&[item], None);
        assert!(
            p.texts.iter().any(|(t, _, _)| t == "BC"),
            "the monogram must still be drawn; painted: {:?}",
            p.texts
        );
        let row = row_tiles(&p)[0].rect;
        assert!(
            marks_in(&p, row).is_empty(),
            "an unrecognised brand painted a mark: {:?}",
            marks_in(&p, row)
        );
        // Control: the same fixture with a nameable brand DOES paint one, so
        // the emptiness above is about the brand and not about this harness.
        let named = paint(&[card_branded("Bank Coin", "Visa")], None);
        assert_eq!(marks_in(&named, row_tiles(&named)[0].rect).len(), 1);
    }

    #[test]
    fn the_mark_costs_the_row_no_height_at_all() {
        // The mark now takes WIDTH off the row -- that is the point of
        // allocating it -- but it must still cost no HEIGHT, because
        // `ScrollArea::show_rows` virtualizes against a fixed
        // `ROW_TILE_HEIGHT` and one taller row slides the whole list out of
        // register. The pill is 18pt against the tile's 32, so it fits inside
        // the height the row already had; this is what says so.
        let p = paint_with_icons(&[card_branded("BoA Credit", "Visa")], None, &["BoA Credit"]);
        let row = one_tile_of_width(&p, TILE_WIDTH);
        assert!(
            (row.rect.height() - ROW_TILE_HEIGHT).abs() < 0.5,
            "a marked card's row is {}pt tall, expected {ROW_TILE_HEIGHT}",
            row.rect.height()
        );
        let marks = marks_in(&p, row.rect);
        assert_eq!(marks.len(), 1, "the mark is drawn: {marks:?}");
        assert!(
            marks[0].0.height() < AVATAR_SIZE,
            "the pill at {:?} is taller than the tile beside it, so it is what sets the row's \
             height",
            marks[0].0
        );
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

    /// A login carrying a TOTP seed -- design 2b's "2FA" chip.
    fn with_totp(mut item: VaultItem) -> VaultItem {
        item.login
            .as_mut()
            .expect("with_totp is only meaningful on a login")
            .totp = Some(zeroize::Zeroizing::new(
            "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP".to_string(),
        ));
        item
    }

    #[test]
    fn an_item_with_both_an_app_match_and_a_totp_paints_both_chips_side_by_side() {
        // The user's decision, recorded: design 2b shows an "app" chip and a
        // "2FA" chip but never two on one row, and gives no precedence rule.
        // Both may now appear, "app" first.
        let items = [with_totp(with_app_match(login("Ledgerline", "a@b.c")))];
        let p = paint(&items, None);
        let app = chip_rect(&p, "app");
        let totp = chip_rect(&p, "2FA");
        assert!(
            app.right() <= totp.left() + 0.01,
            "both chips paint, but \"app\" at {app:?} is not before \"2FA\" at {totp:?}"
        );
        // Side by side on ONE line, not stacked: a row is a fixed
        // `ROW_TILE_HEIGHT` because `show_rows` virtualizes against it, so a
        // second chip wrapping onto its own line would overflow the tile.
        assert!(
            (app.center().y - totp.center().y).abs() < 0.51,
            "the chips are on different lines: \"app\" at {app:?}, \"2FA\" at {totp:?}"
        );
        let tile = one_tile(&p);
        for (name, chip) in [("app", app), ("2FA", totp)] {
            assert!(
                tile.rect.contains_rect(chip),
                "the {name:?} chip at {chip:?} is outside its row tile {:?}",
                tile.rect
            );
        }
    }

    #[test]
    fn each_chip_appears_exactly_when_its_own_condition_holds() {
        // Four states, all four asserted, so neither chip can be the other's
        // shadow. Every negative here has its positive control in the same
        // table -- an "absent" assertion alone would also pass against a row
        // that painted nothing at all.
        let plain = login("Vantage VPN", "a@b.c");
        for (label, item, want_app, want_totp) in [
            ("neither", plain.clone(), false, false),
            ("app only", with_app_match(plain.clone()), true, false),
            ("totp only", with_totp(plain.clone()), false, true),
            (
                "both",
                with_totp(with_app_match(plain.clone())),
                true,
                true,
            ),
        ] {
            let p = paint(&[item], None);
            let has = |needle: &str| p.texts.iter().any(|(t, _, _)| t == needle);
            assert_eq!(
                has("app"),
                want_app,
                "{label}: expected the \"app\" chip to be present={want_app}; painted: {:?}",
                p.texts
            );
            assert_eq!(
                has("2FA"),
                want_totp,
                "{label}: expected the \"2FA\" chip to be present={want_totp}; painted: {:?}",
                p.texts
            );
        }
    }

    #[test]
    fn the_2fa_chip_takes_the_same_two_colour_treatments_the_app_chip_does() {
        // Design 2b draws the "2FA" chip with the identical metrics and
        // colours as the "app" one -- `font-size: 10px; border-radius: 5px;
        // padding: 2px 6px`, `#605d5d on #f3f2f2` unselected.
        let items = [with_totp(login("Git Host", "anovak"))];
        let unselected = paint(&items, None);
        assert_eq!(text_color(&unselected, "2FA"), theme::TEXT_MUTED);
        assert_eq!(text_font(&unselected, "2FA").size, 10.0);
        let selected = paint(&items, Some("Git Host"));
        assert_eq!(text_color(&selected, "2FA"), theme::BLUE_DEEP);
    }

    #[test]
    fn two_chips_on_a_narrow_pane_stay_inside_the_tile_and_squeeze_the_title_instead() {
        // What happens when the row is too tight. The chips are allocated
        // first (right-to-left), so they keep their full size and the title
        // column absorbs the squeeze by truncating -- which is the behaviour
        // that keeps every row exactly `ROW_TILE_HEIGHT` tall and the
        // virtualized list in register. Asserted at a pane less than half the
        // real one, where the two chips plus the avatar leave almost nothing.
        let items = [with_totp(with_app_match(login(
            "A Very Long Item Name Indeed",
            "someone@example.com",
        )))];
        let p = paint_at_width(&items, None, 170.0);
        let tile = one_tile_of_width(&p, 170.0 - 2.0 * LIST_PADDING);
        for name in ["app", "2FA"] {
            let chip = chip_rect(&p, name);
            assert!(
                tile.rect.contains_rect(chip),
                "on a 170pt pane the {name:?} chip at {chip:?} has escaped its row tile {:?}",
                tile.rect
            );
        }
        assert!(
            (tile.rect.height() - ROW_TILE_HEIGHT).abs() < 0.5,
            "the row grew to {} tall on a narrow pane, which would slide the virtualized list \
             out of register with the pitch `show_rows` scrolls by",
            tile.rect.height()
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
    const MENU_VOCABULARY: [&str; 13] = [
        "Copy username",
        "Copy password",
        "Copy TOTP",
        "Open website",
        "Edit",
        MOVE_TO_FOLDER_LABEL,
        "Archive",
        DELETE_LABEL,
        DELETE_CONFIRM_LABEL,
        "Restore",
        "Unarchive",
        PURGE_LABEL,
        PURGE_CONFIRM_LABEL,
    ];

    fn menu_labels(p: &Painted) -> Vec<String> {
        p.texts
            .iter()
            .map(|(text, _, _)| text.clone())
            .filter(|text| MENU_VOCABULARY.contains(&text.as_str()))
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
        open_menu_with(items, folders, None, row, SidebarFilter::All)
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
        open_menu_with(items, folders, None, row, filter)
    }

    fn open_menu_with(
        items: &[VaultItem],
        folders: Vec<Folder>,
        delete_pending: Option<String>,
        row: usize,
        filter: SidebarFilter,
    ) -> Painted {
        let at = row_centre(items, row);
        paint_core(
            items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu {
                folders,
                delete_pending,
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
        let hover = text_centre(&open_menu(items, folders.clone(), row), MOVE_TO_FOLDER_LABEL);
        let at = row_centre(items, row);
        let mut frames = click_frames(at, egui::PointerButton::Secondary);
        frames.extend((0..4).map(|_| vec![egui::Event::PointerMoved(hover)]));
        paint_core(
            items,
            None,
            0,
            PANE_WIDTH,
            |_| IconCache::default(),
            Menu { folders, delete_pending: None, frames, filter: SidebarFilter::All },
        )
    }

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
        let at = row_centre(items, row);
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
            Menu { folders, delete_pending: None, frames, filter },
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

    /// The rect and colour of one painted run, by its exact text.
    fn painted(p: &Painted, text: &str) -> (egui::Rect, egui::Color32) {
        let run = p
            .texts
            .iter()
            .find(|(t, _, _)| t == text)
            .unwrap_or_else(|| {
                panic!(
                    "{text:?} was never painted; the row painted: {:?}",
                    p.texts.iter().map(|(t, _, _)| t.clone()).collect::<Vec<_>>()
                )
            });
        (run.1, run.2)
    }

    /// The font one painted run was laid out in.
    fn painted_font(p: &Painted, text: &str) -> egui::FontId {
        p.fonts
            .iter()
            .find(|(t, _)| t == text)
            .unwrap_or_else(|| panic!("{text:?} was never painted"))
            .1
            .clone()
    }

    /// Every painted run that looks like the card suffix. Used to assert its
    /// ABSENCE without pinning the exact digits a wrong implementation might
    /// have chosen.
    fn suffix_runs(p: &Painted) -> Vec<String> {
        p.texts
            .iter()
            .map(|(t, _, _)| t.clone())
            .filter(|t| t.starts_with("(*"))
            .collect()
    }

    #[test]
    fn a_cards_row_shows_its_last_four_after_the_name() {
        // The user's request, verbatim: "add *4545 after the name in search
        // results not in bold like: `BoA Credit (*4545)`".
        let p = paint(&[card_numbered("BoA Credit", "4242424242424242")], None);
        let (name, _) = painted(&p, "BoA Credit");
        let (suffix, _) = painted(&p, "(*4242)");
        assert!(
            suffix.left() >= name.right() - 0.01,
            "the suffix runs {}..{} and the name {}..{} -- the suffix is not AFTER the name",
            suffix.left(),
            suffix.right(),
            name.left(),
            name.right()
        );
        // On the name's own line, not below it: the two boxes overlap in y.
        assert!(
            suffix.top() < name.bottom() && name.top() < suffix.bottom(),
            "the suffix sits at y {}..{} and the name at {}..{} -- they are not on one line",
            suffix.top(),
            suffix.bottom(),
            name.top(),
            name.bottom()
        );
    }

    #[test]
    fn the_suffix_is_lighter_than_the_name_it_follows() {
        // "not in bold". The name is Archivo SemiBold in `INK`; the suffix
        // takes the row's OWN secondary typography -- the plain proportional
        // face in `TEXT_FAINT`, which is what the username line below it
        // uses. Asserted against the name in the same frame so this cannot
        // stay green by both moving together.
        let p = paint(&[card_numbered("BoA Credit", "4242424242424242")], None);
        let (_, name_colour) = painted(&p, "BoA Credit");
        let (_, suffix_colour) = painted(&p, "(*4242)");
        assert_eq!(name_colour, theme::INK);
        assert_eq!(suffix_colour, theme::TEXT_FAINT);
        assert_ne!(suffix_colour, name_colour);

        let name_font = painted_font(&p, "BoA Credit");
        let suffix_font = painted_font(&p, "(*4242)");
        assert_eq!(
            name_font.family,
            egui::FontFamily::Name(theme::SEMIBOLD.into()),
            "the name stopped being the design's 600 weight"
        );
        assert_eq!(
            suffix_font.family,
            egui::FontFamily::Proportional,
            "the suffix is in a named (bold) Archivo face; it must be the plain proportional one"
        );
        // Same size, so the difference the eye reads is weight and ink and
        // not a second type size on one line.
        assert!((suffix_font.size - name_font.size).abs() < 0.01);
    }

    #[test]
    fn a_selected_cards_suffix_stays_light_while_its_name_goes_bold() {
        // The branch the test above cannot reach: selected rows draw the name
        // in Archivo Bold in `BLUE_DEEP`. The suffix must not follow it.
        let p = paint(&[card_numbered("BoA Credit", "4242424242424242")], Some("BoA Credit"));
        let (_, name_colour) = painted(&p, "BoA Credit");
        let (_, suffix_colour) = painted(&p, "(*4242)");
        assert_eq!(name_colour, theme::BLUE_DEEP);
        assert_eq!(suffix_colour, theme::TEXT_FAINT);
        assert_eq!(
            painted_font(&p, "BoA Credit").family,
            egui::FontFamily::Name(theme::BOLD.into())
        );
        assert_eq!(painted_font(&p, "(*4242)").family, egui::FontFamily::Proportional);
    }

    #[test]
    fn a_card_with_three_digits_shows_no_suffix_and_the_row_is_otherwise_itself() {
        // The card-art spec's floor. The POSITIVE half matters as much as the
        // negative one: an implementation that painted nothing at all would
        // satisfy "no suffix" vacuously.
        let p = paint(&[card_numbered("Partial", "454")], None);
        painted(&p, "Partial");
        assert_eq!(one_tile(&p).rect.height(), ROW_TILE_HEIGHT);
        assert!(
            suffix_runs(&p).is_empty(),
            "a three-digit card grew a suffix: {:?}",
            suffix_runs(&p)
        );
    }

    #[test]
    fn a_card_with_no_number_at_all_looks_exactly_as_it_did() {
        let p = paint(&[card("Bare")], None);
        painted(&p, "Bare");
        assert!(suffix_runs(&p).is_empty(), "a numberless card grew a suffix");
    }

    #[test]
    fn a_login_named_like_a_card_number_grows_no_suffix() {
        // The control that the rule is keyed on KIND and not on text: this
        // row's whole name is sixteen digits and it is a login.
        let p = paint(&[login("4242424242424242", "a.novak@ledgerline.com")], None);
        painted(&p, "4242424242424242");
        painted(&p, "a.novak@ledgerline.com");
        assert!(suffix_runs(&p).is_empty(), "a login grew a card suffix");
    }

    #[test]
    fn a_long_card_name_truncates_and_the_four_digits_survive() {
        // The suffix is what tells two cards from the same bank apart, so it
        // is the NAME that loses its tail. Squeezed at a pane far narrower
        // than the real one, which is fixed at `LIST_WIDTH`.
        //
        // **Measured as painted WIDTH and not as painted text.** A truncated
        // egui galley still reports the whole string from `Galley::text` --
        // only its box shrinks -- so a test reading the text back would be
        // green whatever the row drew.
        let long = "Bank of America Cash Rewards Signature Visa";
        let item = card_numbered(long, "4242424242424242");
        let roomy = paint(std::slice::from_ref(&item), None);
        let tight = paint_at_width(std::slice::from_ref(&item), None, 240.0);

        let (roomy_name, _) = painted(&roomy, long);
        let (roomy_suffix, _) = painted(&roomy, "(*4242)");
        let (tight_name, _) = painted(&tight, long);
        let (tight_suffix, _) = painted(&tight, "(*4242)");

        assert!(
            tight_name.width() < roomy_name.width() - 1.0,
            "the name was not squeezed at all: {} wide on a 240pt pane against {} on a \
             {PANE_WIDTH}pt one",
            tight_name.width(),
            roomy_name.width()
        );
        // The suffix is UNTOUCHED: same width on both panes, and still whole.
        assert!(
            (tight_suffix.width() - roomy_suffix.width()).abs() < 0.01,
            "the suffix was squeezed too: {} against {}",
            tight_suffix.width(),
            roomy_suffix.width()
        );
        // And it is still inside the row it belongs to.
        let tile = one_tile_of_width(&tight, 240.0 - 2.0 * LIST_PADDING).rect;
        assert!(
            tight_suffix.right() <= tile.right() + 0.01,
            "the suffix runs to x={} but the tile ends at x={}",
            tight_suffix.right(),
            tile.right()
        );
    }

    #[test]
    fn a_suffix_never_makes_a_row_taller_than_every_other_row() {
        // The virtualized list scrolls by a fixed `ROW_TILE_HEIGHT` pitch, so
        // one taller row slides the whole list out of register. The suffix is
        // painted into the NAME's own box for exactly this reason.
        let items = [
            login("Ledgerline", "a.novak@ledgerline.com"),
            card_numbered("BoA Credit", "4242424242424242"),
        ];
        let p = paint(&items, None);
        let tiles = row_tiles(&p);
        assert_eq!(tiles.len(), 2, "expected one tile per row");
        for tile in &tiles {
            assert!(
                (tile.rect.height() - ROW_TILE_HEIGHT).abs() < 0.01,
                "a row is {} tall, not {ROW_TILE_HEIGHT}",
                tile.rect.height()
            );
        }
    }

    #[test]
    fn a_cards_suffix_does_not_move_the_rows_trailing_chips() {
        // The chips are allocated before the title column, so a longer title
        // line must be absorbed by the title and never by them.
        let mut with_2fa = card_numbered("BoA Credit", "4242424242424242");
        with_2fa.login = Some(crate::vault_bridge::LoginData {
            username: None,
            password: None,
            totp: Some(zeroize::Zeroizing::new("JBSWY3DPEHPK3PXP".into())),
            uris: vec![],
            other: serde_json::Map::new(),
        });
        let mut without = with_2fa.clone();
        without.card = None;
        let with_chip = chip_rect(&paint(&[with_2fa], None), "2FA");
        let bare = chip_rect(&paint(&[without], None), "2FA");
        assert!(
            (with_chip.left() - bare.left()).abs() < 0.01
                && (with_chip.top() - bare.top()).abs() < 0.01,
            "the 2FA chip moved from {bare:?} to {with_chip:?} when the card grew a suffix"
        );
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
                delete_pending: None,
                frames: click_frames(at, egui::PointerButton::Primary),
                filter: SidebarFilter::All,
            },
        );
        assert_eq!(p.selected.as_deref(), Some("Ledgerline"));
    }

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
                MOVE_TO_FOLDER_LABEL,
                "Archive",
                DELETE_LABEL,
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
            vec!["Edit", MOVE_TO_FOLDER_LABEL, "Archive", DELETE_LABEL]
        );
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
                MOVE_TO_FOLDER_LABEL,
                "Archive",
                DELETE_LABEL,
            ]
        );
    }

    #[test]
    fn an_armed_delete_paints_its_confirming_label() {
        let items = [full_login("Ledgerline")];
        let p = open_menu_with(&items, vec![], Some("Ledgerline".to_string()), 0, SidebarFilter::All);
        assert!(
            menu_labels(&p).contains(&DELETE_CONFIRM_LABEL.to_string()),
            "the menu drew {:?}",
            menu_labels(&p)
        );
        assert!(!menu_labels(&p).contains(&DELETE_LABEL.to_string()));
    }

    #[test]
    fn an_armed_delete_on_another_row_leaves_this_row_s_entry_alone() {
        // `delete_pending_id` is one id, not a flag: an arm belongs to the
        // item it was set on.
        let items = [full_login("Ledgerline"), full_login("Vantage")];
        let p = open_menu_with(&items, vec![], Some("Vantage".to_string()), 0, SidebarFilter::All);
        assert!(menu_labels(&p).contains(&DELETE_LABEL.to_string()));
        assert!(!menu_labels(&p).contains(&DELETE_CONFIRM_LABEL.to_string()));
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
        let hover = text_centre(&open_menu(&items, folders.clone(), 0), MOVE_TO_FOLDER_LABEL);
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
            Menu { folders, delete_pending: None, frames, filter: SidebarFilter::All },
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
            Menu { folders: vec![], delete_pending: None, frames, filter: SidebarFilter::All },
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
                MOVE_TO_FOLDER_LABEL,
                "Archive",
                DELETE_LABEL,
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

    /// The centre of the `+ New` button, read off a painted frame.
    fn new_button_centre(items: &[VaultItem]) -> egui::Pos2 {
        let p = paint(items, None);
        p.texts
            .iter()
            .find(|(text, _, _)| text == "+ New")
            .expect("the \"+ New\" button was never painted")
            .1
            .center()
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
                delete_pending: None,
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
                super::super::record_ui::IMPORT_FROM_SEND_LABEL.to_string(),
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
            super::super::record_ui::IMPORT_FROM_SEND_LABEL,
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
                delete_pending: None,
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
                Menu { folders: vec![], delete_pending: None, frames, filter: SidebarFilter::All },
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
    const LIVE_ONLY_ENTRIES: [&str; 8] = [
        "Copy username",
        "Copy password",
        "Copy TOTP",
        "Open website",
        "Edit",
        MOVE_TO_FOLDER_LABEL,
        "Archive",
        DELETE_LABEL,
    ];

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
        for entry in LIVE_ONLY_ENTRIES {
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
        assert_out_of_vault_menu(&p, &["Restore", PURGE_LABEL]);
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
        for entry in ["Restore", PURGE_LABEL] {
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
        for entry in LIVE_ONLY_ENTRIES {
            assert!(
                painted.contains(&entry),
                "the LIVE menu is missing {entry:?}; painted: {painted:?}"
            );
        }
        for entry in ["Restore", "Unarchive", PURGE_LABEL] {
            assert!(!painted.contains(&entry), "a live row offered {entry:?}");
        }
    }

    #[test]
    fn a_trashed_rows_purge_entry_arms_the_same_two_click_confirmation() {
        // `delete_pending` reaches the out-of-vault menu too -- the one
        // irreversible entry in this window, and the only one whose wording
        // changes. Reaching it proves `draw_item_list` forwards both the
        // source AND the pending id into the same call.
        let items = vec![full_login("Ledgerline")];
        let p = open_menu_with(
            &items,
            vec![],
            Some("Ledgerline".to_string()),
            0,
            SidebarFilter::Trash,
        );
        let painted = painted_strings(&p);
        assert!(
            painted.contains(&PURGE_CONFIRM_LABEL),
            "the armed \"Delete forever\" wording never reached the trash menu; \
             painted: {painted:?}"
        );
        assert!(!painted.contains(&PURGE_LABEL), "both wordings were painted: {painted:?}");
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

    /// One real frame of `draw_item_list` at the item pane's own width.
    /// `before_frame` runs between the settling frames and the measured one,
    /// which is how the Ctrl+K test asks for focus the way `vault_window::run`
    /// does.
    fn run_item_list(
        items: &[VaultItem],
        search: &mut String,
        before_frame: impl FnOnce(&egui::Context),
    ) -> (egui::FullOutput, egui::Context) {
        run_item_list_with_events(items, search, before_frame, Vec::new())
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
                    None,
                    &icons,
                    &mut visible,
                    None,
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

    /// A left click at `pos`: the move, the press and the release, which is
    /// the whole sequence egui needs to report `clicked()`.
    fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
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
    fn the_shortcut_slot_shows_ctrl_k_while_empty_and_esc_once_something_is_typed() {
        // THE REPORT: the search field's `CTRL+K` hint should become an "Esc"
        // that clears the field, in the same slot, once there is anything to
        // clear. Both directions asserted, so neither "absent" claim can pass
        // against a field that renders no hint at all.
        let items = [an_item("Ledgerline")];

        let mut empty = String::new();
        let (output, _) = run_item_list(&items, &mut empty, |_| {});
        let texts: Vec<String> = painted(&output).into_iter().map(|(t, _)| t).collect();
        assert!(texts.iter().any(|t| t == "CTRL+K"), "painted: {texts:?}");
        assert!(!texts.iter().any(|t| t == "Esc"), "painted: {texts:?}");

        let mut typed = "ledger".to_string();
        let (output, _) = run_item_list(&items, &mut typed, |_| {});
        let texts: Vec<String> = painted(&output).into_iter().map(|(t, _)| t).collect();
        assert!(texts.iter().any(|t| t == "Esc"), "painted: {texts:?}");
        assert!(
            !texts.iter().any(|t| t == "CTRL+K"),
            "the focus shortcut and the clear affordance share ONE slot; both are showing: \
             {texts:?}"
        );
    }

    #[test]
    fn the_esc_hint_lands_in_exactly_the_slot_ctrl_k_occupied() {
        // "Same slot, no layout shift". The two strings are different widths,
        // so the slot is sized to the WIDER of them and both are right-aligned
        // in it -- the right edge and the vertical centre are asserted equal
        // to the pt, absolutely, between the two states.
        let items = [an_item("Ledgerline")];
        let mut empty = String::new();
        let (a, _) = run_item_list(&items, &mut empty, |_| {});
        let ctrl_k = painted(&a).into_iter().find(|(t, _)| t == "CTRL+K").expect("CTRL+K").1;

        let mut typed = "ledger".to_string();
        let (b, _) = run_item_list(&items, &mut typed, |_| {});
        let esc = painted(&b).into_iter().find(|(t, _)| t == "Esc").expect("Esc").1;

        assert!(
            (ctrl_k.right() - esc.right()).abs() < 0.01,
            "the hint's right edge moved from x={} to x={} when the field filled",
            ctrl_k.right(),
            esc.right()
        );
        assert!(
            (ctrl_k.center().y - esc.center().y).abs() < 0.01,
            "the hint's baseline moved from y={} to y={}",
            ctrl_k.center().y,
            esc.center().y
        );
    }

    #[test]
    fn clicking_the_esc_hint_clears_the_search() {
        let items = [an_item("Ledgerline")];
        // Where the hint is, measured from a first run rather than guessed.
        let mut probe = "ledger".to_string();
        let (output, _) = run_item_list(&items, &mut probe, |_| {});
        let esc = painted(&output).into_iter().find(|(t, _)| t == "Esc").expect("Esc").1;

        let mut search = "ledger".to_string();
        let (_, _) = run_item_list_with_events(&items, &mut search, |_| {}, click_at(esc.center()));
        assert_eq!(search, "", "clicking the Esc hint must clear the search field");
    }

    #[test]
    fn clicking_where_the_hint_is_does_not_clear_anything_while_it_reads_ctrl_k() {
        // The positive control's mirror: the slot is only an affordance when
        // there is something to clear, so a click in the same place with an
        // empty field must not be wired to anything. Without this, "the click
        // cleared it" could just be "any click anywhere clears it".
        let items = [an_item("Ledgerline")];
        let mut probe = String::new();
        let (output, _) = run_item_list(&items, &mut probe, |_| {});
        let slot = painted(&output).into_iter().find(|(t, _)| t == "CTRL+K").expect("CTRL+K").1;

        // Same click, but the field starts empty and the caller then types --
        // i.e. the click must not have consumed or armed anything.
        let mut search = String::new();
        let (_, ctx) = run_item_list_with_events(&items, &mut search, |_| {}, click_at(slot.center()));
        assert_eq!(search, "");
        let _ = ctx;
    }

    #[test]
    fn escape_clears_the_search_while_the_field_has_focus() {
        let items = [an_item("Ledgerline")];
        let mut search = "ledger".to_string();
        let id = egui::Id::new("vault-search");
        let (_, _) = run_item_list_with_events(
            &items,
            &mut search,
            |ctx| ctx.memory_mut(|m| m.request_focus(id)),
            escape_key(),
        );
        assert_eq!(search, "", "Escape must clear the search field while it has focus");
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

    #[test]
    fn escape_on_an_empty_focused_field_is_left_alone_too() {
        // The other half of the gate: nothing to clear, nothing to claim.
        let items = [an_item("Ledgerline")];
        let mut search = String::new();
        let id = egui::Id::new("vault-search");
        let (output, _) = run_item_list_with_events(
            &items,
            &mut search,
            |ctx| ctx.memory_mut(|m| m.request_focus(id)),
            escape_key(),
        );
        assert_eq!(search, "");
        // ...and the slot is still the focus shortcut, not a clear button.
        let texts: Vec<String> = painted(&output).into_iter().map(|(t, _)| t).collect();
        assert!(texts.iter().any(|t| t == "CTRL+K"), "painted: {texts:?}");
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
                    None,
                    &icons,
                    &mut visible,
                    move_error,
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
        let tiles: Vec<egui::Rect> = with_band
            .rects
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
            "No items match your search"
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
                    None,
                    &icons,
                    &mut visible,
                    None,
                    fetch_failed,
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

    /// A modal's scrim, drawn the way every one of them draws it: a
    /// full-window click-catcher `Area` on `Order::Foreground`. Copied from
    /// `folder_modal::draw_folder_edit_modal` rather than called, so this
    /// module needs none of that modal's state -- and named with ITS id, so
    /// if the id there changes, `every_modal_scrim_in_the_crate_is_named_here`
    /// is what fails.
    fn draw_a_modal_scrim(ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("folder-edit-scrim"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                let screen = ctx.content_rect();
                ui.allocate_response(screen.size(), egui::Sense::click());
            });
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
    }

    impl Pane {
        fn new(items: Vec<VaultItem>) -> Self {
            let ctx = egui::Context::default();
            // Two throwaway frames so `theme::apply`'s font set is live --
            // the same reason the header-strip harness above runs them.
            let _ = ctx.run_ui(raw(Vec::new()), |_ui| {});
            theme::apply(&ctx);
            let _ = ctx.run_ui(raw(Vec::new()), |_ui| {});
            let mut pane = Pane {
                ctx,
                items,
                search: String::new(),
                selected: None,
                drawn: Vec::new(),
                modal: false,
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
            let _ = ctx.run_ui(raw(events), |ui| {
                draw_item_list(
                    ui,
                    Some(items),
                    &[],
                    &SidebarFilter::All,
                    search,
                    selected,
                    None,
                    &icons,
                    &mut drawn,
                    None,
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

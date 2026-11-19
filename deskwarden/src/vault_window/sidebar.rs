//! The vault window's left pane (design 4.8 "Sidebar"): the VAULT section
//! (All items / Favorites / Logins / Cards / Secure notes / Trash, each with
//! a live count) and the FOLDERS section (one row per vault folder, also
//! counted -- including `bw serve`'s virtual "No Folder" bucket, which is
//! reported as a folder but scoped by [`SidebarFilter::Unfiled`] rather than
//! by an id), plus the auto-lock countdown pinned to the bottom.
//!
//! Below the folders sit the rail's own SCREENS -- Password health, and then
//! design 5b's labelled SHARING section: the account's own Sends, that row's
//! three sub-filters ([`SendScope`]), and `Shared with me`. See
//! [`screen_rows`] for why those are two groups rather than one, and
//! [`Screens`] for the three selection axes this rail now carries and why
//! none of them is folded into either of the others.

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
    /// Bitwarden's Archive: items the user has put aside. Design 2b lists it
    /// directly above Trash, which is where it is drawn.
    ///
    /// Its own [`FilterSource`], not a predicate over the live vault, and
    /// that is measured rather than chosen: archiving an item REMOVES it from
    /// `GET /list/object/items`, so there is nothing in the list this window
    /// reads to filter for. The row's contents come from
    /// `?archived=true`, a disjoint query, fetched on demand.
    ///
    /// The same measurement is why no exclusion code exists anywhere else in
    /// this app: archived items never reach the match engine, the item list
    /// or autofill, because they are not in the only list any of them read.
    Archive,
    Trash,
    /// A real, server-side folder, by id.
    Folder(String),
    /// Every item owned by one organisation, by its id.
    ///
    /// # Why this is a `SidebarFilter` and Sends are not
    ///
    /// [`screen_rows`] argues at length that Sends, Password health and
    /// `Shared with me` are *screens* rather than cuts of the item list, and
    /// that giving any of them a variant here would force it through the item
    /// pane. This is the other side of that same rule, and it lands on the
    /// other side: an organisation's items **are** `VaultItem`s, in the live
    /// snapshot this window already holds, and "was this cipher owned by that
    /// organisation" is exactly a per-item predicate. The item pane is where
    /// they belong.
    ///
    /// The whole cost of that is one arm in
    /// [`SidebarFilter::scope_contains`]. `item_list::matches_filter`
    /// delegates to it, so the pane, its search and its empty state follow
    /// with no edit at all -- which is the property the split was designed
    /// for, used for the first time.
    ///
    /// **Membership is `organizationId`, and only that.**
    /// [`crate::rest::organizations::organization_of`] is the single
    /// definition, shared with the audience answer, so the row and a future
    /// `Owner` line in the detail pane cannot come to disagree about what
    /// "owned by" means.
    Organisation(String),
    /// Every item filed in one collection, by its id.
    ///
    /// A **sub-row of its organisation**, exactly as design 5b's three
    /// SHARING sub-rows are sub-rows of `Sends`, and drawn with the same
    /// [`SUB_ROW_EXTRA_INDENT`]. The relationship is the same one: a parent
    /// naming a set and children naming cuts of it.
    ///
    /// It is **not** a `Folder`, and the difference is worth the second
    /// variant rather than a clever reuse. A folder is this account's own
    /// filing, it is personal on Bitwarden (there is no organisation folder
    /// -- see [`crate::rest::sync::VaultKeys::user`]), and an item is in at
    /// most one. A collection belongs to somebody else, an item can be in
    /// several at once, and this app cannot create, rename or delete one. A
    /// collection row wearing the FOLDERS section's edit pencil would offer
    /// three writes that do not exist.
    Collection(String),
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

/// Which of `bw serve`'s three item queries a sidebar row draws from.
///
/// Its own type, and the reason is the defect this file's history is made of.
/// Trash was written as `scope_contains(..) => false` with a comment
/// concluding it was "not implementable from the endpoint this window reads",
/// and that shape can express only one idea: *this row is a predicate over
/// the live vault*. It is not, and neither is Archive -- both read a
/// SEPARATE query whose results are disjoint from the live list, so "no live
/// item is in the trash" is true, and useless, and reads as an empty row.
///
/// Splitting "which list" from "which items within it" is what makes the two
/// answerable at all. [`SidebarFilter::source`] answers the first,
/// [`SidebarFilter::scope_contains`] the second, and [`items_for`] is the one
/// place they are combined -- so no caller has to remember to ask the first
/// question before the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterSource {
    /// `GET /list/object/items` -- the snapshot this window already holds and
    /// keeps up to date. Every type/favourite/folder row reads it.
    LiveVault,
    /// `GET /list/object/items?trash=true`. Returns ONLY trashed items, each
    /// carrying `deletedDate`: 14 against a vault whose live list held 1654,
    /// with zero overlap.
    Trash,
    /// `GET /list/object/items?archived=true`. Returns ONLY archived items.
    ///
    /// The spelling matters and is not guessable: `?archive=true`, without
    /// the "d", is SILENTLY IGNORED and answers 200 with the whole live
    /// vault -- so that typo does not surface as an error, it surfaces as an
    /// Archive row showing the user's entire vault. `list_archive`'s test
    /// therefore asserts the query string on the wire.
    Archive,
}

/// A row whose items are not in the live vault at all.
///
/// Two variants and no "live" one, deliberately: this is what the detail
/// pane's out-of-vault branch takes, so that branch **cannot be drawn for an
/// ordinary item**. The alternative -- passing a [`FilterSource`] and
/// trusting every call site to have checked it first -- is the same
/// "remember to ask the first question" the split above exists to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutOfVault {
    Trash,
    Archive,
}

impl OutOfVault {
    /// The row's own name, for messages about it. A method rather than
    /// `{:?}` at each call site: `Debug` is for developers and happens to
    /// read well here, which is exactly the coincidence that stops holding
    /// the first time a variant is renamed.
    pub fn label(self) -> &'static str {
        match self {
            OutOfVault::Trash => "Trash",
            OutOfVault::Archive => "Archive",
        }
    }
}

impl FilterSource {
    /// `Some` when this source is one of the two queries outside the live
    /// vault, `None` for the live vault itself.
    pub fn out_of_vault(self) -> Option<OutOfVault> {
        match self {
            FilterSource::LiveVault => None,
            FilterSource::Trash => Some(OutOfVault::Trash),
            FilterSource::Archive => Some(OutOfVault::Archive),
        }
    }
}

/// Every item list the vault window holds at once, so one function can pick
/// the one a given row reads.
///
/// `trash` and `archive` are `Option` because "not fetched yet" is a real,
/// visible state and not a synonym for "empty": both are fetched on demand,
/// off the UI thread, the first time their row is selected. A badge that
/// printed `0` for an unfetched list would state a fact this app does not
/// have -- and it is the exact untruth the Trash row shipped for months.
/// See [`badge_text`].
#[derive(Clone, Copy)]
pub struct VaultLists<'a> {
    pub live: &'a [VaultItem],
    pub trash: Option<&'a [VaultItem]>,
    pub archive: Option<&'a [VaultItem]>,
    /// How many Sends this account has, **and how they split across design
    /// 5b's three sub-rows**, or `None` for "this app does not know".
    ///
    /// Counts and not a list, because Sends are not `VaultItem`s and the rail
    /// only ever needs the numbers; the rows themselves live in
    /// `vault_window::send_ui`. Four numbers in one value rather than four
    /// fields here, so a parent badge and a sub-row badge cannot come from
    /// two readings of the list against two clocks -- see [`SendCounts`].
    ///
    /// `None` covers two situations on purpose -- not fetched, and fetched
    /// unsuccessfully -- because every badge in the section must say the same
    /// thing about both: it does not know. See [`badge_for`] and
    /// [`UNKNOWN_COUNT`].
    pub sends: Option<SendCounts>,
    /// How many Sends other people have shared with this user and this app
    /// has imported -- design 5b's `Shared with me` row.
    ///
    /// A plain `usize` and **not** an `Option`, for `health_findings`'
    /// reason one step along: this is a local file, not a query, and
    /// [`crate::receive_history::ReceiveHistory::load`] folds every failure
    /// -- absent, empty, unreadable -- into an empty history. There is no
    /// "not fetched yet" state to represent, and a `0` here really does mean
    /// "nothing has been shared with you", which the pane says in words.
    pub received: usize,
    /// How many distinct items the password-health report has a finding
    /// against -- the Password health row's badge.
    ///
    /// A plain `usize` and **not** an `Option`, unlike every field above it,
    /// and the difference is the point: those are queries that may not have
    /// answered yet, and this is a pure function of `live`, which is already
    /// in hand. There is no "not fetched" state to represent, so there is no
    /// state in which `UNKNOWN_COUNT` would be the honest badge -- and a `0`
    /// here really does mean "nothing is wrong", rather than the "this app
    /// does not know" that a `0` on the Trash row once meant.
    ///
    /// Computed by `password_health::report_for` at the call site rather than
    /// here, so the badge and the pane read one report and cannot disagree
    /// about how many findings there are.
    pub health_findings: usize,
    /// The organisations and collections this account can reach, and what is
    /// known about who else is in them.
    ///
    /// **Never an `Option`, unlike `sends` and the two query-backed lists**,
    /// and this is the whole of how the ORGANISATIONS section stays off an
    /// account that has none. There is no "not fetched" state to draw,
    /// because there is nothing to draw: an account with no organisations, a
    /// `bw serve` account, an account whose server has never heard of these
    /// routes, and an account that has not synced yet all hand over the same
    /// empty [`crate::rest::organizations::Directory`], and
    /// [`draw_organisations`] returns before painting so much as a divider.
    /// An `Option` here would be four call sites each deciding that again.
    ///
    /// A borrow, so `VaultLists` stays `Copy` and the two places that build
    /// one keep costing nothing.
    pub sharing: &'a crate::rest::organizations::Directory,
    /// Person shares, one per (record, person) -- added to Sent's count
    /// (spec 6.10). 0 without a sharing view.
    pub shared_records: usize,
    /// Records others share with this account -- added to Shared with me.
    pub received_records: usize,
    /// An invitation is waiting: a dot on Shared with me (mock 6d).
    pub invitation_waiting: bool,
}

impl<'a> VaultLists<'a> {
    /// A window that holds only the live vault -- neither on-demand query has
    /// answered yet. What every caller starts from.
    pub fn live_only(live: &'a [VaultItem]) -> Self {
        VaultLists {
            live,
            trash: None,
            archive: None,
            sends: None,
            received: 0,
            health_findings: 0,
            // The empty directory, borrowed from the `static` that exists so
            // this constructor does not have to allocate one. Every caller of
            // `live_only` is either a test or a window that has not been told
            // about any sharing, and both want the same thing: a rail with no
            // ORGANISATIONS section in it at all.
            sharing: &crate::rest::organizations::NOTHING_SHARED,
            // No sharing view either: nothing shared, nothing received,
            // nobody waiting.
            shared_records: 0,
            received_records: 0,
            invitation_waiting: false,
        }
    }
}

/// The items on `filter`'s row, or `None` when the query that row reads has
/// not answered yet.
///
/// **The single place "which list" and "which items in it" are combined.**
/// The live rows filter the snapshot through
/// [`SidebarFilter::scope_contains`]; the two query-backed rows return their
/// list whole, because the query already scoped it. Both the sidebar's badge
/// and the item pane's contents go through here, so a row cannot count one
/// thing and list another.
pub fn items_for<'a>(filter: &SidebarFilter, lists: VaultLists<'a>) -> Option<Vec<&'a VaultItem>> {
    let source = filter.source();
    let list = match source {
        FilterSource::LiveVault => Some(lists.live),
        FilterSource::Trash => lists.trash,
        FilterSource::Archive => lists.archive,
    }?;
    Some(list.iter().filter(|item| filter.scope_contains(item)).collect())
}

/// How many items are on `filter`'s row, or `None` when its list has not been
/// fetched yet. Straight off [`items_for`], so the badge and the pane cannot
/// disagree.
pub fn badge_for(filter: &SidebarFilter, lists: VaultLists<'_>) -> Option<usize> {
    items_for(filter, lists).map(|items| items.len())
}

/// What an unfetched count is drawn as: an en dash, never `0`.
///
/// `0` is a claim -- "this row is empty" -- and until the query has answered
/// this app does not know whether it is. The Trash row printed exactly that
/// claim for months while fourteen items sat behind it.
pub const UNKNOWN_COUNT: &str = "\u{2013}";

/// The badge string for a possibly-unfetched count. Pure, so the
/// distinction above is testable without a frame.
pub fn badge_text(count: Option<usize>) -> String {
    match count {
        Some(count) => count.to_string(),
        None => UNKNOWN_COUNT.to_string(),
    }
}

impl SidebarFilter {
    /// Which query this row's items come from. See [`FilterSource`].
    pub fn source(&self) -> FilterSource {
        match self {
            SidebarFilter::Trash => FilterSource::Trash,
            SidebarFilter::Archive => FilterSource::Archive,
            SidebarFilter::All
            | SidebarFilter::Favorites
            | SidebarFilter::Apps
            | SidebarFilter::Logins
            | SidebarFilter::Passkeys
            | SidebarFilter::Cards
            | SidebarFilter::Identities
            | SidebarFilter::SecureNotes
            | SidebarFilter::SshKeys
            | SidebarFilter::Folder(_)
            // Organisation-owned items are in the ordinary sync payload
            // alongside personal ones -- `GET /api/sync` returns both and
            // `rest::sync` decrypts both -- so these two rows read the live
            // snapshot like every type row above them. There is no
            // `?organization=` query to be a third `FilterSource` for.
            | SidebarFilter::Organisation(_)
            | SidebarFilter::Collection(_)
            | SidebarFilter::Unfiled => FilterSource::LiveVault,
        }
    }

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
    /// **PRECONDITION: `item` came from [`Self::source`]'s query.** For every
    /// row but two that is the live snapshot and there is nothing to get
    /// wrong. For `Trash` and `Archive` it is a different list entirely, and
    /// this function cannot tell -- so reach it through [`items_for`], which
    /// picks the list first.
    ///
    /// What used to be here instead: a `Trash => false` arm under a comment
    /// asserting Trash was "not implementable from the endpoint this window
    /// reads ... no `deletedDate` field to filter on (verified against a real
    /// 1657-item vault)". Both halves of that evidence were true and the
    /// conclusion did not follow, because the query parameter was never
    /// tried: `GET /list/object/items?trash=true` answers with 14 items, all
    /// carrying `deletedDate`, against the same vault whose default list has
    /// 1654 and none. The row was not unimplementable; it was reading the
    /// wrong list. See [`FilterSource`].
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
            // THE QUERY IS THE FILTER for these two, so there is nothing
            // left for this function to test -- see [`FilterSource`].
            // `?trash=true` and `?archived=true` each return a set that is
            // DISJOINT from the default list and contains exactly this row's
            // items (measured, `.superpowers/sdd/item-shapes-capture.md`), so
            // every item that reaches here from one of them is on the row.
            //
            // This is where the precondition in this function's doc bites: an
            // item from the LIVE list handed to these two arms gets `true`
            // and is wrong. That is why nothing calls this directly for them
            // -- [`items_for`] picks the list FIRST, from
            // [`SidebarFilter::source`], and only then filters.
            SidebarFilter::Trash | SidebarFilter::Archive => true,
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
            // Both go through `rest::organizations`, which is the one place
            // this crate reads `organizationId` and `collectionIds` off an
            // item. Not re-read from `item.other` here: a second reader would
            // be a second answer to "is this shared", and the detail pane's
            // eventual `Owner` row asks the same module.
            SidebarFilter::Organisation(id) => {
                crate::rest::organizations::organization_of(item) == Some(id.as_str())
            }
            // `contains`, not equality: an item can be filed in several
            // collections at once, which is the property that makes a
            // collection unlike a folder.
            SidebarFilter::Collection(id) => {
                crate::rest::organizations::collection_ids_of(item).contains(&id.as_str())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    None,
    /// The rail's New item button: whatever `Ctrl+N` does.
    NewItem,
    /// The inline New folder row was submitted with this (trimmed, non-blank)
    /// name.
    CreateFolder(String),
    /// The pinned Lock row: whatever `Ctrl+L` does.
    Lock,
    /// The TOOLS section's Settings row: opens the preferences the titlebar
    /// gear used to.
    OpenSettings,
    /// A click on a folder row's edit-pencil icon -- the folder id.
    /// `vault_window::mod` opens the "Edit folder" modal in response (see
    /// `folder_modal`), which is where rename and delete actually happen.
    EditFolder(String),
    /// An item row was dragged out of the item list and dropped on a folder
    /// row that will take it. Both ids are carried; the caller resolves the
    /// item itself.
    /// `folder_id` is `None` for the virtual "No Folder" row, which is a
    /// real destination on a backend that can un-file -- see
    /// [`drop_outcomes`]. An `Option` rather than the empty string that row's
    /// id actually is, because writing that id is what strands an item out of
    /// every sidebar row, and `item_list::RowCommand::Unfile` refuses the
    /// same confusion for the same reason.
    MoveItemToFolder { item_id: String, folder_id: Option<String> },
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
/// The list header's name for `filter` (spec 3.1): the row's own label for
/// the vault's kinds, a folder's or an organisation's own name, and a
/// collection's. A folder or collection that has gone since it was picked
/// reads as its kind.
pub fn scope_title(filter: &SidebarFilter, folders: &[Folder], lists: VaultLists<'_>) -> String {
    let fixed = match filter {
        SidebarFilter::All => "All items",
        SidebarFilter::Favorites => "Favorites",
        SidebarFilter::Apps => "Apps",
        SidebarFilter::Logins => "Logins",
        SidebarFilter::Passkeys => "Passkeys",
        SidebarFilter::Cards => "Cards",
        SidebarFilter::Identities => "Identities",
        SidebarFilter::SecureNotes => "Secure notes",
        SidebarFilter::SshKeys => "SSH keys",
        SidebarFilter::Archive => "Archive",
        SidebarFilter::Trash => "Trash",
        SidebarFilter::Folder(id) => {
            return folders.iter().find(|f| &f.id == id).map_or_else(|| "Folder".to_string(), |f| f.name.clone())
        }
        SidebarFilter::Unfiled => {
            return folders
                .iter()
                .find(|f| is_virtual_folder(f))
                .map_or_else(|| "No folder".to_string(), |f| f.name.clone())
        }
        SidebarFilter::Organisation(id) => {
            return lists
                .sharing
                .visible_organisations()
                .into_iter()
                .find(|o| &o.id == id)
                .map_or_else(|| "Organisation".to_string(), |o| o.label().to_string())
        }
        SidebarFilter::Collection(id) => {
            return lists
                .sharing
                .visible_organisations()
                .into_iter()
                .flat_map(|o| lists.sharing.collections_of(&o.id))
                .find(|c| &c.id == id)
                .map_or_else(|| "Collection".to_string(), |c| c.label().to_string())
        }
    };
    fixed.to_string()
}

pub fn is_virtual_folder(folder: &Folder) -> bool {
    folder.id.is_empty()
}

/// What to call the folder an item says it is in -- or `None`, meaning there
/// is nothing here anyone can be told.
///
/// **In this module because this module is where a folder id becomes a name.**
/// The FOLDERS section resolves ids to names for every row it draws; the
/// detail pane's header needs the same answer for one item, and a second
/// lookup written beside that header is a second place for the virtual bucket
/// and the missing-folder case to be decided differently.
///
/// The three `None`s, each of them a case a caller can reach:
///
///  - **The item is in no folder** (`folder_id: None`), which is most of a
///    vault.
///  - **The id names nothing in `folders`** -- a folder deleted from another
///    client, or a header drawn before the folder list has loaded. Returning
///    the raw id would put a uuid in front of the user, which is the mistake
///    [`crate::accounts::account_label`] exists to avoid; inventing "No
///    folder" would state something this function does not know.
///  - **The id is the virtual "No Folder" bucket's empty one.** It is in the
///    list `bw serve` sends, so a naive `find` matches it and would report an
///    item in a broken `folderId: ""` state (see `detail_edit`'s folder
///    dropdown, which has met one) as deliberately unfiled.
pub fn folder_name<'a>(folders: &'a [Folder], folder_id: Option<&str>) -> Option<&'a str> {
    let id = folder_id?;
    folders
        .iter()
        .find(|folder| folder.id == id && !is_virtual_folder(folder))
        .map(|folder| folder.name.as_str())
}

/// How many of `items` fall under `filter`. Pure and separate from drawing
/// so the sidebar's counts are testable without an egui context.
/// **`items` must be `filter`'s own source list** -- see
/// [`SidebarFilter::scope_contains`]'s precondition. The sidebar's badges go
/// through [`badge_for`] instead, which picks the list itself; this remains
/// for the item pane, which is already handed the right list.
pub fn count_for(items: &[VaultItem], filter: &SidebarFilter) -> usize {
    items.iter().filter(|item| filter.scope_contains(item)).count()
}

/// The Sends row's label, spelled once so the rail and the tests that press
/// it cannot drift apart.
///
/// **`Sent`**, the owner's word for it -- "prob only have Sent and Shared with
/// me (both ways folders) - that's it": two folders, one for each direction.
pub const SENDS_ROW_LABEL: &str = "Sent";

/// Design 5b's `Shared with me` row: the Sends **other people** sent to this
/// user and this app imported.
///
/// Its count is a local record rather than a query -- see
/// [`crate::receive_history`], which is the file, and why it holds no access
/// URL.
pub const RECEIVED_ROW_LABEL: &str = "Shared with me";

/// The design's own name for the section these rows sit in (5b: `SHARING`).
pub const SHARING_SECTION_LABEL: &str = "SHARING";

/// **Design 5b's SHARING sub-filters: a third selection axis, orthogonal to
/// both [`SidebarFilter`] and [`Screen`].**
///
/// # Why it is not either of the two axes that already exist
///
/// It is not a [`SidebarFilter`], for the reason [`Screens`] gives about the
/// screens themselves: `item_list` matches on that type to choose its
/// empty-state nouns and its per-row scoping, and a Send is not a
/// `VaultItem`, so a variant there would force these rows through the item
/// pane.
///
/// It is not a [`Screen`] either, and that distinction is the whole point of
/// a separate type. A `Screen` answers *which pane is drawn*; every one of
/// these draws the SAME pane. What they change is which of its rows are in
/// it. Folding them in as `Screen::SendsWaiting`, `Screen::SendsUsed` and so
/// on would have made `Screens::is(Screen::Sends)` false while the Sends
/// screen was plainly on screen -- so every existing reader of that predicate
/// (the fetch gate, the refetch policy, `vault_body_state`, the panel
/// choice) would have needed a three-way `matches!` and the one that forgot
/// would silently stop fetching. The axes stay separate and the pane reads
/// both.
///
/// # `Ended` is not one of `SendState`'s five, and that is deliberate
///
/// [`crate::send::SendState`] has five members and this has four sub-rows,
/// because `Ended` merges `Expired` and `Revoked`. 5b's own counts confirm the
/// partition rather than merely allowing it: its sub-rows add up to the
/// parent row's number, so every Send is on exactly one sub-row. `Opened` has
/// a sub-row of its own since it became a state -- a live link somebody has
/// opened is not waiting for anybody, and folding it back into `Waiting`
/// would put it under the gloss "nobody has opened it yet".
///
/// **The rail and the pill therefore say different words about the same
/// Send, on purpose.** A pill names *what happened to this link* -- the
/// owner pulled it back, or the clock ran out, and those are different facts
/// with different remedies. A rail row names *a set of links to go and look
/// at*, and at that altitude the useful question is only "is it still live?".
/// Splitting `Ended` in the rail would put two rows in front of the user that
/// differ by a fact the pill already carries, beside the link, at the moment
/// it matters. Three rows are a glance; five are a diagnosis in the wrong
/// place.
///
/// The mismatch is not left silent, which is the condition of making it. The
/// list column's strip names the scope in force, and [`Self::gloss`] spells
/// out under it what `Ended` covers -- naming both pill words in the one row
/// that merges them. See `send_ui::draw_send_pane`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SendScope {
    /// Every Send this account has published. The parent row.
    #[default]
    All,
    /// [`crate::send::SendState::Waiting`].
    Waiting,
    /// [`crate::send::SendState::Opened`].
    Opened,
    /// [`crate::send::SendState::Used`].
    Used,
    /// [`crate::send::SendState::Expired`] **and**
    /// [`crate::send::SendState::Revoked`]. See the type's docs.
    Ended,
}

impl SendScope {
    /// The four sub-rows, in the order 5b draws its three, with `Opened`
    /// after `Waiting` -- the order a link moves through them. A constant
    /// rather than literals at the draw site, so the row loop, the count check
    /// and the tests cannot come to disagree about which rows exist.
    pub const SUB_ROWS: [SendScope; 4] = [Self::Waiting, Self::Opened, Self::Used, Self::Ended];

    /// The word the rail prints.
    ///
    /// Two of the three are `send_ui`'s own pill constants rather than fresh
    /// literals, so a row and the pill beside it cannot drift into two
    /// spellings of one state. `Ended` has no pill to borrow from -- it is
    /// the merge -- so it is spelled here, once.
    pub fn label(self) -> &'static str {
        use crate::vault_window::send_ui;
        match self {
            SendScope::All => SENDS_ROW_LABEL,
            SendScope::Waiting => send_ui::WAITING_LABEL,
            SendScope::Opened => send_ui::OPENED_LABEL,
            SendScope::Used => send_ui::USED_LABEL,
            SendScope::Ended => ENDED_ROW_LABEL,
        }
    }

    /// Whether a Send in `state` belongs on this row.
    ///
    /// `All` admits everything, which is what makes the parent row's count
    /// and the three sub-rows' counts come from one function rather than from
    /// a total plus three predicates that have to agree with it.
    pub fn admits(self, state: crate::send::SendState) -> bool {
        use crate::send::SendState;
        match self {
            SendScope::All => true,
            SendScope::Waiting => state == SendState::Waiting,
            SendScope::Opened => state == SendState::Opened,
            SendScope::Used => state == SendState::Used,
            SendScope::Ended => matches!(state, SendState::Expired | SendState::Revoked),
        }
    }

    /// The same word in the case design 5b draws a list column's strip in.
    ///
    /// A constant per scope rather than `label().to_uppercase()`, because a
    /// case conversion at draw time produces a string no test and no reader
    /// can grep for -- which is the whole reason `theme::eyebrow`'s own doc
    /// tells callers to hand it a literal already in the design's case.
    pub fn eyebrow(self) -> &'static str {
        match self {
            SendScope::All => crate::vault_window::send_ui::SENDS_HEADING,
            SendScope::Waiting => "WAITING",
            SendScope::Opened => "OPENED",
            SendScope::Used => "USED",
            SendScope::Ended => "ENDED",
        }
    }

    /// The one line the Sends pane prints under its strip while this scope is
    /// in force.
    ///
    /// `Ended`'s **names both pill words**, which is the bridge that lets the
    /// rail merge two states without the pill beside a row reading as a
    /// contradiction. See the type's docs; the other two are 5c's own glosses
    /// for the states they select.
    ///
    /// `All` returns `None` rather than a fourth sentence: the pane already
    /// prints [`crate::vault_window::send_ui::SCOPE_SUBTEXT`] there, which
    /// says what this app can and cannot do with a Send, and two lines of
    /// standing explanation on the unfiltered screen is one too many.
    pub fn gloss(self) -> Option<&'static str> {
        match self {
            SendScope::All => None,
            SendScope::Waiting => Some("The link is live, and nobody has opened it yet."),
            SendScope::Opened => Some("The link is live, and somebody has opened it."),
            SendScope::Used => Some("Opened, with every permitted view spent."),
            SendScope::Ended => {
                Some("Out of time or turned off \u{2014} expired and revoked links together.")
            }
        }
    }
}

/// The word [`SendScope::Ended`] prints. Spelled once, and **not** one of
/// `send_ui`'s four pill labels; see [`SendScope`] for why that is deliberate
/// rather than an oversight.
pub const ENDED_ROW_LABEL: &str = "Ended";

/// How many Sends fall on the parent row and on each of its four sub-rows.
///
/// **One value carrying four numbers rather than four `Option<usize>`s**, so
/// the rail cannot draw a parent count from one reading of the list and a
/// sub-row count from another. The whole set is derived in one pass over one
/// answer, against one clock -- which matters because three of the four
/// states are clock-dependent and a badge computed a frame apart from its
/// neighbour would be a rail whose rows do not add up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SendCounts {
    pub all: usize,
    pub waiting: usize,
    pub opened: usize,
    pub used: usize,
    pub ended: usize,
}

impl SendCounts {
    /// Counts every state in `states` onto its row.
    ///
    /// Built by ASKING [`SendScope::admits`] rather than by a `match` written
    /// a second time here: the predicate that decides which rows a Send is
    /// counted on has to be the same one that decides which rows it is listed
    /// on, or a badge says 2 over a list of three.
    ///
    /// **A named generic rather than `impl IntoIterator<Item = ...>` in
    /// argument position**, and that is not style. `job_object`'s
    /// `only_the_files_that_must_leave_the_job_can_open_a_bare_command` reads
    /// every `impl` HEAD in this crate -- the text from the keyword to the
    /// opening brace -- and refuses one that names `crate::send::` outside
    /// `send.rs`, because an inherent impl written from a foreign module can
    /// grow a method onto a type the two job-bearing files are allowed to
    /// hold. An `impl Trait` argument puts that keyword and that path in one
    /// head, so it reads as exactly the shape that rule exists to refuse. The
    /// generic says the same thing without the collision, and leaving it as
    /// `impl Trait` would have meant weakening a spawn guard to spell a
    /// parameter more briefly.
    pub fn over<I>(states: I) -> Self
    where
        I: IntoIterator<Item = crate::send::SendState>,
    {
        let mut counts = SendCounts::default();
        for state in states {
            counts.all += 1;
            if SendScope::Waiting.admits(state) {
                counts.waiting += 1;
            }
            if SendScope::Opened.admits(state) {
                counts.opened += 1;
            }
            if SendScope::Used.admits(state) {
                counts.used += 1;
            }
            if SendScope::Ended.admits(state) {
                counts.ended += 1;
            }
        }
        counts
    }

    /// This scope's own number.
    pub fn of(self, scope: SendScope) -> usize {
        match scope {
            SendScope::All => self.all,
            SendScope::Waiting => self.waiting,
            SendScope::Opened => self.opened,
            SendScope::Used => self.used,
            SendScope::Ended => self.ended,
        }
    }
}

/// The rail rows that are **screens rather than cuts of an item list**, and
/// the one place their mutual exclusion is enforced.
///
/// None of Sends, Password health or Shared with me is a [`SidebarFilter`]:
/// Sends are not `VaultItem`s at all, password reuse is a property of a PAIR
/// of items (which no per-item `scope_contains` predicate can answer), and a
/// received Send is a line in a local history file rather than anything in
/// the vault list. All three therefore sit beside `selected` rather than
/// inside it.
///
/// **Flags behind one handle, and not loose parameters**, because the
/// invariant is not "each row sets its own flag" -- it is *at most one of
/// these is live, and selecting anything else clears them all*. With loose
/// flags that invariant is re-implemented at every row, and the row that
/// forgets one of them leaves the window painting a screen while the rail
/// highlights Cards. Here it is [`Self::clear`] and [`Self::select`], and a
/// row that calls neither does not compile into a working row at all. The
/// third screen was added by writing one variant and one line in `select`,
/// which is the property that shape was chosen for.
///
/// **`scope` rides here rather than beside it**, and that is the same
/// argument one level down. It is only meaningful while the Sends screen is
/// up, so it is [`Self::clear`]'s job to put it back to
/// [`SendScope::All`] -- otherwise a sub-row clicked, left and returned to by
/// the parent row would come back still filtered, with the parent row
/// highlighted and a list that does not match it. Keeping the axis in the
/// same handle as the flag it depends on is what makes that one line rather
/// than a rule every caller has to remember.
///
/// They are not folded into `SidebarFilter` as variants for the reason
/// [`screen_rows`] gives: that type is what `item_list` matches on to choose
/// its empty-state nouns and its per-row scoping, so a variant there would
/// force these screens through the item pane.
pub struct Screens<'a> {
    pub sends: &'a mut bool,
    pub health: &'a mut bool,
    /// Design 5b's `Shared with me`. See [`RECEIVED_ROW_LABEL`].
    pub received: &'a mut bool,
    /// Slice 6: the Generator screen.
    pub generator: &'a mut bool,
    /// Slice 7: the Settings screen.
    pub settings: &'a mut bool,
    /// Which SHARING sub-row is in force, meaningful only while `sends` is
    /// true. See [`SendScope`].
    pub scope: &'a mut SendScope,
}

/// Which screen row, if any, is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Sends,
    Health,
    /// Design 5b's `Shared with me`: the local record of what was imported
    /// from somebody else's link.
    Received,
    /// Slice 6: the Generator screen.
    Generator,
    /// Slice 7: the Settings screen.
    Settings,
}

impl Screens<'_> {
    /// Whether any screen is up -- i.e. whether `selected` is currently
    /// describing a row the user is NOT looking at.
    pub fn any(&self) -> bool {
        *self.sends || *self.health || *self.received || *self.generator || *self.settings
    }

    /// Whether this particular screen is the one that is up.
    pub fn is(&self, screen: Screen) -> bool {
        match screen {
            Screen::Sends => *self.sends,
            Screen::Health => *self.health,
            Screen::Received => *self.received,
            Screen::Generator => *self.generator,
            Screen::Settings => *self.settings,
        }
    }

    /// The SHARING sub-row in force, or [`SendScope::All`] when the Sends
    /// screen is not the one up.
    ///
    /// A reader rather than a bare field access, so nothing outside this type
    /// can read a scope left over from a screen that is no longer showing --
    /// the same hazard [`Self::clear`] resets the field for, closed a second
    /// way at the point of use.
    pub fn scope(&self) -> SendScope {
        if *self.sends { *self.scope } else { SendScope::All }
    }

    /// Whether this SHARING row is the selected one: the parent row while no
    /// sub-filter is in force, and the sub-row itself while one is.
    ///
    /// **Exactly one row in the rail carries the selection wash**, which is
    /// why the parent is not also lit while a sub-row is. Two washes stacked
    /// read as two selections, and every other row in this rail lights one.
    /// What tells the user they are still inside SHARING is the section label
    /// above the group and the list column's own strip, which names the scope
    /// in force.
    pub fn sharing_is(&self, scope: SendScope) -> bool {
        *self.sends && *self.scope == scope
    }

    /// Back to the item list. Every item row calls this, which is what stops
    /// a screen outliving the click that left it.
    pub fn clear(&mut self) {
        *self.sends = false;
        *self.health = false;
        *self.received = false;
        *self.generator = false;
        *self.settings = false;
        // See the type's docs: the scope belongs to the Sends screen, so
        // leaving that screen by ANY door puts it back.
        *self.scope = SendScope::All;
    }

    /// This screen and no other. Written as clear-then-set so adding a screen
    /// cannot leave a second one live.
    pub fn select(&mut self, screen: Screen) {
        self.clear();
        match screen {
            Screen::Sends => *self.sends = true,
            Screen::Health => *self.health = true,
            Screen::Received => *self.received = true,
            Screen::Generator => *self.generator = true,
            Screen::Settings => *self.settings = true,
        }
    }

    /// The Sends screen, filtered to `scope`.
    ///
    /// The parent row calls this with [`SendScope::All`] rather than
    /// `select(Screen::Sends)`, so there is ONE way to arrive on this screen
    /// and it always states which rows it wants. A parent row that used the
    /// plain `select` would be relying on `clear`'s reset for its own
    /// correctness, which is exactly the kind of load-bearing side effect
    /// this rail keeps having to un-write.
    pub fn select_sharing(&mut self, scope: SendScope) {
        self.select(Screen::Sends);
        *self.scope = scope;
    }
}

/// What the rail asks the window for: the three rows whose work the window
/// already does for a chord or a button -- New item for `Ctrl+N`, Lock for
/// `Ctrl+L`, Settings for the preferences. Carried as a one-frame request
/// the window reads beside those, so the rail can never grow a second path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShellRequest {
    NewItem,
    Lock,
    OpenSettings,
    /// The top bar's `Ctrl K` chip: open the command palette.
    OpenPalette,
}

/// Records `request` for the window to answer on its next frame.
pub fn request_shell(ctx: &egui::Context, request: ShellRequest) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(("rail-request", request)), true));
    ctx.request_repaint();
}

/// Whether `request` is waiting, clearing it: each request is answered once.
pub fn take_shell_request(ctx: &egui::Context, request: ShellRequest) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new(("rail-request", request)))).unwrap_or(false)
}

/// Reserved width for a folder row's edit pencil, subtracted from
/// [`sidebar_row`]'s width *before* the row itself is laid out -- otherwise the
/// row's own click target would extend under the pencil and the two would
/// double-fire on the same click.
const FOLDER_EDIT_BUTTON_WIDTH: f32 = 24.0;

/// The v2 panel's side padding (design 1a: `padding: 22px 14px 16px`), and
/// therefore the width of the lane the rail's scroll bar lives in.
///
/// **One constant for two things that must be the same number.** The panel
/// frame keeps it as its LEFT inner margin and sets its RIGHT one to zero; the
/// rail then hands the same value to [`theme::scrollbar_in_gutter`], which
/// reserves it back as a lane down the rail's right edge. The two cancel, so
/// the rows span the panel less 14 on each side -- 224pt on the 252pt panel --
/// and the bar is drawn in the lane instead of on top of them.
///
/// `i8` because that is what `egui::Margin`'s fields are; the scroll helper
/// takes the `f32`.
pub(crate) const PANEL_PAD_X: i8 = 14;

/// The frame the vault window wraps [`draw_sidebar`] in: design 1a's
/// `padding: 22px 14px 16px` on the sidebar style's ground, with the RIGHT 14
/// given up to the rail's scroll bar (see [`PANEL_PAD_X`]).
///
/// A function because it is drawn in two places -- the window and the rail's
/// own tests -- and they must be the same frame.
pub fn panel_frame(style: crate::kit::tokens::SidebarStyle) -> egui::Frame {
    egui::Frame::new().fill(style.palette().bg).inner_margin(egui::Margin {
        left: PANEL_PAD_X,
        right: 0,
        top: 22,
        bottom: 16,
    })
}

/// A nav row (design 1a: `height: 44px`, radius 12).
const ROW_HEIGHT: f32 = 44.0;
/// A folder row is a little shorter (`height: 40px`).
const FOLDER_ROW_HEIGHT: f32 = 40.0;
/// A row's own side padding (`padding: 0 12px`).
const ROW_INSET_X: f32 = 12.0;
/// Between rows in the scroll region (`gap: 2px`).
const ROW_GAP: f32 = 2.0;
/// The gap between a row's glyph and its label.
const GLYPH_GAP: f32 = 12.0;

/// The rail's New item button.
pub const NEW_ITEM_LABEL: &str = "New item";
/// The closed New folder row.
pub const NEW_FOLDER_LABEL: &str = "New folder";
/// The open New folder row's placeholder (design 1a, verbatim).
pub const NEW_FOLDER_PLACEHOLDER: &str = "Folder name, then Enter";
/// The pinned Lock row.
pub const LOCK_ROW_LABEL: &str = "Lock";
/// The TOOLS section's Settings row.
pub const SETTINGS_ROW_LABEL: &str = "Settings";

/// The colours the rail paints in this frame: the window publishes the
/// user's style into the context (`kit::tokens::set_sidebar_style`), so the
/// rail and every test that draws it read one answer.
fn sb(ui: &egui::Ui) -> crate::kit::tokens::SidebarPalette {
    crate::kit::tokens::sidebar_style(ui.ctx()).palette()
}

/// The glyph a filter's row wears: the design's own for the rows it draws, and
/// one in the same stroke language for the rows 0.15 has and 1a does not.
fn glyph_for(filter: &SidebarFilter) -> &'static str {
    use crate::kit::icon::glyph;
    match filter {
        SidebarFilter::All => glyph::ALL,
        SidebarFilter::Favorites => glyph::FAV,
        SidebarFilter::Apps => glyph::APPS,
        SidebarFilter::Logins => glyph::LOGIN,
        SidebarFilter::Passkeys => glyph::PASSKEY,
        SidebarFilter::Cards => glyph::CARD,
        SidebarFilter::Identities => glyph::IDENTITY,
        SidebarFilter::SecureNotes => glyph::NOTE,
        SidebarFilter::SshKeys => glyph::SSH,
        SidebarFilter::Archive => glyph::ARCHIVE,
        SidebarFilter::Trash => glyph::TRASH,
        SidebarFilter::Organisation(_) => glyph::ORG,
        SidebarFilter::Folder(_) | SidebarFilter::Collection(_) | SidebarFilter::Unfiled => {
            glyph::FOLDER
        }
    }
}

/// One VAULT-section row backed by a [`SidebarFilter`], drawn and handled.
///
/// Extracted so every item row clears the screen flags, which is the
/// invariant that stops the window sitting on the Sends or Password health
/// screen while the rail highlights Cards.
fn item_row(
    ui: &mut egui::Ui,
    label: &str,
    filter: SidebarFilter,
    lists: VaultLists<'_>,
    selected: &mut SidebarFilter,
    screens: &mut Screens<'_>,
) {
    // NOT `count_for(items, ..)`: Archive and Trash read a different list,
    // and counting them against the live snapshot is precisely the
    // always-zero badge those rows shipped with. `badge_for` picks the list,
    // and returns `None` while it has not been fetched.
    let count = badge_text(badge_for(&filter, lists));
    let width = ui.available_width();
    let selected_now = *selected == filter && !screens.any();
    if sidebar_row(ui, glyph_for(&filter), label, Some(&count), selected_now, width, 0.0, ROW_HEIGHT)
        .clicked()
    {
        *selected = filter;
        screens.clear();
    }
}

pub fn draw_sidebar(
    ui: &mut egui::Ui,
    lists: VaultLists<'_>,
    folders: &[Folder],
    selected: &mut SidebarFilter,
    // Which of the screen rows -- Shared, Password health, Shared with me --
    // is the one selected, rather than any of the item filters. See
    // [`Screens`] for the "at most one, and any item row clears them" rule.
    mut screens: Screens<'_>,
    lock_countdown: &str,
    // Whether this window's backend can take an item out of a folder --
    // `VaultBackend::can_unfile_items`. Read by the drop decision alone; see
    // [`drop_outcomes`].
    may_unfile: bool,
) -> SidebarAction {
    let mut action = SidebarAction::None;
    let palette = sb(ui);

    ui.vertical(|ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        // The logo and New item sit above the scrolling rows, at the rows'
        // width: the lane the bar lives in is the panel's right padding.
        let head_width = (ui.available_width() - f32::from(PANEL_PAD_X)).max(0.0);
        logo_row(ui, &palette, head_width);
        if new_item_button(ui, &palette, head_width).clicked() {
            action = SidebarAction::NewItem;
        }
        ui.add_space(18.0);

        // **The rail scrolls, and its bar has a lane.** A vault with a folder
        // for every letter must still reach its last rows, and the bar must
        // sit beside the rows rather than on them -- see [`PANEL_PAD_X`] and
        // `theme::scrollbar_in_gutter`, which reserves the lane down this ui's
        // right edge. The width read here is re-imposed on the scroll
        // content so a rail that grows past the fold does not slide every row
        // sideways when the bar appears.
        theme::scrollbar_in_gutter(ui, f32::from(PANEL_PAD_X));
        let rail_width = (ui.available_width() - f32::from(PANEL_PAD_X)).max(0.0);
        // The Lock row is pinned below the scroll region, so the region
        // leaves it its band whatever the row count.
        let lock_band = ROW_HEIGHT + LOCK_ROW_GAP_ABOVE;
        egui::ScrollArea::vertical()
            .id_salt("vault-rail")
            .max_height((ui.available_height() - lock_band).max(0.0))
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.set_max_width(rail_width);
                // Zeroed for the block: every gap here is deliberate and
                // spelled out, and `ROW_GAP` is applied around each row loop.
                ui.spacing_mut().item_spacing.y = 0.0;

                section_label(ui, "Vault", true);
                ui.spacing_mut().item_spacing.y = ROW_GAP;
                for (label, filter) in [
                    ("All items", SidebarFilter::All),
                    ("Favorites", SidebarFilter::Favorites),
                    // Directly after Favorites, by the owner's instruction
                    // ("it is our main feature").
                    ("Apps", SidebarFilter::Apps),
                    ("Logins", SidebarFilter::Logins),
                    ("Passkeys", SidebarFilter::Passkeys),
                    ("Cards", SidebarFilter::Cards),
                    ("Identities", SidebarFilter::Identities),
                    ("Secure notes", SidebarFilter::SecureNotes),
                    ("SSH keys", SidebarFilter::SshKeys),
                    // The section's outer edge: items put away rather than
                    // items in use.
                    ("Archive", SidebarFilter::Archive),
                    ("Trash", SidebarFilter::Trash),
                ] {
                    // **The 0.15 kinds design 1a does not draw stay out of
                    // sight while they hold nothing**: Passkeys, Identities
                    // and SSH keys appear the moment an item of theirs does
                    // (or while selected), so the rail stays the design's
                    // length for a vault without them and loses nothing for
                    // one with them.
                    let optional = matches!(
                        filter,
                        SidebarFilter::Passkeys | SidebarFilter::Identities | SidebarFilter::SshKeys
                    );
                    if optional && badge_for(&filter, lists) == Some(0) && *selected != filter {
                        continue;
                    }
                    item_row(ui, label, filter, lists, selected, &mut screens);
                }
                ui.spacing_mut().item_spacing.y = 0.0;

                section_label(ui, "Tools", false);
                if tools_rows(ui, lists, &mut screens) {
                    action = SidebarAction::OpenSettings;
                }

                section_label(ui, "Folders", false);
                ui.spacing_mut().item_spacing.y = ROW_GAP;
                // What is being dragged over this window right now, if
                // anything, and what each folder row would do with it. Both
                // read ONCE, before the loop: `drop_outcomes` returns one
                // verdict per folder in this exact order.
                let dragged = egui::DragAndDrop::payload::<crate::vault_window::item_list::DraggedItem>(
                    ui.ctx(),
                );
                let outcomes = dragged.as_ref().map(|item| drop_outcomes(folders, item, may_unfile));
                for (index, folder) in folders.iter().enumerate() {
                    // The virtual "No Folder" bucket gets the filter that says
                    // what it means; see `SidebarFilter::Unfiled`.
                    let filter = if is_virtual_folder(folder) {
                        SidebarFilter::Unfiled
                    } else {
                        SidebarFilter::Folder(folder.id.clone())
                    };
                    let count = badge_text(badge_for(&filter, lists));
                    // Reserve the pencil's lane *before* the row claims the
                    // rest of the width -- see `FOLDER_EDIT_BUTTON_WIDTH`.
                    let row_width = (ui.available_width() - FOLDER_EDIT_BUTTON_WIDTH).max(0.0);
                    let response = sidebar_row(
                        ui,
                        crate::kit::icon::glyph::FOLDER,
                        &folder.name,
                        Some(&count),
                        *selected == filter && !screens.any(),
                        row_width,
                        0.0,
                        FOLDER_ROW_HEIGHT,
                    );
                    if response.clicked() {
                        *selected = filter.clone();
                        screens.clear();
                    }
                    // The drop half of the row: an OUTLINE over the rect the
                    // row already claimed, so it cannot change the pitch. Every
                    // folder that would take the item says so for the whole
                    // drag; a refusal is shown, not swallowed.
                    if let Some(outcome) = outcomes.as_ref().map(|o| &o[index]) {
                        let hot = response.contains_pointer();
                        let (color, width) = match outcome {
                            DropOutcome::Accept => (theme::BLUE_BRIGHT, if hot { 2.0 } else { 1.0 }),
                            DropOutcome::Refuse(_) => (theme::ERROR, if hot { 2.0 } else { 1.0 }),
                        };
                        ui.painter().rect_stroke(
                            response.rect,
                            CornerRadius::same(12),
                            egui::Stroke::new(width, color),
                            egui::StrokeKind::Inside,
                        );
                        if hot {
                            ui.ctx().set_cursor_icon(match outcome {
                                DropOutcome::Accept => egui::CursorIcon::Grabbing,
                                DropOutcome::Refuse(_) => egui::CursorIcon::NotAllowed,
                            });
                        }
                    }
                    // Consumed on EVERY folder row, accepting or not: the
                    // gesture ended here.
                    if let Some(item) =
                        response.dnd_release_payload::<crate::vault_window::item_list::DraggedItem>()
                    {
                        action = match drop_outcomes(folders, &item, may_unfile)[index] {
                            DropOutcome::Accept => SidebarAction::MoveItemToFolder {
                                item_id: item.id.clone(),
                                // The virtual row's id is the EMPTY STRING;
                                // `None` is what that row means.
                                folder_id: (!is_virtual_folder(folder)).then(|| folder.id.clone()),
                            },
                            DropOutcome::Refuse(reason) => SidebarAction::RefusedMove(reason),
                        };
                    }
                    // The pencil, in the lane the row gave up, **shown while
                    // the row or the pencil is under the pointer** (design 1a
                    // draws no pencil; rename is 0.15's and stays). The
                    // virtual "No Folder" bucket has nothing to rename.
                    if !is_virtual_folder(folder) {
                        let edit_rect = egui::Rect::from_min_size(
                            egui::pos2(response.rect.right(), response.rect.top()),
                            egui::vec2(FOLDER_EDIT_BUTTON_WIDTH, response.rect.height()),
                        );
                        let edit = ui.interact(
                            edit_rect,
                            egui::Id::new(("folder-edit", folder.id.as_str())),
                            egui::Sense::click(),
                        );
                        if response.hovered() || edit.hovered() {
                            let tint = if edit.hovered() { palette.text } else { palette.nav_fg };
                            crate::kit::icon::stroke(
                                ui.painter(),
                                egui::Rect::from_center_size(edit_rect.center(), egui::Vec2::splat(15.0)),
                                crate::kit::icon::glyph::PENCIL,
                                2.0,
                                tint,
                            );
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if edit.clicked() {
                            action = SidebarAction::EditFolder(folder.id.clone());
                        }
                    }
                }
                received_row(ui, lists, &mut screens);
                if let Some(name) = new_folder_row(ui, &palette) {
                    action = SidebarAction::CreateFolder(name);
                }
                ui.spacing_mut().item_spacing.y = 0.0;

                // Only when there is something in it; see `draw_organisations`.
                draw_organisations(ui, lists, selected, &mut screens);
            });

        // The Lock row, on the rail's floor whatever the row count.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            if lock_row(ui, &palette, lock_countdown).clicked() {
                action = SidebarAction::Lock;
            }
        });
    });

    action
}

/// Air between the scroll region and the Lock row below it.
const LOCK_ROW_GAP_ABOVE: f32 = 4.0;

/// The logo row: the mark at 28x32 in the sidebar's colourway, then the
/// wordmark (design 1a: `padding: 2px 10px 22px`, gap 11).
fn logo_row(ui: &mut egui::Ui, palette: &crate::kit::tokens::SidebarPalette, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 2.0 + 32.0 + 22.0), egui::Sense::hover());
    let mark = egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 2.0), egui::vec2(28.0, 32.0));
    theme::paint_mark_colours(ui.painter(), mark, theme::mark_on_sidebar(palette.q1));
    let word = ui
        .painter()
        .layout_job(crate::kit::tokens::job(crate::kit::tokens::WORDMARK, "Deskwarden", palette.text));
    let at = egui::pos2(mark.right() + 11.0, mark.center().y - word.size().y / 2.0);
    ui.painter().galley(at, word, palette.text);
}

/// New item (design 1a): 46 high, radius 12, in the style's inverse colours,
/// lifting a point under the pointer.
fn new_item_button(
    ui: &mut egui::Ui,
    palette: &crate::kit::tokens::SidebarPalette,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 46.0), egui::Sense::click());
    let lift = ui.ctx().animate_bool_with_time(response.id.with("lift"), response.hovered(), 0.15);
    let rect = rect.translate(egui::vec2(0.0, -lift));
    ui.painter().rect_filled(rect, CornerRadius::same(12), palette.new_bg);
    let plus = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 16.0, rect.center().y - 9.0),
        egui::Vec2::splat(18.0),
    );
    crate::kit::icon::stroke(ui.painter(), plus, crate::kit::icon::glyph::PLUS, 2.4, palette.new_fg);
    let role = crate::kit::tokens::Role { size: 15.0, face: theme::BOLD, tracking_em: 0.0 };
    let label = ui.painter().layout_job(crate::kit::tokens::job(role, NEW_ITEM_LABEL, palette.new_fg));
    ui.painter().galley(
        egui::pos2(plus.right() + 10.0, rect.center().y - label.size().y / 2.0),
        label,
        palette.new_fg,
    );
    crate::kit::tips::hint(ui, &response, NEW_ITEM_LABEL, &["Mod", "N"]);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
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
    "\"No Folder\" isn't a place an item can be moved to \u{2014} the Bitwarden CLI this build talks \
     to cannot take an item out of a folder, so the drop was refused rather than looking like \
     it worked.";

/// Why the folder an item already lives in refuses it. The same fact the
/// row menu's "Move to folder" submenu greys that destination for.
pub const ALREADY_IN_THIS_FOLDER: &str = "That item is already in this folder.";

/// The same refusal for the "No Folder" row, on a backend that CAN un-file:
/// the item is not in a folder, so there is nothing for the drop to do.
///
/// Its own sentence rather than a reuse of [`ALREADY_IN_THIS_FOLDER`],
/// because "already in this folder" is untrue of a row that is not a folder
/// -- and a refusal a user cannot square with what they are looking at is
/// worse than the silent no-op it replaced.
pub const ALREADY_IN_NO_FOLDER: &str = "That item is not in a folder.";

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
pub fn drop_outcomes(
    folders: &[Folder],
    dragged: &crate::vault_window::item_list::DraggedItem,
    // Whether this window's backend can actually take an item out of a
    // folder -- `VaultBackend::can_unfile_items`. See the `CANNOT_UNFILE`
    // arm below, which is what it decides.
    may_unfile: bool,
) -> Vec<DropOutcome> {
    let assignable = super::detail_edit::assignable_folders(folders);
    folders
        .iter()
        .map(|folder| {
            if !assignable.iter().any(|f| f.id == folder.id) {
                // The only way `assignable_folders` drops a row is
                // `is_virtual_folder`, so this is the un-file case -- and
                // whether it works is a fact about the BACKEND rather than
                // about this row. `bw serve` accepts every spelling of "no
                // folder" with a 200 and moves nothing, which is why this
                // was an unconditional refusal; the direct-REST backend
                // replaces the whole cipher, so there the drop is a real
                // move and refusing it would be this app withholding
                // something that works.
                if !may_unfile {
                    return DropOutcome::Refuse(CANNOT_UNFILE);
                }
                // Already unfiled: a write that would achieve nothing, and
                // the same refusal the row below gives for the folder an
                // item already lives in.
                return if dragged.folder_id.as_deref().unwrap_or("").is_empty() {
                    DropOutcome::Refuse(ALREADY_IN_NO_FOLDER)
                } else {
                    DropOutcome::Accept
                };
            }
            if dragged.folder_id.as_deref() == Some(folder.id.as_str()) {
                return DropOutcome::Refuse(ALREADY_IN_THIS_FOLDER);
            }
            DropOutcome::Accept
        })
        .collect()
}

/// A section header (VAULT / TOOLS / FOLDERS / ORGANISATIONS): the design's
/// 12px Bold capitals tracked 0.12em, in the style's muted ink, 12 in from the
/// rows' edge, with `padding: 0 12px 6px` -- and 20 above every section but
/// the first.
fn section_label(ui: &mut egui::Ui, text: &str, first: bool) -> egui::Rect {
    let palette = sb(ui);
    if !first {
        ui.add_space(20.0);
    }
    let job = crate::kit::tokens::job(crate::kit::tokens::SECTION, text, palette.muted);
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), galley.size().y + 6.0),
        egui::Sense::hover(),
    );
    ui.painter().galley(egui::pos2(rect.left() + ROW_INSET_X, rect.top()), galley, palette.muted);
    rect
}

/// What the Lock row's right-hand count shows: the running time out of
/// "Locks in m:ss", or "Off" when auto-lock is off.
pub(crate) fn lock_count(countdown: &str) -> &str {
    if let Some(time) = countdown.strip_prefix("Locks in ") {
        time
    } else if countdown == "Auto-lock is off" {
        "Off"
    } else {
        countdown
    }
}

/// The pinned Lock row (design 1a), carrying 0.15's auto-lock countdown as its
/// count. Its width gives back the bar's lane, as every row above does.
fn lock_row(
    ui: &mut egui::Ui,
    palette: &crate::kit::tokens::SidebarPalette,
    countdown: &str,
) -> egui::Response {
    let width = (ui.available_width() - f32::from(PANEL_PAD_X)).max(0.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    paint_row(
        ui,
        palette,
        rect,
        response.hovered(),
        false,
        crate::kit::icon::glyph::LOCK,
        LOCK_ROW_LABEL,
        Some(lock_count(countdown)),
        0.0,
    );
    crate::kit::tips::hint(ui, &response, LOCK_ROW_LABEL, &["Mod", "L"]);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The design's own name for the organisations section.
///
/// **British, matching this crate's prose and not Bitwarden's own UI**, which
/// says `Organizations`. Every doc comment in `rest::sync`, `rest::backend`
/// and `rest::organizations` spells it with an s -- the wire spells it
/// `organizationId` and always will, because that is the protocol -- and a
/// rail heading that disagreed with every sentence written about it would be
/// a difference a reader has to resolve. One constant, so the owner can
/// disagree in one edit.
pub const ORGANISATIONS_SECTION_LABEL: &str = "ORGANISATIONS";

/// **Design 5b's SHARING section, one section up: the organisations this
/// account is in, and their collections indented underneath.**
///
/// # Where organisation items appear, and what was rejected
///
/// The brief left this open, and 8a's `Owner` row and 5b's sidebar disagree
/// slightly about it. Three arrangements were on the table.
///
/// **A badge on every organisation-owned row in the item list** was rejected,
/// and not only because `item_list.rs` belongs to somebody else this week. A
/// per-row badge is paid for by every row in the pane, on an account where
/// the overwhelming majority of items are personal -- the owner's own vault
/// is 1,654 personal items and zero shared ones -- so the common case pays
/// list-wide visual weight for a fact that is false about every row. It is
/// also the wrong altitude: "which of these is shared" is a question asked
/// about a *set*, and the rail is where this app answers set questions.
///
/// **One flat `Shared` row**, a single cut of everything organisation-owned,
/// was rejected as the opposite mistake. It is one row and it says almost
/// nothing: the useful question is not "is this shared" but "shared with
/// whom", and a single row answers the first while making the second
/// unaskable from the rail.
///
/// **What is drawn instead**: one row per organisation, its collections
/// indented under it. That answers both questions at once -- the parent names
/// the *whom*, the children name the *what* -- with a shape the rail already
/// has, and it costs no new interaction vocabulary at all.
///
/// **This is also the answer to "surface which items are org-owned".** It is
/// not a separate mechanism: `SidebarFilter::Organisation`'s items are the
/// org-owned ones, and `item_list::matches_filter` delegates to
/// [`SidebarFilter::scope_contains`], so clicking the row lists exactly them,
/// with the pane's own search and count following. Nothing in `item_list.rs`
/// changed, and nothing had to.
///
/// # Above the screens, not below them, and not inside FOLDERS
///
/// [`screen_rows`] draws the boundary this section is placed against: every
/// row above the first hairline names a **cut of the vault**, and the rows
/// below the Password health divider name a **screen**. An organisation row
/// is the first kind -- it is a predicate over `VaultItem`s, it reads the
/// live snapshot, and it goes through the item pane -- so it belongs on the
/// cuts side of that line, which means above Password health.
///
/// It is a section of its own rather than more rows in FOLDERS because a
/// folder is this account's own filing and a collection is somebody else's;
/// see [`SidebarFilter::Collection`], which is where the second variant is
/// argued. Concretely, every FOLDERS row carries an edit pencil, and there is
/// no rename, no delete and no create for a collection in this app at all.
///
/// # Nothing at all when there is nothing
///
/// **The section does not exist for an account with no organisations** -- no
/// label, no divider, not a pixel -- which is the owner's own situation and
/// the one the shape had to survive. That is one early return rather than a
/// rule spread over the drawing below, and it covers four situations at once
/// for [`VaultLists::sharing`]'s stated reason: no organisations, a
/// `bw serve` backend, a server with no organisation routes, and a vault that
/// has not synced yet. `the_rail_is_byte_for_byte_unchanged_without_an_\
/// organisation` is the assertion, and it compares the painted output of a
/// rail with an empty directory against one built before this section
/// existed.
///
/// The divider is [`inset_hairline`] at the same 14px above and below the
/// other three, because it is meant to read as the same kind of boundary.
fn draw_organisations(
    ui: &mut egui::Ui,
    lists: VaultLists<'_>,
    selected: &mut SidebarFilter,
    screens: &mut Screens<'_>,
) {
    // **The whole of the empty case.** Not a guard around each paint below,
    // and not a caller's responsibility: a section that is dead weight for an
    // account with none is impossible from here rather than discouraged.
    // Sharing orgs never reach this rail: they are plumbing (spec 6.9), and
    // their records are under Sent and Shared with me.
    let organisations = lists.sharing.visible_organisations();
    if organisations.is_empty() {
        return;
    }

    section_label(ui, ORGANISATIONS_SECTION_LABEL, false);
    ui.spacing_mut().item_spacing.y = ROW_GAP;
    for organisation in organisations {
        // `item_row`, the same helper every VAULT row uses, so an
        // organisation row clears the screen flags exactly as they do --
        // the invariant `Screens` exists to hold, obtained rather than
        // re-implemented. It also means the badge comes from `badge_for`,
        // so the count and the pane's contents are one read.
        item_row(
            ui,
            organisation.label(),
            SidebarFilter::Organisation(organisation.id.clone()),
            lists,
            selected,
            screens,
        );
        for collection in lists.sharing.collections_of(&organisation.id) {
            let filter = SidebarFilter::Collection(collection.id.clone());
            let count = badge_for(&filter, lists);
            let width = ui.available_width();
            let on = *selected == filter && !screens.any();
            // `SUB_ROW_EXTRA_INDENT` -- 5b's own sub-row indent, reused
            // rather than re-chosen. A second indent value here would put
            // two different depths of "child row" in one rail for no reason
            // a reader could recover.
            let count = Some(badge_text(count));
            if sidebar_row(
                ui,
                crate::kit::icon::glyph::FOLDER,
                collection.label(),
                count.as_deref(),
                on,
                width,
                SUB_ROW_EXTRA_INDENT,
                FOLDER_ROW_HEIGHT,
            )
            .clicked()
            {
                *selected = filter;
                screens.clear();
            }
        }
    }
    ui.spacing_mut().item_spacing.y = 0.0;
}

/// The TOOLS section: Shared (the Sent screen), Password health, Settings.
/// Returns whether Settings was clicked.
///
/// **Shared and Password health are screens, not cuts of the vault**: Sends
/// are not `VaultItem`s, and password reuse is a property of a PAIR of items,
/// which no per-item `scope_contains` can answer. So they are flags in
/// [`Screens`] rather than [`SidebarFilter`] variants, and [`Screens::select`]
/// is the only way in.
/// Slice 6: the TOOLS row for the Generator screen.
pub const GENERATOR_ROW_LABEL: &str = "Generator";

fn tools_rows(ui: &mut egui::Ui, lists: VaultLists<'_>, screens: &mut Screens<'_>) -> bool {
    use crate::kit::icon::glyph;
    ui.spacing_mut().item_spacing.y = ROW_GAP;
    {
        let width = ui.available_width();
        let on = screens.sharing_is(SendScope::All);
        // Sends and person shares in one count (spec 6.10). Unknown Sends
        // with no shares stay unknown; unknown Sends with shares count the
        // shares -- a PARTIAL count, knowingly, over an en dash that would
        // hide shares the window does know about.
        let count = match (lists.sends.map(|c| c.all), lists.shared_records) {
            (Some(sends), shares) => Some(sends + shares),
            (None, 0) => None,
            (None, shares) => Some(shares),
        };
        let count = badge_text(count);
        if sidebar_row(ui, glyph::SHARED, SENDS_ROW_LABEL, Some(&count), on, width, 0.0, ROW_HEIGHT)
            .clicked()
        {
            screens.select_sharing(SendScope::All);
        }
    }
    {
        // Distinct items with a finding: always known, so never an en dash.
        let width = ui.available_width();
        let on = screens.is(Screen::Health);
        let label = crate::vault_window::password_health::HEALTH_ROW_LABEL;
        let count = badge_text(Some(lists.health_findings));
        if sidebar_row(ui, glyph::HEALTH, label, Some(&count), on, width, 0.0, ROW_HEIGHT).clicked() {
            screens.select(Screen::Health);
        }
    }
    {
        // Slice 6: the Generator screen.
        let width = ui.available_width();
        let on = screens.is(Screen::Generator);
        if sidebar_row(ui, glyph::GEN, GENERATOR_ROW_LABEL, None, on, width, 0.0, ROW_HEIGHT).clicked() {
            screens.select(Screen::Generator);
        }
    }
    let width = ui.available_width();
    let settings_on = screens.is(Screen::Settings);
    let settings = sidebar_row(ui, glyph::SETTINGS, SETTINGS_ROW_LABEL, None, settings_on, width, 0.0, ROW_HEIGHT)
        .clicked();
    ui.spacing_mut().item_spacing.y = 0.0;
    settings
}

/// Shared with me (design 1a lists it among the folders): the records and
/// Sends other people shared with this user. Its count is never unknown --
/// see `VaultLists::received`.
fn received_row(ui: &mut egui::Ui, lists: VaultLists<'_>, screens: &mut Screens<'_>) {
    let palette = sb(ui);
    let width = ui.available_width();
    let on = screens.is(Screen::Received);
    let count = badge_text(Some(lists.received + lists.received_records));
    let row = sidebar_row(
        ui,
        crate::kit::icon::glyph::RECEIVED,
        RECEIVED_ROW_LABEL,
        Some(&count),
        on,
        width,
        0.0,
        FOLDER_ROW_HEIGHT,
    );
    if lists.invitation_waiting {
        // Somebody is waiting for an answer: a 6px dot, 6px left of the
        // count, placed off the count's MEASURED width.
        let count_width = count_galley(ui, &count, palette.nav_fg).size().x;
        let at = egui::pos2(row.rect.right() - ROW_INSET_X - count_width - 6.0 - 3.0, row.rect.center().y);
        ui.painter().circle_filled(at, 3.0, theme::BLUE_BRIGHT);
    }
    if row.clicked() {
        screens.select(Screen::Received);
    }
}

/// The New folder row (design 1a): closed, a plus and "New folder" in the
/// muted ink; open, an input in the hover tint that creates the folder on
/// Enter and closes on Esc or when it loses focus. Returns the trimmed name
/// when one was submitted.
fn new_folder_row(ui: &mut egui::Ui, palette: &crate::kit::tokens::SidebarPalette) -> Option<String> {
    let state_id = egui::Id::new("vault-rail-new-folder");
    let edit_id = state_id.with("input");
    let open = ui.data(|d| d.get_temp::<bool>(state_id)).unwrap_or(false);
    let width = ui.available_width();
    if !open {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, FOLDER_ROW_HEIGHT), egui::Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(12), palette.hover);
        }
        let glyph = egui::Rect::from_min_size(
            egui::pos2(rect.left() + ROW_INSET_X, rect.center().y - 9.0),
            egui::Vec2::splat(18.0),
        );
        crate::kit::icon::stroke(ui.painter(), glyph, crate::kit::icon::glyph::PLUS, 2.0, palette.muted);
        let label = ui
            .painter()
            .layout_job(crate::kit::tokens::job(crate::kit::tokens::NAV, NEW_FOLDER_LABEL, palette.muted));
        ui.painter().galley(
            egui::pos2(glyph.right() + GLYPH_GAP, rect.center().y - label.size().y / 2.0),
            label,
            palette.muted,
        );
        if response.clicked() {
            ui.data_mut(|d| {
                d.insert_temp(state_id, true);
                d.insert_temp(edit_id, String::new());
            });
            ui.memory_mut(|m| m.request_focus(edit_id));
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
        return None;
    }

    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, FOLDER_ROW_HEIGHT), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(12), palette.hover);
    let mut name = ui.data(|d| d.get_temp::<String>(edit_id)).unwrap_or_default();
    let field = egui::TextEdit::singleline(&mut name)
        .id(edit_id)
        .frame(egui::Frame::NONE)
        .hint_text(egui::RichText::new(NEW_FOLDER_PLACEHOLDER).color(palette.muted).size(14.0))
        .text_color(palette.text)
        .font(egui::FontId::new(14.0, egui::FontFamily::Proportional))
        .desired_width(width - 2.0 * ROW_INSET_X);
    let inner = rect.shrink2(egui::vec2(ROW_INSET_X, 0.0));
    let response = ui.put(inner, field);
    ui.data_mut(|d| d.insert_temp(edit_id, name.clone()));
    if response.lost_focus() {
        let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.data_mut(|d| {
            d.insert_temp(state_id, false);
            d.remove::<String>(edit_id);
        });
        let trimmed = name.trim();
        if entered && !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    None
}

/// How much further in a collection row's glyph and label sit than its
/// organisation's. Horizontal only, so the row pitch stays uniform.
const SUB_ROW_EXTRA_INDENT: f32 = 23.0;

/// A row's count, laid out as [`sidebar_row`] paints it -- one place for its
/// type, so whatever sits beside the count measures the same text.
fn count_galley(ui: &egui::Ui, text: &str, color: egui::Color32) -> std::sync::Arc<egui::Galley> {
    let role = crate::kit::tokens::Role { size: 13.0, face: theme::REGULAR, tracking_em: 0.0 };
    ui.painter().layout_job(crate::kit::tokens::job(role, text, color.gamma_multiply(0.7)))
}

/// One rail row: glyph, label, right-aligned count, allocated at exactly
/// `width` by `height` (design 1a: 44 for nav rows, 40 for folder rows,
/// radius 12, `padding: 0 12px`, gap 12, 15px SemiBold, 19px glyph).
///
/// Selected rows take the style's active ground and ink; others its nav ink
/// with the hover tint. `indent` moves the glyph and label only -- never the
/// band, the hover rect or the count -- so a collection row is as easy to hit
/// as its organisation.
///
/// Every part is PAINTED rather than added as a child widget: a nested
/// layout container rewinds the parent cursor to its own content height,
/// which is what once made rows overlap. The one `allocate_exact_size` is the
/// only thing that moves the cursor.
#[allow(clippy::too_many_arguments)]
fn sidebar_row(
    ui: &mut egui::Ui,
    glyph: &str,
    label: &str,
    count: Option<&str>,
    selected: bool,
    width: f32,
    indent: f32,
    height: f32,
) -> egui::Response {
    let palette = sb(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    paint_row(ui, &palette, rect, response.hovered(), selected, glyph, label, count, indent);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The painting half of [`sidebar_row`], shared with the Lock row.
#[allow(clippy::too_many_arguments)]
fn paint_row(
    ui: &egui::Ui,
    palette: &crate::kit::tokens::SidebarPalette,
    rect: egui::Rect,
    hovered: bool,
    selected: bool,
    glyph: &str,
    label: &str,
    count: Option<&str>,
    indent: f32,
) {
    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(12), palette.active);
    } else if hovered {
        ui.painter().rect_filled(rect, CornerRadius::same(12), palette.hover);
    }
    let ink = if selected { palette.active_fg } else { palette.nav_fg };
    // One 19pt glyph box on every row, with a folder row's 18pt glyph centred
    // in it, so every label starts on the same x whatever the row's height.
    let glyph_size = if rect.height() < ROW_HEIGHT { 18.0 } else { 19.0 };
    let glyph_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + ROW_INSET_X + indent, rect.center().y - 19.0 / 2.0),
        egui::Vec2::splat(19.0),
    );
    crate::kit::icon::stroke(
        ui.painter(),
        egui::Rect::from_center_size(glyph_rect.center(), egui::Vec2::splat(glyph_size)),
        glyph,
        2.0,
        ink,
    );

    // Count first: it is right-aligned, and its measured width bounds how far
    // the label may run.
    let count_left = match count {
        Some(text) => {
            let galley = count_galley(ui, text, ink);
            let at = egui::pos2(
                rect.right() - ROW_INSET_X - galley.size().x,
                rect.center().y - galley.size().y / 2.0,
            );
            ui.painter().galley(at, galley, ink);
            at.x
        }
        None => rect.right() - ROW_INSET_X,
    };
    let label_left = glyph_rect.right() + GLYPH_GAP;
    let room = (count_left - 8.0 - label_left).max(1.0);
    // Ellipsised to the room left of the count, so a long folder name ends
    // in "…" rather than running underneath its own count.
    let galley = theme::truncated_galley(
        ui,
        crate::kit::tokens::job(crate::kit::tokens::NAV, label, ink),
        room,
        egui::TextStyle::Body,
    );
    ui.painter().galley(egui::pos2(label_left, rect.center().y - galley.size().y / 2.0), galley, ink);
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

    /// **The "No Folder" row is a real drop target where the backend can
    /// un-file, and a visible refusal where it cannot.**
    ///
    /// This row was refused unconditionally, and the refusal was the right
    /// answer for the only backend that existed: `bw serve` accepts every
    /// spelling of "no folder" with a 200 and moves nothing
    /// (`.superpowers/sdd/put-semantics-capture.md`), so an accepting row
    /// would have been a drop that appeared to work until the next sync put
    /// the item back. The direct-REST backend replaces the whole cipher, so
    /// there the same drop is a real move -- and refusing it would be this
    /// app withholding something that works.
    ///
    /// Both directions in one test, because each alone passes against a row
    /// that ignores the flag.
    #[test]
    fn the_no_folder_row_accepts_a_drop_only_where_the_backend_can_unfile() {
        let folders = [folder("", "No Folder"), folder("f1", "Work")];
        let filed = dragged(Some("f1"));
        assert_eq!(
            drop_outcomes(&folders, &filed, true)[0],
            DropOutcome::Accept,
            "a backend that can un-file still refused the drop"
        );
        assert_eq!(
            drop_outcomes(&folders, &filed, false)[0],
            DropOutcome::Refuse(CANNOT_UNFILE),
            "a backend that cannot un-file accepted a drop that would do nothing"
        );
        // **The rest of the rail is unaffected either way** -- the flag
        // decides one row, not the rail. Asserted as an equality between the
        // two runs rather than against `Accept`, because the folder this item
        // is already in is correctly refused and that refusal is not what
        // this test is about.
        assert_eq!(
            &drop_outcomes(&folders, &filed, true)[1..],
            &drop_outcomes(&folders, &filed, false)[1..],
            "the un-file flag changed a row that is not the \"No Folder\" one"
        );
    }

    /// **An item that is already out of every folder is refused, with its own
    /// sentence.**
    ///
    /// The same treatment the folder an item already lives in gets, and for
    /// the same reason -- the drop would be a write that achieves nothing.
    /// Its own wording because "already in this folder" is untrue of a row
    /// that is not a folder, and a refusal a user cannot square with what
    /// they are looking at is worse than the no-op it replaced.
    #[test]
    fn dropping_an_unfiled_item_on_the_no_folder_row_is_refused_for_its_own_reason() {
        let folders = [folder("", "No Folder"), folder("f1", "Work")];
        for already in [None, Some("")] {
            assert_eq!(
                drop_outcomes(&folders, &dragged(already), true)[0],
                DropOutcome::Refuse(ALREADY_IN_NO_FOLDER),
                "with folder_id {already:?} the drop was not refused for being a no-op"
            );
        }
        assert_ne!(
            ALREADY_IN_NO_FOLDER, ALREADY_IN_THIS_FOLDER,
            "the two refusals read alike, so one of them is describing the wrong row"
        );
    }

    #[test]
    fn an_unfiled_item_may_go_into_any_real_folder_and_nowhere_else() {
        assert_eq!(
            drop_outcomes(&a_real_looking_vault(), &dragged(None), false),
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
                drop_outcomes(&a_real_looking_vault(), &dragged(from), false)[0],
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
            drop_outcomes(&a_real_looking_vault(), &dragged(Some("f1")), false),
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
        let outcomes = drop_outcomes(&folders, &dragged(Some("f2")), false);
        assert_eq!(outcomes.len(), folders.len());
        assert_eq!(outcomes[2], DropOutcome::Refuse(ALREADY_IN_THIS_FOLDER));
    }

    #[test]
    fn a_vault_with_no_real_folders_accepts_nothing() {
        assert_eq!(
            drop_outcomes(&[folder("", "No Folder")], &dragged(None), false),
            vec![DropOutcome::Refuse(CANNOT_UNFILE)]
        );
        assert_eq!(drop_outcomes(&[], &dragged(None), false), Vec::<DropOutcome>::new());
    }

    #[test]
    fn the_accepting_targets_are_exactly_the_assignable_folders_minus_the_current_one() {
        // Tied to `detail_edit::assignable_folders`, the EXISTING predicate
        // for "folders an item can be moved into", rather than to a second
        // copy of the same filter -- one definition of "virtual" is what
        // keeps this, the row menu's submenu and the edit form's dropdown
        // from drifting apart.
        let folders = a_real_looking_vault();
        let accepting: Vec<&str> = drop_outcomes(&folders, &dragged(Some("f1")), false)
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

    const SIDEBAR_WIDTH: f32 = crate::vault_window::SIDEBAR_WIDTH;
    const LIST_WIDTH: f32 = crate::vault_window::LIST_WIDTH;
    // Tall enough that the v2 rail's 44pt rows all fit: these tests are about
    // drag and drop, not about overflow.
    const HEIGHT: f32 = 2000.0;

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
            ssh_key: None,
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
                            action = draw_sidebar(
                                ui,
                                VaultLists::live_only(items),
                                folders,
                                &mut filter,
                                Screens {
                                    sends: &mut false,
                                    health: &mut false,
                                    received: &mut false,
                                    generator: &mut false,
                                    settings: &mut false,
                                    scope: &mut SendScope::All,
                                },
                                "Locks in 11:42",
                                false,
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(LIST_WIDTH, HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            draw_item_list(
                                ui,
                                Some(items),
                                folders,
                                &SidebarFilter::All,
                                &mut search,
                                &mut selected_id,
                                &icons,
                                false,
                                &mut visible,
                                None,
                                false,
                                // Not about the open gesture; the slide's
                                // arming is covered where it lives.
                                &mut false,
                        &std::collections::HashSet::new(),
                        &crate::vault_window::item_list::RowPeople::default(),
                        "All items",
                        false,
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

    // ## The two id diagnostics, and why only one of them is asserted on
    //
    // egui draws two different red warnings in a debug build, and they are
    // not the same claim. Neither reaches a release build, so a user is told
    // nothing either way:
    //
    // * **`warn_on_id_clash`** (`Context::check_for_id_clash`) fires when one
    //   id is used at two different rects *within a pass*, and paints a thin
    //   `error_fg_color` outline **plus a `🔥` text label**. This is the one
    //   worth guarding: two widgets sharing an id share state, so focus lands
    //   on the wrong row and a click registers against a neighbour. In
    //   release there is no outline, no log line, and nothing else in this
    //   suite that would notice.
    // * **`warn_if_rect_changes_id`** fires when a rect keeps its geometry
    //   *between passes* while its id changes, and paints a bare 2px
    //   `Color32::RED` outline with **no text at all**. That absence of text
    //   is the only thing telling the two apart on screen.
    //
    // The rail and the list are clean of both, but only the clash is
    // asserted on, matching `prefs_ui`'s guards -- where the second one does
    // fire, benignly, on a decorative separator. One rule across both
    // surfaces is easier to trust than two thresholds that have to be
    // remembered. [`rect_id_changes`] therefore exists only so the positive
    // control can prove the harness would see that shape of defect at all.

    /// Every **id clash** egui drew on a frame, read back off the shapes.
    fn id_clashes(run: &Run, out: &mut Vec<String>, at: &str) {
        for (text, _) in &run.texts {
            if text.contains('\u{1f525}') {
                out.push(format!("{at}: ID CLASH {text}"));
            }
        }
    }

    /// Every **rect that changed id between passes**. See the note above:
    /// collected for the positive control, not asserted on by the guards.
    fn rect_id_changes(run: &Run) -> Vec<String> {
        run.strokes
            .iter()
            .filter(|(_, stroke)| {
                stroke.color == egui::Color32::RED && (stroke.width - 2.0).abs() < 0.01
            })
            .map(|(rect, _)| format!("RECT CHANGED ID {rect:?}"))
            .collect()
    }

    /// Collect a frame's shapes into a bare [`Run`] -- only the texts and
    /// strokes the id guards read. The action and selection the gesture
    /// tests care about are not what these guards assert on.
    fn painted(output: &egui::FullOutput) -> Run {
        let mut run = Run {
            action: SidebarAction::None,
            texts: Vec::new(),
            strokes: Vec::new(),
            selected: None,
        };
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut run);
        }
        run
    }

    /// **No id clash as the vault changes underneath the rail.**
    ///
    /// The case the click sweep cannot reach. Sidebar rows carry egui's
    /// positional auto-ids, so inserting a row *above* another shifts every
    /// id below it while the rects can stay put -- which is exactly the
    /// shape `warn_if_rect_changes_id` looks for. This walks the transitions
    /// the real window makes: an empty vault filling in, folders arriving,
    /// the countdown ticking, the filter moving, and both screen flags
    /// toggling.
    #[test]
    fn no_id_diagnostic_as_the_vault_folders_and_screens_change_underneath() {
        let (items, folders) = a_vault();
        let more_folders = {
            let mut f = folders.clone();
            f.push(folder("f3", "Receipts"));
            f.push(folder("f4", "Travel"));
            f
        };
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(SIDEBAR_WIDTH + LIST_WIDTH, HEIGHT),
        );
        let input = || egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut search = String::new();
        let mut selected_id: Option<String> = None;
        let icons = IconCache::default();
        let mut visible = Vec::new();
        let mut hits: Vec<String> = Vec::new();

        let step = |loaded: bool,
                        folders: &[Folder],
                        countdown: &str,
                        filter: &mut SidebarFilter,
                        sends: &mut bool,
                        health: &mut bool,
                        search: &mut String,
                        selected_id: &mut Option<String>,
                        visible: &mut Vec<String>,
                        hits: &mut Vec<String>,
                        at: String| {
            let output = ctx.run_ui(input(), |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(SIDEBAR_WIDTH, HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let lists = if loaded {
                                VaultLists::live_only(&items)
                            } else {
                                VaultLists::live_only(&[])
                            };
                            let _ = draw_sidebar(
                                ui,
                                lists,
                                folders,
                                filter,
                                Screens {
                                    sends,
                                    health,
                                    received: &mut false,
                                    generator: &mut false,
                                    settings: &mut false,
                                    scope: &mut SendScope::All,
                                },
                                countdown,
                                false,
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(LIST_WIDTH, HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            draw_item_list(
                                ui,
                                if loaded { Some(&items) } else { None },
                                folders,
                                &SidebarFilter::All,
                                search,
                                selected_id,
                                &icons,
                                false,
                                visible,
                                None,
                                false,
                                // Not about the open gesture; the slide's
                                // arming is covered where it lives.
                                &mut false,
                        &std::collections::HashSet::new(),
                        &crate::vault_window::item_list::RowPeople::default(),
                        "All items",
                        false,
                    );
                        },
                    );
                });
            });
            id_clashes(&painted(&output), hits, &at);
        };

        let plan: Vec<(bool, bool, &str, SidebarFilter, bool, bool)> = vec![
            (false, false, "Locks in 11:42", SidebarFilter::All, false, false),
            (true, false, "Locks in 11:41", SidebarFilter::All, false, false),
            (true, true, "Locks in 11:40", SidebarFilter::All, false, false),
            (true, true, "Locks in 9:59", SidebarFilter::Cards, false, false),
            (true, true, "Locks in 9:58", SidebarFilter::Cards, true, false),
            (true, false, "Locks in 9:57", SidebarFilter::Cards, false, true),
            (true, false, "Locks in 9:56", SidebarFilter::Trash, false, false),
            (false, false, "Locks in 9:55", SidebarFilter::All, false, false),
        ];
        for (loaded, many, countdown, want, s, h) in plan {
            // Per step, not carried across: what the rail is *shown* is the
            // input under test here, and letting a click on one frame decide
            // the next would make the sequence depend on where the pointer
            // happened to be rather than on the transition being exercised.
            let mut filter = want;
            let mut sends = s;
            let mut health = h;
            let f: &[Folder] = if many { &more_folders } else { &folders };
            step(
                loaded,
                f,
                countdown,
                &mut filter,
                &mut sends,
                &mut health,
                &mut search,
                &mut selected_id,
                &mut visible,
                &mut hits,
                format!("loaded={loaded} many={many} {countdown}"),
            );
        }
        hits.sort();
        hits.dedup();
        assert!(hits.is_empty(), "{hits:#?}");
    }

    /// **The control that stops every guard above from passing vacuously.**
    ///
    /// Both checks are egui's, not this crate's, and both are switched on by
    /// a *default* -- `warn_on_id_clash` and
    /// `style().debug.warn_if_rect_changes_id` are each `cfg!(debug_assertions)`
    /// out of the box. So the guards above assert on a diagnostic nobody in
    /// this crate turns on, which is precisely how this kind of test dies
    /// quietly: an egui release that flips a default, renames a field, moves
    /// the painting to another layer, or drops the `🔥` prefix would leave
    /// every one of them green while checking nothing at all.
    ///
    /// This hands egui one of each defect and insists it is reported, so
    /// that day is a failing test rather than a silent hole.
    #[test]
    fn the_id_diagnostics_are_reported_when_egui_is_handed_one_of_each() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 200.0));
        let input = || egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        let one = egui::Rect::from_min_size(egui::Pos2::new(10.0, 10.0), egui::vec2(20.0, 20.0));
        let two = egui::Rect::from_min_size(egui::Pos2::new(90.0, 90.0), egui::vec2(20.0, 20.0));

        // A CLASH: one id at two rects inside a single pass.
        let output = ctx.run_ui(input(), |ui| {
            let _ = ui.interact(one, egui::Id::new("control-clash"), egui::Sense::click());
            let _ = ui.interact(two, egui::Id::new("control-clash"), egui::Sense::click());
        });
        let mut hits = Vec::new();
        id_clashes(&painted(&output), &mut hits, "clash control");
        assert!(
            !hits.is_empty(),
            "egui reported no clash for two widgets sharing one id"
        );

        // A RECT THAT CHANGED ID: one rect, a different id on the next pass.
        let go = |name: &'static str| {
            ctx.run_ui(input(), |ui| {
                let _ = ui.interact(one, egui::Id::new(name), egui::Sense::click());
            })
        };
        let _ = go("control-a");
        let output = go("control-b");
        assert!(
            !rect_id_changes(&painted(&output)).is_empty(),
            "egui reported no id change for a rect that kept its geometry"
        );
    }

    /// **The rail and the list, clicked all over, report no id clash.**
    ///
    /// The report this exists for was a red flash on clicking "somewhere",
    /// so the gesture is swept across the whole pane rather than aimed only
    /// at the rows a fix would have been written for. Every click is
    /// followed by a settle frame: a click is reported on the frame it
    /// completes on, but what it selected is not drawn until the frame
    /// after, and that later frame is the one a mis-keyed row would show up
    /// on.
    ///
    /// This costs a couple of seconds and is worth them: see the note above
    /// [`id_clashes`] for why a clash that reaches a release build is
    /// invisible to the user and to every other test here.
    #[test]
    fn no_id_diagnostic_while_the_rail_and_the_list_are_clicked_all_over() {
        let (items, folders) = a_vault();
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(SIDEBAR_WIDTH + LIST_WIDTH, HEIGHT),
        );
        let input = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(Vec::new()), |_ui| {});

        let mut filter = SidebarFilter::All;
        let mut search = String::new();
        let mut selected_id: Option<String> = None;
        let icons = IconCache::default();
        let mut visible = Vec::new();
        let mut hits: Vec<String> = Vec::new();
        let mut draw = |events: Vec<egui::Event>, hits: &mut Vec<String>, at: String| {
            let output = ctx.run_ui(input(events), |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(SIDEBAR_WIDTH, HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let _ = draw_sidebar(
                                ui,
                                VaultLists::live_only(&items),
                                &folders,
                                &mut filter,
                                Screens {
                                    sends: &mut false,
                                    health: &mut false,
                                    received: &mut false,
                                    generator: &mut false,
                                    settings: &mut false,
                                    scope: &mut SendScope::All,
                                },
                                "Locks in 11:42",
                                false,
                            );
                        },
                    );
                    ui.allocate_ui_with_layout(
                        egui::vec2(LIST_WIDTH, HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            draw_item_list(
                                ui,
                                Some(&items),
                                &folders,
                                &SidebarFilter::All,
                                &mut search,
                                &mut selected_id,
                                &icons,
                                false,
                                &mut visible,
                                None,
                                false,
                                // Not about the open gesture; the slide's
                                // arming is covered where it lives.
                                &mut false,
                        &std::collections::HashSet::new(),
                        &crate::vault_window::item_list::RowPeople::default(),
                        "All items",
                        false,
                    );
                        },
                    );
                });
            });
            id_clashes(&painted(&output), hits, &at);
        };
        // One settle frame before the sweep, so the first click lands on a
        // pane that has already been laid out once rather than on frame 1 of
        // a fresh context.
        draw(Vec::new(), &mut hits, "warm-up".into());
        let mut y = 8.0;
        while y < HEIGHT {
            let mut x = 8.0;
            while x < SIDEBAR_WIDTH + LIST_WIDTH {
                let pos = egui::Pos2::new(x, y);
                let button = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                draw(
                    vec![egui::Event::PointerMoved(pos), button(true), button(false)],
                    &mut hits,
                    format!("click {pos:?}"),
                );
                draw(Vec::new(), &mut hits, format!("after {pos:?}"));
                x += 28.0;
            }
            y += 18.0;
        }
        hits.sort();
        hits.dedup();
        assert!(hits.is_empty(), "{hits:#?}");
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
                folder_id: Some("f2".to_string()),
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
                folder_id: Some("f2".to_string()),
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
        assert_eq!(stroke_of("Personal"), theme::BLUE_BRIGHT, "an accepting folder was not offered");
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
        let ctx = crate::kit::testing::ctx();
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
                let count = i.to_string();
                let response = sidebar_row(
                    ui,
                    crate::kit::icon::glyph::ALL,
                    "Row",
                    Some(&count),
                    false,
                    180.0,
                    0.0,
                    ROW_HEIGHT,
                );
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
            &AppMatch::for_process("Ledgerline.exe", TriggerMode::Prompt),
        );
        let also_matched = with_app_match(
            &item(Some(3), false, None),
            &AppMatch::for_process("Vantage.exe", TriggerMode::Auto),
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
            value: Some(zeroize::Zeroizing::new("not json".to_string())),
            other: serde_json::Map::new(),
        }];
        assert!(!SidebarFilter::Apps.scope_contains(&broken));
        assert_eq!(crate::vault_bridge::extract_app_match(&broken), None);

        // POSITIVE CONTROL: the same fixture with a value the app CAN read is
        // on the row, so this is not a test that passes against a row which
        // matches nothing at all.
        let readable = crate::vault_bridge::with_app_match(
            &broken,
            &crate::app_match::AppMatch::for_process("Ledgerline.exe", crate::app_match::TriggerMode::Prompt),
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

    /// **What an item's folder is called, and the three ways there is nothing
    /// to call it.**
    ///
    /// The detail pane's header subtitle reads this, so each `None` here is a
    /// line the user sees. The positive case is asserted first and by name:
    /// a `folder_name` that returned `None` for everything would satisfy every
    /// negative below on its own.
    #[test]
    fn a_folder_is_named_only_when_the_list_really_has_it() {
        let folders = vec![
            Folder {
                id: String::new(),
                name: "No Folder".into(),
                other: serde_json::Map::new(),
            },
            Folder {
                id: "957b860f-1130-42d9-a72c-7814f828b4d5".into(),
                name: "Engineering".into(),
                other: serde_json::Map::new(),
            },
        ];

        assert_eq!(
            folder_name(&folders, Some("957b860f-1130-42d9-a72c-7814f828b4d5")),
            Some("Engineering"),
            "an item in a real folder is not given that folder's name"
        );
        assert_eq!(
            folder_name(&folders, None),
            None,
            "an item in no folder was given a folder anyway"
        );
        // A folder deleted from another client, or a header drawn before the
        // folder list has arrived. The alternatives are a raw uuid in front of
        // the user and a claim ("No folder") this cannot know.
        assert_eq!(
            folder_name(&folders, Some("d0e5e0da-9b6a-4c2a-9a4a-1c2f0f8f0c11")),
            None,
            "an id that names nothing in the list was resolved to something"
        );
        // `bw serve` ships the bucket IN the list, so a plain `find` matches
        // it -- and an item carrying `folderId: \"\"` is in a broken state,
        // not deliberately unfiled.
        assert_eq!(
            folder_name(&folders, Some("")),
            None,
            "the virtual \"No Folder\" bucket was reported as a folder an item is in"
        );
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
        painted_sidebar_lists(lock_countdown, VaultLists::live_only(&items), &folders)
    }

    /// [`painted_sidebar_fixture`] over a caller-supplied set of ALL THREE
    /// lists, so a test about the Archive or Trash badge can hand in a vault
    /// where those queries have (or have not) answered.
    fn painted_sidebar_lists(
        lock_countdown: &str,
        lists: VaultLists<'_>,
        folders: &[Folder],
    ) -> (Vec<(String, egui::Rect)>, Vec<egui::Rect>, egui::Rect) {
        let (rects, paths, _, bounds) = painted_sidebar_parts(lock_countdown, lists, folders);
        (rects, paths, bounds)
    }

    /// One painted frame of the rail: its text rows and their boxes, the
    /// bounding box of every polygon it drew, its dividers, and the rail's own
    /// bounds. Named because four parallel vectors in a return type is a shape
    /// no reader can hold, not because the tuple was wrong.
    type PaintedRail =
        (Vec<(String, egui::Rect)>, Vec<egui::Rect>, Vec<egui::Rect>, egui::Rect);

    /// [`painted_sidebar_lists`] with the DIVIDERS as well: every hairline the
    /// rail painted, in painted order, top to bottom.
    ///
    /// Split out rather than copied, because the alternative is a second
    /// harness that runs the sidebar a second way -- and two harnesses obliged
    /// to agree about what "the sidebar" is are exactly this crate's recurring
    /// defect. Every existing caller keeps its three-tuple; only the divider
    /// tests ask for the fourth.
    ///
    /// A divider is identified by what it IS rather than by counting shapes:
    /// a `theme::HAIRLINE`-filled rect exactly `inset_hairline`'s 1pt tall.
    fn painted_sidebar_parts(
        lock_countdown: &str,
        lists: VaultLists<'_>,
        folders: &[Folder],
    ) -> PaintedRail {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(crate::vault_window::SIDEBAR_WIDTH, 2000.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(), |_ui| {});

        let mut selected = SidebarFilter::All;
        let mut sends_selected = false;
        let mut health_selected = false;

        let mut bounds = egui::Rect::NOTHING;
        let output = ctx.run_ui(input(), |ui| {
            bounds = ui.max_rect();
            draw_sidebar(
                ui,
                lists,
                folders,
                &mut selected,
                Screens {
                    sends: &mut sends_selected,
                    health: &mut health_selected,
                    received: &mut false,
                    generator: &mut false,
                    settings: &mut false,
                    scope: &mut SendScope::All,
                },
                lock_countdown,
                false,
            );
        });

        let mut rects = Vec::new();
        let mut paths = Vec::new();
        let mut hairlines = Vec::new();
        for clipped in &output.shapes {
            collect_text_rects(&clipped.shape, &mut rects);
            collect_path_rects(&clipped.shape, &mut paths);
            collect_hairline_rects(&clipped.shape, &mut hairlines);
        }
        hairlines.sort_by(|a, b| a.top().total_cmp(&b.top()));
        (rects, paths, hairlines, bounds)
    }

    /// Every `inset_hairline` in `shape`: a `theme::HAIRLINE` fill, 1pt tall,
    /// no stroke. Named by its appearance rather than by position, so a test
    /// that says "the second divider" is talking about a divider and not about
    /// whatever egui happened to paint second.
    fn collect_hairline_rects(shape: &egui::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect)
                if rect.fill == theme::HAIRLINE && (rect.rect.height() - 1.0).abs() < 0.01 =>
            {
                out.push(rect.rect);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_hairline_rects(shape, out);
                }
            }
            _ => {}
        }
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

    // --- Trash and Archive: which list a row reads ---------------------------

    fn trashed(id: &str) -> VaultItem {
        serde_json::from_str(&format!(
            r#"{{"id":"{id}","name":"Gone","fields":[],"type":1,
                "deletedDate":"2026-07-30T09:15:00.000Z"}}"#
        ))
        .unwrap()
    }

    /// The defect, at the level it was reported: the Trash row's badge read 0
    /// and it listed nothing.
    ///
    /// The expected numbers are the fixtures' own sizes -- 2 trashed, 1
    /// archived, 5 live -- not restatements of what the code computes. The
    /// live count is asserted in the same breath so a change that pointed
    /// every row at the same list could not pass.
    #[test]
    fn trash_and_archive_count_their_own_lists_and_not_the_live_vault() {
        let live = three_unfiled_and_two_filed();
        let trash = vec![trashed("t1"), trashed("t2")];
        let archive = vec![item(Some(1), false, None)];
        let lists = VaultLists {
            live: &live,
            trash: Some(&trash),
            archive: Some(&archive),
            sends: None,
            received: 0,
            health_findings: 0,
            sharing: &crate::rest::organizations::NOTHING_SHARED,
            shared_records: 0,
            received_records: 0,
            invitation_waiting: false,
        };

        assert_eq!(badge_for(&SidebarFilter::Trash, lists), Some(2));
        assert_eq!(badge_for(&SidebarFilter::Archive, lists), Some(1));
        assert_eq!(badge_for(&SidebarFilter::All, lists), Some(5));
    }

    /// ...and the row LISTS them, which is a separate claim from counting
    /// them: the badge and the pane used to come from different places.
    #[test]
    fn the_trash_row_lists_the_trashed_items_themselves() {
        let live = three_unfiled_and_two_filed();
        let trash = vec![trashed("t1"), trashed("t2")];
        let lists = VaultLists { trash: Some(&trash), ..VaultLists::live_only(&live) };

        let listed: Vec<&str> = items_for(&SidebarFilter::Trash, lists)
            .expect("the trash list was fetched")
            .iter()
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(listed, vec!["t1", "t2"]);
    }

    /// An unfetched list is `None` -- NOT an empty list -- everywhere it is
    /// asked about, and it draws as an en dash rather than a `0`.
    ///
    /// `0` is a claim this app cannot make before the query has answered, and
    /// it is exactly the claim the old Trash row made forever. The positive
    /// half is asserted alongside it: a list that HAS answered and is empty
    /// really does read `0`, so this is not a test that would pass against a
    /// badge which never showed a number at all.
    #[test]
    fn an_unfetched_list_reads_as_unknown_and_an_empty_one_reads_as_zero() {
        let live = three_unfiled_and_two_filed();

        let unfetched = VaultLists::live_only(&live);
        assert_eq!(badge_for(&SidebarFilter::Trash, unfetched), None);
        assert!(items_for(&SidebarFilter::Trash, unfetched).is_none());
        assert_eq!(badge_text(None), UNKNOWN_COUNT);

        let empty: Vec<VaultItem> = Vec::new();
        let fetched = VaultLists { trash: Some(&empty), ..VaultLists::live_only(&live) };
        assert_eq!(badge_for(&SidebarFilter::Trash, fetched), Some(0));
        assert_eq!(badge_text(Some(0)), "0");
    }

    /// The painted proof of the same thing: with neither query answered, both
    /// rows show the en dash, and every live row still shows a number.
    #[test]
    fn the_archive_and_trash_badges_are_drawn_unknown_until_their_query_answers() {
        let live = three_unfiled_and_two_filed();
        let folders = one_real_folder_and_the_virtual_bucket();
        let (painted, _, _) =
            painted_sidebar_lists("Locks in 11:42", VaultLists::live_only(&live), &folders);

        assert_eq!(badge_beside(&painted, "Archive"), UNKNOWN_COUNT);
        assert_eq!(badge_beside(&painted, "Trash"), UNKNOWN_COUNT);
        // The control: a row whose list IS in hand still prints a number, so
        // this cannot pass against a sidebar that stopped drawing counts.
        assert_eq!(badge_beside(&painted, "All items"), "5");
    }

    /// ...and once the queries answer, both rows show their real counts.
    #[test]
    fn the_archive_and_trash_badges_show_their_counts_once_fetched() {
        let live = three_unfiled_and_two_filed();
        let trash = vec![trashed("t1"), trashed("t2")];
        let archive = vec![item(Some(1), false, None)];
        let folders = one_real_folder_and_the_virtual_bucket();
        let (painted, _, _) = painted_sidebar_lists(
            "Locks in 11:42",
            VaultLists { trash: Some(&trash), archive: Some(&archive), ..VaultLists::live_only(&live) },
            &folders,
        );

        assert_eq!(badge_beside(&painted, "Trash"), "2");
        assert_eq!(badge_beside(&painted, "Archive"), "1");
        assert_eq!(badge_beside(&painted, "All items"), "5");
    }

    /// Which query each row reads, stated for every variant.
    ///
    /// Exhaustive rather than "Trash is Trash": a row that quietly fell back
    /// to `LiveVault` would show an empty list and a `0` badge with nothing
    /// to indicate anything was wrong -- which is the defect this whole
    /// type exists to make unrepresentable.
    #[test]
    fn every_row_states_which_query_it_reads() {
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
            SidebarFilter::Folder("f1".into()),
            SidebarFilter::Unfiled,
        ] {
            assert_eq!(
                filter.source(),
                FilterSource::LiveVault,
                "{filter:?} stopped reading the live vault"
            );
            assert_eq!(filter.source().out_of_vault(), None);
        }
        assert_eq!(SidebarFilter::Trash.source(), FilterSource::Trash);
        assert_eq!(SidebarFilter::Archive.source(), FilterSource::Archive);
        assert_eq!(
            SidebarFilter::Trash.source().out_of_vault(),
            Some(OutOfVault::Trash)
        );
        assert_eq!(
            SidebarFilter::Archive.source().out_of_vault(),
            Some(OutOfVault::Archive)
        );
    }

    /// The Sends row's badge is its own count, and `None` -- an en dash --
    /// for a fetch that has not happened OR has failed.
    ///
    /// The row where the `0`-versus-en-dash rule matters most: a `0` beside
    /// Sends reads as "nothing of yours is published", and a fetch that
    /// failed has no business saying that. All three states are asserted, so
    /// this cannot pass against a badge that always reads unknown.
    #[test]
    fn the_sends_row_badges_its_own_count_and_never_the_vault() {
        let live = three_unfiled_and_two_filed();
        let folders = one_real_folder_and_the_virtual_bucket();

        // Not fetched, or fetched and failed -- both `None`, both an en dash.
        let (unknown, _, _) =
            painted_sidebar_lists("Locks in 11:42", VaultLists::live_only(&live), &folders);
        assert_eq!(badge_beside(&unknown, SENDS_ROW_LABEL), UNKNOWN_COUNT);

        // Answered, and empty. `0` is a claim, and here it is one this app
        // has: the CLI said so.
        let (none_published, _, _) = painted_sidebar_lists(
            "Locks in 11:42",
            VaultLists { sends: Some(SendCounts::default()), ..VaultLists::live_only(&live) },
            &folders,
        );
        assert_eq!(badge_beside(&none_published, SENDS_ROW_LABEL), "0");

        // Answered, with three. And it is not the live vault's count leaking
        // through: the live list here has five items.
        let (three, _, _) = painted_sidebar_lists(
            "Locks in 11:42",
            VaultLists {
                sends: Some(SendCounts::over([
                    crate::send::SendState::Waiting,
                    crate::send::SendState::Used,
                    crate::send::SendState::Revoked,
                ])),
                ..VaultLists::live_only(&live)
            },
            &folders,
        );
        assert_eq!(badge_beside(&three, SENDS_ROW_LABEL), "3");
        assert_eq!(badge_beside(&three, "All items"), "5");
    }

    /// The row is in the rail, once, and pressing it selects the Sends screen
    /// **without** disturbing which item filter is selected underneath.
    ///
    /// The count assertion comes first on purpose: a row pushed out of the
    /// sidebar is culled entirely by egui and comes back as nothing, so
    /// reading geometry before counting would read the rows that survived.
    #[test]
    fn the_sends_row_is_in_the_rail_once_and_is_the_only_thing_it_selects() {
        let live = three_unfiled_and_two_filed();
        let folders = one_real_folder_and_the_virtual_bucket();
        let (painted, _, _) =
            painted_sidebar_lists("Locks in 11:42", VaultLists::live_only(&live), &folders);
        assert_eq!(
            painted.iter().filter(|(t, _)| t == SENDS_ROW_LABEL).count(),
            1,
            "the Sends row is not in the rail exactly once: {painted:?}"
        );

        // Press it, and the item filter is untouched: Sends is not a cut of
        // the item list and selecting it must not pretend to be one.
        let (filter, sends) =
            press_row(
                SENDS_ROW_LABEL,
                SidebarFilter::Logins,
                false,
                VaultLists::live_only(&live),
                &folders,
            );
        assert!(sends, "the Sends row did not select the Sends screen");
        assert_eq!(filter, SidebarFilter::Logins, "the Sends row changed the item filter");

        // ...and pressing any item row leaves the Sends screen. This is the
        // invariant `draw_sidebar` exists to keep, and without it the window
        // would sit on the Sends screen while the rail highlights Cards.
        let (filter, sends) = press_row(
            "Cards",
            SidebarFilter::Logins,
            true,
            VaultLists::live_only(&live),
            &folders,
        );
        assert!(!sends, "an item row was clicked and the Sends screen stayed up");
        assert_eq!(filter, SidebarFilter::Cards);

        // The same for a folder row, which is a separate loop with a separate
        // click handler -- and therefore a separate chance to forget.
        let (filter, sends) =
            press_row(
            "Engineering",
            SidebarFilter::Logins,
            true,
            VaultLists::live_only(&live),
            &folders,
        );
        assert!(!sends, "a folder row was clicked and the Sends screen stayed up");
        assert_eq!(filter, SidebarFilter::Folder("f1".into()));
    }


    /// **Every Send is on exactly one sub-row, and the sub-rows add up to the
    /// parent.**
    ///
    /// Design 5b's own counts say so -- 2 + 1 + 2 = 5 -- and that is the only
    /// thing that makes merging `Expired` and `Revoked` into `Ended` a
    /// partition rather than an overlap. Asserted over every state rather
    /// than over a fixture, so a state added to `SendState` without a row
    /// fails here instead of quietly going uncounted -- which is how
    /// `Opened` got its row.
    #[test]
    fn the_sharing_sub_rows_partition_every_send_state() {
        use crate::send::SendState;
        for state in [
            SendState::Waiting,
            SendState::Opened,
            SendState::Used,
            SendState::Expired,
            SendState::Revoked,
        ] {
            let on: Vec<&str> = SendScope::SUB_ROWS
                .iter()
                .filter(|scope| scope.admits(state))
                .map(|scope| scope.label())
                .collect();
            assert_eq!(
                on.len(),
                1,
                "{state:?} is on {} sub-rows ({on:?}) -- they are meant to be a partition, \
                 so a Send is listed once and the counts add up",
                on.len()
            );
            assert!(
                SendScope::All.admits(state),
                "{state:?} is not on the parent row, so the sub-rows count Sends the parent \
                 does not"
            );
        }

        let counts = SendCounts::over([
            SendState::Waiting,
            SendState::Waiting,
            SendState::Used,
            SendState::Expired,
            SendState::Revoked,
        ]);
        // 5b's own numbers, from 5b's own list.
        assert_eq!((counts.waiting, counts.used, counts.ended, counts.all), (2, 1, 2, 5));
        assert_eq!(
            counts.waiting + counts.opened + counts.used + counts.ended,
            counts.all,
            "the sub-rows do not add up to their parent: {counts:?}"
        );

        // And an opened live link is counted on its own row, not as waiting.
        let counts = SendCounts::over([SendState::Waiting, SendState::Opened, SendState::Opened]);
        assert_eq!((counts.waiting, counts.opened, counts.all), (1, 2, 3), "{counts:?}");
        assert_eq!(SendScope::Opened.label(), crate::vault_window::send_ui::OPENED_LABEL);
    }

    /// **The rail's `Ended` and the pill's `Expired`/`Revoked` are different
    /// words on purpose, and the difference is not allowed to be silent.**
    ///
    /// Two halves. The words really do differ -- so this is a decision and
    /// not a coincidence of spelling -- and the row that merges them has a
    /// gloss that names BOTH pill words, which is the bridge that stops a
    /// user who clicked `Ended` reading `Revoked` beside a row as a
    /// contradiction. See [`SendScope`].
    #[test]
    fn the_ended_row_says_a_different_word_from_the_pills_and_explains_which_ones() {
        use crate::vault_window::send_ui;
        assert_ne!(ENDED_ROW_LABEL, send_ui::EXPIRED_LABEL);
        assert_ne!(ENDED_ROW_LABEL, send_ui::REVOKED_LABEL);
        // The two rows that do NOT merge anything borrow the pill's own word,
        // so the only row spelled differently is the only row that differs.
        assert_eq!(SendScope::Waiting.label(), send_ui::WAITING_LABEL);
        assert_eq!(SendScope::Used.label(), send_ui::USED_LABEL);

        let gloss = SendScope::Ended
            .gloss()
            .expect("the merging row has no gloss, so nothing on screen bridges the two words");
        for word in [send_ui::EXPIRED_LABEL, send_ui::REVOKED_LABEL] {
            assert!(
                gloss.to_lowercase().contains(&word.to_lowercase()),
                "the `Ended` gloss does not name {word:?}, so a pill saying it reads as a \
                 contradiction of the row the user clicked: {gloss:?}"
            );
        }
    }

    /// **The move's own risk, and the reason this test exists at all.** The
    /// rail was a plain column in a fixed-height panel with NO `ScrollArea`
    /// anywhere: whatever ran past the bottom was simply unreachable. That was
    /// survivable while the two screen rows sat up among the type rows, and it
    /// stopped being survivable the moment they moved below the folders --
    /// the folder list is the one part of this rail whose length the user
    /// controls, so Sends and Password health became the first things a long
    /// folder list would push out of reach.
    ///
    /// A vault with a folder for every letter of the alphabet, at the shipped
    /// rail's own height. **Both halves are asserted, and the first is what
    /// makes the second mean anything**: the rows really are pushed past the
    /// bottom edge by this many folders (so this is not a test of a vault that
    /// happens to fit), and scrolling really does bring them back.
    #[test]
    fn the_screen_rows_survive_a_vault_with_a_folder_for_every_letter() {
        let live = three_unfiled_and_two_filed();
        let folders: Vec<Folder> = ('a'..='z')
            .map(|c| Folder {
                id: format!("f-{c}"),
                name: format!("Folder {}", c.to_ascii_uppercase()),
                other: serde_json::Map::new(),
            })
            .collect();
        let health = crate::vault_window::password_health::HEALTH_ROW_LABEL;

        // Unscrolled: off the bottom. If this ever stops being true the test
        // below has stopped exercising anything -- the rail would simply fit.
        //
        // **RE-MEASURED, not deleted, when the SHARING section landed.** That
        // section added a divider, a section label, three sub-rows and
        // `Shared with me` -- a little over 200pt -- so this rail overflows
        // sooner than it did and the LAST row is no longer Password health.
        // Both ends are asserted: Sends is still past the floor, and the row
        // that is now below it is further past it again.
        let (resting, bounds) = scrolled_rail(VaultLists::live_only(&live), &folders, 0.0);
        let sends_at_rest = row_top(&resting, RECEIVED_ROW_LABEL);
        assert!(
            sends_at_rest > bounds.bottom(),
            "{} folders no longer overflow the rail (Sends rests at y={sends_at_rest}, floor \\
             y={}), so this test is not measuring what it is named for",
            folders.len(),
            bounds.bottom()
        );
        let last_at_rest = row_top(&resting, NEW_FOLDER_LABEL);
        assert!(
            last_at_rest > sends_at_rest,
            "control: the rail's last row rests at y={last_at_rest}, above its Sends row at              y={sends_at_rest} -- the SHARING section this test was re-measured for is not              being drawn below it"
        );

        // Scrolled to the bottom: EVERY row below the folders is inside the
        // rail, which is what "reachable" means for a row nobody can drag a
        // window taller for. The three sub-rows and `Shared with me` are in
        // the list because they are now the lowest things in the rail, and so
        // the ones a long folder list pushes out first.
        let (scrolled, bounds) = scrolled_rail(VaultLists::live_only(&live), &folders, -4000.0);
        let _ = health;
        for label in [RECEIVED_ROW_LABEL, NEW_FOLDER_LABEL] {
            let rect = scrolled
                .iter()
                .find(|(text, _)| text == label)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{label:?} was not painted at all: {scrolled:?}"));
            assert!(
                bounds.contains_rect(rect),
                "{label:?} is at {rect:?} after scrolling the rail to its end, still outside \\
                 the rail's own {bounds:?} -- the screens are unreachable with {} folders",
                folders.len()
            );
        }
    }

    fn row_top(painted: &[(String, egui::Rect)], needle: &str) -> f32 {
        painted
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, rect)| rect.top())
            .unwrap_or_else(|| panic!("the sidebar painted no {needle:?}: {painted:?}"))
    }

    /// The rail after `delta` points of wheel scrolling, with the pointer over
    /// it -- which is what egui requires before a scroll event reaches a
    /// scroll area at all.
    ///
    /// Several frames, because a scroll offset applied on one frame is laid out
    /// on the next, and the clamp to the content's end takes another.
    fn scrolled_rail(
        lists: VaultLists<'_>,
        folders: &[Folder],
        delta: f32,
    ) -> (Vec<(String, egui::Rect)>, egui::Rect) {
        let (shapes, bounds) = scrolled_rail_shapes(lists, folders, delta);
        let mut rects = Vec::new();
        for shape in &shapes {
            collect_text_rects(shape, &mut rects);
        }
        (rects, bounds)
    }

    /// [`scrolled_rail`]'s last frame as painted shapes, for a test that
    /// reads geometry text does not carry -- the invitation dot.
    fn scrolled_rail_shapes(
        lists: VaultLists<'_>,
        folders: &[Folder],
        delta: f32,
    ) -> (Vec<egui::Shape>, egui::Rect) {
        const HEIGHT: f32 = 800.0;
        let ctx = egui::Context::default();
        let base = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(crate::vault_window::SIDEBAR_WIDTH, HEIGHT),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(base(), |_ui| {});
        theme::apply(&ctx);

        let mut selected = SidebarFilter::All;
        let (mut sends, mut health) = (false, false);
        let mut bounds = egui::Rect::NOTHING;
        let mut output = None;
        for frame in 0..4 {
            let mut raw = base();
            if frame > 0 {
                raw.events.push(egui::Event::PointerMoved(egui::Pos2::new(
                    crate::vault_window::SIDEBAR_WIDTH / 2.0,
                    HEIGHT / 2.0,
                )));
                raw.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, delta),
                    modifiers: Default::default(),
                    phase: egui::TouchPhase::Move,
                });
            }
            output = Some(ctx.run_ui(raw, |ui| {
                bounds = ui.max_rect();
                draw_sidebar(
                    ui,
                    lists,
                    folders,
                    &mut selected,
                    Screens {
                            sends: &mut sends,
                            health: &mut health,
                            received: &mut false,
                            generator: &mut false,
                            settings: &mut false,
                            scope: &mut SendScope::All,
                        },
                    "Locks in 11:42",
                    false,
                );
            }));
        }
        let shapes = output.expect("four frames were run").shapes.into_iter().map(|clipped| clipped.shape).collect();
        (shapes, bounds)
    }

    /// **The invariant, asserted from the rows' NEW HOME.** `Screens` promises
    /// at most one screen is live and that selecting any item row clears them
    /// all; moving the two rows out of the VAULT run and below the folders
    /// moved the only two places that call `Screens::select`, so this is that
    /// promise re-measured from down there.
    ///
    /// Both flags, in both directions, because the failure this guards is a
    /// window painting one screen while the rail highlights something else --
    /// and a helper that can only see the Sends flag would have covered half of
    /// it.
    #[test]
    fn the_screen_rows_still_hold_their_invariant_from_below_the_folders() {
        let live = three_unfiled_and_two_filed();
        let folders = one_real_folder_and_the_virtual_bucket();
        let health_label = crate::vault_window::password_health::HEALTH_ROW_LABEL;

        // Password health selects itself and nothing else.
        let (filter, sends, health) =
            press_row_screens(
                health_label,
                SidebarFilter::Logins,
                false,
                false,
                VaultLists::live_only(&live),
                &folders,
            );
        assert!(health, "the Password health row did not select its screen");
        assert!(!sends, "selecting Password health left the Sends screen up as well");
        assert_eq!(filter, SidebarFilter::Logins, "it changed the item filter");

        // At most one: pressing Sends while Password health is up clears it.
        let (_, sends, health) = press_row_screens(
            SENDS_ROW_LABEL,
            SidebarFilter::Logins,
            false,
            true,
            VaultLists::live_only(&live),
            &folders,
        );
        assert!(sends && !health, "both screens were live at once (sends={sends}, health={health})");

        // ...and any item row clears them. A FOLDER row, specifically: it is
        // the loop the two rows now sit directly beneath, and it is a separate
        // click handler from the type rows -- a separate chance to forget.
        let (filter, sends, health) =
            press_row_screens(
                "Engineering",
                SidebarFilter::Logins,
                true,
                true,
                VaultLists::live_only(&live),
                &folders,
            );
        assert!(
            !sends && !health,
            "a folder row was clicked and a screen stayed up (sends={sends}, health={health})"
        );
        assert_eq!(filter, SidebarFilter::Folder("f1".into()));
    }

    /// Presses the sidebar row whose label is `label` and reports the two
    /// selections afterwards.
    ///
    /// A press **and** a release is what egui counts as a click, and the
    /// frame that locates the row cannot be the frame that clicks it.
    fn press_row(
        label: &str,
        filter: SidebarFilter,
        sends: bool,
        lists: VaultLists<'_>,
        folders: &[Folder],
    ) -> (SidebarFilter, bool) {
        let (filter, sends, _) = press_row_screens(label, filter, sends, false, lists, folders);
        (filter, sends)
    }

    /// [`press_row`] with the Password health flag as well, in and out.
    ///
    /// The two screens are one invariant -- at most one of them, and any item
    /// row clears BOTH -- so a helper that can only see one of the flags can
    /// only ever test half of it.
    fn press_row_screens(
        label: &str,
        filter: SidebarFilter,
        sends: bool,
        health: bool,
        lists: VaultLists<'_>,
        folders: &[Folder],
    ) -> (SidebarFilter, bool, bool) {
        const HEIGHT: f32 = 2000.0;
        let ctx = egui::Context::default();
        let base = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(crate::vault_window::SIDEBAR_WIDTH, HEIGHT),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(base(), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(base(), |_ui| {});

        let mut selected = filter;
        let mut sends_selected = sends;
        let mut health_selected = health;
        let mut run = |raw: egui::RawInput| {
            ctx.run_ui(raw, |ui| {
                draw_sidebar(
                    ui,
                    lists,
                    folders,
                    &mut selected,
                    Screens {
                    sends: &mut sends_selected,
                    health: &mut health_selected,
                    received: &mut false,
                    generator: &mut false,
                    settings: &mut false,
                    scope: &mut SendScope::All,
                },
                    "Locks in 11:42",
                    false,
                );
            })
        };
        let output = run(base());
        let mut rects: Vec<(String, egui::Rect)> = Vec::new();
        for clipped in &output.shapes {
            collect_labelled_rects(&clipped.shape, &mut rects);
        }
        let pos = rects
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, rect)| rect.center())
            .unwrap_or_else(|| panic!("no sidebar row labelled {label:?}: {rects:?}"));

        let _ = run(egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            ..base()
        });
        let _ = run(egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            ..base()
        });
        (selected, sends_selected, health_selected)
    }

    fn collect_labelled_rects(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                out.push((text.galley.text().to_string(), text.visual_bounding_rect()))
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_labelled_rects(shape, out);
                }
            }
            _ => {}
        }
    }



    /// **The rail asks for the app's bar -- width, lane and all -- and asks
    /// BEFORE it opens the scroll area.**
    ///
    /// `theme::scrollbar_in_gutter` writes into `ui.spacing_mut()`, which an
    /// `egui::ScrollArea` reads when it is BUILT -- so the same call moved
    /// one line down, inside the `show` closure, would style whatever the
    /// rail's CONTENT scrolls and leave the rail's own bar at egui's default:
    /// 10pt, in the text colour, and with no lane, i.e. back on the rows.
    /// That is a mistake with no symptom in the source and the whole symptom
    /// on screen, which is why the ORDER is pinned and not merely the
    /// presence.
    ///
    /// **The needle is the whole call, ARGUMENT INCLUDED, and that is the
    /// only thing guarding the argument.** Measured: with the panel frame's
    /// right margin already at zero and the scroll area's content already
    /// capped at `rail_width`, passing a gutter of `0.0` here paints an
    /// identical frame -- rows 10..202, bar 206..212. egui pins a floating
    /// bar to `outer_rect.right()`, which is the panel's own edge either way,
    /// and the cap is what holds the rows off it; the reserved lane only
    /// duplicates the cap on the frames that show a bar. So no painted-frame
    /// test in this module can tell the rail's real gutter from a wrong one,
    /// and asking for one that is not the padding the frame gave up would be
    /// a claim in the source that nothing checks. It is checked here.
    ///
    /// A source pin because driving the rail needs the lists, the selection,
    /// the screen set and a lock countdown, and because what is being checked
    /// is which line comes first rather than anything a frame reports. The
    /// needle is split with `concat!` so this pin cannot match itself.
    #[test]
    fn the_rail_takes_the_apps_scrollbar_width_before_it_builds_its_scroll_area() {
        let source = include_str!("sidebar.rs");
        let bar = concat!("theme::scrollbar_in_", "gutter(ui, f32::from(PANEL_PAD_X));");
        let area = concat!("egui::ScrollArea::", "vertical()");

        let at_bar = source.find(bar).unwrap_or_else(|| {
            panic!(
                "the rail no longer takes the app's scroll bar with the panel's own padding as \
                 its gutter. Either it is back to egui's `ScrollStyle::floating()` default -- a \
                 10pt bar in the text colour beside the item list's 6pt one, which is the first \
                 report -- or it is asking for a lane that is not the one `panel_frame` gave \
                 up, which no painted frame in this module can see"
            )
        });
        let at_area = source
            .find(area)
            .expect("control: the rail has no scroll area at all, so this pin guards nothing");
        assert!(
            at_bar < at_area,
            "the bar is styled BELOW the scroll area, so it applies to the rail's contents and \
             the rail's own bar keeps egui's default"
        );
        // Control on the ordering: both needles are really found in the rail,
        // not somewhere else that happens to spell them. There is one scroll
        // area in this file and one call, and a second of either would make
        // `find` answer about whichever came first.
        assert_eq!(
            source.matches(area).count(),
            1,
            "a second scroll area appeared in the rail; the assertion above is now about \
             whichever one is written first, which may not be the one that scrolls"
        );
        assert_eq!(source.matches(bar).count(), 1, "the bar is styled twice in the rail");
    }

    /// The rail INSIDE the panel frame `vault_window` really gives it, clicked
    /// once at `click_x` on the "Cards" row, with `folder_count` folders under
    /// it. Answers which filter the rail was left on.
    ///
    /// **The frame is the point.** Every other harness in this module draws
    /// `draw_sidebar` straight onto the root ui, so the rail's ui is the whole
    /// 212pt screen; production wraps it in [`panel_frame`], which insets it
    /// by `PANEL_PAD_X` on the left and hands the same width on the right to
    /// the scroll bar's lane. A floating scroll bar is pinned to the ui's
    /// right edge, so the frame is what decides whether the bar lands on the
    /// rows or beside them -- a harness without it cannot see the bar at all.
    /// The frame is called rather than spelled out again: it used to be
    /// copied here and kept honest by a comment, which is the two-copies-
    /// obliged-to-agree defect this crate keeps finding.
    ///
    /// The pointer is moved onto the row before the press, because egui only
    /// routes a click to a widget the pointer is already over.
    fn rail_click_selects(click_x: f32, folder_count: usize, height: f32) -> SidebarFilter {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(crate::vault_window::SIDEBAR_WIDTH, height),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let items = vec![item(Some(1), false, Some("f0"))];
        let folders: Vec<Folder> = (0..folder_count)
            .map(|i| Folder {
                id: format!("f{i}"),
                name: format!("Folder {i}"),
                other: serde_json::Map::new(),
            })
            .collect();
        let mut selected = SidebarFilter::All;
        let (mut sends, mut health) = (false, false);
        // Where the row actually is, re-read every frame off the frame just
        // painted rather than computed: the rail's rows move with the folder
        // count, and a hard-coded y would silently click empty space.
        let mut cards_y = 0.0_f32;
        // Eight frames to settle -- `theme::apply`'s fonts land on the next
        // frame, and egui fades a floating bar in over several more, so a
        // press on frame 1 would be a press on a bar that is not there yet.
        const PRESS: usize = 8;
        for frame in 0..=PRESS + 1 {
            let mut raw = input();
            let at = egui::pos2(click_x, cards_y);
            if frame == PRESS {
                raw.events.push(egui::Event::PointerMoved(at));
                raw.events.push(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            if frame == PRESS + 1 {
                raw.events.push(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                });
            }
            let output = ctx.run_ui(raw, |ui| {
                egui::Panel::left("vault-sidebar")
                    .exact_size(crate::vault_window::SIDEBAR_WIDTH)
                    .resizable(false)
                    .frame(panel_frame(crate::kit::tokens::SidebarStyle::Navy))
                    .show(ui, |ui| {
                        draw_sidebar(
                            ui,
                            VaultLists::live_only(&items),
                            &folders,
                            &mut selected,
                            Screens {
                            sends: &mut sends,
                            health: &mut health,
                            received: &mut false,
                            generator: &mut false,
                            settings: &mut false,
                            scope: &mut SendScope::All,
                        },
                            "Locks in 11:42",
                            false,
                        );
                    });
            });
            let mut texts = Vec::new();
            for clipped in &output.shapes {
                collect_text_rects(&clipped.shape, &mut texts);
            }
            if let Some((_, rect)) = texts.iter().find(|(text, _)| text == "Cards") {
                cards_y = rect.center().y;
            }
        }
        selected
    }

    /// **A rail row is clickable to its right edge whether or not the bar is
    /// showing -- which it was NOT before the bar got its lane.**
    ///
    /// The two panes' scroll bars are one bar, field for field
    /// (`theme::tests::the_two_panes_scroll_bars_are_one_bar_field_for_field`),
    /// and now in placement too. This is the measurement of what the placement
    /// buys the mouse.
    ///
    /// Before: the rail reserved no lane, so a floating bar pinned to the ui's
    /// right edge painted -- and registered its hit rect -- over the rows'
    /// rightmost `theme::SCROLLBAR_WIDTH`. Measured inside the real panel
    /// frame, rows x=10..202 and bar x=196..202. That rect is registered after
    /// the rows, so it wins the hit test, and a click at x=199 on a 900pt rail
    /// with 26 folders (overflowing, bar showing) selected NOTHING while the
    /// same click on a rail with one folder (fitting, no bar) selected Cards.
    /// Only the fitting case could be asserted, and it was.
    ///
    /// After: the panel frame's right margin is the bar's lane, so the rows
    /// still span x=10..202 and the bar has moved to x=206..212 -- beside
    /// them, not on them. **The click coordinate below did not move**, because
    /// the rows did not move; what moved is that the SAME coordinate now works
    /// in both states, which is what this test asserts.
    ///
    /// **This is also what makes the visibility mode a free choice again.**
    /// `VisibleWhenNeeded` was load-bearing: `AlwaysVisible` registers the
    /// bar's rect on every frame and `theme::hide_scrollbar` only zeroes the
    /// six opacities without giving the rect back, so copying the item list's
    /// mode would have made the right 6pt of every rail row dead to the mouse
    /// on a rail with nothing to scroll. With the rect off the rows entirely
    /// that cost is gone -- see the note at the `scrollbar_in_gutter` call for
    /// why the mode is nonetheless kept.
    #[test]
    fn a_rail_row_is_clickable_to_its_right_edge_with_the_bar_showing_or_not() {
        const TALL: f32 = 900.0;
        // 3pt clear of the rows' right edge at x=202, and inside the 6pt
        // column the bar used to occupy when it was showing (196..202). It is
        // now 7pt left of the bar's lane, which starts at 202.
        const AT_THE_ROWS_EDGE: f32 = 199.0;
        // One folder fits in a 900pt rail and 26 do not, so this pair is the
        // bar hidden and the bar showing -- the two states the old placement
        // answered differently.
        for folders in [1, 26] {
            assert_eq!(
                rail_click_selects(AT_THE_ROWS_EDGE, folders, TALL),
                SidebarFilter::Cards,
                "a click {AT_THE_ROWS_EDGE}pt across a rail with {folders} folder(s) did not \
                 reach the row under it -- the scroll bar is holding a hit rect over the rows \
                 again, so its lane is gone or the rows have grown into it"
            );
            // Control, so the assertion above cannot pass because this harness
            // never selects anything: the same click well left of the bar's
            // column works, in both states.
            assert_eq!(
                rail_click_selects(100.0, folders, TALL),
                SidebarFilter::Cards,
                "control: a click in the MIDDLE of the Cards row did not select it with \
                 {folders} folder(s), so this harness cannot select a row at all and the \
                 assertion above proves nothing"
            );
        }
    }

    /// Every rect the frame painted, fill and stroke kept, so a caller can
    /// tell ink from egui's transparent layout rects.
    fn collect_rect_shapes(shape: &egui::Shape, out: &mut Vec<egui::epaint::RectShape>) {
        match shape {
            egui::Shape::Rect(rect) => out.push(rect.clone()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_rect_shapes(shape, out);
                }
            }
            _ => {}
        }
    }

    /// Where the four things the panel frame's right margin could have moved
    /// actually landed, on one settled frame of the rail.
    struct RailGeometry {
        /// The selected row's own wash: a full-width `sidebar_row` band, and
        /// therefore the x-span every VAULT row occupies.
        row: egui::Rect,
        /// Everything VISIBLE painted right of the rows -- the scroll bar's
        /// track and handle, or nothing at all when the rail fits. Keyed on
        /// position rather than on colour, exactly like `item_list.rs`'s
        /// `gutter_marks`, so the two panes' tests mean the same thing by
        /// "the bar is on screen".
        bar: Vec<egui::Rect>,
        /// The lock countdown's painted text box.
        countdown: egui::Rect,
    }

    /// One settled frame of the rail inside the REAL [`panel_frame`], with
    /// `folder_count` folders in a `height`-tall window.
    ///
    /// **Settled, and with the pointer parked in the rail.** egui's floating
    /// bar has `dormant_*_opacity: 0.0`, so a bar nobody is pointing at is
    /// emitted at alpha 0 -- a frame read without the pointer would report no
    /// bar on a rail that scrolls, and every assertion about placement would
    /// be vacuous. It also fades in over several frames, hence the settle
    /// count; `item_list.rs`'s `SETTLE_FRAMES` exists for the same two
    /// reasons.
    fn rail_geometry(folder_count: usize, height: f32) -> RailGeometry {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(crate::vault_window::SIDEBAR_WIDTH, height),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |_ui| {});
        theme::apply(&ctx);
        let items = vec![item(Some(1), false, Some("f0"))];
        let folders: Vec<Folder> = (0..folder_count)
            .map(|i| Folder {
                id: format!("f{i}"),
                name: format!("Folder {i}"),
                other: serde_json::Map::new(),
            })
            .collect();
        let mut selected = SidebarFilter::All;
        let (mut sends, mut health) = (false, false);
        const SETTLE: usize = 12;
        let mut rects = Vec::new();
        let mut texts = Vec::new();
        let mut paths = Vec::new();
        for frame in 0..=SETTLE {
            let mut raw = input();
            raw.events.push(egui::Event::PointerMoved(egui::pos2(100.0, height / 2.0)));
            let output = ctx.run_ui(raw, |ui| {
                egui::Panel::left("vault-sidebar")
                    .exact_size(crate::vault_window::SIDEBAR_WIDTH)
                    .resizable(false)
                    .frame(panel_frame(crate::kit::tokens::SidebarStyle::Navy))
                    .show(ui, |ui| {
                        draw_sidebar(
                            ui,
                            VaultLists::live_only(&items),
                            &folders,
                            &mut selected,
                            Screens {
                            sends: &mut sends,
                            health: &mut health,
                            received: &mut false,
                            generator: &mut false,
                            settings: &mut false,
                            scope: &mut SendScope::All,
                        },
                            COUNTDOWN,
                            false,
                        );
                    });
            });
            if frame == SETTLE {
                for clipped in &output.shapes {
                    collect_rect_shapes(&clipped.shape, &mut rects);
                    collect_text_rects(&clipped.shape, &mut texts);
                    collect_path_rects(&clipped.shape, &mut paths);
                }
            }
        }
        // The row `draw_sidebar` was handed as selected, found by ITS OWN
        // wash rather than by position: `sidebar_row` paints `BLUE_WASH` over
        // exactly the rect it allocated, so this is the row's real span and
        // not an inference from a label's x.
        let active = crate::kit::tokens::SidebarStyle::Navy.palette().active;
        let row = rects
            .iter()
            .find(|r| r.fill == active && (r.rect.height() - ROW_HEIGHT).abs() < 0.5)
            .map(|r| r.rect)
            .expect("the selected row painted its wash");
        let bar: Vec<egui::Rect> = rects
            .iter()
            .filter(|r| r.fill.a() > 0 || (r.stroke.width > 0.0 && r.stroke.color.a() > 0))
            .map(|r| r.rect)
            .filter(|r| r.left() > row.right() + 0.5)
            .collect();
        let countdown = texts
            .iter()
            .find(|(t, _)| t == lock_count(COUNTDOWN))
            .map(|(_, r)| *r)
            .expect("the countdown painted on the Lock row");
        let _ = &paths;
        RailGeometry { row, bar, countdown }
    }

    /// The countdown line every in-frame harness here draws, so the string
    /// looked for on the way out is the string handed in.
    const COUNTDOWN: &str = "Locks in 11:42";

    /// A rail tall enough to hold one folder and far too short for 26, so
    /// `[1, 26]` is "the bar is hidden" and "the bar is showing".
    const TALL_RAIL: f32 = 1100.0;

    /// The rail's own edges inside `panel_frame`, all four derived from the
    /// panel's width and its one horizontal pad rather than written out, so a
    /// design change to either moves the expectations with the code.
    const PANEL_RIGHT: f32 = crate::vault_window::SIDEBAR_WIDTH;
    const ROWS_LEFT: f32 = PANEL_PAD_X as f32;
    const ROWS_RIGHT: f32 = PANEL_RIGHT - PANEL_PAD_X as f32;

    /// **The rail's rows, bar and countdown are laid out like the item
    /// list's** -- the bar in a lane of its own, flush to the pane's outer
    /// edge, with the content it scrolls untouched.
    ///
    /// THE REPORT, twice: "Sidebar scroll bar - doesn't match the results".
    /// The first answer made every `ScrollStyle` field agree between the two
    /// panes and did not settle it, because what a reader compares with both
    /// panes on screen is where the bar IS. Measured then, inside this same
    /// frame: rows x=10..202, bar x=196..202 -- painted on the rows' right
    /// edge, stopping 10pt short of the panel's own edge, against an item
    /// list whose tiles end at 380 on a 390pt pane with its bar at 384..390.
    ///
    /// So the numbers below are absolute, and they are the item list's
    /// arrangement transposed onto a 212pt panel:
    ///
    /// * the rows span 10..202, the 192pt they spanned before this changed
    ///   anything -- the panel frame gave up exactly the width the lane takes;
    /// * the bar is inside 202..212 and flush to its OUTER edge, so all of the
    ///   lane's leftover -- `PANEL_PAD_X - SCROLLBAR_WIDTH` = 4pt -- lies
    ///   between the bar and the rows, where the reader compares it to the
    ///   10pt on the left, and none of it behind the bar. That is
    ///   `theme::scrollbar_in_gutter`'s rule, asserted here on the rail the
    ///   same way `item_list.rs` asserts it on the tiles;
    /// * the countdown, the one thing drawn OUTSIDE the scroll area and so
    ///   the one thing the lane does not reserve for, still starts on the row
    ///   labels' x;
    /// * the glyph column the FOLDERS "+" and the folder pencils share, which
    ///   `glyph_column_center_x` derives from the rail's width, has not moved
    ///   either.
    ///
    /// Read from the SETTLED frame of an overflowing rail, because a bar that
    /// is not being drawn cannot be measured; see [`rail_geometry`].
    #[test]
    fn the_rail_lays_its_rows_bar_and_countdown_out_like_the_item_list() {
        let g = rail_geometry(26, TALL_RAIL);

        assert_eq!(
            (g.row.left(), g.row.right()),
            (ROWS_LEFT, ROWS_RIGHT),
            "the rail's rows span {}..{}, expected {ROWS_LEFT}..{ROWS_RIGHT} -- the panel frame \
             and the bar's lane no longer cancel, so every row inset in the rail has moved",
            g.row.left(),
            g.row.right()
        );
        assert_eq!(
            g.row.width(),
            crate::vault_window::SIDEBAR_WIDTH - 2.0 * f32::from(PANEL_PAD_X),
            "the rail's rows are {}pt wide, not the 192pt they were before the bar was given a \
             lane; the whole point of paying for it out of the panel's right margin is that the \
             rows do not resize",
            g.row.width()
        );

        assert!(
            !g.bar.is_empty(),
            "nothing VISIBLE was painted right of the rows on a rail of 26 folders in a \
             {TALL_RAIL}pt window, so the rail is not showing the user that it can scroll"
        );
        for bar in &g.bar {
            let slack_left = bar.left() - ROWS_RIGHT;
            let slack_right = PANEL_RIGHT - bar.right();
            assert!(
                bar.right() <= PANEL_RIGHT + 0.01 && slack_left >= -0.01,
                "the bar spans x={}..{}, which is outside the {ROWS_RIGHT}..{PANEL_RIGHT} lane \
                 -- it is back over the rows, or off the panel",
                bar.left(),
                bar.right()
            );
            assert!(
                (slack_left - (f32::from(PANEL_PAD_X) - theme::SCROLLBAR_WIDTH)).abs() < 0.51
                    && slack_right.abs() < 0.51,
                "the bar has {slack_left}pt of lane to its left and {slack_right}pt to its \
                 right, expected {}pt and 0pt -- what is left of the lane once the bar is in it \
                 belongs on the ROW side, where the reader compares it to the gap on the left, \
                 and none of it between the bar and the panel's own edge",
                f32::from(PANEL_PAD_X) - theme::SCROLLBAR_WIDTH
            );
        }

        assert!(
            (g.countdown.right() - (ROWS_RIGHT - ROW_INSET_X)).abs() < 1.0,
            "the lock countdown ends at x={}, expected {} -- the Lock row is drawn outside the \
             scroll area, so it gives the bar's lane back itself",
            g.countdown.right(),
            ROWS_RIGHT - ROW_INSET_X
        );
    }

    /// **The rail lays itself out identically whether or not the bar is
    /// showing**, which `theme::scrollbar_in_gutter` does NOT give for free.
    ///
    /// That helper reserves its lane through `floating_allocated_width`, and
    /// egui applies that only on an axis it is showing a bar for. The rail
    /// keeps `ScrollBarVisibility::VisibleWhenNeeded`, so on its own the
    /// reservation comes and goes with the folder count: measured before the
    /// `set_max_width` that now caps the scroll area's content, a rail with
    /// one folder laid its rows out 10..212 and a rail with 26 laid them
    /// 10..202 -- so adding a folder slid every row's right edge, every
    /// badge and the whole glyph column 10pt sideways, and a fitting rail's
    /// rows ran flush to the panel's edge with no padding at all.
    ///
    /// The item list solves the same problem with `AlwaysVisible` plus
    /// `theme::hide_scrollbar`; the rail cannot, because that pairing needs to
    /// know on each frame whether the content fits and the rail's height is
    /// not known until it has been drawn. See the note at the
    /// `scrollbar_in_gutter` call.
    ///
    /// Asserted as an EQUALITY between the two states and then against the
    /// absolute number, because "both states agree" is a claim two equally
    /// wrong layouts would also satisfy.
    #[test]
    fn the_rails_rows_keep_one_width_whether_or_not_the_bar_is_showing() {
        let fits = rail_geometry(1, TALL_RAIL);
        let scrolls = rail_geometry(26, TALL_RAIL);

        assert!(
            fits.bar.is_empty(),
            "a rail with one folder in a {TALL_RAIL}pt window painted something visible in the \
             bar's lane: {:?}. It has nothing to scroll, so this test is no longer comparing a \
             hidden bar against a shown one",
            fits.bar
        );
        assert!(
            !scrolls.bar.is_empty(),
            "a rail with 26 folders painted no visible bar, so this test is comparing two \
             hidden-bar frames and says nothing about the reservation"
        );

        assert_eq!(
            (fits.row.left(), fits.row.right()),
            (scrolls.row.left(), scrolls.row.right()),
            "the rows span {}..{} on a rail that fits and {}..{} on one that scrolls -- the \
             bar's lane is being reserved only on the frames that show a bar, so adding a \
             folder slides every row sideways",
            fits.row.left(),
            fits.row.right(),
            scrolls.row.left(),
            scrolls.row.right()
        );
        assert_eq!(
            (fits.row.left(), fits.row.right()),
            (ROWS_LEFT, ROWS_RIGHT),
            "both states agree, on {}..{} rather than the {ROWS_LEFT}..{ROWS_RIGHT} the panel \
             frame and the lane are meant to leave",
            fits.row.left(),
            fits.row.right()
        );
    }

    // ---- design 5b's ORGANISATIONS section ----------------------------------

    /// One organisation and one collection, built through the REAL mapper --
    /// `Directory::from_sync` over a sync payload -- rather than through a
    /// test-only constructor.
    ///
    /// Two reasons, and the second is the one that matters. `Directory`'s
    /// fields are private and a `pub fn` taking them would be a door this
    /// feature does not need. And a rail test that built its directory by
    /// hand would pass on a shape `rest::sync` never produces, which is
    /// exactly the always-green test this file's history is made of.
    ///
    /// **Not called `Engineering`**: this file's folder fixture already has a
    /// folder by that name, and a painted-text search cannot tell two rows
    /// with one label apart.
    fn a_directory() -> crate::rest::organizations::Directory {
        let response: crate::rest::sync::SyncResponse = serde_json::from_value(serde_json::json!({
            "profile": {
                "key": "2.aaa|bbb|ccc",
                "organizations": [{ "id": "org-1", "name": "Acme Security", "enabled": true }]
            },
            "ciphers": [],
            "folders": [],
            "collections": [
                { "id": "col-1", "organizationId": "org-1", "name": "Production" }
            ]
        }))
        .expect("the fixture parses");
        crate::rest::organizations::Directory::from_sync(
            &response,
            &crate::rest::sync::tests::keys_from_user(&[3u8; 64]),
        )
    }

    /// One item owned by the fixture organisation and filed in the given
    /// collections. The two facts ride `VaultItem::other`, which is where
    /// `rest::sync`'s mapper leaves them and the only place this crate reads
    /// them from.
    fn shared_item(collections: &[&str]) -> VaultItem {
        let mut shared = item(Some(1), false, None);
        shared.other.insert("organizationId".into(), "org-1".into());
        shared.other.insert(
            "collectionIds".into(),
            serde_json::Value::Array(
                collections.iter().map(|id| serde_json::Value::String((*id).into())).collect(),
            ),
        );
        shared
    }

    /// **The zero-organisation case, which is this owner's own: the section
    /// does not exist.**
    ///
    /// Not "is empty", not "is collapsed" -- *absent*. No label, no divider,
    /// not a pixel, so a rail on an account that has never touched
    /// organisations is the rail it was before this section was written.
    ///
    /// **Both halves, and the second is what stops the first being vacuous.**
    /// An assertion that a string is missing passes just as well when the
    /// section is broken as when it is deliberately absent, so the same
    /// harness is run again WITH a directory and the same two facts are
    /// asserted the other way round.
    #[test]
    fn the_rail_has_no_organisations_section_without_an_organisation() {
        let live = three_unfiled_and_two_filed();
        let folders = one_real_folder_and_the_virtual_bucket();

        let (painted, _, _, _) =
            painted_sidebar_parts("Locks in 11:42", VaultLists::live_only(&live), &folders);
        assert!(
            !painted.iter().any(|(text, _)| text == ORGANISATIONS_SECTION_LABEL),
            "an account with no organisations was given an ORGANISATIONS heading: {painted:?}"
        );
        assert!(
            !painted.iter().any(|(text, _)| text == "Acme Security" || text == "Production"),
            "the rail painted an organisation nobody is in: {painted:?}"
        );

        // The control. Same harness, same vault, same folders; only the
        // directory differs.
        let sharing = a_directory();
        let (painted, _, _, _) = painted_sidebar_parts(
            "Locks in 11:42",
            VaultLists { sharing: &sharing, ..VaultLists::live_only(&live) },
            &folders,
        );
        assert!(
            painted.iter().any(|(text, _)| text == ORGANISATIONS_SECTION_LABEL),
            "control: the section does not appear even WITH an organisation, so the absence \
             asserted above is not evidence of anything: {painted:?}"
        );
    }

    /// **Where the section sits, top to bottom.** Below the folders, because
    /// an organisation row is a cut of the vault and belongs on that side of
    /// the rail's first boundary; above Password health, because everything
    /// below THAT boundary is a screen rather than a cut. See
    /// [`draw_organisations`], which argues the placement, and
    /// [`screen_rows`], which drew the line it is placed against.
    ///
    /// The collection is under its own organisation, which is the whole shape
    /// of the section: a parent naming the *whom* and children naming the
    /// *what*.
    #[test]
    fn the_organisations_section_sits_between_the_folders_and_the_screens() {
        let live = three_unfiled_and_two_filed();
        let folders = one_real_folder_and_the_virtual_bucket();
        let sharing = a_directory();
        let (painted, _, _, _) = painted_sidebar_parts(
            "Locks in 11:42",
            VaultLists { sharing: &sharing, ..VaultLists::live_only(&live) },
            &folders,
        );
        let top_of = |needle: &str| row_top(&painted, needle);

        // v2: the folders, Shared with me and New folder, then ORGANISATIONS.
        let order = [
            ("FOLDERS", top_of("FOLDERS")),
            ("No Folder", top_of("No Folder")),
            (RECEIVED_ROW_LABEL, top_of(RECEIVED_ROW_LABEL)),
            (NEW_FOLDER_LABEL, top_of(NEW_FOLDER_LABEL)),
            (ORGANISATIONS_SECTION_LABEL, top_of(ORGANISATIONS_SECTION_LABEL)),
            ("Acme Security", top_of("Acme Security")),
            ("Production", top_of("Production")),
        ];
        for pair in order.windows(2) {
            assert!(
                pair[0].1 < pair[1].1,
                "{} is painted at y={} and {} at y={} -- the rail is not in the order \
                 folders / ORGANISATIONS / Password health / SHARING",
                pair[0].0,
                pair[0].1,
                pair[1].0,
                pair[1].1
            );
        }
    }

    /// **A collection is indented under its organisation by exactly the
    /// indent design 5b gives a SHARING sub-row**, because it is the same
    /// relationship: a parent naming a set and a child naming a cut of it.
    ///
    /// Measured against the SHARING rows in the same painted frame rather
    /// than against the constant, so a rail with two different depths of
    /// "child row" in it fails here even if both were spelled from the same
    /// number.
    #[test]
    fn a_collection_is_indented_under_its_organisation_like_a_sharing_sub_row() {
        let live = three_unfiled_and_two_filed();
        let folders = one_real_folder_and_the_virtual_bucket();
        let sharing = a_directory();
        let (painted, _, _, _) = painted_sidebar_parts(
            "Locks in 11:42",
            VaultLists { sharing: &sharing, ..VaultLists::live_only(&live) },
            &folders,
        );
        let left_of = |needle: &str| {
            painted
                .iter()
                .find(|(text, _)| text == needle)
                .unwrap_or_else(|| panic!("the rail painted no {needle:?}"))
                .1
                .left()
        };

        // The SHARING section has no sub-rows any more (the owner: "only have
        // Sent and Shared with me"), so the collection is held to the
        // sub-row depth itself rather than to a row that no longer exists.
        let collection_indent = left_of("Production") - left_of("Acme Security");
        assert!(
            (collection_indent - SUB_ROW_EXTRA_INDENT).abs() < 0.01,
            "control: neither row is indented by `SUB_ROW_EXTRA_INDENT` at all \
             ({collection_indent}pt against {SUB_ROW_EXTRA_INDENT}pt), so the comparison \
             above is between two wrong numbers"
        );
    }

    /// **What the two rows actually list**, which is the half a painted frame
    /// cannot show.
    ///
    /// Three items: one personal, one shared and filed in the collection, one
    /// shared and filed in nothing. The organisation row holds both shared
    /// ones and neither row holds the personal one -- and the collection row
    /// holds only the item filed in it, which is what makes it a cut of its
    /// parent rather than a second name for it.
    ///
    /// Through [`badge_for`], so the badge the rail draws and the list the
    /// item pane shows are one read; `item_list::matches_filter` delegates to
    /// the same [`SidebarFilter::scope_contains`].
    #[test]
    fn an_organisation_row_lists_its_items_and_a_collection_row_lists_its_own() {
        let live = vec![item(Some(1), false, None), shared_item(&["col-1"]), shared_item(&[])];
        let sharing = a_directory();
        let lists = VaultLists { sharing: &sharing, ..VaultLists::live_only(&live) };

        assert_eq!(badge_for(&SidebarFilter::All, lists), Some(3));
        assert_eq!(
            badge_for(&SidebarFilter::Organisation("org-1".into()), lists),
            Some(2),
            "the organisation row is not the organisation's items"
        );
        assert_eq!(
            badge_for(&SidebarFilter::Collection("col-1".into()), lists),
            Some(1),
            "the collection row is not a cut of its parent"
        );
        // An organisation nobody is in has no items, and says zero rather
        // than matching everything.
        assert_eq!(badge_for(&SidebarFilter::Organisation("org-2".into()), lists), Some(0));

        // An item can be in several collections at once, which is the
        // property that makes a collection unlike a folder -- and the reason
        // `scope_contains` asks `contains` rather than comparing.
        let both = vec![shared_item(&["col-1", "col-2"])];
        let lists = VaultLists { sharing: &sharing, ..VaultLists::live_only(&both) };
        assert_eq!(badge_for(&SidebarFilter::Collection("col-1".into()), lists), Some(1));
        assert_eq!(badge_for(&SidebarFilter::Collection("col-2".into()), lists), Some(1));
    }

    /// **An organisation row is an item row**, so the invariant every item
    /// row carries holds here without being re-implemented: it selects
    /// itself, and it clears every screen flag.
    ///
    /// That is what `item_row` is for, and it is asserted rather than assumed
    /// because a section written with a bare `sidebar_row` would look
    /// identical on screen and would leave the window painting the Sends
    /// screen while the rail highlighted a collection.
    #[test]
    fn pressing_an_organisation_or_collection_row_clears_the_screens() {
        let live = vec![shared_item(&["col-1"])];
        let folders = one_real_folder_and_the_virtual_bucket();
        let sharing = a_directory();
        let lists = VaultLists { sharing: &sharing, ..VaultLists::live_only(&live) };

        let (filter, sends, health) =
            press_row_screens("Acme Security", SidebarFilter::Logins, true, true, lists, &folders);
        assert_eq!(filter, SidebarFilter::Organisation("org-1".into()));
        assert!(
            !sends && !health,
            "an organisation row was clicked and a screen stayed up (sends={sends}, \
             health={health})"
        );

        let (filter, sends, health) =
            press_row_screens("Production", SidebarFilter::Logins, true, true, lists, &folders);
        assert_eq!(filter, SidebarFilter::Collection("col-1".into()));
        assert!(
            !sends && !health,
            "a collection row was clicked and a screen stayed up (sends={sends}, \
             health={health})"
        );
    }

    /// **The rail's overflow, re-measured with this section in it.**
    ///
    /// `the_screen_rows_survive_a_vault_with_a_folder_for_every_letter` is the
    /// existing measurement and it is untouched, because it runs on an EMPTY
    /// directory and this section draws nothing there -- which is the whole
    /// point of the empty case. This is the same measurement on the rail that
    /// now exists for an account that does share: four organisations and
    /// their collections on top of twenty-six folders.
    ///
    /// Both ends again, for that test's own reason: the rows really are
    /// pushed past the floor by this much rail (so this is not a test of a
    /// rail that happens to fit), and scrolling really does bring every one
    /// of them back.
    #[test]
    fn the_organisations_section_overflows_the_rail_and_everything_is_still_reachable() {
        let live = vec![shared_item(&["col-1"])];
        let folders: Vec<Folder> = ('a'..='z')
            .map(|c| Folder {
                id: format!("f-{c}"),
                name: format!("Folder {}", c.to_ascii_uppercase()),
                other: serde_json::Map::new(),
            })
            .collect();
        let organisations: Vec<serde_json::Value> = (1..=4)
            .map(|n| serde_json::json!({ "id": format!("org-{n}"),
                                         "name": format!("Organisation {n}") }))
            .collect();
        let collections: Vec<serde_json::Value> = (1..=4)
            .flat_map(|n| {
                (1..=2).map(move |c| {
                    serde_json::json!({
                        "id": format!("col-{n}-{c}"),
                        "organizationId": format!("org-{n}"),
                        "name": format!("Collection {n}.{c}")
                    })
                })
            })
            .collect();
        let response: crate::rest::sync::SyncResponse = serde_json::from_value(serde_json::json!({
            "profile": { "key": "2.aaa|bbb|ccc", "organizations": organisations },
            "ciphers": [],
            "folders": [],
            "collections": collections
        }))
        .expect("the fixture parses");
        let sharing = crate::rest::organizations::Directory::from_sync(
            &response,
            &crate::rest::sync::tests::keys_from_user(&[3u8; 64]),
        );
        let lists = VaultLists { sharing: &sharing, ..VaultLists::live_only(&live) };

        // Unscrolled: the section's own first row is already past the floor.
        let (resting, bounds) = scrolled_rail(lists, &folders, 0.0);
        let first_at_rest = row_top(&resting, "Organisation 1");
        assert!(
            first_at_rest > bounds.bottom(),
            "the ORGANISATIONS section no longer overflows the rail (its first row rests at \
             y={first_at_rest}, floor y={}), so this test is not measuring what it is named \
             for",
            bounds.bottom()
        );

        // Scrolled to the end: every row the section added, and every row it
        // pushed further down, is inside the rail.
        let (scrolled, bounds) = scrolled_rail(lists, &folders, -6000.0);
        for label in ["Organisation 4", "Collection 4.2"] {
            let rect = scrolled
                .iter()
                .find(|(text, _)| text == label)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{label:?} was not painted at all: {scrolled:?}"));
            assert!(
                bounds.contains_rect(rect),
                "{label:?} is at {rect:?} after scrolling the rail to its end, still outside \
                 the rail's own {bounds:?} -- the section is unreachable with {} folders and \
                 four organisations",
                folders.len()
            );
        }
    }

    /// **The ORGANISATIONS rail never lists a sharing org** -- spec 6.9's
    /// hidden org. It is plumbing; its records live under Shared with me and
    /// Sent instead.
    #[test]
    fn sharing_orgs_are_absent_from_the_organisations_rail() {
        let response: crate::rest::sync::SyncResponse = serde_json::from_value(serde_json::json!({
            "profile": { "key": "2.a|b|c", "email": "me@example.com", "organizations": [
                { "id": "o-eng", "name": "Engineering" },
                { "id": "o-share", "name": "Shared by me@example.com", "role": "owner" }
            ]},
            "ciphers": [], "folders": [], "collections": []
        })).expect("parses");
        let sharing = crate::rest::organizations::Directory::from_sync(
            &response,
            &crate::rest::sync::tests::keys_from_user(&[1u8; 64]),
        );
        let lists = VaultLists { sharing: &sharing, ..VaultLists::live_only(&[]) };
        // `scrolled_rail` is the sidebar tests' own harness (see the
        // ORGANISATIONS overflow test above): it draws the real rail and
        // returns every painted label with its rect. Scrolled to the end so
        // the section is on screen whatever the rail's height.
        let (painted, _bounds) = scrolled_rail(lists, &[], -6000.0);
        let labels: Vec<&str> = painted.iter().map(|(text, _)| text.as_str()).collect();
        assert!(labels.contains(&"Engineering"), "control: the rail drew no organisation at all: {labels:?}");
        assert!(!labels.iter().any(|l| l.starts_with("Shared by")), "a sharing org reached the rail: {labels:?}");
    }

    /// **Sent and Shared with me count records too** (spec 6.10), and a
    /// pending invitation puts a dot on the Shared-with-me row (mock 6d).
    #[test]
    fn the_sharing_rows_count_person_shares_and_received_records() {
        let lists = VaultLists { shared_records: 3, received_records: 2, invitation_waiting: true, ..VaultLists::live_only(&[]) };
        let (painted, _bounds) = scrolled_rail(lists, &[], -6000.0);
        let near = |label: &str| painted.iter().find(|(t, _)| t == label).map(|(_, r)| *r).expect(label);
        let sent = near(SENDS_ROW_LABEL);
        let received = near(RECEIVED_ROW_LABEL);
        assert!(painted.iter().any(|(t, r)| t == "3" && (r.center().y - sent.center().y).abs() < 2.0), "Sent does not count 3: {painted:?}");
        assert!(painted.iter().any(|(t, r)| t == "2" && (r.center().y - received.center().y).abs() < 2.0), "Shared with me does not count 2: {painted:?}");
    }

    /// Unknown Sends and no person shares stay unknown -- the en dash, not a
    /// `0` -- and known Sends add to the shares.
    #[test]
    fn sent_is_unknown_only_while_both_halves_are() {
        let (unknown, _) = scrolled_rail(VaultLists::live_only(&[]), &[], -6000.0);
        assert_eq!(badge_beside(&unknown, SENDS_ROW_LABEL), UNKNOWN_COUNT);
        let both = VaultLists {
            sends: Some(SendCounts::over([crate::send::SendState::Waiting, crate::send::SendState::Used])),
            shared_records: 3,
            ..VaultLists::live_only(&[])
        };
        let (painted, _) = scrolled_rail(both, &[], -6000.0);
        assert_eq!(badge_beside(&painted, SENDS_ROW_LABEL), "5");
    }

    /// **The invitation dot**: one blue 6px dot on the Shared-with-me row,
    /// between its label and its count, 6px clear of the count -- and none
    /// when nothing is waiting.
    #[test]
    fn a_waiting_invitation_dots_shared_with_me_beside_its_count() {
        fn blue_dots(shape: &egui::Shape, out: &mut Vec<egui::Pos2>) {
            match shape {
                egui::Shape::Circle(c) if c.fill == theme::BLUE_BRIGHT && (c.radius - 3.0).abs() < 0.01 => out.push(c.center),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| blue_dots(s, out)),
                _ => {}
            }
        }
        let dots_of = |lists: VaultLists<'_>| {
            let (shapes, _) = scrolled_rail_shapes(lists, &[], -6000.0);
            let mut texts = Vec::new();
            let mut dots = Vec::new();
            for shape in &shapes {
                collect_text_rects(shape, &mut texts);
                blue_dots(shape, &mut dots);
            }
            (texts, dots)
        };
        let (_, none) = dots_of(VaultLists { received_records: 12, ..VaultLists::live_only(&[]) });
        assert!(none.is_empty(), "no invitation, no dot: {none:?}");
        let (texts, dots) = dots_of(VaultLists { received_records: 12, invitation_waiting: true, ..VaultLists::live_only(&[]) });
        let label = texts.iter().find(|(t, _)| t == RECEIVED_ROW_LABEL).map(|(_, r)| *r).expect("the row");
        let count = texts.iter().find(|(t, r)| t == "12" && (r.center().y - label.center().y).abs() < 2.0).map(|(_, r)| *r).expect("its count");
        assert_eq!(dots.len(), 1, "{dots:?}");
        let dot = dots[0];
        assert!((dot.y - label.center().y).abs() < 2.0, "the dot is on the row");
        assert!(dot.x > label.right(), "the dot is right of the label");
        assert!((count.left() - (dot.x + 3.0) - 6.0).abs() < 0.5, "6px between the dot and a two-digit count: {dot:?} {count:?}");
    }
}

/// **The v2 rail** (design 1a, spec part A section 2.2): navy or light, a
/// logo, New item, VAULT / TOOLS / FOLDERS, the inline New folder row and a
/// pinned Lock row.
#[cfg(test)]
mod v2_rail_tests {
    use super::*;
    use crate::kit::testing::{self, Painted};
    use crate::kit::tokens::{set_sidebar_style, SidebarStyle};

    /// Tall enough for every row to be on screen: these tests click rows.
    const SIZE: egui::Vec2 = egui::vec2(252.0, 2000.0);
    /// The design's own height, for the tests about what stays in view.
    const DESIGN: egui::Vec2 = egui::vec2(252.0, 820.0);

    struct Rail {
        painted: Painted,
        action: SidebarAction,
        panel: egui::Rect,
    }

    fn folder(id: &str, name: &str) -> Folder {
        Folder { id: id.into(), name: name.into(), other: serde_json::Map::new() }
    }

    fn login(id: &str, folder: Option<&str>) -> VaultItem {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": format!("Login {id}"), "fields": [], "type": 1,
            "login": {}, "folderId": folder
        }))
        .unwrap()
    }

    struct Fixture {
        items: Vec<VaultItem>,
        folders: Vec<Folder>,
        selected: SidebarFilter,
        size: egui::Vec2,
        sends: bool,
        health: bool,
        received: bool,
        scope: SendScope,
    }

    impl Fixture {
        fn new(folders: Vec<Folder>) -> Self {
            Fixture {
                items: vec![login("1", None), login("2", Some("f1"))],
                folders,
                selected: SidebarFilter::All,
                size: SIZE,
                sends: false,
                health: false,
                received: false,
                scope: SendScope::All,
            }
        }

        fn frame(&mut self, ctx: &egui::Context, time: f64, events: Vec<egui::Event>, countdown: &str) -> Rail {
            let mut action = SidebarAction::None;
            let mut panel = egui::Rect::NOTHING;
            let painted = testing::frame(ctx, time, self.size, events, |ui| {
                let style = crate::kit::tokens::sidebar_style(ui.ctx());
                panel = egui::Panel::left("vault-sidebar")
                    .exact_size(crate::vault_window::SIDEBAR_WIDTH)
                    .resizable(false)
                    .frame(panel_frame(style))
                    .show(ui, |ui| {
                        action = draw_sidebar(
                            ui,
                            VaultLists::live_only(&self.items),
                            &self.folders,
                            &mut self.selected,
                            Screens {
                                sends: &mut self.sends,
                                health: &mut self.health,
                                received: &mut self.received,
                                generator: &mut false,
                                settings: &mut false,
                                scope: &mut self.scope,
                            },
                            countdown,
                            true,
                        );
                    })
                    .response
                    .rect;
            });
            Rail { painted, action, panel }
        }
    }

    fn row_order(painted: &Painted) -> Vec<String> {
        let mut rows: Vec<_> = painted.texts.iter().map(|(t, _, r)| (r.top(), t.clone())).collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        rows.into_iter().map(|(_, t)| t).collect()
    }

    fn y_of(painted: &Painted, text: &str) -> f32 {
        painted
            .texts
            .iter()
            .find(|(t, _, _)| t == text)
            .unwrap_or_else(|| panic!("{text:?} not painted; got {:?}", row_order(painted)))
            .2
            .center()
            .y
    }

    #[test]
    fn the_sidebar_is_252_wide_and_wears_its_styles_colours() {
        for style in [SidebarStyle::Navy, SidebarStyle::Light] {
            let ctx = testing::ctx();
            set_sidebar_style(&ctx, style);
            let mut fixture = Fixture::new(vec![]);
            let rail = fixture.frame(&ctx, 0.0, vec![], "Locks in 11:42");
            let sb = style.palette();
            assert_eq!(rail.panel.width(), 252.0);
            assert!(
                rail.painted.rects.iter().any(|r| r.fill == sb.bg && r.rect.width() >= 252.0),
                "{style:?}: no panel fill of {:?}",
                sb.bg
            );
            let (_, ink, _) = rail.painted.text("Deskwarden").expect("the wordmark");
            assert_eq!(*ink, sb.text, "{style:?}");
        }
    }

    #[test]
    fn the_vault_section_lists_its_rows_in_order() {
        let ctx = testing::ctx();
        let rail = Fixture::new(vec![]).frame(&ctx, 0.0, vec![], "Locks in 11:42");
        let order = row_order(&rail.painted);
        let vault: Vec<&str> = [
            "All items", "Favorites", "Apps", "Logins", "Cards", "Secure notes", "Archive", "Trash",
        ]
        .to_vec();
        let positions: Vec<usize> = vault
            .iter()
            .map(|name| order.iter().position(|t| t == name).unwrap_or_else(|| panic!("{name} missing: {order:?}")))
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{order:?}");
        let section = order.iter().position(|t| t == "VAULT").expect("a VAULT label");
        assert!(section < positions[0]);
    }

    /// Passkeys, Identities and SSH keys are 0.15's, not design 1a's: they
    /// show once they hold something, and not before.
    #[test]
    fn empty_passkeys_identities_and_ssh_rows_hide_until_they_hold_something() {
        let ctx = testing::ctx();
        let rail = Fixture::new(vec![]).frame(&ctx, 0.0, vec![], "Locks in 11:42");
        for hidden in ["Passkeys", "Identities", "SSH keys"] {
            assert!(rail.painted.text(hidden).is_none(), "{hidden} shown while empty");
        }
        let mut fixture = Fixture::new(vec![]);
        fixture.items.push(
            serde_json::from_value(serde_json::json!({
                "id": "9", "name": "Me", "fields": [], "type": 4, "identity": {}
            }))
            .unwrap(),
        );
        let rail = fixture.frame(&testing::ctx(), 0.0, vec![], "Locks in 11:42");
        assert!(rail.painted.text("Identities").is_some());
    }

    #[test]
    fn the_tools_section_holds_shared_password_health_and_settings() {
        let ctx = testing::ctx();
        let rail = Fixture::new(vec![folder("f1", "Engineering")]).frame(&ctx, 0.0, vec![], "Locks in 11:42");
        let order = row_order(&rail.painted);
        let at = |t: &str| order.iter().position(|x| x == t).unwrap_or_else(|| panic!("{t} missing: {order:?}"));
        assert!(at("Trash") < at("TOOLS"));
        assert!(at("TOOLS") < at(SENDS_ROW_LABEL));
        assert!(at(SENDS_ROW_LABEL) < at(crate::vault_window::password_health::HEALTH_ROW_LABEL));
        assert!(at(crate::vault_window::password_health::HEALTH_ROW_LABEL) < at(SETTINGS_ROW_LABEL));
        assert!(at(SETTINGS_ROW_LABEL) < at("FOLDERS"));
        assert!(at("FOLDERS") < at("Engineering"));
        assert!(at("Engineering") < at(RECEIVED_ROW_LABEL));
        assert!(at(RECEIVED_ROW_LABEL) < at(NEW_FOLDER_LABEL));
    }

    #[test]
    fn the_active_row_is_painted_in_the_styles_active_colours() {
        let ctx = testing::ctx();
        let rail = Fixture::new(vec![]).frame(&ctx, 0.0, vec![], "Locks in 11:42");
        let sb = SidebarStyle::Navy.palette();
        let (_, ink, at) = rail.painted.text("All items").unwrap();
        assert_eq!(*ink, sb.active_fg);
        assert!(rail.painted.rects.iter().any(|r| r.fill == sb.active && r.rect.contains(at.center())));
        assert_eq!(rail.painted.text("Logins").unwrap().1, sb.nav_fg);
    }

    #[test]
    fn rows_are_44_tall_and_two_apart() {
        let ctx = testing::ctx();
        let rail = Fixture::new(vec![]).frame(&ctx, 0.0, vec![], "Locks in 11:42");
        let sb = SidebarStyle::Navy.palette();
        let active = rail.painted.rects.iter().find(|r| r.fill == sb.active).expect("the active row");
        assert_eq!(active.rect.height(), ROW_HEIGHT);
        assert_eq!(ROW_HEIGHT, 44.0);
        let pitch = y_of(&rail.painted, "Favorites") - y_of(&rail.painted, "All items");
        assert!((pitch - (ROW_HEIGHT + ROW_GAP)).abs() < 0.5, "pitch {pitch}");
    }

    fn click_text(fixture: &mut Fixture, ctx: &egui::Context, text: &str) -> SidebarAction {
        let first = fixture.frame(ctx, 0.0, vec![], "Locks in 11:42");
        let at = first.painted.text(text).unwrap_or_else(|| panic!("{text} not painted")).2.center();
        fixture.frame(ctx, 0.1, testing::click_at(at), "Locks in 11:42").action
    }

    #[test]
    fn new_item_returns_new_item() {
        let ctx = testing::ctx();
        assert_eq!(click_text(&mut Fixture::new(vec![]), &ctx, NEW_ITEM_LABEL), SidebarAction::NewItem);
    }

    #[test]
    fn clicking_lock_returns_lock() {
        let ctx = testing::ctx();
        assert_eq!(click_text(&mut Fixture::new(vec![]), &ctx, LOCK_ROW_LABEL), SidebarAction::Lock);
    }

    #[test]
    fn clicking_settings_opens_settings() {
        let ctx = testing::ctx();
        assert_eq!(click_text(&mut Fixture::new(vec![]), &ctx, SETTINGS_ROW_LABEL), SidebarAction::OpenSettings);
    }

    fn typed(text: &str) -> Vec<egui::Event> {
        vec![egui::Event::Text(text.into())]
    }

    fn key(key: egui::Key) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }]
    }

    #[test]
    fn the_new_folder_row_opens_an_input_and_enter_creates_the_folder() {
        let ctx = testing::ctx();
        let mut fixture = Fixture::new(vec![]);
        assert_eq!(click_text(&mut fixture, &ctx, NEW_FOLDER_LABEL), SidebarAction::None);
        let open = fixture.frame(&ctx, 0.2, vec![], "Locks in 11:42");
        assert!(open.painted.text(NEW_FOLDER_LABEL).is_none(), "the row became an input");
        assert!(ctx.memory(|m| m.focused().is_some()), "the input takes the keyboard");
        fixture.frame(&ctx, 0.3, typed("Taxes"), "Locks in 11:42");
        assert!(ctx.text_edit_focused(), "letters go to the folder name, not to vault shortcuts");
        let done = fixture.frame(&ctx, 0.4, key(egui::Key::Enter), "Locks in 11:42");
        assert_eq!(done.action, SidebarAction::CreateFolder("Taxes".into()));
        let after = fixture.frame(&ctx, 0.5, vec![], "Locks in 11:42");
        assert!(after.painted.text(NEW_FOLDER_LABEL).is_some(), "and closes again");
    }

    #[test]
    fn escape_or_a_blank_name_creates_nothing() {
        let ctx = testing::ctx();
        let mut fixture = Fixture::new(vec![]);
        click_text(&mut fixture, &ctx, NEW_FOLDER_LABEL);
        fixture.frame(&ctx, 0.2, typed("   "), "Locks in 11:42");
        let blank = fixture.frame(&ctx, 0.3, key(egui::Key::Enter), "Locks in 11:42");
        assert_eq!(blank.action, SidebarAction::None);

        let ctx = testing::ctx();
        let mut fixture = Fixture::new(vec![]);
        click_text(&mut fixture, &ctx, NEW_FOLDER_LABEL);
        fixture.frame(&ctx, 0.2, typed("Taxes"), "Locks in 11:42");
        let escaped = fixture.frame(&ctx, 0.3, key(egui::Key::Escape), "Locks in 11:42");
        assert_eq!(escaped.action, SidebarAction::None);
        let after = fixture.frame(&ctx, 0.4, vec![], "Locks in 11:42");
        assert!(after.painted.text(NEW_FOLDER_LABEL).is_some());
    }

    #[test]
    fn the_lock_row_is_pinned_below_the_rows_and_shows_the_countdown() {
        let ctx = testing::ctx();
        let mut fixture = Fixture::new(vec![]);
        fixture.size = DESIGN;
        let rail = fixture.frame(&ctx, 0.0, vec![], "Locks in 11:42");
        let lock = y_of(&rail.painted, LOCK_ROW_LABEL);
        let time = y_of(&rail.painted, "11:42");
        assert!((lock - time).abs() < 2.0, "the countdown sits on the Lock row");
        assert!(rail.panel.bottom() - lock < 16.0 + ROW_HEIGHT, "Lock is at the foot: {lock}");
        let off = Fixture::new(vec![]).frame(&testing::ctx(), 0.0, vec![], "Auto-lock is off");
        assert!(off.painted.text("Off").is_some());
    }

    #[test]
    fn the_lock_row_stays_visible_with_a_hundred_folders() {
        let ctx = testing::ctx();
        let folders: Vec<Folder> = (0..100).map(|i| folder(&format!("f{i}"), &format!("Folder {i:03}"))).collect();
        let mut fixture = Fixture::new(folders);
        fixture.size = DESIGN;
        let rail = fixture.frame(&ctx, 0.0, vec![], "Locks in 11:42");
        let lock = rail.painted.text(LOCK_ROW_LABEL).unwrap().2;
        assert!(lock.bottom() < rail.panel.bottom(), "Lock at {lock:?} is off the panel");
        // What a person can SEE of each folder row: its rect inside the clip
        // it was painted under. None of it reaches the Lock row.
        for ((text, _, rect), clip) in rail.painted.texts.iter().zip(&rail.painted.text_clips) {
            if text.starts_with("Folder ") {
                let seen = rect.intersect(*clip);
                assert!(
                    !seen.is_positive() || seen.bottom() <= lock.top(),
                    "{text} shows at {seen:?}, over the Lock row at {lock:?}"
                );
            }
        }
    }

    #[test]
    fn the_folder_pencil_shows_on_hover_and_still_edits() {
        let ctx = testing::ctx();
        let mut fixture = Fixture::new(vec![folder("f1", "Engineering")]);
        let first = fixture.frame(&ctx, 0.0, vec![], "Locks in 11:42");
        let row = first.painted.text("Engineering").unwrap().2;
        let paths_without_hover = first.painted.rects.len();
        let pencil_x = first.panel.right() - PANEL_PAD_X as f32 - FOLDER_EDIT_BUTTON_WIDTH / 2.0;
        let pencil = egui::pos2(pencil_x, row.center().y);
        fixture.frame(&ctx, 0.1, testing::hover_at(pencil), "Locks in 11:42");
        let hovered = fixture.frame(&ctx, 0.2, testing::hover_at(pencil), "Locks in 11:42");
        assert!(hovered.painted.rects.len() >= paths_without_hover);
        let clicked = fixture.frame(&ctx, 0.3, testing::click_at(pencil), "Locks in 11:42");
        assert_eq!(clicked.action, SidebarAction::EditFolder("f1".into()));
    }

    #[test]
    fn a_long_folder_name_ellipsises_before_its_count() {
        let ctx = testing::ctx();
        let long = "A folder whose name is far too long for any sidebar ever drawn";
        let rail = Fixture::new(vec![folder("f1", long)]).frame(&ctx, 0.0, vec![], "Locks in 11:42");
        let (drawn, _, rect) = rail
            .painted
            .texts
            .iter()
            .find(|(t, _, _)| t.starts_with("A folder"))
            .expect("the folder row");
        assert!(drawn.ends_with('…'), "{drawn}");
        let count = rail
            .painted
            .texts
            .iter()
            .find(|(t, _, r)| t == "1" && (r.center().y - rect.center().y).abs() < 4.0)
            .expect("its count")
            .2;
        assert!(rect.right() < count.left(), "{rect:?} runs into {count:?}");
    }
}

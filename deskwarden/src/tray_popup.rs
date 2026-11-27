//! **Design 1d: the tray popup.** Left-click on the tray icon opens this small
//! panel -- search, the four most used logins with a copy-username and a
//! copy-password button each, Generate / New item / Share, and a status line --
//! instead of the vault window. Right-click keeps a short native menu
//! (`crate::tray`), so a keyboard or screen-reader user still has one.
//!
//! # Bare Win32, in the panel look
//!
//! The daemon is the memory-sensitive process: an egui window costs it ~50 MB
//! of OpenGL driver arenas for the rest of its life, against low single-digit
//! MB for a GDI window (`crate::locked_card` carries the measurement). So this
//! panel is painted by hand through `crate::panel_draw` and `crate::win32_draw`,
//! exactly as slice 11's fill-time cards are, and it creates **no thread**: it
//! is modal on the daemon's own thread for as long as it is up, like every other
//! card, and the loop's `pending_menu_events` sweep answers what was clicked at
//! the tray meanwhile.
//!
//! # No secret rides on anything here
//!
//! [`Item`] is an id, a name, a username and whether a password exists. The
//! password is fetched **at the moment of the copy**, by the closure the daemon
//! passes to [`run_with`], which is the one that asks the master-password
//! re-prompt first -- the same gate (`crate::reprompt::permit`) every other copy
//! and fill path goes through. The search the user types is a filter over names
//! and is never logged.
//!
//! # Layout of this file
//!
//! The decision half (state, layout, hit testing, the footer's words, the
//! reopen rule, [`run_with`]) is pure and tested without a desktop. The Win32
//! half is the `win32` module at the bottom, behind the [`PopupCalls`] seam.

use crate::i18n;
use crate::prompt_card::Box2;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The window's title. **Unique across this process**, because
/// `crate::foreground::pick` finds a window by title.
pub const TRAY_POPUP_TITLE: &str = "Deskwarden quick access";

// ---------------------------------------------------------------------------
// Geometry (design D 3.2). Logical pixels at 100%.
// ---------------------------------------------------------------------------

/// The panel's width (D 3.2: `width 390`).
pub const WIDTH: i32 = 390;
/// The panel's inner padding and the gap between its blocks (`pad 16`, `gap 14`).
pub const PAD: i32 = 16;
pub const GAP: i32 = 14;
/// How far the panel stands off the work area's edges (the design's 14 px).
pub const EDGE: i32 = 14;
/// The header's icon buttons (`36 x 36`) and the gap between the two.
pub const ICON_BUTTON: i32 = 36;
const ICON_BUTTON_GAP: i32 = 4;
/// The search box (`h44`, `r13`).
pub const SEARCH_H: i32 = 44;
pub const SEARCH_RADIUS: i32 = 13;
/// What sits left of the search text: `pad 0 14`, a 16 px magnifier, `gap 10`.
const SEARCH_PAD_X: i32 = 14;
const SEARCH_ICON: i32 = 16;
const SEARCH_ICON_GAP: i32 = 10;
/// The section label's height, and the gap under it (the list's `margin-top -6`
/// against the block gap leaves 8).
const LABEL_H: i32 = 14;
const LABEL_TO_LIST: i32 = 8;
/// A login row: a 38 px tile with 8 px round it, 2 px apart (D 3.2).
pub const ROW_AVATAR: i32 = 38;
const ROW_PAD: i32 = 8;
pub const ROW_H: i32 = ROW_AVATAR + 2 * ROW_PAD;
const ROW_GAP: i32 = 2;
pub const ROW_RADIUS: i32 = 13;
/// A row's two copy buttons (`36 x 36`, `r10`).
pub const ROW_BUTTON: i32 = 36;
const ROW_BUTTON_GAP: i32 = 2;
const ROW_TEXT_GAP: i32 = 12;
/// The action tiles (`h74`, `r15`, `gap 8`).
pub const TILE_H: i32 = 74;
pub const TILE_RADIUS: i32 = 15;
const TILE_GAP: i32 = 8;
/// The status footer.
pub const FOOTER_H: i32 = 20;
/// The footer's dot (`7 x 7`) and the gap after it.
pub const DOT: i32 = 7;
const DOT_GAP: i32 = 8;
/// The locked state's title, body and button (`TALL_BUTTON_H` of the fill
/// cards' Locked panel, D 1.6).
const TITLE_H: i32 = 26;
const BODY_H: i32 = 22;
const UNLOCK_H: i32 = crate::panel_draw::TALL_BUTTON_H;

/// How many logins the list shows: design 3.2's "first four logins", and the
/// cap on a search's results too, so the panel is never taller than this.
pub const ROWS_MAX: usize = 4;

/// How long a "Username copied" stays in the footer.
pub const TOAST_MS: u32 = 1800;

/// How long after the panel closed **because the pointer clicked away from it**
/// a click on the tray icon is still the same click. See [`click_reopens`].
pub const SAME_CLICK: Duration = Duration::from_millis(350);

// ---------------------------------------------------------------------------
// What the panel holds
// ---------------------------------------------------------------------------

/// One login, as the panel sees it. **No secret**: whether a password exists,
/// never what it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub username: String,
    pub has_password: bool,
}

/// Every login the daemon can see, and how often each was filled
/// (`crate::fill_stats`) -- the only record of use this app keeps.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub items: Vec<Item>,
    pub fill_counts: HashMap<String, u32>,
}

/// Whether the daemon can read a vault at all.
#[derive(Clone, Debug)]
pub enum Vault {
    /// Nothing is decrypted: the panel offers *Unlock* and nothing else.
    Locked,
    Open(Listing),
}

/// What the status footer reports about the last sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync {
    Synced,
    Syncing,
    /// The last sync failed: the vault is the copy this device already holds,
    /// and filling still works from it.
    Failed,
}

/// What the footer is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub sync: Sync,
    /// The idle auto-lock, in minutes, when one is set.
    pub auto_lock_minutes: Option<u64>,
}

/// How a footer's dot is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Green (`#1b7a3f`): synced.
    Good,
    /// Amber (`#b7791f`): offline.
    Warn,
    /// Neutral: locked, or working.
    Quiet,
}

/// The status footer's words and dot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Footer {
    pub text: String,
    pub tone: Tone,
}

/// **The footer's words**, decided here and painted by `win32`.
///
/// Design 3.2 says `Synced · locks in 14 min` and `Offline · filling still
/// works`. The first is a countdown, and nothing in the daemon keeps an idle
/// clock to count down from (the auto-lock timer lives in the vault window), so
/// it reads `locks after 15 min idle` instead -- the setting, which is true,
/// rather than a countdown, which would not be.
pub fn footer(locked: bool, status: &Status) -> Footer {
    if locked {
        return Footer { text: i18n::t("tray_popup.footer_locked").to_string(), tone: Tone::Quiet };
    }
    match status.sync {
        Sync::Failed => Footer {
            text: i18n::t("tray_popup.footer_offline").to_string(),
            tone: Tone::Warn,
        },
        Sync::Syncing => Footer { text: i18n::t("tray_popup.footer_syncing").to_string(), tone: Tone::Quiet },
        Sync::Synced => Footer {
            text: match status.auto_lock_minutes {
                Some(1) => i18n::t("tray_popup.footer_synced_one_minute").to_string(),
                Some(minutes) => i18n::tf("tray_popup.footer_synced_minutes", &[("minutes", &minutes.to_string())]),
                None => i18n::t("tray_popup.footer_synced").to_string(),
            },
            tone: Tone::Good,
        },
    }
}

/// **The footer's minutes**, from the auto-lock setting: rounded up and never
/// zero, so "locks after 0 min idle" is not a sentence anyone reads.
pub fn auto_lock_minutes(lock: crate::settings::AutoLock) -> Option<u64> {
    match lock {
        crate::settings::AutoLock::Never => None,
        crate::settings::AutoLock::After(after) => Some(after.as_secs().div_ceil(60).max(1)),
    }
}

/// Which of a login's two secrets-to-be a copy button is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Username,
    Password,
}

/// What a copy did, as the panel will say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyResult {
    Copied,
    /// The item has nothing in that field.
    Empty,
    /// The item asks for the master password and the user declined.
    Cancelled,
    /// The item asks for the master password and this account cannot be asked
    /// (no Windows Hello enrollment).
    NeedsHello,
    /// The item is no longer in the vault.
    Gone,
}

/// **The footer line a copy leaves behind.** Short on purpose: the footer is one
/// line, and a refusal that wraps is a refusal nobody reads.
pub fn toast_for(field: Field, result: CopyResult) -> &'static str {
    match (field, result) {
        (Field::Username, CopyResult::Copied) => i18n::t("tray_popup.toast_username_copied"),
        (Field::Password, CopyResult::Copied) => i18n::t("tray_popup.toast_password_copied"),
        (Field::Username, CopyResult::Empty) => i18n::t("tray_popup.toast_no_username"),
        (Field::Password, CopyResult::Empty) => i18n::t("tray_popup.toast_no_password"),
        (_, CopyResult::Cancelled) => i18n::t("tray_popup.toast_cancelled"),
        (_, CopyResult::NeedsHello) => i18n::t("tray_popup.toast_needs_hello"),
        (_, CopyResult::Gone) => i18n::t("tray_popup.toast_gone"),
    }
}

// ---------------------------------------------------------------------------
// Recent and search
// ---------------------------------------------------------------------------

/// **The rows shown when nothing is typed**: the logins filled most often, then
/// the rest in vault order, [`ROWS_MAX`] of them.
///
/// Design 3.2 says "the first four logins". This app keeps one record of use,
/// `crate::fill_stats`'s per-item fill count, so the most filled come first and
/// the vault's own order breaks ties -- which, on a vault never filled from, is
/// exactly the design's list. A login with no username and no password has
/// nothing to copy and is not offered.
///
/// Indices into [`Listing::items`], so no row is cloned.
pub fn recent(listing: &Listing) -> Vec<usize> {
    let mut order: Vec<usize> =
        (0..listing.items.len()).filter(|&i| copyable(&listing.items[i])).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(count(listing, &listing.items[i])));
    order.truncate(ROWS_MAX);
    order
}

/// What one keystroke found: up to [`ROWS_MAX`] rows and how many matched in all.
pub struct Found {
    pub rows: Vec<usize>,
    pub total: usize,
}

/// **The rows a typed filter keeps.** The predicate is
/// [`crate::picker_ui::name_matches_filter`] -- the vault window's own rule and
/// the account picker's -- plus the username, which is on the row and so is what
/// a user types to tell two accounts of one site apart. An empty filter is
/// [`recent`].
pub fn search(listing: &Listing, query: &str) -> Found {
    let filter = query.trim().to_lowercase();
    if filter.is_empty() {
        let rows = recent(listing);
        return Found { total: rows.len(), rows };
    }
    let mut hits: Vec<usize> = (0..listing.items.len())
        .filter(|&i| {
            let item = &listing.items[i];
            copyable(item)
                && (crate::picker_ui::name_matches_filter(&item.name, &filter)
                    || item.username.to_lowercase().contains(&filter))
        })
        .collect();
    let total = hits.len();
    hits.sort_by_key(|&i| std::cmp::Reverse(count(listing, &listing.items[i])));
    hits.truncate(ROWS_MAX);
    Found { rows: hits, total }
}

fn copyable(item: &Item) -> bool {
    !item.id.is_empty() && (!item.username.is_empty() || item.has_password)
}

fn count(listing: &Listing, item: &Item) -> u32 {
    listing.fill_counts.get(&item.id).copied().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The state, and what the window is told to draw
// ---------------------------------------------------------------------------

/// Which of the panel's two shapes is on screen, and how tall the list is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Locked: a message and *Unlock*.
    Locked,
    /// Unlocked, with this many rows (`0` is the empty note, one row tall).
    Open { rows: usize },
}

/// One row, as painted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowView {
    pub name: String,
    pub username: String,
    /// Which of the three avatar colourways, stable for a name.
    pub colour: usize,
    pub can_copy_username: bool,
    pub can_copy_password: bool,
}

/// Everything the window paints, in one value. The window keeps a copy and
/// paints from it; it decides nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub shape: Shape,
    pub rows: Vec<RowView>,
    pub selected: usize,
    /// The section label (`RECENT`), already upper-cased -- GDI has no
    /// `text-transform`.
    pub label: String,
    /// The empty note's two lines, when there are no rows.
    pub empty: Option<(&'static str, &'static str)>,
    pub footer: Footer,
    /// A copy's confirmation, standing in for the footer for [`TOAST_MS`].
    pub toast: Option<String>,
}

/// The panel's state: what is shown, what is selected, what was typed.
pub struct Popup {
    vault: Vault,
    status: Status,
    query: String,
    shown: Vec<usize>,
    total: usize,
    selected: usize,
    toast: Option<String>,
}

impl Popup {
    pub fn new(vault: Vault, status: Status) -> Self {
        let mut popup = Popup {
            vault,
            status,
            query: String::new(),
            shown: Vec::new(),
            total: 0,
            selected: 0,
            toast: None,
        };
        popup.refresh();
        popup
    }

    fn refresh(&mut self) {
        match &self.vault {
            Vault::Locked => {
                self.shown.clear();
                self.total = 0;
            }
            Vault::Open(listing) => {
                let found = search(listing, &self.query);
                self.shown = found.rows;
                self.total = found.total;
            }
        }
        self.selected = 0;
    }

    pub fn is_locked(&self) -> bool {
        matches!(self.vault, Vault::Locked)
    }

    pub fn shape(&self) -> Shape {
        match self.vault {
            Vault::Locked => Shape::Locked,
            Vault::Open(_) => Shape::Open { rows: self.shown.len() },
        }
    }

    /// The user typed. The selection goes back to the first row.
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.refresh();
    }

    /// Up and down. Stops at the ends rather than wrapping: with four rows a
    /// wrap is a jump the eye does not follow.
    pub fn step(&mut self, down: bool) {
        if self.shown.is_empty() {
            return;
        }
        let last = self.shown.len() - 1;
        self.selected = if down { (self.selected + 1).min(last) } else { self.selected.saturating_sub(1) };
    }

    pub fn select(&mut self, row: usize) {
        if row < self.shown.len() {
            self.selected = row;
        }
    }

    pub fn set_toast(&mut self, toast: Option<String>) {
        self.toast = toast;
    }

    /// The id of the row at `row`, if there is one.
    pub fn id_at(&self, row: usize) -> Option<&str> {
        let Vault::Open(listing) = &self.vault else { return None };
        let index = *self.shown.get(row)?;
        listing.items.get(index).map(|item| item.id.as_str())
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.id_at(self.selected)
    }

    pub fn view(&self) -> View {
        let locked = self.is_locked();
        let mut rows = Vec::new();
        if let Vault::Open(listing) = &self.vault {
            for &index in &self.shown {
                let item = &listing.items[index];
                rows.push(RowView {
                    name: item.name.clone(),
                    username: item.username.clone(),
                    colour: colour_of(&item.name),
                    can_copy_username: !item.username.is_empty(),
                    can_copy_password: item.has_password,
                });
            }
        }
        let empty = (!locked && rows.is_empty()).then(|| {
            if self.query.trim().is_empty() {
                (i18n::t("tray_popup.empty_title"), i18n::t("tray_popup.empty_body"))
            } else {
                (i18n::t("tray_popup.no_matches_title"), i18n::t("tray_popup.no_matches_body"))
            }
        });
        View {
            shape: self.shape(),
            rows,
            selected: self.selected,
            label: label_for(self.query.trim().is_empty(), self.shown.len(), self.total),
            empty,
            footer: footer(locked, &self.status),
            toast: self.toast.clone(),
        }
    }
}

/// The section label: `RECENT`, or `RESULTS`, or -- when the cap hid some --
/// how many of how many. **A list that hides matches without saying so is the
/// defect this project keeps finding**, so the count is the news.
pub fn label_for(no_query: bool, shown: usize, total: usize) -> String {
    if no_query {
        i18n::t("tray_popup.label_recent").to_string()
    } else if total > shown {
        i18n::tf("tray_popup.label_top", &[("shown", &shown.to_string()), ("total", &total.to_string())])
    } else {
        i18n::t("tray_popup.label_results").to_string()
    }
}

/// A stable avatar colourway for a name, so a row keeps its colour as the list
/// changes under a search.
pub fn colour_of(name: &str) -> usize {
    name.bytes().fold(0usize, |sum, b| sum.wrapping_add(b as usize)) % 3
}

// ---------------------------------------------------------------------------
// Layout and hit testing
// ---------------------------------------------------------------------------

/// What can be clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Lock,
    OpenWindow,
    Row(usize),
    CopyUsername(usize),
    CopyPassword(usize),
    Generate,
    NewItem,
    Share,
    Unlock,
}

/// The three tiles, left to right, and their words.
pub const TILES: [(Hit, &str); 3] = [
    (Hit::Generate, crate::tk!("tray_popup.tile_generate")),
    (Hit::NewItem, crate::tk!("tray_popup.tile_new_item")),
    (Hit::Share, crate::tk!("tray_popup.tile_share")),
];

/// What the locked panel says.
pub fn locked_title() -> &'static str {
    i18n::t("tray_popup.locked_title")
}
pub fn locked_body() -> &'static str {
    i18n::t("tray_popup.locked_body")
}
pub fn unlock_label() -> &'static str {
    i18n::t("tray_popup.unlock")
}
pub fn search_placeholder() -> &'static str {
    i18n::t("tray_popup.search")
}

/// One row's boxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowBoxes {
    pub row: Box2,
    pub avatar: Box2,
    pub text: Box2,
    pub copy_username: Box2,
    pub copy_password: Box2,
}

/// Every rectangle the panel paints, in logical pixels from its top left.
///
/// A box a shape does not have is `EMPTY`, and [`hit`] skips empty boxes, so one
/// struct serves both shapes without an `Option` on every field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    pub mark: Box2,
    pub wordmark: Box2,
    pub lock: Box2,
    pub open: Box2,
    pub search: Box2,
    pub search_icon: Box2,
    /// Where the search `EDIT` control sits, inside [`Self::search`].
    pub edit: Box2,
    pub label: Box2,
    pub rows: Vec<RowBoxes>,
    /// The empty note, when the list has no rows.
    pub note: Box2,
    pub tiles: [Box2; 3],
    pub title: Box2,
    pub body: Box2,
    pub unlock: Box2,
    pub footer: Box2,
}

pub const EMPTY: Box2 = Box2 { x: 0, y: 0, w: 0, h: 0 };

/// The panel's geometry for `shape`. Pure, so a box falling off the window is a
/// failing test and not a button nobody can see.
pub fn layout(shape: Shape) -> Layout {
    let inner = WIDTH - 2 * PAD;
    let mut y = PAD;

    // The header: the lockup left, the two icon buttons right.
    let mark_h = crate::panel_draw::MARK_H;
    let mark_w = crate::win32_draw::mark_width(mark_h);
    let header = Box2 { x: PAD, y, w: inner, h: ICON_BUTTON };
    let mark = Box2 { x: PAD, y: y + (ICON_BUTTON - mark_h) / 2, w: mark_w, h: mark_h };
    let wordmark = Box2 {
        x: PAD + mark_w + crate::panel_draw::LOCKUP_GAP,
        y,
        w: crate::panel_draw::WORD_W,
        h: ICON_BUTTON,
    };
    let open = Box2 { x: header.right() - ICON_BUTTON, y, w: ICON_BUTTON, h: ICON_BUTTON };
    let lock = match shape {
        Shape::Locked => EMPTY,
        Shape::Open { .. } => Box2 { x: open.x - ICON_BUTTON_GAP - ICON_BUTTON, ..open },
    };
    y += ICON_BUTTON + GAP;

    let mut l = Layout {
        window: EMPTY,
        mark,
        wordmark,
        lock,
        open,
        search: EMPTY,
        search_icon: EMPTY,
        edit: EMPTY,
        label: EMPTY,
        rows: Vec::new(),
        note: EMPTY,
        tiles: [EMPTY; 3],
        title: EMPTY,
        body: EMPTY,
        unlock: EMPTY,
        footer: EMPTY,
    };

    match shape {
        Shape::Locked => {
            l.title = Box2 { x: PAD, y, w: inner, h: TITLE_H };
            y += TITLE_H + 4;
            l.body = Box2 { x: PAD, y, w: inner, h: BODY_H };
            y += BODY_H + GAP;
            l.unlock = Box2 { x: PAD, y, w: inner, h: UNLOCK_H };
            y += UNLOCK_H + GAP;
        }
        Shape::Open { rows } => {
            l.search = Box2 { x: PAD, y, w: inner, h: SEARCH_H };
            l.search_icon = Box2 {
                x: PAD + SEARCH_PAD_X,
                y: y + (SEARCH_H - SEARCH_ICON) / 2,
                w: SEARCH_ICON,
                h: SEARCH_ICON,
            };
            let edit_x = l.search_icon.right() + SEARCH_ICON_GAP;
            l.edit = Box2 {
                x: edit_x,
                y: y + (SEARCH_H - 22) / 2,
                w: l.search.right() - SEARCH_PAD_X - edit_x,
                h: 22,
            };
            y += SEARCH_H + GAP;

            l.label = Box2 { x: PAD + 4, y, w: inner - 8, h: LABEL_H };
            y += LABEL_H + LABEL_TO_LIST;

            let shown = rows.min(ROWS_MAX);
            for _ in 0..shown {
                let row = Box2 { x: PAD, y, w: inner, h: ROW_H };
                let copy_password = Box2 {
                    x: row.right() - ROW_PAD - ROW_BUTTON,
                    y: y + (ROW_H - ROW_BUTTON) / 2,
                    w: ROW_BUTTON,
                    h: ROW_BUTTON,
                };
                let copy_username =
                    Box2 { x: copy_password.x - ROW_BUTTON_GAP - ROW_BUTTON, ..copy_password };
                let avatar = Box2 { x: row.x + ROW_PAD, y: y + ROW_PAD, w: ROW_AVATAR, h: ROW_AVATAR };
                let text_x = avatar.right() + ROW_TEXT_GAP;
                let text = Box2 { x: text_x, y: avatar.y, w: copy_username.x - 8 - text_x, h: ROW_AVATAR };
                l.rows.push(RowBoxes { row, avatar, text, copy_username, copy_password });
                y += ROW_H + ROW_GAP;
            }
            if shown == 0 {
                l.note = Box2 { x: PAD, y, w: inner, h: ROW_H };
                y += ROW_H + ROW_GAP;
            }
            // The last row's gap is not part of the list.
            y += GAP - ROW_GAP;

            let tile_w = (inner - 2 * TILE_GAP) / 3;
            for (i, tile) in l.tiles.iter_mut().enumerate() {
                *tile = Box2 { x: PAD + i as i32 * (tile_w + TILE_GAP), y, w: tile_w, h: TILE_H };
            }
            y += TILE_H + GAP;
        }
    }

    l.footer = Box2 { x: PAD + 4, y, w: inner - 8, h: FOOTER_H };
    y += FOOTER_H + PAD;
    l.window = Box2 { x: 0, y: 0, w: WIDTH, h: y };
    l
}

fn inside(b: Box2, x: i32, y: i32) -> bool {
    b.w > 0 && b.h > 0 && x >= b.x && x < b.right() && y >= b.y && y < b.bottom()
}

/// **What is under the pointer.** Buttons are tested before the rows they sit
/// on, so a click on a copy button never selects the row instead.
pub fn hit(l: &Layout, x: i32, y: i32) -> Option<Hit> {
    if inside(l.lock, x, y) {
        return Some(Hit::Lock);
    }
    if inside(l.open, x, y) {
        return Some(Hit::OpenWindow);
    }
    if inside(l.unlock, x, y) {
        return Some(Hit::Unlock);
    }
    for (i, row) in l.rows.iter().enumerate() {
        if inside(row.copy_username, x, y) {
            return Some(Hit::CopyUsername(i));
        }
        if inside(row.copy_password, x, y) {
            return Some(Hit::CopyPassword(i));
        }
        if inside(row.row, x, y) {
            return Some(Hit::Row(i));
        }
    }
    for (tile, (what, _)) in l.tiles.iter().zip(TILES) {
        if inside(*tile, x, y) {
            return Some(what);
        }
    }
    None
}

/// **Where the panel opens**: the work area's bottom right, [`EDGE`] in from
/// both sides -- "just above the taskbar" on the common bottom taskbar, and the
/// nearest corner to a tray icon on any other.
pub fn place(work: (i32, i32, i32, i32), w: i32, h: i32) -> (i32, i32) {
    let (left, top, right, bottom) = work;
    ((right - EDGE - w).max(left), (bottom - EDGE - h).max(top))
}

/// **Whether a click on the tray icon opens the panel**, given when the last one
/// closed.
///
/// The panel closes when it loses the foreground, and clicking the tray icon
/// takes the foreground -- so the click that was meant to *close* it arrives a
/// moment after it has closed itself, and would open it again. A panel that can
/// never be dismissed by the icon that opened it is the failure; this is the
/// rule that stops it: a click within [`SAME_CLICK`] of a focus-loss close is
/// that click.
pub fn click_reopens(lost_focus_at: Option<Instant>, now: Instant) -> bool {
    match lost_focus_at {
        Some(at) => now.saturating_duration_since(at) > SAME_CLICK,
        None => true,
    }
}

// ---------------------------------------------------------------------------
// The decision loop
// ---------------------------------------------------------------------------

/// The window handle [`run_with`] deals in. A bare `isize`, for
/// `locked_card::LockedWindow`'s reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PopupWindow(pub isize);

/// What the user did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Esc.
    Cancel,
    /// The window went away underneath the pump.
    Closed,
    /// The panel lost the foreground: the user clicked elsewhere.
    LostFocus,
    /// The search box changed. **A filter the user typed, never a secret** --
    /// and never logged.
    Typed(String),
    Click(Hit),
    /// Up (`false`) or down (`true`).
    Step(bool),
    /// Enter, or a copy chord: copy this field of the selected row.
    Copy(Field),
    /// `Ctrl+L`.
    LockKey,
    /// [`TOAST_MS`] passed.
    ToastOver,
}

/// How the panel ended. Everything but the first two leaves a request for the
/// daemon's loop to carry out once the panel is down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Dismissed,
    /// Dismissed because the foreground moved; see [`click_reopens`].
    LostFocus,
    /// The window could not be put on screen.
    Unavailable,
    /// Open the vault window.
    OpenWindow,
    /// Open the vault window on a new item.
    NewItem,
    /// Open the vault window on Sends.
    Share,
    /// The password generator.
    Generate,
    /// Lock the vault.
    Lock,
    /// The master-password prompt.
    Unlock,
}

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything.
pub struct PopupCalls {
    /// Puts the panel on screen in `shape`. `None` if it could not be.
    pub open: fn(Shape) -> Option<PopupWindow>,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the top-level
    /// window, called before the first `next`: the panel lists the names of the
    /// user's accounts.
    pub protect: fn(PopupWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(PopupWindow) -> Event,
    /// Repaints (and, if the shape changed, resizes) for this view.
    pub show: fn(PopupWindow, &View),
    /// Destroys the window.
    pub close: fn(PopupWindow),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// `copy` is the daemon's: it fetches the one secret at the moment of the copy,
/// behind the master-password re-prompt, and puts it on the clipboard through
/// `crate::clipboard::copy_secret`. It is the only thing here that touches a
/// secret, and it is told an id and a field -- never handed a value.
///
/// `protect` runs before the first `next`; `close` runs on every exit path.
pub fn run_with(
    calls: &PopupCalls,
    mut popup: Popup,
    copy: &mut dyn FnMut(&str, Field) -> CopyResult,
) -> Outcome {
    let Some(window) = (calls.open)(popup.shape()) else {
        log::warn!("the tray popup could not be put on screen");
        return Outcome::Unavailable;
    };
    if !(calls.protect)(window) {
        log::warn!(
            "SetWindowDisplayAffinity was refused for the tray popup; the account names it \
             lists are visible to screen capture on this machine"
        );
    }
    (calls.show)(window, &popup.view());

    let outcome = loop {
        match (calls.next)(window) {
            Event::Cancel | Event::Closed => break Outcome::Dismissed,
            Event::LostFocus => break Outcome::LostFocus,
            Event::Typed(query) => {
                if !popup.is_locked() {
                    popup.set_query(&query);
                    popup.set_toast(None);
                }
            }
            Event::Step(down) => popup.step(down),
            Event::ToastOver => popup.set_toast(None),
            Event::LockKey => {
                if !popup.is_locked() {
                    break Outcome::Lock;
                }
            }
            Event::Copy(field) => {
                // Enter on the locked panel is the one thing it offers.
                if popup.is_locked() {
                    break Outcome::Unlock;
                }
                let selected = popup.selected;
                copy_row(&mut popup, selected, field, copy);
            }
            Event::Click(what) => match what {
                Hit::Lock => break Outcome::Lock,
                Hit::OpenWindow => break Outcome::OpenWindow,
                Hit::Generate => break Outcome::Generate,
                Hit::NewItem => break Outcome::NewItem,
                Hit::Share => break Outcome::Share,
                Hit::Unlock => break Outcome::Unlock,
                Hit::Row(row) => popup.select(row),
                Hit::CopyUsername(row) => copy_row(&mut popup, row, Field::Username, copy),
                Hit::CopyPassword(row) => copy_row(&mut popup, row, Field::Password, copy),
            },
        }
        (calls.show)(window, &popup.view());
    };
    (calls.close)(window);
    outcome
}

fn copy_row(
    popup: &mut Popup,
    row: usize,
    field: Field,
    copy: &mut dyn FnMut(&str, Field) -> CopyResult,
) {
    popup.select(row);
    let Some(id) = popup.id_at(row).map(str::to_string) else { return };
    let result = copy(&id, field);
    popup.set_toast(Some(toast_for(field, result).to_string()));
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// Shows the panel for `vault` and answers how it ended.
///
/// The one production door; `crate::app` and `main` go through it.
pub fn show(
    vault: Vault,
    status: Status,
    copy: &mut dyn FnMut(&str, Field) -> CopyResult,
) -> Outcome {
    run_with(&win32::REAL, Popup::new(vault, status), copy)
}

/// The shipped window calls, for the preview example: it swaps `protect` and
/// `next` and nothing else (the same shape as every other card's `REAL`).
pub fn real_calls() -> &'static PopupCalls {
    &win32::REAL
}

mod win32 {
    use super::{
        hit, layout, place, Event, Field, Hit, Layout, PopupCalls, PopupWindow, Shape, Tone, View,
        ROW_RADIUS, search_placeholder, TILES, TILE_RADIUS, TOAST_MS, TRAY_POPUP_TITLE,
    };
    use crate::prompt_card::Box2;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use eframe::egui::Color32;
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        Arc, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontIndirectW,
        CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, Ellipse, EndPaint, FillRect, GetDC,
        GetDeviceCaps, GetStockObject, InvalidateRect, Polyline, ReleaseDC,
        SelectObject, SetBkColor, SetBkMode, SetTextCharacterExtra, SetTextColor,
        CLEARTYPE_QUALITY, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
        DT_VCENTER, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, LOGFONTW, LOGPIXELSX, NULL_BRUSH,
        PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL, VK_DOWN,
        VK_ESCAPE, VK_RETURN, VK_UP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetWindowTextW, IsChild, KillTimer, LoadCursorW,
        MoveWindow, PeekMessageW, RegisterClassW, SendMessageW, SetForegroundWindow, SetTimer,
        SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
        CS_HREDRAW, CS_VREDRAW, EN_CHANGE, ES_AUTOHSCROLL,
        GWLP_WNDPROC, HMENU, IDC_ARROW, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE,
        SW_SHOW, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_COMMAND,
        WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MOUSEMOVE,
        WM_PAINT, WM_QUIT, WM_SETFONT, WM_TIMER, WNDCLASSW, WS_CHILD, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    };

    use crate::win32_draw::{draw_button, draw_text, rgb};

    const CLASS_NAME: PCWSTR = w!("DeskwardenTrayPopup");
    const ID_EDIT: usize = 201;
    const TIMER_TOAST: usize = 1;
    /// `EM_GETSEL` and `EM_SETLIMITTEXT`, which the `windows` crate files under a
    /// feature this crate does not enable.
    const EM_GETSEL: u32 = 0x00B0;
    const EM_SETLIMITTEXT: u32 = 0x00C5;

    pub(super) static REAL: PopupCalls =
        PopupCalls { open, protect, next, show: show_view, close };

    /// The window's DPI as a percentage of 96, sampled once per open.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);
    static GONE: AtomicBool = AtomicBool::new(false);
    static PENDING: Mutex<Option<Event>> = Mutex::new(None);
    static VIEW: Mutex<Option<View>> = Mutex::new(None);
    static HOVER: Mutex<Option<Hit>> = Mutex::new(None);
    static TRACKING: AtomicBool = AtomicBool::new(false);
    static SHAPE: Mutex<Option<Shape>> = Mutex::new(None);
    static EDIT: AtomicIsize = AtomicIsize::new(0);
    static EDIT_BRUSH: AtomicIsize = AtomicIsize::new(0);
    static EDIT_ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut c_void)
    }

    fn set_pending(event: Event) {
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(event);
        }
    }

    fn take_pending() -> Option<Event> {
        PENDING.lock().ok().and_then(|mut slot| slot.take())
    }

    // ---- fonts ------------------------------------------------------------

    fn font(family: &str, px: i32) -> HFONT {
        let (face, weight) = crate::theme::gdi_face_for(family);
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -scale(px),
                lfWeight: if weight >= 700 { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            for (i, ch) in face.encode_utf16().take(31).enumerate() {
                lf.lfFaceName[i] = ch;
            }
            CreateFontIndirectW(&lf)
        }
    }

    /// Every face the panel paints with, created at open and destroyed at close.
    struct Fonts {
        brand: HFONT,
        name: HFONT,
        sub: HFONT,
        avatar: HFONT,
        label: HFONT,
        tile: HFONT,
        footer: HFONT,
        title: HFONT,
        body: HFONT,
        button: HFONT,
        edit: HFONT,
    }

    // `Fonts` holds raw GDI handles, which are process-wide rather than
    // thread-owned; the panel is modal on one thread.
    unsafe impl std::marker::Send for Fonts {}

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, EXTRABOLD, REGULAR};
            Fonts {
                brand: font(EXTRABOLD, 16),
                name: font(BOLD, 15),
                sub: font(REGULAR, 13),
                avatar: font(BOLD, 13),
                label: font(EXTRABOLD, 12),
                tile: font(BOLD, 13),
                footer: font(REGULAR, 13),
                title: font(EXTRABOLD, crate::panel_draw::TITLE_PX),
                body: font(REGULAR, crate::panel_draw::BODY_PX),
                button: font(BOLD, 16),
                edit: font(REGULAR, 15),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [
                    self.brand, self.name, self.sub, self.avatar, self.label, self.tile,
                    self.footer, self.title, self.body, self.button, self.edit,
                ] {
                    let _ = DeleteObject(f);
                }
            }
        }
    }

    static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);

    // ---- open / protect / close -------------------------------------------

    fn work_area() -> (i32, i32, i32, i32) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{
                SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
            };
            let mut area = RECT::default();
            let ok = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut area as *mut _ as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            if ok.is_err() || area.right <= area.left || area.bottom <= area.top {
                return (0, 0, 1920, 1080);
            }
            (area.left, area.top, area.right, area.bottom)
        }
    }

    fn register_class() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                lpszClassName: CLASS_NAME,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
    }

    /// Hands the card's corners to [`crate::card_shell`]: the design's 18 px
    /// radius, antialiased, with its soft shadow, in place of the fixed ~8 px
    /// corner `DWMWCP_ROUND` gave every card. The card's own background is
    /// `theme::CARD`, which is what the shell paints under it.
    fn round_corners(window: HWND) {
        // The theme is chosen here, on the window's own thread, before the
        // first paint: every painter below reads it.
        crate::gdi_palette::open_for_card();
        crate::card_shell::attach(window, crate::gdi_palette::card());
    }

    fn dev(b: Box2) -> RECT {
        RECT { left: scale(b.x), top: scale(b.y), right: scale(b.right()), bottom: scale(b.bottom()) }
    }

    fn open(shape: Shape) -> Option<PopupWindow> {
        crate::win32_draw::register_fonts();
        GONE.store(false, Ordering::SeqCst);
        TRACKING.store(false, Ordering::SeqCst);
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = HOVER.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = VIEW.lock() {
            *slot = None;
        }
        unsafe {
            let dc = GetDC(None);
            let dpi = GetDeviceCaps(dc, LOGPIXELSX);
            ReleaseDC(None, dc);
            DPI_PERCENT.store(if dpi > 0 { dpi * 100 / 96 } else { 100 }, Ordering::SeqCst);
        }
        register_class();
        {
            let mut slot = FONTS.lock().ok()?;
            if let Some(previous) = slot.take() {
                previous.destroy();
            }
            *slot = Some(Fonts::build());
        }

        let l = layout(shape);
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        let (x, y) = place(work_area(), w, h);
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                CLASS_NAME,
                &HSTRING::from(TRAY_POPUP_TITLE),
                WS_POPUP | WS_VISIBLE,
                x,
                y,
                w,
                h,
                None,
                None,
                None,
                None,
            )
        }
        .ok();
        let Some(window) = window else {
            release();
            return None;
        };
        round_corners(window);
        if let Ok(mut slot) = SHAPE.lock() {
            *slot = Some(shape);
        }

        // The search box: a system EDIT, borderless, on the search field's own
        // ground (see `WM_CTLCOLOREDIT`). Present in both shapes and hidden in
        // the locked one, so a shape change is a show/hide and a move.
        let edit_font = FONTS.lock().ok().and_then(|g| g.as_ref().map(|f| f.edit));
        let edit = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                w!(""),
                WINDOW_STYLE(WS_CHILD.0 | ES_AUTOHSCROLL as u32),
                scale(l.edit.x),
                scale(l.edit.y),
                scale(l.edit.w),
                scale(l.edit.h),
                window,
                HMENU(ID_EDIT as *mut c_void),
                None,
                None,
            )
        }
        .ok();
        let Some(edit) = edit else {
            unsafe {
                let _ = DestroyWindow(window);
            }
            release();
            return None;
        };
        unsafe {
            if let Some(f) = edit_font {
                SendMessageW(edit, WM_SETFONT, WPARAM(f.0 as usize), LPARAM(1));
            }
            SendMessageW(edit, EM_SETLIMITTEXT, WPARAM(120), LPARAM(0));
            let previous = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize);
            if previous != 0 {
                EDIT_ORIGINAL_PROC.store(previous, Ordering::SeqCst);
            }
            let brush = CreateSolidBrush(rgb(crate::gdi_palette::field_bg()));
            EDIT_BRUSH.store(brush.0 as isize, Ordering::SeqCst);
        }
        EDIT.store(edit.0 as isize, Ordering::SeqCst);

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            if matches!(shape, Shape::Open { .. }) {
                let _ = ShowWindow(edit, SW_SHOW);
            }
            // Allowed to refuse, and handled rather than asserted: a panel that
            // never gets the foreground cannot hear Esc, and the next click
            // elsewhere cannot deactivate a window that was never active -- so
            // the native menu and the icon's second click stay the way out.
            let _ = SetForegroundWindow(window);
            if matches!(shape, Shape::Open { .. }) {
                let _ = SetFocus(edit);
            }
        }
        Some(PopupWindow(window.0 as isize))
    }

    /// Frees what `open` made before a window existed to own it.
    fn release() {
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
    }

    fn protect(window: PopupWindow) -> bool {
        if !crate::screen_share::capture_protection_enabled() {
            return true;
        }
        unsafe { SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    fn close(window: PopupWindow) {
        unsafe {
            let _ = KillTimer(hwnd(window.0), TIMER_TOAST);
            let _ = DestroyWindow(hwnd(window.0));
            let brush = EDIT_BRUSH.swap(0, Ordering::SeqCst);
            if brush != 0 {
                let _ = DeleteObject(HBRUSH(brush as *mut c_void));
            }
        }
        EDIT.store(0, Ordering::SeqCst);
        release();
        // What was typed is a filter over the user's own item names; nothing
        // needs it once the panel is down.
        if let Ok(mut slot) = VIEW.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
    }

    // ---- show: resize, repaint, arm the toast ----------------------------

    fn show_view(window: PopupWindow, view: &View) {
        let top = hwnd(window.0);
        let previous = SHAPE.lock().ok().and_then(|mut slot| slot.replace(view.shape));
        if let Ok(mut slot) = VIEW.lock() {
            *slot = Some(view.clone());
        }
        let l = layout(view.shape);
        if previous != Some(view.shape) {
            let (w, h) = (scale(l.window.w), scale(l.window.h));
            let (x, y) = place(work_area(), w, h);
            unsafe {
                let _ = SetWindowPos(top, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        let edit = hwnd(EDIT.load(Ordering::SeqCst));
        unsafe {
            if matches!(view.shape, Shape::Open { .. }) {
                let _ = MoveWindow(
                    edit,
                    scale(l.edit.x),
                    scale(l.edit.y),
                    scale(l.edit.w),
                    scale(l.edit.h),
                    true,
                );
                let _ = ShowWindow(edit, SW_SHOW);
            } else {
                let _ = ShowWindow(edit, SW_HIDE);
            }
            if view.toast.is_some() {
                SetTimer(top, TIMER_TOAST, TOAST_MS, None);
            } else {
                let _ = KillTimer(top, TIMER_TOAST);
            }
            let _ = InvalidateRect(top, None, false);
        }
    }

    // ---- the pump ----------------------------------------------------------

    /// Whether the search box has a selection -- `Ctrl+C` means *copy that text*
    /// then, and only otherwise *copy the password*.
    fn edit_has_selection(edit: HWND) -> bool {
        let (mut start, mut end) = (0u32, 0u32);
        unsafe {
            SendMessageW(
                edit,
                EM_GETSEL,
                WPARAM(&mut start as *mut u32 as usize),
                LPARAM(&mut end as *mut u32 as isize),
            );
        }
        start != end
    }

    /// Pumps until the user does something. Keys are read here, ahead of
    /// dispatch, so Up, Down and Enter reach the panel from the search box's
    /// focus without a second window procedure to keep in step.
    fn next(window: PopupWindow) -> Event {
        let top = hwnd(window.0);
        loop {
            if GONE.load(Ordering::SeqCst) {
                return Event::Closed;
            }
            if let Some(event) = take_pending() {
                return event;
            }
            let mut msg = MSG::default();
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        GONE.store(true, Ordering::SeqCst);
                        return Event::Closed;
                    }
                    let ours = msg.hwnd == top || IsChild(top, msg.hwnd).as_bool();
                    if ours && msg.message == WM_KEYDOWN {
                        let key = msg.wParam.0 as u16;
                        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                        let event = if key == VK_ESCAPE.0 {
                            Some(Event::Cancel)
                        } else if key == VK_UP.0 {
                            Some(Event::Step(false))
                        } else if key == VK_DOWN.0 {
                            Some(Event::Step(true))
                        } else if key == VK_RETURN.0 {
                            Some(Event::Copy(Field::Password))
                        } else if ctrl && key == u16::from(b'B') {
                            Some(Event::Copy(Field::Username))
                        } else if ctrl && key == u16::from(b'L') {
                            Some(Event::LockKey)
                        } else if ctrl
                            && key == u16::from(b'C')
                            && !edit_has_selection(hwnd(EDIT.load(Ordering::SeqCst)))
                        {
                            Some(Event::Copy(Field::Password))
                        } else {
                            None
                        };
                        if let Some(event) = event {
                            return event;
                        }
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                    if GONE.load(Ordering::SeqCst) {
                        return Event::Closed;
                    }
                    if let Some(event) = take_pending() {
                        return event;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    // ---- window procedures -------------------------------------------------

    fn current_layout() -> Option<Layout> {
        let shape = SHAPE.lock().ok().and_then(|slot| *slot)?;
        Some(layout(shape))
    }

    fn pointer_hit(lparam: LPARAM) -> Option<Hit> {
        let x = (lparam.0 & 0xffff) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
        let pct = DPI_PERCENT.load(Ordering::SeqCst).max(1);
        hit(&current_layout()?, x * 100 / pct, y * 100 / pct)
    }

    unsafe extern "system" fn wnd_proc(
        window: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(window);
                LRESULT(0)
            }
            WM_ACTIVATE => {
                // WA_INACTIVE: another window took the foreground.
                if (wparam.0 & 0xffff) == 0 {
                    set_pending(Event::LostFocus);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if !TRACKING.swap(true, Ordering::SeqCst) {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: window,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut track);
                }
                let now = pointer_hit(lparam);
                if let Ok(mut slot) = HOVER.lock() {
                    if *slot != now {
                        *slot = now;
                        let _ = InvalidateRect(window, None, false);
                    }
                }
                LRESULT(0)
            }
            m if m == crate::panel_draw::WM_MOUSELEAVE => {
                TRACKING.store(false, Ordering::SeqCst);
                if let Ok(mut slot) = HOVER.lock() {
                    if slot.take().is_some() {
                        let _ = InvalidateRect(window, None, false);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if let Some(what) = pointer_hit(lparam) {
                    set_pending(Event::Click(what));
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == TIMER_TOAST {
                    let _ = KillTimer(window, TIMER_TOAST);
                    set_pending(Event::ToastOver);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = wparam.0 & 0xffff;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                if id == ID_EDIT && notification == EN_CHANGE {
                    let mut buffer = [0u16; 160];
                    let len = GetWindowTextW(hwnd(EDIT.load(Ordering::SeqCst)), &mut buffer);
                    let len = usize::try_from(len).unwrap_or(0).min(buffer.len());
                    set_pending(Event::Typed(String::from_utf16_lossy(&buffer[..len])));
                }
                LRESULT(0)
            }
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wparam.0 as *mut c_void);
                SetTextColor(hdc, rgb(crate::gdi_palette::ink()));
                SetBkColor(hdc, rgb(crate::gdi_palette::field_bg()));
                LRESULT(EDIT_BRUSH.load(Ordering::SeqCst))
            }
            WM_DESTROY => {
                // **NO `PostQuitMessage` HERE, EVER** -- `locked_card`'s
                // WM_DESTROY says why at length: this window lives on the
                // daemon's own thread, and a stale WM_QUIT breaks the next
                // `eframe` window that thread opens.
                GONE.store(true, Ordering::SeqCst);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }

    /// The search box's procedure: the system's, plus the placeholder. An EDIT
    /// paints over its whole client, so a placeholder cannot be drawn beneath it
    /// -- it is drawn over the empty control after the control has painted.
    unsafe extern "system" fn edit_proc(
        edit: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let original = EDIT_ORIGINAL_PROC.load(Ordering::SeqCst);
        if original == 0 {
            return DefWindowProcW(edit, msg, wparam, lparam);
        }
        let result = CallWindowProcW(
            Some(std::mem::transmute::<
                isize,
                unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
            >(original)),
            edit,
            msg,
            wparam,
            lparam,
        );
        if msg == WM_PAINT {
            use windows::Win32::UI::WindowsAndMessaging::GetWindowTextLengthW;
            if GetWindowTextLengthW(edit) == 0 {
                let hdc = GetDC(edit);
                let mut rc = RECT::default();
                let _ = GetClientRect(edit, &mut rc);
                let font = FONTS.lock().ok().and_then(|g| g.as_ref().map(|f| f.edit));
                if let Some(font) = font {
                    let old = SelectObject(hdc, font);
                    SetBkMode(hdc, TRANSPARENT);
                    SetTextColor(hdc, rgb(crate::gdi_palette::text_ghost()));
                    draw_text(
                        hdc,
                        search_placeholder(),
                        &mut rc,
                        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                    );
                    SelectObject(hdc, old);
                }
                ReleaseDC(edit, hdc);
            }
        }
        result
    }

    // ---- painting ----------------------------------------------------------

    fn fill_rect(hdc: HDC, rc: RECT, colour: Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(colour));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
        }
    }

    fn round(hdc: HDC, at: Box2, radius: i32, fill: Color32) {
        crate::panel_draw::round_rect(hdc, dev(at), scale(radius), fill, None);
    }

    fn text(hdc: HDC, font: HFONT, at: Box2, run: &str, ink: Color32, format: u32) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, rgb(ink));
            let mut rc = dev(at);
            draw_text(
                hdc,
                run,
                &mut rc,
                windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(
                    format | DT_SINGLELINE.0 | DT_VCENTER.0 | DT_NOPREFIX.0,
                ),
            );
            SelectObject(hdc, old);
        }
    }

    /// A glyph of the design's icon set, drawn as strokes in a square `at`.
    #[derive(Clone, Copy)]
    enum Glyph {
        Padlock,
        External,
        Magnifier,
        Person,
        Plus,
        Plane,
        Regenerate,
    }

    fn glyph(hdc: HDC, at: Box2, which: Glyph, ink: Color32) {
        let rc = dev(at);
        let (l, t, r, b) = (rc.left, rc.top, rc.right, rc.bottom);
        let (w, h) = (r - l, b - t);
        let s = scale(100);
        unsafe {
            if let Glyph::Regenerate = which {
                crate::panel_draw::draw_regenerate_icon(hdc, rc, ink, s);
                return;
            }
            let pen = CreatePen(PS_SOLID, (2 * s / 100).max(1), rgb(ink));
            let old_pen = SelectObject(hdc, pen);
            let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
            let line = |points: &[(i32, i32)]| {
                let pts: Vec<windows::Win32::Foundation::POINT> = points
                    .iter()
                    .map(|&(x, y)| windows::Win32::Foundation::POINT { x: l + x * w / 24, y: t + y * h / 24 })
                    .collect();
                let _ = Polyline(hdc, &pts);
            };
            match which {
                Glyph::Padlock => {
                    // M5 11h14v10H5z  M8 11V7a4 4 0 0 1 8 0v4
                    line(&[(5, 11), (19, 11), (19, 21), (5, 21), (5, 11)]);
                    line(&[(8, 11), (8, 7), (9, 4), (12, 3), (15, 4), (16, 7), (16, 11)]);
                }
                Glyph::External => {
                    line(&[(15, 3), (21, 3), (21, 9)]);
                    line(&[(10, 14), (21, 3)]);
                    line(&[(18, 13), (18, 19), (16, 21), (5, 21), (3, 19), (3, 8), (5, 6), (11, 6)]);
                }
                Glyph::Magnifier => {
                    let _ = Ellipse(hdc, l + 2 * w / 24, t + 2 * h / 24, l + 18 * w / 24, t + 18 * h / 24);
                    line(&[(16, 16), (22, 22)]);
                }
                Glyph::Person => {
                    let _ = Ellipse(hdc, l + 7 * w / 24, t + 3 * h / 24, l + 17 * w / 24, t + 13 * h / 24);
                    let _ = Arc(
                        hdc,
                        l + 4 * w / 24,
                        t + 13 * h / 24,
                        l + 20 * w / 24,
                        t + 29 * h / 24,
                        l + 20 * w / 24,
                        t + 21 * h / 24,
                        l + 4 * w / 24,
                        t + 21 * h / 24,
                    );
                }
                Glyph::Plus => {
                    line(&[(12, 5), (12, 19)]);
                    line(&[(5, 12), (19, 12)]);
                }
                Glyph::Plane => {
                    // M22 2 11 13 M22 2l-7 20-4-9-9-4z
                    line(&[(22, 2), (11, 13)]);
                    line(&[(22, 2), (15, 22), (11, 13), (2, 9), (22, 2)]);
                }
                Glyph::Regenerate => {}
            }
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(pen);
        }
    }

    fn paint(window: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(window, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(window, &mut client);
            let (cw, ch) = (client.right, client.bottom);
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, cw, ch);
            let old = SelectObject(mem, bmp);

            let view = VIEW.lock().ok().and_then(|slot| slot.clone());
            let hover = HOVER.lock().ok().and_then(|slot| *slot);
            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());
            fill_rect(mem, client, crate::gdi_palette::card());
            SetBkMode(mem, TRANSPARENT);
            if let (Some(view), Some(fonts)) = (view, fonts) {
                paint_panel(mem, &view, fonts, hover);
            }
            drop(guard);

            let _ = BitBlt(hdc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(window, &ps);
        }
    }

    fn paint_panel(hdc: HDC, view: &View, fonts: &Fonts, hover: Option<Hit>) {
        let l = layout(view.shape);

        // Header.
        crate::panel_draw::draw_lockup(hdc, dev(l.mark), dev(l.wordmark), fonts.brand);
        for (button, which, what) in
            [(l.lock, Glyph::Padlock, Hit::Lock), (l.open, Glyph::External, Hit::OpenWindow)]
        {
            if button.w == 0 {
                continue;
            }
            if hover == Some(what) {
                round(hdc, button, 11, crate::gdi_palette::field_bg());
            }
            let icon = Box2 { x: button.x + 9, y: button.y + 9, w: 18, h: 18 };
            glyph(hdc, icon, which, crate::gdi_palette::text_nav());
        }

        match view.shape {
            Shape::Locked => {
                text(hdc, fonts.title, l.title, super::locked_title(), crate::gdi_palette::ink(), DT_LEFT.0);
                text(hdc, fonts.body, l.body, super::locked_body(), crate::gdi_palette::text_faint(), DT_LEFT.0);
                let skin = crate::panel_draw::skin(
                    crate::panel_draw::PanelButton::Deep,
                    hover == Some(Hit::Unlock),
                );
                draw_button(
                    hdc,
                    dev(l.unlock),
                    super::unlock_label(),
                    fonts.button,
                    skin,
                    scale(crate::panel_draw::TALL_BUTTON_RADIUS),
                );
            }
            Shape::Open { .. } => {
                round(hdc, l.search, super::SEARCH_RADIUS, crate::gdi_palette::field_bg());
                glyph(hdc, l.search_icon, Glyph::Magnifier, crate::gdi_palette::text_ghost());

                unsafe {
                    SetTextCharacterExtra(hdc, scale(1));
                }
                text(hdc, fonts.label, l.label, &view.label, crate::gdi_palette::text_ghost(), DT_LEFT.0);
                unsafe {
                    SetTextCharacterExtra(hdc, 0);
                }

                for (i, (boxes, row)) in l.rows.iter().zip(&view.rows).enumerate() {
                    let on_row = matches!(
                        hover,
                        Some(Hit::Row(n) | Hit::CopyUsername(n) | Hit::CopyPassword(n)) if n == i
                    );
                    if i == view.selected {
                        round(hdc, boxes.row, ROW_RADIUS, crate::gdi_palette::blue_wash());
                    } else if on_row {
                        round(hdc, boxes.row, ROW_RADIUS, crate::gdi_palette::secondary_hover());
                    }
                    crate::panel_draw::draw_avatar(
                        hdc,
                        dev(boxes.avatar),
                        &crate::theme::initials(&row.name),
                        fonts.avatar,
                        crate::panel_draw::avatar_colours(row.colour),
                        scale(100),
                    );
                    let half = boxes.text.h / 2 + 1;
                    let ell = DT_LEFT.0 | DT_END_ELLIPSIS.0;
                    text(
                        hdc,
                        fonts.name,
                        Box2 { h: half, ..boxes.text },
                        &row.name,
                        crate::gdi_palette::ink(),
                        ell,
                    );
                    text(
                        hdc,
                        fonts.sub,
                        Box2 { y: boxes.text.y + half - 1, h: boxes.text.h - half + 1, ..boxes.text },
                        if row.username.is_empty() { "No username" } else { &row.username },
                        crate::gdi_palette::text_faint(),
                        ell,
                    );
                    for (button, which, what, enabled) in [
                        (
                            boxes.copy_username,
                            Glyph::Person,
                            Hit::CopyUsername(i),
                            row.can_copy_username,
                        ),
                        (
                            boxes.copy_password,
                            Glyph::Padlock,
                            Hit::CopyPassword(i),
                            row.can_copy_password,
                        ),
                    ] {
                        let hot = enabled && hover == Some(what);
                        if hot {
                            round(hdc, button, 10, crate::gdi_palette::blue_wash());
                        }
                        let icon = Box2 { x: button.x + 9, y: button.y + 9, w: 18, h: 18 };
                        let ink = if !enabled {
                            crate::gdi_palette::kbd_border()
                        } else if hot {
                            crate::gdi_palette::blue()
                        } else {
                            crate::gdi_palette::text_muted()
                        };
                        glyph(hdc, icon, which, ink);
                    }
                }
                if let Some((title, body)) = view.empty {
                    let half = l.note.h / 2;
                    text(hdc, fonts.name, Box2 { h: half, ..l.note }, title, crate::gdi_palette::ink(), DT_CENTER.0);
                    text(
                        hdc,
                        fonts.sub,
                        Box2 { y: l.note.y + half, h: l.note.h - half, ..l.note },
                        body,
                        crate::gdi_palette::text_faint(),
                        DT_CENTER.0,
                    );
                }

                for (tile, (what, label)) in l.tiles.iter().zip(TILES) {
                    let hot = hover == Some(what);
                    round(hdc, *tile, TILE_RADIUS, if hot { crate::gdi_palette::blue_wash() } else { crate::gdi_palette::secondary_hover() });
                    let ink = if hot { crate::gdi_palette::blue_deep() } else { crate::gdi_palette::ink() };
                    let which = match what {
                        Hit::Generate => Glyph::Regenerate,
                        Hit::NewItem => Glyph::Plus,
                        _ => Glyph::Plane,
                    };
                    let icon = Box2 { x: tile.x + (tile.w - 20) / 2, y: tile.y + 14, w: 20, h: 20 };
                    glyph(hdc, icon, which, ink);
                    text(
                        hdc,
                        fonts.tile,
                        Box2 { x: tile.x, y: tile.y + 40, w: tile.w, h: 22 },
                        crate::i18n::t(label),
                        ink,
                        DT_CENTER.0,
                    );
                }
            }
        }

        // Footer: a toast stands in for it while a copy's confirmation is up.
        let (words, tone, ink) = match &view.toast {
            Some(toast) => (toast.as_str(), Tone::Good, crate::gdi_palette::ink()),
            None => (view.footer.text.as_str(), view.footer.tone, crate::gdi_palette::text_muted()),
        };
        let dot_colour = match tone {
            Tone::Good => crate::gdi_palette::done_mark(),
            Tone::Warn => crate::gdi_palette::caution_dot(),
            Tone::Quiet => crate::gdi_palette::text_ghost(),
        };
        let dot = Box2 {
            x: l.footer.x,
            y: l.footer.y + (l.footer.h - super::DOT) / 2,
            w: super::DOT,
            h: super::DOT,
        };
        let d = dev(dot);
        crate::win32_draw::aa_round_rect(
            hdc,
            d,
            (d.right - d.left).min(d.bottom - d.top) / 2,
            rgb(dot_colour),
            None,
        );
        let text_x = dot.right() + super::DOT_GAP;
        text(
            hdc,
            fonts.footer,
            Box2 { x: text_x, w: l.footer.right() - text_x, ..l.footer },
            words,
            ink,
            DT_LEFT.0 | DT_END_ELLIPSIS.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn item(id: &str, name: &str, username: &str, has_password: bool) -> Item {
        Item { id: id.into(), name: name.into(), username: username.into(), has_password }
    }

    fn listing(items: Vec<Item>, counts: &[(&str, u32)]) -> Listing {
        Listing {
            items,
            fill_counts: counts.iter().map(|(id, n)| (id.to_string(), *n)).collect(),
        }
    }

    fn six() -> Listing {
        listing(
            vec![
                item("1", "SAP Production", "a.novak", true),
                item("2", "Ledgerline", "a.novak@ledgerline.com", true),
                item("3", "Atlas Studio", "a.novak@studio.atlas.com", true),
                item("4", "Vantage VPN", "a.novak@vantage.io", true),
                item("5", "Harbor Bank", "anna", true),
                item("6", "Notes only", "", false),
            ],
            &[],
        )
    }

    const SYNCED: Status = Status { sync: Sync::Synced, auto_lock_minutes: Some(15) };

    fn names(l: &Listing, rows: &[usize]) -> Vec<String> {
        rows.iter().map(|&i| l.items[i].name.clone()).collect()
    }

    // ---- the footer ---------------------------------------------------------

    #[test]
    fn the_footer_says_what_design_1d_says() {
        assert_eq!(
            footer(false, &SYNCED),
            Footer { text: "Synced \u{b7} locks after 15 min idle".into(), tone: Tone::Good }
        );
        assert_eq!(
            footer(false, &Status { sync: Sync::Failed, auto_lock_minutes: Some(15) }),
            Footer { text: "Offline \u{b7} filling still works".into(), tone: Tone::Warn }
        );
    }

    #[test]
    fn the_footer_reports_a_sync_in_flight_a_single_minute_and_no_auto_lock() {
        let working = footer(false, &Status { sync: Sync::Syncing, auto_lock_minutes: Some(15) });
        assert_eq!(working.text, "Syncing...");
        assert_eq!(working.tone, Tone::Quiet);
        let one = footer(false, &Status { sync: Sync::Synced, auto_lock_minutes: Some(1) });
        assert!(one.text.ends_with("1 min idle"), "{one:?}");
        let never = footer(false, &Status { sync: Sync::Synced, auto_lock_minutes: None });
        assert_eq!(never.text, "Synced", "no auto-lock, so no claim about one");
    }

    #[test]
    fn the_footer_minutes_come_from_the_auto_lock_setting() {
        use crate::settings::AutoLock;
        assert_eq!(auto_lock_minutes(AutoLock::Never), None);
        assert_eq!(auto_lock_minutes(AutoLock::After(Duration::from_secs(15 * 60))), Some(15));
        assert_eq!(auto_lock_minutes(AutoLock::After(Duration::from_secs(61))), Some(2), "rounded up");
        assert_eq!(auto_lock_minutes(AutoLock::After(Duration::from_secs(5))), Some(1), "never zero");
    }

    #[test]
    fn a_locked_footer_never_says_synced() {
        let locked = footer(true, &SYNCED);
        assert!(!locked.text.contains("Synced"), "{locked:?}");
        assert_eq!(locked.tone, Tone::Quiet);
    }

    #[test]
    fn every_copy_result_has_a_short_line_and_none_is_empty() {
        for field in [Field::Username, Field::Password] {
            for result in [
                CopyResult::Copied,
                CopyResult::Empty,
                CopyResult::Cancelled,
                CopyResult::NeedsHello,
                CopyResult::Gone,
            ] {
                let line = toast_for(field, result);
                assert!(!line.is_empty());
                assert!(line.len() <= 52, "{line:?} will not fit the footer's one line");
            }
        }
        assert_eq!(toast_for(Field::Username, CopyResult::Copied), "Username copied");
        assert_eq!(toast_for(Field::Password, CopyResult::Copied), "Password copied");
    }

    #[test]
    fn a_refusal_never_claims_a_copy_that_did_not_happen() {
        for result in [CopyResult::Cancelled, CopyResult::NeedsHello, CopyResult::Gone, CopyResult::Empty]
        {
            for field in [Field::Username, Field::Password] {
                let line = toast_for(field, result);
                assert!(
                    !line.contains("copied") || line.contains("Nothing was copied"),
                    "{field:?}/{result:?}: {line:?} claims a copy that did not happen"
                );
            }
        }
    }

    // ---- recent and search --------------------------------------------------

    #[test]
    fn with_no_use_recorded_recent_is_the_first_four_logins_in_vault_order() {
        let l = six();
        assert_eq!(
            names(&l, &recent(&l)),
            ["SAP Production", "Ledgerline", "Atlas Studio", "Vantage VPN"]
        );
    }

    #[test]
    fn recent_puts_the_most_filled_first_and_vault_order_breaks_ties() {
        let mut l = six();
        l.fill_counts.insert("5".into(), 9);
        l.fill_counts.insert("3".into(), 9);
        l.fill_counts.insert("2".into(), 1);
        assert_eq!(
            names(&l, &recent(&l)),
            ["Atlas Studio", "Harbor Bank", "Ledgerline", "SAP Production"]
        );
    }

    #[test]
    fn a_login_with_nothing_to_copy_is_never_offered() {
        let l = six();
        assert!(!names(&l, &recent(&l)).contains(&"Notes only".to_string()));
        assert_eq!(search(&l, "notes").total, 0);
        let anonymous = listing(vec![item("", "No id", "x", true)], &[]);
        assert!(recent(&anonymous).is_empty(), "an item with no id has nothing to fill from later");
    }

    #[test]
    fn search_matches_names_and_usernames_without_regard_to_case() {
        let l = six();
        assert_eq!(names(&l, &search(&l, "LEDGER").rows), ["Ledgerline"]);
        assert_eq!(names(&l, &search(&l, "  vantage.io ").rows), ["Vantage VPN"]);
        assert_eq!(search(&l, "zzz").total, 0);
    }

    #[test]
    fn search_counts_every_match_and_shows_at_most_the_cap() {
        let l = six();
        let found = search(&l, "a.novak");
        assert_eq!(found.total, 4);
        assert_eq!(found.rows.len(), ROWS_MAX);
        let many = listing(
            (0..9).map(|n| item(&n.to_string(), &format!("Site {n}"), "u", true)).collect(),
            &[],
        );
        let found = search(&many, "site");
        assert_eq!(found.total, 9, "the count is taken before the cap");
        assert_eq!(found.rows.len(), ROWS_MAX);
    }

    #[test]
    fn an_empty_search_is_recent() {
        let l = six();
        assert_eq!(search(&l, "").rows, recent(&l));
        assert_eq!(search(&l, "   ").rows, recent(&l));
    }

    #[test]
    fn the_search_uses_the_vault_windows_own_predicate() {
        assert!(production().contains(concat!("picker_ui::name_matches", "_filter(")));
    }

    #[test]
    fn the_label_names_the_cap_when_the_cap_hid_matches() {
        assert_eq!(label_for(true, 4, 4), "RECENT");
        assert_eq!(label_for(false, 2, 2), "RESULTS");
        assert_eq!(label_for(false, 4, 12), "TOP 4 OF 12 MATCHES");
    }

    // ---- the state ----------------------------------------------------------

    #[test]
    fn the_selection_stops_at_the_ends_and_a_search_resets_it() {
        let mut p = Popup::new(Vault::Open(six()), SYNCED);
        p.step(false);
        assert_eq!(p.view().selected, 0);
        for _ in 0..10 {
            p.step(true);
        }
        assert_eq!(p.view().selected, 3);
        p.set_query("a");
        assert_eq!(p.view().selected, 0);
    }

    #[test]
    fn selecting_past_the_last_row_is_ignored() {
        let mut p = Popup::new(Vault::Open(six()), SYNCED);
        p.select(2);
        p.select(40);
        assert_eq!(p.view().selected, 2);
        assert_eq!(p.selected_id(), Some("3"));
    }

    #[test]
    fn the_view_carries_what_a_row_can_do() {
        let l = listing(
            vec![item("1", "Mail", "", true), item("2", "Door code", "x", false)],
            &[],
        );
        let view = Popup::new(Vault::Open(l), SYNCED).view();
        assert!(!view.rows[0].can_copy_username && view.rows[0].can_copy_password);
        assert!(view.rows[1].can_copy_username && !view.rows[1].can_copy_password);
        assert_eq!(view.shape, Shape::Open { rows: 2 });
    }

    #[test]
    fn an_empty_vault_and_an_empty_search_say_different_things() {
        let mut p = Popup::new(Vault::Open(listing(vec![], &[])), SYNCED);
        assert_eq!(p.view().empty.map(|e| e.0), Some("Nothing to show yet"));
        assert_eq!(p.shape(), Shape::Open { rows: 0 });
        p.set_query("zzz");
        assert_eq!(p.view().empty.map(|e| e.0), Some("No matches"));
    }

    #[test]
    fn the_locked_view_has_no_rows_and_no_empty_note() {
        let p = Popup::new(Vault::Locked, SYNCED);
        let view = p.view();
        assert_eq!(view.shape, Shape::Locked);
        assert!(view.rows.is_empty() && view.empty.is_none());
        assert_eq!(p.selected_id(), None);
        assert_eq!(view.footer.tone, Tone::Quiet);
    }

    #[test]
    fn a_name_keeps_its_avatar_colour() {
        assert_eq!(colour_of("Ledgerline"), colour_of("Ledgerline"));
        assert!(colour_of("anything at all") < 3);
    }

    // ---- layout -------------------------------------------------------------

    fn every_box(l: &Layout) -> Vec<Box2> {
        let mut all = vec![l.mark, l.wordmark, l.lock, l.open, l.search, l.search_icon, l.edit];
        all.extend([l.label, l.note, l.title, l.body, l.unlock, l.footer]);
        all.extend(l.tiles);
        for r in &l.rows {
            all.extend([r.row, r.avatar, r.text, r.copy_username, r.copy_password]);
        }
        all.into_iter().filter(|b| b.w > 0).collect()
    }

    #[test]
    fn nothing_the_panel_lays_out_falls_off_it() {
        let mut shapes = vec![Shape::Locked];
        shapes.extend((0..=ROWS_MAX + 2).map(|rows| Shape::Open { rows }));
        for shape in shapes {
            let l = layout(shape);
            assert_eq!(l.window.w, WIDTH);
            for b in every_box(&l) {
                assert!(
                    b.x >= 0 && b.y >= 0 && b.right() <= l.window.w && b.bottom() <= l.window.h,
                    "{shape:?}: {b:?} falls off the {}x{} panel",
                    l.window.w,
                    l.window.h
                );
            }
        }
    }

    #[test]
    fn the_panel_has_the_designs_width_and_grows_one_row_at_a_time() {
        assert_eq!(WIDTH, 390);
        let h = |rows| layout(Shape::Open { rows }).window.h;
        assert_eq!(h(2) - h(1), ROW_H + 2);
        assert_eq!(h(4) - h(3), ROW_H + 2);
        assert_eq!(h(0), h(1), "the empty note is one row tall");
        assert_eq!(h(9), h(ROWS_MAX), "the list is capped");
    }

    #[test]
    fn rows_do_not_overlap_each_other_the_tiles_or_the_footer() {
        let l = layout(Shape::Open { rows: 4 });
        for pair in l.rows.windows(2) {
            assert!(pair[0].row.bottom() <= pair[1].row.y);
        }
        assert!(l.rows.last().unwrap().row.bottom() <= l.tiles[0].y);
        assert!(l.tiles[0].bottom() <= l.footer.y);
        assert!(l.tiles[0].right() <= l.tiles[1].x && l.tiles[1].right() <= l.tiles[2].x);
        assert_eq!(l.tiles[0].h, TILE_H);
    }

    #[test]
    fn a_rows_text_stops_short_of_its_buttons() {
        let l = layout(Shape::Open { rows: 1 });
        let r = l.rows[0];
        assert!(r.text.right() < r.copy_username.x);
        assert!(r.copy_username.right() <= r.copy_password.x);
        assert!(r.copy_password.right() <= r.row.right());
        assert_eq!((r.copy_username.w, r.copy_username.h), (ROW_BUTTON, ROW_BUTTON));
    }

    #[test]
    fn the_search_box_sits_inside_its_field() {
        let l = layout(Shape::Open { rows: 4 });
        assert!(l.edit.x >= l.search_icon.right());
        assert!(l.edit.right() <= l.search.right());
        assert!(l.edit.y >= l.search.y && l.edit.bottom() <= l.search.bottom());
        assert_eq!(l.search.h, SEARCH_H);
    }

    #[test]
    fn the_locked_shape_offers_unlock_and_neither_search_nor_tiles() {
        let l = layout(Shape::Locked);
        assert!(l.unlock.w > 0 && l.lock.w == 0 && l.search.w == 0 && l.rows.is_empty());
        assert!(l.tiles.iter().all(|t| t.w == 0));
        assert!(l.open.w > 0, "the window can still be opened while locked");
    }

    // ---- hit testing --------------------------------------------------------

    fn centre(b: Box2) -> (i32, i32) {
        (b.x + b.w / 2, b.y + b.h / 2)
    }

    #[test]
    fn every_target_answers_the_click_on_its_middle() {
        let l = layout(Shape::Open { rows: 3 });
        let at = |b| {
            let (x, y) = centre(b);
            hit(&l, x, y)
        };
        assert_eq!(at(l.lock), Some(Hit::Lock));
        assert_eq!(at(l.open), Some(Hit::OpenWindow));
        assert_eq!(at(l.tiles[0]), Some(Hit::Generate));
        assert_eq!(at(l.tiles[1]), Some(Hit::NewItem));
        assert_eq!(at(l.tiles[2]), Some(Hit::Share));
        for i in 0..3 {
            assert_eq!(at(l.rows[i].copy_username), Some(Hit::CopyUsername(i)));
            assert_eq!(at(l.rows[i].copy_password), Some(Hit::CopyPassword(i)));
            assert_eq!(at(l.rows[i].avatar), Some(Hit::Row(i)));
        }
        let locked = layout(Shape::Locked);
        let (x, y) = centre(locked.unlock);
        assert_eq!(hit(&locked, x, y), Some(Hit::Unlock));
    }

    #[test]
    fn a_click_on_nothing_answers_nothing() {
        let l = layout(Shape::Open { rows: 2 });
        assert_eq!(hit(&l, 0, 0), None);
        assert_eq!(hit(&l, l.window.w - 1, l.window.h - 1), None);
        assert_eq!(hit(&l, -5, 10), None);
        // The search field and the label are not buttons.
        let (x, y) = centre(l.search);
        assert_eq!(hit(&l, x, y), None);
    }

    #[test]
    fn an_empty_box_is_never_hit() {
        // The locked shape's zero-size lock button sits at (0, 0).
        let l = layout(Shape::Locked);
        assert_eq!(hit(&l, 0, 0), None);
    }

    // ---- placement and the reopen rule -------------------------------------

    #[test]
    fn the_panel_opens_at_the_bottom_right_of_the_work_area() {
        assert_eq!(place((0, 0, 1920, 1040), 390, 500), (1920 - 14 - 390, 1040 - 14 - 500));
        // A second monitor's work area is not at the origin.
        assert_eq!(place((1920, 0, 3840, 1040), 390, 500), (3840 - 14 - 390, 1040 - 14 - 500));
    }

    #[test]
    fn a_work_area_smaller_than_the_panel_keeps_its_top_left() {
        assert_eq!(place((0, 0, 300, 300), 390, 500), (0, 0));
    }

    #[test]
    fn a_click_right_after_a_focus_loss_close_is_the_same_click() {
        let t0 = Instant::now();
        assert!(!click_reopens(Some(t0), t0 + Duration::from_millis(100)));
        assert!(click_reopens(Some(t0), t0 + Duration::from_millis(600)));
        assert!(click_reopens(None, t0), "a panel that closed another way is not toggled");
    }

    // ---- the decision loop --------------------------------------------------

    thread_local! {
        static SCRIPT: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static VIEWS: RefCell<Vec<View>> = const { RefCell::new(Vec::new()) };
        static CAN_OPEN: RefCell<bool> = const { RefCell::new(true) };
    }

    fn fake_open(_: Shape) -> Option<PopupWindow> {
        LOG.with(|l| l.borrow_mut().push("open".into()));
        CAN_OPEN.with(|c| *c.borrow()).then_some(PopupWindow(7))
    }
    fn fake_protect(_: PopupWindow) -> bool {
        LOG.with(|l| l.borrow_mut().push("protect".into()));
        true
    }
    fn fake_next(_: PopupWindow) -> Event {
        LOG.with(|l| l.borrow_mut().push("next".into()));
        SCRIPT.with(|s| {
            let mut s = s.borrow_mut();
            assert!(!s.is_empty(), "the loop asked for an event nobody scripted");
            s.remove(0)
        })
    }
    fn fake_show(_: PopupWindow, view: &View) {
        VIEWS.with(|v| v.borrow_mut().push(view.clone()));
    }
    fn fake_close(_: PopupWindow) {
        LOG.with(|l| l.borrow_mut().push("close".into()));
    }

    const FAKE: PopupCalls = PopupCalls {
        open: fake_open,
        protect: fake_protect,
        next: fake_next,
        show: fake_show,
        close: fake_close,
    };

    fn drive(
        vault: Vault,
        script: Vec<Event>,
        copy: &mut dyn FnMut(&str, Field) -> CopyResult,
    ) -> (Outcome, Vec<View>) {
        SCRIPT.with(|s| *s.borrow_mut() = script);
        LOG.with(|l| l.borrow_mut().clear());
        VIEWS.with(|v| v.borrow_mut().clear());
        CAN_OPEN.with(|c| *c.borrow_mut() = true);
        let outcome = run_with(&FAKE, Popup::new(vault, SYNCED), copy);
        (outcome, VIEWS.with(|v| v.borrow().clone()))
    }

    fn log() -> Vec<String> {
        LOG.with(|l| l.borrow().clone())
    }

    fn never(_: &str, _: Field) -> CopyResult {
        panic!("nothing should have been copied");
    }

    #[test]
    fn the_window_is_protected_before_it_is_pumped_and_closed_on_every_exit() {
        for end in [Event::Cancel, Event::Closed, Event::LostFocus, Event::Click(Hit::OpenWindow)] {
            let _ = drive(Vault::Open(six()), vec![end], &mut never);
            assert_eq!(log(), ["open", "protect", "next", "close"]);
        }
    }

    #[test]
    fn a_window_that_cannot_open_is_unavailable_and_is_not_closed() {
        SCRIPT.with(|s| s.borrow_mut().clear());
        LOG.with(|l| l.borrow_mut().clear());
        CAN_OPEN.with(|c| *c.borrow_mut() = false);
        let outcome = run_with(&FAKE, Popup::new(Vault::Open(six()), SYNCED), &mut never);
        assert_eq!(outcome, Outcome::Unavailable);
        assert_eq!(log(), ["open"], "no window, so nothing to protect, pump or close");
        CAN_OPEN.with(|c| *c.borrow_mut() = true);
    }

    #[test]
    fn the_ways_out_are_told_apart() {
        let out = |e| drive(Vault::Open(six()), vec![e], &mut never).0;
        assert_eq!(out(Event::Cancel), Outcome::Dismissed);
        assert_eq!(out(Event::Closed), Outcome::Dismissed);
        assert_eq!(out(Event::LostFocus), Outcome::LostFocus);
        assert_eq!(out(Event::Click(Hit::Lock)), Outcome::Lock);
        assert_eq!(out(Event::LockKey), Outcome::Lock);
        assert_eq!(out(Event::Click(Hit::OpenWindow)), Outcome::OpenWindow);
        assert_eq!(out(Event::Click(Hit::Generate)), Outcome::Generate);
        assert_eq!(out(Event::Click(Hit::NewItem)), Outcome::NewItem);
        assert_eq!(out(Event::Click(Hit::Share)), Outcome::Share);
    }

    #[test]
    fn a_copy_button_copies_that_rows_field_and_the_panel_stays_up() {
        let seen = RefCell::new(Vec::new());
        let mut copy = |id: &str, field: Field| {
            seen.borrow_mut().push((id.to_string(), field));
            CopyResult::Copied
        };
        let (outcome, views) = drive(
            Vault::Open(six()),
            vec![
                Event::Click(Hit::CopyPassword(1)),
                Event::Click(Hit::CopyUsername(2)),
                Event::Cancel,
            ],
            &mut copy,
        );
        assert_eq!(outcome, Outcome::Dismissed);
        assert_eq!(
            *seen.borrow(),
            [("2".to_string(), Field::Password), ("3".to_string(), Field::Username)]
        );
        assert_eq!(views[1].toast.as_deref(), Some("Password copied"));
        assert_eq!(views[2].toast.as_deref(), Some("Username copied"));
        assert_eq!(views[2].selected, 2, "the row copied from becomes the selected one");
    }

    #[test]
    fn enter_and_the_chords_copy_from_the_selected_row() {
        let seen = RefCell::new(Vec::new());
        let mut copy = |id: &str, field: Field| {
            seen.borrow_mut().push((id.to_string(), field));
            CopyResult::Copied
        };
        drive(
            Vault::Open(six()),
            vec![
                Event::Step(true),
                Event::Copy(Field::Password),
                Event::Copy(Field::Username),
                Event::Cancel,
            ],
            &mut copy,
        );
        assert_eq!(
            *seen.borrow(),
            [("2".to_string(), Field::Password), ("2".to_string(), Field::Username)]
        );
    }

    #[test]
    fn a_refused_copy_says_so_and_does_not_say_copied() {
        for result in [CopyResult::Cancelled, CopyResult::NeedsHello] {
            let mut copy = |_: &str, _: Field| result;
            let (_, views) = drive(
                Vault::Open(six()),
                vec![Event::Click(Hit::CopyPassword(0)), Event::Cancel],
                &mut copy,
            );
            assert_eq!(
                views.last().unwrap().toast.as_deref(),
                Some(toast_for(Field::Password, result))
            );
        }
    }

    #[test]
    fn the_toast_goes_when_its_time_is_up_and_when_the_user_types() {
        let mut copy = |_: &str, _: Field| CopyResult::Copied;
        let (_, views) = drive(
            Vault::Open(six()),
            vec![Event::Copy(Field::Password), Event::ToastOver, Event::Cancel],
            &mut copy,
        );
        assert!(views[1].toast.is_some());
        assert!(views[2].toast.is_none());
        let (_, views) = drive(
            Vault::Open(six()),
            vec![Event::Copy(Field::Password), Event::Typed("a".into()), Event::Cancel],
            &mut copy,
        );
        assert!(views[2].toast.is_none());
    }

    #[test]
    fn typing_filters_the_rows_and_a_copy_then_reaches_the_filtered_row() {
        let seen = RefCell::new(Vec::new());
        let mut copy = |id: &str, field: Field| {
            seen.borrow_mut().push((id.to_string(), field));
            CopyResult::Copied
        };
        let (_, views) = drive(
            Vault::Open(six()),
            vec![
                Event::Typed("harbor".into()),
                Event::Click(Hit::CopyPassword(0)),
                Event::Cancel,
            ],
            &mut copy,
        );
        assert_eq!(views[1].rows.len(), 1);
        assert_eq!(views[1].shape, Shape::Open { rows: 1 });
        assert_eq!(*seen.borrow(), [("5".to_string(), Field::Password)]);
    }

    #[test]
    fn a_copy_at_a_row_that_is_not_there_copies_nothing() {
        let (outcome, views) = drive(
            Vault::Open(six()),
            vec![
                Event::Typed("zzz".into()),
                Event::Click(Hit::CopyPassword(3)),
                Event::Copy(Field::Password),
                Event::Cancel,
            ],
            &mut never,
        );
        assert_eq!(outcome, Outcome::Dismissed);
        assert!(views.last().unwrap().rows.is_empty());
    }

    #[test]
    fn a_locked_panel_unlocks_on_the_button_or_enter_and_nothing_else_acts() {
        assert_eq!(drive(Vault::Locked, vec![Event::Click(Hit::Unlock)], &mut never).0, Outcome::Unlock);
        assert_eq!(
            drive(Vault::Locked, vec![Event::Copy(Field::Password)], &mut never).0,
            Outcome::Unlock
        );
        // A chord that would lock, and a stray search, do nothing while locked.
        let (outcome, views) = drive(
            Vault::Locked,
            vec![Event::LockKey, Event::Typed("x".into()), Event::Step(true), Event::Cancel],
            &mut never,
        );
        assert_eq!(outcome, Outcome::Dismissed);
        assert!(views.iter().all(|v| v.shape == Shape::Locked && v.rows.is_empty()));
    }

    // ---- source pins --------------------------------------------------------

    fn production() -> &'static str {
        let source = include_str!("tray_popup.rs");
        source.split_once("\n#[cfg(test)]").expect("a test module").0
    }

    fn code(text: &str) -> String {
        text.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_panel_never_touches_the_clipboard_or_a_password_itself() {
        let code = code(production());
        assert!(
            !code.contains(concat!("copy_", "secret")),
            "the daemon's closure copies; this file never does"
        );
        assert!(!code.contains(concat!("clipboard", "::")));
        assert!(
            !code.contains(concat!(".pass", "word")),
            "an Item holds whether a password exists, never one"
        );
    }

    #[test]
    fn nothing_the_user_typed_reaches_the_log() {
        for line in code(production()).lines().filter(|l| l.contains("log::")) {
            assert!(
                !line.contains("query") && !line.contains("typed") && !line.contains("Typed"),
                "{line:?} logs what the user searched for"
            );
        }
    }

    #[test]
    fn the_window_never_posts_a_thread_quit_and_spawns_no_thread() {
        let code = code(production());
        assert!(!code.contains(concat!("PostQuit", "Message(")));
        assert!(!code.contains(concat!("thread::", "spawn")));
    }

    #[test]
    fn the_window_opens_under_a_title_nothing_else_uses() {
        assert_eq!(TRAY_POPUP_TITLE, "Deskwarden quick access");
        assert_ne!(TRAY_POPUP_TITLE, crate::generate_prompt::GENERATE_PROMPT_TITLE);
    }
}

//! **The account picker's decision: which item, then which field.**
//!
//! `CTRL+ALT+B` on an app with no configured binding offers a small card of
//! plausible accounts (`crate::app_candidates::Candidate`, Task 1) painted in
//! bare Win32 (`crate::win32_draw`, Tasks 2-3). This module is the *decision*
//! that sits between them: which candidate was picked, and once picked, which
//! field of it should be typed. No window is created here -- a later task
//! writes the Win32 half and calls [`run_with`].
//!
//! # The seam
//!
//! Mirrors `crate::unlock_prompt::run_with` exactly: `open`, then `protect`
//! **before** the first `next`, then a loop over `next`, with `close` on
//! every exit path including the failures. That ordering is security-relevant
//! there and stays security-relevant here for the same reason -- a window
//! that can be typed into (or, here, clicked into to pick a private account
//! name) before it is excluded from screen capture is one a recorder can
//! catch.
//!
//! # No secret ever rides on these types
//!
//! [`crate::app_candidates::Candidate`] already carries only an id, a name and
//! a username -- never a password. [`Outcome`] and [`Event`] keep that
//! property: `Outcome::Fill` carries the item's id and *which* field to type,
//! never the field's value. The value is fetched at dispatch, by the
//! component that already holds it, exactly as the module doc for
//! `Candidate` requires.

use std::sync::atomic::{AtomicBool, AtomicIsize};

use crate::app_candidates::Candidate;
use crate::key_sequence::FieldRef;

/// The window handle [`run_with`] deals in.
///
/// A bare `isize` newtype, not an `HWND`, for the same reason
/// `unlock_prompt::PromptWindow` is: a decision layer a test can drive must
/// not name a type that only exists behind a Win32 feature gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickerWindow(pub isize);

/// Which field of the chosen item to type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Send {
    /// One field, by reference -- a username, a password, a TOTP code, or a
    /// custom field, exactly as offered by
    /// `crate::key_sequence::field_palette`.
    Field(FieldRef),
    /// Username, then Tab, then password. See [`tokens_for`] for why there is
    /// no trailing Enter.
    All,
    /// The item's own stored sequence, interpreted by
    /// `crate::key_sequence::parse` -- never a second reading of the string.
    Sequence,
}

/// How [`run_with`] finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A candidate and a field were chosen. `id` is the item's id, never a
    /// secret.
    Fill { id: String, send: Send },
    /// The user asked to create a new login for this app.
    NewLogin,
    /// Too many candidates to list; the user asked to search the vault
    /// instead of picking from the truncated card.
    SearchVault,
    /// The user asked to edit the chosen candidate's binding.
    Edit(String),
    /// The user declined. Nothing is armed.
    Cancelled,
    /// The window could not be put on screen at all.
    Unavailable,
}

/// What the user did with the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Cancel, Escape, or the close glyph.
    Cancel,
    /// The window went away underneath us. Treated exactly as `Cancel`.
    Closed,
    /// Picked the candidate at this index into the slice `run_with` was
    /// given.
    Chose(usize),
    /// Asked to search the vault instead -- offered only when the card is
    /// truncated. See `crate::win32_draw::visible_rows`.
    Overflow,
    /// Asked to create a new login for this app.
    NewLogin,
    /// Asked to edit the previously chosen candidate's binding.
    EditSelected,
    /// Picked which field of the previously chosen candidate to type.
    Sends(Send),
}

/// The fields a candidate offers, and whether it has a stored sequence worth
/// offering as [`Send::Sequence`].
///
/// A named struct rather than a bare `(Vec<FieldRef>, bool)` so call sites
/// say what the `bool` means instead of `show_palette(window, &fields,
/// false)` and `|_| (vec![], false)` -- neither of which tells a reader
/// anything on its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The fields offered, same shape as
    /// `crate::key_sequence::field_palette`.
    pub fields: Vec<FieldRef>,
    /// Whether the item has a stored sequence worth offering as
    /// [`Send::Sequence`].
    pub has_sequence: bool,
}

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything; every decision lives in
/// [`run_with`].
pub struct PickerCalls {
    /// Lays out and shows the card of candidates. `None` if it could not be
    /// put on screen.
    pub open: fn(&[Candidate]) -> Option<PickerWindow>,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the top-level
    /// window, called before the first `next` -- see the module doc.
    pub protect: fn(PickerWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(PickerWindow) -> Event,
    /// Shows the field palette for the chosen candidate.
    pub show_palette: fn(PickerWindow, &Palette),
    /// Destroys the window and releases its resources.
    pub close: fn(PickerWindow),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// `palette` maps a chosen candidate's id to the fields it offers and whether
/// it has a sequence worth offering -- the same shape
/// `crate::key_sequence::field_palette` produces, wrapped by the caller
/// because this layer works in ids and that one works in `VaultItem`s.
///
/// 1. `protect` runs immediately after `open` and before the first `next`.
/// 2. Choosing a row (`Event::Chose`) looks up that candidate's palette and
///    shows it; it does not by itself produce an `Outcome`.
/// 3. A field choice (`Event::Sends`) answers `Outcome::Fill` for the
///    most-recently-chosen candidate.
/// 4. `close` runs on every exit path, including `Unavailable`'s
///    predecessor -- there is no window to close there, which is exactly why
///    `open` returning `None` returns before ever calling it.
pub fn run_with(
    calls: &PickerCalls,
    candidates: &[Candidate],
    palette: fn(&str) -> Palette,
) -> Outcome {
    let Some(window) = (calls.open)(candidates) else {
        log::warn!("the account picker could not be put on screen");
        return Outcome::Unavailable;
    };

    // Before the first pump, so nothing in the card can be clicked while the
    // window is still capturable.
    if !(calls.protect)(window) {
        log::warn!(
            "SetWindowDisplayAffinity was refused for the account picker; its contents are \
             visible to screen capture on this machine"
        );
    }

    let mut chosen: Option<usize> = None;

    loop {
        match (calls.next)(window) {
            Event::Cancel | Event::Closed => {
                (calls.close)(window);
                return Outcome::Cancelled;
            }
            Event::Overflow => {
                (calls.close)(window);
                return Outcome::SearchVault;
            }
            Event::NewLogin => {
                (calls.close)(window);
                return Outcome::NewLogin;
            }
            Event::Chose(index) => {
                let Some(candidate) = candidates.get(index) else {
                    log::warn!(
                        "the account picker chose row {index} but only {len} candidates were \
                         offered; the Win32 row list and the candidate slice have disagreed, \
                         which would otherwise surface later as the picker typing the wrong \
                         account's password -- ignoring the choice",
                        len = candidates.len()
                    );
                    continue;
                };
                chosen = Some(index);
                let palette = palette(&candidate.id);
                (calls.show_palette)(window, &palette);
            }
            Event::EditSelected => {
                if let Some(candidate) = chosen.and_then(|index| candidates.get(index)) {
                    let id = candidate.id.clone();
                    (calls.close)(window);
                    return Outcome::Edit(id);
                }
                log::warn!(
                    "the account picker got EditSelected with nothing chosen yet; ignoring it"
                );
            }
            Event::Sends(send) => {
                if let Some(candidate) = chosen.and_then(|index| candidates.get(index)) {
                    let id = candidate.id.clone();
                    (calls.close)(window);
                    return Outcome::Fill { id, send };
                }
                // **`describe_send`, not `{send:?}`.** A `Debug` of this value
                // prints `Field(Custom("Recovery PIN"))` -- a name out of the
                // user's own vault item, in a diagnostic that lands in a log
                // file on disk. See that function for why the crate's one
                // precedent for spelling such a name does not extend here.
                log::warn!(
                    "the account picker got a field choice ({}) with nothing chosen yet; \
                     ignoring it",
                    describe_send(&send)
                );
            }
        }
    }
}

/// What each choice types.
///
/// `All` is `{USERNAME}{TAB}{PASSWORD}` **with no trailing Enter** -- see the
/// test, which carries the reasoning: a trailing Enter submits, and if the
/// target's field order differs from this assumption it submits the wrong
/// content. Typing without submitting fails visibly; submitting fails
/// invisibly. `Sequence` goes through [`crate::key_sequence::parse`] rather
/// than a second reading of the string, so the picker and the sequence editor
/// can never disagree about what a sequence means.
pub fn tokens_for(send: &Send, sequence: Option<&str>) -> Vec<crate::key_sequence::Token> {
    use crate::key_sequence::Token;
    match send {
        Send::Field(field) => vec![Token::Field(field.clone())],
        Send::All => {
            // A half-sequence here is worse than none: username with no Tab
            // between it and password types the password straight into the
            // username box, in plaintext, in whatever app is focused. If
            // "TAB" is ever not a known key that is a bug in the key table,
            // not a reason to degrade -- refuse loudly instead.
            let tab = crate::key_sequence::key_named("TAB").expect("TAB is a known key");
            vec![
                Token::Field(FieldRef::Username),
                Token::Key(tab),
                Token::Field(FieldRef::Password),
            ]
        }
        Send::Sequence => sequence.map(crate::key_sequence::parse).unwrap_or_default(),
    }
}


// ---------------------------------------------------------------------------
// The window, and everything the caller must gather before there can be one.
// ---------------------------------------------------------------------------

/// The window's title.
///
/// **Unique across this process**, for the reason
/// [`crate::unlock_prompt::UNLOCK_PROMPT_TITLE`] is: `foreground::pick` finds
/// a window by title and takes the FIRST match in `EnumWindows` order, and
/// this card is provably alive alongside the tray icon's and the hotkey
/// listener's helper windows. `foreground`'s
/// `only_one_window_of_this_process_can_exist_at_a_time` asserts it differs
/// from every other title this crate opens under.
pub const PICKER_PROMPT_TITLE: &str = "Deskwarden account picker";

/// One row's worth of everything the card needs, gathered by the caller.
///
/// **The caller reads the icon off disk; the paint path never does.** The
/// favicon cache directory is `main`'s
/// (`project_dirs.cache_dir().join("icons")`), and threading it down into a
/// window procedure would mean either a second derivation of that path or a
/// static holding it. Handing the bytes in is also what keeps the promise the
/// paint path has to make: no file read, and above all no network fetch,
/// between a repaint and the pixels.
///
/// **Still no secret.** [`Candidate`] carries display strings and an id;
/// [`Palette`] is presence-only (`crate::key_sequence::field_palette` asks
/// whether a value is there, never what it is); and `icon` is a picture of a
/// website. `Outcome::Fill` still names a field rather than carrying its
/// value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub candidate: Candidate,
    /// What this candidate offers once chosen -- the second step's rows.
    pub palette: Palette,
    /// The cached favicon as PNG bytes, if the on-disk cache had one.
    /// `None` draws the row without an icon.
    pub icon: Option<Vec<u8>>,
}

/// What each [`Send`] is called on screen, and what it says it will type.
///
/// A function rather than literals at the paint site, so both steps of this
/// card and any later reader agree about what a choice is called. The field
/// names defer to [`crate::key_sequence::FieldRef::label`] rather than
/// restating it, exactly as `crate::app::FillChoice::label` does.
pub fn send_label(send: &Send) -> (String, &'static str) {
    match send {
        Send::All => ("Username + Tab + Password".to_string(), "Types both fields in order"),
        Send::Sequence => ("Saved sequence".to_string(), "Runs this item's own sequence"),
        Send::Field(field) => (
            field.label(),
            match field {
                FieldRef::Username => "Types the username",
                FieldRef::Password => "Types the password",
                FieldRef::Totp => "Types a one-time code",
                FieldRef::Custom(_) => "Types this custom field",
            },
        ),
    }
}

/// What a [`Send`] is called **in a log line**, which is not what it is called
/// on screen.
///
/// **It never spells a custom field's name**, and that is the whole reason it
/// exists. [`send_label`] returns `FieldRef::label`, which for
/// `FieldRef::Custom` is the user's own field name -- right on a button they
/// are looking at, wrong in a diagnostic written to a log file on disk and
/// read later by whoever is debugging. The crate's one existing precedent for
/// spelling the name (`crate::injector::sequence::Refusal::Unresolved`, "a
/// field called PIN") is a message shown *to that user, about their own item,
/// at the moment they asked for it*; a warning about an event the picker is
/// discarding is not that, so it does not follow it.
pub fn describe_send(send: &Send) -> &'static str {
    match send {
        Send::All => "username, Tab and password",
        Send::Sequence => "the item's own sequence",
        Send::Field(FieldRef::Username) => "the username",
        Send::Field(FieldRef::Password) => "the password",
        Send::Field(FieldRef::Totp) => "a one-time code",
        Send::Field(FieldRef::Custom(_)) => "a custom field",
    }
}

/// The rows the second step offers, in the order they are shown.
///
/// Pure, so the ordering and the bound below are testable without a window.
///
/// **Bounded by construction, and that is load-bearing.** This card does not
/// scroll and cannot be resized, so an entry past the last slot is one the
/// user can neither see nor reach. [`Palette::fields`] is
/// `crate::key_sequence::field_palette`'s answer, which is unbounded -- an
/// item may carry any number of custom fields -- so a card sized for it is a
/// card that cannot be sized. `crate::app::fill_choices` met the same wall and
/// answered it the same way, with the same reason: the sequence builder
/// already covers custom fields, and an unbounded row count is a geometry
/// hazard for a fixed-size surface. This is that decision again rather than a
/// second one.
pub fn palette_rows(palette: &Palette) -> Vec<Send> {
    // The item's own sequence and nothing else, for `fill_choices`' reason:
    // the user wrote it precisely because the generic rows were not what that
    // app wanted, so offering them back is offering the thing they rejected.
    if palette.has_sequence {
        return vec![Send::Sequence];
    }
    let has = |field: FieldRef| palette.fields.contains(&field);
    let mut out = Vec::new();
    if has(FieldRef::Username) && has(FieldRef::Password) {
        // First, because it is what the overwhelming majority of screens want;
        // the single-field rows below exist for the ones that do not.
        out.push(Send::All);
    }
    for field in [FieldRef::Username, FieldRef::Password, FieldRef::Totp] {
        if has(field.clone()) {
            out.push(Send::Field(field));
        }
    }
    out
}

/// **Puts the card on screen and answers what the user did with it.**
///
/// The production [`REAL`] calls, [`run_with`]'s decision, and nothing else.
/// `run_with`'s `palette` argument is a bare `fn` pointer -- so it cannot
/// close over anything -- and what it reads is the [`Offer`] slice this
/// function parks for it, which is why the offers go in and come back out
/// around the call rather than being a parameter of the seam.
pub fn ask(offers: &[Offer]) -> Outcome {
    let candidates: Vec<Candidate> = offers.iter().map(|o| o.candidate.clone()).collect();
    if let Ok(mut slot) = OFFERS.lock() {
        *slot = offers.to_vec();
    }
    let outcome = run_with(&REAL, &candidates, palette_of);
    if let Ok(mut slot) = OFFERS.lock() {
        slot.clear();
    }
    outcome
}

/// The offers [`ask`] parked, read by [`palette_of`] and by the window.
static OFFERS: std::sync::Mutex<Vec<Offer>> = std::sync::Mutex::new(Vec::new());

/// [`run_with`]'s `palette` argument in production: the parked offer's own.
///
/// An empty palette for an id that is not there is not a silent nothing -- the
/// second step then shows no rows, which is visible on screen -- and the id
/// came out of the same slice one line earlier, so it cannot honestly happen.
fn palette_of(id: &str) -> Palette {
    OFFERS
        .lock()
        .ok()
        .and_then(|offers| offers.iter().find(|o| o.candidate.id == id).map(|o| o.palette.clone()))
        .unwrap_or(Palette { fields: Vec::new(), has_sequence: false })
}

/// The production [`PickerCalls`].
pub static REAL: PickerCalls = PickerCalls {
    open: win32::open,
    protect: win32::protect,
    next: win32::next,
    show_palette: win32::show_palette,
    close: win32::close,
};

// ---------------------------------------------------------------------------
// Layout
//
// Logical pixels, at 100%, every one of them read off `theme` or off the
// surfaces this card sits beside. Numbers invented here would be a second
// layout that has to agree with a first, which is this codebase's standing
// defect shape -- `unlock_prompt::layout`'s header says the same, and this is
// that discipline rather than a copy of that window.
// ---------------------------------------------------------------------------

/// The card's width, and so the window's. Narrower than
/// `unlock_prompt::WIDTH`: that card holds a 470px form, this one holds a list
/// whose longest line is an item name.
pub const WIDTH: i32 = 380;

/// Content inset, and the top margin.
const MARGIN_X: i32 = 16;
const MARGIN_TOP: i32 = 16;

/// One row. Tall enough for a name over a username, and for a square icon
/// gutter beside them -- [`crate::win32_draw::draw_row`] takes the gutter to
/// be the row's own height, so this is also the icon column's width.
const ROW_H: i32 = 44;

/// **How many rows the card has room for, and it is the same number in both
/// steps.**
///
/// The card does not scroll and cannot be resized, so this is not a viewport
/// onto a longer list -- it is the whole of what is reachable.
/// [`crate::win32_draw::visible_rows`] spends one of these slots on a *Search
/// the vault* row when there are more candidates than fit, which is what stops
/// the truncation being silent.
pub const ROW_CAP: usize = 5;

/// Button height. `theme::BUTTON_HEIGHT`.
const BUTTON_H: i32 = 32;

/// One rectangle of the card, in logical pixels from the window's top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Box2 {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Box2 {
    pub fn bottom(self) -> i32 {
        self.y + self.h
    }
    pub fn right(self) -> i32 {
        self.x + self.w
    }
}

/// Every rectangle the card paints, computed once.
///
/// Pure arithmetic with no Win32 in it, for `unlock_prompt::layout`'s reason:
/// a control whose bottom edge fell past the window's would simply be
/// invisible on a window that neither scrolls nor resizes, and that is a
/// property worth asserting without opening anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    pub title: Box2,
    pub subtitle: Box2,
    /// The whole list area. Individual rows are [`row_at`].
    pub list: Box2,
    /// The footer's left button: *New login* in the first step, *Edit binding*
    /// in the second.
    pub secondary: Box2,
    pub cancel: Box2,
    pub close_glyph: Box2,
}

/// The card's geometry.
///
/// **The height is fixed, and the list is [`ROW_CAP`] rows tall whichever step
/// is showing.** A window that shrank when the second step had fewer rows than
/// the first would move its own Cancel button out from under the pointer at
/// the moment the user is about to click it; `unlock_prompt::layout` reserves
/// its error row for exactly that reason, and the same argument applies to a
/// card whose two steps have different row counts.
pub fn layout() -> Layout {
    let content_w = WIDTH - 2 * MARGIN_X;

    let title = Box2 { x: MARGIN_X, y: MARGIN_TOP, w: content_w, h: 21 };
    let subtitle = Box2 { x: MARGIN_X, y: title.bottom() + 1, w: content_w, h: 17 };
    let list =
        Box2 { x: MARGIN_X, y: subtitle.bottom() + 10, w: content_w, h: ROW_H * ROW_CAP as i32 };

    // Right-aligned, Cancel outermost: the choice that does nothing sits where
    // the eye leaves the card.
    let cancel = Box2 { x: MARGIN_X + content_w - 84, y: list.bottom() + 12, w: 84, h: BUTTON_H };
    let secondary = Box2 { x: cancel.x - 10 - 104, y: cancel.y, w: 104, h: BUTTON_H };

    let window = Box2 { x: 0, y: 0, w: WIDTH, h: cancel.bottom() + MARGIN_TOP };
    let close_glyph = Box2 { x: WIDTH - MARGIN_X - 20, y: MARGIN_TOP, w: 20, h: 20 };

    Layout { window, title, subtitle, list, secondary, cancel, close_glyph }
}

/// The `index`th row's rectangle, in logical pixels.
pub fn row_at(index: usize) -> Box2 {
    let list = layout().list;
    Box2 { x: list.x, y: list.y + ROW_H * index as i32, w: list.w, h: ROW_H }
}

// ---------------------------------------------------------------------------
// The Win32 half. No decisions live below this line.
// ---------------------------------------------------------------------------

/// Which step the card is showing.
///
/// A static because a window procedure is an `extern "system"` function with
/// nowhere to keep state -- the same reason `unlock_prompt`'s `PENDING` is one.
static MODE: AtomicIsize = AtomicIsize::new(MODE_LIST);
const MODE_LIST: isize = 0;
const MODE_PALETTE: isize = 1;

/// Set by `WM_DESTROY`, so a window that goes away underneath the pump is
/// reported as [`Event::Closed`] rather than pumped forever.
static GONE: AtomicBool = AtomicBool::new(false);

/// What the window procedure last recorded. **Taken** by `next`, never merely
/// read -- see that function for why an event that could be delivered twice
/// would turn `run_with`'s ignore-and-continue arms into a spin.
static PENDING: std::sync::Mutex<Option<Event>> = std::sync::Mutex::new(None);

/// The candidates the first step is showing, and whether the last used slot is
/// the *Search the vault* row rather than a candidate.
static SHOWN: std::sync::Mutex<Vec<Candidate>> = std::sync::Mutex::new(Vec::new());
static OVERFLOWING: AtomicBool = AtomicBool::new(false);

/// The second step's rows. Empty while the first step is showing.
static ENTRIES: std::sync::Mutex<Vec<Send>> = std::sync::Mutex::new(Vec::new());

/// The Win32 calls, and **nothing else**.
///
/// # Why every pixel here is painted by hand
///
/// `crate::unlock_prompt`'s `win32` module carries the whole argument and it
/// is not restated: a themed control renders in the shell's grey with the
/// shell's font, and the last raw-Win32 surface in this project was deleted
/// for looking foreign rather than for being broken. The rows and the footer
/// buttons here are real `BUTTON` windows -- which is what buys focus, the
/// space bar, and `IsDialogMessage` traversal -- with their painting taken
/// over completely and handed to [`crate::win32_draw`], the module both this
/// card and that prompt draw through so neither can drift from the palette.
///
/// # GDI only
///
/// Nothing here creates a Direct2D or Direct3D device. That is measured rather
/// than stylistic: the daemon/UI split put an egui window at ~102 MB and a D2D
/// device at 53.85 MB against the Win32 prompt's 1.79 MB, and a card that cost
/// either would have no reason to exist.
///
/// # GDI object hygiene
///
/// Every brush, pen, font, DC and DIB created below is restored and deleted
/// before its function returns. This is a daemon's repaint path -- one card per
/// hotkey press for as long as the machine is up -- and a leaked handle here
/// exhausts the table over a session rather than over a run.
///
/// Nothing in this module decides anything. See [`run_with`].
mod win32 {
    use super::{
        Box2, Candidate, Event, Palette, PickerWindow, ENTRIES, GONE, MODE, MODE_LIST,
        MODE_PALETTE, OVERFLOWING, PENDING, PICKER_PROMPT_TITLE, ROW_CAP, SHOWN,
    };
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AddFontMemResourceEx, AlphaBlend, BeginPaint, BitBlt, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, CreatePen, CreateSolidBrush,
        DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetDeviceCaps,
        InvalidateRect, ReleaseDC, RoundRect, SelectObject, SetBkMode, SetTextColor, AC_SRC_ALPHA,
        AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CLEARTYPE_QUALITY,
        DIB_RGB_COLORS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL,
        HBITMAP, HBRUSH, HDC, HFONT, LOGFONTW, LOGPIXELSX, PAINTSTRUCT, PS_SOLID, SRCCOPY,
        TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetDlgItem, GetWindowLongPtrW, IsDialogMessageW, LoadCursorW, PeekMessageW,
        PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow,
        SetWindowDisplayAffinity, SetWindowLongPtrW, ShowWindow, TranslateMessage, BN_CLICKED,
        BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, GWLP_WNDPROC, HMENU, HTCAPTION, IDC_ARROW, MSG,
        PM_REMOVE, SW_HIDE, SW_SHOW, WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE,
        WM_COMMAND, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCHITTEST,
        WM_PAINT, WM_QUIT, WM_SETFONT, WNDCLASSW, WS_CHILD, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP,
        WS_VISIBLE,
    };

    use crate::win32_draw::{draw_button, draw_row, rgb, ButtonSkin, RowState};

    /// Row `i` is control `ID_ROW + i`; the footer's two ids sit below them
    /// all, so a row id can never collide with a button id however many rows
    /// there are.
    const ID_ROW: usize = 200;
    const ID_SECONDARY: usize = 101;
    const ID_CANCEL: usize = 102;

    const CLASS_NAME: PCWSTR = w!("DeskwardenAccountPicker");

    /// The window's DPI as a percentage of 96, sampled once per open.
    ///
    /// **The SYSTEM DPI, not the monitor's**, and a known limitation rather
    /// than an oversight -- `unlock_prompt`'s own `DPI_PERCENT` carries the
    /// whole argument: `GetDpiForWindow` lives behind a `windows` crate
    /// feature this crate does not enable, and enabling it re-pins
    /// `job_object.rs`'s whole-file hash of `Cargo.toml`.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    /// The icon's drawn size inside the row's square gutter.
    const ICON_SIDE: i32 = 24;

    // ---- fonts -------------------------------------------------------------

    /// Registers the bundled Archivo cuts privately with GDI, once.
    ///
    /// `AddFontMemResourceEx` makes a face available to **this process only**
    /// -- nothing is installed and nothing touches the user's font list -- and
    /// the handles are deliberately never released, because freeing one while
    /// a window still has it selected is how a surface repaints in the
    /// fallback face.
    fn register_fonts() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            for (_, _, _, bytes) in crate::theme::ARCHIVO_FACES {
                // A `Cell` rather than a `mut` local: GDI writes the count back
                // through a `*const u32`, so a plain immutable binding read
                // afterwards is a value the compiler may fold to its
                // initialiser.
                let installed = std::cell::Cell::new(0u32);
                let handle = AddFontMemResourceEx(
                    bytes.as_ptr() as *const c_void,
                    bytes.len() as u32,
                    None,
                    installed.as_ptr(),
                );
                if handle.0.is_null() || installed.get() == 0 {
                    // Cosmetic degradation, never a reason to refuse to offer
                    // the accounts. GDI falls back to the shell font.
                    log::warn!("could not register a bundled Archivo face with GDI");
                }
            }
        });
    }

    /// An `HFONT` for one of the app's faces at one logical size. The GDI
    /// family and weight come from `crate::theme::gdi_face_for`, which reads
    /// them out of the files' own `name` records rather than guessing.
    fn font(family: &str, px: i32) -> HFONT {
        let (face, weight) = crate::theme::gdi_face_for(family);
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -scale(px),
                lfWeight: if weight >= 700 { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 },
                // ClearType, explicitly: the default quality on a memory DC is
                // not it, and greyscale-antialiased Archivo beside the app's
                // ClearType egui text is exactly the "almost right" that reads
                // as a different program.
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            for (i, ch) in face.encode_utf16().take(31).enumerate() {
                lf.lfFaceName[i] = ch;
            }
            CreateFontIndirectW(&lf)
        }
    }

    /// Every face the card paints with, created at open and destroyed at
    /// close. Kept together so `close` cannot leak one by forgetting it.
    struct Fonts {
        title: HFONT,
        subtitle: HFONT,
        name: HFONT,
        username: HFONT,
        button: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, REGULAR, SEMIBOLD};
            Fonts {
                title: font(BOLD, 15),
                subtitle: font(REGULAR, 12),
                name: font(SEMIBOLD, 13),
                username: font(REGULAR, 11),
                button: font(SEMIBOLD, 12),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [self.title, self.subtitle, self.name, self.username, self.button] {
                    let _ = DeleteObject(f);
                }
            }
        }
    }

    static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
    // `Fonts` holds raw GDI handles, which are process-wide rather than
    // thread-owned. The card is modal on one thread, so nothing shares them;
    // the `Mutex` is only what lets them live in a `static` beside a window
    // procedure that has nowhere else to keep state.
    unsafe impl std::marker::Send for Fonts {}

    // ---- icons -------------------------------------------------------------

    /// One decoded favicon as a 32-bit premultiplied DIB, ready to blend.
    struct Icon {
        bitmap: HBITMAP,
        w: i32,
        h: i32,
    }

    static ICONS: Mutex<Vec<Option<Icon>>> = Mutex::new(Vec::new());
    // Same reason as `Fonts`: a GDI handle is process-wide, and this static is
    // only what lets one live beside a window procedure.
    unsafe impl std::marker::Send for Icon {}

    /// Turns one cached PNG into a DIB section this card can blend.
    ///
    /// **Decoded once, at open, and never in the paint path.** A repaint runs
    /// on every hover; a PNG decode there would put milliseconds between the
    /// pointer moving and the row lighting up, and a file read there would put
    /// the disk on it.
    ///
    /// **Premultiplied, because `AC_SRC_ALPHA` says so.** `AlphaBlend` with
    /// that flag reads the source as premultiplied; handing it straight RGBA
    /// draws a bright halo around every pixel with partial alpha, which is
    /// most of a favicon's edge.
    ///
    /// `None` at any step draws the row without an icon. An icon is decoration
    /// on a row that already says the account's name and its username, and no
    /// part of this card may block on one.
    fn make_icon(png: &[u8]) -> Option<Icon> {
        let (width, height, rgba) = crate::favicon::decode_rgba(png)?;
        if width == 0 || height == 0 || rgba.len() < width * height * 4 {
            return None;
        }
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    // Negative: a top-down DIB, so the rows arrive in the order
                    // `decode_rgba` hands them over and nothing has to be
                    // flipped.
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            if bits.is_null() {
                let _ = DeleteObject(bitmap);
                return None;
            }
            let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, width * height * 4);
            for i in 0..width * height {
                let r = rgba[i * 4] as u32;
                let g = rgba[i * 4 + 1] as u32;
                let b = rgba[i * 4 + 2] as u32;
                let a = rgba[i * 4 + 3] as u32;
                // BGRA, premultiplied.
                pixels[i * 4] = ((b * a) / 255) as u8;
                pixels[i * 4 + 1] = ((g * a) / 255) as u8;
                pixels[i * 4 + 2] = ((r * a) / 255) as u8;
                pixels[i * 4 + 3] = a as u8;
            }
            Some(Icon { bitmap, w: width as i32, h: height as i32 })
        }
    }

    /// Blends one icon into the row's gutter, centred and square.
    fn draw_icon(hdc: HDC, gutter: RECT, icon: &Icon) {
        unsafe {
            let side = scale(ICON_SIDE);
            let x = gutter.left + ((gutter.right - gutter.left) - side) / 2;
            let y = gutter.top + ((gutter.bottom - gutter.top) - side) / 2;
            let mem = CreateCompatibleDC(hdc);
            let old = SelectObject(mem, icon.bitmap);
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = AlphaBlend(hdc, x, y, side, side, mem, 0, 0, icon.w, icon.h, blend);
            SelectObject(mem, old);
            let _ = DeleteDC(mem);
        }
    }

    /// Frees every decoded icon. Called from `open` before it decodes a new
    /// set and from `close` on the way out, so a card's DIBs never outlive it.
    fn drop_icons() {
        if let Ok(mut icons) = ICONS.lock() {
            for icon in icons.drain(..).flatten() {
                unsafe {
                    let _ = DeleteObject(icon.bitmap);
                }
            }
        }
    }

    /// Which control the pointer is over, as a control id, or 0.
    static HOVERED: AtomicIsize = AtomicIsize::new(0);

    /// The subclassed controls' original procedure.
    ///
    /// **One slot for every control**, unlike `unlock_prompt`'s slot per
    /// button: every control here is the same `BUTTON` class registered by the
    /// same comctl32, so the procedure it replaces is the same pointer -- and
    /// a slot per control would be `ROW_CAP + 2` statics that must all hold
    /// one value.
    static ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);

    // ---- the window --------------------------------------------------------

    pub(super) fn open(candidates: &[Candidate]) -> Option<PickerWindow> {
        register_fonts();
        GONE.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        MODE.store(MODE_LIST, Ordering::SeqCst);
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = ENTRIES.lock() {
            slot.clear();
        }

        // **The cap, and the slot it spends to say the list was cut.** See
        // `win32_draw::visible_rows`: a card that hid candidates without
        // saying so is the defect this project keeps finding, and this window
        // cannot scroll to show them.
        let (shown, overflow) = crate::win32_draw::visible_rows(candidates.len(), ROW_CAP);
        OVERFLOWING.store(overflow, Ordering::SeqCst);
        if let Ok(mut slot) = SHOWN.lock() {
            *slot = candidates[..shown].to_vec();
        }

        unsafe {
            DPI_PERCENT.store(
                {
                    let dc = GetDC(None);
                    let dpi = GetDeviceCaps(dc, LOGPIXELSX);
                    ReleaseDC(None, dc);
                    if dpi > 0 {
                        dpi * 100 / 96
                    } else {
                        100
                    }
                },
                Ordering::SeqCst,
            );
        }

        // The icons, decoded here and only blended in the paint path. Never
        // read from disk and never fetched from the network at any point on
        // this path -- see `make_icon`.
        drop_icons();
        if let (Ok(offers), Ok(mut icons)) = (super::OFFERS.lock(), ICONS.lock()) {
            for candidate in &candidates[..shown] {
                let png = offers
                    .iter()
                    .find(|o| o.candidate.id == candidate.id)
                    .and_then(|o| o.icon.as_deref());
                icons.push(png.and_then(make_icon));
            }
        }

        register_class();
        *FONTS.lock().ok()? = Some(Fonts::build());

        let l = super::layout();
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        // Centred on the primary work area rather than on the foreground
        // window, for `unlock_prompt::centred`'s reason: a card that jumped
        // around the desktop depending on which app happened to be in front is
        // one the user has to hunt for.
        let (x, y) = centred(w, h);

        let window = unsafe {
            CreateWindowExW(
                // Topmost, because it is a question asked over whatever the
                // user was doing. It takes focus deliberately: the rows are
                // answered with Tab and Enter as well as with the pointer.
                WS_EX_TOPMOST,
                CLASS_NAME,
                &HSTRING::from(PICKER_PROMPT_TITLE),
                // Frameless. A `WS_CAPTION` frame is the loudest "system
                // dialog" signal there is, and this app's own windows are
                // frameless with drawn chrome.
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
        .ok()?;

        round_corners(window);

        {
            let guard = FONTS.lock().ok()?;
            let fonts = guard.as_ref()?;

            // Every slot gets a control whether or not this list fills it: the
            // second step reuses the same controls for its own rows, and
            // creating them lazily would mean creating a window from inside a
            // repaint.
            for index in 0..ROW_CAP {
                let control = child(
                    window,
                    w!("BUTTON"),
                    WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                    super::row_at(index),
                    ID_ROW + index,
                    fonts.name,
                )?;
                subclass(control);
            }
            let secondary = child(
                window,
                w!("BUTTON"),
                WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                l.secondary,
                ID_SECONDARY,
                fonts.button,
            )?;
            let cancel = child(
                window,
                w!("BUTTON"),
                WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                l.cancel,
                ID_CANCEL,
                fonts.button,
            )?;
            subclass(secondary);
            subclass(cancel);
        }

        apply_mode(window);

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            // Allowed to refuse, and handled rather than asserted -- the
            // property `foreground` records. A refusal leaves a topmost card
            // on screen that the user clicks once to focus.
            let _ = SetForegroundWindow(window);
        }

        Some(PickerWindow(handle_of(window)))
    }

    /// **The protection, on the top-level window.**
    ///
    /// Applied to the card itself and never to a child: Windows refuses
    /// `SetWindowDisplayAffinity` on a child control with `E_INVALIDARG`, and
    /// the top-level flag covers every child it owns. What it protects is not
    /// a password -- there is none on this surface -- but *which accounts this
    /// user holds for the app they are in front of*, which is exactly the
    /// thing a screen recorder should not be handed.
    pub(super) fn protect(window: PickerWindow) -> bool {
        unsafe { SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    /// Pumps until the user does something.
    ///
    /// **This blocks.** It does not return until the window procedure has
    /// recorded an event or the window has gone away, and the event it hands
    /// back is *taken* out of `PENDING` rather than read from it -- so no
    /// event can be delivered twice. That is what [`super::run_with`]'s
    /// ignore-and-continue arms rest on: an implementation that returned the
    /// same ignorable event over and over would turn each of those `continue`s
    /// into a spin that filled the log.
    ///
    /// **`IsDialogMessageW` is what makes Tab, Shift+Tab, Space and Enter work
    /// at all.** A bare `TranslateMessage`/`DispatchMessage` pump around
    /// controls that are not in a dialog gives none of them. Escape is handled
    /// before it, because `IsDialogMessage` only cancels for a real dialog box.
    pub(super) fn next(window: PickerWindow) -> Event {
        use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
        use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;

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
                    if msg.message == WM_KEYDOWN && msg.wParam.0 as u16 == VK_ESCAPE.0 {
                        return Event::Cancel;
                    }
                    if !IsDialogMessageW(top, &msg).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if GONE.load(Ordering::SeqCst) {
                        return Event::Closed;
                    }
                    if let Some(event) = take_pending() {
                        return event;
                    }
                }
            }
            // Idle. Nothing on this card animates, so this is a plain wait for
            // the next message rather than a frame tick.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    fn take_pending() -> Option<Event> {
        PENDING.lock().ok().and_then(|mut slot| slot.take())
    }

    fn set_pending(event: Event) {
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(event);
        }
    }

    /// Swaps the card to its second step: the chosen account's fields.
    pub(super) fn show_palette(window: PickerWindow, palette: &Palette) {
        if let Ok(mut slot) = ENTRIES.lock() {
            *slot = super::palette_rows(palette);
        }
        MODE.store(MODE_PALETTE, Ordering::SeqCst);
        let top = hwnd(window.0);
        apply_mode(top);
    }

    /// Shows exactly the controls this step has rows for, hides the rest, and
    /// puts the keyboard on the first of them.
    ///
    /// **Hiding is what stops an empty slot being a clickable nothing.** A
    /// `BUTTON` left visible with no label is still a tab stop and still posts
    /// `BN_CLICKED`.
    fn apply_mode(window: HWND) {
        let count = visible_row_count();
        unsafe {
            for index in 0..ROW_CAP {
                if let Ok(control) = GetDlgItem(window, (ID_ROW + index) as i32) {
                    let _ = ShowWindow(control, if index < count { SW_SHOW } else { SW_HIDE });
                }
            }
            if let Ok(control) = GetDlgItem(window, ID_ROW as i32) {
                let _ = SetFocus(control);
            }
        }
        repaint(window);
    }

    /// How many row controls this step is using.
    fn visible_row_count() -> usize {
        if MODE.load(Ordering::SeqCst) == MODE_PALETTE {
            ENTRIES.lock().map(|e| e.len()).unwrap_or(0).min(ROW_CAP)
        } else {
            let rows = SHOWN.lock().map(|s| s.len()).unwrap_or(0);
            let overflow = usize::from(OVERFLOWING.load(Ordering::SeqCst));
            (rows + overflow).min(ROW_CAP)
        }
    }

    pub(super) fn close(window: PickerWindow) {
        unsafe {
            let _ = DestroyWindow(hwnd(window.0));
        }
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
        drop_icons();
        if let Ok(mut slot) = SHOWN.lock() {
            slot.clear();
        }
        if let Ok(mut slot) = ENTRIES.lock() {
            slot.clear();
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
    }

    // ---- plumbing ----------------------------------------------------------

    fn handle_of(h: HWND) -> isize {
        h.0 as isize
    }

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut c_void)
    }

    fn repaint(window: HWND) {
        unsafe {
            let _ = InvalidateRect(window, None, false);
        }
    }

    fn centred(w: i32, h: i32) -> (i32, i32) {
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
            if ok.is_err() || area.right <= area.left {
                return (200, 200);
            }
            (
                area.left + (area.right - area.left - w) / 2,
                // Slightly above centre, where every OS credential prompt puts
                // itself: a card the eye has to find sits better a little high.
                area.top + (area.bottom - area.top - h) * 2 / 5,
            )
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
                // No background brush: `WM_ERASEBKGND` is answered and the
                // whole client area is painted from one back buffer, which is
                // what keeps the card from flashing system grey on a repaint.
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
    }

    /// One child control. It is created with **no text**: every label on this
    /// card is painted by `paint_control` from the app's own palette and type,
    /// so a control's own caption would only ever be a second, stale copy.
    fn child(
        parent: HWND,
        class: PCWSTR,
        style: u32,
        at: Box2,
        id: usize,
        font: HFONT,
    ) -> Option<HWND> {
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!(""),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                scale(at.x),
                scale(at.y),
                scale(at.w),
                scale(at.h),
                parent,
                HMENU(id as *mut c_void),
                None,
                None,
            )
        }
        .ok()?;
        unsafe {
            SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        }
        Some(h)
    }

    fn round_corners(window: HWND) {
        // The same `DWMWCP_ROUND` the login window's frameless chrome asks
        // for, so every surface in this app has the same silhouette.
        unsafe {
            use windows::Win32::Graphics::Dwm::{
                DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            };
            let preference = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                window,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &preference as *const _ as *const c_void,
                std::mem::size_of_val(&preference) as u32,
            );
        }
    }

    /// Takes over a control's painting without losing the focus and keyboard
    /// behaviour that makes `IsDialogMessage` work.
    fn subclass(control: HWND) {
        unsafe {
            let previous =
                SetWindowLongPtrW(control, GWLP_WNDPROC, control_proc as *const () as isize);
            if previous != 0 {
                ORIGINAL_PROC.store(previous, Ordering::SeqCst);
            }
        }
    }

    // ---- the window procedures ---------------------------------------------

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
            // Frameless windows are dragged by their background.
            WM_NCHITTEST => {
                let hit = DefWindowProcW(window, msg, wparam, lparam);
                if hit.0 == 1 {
                    LRESULT(HTCAPTION as isize)
                } else {
                    hit
                }
            }
            WM_LBUTTONDOWN => {
                if in_close_glyph(lparam) {
                    set_pending(Event::Cancel);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                // A pointer that left a control without entering another one is
                // seen here rather than by the control it left.
                if HOVERED.swap(0, Ordering::SeqCst) != 0 {
                    repaint(window);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as i32;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                if notification == BN_CLICKED {
                    clicked(id as usize);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                GONE.store(true, Ordering::SeqCst);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }

    /// **What a click on control `id` means**, which is the only place the two
    /// steps differ in behaviour.
    ///
    /// A row past the end of this step's list is ignored rather than answered:
    /// the control is hidden there, so it can be reached by neither pointer
    /// nor Tab, and inventing an answer for it would be inventing a choice the
    /// user did not make.
    fn clicked(id: usize) {
        let palette = MODE.load(Ordering::SeqCst) == MODE_PALETTE;
        if id == ID_CANCEL {
            set_pending(Event::Cancel);
            return;
        }
        if id == ID_SECONDARY {
            set_pending(if palette { Event::EditSelected } else { Event::NewLogin });
            return;
        }
        if id < ID_ROW {
            return;
        }
        let index = id - ID_ROW;
        if index >= visible_row_count() {
            return;
        }
        if palette {
            if let Some(send) = ENTRIES.lock().ok().and_then(|e| e.get(index).cloned()) {
                set_pending(Event::Sends(send));
            }
            return;
        }
        let shown = SHOWN.lock().map(|s| s.len()).unwrap_or(0);
        if index >= shown {
            // The slot `win32_draw::visible_rows` spent so that a truncated
            // list says it was truncated.
            set_pending(Event::Overflow);
        } else {
            set_pending(Event::Chose(index));
        }
    }

    /// The subclassed controls: everything except painting and hover is the
    /// original `BUTTON` procedure's, which is what keeps focus, the space bar
    /// and `IsDialogMessage`'s traversal working.
    unsafe extern "system" fn control_proc(
        control: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let id = GetWindowLongPtrW(control, windows::Win32::UI::WindowsAndMessaging::GWLP_ID);
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint_control(control, id as usize);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if HOVERED.swap(id, Ordering::SeqCst) != id {
                    repaint(control);
                }
                LRESULT(0)
            }
            _ => {
                let original = ORIGINAL_PROC.load(Ordering::SeqCst);
                if original == 0 {
                    DefWindowProcW(control, msg, wparam, lparam)
                } else {
                    CallWindowProcW(
                        Some(std::mem::transmute::<
                            isize,
                            unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
                        >(original)),
                        control,
                        msg,
                        wparam,
                        lparam,
                    )
                }
            }
        }
    }

    fn in_close_glyph(lparam: LPARAM) -> bool {
        let l = super::layout();
        let x = (lparam.0 & 0xffff) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
        x >= scale(l.close_glyph.x)
            && x < scale(l.close_glyph.right())
            && y >= scale(l.close_glyph.y)
            && y < scale(l.close_glyph.bottom())
    }

    // ---- painting ----------------------------------------------------------

    /// The card's own surface: the heading pair, the list's card and the close
    /// glyph. Every row and every button is a child control that paints itself.
    fn paint(window: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(window, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(window, &mut client);
            let (w, h) = (client.right, client.bottom);

            // Double-buffered, for `unlock_prompt::paint`'s reason: a surface
            // painted straight to the window flickers on every hover.
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp);

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            fill(mem, client, crate::theme::WINDOW_BG);
            SetBkMode(mem, TRANSPARENT);

            let l = super::layout();
            // The card the rows sit on, so a row's own white reads as part of
            // one surface rather than as five floating strips.
            rounded(mem, l.list, 8, crate::theme::CARD, Some((1, crate::theme::HAIRLINE)));

            if let Some(fonts) = fonts {
                let palette = MODE.load(Ordering::SeqCst) == MODE_PALETTE;
                text(
                    mem,
                    fonts.title,
                    l.title,
                    if palette { "What should I type?" } else { "Fill from vault" },
                    crate::theme::INK,
                );
                text(
                    mem,
                    fonts.subtitle,
                    l.subtitle,
                    if palette {
                        "Pick a field. Nothing is typed until you do."
                    } else {
                        "These accounts look like they belong to this app."
                    },
                    crate::theme::TEXT_FAINT,
                );
            }

            paint_close_glyph(mem, l.close_glyph);

            drop(guard);
            let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(window, &ps);
        }
    }

    /// One child control: a row in either step, or one of the two footer
    /// buttons.
    fn paint_control(control: HWND, id: usize) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(control, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(control, &mut rc);

            let hovered = HOVERED.load(Ordering::SeqCst) == id as isize;
            let focused = GetFocus() == control;

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            let whole = RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom };

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            if id >= ID_ROW {
                // The row sits on the list's card, so anything `draw_row` does
                // not cover is that card's white. `draw_row` fills the whole
                // rect edge to edge, which is what keeps a hover from hugging
                // just the text.
                fill(mem, whole, crate::theme::CARD);
                if let Some(fonts) = fonts {
                    // **Focus is drawn as selection.** These rows are reached
                    // by Tab as readily as by the pointer, and a focused row
                    // with no mark on it is a keyboard user pressing Enter on
                    // a card that never said which account they were on.
                    paint_row(
                        mem,
                        whole,
                        id - ID_ROW,
                        RowState { selected: focused, hovered },
                        fonts,
                    );
                }
            } else {
                // The footer sits on the window's own background, not on the
                // card -- otherwise the button's rounded corners show system
                // grey through them.
                fill(mem, whole, crate::theme::WINDOW_BG);
                SetBkMode(mem, TRANSPARENT);
                let skin =
                    if hovered { ButtonSkin::secondary().hovered() } else { ButtonSkin::secondary() };
                if let Some(fonts) = fonts {
                    let label = footer_label(id);
                    if focused {
                        rounded(
                            mem,
                            Box2 { x: 0, y: 0, w: rc.right, h: rc.bottom },
                            8,
                            crate::theme::FOCUS_RING,
                            None,
                        );
                        let inner = RECT {
                            left: whole.left + 2,
                            top: whole.top + 2,
                            right: whole.right - 2,
                            bottom: whole.bottom - 2,
                        };
                        draw_button(mem, inner, &label, fonts.button, skin, scale(7));
                    } else {
                        draw_button(mem, whole, &label, fonts.button, skin, scale(7));
                    }
                }
            }
            drop(guard);

            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(control, &ps);
        }
    }

    fn footer_label(id: usize) -> String {
        if id == ID_CANCEL {
            return "Cancel".to_string();
        }
        if MODE.load(Ordering::SeqCst) == MODE_PALETTE {
            "Edit binding".to_string()
        } else {
            "New login".to_string()
        }
    }

    /// One row, in whichever step is showing.
    ///
    /// Both steps go through [`crate::win32_draw::draw_row`] rather than one
    /// of them growing its own painter: the edge-to-edge highlight is the
    /// property that function exists to hold, and a second row painter is a
    /// second place for it to be got wrong.
    fn paint_row(hdc: HDC, rect: RECT, index: usize, state: RowState, fonts: &Fonts) {
        if MODE.load(Ordering::SeqCst) == MODE_PALETTE {
            let Some(send) = ENTRIES.lock().ok().and_then(|e| e.get(index).cloned()) else {
                return;
            };
            let (name, says) = super::send_label(&send);
            let row = Candidate { id: String::new(), name, username: says.to_string() };
            draw_row(hdc, rect, &row, state, fonts.name, fonts.username);
            return;
        }

        let shown = SHOWN.lock().map(|s| s.clone()).unwrap_or_default();
        if let Some(candidate) = shown.get(index) {
            draw_row(hdc, rect, candidate, state, fonts.name, fonts.username);
            // The gutter `draw_row` deliberately leaves blank.
            if let Ok(icons) = ICONS.lock() {
                if let Some(Some(icon)) = icons.get(index) {
                    let gutter = RECT {
                        left: rect.left,
                        top: rect.top,
                        right: rect.left + (rect.bottom - rect.top),
                        bottom: rect.bottom,
                    };
                    draw_icon(hdc, gutter, icon);
                }
            }
            return;
        }
        // The overflow row: the slot `win32_draw::visible_rows` spends so that
        // a truncated list says it was truncated.
        let row = Candidate {
            id: String::new(),
            name: "Search the vault".to_string(),
            username: "More accounts match than fit on this card".to_string(),
        };
        draw_row(hdc, rect, &row, state, fonts.name, fonts.username);
    }

    /// The header's close glyph, drawn as two strokes because no bundled face
    /// has it at this weight.
    fn paint_close_glyph(hdc: HDC, at: Box2) {
        unsafe {
            use windows::Win32::Graphics::Gdi::{LineTo, MoveToEx};
            let pen = CreatePen(PS_SOLID, scale(1).max(1), rgb(crate::theme::TEXT_FAINT));
            let old = SelectObject(hdc, pen);
            let (x, y, w, h) = (scale(at.x), scale(at.y), scale(at.w), scale(at.h));
            let pad = w / 3;
            let _ = MoveToEx(hdc, x + pad, y + pad, None);
            let _ = LineTo(hdc, x + w - pad, y + h - pad);
            let _ = MoveToEx(hdc, x + w - pad, y + pad, None);
            let _ = LineTo(hdc, x + pad, y + h - pad);
            SelectObject(hdc, old);
            let _ = DeleteObject(pen);
        }
    }

    fn fill(hdc: HDC, rc: RECT, colour: eframe::egui::Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(colour));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
        }
    }

    /// A rounded rectangle in logical coordinates, optionally stroked.
    fn rounded(
        hdc: HDC,
        at: Box2,
        radius: i32,
        fill_colour: eframe::egui::Color32,
        border: Option<(i32, eframe::egui::Color32)>,
    ) {
        unsafe {
            let brush = CreateSolidBrush(rgb(fill_colour));
            let (width, colour) = border.unwrap_or((1, fill_colour));
            let pen = CreatePen(PS_SOLID, scale(width).max(1), rgb(colour));
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, pen);
            let r = scale(radius) * 2;
            let _ =
                RoundRect(hdc, scale(at.x), scale(at.y), scale(at.right()), scale(at.bottom()), r, r);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush);
            let _ = DeleteObject(pen);
        }
    }

    /// One run of text, left-aligned and vertically centred in `at`.
    fn text(hdc: HDC, font: HFONT, at: Box2, run: &str, colour: eframe::egui::Color32) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(colour));
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            let mut chars: Vec<u16> = run.encode_utf16().collect();
            // `DT_NOPREFIX`: these are the app's own words, and an `&` in one
            // of them is an ampersand rather than a mnemonic.
            DrawTextW(hdc, &mut chars, &mut rc, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
            SelectObject(hdc, old);
        }
    }
}

/// **The card's own content decisions**, which are the ones that can be made
/// without a window: which rows the second step offers, what each is called,
/// and what a log line is allowed to say about one.
#[cfg(test)]
mod card_tests {
    use super::*;

    fn palette(fields: Vec<FieldRef>, has_sequence: bool) -> Palette {
        Palette { fields, has_sequence }
    }

    #[test]
    fn an_item_with_a_stored_sequence_offers_that_and_nothing_else() {
        let rows = palette_rows(&palette(vec![FieldRef::Username, FieldRef::Password], true));
        assert_eq!(
            rows,
            vec![Send::Sequence],
            "the user wrote the sequence precisely because the generic rows were not what that \
             app wanted, so offering them back is offering the thing they rejected -- the same \
             decision `app::fill_choices` makes"
        );
    }

    #[test]
    fn both_credentials_put_the_pair_first_and_then_each_alone() {
        assert_eq!(
            palette_rows(&palette(vec![FieldRef::Username, FieldRef::Password], false)),
            vec![
                Send::All,
                Send::Field(FieldRef::Username),
                Send::Field(FieldRef::Password),
            ]
        );
    }

    #[test]
    fn one_credential_alone_is_not_offered_as_a_pair() {
        assert_eq!(
            palette_rows(&palette(vec![FieldRef::Password], false)),
            vec![Send::Field(FieldRef::Password)],
            "`Send::All` types a Tab between two values; offered for an item that has only one, \
             it would type the password into whatever field followed the empty username"
        );
    }

    /// **The bound that makes a card with no scrolling honest.**
    ///
    /// `field_palette` is unbounded -- an item may carry any number of custom
    /// fields -- and this card cannot grow or scroll, so a row past
    /// [`ROW_CAP`] is one the user can neither see nor reach. The rows are
    /// therefore built from the three fields that are bounded, plus at most a
    /// pair row, which is four; the custom fields are covered by the sequence
    /// builder, exactly as `app::fill_choices` records.
    #[test]
    fn a_wall_of_custom_fields_cannot_push_a_row_off_the_card() {
        let customs: Vec<FieldRef> = (0..40)
            .map(|i| FieldRef::Custom(format!("field {i}")))
            .chain([FieldRef::Username, FieldRef::Password, FieldRef::Totp])
            .collect();
        let rows = palette_rows(&palette(customs, false));
        assert!(
            rows.len() <= ROW_CAP,
            "the second step offered {} rows onto a card with room for {ROW_CAP}, and this card \
             does not scroll -- the rest would simply be unreachable",
            rows.len()
        );
        assert_eq!(
            rows,
            vec![
                Send::All,
                Send::Field(FieldRef::Username),
                Send::Field(FieldRef::Password),
                Send::Field(FieldRef::Totp),
            ]
        );
    }

    /// The label on screen defers to `FieldRef::label`, so a field renamed
    /// there cannot end up named two different things in two parts of this UI.
    #[test]
    fn a_field_is_called_on_screen_what_the_rest_of_the_app_calls_it() {
        let (label, _) = send_label(&Send::Field(FieldRef::Totp));
        assert_eq!(label, FieldRef::Totp.label());
        let (custom, _) = send_label(&Send::Field(FieldRef::Custom("Recovery PIN".to_string())));
        assert_eq!(custom, "Recovery PIN", "on the button the user is looking at, the name is right");
    }

    /// **And the log line is not the button.**
    ///
    /// The same value described for a diagnostic never spells the custom
    /// field's name. See [`describe_send`] for why the crate's one precedent
    /// for spelling it -- a refusal shown to that user about their own item --
    /// does not extend to a line written into a file on disk.
    #[test]
    fn a_log_line_never_spells_a_custom_fields_name() {
        assert_eq!(
            describe_send(&Send::Field(FieldRef::Custom("Recovery PIN".to_string()))),
            "a custom field"
        );
        // Controls: the built-in fields ARE named, so the line above is a
        // deliberate omission rather than a function that says nothing useful.
        assert_eq!(describe_send(&Send::Field(FieldRef::Password)), "the password");
        assert_eq!(describe_send(&Send::All), "username, Tab and password");
    }

    /// **Every control the card lays out is inside the window it lays out.**
    ///
    /// The card neither scrolls nor resizes, so a control whose bottom edge
    /// fell past the window's would simply be invisible -- and the last row is
    /// the one that would go first.
    #[test]
    fn nothing_the_card_lays_out_falls_off_the_bottom_of_it() {
        let l = layout();
        assert!(l.subtitle.bottom() <= l.list.y);
        assert!(l.list.bottom() <= l.cancel.y);
        assert!(l.cancel.bottom() <= l.window.bottom());
        assert!(l.secondary.right() < l.cancel.x, "the two footer buttons overlap");
        assert!(l.secondary.x >= 0, "the footer runs off the left edge of the card");
        let last = row_at(ROW_CAP - 1);
        assert!(
            last.bottom() <= l.list.bottom(),
            "the last row is outside the list area, and this card cannot scroll to it"
        );
        assert!(l.close_glyph.right() <= l.window.right());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_sequence::{FieldRef, Token};

    fn one(name: &str) -> Vec<Candidate> {
        vec![Candidate {
            id: "id-1".to_string(),
            name: name.to_string(),
            username: "me@example.com".to_string(),
        }]
    }

    #[test]
    fn all_types_username_tab_password_and_never_presses_enter() {
        let tokens = tokens_for(&Send::All, None);
        let tab = crate::key_sequence::key_named("TAB").expect("TAB is a known key");
        assert_eq!(
            tokens,
            vec![
                Token::Field(FieldRef::Username),
                Token::Key(tab),
                Token::Field(FieldRef::Password),
            ],
            "a trailing Enter submits, and if the target's field order differs from this \
             assumption it submits the wrong content -- typing without submitting fails \
             visibly, submitting fails invisibly"
        );
    }

    #[test]
    fn one_field_is_one_token_and_nothing_else() {
        assert_eq!(
            tokens_for(&Send::Field(FieldRef::Totp), None),
            vec![Token::Field(FieldRef::Totp)]
        );
    }

    #[test]
    fn the_sequence_choice_runs_the_items_own_sequence() {
        let tokens = tokens_for(&Send::Sequence, Some("{USERNAME}{TAB}{PASSWORD}{ENTER}"));
        assert_eq!(
            tokens,
            crate::key_sequence::parse("{USERNAME}{TAB}{PASSWORD}{ENTER}"),
            "the configured sequence goes through the existing parser, not a second \
             interpretation of the same string"
        );
    }

    #[test]
    fn choosing_a_row_then_a_field_answers_that_item_and_that_field() {
        let calls = PickerCalls {
            open: |_| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| {
                use std::sync::atomic::{AtomicUsize, Ordering};
                static STEP: AtomicUsize = AtomicUsize::new(0);
                match STEP.fetch_add(1, Ordering::SeqCst) {
                    0 => Event::Chose(0),
                    _ => Event::Sends(Send::Field(FieldRef::Password)),
                }
            },
            show_palette: |_, _| {},
            close: |_| {},
        };
        let outcome = run_with(&calls, &one("Slack"), |_| Palette {
            fields: vec![FieldRef::Password],
            has_sequence: false,
        });
        assert_eq!(
            outcome,
            Outcome::Fill { id: "id-1".to_string(), send: Send::Field(FieldRef::Password) }
        );
    }

    #[test]
    fn the_window_is_protected_before_it_is_ever_pumped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ORDER: AtomicUsize = AtomicUsize::new(0);
        static PROTECTED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        static PUMPED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = PickerCalls {
            open: |_| Some(PickerWindow(1)),
            protect: |_| {
                PROTECTED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                true
            },
            next: |_| {
                // Record only the FIRST pump. If every pump overwrote this,
                // the last write would win and the assertion below would
                // only mean "protect happened before the final pump" -- which
                // passes even if an earlier pump ran before protect.
                let _ = PUMPED_AT.compare_exchange(
                    usize::MAX,
                    ORDER.fetch_add(1, Ordering::SeqCst),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                Event::Cancel
            },
            show_palette: |_, _| {},
            close: |_| {},
        };
        let _ = run_with(&calls, &one("Slack"), |_| Palette { fields: vec![], has_sequence: false });
        assert!(
            PROTECTED_AT.load(Ordering::SeqCst) < PUMPED_AT.load(Ordering::SeqCst),
            "a window that can be typed into before it is excluded from capture is a window a \
             recorder can catch a keystroke in"
        );
    }

    #[test]
    fn closing_the_window_closes_it_and_fills_nothing() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CLOSED: AtomicBool = AtomicBool::new(false);
        let calls = PickerCalls {
            open: |_| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| Event::Closed,
            show_palette: |_, _| {},
            close: |_| CLOSED.store(true, Ordering::SeqCst),
        };
        assert_eq!(
            run_with(&calls, &one("Slack"), |_| Palette { fields: vec![], has_sequence: false }),
            Outcome::Cancelled
        );
        assert!(CLOSED.load(Ordering::SeqCst), "close runs on every exit path");
    }

    #[test]
    fn a_window_that_cannot_be_opened_is_unavailable_and_not_a_silent_nothing() {
        let calls = PickerCalls {
            open: |_| None,
            protect: |_| true,
            next: |_| Event::Cancel,
            show_palette: |_, _| {},
            close: |_| {},
        };
        assert_eq!(
            run_with(&calls, &one("Slack"), |_| Palette { fields: vec![], has_sequence: false }),
            Outcome::Unavailable
        );
    }

    #[test]
    fn the_fill_path_also_closes_the_window() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        static CLOSED: AtomicBool = AtomicBool::new(false);
        static STEP: AtomicUsize = AtomicUsize::new(0);
        let calls = PickerCalls {
            open: |_| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Chose(0),
                _ => Event::Sends(Send::Field(FieldRef::Password)),
            },
            show_palette: |_, _| {},
            close: |_| CLOSED.store(true, Ordering::SeqCst),
        };
        let outcome = run_with(&calls, &one("Slack"), |_| Palette {
            fields: vec![FieldRef::Password],
            has_sequence: false,
        });
        assert_eq!(
            outcome,
            Outcome::Fill { id: "id-1".to_string(), send: Send::Field(FieldRef::Password) }
        );
        assert!(
            CLOSED.load(Ordering::SeqCst),
            "the Fill path is the one that most needs close -- the window's lifetime bounds an \
             un-wipeable copy of typed text"
        );
    }

    #[test]
    fn choosing_a_row_shows_the_palette_it_was_given() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static STEP: AtomicUsize = AtomicUsize::new(0);
        static SHOWN_FIELDS: std::sync::Mutex<Vec<FieldRef>> = std::sync::Mutex::new(Vec::new());
        static SHOWN_HAS_SEQUENCE: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = PickerCalls {
            open: |_| Some(PickerWindow(1)),
            protect: |_| true,
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Chose(0),
                _ => Event::Cancel,
            },
            show_palette: |_, palette| {
                *SHOWN_FIELDS.lock().unwrap() = palette.fields.clone();
                SHOWN_HAS_SEQUENCE.store(palette.has_sequence as usize, Ordering::SeqCst);
            },
            close: |_| {},
        };
        let outcome = run_with(&calls, &one("Slack"), |id| {
            assert_eq!(id, "id-1");
            Palette { fields: vec![FieldRef::Totp], has_sequence: true }
        });
        assert_eq!(outcome, Outcome::Cancelled);
        assert_eq!(*SHOWN_FIELDS.lock().unwrap(), vec![FieldRef::Totp]);
        assert_eq!(SHOWN_HAS_SEQUENCE.load(Ordering::SeqCst), 1);
    }
}

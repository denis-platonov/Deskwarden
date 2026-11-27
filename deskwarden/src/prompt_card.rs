//! **Design 2a: the matched-item prompt, in bare Win32.**
//!
//! The card the daemon puts beside a password field when the vault *does*
//! have a login for the app in front of the user. It names the account that
//! is about to be typed, offers the ways it can be typed, and gets out of the
//! way.
//!
//! This is the fifth surface in this crate drawn with `CreateWindowExW` and
//! GDI rather than with egui -- after `crate::unlock_prompt`,
//! `crate::picker_prompt`, `crate::generate_prompt` and `crate::locked_card`
//! -- and it is one for the same measured reason they are.
//!
//! # Why it is not an egui window any more
//!
//! The tray daemon measures 9.9 MB with no window ever opened. The moment any
//! egui window opens it becomes ~60 MB resident and **never returns**: the
//! OpenGL driver's committed arenas survive the window's destruction and are
//! only reclaimed at process exit. The Win32 cards already in this crate
//! measure under 2 MB with their window on screen. **This card is the most
//! frequently opened surface in the whole product** -- it fires on every
//! matched fill -- so it paid that ~50 MB on the commonest thing the app
//! does, and paid it permanently.
//!
//! # It keeps its anchor, and that is a decision
//!
//! `crate::picker_prompt` dropped the no-match card's placement and
//! `crate::generate_prompt` dropped the generator's, both on the ground that
//! the daemon's cards should appear in one place rather than wherever the app
//! that raised them happens to be. **That argument does not reach this card.**
//! The picker and the generator are answers to something the user asked for by
//! chord or by click; this card appears *unbidden*, in response to a field
//! being focused, and the only thing that makes it legible as a reply to
//! **that** field rather than an interruption is that it is beside it. So
//! [`show_prompt_card`] still takes the anchor `crate::app::overlay_position`
//! computes, and [`place`] is the arithmetic that puts the card there -- pure,
//! and clamped against the work area by this module rather than by the caller,
//! because the size being clamped against is this card's own.
//!
//! # It takes the foreground, unlike the egui card it replaces
//!
//! The egui overlay deliberately did not raise, and its footer promised
//! `Enter Fill · Esc Dismiss` that Windows' foreground lock was free to never
//! deliver -- its own source says so ("Esc is not guaranteed to reach us at
//! all"). A card whose two advertised keys may not arrive is a card that
//! cannot be answered the way it says it can. So this one asks for the
//! foreground, the way `crate::unlock_prompt` does, and for that module's
//! reason: `injector::send_input::ensure_foreground` restores the target
//! window before any keystroke is sent, so the fill costs nothing for it. A
//! refusal is handled rather than asserted -- `foreground`'s own tests record
//! that `SetForegroundWindow` is allowed to say no -- and leaves a topmost
//! card the user clicks once.
//!
//! # No secret is on this card or through its seam
//!
//! **Usernames are shown; passwords never are.** The whole point of 2a is that
//! the user can recognise *which* account is about to be filled, and the
//! username is what identifies it. Nothing on [`Row`], [`Event`], [`Outcome`]
//! or [`PromptCalls`] can carry a password: this module holds no vault handle,
//! reads no item, and the fill itself happens in `crate::app` long after this
//! window is down.
//!
//! # The seam
//!
//! Mirrors `crate::generate_prompt` exactly: [`PromptCalls`] is a struct of
//! `fn` pointers and [`run_with`] is the whole decision, drivable by a test
//! with no window and no vault. `protect` runs immediately after `open` and
//! **before the first pump**, and `close` runs on every exit path including
//! the failures.

use crate::app::FillChoice;

/// What the card shows about the matched vault item: enough for the user to
/// recognize *which* credentials are about to be filled (design 2a shows the
/// username with the item name under it), without ever putting the password
/// itself on screen.
///
/// **It lived in `overlay_ui` until that module was deleted**, which is where
/// every card in this crate was drawn before they were ported to bare Win32.
/// It is here rather than in `app` because this is the module that paints it,
/// and a type whose only reader is a painter belongs beside the painter.
pub struct OverlayMatch {
    pub item_name: String,
    pub username: Option<String>,
}

/// The window handle [`run_with`] deals in.
///
/// A bare `isize` newtype, not an `HWND`, for the reason
/// `picker_prompt::PickerWindow` is: a decision layer a test can drive must
/// not name a type that only exists behind a Win32 feature gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptWindow(pub isize);

/// One row of the card, exactly as it is painted.
///
/// **Two lines and an avatar seed, and nothing else.** In particular no id and
/// no `FillChoice`: which choice a row answers is its *index*, resolved by
/// [`choice_at`] on the way out, so a row cannot be drawn carrying one thing
/// and answer with another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The row's own line: **what** pressing it will type.
    pub primary: String,
    /// **Who** is about to be typed -- the username, or the fallback when the
    /// item has none.
    ///
    /// Carried on every row although it is the same string on all of them,
    /// because the card's header block is built from row 0 and the seam
    /// [`run_with`] takes is a `&[Row]` and nothing else. It is also the text
    /// the initials tile is built from.
    ///
    /// **This is the regression this field exists to close.** The egui card
    /// this replaced led every row with the username, and the port replaced it
    /// with the fill choice -- so the card that exists so the user can
    /// recognise *which* credentials are about to be typed into their window
    /// stopped naming the account at all. Never a password: see the module
    /// header.
    pub account: String,
    /// The faint line under the account: the item, and the app it fills.
    pub context: String,
}

/// **How many rows the card has room for.**
///
/// `crate::app::fill_choices` is bounded at five by construction -- a saved
/// sequence, username + tab + password, username, password, one-time code,
/// and custom `{S:Field}` rows are deliberately absent precisely because an
/// unbounded row count is a geometry hazard for a card that cannot scroll.
/// **Six, not five**, because the remembered record's card
/// (`crate::app::recall_rows`) draws all of those and its way back under
/// them; at four it cut a record's password and one-time code off to make
/// room. This is that bound written where the geometry can be checked against
/// it, and [`the_row_cap_is_the_choice_lists_own_bound`] holds the two
/// together.
pub const ROW_CAP: usize = 6;

/// The rows the card paints, for this match and this choice list.
///
/// **Pure, and the whole of "what does the card say".** The egui card built
/// these two strings inside a `ui` closure no test could execute; here they
/// are a function of four `&str`s.
///
/// An **empty** `choices` paints the single matched-credential row the overlay
/// has always painted, whose primary line is the username (or the item name
/// when there is none). A non-empty one paints a row per choice, labelled by
/// [`FillChoice::label`], all of them naming the same account.
///
/// Truncated at [`ROW_CAP`] rather than growing the card: a row past the
/// bottom edge of a frameless window with no scrollbar is unreachable, and a
/// list that silently loses one is worse than one that never offered it. The
/// cap is not reachable today -- see [`ROW_CAP`] -- so this is the bound
/// stated as code rather than a state the app has.
pub fn rows(
    app_name: &str,
    item_name: &str,
    username: Option<&str>,
    choices: &[FillChoice],
) -> Vec<Row> {
    let (account, context) = account_lines(app_name, item_name, username);
    if choices.is_empty() {
        return vec![Row { primary: FillChoice::Saved.label(), account, context }];
    }
    choices
        .iter()
        .take(ROW_CAP)
        .map(|choice| Row {
            primary: choice.label(),
            account: account.clone(),
            context: context.clone(),
        })
        .collect()
}

/// **The card's account block**: who, and what it fills.
///
/// Lifted from the egui card unchanged, including the fallback for an item
/// that could not be read back from the vault at prompt time. It was that
/// card's ROW; here it is the header block above the choices, which is where
/// it has to be once the rows are labelled by what they type rather than by
/// whom -- one account named once, and the ways to type it under it.
pub fn account_lines(
    app_name: &str,
    item_name: &str,
    username: Option<&str>,
) -> (String, String) {
    match (username, item_name.is_empty()) {
        (Some(u), false) => (
            u.to_string(),
            crate::i18n::tf("prompt_card.item_fills", &[("item_name", item_name), ("app_name", app_name)]),
        ),
        (Some(u), true) => {
            (u.to_string(), crate::i18n::tf("prompt_card.fills", &[("app_name", app_name)]))
        }
        (None, false) => {
            (item_name.to_string(), crate::i18n::tf("prompt_card.fills", &[("app_name", app_name)]))
        }
        (None, true) => (
            crate::i18n::t("prompt_card.saved_credentials").to_string(),
            crate::i18n::tf("prompt_card.fills", &[("app_name", app_name)]),
        ),
    }
}

/// Which [`FillChoice`] row `index` answers.
///
/// **Out of range answers [`FillChoice::Saved`]**, which is not a fallback
/// invented here: it is exactly what the empty-choice card has always
/// answered, and the empty-choice card is the one case where an index has no
/// entry to look up.
pub fn choice_at(choices: &[FillChoice], index: usize) -> FillChoice {
    choices.get(index).cloned().unwrap_or(FillChoice::Saved)
}

/// Which row is drawn in the selected treatment, and which one Enter fills.
///
/// **One function for both**, so the row wearing the `Enter` chip is provably
/// the row Enter takes. They were two statements in the egui card and the
/// second lived where no test could reach it.
///
/// `focused` is the row the keyboard is on, or `None` when focus is elsewhere
/// -- in which case the primary row is the answer, which is what the card
/// opens on.
pub fn selected_row(focused: Option<usize>, count: usize) -> usize {
    match focused {
        Some(index) if index < count => index,
        _ => 0,
    }
}

/// What the user did with the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The header ✕, or Escape.
    Cancel,
    /// The window went away underneath us. Treated exactly as `Cancel`.
    Closed,
    /// A row was clicked, or Enter was pressed on it. Carries **which**.
    Pick(usize),
}

/// How [`run_with`] finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The user picked row `usize`. [`choice_at`] turns it into an answer.
    Fill(usize),
    /// The user dismissed the card. Nothing is armed.
    Cancelled,
    /// The window could not be put on screen at all. Distinguished from
    /// `Cancelled` because "the user said no" and "we could not ask" are
    /// different facts.
    Unavailable,
}

/// Opens the card for these rows at an anchor.
pub type OpenPromptFn = fn(&[Row], Option<(f32, f32)>) -> Option<PromptWindow>;

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything.
pub struct PromptCalls {
    /// Lays out and shows the card for these rows, at this anchor. `None` if
    /// it could not be put on screen.
    pub open: OpenPromptFn,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the **top-level**
    /// window, called before the first `next`. Windows refuses it on a child
    /// control with `E_INVALIDARG`, and the top-level flag covers every child
    /// it owns.
    pub protect: fn(PromptWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(PromptWindow) -> Event,
    /// Destroys the window and releases its resources.
    pub close: fn(PromptWindow),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// 1. `protect` runs immediately after `open` and before the first `next`.
///    What it hides is not a password but the list of accounts this user holds
///    for the app they are in front of, which is the thing a screen recorder
///    should not be handed.
/// 2. A pick past the end of the row list is refused rather than answered.
///    The window layer only ever posts an index it drew a control for, so this
///    is the invariant stated where it can fail rather than a case that
///    happens -- an out-of-range `Fill` would become a `FillChoice::Saved` in
///    [`choice_at`], i.e. a credential typed for a row the user never saw.
/// 3. `close` runs on every exit path. `open` answering `None` returns before
///    it, because there is no window to close there.
pub fn run_with(
    calls: &PromptCalls,
    rows: &[Row],
    anchor: Option<(f32, f32)>,
) -> Outcome {
    let Some(window) = (calls.open)(rows, anchor) else {
        log::warn!("the autofill prompt could not be put on screen");
        return Outcome::Unavailable;
    };

    if !(calls.protect)(window) {
        log::warn!(
            "SetWindowDisplayAffinity was refused for the autofill prompt; the account names it \
             shows are visible to screen capture on this machine"
        );
    }

    loop {
        match (calls.next)(window) {
            Event::Cancel | Event::Closed => {
                (calls.close)(window);
                return Outcome::Cancelled;
            }
            Event::Pick(index) => {
                if index >= rows.len() {
                    log::warn!(
                        "the autofill prompt answered with a row it never drew; ignoring it \
                         rather than filling from a row the user could not have seen"
                    );
                    continue;
                }
                (calls.close)(window);
                return Outcome::Fill(index);
            }
        }
    }
}

/// The window's title.
///
/// **Unique across this process**, and load-bearing rather than cosmetic:
/// `crate::foreground::pick` finds a window by title and takes the FIRST match
/// in `EnumWindows` order, and this card is up alongside the tray's and the
/// hotkey listener's helper windows.
///
/// The egui card it replaces opened under the bare literal `"Deskwarden"` --
/// the same title three other windows of this process open under, which is
/// exactly why that card could never be raised safely.
pub const PROMPT_CARD_TITLE: &str = "Deskwarden autofill";

/// **Puts design 2a on screen and answers which row the user picked** --
/// `None` if they dismissed it.
///
/// The same signature `overlay_ui::show_prompt_overlay` had, so
/// `crate::app::REAL_OVERLAY` changes by one path and nothing else: `matched`
/// is `None` when the item could not be read back from the vault at prompt
/// time, and `anchor` is the top-left corner `crate::app::overlay_position`
/// computed from where the field actually is.
pub fn show_prompt_card(
    app_name: &str,
    matched: Option<&OverlayMatch>,
    anchor: Option<(f32, f32)>,
    choices: &[FillChoice],
) -> Option<FillChoice> {
    ask_with(&REAL, app_name, matched, anchor, choices)
}

/// [`show_prompt_card`], told which [`PromptCalls`] to use.
///
/// `examples/prompt_preview.rs` is its one non-production caller, swapping
/// [`PromptCalls::protect`] for a stub so the window can be screenshotted.
pub fn ask_with(
    calls: &PromptCalls,
    app_name: &str,
    matched: Option<&OverlayMatch>,
    anchor: Option<(f32, f32)>,
    choices: &[FillChoice],
) -> Option<FillChoice> {
    let (item_name, username) = match matched {
        Some(m) => (m.item_name.as_str(), m.username.as_deref()),
        None => ("", None),
    };
    let rows = rows(app_name, item_name, username, choices);
    match run_with(calls, &rows, anchor) {
        Outcome::Fill(index) => Some(choice_at(choices, index)),
        Outcome::Cancelled | Outcome::Unavailable => None,
    }
}

/// The production [`PromptCalls`].
pub static REAL: PromptCalls = PromptCalls {
    open: win32::open,
    protect: win32::protect,
    next: win32::next,
    close: win32::close,
};

// ---------------------------------------------------------------------------
// Layout
//
// Logical pixels, at 100%, every one of them read off `theme` or off the
// Win32 cards this one sits beside. Numbers invented here would be a second
// layout that has to agree with a first, which is this codebase's standing
// defect shape.
// ---------------------------------------------------------------------------

/// The card's width, and so the window's: design 1b's Fill panel, 400.
/// [`crate::picker_prompt::WIDTH`] is the same number, because the two are the
/// same panel in two states and two frameless daemon cards of different widths
/// read as two different programs.
pub const WIDTH: i32 = crate::panel_draw::LIST_W;

/// The header's inset and top margin (design D 1.3: `pad 14 16 10`).
const MARGIN_X: i32 = crate::panel_draw::HEADER_PAD_X;
const MARGIN_TOP: i32 = crate::panel_draw::HEADER_PAD_TOP;

/// One choice row: an account tile with the design's padding round it. Each row
/// names **what pressing it will type** on its bold line and **whose** it is on
/// the faint line under it.
const ROW_H: i32 = crate::panel_draw::ROW_H;
/// The gap between two rows (D 1.3: `gap 2`).
const ROW_GAP: i32 = crate::panel_draw::ROW_GAP;
/// How far one row is from the next.
const ROW_PITCH: i32 = ROW_H + ROW_GAP;

/// The gap between the header and the list (D 1.3: `10`).
const HEADER_GAP: i32 = 10;
/// The gap between the `?` badge and the ✕ beside it.
const HEADER_BUTTON_GAP: i32 = 8;

/// The `Enter` chip the 0.15 footer drew, kept as the word the notice card's
/// button borrows.
pub const ENTER_SHORTCUT: &str = "ENTER";

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
/// Pure arithmetic with no Win32 in it, for `picker_prompt::layout`'s reason:
/// a control whose bottom edge fell past the window's would simply be
/// invisible on a window that neither scrolls nor resizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    /// The brand lockup's shield, and "Deskwarden" beside it. **Not optional**:
    /// a frameless always-on-top window that appears over another app's
    /// password field and offers to type into it has to say whose window it is.
    pub mark: Box2,
    pub wordmark: Box2,
    /// The `?` badge, and the key-hint tooltip it opens (D 1.3).
    pub help: Box2,
    pub tip: Box2,
    pub close_glyph: Box2,
    /// The whole list area. Individual rows are [`row_at`].
    pub list: Box2,
}

/// The card's geometry, for a list `rows` rows tall.
///
/// **Sized to the rows it will really draw**, and not to [`ROW_CAP`]: this
/// card has one shape decided once in `open` and never transitions.
///
/// `rows.max(1)`: a card with no rows is not a shorter card. The overlay
/// always paints at least one row -- with no choices it paints the matched
/// credential row it has always painted -- and a zero-row height would clip
/// that row off a window the user cannot scroll.
pub fn layout(rows: usize) -> Layout {
    let rows = rows.clamp(1, ROW_CAP) as i32;

    let lockup = crate::panel_draw::lockup();
    let mark = Box2 { x: MARGIN_X, y: MARGIN_TOP, w: lockup.mark_w, h: lockup.mark_h };
    let wordmark =
        Box2 { x: mark.right() + lockup.gap, y: MARGIN_TOP, w: lockup.word_w, h: lockup.mark_h };
    let close = crate::panel_draw::CLOSE;
    let close_glyph = Box2 {
        x: WIDTH - MARGIN_X - close,
        y: mark.y + (mark.h - close) / 2,
        w: close,
        h: close,
    };
    let badge = crate::panel_draw::HELP;
    let help = Box2 {
        x: close_glyph.x - HEADER_BUTTON_GAP - badge,
        y: mark.y + (mark.h - badge) / 2,
        w: badge,
        h: badge,
    };
    let (tx, ty, tw, th) = crate::panel_draw::tip_box(
        (help.x, help.y, help.right(), help.bottom()),
        WIDTH,
        crate::panel_draw::fill_key_hints().len(),
    );
    let tip = Box2 { x: tx, y: ty, w: tw, h: th };

    let list = Box2 {
        x: crate::panel_draw::LIST_PAD_X,
        y: mark.bottom() + HEADER_GAP,
        w: WIDTH - 2 * crate::panel_draw::LIST_PAD_X,
        h: ROW_H * rows + ROW_GAP * (rows - 1),
    };
    let window =
        Box2 { x: 0, y: 0, w: WIDTH, h: list.bottom() + crate::panel_draw::LIST_PAD_BOTTOM };

    Layout { window, mark, wordmark, help, tip, close_glyph, list }
}

/// Row `index` of a card laid out for `rows` rows.
pub fn row_at(rows: usize, index: usize) -> Box2 {
    let list = layout(rows).list;
    Box2 { x: list.x, y: list.y + ROW_PITCH * index as i32, w: list.w, h: ROW_H }
}

/// **Where the card actually goes**, given the anchor the caller computed and
/// the work area it has to fit inside.
///
/// Pure, and this module's rather than the caller's, because the size being
/// clamped against is this card's own: `crate::app::clamp_into_work_area`
/// clamps against the *egui* overlay's height, which is a different number
/// from this one, and a card clamped against somebody else's height is a card
/// whose last row can still end up under the taskbar.
///
/// `None` centres, slightly above the middle, where every OS credential prompt
/// puts itself -- which is what happens when the caller could not find the
/// field at all.
///
/// `.max(left)` / `.max(top)` after the `min`, for
/// `app::clamp_into_work_area`'s reason: on a work area smaller than the card,
/// the top-left corner is what survives, because the header and the first row
/// are worth more than the footer.
pub fn place(
    anchor: Option<(f32, f32)>,
    work: (i32, i32, i32, i32),
    w: i32,
    h: i32,
) -> (i32, i32) {
    let (left, top, right, bottom) = work;
    match anchor {
        Some((x, y)) => (
            (x as i32).min(right - w).max(left),
            (y as i32).min(bottom - h).max(top),
        ),
        None => (
            left + (right - left - w) / 2,
            top + (bottom - top - h) * 2 / 5,
        ),
    }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// Whether the window has gone away underneath the pump.
static GONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the window procedure recorded, taken by `next` rather than read, so no
/// event can be delivered twice.
static PENDING: std::sync::Mutex<Option<Event>> = std::sync::Mutex::new(None);

/// The rows the paint path draws. Written by `open`, read by every painter.
static ROWS: std::sync::Mutex<Vec<Row>> = std::sync::Mutex::new(Vec::new());

/// # Why every pixel here is painted by hand
///
/// `crate::unlock_prompt`'s `win32` module carries the whole argument and it
/// is not restated: a themed control renders in the shell's grey with the
/// shell's font. Every control here is a real `BUTTON` window -- which is what
/// buys focus, the space bar and `IsDialogMessage` traversal -- with its
/// painting taken over completely and handed to [`crate::win32_draw`], the
/// module every card draws through so none can drift from the palette.
///
/// # GDI only
///
/// Nothing here creates a Direct2D or Direct3D device. That is measured rather
/// than stylistic: an egui window was measured at ~102 MB and a D2D device at
/// 53.85 MB against the Win32 prompt's 1.79 MB.
///
/// # GDI object hygiene
///
/// Every brush, pen, font and DC created below is restored and deleted before
/// its function returns. This is a daemon's repaint path, and a leaked handle
/// here exhausts the table over a session rather than over a run.
mod win32 {
    use super::{
        Box2, Event, PromptWindow, Row, GONE, PENDING, PROMPT_CARD_TITLE, ROWS, ROW_CAP,
    };
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        CreateFontIndirectW, CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, FillRect,
        GetDC, GetDeviceCaps, InvalidateRect, ReleaseDC,
        SelectObject, SetBkMode, CLEARTYPE_QUALITY, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT,
        LOGFONTW, LOGPIXELSX, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetDlgItem, GetWindowLongPtrW, IsDialogMessageW, LoadCursorW, PeekMessageW,
        RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowDisplayAffinity,
        SetWindowLongPtrW, ShowWindow, TranslateMessage, BN_CLICKED, BS_PUSHBUTTON, CS_HREDRAW,
        CS_VREDRAW, GWLP_WNDPROC, HMENU, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW,
        WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_DESTROY,
        WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_QUIT, WM_SETFONT,
        WNDCLASSW, WS_CHILD, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
    };

    use crate::panel_draw;
    use crate::win32_draw::rgb;

    /// Row `i` is control `ID_ROW + i`.
    const ID_ROW: usize = 100;
    const CLASS_NAME: PCWSTR = w!("DeskwardenAutofillPrompt");

    /// The window's DPI as a percentage of 96, sampled once per open.
    ///
    /// **The system DPI, not the monitor's**, and a known limitation rather
    /// than an oversight -- `unlock_prompt`'s own `DPI_PERCENT` carries the
    /// whole argument: `GetDpiForWindow` lives behind a `windows` crate
    /// feature this crate does not enable, and enabling it re-pins
    /// `job_object.rs`'s whole-file hash of `Cargo.toml`.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    /// Which control the pointer is over, as a control id, or 0.
    static HOVERED: AtomicIsize = AtomicIsize::new(0);

    /// The subclassed controls' original procedure. One slot for all of them:
    /// every control here is the same `BUTTON` class registered by the same
    /// comctl32, so the procedure it replaces is the same pointer.
    static ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);

    /// How many rows the live card has. Read by the paint path and by the
    /// Enter handler, which has to know which control ids exist.
    static ROW_COUNT: AtomicI32 = AtomicI32::new(0);

    // ---- fonts -------------------------------------------------------------

    /// Registers every bundled face privately with GDI, once.
    ///
    /// **The loop that used to be here now lives in
    /// [`crate::win32_draw::register_fonts`], behind one process-wide
    /// `OnceLock` instead of one per card.** `AddFontMemResourceEx` copies the
    /// font data into the process font table rather than refcounting a shared
    /// buffer, so every card that carried its own copy of this loop handed GDI
    /// another private copy of all four Archivo cuts -- roughly 750 KB of pure
    /// duplicate per card opened in a session. Adding the four Noto Cyrillic
    /// cuts the cards now need for Cyrillic runs would have made that worse
    /// rather than better, so the registration moved to the one module every
    /// card already draws its text through.
    fn register_fonts() {
        crate::win32_draw::register_fonts();
    }

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

    /// Every face the card paints with, created at open and destroyed at
    /// close. Kept together so `close` cannot leak one by forgetting it.
    struct Fonts {
        /// The wordmark: "Deskwarden" at 15/800 (design D 1.3).
        brand: HFONT,
        /// A row's bold line, 16/700.
        name: HFONT,
        /// A row's faint line, 14.
        user: HFONT,
        /// The avatar's initials, 14/700.
        avatar: HFONT,
        /// The `Enter` badge, 13/700.
        badge: HFONT,
        /// The `?` badge, 12/800.
        help: HFONT,
        /// The tooltip, 14.
        tip: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, EXTRABOLD, REGULAR};
            Fonts {
                brand: font(EXTRABOLD, panel_draw::WORD_PX),
                name: font(BOLD, panel_draw::ROW_NAME_PX),
                user: font(REGULAR, panel_draw::ROW_SUB_PX),
                avatar: font(BOLD, panel_draw::ROW_SUB_PX),
                badge: font(BOLD, panel_draw::BADGE_PX),
                help: font(EXTRABOLD, panel_draw::PILL_PX),
                tip: font(REGULAR, panel_draw::TIP_PX),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in
                    [self.brand, self.name, self.user, self.avatar, self.badge, self.help, self.tip]
                {
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

    // ---- the window --------------------------------------------------------

    pub(super) fn open(rows: &[Row], anchor: Option<(f32, f32)>) -> Option<PromptWindow> {
        register_fonts();
        GONE.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        let drawn: Vec<Row> = rows.iter().take(ROW_CAP).cloned().collect();
        let count = drawn.len().max(1);
        if let Ok(mut slot) = ROWS.lock() {
            *slot = drawn;
        }
        ROW_COUNT.store(count as i32, Ordering::SeqCst);

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

        register_class();
        // **Destroy the previous set before overwriting it.** `Fonts` has no
        // `Drop` -- it holds raw `HFONT`s -- so assigning over a `Some` would
        // leak six fonts per `open` that ran without a matching `close`.
        {
            let mut slot = FONTS.lock().ok()?;
            if let Some(previous) = slot.take() {
                previous.destroy();
            }
            *slot = Some(Fonts::build());
        }

        let l = super::layout(count);
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        let (x, y) = super::place(anchor, work_area(), w, h);

        let window = unsafe {
            CreateWindowExW(
                // Topmost, because it appears over whatever the user was
                // doing. Frameless: a `WS_CAPTION` frame is the loudest
                // "system dialog" signal there is, and this app's own windows
                // are frameless with drawn chrome.
                WS_EX_TOPMOST,
                CLASS_NAME,
                &HSTRING::from(PROMPT_CARD_TITLE),
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

        // **Below this line the card is on screen.** `WS_VISIBLE` is in the
        // style, so a bare `?` here would return `None`, make `run_with`
        // answer `Unavailable`, and leave a frameless topmost card with no
        // controls and no way for the user to dismiss it -- `close` is only
        // reached with a `PromptWindow` in hand. Every failure path from here
        // on goes through `abandon`, which takes the window down and frees the
        // fonts before answering `None`.
        fn abandon(window: HWND) -> Option<PromptWindow> {
            unsafe {
                let _ = DestroyWindow(window);
            }
            if let Ok(mut slot) = FONTS.lock() {
                if let Some(fonts) = slot.take() {
                    fonts.destroy();
                }
            }
            if let Ok(mut slot) = ROWS.lock() {
                slot.clear();
            }
            ROW_COUNT.store(0, Ordering::SeqCst);
            None
        }

        // The handle is copied out and the guard dropped at the end of this
        // statement: `abandon` locks `FONTS` itself, so holding the guard
        // across the `child` calls below would deadlock the failure path.
        let Some(row_font) =
            FONTS.lock().ok().and_then(|guard| guard.as_ref().map(|f| f.name))
        else {
            return abandon(window);
        };

        for index in 0..count {
            let Some(control) = child(window, super::row_at(count, index), ID_ROW + index, row_font)
            else {
                return abandon(window);
            };
            subclass(control);
        }

        // The key-hint tooltip, created last so it is the topmost child, and
        // hidden until the pointer is on the `?`.
        let Some(tip_font) = FONTS.lock().ok().and_then(|guard| guard.as_ref().map(|f| f.tip))
        else {
            return abandon(window);
        };
        panel_draw::set_tip(tip_font, &panel_draw::fill_key_hints(), DPI_PERCENT.load(Ordering::SeqCst));
        if !panel_draw::create_tip(window, dev(l.tip)) {
            return abandon(window);
        }

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            // **Asked for, and allowed to refuse.** See the module doc: the
            // egui card this replaces advertised `Enter Fill · Esc Dismiss` on
            // a window that never took focus, so neither key was guaranteed to
            // arrive. A refusal leaves a topmost card the user clicks once.
            let _ = SetForegroundWindow(window);
            // The keyboard starts on the primary row, which is the row Enter
            // fills -- so the focus ring and the default action agree.
            if let Ok(control) = GetDlgItem(window, ID_ROW as i32) {
                let _ = SetFocus(control);
            }
        }

        Some(PromptWindow(handle_of(window)))
    }

    /// The monitor work area, as [`super::place`] wants it. A refusal answers
    /// a plausible desktop rather than nothing, because the alternative is a
    /// card placed at a negative coordinate.
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

    /// **The protection, on the top-level window.**
    ///
    /// Applied to the card itself and never to a child: Windows refuses
    /// `SetWindowDisplayAffinity` on a child control with `E_INVALIDARG`, and
    /// the top-level flag covers every child it owns. What it protects here is
    /// not a password -- this card shows none -- but the account names this
    /// user holds for the app they are in front of.
    pub(super) fn protect(window: PromptWindow) -> bool {
        if !crate::screen_share::capture_protection_enabled() {
            return true;
        }
        unsafe { SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    /// Pumps until the user does something.
    ///
    /// **This blocks**, and the event it hands back is *taken* out of
    /// `PENDING` rather than read from it, so no event can be delivered twice.
    ///
    /// **Enter fills the FOCUSED row, not always the first.** The card's rows
    /// are tab stops, so a user who tabs to the third row and presses Enter
    /// means the third row; an Enter hard-wired to the primary would fill
    /// credentials from a row they had visibly moved off. `super::selected_row`
    /// is the same function the painter uses to decide which row wears the
    /// `Enter` chip, so the chip is provably on the row Enter takes.
    ///
    /// **Up and Down move that focus** from row to row -- see
    /// [`arrow_to_row`] -- so a user can walk the list and press Enter without
    /// reaching for Tab.
    pub(super) fn next(window: PromptWindow) -> Event {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP,
        };
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
                    if msg.message == WM_KEYDOWN && msg.wParam.0 as u16 == VK_RETURN.0 {
                        return Event::Pick(focused_row(top));
                    }
                    // **Up and Down move the keyboard between the choice
                    // rows**, ahead of `IsDialogMessageW`: its own arrow
                    // traversal wraps round the ends, and the next row is a
                    // decision `row_after_arrow` makes once for both Win32
                    // cards. The row that now has focus is the one that wears
                    // the `Enter` chip and the one Enter takes.
                    if msg.message == WM_KEYDOWN
                        && (msg.wParam.0 as u16 == VK_DOWN.0 || msg.wParam.0 as u16 == VK_UP.0)
                    {
                        arrow_to_row(top, msg.wParam.0 as u16 == VK_DOWN.0);
                        continue;
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

    pub(super) fn close(window: PromptWindow) {
        unsafe {
            let _ = DestroyWindow(hwnd(window.0));
        }
        panel_draw::clear_tip();
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
        // Not a secret, but it is the name of an account this user holds and
        // the app they were in front of, and nothing needs either once the
        // card is down.
        if let Ok(mut slot) = ROWS.lock() {
            slot.clear();
        }
        ROW_COUNT.store(0, Ordering::SeqCst);
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

    fn row_count() -> usize {
        ROW_COUNT.load(Ordering::SeqCst).max(1) as usize
    }

    /// The row control that has the keyboard, or `None` when focus is
    /// anywhere else.
    fn focus_index(window: HWND) -> Option<usize> {
        let focus = unsafe { GetFocus() };
        (0..row_count()).find(|index| {
            matches!(
                unsafe { GetDlgItem(window, (ID_ROW + index) as i32) },
                Ok(control) if control == focus
            )
        })
    }

    /// Which row the keyboard is on, as [`super::selected_row`] wants it.
    fn focused_row(window: HWND) -> usize {
        super::selected_row(focus_index(window), row_count())
    }

    /// **Up or Down: the keyboard moves to the neighbouring choice row.**
    ///
    /// The destination is [`crate::win32_draw::row_after_arrow`]'s, so it
    /// clamps at both ends and is always a row -- this card has no footer
    /// control for an arrow to land on. Every row is repainted afterwards
    /// rather than trusting the focus change to: the painter reads
    /// `GetFocus()` to put the selected treatment and the `Enter` chip on a
    /// row, and the row that lost the keyboard has to lose both with it.
    fn arrow_to_row(window: HWND, down: bool) {
        let count = row_count();
        let current = focus_index(window);
        let to = crate::win32_draw::row_after_arrow(current, count, down);
        if current == Some(to) {
            return;
        }
        unsafe {
            if let Ok(control) = GetDlgItem(window, (ID_ROW + to) as i32) {
                let _ = SetFocus(control);
            }
        }
        repaint_all(window);
    }

    fn take_pending() -> Option<Event> {
        PENDING.lock().ok().and_then(|mut slot| slot.take())
    }

    fn set_pending(event: Event) {
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(event);
        }
    }

    /// The rows the paint path draws.
    fn view() -> Vec<Row> {
        ROWS.lock().map(|slot| slot.clone()).unwrap_or_default()
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

    /// One child control, created with **no text**: every label on this card
    /// is painted by `paint_control` from the app's own palette and type, so a
    /// control's own caption would only ever be a second, stale copy.
    fn child(parent: HWND, at: Box2, id: usize, font: HFONT) -> Option<HWND> {
        let h = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                w!(""),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32),
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
                // **The close glyph is the one part of the background that is
                // not a title bar.** It is painted by this window rather than
                // being a child control, so answering `HTCAPTION` for the whole
                // client area turned every press on it into a window drag and
                // `WM_LBUTTONDOWN` below never fired -- the reported "clicking
                // on X doesn't work". See `win32_draw::frameless_hit`, which is
                // the pure half of this and the half the pin decides.
                crate::win32_draw::frameless_hit_test(
                    window,
                    DefWindowProcW(window, msg, wparam, lparam),
                    lparam,
                    close_glyph_rect(),
                )
            }
            WM_LBUTTONDOWN => {
                if in_close_glyph(lparam) {
                    set_pending(Event::Cancel);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                // A pointer that left a control without entering another one
                // is seen here rather than by the control it left.
                if HOVERED.swap(0, Ordering::SeqCst) != 0 {
                    repaint_all(window);
                }
                // The `?` badge is painted by this window, so its hover is
                // decided here: the tooltip is up exactly while the pointer is
                // on it.
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                let on_badge = crate::win32_draw::on_close_glyph(x, y, help_rect());
                panel_draw::help_pointer(window, on_badge);
                LRESULT(0)
            }
            panel_draw::WM_MOUSELEAVE => {
                panel_draw::set_help(window, false);
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = wparam.0 & 0xffff;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                if notification == BN_CLICKED && id >= ID_ROW && id - ID_ROW < row_count() {
                    set_pending(Event::Pick(id - ID_ROW));
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // **NO `PostQuitMessage` HERE, EVER.** This window is opened
                // on the daemon thread, and that thread goes on to run egui
                // windows -- the vault window, the preferences window, the
                // save-a-login form. `close()` calls `DestroyWindow`, which
                // dispatches this message synchronously on that thread, so a
                // `PostQuitMessage` here leaves the thread's quit flag set
                // with nothing left to drain it: `next()` has already returned
                // and no pump of ours runs again. The next
                // `eframe::run_native` then takes that stale `WM_QUIT` out of
                // `GetMessageW`, leaves its loop before it draws a frame, and
                // returns its default answer -- so the window never appears
                // and whatever the user asked for is silently dropped.
                //
                // Quitting is not this handler's job in the first place:
                // `GONE` on the line below is what `next()` reads to report
                // `Event::Closed`, and the `WM_QUIT` branch in `next()` stays
                // for a quit posted from outside.
                GONE.store(true, Ordering::SeqCst);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }

    /// The card and every row on it.
    fn repaint_all(window: HWND) {
        repaint(window);
        unsafe {
            for index in 0..row_count() {
                if let Ok(control) = GetDlgItem(window, (ID_ROW + index) as i32) {
                    repaint(control);
                }
            }
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
                // A row under the pointer is not the `?` badge: take the
                // tooltip down.
                panel_draw::help_pointer_left_badge(control);
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

    /// The close glyph's rect in DEVICE pixels.
    ///
    /// One derivation, read by both the hit test and `in_close_glyph`, so the
    /// rect `WM_NCHITTEST` excuses from the drag and the rect `WM_LBUTTONDOWN`
    /// answers on can never be two different rectangles.
    fn close_glyph_rect() -> RECT {
        let l = super::layout(row_count());
        RECT {
            left: scale(l.close_glyph.x),
            top: scale(l.close_glyph.y),
            right: scale(l.close_glyph.right()),
            bottom: scale(l.close_glyph.bottom()),
        }
    }

    fn in_close_glyph(lparam: LPARAM) -> bool {
        crate::win32_draw::on_close_glyph(
            (lparam.0 & 0xffff) as i16 as i32,
            ((lparam.0 >> 16) & 0xffff) as i16 as i32,
            close_glyph_rect(),
        )
    }

    /// The `?` badge's rect in DEVICE pixels, for the hover test.
    fn help_rect() -> RECT {
        dev(super::layout(row_count()).help)
    }

    fn dev(b: Box2) -> RECT {
        RECT {
            left: scale(b.x),
            top: scale(b.y),
            right: scale(b.right()),
            bottom: scale(b.bottom()),
        }
    }

    // ---- painting ----------------------------------------------------------

    /// The card's own surface: the header. Every row paints itself.
    fn paint(window: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(window, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(window, &mut client);
            let (w, h) = (client.right, client.bottom);

            // Double-buffered: a surface painted straight to the window
            // flickers on every hover.
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp);

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());
            let count = row_count();
            let l = super::layout(count);

            // The window IS the card, so its whole client area is
            // `theme::CARD`.
            fill_rect(mem, client, crate::gdi_palette::card());
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                let dev = |b: Box2| RECT {
                    left: scale(b.x),
                    top: scale(b.y),
                    right: scale(b.right()),
                    bottom: scale(b.bottom()),
                };
                panel_draw::draw_lockup(mem, dev(l.mark), dev(l.wordmark), fonts.brand);
                panel_draw::draw_help_badge(
                    mem,
                    dev(l.help),
                    fonts.help,
                    panel_draw::help_hot(),
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

    /// One row.
    ///
    /// Drawn by [`crate::win32_draw::draw_row`] -- the crate's row painter,
    /// the same one the account picker's candidates go through -- so this
    /// card's two lines truncate with an ellipsis and stop short of their
    /// keyboard chip exactly as the picker's do. Its `Candidate` argument is
    /// the two lines and nothing else; the id it also carries is not used by
    /// the painter and this card has none to give it.
    fn paint_control(control: HWND, id: usize) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(control, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(control, &mut rc);

            let index = id.saturating_sub(ID_ROW);
            let hovered = HOVERED.load(Ordering::SeqCst) == id as isize;
            let focused = GetFocus() == control;
            let count = row_count();
            let selected = super::selected_row(focused.then_some(index), count) == index;

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            let whole = RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom };

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());
            let rows = view();
            let dpi = DPI_PERCENT.load(Ordering::SeqCst);

            fill_rect(mem, whole, crate::gdi_palette::card());
            SetBkMode(mem, TRANSPARENT);

            if let (Some(fonts), Some(row)) = (fonts, rows.get(index)) {
                paint_choice(mem, whole, row, selected, hovered, fonts, dpi);
            }
            drop(guard);

            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(control, &ps);
        }
    }

    /// **One choice row, as design 1b draws an account row**: the account's
    /// tile, a bold line saying what pressing the row will type, the account
    /// under it, and on the selected row the blue `Enter` badge. The painting is
    /// [`panel_draw::draw_account_row`], which the account picker's rows go
    /// through too, so the two cards cannot drift.
    fn paint_choice(
        hdc: HDC,
        rect: RECT,
        row: &Row,
        selected: bool,
        hovered: bool,
        fonts: &Fonts,
        dpi: i32,
    ) {
        let faces = panel_draw::RowFonts {
            name: fonts.name,
            sub: fonts.user,
            avatar: fonts.avatar,
            badge: fonts.badge,
            // This card's rows carry no key chip: Enter fills the selected one.
            chip: fonts.badge,
        };
        panel_draw::draw_account_row(
            hdc,
            rect,
            panel_draw::RowLook {
                tile: panel_draw::Tile::Initials(&row.account, 0),
                name: &row.primary,
                sub: &row.account,
                selected,
                hovered,
                chip: None,
            },
            &faces,
            dpi,
        );
    }

    /// The header's close glyph, drawn as two strokes because no bundled face
    /// has it at this weight.
    fn paint_close_glyph(hdc: HDC, at: Box2) {
        panel_draw::draw_close_x(hdc, dev(at), scale(1).max(1), crate::gdi_palette::text_faint());
    }

    fn fill_rect(hdc: HDC, rc: RECT, colour: eframe::egui::Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(colour));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_sequence::FieldRef;

    const APP: &str = "Ledgerline";

    fn matched() -> OverlayMatch {
        OverlayMatch {
            item_name: "Ledgerline Desktop".to_string(),
            username: Some("ada@example.com".to_string()),
        }
    }

    /// A [`PromptCalls`] whose every pointer does nothing, for a test to
    /// override the one it is about.
    fn inert() -> PromptCalls {
        PromptCalls {
            open: |_, _| Some(PromptWindow(1)),
            protect: |_| true,
            next: |_| Event::Cancel,
            close: |_| {},
        }
    }

    fn one_row() -> Vec<Row> {
        rows(APP, "Ledgerline Desktop", Some("ada@example.com"), &[])
    }

    // ---- the decision ------------------------------------------------------

    #[test]
    fn the_window_is_protected_before_it_is_ever_pumped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ORDER: AtomicUsize = AtomicUsize::new(0);
        static PROTECTED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        static PUMPED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = PromptCalls {
            protect: |_| {
                PROTECTED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                true
            },
            next: |_| {
                // Record only the FIRST pump. If every pump overwrote this,
                // the last write would win and the assertion below would only
                // mean "protect happened before the final pump".
                let _ = PUMPED_AT.compare_exchange(
                    usize::MAX,
                    ORDER.fetch_add(1, Ordering::SeqCst),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                Event::Cancel
            },
            ..inert()
        };
        let _ = run_with(&calls, &one_row(), None);
        assert!(
            PROTECTED_AT.load(Ordering::SeqCst) < PUMPED_AT.load(Ordering::SeqCst),
            "this card names the accounts this user holds for the app they are in front of; a \
             window that can be read before it is excluded from capture is a window a recorder \
             catches that list in"
        );
    }

    #[test]
    fn a_window_that_cannot_be_opened_is_unavailable_and_not_a_silent_nothing() {
        let calls = PromptCalls { open: |_, _| None, ..inert() };
        assert_eq!(run_with(&calls, &one_row(), None), Outcome::Unavailable);
    }

    #[test]
    fn every_exit_path_closes_the_window() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CLOSED: AtomicUsize = AtomicUsize::new(0);

        // one-off test table; an alias would only be read here
        #[allow(clippy::type_complexity)]
        let paths: [(fn(PromptWindow) -> Event, Outcome); 3] = [
            (|_| Event::Cancel, Outcome::Cancelled),
            (|_| Event::Closed, Outcome::Cancelled),
            (|_| Event::Pick(0), Outcome::Fill(0)),
        ];
        for (next, expected) in paths {
            CLOSED.store(0, Ordering::SeqCst);
            let calls = PromptCalls {
                next,
                close: |_| {
                    CLOSED.fetch_add(1, Ordering::SeqCst);
                },
                ..inert()
            };
            assert_eq!(run_with(&calls, &one_row(), None), expected);
            assert_eq!(
                CLOSED.load(Ordering::SeqCst),
                1,
                "{expected:?} left the window on screen -- a frameless topmost card with no way \
                 out"
            );
        }
    }

    /// **The row that was clicked is the row that answers.**
    ///
    /// Four rows are four ways to do one thing if the answer is always the
    /// first, and the whole reason this card offers a list is that the user
    /// wants a particular one of them.
    #[test]
    fn the_row_that_was_picked_is_the_row_that_answers() {
        let choices = vec![
            FillChoice::UserTabPass,
            FillChoice::Just(FieldRef::Username),
            FillChoice::Just(FieldRef::Password),
        ];
        let drawn = rows(APP, "Ledgerline Desktop", Some("ada@example.com"), &choices);
        for index in 0..drawn.len() {
            static WANTED: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            WANTED.store(index, std::sync::atomic::Ordering::SeqCst);
            let calls = PromptCalls {
                next: |_| {
                    Event::Pick(WANTED.load(std::sync::atomic::Ordering::SeqCst))
                },
                ..inert()
            };
            assert_eq!(run_with(&calls, &drawn, None), Outcome::Fill(index));
            assert_eq!(choice_at(&choices, index), choices[index]);
        }
    }

    /// **A pick the card never drew fills nothing.**
    ///
    /// [`choice_at`] answers `FillChoice::Saved` for an index with no entry --
    /// which is right for the empty-choice card and would be a credential
    /// typed for a row the user never saw anywhere else.
    #[test]
    fn a_row_the_card_never_drew_cannot_be_filled_from() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static STEP: AtomicUsize = AtomicUsize::new(0);
        STEP.store(0, Ordering::SeqCst);
        let calls = PromptCalls {
            next: |_| match STEP.fetch_add(1, Ordering::SeqCst) {
                0 => Event::Pick(7),
                _ => Event::Cancel,
            },
            ..inert()
        };
        assert_eq!(
            run_with(&calls, &one_row(), None),
            Outcome::Cancelled,
            "a one-row card answered with row 7"
        );
    }

    /// **The row the user picked is the `FillChoice` the caller is handed.**
    ///
    /// The one end-to-end run of the module's public entry point: `matched`
    /// goes in, a row index comes back out of the seam, and what the caller
    /// gets is the choice that row was drawn for. Everything between is the
    /// two pure functions above, which is exactly why this can be run at all
    /// on a surface no test may open.
    #[test]
    fn the_answer_the_caller_gets_is_the_choice_the_picked_row_was_drawn_for() {
        let choices = vec![
            FillChoice::UserTabPass,
            FillChoice::Just(FieldRef::Password),
        ];
        let calls = PromptCalls { next: |_| Event::Pick(1), ..inert() };
        assert_eq!(
            ask_with(&calls, APP, Some(&matched()), None, &choices),
            Some(FillChoice::Just(FieldRef::Password))
        );

        // A dismissal authorises nothing, which is the whole reason this
        // answers an `Option` rather than a `FillChoice`.
        let calls = PromptCalls { next: |_| Event::Cancel, ..inert() };
        assert_eq!(ask_with(&calls, APP, Some(&matched()), None, &choices), None);

        // And a card with no choices at all answers the item's own saved
        // sequence -- the row the overlay has always painted.
        let calls = PromptCalls { next: |_| Event::Pick(0), ..inert() };
        assert_eq!(
            ask_with(&calls, APP, Some(&matched()), None, &[]),
            Some(FillChoice::Saved)
        );
    }

    /// **The anchor the caller computed is the anchor the window is opened
    /// at**, which is the whole of this card's placement decision.
    #[test]
    fn the_card_opens_at_the_anchor_it_was_given() {
        static SEEN: std::sync::Mutex<Option<Option<(f32, f32)>>> =
            std::sync::Mutex::new(None);
        let calls = PromptCalls {
            open: |_, anchor| {
                if let Ok(mut slot) = SEEN.lock() {
                    *slot = Some(anchor);
                }
                Some(PromptWindow(1))
            },
            ..inert()
        };
        let _ = run_with(&calls, &one_row(), Some((640.0, 480.0)));
        assert_eq!(
            *SEEN.lock().unwrap(),
            Some(Some((640.0, 480.0))),
            "the placement did not reach the window. This card appears unbidden, in response to \
             a field being focused, and beside that field is the only thing that makes it legible \
             as a reply to it rather than an interruption"
        );
    }

    // ---- the rows ----------------------------------------------------------

    /// **The card names the account, whatever it is offering.**
    ///
    /// This is the regression: the port labelled every row by the fill choice
    /// and dropped the username from the card entirely, on the one surface
    /// whose stated purpose is that the user can recognise WHICH credentials
    /// are about to be typed into their window before they are typed.
    #[test]
    fn every_row_carries_the_username_so_the_user_knows_whose_password_this_is() {
        let every = vec![
            FillChoice::UserTabPass,
            FillChoice::Just(FieldRef::Username),
            FillChoice::Just(FieldRef::Password),
            FillChoice::Just(FieldRef::Totp),
        ];
        for choices in [Vec::new(), every] {
            let drawn = rows(APP, "Ledgerline Desktop", Some("ada@example.com"), &choices);
            assert!(!drawn.is_empty());
            for row in &drawn {
                assert_eq!(
                    row.account, "ada@example.com",
                    "a row of this card does not name the account it is offering"
                );
                assert_eq!(row.context, "Ledgerline Desktop · fills Ledgerline");
            }
        }
    }

    #[test]
    fn a_card_with_no_choices_still_says_what_pressing_it_will_type() {
        let drawn = rows(APP, "Ledgerline Desktop", Some("ada@example.com"), &[]);
        assert_eq!(drawn.len(), 1);
        assert_eq!(
            drawn[0].primary,
            FillChoice::Saved.label(),
            "the empty-choice card answers `FillChoice::Saved` -- see `choice_at` -- so its one \
             row has to be labelled with what that will type"
        );
    }

    #[test]
    fn the_account_line_falls_back_to_the_item_and_then_to_a_neutral_line() {
        assert_eq!(
            rows(APP, "Ledgerline Desktop", None, &[])[0].account,
            "Ledgerline Desktop"
        );
        assert_eq!(rows(APP, "", None, &[])[0].account, "Saved credentials");
        assert_eq!(rows(APP, "", Some("ada"), &[])[0].context, "fills Ledgerline");
    }

    /// **Every choice row names the same account**, and is labelled by what it
    /// will type.
    #[test]
    fn every_choice_row_names_the_account_and_is_labelled_by_what_it_types() {
        let choices = vec![FillChoice::UserTabPass, FillChoice::Just(FieldRef::Totp)];
        let drawn = rows(APP, "Ledgerline Desktop", Some("ada@example.com"), &choices);
        assert_eq!(drawn.len(), 2);
        for (row, choice) in drawn.iter().zip(&choices) {
            assert_eq!(row.primary, choice.label());
            assert_eq!(row.context, "Ledgerline Desktop · fills Ledgerline");
            assert_eq!(
                row.account, "ada@example.com",
                "the initials tile names WHO is filled; initials of what is TYPED name nothing"
            );
        }
    }

    /// **No password is on any of these strings, whatever the item was.**
    #[test]
    fn nothing_the_card_says_can_be_a_password() {
        let choices = vec![
            FillChoice::UserTabPass,
            FillChoice::Just(FieldRef::Username),
            FillChoice::Just(FieldRef::Password),
            FillChoice::Just(FieldRef::Totp),
        ];
        let drawn = rows(APP, "Ledgerline Desktop", Some("ada@example.com"), &choices);
        let printed = format!("{drawn:?}");
        for forbidden in ["hunter2", "correct-horse"] {
            assert!(!printed.contains(forbidden));
        }
        assert!(
            printed.contains("ada@example.com"),
            "control: the rows under test printed nothing recognisable at all: {printed}"
        );
    }

    /// **The card's row cap is the choice list's own bound.**
    #[test]
    fn the_row_cap_is_the_choice_lists_own_bound() {
        // A record with a saved sequence, a username, a password and a
        // one-time code: the longest list `app::fill_choices` can build.
        let widest = vec![
            FillChoice::Saved,
            FillChoice::UserTabPass,
            FillChoice::Just(FieldRef::Username),
            FillChoice::Just(FieldRef::Password),
            FillChoice::Just(FieldRef::Totp),
        ];
        assert_eq!(
            widest.len() + 1,
            ROW_CAP,
            "`app::fill_choices` can offer {} rows, the remembered record's card adds its way \
             back under them, and the card lays out {ROW_CAP}. A row past the bottom edge of a \
             frameless window with no scrollbar is unreachable",
            widest.len()
        );
        assert_eq!(
            rows(APP, "x", Some("y"), &widest).len(),
            widest.len(),
            "the matched app's card dropped one of the record's values"
        );
        // And a longer list is truncated rather than allowed to grow the card.
        let mut over = widest.clone();
        over.extend([FillChoice::Saved, FillChoice::Saved]);
        assert_eq!(rows(APP, "x", Some("y"), &over).len(), ROW_CAP);
    }

    /// **The row wearing the `Enter` chip is the row Enter fills.**
    #[test]
    fn the_selected_row_is_the_one_enter_takes() {
        assert_eq!(selected_row(None, 3), 0, "with focus elsewhere Enter takes the primary");
        assert_eq!(selected_row(Some(2), 3), 2);
        assert_eq!(
            selected_row(Some(9), 3),
            0,
            "a focus index the card has no row for must not select one it does not have"
        );
    }

    // ---- placement ---------------------------------------------------------

    /// **The card is clamped against its own height**, so its last row cannot
    /// end up under the taskbar.
    #[test]
    fn a_card_anchored_at_the_bottom_of_the_work_area_is_pulled_back_onto_it() {
        let work = (0, 0, 1920, 1040);
        let l = layout(ROW_CAP);
        let (x, y) = place(Some((1900.0, 1030.0)), work, l.window.w, l.window.h);
        assert!(
            x + l.window.w <= 1920,
            "the card's right edge is off the work area at {}",
            x + l.window.w
        );
        assert!(
            y + l.window.h <= 1040,
            "the card's bottom edge is at {}, past the work area's 1040 -- and this window has no \
             scrollbar and no title bar to drag",
            y + l.window.h
        );
    }

    #[test]
    fn an_anchor_that_fits_is_left_exactly_where_it_was_asked_for() {
        let l = layout(1);
        assert_eq!(place(Some((300.0, 200.0)), (0, 0, 1920, 1040), l.window.w, l.window.h), (300, 200));
    }

    #[test]
    fn a_work_area_smaller_than_the_card_keeps_the_header_and_the_first_row() {
        let l = layout(ROW_CAP);
        let (x, y) = place(Some((50.0, 50.0)), (10, 20, 200, 100), l.window.w, l.window.h);
        assert_eq!(
            (x, y),
            (10, 20),
            "on a work area smaller than the card the top-left corner is what survives: the \
             header and the first row are worth more than the footer"
        );
    }

    #[test]
    fn no_anchor_centres_the_card_rather_than_pinning_it_to_a_corner() {
        let l = layout(1);
        let (x, y) = place(None, (0, 0, 1920, 1040), l.window.w, l.window.h);
        assert!(x > 0 && y > 0 && x + l.window.w < 1920 && y + l.window.h < 1040);
        assert_eq!(x, (1920 - l.window.w) / 2);
    }

    // ---- geometry ----------------------------------------------------------

    /// **Nothing the card lays out falls off it, at any row count.**
    ///
    /// This window is frameless, always-on-top, unresizable and has **no
    /// scroll area anywhere**, so a control past an edge is not merely awkward
    /// -- it is unreachable, on the surface whose rows are the only way the
    /// credential the user is looking at gets typed.
    #[test]
    fn nothing_the_card_lays_out_falls_off_it() {
        let mut checked = 0;
        for count in 1..=ROW_CAP {
            let l = layout(count);
            assert_eq!(l.window.w, WIDTH);
            assert_eq!((l.window.x, l.window.y), (0, 0));

            // **The brand lockup**, which a card that opens unbidden over
            // another app's password field has to carry.
            let lockup = crate::panel_draw::lockup();
            assert_eq!(
                (l.mark.x, l.mark.y),
                (MARGIN_X, MARGIN_TOP),
                "the lockup does not start at the card's own top-left inset"
            );
            assert_eq!(l.mark.h, lockup.mark_h);
            assert_eq!(
                l.mark.w,
                crate::win32_draw::mark_width(l.mark.h),
                "the mark's box is not the design artboard's ratio, so the shield would be \
                 letterboxed inside it and drift away from the word beside it"
            );
            assert!(l.mark.right() < l.wordmark.x, "the wordmark is drawn over the shield");
            assert_eq!(l.wordmark.h, l.mark.h, "the lockup's two halves are different heights");
            assert!(l.wordmark.right() <= l.help.x, "the wordmark runs under the `?` badge");

            // The header's two buttons sit on the lockup's line, inside the
            // card's margin, and do not touch.
            assert!(l.help.right() < l.close_glyph.x, "the `?` badge touches the close glyph");
            assert!(
                l.close_glyph.right() <= l.window.right() - MARGIN_X,
                "the close glyph has crossed the card's right margin"
            );
            assert_eq!(l.help.w, crate::panel_draw::HELP);
            assert_eq!(l.help.w, l.help.h, "the `?` badge is not a circle");

            // The tooltip lies on the card and clears the badge it explains.
            assert!(l.tip.x >= 0 && l.tip.right() <= l.window.right(), "the tooltip left the card");
            assert!(l.tip.right() <= l.help.x, "the tooltip covers its own badge");
            assert!(
                l.tip.bottom() <= l.window.bottom(),
                "a {count}-row card is too short to hold the key-hint tooltip"
            );

            assert!(l.mark.bottom() <= l.list.y, "the list runs into the header");
            assert_eq!(l.list.x, crate::panel_draw::LIST_PAD_X);
            assert_eq!(
                l.list.right(),
                WIDTH - crate::panel_draw::LIST_PAD_X,
                "the list does not sit inside the card's margins"
            );
            assert_eq!(
                l.list.h,
                ROW_H * count as i32 + ROW_GAP * (count as i32 - 1),
                "the list is not sized to the rows it will draw"
            );
            assert_eq!(
                l.window.bottom(),
                l.list.bottom() + crate::panel_draw::LIST_PAD_BOTTOM,
                "the card has a band under its last row that is not the design's padding"
            );

            // Every row is inside the list, one gap apart, and none overlaps.
            for index in 0..count {
                let row = row_at(count, index);
                assert_eq!(row.x, l.list.x);
                assert_eq!(row.w, l.list.w);
                assert_eq!(row.h, ROW_H);
                assert!(row.y >= l.list.y, "row {index} starts above the list");
                assert!(
                    row.bottom() <= l.list.bottom(),
                    "row {index} of {count} ends at {} px, past the list's {} px -- so it is off \
                     a card that cannot scroll",
                    row.bottom(),
                    l.list.bottom()
                );
                if index > 0 {
                    assert_eq!(
                        row.y,
                        row_at(count, index - 1).bottom() + ROW_GAP,
                        "rows {} and {index} overlap or are not one design gap apart",
                        index - 1
                    );
                }
            }
            checked += 1;
        }
        assert_eq!(checked, ROW_CAP);
    }

    /// A row is the design's account row: its tile and the padding round it.
    #[test]
    fn a_row_is_the_designs_account_row() {
        assert_eq!(ROW_H, 64);
        assert_eq!(ROW_GAP, 2);
        assert_eq!(WIDTH, 400, "design D 1.3's Fill panel is 400 wide");
    }

    /// **The window grows by exactly one row per row**, so a card asked for
    /// four rows is not a one-row card with three of them off the bottom.
    #[test]
    fn each_row_costs_the_window_exactly_one_row_of_height() {
        for count in 2..=ROW_CAP {
            assert_eq!(
                layout(count).window.h - layout(count - 1).window.h,
                ROW_PITCH,
                "the {count}-row card is not one row taller than the {}-row one",
                count - 1
            );
        }
        assert_eq!(
            layout(0).window.h,
            layout(1).window.h,
            "a card with no choices still paints the matched-credential row, so it is not a \
             shorter card"
        );
    }

    /// **The card's dimensions are the theme's and the crate's**, so a
    /// redesign cannot leave this card drawing controls of its own invented
    /// size.
    #[test]
    fn the_cards_dimensions_are_the_crates() {
        assert_eq!(
            WIDTH,
            crate::picker_prompt::WIDTH,
            "the daemon's Win32 cards are different widths, which reads as two different \
             programs answering the same event"
        );
    }

    /// **The card says every one of its own words**, and each of them is a
    /// constant rather than a literal at the paint site -- which is the only
    /// reason a test can read them at all on a surface no test may open.
    #[test]
    fn the_cards_words_are_the_ones_it_promises() {
        assert_eq!(ENTER_SHORTCUT, "ENTER");
        // The panel's own key hints, which the `?` badge opens.
        assert_eq!(crate::panel_draw::enter_label(), "Enter");
        assert!(crate::panel_draw::fill_key_hints().iter().any(|l| l.contains("Esc")));
    }

    /// **The title is this window's own.**
    ///
    /// The egui card this replaces opened under the bare `"Deskwarden"` three
    /// other windows of this process also open under, which is exactly why it
    /// could never be raised safely.
    #[test]
    fn the_window_opens_under_a_title_nothing_else_uses() {
        assert!(!PROMPT_CARD_TITLE.is_empty());
        assert_ne!(PROMPT_CARD_TITLE, "Deskwarden");
        assert_ne!(PROMPT_CARD_TITLE, crate::picker_prompt::PICKER_PROMPT_TITLE);
        assert_ne!(PROMPT_CARD_TITLE, crate::unlock_prompt::UNLOCK_PROMPT_TITLE);
        assert_ne!(PROMPT_CARD_TITLE, crate::generate_prompt::GENERATE_PROMPT_TITLE);
        assert_ne!(PROMPT_CARD_TITLE, crate::vault_window::WINDOW_TITLE);
    }

    // ---- source pins -------------------------------------------------------

    /// The production half of this file: everything before the first column-0
    /// `#[cfg(test)]`, with line endings normalised first because this
    /// repository checks out CRLF.
    fn production() -> (String, usize) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("prompt_card.rs");
        let raw = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let cut = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap().to_string();
        let discarded = raw.len() - cut.len();
        (cut, discarded)
    }

    /// The production half with comments stripped, so a rule that forbids a
    /// call does not also forbid explaining why the call is not there.
    fn code(source: &str) -> String {
        source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_prompt_window_never_posts_a_thread_quit() {
        let (production, discarded) = production();
        let code = code(&production);

        // CONTROLS, so a pin that scanned nothing cannot pass.
        assert!(
            discarded > 0,
            "control: the `#[cfg(test)]` cut marker was not found, so this scan is reading the \
             test module as production and the rule below is meaningless"
        );
        assert!(
            code.contains("WM_DESTROY =>"),
            "control: the production cut does not contain the window procedure's WM_DESTROY arm, \
             so the cut is in the wrong place"
        );
        assert!(
            code.contains("GONE.store(true, Ordering::SeqCst);"),
            "control: the comment stripper has eaten code -- the WM_DESTROY arm's one surviving \
             statement is not in the text this rule scans"
        );

        assert!(
            !code.contains(concat!("PostQuit", "Message")),
            "prompt_card.rs's production half posts a thread quit. This window is opened on the \
             daemon thread, and that thread goes on to run egui windows. `close()` calls \
             `DestroyWindow`, which dispatches WM_DESTROY synchronously on that thread, and \
             nothing drains the queue afterwards: `next()` has already returned. The next \
             `eframe::run_native` takes the stale WM_QUIT out of `GetMessageW`, leaves its loop \
             before it draws, and returns its DEFAULT answer -- so the window never appears. \
             `GONE` is what `next()` reads; quitting the thread is not this window's job."
        );
    }

    /// **The capture exclusion goes on the top-level window, and once.**
    ///
    /// Windows refuses `SetWindowDisplayAffinity` on a child control with
    /// `E_INVALIDARG`, so a call aimed at one of this card's row `BUTTON`s
    /// would fail silently and leave the account list capturable.
    #[test]
    fn the_capture_exclusion_goes_on_the_top_level_window() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert_eq!(
            code.matches("SetWindowDisplayAffinity(").count(),
            1,
            "this card names the accounts this user holds and excludes itself from screen \
             capture other than exactly once"
        );
        assert!(
            code.contains("SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE)"),
            "the exclusion is not applied to the top-level window this module was handed. \
             Windows refuses it on a child control with E_INVALIDARG, and the top-level flag \
             covers every child it owns"
        );
        assert_eq!(
            code.matches("SetForegroundWindow(").count(),
            1,
            "this card advertises `Enter Fill` and `Esc Dismiss` in its own footer, so it has to \
             be able to receive them -- and it asks for the foreground other than exactly once"
        );
        assert_eq!(
            code.matches("run_ui_native(").count(),
            0,
            "this card has become an `eframe` window, which is the ~50 MB of unreleasable OpenGL \
             driver arenas it exists to not spend"
        );
    }

    /// **Up and Down move between the rows, ahead of `IsDialogMessageW`.**
    ///
    /// A source pin, because the key arrives in the card's own pump and no
    /// test can open that window. Decidable: `next` answers `VK_UP` and
    /// `VK_DOWN` itself through `arrow_to_row` -- which asks
    /// `row_after_arrow` where to go, so the end-stops are the pure
    /// function's -- and does so before `IsDialogMessageW`, whose own arrow
    /// traversal wraps round the ends. And that Enter still takes the focused
    /// row, which is what the arrows move.
    #[test]
    fn up_and_down_move_between_rows_before_the_dialog_manager_sees_them() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        let flat = code.split_whitespace().collect::<Vec<_>>().join(" ");
        let traversal = flat
            .find("IsDialogMessageW(top, &msg)")
            .expect("control: the production cut does not contain the pump this rule is about");
        let escape = flat.find("VK_ESCAPE.0").expect("`next` no longer answers Escape");
        assert!(escape < traversal, "control: Escape is handled after the dialog manager");

        let down = flat
            .find("VK_DOWN.0 ||")
            .expect("`next` no longer handles VK_DOWN itself, so Down is left to the dialog manager");
        let up = flat
            .find("VK_UP.0")
            .expect("`next` no longer handles VK_UP itself, so Up is left to the dialog manager");
        assert!(
            down < traversal && up < traversal,
            "Up or Down is handled after `IsDialogMessageW`, which swallows it first"
        );
        assert!(
            flat.contains("arrow_to_row(top, msg.wParam.0 as u16 == VK_DOWN.0)"),
            "the arrow arm no longer moves the card's focus through `arrow_to_row`"
        );
        assert!(
            flat.contains("crate::win32_draw::row_after_arrow(current, count, down)"),
            "`arrow_to_row` no longer asks the shared pure function where to go, so the clamp \
             the unit tests pin is not the clamp the card uses"
        );
        assert!(
            flat.contains("return Event::Pick(focused_row(top))"),
            "Enter no longer fills the focused row, which is the row the arrows move"
        );
    }

    /// **Nothing on this card is logged.**
    ///
    /// The rows carry a username and an item name -- not secrets, but the
    /// user's own account names for the app they are in front of, and a daemon
    /// that wrote them to a log file on disk would be keeping a record of
    /// which apps this person signs into.
    #[test]
    fn the_card_writes_no_account_name_to_the_log() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert!(
            code.contains("log::warn!"),
            "control: this module logs nothing at all, so the rule below is vacuous"
        );
        for forbidden in ["log::info!", "log::debug!", "log::trace!"] {
            assert!(
                !code.contains(forbidden),
                "`{forbidden}` appears in a module that holds the user's own account names"
            );
        }
        // **Every log line here is a fixed sentence.** Not "no `{}` anywhere"
        // -- the card's own two lines are built with `format!` and must be --
        // but no interpolation inside a `log::` call, which is where a
        // username or an item name would reach a file on disk.
        let mut scanned = 0;
        for line in code.lines() {
            let Some(start) = line.find("log::") else { continue };
            scanned += 1;
            assert!(
                !line[start..].contains('{'),
                "a log line in prompt_card.rs interpolates a value: {line:?}. The rows carry a \
                 username and an item name, and a daemon that wrote them to disk would be \
                 keeping a record of which apps this person signs into"
            );
        }
        assert!(scanned >= 2, "control: only {scanned} log lines were scanned");
    }
}

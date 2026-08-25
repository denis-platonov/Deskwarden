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
use crate::overlay_ui::OverlayMatch;

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
    /// The text the initials tile is built from -- **who** is being filled,
    /// which is the username when there is one. Not [`Row::primary`]: once a
    /// row is labelled by *what* it will type, initials of "Username + Tab +
    /// Password" would name nothing.
    pub avatar_of: String,
    /// The bold line: the username, or what this row will type.
    pub primary: String,
    /// The faint line under it: the item, and the app it fills.
    pub secondary: String,
}

/// **How many rows the card has room for.**
///
/// `crate::app::fill_choices` is bounded at four by construction -- username +
/// tab + password, username, password, one-time code, and custom `{S:Field}`
/// rows are deliberately absent precisely because an unbounded row count is a
/// geometry hazard for a card that cannot scroll. This is that bound written
/// where the geometry can be checked against it, and
/// [`the_row_cap_is_the_choice_lists_own_bound`] holds the two together.
pub const ROW_CAP: usize = 4;

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
    let (primary, secondary) = row_text(app_name, item_name, username);
    if choices.is_empty() {
        return vec![Row { avatar_of: primary.clone(), primary, secondary }];
    }
    choices
        .iter()
        .take(ROW_CAP)
        .map(|choice| Row {
            avatar_of: primary.clone(),
            primary: choice.label(),
            secondary: secondary.clone(),
        })
        .collect()
}

/// The two lines every row shares: who, and what it fills.
///
/// Lifted from the egui card unchanged, including the fallback for an item
/// that could not be read back from the vault at prompt time.
fn row_text(app_name: &str, item_name: &str, username: Option<&str>) -> (String, String) {
    match (username, item_name.is_empty()) {
        (Some(u), false) => (u.to_string(), format!("{item_name} · fills {app_name}")),
        (Some(u), true) => (u.to_string(), format!("fills {app_name}")),
        (None, false) => (item_name.to_string(), format!("fills {app_name}")),
        (None, true) => ("Saved credentials".to_string(), format!("fills {app_name}")),
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

/// The card's header line: `"1 match"` / `"4 matches"`.
pub fn match_count_label(rows: usize) -> String {
    if rows == 1 {
        "1 match".to_string()
    } else {
        format!("{rows} matches")
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

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything.
pub struct PromptCalls {
    /// Lays out and shows the card for these rows, at this anchor. `None` if
    /// it could not be put on screen.
    pub open: fn(&[Row], Option<(f32, f32)>) -> Option<PromptWindow>,
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

/// The `Enter` chip in the footer, and the word beside it.
pub const ENTER_SHORTCUT: &str = "ENTER";
/// What Enter does.
pub const FILL_LABEL: &str = "Fill";
/// The `Esc` chip in the footer, and the word beside it.
pub const ESC_SHORTCUT: &str = "ESC";
/// What Esc does.
pub const DISMISS_LABEL: &str = "Dismiss";

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

/// The card's width, and so the window's. The same
/// [`crate::picker_prompt::WIDTH`], because it is the same kind of card in the
/// same place on screen and two frameless daemon cards of different widths
/// read as two different programs.
pub const WIDTH: i32 = 380;

/// Content inset, and the top margin. `picker_prompt`'s.
const MARGIN_X: i32 = 16;
const MARGIN_TOP: i32 = 16;

/// One row. The same [`crate::picker_prompt`] row height, because it is drawn
/// by the same painter -- `crate::win32_draw::draw_row`, which takes the
/// gutter to be the row's own height, so this is also the avatar column's
/// width.
const ROW_H: i32 = 44;

/// The footer strip's height: the two keyboard hints and nothing else.
const FOOTER_H: i32 = 30;

/// The `ENTER` and `ESC` chips' boxes, and the gap between a chip and the word
/// it explains.
const ENTER_CHIP_W: i32 = 46;
const ESC_CHIP_W: i32 = 34;
const CHIP_GAP: i32 = 5;
/// The gap between the `Fill` hint and the `Dismiss` one.
const HINT_GAP: i32 = 14;
/// The `Fill` word's lane.
const FILL_W: i32 = 30;

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
    pub title: Box2,
    pub close_glyph: Box2,
    pub header_rule: Box2,
    pub footer_rule: Box2,
    /// The tinted band the hints sit on.
    pub footer: Box2,
    /// The whole list area. Individual rows are [`row_at`].
    pub list: Box2,
    pub enter_chip: Box2,
    pub fill: Box2,
    pub esc_chip: Box2,
    pub dismiss: Box2,
}

/// The card's geometry, for a list `rows` rows tall.
///
/// **Sized to the rows it will really draw**, and not to [`ROW_CAP`]. Unlike
/// the picker, whose two steps share one live window and must not move a
/// button out from under a pointer, this card has one shape decided once in
/// `open` and never transitions -- so a window sized for four rows showing one
/// would simply be a band of bare card under it.
///
/// `rows.max(1)`: a card with no rows is not a shorter card. The overlay
/// always paints at least one row -- with no choices it paints the matched
/// credential row it has always painted -- and a zero-row height would clip
/// that row off a window the user cannot scroll.
pub fn layout(rows: usize) -> Layout {
    let content_w = WIDTH - 2 * MARGIN_X;
    let rows = rows.max(1).min(ROW_CAP) as i32;

    let close_glyph = Box2 { x: WIDTH - MARGIN_X - 20, y: MARGIN_TOP, w: 20, h: 20 };
    let title = Box2 { x: MARGIN_X, y: MARGIN_TOP, w: content_w - 24, h: 20 };
    let header_rule = Box2 { x: 0, y: title.bottom() + 10, w: WIDTH, h: 1 };

    let list = Box2 {
        x: MARGIN_X,
        y: header_rule.bottom() + 6,
        w: content_w,
        h: ROW_H * rows,
    };

    let footer_rule = Box2 { x: 0, y: list.bottom() + 6, w: WIDTH, h: 1 };
    let footer = Box2 { x: 0, y: footer_rule.bottom(), w: WIDTH, h: FOOTER_H };

    let enter_chip =
        Box2 { x: MARGIN_X, y: footer.y, w: ENTER_CHIP_W, h: FOOTER_H };
    let fill = Box2 { x: enter_chip.right() + CHIP_GAP, y: footer.y, w: FILL_W, h: FOOTER_H };
    let esc_chip =
        Box2 { x: fill.right() + HINT_GAP, y: footer.y, w: ESC_CHIP_W, h: FOOTER_H };
    let dismiss = Box2 {
        x: esc_chip.right() + CHIP_GAP,
        y: footer.y,
        w: MARGIN_X + content_w - (esc_chip.right() + CHIP_GAP),
        h: FOOTER_H,
    };

    let window = Box2 { x: 0, y: 0, w: WIDTH, h: footer.bottom() };

    Layout {
        window,
        title,
        close_glyph,
        header_rule,
        footer_rule,
        footer,
        list,
        enter_chip,
        fill,
        esc_chip,
        dismiss,
    }
}

/// Row `index` of a card laid out for `rows` rows.
pub fn row_at(rows: usize, index: usize) -> Box2 {
    let list = layout(rows).list;
    Box2 { x: list.x, y: list.y + ROW_H * index as i32, w: list.w, h: ROW_H }
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
        Box2, Event, PromptWindow, Row, DISMISS_LABEL, ENTER_SHORTCUT, ESC_SHORTCUT, FILL_LABEL,
        GONE, PENDING, PROMPT_CARD_TITLE, ROWS, ROW_CAP,
    };
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AddFontMemResourceEx, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        CreateFontIndirectW, CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW,
        EndPaint, FillRect, GetDC, GetDeviceCaps, InvalidateRect, ReleaseDC, RoundRect,
        SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, DT_CENTER, DT_LEFT, DT_NOPREFIX,
        DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, LOGFONTW, LOGPIXELSX,
        PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetDlgItem, GetWindowLongPtrW, IsDialogMessageW, LoadCursorW, PeekMessageW,
        RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowDisplayAffinity,
        SetWindowLongPtrW, ShowWindow, TranslateMessage, BN_CLICKED, BS_PUSHBUTTON, CS_HREDRAW,
        CS_VREDRAW, GWLP_WNDPROC, HMENU, HTCAPTION, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW,
        WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_DESTROY,
        WM_ERASEBKGND, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_QUIT, WM_SETFONT,
        WNDCLASSW, WS_CHILD, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
    };

    use crate::win32_draw::{draw_hint_chip, draw_row, rgb, RowState};

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
                let installed = std::cell::Cell::new(0u32);
                let handle = AddFontMemResourceEx(
                    bytes.as_ptr() as *const c_void,
                    bytes.len() as u32,
                    None,
                    installed.as_ptr(),
                );
                if handle.0.is_null() || installed.get() == 0 {
                    log::warn!("could not register a bundled Archivo face with GDI");
                }
            }
        });
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

    fn mono(px: i32) -> HFONT {
        unsafe {
            let mut lf = LOGFONTW {
                lfHeight: -scale(px),
                lfWeight: FW_NORMAL.0 as i32,
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            for (i, ch) in crate::theme::GDI_MONO_FACE.encode_utf16().take(31).enumerate() {
                lf.lfFaceName[i] = ch;
            }
            CreateFontIndirectW(&lf)
        }
    }

    /// Every face the card paints with, created at open and destroyed at
    /// close. Kept together so `close` cannot leak one by forgetting it.
    struct Fonts {
        title: HFONT,
        name: HFONT,
        user: HFONT,
        avatar: HFONT,
        hint: HFONT,
        prose: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, REGULAR, SEMIBOLD};
            Fonts {
                title: font(BOLD, 14),
                name: font(SEMIBOLD, 13),
                user: font(REGULAR, 11),
                avatar: font(SEMIBOLD, 11),
                hint: mono(crate::theme::CHIP_TEXT_PX as i32),
                prose: font(REGULAR, 11),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [self.title, self.name, self.user, self.avatar, self.hint, self.prose] {
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
    pub(super) fn next(window: PromptWindow) -> Event {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
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

    /// Which row the keyboard is on, as [`super::selected_row`] wants it.
    fn focused_row(window: HWND) -> usize {
        let count = row_count();
        let focus = unsafe { GetFocus() };
        let mut found = None;
        for index in 0..count {
            if let Ok(control) = unsafe { GetDlgItem(window, (ID_ROW + index) as i32) } {
                if control == focus {
                    found = Some(index);
                    break;
                }
            }
        }
        super::selected_row(found, count)
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

    fn round_corners(window: HWND) {
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
                // A pointer that left a control without entering another one
                // is seen here rather than by the control it left.
                if HOVERED.swap(0, Ordering::SeqCst) != 0 {
                    repaint_all(window);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as usize;
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
        let l = super::layout(row_count());
        let x = (lparam.0 & 0xffff) as i16 as i32;
        let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
        x >= scale(l.close_glyph.x)
            && x < scale(l.close_glyph.right())
            && y >= scale(l.close_glyph.y)
            && y < scale(l.close_glyph.bottom())
    }

    // ---- painting ----------------------------------------------------------

    /// The card's own surface: the header, the two hairlines, the footer's
    /// tint and its two hints. Every row paints itself.
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
            let dpi = DPI_PERCENT.load(Ordering::SeqCst);

            // The window IS the card, so its whole client area is
            // `theme::CARD`.
            fill_rect(mem, client, crate::theme::CARD);
            fill_box(mem, l.footer, crate::theme::CARD_TINT);
            fill_box(mem, l.header_rule, crate::theme::HAIRLINE);
            fill_box(mem, l.footer_rule, crate::theme::HAIRLINE);
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                text(
                    mem,
                    fonts.title,
                    l.title,
                    &super::match_count_label(count),
                    crate::theme::INK,
                );

                // `Enter Fill` and `Esc Dismiss`. Both chips are
                // `win32_draw`'s, so they are the same chip every other card
                // draws, and the words beside them are the hints' own.
                for (chip, word, label) in [
                    (l.enter_chip, l.fill, (ENTER_SHORTCUT, FILL_LABEL)),
                    (l.esc_chip, l.dismiss, (ESC_SHORTCUT, DISMISS_LABEL)),
                ] {
                    let rc = RECT {
                        left: scale(chip.x),
                        top: scale(chip.y),
                        right: scale(chip.right()),
                        bottom: scale(chip.bottom()),
                    };
                    draw_hint_chip(mem, rc, label.0, fonts.hint, dpi);
                    text(mem, fonts.prose, word, label.1, crate::theme::TEXT_FAINT);
                }
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

            fill_rect(mem, whole, crate::theme::CARD);
            SetBkMode(mem, TRANSPARENT);

            if let (Some(fonts), Some(row)) = (fonts, rows.get(index)) {
                let candidate = crate::app_candidates::Candidate {
                    id: String::new(),
                    name: row.primary.clone(),
                    username: row.secondary.clone(),
                };
                draw_row(
                    mem,
                    whole,
                    &candidate,
                    RowState { selected, hovered },
                    fonts.name,
                    fonts.user,
                    // The `Enter` chip goes on the selected row and only
                    // there, because Enter fills that row and only that one.
                    selected.then_some((ENTER_SHORTCUT, fonts.hint)),
                    dpi,
                );
                // The initials tile, in the square gutter `draw_row` leaves
                // blank on the left. It keeps showing WHO is being filled --
                // the label already says what is being typed.
                paint_avatar(mem, whole, &row.avatar_of, fonts.avatar, selected);
            }
            drop(guard);

            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(control, &ps);
        }
    }

    /// The initials tile, centred in the row's square left gutter.
    fn paint_avatar(hdc: HDC, row: RECT, name: &str, font: HFONT, selected: bool) {
        unsafe {
            let gutter = row.bottom - row.top;
            let side = scale(28);
            let x = row.left + (gutter - side) / 2;
            let y = row.top + (gutter - side) / 2;
            let (fill, ink) = if selected {
                (crate::theme::BLUE, crate::theme::CARD)
            } else {
                (crate::theme::CANVAS, crate::theme::TEXT_SECONDARY)
            };
            let brush = CreateSolidBrush(rgb(fill));
            let pen = CreatePen(PS_SOLID, 1, rgb(crate::theme::BORDER_STRONG));
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, pen);
            let radius = scale(8) * 2;
            let _ = RoundRect(hdc, x, y, x + side, y + side, radius, radius);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush);
            let _ = DeleteObject(pen);

            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, rgb(ink));
            let old_font = SelectObject(hdc, font);
            let mut chars: Vec<u16> =
                crate::theme::initials(name).encode_utf16().collect();
            let mut rc = RECT { left: x, top: y, right: x + side, bottom: y + side };
            DrawTextW(
                hdc,
                &mut chars,
                &mut rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(hdc, old_font);
        }
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

    fn fill_rect(hdc: HDC, rc: RECT, colour: eframe::egui::Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(colour));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
        }
    }

    /// [`fill_rect`], for a **logical** rectangle.
    fn fill_box(hdc: HDC, at: Box2, colour: eframe::egui::Color32) {
        fill_rect(
            hdc,
            RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()).max(scale(at.y) + 1),
            },
            colour,
        );
    }

    /// One run of text, left-aligned and vertically centred in `at`.
    fn text(hdc: HDC, font: HFONT, at: Box2, run: &str, colour: eframe::egui::Color32) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(colour));
            let mut chars: Vec<u16> = run.encode_utf16().collect();
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            // `DT_NOPREFIX`: these are the app's own words and a user's own
            // account name, in which an `&` is an ampersand and never a
            // mnemonic that would be drawn as an underscore.
            DrawTextW(
                hdc,
                &mut chars,
                &mut rc,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
            SelectObject(hdc, old);
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

    #[test]
    fn a_row_leads_with_the_username_so_the_user_knows_whose_password_this_is() {
        let drawn = rows(APP, "Ledgerline Desktop", Some("ada@example.com"), &[]);
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].primary, "ada@example.com");
        assert_eq!(drawn[0].secondary, "Ledgerline Desktop · fills Ledgerline");
    }

    #[test]
    fn a_row_falls_back_to_the_item_and_then_to_a_neutral_line() {
        assert_eq!(rows(APP, "Ledgerline Desktop", None, &[])[0].primary, "Ledgerline Desktop");
        assert_eq!(rows(APP, "", None, &[])[0].primary, "Saved credentials");
        assert_eq!(rows(APP, "", Some("ada"), &[])[0].secondary, "fills Ledgerline");
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
            assert_eq!(row.secondary, "Ledgerline Desktop · fills Ledgerline");
            assert_eq!(
                row.avatar_of, "ada@example.com",
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
        let widest = vec![
            FillChoice::UserTabPass,
            FillChoice::Just(FieldRef::Username),
            FillChoice::Just(FieldRef::Password),
            FillChoice::Just(FieldRef::Totp),
        ];
        assert_eq!(
            widest.len(),
            ROW_CAP,
            "`app::fill_choices` can offer {} rows and the card lays out {ROW_CAP}. A row past \
             the bottom edge of a frameless window with no scrollbar is unreachable",
            widest.len()
        );
        assert_eq!(rows(APP, "x", Some("y"), &widest).len(), ROW_CAP);
        // And a longer list is truncated rather than allowed to grow the card.
        let mut over = widest.clone();
        over.push(FillChoice::Saved);
        assert_eq!(rows(APP, "x", Some("y"), &over).len(), ROW_CAP);
    }

    #[test]
    fn the_header_counts_the_rows_that_are_really_drawn() {
        assert_eq!(match_count_label(1), "1 match");
        assert_eq!(match_count_label(4), "4 matches");
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

            assert!(l.title.right() <= l.close_glyph.x, "the header text runs under the ✕");
            assert!(
                l.close_glyph.right() <= l.window.right() - MARGIN_X,
                "the close glyph has crossed the card's right margin"
            );
            assert!(l.title.bottom() <= l.header_rule.y);
            assert!(l.header_rule.bottom() <= l.list.y);
            assert_eq!(l.list.x, MARGIN_X);
            assert_eq!(
                l.list.right(),
                WIDTH - MARGIN_X,
                "the list does not sit inside the card's margins"
            );
            assert_eq!(
                l.list.h,
                ROW_H * count as i32,
                "the list is not sized to the rows it will draw"
            );

            // Every row is inside the list, adjacent, and none overlaps.
            for index in 0..count {
                let row = row_at(count, index);
                assert_eq!(row.x, l.list.x);
                assert_eq!(row.w, l.list.w);
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
                        row_at(count, index - 1).bottom(),
                        "rows {} and {index} overlap or leave a gap",
                        index - 1
                    );
                }
            }

            assert!(l.list.bottom() <= l.footer_rule.y);
            assert_eq!(
                l.footer.y,
                l.footer_rule.bottom(),
                "the footer's tint does not start at its rule"
            );
            assert_eq!(
                l.footer.bottom(),
                l.window.bottom(),
                "the footer's tint stops short of the card's bottom edge, leaving a band of the \
                 card's own colour under it"
            );
            assert!(l.enter_chip.right() < l.fill.x, "the `Fill` word sits on its own chip");
            assert!(l.fill.right() < l.esc_chip.x, "the two footer hints overlap");
            assert!(l.esc_chip.right() < l.dismiss.x);
            assert!(
                l.dismiss.right() <= l.window.right() - MARGIN_X,
                "the footer hint has crossed the card's right margin"
            );
            assert!(l.dismiss.w > 0, "the footer hint has no room for its word");
            checked += 1;
        }
        assert_eq!(checked, ROW_CAP);
    }

    /// **The window grows by exactly one row per row**, so a card asked for
    /// four rows is not a one-row card with three of them off the bottom.
    #[test]
    fn each_row_costs_the_window_exactly_one_row_of_height() {
        for count in 2..=ROW_CAP {
            assert_eq!(
                layout(count).window.h - layout(count - 1).window.h,
                ROW_H,
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
        assert_eq!(FILL_LABEL, "Fill");
        assert_eq!(ESC_SHORTCUT, "ESC");
        assert_eq!(DISMISS_LABEL, "Dismiss");
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

//! **The v2 autofill panel kit: design 1b's numbers and painters, in GDI.**
//!
//! Slice 11 of the 1.0 redesign restyles the daemon's fill-time cards -- the
//! account list, the matched-item card, no match, locked, save a login and the
//! new-password generator -- to the panel look design section 1b draws. Those
//! cards are bare Win32 and stay bare Win32 (an egui window costs the daemon
//! ~50 MB for ever; see `crate::locked_card`), so what the design asks for is
//! said here once, in logical pixels at 100%, and every card lays itself out
//! and paints through it:
//!
//! * the geometry: panel widths, the 18 px outer radius, 42 px avatars, 46 px
//!   buttons with a 13 px radius, the 28 px `Enter` badge;
//! * the painters: the "Deskwarden" lockup, avatar tiles, the blue `Enter`
//!   badge, status pills, the `?` help badge and its dark key-hint tooltip, and
//!   the done check;
//! * the done check's animation as **pure functions of elapsed milliseconds**,
//!   so the curve is decidable without a window.
//!
//! It creates no window. Everything is drawn into a device context some card
//! already owns, so it is classified `OPENS_NO_WINDOW` in `crate::foreground`.
//!
//! # What GDI cannot do, and what stands in
//!
//! GDI has no antialiasing, no alpha and no blur: corners are hard, the design's
//! soft `0 24px 60px` panel shadow is not drawn (the window's own DWM rounding
//! and shadow stand in), and the keyframes' opacity ramps (`dw-fade`, `dw-up`,
//! the ring's fade in) are not drawn either. What is kept is **movement**: the
//! done check's ring still overshoots and its stroke still draws on.

use crate::win32_draw::{draw_mark, draw_text, mark_width, rgb, text_width_utf16, ButtonSkin};
use eframe::egui::Color32;
use std::ffi::c_void;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateSolidBrush, DeleteObject, Ellipse, LineTo, MoveToEx, RoundRect, SelectObject,
    SetBkMode, SetTextColor, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, HDC, HFONT, PS_SOLID, TRANSPARENT,
};

// ---------------------------------------------------------------------------
// Geometry (design D 1.3-1.8). Logical pixels, 1 canvas px = 1 point.
// ---------------------------------------------------------------------------

/// The Fill panel's width (D 1.3), and so the account list's.
pub const LIST_W: i32 = 400;
/// No match, Locked and New password (D 1.5, 1.6, 1.8).
pub const CARD_W: i32 = 410;
/// The Save-login card (D 1.7).
pub const SAVE_W: i32 = 370;

/// A card's inner padding (D 1.5-1.8: `pad 18`).
pub const PAD: i32 = 18;
/// The list panel's header padding (D 1.3: `14 16 10`).
pub const HEADER_PAD_X: i32 = 16;
pub const HEADER_PAD_TOP: i32 = 14;
/// The gap between a card's blocks (D 1.5: `gap 14`).
pub const BLOCK_GAP: i32 = 14;
/// The panel's outer radius. DWM rounds the window itself; see the module doc.
pub const RADIUS: i32 = 18;

/// The lockup's shield is 20 x 23 (D 0).
pub const MARK_H: i32 = 23;
/// The gap between the shield and the word (D 1.3: `gap 10`).
pub const LOCKUP_GAP: i32 = 10;
/// The wordmark's box. "Deskwarden" at 15/800 measures ~84; slack, because
/// `layout` is pure and cannot ask a device context.
pub const WORD_W: i32 = 110;
/// The wordmark itself, in the design's own case.
pub const WORDMARK: &str = "Deskwarden";

/// Buttons (D 1.5: `h46, r13`, gap 8).
pub const BUTTON_H: i32 = 46;
pub const BUTTON_RADIUS: i32 = 13;
pub const BUTTON_GAP: i32 = 8;
/// The Locked card's tall button (D 1.6: `h54, r15`).
pub const TALL_BUTTON_H: i32 = 54;
pub const TALL_BUTTON_RADIUS: i32 = 15;

/// An account row (D 1.3): a 42 px avatar with `pad 11 x 10` round it, 2 px
/// apart, `r13`, inside a list padded 8 px each side.
pub const AVATAR: i32 = 42;
pub const AVATAR_RADIUS: i32 = 12;
pub const ROW_PAD_Y: i32 = 11;
pub const ROW_PAD_X: i32 = 10;
pub const ROW_H: i32 = AVATAR + 2 * ROW_PAD_Y;
pub const ROW_GAP: i32 = 2;
pub const ROW_RADIUS: i32 = 13;
pub const LIST_PAD_X: i32 = 8;
pub const LIST_PAD_BOTTOM: i32 = 8;
/// The gap between a row's avatar and its text (D 1.3: `gap 13`).
pub const ROW_TEXT_GAP: i32 = 13;

/// The `Enter` badge on the selected row (D 1.3): `h28, pad 0 10, r8`.
pub const ENTER_H: i32 = 28;
pub const ENTER_PAD_X: i32 = 10;
pub const ENTER_RADIUS: i32 = 8;
/// How far the badge stands off the row's right edge.
pub const ENTER_INSET: i32 = 10;
pub fn enter_label() -> &'static str {
    crate::i18n::t("panel.enter_label")
}

/// The `?` help badge (D 1.3): a 20 px circle.
pub const HELP: i32 = 20;
/// The status pills (D 1.6, 1.8): `12/700, pad 4 x 10, r999`.
pub const PILL_H: i32 = 22;
pub const PILL_PAD_X: i32 = 10;

/// The key-hint tooltip (D 1.3): `width 230, pad 12 x 14, r12`, 14/500 on a
/// 1.6 line.
pub const TIP_W: i32 = 230;
pub const TIP_PAD_X: i32 = 14;
pub const TIP_PAD_Y: i32 = 12;
pub const TIP_LINE_H: i32 = 22;
pub const TIP_RADIUS: i32 = 12;

/// The close glyph's box. The design's Save card gives its ✕ a 32 px target;
/// the other panels have none, and keep the 0.15 cards' 20 px one.
pub const CLOSE: i32 = 20;
pub const CLOSE_LARGE: i32 = 32;

// ---- type sizes (D 1.3-1.8) ----------------------------------------------

pub const WORD_PX: i32 = 15;
pub const TITLE_PX: i32 = 19;
pub const SAVE_TITLE_PX: i32 = 17;
pub const BODY_PX: i32 = 15;
pub const ROW_NAME_PX: i32 = 16;
pub const ROW_SUB_PX: i32 = 14;
pub const BADGE_PX: i32 = 13;
pub const PILL_PX: i32 = 12;
pub const BUTTON_PX: i32 = 15;
pub const VALUE_PX: i32 = 21;
pub const TIP_PX: i32 = 14;

// ---- colours not in `theme` ------------------------------------------------

/// The `?` badge's fill (D 0: `#eceae7`).
pub const HELP_FILL: Color32 = Color32::from_rgb(0xec, 0xea, 0xe7);
/// The tooltip's ground is the design's `ink`.
pub const TIP_FILL: Color32 = crate::theme::INK;

/// An account's avatar colours (D 1.3's `OV` table): the first account is the
/// brand blue, then a pale blue, then a neutral -- and round again.
pub fn avatar_colours(index: usize) -> (Color32, Color32) {
    match index % 3 {
        0 => (crate::theme::BLUE, Color32::WHITE),
        1 => (Color32::from_rgb(0xdb, 0xe4, 0xf7), crate::theme::BLUE_DEEP),
        _ => (HELP_FILL, crate::theme::INK),
    }
}

/// What the `?` tooltip says on the Fill card (D 1.3, verbatim apart from the
/// `Tab` line -- see `Ruling: Tab` in the slice 11 plan).
pub fn fill_key_hints() -> [&'static str; 3] {
    [
        crate::i18n::t("panel.hint_choose_account"),
        crate::i18n::t("panel.hint_enter_fill"),
        crate::i18n::t("panel.hint_esc_close"),
    ]
}

/// The tooltip's height for `lines` lines.
pub fn tip_height(lines: usize) -> i32 {
    2 * TIP_PAD_Y + TIP_LINE_H * lines as i32
}

/// The tooltip's distance from the top of the card.
pub const TIP_TOP: i32 = 8;

/// Where the tooltip sits for a `?` badge at `badge` inside a window `window_w`
/// wide: **to the left of the badge, over the header**, so it lies on the card
/// rather than above it -- a child window cannot paint outside itself, and a
/// second topmost window would need its own foreground classification. Left
/// rather than below because the shortest card (one row) is only ~120 px tall
/// and the tooltip is ~90. See the plan's rulings.
pub fn tip_box(badge: (i32, i32, i32, i32), window_w: i32, lines: usize) -> (i32, i32, i32, i32) {
    let (left, ..) = badge;
    let x = (left - 8 - TIP_W).clamp(0, (window_w - TIP_W).max(0));
    (x, TIP_TOP, TIP_W, tip_height(lines))
}

// ---------------------------------------------------------------------------
// The lockup
// ---------------------------------------------------------------------------

/// **The panel header's lockup geometry**: the 20 x 23 shield and the room the
/// word takes beside it. [`crate::win32_draw::card_lockup`] is the 0.15 cards'
/// letterspaced capitals and stays for `unlock_prompt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelLockup {
    pub mark_w: i32,
    pub mark_h: i32,
    pub gap: i32,
    pub word_w: i32,
    pub word_px: i32,
}

pub fn lockup() -> PanelLockup {
    PanelLockup {
        mark_w: mark_width(MARK_H),
        mark_h: MARK_H,
        gap: LOCKUP_GAP,
        word_w: WORD_W,
        word_px: WORD_PX,
    }
}

/// The shield and "Deskwarden" in `font`. Both rects are DEVICE pixels.
pub fn draw_lockup(hdc: HDC, mark: RECT, word: RECT, font: HFONT) {
    draw_mark(hdc, mark);
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(crate::theme::INK));
        let mut rc = word;
        draw_text(hdc, WORDMARK, &mut rc, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
        SelectObject(hdc, old);
    }
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

/// A rounded rectangle in DEVICE pixels, filled and optionally edged. `radius`
/// is the corner radius (not the ellipse's diameter).
pub fn round_rect(hdc: HDC, rc: RECT, radius: i32, fill: Color32, edge: Option<Color32>) {
    unsafe {
        let brush = CreateSolidBrush(rgb(fill));
        let pen = CreatePen(PS_SOLID, 1, rgb(edge.unwrap_or(fill)));
        let old_brush = SelectObject(hdc, brush);
        let old_pen = SelectObject(hdc, pen);
        let d = radius.max(0) * 2;
        let _ = RoundRect(hdc, rc.left, rc.top, rc.right, rc.bottom, d, d);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);
    }
}

fn px(v: i32, scale: i32) -> i32 {
    v * scale / 100
}

/// An initials tile (D 1.3: 42 x 42, r12, 14/700), `rc` DEVICE pixels.
pub fn draw_avatar(hdc: HDC, rc: RECT, initials: &str, font: HFONT, colours: (Color32, Color32), scale: i32) {
    round_rect(hdc, rc, px(AVATAR_RADIUS, scale), colours.0, None);
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(colours.1));
        let mut at = rc;
        draw_text(hdc, initials, &mut at, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(hdc, old);
    }
}

/// The `Enter` badge's rectangle for a label `text_w` DEVICE pixels wide,
/// right-aligned and centred vertically in `row`. Pure.
pub fn enter_badge_rect(row: RECT, text_w: i32, scale: i32) -> RECT {
    let w = text_w.max(0) + 2 * px(ENTER_PAD_X, scale);
    let h = px(ENTER_H, scale);
    let right = row.right - px(ENTER_INSET, scale);
    let top = row.top + ((row.bottom - row.top) - h) / 2;
    RECT { left: (right - w).max(row.left), top, right, bottom: top + h }
}

/// Paints the blue `Enter` badge on a selected row and answers the lane it
/// takes off the row's right edge, so the name beside it can stop short.
pub fn draw_enter_badge(hdc: HDC, row: RECT, font: HFONT, scale: i32) -> i32 {
    let chars: Vec<u16> = enter_label().encode_utf16().collect();
    let text_w = text_width_utf16(hdc, font, &chars);
    let rc = enter_badge_rect(row, text_w, scale);
    round_rect(hdc, rc, px(ENTER_RADIUS, scale), crate::theme::BLUE, None);
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(Color32::WHITE));
        let mut at = rc;
        draw_text(hdc, enter_label(), &mut at, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(hdc, old);
    }
    row.right - rc.left + px(6, scale)
}

/// A status pill (`Locked`, `Very strong`): fully rounded, `rc` DEVICE pixels.
pub fn draw_pill(hdc: HDC, rc: RECT, label: &str, font: HFONT, fill: Color32, ink: Color32) {
    round_rect(hdc, rc, (rc.bottom - rc.top) / 2, fill, None);
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(ink));
        let mut at = rc;
        draw_text(hdc, label, &mut at, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(hdc, old);
    }
}

/// The pill's width for a label `text_w` DEVICE pixels wide.
pub fn pill_width(text_w: i32, scale: i32) -> i32 {
    text_w.max(0) + 2 * px(PILL_PAD_X, scale)
}

/// The `?` badge: a 20 px circle, `?` at 12/800. Darker while `hot`.
pub fn draw_help_badge(hdc: HDC, rc: RECT, font: HFONT, hot: bool) {
    unsafe {
        let fill = if hot { crate::theme::BORDER_STRONG } else { HELP_FILL };
        let brush = CreateSolidBrush(rgb(fill));
        let pen = CreatePen(PS_SOLID, 1, rgb(fill));
        let old_brush = SelectObject(hdc, brush);
        let old_pen = SelectObject(hdc, pen);
        let _ = Ellipse(hdc, rc.left, rc.top, rc.right, rc.bottom);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);

        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(crate::theme::TEXT_MUTED));
        let mut at = rc;
        draw_text(hdc, "?", &mut at, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(hdc, old);
    }
}

/// The dark key-hint tooltip over `rc` (DEVICE pixels), one line per entry.
pub fn draw_key_tooltip(hdc: HDC, rc: RECT, font: HFONT, lines: &[&str], scale: i32) {
    round_rect(hdc, rc, px(TIP_RADIUS, scale), TIP_FILL, None);
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(Color32::WHITE));
        for (i, line) in lines.iter().enumerate() {
            let top = rc.top + px(TIP_PAD_Y, scale) + i as i32 * px(TIP_LINE_H, scale);
            let mut at = RECT {
                left: rc.left + px(TIP_PAD_X, scale),
                top,
                right: rc.right - px(TIP_PAD_X, scale),
                bottom: top + px(TIP_LINE_H, scale),
            };
            draw_text(
                hdc,
                line,
                &mut at,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
        }
        SelectObject(hdc, old);
    }
}

/// A text-only link, left-aligned in `rc` (the Save card's `Never for this
/// app`): `ink` normally, [`crate::theme::INK`] when `hot`.
pub fn draw_link(hdc: HDC, rc: RECT, label: &str, font: HFONT, hot: bool, centre: bool) {
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, rgb(if hot { crate::theme::INK } else { crate::theme::TEXT_FAINT }));
        let mut at = rc;
        let align = if centre { DT_CENTER } else { DT_LEFT };
        draw_text(hdc, label, &mut at, align | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
        SelectObject(hdc, old);
    }
}

/// The ✕ as two strokes. `weight` is in DEVICE pixels.
pub fn draw_close_x(hdc: HDC, rc: RECT, weight: i32, ink: Color32) {
    unsafe {
        let pen = CreatePen(PS_SOLID, weight.max(1), rgb(ink));
        let old = SelectObject(hdc, pen);
        let w = rc.right - rc.left;
        let pad = w * 3 / 10;
        let _ = MoveToEx(hdc, rc.left + pad, rc.top + pad, None);
        let _ = LineTo(hdc, rc.right - pad, rc.bottom - pad);
        let _ = MoveToEx(hdc, rc.right - pad, rc.top + pad, None);
        let _ = LineTo(hdc, rc.left + pad, rc.bottom - pad);
        SelectObject(hdc, old);
        let _ = DeleteObject(pen);
    }
}

/// The regenerate glyph (D 1.8): a circular arrow, stroked, in `rc`.
pub fn draw_regenerate_icon(hdc: HDC, rc: RECT, ink: Color32, scale: i32) {
    use windows::Win32::Graphics::Gdi::Arc;
    unsafe {
        let w = (rc.right - rc.left).min(rc.bottom - rc.top);
        let cx = (rc.left + rc.right) / 2;
        let cy = (rc.top + rc.bottom) / 2;
        let r = w / 2;
        let pen = CreatePen(PS_SOLID, px(2, scale).max(1), rgb(ink));
        let old = SelectObject(hdc, pen);
        // A three-quarter circle open at the upper right, with a two-stroke head.
        let _ = Arc(hdc, cx - r, cy - r, cx + r, cy + r, cx + r, cy - r / 2, cx + r / 2, cy - r);
        let _ = MoveToEx(hdc, cx + r / 2, cy - r - px(2, scale), None);
        let _ = LineTo(hdc, cx + r / 2, cy - r + px(3, scale));
        let _ = MoveToEx(hdc, cx + r / 2, cy - r + px(3, scale), None);
        let _ = LineTo(hdc, cx + r - px(3, scale), cy - r + px(3, scale));
        SelectObject(hdc, old);
        let _ = DeleteObject(pen);
    }
}

// ---------------------------------------------------------------------------
// The `?` badge's hover and its tooltip window
//
// The badge is painted by the card's own window, so the card decides whether the
// pointer is on it and tells this module; what lives here is the tooltip -- a
// disabled, hidden child window shown over the list, because a child is what can
// lie *above* the row controls -- and the state that goes with it. One card is
// on screen at a time, so the state is process-wide.
// ---------------------------------------------------------------------------

/// The tooltip child's control id.
pub const ID_TIP: usize = 199;

/// `WM_MOUSELEAVE`, which `TrackMouseEvent` posts. Named here because the
/// `windows` crate files it under a feature this crate does not enable.
pub const WM_MOUSELEAVE: u32 = 0x02A3;

static HELP_HOT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the tooltip paints: a raw `HFONT`, its lines and the card's DPI.
struct TipState {
    font: isize,
    lines: Vec<String>,
    scale: i32,
}

static TIP: std::sync::Mutex<Option<TipState>> = std::sync::Mutex::new(None);

/// Whether the tooltip is up.
pub fn help_hot() -> bool {
    HELP_HOT.load(std::sync::atomic::Ordering::SeqCst)
}

/// Hands the tooltip what to paint. `font` stays the caller's: it must outlive
/// the card, and [`clear_tip`] runs before the caller destroys it.
pub fn set_tip(font: HFONT, lines: &[&str], scale: i32) {
    if let Ok(mut slot) = TIP.lock() {
        *slot = Some(TipState {
            font: font.0 as isize,
            lines: lines.iter().map(|l| l.to_string()).collect(),
            scale,
        });
    }
    HELP_HOT.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// Forgets the tooltip's state. Called as the card closes.
pub fn clear_tip() {
    if let Ok(mut slot) = TIP.lock() {
        *slot = None;
    }
    HELP_HOT.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// Creates the tooltip child at `at` (DEVICE pixels): no text, not visible, not
/// a tab stop and **disabled**, so it can never take focus or a click. Created
/// last, so it is the topmost child.
pub fn create_tip(parent: HWND, at: RECT) -> bool {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, SetWindowLongPtrW, BS_PUSHBUTTON, GWLP_WNDPROC, HMENU, WINDOW_EX_STYLE,
        WINDOW_STYLE, WS_CHILD, WS_DISABLED,
    };
    let made = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("BUTTON"),
            w!(""),
            WINDOW_STYLE(WS_CHILD.0 | WS_DISABLED.0 | BS_PUSHBUTTON as u32),
            at.left,
            at.top,
            at.right - at.left,
            at.bottom - at.top,
            parent,
            HMENU(ID_TIP as *mut c_void),
            None,
            None,
        )
    };
    match made {
        Ok(tip) => {
            unsafe {
                SetWindowLongPtrW(tip, GWLP_WNDPROC, tip_proc as *const () as isize);
            }
            true
        }
        Err(_) => false,
    }
}

/// The pointer moved over the card's own window: `on_badge` says whether it is
/// on the `?`. Puts the tooltip up or takes it down, and asks Windows for a
/// `WM_MOUSELEAVE` so it cannot outlive the pointer.
pub fn help_pointer(window: HWND, on_badge: bool) {
    if on_badge {
        unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
            };
            let mut request = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: window,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut request);
        }
    }
    set_help(window, on_badge);
}

/// A child control saw the pointer: it is not on the badge, so the tooltip comes
/// down.
pub fn help_pointer_left_badge(child: HWND) {
    if !help_hot() {
        return;
    }
    if let Ok(parent) = unsafe { windows::Win32::UI::WindowsAndMessaging::GetParent(child) } {
        set_help(parent, false);
    }
}

/// Puts the tooltip up or takes it down, repainting only on a change.
pub fn set_help(window: HWND, hot: bool) {
    use windows::Win32::Graphics::Gdi::InvalidateRect;
    use windows::Win32::UI::WindowsAndMessaging::{GetDlgItem, ShowWindow, SW_HIDE, SW_SHOW};
    if HELP_HOT.swap(hot, std::sync::atomic::Ordering::SeqCst) == hot {
        return;
    }
    unsafe {
        if let Ok(tip) = GetDlgItem(window, ID_TIP as i32) {
            let _ = ShowWindow(tip, if hot { SW_SHOW } else { SW_HIDE });
        }
        let _ = InvalidateRect(window, None, false);
    }
}

/// The tooltip paints itself and is transparent to the pointer, so moving down
/// off the badge onto the list reaches the row beneath it.
unsafe extern "system" fn tip_proc(
    tip: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, WM_ERASEBKGND, WM_NCHITTEST, WM_PAINT,
    };
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_tip(tip);
            LRESULT(0)
        }
        // `HTTRANSPARENT`.
        WM_NCHITTEST => LRESULT(-1),
        _ => DefWindowProcW(tip, msg, wparam, lparam),
    }
}

fn paint_tip(tip: HWND) {
    use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, FillRect, PAINTSTRUCT};
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(tip, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(tip, &mut rc);
        if let Ok(slot) = TIP.lock() {
            if let Some(state) = slot.as_ref() {
                // The ground behind the tooltip's rounded corners is the card.
                let brush = CreateSolidBrush(rgb(crate::theme::CARD));
                FillRect(hdc, &rc, brush);
                let _ = DeleteObject(brush);
                let lines: Vec<&str> = state.lines.iter().map(String::as_str).collect();
                draw_key_tooltip(hdc, rc, HFONT(state.font as *mut c_void), &lines, state.scale);
            }
        }
        let _ = EndPaint(tip, &ps);
    }
}

// ---------------------------------------------------------------------------
// The account row (D 1.3)
// ---------------------------------------------------------------------------

/// What the tile at the left of a row holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile<'a> {
    /// Initials on one of the design's three colourways ([`avatar_colours`]).
    Initials(&'a str, usize),
    /// A neutral tile the caller blends an icon or a field mark into; the
    /// rectangle it was drawn at is handed back.
    Blank,
    /// No tile at all: a row that is text, not an account (an overflow notice,
    /// the search row). The text starts where the tile would have.
    None,
}

/// The faces a row is painted with.
#[derive(Clone, Copy)]
pub struct RowFonts {
    pub name: HFONT,
    pub sub: HFONT,
    pub avatar: HFONT,
    pub badge: HFONT,
    /// The monospace face of a keyboard-hint chip.
    pub chip: HFONT,
}

/// What a row says and how it is drawn.
#[derive(Clone, Copy)]
pub struct RowLook<'a> {
    pub tile: Tile<'a>,
    pub name: &'a str,
    pub sub: &'a str,
    pub selected: bool,
    pub hovered: bool,
    /// A keyboard-hint chip for a key that runs this row (`1`, `N`, `CTRL+2`).
    pub chip: Option<&'a str>,
}

/// The tile's rectangle inside a row `rect` (DEVICE pixels): the design's
/// 10 px inset, centred vertically so a row shorter than the design's 64 keeps
/// its tile centred. Pure.
pub fn tile_rect(rect: RECT, scale: i32) -> RECT {
    let side = px(AVATAR, scale);
    let top = rect.top + ((rect.bottom - rect.top) - side) / 2;
    let left = rect.left + px(ROW_PAD_X, scale);
    RECT { left, top, right: left + side, bottom: top + side }
}

/// Line boxes of a row's two text lines (D 1.3: the text column is a flex
/// column with `gap 2`; a 16 px line is ~19 high and a 14 px line ~17, so the
/// 38 px block sits centred on the 42 px tile, 2 px off its top).
pub const NAME_LINE_H: i32 = 19;
pub const SUB_LINE_H: i32 = 17;
pub const TEXT_GAP: i32 = 2;

/// The wash's rectangle inside a row `rect` (DEVICE pixels): the design's 64 px
/// row, centred in whatever pitch the caller gave. A 66 px pitch (64 + the 2 px
/// gap) is inset 1 px top and bottom; a rect already 64 or shorter is not
/// inset at all. Pure.
pub fn wash_rect(rect: RECT, scale: i32) -> RECT {
    let spare = ((rect.bottom - rect.top) - px(ROW_H, scale)).max(0);
    RECT { top: rect.top + spare / 2, bottom: rect.bottom - (spare - spare / 2), ..rect }
}

/// The name line's and the sub line's bands (DEVICE pixels, full tile height
/// of text block), `left`/`right` still to be set by the caller. Pure.
pub fn text_bands(tile: RECT, scale: i32) -> (RECT, RECT) {
    let block = NAME_LINE_H + TEXT_GAP + SUB_LINE_H;
    let side = tile.bottom - tile.top;
    let top = tile.top + (side - px(block, scale)) / 2;
    let name = RECT { left: 0, right: 0, top, bottom: top + px(NAME_LINE_H, scale) };
    let sub_top = name.bottom + px(TEXT_GAP, scale);
    let sub = RECT { left: 0, right: 0, top: sub_top, bottom: sub_top + px(SUB_LINE_H, scale) };
    (name, sub)
}

/// **One account row, as design 1b draws it**: a rounded wash over the row
/// (blue when selected, soft grey on hover), the tile, a bold line and a faint
/// one under it, the blue `Enter` badge on the selected row and a hint chip for
/// the key that runs it. `rect` is DEVICE pixels and a pitch row: the wash is
/// inset a pixel top and bottom so two rows are the design's 2 px apart.
///
/// Answers the tile's rectangle, so a caller can blend a favicon or a field mark
/// into a [`Tile::Blank`].
pub fn draw_account_row(
    hdc: HDC,
    rect: RECT,
    look: RowLook<'_>,
    fonts: &RowFonts,
    scale: i32,
) -> RECT {
    let wash = wash_rect(rect, scale);
    if look.selected {
        round_rect(hdc, wash, px(ROW_RADIUS, scale), crate::theme::BLUE_WASH, None);
    } else if look.hovered {
        round_rect(hdc, wash, px(ROW_RADIUS, scale), crate::theme::WINDOW_BG, None);
    }

    // The badge and the chip first, so the text lane can stop short of them:
    // `DT_END_ELLIPSIS` truncates against the rect it is given.
    let mut lane = 0;
    if look.selected {
        lane += draw_enter_badge(hdc, rect, fonts.badge, scale);
    }
    if let Some(chip) = look.chip {
        let beside = RECT { right: rect.right - lane, ..rect };
        lane += crate::win32_draw::draw_hint_chip(hdc, beside, chip, fonts.chip, scale);
    }

    let tile = tile_rect(rect, scale);
    match look.tile {
        Tile::Initials(name, index) => {
            draw_avatar(hdc, tile, &crate::theme::initials(name), fonts.avatar, avatar_colours(index), scale)
        }
        Tile::Blank => round_rect(hdc, tile, px(AVATAR_RADIUS, scale), HELP_FILL, None),
        Tile::None => {}
    }

    let left = if look.tile == Tile::None {
        tile.left
    } else {
        tile.right + px(ROW_TEXT_GAP, scale)
    };
    let right = rect.right - crate::theme::TEXT_CLIP_INSET as i32 - lane;
    let (name_band, sub_band) = text_bands(tile, scale);
    let line = DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
    unsafe {
        SetBkMode(hdc, TRANSPARENT);

        SetTextColor(hdc, rgb(crate::theme::INK));
        let old = SelectObject(hdc, fonts.name);
        let mut at = RECT { left, right, ..name_band };
        draw_text(hdc, look.name, &mut at, line);
        SelectObject(hdc, old);

        SetTextColor(hdc, rgb(crate::theme::TEXT_FAINT));
        let old = SelectObject(hdc, fonts.sub);
        let mut at = RECT { left, right, ..sub_band };
        draw_text(hdc, look.sub, &mut at, line);
        SelectObject(hdc, old);
    }
    tile
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// Which of the design's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelButton {
    /// `#1b3fa0`, hover `#14307a`.
    Primary,
    /// A 1 px `#e0deda` edge, hover `#f7f6f5`.
    Outline,
    /// The Locked card's `#14307a`, hover `#1b3fa0`.
    Deep,
}

/// The [`ButtonSkin`] for `kind`, hovered or not.
pub fn skin(kind: PanelButton, hovered: bool) -> ButtonSkin {
    let base = match kind {
        PanelButton::Primary => ButtonSkin::panel_primary(),
        PanelButton::Outline => ButtonSkin::panel_outline(),
        PanelButton::Deep => ButtonSkin::panel_deep(),
    };
    if hovered {
        base.hovered()
    } else {
        base
    }
}

// ---------------------------------------------------------------------------
// The done check (D 1.4)
// ---------------------------------------------------------------------------

/// `dw-ring .4 s cubic-bezier(.2,.9,.3,1.3)`.
pub const DONE_RING_MS: u32 = 400;
/// `dw-draw .5 s .2 s ease-out`: the stroke starts after 200 ms and takes 500.
pub const DONE_DRAW_DELAY_MS: u32 = 200;
pub const DONE_DRAW_MS: u32 = 500;
/// How long the card stays up once the check has finished drawing.
pub const DONE_HOLD_MS: u32 = 700;
/// The whole card's life.
pub const DONE_TOTAL_MS: u32 = DONE_DRAW_DELAY_MS + DONE_DRAW_MS + DONE_HOLD_MS;

/// The check disc (D 1.4: 84 x 84) and the mark inside it (42).
pub const DONE_DISC: i32 = 84;
pub const DONE_MARK: i32 = 42;

/// `easeOutBack`, the stand-in for the design's overshooting bezier: it rises
/// past 1 and settles. `t` is clamped to `0..=1`.
pub fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c1 = 1.2_f32;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// The disc's scale `elapsed_ms` into the card: `0.6` growing to `1.0` with an
/// overshoot (`dw-ring`'s `scale(.6) -> scale(1)`).
pub fn ring_scale(elapsed_ms: u32) -> f32 {
    let t = elapsed_ms as f32 / DONE_RING_MS as f32;
    0.6 + 0.4 * ease_out_back(t)
}

/// How much of the checkmark's stroke is drawn, `0.0..=1.0`: nothing for 200 ms,
/// then an ease-out over 500 (`dw-draw`).
pub fn check_progress(elapsed_ms: u32) -> f32 {
    if elapsed_ms <= DONE_DRAW_DELAY_MS {
        return 0.0;
    }
    let t = ((elapsed_ms - DONE_DRAW_DELAY_MS) as f32 / DONE_DRAW_MS as f32).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// The checkmark's polyline in the design's 24-unit box (`M20 6 9 17l-5-5`),
/// cut off after `progress` of its length.
pub fn check_points(progress: f32) -> Vec<(f32, f32)> {
    const PATH: [(f32, f32); 3] = [(20.0, 6.0), (9.0, 17.0), (4.0, 12.0)];
    let progress = progress.clamp(0.0, 1.0);
    let seg = |a: (f32, f32), b: (f32, f32)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let (l0, l1) = (seg(PATH[0], PATH[1]), seg(PATH[1], PATH[2]));
    let mut left = progress * (l0 + l1);
    let mut out = vec![PATH[0]];
    for (a, b, len) in [(PATH[0], PATH[1], l0), (PATH[1], PATH[2], l1)] {
        if left <= 0.0 {
            break;
        }
        if left >= len {
            out.push(b);
            left -= len;
        } else {
            let f = left / len;
            out.push((a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f));
            break;
        }
    }
    out
}

/// Paints the done check centred on `centre` (DEVICE pixels), `elapsed_ms` into
/// its life: the green disc at [`ring_scale`], and the stroke at
/// [`check_progress`].
pub fn draw_done_check(hdc: HDC, centre: POINT, scale: i32, elapsed_ms: u32) {
    unsafe {
        let r = (px(DONE_DISC, scale) as f32 / 2.0 * ring_scale(elapsed_ms)).round() as i32;
        let brush = CreateSolidBrush(rgb(crate::theme::DONE_WASH));
        let pen = CreatePen(PS_SOLID, 1, rgb(crate::theme::DONE_WASH));
        let old_brush = SelectObject(hdc, brush);
        let old_pen = SelectObject(hdc, pen);
        let _ = Ellipse(hdc, centre.x - r, centre.y - r, centre.x + r, centre.y + r);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);

        let points = check_points(check_progress(elapsed_ms));
        if points.len() < 2 {
            return;
        }
        // 42 px mark from a 24-unit path, drawn at the design's 2.6 stroke.
        let unit = px(DONE_MARK, scale) as f32 / 24.0;
        let to = |p: (f32, f32)| POINT {
            x: centre.x + ((p.0 - 12.0) * unit).round() as i32,
            y: centre.y + ((p.1 - 12.0) * unit).round() as i32,
        };
        // A `PS_SOLID` pen wider than one pixel is a geometric pen with round
        // caps and joins, which is what the design's `stroke-linecap: round` is.
        let pen = CreatePen(PS_SOLID, (2.6 * unit).round().max(1.0) as i32, rgb(crate::theme::DONE_MARK));
        let old = SelectObject(hdc, pen);
        let first = to(points[0]);
        let _ = MoveToEx(hdc, first.x, first.y, None);
        for p in &points[1..] {
            let q = to(*p);
            let _ = LineTo(hdc, q.x, q.y);
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(pen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_widths_are_the_designs() {
        assert_eq!((LIST_W, CARD_W, SAVE_W), (400, 410, 370));
        assert_eq!((BUTTON_H, BUTTON_RADIUS, TALL_BUTTON_H), (46, 13, 54));
        assert_eq!((AVATAR, AVATAR_RADIUS, ENTER_H, HELP), (42, 12, 28, 20));
    }

    #[test]
    fn an_account_row_is_its_avatar_and_the_designs_padding() {
        assert_eq!(ROW_H, 42 + 2 * 11);
    }

    /// D 1.3's row, in device pixels at 100% and at 150%: the wash is the
    /// design's 64 whatever pitch the caller gave, the tile sits 10 px in, and
    /// the two text lines (19 + 2 + 17) are centred on the tile.
    #[test]
    fn a_row_paints_the_designs_64_wash_and_centres_its_text_on_the_tile() {
        for (scale, pitch) in [(100, 66), (100, 64), (150, 99), (150, 96)] {
            let rc = RECT { left: 8, top: 100, right: 392, bottom: 100 + pitch };
            let wash = wash_rect(rc, scale);
            assert_eq!(wash.bottom - wash.top, px(ROW_H, scale), "scale {scale} pitch {pitch}");
            let tile = tile_rect(rc, scale);
            assert_eq!(tile.left - rc.left, px(ROW_PAD_X, scale));
            let (name, sub) = text_bands(tile, scale);
            assert_eq!(name.bottom - name.top, px(NAME_LINE_H, scale));
            assert_eq!(sub.top - name.bottom, px(TEXT_GAP, scale));
            let block = sub.bottom - name.top;
            let spare = (tile.bottom - tile.top) - block;
            assert!((name.top - tile.top - spare / 2).abs() <= 1, "block not centred on the tile");
        }
        assert_eq!((NAME_LINE_H, SUB_LINE_H, TEXT_GAP), (19, 17, 2));
        assert_eq!((ROW_NAME_PX, ROW_SUB_PX, BADGE_PX, ENTER_INSET), (16, 14, 13, 10));
        assert_eq!((ENTER_H, ENTER_PAD_X, ENTER_RADIUS, ROW_RADIUS, ROW_TEXT_GAP), (28, 10, 8, 13, 13));
    }

    #[test]
    fn the_lockup_is_the_20_by_23_shield() {
        let l = lockup();
        assert_eq!((l.mark_w, l.mark_h, l.gap, l.word_px), (20, 23, 10, 15));
    }

    #[test]
    fn the_wordmark_is_the_designs_own_case_and_not_capitals() {
        assert_eq!(WORDMARK, "Deskwarden");
    }

    #[test]
    fn the_enter_badge_hangs_off_the_rows_right_edge_and_is_centred() {
        let row = RECT { left: 0, top: 100, right: 400, bottom: 164 };
        let b = enter_badge_rect(row, 34, 100);
        assert_eq!(b.right, 400 - ENTER_INSET);
        assert_eq!(b.bottom - b.top, ENTER_H);
        assert_eq!(b.top, 100 + (64 - 28) / 2);
        assert_eq!(b.right - b.left, 34 + 2 * ENTER_PAD_X);
    }

    #[test]
    fn the_enter_badge_scales_with_the_display() {
        let row = RECT { left: 0, top: 0, right: 800, bottom: 128 };
        let b = enter_badge_rect(row, 68, 200);
        assert_eq!(b.bottom - b.top, 2 * ENTER_H);
        assert_eq!(row.right - b.right, 2 * ENTER_INSET);
    }

    #[test]
    fn a_badge_wider_than_its_row_stays_inside_it() {
        let row = RECT { left: 0, top: 0, right: 30, bottom: 64 };
        assert!(enter_badge_rect(row, 500, 100).left >= row.left);
    }

    #[test]
    fn the_avatars_cycle_through_the_designs_three_colourways() {
        assert_eq!(avatar_colours(0).0, crate::theme::BLUE);
        assert_eq!(avatar_colours(1).1, crate::theme::BLUE_DEEP);
        assert_eq!(avatar_colours(2).0, HELP_FILL);
        assert_eq!(avatar_colours(3), avatar_colours(0));
    }

    #[test]
    fn the_tooltip_lies_on_the_panel_and_never_off_its_edge() {
        let badge = (340, 14, 360, 34);
        let (x, y, w, h) = tip_box(badge, 400, fill_key_hints().len());
        assert!(x >= 0 && x + w <= 400, "the tooltip left the window: {x} + {w}");
        assert!(x + w <= badge.0, "the tooltip covers the badge it explains");
        assert_eq!(y, TIP_TOP);
        assert_eq!(h, tip_height(3));
    }

    #[test]
    fn the_key_hints_promise_only_keys_the_cards_answer() {
        let joined = fill_key_hints().join("\n");
        assert!(joined.contains("Enter fill") && joined.contains("Esc close"));
        // The design's `Tab fill and sign in` is not offered: Tab moves focus on
        // these cards and nothing presses a sign-in button.
        assert!(!joined.contains("Tab"));
    }

    #[test]
    fn the_ring_starts_small_overshoots_and_settles_at_full_size() {
        assert!((ring_scale(0) - 0.6).abs() < 1e-4);
        let peak = (0..=DONE_RING_MS).map(ring_scale).fold(0.0_f32, f32::max);
        assert!(peak > 1.0, "the ring never overshot: {peak}");
        assert!((ring_scale(DONE_RING_MS) - 1.0).abs() < 1e-4);
        assert!((ring_scale(5000) - 1.0).abs() < 1e-4, "the ring does not hold at full size");
    }

    #[test]
    fn the_check_draws_on_after_a_delay_and_never_runs_backwards() {
        assert_eq!(check_progress(0), 0.0);
        assert_eq!(check_progress(DONE_DRAW_DELAY_MS), 0.0);
        assert_eq!(check_progress(DONE_DRAW_DELAY_MS + DONE_DRAW_MS), 1.0);
        let mut last = 0.0;
        for ms in 0..=DONE_TOTAL_MS {
            let p = check_progress(ms);
            assert!(p >= last, "the stroke un-drew at {ms} ms");
            last = p;
        }
    }

    #[test]
    fn the_checks_path_is_cut_along_its_length() {
        assert_eq!(check_points(0.0), vec![(20.0, 6.0)]);
        assert_eq!(check_points(1.0), vec![(20.0, 6.0), (9.0, 17.0), (4.0, 12.0)]);
        let past = check_points(0.8);
        assert_eq!(past.len(), 3, "four fifths of the stroke has passed the elbow: {past:?}");
        let early = check_points(0.2);
        assert_eq!(early.len(), 2);
        assert!(early[1].0 < 20.0 && early[1].0 > 9.0);
    }

    #[test]
    fn the_done_card_outlives_its_animation() {
        const {
            assert!(DONE_TOTAL_MS > DONE_DRAW_DELAY_MS + DONE_DRAW_MS);
            assert!(DONE_TOTAL_MS < 3000, "a confirmation that lingers is an interruption");
        }
    }
}

//! **The autofill notice: a refused or abandoned fill, said on Deskwarden's own
//! card, in bare Win32.**
//!
//! Every fill that ends without typing what the user asked for reaches the
//! user through `injector::sequence::Notifier`, and production wires that to
//! `sequence::show_refusal`. That used to be a plain `MessageBoxW` -- the
//! system's grey box, its yellow triangle and its OK -- which the owner
//! rejected as "not DW native modal": it was the one surface on the fill path
//! that did not look like this app. This card replaces it, in the style of the
//! account picker's "What should I type?" card: the DESKWARDEN lockup, a bold
//! title, the refusal sentence, one *OK* button carrying its `ENTER` chip, and
//! the ✕.
//!
//! # Why bare Win32 and not egui
//!
//! For the measured reason every daemon card gives (see `crate::locked_card`):
//! the tray daemon is ~10 MB with no window ever opened, and the first egui
//! window takes it to ~60 MB **permanently**. A refusal is the last surface
//! that should cost that.
//!
//! # Its own thread, exactly as the message box had
//!
//! `show_refusal` is called from the daemon's message loop (a plan-time
//! refusal) or from the typing thread (an abandoned run). Neither may sit in a
//! modal pump, so the card runs on a thread `show_refusal` spawns -- the same
//! single thread the `MessageBoxW` ran on -- and the caller returns at once.
//! [`show_notice`] is that thread's whole body. Notices that arrive while one
//! is on screen wait their turn behind [`ONE_AT_A_TIME`], because the window's
//! state lives in this module's statics and two cards at once would share them.
//!
//! # Foreground: taken once, never retaken
//!
//! The card asks for the foreground once, on open, exactly as the other daemon
//! cards do and as `MB_SETFOREGROUND` did -- it advertises Enter and Esc, and
//! a card that cannot receive them cannot be answered the way it says. It
//! never asks again: a user who clicks back into their own app keeps it, and
//! the card stays topmost until dismissed. Closing it destroys the window,
//! which hands activation back to the window beneath it -- the one the user
//! was in.
//!
//! # The seam
//!
//! [`NoticeCalls`] is a struct of `fn` pointers and [`run_with`] is the whole
//! decision, drivable by a test with no window. What the card says
//! ([`notice_for`]), how big it is ([`layout`], [`body_height`]) and which keys
//! close it ([`event_for_key`]) are pure and tested here.

use crate::prompt_card::{place, Box2};

/// The window handle [`run_with`] deals in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoticeWindow(pub isize);

/// What the user did with the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// *OK*, Enter, Escape or the ✕. A notice has one answer, so every way of
    /// giving it is the same event.
    Dismiss,
    /// The window went away underneath the pump. Treated exactly as `Dismiss`.
    Closed,
}

/// How [`run_with`] finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Dismissed,
    /// The window could not be put on screen. Nothing was shown; the refusal
    /// is still in the log, where `show_refusal` wrote it first.
    Unavailable,
}

/// What one card says: a short title, and the refusal as a sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: &'static str,
    pub body: String,
}

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything.
pub struct NoticeCalls {
    /// Lays out and shows the card for this notice.
    pub open: fn(&Notice) -> Option<NoticeWindow>,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the top-level
    /// window, before the first `next`.
    pub protect: fn(NoticeWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(NoticeWindow) -> Event,
    /// Destroys the window and releases its resources.
    pub close: fn(NoticeWindow),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// `protect` runs immediately after `open` and before the first `next`, and
/// `close` runs on every path that opened something.
pub fn run_with(calls: &NoticeCalls, notice: &Notice) -> Outcome {
    let Some(window) = (calls.open)(notice) else {
        log::warn!("the autofill notice card could not be put on screen");
        return Outcome::Unavailable;
    };
    if !(calls.protect)(window) {
        log::warn!(
            "SetWindowDisplayAffinity was refused for the autofill notice card; it is visible \
             to screen capture on this machine"
        );
    }
    // One `next`, not a loop: every `Event` it can return closes the card.
    match (calls.next)(window) {
        Event::Dismiss | Event::Closed => {
            (calls.close)(window);
            Outcome::Dismissed
        }
    }
}

/// The window's title. **Unique across this process**, because
/// `crate::foreground::pick` finds windows by title -- and in particular not
/// `crate::prompt_card::PROMPT_CARD_TITLE`, which is the caption the message
/// box this card replaces used to carry.
pub const NOTICE_CARD_TITLE: &str = "Deskwarden notice";

/// The card's title for a fill that typed nothing at all.
pub fn nothing_typed_title() -> &'static str {
    crate::i18n::t("notice_card.nothing_typed")
}

/// The card's title for a run that was abandoned part-way.
///
/// **Its own title because the other one would be false.** A run abandoned
/// after its first step has already typed something -- typically the username
/// -- and "Nothing was typed" over it would tell the user the opposite of what
/// is on their screen. The code already tells the two apart: an abandoned run
/// is the one message that leads with
/// [`crate::injector::sequence::STOPPED_PREFIX`].
pub fn stopped_title() -> &'static str {
    crate::i18n::t("notice_card.stopped")
}

/// The one button's label.
pub fn ok_label() -> &'static str {
    crate::i18n::t("notice_card.ok")
}

/// What the body says when the refusal arrived with no words at all. No
/// caller sends one today; a card with an empty well is worse than this.
pub fn fallback_body() -> &'static str {
    crate::i18n::t("notice_card.fallback_body")
}

/// **What the card says for one refusal.**
///
/// The refusal texts are written as log lines as much as for a person -- most
/// start lower-case, and some do not end in a full stop -- and the message box
/// showed them verbatim. Here they are turned into the sentence a card body
/// wants, and the title is chosen by the one distinction the code already
/// makes (see [`STOPPED_TITLE`]). Two prefixes are lifted off the body because
/// the title now says them: the abandoned run's `auto-type stopped:` and the
/// preflight's `Nothing sent.`.
pub fn notice_for(detail: &str) -> Notice {
    let detail = detail.trim();
    let (title, rest) = match detail.strip_prefix(crate::injector::sequence::STOPPED_PREFIX) {
        Some(rest) => (stopped_title(), rest.trim_start()),
        None => (nothing_typed_title(), detail),
    };
    let heading = format!("{}. ", crate::vault_window::preflight::refused_heading());
    let rest = rest.strip_prefix(heading.as_str()).unwrap_or(rest);
    // The refusal is written in English (it is a log line too); the card says
    // it in the language in force, or as it arrived if it is not one we know.
    let body = crate::injector::words::localize(rest).unwrap_or_else(|| rest.to_string());
    Notice { title, body: sentence(&body) }
}

/// `text`, trimmed, with a capital first letter and a closing full stop.
fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return fallback_body().to_string();
    };
    let mut out: String = first.to_uppercase().collect();
    out.push_str(chars.as_str());
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// **Which keys close the card.** Enter -- the key its button's chip names --
/// and Escape. Everything else is left to the dialog manager (Tab, Space on
/// the focused button) or ignored.
///
/// Both are handled before `IsDialogMessageW` sees them: that only turns
/// Escape into a cancel for a real dialog box, and what it does with Enter on
/// a window that is not one is a property of the dialog manager rather than a
/// decision this module made.
///
/// **A key that was already down when it arrived closes nothing.** `held`
/// is `WM_KEYDOWN`'s previous-key-state bit. The card opens moments after a
/// fill the user started, often with Enter -- the picker's own key -- and a
/// still-held Enter auto-repeating into this window would close the notice
/// before it was ever read. Only a fresh press is an answer.
pub fn event_for_key(virtual_key: u16, held: bool) -> Option<Event> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
    if held {
        return None;
    }
    if virtual_key == VK_RETURN.0 || virtual_key == VK_ESCAPE.0 {
        Some(Event::Dismiss)
    } else {
        None
    }
}

/// What kind of run a [`Painted`] is, which is what the painter reads to pick
/// its font and its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Title,
    Body,
    /// The *OK* button's label. Painted by the control, with its chip.
    Button,
}

/// One run of text the card paints, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Painted {
    pub text: String,
    pub at: Box2,
    pub role: Role,
}

/// **Every word this card paints, with the box it is painted in** -- the one
/// list the painter and the tests both read, as in `crate::locked_card`.
pub fn painted(notice: &Notice, body_text_h: i32) -> Vec<Painted> {
    let l = layout(body_text_h);
    vec![
        Painted { text: notice.title.to_string(), at: l.title, role: Role::Title },
        Painted { text: notice.body.clone(), at: l.body_text, role: Role::Body },
        Painted { text: ok_label().to_string(), at: l.ok, role: Role::Button },
    ]
}

/// Puts the notice on screen and returns when the user has dismissed it.
///
/// **Blocks**, which is why its one production caller is the thread
/// `crate::injector::sequence::show_refusal` spawns for it.
pub fn show_notice(detail: &str) {
    let notice = notice_for(detail);
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = run_with(&REAL, &notice);
}

/// Held for as long as a card is on screen. See the module doc.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The production [`NoticeCalls`].
pub static REAL: NoticeCalls = NoticeCalls {
    open: win32::open,
    protect: win32::protect,
    next: win32::next,
    close: win32::close,
};

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// The card's width: `crate::prompt_card::WIDTH`, the width every daemon card
/// in this family is.
pub const WIDTH: i32 = crate::panel_draw::CARD_W;

const MARGIN_X: i32 = crate::panel_draw::PAD;
const MARGIN_TOP: i32 = crate::panel_draw::PAD;

/// The body well's padding around its text.
const BODY_PAD_X: i32 = 12;
const BODY_PAD_Y: i32 = 10;

/// One line of body text, at 100%, in the body's 13 px face.
pub const BODY_LINE_H: i32 = 20;

/// The fewest lines the body is sized for, and the most. Every refusal this
/// app writes fits in five; past eight the text is cut at a whole line
/// rather than growing a window that cannot scroll off the screen.
pub const MIN_BODY_LINES: i32 = 1;
pub const MAX_BODY_LINES: i32 = 8;

/// Button height: `crate::theme::BUTTON_HEIGHT`.
const BUTTON_H: i32 = crate::panel_draw::BUTTON_H;

/// The *OK* button's width: the word and its `ENTER` chip, with the padding a
/// pill needs.
const OK_W: i32 = 140;

/// Every rectangle the card paints, computed once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    pub mark: Box2,
    pub wordmark: Box2,
    pub title: Box2,
    pub close_glyph: Box2,
    pub header_rule: Box2,
    /// The tinted well the refusal sits in.
    pub body: Box2,
    /// The refusal's own box inside the well, word-wrapped.
    pub body_text: Box2,
    pub footer_rule: Box2,
    pub footer: Box2,
    pub ok: Box2,
}

/// **How tall the body's text is, in logical pixels**, from what GDI measured
/// in device pixels at `dpi_percent`.
///
/// Rounded up, so a measurement that lands between two logical pixels never
/// clips the last line's descenders; clamped to between [`MIN_BODY_LINES`]
/// and [`MAX_BODY_LINES`] lines. A refused measurement (`0`) is one line.
pub fn body_height(measured_device_h: i32, dpi_percent: i32) -> i32 {
    let dpi = if dpi_percent > 0 { dpi_percent } else { 100 };
    let logical = if measured_device_h > 0 {
        (measured_device_h * 100 + dpi - 1) / dpi
    } else {
        0
    };
    logical.clamp(MIN_BODY_LINES * BODY_LINE_H, MAX_BODY_LINES * BODY_LINE_H)
}

/// **The card's geometry, for a body `body_text_h` logical pixels tall.**
///
/// The only thing about this card that varies is how long its sentence is, so
/// that is the only argument; it is clamped exactly as [`body_height`] clamps.
pub fn layout(body_text_h: i32) -> Layout {
    let body_text_h =
        body_text_h.clamp(MIN_BODY_LINES * BODY_LINE_H, MAX_BODY_LINES * BODY_LINE_H);
    let content_w = WIDTH - 2 * MARGIN_X;

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
    let title = Box2 {
        x: MARGIN_X,
        y: mark.bottom() + crate::panel_draw::BLOCK_GAP,
        w: content_w - 24,
        h: 26,
    };
    let header_rule = Box2 { x: 0, y: title.bottom() + 10, w: WIDTH, h: 1 };

    let body = Box2 {
        x: MARGIN_X,
        y: header_rule.bottom() + 12,
        w: content_w,
        h: body_text_h + 2 * BODY_PAD_Y,
    };
    let body_text = Box2 {
        x: body.x + BODY_PAD_X,
        y: body.y + BODY_PAD_Y,
        w: body.w - 2 * BODY_PAD_X,
        h: body_text_h,
    };

    let footer_rule = Box2 { x: 0, y: body.bottom() + 12, w: WIDTH, h: 1 };
    let ok = Box2 { x: WIDTH - MARGIN_X - OK_W, y: footer_rule.bottom() + 8, w: OK_W, h: BUTTON_H };
    let height = ok.bottom() + 8;
    let window = Box2 { x: 0, y: 0, w: WIDTH, h: height };
    let footer =
        Box2 { x: 0, y: footer_rule.bottom(), w: WIDTH, h: height - footer_rule.bottom() };

    Layout {
        window,
        mark,
        wordmark,
        title,
        close_glyph,
        header_rule,
        body,
        body_text,
        footer_rule,
        footer,
        ok,
    }
}

// ---------------------------------------------------------------------------
// The done check (design D 1.4)
// ---------------------------------------------------------------------------

/// Which confirmation the done card shows. The design's three (D 1.4's
/// `doneText`), with the one fabricated detail -- "in 1.2 s" -- left out: no
/// timer here knows how long a fill took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoneCase {
    /// A login was saved from the save-a-login card.
    Saved,
    /// A password was changed and kept.
    Updated,
    /// A fill went through.
    SignedIn,
}

/// The title and subtitle for `case` (D 1.4).
pub fn done_copy(case: DoneCase) -> (&'static str, &'static str) {
    match case {
        DoneCase::Saved => (
            crate::i18n::t("notice_card.saved_title"),
            crate::i18n::t("notice_card.saved_sub"),
        ),
        DoneCase::Updated => (
            crate::i18n::t("notice_card.updated_title"),
            crate::i18n::t("notice_card.updated_sub"),
        ),
        DoneCase::SignedIn => (
            crate::i18n::t("notice_card.signed_in_title"),
            crate::i18n::t("notice_card.signed_in_sub"),
        ),
    }
}

/// The done card's width: the design's overlay is the host window; this is a
/// card of its own, as wide as the disc and the longest subtitle need.
pub const DONE_WIDTH: i32 = 400;
const DONE_PAD: i32 = 28;
const DONE_GAP: i32 = 16;

/// Every rectangle the done card paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DoneLayout {
    pub window: Box2,
    /// The disc's centre.
    pub disc: (i32, i32),
    pub title: Box2,
    pub subtitle: Box2,
}

/// The done card's geometry (D 1.4: a centred column, disc 84, `gap 16`).
pub fn done_layout() -> DoneLayout {
    let disc_d = crate::panel_draw::DONE_DISC;
    let disc = (DONE_WIDTH / 2, DONE_PAD + disc_d / 2);
    let title = Box2 { x: DONE_PAD, y: DONE_PAD + disc_d + DONE_GAP, w: DONE_WIDTH - 2 * DONE_PAD, h: 32 };
    let subtitle = Box2 { x: DONE_PAD, y: title.bottom() + 2, w: title.w, h: 22 };
    let window = Box2 { x: 0, y: 0, w: DONE_WIDTH, h: subtitle.bottom() + DONE_PAD };
    DoneLayout { window, disc, title, subtitle }
}

/// **Shows the done check and returns when it has closed** -- about a second
/// and a half. It runs on the calling thread, which is the daemon thread right
/// after a save, where nothing is waiting on it; a thread of its own would be a
/// new entry in `job_object`'s exact `std::thread::spawn` census for a card that
/// lives 1.5 s. It takes no focus (`WS_EX_NOACTIVATE`), closes itself when the
/// animation and its hold are over and on any click, and queues behind any card
/// already up.
pub fn show_done(case: DoneCase) {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    win32::run_done(case);
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// Whether the window has gone away underneath the pump.
static GONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the window procedure recorded, taken by `next` rather than read.
static PENDING: std::sync::Mutex<Option<Event>> = std::sync::Mutex::new(None);

/// What the card on screen says, and how tall its body was measured.
static SHOWN: std::sync::Mutex<Option<(Notice, i32)>> = std::sync::Mutex::new(None);

/// # Every pixel is painted by hand, through `crate::win32_draw`
///
/// The same arrangement as `crate::locked_card`'s `win32` module: one real
/// `BUTTON` child for focus, Space and Tab, with its painting handed to
/// [`crate::win32_draw`] so this card cannot drift from the palette. GDI only.
mod win32 {
    use super::{Event, Notice, Painted, Role, GONE, NOTICE_CARD_TITLE, PENDING, SHOWN};
    use crate::prompt_card::Box2;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontIndirectW,
        CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, FillRect, GetDC,
        GetDeviceCaps, InvalidateRect, ReleaseDC, SelectObject, SetBkMode,
        SetTextColor, CLEARTYPE_QUALITY, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_EDITCONTROL,
        DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FW_BOLD,
        FW_NORMAL, HBRUSH, HDC, HFONT, LOGFONTW, LOGPIXELSX, PAINTSTRUCT, PS_SOLID, SRCCOPY,
        TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetClientRect, GetDlgItem, GetWindowLongPtrW, IsDialogMessageW, LoadCursorW, PeekMessageW,
        RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowDisplayAffinity,
        SetWindowLongPtrW, ShowWindow, TranslateMessage, BN_CLICKED, BS_PUSHBUTTON, CS_HREDRAW,
        CS_VREDRAW, GWLP_WNDPROC, HMENU, IDC_ARROW, MSG, PM_REMOVE, SW_SHOW,
        WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_DESTROY,
        WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_QUIT,
        WM_SETFONT, WNDCLASSW, WS_CHILD, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
    };

    use crate::win32_draw::{draw_button_with_shortcut, draw_text, rgb};

    const ID_OK: usize = 101;
    const CLASS_NAME: PCWSTR = w!("DeskwardenAutofillNoticeCard");

    /// The system DPI as a percentage of 96, sampled once per open --
    /// `unlock_prompt`'s `DPI_PERCENT` carries the argument.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    static HOVERED: AtomicIsize = AtomicIsize::new(0);
    static ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);

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
    /// close, together so `close` cannot leak one by forgetting it.
    struct Fonts {
        brand: HFONT,
        title: HFONT,
        body: HFONT,
        button: HFONT,
        hint: HFONT,
        /// The done card's title (24/800) and subtitle (15).
        done_title: HFONT,
        done_sub: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, EXTRABOLD, REGULAR};
            Fonts {
                brand: font(EXTRABOLD, crate::panel_draw::WORD_PX),
                title: font(EXTRABOLD, crate::panel_draw::TITLE_PX),
                body: font(REGULAR, crate::panel_draw::BODY_PX),
                button: font(BOLD, crate::panel_draw::BUTTON_PX),
                hint: mono(crate::theme::CHIP_TEXT_PX as i32),
                done_title: font(EXTRABOLD, 24),
                done_sub: font(REGULAR, crate::panel_draw::BODY_PX),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [
                    self.brand,
                    self.title,
                    self.body,
                    self.button,
                    self.hint,
                    self.done_title,
                    self.done_sub,
                ] {
                    let _ = DeleteObject(f);
                }
            }
        }

        /// The face, the ink and the `DrawTextW` flags one [`Role`] is drawn
        /// with. One table, so no run is painted in a colour nothing chose.
        fn skin(&self, role: Role) -> (HFONT, eframe::egui::Color32, DRAW_TEXT_FORMAT) {
            let line = DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
            match role {
                Role::Title => (self.title, crate::theme::INK, line),
                Role::Body => (self.body, crate::theme::CAUTION_INK, body_format()),
                Role::Button => (self.button, crate::theme::INK, line),
            }
        }
    }

    /// The body's flags, measured and painted alike: wrapped at word breaks,
    /// and a partly visible last line left out rather than cut through its
    /// letters -- which is how a sentence too long for
    /// [`super::MAX_BODY_LINES`] ends; the whole of it is in the log.
    /// `DT_NOPREFIX`: an `&` in a refusal is an ampersand, never a mnemonic.
    fn body_format() -> DRAW_TEXT_FORMAT {
        DT_LEFT | DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX
    }

    static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
    // Raw GDI handles are process-wide. One card is up at a time
    // (`ONE_AT_A_TIME`), on one thread, so nothing shares them.
    unsafe impl std::marker::Send for Fonts {}

    // ---- the window --------------------------------------------------------

    pub(super) fn open(notice: &Notice) -> Option<super::NoticeWindow> {
        crate::win32_draw::register_fonts();
        GONE.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
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

        register_class();
        let body_font = {
            let mut slot = FONTS.lock().ok()?;
            if let Some(previous) = slot.take() {
                previous.destroy();
            }
            let fonts = Fonts::build();
            let body = fonts.body;
            *slot = Some(fonts);
            body
        };

        // **Sized to the sentence.** The body is measured with the face and
        // the flags it will be painted with, at the width it will be painted
        // into, and the window is laid out around that.
        let measured = measure(&notice.body, body_font, scale(super::layout(0).body_text.w));
        let body_h = super::body_height(measured, DPI_PERCENT.load(Ordering::SeqCst));
        if let Ok(mut slot) = SHOWN.lock() {
            *slot = Some((notice.clone(), body_h));
        }

        let l = super::layout(body_h);
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        // Centred on the work area, slightly high -- where the account
        // picker's card opens, and where every OS prompt puts itself. The
        // notifier is not told which window the fill was aimed at, and a
        // refusal about that window is better not drawn over it.
        let (x, y) = super::place(None, work_area(), w, h);

        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                CLASS_NAME,
                &HSTRING::from(NOTICE_CARD_TITLE),
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

        // Below this line the card is on screen, so a bare `?` would leave a
        // frameless topmost window with no way to dismiss it.
        fn abandon(window: HWND) -> Option<super::NoticeWindow> {
            unsafe {
                let _ = DestroyWindow(window);
            }
            release();
            None
        }

        let Some(button_font) =
            FONTS.lock().ok().and_then(|guard| guard.as_ref().map(|f| f.button))
        else {
            return abandon(window);
        };
        let Some(control) = child(window, l.ok, ID_OK, button_font) else {
            return abandon(window);
        };
        subclass(control);

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            // Asked for once, and allowed to be refused -- a refusal leaves a
            // topmost card the user clicks. Never asked for again: see the
            // module doc.
            let _ = SetForegroundWindow(window);
            let _ = SetFocus(control);
        }

        Some(super::NoticeWindow(window.0 as isize))
    }

    /// The body's height in device pixels, as `DrawTextW` lays it out.
    fn measure(body: &str, font: HFONT, width: i32) -> i32 {
        unsafe {
            let dc = GetDC(None);
            let old = SelectObject(dc, font);
            let mut rc = RECT { left: 0, top: 0, right: width, bottom: 0 };
            let height = draw_text(dc, body, &mut rc, body_format() | DT_CALCRECT);
            SelectObject(dc, old);
            ReleaseDC(None, dc);
            height.max(rc.bottom - rc.top)
        }
    }

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

    /// The capture exclusion, on the top-level window -- Windows refuses it
    /// on a child with `E_INVALIDARG`. A refusal can name an item's custom
    /// fields and the shape of its sequence, which a recorder need not keep.
    pub(super) fn protect(window: super::NoticeWindow) -> bool {
        if !crate::screen_share::capture_protection_enabled() {
            return true;
        }
        unsafe { SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    /// Pumps until the user does something. Keys go through
    /// [`super::event_for_key`] before `IsDialogMessageW` sees them.
    pub(super) fn next(window: super::NoticeWindow) -> Event {
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
                    if msg.message == WM_KEYDOWN {
                        // Bit 30 of a key message: the key was down before.
                        let held = (msg.lParam.0 >> 30) & 1 == 1;
                        if let Some(event) = super::event_for_key(msg.wParam.0 as u16, held) {
                            return event;
                        }
                        if held {
                            // A held key is swallowed, not handed to the
                            // dialog manager to press the button with.
                            continue;
                        }
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
            // Idle. Nothing on this card animates.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    pub(super) fn close(window: super::NoticeWindow) {
        unsafe {
            let _ = DestroyWindow(hwnd(window.0));
        }
        release();
    }

    /// Everything `open` set up that outlives the window.
    fn release() {
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
        if let Ok(mut slot) = SHOWN.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
    }

    // ---- the done card -------------------------------------------------------

    /// Which confirmation is on screen, if the window up is the done card, and
    /// how many milliseconds into its life it is.
    static DONE: Mutex<Option<super::DoneCase>> = Mutex::new(None);
    static DONE_MS: AtomicI32 = AtomicI32::new(0);

    fn done_active() -> bool {
        DONE.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }

    /// **Runs the done card to its end** on the calling thread: opens a
    /// non-activating topmost card, repaints it every ~16 ms with the
    /// animation's elapsed time, and closes it when the animation and its hold
    /// are over, on a click, or when something quits the thread.
    ///
    /// It never takes the foreground and never asks for the keyboard, so the
    /// fill or the save it confirms is not interrupted: no
    /// `SetForegroundWindow` and no focus here. It shows nothing secret, so it
    /// is not excluded from capture either.
    pub(super) fn run_done(case: super::DoneCase) {
        use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};
        crate::win32_draw::register_fonts();
        GONE.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        DONE_MS.store(0, Ordering::SeqCst);
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
        {
            let Ok(mut slot) = FONTS.lock() else { return };
            if let Some(previous) = slot.take() {
                previous.destroy();
            }
            *slot = Some(Fonts::build());
        }
        if let Ok(mut slot) = DONE.lock() {
            *slot = Some(case);
        }

        let l = super::done_layout();
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        let (x, y) = super::place(None, work_area(), w, h);
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0),
                CLASS_NAME,
                &HSTRING::from(NOTICE_CARD_TITLE),
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
        };
        let Ok(window) = window else {
            if let Ok(mut slot) = DONE.lock() {
                *slot = None;
            }
            release();
            return;
        };
        round_corners(window);
        unsafe {
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        }

        let started = std::time::Instant::now();
        loop {
            let elapsed = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
            DONE_MS.store(elapsed as i32, Ordering::SeqCst);
            if GONE.load(Ordering::SeqCst) || elapsed >= crate::panel_draw::DONE_TOTAL_MS {
                break;
            }
            repaint(window);
            let mut msg = MSG::default();
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        GONE.store(true, Ordering::SeqCst);
                        break;
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        unsafe {
            let _ = DestroyWindow(window);
        }
        if let Ok(mut slot) = DONE.lock() {
            *slot = None;
        }
        release();
    }

    /// The done card: the disc and its drawn-on check at the animation's
    /// current time, and the two lines under it.
    fn paint_done(window: HWND) {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::DT_CENTER;
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(window, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(window, &mut client);
            let (w, h) = (client.right, client.bottom);
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp);

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());
            let case = DONE.lock().ok().and_then(|slot| *slot);
            let l = super::done_layout();
            fill_rect(mem, client, crate::theme::CARD);
            SetBkMode(mem, TRANSPARENT);
            if let (Some(fonts), Some(case)) = (fonts, case) {
                let (title, subtitle) = super::done_copy(case);
                let centre = POINT { x: scale(l.disc.0), y: scale(l.disc.1) };
                crate::panel_draw::draw_done_check(
                    mem,
                    centre,
                    DPI_PERCENT.load(Ordering::SeqCst),
                    DONE_MS.load(Ordering::SeqCst).max(0) as u32,
                );
                let centred = DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
                text(mem, fonts.done_title, l.title, title, crate::theme::INK, centred);
                text(mem, fonts.done_sub, l.subtitle, subtitle, crate::theme::TEXT_MUTED, centred);
            }
            drop(guard);
            let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(window, &ps);
        }
    }

    // ---- plumbing ----------------------------------------------------------

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut c_void)
    }

    fn repaint(window: HWND) {
        unsafe {
            let _ = InvalidateRect(window, None, false);
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

    /// The runs the paint path draws, and the body height they were laid out
    /// for.
    fn shown() -> (Vec<Painted>, i32) {
        let held = SHOWN.lock().ok().and_then(|slot| slot.clone());
        let (notice, body_h) = held.unwrap_or_else(|| {
            (Notice { title: super::nothing_typed_title(), body: String::new() }, 0)
        });
        (super::painted(&notice, body_h), body_h)
    }

    fn register_class() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                lpszClassName: CLASS_NAME,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                // `WM_ERASEBKGND` is answered and the whole client area is
                // painted from one back buffer.
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
    }

    /// The one child control, created with no text: its label is painted.
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
        crate::card_shell::attach(window, crate::theme::CARD);
    }

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
            // Dragged by its background, except over the ✕ -- see
            // `win32_draw::frameless_hit`.
            WM_NCHITTEST => crate::win32_draw::frameless_hit_test(
                window,
                DefWindowProcW(window, msg, wparam, lparam),
                lparam,
                close_glyph_rect(),
            ),
            WM_LBUTTONDOWN => {
                if done_active() {
                    // Any click puts the confirmation away.
                    GONE.store(true, Ordering::SeqCst);
                } else if in_close_glyph(lparam) {
                    set_pending(Event::Dismiss);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if HOVERED.swap(0, Ordering::SeqCst) != 0 {
                    repaint(window);
                    if let Ok(control) = GetDlgItem(window, ID_OK as i32) {
                        repaint(control);
                    }
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = wparam.0 & 0xffff;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                if notification == BN_CLICKED && id == ID_OK {
                    set_pending(Event::Dismiss);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // No thread quit posted here, as on every card in this crate:
                // `GONE` is what `next()` reads.
                GONE.store(true, Ordering::SeqCst);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, msg, wparam, lparam),
        }
    }

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
                paint_control(control);
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

    /// The ✕'s rect in DEVICE pixels, read by the hit test and the click
    /// alike so the two can never disagree.
    fn close_glyph_rect() -> RECT {
        let l = super::layout(shown().1);
        dev(l.close_glyph)
    }

    fn in_close_glyph(lparam: LPARAM) -> bool {
        crate::win32_draw::on_close_glyph(
            (lparam.0 & 0xffff) as i16 as i32,
            ((lparam.0 >> 16) & 0xffff) as i16 as i32,
            close_glyph_rect(),
        )
    }

    fn dev(b: Box2) -> RECT {
        RECT { left: scale(b.x), top: scale(b.y), right: scale(b.right()), bottom: scale(b.bottom()) }
    }

    // ---- painting ----------------------------------------------------------

    /// The card's own surface. Its words are [`super::painted`]'s.
    fn paint(window: HWND) {
        if done_active() {
            paint_done(window);
            return;
        }
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(window, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(window, &mut client);
            let (w, h) = (client.right, client.bottom);

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(mem, bmp);

            let (runs, body_h) = shown();
            let l = super::layout(body_h);
            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            // Design 1b's panels have no rules and no tinted footer; the layout
            // keeps those slots as spacing and nothing is painted in them.
            fill_rect(mem, client, crate::theme::CARD);
            SetBkMode(mem, TRANSPARENT);

            // The well: the theme's caution wash and edge, which is what the
            // message box's yellow triangle was saying.
            rounded(mem, l.body, 12, crate::theme::CAUTION_WASH, crate::theme::CAUTION_EDGE);

            if let Some(fonts) = fonts {
                crate::panel_draw::draw_lockup(mem, dev(l.mark), dev(l.wordmark), fonts.brand);
                for run in &runs {
                    if run.role == Role::Button {
                        // The control paints its own.
                        continue;
                    }
                    let (face, ink, format) = fonts.skin(run.role);
                    text(mem, face, run.at, &run.text, ink, format);
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

    /// The *OK* button, with its `ENTER` chip inside it.
    fn paint_control(control: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(control, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(control, &mut rc);

            let hovered = HOVERED.load(Ordering::SeqCst) == ID_OK as isize;
            let focused = GetFocus() == control;

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            let whole = RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom };

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            // The button sits on the card's white.
            fill_rect(mem, whole, crate::theme::CARD);
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                let (runs, body_h) = shown();
                let label = runs
                    .into_iter()
                    .find(|r| r.role == Role::Button)
                    .map(|r| r.text)
                    .unwrap_or_default();
                let skin = crate::panel_draw::skin(crate::panel_draw::PanelButton::Primary, hovered);
                // The card's ONLY button: Enter plainly presses it, so it
                // carries no `ENTER` chip (the owner's call; the chip is kept
                // for cards with a choice of buttons).
                let hint: Option<(&str, windows::Win32::Graphics::Gdi::HFONT)> = None;
                let dpi = DPI_PERCENT.load(Ordering::SeqCst);
                if focused {
                    // The ring in LOGICAL size, from `layout`: `rounded`
                    // scales what it is handed.
                    let ok = super::layout(body_h).ok;
                    let ring = Box2 { x: 0, y: 0, w: ok.w, h: ok.h };
                    rounded(
                        mem,
                        ring,
                        crate::panel_draw::BUTTON_RADIUS + 1,
                        crate::theme::FOCUS_RING,
                        crate::theme::FOCUS_RING,
                    );
                    let inner = RECT {
                        left: whole.left + 2,
                        top: whole.top + 2,
                        right: whole.right - 2,
                        bottom: whole.bottom - 2,
                    };
                    draw_button_with_shortcut(
                        mem,
                        inner,
                        &label,
                        fonts.button,
                        skin,
                        scale(crate::panel_draw::BUTTON_RADIUS),
                        hint,
                        dpi,
                    );
                } else {
                    draw_button_with_shortcut(
                        mem,
                        whole,
                        &label,
                        fonts.button,
                        skin,
                        scale(crate::panel_draw::BUTTON_RADIUS),
                        hint,
                        dpi,
                    );
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

    /// The header's ✕, as two strokes.
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

    /// A rounded rectangle in logical coordinates, filled and edged.
    fn rounded(
        hdc: HDC,
        at: Box2,
        radius: i32,
        fill_colour: eframe::egui::Color32,
        edge: eframe::egui::Color32,
    ) {
        crate::win32_draw::aa_round_rect(hdc, dev(at), scale(radius), rgb(fill_colour), Some((rgb(edge), 1)));
    }

    /// One run of text, through `win32_draw::draw_text` -- the crate's one
    /// caller of `DrawTextW`, which guards the empty run.
    fn text(
        hdc: HDC,
        font: HFONT,
        at: Box2,
        run: &str,
        colour: eframe::egui::Color32,
        format: DRAW_TEXT_FORMAT,
    ) {
        unsafe {
            let old = SelectObject(hdc, font);
            SetTextColor(hdc, rgb(colour));
            let mut rc = dev(at);
            draw_text(hdc, run, &mut rc, format);
            SelectObject(hdc, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFLIGHT: &str = "Nothing sent. The window in front is not the one this item's rule \
                             was written for. Deskwarden will not type a password there.";

    fn inert() -> NoticeCalls {
        NoticeCalls {
            open: |_| Some(NoticeWindow(1)),
            protect: |_| true,
            next: |_| Event::Dismiss,
            close: |_| {},
        }
    }

    // ---- what it says ------------------------------------------------------

    /// **The reported refusal reads as the owner's card asks**: "Nothing was
    /// typed" over the sentence, and the sentence no longer repeats the title.
    #[test]
    fn the_preflight_refusal_is_titled_nothing_was_typed() {
        let notice = notice_for(PREFLIGHT);
        assert_eq!(notice.title, "Nothing was typed");
        assert_eq!(
            notice.body,
            "The window in front is not the one this item's rule was written for. Deskwarden \
             will not type a password there."
        );
        // Control: the text under test is the one the preflight really writes.
        assert_eq!(
            crate::vault_window::preflight::refusal_notice(Some(
                crate::vault_window::preflight::Refusal::WrongProcess
            )),
            PREFLIGHT
        );
    }

    /// **An abandoned run is not "nothing was typed"** -- the username may
    /// already be on screen -- so it is the one refusal with its own title, and
    /// the message it is told apart by is the one the runner really writes.
    #[test]
    fn an_abandoned_run_is_titled_typing_stopped() {
        use crate::injector::sequence::{plan, run, run_tests::FakeKeyboard, Resolved};
        let tokens = crate::key_sequence::parse("{USERNAME}{TAB}{PASSWORD}");
        let values = Resolved { username: "ana", password: "hunter2", totp: None, custom: vec![] };
        let plan = plan(&tokens, &values).expect("plans");
        let kb = FakeKeyboard::loses_foreground_after(1);
        let detail = run(&kb, 7, &plan).expect_err("the run is abandoned");

        let notice = notice_for(&detail);
        assert_eq!(notice.title, stopped_title());
        assert!(
            notice.body.starts_with("Window 7 is no longer in front"),
            "the body still carries the prefix the title replaced: {:?}",
            notice.body
        );
        assert!(notice.body.ends_with("Nothing further was typed."));
    }

    /// Every other refusal on the fill path typed nothing, and says so.
    #[test]
    fn every_other_fill_refusal_is_titled_nothing_was_typed() {
        let details = [
            crate::injector::ALREADY_TYPING.to_string(),
            crate::injector::sequence::Refusal::Nothing.message(),
            crate::reprompt::refusal_text(true).to_string(),
            crate::reprompt::refusal_text(false).to_string(),
            crate::vault_window::preflight::refusal_notice(None),
        ];
        for detail in details {
            let notice = notice_for(&detail);
            assert_eq!(notice.title, nothing_typed_title(), "for {detail:?}");
            let first = notice.body.chars().next().unwrap();
            assert!(first.is_uppercase(), "{:?} does not start a sentence", notice.body);
            assert!(notice.body.ends_with('.'), "{:?} does not end one", notice.body);
        }
    }

    #[test]
    fn a_refusal_becomes_one_sentence_and_nothing_is_lost() {
        assert_eq!(
            notice_for("  the auto-type sequence types nothing ").body,
            "The auto-type sequence types nothing."
        );
        assert_eq!(notice_for("Already a sentence!").body, "Already a sentence!");
        assert_eq!(notice_for("").body, fallback_body());
        assert_eq!(notice_for("   ").title, nothing_typed_title());
        // A non-ASCII first letter is capitalised as a letter, not a byte.
        assert_eq!(notice_for("élan").body, "Élan.");
    }

    // ---- keys ----------------------------------------------------------------

    #[test]
    fn enter_and_escape_close_the_card_and_nothing_else_does() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            VK_A, VK_ESCAPE, VK_RETURN, VK_SPACE, VK_TAB,
        };
        assert_eq!(event_for_key(VK_RETURN.0, false), Some(Event::Dismiss));
        assert_eq!(event_for_key(VK_ESCAPE.0, false), Some(Event::Dismiss));
        // Tab and Space are the dialog manager's: Space presses the focused
        // button, which is a click and arrives as one.
        for other in [VK_TAB, VK_SPACE, VK_A] {
            assert_eq!(event_for_key(other.0, false), None, "{other:?} closed the card");
        }
    }

    /// The Enter that started the fill, still held, auto-repeats into the card
    /// the moment it opens. It must not close a notice nobody has read.
    #[test]
    fn a_key_already_held_when_the_card_opened_closes_nothing() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
        assert_eq!(event_for_key(VK_RETURN.0, true), None);
        assert_eq!(event_for_key(VK_ESCAPE.0, true), None);
        // Control: the same keys, pressed fresh, do close it.
        assert_eq!(event_for_key(VK_RETURN.0, false), Some(Event::Dismiss));
    }

    // ---- the decision ------------------------------------------------------

    #[test]
    fn the_window_is_protected_before_it_is_ever_pumped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ORDER: AtomicUsize = AtomicUsize::new(0);
        static PROTECTED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        static PUMPED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = NoticeCalls {
            protect: |_| {
                PROTECTED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                true
            },
            next: |_| {
                let _ = PUMPED_AT.compare_exchange(
                    usize::MAX,
                    ORDER.fetch_add(1, Ordering::SeqCst),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                Event::Dismiss
            },
            ..inert()
        };
        let _ = run_with(&calls, &notice_for(PREFLIGHT));
        assert!(PROTECTED_AT.load(Ordering::SeqCst) < PUMPED_AT.load(Ordering::SeqCst));
    }

    #[test]
    fn every_exit_path_closes_the_window_and_a_failed_open_closes_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CLOSED: AtomicUsize = AtomicUsize::new(0);
        for next in [|_| Event::Dismiss, |_| Event::Closed] as [fn(NoticeWindow) -> Event; 2] {
            CLOSED.store(0, Ordering::SeqCst);
            let calls = NoticeCalls {
                next,
                close: |_| {
                    CLOSED.fetch_add(1, Ordering::SeqCst);
                },
                ..inert()
            };
            assert_eq!(run_with(&calls, &notice_for(PREFLIGHT)), Outcome::Dismissed);
            assert_eq!(CLOSED.load(Ordering::SeqCst), 1, "a topmost card was left on screen");
        }
        CLOSED.store(0, Ordering::SeqCst);
        let calls = NoticeCalls {
            open: |_| None,
            close: |_| {
                CLOSED.fetch_add(1, Ordering::SeqCst);
            },
            ..inert()
        };
        assert_eq!(run_with(&calls, &notice_for(PREFLIGHT)), Outcome::Unavailable);
        assert_eq!(CLOSED.load(Ordering::SeqCst), 0, "a window that never opened was closed");
    }

    #[test]
    fn the_card_is_opened_with_the_notice_it_was_given() {
        static SEEN: std::sync::Mutex<Option<Notice>> = std::sync::Mutex::new(None);
        let calls = NoticeCalls {
            open: |notice| {
                *SEEN.lock().unwrap() = Some(notice.clone());
                Some(NoticeWindow(1))
            },
            ..inert()
        };
        let notice = notice_for(PREFLIGHT);
        let _ = run_with(&calls, &notice);
        assert_eq!(SEEN.lock().unwrap().as_ref(), Some(&notice));
    }

    // ---- geometry ----------------------------------------------------------

    #[test]
    fn the_body_is_sized_to_its_measured_text_and_clamped() {
        // One line at 100% is one line.
        assert_eq!(body_height(17, 100), BODY_LINE_H);
        // Rounded up, never down: 37 device px at 150% is 24.67 logical.
        assert_eq!(body_height(37 * 3, 150), 74);
        assert_eq!(body_height(54, 100), 54);
        // A refused measurement, or a nonsense DPI, still lays out a line.
        assert_eq!(body_height(0, 100), BODY_LINE_H);
        assert_eq!(body_height(54, 0), 54);
        // And a runaway sentence stops growing the window.
        assert_eq!(body_height(10_000, 100), MAX_BODY_LINES * BODY_LINE_H);
    }

    /// **Nothing the card lays out falls off it, at any body height.**
    #[test]
    fn nothing_the_card_lays_out_falls_off_it() {
        let mut previous_height = 0;
        for lines in MIN_BODY_LINES..=MAX_BODY_LINES {
            let l = layout(lines * BODY_LINE_H);
            assert_eq!((l.window.x, l.window.y, l.window.w), (0, 0, WIDTH));
            assert!(l.window.h > previous_height, "a longer body did not grow the card");
            previous_height = l.window.h;

            assert!(l.mark.right() < l.wordmark.x, "the wordmark is drawn over the shield");
            assert!(l.wordmark.right() <= l.close_glyph.x, "the wordmark runs under the ✕");
            assert!(l.wordmark.bottom() <= l.title.y);
            assert!(l.title.right() <= l.close_glyph.x);
            assert!(l.close_glyph.right() <= WIDTH - MARGIN_X);
            assert!(l.title.bottom() <= l.header_rule.y);
            assert!(l.header_rule.bottom() <= l.body.y);
            assert_eq!((l.body.x, l.body.right()), (MARGIN_X, WIDTH - MARGIN_X));
            assert!(l.body_text.x > l.body.x && l.body_text.right() < l.body.right());
            assert!(l.body_text.y > l.body.y && l.body_text.bottom() < l.body.bottom());
            assert_eq!(l.body_text.h, lines * BODY_LINE_H);
            assert!(l.body.bottom() <= l.footer_rule.y);
            assert_eq!(l.footer.y, l.footer_rule.bottom());
            assert_eq!(l.footer.bottom(), l.window.bottom());
            assert!(l.ok.y >= l.footer.y);
            assert_eq!(l.ok.right(), WIDTH - MARGIN_X, "OK is not on the footer's right margin");
            assert_eq!(l.ok.bottom() + 8, l.window.bottom(), "the card is not sized to its footer");
        }
        // Outside the range, the layout is the range's ends.
        assert_eq!(layout(0), layout(MIN_BODY_LINES * BODY_LINE_H));
        assert_eq!(layout(i32::MAX / 4), layout(MAX_BODY_LINES * BODY_LINE_H));
        // And the measuring width `open` uses is the width painted into.
        assert_eq!(layout(0).body_text.w, layout(MAX_BODY_LINES * BODY_LINE_H).body_text.w);
    }

    #[test]
    fn every_word_this_card_paints_is_inside_its_window() {
        let notice = notice_for(PREFLIGHT);
        for lines in [MIN_BODY_LINES, 3, MAX_BODY_LINES] {
            let l = layout(lines * BODY_LINE_H);
            let runs = painted(&notice, lines * BODY_LINE_H);
            let roles: Vec<Role> = runs.iter().map(|r| r.role).collect();
            assert_eq!(roles, vec![Role::Title, Role::Body, Role::Button]);
            for run in &runs {
                assert!(!run.text.is_empty(), "the {:?} run is empty", run.role);
                assert!(
                    run.at.x >= 0
                        && run.at.y >= 0
                        && run.at.right() <= l.window.right()
                        && run.at.bottom() <= l.window.bottom(),
                    "{:?} is painted at {:?}, outside the window",
                    run.role,
                    run.at
                );
            }
        }
    }

    #[test]
    fn the_cards_words_and_dimensions_are_the_ones_it_promises() {
        assert_eq!(nothing_typed_title(), "Nothing was typed");
        assert_eq!(ok_label(), "OK");
        assert_eq!(BUTTON_H, crate::panel_draw::BUTTON_H);
        assert_eq!(WIDTH, crate::panel_draw::CARD_W);
        assert_ne!(nothing_typed_title(), stopped_title());
    }

    /// **The done card says what the design says**, and nothing it cannot know.
    #[test]
    fn the_done_card_says_the_designs_words_and_no_invented_timing() {
        assert_eq!(done_copy(DoneCase::Saved), ("Login saved", "Deskwarden fills it next time"));
        assert_eq!(done_copy(DoneCase::Updated).0, "Password updated");
        assert_eq!(done_copy(DoneCase::SignedIn).0, "Signed in");
        for case in [DoneCase::Saved, DoneCase::Updated, DoneCase::SignedIn] {
            let (title, subtitle) = done_copy(case);
            assert!(!title.is_empty() && !subtitle.is_empty());
            // D 1.4's `Filled by Deskwarden in 1.2 s` is a mock-up's stopwatch: no
            // timer here knows how long a fill took.
            assert!(!subtitle.contains(" in ") && !subtitle.chars().any(|c| c.is_ascii_digit()));
        }
    }

    /// The done card's disc is centred and everything lies inside its window.
    #[test]
    fn the_done_card_lays_out_a_centred_column_inside_its_window() {
        let l = done_layout();
        let r = crate::panel_draw::DONE_DISC / 2;
        assert_eq!(l.disc.0, l.window.w / 2, "the disc is not centred");
        assert!(l.disc.1 - r >= 0 && l.disc.0 - r >= 0 && l.disc.0 + r <= l.window.w);
        assert!(l.disc.1 + r < l.title.y, "the title runs into the disc");
        assert!(l.title.bottom() <= l.subtitle.y);
        for b in [l.title, l.subtitle] {
            assert!(b.x >= 0 && b.right() <= l.window.w && b.bottom() <= l.window.h);
        }
        assert_eq!(l.window.h, l.subtitle.bottom() + DONE_PAD);
    }

    /// **The done card never takes the keyboard.** It confirms a fill or a save
    /// the user just made; stealing the foreground from the app that was just
    /// typed into would be the card undoing what it confirms.
    #[test]
    fn the_done_card_is_opened_without_activating_anything() {
        let (production, _) = production();
        let code = code(&production);
        let a = code.find("pub(super) fn run_done").expect("control: run_done is gone");
        let b = code[a..].find("fn paint_done").expect("control: paint_done is gone") + a;
        let body = &code[a..b];
        assert!(body.contains("WS_EX_NOACTIVATE"));
        assert!(body.contains("SW_SHOWNOACTIVATE"));
        assert!(!body.contains("SetForegroundWindow("), "the done card asks for the foreground");
        assert!(!body.contains("SetFocus("), "the done card asks for the keyboard");
    }

    #[test]
    fn the_window_opens_under_a_title_nothing_else_uses() {
        assert!(!NOTICE_CARD_TITLE.is_empty());
        for other in [
            "Deskwarden",
            crate::prompt_card::PROMPT_CARD_TITLE,
            crate::picker_prompt::PICKER_PROMPT_TITLE,
            crate::unlock_prompt::UNLOCK_PROMPT_TITLE,
            crate::generate_prompt::GENERATE_PROMPT_TITLE,
            crate::locked_card::LOCKED_CARD_TITLE,
            crate::save_login_card::SAVE_LOGIN_CARD_TITLE,
            crate::vault_window::WINDOW_TITLE,
        ] {
            assert_ne!(NOTICE_CARD_TITLE, other);
        }
    }

    // ---- source pins -------------------------------------------------------

    fn production() -> (String, usize) {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("notice_card.rs");
        let raw = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let cut = raw.split(concat!("\n#[cfg(", "test)]\n")).next().unwrap().to_string();
        let discarded = raw.len() - cut.len();
        (cut, discarded)
    }

    fn code(source: &str) -> String {
        source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **The card takes the foreground once, excludes itself from capture
    /// once, never posts a thread quit, and is not a message box.**
    #[test]
    fn the_card_is_a_native_card_and_not_a_message_box() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: the `#[cfg(test)]` cut marker was not found");
        let code = code(&production);
        assert!(code.contains("WM_DESTROY =>"), "control: the window procedure is not in the cut");
        assert!(code.contains("CreateWindowExW("), "control: this card creates no window");
        assert_eq!(code.matches("SetForegroundWindow(").count(), 1);
        assert_eq!(code.matches("SetWindowDisplayAffinity(").count(), 1);
        assert!(code.contains("SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE)"));
        assert!(!code.contains(concat!("PostQuit", "Message")));
        assert!(!code.contains(concat!("MessageBox", "W")));
        assert!(!code.contains("run_ui_native("));
        // The keys `next` acts on are the ones the pure decision answers.
        assert!(code.contains("super::event_for_key(msg.wParam.0 as u16, held)"));
        assert!(code.contains("let held = (msg.lParam.0 >> 30) & 1 == 1;"));
    }
}

//! **Design 3b: the locked-vault card, in bare Win32.**
//!
//! The card the daemon puts beside a password field when the vault is
//! **locked**. It is the sixth surface in this crate drawn with
//! `CreateWindowExW` and GDI rather than with egui -- after
//! `crate::unlock_prompt`, `crate::picker_prompt`, `crate::generate_prompt`
//! and `crate::prompt_card` -- and it is one for the same measured reason they
//! are: the tray daemon measures 9.9 MB with no window ever opened, and the
//! first egui window takes it to ~60 MB resident **permanently**, because the
//! OpenGL driver's committed arenas survive the window's destruction.
//!
//! # It claims nothing about the vault's contents, and that is the whole card
//!
//! **This is a correction, not an addition, and the correction survives the
//! port.** Until 3b existed a locked vault reached design 3a's card and the
//! user was told "No saved login for `<app>`" -- a statement about the
//! contents of a vault this process cannot read. `main`'s
//! `stand_down_after_unlock` empties the match engine on every lock, so while
//! locked *every* window is unmatched, **including every window that does have
//! a saved login**, and 3a asserted the opposite of the truth about each of
//! them.
//!
//! So neither of [`locked_text`]'s two lines says whether the vault has a
//! login for this app, and neither counts anything. The design as drawn counts
//! them ("3 logins for Ledgerline Desktop"); this build cannot count them,
//! because the engine that would is exactly what the lock cleared, and a
//! number here would be the same lie in the other direction. Nor is Windows
//! Hello or a PIN offered: neither exists in this app.
//!
//! [`the_locked_card_claims_nothing_about_a_match`] is the egui card's own
//! forbidden-phrase test, carried across unchanged, and
//! [`every_word_this_card_paints_is_inside_its_window`] is the sibling that
//! checks each painted run is found exactly once and lies inside the window --
//! which on this renderer is a claim about [`painted`], the one list both the
//! test and the painter read.
//!
//! # What it offers, and why exactly one thing
//!
//! [`UNLOCK_LABEL`], and nothing else. Design 3a's *New login* would end in a
//! write through `bw serve` against an unlocked vault, and its *Search vault*
//! would open a vault window on a vault with nothing readable in it -- both
//! offers this state cannot honour, and a control on a frameless always-on-top
//! card that does nothing when clicked is worse than no control. Unlocking is
//! the one offer this state can honour, and [`crate::unlock_prompt`] is where
//! [`LockedAnswer::Unlock`] leads.
//!
//! # It keeps its anchor
//!
//! Like `crate::prompt_card` and unlike the daemon's other Win32 cards: this
//! one also appears unbidden, in response to a field being focused, and beside
//! that field is what makes it a reply to it. The arithmetic is
//! `crate::prompt_card::place`, reused rather than restated.
//!
//! # No secret is on this card or through its seam
//!
//! There is nothing here to leak: the card holds no item, no username and no
//! password, and it is shown precisely because the process cannot read one.

use crate::prompt_card::{place, Box2};

/// The window handle [`run_with`] deals in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockedWindow(pub isize);

/// What 3b answered: nothing, or "open the master-password prompt".
///
/// A two-variant enum rather than a `bool` because the call site in
/// `crate::app::locked_arm` reads as the two states it is and cannot be
/// silently inverted by a `!`.
///
/// A separate type from the account picker's `Outcome`, for the reason
/// `PromptPresenter::show_locked` is a separate method from its own: the two
/// cards make opposite claims about the vault, and the answers they may give
/// are disjoint. A shared enum would let a 3a dismissal be handed to the
/// unlock wiring, and vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockedAnswer {
    /// The ✕, Esc, or the window closing. Nothing follows, and in particular
    /// nothing is armed.
    #[default]
    Dismissed,
    /// *Unlock* was clicked: put [`crate::unlock_prompt`] on screen for this
    /// window, and if it answers with a session token, resume the fill this
    /// card interrupted.
    Unlock,
}

/// What the user did with the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The header ✕, or Escape.
    Cancel,
    /// The window went away underneath us. Treated exactly as `Cancel`.
    Closed,
    /// *Unlock* was clicked, or Enter pressed on it.
    Unlock,
}

/// How [`run_with`] finished.
///
/// **A third variant [`LockedAnswer`] does not have.** A window that could not
/// be put on screen is not the user dismissing anything, and the caller may
/// want to fall back; `crate::app`'s presenter collapses it to `Dismissed`,
/// which is the conservative reading -- nothing was shown, so the user cannot
/// have pressed anything, and answering `Unlock` would put a modal
/// master-password prompt up for a card that was never on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Unlock,
    Dismissed,
    Unavailable,
}

/// Opens the card for an app name at an anchor.
pub type OpenLockedFn = fn(&str, Option<(f32, f32)>) -> Option<LockedWindow>;

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything.
pub struct LockedCalls {
    /// Lays out and shows the card for this app name, at this anchor.
    pub open: OpenLockedFn,
    /// `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the **top-level**
    /// window, called before the first `next`. Windows refuses it on a child
    /// control with `E_INVALIDARG`.
    pub protect: fn(LockedWindow) -> bool,
    /// Pumps until the user does something.
    pub next: fn(LockedWindow) -> Event,
    /// Destroys the window and releases its resources.
    pub close: fn(LockedWindow),
}

/// **The whole decision, and the only part of this module a test can run.**
///
/// `protect` runs immediately after `open` and before the first `next`, and
/// `close` runs on every exit path. `open` answering `None` returns before it,
/// because there is no window to close there.
pub fn run_with(
    calls: &LockedCalls,
    app_name: &str,
    anchor: Option<(f32, f32)>,
) -> Outcome {
    let Some(window) = (calls.open)(app_name, anchor) else {
        log::warn!("the locked-vault card could not be put on screen");
        return Outcome::Unavailable;
    };

    if !(calls.protect)(window) {
        log::warn!(
            "SetWindowDisplayAffinity was refused for the locked-vault card; the app name it \
             shows is visible to screen capture on this machine"
        );
    }

    // **One `next`, not a loop**, and the difference is `next`'s own
    // contract: it "pumps until the user does something", so the pumping
    // already happens inside it and every [`Event`] it can return is a
    // decision. A `loop` here was a second pump around a first one whose
    // every arm returns -- `clippy::never_loop` is right about it, and it was
    // a denied lint failing this crate's `cargo clippy` rather than a style
    // note.
    //
    // If an `Event` is ever added that means "keep waiting", it goes back to
    // a loop and this comment is why the change is deliberate rather than a
    // revert.
    match (calls.next)(window) {
        Event::Cancel | Event::Closed => {
            (calls.close)(window);
            Outcome::Dismissed
        }
        Event::Unlock => {
            (calls.close)(window);
            Outcome::Unlock
        }
    }
}

/// The window's title. **Unique across this process**, because
/// `crate::foreground::pick` finds a window by title and takes the FIRST match
/// in `EnumWindows` order.
///
/// The egui card this replaces opened under the bare literal `"Deskwarden"` --
/// the same title three other windows of this process open under.
pub const LOCKED_CARD_TITLE: &str = "Deskwarden vault locked";

/// The status pill in the header (design D 1.6): the state, in one word.
///
/// **It must not be the body's first line**: the two are different claims --
/// the pill names the state, the title explains it -- and a pill that repeated
/// the title would leave the card saying one thing twice. The design's own
/// title, "3 logins match", is not used: the process cannot know how many
/// logins match while the vault is locked, and a number here would be the old
/// lie in a new place.
pub fn locked_pill() -> &'static str {
    crate::i18n::t("locked_card.pill")
}

/// What 3b's one button says, and **the only card that may carry it**.
///
/// **It is not "Unlock Deskwarden"**, which is
/// [`crate::unlock_prompt::UNLOCK_PROMPT_TITLE`]: a button says what pressing
/// it does, and the window it opens says what it is.
pub fn unlock_label() -> &'static str {
    crate::i18n::t("locked_card.unlock")
}

/// The number of `crate::app::overlay_height` choice-row pitches the
/// caller's placement is computed from.
///
/// Kept at the `1` the egui card was sized by, because it is the argument
/// `crate::app::locked_arm` hands the presenter's `position` and that
/// arithmetic is the caller's, not this card's. **This card's own clamp is
/// against its own height**, in [`crate::prompt_card::place`], which is what
/// makes the number here an approximation the window then corrects rather than
/// a size anything is drawn at.
pub const LOCKED_ROWS: usize = 1;

/// The two lines of the card's body.
///
/// **Neither says whether the vault has a login for `app_name`**, and that is
/// the whole correction: the process cannot know while locked, so it says what
/// it can know instead -- that it is locked, and that unlocking is what
/// answers the question.
///
/// **The second line's verb is "check", not "see".** With [`UNLOCK_LABEL`] in
/// the footer the instruction is the button, so the line says what the button
/// will *find out*. "See the login" would be the old lie in a new place: it
/// promises there is one.
///
/// `app_name` is `crate::app::window_label`'s answer, so it is user-controlled
/// -- which is why the line it lands on is drawn with `DT_END_ELLIPSIS` into a
/// fixed box rather than being allowed to grow a card that cannot scroll.
pub fn locked_text(app_name: &str) -> (String, String) {
    (
        crate::i18n::t("locked_card.title").to_string(),
        crate::i18n::tf("locked_card.body", &[("app_name", app_name)]),
    )
}

/// What kind of run a [`Painted`] is, which is what the painter reads to pick
/// its font and its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The status pill's word.
    Pill,
    /// The title (19/800).
    Primary,
    /// The body line under it (15).
    Secondary,
    /// The *Unlock* button's label. Painted by the control, not by the card.
    Button,
}

/// One run of text the card paints, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Painted {
    pub text: String,
    pub at: Box2,
    pub role: Role,
}

/// **Every word this card paints, with the box it is painted in.**
///
/// The one list, read by the painter and by
/// [`every_word_this_card_paints_is_inside_its_window`] alike. That is the
/// point of it: the egui card's equivalent test walked the glyph runs egui had
/// laid out, which is a thing no test can do on a GDI surface -- so the runs
/// are named here instead, and the painter is what has no words of its own.
///
/// The `Unlock` label is in the list even though a child control paints it,
/// because "the card says this, here" is the claim, and which window handle
/// carries the pixels is not part of it.
pub fn painted(app_name: &str) -> Vec<Painted> {
    let l = layout();
    let (primary, secondary) = locked_text(app_name);
    vec![
        Painted { text: locked_pill().to_string(), at: l.pill, role: Role::Pill },
        Painted { text: primary, at: l.primary, role: Role::Primary },
        Painted { text: secondary, at: l.secondary, role: Role::Secondary },
        Painted { text: unlock_label().to_string(), at: l.unlock, role: Role::Button },
    ]
}

/// **Puts design 3b on screen and answers whether the user asked to unlock.**
///
/// The same signature `overlay_ui::show_locked_overlay` had, so
/// `crate::app::REAL_OVERLAY` changes by one path and nothing else.
pub fn show_locked_card(app_name: &str, anchor: Option<(f32, f32)>) -> LockedAnswer {
    ask_with(&REAL, app_name, anchor)
}

/// [`show_locked_card`], told which [`LockedCalls`] to use.
///
/// `examples/locked_preview.rs` is its one non-production caller, swapping
/// [`LockedCalls::protect`] for a stub so the window can be screenshotted.
pub fn ask_with(
    calls: &LockedCalls,
    app_name: &str,
    anchor: Option<(f32, f32)>,
) -> LockedAnswer {
    match run_with(calls, app_name, anchor) {
        Outcome::Unlock => LockedAnswer::Unlock,
        // **`Unavailable` is `Dismissed`, and that is the conservative
        // reading rather than an arbitrary one**: nothing was put on screen,
        // so the user cannot have pressed anything, and answering `Unlock`
        // would put a modal master-password prompt up for a card that was
        // never shown.
        Outcome::Dismissed | Outcome::Unavailable => LockedAnswer::Dismissed,
    }
}

/// The production [`LockedCalls`].
pub static REAL: LockedCalls = LockedCalls {
    open: win32::open,
    protect: win32::protect,
    next: win32::next,
    close: win32::close,
};

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// The card's width: design 1b's Locked panel, 410.
pub const WIDTH: i32 = crate::panel_draw::CARD_W;

/// The card's padding on every side (design D 1.6: `pad 18`).
const MARGIN_X: i32 = crate::panel_draw::PAD;
const MARGIN_TOP: i32 = crate::panel_draw::PAD;

/// The gap between the card's blocks (D 1.6: `gap 14`), and between the title and
/// the body under it (D 1.5's title block: `gap 4`).
const BLOCK_GAP: i32 = crate::panel_draw::BLOCK_GAP;
const TITLE_GAP: i32 = 4;

/// The title's and the body's line boxes.
const TITLE_H: i32 = 26;
const BODY_H: i32 = 22;

/// Button height: the Locked panel's tall one (D 1.6: `h54, r15`).
const BUTTON_H: i32 = crate::panel_draw::TALL_BUTTON_H;

/// The status pill's width: "Locked" at 12/700 and the design's 10 px padding.
const PILL_W: i32 = 80;

/// Every rectangle the card paints, computed once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    /// The brand lockup's shield, and "Deskwarden" beside it. **Not optional**: a
    /// frameless always-on-top window that names the user's apps and asks for
    /// their master password has to say whose window it is.
    pub mark: Box2,
    pub wordmark: Box2,
    /// The `Locked` pill, left of the ✕ on the lockup's line.
    pub pill: Box2,
    pub close_glyph: Box2,
    pub primary: Box2,
    pub secondary: Box2,
    pub unlock: Box2,
}

/// **The card's geometry. There is exactly one shape.**
///
/// Nothing about this card varies at runtime: it has no rows, no modes, no
/// second step and no count, and its one user-controlled string is drawn into
/// a fixed box with an ellipsis. The window is sized to this content and to
/// nothing else.
pub fn layout() -> Layout {
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
    let pill = Box2 {
        x: close_glyph.x - crate::panel_draw::BUTTON_GAP - PILL_W,
        y: mark.y + (mark.h - crate::panel_draw::PILL_H) / 2,
        w: PILL_W,
        h: crate::panel_draw::PILL_H,
    };

    // Two lines, both drawn with `DT_END_ELLIPSIS` into these boxes: the second
    // carries `app_name`, and this window cannot grow and cannot scroll.
    let primary = Box2 { x: MARGIN_X, y: mark.bottom() + BLOCK_GAP, w: content_w, h: TITLE_H };
    let secondary =
        Box2 { x: MARGIN_X, y: primary.bottom() + TITLE_GAP, w: content_w, h: BODY_H };
    let unlock = Box2 { x: MARGIN_X, y: secondary.bottom() + BLOCK_GAP, w: content_w, h: BUTTON_H };

    let window = Box2 { x: 0, y: 0, w: WIDTH, h: unlock.bottom() + MARGIN_TOP };

    Layout { window, mark, wordmark, pill, close_glyph, primary, secondary, unlock }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// Whether the window has gone away underneath the pump.
static GONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the window procedure recorded, taken by `next` rather than read.
static PENDING: std::sync::Mutex<Option<Event>> = std::sync::Mutex::new(None);

/// The name of the app the card was opened in front of.
static APP_NAME: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// # Why every pixel here is painted by hand
///
/// `crate::unlock_prompt`'s `win32` module carries the whole argument. Every
/// control here is a real `BUTTON` window -- which is what buys focus, the
/// space bar and `IsDialogMessage` traversal -- with its painting handed to
/// [`crate::win32_draw`], the module every card draws through so none can
/// drift from the palette.
///
/// # GDI only
///
/// Nothing here creates a Direct2D or Direct3D device. That is measured rather
/// than stylistic: a D2D device was measured at 53.85 MB against this kind of
/// window's 1.79 MB.
mod win32 {
    use super::{Painted, Role, APP_NAME, GONE, LOCKED_CARD_TITLE, PENDING};
    use crate::prompt_card::Box2;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        CreateFontIndirectW, CreateSolidBrush, DeleteDC, DeleteObject,
        EndPaint, FillRect, GetDC, GetDeviceCaps, InvalidateRect, ReleaseDC, SelectObject,
        SetBkMode, SetTextColor, CLEARTYPE_QUALITY, DT_END_ELLIPSIS, DT_LEFT,
        DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, LOGFONTW,
        LOGPIXELSX, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
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

    use crate::panel_draw::{self, PanelButton};
    use crate::win32_draw::{draw_button_with_shortcut, draw_text, rgb};

    const ID_UNLOCK: usize = 101;
    const CLASS_NAME: PCWSTR = w!("DeskwardenVaultLockedCard");

    /// The window's DPI as a percentage of 96, sampled once per open. The
    /// system DPI, not the monitor's -- `unlock_prompt`'s own `DPI_PERCENT`
    /// carries the whole argument.
    static DPI_PERCENT: AtomicI32 = AtomicI32::new(100);

    fn scale(v: i32) -> i32 {
        v * DPI_PERCENT.load(Ordering::SeqCst) / 100
    }

    static HOVERED: AtomicIsize = AtomicIsize::new(0);
    static ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);

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
        /// The wordmark: "Deskwarden" at 15/800.
        brand: HFONT,
        /// The status pill's word, 12/700.
        pill: HFONT,
        /// The title, 19/800, and the body under it, 15.
        primary: HFONT,
        secondary: HFONT,
        /// The *Unlock* button's label, 16/700, and its `Enter` chip.
        button: HFONT,
        hint: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, EXTRABOLD, REGULAR};
            Fonts {
                brand: font(EXTRABOLD, panel_draw::WORD_PX),
                pill: font(BOLD, panel_draw::PILL_PX),
                primary: font(EXTRABOLD, panel_draw::TITLE_PX),
                secondary: font(REGULAR, panel_draw::BODY_PX),
                button: font(BOLD, 16),
                hint: mono(crate::theme::CHIP_TEXT_PX as i32),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [self.brand, self.pill, self.primary, self.secondary, self.button, self.hint]
                {
                    let _ = DeleteObject(f);
                }
            }
        }

        /// The face and the ink one [`Role`] is drawn in. **One table**, so a
        /// run in [`super::painted`] cannot be painted in a colour nothing
        /// chose.
        fn skin(&self, role: Role) -> (HFONT, eframe::egui::Color32) {
            match role {
                Role::Pill => (self.pill, crate::theme::BLUE_DEEP),
                Role::Primary => (self.primary, crate::theme::INK),
                Role::Secondary => (self.secondary, crate::theme::TEXT_FAINT),
                Role::Button => (self.button, crate::theme::CARD),
            }
        }
    }

    static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
    // `Fonts` holds raw GDI handles, which are process-wide rather than
    // thread-owned. The card is modal on one thread, so nothing shares them.
    unsafe impl std::marker::Send for Fonts {}

    // ---- the window --------------------------------------------------------

    pub(super) fn open(
        app_name: &str,
        anchor: Option<(f32, f32)>,
    ) -> Option<super::LockedWindow> {
        register_fonts();
        GONE.store(false, Ordering::SeqCst);
        HOVERED.store(0, Ordering::SeqCst);
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = APP_NAME.lock() {
            *slot = app_name.to_string();
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
        // **Destroy the previous set before overwriting it.** `Fonts` has no
        // `Drop` -- it holds raw `HFONT`s -- so assigning over a `Some` would
        // leak five fonts per `open` that ran without a matching `close`.
        {
            let mut slot = FONTS.lock().ok()?;
            if let Some(previous) = slot.take() {
                previous.destroy();
            }
            *slot = Some(Fonts::build());
        }

        let l = super::layout();
        let (w, h) = (scale(l.window.w), scale(l.window.h));
        let (x, y) = super::place(anchor, work_area(), w, h);

        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                CLASS_NAME,
                &HSTRING::from(LOCKED_CARD_TITLE),
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
        // style, so a bare `?` here would leave a frameless topmost card with
        // no controls and no way for the user to dismiss it.
        fn abandon(window: HWND) -> Option<super::LockedWindow> {
            unsafe {
                let _ = DestroyWindow(window);
            }
            if let Ok(mut slot) = FONTS.lock() {
                if let Some(fonts) = slot.take() {
                    fonts.destroy();
                }
            }
            if let Ok(mut slot) = APP_NAME.lock() {
                slot.clear();
            }
            None
        }

        // The handle is copied out and the guard dropped at the end of this
        // statement: `abandon` locks `FONTS` itself.
        let Some(button_font) =
            FONTS.lock().ok().and_then(|guard| guard.as_ref().map(|f| f.button))
        else {
            return abandon(window);
        };

        let Some(control) = child(window, l.unlock, ID_UNLOCK, button_font) else {
            return abandon(window);
        };
        subclass(control);

        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            // Allowed to refuse, and handled rather than asserted -- the
            // property `foreground` records. This card advertises `Esc
            // Dismiss` and has a button Enter presses, so it has to be able to
            // receive them.
            let _ = SetForegroundWindow(window);
            let _ = SetFocus(control);
        }

        Some(super::LockedWindow(handle_of(window)))
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

    /// **The protection, on the top-level window.**
    ///
    /// Never on a child: Windows refuses `SetWindowDisplayAffinity` on a child
    /// control with `E_INVALIDARG`, and the top-level flag covers every child
    /// it owns. What it protects here is not a credential -- this card holds
    /// none -- but the name of the app this user is signing into.
    pub(super) fn protect(window: super::LockedWindow) -> bool {
        if !crate::screen_share::capture_protection_enabled() {
            return true;
        }
        unsafe { SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE).is_ok() }
    }

    /// Pumps until the user does something.
    ///
    /// **Escape is handled before `IsDialogMessageW`**, which only cancels for
    /// a real dialog box. Enter is not intercepted at all: the *Unlock* button
    /// has focus, so `IsDialogMessage` turns Enter into its `BN_CLICKED` --
    /// which is the right answer here and, unlike on the matched card, the
    /// only one there is.
    pub(super) fn next(window: super::LockedWindow) -> Event {
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
            // Idle. Nothing on this card animates.
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    }

    pub(super) fn close(window: super::LockedWindow) {
        unsafe {
            let _ = DestroyWindow(hwnd(window.0));
        }
        if let Ok(mut slot) = FONTS.lock() {
            if let Some(fonts) = slot.take() {
                fonts.destroy();
            }
        }
        // Not a secret, but it is the name of an app this user was in front
        // of, and nothing needs it once the card is down.
        if let Ok(mut slot) = APP_NAME.lock() {
            slot.clear();
        }
        if let Ok(mut slot) = PENDING.lock() {
            *slot = None;
        }
    }

    // ---- plumbing ----------------------------------------------------------

    use super::Event;

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

    fn take_pending() -> Option<Event> {
        PENDING.lock().ok().and_then(|mut slot| slot.take())
    }

    fn set_pending(event: Event) {
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(event);
        }
    }

    /// The runs the paint path draws, for the app the card was opened over.
    fn runs() -> Vec<Painted> {
        let name = APP_NAME.lock().map(|slot| slot.clone()).unwrap_or_default();
        super::painted(&name)
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
                // whole client area is painted from one back buffer.
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
    }

    /// The one child control, created with **no text**: its label is painted
    /// from the app's own palette and type, so a caption would only ever be a
    /// second, stale copy.
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
                if HOVERED.swap(0, Ordering::SeqCst) != 0 {
                    repaint(window);
                    if let Ok(control) = GetDlgItem(window, ID_UNLOCK as i32) {
                        repaint(control);
                    }
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = wparam.0 & 0xffff;
                let notification = ((wparam.0 >> 16) & 0xffff) as u32;
                if notification == BN_CLICKED && id == ID_UNLOCK {
                    set_pending(Event::Unlock);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // **NO `PostQuitMessage` HERE, EVER.** This window is opened
                // on the daemon thread, and that thread goes on to run egui
                // windows -- and, on this card's own answer, the bare-Win32
                // unlock prompt and then whatever the resumed fill leads to.
                // `close()` calls `DestroyWindow`, which dispatches this
                // message synchronously on that thread, so a `PostQuitMessage`
                // here leaves the thread's quit flag set with nothing left to
                // drain it: `next()` has already returned and no pump of ours
                // runs again. The next `eframe::run_native` then takes that
                // stale `WM_QUIT` out of `GetMessageW`, leaves its loop before
                // it draws a frame, and returns its default answer -- so the
                // window never appears.
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

    /// The close glyph's rect in DEVICE pixels.
    ///
    /// One derivation, read by both the hit test and `in_close_glyph`, so the
    /// rect `WM_NCHITTEST` excuses from the drag and the rect `WM_LBUTTONDOWN`
    /// answers on can never be two different rectangles.
    fn close_glyph_rect() -> RECT {
        let l = super::layout();
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

    // ---- painting ----------------------------------------------------------

    /// The card's own surface.
    ///
    /// **Its words are [`super::painted`]'s and none of its own.** Every run
    /// except the button's -- which its own control paints -- is drawn from
    /// that list, at the box the list gives it, in the face and ink
    /// [`Fonts::skin`] gives its role. That is what makes
    /// `every_word_this_card_paints_is_inside_its_window` a claim about the
    /// painting rather than about a second list beside it.
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
            let l = super::layout();

            fill_rect(mem, client, crate::theme::CARD);
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                let dev = |b: Box2| RECT {
                    left: scale(b.x),
                    top: scale(b.y),
                    right: scale(b.right()),
                    bottom: scale(b.bottom()),
                };
                panel_draw::draw_lockup(mem, dev(l.mark), dev(l.wordmark), fonts.brand);
                for run in runs() {
                    match run.role {
                        // The control paints its own.
                        Role::Button => {}
                        // The status pill: the design's pale blue wash.
                        Role::Pill => panel_draw::draw_pill(
                            mem,
                            fitted_pill(mem, fonts.pill, dev(run.at), &run.text),
                            &run.text,
                            fonts.pill,
                            crate::theme::BLUE_WASH,
                            crate::theme::BLUE_DEEP,
                        ),
                        role => {
                            let (face, ink) = fonts.skin(role);
                            text(mem, face, run.at, &run.text, ink);
                        }
                    }
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

    /// The *Unlock* button.
    fn paint_control(control: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(control, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(control, &mut rc);

            let hovered = HOVERED.load(Ordering::SeqCst) == ID_UNLOCK as isize;

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            let whole = RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom };

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            // The button sits on the card's white, which is what its rounded
            // corners show through.
            fill_rect(mem, whole, crate::theme::CARD);
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                let label = runs()
                    .into_iter()
                    .find(|r| r.role == Role::Button)
                    .map(|r| r.text)
                    .unwrap_or_default();
                // The design's deep blue, hovering to brand blue.
                let skin = panel_draw::skin(PanelButton::Deep, hovered);
                let dpi = DPI_PERCENT.load(Ordering::SeqCst);
                // The card's ONLY button: Enter plainly presses it, so it
                // carries no `ENTER` chip (the owner's call; the chip is kept
                // for cards with a choice of buttons).
                let hint: Option<(&str, windows::Win32::Graphics::Gdi::HFONT)> = None;
                // **No focus ring.** It is the card's only control and it always
                // holds the focus, so a ring would be a permanent second border
                // around the button -- which the design (D 1.6) does not have.
                draw_button_with_shortcut(
                    mem,
                    whole,
                    &label,
                    fonts.button,
                    skin,
                    scale(panel_draw::TALL_BUTTON_RADIUS),
                    hint,
                    dpi,
                );
            }
            drop(guard);

            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            let _ = EndPaint(control, &ps);
        }
    }

    /// The pill's device rect, **as wide as its word and the design's 10 px
    /// padding** (D 1.6: `pad 4x10`), right-aligned in the layout's box and
    /// never wider than it. The box is the room a translation may take; a
    /// fixed-width pill left "Locked" floating in a blue lozenge.
    fn fitted_pill(hdc: HDC, font: HFONT, room: RECT, word: &str) -> RECT {
        let chars: Vec<u16> = word.encode_utf16().collect();
        let text_w = crate::win32_draw::text_width_utf16(hdc, font, &chars);
        let w = panel_draw::pill_width(text_w, DPI_PERCENT.load(Ordering::SeqCst))
            .min(room.right - room.left);
        RECT { left: room.right - w, ..room }
    }

    /// The header's close glyph, drawn as two strokes because no bundled face
    /// has it at this weight.
    ///
    /// **It matters more on this card than anywhere.** This window is raised
    /// in response to ANOTHER app being foregrounded, which is exactly the
    /// situation Windows' foreground lock may refuse focus for -- so Esc is
    /// not guaranteed to arrive, and the ✕ is the only mouse-operable way out
    /// of a window with no title bar.
    fn paint_close_glyph(hdc: HDC, at: Box2) {
        let rc = RECT {
            left: scale(at.x),
            top: scale(at.y),
            right: scale(at.right()),
            bottom: scale(at.bottom()),
        };
        panel_draw::draw_close_x(hdc, rc, scale(1).max(1), crate::theme::TEXT_FAINT);
    }

    fn fill_rect(hdc: HDC, rc: RECT, colour: eframe::egui::Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(colour));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(brush);
        }
    }

    /// One run of text, left-aligned, vertically centred, and **truncated with
    /// an ellipsis rather than clipped mid-letter**.
    ///
    /// `DT_END_ELLIPSIS` is load-bearing on this card: its second line carries
    /// `app_name`, which is `crate::app::window_label`'s answer and therefore
    /// user-controlled, and this window cannot grow and cannot scroll.
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
            // `win32_draw::draw_text`, not `DrawTextW`: an empty run through
            // the raw call is an access violation at address 0x2 that kills
            // the whole daemon without a log line. The guard that used to be
            // written out here now lives in that wrapper, which is the only
            // place in the crate that calls `DrawTextW` -- see the block
            // comment above it for the full account of both occurrences.
            //
            // `DT_NOPREFIX`: these are the app's own words and a user's app
            // name, in which an `&` is an ampersand and never a mnemonic.
            draw_text(
                hdc,
                run,
                &mut rc,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            SelectObject(hdc, old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "Ledgerline Desktop";

    /// The adversarial app names the egui card's geometry was measured
    /// against. `app_name` is `crate::app::window_label`'s answer, so all four
    /// are strings a user can produce.
    const FIXTURES: [(&str, &str); 4] = [
        ("short", "Ledgerline"),
        ("long", "Northwind Group Consolidated Accounts Portal (production)"),
        ("unbreakable", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("cjk", "株式会社ノースウィンド・コンソリデーテッド・アカウンツ"),
    ];

    fn inert() -> LockedCalls {
        LockedCalls {
            open: |_, _| Some(LockedWindow(1)),
            protect: |_| true,
            next: |_| Event::Cancel,
            close: |_| {},
        }
    }

    // ---- the claim ---------------------------------------------------------

    /// **The locked card claims nothing about whether a match exists.**
    ///
    /// Carried across from the egui card unchanged, forbidden-phrase list
    /// included. While locked the match engine is empty, so *every* window is
    /// unmatched -- including every window that does have a saved login -- and
    /// a card that said "No saved login for X" would be asserting the opposite
    /// of the truth about each of them.
    #[test]
    fn the_locked_card_claims_nothing_about_a_match() {
        let (primary, secondary) = locked_text(APP);
        for line in [&primary, &secondary] {
            for phrase in ["No saved login", "logins for", "nothing for"] {
                assert!(
                    !line.contains(phrase),
                    "the locked card says {line:?}, which contains {phrase:?} -- a claim about \
                     the contents of a vault this process cannot read"
                );
            }
        }
        assert!(
            primary.contains("locked"),
            "control: the locked card does not say it is locked, so the assertions above are \
             about a card that says nothing useful at all"
        );
        assert!(
            secondary.contains(APP),
            "control: the locked card never names the window it appeared over"
        );
    }

    /// **The forbidden phrases are forbidden of every word on the card**, not
    /// only of the two body lines.
    ///
    /// The egui test read `locked_text` alone, which was all there was to
    /// read; on this renderer the header, the button and the footer hint are
    /// separately nameable, so the same rule is applied to all of them. A
    /// header reading "No saved login" would be the identical defect one
    /// element up.
    #[test]
    fn no_word_anywhere_on_the_card_claims_a_match() {
        let mut checked = 0;
        for (name, app) in FIXTURES {
            for run in painted(app) {
                for phrase in ["No saved login", "logins for", "nothing for", "match"] {
                    assert!(
                        !run.text.contains(phrase),
                        "the {name:?} fixture's card paints {:?} in its {:?}, which contains \
                         {phrase:?} -- a claim about the contents of a vault this process \
                         cannot read",
                        run.text,
                        run.role
                    );
                }
                checked += 1;
            }
        }
        assert_eq!(checked, FIXTURES.len() * 4, "the loop did not cover every run");
    }

    /// **The card counts nothing.** The design as drawn counts matches ("3
    /// logins for ..."); this build cannot count them, because the engine that
    /// would is exactly what the lock cleared, and a number here would be the
    /// same lie in the other direction.
    #[test]
    fn nothing_the_card_says_is_a_number() {
        for run in painted(APP) {
            assert!(
                !run.text.chars().any(|c| c.is_ascii_digit()),
                "the locked card paints {:?}, which contains a digit -- and the only thing there \
                 would be to count is a vault this process cannot read",
                run.text
            );
        }
    }

    /// **The card offers exactly one thing, and it is the one this state can
    /// honour.**
    ///
    /// Design 3a's two offers would each be an offer the process cannot keep:
    /// *New login* ends in a write through `bw serve` against an unlocked
    /// vault, and *Search vault* opens a window onto a vault with nothing
    /// readable in it.
    #[test]
    fn the_card_offers_only_the_unlock_it_can_honour() {
        let buttons: Vec<String> = painted(APP)
            .into_iter()
            .filter(|r| r.role == Role::Button)
            .map(|r| r.text)
            .collect();
        assert_eq!(buttons, vec![unlock_label().to_string()]);
        let all = painted(APP)
            .into_iter()
            .map(|r| r.text)
            .collect::<Vec<_>>()
            .join(" ");
        for forbidden in ["New login", "Search vault"] {
            assert!(
                !all.contains(forbidden),
                "the locked card carries {forbidden:?}, which is an offer nothing on this path \
                 can honour while the vault cannot be read"
            );
        }
    }

    /// **Every word this card paints is painted once, and inside the window.**
    ///
    /// The egui sibling walked the glyph runs egui had laid out, which is a
    /// thing no test can do on a GDI surface. So the runs are named by
    /// [`painted`] -- the one list the painter itself reads -- and this checks
    /// each is distinct and each box lies inside the window. That window is
    /// frameless, always-on-top, and has no scrollbar and no title bar to
    /// drag.
    #[test]
    fn every_word_this_card_paints_is_inside_its_window() {
        let l = layout();
        let mut checked = 0;
        for (name, app) in FIXTURES {
            let runs = painted(app);
            assert_eq!(runs.len(), 4, "the {name:?} card paints a different number of runs");
            for run in &runs {
                assert!(!run.text.is_empty(), "the {:?} run is empty", run.role);
                assert_eq!(
                    runs.iter().filter(|other| other.text == run.text).count(),
                    1,
                    "the {name:?} card paints {:?} more than once, so a test finding it cannot \
                     say which one it found",
                    run.text
                );
                assert!(
                    run.at.x >= 0
                        && run.at.y >= 0
                        && run.at.right() <= l.window.right()
                        && run.at.bottom() <= l.window.bottom(),
                    "the {name:?} card paints {:?} at {:?}, outside the {}x{} window it asks the \
                     OS for. There is no scrollbar and no title bar to drag",
                    run.text,
                    run.at,
                    l.window.w,
                    l.window.h
                );
                assert!(run.at.w > 0 && run.at.h > 0, "{:?} has no room to be drawn", run.role);
                checked += 1;
            }
        }
        assert_eq!(checked, FIXTURES.len() * 4);
    }

    /// **No app name a user can supply changes the card's shape.**
    ///
    /// The egui sibling measured the card's height, which grew when its second
    /// line wrapped. Nothing here can wrap -- the line is drawn into a fixed
    /// box with `DT_END_ELLIPSIS` -- so the claim is the stronger one: the
    /// geometry does not depend on the name at all.
    #[test]
    fn no_app_name_a_user_can_supply_changes_the_cards_shape() {
        let baseline: Vec<Box2> = painted(APP).into_iter().map(|r| r.at).collect();
        for (name, app) in FIXTURES {
            let boxes: Vec<Box2> = painted(app).into_iter().map(|r| r.at).collect();
            assert_eq!(
                boxes, baseline,
                "the {name:?} fixture's app name moved the card's boxes. The window is a fixed \
                 {}x{} with no scrollbar, so anything that moved is clipped off it",
                layout().window.w,
                layout().window.h
            );
        }
    }

    // ---- the decision ------------------------------------------------------

    #[test]
    fn the_window_is_protected_before_it_is_ever_pumped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static ORDER: AtomicUsize = AtomicUsize::new(0);
        static PROTECTED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        static PUMPED_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
        let calls = LockedCalls {
            protect: |_| {
                PROTECTED_AT.store(ORDER.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                true
            },
            next: |_| {
                // Record only the FIRST pump; otherwise the last write wins
                // and the assertion below passes even if an earlier pump ran
                // before protect.
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
        let _ = run_with(&calls, APP, None);
        assert!(
            PROTECTED_AT.load(Ordering::SeqCst) < PUMPED_AT.load(Ordering::SeqCst),
            "the card names the app this user is signing into; a window that can be read before \
             it is excluded from capture is a window a recorder catches that in"
        );
    }

    #[test]
    fn a_window_that_cannot_be_opened_is_unavailable_and_not_a_silent_nothing() {
        let calls = LockedCalls { open: |_, _| None, ..inert() };
        assert_eq!(run_with(&calls, APP, None), Outcome::Unavailable);
    }

    /// **A card that was never shown does not ask for a master password.**
    #[test]
    fn a_card_that_could_not_be_shown_answers_dismissed() {
        let calls = LockedCalls { open: |_, _| None, ..inert() };
        assert_eq!(
            ask_with(&calls, APP, None),
            LockedAnswer::Dismissed,
            "nothing was put on screen, so the user cannot have pressed anything -- and `Unlock` \
             would put a modal master-password prompt up for a card that was never shown"
        );
    }

    #[test]
    fn every_exit_path_closes_the_window() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CLOSED: AtomicUsize = AtomicUsize::new(0);
        // one-off test table; an alias would only be read here
        #[allow(clippy::type_complexity)]
        let paths: [(fn(LockedWindow) -> Event, Outcome, LockedAnswer); 3] = [
            (|_| Event::Cancel, Outcome::Dismissed, LockedAnswer::Dismissed),
            (|_| Event::Closed, Outcome::Dismissed, LockedAnswer::Dismissed),
            (|_| Event::Unlock, Outcome::Unlock, LockedAnswer::Unlock),
        ];
        for (next, outcome, answer) in paths {
            CLOSED.store(0, Ordering::SeqCst);
            let calls = LockedCalls {
                next,
                close: |_| {
                    CLOSED.fetch_add(1, Ordering::SeqCst);
                },
                ..inert()
            };
            assert_eq!(run_with(&calls, APP, None), outcome);
            assert_eq!(
                CLOSED.load(Ordering::SeqCst),
                1,
                "{outcome:?} left the window on screen -- and this card's answer opens a modal \
                 master-password prompt, which would then be behind it"
            );
            let calls = LockedCalls { next, ..inert() };
            assert_eq!(ask_with(&calls, APP, None), answer);
        }
    }

    /// **The anchor the caller computed is the anchor the window opens at.**
    #[test]
    fn the_card_opens_at_the_anchor_it_was_given() {
        static SEEN: std::sync::Mutex<Option<Option<(f32, f32)>>> = std::sync::Mutex::new(None);
        let calls = LockedCalls {
            open: |_, anchor| {
                if let Ok(mut slot) = SEEN.lock() {
                    *slot = Some(anchor);
                }
                Some(LockedWindow(1))
            },
            ..inert()
        };
        let _ = run_with(&calls, APP, Some((640.0, 480.0)));
        assert_eq!(*SEEN.lock().unwrap(), Some(Some((640.0, 480.0))));
    }

    /// **The app the card was told about is the app it names.**
    #[test]
    fn the_card_is_opened_for_the_app_it_was_told_about() {
        static SEEN: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
        let calls = LockedCalls {
            open: |name, _| {
                if let Ok(mut slot) = SEEN.lock() {
                    *slot = name.to_string();
                }
                Some(LockedWindow(1))
            },
            ..inert()
        };
        let _ = run_with(&calls, APP, None);
        assert_eq!(*SEEN.lock().unwrap(), APP);
    }

    // ---- geometry ----------------------------------------------------------

    /// **Nothing the card lays out falls off it.**
    #[test]
    fn nothing_the_card_lays_out_falls_off_it() {
        let l = layout();
        assert_eq!(l.window.w, WIDTH);
        assert_eq!((l.window.x, l.window.y), (0, 0));

        // **The brand lockup**, which a window that names the user's apps and
        // asks for their master password has to carry.
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
            "the mark's box is not the design artboard's ratio, so the shield would be letterboxed \
             inside it and drift away from the word beside it"
        );
        assert!(l.mark.right() < l.wordmark.x, "the wordmark is drawn over the shield");
        assert_eq!(l.wordmark.h, l.mark.h, "the lockup's two halves are different heights");
        assert!(l.wordmark.right() <= l.pill.x, "the wordmark runs under the status pill");
        assert!(l.pill.right() < l.close_glyph.x, "the status pill touches the ✕");
        assert!(
            l.close_glyph.right() <= l.window.right() - MARGIN_X,
            "the close glyph has crossed the card's right margin"
        );
        assert_eq!(l.pill.h, crate::panel_draw::PILL_H);

        assert!(l.mark.bottom() <= l.primary.y, "the title runs into the lockup above it");
        for line in [l.primary, l.secondary, l.unlock] {
            assert_eq!(line.x, MARGIN_X);
            assert_eq!(line.right(), WIDTH - MARGIN_X, "a block does not run to the card's margin");
        }
        assert!(l.primary.bottom() <= l.secondary.y, "the two text lines overlap");
        assert!(l.secondary.bottom() <= l.unlock.y, "the body runs into the button");
        assert_eq!(
            l.unlock.bottom() + MARGIN_TOP,
            l.window.bottom(),
            "the card is not sized to its own button: it asks the OS for a {} px window whose \
             last control ends at {} px",
            l.window.h,
            l.unlock.bottom()
        );
    }

    /// **The card's dimensions are the design's.**
    #[test]
    fn the_cards_dimensions_are_the_designs() {
        assert_eq!((BUTTON_H, WIDTH), (54, 410), "design D 1.6's Unlock button and panel");
        assert_eq!(BUTTON_H, crate::panel_draw::TALL_BUTTON_H);
        assert_eq!(WIDTH, crate::panel_draw::CARD_W);
    }

    /// **The card says every one of its own words**, each a constant rather
    /// than a literal at the paint site.
    #[test]
    fn the_cards_words_are_the_ones_it_promises() {
        assert_eq!(locked_pill(), "Locked");
        assert_eq!(unlock_label(), "Unlock");
        assert_eq!(
            locked_text(APP),
            (
                "Deskwarden is locked".to_string(),
                "Unlock to check the vault for Ledgerline Desktop.".to_string()
            ),
            "the second line's verb is `check`: `see the login` would promise there is one"
        );
        assert_ne!(
            locked_pill(),
            locked_text(APP).0,
            "the pill names the state and the title explains it; a pill that repeated the title \
             would leave the card saying one thing twice"
        );
        assert_ne!(
            unlock_label(),
            crate::unlock_prompt::UNLOCK_PROMPT_TITLE,
            "a button says what pressing it does; the window it opens says what it is"
        );
    }

    /// **The card does not count logins it cannot see.** Design 1b titles this
    /// panel "3 logins match"; while the vault is locked that number is not
    /// knowable, so nothing here may be a number.
    #[test]
    fn the_card_never_prints_a_match_count() {
        for (_, app) in FIXTURES {
            let (title, body) = locked_text(app);
            assert!(!title.contains("match") && !body.contains("match"));
            assert!(!title.chars().any(|c| c.is_ascii_digit()));
        }
    }

    /// **The title is this window's own.**
    #[test]
    fn the_window_opens_under_a_title_nothing_else_uses() {
        assert!(!LOCKED_CARD_TITLE.is_empty());
        assert_ne!(LOCKED_CARD_TITLE, "Deskwarden");
        assert_ne!(LOCKED_CARD_TITLE, crate::prompt_card::PROMPT_CARD_TITLE);
        assert_ne!(LOCKED_CARD_TITLE, crate::picker_prompt::PICKER_PROMPT_TITLE);
        assert_ne!(LOCKED_CARD_TITLE, crate::unlock_prompt::UNLOCK_PROMPT_TITLE);
        assert_ne!(LOCKED_CARD_TITLE, crate::generate_prompt::GENERATE_PROMPT_TITLE);
        assert_ne!(LOCKED_CARD_TITLE, crate::vault_window::WINDOW_TITLE);
    }

    // ---- source pins -------------------------------------------------------

    /// The production half of this file: everything before the first column-0
    /// `#[cfg(test)]`, with line endings normalised first because this
    /// repository checks out CRLF.
    fn production() -> (String, usize) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("locked_card.rs");
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

    #[test]
    fn the_locked_window_never_posts_a_thread_quit() {
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
            "control: the production cut does not contain the window procedure's WM_DESTROY arm"
        );
        assert!(
            code.contains("GONE.store(true, Ordering::SeqCst);"),
            "control: the comment stripper has eaten code -- the WM_DESTROY arm's one surviving \
             statement is not in the text this rule scans"
        );

        assert!(
            !code.contains(concat!("PostQuit", "Message")),
            "locked_card.rs's production half posts a thread quit. This window is opened on the \
             daemon thread, and that thread goes on to run the unlock prompt and then egui \
             windows. `close()` calls `DestroyWindow`, which dispatches WM_DESTROY synchronously \
             on that thread, and nothing drains the queue afterwards: `next()` has already \
             returned. The next `eframe::run_native` takes the stale WM_QUIT out of \
             `GetMessageW`, leaves its loop before it draws, and returns its DEFAULT answer. \
             `GONE` is what `next()` reads; quitting the thread is not this window's job."
        );
    }

    /// **The capture exclusion goes on the top-level window, and once.**
    #[test]
    fn the_capture_exclusion_goes_on_the_top_level_window() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert_eq!(
            code.matches("SetWindowDisplayAffinity(").count(),
            1,
            "this card names the app this user is signing into and excludes itself from screen \
             capture other than exactly once"
        );
        assert!(
            code.contains("SetWindowDisplayAffinity(hwnd(window.0), WDA_EXCLUDEFROMCAPTURE)"),
            "the exclusion is not applied to the top-level window this module was handed. \
             Windows refuses it on a child control with E_INVALIDARG"
        );
        assert_eq!(
            code.matches("SetForegroundWindow(").count(),
            1,
            "this card advertises `Esc Dismiss` and has a button Enter presses, so it has to be \
             able to receive them -- and it asks for the foreground other than exactly once"
        );
        assert_eq!(
            code.matches("run_ui_native(").count(),
            0,
            "this card has become an `eframe` window, which is the ~50 MB of unreleasable OpenGL \
             driver arenas it exists to not spend"
        );
    }

    /// **The card's user-controlled line is drawn with an ellipsis.**
    ///
    /// A source pin, because `DT_END_ELLIPSIS` is a painting flag and nothing
    /// this crate can drive in a test reads pixels back off the daemon's card.
    /// Without it `DrawTextW` clips: a long app name is cut through the middle
    /// of a letter with nothing to say it was truncated -- on the one line of
    /// this card that carries a string the user chose.
    #[test]
    fn the_body_lines_are_drawn_with_an_end_ellipsis() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        // The needle is `draw_text(` and no longer `DrawTextW(`: this card's
        // one painter now goes through `win32_draw::draw_text`, which is the
        // crate's only caller of the raw API. See
        // `win32_draw::tests::the_crate_calls_draw_text_w_in_exactly_one_place`
        // for why that indirection exists -- an empty run through the raw call
        // is an access violation that kills the daemon with no log line, and
        // this card carried one of the two hand-written guards it replaced.
        let drawn = code.matches(concat!("draw_", "text(")).count();
        assert_eq!(
            drawn, 1,
            "control: locked_card.rs draws text in one place -- its `text` helper, which every \
             run goes through. It now draws it in {drawn}, so the rule below no longer covers \
             every run"
        );
        assert!(
            code.contains(concat!("DT_NOPREFIX | DT_END_", "ELLIPSIS")),
            "the card's one text painter no longer truncates with an ellipsis. Its second line \
             carries `app::window_label`'s answer, which is user-controlled, and this window \
             cannot grow and cannot scroll"
        );
    }

    /// **Nothing on this card is logged.**
    #[test]
    fn the_card_writes_no_app_name_to_the_log() {
        let (production, discarded) = production();
        assert!(discarded > 0, "control: nothing was cut out of the file");
        let code = code(&production);
        assert!(
            code.contains("log::warn!"),
            "control: this module logs nothing at all, so the rule below is vacuous"
        );
        for forbidden in ["log::info!", "log::debug!", "log::trace!"] {
            assert!(!code.contains(forbidden), "`{forbidden}` appears in this module");
        }
        let mut scanned = 0;
        for line in code.lines() {
            let Some(start) = line.find("log::") else { continue };
            scanned += 1;
            assert!(
                !line[start..].contains('{'),
                "a log line in locked_card.rs interpolates a value: {line:?}"
            );
        }
        assert!(scanned >= 2, "control: only {scanned} log lines were scanned");
    }
}

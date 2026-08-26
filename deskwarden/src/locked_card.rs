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

/// The Win32 half, as `fn` pointers so [`run_with`] can be driven without a
/// desktop. Nothing here decides anything.
pub struct LockedCalls {
    /// Lays out and shows the card for this app name, at this anchor.
    pub open: fn(&str, Option<(f32, f32)>) -> Option<LockedWindow>,
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

/// What the card's header says.
///
/// **It must not be the body's first line**: the two are different claims --
/// the header names the state, the body explains it -- and a header that
/// repeated the body would leave the card saying one thing twice and the app
/// unnamed.
pub const LOCKED_LABEL: &str = "Vault locked";

/// What 3b's one button says, and **the only card that may carry it**.
///
/// **It is not "Unlock Deskwarden"**, which is
/// [`crate::unlock_prompt::UNLOCK_PROMPT_TITLE`]: a button says what pressing
/// it does, and the window it opens says what it is.
pub const UNLOCK_LABEL: &str = "Unlock";

/// The `Esc` chip in the footer, and the word beside it.
pub const ESC_SHORTCUT: &str = "ESC";
/// The footer hint's word.
pub const DISMISS_LABEL: &str = "Dismiss";

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
        "Deskwarden is locked".to_string(),
        format!("Unlock to check the vault for {app_name}."),
    )
}

/// What kind of run a [`Painted`] is, which is what the painter reads to pick
/// its font and its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The header.
    Title,
    /// The body's first line.
    Primary,
    /// The body's second line.
    Secondary,
    /// The *Unlock* button's label. Painted by the control, not by the card.
    Button,
    /// The `ESC` keyboard chip.
    Chip,
    /// The word beside the chip.
    Hint,
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
        Painted { text: LOCKED_LABEL.to_string(), at: l.title, role: Role::Title },
        Painted { text: primary, at: l.primary, role: Role::Primary },
        Painted { text: secondary, at: l.secondary, role: Role::Secondary },
        Painted { text: UNLOCK_LABEL.to_string(), at: l.unlock, role: Role::Button },
        Painted { text: ESC_SHORTCUT.to_string(), at: l.esc_chip, role: Role::Chip },
        Painted { text: DISMISS_LABEL.to_string(), at: l.dismiss, role: Role::Hint },
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

/// The card's width, and so the window's. `crate::prompt_card::WIDTH`, because
/// this card and that one are the same card in two states of the same vault
/// and appear in the same place on screen.
pub const WIDTH: i32 = crate::prompt_card::WIDTH;

const MARGIN_X: i32 = 16;
const MARGIN_TOP: i32 = 16;

/// The body well's height: two lines and the padding around them, fixed
/// whatever the app is called.
const BODY_H: i32 = 52;

/// Button height. `crate::theme::BUTTON_HEIGHT`, pinned by
/// [`the_cards_dimensions_are_the_themes`].
const BUTTON_H: i32 = 32;

/// The *Unlock* button's width: its label, at the app's own type, with the
/// padding a pill needs.
const UNLOCK_W: i32 = 96;

const ESC_CHIP_W: i32 = 34;

/// Every rectangle the card paints, computed once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub window: Box2,
    /// The brand lockup's shield, and the wordmark beside it. **The card had
    /// no brand at all after the port**: the egui notice it replaced carried
    /// `theme::card_header`'s shield and letterspaced DESKWARDEN, and a
    /// frameless always-on-top window that names the user's apps and asks for
    /// their master password has to say whose window it is. The compact
    /// lockup, not the login window's -- see
    /// [`crate::win32_draw::draw_card_lockup`].
    pub mark: Box2,
    pub wordmark: Box2,
    pub title: Box2,
    pub close_glyph: Box2,
    pub header_rule: Box2,
    pub footer_rule: Box2,
    pub footer: Box2,
    /// The tinted well the two body lines sit in.
    pub body: Box2,
    pub primary: Box2,
    pub secondary: Box2,
    pub unlock: Box2,
    pub esc_chip: Box2,
    pub dismiss: Box2,
}

/// **The card's geometry. There is exactly one shape.**
///
/// Nothing about this card varies at runtime: it has no rows, no modes, no
/// second step and no count, and its one user-controlled string is drawn into
/// a fixed box with an ellipsis. The window is sized to this content and to
/// nothing else.
pub fn layout() -> Layout {
    let content_w = WIDTH - 2 * MARGIN_X;

    let lockup = crate::win32_draw::card_lockup();
    let mark = Box2 { x: MARGIN_X, y: MARGIN_TOP, w: lockup.mark_w, h: lockup.mark_h };
    let wordmark =
        Box2 { x: mark.right() + lockup.gap, y: MARGIN_TOP, w: lockup.word_w, h: lockup.mark_h };
    // The ✕ moves up onto the lockup's line, which is where every card header
    // in the design carries it -- and where it has to be now, because the
    // title is no longer the top line.
    let close_glyph =
        Box2 { x: WIDTH - MARGIN_X - 20, y: MARGIN_TOP - 2, w: 20, h: 20 };
    let title =
        Box2 { x: MARGIN_X, y: mark.bottom() + lockup.gap_below, w: content_w - 24, h: 20 };
    let header_rule = Box2 { x: 0, y: title.bottom() + 10, w: WIDTH, h: 1 };

    let body = Box2 { x: MARGIN_X, y: header_rule.bottom() + 8, w: content_w, h: BODY_H };
    // Two lines inside the well, inset from its edges. Both are drawn with
    // `DT_END_ELLIPSIS` into these boxes: the second carries `app_name`, and
    // this window cannot grow and cannot scroll.
    let primary = Box2 { x: body.x + 10, y: body.y + 8, w: body.w - 20, h: 18 };
    let secondary = Box2 { x: body.x + 10, y: primary.bottom() + 1, w: body.w - 20, h: 16 };

    let footer_rule = Box2 { x: 0, y: body.bottom() + 8, w: WIDTH, h: 1 };
    let unlock =
        Box2 { x: MARGIN_X, y: footer_rule.bottom() + 8, w: UNLOCK_W, h: BUTTON_H };
    let esc_chip = Box2 { x: unlock.right() + 10, y: unlock.y, w: ESC_CHIP_W, h: BUTTON_H };
    let dismiss = Box2 {
        x: esc_chip.right() + 5,
        y: unlock.y,
        w: MARGIN_X + content_w - (esc_chip.right() + 5),
        h: BUTTON_H,
    };

    let height = unlock.bottom() + 8;
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
        footer_rule,
        footer,
        body,
        primary,
        secondary,
        unlock,
        esc_chip,
        dismiss,
    }
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
    use super::{
        Painted, Role, APP_NAME, ESC_SHORTCUT, GONE, LOCKED_CARD_TITLE, PENDING,
    };
    use crate::prompt_card::Box2;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AddFontMemResourceEx, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        CreateFontIndirectW, CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW,
        EndPaint, FillRect, GetDC, GetDeviceCaps, InvalidateRect, ReleaseDC, RoundRect,
        SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, DT_END_ELLIPSIS, DT_LEFT,
        DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, HBRUSH, HDC, HFONT, LOGFONTW,
        LOGPIXELSX, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
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

    use crate::win32_draw::{draw_button, draw_card_lockup, draw_hint_chip, rgb, ButtonSkin};

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
        /// The lockup's wordmark: `theme::CARD_HEADER_WORD_PX` in the bold
        /// cut, which is what `theme::card_header` letterspaces "DESKWARDEN"
        /// in.
        brand: HFONT,
        title: HFONT,
        primary: HFONT,
        secondary: HFONT,
        button: HFONT,
        hint: HFONT,
    }

    impl Fonts {
        fn build() -> Self {
            use crate::theme::{BOLD, REGULAR, SEMIBOLD};
            Fonts {
                brand: font(BOLD, crate::win32_draw::card_lockup().word_px),
                title: font(BOLD, 14),
                primary: font(SEMIBOLD, 13),
                secondary: font(REGULAR, 11),
                button: font(SEMIBOLD, 12),
                hint: mono(crate::theme::CHIP_TEXT_PX as i32),
            }
        }

        fn destroy(&self) {
            unsafe {
                for f in [self.brand, self.title, self.primary, self.secondary, self.button, self.hint]
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
                Role::Title => (self.title, crate::theme::INK),
                Role::Primary => (self.primary, crate::theme::INK),
                Role::Secondary => (self.secondary, crate::theme::TEXT_FAINT),
                Role::Button => (self.button, crate::theme::INK),
                Role::Chip => (self.hint, crate::theme::TEXT_FAINT),
                Role::Hint => (self.secondary, crate::theme::TEXT_FAINT),
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
                let id = (wparam.0 & 0xffff) as usize;
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
            let dpi = DPI_PERCENT.load(Ordering::SeqCst);

            fill_rect(mem, client, crate::theme::CARD);
            fill_box(mem, l.footer, crate::theme::CARD_TINT);
            fill_box(mem, l.header_rule, crate::theme::HAIRLINE);
            fill_box(mem, l.footer_rule, crate::theme::HAIRLINE);
            SetBkMode(mem, TRANSPARENT);

            // The body well. `theme::CANVAS`, the same inset well the picker's
            // rows sit in.
            rounded(mem, l.body, 8, crate::theme::CANVAS);

            if let Some(fonts) = fonts {
                paint_lockup(mem, &l, fonts.brand);
                for run in runs() {
                    match run.role {
                        // The control paints its own.
                        Role::Button => {}
                        Role::Chip => {
                            let rc = RECT {
                                left: scale(run.at.x),
                                top: scale(run.at.y),
                                right: scale(run.at.right()),
                                bottom: scale(run.at.bottom()),
                            };
                            draw_hint_chip(mem, rc, ESC_SHORTCUT, fonts.hint, dpi);
                        }
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

    /// The brand lockup, through [`crate::win32_draw::draw_card_lockup`] --
    /// the crate's one mark painter, which `unlock_prompt` also draws through.
    /// What is this card's own is only the logical-to-device conversion, which
    /// no other card's `Box2` type can share.
    fn paint_lockup(hdc: HDC, l: &super::Layout, font: HFONT) {
        let dev = |b: Box2| RECT {
            left: scale(b.x),
            top: scale(b.y),
            right: scale(b.right()),
            bottom: scale(b.bottom()),
        };
        let tracking = scale(crate::win32_draw::card_lockup().tracking);
        draw_card_lockup(hdc, dev(l.mark), dev(l.wordmark), font, tracking);
    }

    /// The *Unlock* button.
    fn paint_control(control: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(control, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(control, &mut rc);

            let hovered = HOVERED.load(Ordering::SeqCst) == ID_UNLOCK as isize;
            let focused = GetFocus() == control;

            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let old = SelectObject(mem, bmp);
            let whole = RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom };

            let guard = FONTS.lock();
            let fonts = guard.as_ref().ok().and_then(|slot| slot.as_ref());

            // The button sits on the footer's tint -- otherwise its rounded
            // corners show the wrong colour through them.
            fill_rect(mem, whole, crate::theme::CARD_TINT);
            SetBkMode(mem, TRANSPARENT);

            if let Some(fonts) = fonts {
                let label = runs()
                    .into_iter()
                    .find(|r| r.role == Role::Button)
                    .map(|r| r.text)
                    .unwrap_or_default();
                let skin = if hovered {
                    ButtonSkin::primary().hovered()
                } else {
                    ButtonSkin::primary()
                };
                let l = super::layout();
                if focused {
                    // **The ring is given LOGICAL size, from `layout`.**
                    // `rounded` scales what it is handed, and `rc` came back
                    // from `GetClientRect` in device pixels already.
                    rounded(
                        mem,
                        Box2 { x: 0, y: 0, w: l.unlock.w, h: l.unlock.h },
                        9,
                        crate::theme::FOCUS_RING,
                    );
                    let inner = RECT {
                        left: whole.left + 2,
                        top: whole.top + 2,
                        right: whole.right - 2,
                        bottom: whole.bottom - 2,
                    };
                    draw_button(mem, inner, &label, fonts.button, skin, scale(8));
                } else {
                    draw_button(mem, whole, &label, fonts.button, skin, scale(8));
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

    /// The header's close glyph, drawn as two strokes because no bundled face
    /// has it at this weight.
    ///
    /// **It matters more on this card than anywhere.** This window is raised
    /// in response to ANOTHER app being foregrounded, which is exactly the
    /// situation Windows' foreground lock may refuse focus for -- so Esc is
    /// not guaranteed to arrive, and the ✕ is the only mouse-operable way out
    /// of a window with no title bar.
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

    /// A rounded rectangle in logical coordinates.
    fn rounded(hdc: HDC, at: Box2, radius: i32, fill_colour: eframe::egui::Color32) {
        unsafe {
            let brush = CreateSolidBrush(rgb(fill_colour));
            let pen = CreatePen(PS_SOLID, 1, rgb(fill_colour));
            let old_brush = SelectObject(hdc, brush);
            let old_pen = SelectObject(hdc, pen);
            let r = scale(radius) * 2;
            let _ = RoundRect(
                hdc,
                scale(at.x),
                scale(at.y),
                scale(at.right()),
                scale(at.bottom()),
                r,
                r,
            );
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush);
            let _ = DeleteObject(pen);
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
            let mut chars: Vec<u16> = run.encode_utf16().collect();
            let mut rc = RECT {
                left: scale(at.x),
                top: scale(at.y),
                right: scale(at.right()),
                bottom: scale(at.bottom()),
            };
            // `DT_NOPREFIX`: these are the app's own words and a user's app
            // name, in which an `&` is an ampersand and never a mnemonic.
            DrawTextW(
                hdc,
                &mut chars,
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
        assert_eq!(checked, FIXTURES.len() * 6, "the loop did not cover every run");
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
        assert_eq!(buttons, vec![UNLOCK_LABEL.to_string()]);
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
            assert_eq!(runs.len(), 6, "the {name:?} card paints a different number of runs");
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
        assert_eq!(checked, FIXTURES.len() * 6);
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


        // **The brand lockup**, which the port had dropped entirely and which
        // this card now carries again. Pinned to the new truth rather than
        // loosened: the card grew by the lockup's height plus its gap, and the
        // window's own height assertions below are what hold that honest.
        let lockup = crate::win32_draw::card_lockup();
        assert_eq!(
            (l.mark.x, l.mark.y),
            (MARGIN_X, MARGIN_TOP),
            "the lockup does not start at the card's own top-left inset"
        );
        assert_eq!(l.mark.h, lockup.mark_h);
        assert_eq!(
            l.mark.w,
            crate::win32_draw::mark_width(l.mark.h),
            "the mark's box is not the design artboard's ratio, so the shield would be              letterboxed inside it and drift away from the word beside it"
        );
        assert!(l.mark.right() < l.wordmark.x, "the wordmark is drawn over the shield");
        assert_eq!(l.wordmark.h, l.mark.h, "the lockup's two halves are different heights");
        assert!(
            l.wordmark.right() <= l.close_glyph.x,
            "the wordmark runs under the ✕"
        );
        assert!(
            l.wordmark.bottom() <= l.title.y,
            "the card's title runs into the brand lockup above it"
        );

        assert!(l.title.right() <= l.close_glyph.x, "the header text runs under the ✕");
        assert!(
            l.close_glyph.right() <= l.window.right() - MARGIN_X,
            "the close glyph has crossed the card's right margin"
        );
        assert!(l.title.bottom() <= l.header_rule.y);
        assert!(l.header_rule.bottom() <= l.body.y);
        assert_eq!(l.body.x, MARGIN_X);
        assert_eq!(l.body.right(), WIDTH - MARGIN_X);
        for line in [l.primary, l.secondary] {
            assert!(line.x >= l.body.x, "a body line starts outside the well");
            assert!(line.right() <= l.body.right(), "a body line runs past the well");
            assert!(line.y >= l.body.y && line.bottom() <= l.body.bottom());
        }
        assert!(l.primary.bottom() <= l.secondary.y, "the two body lines overlap");
        assert!(l.body.bottom() <= l.footer_rule.y);
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
        assert!(l.footer_rule.bottom() <= l.unlock.y);
        assert!(l.unlock.right() < l.esc_chip.x, "the Esc chip sits on the Unlock button");
        assert!(l.esc_chip.right() < l.dismiss.x);
        assert!(
            l.dismiss.right() <= l.window.right() - MARGIN_X,
            "the footer hint has crossed the card's right margin"
        );
        assert!(l.dismiss.w > 0, "the footer hint has no room for its word");
        // **Against the MARGIN, not against the window's edge.** A pin that
        // only forbade a control leaving the window is slacker than the layout
        // it guards.
        assert_eq!(
            l.unlock.bottom() + 8,
            l.window.bottom(),
            "the card is not sized to its own footer: it asks the OS for a {} px window whose \
             last control ends at {} px",
            l.window.h,
            l.unlock.bottom()
        );
    }

    /// **The card's dimensions are the theme's and the crate's.**
    #[test]
    fn the_cards_dimensions_are_the_themes() {
        assert_eq!(
            BUTTON_H,
            crate::theme::BUTTON_HEIGHT as i32,
            "the Unlock button is not the app's button height"
        );
        assert_eq!(
            WIDTH,
            crate::prompt_card::WIDTH,
            "3b and 2a are two states of the same vault shown in the same place; two widths read \
             as two different programs"
        );
    }

    /// **The card says every one of its own words**, each a constant rather
    /// than a literal at the paint site.
    #[test]
    fn the_cards_words_are_the_ones_it_promises() {
        assert_eq!(LOCKED_LABEL, "Vault locked");
        assert_eq!(UNLOCK_LABEL, "Unlock");
        assert_eq!(ESC_SHORTCUT, "ESC");
        assert_eq!(DISMISS_LABEL, "Dismiss");
        assert_eq!(
            locked_text(APP),
            (
                "Deskwarden is locked".to_string(),
                "Unlock to check the vault for Ledgerline Desktop.".to_string()
            ),
            "the second line's verb is `check`: `see the login` would promise there is one"
        );
        assert_ne!(
            LOCKED_LABEL,
            locked_text(APP).0,
            "the header and the body's first line are different claims -- the header names the \
             state, the body explains it -- and a header that repeated the body would leave the \
             card saying one thing twice"
        );
        assert_ne!(
            UNLOCK_LABEL,
            crate::unlock_prompt::UNLOCK_PROMPT_TITLE,
            "a button says what pressing it does; the window it opens says what it is"
        );
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
        let drawn = code.matches(concat!("Draw", "TextW(")).count();
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

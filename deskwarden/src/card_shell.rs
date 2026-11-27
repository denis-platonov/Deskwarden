//! **The "shell": smooth 18 px corners and the design's soft shadow for the
//! daemon's bare-Win32 cards.**
//!
//! # The two complaints this answers
//!
//! Every fill-time card (`picker_prompt`, `prompt_card`, `locked_card`,
//! `notice_card`, `save_login_card`, `unlock_prompt`, `generate_prompt`,
//! `tray_popup`) took its silhouette from
//! `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND)`.
//! That is Windows' own fixed radius -- about 8 px -- and the design's panel is
//! [`crate::panel_draw::RADIUS`] = 18, with a soft `0 24px 60px` shadow. "The
//! screens should be more rounded, as per design."
//!
//! # Why not make the card itself a per-pixel-alpha window
//!
//! `UpdateLayeredWindow` is the one API that gives a window antialiased edges
//! and a real soft shadow, and a window painted that way does not render its
//! **child windows**: the cards' subclassed buttons and edit fields would
//! vanish. Direct2D / GDI+ were rejected for memory (53 MB against 1.8 MB for a
//! plain GDI window; see `win32_draw`). So the card stays an ordinary window,
//! and a second window is put under it.
//!
//! # What is built
//!
//! * **The shell**: a top-level `WS_EX_LAYERED | WS_EX_TRANSPARENT |
//!   WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` popup, a little bigger than the card
//!   on every side. It is painted ONCE per card size with `UpdateLayeredWindow`
//!   from a premultiplied 32 bpp picture: the card's colour in an antialiased
//!   18 px rounded rectangle, and a blurred shadow in the margin around it.
//!   It is click-through (`WM_NCHITTEST` answers `HTTRANSPARENT`), never takes
//!   focus (`WM_MOUSEACTIVATE` answers `MA_NOACTIVATE`) and has no title, so
//!   nothing that looks for a window by title can find it.
//! * **The card**: `DWMWCP_DONOTROUND` (so Windows stops rounding it to 8 px),
//!   DWM non-client rendering off (so it keeps no DWM shadow of its own), and
//!   `SetWindowRgn` with a round-rect region of the same radius **inset past
//!   the hairline** ([`region_inset`]). The region's stair-steps lie over the
//!   shell's fully opaque card-coloured area, so they are invisible, and the
//!   smooth edge the eye sees -- the hairline and the curve -- is the shell's.
//!   (Windows drew a 1 px border round these cards too; the shell draws it now,
//!   in the design's `#d7d3d3`, along the new curve.)
//!
//! # Gluing the shell to the card
//!
//! [`attach`] subclasses the card's window procedure, so no card's own
//! `wnd_proc` changes. The subclass follows the card's move, size, z-order,
//! show and hide through `WM_WINDOWPOSCHANGED` and `WM_SHOWWINDOW`, keeps the
//! shell directly **below** the card in z-order (`SetWindowPos(shell, card,
//! ..)`), re-paints the shell and re-cuts the region whenever the size changes
//! (the picker changes size between its list and its search), mirrors the
//! card's capture exclusion onto the shell, and destroys the shell on
//! `WM_DESTROY`.
//!
//! # About `region_overlay`'s history with layered windows
//!
//! `region_overlay` records four compositing mechanisms that Windows accepted
//! and that showed black on the owner's monitor, among them `WS_EX_LAYERED` +
//! `SetLayeredWindowAttributes(LWA_ALPHA)`. That failure was an OpenGL swap
//! chain presented past the layered surface: the alpha applied to the first
//! composite and to nothing after it. The shell has no swap chain and no
//! `SetLayeredWindowAttributes`: it is a GDI bitmap handed to
//! `UpdateLayeredWindow` once, which is the call the layered-window documentation
//! prescribes for per-pixel alpha, and the surface never changes after it.
//!
//! # Memory
//!
//! The picture is rendered into a DIB section, handed to
//! `UpdateLayeredWindow`, and freed before the call returns to the message
//! loop. The shadow is blurred on a grid a quarter the size (it is smooth, so
//! nothing is lost) and sampled up bilinearly, so the transient cost of a
//! 200 % display is a few MB and the steady cost is the system's copy of the
//! surface. Every GDI object created here is deleted before returning.
//!
//! # The main windows: [`attach_shadow`]
//!
//! The frameless main windows (sign-in/unlock, spinner, Preferences, vault)
//! paint their own content on a GL surface and cut their own antialiased 18 px
//! corners out of its alpha, so the shell under them is [`Look::ShadowOnly`]:
//! the same blurred shadow, with the window's footprint carved out of it by the
//! same curve and left fully transparent. Everything above applies except the
//! card-only parts (no fill, no hairline, no `DWMWCP_DONOTROUND` here -- the
//! window already asked -- and no stair-stepped region over the silhouette).
//! The shell is hidden while the window is maximized, fullscreen, minimized or
//! cloaked ([`resync`] is how `window_host`'s cloak reaches it, since DWM
//! announces a cloak to nobody), and DWM's own square shadow is removed with a
//! window region rather than with non-client rendering, which brings back a dark
//! top line; see [`attach_shadow`].
//!
//! # Classification
//!
//! This module **does create a window**, and says so. It is not a surface: it
//! holds no content, takes no input and no focus, and exists only while a card
//! does -- the raise guard in `crate::foreground` is about windows a user types
//! into, and a click-through shadow cannot "open behind something" in a way
//! that matters. See the entry in that file's `OPENS_NO_WINDOW`.

use crate::kit::tokens::Palette;
use crate::win32_draw::{coverage, rounded_rect_sdf};
use eframe::egui::Color32;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_CLOAKED,
    DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
    DWM_CLOAKED_APP,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateRoundRectRgn, DeleteDC, DeleteObject, GetDC,
    GetDeviceCaps, ReleaseDC, SelectObject, SetWindowRgn, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, GetMonitorInfoW, LOGPIXELSX,
    HRGN, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowDisplayAffinity,
    GetWindowLongPtrW, GetWindowRect, IsIconic, IsWindowVisible, IsZoomed, RegisterClassW, SetWindowDisplayAffinity,
    SetWindowLongPtrW, SetWindowPos, UpdateLayeredWindow, GWLP_WNDPROC, GWL_EXSTYLE,
    HTTRANSPARENT, MA_NOACTIVATE, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, ULW_ALPHA, WINDOW_DISPLAY_AFFINITY, WINDOW_EX_STYLE, WM_DESTROY,
    WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_MOUSEACTIVATE, WM_SIZING, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT,
    WM_SHOWWINDOW, WM_WINDOWPOSCHANGED,
    WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

// ---------------------------------------------------------------------------
// The numbers. Logical pixels at 100 %.
// ---------------------------------------------------------------------------

/// The card's corner radius: the design's panel radius, not Windows' 8.
pub const RADIUS: i32 = crate::panel_draw::RADIUS;

/// The shadow's Gaussian sigma. The design's `0 24px 60px` is a 60 px blur,
/// which CSS defines as sigma 30; 26 keeps the picture (and the memory)
/// smaller and reads the same at the size of these cards.
pub const SIGMA: f32 = 26.0;

/// How far the shadow stands below the card (the design's `24px`).
pub const OFFSET_Y: i32 = 22;

/// How many full-resolution pixels one pixel of the shadow's blur grid covers.
const BLUR_GRID: usize = 4;

/// The hairline's width in device pixels: one logical pixel, never less than
/// one device pixel.
pub fn border_px(scale_pct: i32) -> i32 {
    ((scale_pct.max(1) as f32 / 100.0).round() as i32).max(1)
}

/// How far the card's window region is pulled in from its edge, in device
/// pixels: the hairline plus one. The region is stair-stepped, and the shell
/// shows through everywhere outside it; this keeps the stair-steps over the
/// shell's pixels that are fully opaque card colour, past the hairline.
pub fn region_inset(scale_pct: i32) -> i32 {
    border_px(scale_pct) + 1
}

/// The shell's margins around the card, in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Margins {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

fn dev(logical: f32, scale_pct: i32) -> f32 {
    logical * scale_pct.max(1) as f32 / 100.0
}

/// The shell's margins at `scale_pct` (100 = 96 dpi). Wide enough that the
/// shadow has faded below one part in forty at the shell's edge. Pure.
pub fn margins(scale_pct: i32) -> Margins {
    let reach = (dev(SIGMA, scale_pct) * 2.2).ceil() as i32;
    let offset = dev(OFFSET_Y as f32, scale_pct).round() as i32;
    Margins {
        left: reach,
        top: (reach - offset).max(0),
        right: reach,
        bottom: reach + offset,
    }
}

/// The shell's size for a `card_w` x `card_h` card (device pixels). Pure.
pub fn shell_size(card_w: i32, card_h: i32, scale_pct: i32) -> (i32, i32) {
    let m = margins(scale_pct);
    (card_w + m.left + m.right, card_h + m.top + m.bottom)
}

/// The region radius's ELLIPSE DIAMETER for `CreateRoundRectRgn`: the card's
/// radius less the inset, doubled. Pure.
pub fn region_diameter(scale_pct: i32) -> i32 {
    (dev(RADIUS as f32, scale_pct).round() as i32 - region_inset(scale_pct)).max(0) * 2
}

// ---------------------------------------------------------------------------
// The picture. Pure: a premultiplied ARGB buffer and nothing else.
// ---------------------------------------------------------------------------

fn box_blur_pass(src: &[f32], dst: &mut [f32], len: usize, count: usize, stride: usize, step: usize, r: usize) {
    // One axis of one pass. `count` lines of `len` samples; a sample's
    // neighbour on the line is `step` away and the next line is `stride`
    // away. Outside the grid is zero, which is the shadow fading out.
    let norm = 1.0 / (2 * r + 1) as f32;
    for line in 0..count {
        let base = line * stride;
        let at = |i: isize| -> f32 {
            if i < 0 || i >= len as isize {
                0.0
            } else {
                src[base + i as usize * step]
            }
        };
        let mut sum = 0.0f32;
        for i in -(r as isize)..=(r as isize) {
            sum += at(i);
        }
        for i in 0..len {
            dst[base + i * step] = sum * norm;
            sum += at(i as isize + r as isize + 1) - at(i as isize - r as isize);
        }
    }
}

/// Gaussian-ish blur of a `w` x `h` grid: three box blurs per axis, which is
/// within a percent of a Gaussian and costs `O(pixels)`. Pure.
pub fn blur(grid: &mut [f32], w: usize, h: usize, sigma: f32) {
    if w == 0 || h == 0 || sigma < 0.3 {
        return;
    }
    let ideal = ((12.0 * sigma * sigma / 3.0) + 1.0).sqrt();
    let r = (((ideal - 1.0) / 2.0).round() as usize).max(1);
    let mut scratch = vec![0.0f32; grid.len()];
    for _ in 0..3 {
        box_blur_pass(grid, &mut scratch, w, h, w, 1, r);
        box_blur_pass(&scratch, grid, h, w, 1, w, r);
    }
}

/// The four colours a card's shell is painted with, all of them read off one
/// [`Palette`]: the card's fill, its hairline (`kbd_border`), and the shadow's
/// colour and strength (`shadow`, whose alpha is the peak). Light and dark
/// are the palette's, not this module's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardColours {
    pub fill: [u8; 3],
    pub border: [u8; 3],
    pub shadow: [u8; 3],
    /// The shadow's alpha at the card's own edge, 0..=255.
    pub peak: u8,
}

impl CardColours {
    /// `palette`'s card.
    pub fn of(palette: &Palette) -> Self {
        let rgb = |c: Color32| [c.r(), c.g(), c.b()];
        Self {
            fill: rgb(palette.card),
            border: rgb(palette.kbd_border),
            shadow: rgb(palette.shadow),
            peak: palette.shadow.a(),
        }
    }
}

/// What a shell paints in the card's own footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// The card's colour and hairline in an antialiased rounded rectangle, over
    /// the shadow: the bare-Win32 cards, which paint nothing of their own
    /// along the curve.
    Card(CardColours),
    /// **Only the shadow**: the footprint is fully transparent (alpha 0) and
    /// the shadow is cut out of it by the same antialiased curve. The main
    /// windows paint their own content and edge on the GL surface and cut their
    /// own corners out of its alpha; the shell under them contributes nothing
    /// but the soft shadow round that silhouette.
    ShadowOnly,
}

/// Fills `out` (`shell_w` x `shell_h`, premultiplied `0xAARRGGBB`) with the
/// shell for a `card_w` x `card_h` card: the card's `fill` in an antialiased
/// rounded rectangle, over a blurred shadow. `out.len()` must be
/// `shell_w * shell_h` (the sizes [`shell_size`] answers). Pure.
pub fn render_into(out: &mut [u32], card_w: i32, card_h: i32, scale_pct: i32, fill: [u8; 3]) {
    let colours = CardColours { fill, ..CardColours::of(&Palette::LIGHT) };
    render_look_into(out, card_w, card_h, scale_pct, Look::Card(colours));
}

/// [`render_into`] for either [`Look`]. Pure.
pub fn render_look_into(out: &mut [u32], card_w: i32, card_h: i32, scale_pct: i32, look: Look) {
    let m = margins(scale_pct);
    let (sw, sh) = shell_size(card_w, card_h, scale_pct);
    let (sw_u, sh_u) = (sw.max(0) as usize, sh.max(0) as usize);
    if out.len() != sw_u * sh_u || sw_u == 0 || sh_u == 0 {
        return;
    }
    let radius = dev(RADIUS as f32, scale_pct);
    let offset = dev(OFFSET_Y as f32, scale_pct);
    let sigma = dev(SIGMA, scale_pct);
    let border = border_px(scale_pct) as f32;
    // The main windows' shadow-only shell is always the light theme's.
    let colours = match look {
        Look::Card(colours) => colours,
        _ => CardColours::of(&Palette::LIGHT),
    };
    let (line_rgb, shadow_rgb, peak) = (colours.border, colours.shadow, f32::from(colours.peak) / 255.0);

    // ---- the shadow: a card-shaped mask, blurred on a coarser grid ---------
    let k = BLUR_GRID;
    let (lw, lh) = (sw_u.div_ceil(k), sh_u.div_ceil(k));
    let mut grid = vec![0.0f32; lw * lh];
    for j in 0..lh {
        for i in 0..lw {
            // The grid cell's centre in shell pixels, relative to the card
            // shifted down by the shadow's offset.
            let x = (i as f32 + 0.5) * k as f32 - m.left as f32;
            let y = (j as f32 + 0.5) * k as f32 - m.top as f32 - offset;
            // `coverage` is the pixel-wide ramp; on this coarser grid it is a
            // hard-ish edge the blur then erases.
            grid[j * lw + i] = coverage(rounded_rect_sdf(x, y, card_w as f32, card_h as f32, radius));
        }
    }
    blur(&mut grid, lw, lh, sigma / k as f32);

    let sample = |x: usize, y: usize| -> f32 {
        // Bilinear, with the cell centres at (i + 0.5) * k.
        let fx = ((x as f32 + 0.5) / k as f32 - 0.5).clamp(0.0, (lw - 1) as f32);
        let fy = ((y as f32 + 0.5) / k as f32 - 0.5).clamp(0.0, (lh - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(lw - 1), (y0 + 1).min(lh - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let top = grid[y0 * lw + x0] * (1.0 - tx) + grid[y0 * lw + x1] * tx;
        let bottom = grid[y1 * lw + x0] * (1.0 - tx) + grid[y1 * lw + x1] * tx;
        top * (1.0 - ty) + bottom * ty
    };

    // ---- compose: the card over the shadow ---------------------------------
    for y in 0..sh_u {
        for x in 0..sw_u {
            let cx = x as f32 + 0.5 - m.left as f32;
            let cy = y as f32 + 0.5 - m.top as f32;
            let d = rounded_rect_sdf(cx, cy, card_w as f32, card_h as f32, radius);
            if d < -1.0 && matches!(look, Look::ShadowOnly) {
                // Deep inside the footprint, which a shadow-only shell leaves
                // fully transparent: nearly every pixel of a main window's
                // shell, so it skips the shadow sample.
                out[y * sw_u + x] = 0;
                continue;
            }
            let shadow_a = (sample(x, y) * peak).clamp(0.0, 1.0);
            // `c` is the card's whole footprint; `inner` the part of it that
            // is the card's colour rather than the hairline round it.
            let c = coverage(d);
            if let Look::ShadowOnly = look {
                // Premultiplied shadow, with the footprint carved out of it.
                let a = shadow_a * (1.0 - c);
                let channel = |shadow: u8| -> u32 {
                    (shadow as f32 * a).round().clamp(0.0, 255.0) as u32
                };
                out[y * sw_u + x] = ((a * 255.0).round().clamp(0.0, 255.0) as u32) << 24
                    | channel(shadow_rgb[0]) << 16
                    | channel(shadow_rgb[1]) << 8
                    | channel(shadow_rgb[2]);
                continue;
            }
            let Look::Card(CardColours { fill, .. }) = look else { continue };
            let inner = coverage(d + border).min(c);
            // Premultiplied "card (fill + hairline) over shadow".
            let a = c + shadow_a * (1.0 - c);
            let channel = |card: u8, line: u8, shadow: u8| -> u32 {
                let v = card as f32 * inner
                    + line as f32 * (c - inner)
                    + shadow as f32 * shadow_a * (1.0 - c);
                v.round().clamp(0.0, 255.0) as u32
            };
            out[y * sw_u + x] = ((a * 255.0).round().clamp(0.0, 255.0) as u32) << 24
                | channel(fill[0], line_rgb[0], shadow_rgb[0]) << 16
                | channel(fill[1], line_rgb[1], shadow_rgb[1]) << 8
                | channel(fill[2], line_rgb[2], shadow_rgb[2]);
        }
    }
}

/// [`render_into`] into a fresh buffer. Pure; the tests' entry point.
pub fn render(card_w: i32, card_h: i32, scale_pct: i32, fill: [u8; 3]) -> (Vec<u32>, i32, i32) {
    let (sw, sh) = shell_size(card_w, card_h, scale_pct);
    let mut out = vec![0u32; (sw.max(0) * sh.max(0)) as usize];
    render_into(&mut out, card_w, card_h, scale_pct, fill);
    (out, sw, sh)
}

// ---------------------------------------------------------------------------
// The windows.
// ---------------------------------------------------------------------------

/// The class the shells register under. It has no title and is never found by
/// one.
const SHELL_CLASS: windows::core::PCWSTR = w!("DeskwardenCardShell");

/// One attached card.
#[derive(Clone, Copy)]
struct Entry {
    card: isize,
    shell: isize,
    /// The card's window procedure before [`attach`] replaced it.
    original: isize,
    /// The card's size the shell and region were last cut for.
    size: (i32, i32),
    scale_pct: i32,
    look: Look,
    /// The capture exclusion last mirrored onto the shell.
    affinity: u32,
    /// Inside the user's move/resize drag (`WM_ENTERSIZEMOVE` ..
    /// `WM_EXITSIZEMOVE`). A [`Look::ShadowOnly`] shadow sits the drag out:
    /// re-rendering a window-sized blurred bitmap on every mouse move made
    /// resizing crawl. It is painted once, when the drag ends.
    sizing: bool,
    /// A [`Look::ShadowOnly`] window made opaque for a resize drag: set at the
    /// drag's first `WM_SIZING`, cleared at `WM_EXITSIZEMOVE`. A transparent
    /// window's not-yet-painted strip is see-through, which showed the windows
    /// behind it all along the dragged edge; an opaque one shows its own
    /// background there. See [`set_per_pixel_alpha`].
    opaque: bool,
}

/// Every card with a shell. A `Vec`, not a single slot: two cards can be alive
/// at once (the notice and the picker), each on its own thread.
static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

fn entry_of(card: HWND) -> Option<Entry> {
    ENTRIES.lock().ok()?.iter().find(|e| e.card == card.0 as isize).copied()
}

fn store(entry: Entry) {
    if let Ok(mut all) = ENTRIES.lock() {
        match all.iter_mut().find(|e| e.card == entry.card) {
            Some(slot) => *slot = entry,
            None => all.push(entry),
        }
    }
}

fn forget(card: HWND) -> Option<Entry> {
    let mut all = ENTRIES.lock().ok()?;
    let at = all.iter().position(|e| e.card == card.0 as isize)?;
    Some(all.remove(at))
}

fn hwnd_of(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

fn screen_scale_pct() -> i32 {
    unsafe {
        let dc = GetDC(None);
        let dpi = GetDeviceCaps(dc, LOGPIXELSX);
        ReleaseDC(None, dc);
        if dpi > 0 {
            dpi * 100 / 96
        } else {
            100
        }
    }
}

unsafe extern "system" fn shell_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // Click-through: the card under it gets every mouse message.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(window, msg, wparam, lparam),
    }
}

fn register_class() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(shell_proc),
            lpszClassName: SHELL_CLASS,
            ..Default::default()
        };
        RegisterClassW(&class);
    });
}

fn create_shell(topmost: bool) -> Option<HWND> {
    register_class();
    let mut ex = WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;
    if topmost {
        ex |= WS_EX_TOPMOST.0;
    }
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(ex),
            SHELL_CLASS,
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )
    }
    .ok()
}

/// Paints the shell for `card_w` x `card_h` and places it at `at` (its own
/// top-left, device pixels) with `UpdateLayeredWindow`. Every GDI object made
/// here is gone before it returns. `false` if any step was refused.
fn paint_shell(shell: HWND, at: POINT, card_w: i32, card_h: i32, scale_pct: i32, look: Look) -> bool {
    let (sw, sh) = shell_size(card_w, card_h, scale_pct);
    if sw <= 0 || sh <= 0 {
        return false;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(screen);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: sw,
                biHeight: -sh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
        let mut ok = false;
        if let Ok(dib) = dib {
            if !bits.is_null() && !mem.is_invalid() {
                let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (sw * sh) as usize);
                render_look_into(pixels, card_w, card_h, scale_pct, look);
                let old = SelectObject(mem, dib);
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                let size = SIZE { cx: sw, cy: sh };
                let origin = POINT { x: 0, y: 0 };
                ok = UpdateLayeredWindow(
                    shell,
                    screen,
                    Some(&at),
                    Some(&size),
                    mem,
                    Some(&origin),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                )
                .is_ok();
                SelectObject(mem, old);
            }
            let _ = DeleteObject(dib);
        }
        if !mem.is_invalid() {
            let _ = DeleteDC(mem);
        }
        ReleaseDC(None, screen);
        ok
    }
}

/// Cuts the card's window region: the round rectangle of the same radius,
/// pulled in by [`REGION_INSET`]. The system owns the region afterwards.
fn cut_region(card: HWND, w: i32, h: i32, scale_pct: i32) {
    unsafe {
        let d = region_diameter(scale_pct);
        let inset = region_inset(scale_pct);
        let region = CreateRoundRectRgn(inset, inset, w - inset + 1, h - inset + 1, d, d);
        // `SetWindowRgn` takes ownership on success. On failure it does not,
        // so the region is deleted here -- a leak in a repaint-adjacent path
        // would exhaust the GDI table over a long-lived daemon.
        if SetWindowRgn(card, region, true) == 0 {
            let _ = DeleteObject(region);
        }
    }
}

/// The region that makes DWM stop drawing its own square shadow round a
/// [`Look::ShadowOnly`] window: the whole window as a round rectangle with
/// HALF the window's corner radius, so it cuts nothing the window's own
/// 18 px corner cut (done in the GL surface's alpha) has not already made
/// transparent. Measured: any non-rectangular region does it; the bare
/// rectangle does not.
fn shadow_region_diameter(scale_pct: i32) -> i32 {
    (dev(RADIUS as f32 / 2.0, scale_pct).round() as i32).max(1) * 2
}

fn cut_shadow_region(window: HWND, w: i32, h: i32, scale_pct: i32) {
    unsafe {
        let d = shadow_region_diameter(scale_pct);
        let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, d, d);
        // As `cut_region`: the system owns it on success, a refusal frees it.
        if SetWindowRgn(window, region, true) == 0 {
            let _ = DeleteObject(region);
        }
    }
}

/// [`cut_shadow_region`] without asking for a repaint: during a resize drag
/// the window repaints itself for the new size anyway.
fn cut_shadow_region_quietly(window: HWND, w: i32, h: i32, d: i32) {
    unsafe {
        let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, d, d);
        if SetWindowRgn(window, region, false) == 0 {
            let _ = DeleteObject(region);
        }
    }
}

/// The region a window wears while it is opaque for a resize drag: its whole
/// corner radius, because with per-pixel alpha off the GL surface's own
/// antialiased corner cut paints black rather than see-through.
fn full_region_diameter(scale_pct: i32) -> i32 {
    (dev(RADIUS as f32, scale_pct).round() as i32).max(1) * 2
}

/// **Per-pixel alpha off (`opaque`) or back on, for a transparent GL window.**
///
/// `winit` makes a `with_transparent(true)` window see-through with
/// `DwmEnableBlurBehindWindow` over an empty region; this is that call, and
/// its undoing. While it is off, DWM shows the window as opaque, so the strip
/// a resize drag exposes before the next frame reaches it is the window's own
/// background instead of whatever lies behind the window.
fn set_per_pixel_alpha(window: HWND, on: bool) {
    use windows::Win32::Graphics::Dwm::{DwmEnableBlurBehindWindow, DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND};
    use windows::Win32::Graphics::Gdi::CreateRectRgn;
    unsafe {
        let region = CreateRectRgn(0, 0, -1, -1);
        let bb = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
            fEnable: on.into(),
            hRgnBlur: region,
            fTransitionOnMaximized: false.into(),
        };
        let _ = DwmEnableBlurBehindWindow(window, &bb);
        let _ = DeleteObject(region);
    }
}

/// Fills the window's client area with the app's window background -- what an
/// opaque window shows in the strip a resize drag uncovers.
fn fill_with_window_background(window: HWND, dc: windows::Win32::Graphics::Gdi::HDC) {
    use windows::Win32::Graphics::Gdi::{CreateSolidBrush, FillRect};
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
    let colour = crate::win32_draw::rgb(crate::gdi_palette::window_bg());
    unsafe {
        let mut rect = RECT::default();
        if GetClientRect(window, &mut rect).is_ok() {
            let brush = CreateSolidBrush(colour);
            FillRect(dc, &rect, brush);
            let _ = DeleteObject(brush);
        }
    }
}

/// Gives the window back its rectangular region.
fn clear_region(window: HWND) {
    unsafe {
        SetWindowRgn(window, HRGN::default(), true);
    }
}

/// Mirrors the card's `SetWindowDisplayAffinity` onto the shell, so a card
/// excluded from capture does not leave its shadow behind in a recording.
fn mirror_affinity(entry: &mut Entry) {
    unsafe {
        let mut now = 0u32;
        if GetWindowDisplayAffinity(hwnd_of(entry.card), &mut now).is_ok() && now != entry.affinity {
            let _ = SetWindowDisplayAffinity(hwnd_of(entry.shell), WINDOW_DISPLAY_AFFINITY(now));
            entry.affinity = now;
        }
    }
}

/// Brings the shell into step with the card: size (re-paint and re-cut the
/// region), position, z-order directly below the card, and visibility.
/// `visible` overrides `IsWindowVisible`, which is not yet true inside
/// `WM_SHOWWINDOW`.
fn sync(card: HWND, visible: Option<bool>) {
    let Some(mut entry) = entry_of(card) else { return };
    let shell = hwnd_of(entry.shell);
    let mut rc = RECT::default();
    unsafe {
        if GetWindowRect(card, &mut rc).is_err() {
            return;
        }
    }
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    if w <= 0 || h <= 0 {
        return;
    }
    let mut show = visible.unwrap_or_else(|| unsafe { IsWindowVisible(card).as_bool() });
    let shadow_only = entry.look == Look::ShadowOnly;
    if shadow_only && show && !shadow_wanted(card, &rc) {
        // A maximized, fullscreen, minimized or cloaked window has no shadow.
        show = false;
    }
    let scale_pct = screen_scale_pct();
    let m = margins(scale_pct);
    let at = POINT { x: rc.left - m.left, y: rc.top - m.top };

    if shadow_only && entry.sizing && (w, h) != entry.size {
        // Mid-resize (a move-only drag keeps its size and its shadow): no
        // shadow, and the region kept in step without a forced repaint.
        // `size` is zeroed so the drag's end paints afresh.
        if show {
            // Opaque for the drag (see [`Entry::opaque`]): the region is the
            // corner, at the window's full radius, since the GL alpha cut no
            // longer shows through.
            let d = if entry.opaque { full_region_diameter(scale_pct) } else { shadow_region_diameter(scale_pct) };
            cut_shadow_region_quietly(card, w, h, d);
        }
        entry.size = (0, 0);
        store(entry);
        unsafe {
            let _ = SetWindowPos(shell, card, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE | SWP_HIDEWINDOW);
        }
        return;
    }

    if shadow_only && !show && entry.size != (0, 0) {
        // Maximized, fullscreen, minimized, cloaked or hidden: the window is
        // square again, and DWM's own treatment of it (none, at the screen's
        // edge) is the right one.
        clear_region(card);
        entry.size = (0, 0);
    }
    let resized = (w, h) != entry.size || scale_pct != entry.scale_pct;
    if resized {
        // Only worth painting for a card that is about to be seen; a hidden
        // card's next show repaints.
        if show {
            if paint_shell(shell, at, w, h, scale_pct, entry.look) {
                entry.size = (w, h);
                entry.scale_pct = scale_pct;
                if shadow_only {
                    cut_shadow_region(card, w, h, scale_pct);
                } else {
                    cut_region(card, w, h, scale_pct);
                }
            }
        } else {
            entry.size = (0, 0);
        }
    }
    mirror_affinity(&mut entry);
    store(entry);

    unsafe {
        let flags = if show { SWP_NOACTIVATE | SWP_NOSIZE | SWP_SHOWWINDOW } else { SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE | SWP_HIDEWINDOW };
        // `card` as the insert-after window puts the shell directly BELOW it.
        let _ = SetWindowPos(shell, card, at.x, at.y, 0, 0, flags);
    }
}

/// Whether a [`Look::ShadowOnly`] window has a shadow right now: not maximized,
/// not minimized, not covering its whole monitor (borderless fullscreen), and
/// not cloaked by this app (`window_host::Reveal` cloaks a window it has shown
/// but not yet presented, and a shadow standing alone on the desktop for that
/// moment would be the very flash the cloak exists to prevent).
fn shadow_wanted(card: HWND, rc: &RECT) -> bool {
    unsafe {
        if IsZoomed(card).as_bool() || IsIconic(card).as_bool() {
            return false;
        }
        let monitor = MonitorFromWindow(card, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let m = info.rcMonitor;
            if rc.left <= m.left && rc.top <= m.top && rc.right >= m.right && rc.bottom >= m.bottom {
                return false;
            }
        }
        let mut cloaked = 0u32;
        let read = DwmGetWindowAttribute(
            card,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        );
        !(read.is_ok() && cloaked & DWM_CLOAKED_APP != 0)
    }
}

/// Re-reads a [`Look::ShadowOnly`] window's state and shows or hides its
/// shadow to match. For the one change `WM_WINDOWPOSCHANGED` does not announce:
/// `window_host` cloaking or uncloaking the window. `hwnd` is the raw handle;
/// a window without a shell is left alone.
pub fn resync(hwnd: isize) {
    sync(hwnd_of(hwnd), None);
}

unsafe extern "system" fn card_proc(
    card: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let Some(entry) = entry_of(card) else {
        return DefWindowProcW(card, msg, wparam, lparam);
    };
    if msg == WM_DESTROY {
        // Before the card's own teardown, so the shell is not left standing
        // for the frame the card is already gone.
        let _ = DestroyWindow(hwnd_of(entry.shell));
    }
    // An opaque window mid-drag paints the strip the drag uncovers in its own
    // background, before the next frame reaches it. See [`Entry::opaque`].
    if msg == WM_ERASEBKGND && entry.opaque {
        fill_with_window_background(card, windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _));
        return LRESULT(1);
    }
    // The drag's first resize step: opaque, and the corner cut by the region.
    if msg == WM_SIZING && entry.sizing && !entry.opaque && matches!(entry.look, Look::ShadowOnly) {
        let mut e = entry;
        e.opaque = true;
        store(e);
        set_per_pixel_alpha(card, false);
    }
    let original = std::mem::transmute::<
        isize,
        Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
    >(entry.original);
    let result = CallWindowProcW(original, card, msg, wparam, lparam);
    match msg {
        WM_WINDOWPOSCHANGED => sync(card, None),
        WM_SHOWWINDOW => sync(card, Some(wparam.0 != 0)),
        WM_ENTERSIZEMOVE | WM_EXITSIZEMOVE => {
            if let Some(mut e) = entry_of(card) {
                e.sizing = msg == WM_ENTERSIZEMOVE;
                let was_opaque = std::mem::take(&mut e.opaque);
                store(e);
                if was_opaque {
                    // Back to see-through corners; `sync` below re-cuts the
                    // half-radius region and repaints the shadow.
                    set_per_pixel_alpha(card, true);
                }
            }
            sync(card, None);
        }
        WM_PAINT => {
            if let Some(mut e) = entry_of(card) {
                mirror_affinity(&mut e);
                store(e);
            }
        }
        WM_NCDESTROY => {
            let _ = forget(card);
        }
        _ => {}
    }
    result
}

/// Gives `card` the design's 18 px antialiased corners and soft shadow, with
/// `fill` as the card's own background colour. Call it once, right after
/// `CreateWindowExW` and before the first `ShowWindow`; it replaces each
/// card's old `round_corners`.
///
/// If the shell cannot be made the card falls back to Windows' own rounding,
/// which is the smaller radius but never a stair-stepped one.
pub fn attach(card: HWND, fill: Color32) {
    unsafe {
        if entry_of(card).is_some() {
            return;
        }
        let topmost = GetWindowLongPtrW(card, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0;
        let Some(shell) = create_shell(topmost) else {
            round_with_dwm(card);
            return;
        };
        let original = GetWindowLongPtrW(card, GWLP_WNDPROC);
        if original == 0 {
            let _ = DestroyWindow(shell);
            round_with_dwm(card);
            return;
        }
        store(Entry {
            card: card.0 as isize,
            shell: shell.0 as isize,
            original,
            size: (0, 0),
            scale_pct: screen_scale_pct(),
            // The border and shadow are the palette the card opened with; the
            // fill is the caller's (the palette's card).
            look: Look::Card(CardColours {
                fill: [fill.r(), fill.g(), fill.b()],
                ..CardColours::of(&crate::gdi_palette::current())
            }),
            affinity: 0,
            sizing: false,
            opaque: false,
        });
        // Windows stops rounding the card (the shell draws the corners) and
        // stops drawing its own shadow (the shell draws that too).
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            card,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const c_void,
            std::mem::size_of_val(&corner) as u32,
        );
        let policy = DWMNCRP_DISABLED;
        let _ = DwmSetWindowAttribute(
            card,
            DWMWA_NCRENDERING_POLICY,
            &policy as *const _ as *const c_void,
            std::mem::size_of_val(&policy) as u32,
        );
        SetWindowLongPtrW(card, GWLP_WNDPROC, card_proc as *const () as isize);
        // The card may already be sized and not yet shown: cut the region now
        // so its first frame is already round, and paint the shell when shown.
        let mut rc = RECT::default();
        if GetWindowRect(card, &mut rc).is_ok() {
            cut_region(card, rc.right - rc.left, rc.bottom - rc.top, screen_scale_pct());
        }
    }
}

/// Gives a frameless, transparent main window the soft rounded shadow of the
/// design, and takes away DWM's own square one. Call it on the thread that owns
/// the window (the subclass is run by that thread's message loop), once the
/// window exists; calling it again is a no-op. `login_ui::round_window_corners`
/// is the caller, from the UI closure on winit's event-loop thread.
///
/// Unlike [`attach`] this changes nothing about the window's silhouette: the
/// window cuts its own 18 px corners out of its alpha (`window_host::CornerMask`)
/// and asked DWM for no rounding of its own. The shell under it is
/// [`Look::ShadowOnly`] and hides itself while the window is maximized,
/// fullscreen, minimized or cloaked.
///
/// **DWM's own shadow is removed with a window region, not with
/// `DWMNCRP_DISABLED`.** Switching non-client rendering off does remove it, and
/// measured on the real window it brings back an opaque dark 1 px line along the
/// top edge (`login_ui::round_window_corners` records the history). A round-rect
/// region ([`shadow_region_diameter`], half the window's own radius, so it cuts
/// nothing visible) makes DWM draw no shadow and leaves the top row alone. The
/// region is dropped whenever the shell is hidden, so a maximized window is
/// the plain rectangle it was.
pub fn attach_shadow(window: HWND) {
    unsafe {
        if entry_of(window).is_some() {
            return;
        }
        let topmost = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0;
        let Some(shell) = create_shell(topmost) else { return };
        let original = GetWindowLongPtrW(window, GWLP_WNDPROC);
        if original == 0 {
            let _ = DestroyWindow(shell);
            return;
        }
        store(Entry {
            card: window.0 as isize,
            shell: shell.0 as isize,
            original,
            size: (0, 0),
            scale_pct: screen_scale_pct(),
            look: Look::ShadowOnly,
            affinity: 0,
            sizing: false,
            opaque: false,
        });
        SetWindowLongPtrW(window, GWLP_WNDPROC, card_proc as *const () as isize);
        // The window may already be on screen (cloaked or not): place the
        // shadow now rather than waiting for the next move.
        sync(window, None);
    }
}

fn round_with_dwm(card: HWND) {
    unsafe {
        let preference = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            card,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const c_void,
            std::mem::size_of_val(&preference) as u32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [u8; 3] = [255, 255, 255];

    fn px(buf: &[u32], w: i32, x: i32, y: i32) -> u32 {
        buf[(y * w + x) as usize]
    }
    fn alpha(p: u32) -> u32 {
        p >> 24
    }

    #[test]
    fn the_radius_is_the_designs_and_not_windows() {
        assert_eq!(RADIUS, 18);
        assert_eq!(RADIUS, crate::panel_draw::RADIUS);
        // The region is the same curve pulled in past the hairline: at 100 %
        // the hairline is 1 px, the inset 2 and the ellipse diameter 2 * (18 - 2).
        assert_eq!(border_px(100), 1);
        assert_eq!(region_inset(100), 2);
        assert_eq!(region_diameter(100), 32);
        assert_eq!(region_inset(200), 3);
        assert_eq!(region_diameter(200), 66);
        let light = CardColours::of(&Palette::LIGHT);
        let kbd = Palette::LIGHT.kbd_border;
        assert_eq!(light.border, [kbd.r(), kbd.g(), kbd.b()], "the hairline is the palette's kbd_border");
    }

    #[test]
    fn the_margins_hold_the_shadow_and_scale_with_the_display() {
        let m = margins(100);
        assert!(m.left > 40 && m.left == m.right, "{m:?}");
        // The shadow stands below the card, so more room under it than over it.
        assert!(m.bottom > m.top, "{m:?}");
        assert_eq!(m.bottom - m.top, 2 * OFFSET_Y);
        let m2 = margins(200);
        assert!(m2.left >= 2 * m.left - 1 && m2.left <= 2 * m.left + 1, "{m2:?} vs {m:?}");
        assert_eq!(shell_size(410, 500, 100), (410 + m.left + m.right, 500 + m.top + m.bottom));
    }

    #[test]
    fn the_cards_interior_is_opaque_in_the_cards_colour() {
        let fill = [0x12, 0x34, 0x56];
        let (buf, w, _) = render(200, 120, 100, fill);
        let m = margins(100);
        for (x, y) in [(100, 60), (18, 18), (30, 100), (180, 10)] {
            let p = px(&buf, w, m.left + x, m.top + y);
            assert_eq!(alpha(p), 255, "({x},{y}) is not opaque");
            assert_eq!(p & 0x00ff_ffff, 0x0012_3456, "({x},{y}) is not the card's colour");
        }
    }

    /// **The region's stair-steps lie over pixels that are exactly the card's
    /// colour.** The card's window region is stair-stepped, `region_inset`
    /// pixels in from the true curve; whatever the card paints there is shown,
    /// and the shell shows everywhere outside it. If the hairline or the
    /// antialiased edge reached the region's boundary, the stair-steps would
    /// show as a dotted line. Past the hairline -- every pixel centre more than
    /// `border + 0.5` inside the curve -- the shell is fully opaque card colour,
    /// and the hairline itself is the border colour.
    #[test]
    fn the_pixels_past_the_hairline_are_exactly_the_cards_colour() {
        let (buf, w, _) = render(200, 120, 100, WHITE);
        let m = margins(100);
        let mut inside = 0;
        let mut line = 0;
        for y in 0..120 {
            for x in 0..200 {
                let d = rounded_rect_sdf(x as f32 + 0.5, y as f32 + 0.5, 200.0, 120.0, 18.0);
                let p = px(&buf, w, m.left + x, m.top + y);
                if d <= -1.5 {
                    assert_eq!(p, 0xffff_ffff, "({x},{y}) at d={d} is not opaque white");
                    inside += 1;
                } else if (d + 0.5).abs() < 1e-4 {
                    // The edge's own first pixel row, along the straight
                    // sides: the hairline (the palette's kbd_border, #dcdad7).
                    assert_eq!(alpha(p), 255, "({x},{y}) hairline is not opaque");
                    assert_eq!(p & 0x00ff_ffff, 0x00dc_dad7, "({x},{y}) hairline is not the border colour: {p:#x}");
                    line += 1;
                }
            }
        }
        assert!(inside > 20_000, "control: only {inside} interior pixels were visited");
        assert!(line > 50, "control: only {line} hairline pixels were visited");
        // The region sits wholly inside that zone: inset 2 puts its boundary
        // at d = -2, a pixel clear of the -1.5 where the fill begins.
        assert!(region_inset(100) as f32 >= border_px(100) as f32 + 1.0);
    }

    #[test]
    fn the_corner_is_antialiased_not_stepped() {
        let (buf, w, _) = render(200, 120, 100, WHITE);
        let m = margins(100);
        // The card's own corner pixel is outside an 18 px arc: it must not be
        // opaque white, but it is not empty either (the shadow lies under it).
        let corner = px(&buf, w, m.left, m.top);
        assert!(alpha(corner) < 255, "the corner pixel is the card's colour: {corner:#x}");
        // Walk the top-left arc and collect the distinct coverage levels of the
        // boundary pixels: a stair-step has two (in, out); an antialiased edge
        // has many intermediate ones.
        let mut levels = std::collections::BTreeSet::new();
        for y in 0..18 {
            for x in 0..18 {
                let p = px(&buf, w, m.left + x, m.top + y);
                if p & 0x00ff_ffff != 0 && alpha(p) > 0 && alpha(p) < 255 {
                    levels.insert(alpha(p));
                }
            }
        }
        assert!(levels.len() >= 6, "the arc has {} distinct partial-alpha levels", levels.len());
    }

    #[test]
    fn the_shadow_is_soft_subtle_and_fades_to_nothing() {
        let (buf, w, h) = render(300, 200, 100, WHITE);
        let m = margins(100);
        let mid_x = m.left + 150;
        // Straight under the card the shadow is darkest right at the edge and
        // falls away monotonically to the shell's own edge.
        let below: Vec<u32> = (0..m.bottom)
            .map(|d| alpha(px(&buf, w, mid_x, m.top + 200 + d)))
            .collect();
        assert!(below[0] > 0, "no shadow under the card");
        for pair in below.windows(2) {
            assert!(pair[1] <= pair[0], "the shadow is not monotonic: {below:?}");
        }
        assert!(
            below[0] as f32 <= f32::from(Palette::LIGHT.shadow.a()) + 1.0,
            "the shadow is darker than its peak: {}",
            below[0]
        );
        assert!(*below.last().unwrap() <= 6, "the shadow has not faded at the shell's edge: {below:?}");
        // The whole outer ring of the shell is (nearly) clear, so the shell
        // has no visible rectangular edge.
        for x in 0..w {
            assert!(alpha(px(&buf, w, x, 0)) <= 6);
            assert!(alpha(px(&buf, w, x, h - 1)) <= 6);
        }
        for y in 0..h {
            assert!(alpha(px(&buf, w, 0, y)) <= 6);
            assert!(alpha(px(&buf, w, w - 1, y)) <= 6);
        }
        // It stands lower than it stands high.
        let above = alpha(px(&buf, w, mid_x, m.top - 1));
        assert!(above < below[0], "the shadow is not offset downwards: {above} vs {}", below[0]);
    }

    #[test]
    fn the_buffer_is_premultiplied() {
        let (buf, ..) = render(120, 80, 100, [255, 255, 255]);
        for p in buf {
            let a = alpha(p);
            for shift in [16, 8, 0] {
                assert!(
                    (p >> shift) & 0xff <= a + 1,
                    "channel exceeds alpha in a premultiplied pixel {p:#x}"
                );
            }
        }
    }

    #[test]
    fn the_dark_card_keeps_its_fill_and_has_an_edge_and_a_shadow_that_read_on_dark() {
        let fill = [0x26, 0x24, 0x23];
        let (card_w, card_h) = (200, 120);
        let (sw, sh) = shell_size(card_w, card_h, 100);
        let mut dark = vec![0u32; (sw * sh) as usize];
        render_look_into(&mut dark, card_w, card_h, 100, Look::Card(CardColours { fill, ..CardColours::of(&Palette::DARK) }));
        let mut light = vec![0u32; (sw * sh) as usize];
        render_look_into(&mut light, card_w, card_h, 100, Look::Card(CardColours { fill, ..CardColours::of(&Palette::LIGHT) }));
        let m = margins(100);
        let at = |buf: &[u32], x: i32, y: i32| buf[(y * sw + x) as usize];
        // The middle of the card is the fill, opaque.
        let centre = at(&dark, m.left + card_w / 2, m.top + card_h / 2);
        assert_eq!(centre, 0xff00_0000 | (0x26 << 16) | (0x24 << 8) | 0x23);
        // The hairline is the dark one, a step above the card: its pixel is
        // lighter than the fill.
        let edge = at(&dark, m.left + card_w / 2, m.top);
        assert!((edge >> 16) & 0xff > 0x26, "{edge:#x}");
        // The shadow below the card is stronger than the light shadow's.
        let below = |buf: &[u32]| (at(buf, m.left + card_w / 2, m.top + card_h + 8) >> 24) & 0xff;
        assert!(below(&dark) > below(&light), "{} vs {}", below(&dark), below(&light));
    }

    #[test]
    fn a_wrong_sized_buffer_is_left_alone() {
        let mut buf = vec![7u32; 10];
        render_into(&mut buf, 200, 120, 100, WHITE);
        assert!(buf.iter().all(|p| *p == 7));
    }

    #[test]
    fn blur_conserves_mass_away_from_the_edges() {
        let (w, h) = (80usize, 80usize);
        let mut grid = vec![0.0f32; w * h];
        grid[40 * w + 40] = 1000.0;
        blur(&mut grid, w, h, 4.0);
        let total: f32 = grid.iter().sum();
        assert!((total - 1000.0).abs() < 1.0, "{total}");
        assert!(grid[40 * w + 40] < 100.0, "the spike was not spread");
        assert!(grid[40 * w + 40] > grid[40 * w + 47], "the blur is not peaked at the centre");
    }

    // ---- the shadow-only look (the main windows) ---------------------------

    fn shadow_only(card_w: i32, card_h: i32, scale_pct: i32) -> (Vec<u32>, i32, i32) {
        let (sw, sh) = shell_size(card_w, card_h, scale_pct);
        let mut out = vec![0u32; (sw * sh) as usize];
        render_look_into(&mut out, card_w, card_h, scale_pct, Look::ShadowOnly);
        (out, sw, sh)
    }

    #[test]
    fn the_shadow_only_look_leaves_the_footprint_transparent() {
        let (buf, w, _) = shadow_only(300, 200, 100);
        let m = margins(100);
        for (x, y) in [(150, 100), (18, 18), (40, 180), (280, 20), (150, 1), (1, 100)] {
            assert_eq!(px(&buf, w, m.left + x, m.top + y), 0, "({x},{y}) inside the window is not transparent");
        }
    }

    #[test]
    fn the_shadow_only_look_follows_the_rounded_silhouette_not_a_square() {
        let (buf, w, _) = shadow_only(300, 200, 100);
        let m = margins(100);
        // The footprint's own corner pixel lies outside the 18 px curve, so
        // the shadow shows there; stepping out along the bottom-right
        // diagonal it only ever fades.
        let corner = alpha(px(&buf, w, m.left + 299, m.top + 199));
        assert!(corner > 0, "no shadow in the cut corner");
        let mut last = corner;
        for k in 1..40 {
            let a = alpha(px(&buf, w, m.left + 299 + k, m.top + 199 + k));
            assert!(a <= last, "the shadow gets darker moving away from the corner: {a} > {last}");
            last = a;
        }
        // Beside the window the shadow is darker than the same distance out
        // beyond its corner, which is what a rounded shadow does and a square
        // outline does not.
        let beside = alpha(px(&buf, w, m.left + 150, m.top + 199 + 3));
        let beyond = alpha(px(&buf, w, m.left + 299 + 3, m.top + 199 + 3));
        assert!(beyond < beside, "{beyond} vs {beside}");
        assert!(buf.iter().all(|p| (p >> 24) <= 255 * 21 / 100 + 1), "darker than the shadow's peak");
    }

    #[test]
    fn the_shadow_only_look_is_premultiplied() {
        let (buf, ..) = shadow_only(200, 120, 100);
        for p in buf {
            let a = p >> 24;
            for shift in [16, 8, 0] {
                assert!((p >> shift) & 0xff <= a, "channel exceeds alpha in {p:#x}");
            }
        }
    }

    #[test]
    fn the_region_that_removes_dwms_square_shadow_never_cuts_the_silhouette() {
        // Half the window's radius at 100 % and 200 %.
        assert_eq!(shadow_region_diameter(100), 18);
        assert_eq!(shadow_region_diameter(200), 36);
        assert!(shadow_region_diameter(100) / 2 < RADIUS);
    }

    // ---- source pins: the glue ---------------------------------------------

    fn production() -> String {
        let source = include_str!("card_shell.rs").replace("\r\n", "\n");
        let cut = source.find("#[cfg(test)]").expect("the test module marker");
        source[..cut]
            .lines()
            .map(|l| match l.find("//") {
                Some(at) => &l[..at],
                None => l,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_resize_drag_does_not_repaint_the_shadow_on_every_step() {
        let code = production();
        assert!(code.contains("WM_ENTERSIZEMOVE | WM_EXITSIZEMOVE =>"), "the shell no longer knows when a drag starts and ends");
        let at = code.find("entry.sizing && (w, h) != entry.size").expect("a resize no longer skips the shadow repaint");
        let branch = &code[at..at + code[at..].find("return;").expect("the branch ends")];
        assert!(!branch.contains("paint_shell("), "the shadow is painted mid-drag again");
        assert!(branch.contains("cut_shadow_region_quietly("), "the region no longer follows the drag");
    }

    #[test]
    fn a_shadow_only_shell_spends_nothing_inside_the_footprint() {
        let (card_w, card_h, scale_pct) = (1280, 820, 100);
        let (sw, sh) = shell_size(card_w, card_h, scale_pct);
        let mut out = vec![0xFFFF_FFFFu32; (sw * sh) as usize];
        let started = std::time::Instant::now();
        render_look_into(&mut out, card_w, card_h, scale_pct, Look::ShadowOnly);
        let m = margins(scale_pct);
        let centre = ((m.top + card_h / 2) * sw + m.left + card_w / 2) as usize;
        assert_eq!(out[centre], 0, "the footprint is transparent");
        // Measured, not a benchmark: it bounds the one paint a drag's end
        // costs, generously enough for a debug build on a busy machine.
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn the_shell_is_layered_click_through_and_never_activates() {
        let code = production();
        for needle in [
            "WS_EX_LAYERED",
            "WS_EX_TRANSPARENT",
            "WS_EX_NOACTIVATE",
            "WS_EX_TOOLWINDOW",
            "UpdateLayeredWindow(",
            "HTTRANSPARENT",
            "MA_NOACTIVATE",
        ] {
            assert!(code.contains(needle), "the shell no longer uses {needle}");
        }
        // The compositing mechanism `region_overlay` recorded as going black
        // is not what this uses.
        for gone in ["SetLayeredWindowAttributes", "LWA_ALPHA", "LWA_COLORKEY"] {
            assert!(!code.contains(gone), "{gone} is back");
        }
    }

    #[test]
    fn the_card_is_not_rounded_by_windows_and_is_cut_to_the_same_curve() {
        let code = production();
        assert!(code.contains("DWMWCP_DONOTROUND"));
        assert!(code.contains("DWMNCRP_DISABLED"));
        assert!(code.contains("SetWindowRgn("));
        assert!(code.contains("CreateRoundRectRgn("));
        // The only DWMWCP_ROUND is the fallback when the shell cannot be made.
        assert_eq!(code.matches("DWMWCP_ROUND").count(), 2, "one import and one fallback use");
        assert!(code.contains("fn round_with_dwm"));
    }

    #[test]
    fn the_shell_follows_the_card_and_goes_when_it_goes() {
        let code = production();
        for needle in [
            "WM_WINDOWPOSCHANGED =>",
            "WM_SHOWWINDOW =>",
            "WM_DESTROY",
            "WM_NCDESTROY =>",
            // Directly below the card in z-order.
            "SetWindowPos(shell, card,",
            "DestroyWindow(hwnd_of(entry.shell))",
        ] {
            assert!(code.contains(needle), "the shell glue lost `{needle}`");
        }
    }

    #[test]
    fn the_main_windows_get_a_shadow_only_shell_with_dwms_own_removed_by_a_region() {
        let code = production();
        let body = code
            .split("pub fn attach_shadow(")
            .nth(1)
            .expect("`attach_shadow` is gone")
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(body.contains("Look::ShadowOnly"));
        // Non-client rendering stays on: switching it off brings back DWM's
        // opaque 1 px top line (measured). The region removes the shadow.
        assert!(!body.contains("DWMNCRP_DISABLED"), "`attach_shadow` turned non-client rendering off");
        assert!(code.contains("fn cut_shadow_region") && code.contains("fn clear_region"));
        assert!(code.contains("shadow_wanted(card, &rc)"), "the shell no longer hides for maximized/fullscreen");
        for needle in ["IsZoomed(", "IsIconic(", "DWMWA_CLOAKED"] {
            assert!(code.contains(needle), "`shadow_wanted` lost {needle}");
        }
    }

    #[test]
    fn the_cloak_reaches_the_shadow() {
        let host = include_str!("window_host.rs").replace("\r\n", "\n");
        let cloak = host
            .split("fn set_cloak(hwnd: isize")
            .nth(1)
            .expect("`set_cloak` is gone")
            .split("\n}")
            .next()
            .unwrap();
        assert!(cloak.contains("crate::card_shell::resync(hwnd)"));
    }

    #[test]
    fn every_gdi_object_made_here_is_freed() {
        let code = production();
        // One DIB, one memory DC and one screen DC per paint, each released.
        assert_eq!(code.matches("CreateDIBSection(").count(), 1);
        assert_eq!(code.matches("CreateCompatibleDC(").count(), 1);
        assert!(code.contains("DeleteObject(dib)"));
        assert!(code.contains("DeleteDC(mem)"));
        // Both `GetDC(None)` users release what they took.
        assert_eq!(code.matches("GetDC(None)").count(), code.matches("ReleaseDC(None").count());
        // A refused region is deleted, not leaked.
        assert!(code.contains("DeleteObject(region)"));
    }

    #[test]
    fn every_card_hands_its_corners_to_the_shell() {
        for (name, source) in [
            ("picker_prompt", include_str!("picker_prompt.rs")),
            ("prompt_card", include_str!("prompt_card.rs")),
            ("notice_card", include_str!("notice_card.rs")),
            ("save_login_card", include_str!("save_login_card.rs")),
            ("unlock_prompt", include_str!("unlock_prompt.rs")),
            ("generate_prompt", include_str!("generate_prompt.rs")),
            ("tray_popup", include_str!("tray_popup.rs")),
        ] {
            let production = source.split("#[cfg(test)]").next().unwrap();
            let code: String = production
                .lines()
                .map(|l| match l.find("//") {
                    Some(at) => &l[..at],
                    None => l,
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                code.contains("crate::card_shell::attach("),
                "`{name}` no longer gives its corners to the card shell"
            );
            assert!(
                !code.contains("DWMWCP_ROUND"),
                "`{name}` asks Windows for its fixed 8 px corner again"
            );
        }
    }
}

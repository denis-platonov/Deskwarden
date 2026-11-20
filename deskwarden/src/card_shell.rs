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
//! # Classification
//!
//! This module **does create a window**, and says so. It is not a surface: it
//! holds no content, takes no input and no focus, and exists only while a card
//! does -- the raise guard in `crate::foreground` is about windows a user types
//! into, and a click-through shadow cannot "open behind something" in a way
//! that matters. See the entry in that file's `OPENS_NO_WINDOW`.

use crate::win32_draw::{coverage, rounded_rect_sdf};
use eframe::egui::Color32;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_NCRENDERING_POLICY,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateRoundRectRgn, DeleteDC, DeleteObject, GetDC,
    GetDeviceCaps, ReleaseDC, SelectObject, SetWindowRgn, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, LOGPIXELSX,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowDisplayAffinity,
    GetWindowLongPtrW, GetWindowRect, IsWindowVisible, RegisterClassW, SetWindowDisplayAffinity,
    SetWindowLongPtrW, SetWindowPos, UpdateLayeredWindow, GWLP_WNDPROC, GWL_EXSTYLE,
    HTTRANSPARENT, MA_NOACTIVATE, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_SHOWWINDOW, ULW_ALPHA, WINDOW_DISPLAY_AFFINITY, WINDOW_EX_STYLE, WM_DESTROY,
    WM_MOUSEACTIVATE, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_SHOWWINDOW, WM_WINDOWPOSCHANGED,
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

/// The shadow's strength where it is darkest, at the card's own edge. Kept
/// low on purpose: the brief is "subtle".
pub const PEAK_ALPHA: f32 = 0.20;

/// The shadow's colour: the design's ink at the shadow's alpha.
pub const SHADOW_RGB: [u8; 3] = [45, 43, 43];

/// How many full-resolution pixels one pixel of the shadow's blur grid covers.
const BLUR_GRID: usize = 4;

/// The panel's hairline: the design's `border: 1px solid #d7d3d3`, which is
/// `theme::BORDER_STRONG`. It was Windows' own DWM border before; the shell
/// draws it now, so it follows the new curve.
pub const BORDER_RGB: [u8; 3] = [0xd7, 0xd3, 0xd3];

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

/// Fills `out` (`shell_w` x `shell_h`, premultiplied `0xAARRGGBB`) with the
/// shell for a `card_w` x `card_h` card: the card's `fill` in an antialiased
/// rounded rectangle, over a blurred shadow. `out.len()` must be
/// `shell_w * shell_h` (the sizes [`shell_size`] answers). Pure.
pub fn render_into(out: &mut [u32], card_w: i32, card_h: i32, scale_pct: i32, fill: [u8; 3]) {
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
            let shadow_a = (sample(x, y) * PEAK_ALPHA).clamp(0.0, 1.0);
            let cx = x as f32 + 0.5 - m.left as f32;
            let cy = y as f32 + 0.5 - m.top as f32;
            let d = rounded_rect_sdf(cx, cy, card_w as f32, card_h as f32, radius);
            // `c` is the card's whole footprint; `inner` the part of it that
            // is the card's colour rather than the hairline round it.
            let c = coverage(d);
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
                | channel(fill[0], BORDER_RGB[0], SHADOW_RGB[0]) << 16
                | channel(fill[1], BORDER_RGB[1], SHADOW_RGB[1]) << 8
                | channel(fill[2], BORDER_RGB[2], SHADOW_RGB[2]);
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
    fill: [u8; 3],
    /// The capture exclusion last mirrored onto the shell.
    affinity: u32,
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
fn paint_shell(shell: HWND, at: POINT, card_w: i32, card_h: i32, scale_pct: i32, fill: [u8; 3]) -> bool {
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
                render_into(pixels, card_w, card_h, scale_pct, fill);
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
    let show = visible.unwrap_or_else(|| unsafe { IsWindowVisible(card).as_bool() });
    let scale_pct = screen_scale_pct();
    let m = margins(scale_pct);
    let at = POINT { x: rc.left - m.left, y: rc.top - m.top };

    let resized = (w, h) != entry.size || scale_pct != entry.scale_pct;
    if resized {
        // Only worth painting for a card that is about to be seen; a hidden
        // card's next show repaints.
        if show {
            if paint_shell(shell, at, w, h, scale_pct, entry.fill) {
                entry.size = (w, h);
                entry.scale_pct = scale_pct;
                cut_region(card, w, h, scale_pct);
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
    let original = std::mem::transmute::<
        isize,
        Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
    >(entry.original);
    let result = CallWindowProcW(original, card, msg, wparam, lparam);
    match msg {
        WM_WINDOWPOSCHANGED => sync(card, None),
        WM_SHOWWINDOW => sync(card, Some(wparam.0 != 0)),
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
            fill: [fill.r(), fill.g(), fill.b()],
            affinity: 0,
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
        assert_eq!(BORDER_RGB, [0xd7, 0xd3, 0xd3], "the design's hairline is theme::BORDER_STRONG");
        let strong = crate::theme::BORDER_STRONG;
        assert_eq!(BORDER_RGB, [strong.r(), strong.g(), strong.b()]);
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
                    // sides: the hairline.
                    assert_eq!(alpha(p), 255, "({x},{y}) hairline is not opaque");
                    assert_eq!(p & 0x00ff_ffff, 0x00d7_d3d3, "({x},{y}) hairline is not the border colour: {p:#x}");
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
            below[0] as f32 <= 255.0 * PEAK_ALPHA + 1.0,
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
            ("locked_card", include_str!("locked_card.rs")),
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

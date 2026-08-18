//! Copying one rectangle of the screen into a buffer of pixels.
//!
//! This is the OS-touching half of "scan a region of my screen" (design
//! section 6b). It is handed a rectangle and it produces pixels; what the
//! rectangle came from (a drag across a dimmed overlay) and what the pixels
//! are for ([`crate::qr::decode_qr`], then [`crate::otpauth::parse_otpauth`])
//! are somebody else's problem.
//!
//! # The pixels ARE the secret
//!
//! A QR code of an `otpauth://` URI is the seed in visual form, so this
//! module's output is exactly as sensitive as the seed itself. Two
//! consequences, both structural rather than a matter of care:
//!
//! * [`Rgba`]'s buffer is a [`Zeroizing`], so it wipes on drop.
//! * **Nothing here writes a file.** There is no temp bitmap, no debug dump,
//!   no diagnostic path that saves what it saw, and no `log` call carrying
//!   pixel data. The only thing this module reports about a capture is its
//!   dimensions and whether it worked -- including [`Rgba`]'s hand-written
//!   [`std::fmt::Debug`], which prints the size and never the contents.
//!
//! # Only the rectangle that was asked for
//!
//! [`capture_rect`] blits `rect`'s width and height out of the screen DC at
//! `rect`'s origin. There is no path through this module that captures a
//! whole monitor, or the whole desktop, implicitly: a rectangle that reaches
//! past the monitor it sits on is **clamped down** to it, never widened, and
//! a rectangle that is nowhere on any monitor is refused rather than
//! substituted with something else.
//!
//! # The seam, and what is behind it
//!
//! One function in this file talks to Win32: [`blit_rect`]. Everything else
//! -- normalising a drag into a rectangle, clamping it to a monitor, deciding
//! whether a buffer that came back is a protected window's black hole -- is a
//! pure function taking its world as an argument, and is unit-tested
//! directly. `BitBlt` itself is **not** unit-testable (no test in this crate
//! may capture the real screen), and that is accepted rather than faked: the
//! tests below prove the arithmetic around the call, not the call.

use zeroize::Zeroizing;

/// A rectangle in **virtual-screen physical pixels** -- the coordinate space
/// `BitBlt` from the screen DC, `MONITORINFO::rcMonitor` and
/// `SM_XVIRTUALSCREEN` all speak, whose origin is the primary monitor's
/// top-left and whose axes may go negative on a monitor placed left of or
/// above it.
///
/// `right` and `bottom` are **exclusive**, so an empty rectangle is one where
/// they are not greater than `left`/`top`, and the width of a rectangle is
/// plainly `right - left`.
///
/// **Not egui points.** `login_ui::monitor_work_areas` converts to points
/// because window placement happens there; this module deliberately does not,
/// because every consumer of these numbers is a GDI call that wants pixels,
/// and a round-trip through a scale factor is where an off-by-one crops a
/// QR's quiet zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ScreenRect {
    /// Width in pixels, `0` if the rectangle is empty or inverted.
    ///
    /// `i64` throughout: `right - left` over the full `i32` range overflows,
    /// and a rectangle assembled from two arbitrary drag points really can
    /// span it.
    pub fn width(&self) -> u32 {
        let w = i64::from(self.right) - i64::from(self.left);
        u32::try_from(w.max(0)).unwrap_or(u32::MAX)
    }

    /// Height in pixels, `0` if the rectangle is empty or inverted.
    pub fn height(&self) -> u32 {
        let h = i64::from(self.bottom) - i64::from(self.top);
        u32::try_from(h.max(0)).unwrap_or(u32::MAX)
    }

    /// Area in pixels, as `u64` so that a full-desktop rectangle cannot wrap.
    pub fn area(&self) -> u64 {
        u64::from(self.width()) * u64::from(self.height())
    }

    /// The overlap between two rectangles, or `None` if they do not overlap.
    ///
    /// Touching edges do **not** overlap, because `right`/`bottom` are
    /// exclusive: a rectangle ending at x=1920 and one starting at x=1920 are
    /// adjacent monitors, not an intersection.
    pub fn intersect(&self, other: &ScreenRect) -> Option<ScreenRect> {
        let out = ScreenRect {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        if out.width() == 0 || out.height() == 0 {
            None
        } else {
            Some(out)
        }
    }
}

/// Builds a rectangle out of the two ends of a drag, in either direction on
/// either axis.
///
/// **The right-to-left, bottom-to-top drag is the common case**, not the
/// exotic one: a right-handed user framing something on screen tends to start
/// past it and pull back. A capture path that assumed `anchor` was the
/// top-left would hand that user an empty rectangle and a "no code in that
/// region" for a box they drew perfectly well.
pub fn rect_from_drag(anchor: (i32, i32), cursor: (i32, i32)) -> ScreenRect {
    ScreenRect {
        left: anchor.0.min(cursor.0),
        top: anchor.1.min(cursor.1),
        right: anchor.0.max(cursor.0),
        bottom: anchor.1.max(cursor.1),
    }
}

/// The smallest side [`capture_rect`] will blit, in pixels.
///
/// This is a guard on the GDI call and on pointless work, **not** a judgement
/// about QR codes. A QR symbol is at least 21 modules on a side and needs a
/// quiet zone besides, so nothing near this size can hold one -- but deciding
/// that is [`crate::qr::decode_qr`]'s job, and its answer is the "No code in
/// that region" the design already has words for. This module refuses two
/// cases only: a zero-area rectangle (a click that never became a drag) and a
/// one-pixel one (a drag that moved a single pixel), because
/// `CreateCompatibleBitmap` with a zero dimension returns a stock 1x1
/// monochrome bitmap rather than failing, and a blit into that would look
/// like a successful capture of a black image -- which is precisely the
/// signature [`looks_blocked`] reads as a protected window. Refusing them
/// here keeps those two answers from being confused.
pub const MIN_SIDE: u32 = 2;

/// Why a capture produced nothing. Every variant names its reason, because
/// design 6d's whole point about this screen is that a refusal the user can
/// act on is different from a blank one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRefusal {
    /// The rectangle does not overlap any monitor at all.
    OffScreen,
    /// The rectangle is smaller than [`MIN_SIDE`] on at least one side.
    TooSmall,
    /// The rectangle is larger than [`crate::qr::MAX_PIXELS`]. Not reachable
    /// from a drag across real monitors; it exists so an absurd rectangle is
    /// refused in one comparison rather than in an allocation.
    TooLarge,
    /// A Win32 call failed: no screen DC, no memory DC, no bitmap, or
    /// `BitBlt`/`GetDIBits` returned failure.
    GdiFailed,
    /// The blit succeeded and returned uniform pure black -- the signature of
    /// a window whose app called `SetWindowDisplayAffinity`. See
    /// [`looks_blocked`] for what that detection can and cannot tell apart.
    Blocked,
}

impl CaptureRefusal {
    /// The headline, verbatim from design 6d for the variants it names.
    pub fn title(&self) -> &'static str {
        match self {
            CaptureRefusal::Blocked => "Screen capture is blocked",
            CaptureRefusal::OffScreen => "That region isn't on screen",
            CaptureRefusal::TooSmall => "That region is too small",
            CaptureRefusal::TooLarge => "That region is too large",
            CaptureRefusal::GdiFailed => "Windows couldn't copy that region",
        }
    }

    /// The sentence under the headline: what the user can do about it.
    pub fn detail(&self) -> &'static str {
        match self {
            CaptureRefusal::Blocked => {
                "The window is marked protected by its app. Use the secret the site prints \
                 under the code instead."
            }
            CaptureRefusal::OffScreen => "Drag the box over the code again.",
            CaptureRefusal::TooSmall => {
                "Drag a box around the whole code, including its white margin."
            }
            CaptureRefusal::TooLarge => "Drag a box around the code rather than the whole desktop.",
            CaptureRefusal::GdiFailed => {
                "Try once more, or enter the secret the site prints under the code by hand."
            }
        }
    }
}

/// A captured region: straight (non-premultiplied), row-major, top-to-bottom
/// RGBA8, exactly `width * height * 4` bytes, ready to hand to
/// [`crate::qr::decode_qr`].
///
/// The buffer is a [`Zeroizing`], so it wipes when this value drops, and this
/// type is deliberately not `Clone`: every copy of it is another copy of the
/// seed, and a derived `Clone` is how one gets made without anyone deciding
/// to make it.
pub struct Rgba {
    width: u32,
    height: u32,
    pixels: Zeroizing<Vec<u8>>,
}

impl Rgba {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels. Borrowed rather than handed over, so that the only owner
    /// -- and therefore the only thing that decides when they are wiped --
    /// stays this value.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Builds one from parts. `pub(crate)` and length-checked so that no
    /// caller can construct an `Rgba` whose dimensions lie about its buffer.
    pub(crate) fn from_parts(width: u32, height: u32, pixels: Zeroizing<Vec<u8>>) -> Option<Rgba> {
        let expected = u64::from(width) * u64::from(height) * 4;
        if expected == 0 || expected != pixels.len() as u64 {
            return None;
        }
        Some(Rgba {
            width,
            height,
            pixels,
        })
    }
}

/// Hand-written, and it must stay hand-written: `debug_leak_guard` refuses a
/// derived `Debug` on any type that can reach a [`Zeroizing`], and it is
/// right to -- `Zeroizing<Vec<u8>>` prints its inner value, so a derived impl
/// here would print the seed as a list of pixel bytes. Dimensions are the
/// useful part of a capture for a log line and are not sensitive; the pixels
/// never reach a formatter.
impl std::fmt::Debug for Rgba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Rgba {{ {}x{}, {} bytes not shown }}",
            self.width,
            self.height,
            self.pixels.len()
        )
    }
}

/// Cuts `rect` down to the monitor it belongs to, or refuses.
///
/// The monitor it belongs to is **the one it overlaps most**, which is the
/// answer for the case the user actually creates: a drag that starts on one
/// monitor and ends slightly past its edge onto another. The part on the
/// other monitor is dropped rather than the whole thing being refused,
/// because a QR code lives on one screen and the overhang is the slop in the
/// user's aim.
///
/// A rectangle that overlaps nothing is [`CaptureRefusal::OffScreen`]: with a
/// monitor unplugged between the drag and the capture, or with coordinates
/// from a stale overlay, there is no honest rectangle to substitute.
///
/// Pure -- the monitors are an argument. [`monitor_bounds`] is what reads the
/// real ones.
pub fn clamp_to_monitors(
    rect: ScreenRect,
    monitors: &[ScreenRect],
) -> Result<ScreenRect, CaptureRefusal> {
    let clamped = monitors
        .iter()
        .filter_map(|m| rect.intersect(m))
        .max_by_key(|overlap| overlap.area())
        .ok_or(CaptureRefusal::OffScreen)?;

    if clamped.width() < MIN_SIDE || clamped.height() < MIN_SIDE {
        return Err(CaptureRefusal::TooSmall);
    }
    if clamped.area() > crate::qr::MAX_PIXELS as u64 {
        return Err(CaptureRefusal::TooLarge);
    }
    Ok(clamped)
}

/// Whether a captured buffer carries the signature of a window whose app
/// called `SetWindowDisplayAffinity` -- **every pixel exactly pure black**.
///
/// # Why "exactly", and why not a brightness threshold
///
/// This is the decision in this module most likely to be got wrong, so the
/// argument is written out rather than assumed.
///
/// A protected window does not make `BitBlt` fail. The call succeeds and the
/// destination comes back filled with zeroes, so a naive success check
/// reports success and hands back nothing -- which is why this exists at all.
/// The tempting detection is "average brightness below N", and it is wrong,
/// because the thing being captured is a **QR code, which is black by
/// nature**: a dark-themed page renders a QR as light modules on black, and a
/// tight crop of one can be three-quarters black with no white margin at all.
/// Any brightness threshold high enough to be robust would call that region
/// protected and send the user off to type the secret by hand for a code that
/// would have decoded.
///
/// So the test is not "dark". It is **uniform and exactly zero**: every
/// pixel's red, green and blue bytes are `0`. That is a claim with no
/// tolerance in it, and its safety comes from a second observation rather
/// than from the number: *a region whose every pixel is identical cannot
/// contain a QR code*, because a QR code is by definition two colours. So the
/// only regions this can misdiagnose are ones that were going to fail to
/// decode anyway, and the mistake is between two refusals -- "protected" vs
/// "no code in that region" -- never between a refusal and a success. That is
/// what makes an exact test affordable where a fuzzy one would not be.
///
/// The alpha byte is **not** examined. `BitBlt` from the screen DC into a
/// compatible bitmap leaves alpha undefined -- in practice zero, but it is
/// not a promise -- so folding it in would make the verdict depend on a
/// driver detail.
///
/// # The honest limit
///
/// This is a *diagnosis*, not a detection of the API. It cannot see
/// `SetWindowDisplayAffinity`; it sees the shape of its result. A genuinely
/// all-black region -- a fully black wallpaper, a video letterbox, a
/// terminal -- produces the same bytes and gets the same verdict, and the
/// user is told a window is protected when it merely happened to be black.
/// The design's wording survives that ("Use the secret the site prints under
/// the code instead" is what they should do either way), and the alternative
/// wording would have been just as unhelpful, but it is a guess and is
/// documented as one.
///
/// An empty buffer is **not** blocked: there is nothing to have observed, and
/// the caller's refusal for that is [`CaptureRefusal::GdiFailed`].
pub fn looks_blocked(rgba: &[u8]) -> bool {
    if rgba.len() < 4 {
        return false;
    }
    rgba.chunks_exact(4).all(|px| px[0..3] == [0, 0, 0])
}

/// Every monitor's full bounds (including the taskbar, unlike
/// `login_ui::monitor_work_areas`'s work areas), in virtual-screen physical
/// pixels. Empty if the enumeration fails.
///
/// **Full bounds, not work areas**, because a QR code can perfectly well sit
/// under a floating taskbar or in a maximised window's last row, and clamping
/// a drag to the work area would silently crop it.
///
/// **No DPI conversion, and none is missing.** `login_ui::monitor_work_areas`
/// divides by a scale factor and carries an honest caveat about mixed-DPI
/// desktops for doing so; it has to, because its consumer places windows in
/// egui points. This one's consumer is `BitBlt`, which speaks the same
/// physical pixels `rcMonitor` is already reported in, so the conversion --
/// and the whole class of error it brings -- simply does not arise here.
pub fn monitor_bounds() -> Vec<ScreenRect> {
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};

    let mut found: Vec<ScreenRect> = Vec::new();

    unsafe extern "system" fn callback(
        monitor: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        lparam: LPARAM,
    ) -> windows::Win32::Foundation::BOOL {
        let found = unsafe { &mut *(lparam.0 as *mut Vec<ScreenRect>) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            found.push(ScreenRect {
                left: info.rcMonitor.left,
                top: info.rcMonitor.top,
                right: info.rcMonitor.right,
                bottom: info.rcMonitor.bottom,
            });
        }
        true.into()
    }

    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM(&mut found as *mut Vec<ScreenRect> as isize),
        );
    }

    found
}

/// Copies one rectangle of the screen into an owned, self-wiping buffer.
///
/// `rect` is clamped to the monitor it overlaps most (see
/// [`clamp_to_monitors`]) and then blitted; nothing outside it is read.
///
/// This is the only function in the feature that touches the OS, and it is
/// therefore the only one that has no unit test. Everything it decides before
/// and after [`blit_rect`] -- the clamp, the size bounds, the blocked-window
/// diagnosis -- is a pure function tested on its own.
pub fn capture_rect(rect: ScreenRect) -> Result<Rgba, CaptureRefusal> {
    let clamped = clamp_to_monitors(rect, &monitor_bounds())?;
    let captured = blit_rect(clamped)?;
    if looks_blocked(captured.pixels()) {
        return Err(CaptureRefusal::Blocked);
    }
    Ok(captured)
}

/// **The seam.** The one function here that calls Win32, kept as small as it
/// can be made: it takes an already-clamped, already-bounds-checked rectangle
/// and returns its pixels, and it makes no decisions of its own. Every
/// judgement in this module lives on the pure side of this call.
///
/// `CAPTUREBLT` is included in the raster op so that layered windows -- which
/// is what a browser's or an authenticator app's popup often is -- are in the
/// result rather than punched out of it. Its cost is a screen flicker on some
/// configurations, which is acceptable for a one-shot capture the user asked
/// for and would not be for a live preview.
///
/// Not unit-tested, and cannot be: it reads the real screen, which no test in
/// this crate is allowed to do.
fn blit_rect(rect: ScreenRect) -> Result<Rgba, CaptureRefusal> {
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
    };

    let width = rect.width();
    let height = rect.height();
    // Re-checked rather than assumed: this function is private and its one
    // caller clamps first, but a zero dimension reaching
    // `CreateCompatibleBitmap` is the case whose failure looks like success.
    if width < MIN_SIDE || height < MIN_SIDE {
        return Err(CaptureRefusal::TooSmall);
    }
    let byte_len = usize::try_from(rect.area() * 4).map_err(|_| CaptureRefusal::TooLarge)?;

    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(CaptureRefusal::GdiFailed);
        }

        let mem = CreateCompatibleDC(screen);
        if mem.is_invalid() {
            ReleaseDC(None, screen);
            return Err(CaptureRefusal::GdiFailed);
        }

        let bitmap = CreateCompatibleBitmap(screen, width as i32, height as i32);
        if bitmap.is_invalid() {
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return Err(CaptureRefusal::GdiFailed);
        }

        let previous = SelectObject(mem, HGDIOBJ::from(bitmap));
        let blitted = BitBlt(
            mem,
            0,
            0,
            width as i32,
            height as i32,
            screen,
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        // Put the DC's original bitmap back before reading the pixels out:
        // `GetDIBits` is documented as reading a bitmap that is *not*
        // currently selected into a DC.
        SelectObject(mem, previous);

        let mut buffer: Zeroizing<Vec<u8>> = Zeroizing::new(vec![0u8; byte_len]);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Negative height requests a top-down DIB, matching the
                // row-major top-to-bottom order `decode_qr` documents.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let copied = if blitted.is_ok() {
            GetDIBits(
                mem,
                bitmap,
                0,
                height,
                Some(buffer.as_mut_ptr() as *mut core::ffi::c_void),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };

        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);

        if copied == 0 {
            // `buffer` drops here and wipes; nothing partial escapes.
            return Err(CaptureRefusal::GdiFailed);
        }

        // GDI hands back BGRA; `decode_qr` and egui both want RGBA.
        for px in buffer.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        Rgba::from_parts(width, height, buffer).ok_or(CaptureRefusal::GdiFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- 1. normalising a drag ---------------------------------------------

    /// All four drag directions produce the same rectangle. The
    /// bottom-right-to-top-left one is the case a right-handed user creates
    /// constantly, and the one an implementation that assumed the anchor was
    /// the top-left would turn into an empty box.
    #[test]
    fn a_drag_in_any_direction_normalises_to_the_same_rectangle() {
        let expected = ScreenRect {
            left: 100,
            top: 200,
            right: 340,
            bottom: 460,
        };
        assert_eq!(rect_from_drag((100, 200), (340, 460)), expected);
        assert_eq!(rect_from_drag((340, 460), (100, 200)), expected);
        assert_eq!(rect_from_drag((340, 200), (100, 460)), expected);
        assert_eq!(rect_from_drag((100, 460), (340, 200)), expected);
        // And positively: the rectangle really has the size that was dragged,
        // so "they all agree" is not four agreeing wrong answers.
        assert_eq!((expected.width(), expected.height()), (240, 260));
    }

    /// Negative coordinates are ordinary: a monitor to the left of the
    /// primary one has them, and a drag entirely inside it is all negative.
    #[test]
    fn a_drag_on_a_monitor_left_of_the_primary_normalises_too() {
        let r = rect_from_drag((-200, -900), (-1500, -1100));
        assert_eq!(
            r,
            ScreenRect {
                left: -1500,
                top: -1100,
                right: -200,
                bottom: -900
            }
        );
        assert_eq!((r.width(), r.height()), (1300, 200));
    }

    /// A click that never became a drag is a zero-area rectangle, and an
    /// inverted one is zero rather than a wrapped-around huge number.
    #[test]
    fn a_zero_area_rectangle_measures_zero_and_does_not_wrap() {
        let click = rect_from_drag((640, 480), (640, 480));
        assert_eq!((click.width(), click.height()), (0, 0));
        assert_eq!(click.area(), 0);

        let inverted = ScreenRect {
            left: 500,
            top: 500,
            right: 100,
            bottom: 100,
        };
        assert_eq!((inverted.width(), inverted.height()), (0, 0));

        // The extreme: a rectangle spanning the whole `i32` range would
        // overflow an `i32` subtraction. Control that the arithmetic is done
        // wide enough to answer at all.
        let huge = ScreenRect {
            left: i32::MIN,
            top: i32::MIN,
            right: i32::MAX,
            bottom: i32::MAX,
        };
        assert_eq!(huge.width(), u32::MAX);
        assert!(huge.area() > u64::from(u32::MAX));
    }

    // -- 2. clamping to monitors -------------------------------------------

    fn primary() -> ScreenRect {
        ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    }

    /// A monitor placed to the RIGHT of the primary one, sharing its edge at
    /// x=1920 -- the ordinary two-screen desktop.
    fn second() -> ScreenRect {
        ScreenRect {
            left: 1920,
            top: 0,
            right: 3840,
            bottom: 1080,
        }
    }

    /// A rectangle wholly inside one monitor comes back untouched -- the
    /// positive half of every clamp claim below. Without this, "clamping
    /// works" would be consistent with a clamp that shrinks everything.
    #[test]
    fn a_rectangle_inside_a_monitor_is_returned_unchanged() {
        let r = ScreenRect {
            left: 300,
            top: 300,
            right: 500,
            bottom: 500,
        };
        assert_eq!(clamp_to_monitors(r, &[primary(), second()]), Ok(r));
    }

    /// Overhanging the monitor's edge is cut to the edge, never widened, and
    /// the clamp really did remove something.
    #[test]
    fn a_rectangle_hanging_off_the_edge_is_cut_to_the_monitor() {
        let r = ScreenRect {
            left: 1800,
            top: 900,
            right: 2200,
            bottom: 1400,
        };
        let clamped = clamp_to_monitors(r, &[primary()]).expect("overlaps the primary");
        assert_eq!(
            clamped,
            ScreenRect {
                left: 1800,
                top: 900,
                right: 1920,
                bottom: 1080
            }
        );
        assert!(clamped.area() < r.area(), "the clamp removed nothing");
    }

    /// Straddling two monitors: the result is the larger overlap, whichever
    /// monitor that is, and it never spans both. Asserted in both directions
    /// so the answer is not just "always the first monitor in the list".
    #[test]
    fn a_rectangle_straddling_two_monitors_lands_on_the_one_it_covers_most() {
        // Mostly on the primary: 1620..1920 is 300 wide, 1920..1970 is 50.
        let mostly_left = ScreenRect {
            left: 1620,
            top: 100,
            right: 1970,
            bottom: 300,
        };
        assert_eq!(
            clamp_to_monitors(mostly_left, &[primary(), second()]),
            Ok(ScreenRect {
                left: 1620,
                top: 100,
                right: 1920,
                bottom: 300
            })
        );

        // Mostly on the second, and with the monitor list in the same order,
        // so the choice is driven by overlap and not by position in the list.
        let mostly_right = ScreenRect {
            left: 1870,
            top: 100,
            right: 2400,
            bottom: 300,
        };
        assert_eq!(
            clamp_to_monitors(mostly_right, &[primary(), second()]),
            Ok(ScreenRect {
                left: 1920,
                top: 100,
                right: 2400,
                bottom: 300
            })
        );
    }

    /// Wholly off every monitor is a named refusal, not a substituted
    /// rectangle and not a silent whole-screen capture.
    #[test]
    fn a_rectangle_on_no_monitor_is_refused_by_name() {
        let nowhere = ScreenRect {
            left: 5000,
            top: 5000,
            right: 5200,
            bottom: 5200,
        };
        assert_eq!(
            clamp_to_monitors(nowhere, &[primary(), second()]),
            Err(CaptureRefusal::OffScreen)
        );
        // Control: the same call with a monitor that does contain it succeeds,
        // so `OffScreen` is about the geometry and not about the function.
        let far = ScreenRect {
            left: 4000,
            top: 4000,
            right: 6000,
            bottom: 6000,
        };
        assert!(clamp_to_monitors(nowhere, &[far]).is_ok());
    }

    /// An empty monitor list -- `EnumDisplayMonitors` having failed -- is
    /// `OffScreen` rather than an unclamped capture. The fallback is a
    /// refusal, not a whole screen.
    #[test]
    fn no_monitors_at_all_refuses_rather_than_capturing_unclamped() {
        let r = ScreenRect {
            left: 10,
            top: 10,
            right: 200,
            bottom: 200,
        };
        assert_eq!(clamp_to_monitors(r, &[]), Err(CaptureRefusal::OffScreen));
    }

    /// Adjacent edges do not count as overlap: a rectangle that only touches
    /// the second monitor's left edge belongs entirely to the primary.
    #[test]
    fn touching_a_monitors_edge_is_not_overlapping_it() {
        let touching = ScreenRect {
            left: 1900,
            top: 0,
            right: 1920,
            bottom: 100,
        };
        assert_eq!(clamp_to_monitors(touching, &[second()]), Err(CaptureRefusal::OffScreen));
        assert!(clamp_to_monitors(touching, &[primary()]).is_ok());
    }

    /// Zero-area and one-pixel rectangles are refused as `TooSmall`, with the
    /// two-pixel one accepted right beside them so the boundary is pinned
    /// rather than merely "small things fail".
    #[test]
    fn zero_area_and_one_pixel_rectangles_are_refused_and_two_pixels_is_not() {
        let at = |w: i32, h: i32| ScreenRect {
            left: 100,
            top: 100,
            right: 100 + w,
            bottom: 100 + h,
        };
        assert_eq!(clamp_to_monitors(at(0, 0), &[primary()]), Err(CaptureRefusal::OffScreen));
        assert_eq!(clamp_to_monitors(at(1, 1), &[primary()]), Err(CaptureRefusal::TooSmall));
        // One pixel thin in one axis only, which is what a straight-line drag
        // produces and what a `width == 0` check alone would let through.
        assert_eq!(clamp_to_monitors(at(1, 400), &[primary()]), Err(CaptureRefusal::TooSmall));
        assert_eq!(clamp_to_monitors(at(400, 1), &[primary()]), Err(CaptureRefusal::TooSmall));
        assert!(clamp_to_monitors(at(2, 2), &[primary()]).is_ok());
        assert_eq!(MIN_SIDE, 2, "the boundary this test pins is `MIN_SIDE`");
    }

    /// A rectangle clamped down to a legal size passes even though the
    /// rectangle handed in was absurd -- the bound is on what is captured,
    /// not on what was asked for.
    #[test]
    fn an_absurd_rectangle_clamped_onto_a_monitor_is_captured_not_refused() {
        let absurd = ScreenRect {
            left: -100_000,
            top: -100_000,
            right: 100_000,
            bottom: 100_000,
        };
        assert!(absurd.area() > crate::qr::MAX_PIXELS as u64);
        assert_eq!(clamp_to_monitors(absurd, &[primary()]), Ok(primary()));
    }

    /// And the `TooLarge` bound really can fire, on a monitor rect large
    /// enough to exceed it -- so the variant is not dead code that nothing
    /// can reach.
    #[test]
    fn a_region_larger_than_the_pixel_bound_is_refused_by_name() {
        let enormous = ScreenRect {
            left: 0,
            top: 0,
            right: 40_000,
            bottom: 40_000,
        };
        assert!(enormous.area() > crate::qr::MAX_PIXELS as u64);
        assert_eq!(
            clamp_to_monitors(enormous, &[enormous]),
            Err(CaptureRefusal::TooLarge)
        );
    }

    // -- 3. the blocked-window diagnosis -----------------------------------

    /// Builds a buffer of `count` pixels, each `(r, g, b, a)`.
    fn pixels(count: usize, px: [u8; 4]) -> Vec<u8> {
        px.iter().copied().cycle().take(count * 4).collect()
    }

    /// Uniform pure black is the protected-window signature. Alpha is
    /// deliberately not part of the test, so both an opaque and a
    /// zero-alpha black read as blocked -- which matters because `BitBlt`
    /// leaves alpha undefined.
    #[test]
    fn a_uniformly_black_buffer_reads_as_blocked_whatever_its_alpha_says() {
        assert!(looks_blocked(&pixels(64, [0, 0, 0, 0])));
        assert!(looks_blocked(&pixels(64, [0, 0, 0, 255])));
    }

    /// **The case this threshold exists to protect.** A tight crop of a
    /// dark-mode QR code is mostly black -- here 63 of 64 pixels -- and must
    /// NOT be called blocked, because it is exactly the region that would
    /// have decoded. One non-black pixel is enough to prove the compositor
    /// gave us real content.
    #[test]
    fn a_nearly_black_region_is_not_blocked_because_a_qr_is_black_by_nature() {
        let mut nearly = pixels(64, [0, 0, 0, 255]);
        nearly[4 * 40] = 1; // a single red byte of 1, the smallest possible
        assert!(
            !looks_blocked(&nearly),
            "a region 63/64 black was called a protected window; a dark-mode QR code looks \
             like this and would be refused instead of decoded"
        );

        // And the same buffer without that one byte IS blocked, so the
        // difference is that pixel and not something about the fixture.
        let all_black = pixels(64, [0, 0, 0, 255]);
        assert!(looks_blocked(&all_black));
    }

    /// A non-black uniform region -- a white margin, a solid brand colour --
    /// is not blocked either. Only zero is the signature.
    #[test]
    fn a_uniform_non_black_buffer_is_not_blocked() {
        assert!(!looks_blocked(&pixels(64, [255, 255, 255, 255])));
        assert!(!looks_blocked(&pixels(64, [0, 0, 1, 255])));
        assert!(!looks_blocked(&pixels(64, [1, 0, 0, 255])));
        assert!(!looks_blocked(&pixels(64, [0, 1, 0, 255])));
    }

    /// An ordinary QR-shaped buffer -- black and white in equal measure -- is
    /// not blocked. The control for the whole diagnosis: the common case must
    /// pass through it untouched.
    #[test]
    fn a_black_and_white_buffer_is_not_blocked() {
        let mut buf = Vec::new();
        for i in 0..64 {
            let v = if i % 2 == 0 { 0 } else { 255 };
            buf.extend_from_slice(&[v, v, v, 255]);
        }
        assert!(!looks_blocked(&buf));
    }

    /// An empty or truncated buffer is not "blocked": there is nothing to
    /// have observed, and calling it blocked would put the protected-window
    /// wording on a GDI failure.
    #[test]
    fn an_empty_buffer_is_not_diagnosed_as_blocked() {
        assert!(!looks_blocked(&[]));
        assert!(!looks_blocked(&[0, 0, 0]));
        // Control: four zero bytes -- one whole pixel -- is enough to judge.
        assert!(looks_blocked(&[0, 0, 0, 0]));
    }

    // -- 4. the refusal wording --------------------------------------------

    /// Design 6d's protected-window sentence, pinned by content the way this
    /// crate pins its other refusal messages. The user is told *why* nothing
    /// appeared and what to do instead; a blank result is what this replaces.
    #[test]
    fn the_protected_window_refusal_says_what_the_design_says() {
        assert_eq!(
            CaptureRefusal::Blocked.title(),
            "Screen capture is blocked"
        );
        assert_eq!(
            CaptureRefusal::Blocked.detail(),
            "The window is marked protected by its app. Use the secret the site prints under \
             the code instead."
        );
    }

    /// Every refusal names itself: no variant may fall back to a shared
    /// "something went wrong", because the whole point of the enum is that
    /// the user can tell an aiming mistake from a protected window.
    #[test]
    fn every_refusal_has_its_own_wording() {
        let all = [
            CaptureRefusal::OffScreen,
            CaptureRefusal::TooSmall,
            CaptureRefusal::TooLarge,
            CaptureRefusal::GdiFailed,
            CaptureRefusal::Blocked,
        ];
        let mut titles: Vec<&str> = all.iter().map(|r| r.title()).collect();
        titles.sort_unstable();
        let count = titles.len();
        titles.dedup();
        assert_eq!(titles.len(), count, "two refusals share a headline");
        for refusal in all {
            assert!(!refusal.title().is_empty() && !refusal.detail().is_empty());
            assert!(
                refusal.detail().ends_with('.'),
                "{refusal:?}'s detail is not a sentence"
            );
        }
    }

    // -- 5. the buffer, its Debug and its wipe ------------------------------

    /// `from_parts` refuses dimensions that disagree with the buffer, so an
    /// `Rgba` cannot exist whose `width`/`height` lie to `decode_qr` about how
    /// to read its bytes.
    #[test]
    fn a_buffer_whose_length_disagrees_with_its_dimensions_is_refused() {
        assert!(Rgba::from_parts(4, 4, Zeroizing::new(vec![0u8; 63])).is_none());
        assert!(Rgba::from_parts(4, 4, Zeroizing::new(vec![0u8; 65])).is_none());
        assert!(Rgba::from_parts(0, 4, Zeroizing::new(Vec::new())).is_none());
        // Control: the matching length really does construct.
        let ok = Rgba::from_parts(4, 4, Zeroizing::new(vec![7u8; 64])).expect("4*4*4 == 64");
        assert_eq!((ok.width(), ok.height(), ok.pixels().len()), (4, 4, 64));
    }

    /// The hand-written `Debug` prints the shape and never the pixels. Pinned
    /// with a buffer whose contents would be unmistakable if they leaked.
    #[test]
    fn the_debug_impl_prints_dimensions_and_not_pixels() {
        let marker = 0xAB;
        let rgba = Rgba::from_parts(2, 2, Zeroizing::new(vec![marker; 16])).expect("2*2*4 == 16");
        let printed = format!("{rgba:?}");
        assert_eq!(printed, "Rgba { 2x2, 16 bytes not shown }");
        assert!(
            !printed.contains("171") && !printed.to_lowercase().contains("ab,"),
            "a pixel value reached the formatter: {printed}"
        );
    }

    /// The captured pixels wipe when the buffer drops, under the crate's
    /// `#[global_allocator]` probe.
    ///
    /// **The control is asserted first**, as every probe test in this crate
    /// does, and it is not a formality: a zeroization test has shipped in
    /// this crate that could not fail. The control here proves the instrument
    /// can say "yes", and the second half -- an `Rgba` built over a *plain*
    /// `Vec` rather than a `Zeroizing` one -- proves it can still say "yes"
    /// for a buffer of exactly this shape, so the final `!` assertion is a
    /// claim about `Zeroizing` and not about pixel buffers being invisible to
    /// the probe.
    #[test]
    fn a_dropped_capture_does_not_hand_its_pixels_back_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        let needle = PROBE.as_bytes();
        // A pixel buffer that literally contains the probe bytes, padded to a
        // whole number of RGBA pixels. This stands in for the QR's pixels:
        // the probe scans freed blocks for a byte pattern, and what the bytes
        // mean is not something it can know.
        //
        // **Built with an exact capacity, never grown.** A `Vec` that grows
        // reallocs, and a realloc hands the old block -- probe bytes and all
        // -- back to the allocator. Outside this test's own armed window that
        // is invisible to its verdict but perfectly visible to whatever other
        // probe test is armed on another thread at that instant, which is the
        // cross-thread noise `login_ui`'s `hold_the_probe_lock` documents.
        // Reserving once removes the producer rather than filtering it.
        let padded = needle.len().div_ceil(4) * 4;
        let build = || {
            let mut v: Vec<u8> = Vec::with_capacity(padded);
            v.extend_from_slice(needle);
            v.resize(padded, 0);
            v
        };
        let pixel_count = (padded / 4) as u32;

        // Control 1: a plain `Vec` carrying the probe is seen.
        let bare = build();
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "control: the allocator probe did not see a plain pixel buffer carrying the probe \
             go back to the allocator, so every verdict below is meaningless"
        );

        // Control 2: and it is still seen when that same buffer is wrapped in
        // an `Rgba` whose field is NOT `Zeroizing` -- built here by hand,
        // because `from_parts` will not make one. Without this, the assertion
        // below would also pass for an `Rgba` the probe simply cannot see.
        struct Unwiped(#[allow(dead_code)] Vec<u8>);
        let unwiped = Unwiped(build());
        assert!(
            plaintext_reached_the_allocator(move || drop(unwiped)),
            "control: the probe cannot see a pixel buffer of this shape at all, so the claim \
             below would hold for a capture that never wiped"
        );

        // The claim.
        let captured = Rgba::from_parts(pixel_count, 1, Zeroizing::new(build()))
            .expect("the padded probe is a whole number of pixels");
        assert!(
            !plaintext_reached_the_allocator(move || drop(captured)),
            "a dropped `Rgba` handed its captured pixels back to the allocator in the clear -- \
             those pixels are the TOTP seed in visual form"
        );
    }

    // -- 6. what is NOT tested ---------------------------------------------

    /// Not a test of behaviour: a note in the place a reader looks for one.
    ///
    /// [`blit_rect`] and [`monitor_bounds`] read the real screen and the real
    /// display configuration, and no test in this crate may do either. So
    /// nothing here proves that a capture *works* -- only that the rectangle
    /// handed to `BitBlt` is the right one, that a black result is diagnosed,
    /// and that the buffer wipes. **The manual check is the feature itself:**
    /// drag a box around a QR code on screen and see the code read; drag one
    /// over a window whose app protects itself and see 6d's named refusal
    /// rather than a blank.
    ///
    /// It is a `#[test]` so that this paragraph is in the file where the
    /// tests are and cannot drift out of the suite; it asserts the one thing
    /// it can, which is that the seam it describes is still where it says.
    #[test]
    fn the_os_call_itself_is_not_unit_tested_and_this_says_so() {
        let source = include_str!("screen_capture.rs");
        // Exactly one function in this file calls into GDI's blitting family.
        // If a second one appears, this seam is no longer one call and this
        // note is no longer true.
        assert_eq!(
            source.matches("BitBlt(").count(),
            2,
            "the `BitBlt` seam is not exactly one import and one call any more; the claim that \
             everything except one function is pure needs re-checking"
        );
        assert!(
            source.contains("fn blit_rect("),
            "the seam this note describes has been renamed"
        );
    }
}

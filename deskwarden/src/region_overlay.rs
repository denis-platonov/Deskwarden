//! **6b -- the dimmed full-screen surface the user drags a box on.**
//!
//! The desktop dims, the selection stays lit, and the overlay **locks on
//! before the user releases**: *"Code found · release to read"*. That last
//! part is the whole character of this screen. A capture tool that only tells
//! you whether it worked after you let go teaches the user to drag, release,
//! read a refusal, and drag again; one that says "found" while the button is
//! still down turns the same drag into a single, confident gesture.
//!
//! # What this module is, and what it deliberately is not
//!
//! It is the **window**. It is not the capture and it is not the decoder.
//! [`crate::screen_capture`] owns every piece of geometry and the one GDI
//! call; [`crate::qr`] owns the decode. This file calls both and adds nothing
//! of its own to either -- in particular it does **not** re-derive a rectangle
//! from two drag points. [`crate::screen_capture::rect_from_drag`] already
//! normalises a drag in all four directions, and a second copy of that
//! arithmetic living next to a mouse handler is the defect this crate has
//! lost to before: the copy near the UI is the one that gets "fixed" for a
//! right-to-left drag, and the two then disagree about what the user selected.
//! [`Drag::rect`] is a one-line delegation for exactly that reason.
//!
//! # Why this is an `egui` viewport and not a Win32 window
//!
//! The same reason [`crate::scratch_window`] is one, and it is worth
//! restating because this surface is more tempting to build the raw way. The
//! picker that launches it lives inside `vault_window::run`'s
//! `eframe::run_ui_native` closure, so a second `eframe::run_*native` is not
//! available -- `winit` refuses to build a second event loop while one is
//! alive. The rehearsal window's first implementation answered that with
//! `CreateWindowExW`, it worked, and the user's verdict on it was that it
//! "doesn't look good": it was the only surface in the product with none of
//! the app's theme, tokens or type. [`egui::Context::show_viewport_deferred`]
//! opens a second **real OS window inside the already-running loop**, painted
//! by egui, and that is what this is.
//!
//! # What is testable here, and what is not
//!
//! Testable, and tested below as plain functions with no window anywhere:
//!
//! * [`Drag::rect`] and [`whole_screen`] -- the geometry that is this
//!   module's rather than `screen_capture`'s;
//! * [`to_screen`] -- the one conversion this window owns, points to
//!   virtual-screen pixels;
//! * [`lockon_badge`] -- the found/not-found label decision;
//! * [`DecodeThrottle`] -- the bound on how often a decode is attempted;
//! * [`read_region_with`] -- every outcome, through seams.
//!
//! **Not testable, and no assertion below pretends otherwise:**
//!
//! * that Windows grants this window the foreground when it opens. The raise
//!   is asked for; the OS may refuse it and flash a taskbar button instead.
//! * that the viewport covers **every** monitor. The rectangle handed to the
//!   builder is computed from [`crate::screen_capture::monitor_bounds`], and
//!   that computation is tested -- but whether the window manager honours a
//!   position and size spanning a mixed-DPI virtual desktop is a fact about a
//!   real desktop.
//! * that the dimming composites correctly over other windows. That needs a
//!   transparent, always-on-top window over a real compositor.
//! * that the overlay itself is excluded from the blit. See
//!   [`exclude_from_capture`].
//!
//! Every mechanism here is a *necessary condition* for those four, never a
//! proof of them.
//!
//! # Security
//!
//! The captured pixels are the secret in visual form, so nothing here writes
//! them anywhere: [`crate::screen_capture::Rgba`] wipes on drop, the decoded
//! URI is a [`Zeroizing`], and [`Outcome`]'s `Debug` is hand-written so that
//! `debug_leak_guard` has nothing to catch. Escape cancels and captures
//! nothing at all -- [`Outcome::Cancelled`] carries no buffer, because there
//! was never one to carry.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use eframe::egui;
use zeroize::Zeroizing;

use crate::qr;
use crate::screen_capture::{self, CaptureRefusal, Rgba, ScreenRect, MIN_SIDE};
use crate::theme;

// ---------------------------------------------------------------------------
// The words, verbatim from design 6b
// ---------------------------------------------------------------------------

/// The window's title.
///
/// **Distinct from every other title in this process**, and that is
/// load-bearing rather than cosmetic: [`crate::foreground::raise_window`]
/// matches this process's windows *by string* and takes the first `EnumWindows`
/// match, and this window is open **alongside** the vault window. A title it
/// shared with that one would make the raise pick either. `foreground`'s
/// `only_one_window_of_this_process_can_exist_at_a_time` asserts the
/// distinctness rather than trusting this comment.
pub const REGION_TITLE: &str = "Deskwarden \u{2014} scan a region";

/// 6b's instruction, shown until a drag starts and again after one is
/// abandoned.
pub const DRAG_TITLE: &str = "Drag a box around the QR code";

/// 6b's sub-line. **"Nothing is saved yet" has to stay true of the code**:
/// this module writes nothing anywhere, and the confirmation screen is what
/// saves.
pub const DRAG_HINT: &str = "Deskwarden reads it the moment you let go. Nothing is saved yet.";

/// 6b's lock-on badge, shown **while the button is still down**.
pub const LOCKED_ON: &str = "Code found \u{b7} release to read";

/// 6b's two shortcut affordances, bottom right.
pub const WHOLE_SCREEN_HINT: &str = "Whole screen";
pub const CANCEL_HINT: &str = "Cancel";

/// How dark the desktop goes outside the selection. Alpha over black; the
/// selection itself is left entirely unpainted, which is what "stays lit"
/// means on a transparent viewport.
pub const DIM_ALPHA: u8 = 140;

// ---------------------------------------------------------------------------
// The geometry that is this module's own
// ---------------------------------------------------------------------------

/// A drag in progress: where the button went down, and where the pointer is
/// now, both in **virtual-screen physical pixels**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drag {
    pub anchor: (i32, i32),
    pub cursor: (i32, i32),
}

impl Drag {
    /// The selected rectangle.
    ///
    /// **A delegation, and it must stay one.** See this module's header: the
    /// four-direction normalisation lives in `screen_capture` and is tested
    /// there, and a second copy of it here is the bug this crate keeps
    /// re-introducing.
    pub fn rect(&self) -> ScreenRect {
        screen_capture::rect_from_drag(self.anchor, self.cursor)
    }
}

/// The rectangle 6b's *"Whole screen · A"* shortcut selects: the bounding box
/// of every monitor, in virtual-screen physical pixels. `None` when the
/// monitor enumeration came back empty, which is the one case where there is
/// no honest answer to substitute.
///
/// **A bounding box, not a union.** On an L-shaped desktop the box covers
/// pixels no monitor owns, and that is deliberate: this value is fed to
/// [`crate::screen_capture::clamp_to_monitors`] exactly like a drag is, and
/// that function cuts it down to the monitor it overlaps most. So "whole
/// screen" means *the whole of the dominant monitor* rather than a stitched
/// panorama -- which is also the only thing that could hold a QR code, since a
/// code does not span a bezel.
pub fn whole_screen(monitors: &[ScreenRect]) -> Option<ScreenRect> {
    let mut bounds: Option<ScreenRect> = None;
    for monitor in monitors {
        if monitor.width() == 0 || monitor.height() == 0 {
            continue;
        }
        bounds = Some(match bounds {
            None => *monitor,
            Some(so_far) => ScreenRect {
                left: so_far.left.min(monitor.left),
                top: so_far.top.min(monitor.top),
                right: so_far.right.max(monitor.right),
                bottom: so_far.bottom.max(monitor.bottom),
            },
        });
    }
    bounds
}

/// Converts a pointer position **in egui points, relative to this viewport**
/// into virtual-screen physical pixels -- the space
/// [`crate::screen_capture`] and every GDI call speak.
///
/// `origin` is the viewport's top-left in those same pixels, which is where
/// [`whole_screen`] put it. `points_per_pixel` is `Context::pixels_per_point`.
///
/// The rounding is [`f32::round`] and not a truncation: a truncating cast
/// biases every coordinate toward the origin, which on a tight crop eats the
/// QR's quiet zone on two sides and turns a decodable capture into "no code in
/// that region".
///
/// **The honest limit**: one scale factor for the whole virtual desktop is
/// wrong on a mixed-DPI setup, where a 200% monitor and a 100% monitor need
/// different ones. egui reports a single value per viewport, so this is the
/// only number available; the failure mode is a rectangle offset on the
/// secondary monitor, which the user sees and corrects by dragging again. It
/// is not a silent capture of the wrong region -- what is captured is what the
/// lit rectangle showed.
pub fn to_screen(origin: (i32, i32), points_per_pixel: f32, at: (f32, f32)) -> (i32, i32) {
    let scale = if points_per_pixel.is_finite() && points_per_pixel > 0.0 {
        points_per_pixel
    } else {
        1.0
    };
    (
        origin.0.saturating_add((at.0 * scale).round() as i32),
        origin.1.saturating_add((at.1 * scale).round() as i32),
    )
}

// ---------------------------------------------------------------------------
// The label decision
// ---------------------------------------------------------------------------

/// The badge over the selection, or `None` for "keep showing the
/// instruction".
///
/// **`None` and not a second sentence.** 6b shows the badge only once a code
/// is found; while the overlay is still searching it shows nothing extra,
/// because a "searching..." that appears on every drag would be on screen
/// almost all the time and would say nothing the user cannot already see. The
/// absence of the badge *is* the not-found state, and that is the whole
/// decision this function makes.
pub fn lockon_badge(found: bool) -> Option<&'static str> {
    if found {
        Some(LOCKED_ON)
    } else {
        None
    }
}

/// 6b's size readout under the selection, e.g. `250 × 250`.
pub fn size_label(rect: &ScreenRect) -> String {
    format!("{} \u{d7} {}", rect.width(), rect.height())
}

// ---------------------------------------------------------------------------
// The decode throttle
// ---------------------------------------------------------------------------

/// How long the overlay waits between lock-on decode attempts.
///
/// **Why a number at all.** A decode is a full binarisation and grid search
/// over the selected region. Mouse-move events arrive far faster than that on
/// a large one, so an unthrottled attempt-per-move is an unbounded queue of
/// work behind a moving pointer -- the selection rectangle visibly lags the
/// cursor, which is the exact opposite of what a live lock-on is for.
///
/// **Why 150 ms.** It is set by what the *user* can perceive rather than by
/// what the decoder costs, because the decoder's cost varies by two orders of
/// magnitude with the region size and no single number is right for both ends
/// of that. The requirement is that the badge appears to arrive "as you frame
/// it": under roughly a fifth of a second reads as immediate, and above it
/// reads as a delay the user starts waiting through. 150 ms sits under that
/// with margin, and at six or seven attempts a second the decoder is a small
/// fraction of a frame budget even on a large region.
///
/// It is a `const` rather than a literal in the loop so that
/// [`DecodeThrottle::new`] can be handed a different one by a test -- which is
/// what makes every branch of the throttle reachable without a clock and
/// without a window.
pub const DECODE_INTERVAL: Duration = Duration::from_millis(150);

/// Bounds how often a lock-on decode runs.
///
/// **Two gates, and the second one matters more than the first.** Time alone
/// is not enough: a pointer held still still produces frames (the overlay
/// repaints for its own cursor and hint), and a time-only throttle would
/// re-decode the *identical* rectangle six times a second forever, burning a
/// core for an answer that cannot change. So an attempt also requires the
/// rectangle to have **changed** since the last one attempted. Together they
/// make the attempt rate bounded above by the interval and bounded below by
/// the user actually moving.
///
/// Rectangles too small to hold anything are refused outright, so a click that
/// never became a drag costs nothing.
#[derive(Debug, Clone)]
pub struct DecodeThrottle {
    interval: Duration,
    last_attempt: Option<Instant>,
    last_rect: Option<ScreenRect>,
}

impl DecodeThrottle {
    /// A throttle with the given minimum spacing. [`DECODE_INTERVAL`] is what
    /// production passes.
    pub fn new(interval: Duration) -> Self {
        DecodeThrottle {
            interval,
            last_attempt: None,
            last_rect: None,
        }
    }

    /// Whether to attempt a decode of `rect` at `now`, recording the attempt
    /// if so.
    ///
    /// `now` is an argument rather than an `Instant::now()` inside, which is
    /// what lets a test drive the interval boundary exactly instead of
    /// sleeping through it.
    pub fn should_attempt(&mut self, rect: ScreenRect, now: Instant) -> bool {
        if rect.width() < MIN_SIDE || rect.height() < MIN_SIDE {
            return false;
        }
        if self.last_rect == Some(rect) {
            return false;
        }
        if let Some(last) = self.last_attempt {
            if now.saturating_duration_since(last) < self.interval {
                return false;
            }
        }
        self.last_attempt = Some(now);
        self.last_rect = Some(rect);
        true
    }

    /// The spacing this throttle was built with. Read by the test that pins
    /// production's value; not used by the overlay itself.
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

// ---------------------------------------------------------------------------
// Reading a region
// ---------------------------------------------------------------------------

/// What a region came to.
///
/// **Hand-written `Debug`, and it must stay hand-written**: `Decoded` holds a
/// [`Zeroizing<String>`], whose own `Debug` prints the seed. `debug_leak_guard`
/// refuses a derived one here and it is right to.
pub enum Outcome {
    /// Escape, or the window closed. **Nothing was captured** -- not a
    /// buffer, not a partial one.
    Cancelled,
    /// The pixels came back and held no QR code.
    NoCode,
    /// The capture itself refused, and says why. The words are
    /// [`CaptureRefusal::title`] and `detail`, from design 6d.
    Refused(CaptureRefusal),
    /// A QR decoded. The payload is **untrusted** -- it is whatever was on
    /// screen -- and is handed to `otpauth::parse_otpauth` by the caller, not
    /// treated as a URL, a path or a command by anything here.
    Decoded(Zeroizing<String>),
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::Cancelled => write!(f, "Cancelled"),
            Outcome::NoCode => write!(f, "NoCode"),
            Outcome::Refused(why) => write!(f, "Refused({why:?})"),
            Outcome::Decoded(text) => {
                write!(f, "Decoded({} chars not shown)", text.len())
            }
        }
    }
}

impl Outcome {
    /// Whether this is a code. The lock-on badge's input, and the reason the
    /// live path does not need to hold the string.
    pub fn is_decoded(&self) -> bool {
        matches!(self, Outcome::Decoded(_))
    }
}

/// The two calls this module makes into the outside world, as function
/// pointers.
///
/// Not an aesthetic choice: [`crate::screen_capture::capture_rect`] reads the
/// real screen and no test in this crate may do that. Behind a seam, every
/// arm of [`read_region_with`] is reachable from a test that builds its pixels
/// arithmetically.
#[derive(Clone, Copy)]
pub struct RegionSeams {
    pub capture: fn(ScreenRect) -> Result<Rgba, CaptureRefusal>,
    pub decode: fn(&[u8], usize, usize) -> Option<Zeroizing<String>>,
}

impl RegionSeams {
    /// The real ones. A test asserts these are the real functions by address,
    /// so a seam quietly re-pointed at a stub fails rather than passing.
    pub fn production() -> Self {
        RegionSeams {
            capture: screen_capture::capture_rect,
            decode: qr::decode_qr,
        }
    }
}

/// Captures `rect` and tries to read a QR out of it.
///
/// The `Rgba` is dropped at the end of this function in every arm, which is
/// what wipes the pixels; nothing here copies them anywhere, and no arm
/// returns them.
pub fn read_region_with(seams: &RegionSeams, rect: ScreenRect) -> Outcome {
    let pixels = match (seams.capture)(rect) {
        Ok(pixels) => pixels,
        Err(why) => return Outcome::Refused(why),
    };
    let (width, height) = (pixels.width() as usize, pixels.height() as usize);
    match (seams.decode)(pixels.pixels(), width, height) {
        Some(text) => Outcome::Decoded(text),
        None => Outcome::NoCode,
    }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// The viewport this window is. Derived from the title rather than freshly
/// generated: a second id would be a second OS window, and the whole
/// title-uniqueness argument in `foreground` is about there being one.
fn region_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of(REGION_TITLE)
}

/// What [`draw`] needs, extracted from the shared state under one lock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionView {
    /// The selection in **points relative to this viewport** -- already
    /// converted back out of screen pixels, because painting happens in
    /// points. `None` when no drag is in progress.
    pub selection: Option<egui::Rect>,
    /// The selection's size in screen pixels, which is what 6b's readout
    /// shows. Points would be the wrong number: the user is framing pixels.
    pub size: Option<(u32, u32)>,
    /// Whether a lock-on has succeeded for the current rectangle.
    pub found: bool,
}

/// Everything the overlay holds, behind the `Arc<Mutex<_>>` that
/// `show_viewport_deferred` requires: the callback it stores is
/// `Fn + Send + Sync + 'static`, so it cannot borrow.
///
/// Nothing here is touched off the UI thread. The mutex is what the signature
/// demands, not a claim about concurrency.
#[derive(Debug)]
struct Inner {
    origin: (i32, i32),
    points_per_pixel: f32,
    drag: Option<Drag>,
    found: bool,
    throttle: DecodeThrottle,
    /// `None` while the overlay is still up. Set once, by the frame that ends
    /// it.
    outcome: Option<Outcome>,
    /// Whether `raise_window` has been asked for yet. Once, on the frame the
    /// OS window first exists.
    raised: bool,
    open: bool,
}

/// The 6b overlay. Cheap to clone -- every clone is the same window.
#[derive(Debug, Clone)]
pub struct RegionOverlay {
    inner: Arc<Mutex<Inner>>,
}

fn locked(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl RegionOverlay {
    /// Opens over the given monitors. `points_per_pixel` is the parent
    /// context's, and `monitors` is [`crate::screen_capture::monitor_bounds`]
    /// in production -- an argument so that the placement arithmetic can be
    /// exercised without a desktop.
    ///
    /// `None` when there are no monitors to cover: there is no rectangle to
    /// put a window on, and a zero-sized always-on-top window would be a
    /// surface the user cannot dismiss.
    pub fn open(monitors: &[ScreenRect], points_per_pixel: f32) -> Option<RegionOverlay> {
        let bounds = whole_screen(monitors)?;
        Some(RegionOverlay {
            inner: Arc::new(Mutex::new(Inner {
                origin: (bounds.left, bounds.top),
                points_per_pixel,
                drag: None,
                found: false,
                throttle: DecodeThrottle::new(DECODE_INTERVAL),
                outcome: None,
                raised: false,
                open: true,
            })),
        })
    }

    /// Whether the overlay is still up.
    pub fn is_open(&self) -> bool {
        locked(&self.inner).open
    }

    /// Takes the answer, once. `None` while the overlay is still up.
    pub fn take_outcome(&self) -> Option<Outcome> {
        locked(&self.inner).outcome.take()
    }

    /// What [`draw`] should paint this frame.
    pub fn view(&self) -> RegionView {
        let held = locked(&self.inner);
        let scale = if held.points_per_pixel.is_finite() && held.points_per_pixel > 0.0 {
            held.points_per_pixel
        } else {
            1.0
        };
        let (selection, size) = match held.drag {
            None => (None, None),
            Some(drag) => {
                let rect = drag.rect();
                let to_points = |x: i32, y: i32| {
                    egui::pos2(
                        (x - held.origin.0) as f32 / scale,
                        (y - held.origin.1) as f32 / scale,
                    )
                };
                (
                    Some(egui::Rect::from_min_max(
                        to_points(rect.left, rect.top),
                        to_points(rect.right, rect.bottom),
                    )),
                    Some((rect.width(), rect.height())),
                )
            }
        };
        RegionView {
            selection,
            size,
            found: held.found,
        }
    }

    /// Ends the overlay with `outcome`, unless one is already recorded.
    fn finish(&self, outcome: Outcome) {
        let mut held = locked(&self.inner);
        if held.outcome.is_none() {
            held.outcome = Some(outcome);
        }
        held.open = false;
    }

    /// One frame of pointer handling, with the outside world as arguments.
    ///
    /// `pointer` is the pointer position in points, `down` whether the primary
    /// button is held, `now` the clock. Returns nothing: everything it decides
    /// lands in [`Inner`], which is what [`RegionOverlay::view`] reads and what
    /// [`RegionOverlay::take_outcome`] hands back.
    fn advance(
        &self,
        seams: &RegionSeams,
        pointer: Option<(f32, f32)>,
        down: bool,
        now: Instant,
    ) {
        // The rectangle to lock on to, decided under the lock and then
        // released, because a decode must not be run while holding it.
        let candidate = {
            let mut held = locked(&self.inner);
            let Some(at) = pointer else {
                return;
            };
            let cursor = to_screen(held.origin, held.points_per_pixel, at);
            match (held.drag, down) {
                // A drag begins.
                (None, true) => {
                    held.drag = Some(Drag {
                        anchor: cursor,
                        cursor,
                    });
                    held.found = false;
                    None
                }
                // A drag continues: the rectangle changed, so the badge's
                // answer is stale until the next attempt says otherwise.
                (Some(drag), true) => {
                    let drag = Drag {
                        anchor: drag.anchor,
                        cursor,
                    };
                    held.drag = Some(drag);
                    let rect = drag.rect();
                    if held.throttle.should_attempt(rect, now) {
                        Some(rect)
                    } else {
                        None
                    }
                }
                // Released: 6b's "reads it the moment you let go".
                (Some(drag), false) => {
                    let rect = drag.rect();
                    drop(held);
                    self.finish(read_region_with(seams, rect));
                    return;
                }
                (None, false) => None,
            }
        };
        if let Some(rect) = candidate {
            // **The decoded string is dropped here rather than kept.** Keeping
            // it would save one decode on release and would hold the seed in
            // memory for the whole rest of the drag; the badge only needs the
            // boolean, so the boolean is all that is kept.
            let found = read_region_with(seams, rect).is_decoded();
            locked(&self.inner).found = found;
        }
    }

    /// **The viewport.** Called once per frame from the window that opened the
    /// overlay; answers `false` when there is nothing left to show.
    pub fn show(&self, ctx: &egui::Context) -> bool {
        if !self.is_open() {
            return false;
        }
        let (origin, scale) = {
            let held = locked(&self.inner);
            (held.origin, held.points_per_pixel)
        };
        let bounds = screen_capture::monitor_bounds();
        let size = whole_screen(&bounds).unwrap_or(ScreenRect {
            left: origin.0,
            top: origin.1,
            right: origin.0,
            bottom: origin.1,
        });
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };

        // Every frame: a lock-on that only ran on egui input would stop the
        // moment the pointer paused, and the badge would then be stale for as
        // long as the user held still.
        ctx.request_repaint_of(region_viewport());

        let mine = self.clone();
        ctx.show_viewport_deferred(
            region_viewport(),
            // `with_title(REGION_TITLE)` is load-bearing: it is what
            // `raise_window` and `own_window_titled` match on, and its
            // uniqueness is why this window may be alive alongside the vault
            // window. See
            // `foreground::only_one_window_of_this_process_can_exist_at_a_time`.
            egui::ViewportBuilder::default()
                .with_title(REGION_TITLE)
                .with_position(egui::pos2(origin.0 as f32 / scale, origin.1 as f32 / scale))
                .with_inner_size([
                    size.width() as f32 / scale,
                    size.height() as f32 / scale,
                ])
                .with_decorations(false)
                .with_always_on_top()
                .with_taskbar(false)
                .with_transparent(true),
            move |root, _class| {
                let first_frame = {
                    let mut held = locked(&mine.inner);
                    let first = !held.raised;
                    held.raised = true;
                    first
                };
                if first_frame {
                    // The OS window exists by here -- the same hook every
                    // window in this crate raises from. A selection surface
                    // that opens behind the window being selected from is
                    // useless, so this one raises; see its row in
                    // `foreground::OPENS_A_VIEWPORT_AND_RAISES_IT`.
                    crate::foreground::raise_window(REGION_TITLE);
                    exclude_from_capture(REGION_TITLE);
                }

                let (pointer, down) = root.input(|i| {
                    (
                        i.pointer.latest_pos().map(|p| (p.x, p.y)),
                        i.pointer.primary_down(),
                    )
                });
                if root.input(|i| i.key_pressed(egui::Key::Escape))
                    || root.input(|i| i.viewport().close_requested())
                {
                    mine.finish(Outcome::Cancelled);
                    return;
                }
                if root.input(|i| i.key_pressed(egui::Key::A)) {
                    let rect = whole_screen(&screen_capture::monitor_bounds());
                    match rect {
                        Some(rect) => {
                            mine.finish(read_region_with(&RegionSeams::production(), rect))
                        }
                        None => mine.finish(Outcome::Refused(CaptureRefusal::OffScreen)),
                    }
                    return;
                }
                mine.advance(&RegionSeams::production(), pointer, down, Instant::now());

                let view = mine.view();
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| draw(ui, &view));
            },
        );
        self.is_open()
    }
}

/// **Paints 6b.** Pure in the sense that matters here: everything it decides
/// comes out of `view`, so what it draws for a given state is the same every
/// time. It is not unit-tested beyond that -- painting is checked by looking
/// at it, and `RegionView` is what the tests below pin instead.
pub fn draw(ui: &mut egui::Ui, view: &RegionView) {
    let full = ui.max_rect();
    let painter = ui.painter();
    let dim = egui::Color32::from_black_alpha(DIM_ALPHA);

    match view.selection {
        // Nothing selected yet: the whole desktop dims.
        None => {
            painter.rect_filled(full, 0.0, dim);
        }
        // The selection stays lit -- left entirely unpainted, so the
        // transparent viewport shows the desktop through it -- and the four
        // bands around it dim.
        Some(sel) => {
            let sel = sel.intersect(full);
            for band in [
                egui::Rect::from_min_max(full.left_top(), egui::pos2(full.right(), sel.top())),
                egui::Rect::from_min_max(egui::pos2(full.left(), sel.bottom()), full.right_bottom()),
                egui::Rect::from_min_max(egui::pos2(full.left(), sel.top()), sel.left_bottom()),
                egui::Rect::from_min_max(sel.right_top(), egui::pos2(full.right(), sel.bottom())),
            ] {
                if band.is_positive() {
                    painter.rect_filled(band, 0.0, dim);
                }
            }
            painter.rect_stroke(
                sel,
                0.0,
                egui::Stroke::new(1.0, theme::CARD),
                egui::StrokeKind::Outside,
            );
        }
    }

    let centre = view
        .selection
        .map(|sel| sel.center())
        .unwrap_or_else(|| full.center());

    // The badge, or the instruction. See `lockon_badge` for why there is no
    // third "searching" state.
    match lockon_badge(view.found) {
        Some(badge) => {
            painter.text(
                centre,
                egui::Align2::CENTER_CENTER,
                badge,
                egui::FontId::proportional(12.0),
                theme::CARD,
            );
        }
        None => {
            painter.text(
                egui::pos2(full.center().x, full.top() + 64.0),
                egui::Align2::CENTER_CENTER,
                DRAG_TITLE,
                egui::FontId::proportional(18.0),
                theme::CARD,
            );
            painter.text(
                egui::pos2(full.center().x, full.top() + 90.0),
                egui::Align2::CENTER_CENTER,
                DRAG_HINT,
                egui::FontId::proportional(12.0),
                theme::TOGGLE_OFF,
            );
        }
    }

    if let Some((w, h)) = view.size {
        painter.text(
            egui::pos2(centre.x, centre.y + 20.0),
            egui::Align2::CENTER_CENTER,
            size_label(&ScreenRect {
                left: 0,
                top: 0,
                right: w as i32,
                bottom: h as i32,
            }),
            egui::FontId::monospace(11.0),
            theme::TOGGLE_OFF,
        );
    }

    painter.text(
        full.right_bottom() + egui::vec2(-24.0, -24.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{WHOLE_SCREEN_HINT}   A\n{CANCEL_HINT}   ESC"),
        egui::FontId::proportional(11.0),
        theme::TOGGLE_OFF,
    );
}

/// **Asks Windows to leave this window out of screen captures.**
///
/// The problem it solves is specific and is not optional: the overlay is an
/// always-on-top window covering the desktop, so a `BitBlt` of the screen
/// while it is up captures **the overlay's own dimming**, not what is under
/// it. Every pixel would come back darkened and the QR would not decode.
///
/// `WDA_EXCLUDEFROMCAPTURE` is the same flag an app sets to protect its own
/// window -- the one [`crate::screen_capture::looks_blocked`] diagnoses the
/// effect of -- pointed at ourselves. It makes the compositor render this
/// window to the screen but not into any capture, so the blit sees the desktop
/// beneath.
///
/// **This is a real-desktop fact and is not tested.** There is no assertion in
/// this crate that the flag was accepted, and none that the resulting blit is
/// undimmed; a failure here shows up as "no code in that region" for a code
/// that is plainly on screen. A silent failure is also the *safe* direction:
/// nothing is captured that the user did not drag over either way.
///
/// The alternative -- capturing the whole desktop once when the overlay opens
/// and cropping from that frozen frame -- was rejected. It would make the
/// lock-on decode cheaper, and it would mean this feature takes a full-screen
/// capture the user never asked for, which is exactly the third security
/// property of the design ("capture only the rectangle the user dragged").
fn exclude_from_capture(title: &str) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };

    let Some(hwnd) = crate::foreground::own_window_titled(title) else {
        return;
    };
    // Failure is ignored on purpose: see this function's note. There is
    // nothing useful to do about it and nothing secret is at risk.
    unsafe {
        let _ = SetWindowDisplayAffinity(HWND(hwnd as *mut _), WDA_EXCLUDEFROMCAPTURE);
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    /// A capture seam that hands back a buffer of one colour, so the decode
    /// seam has something correctly shaped to be given.
    fn flat_capture(r: ScreenRect) -> Result<Rgba, CaptureRefusal> {
        let (w, h) = (r.width(), r.height());
        let pixels = zeroize::Zeroizing::new(vec![0xffu8; (w as usize) * (h as usize) * 4]);
        Rgba::from_parts(w, h, pixels).ok_or(CaptureRefusal::GdiFailed)
    }

    fn refusing_capture(_: ScreenRect) -> Result<Rgba, CaptureRefusal> {
        Err(CaptureRefusal::Blocked)
    }

    fn no_code(_: &[u8], _: usize, _: usize) -> Option<Zeroizing<String>> {
        None
    }

    fn a_code(_: &[u8], _: usize, _: usize) -> Option<Zeroizing<String>> {
        Some(Zeroizing::new(
            "otpauth://totp/Git%20Host:anovak?secret=JBSWY3DPEHPK3PXP".to_string(),
        ))
    }

    fn seams(
        capture: fn(ScreenRect) -> Result<Rgba, CaptureRefusal>,
        decode: fn(&[u8], usize, usize) -> Option<Zeroizing<String>>,
    ) -> RegionSeams {
        RegionSeams { capture, decode }
    }

    // -- the geometry ------------------------------------------------------

    /// **The drag is `screen_capture`'s arithmetic and not a second copy of
    /// it.** Both halves: a right-to-left, bottom-to-top drag -- the common
    /// one for a right-handed user framing something -- produces the same
    /// rectangle as the left-to-right drag between the same two points, and
    /// that rectangle is exactly what `rect_from_drag` returns.
    #[test]
    fn a_drag_in_any_direction_is_the_same_rectangle_screen_capture_would_build() {
        let forward = Drag {
            anchor: (100, 200),
            cursor: (350, 450),
        };
        let backward = Drag {
            anchor: (350, 450),
            cursor: (100, 200),
        };
        assert_eq!(forward.rect(), rect(100, 200, 350, 450));
        assert_eq!(backward.rect(), forward.rect());
        // The delegation itself, so that a copy of the arithmetic pasted into
        // `Drag::rect` that happened to agree on these points still fails when
        // it stops agreeing on others.
        for anchor in [(0, 0), (-1900, -30), (7, 4000)] {
            for cursor in [(5, -5), (2000, 1200), (-3, 9)] {
                assert_eq!(
                    Drag { anchor, cursor }.rect(),
                    screen_capture::rect_from_drag(anchor, cursor),
                    "Drag::rect disagreed with rect_from_drag for {anchor:?}->{cursor:?}"
                );
            }
        }
    }

    /// **"Whole screen" spans every monitor, including ones left of and above
    /// the primary** -- where the coordinates are negative and a `max`-only
    /// bounding box silently drops them.
    #[test]
    fn whole_screen_covers_monitors_placed_before_the_origin() {
        let monitors = [rect(0, 0, 1920, 1080), rect(-1280, -200, 0, 520)];
        assert_eq!(whole_screen(&monitors), Some(rect(-1280, -200, 1920, 1080)));
        // Positive control on the negative half: with only the primary, the
        // answer really is just the primary, so the assertion above is about
        // the second monitor and not about a hardcoded box.
        assert_eq!(whole_screen(&monitors[..1]), Some(rect(0, 0, 1920, 1080)));
    }

    /// No monitors -- and an empty degenerate one -- give no rectangle rather
    /// than a zero-sized always-on-top window the user cannot dismiss.
    #[test]
    fn no_monitors_is_no_overlay() {
        assert_eq!(whole_screen(&[]), None);
        assert_eq!(whole_screen(&[rect(10, 10, 10, 10)]), None);
        assert!(RegionOverlay::open(&[], 1.0).is_none());
        // Positive control: with a monitor, one really does open, so the
        // `is_none` above is about the empty list.
        assert!(RegionOverlay::open(&[rect(0, 0, 800, 600)], 1.0).is_some());
    }

    /// **Points to virtual-screen pixels, on a monitor whose origin is
    /// negative and at a scale factor that is not 1.**
    #[test]
    fn a_pointer_position_becomes_a_screen_pixel() {
        // Origin at the top-left of a monitor placed left of the primary.
        assert_eq!(to_screen((-1280, -200), 1.0, (10.0, 20.0)), (-1270, -180));
        // At 150%, a point is 1.5 pixels.
        assert_eq!(to_screen((0, 0), 1.5, (100.0, 200.0)), (150, 300));
        // Rounds rather than truncating: at 1.5, 33 points is 49.5 pixels and
        // the answer is 50. A truncating cast would say 49, which biases every
        // coordinate toward the origin and eats a tight crop's quiet zone.
        assert_eq!(to_screen((0, 0), 1.5, (33.0, 33.0)), (50, 50));
        // A nonsense scale factor is treated as 1 rather than producing NaN
        // coordinates.
        assert_eq!(to_screen((5, 5), 0.0, (10.0, 10.0)), (15, 15));
        assert_eq!(to_screen((5, 5), f32::NAN, (10.0, 10.0)), (15, 15));
    }

    // -- the label ---------------------------------------------------------

    /// **The found/not-found decision, both ways**, and the found string is
    /// 6b's verbatim.
    #[test]
    fn the_badge_appears_only_once_a_code_is_found() {
        assert_eq!(lockon_badge(true), Some("Code found \u{b7} release to read"));
        assert_eq!(lockon_badge(false), None);
        // The constant really is the design's sentence, separator included --
        // so a `LOCKED_ON` edited to something else fails here rather than
        // shipping.
        assert_eq!(LOCKED_ON, "Code found · release to read");
        assert_eq!(DRAG_TITLE, "Drag a box around the QR code");
        assert_eq!(
            DRAG_HINT,
            "Deskwarden reads it the moment you let go. Nothing is saved yet."
        );
    }

    /// 6b's readout is the size in **pixels**, with the design's `×`.
    #[test]
    fn the_size_readout_is_the_pixels_the_user_framed() {
        assert_eq!(size_label(&rect(100, 100, 350, 350)), "250 × 250");
        assert_eq!(size_label(&rect(-10, 0, 10, 5)), "20 × 5");
        // An inverted rectangle reads zero rather than a negative or a wrapped
        // huge number.
        assert_eq!(size_label(&rect(10, 10, 0, 0)), "0 × 0");
    }

    // -- the throttle ------------------------------------------------------

    /// **The attempt rate is bounded by the interval.** Driven with a
    /// constructed clock rather than by sleeping, which is what makes the
    /// boundary itself assertable.
    #[test]
    fn decodes_are_not_attempted_faster_than_the_interval() {
        let start = Instant::now();
        let mut throttle = DecodeThrottle::new(Duration::from_millis(100));
        // First attempt on a fresh throttle: immediate.
        assert!(throttle.should_attempt(rect(0, 0, 100, 100), start));
        // A changed rectangle, but too soon.
        assert!(!throttle.should_attempt(rect(0, 0, 101, 100), start + Duration::from_millis(99)));
        // Exactly the interval is enough -- the bound is "at least", not
        // "more than".
        assert!(throttle.should_attempt(rect(0, 0, 101, 100), start + Duration::from_millis(100)));
    }

    /// **The second gate, which is the one that stops the unbounded loop.**
    /// A pointer held still repaints, and a time-only throttle would re-decode
    /// the identical rectangle forever for an answer that cannot change.
    #[test]
    fn an_unchanged_rectangle_is_never_re_attempted_however_long_you_wait() {
        let start = Instant::now();
        let mut throttle = DecodeThrottle::new(Duration::from_millis(10));
        let same = rect(0, 0, 400, 400);
        assert!(throttle.should_attempt(same, start));
        for after in [1_u64, 50, 5_000, 3_600_000] {
            assert!(
                !throttle.should_attempt(same, start + Duration::from_millis(after)),
                "the same rectangle was attempted again after {after} ms"
            );
        }
        // Positive control: the throttle is not simply exhausted -- a
        // different rectangle at the same late time is attempted.
        assert!(throttle.should_attempt(
            rect(0, 0, 400, 401),
            start + Duration::from_millis(3_600_000)
        ));
    }

    /// A click that never became a drag costs no decode, at any spacing.
    #[test]
    fn a_rectangle_too_small_to_hold_anything_is_never_attempted() {
        let start = Instant::now();
        let mut throttle = DecodeThrottle::new(Duration::ZERO);
        assert!(!throttle.should_attempt(rect(7, 7, 7, 7), start));
        assert!(!throttle.should_attempt(rect(7, 7, 8, 900), start));
        assert!(!throttle.should_attempt(rect(7, 7, 900, 8), start));
        // Positive control on `MIN_SIDE`: one pixel larger on both sides, and
        // with a zero interval, it is attempted -- so the refusals above are
        // about the size and not about the throttle refusing everything.
        assert!(throttle.should_attempt(rect(7, 7, 9, 9), start));
    }

    /// Production's spacing is the constant, and the constant is a bound a
    /// human would not perceive as a delay. A `DECODE_INTERVAL` raised to
    /// seconds -- or dropped to zero, which is the unbounded loop -- fails
    /// here.
    #[test]
    fn the_production_throttle_is_the_documented_interval() {
        assert_eq!(DECODE_INTERVAL, Duration::from_millis(150));
        assert_eq!(DecodeThrottle::new(DECODE_INTERVAL).interval(), DECODE_INTERVAL);
        assert!(DECODE_INTERVAL > Duration::ZERO);
        assert!(DECODE_INTERVAL <= Duration::from_millis(200));
        let overlay = RegionOverlay::open(&[rect(0, 0, 800, 600)], 1.0).expect("opens");
        assert_eq!(
            locked(&overlay.inner).throttle.interval(),
            DECODE_INTERVAL,
            "the overlay built a throttle with a different spacing from the constant"
        );
    }

    // -- reading a region --------------------------------------------------

    /// **All four outcomes**, each from the state that produces it.
    #[test]
    fn every_outcome_of_reading_a_region_is_reachable() {
        let r = rect(0, 0, 40, 40);
        assert!(matches!(
            read_region_with(&seams(flat_capture, a_code), r),
            Outcome::Decoded(_)
        ));
        assert!(matches!(
            read_region_with(&seams(flat_capture, no_code), r),
            Outcome::NoCode
        ));
        assert!(matches!(
            read_region_with(&seams(refusing_capture, a_code), r),
            Outcome::Refused(CaptureRefusal::Blocked)
        ));
        // A refusal short-circuits: the decoder is never handed a buffer that
        // does not exist. `refusing_capture` with a decoder that panics would
        // prove it, and this is the non-panicking form -- the refusal wins over
        // a decoder that always finds a code.
        assert!(!read_region_with(&seams(refusing_capture, a_code), r).is_decoded());
    }

    /// The decoder is handed the buffer's **own** dimensions, not the
    /// rectangle's. They agree when the rectangle was inside a monitor and
    /// differ when `capture_rect` clamped it, and a decode against the
    /// unclamped numbers reads past the end of the buffer or misreads its
    /// rows.
    #[test]
    fn the_decoder_is_given_the_captured_size_and_not_the_requested_one() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static SEEN_W: AtomicUsize = AtomicUsize::new(0);
        static SEEN_H: AtomicUsize = AtomicUsize::new(0);

        fn clamping_capture(_: ScreenRect) -> Result<Rgba, CaptureRefusal> {
            // Whatever was asked for, 60x30 came back.
            flat_capture(ScreenRect {
                left: 0,
                top: 0,
                right: 60,
                bottom: 30,
            })
        }
        fn recording_decode(
            rgba: &[u8],
            w: usize,
            h: usize,
        ) -> Option<Zeroizing<String>> {
            SEEN_W.store(w, Ordering::SeqCst);
            SEEN_H.store(h, Ordering::SeqCst);
            assert_eq!(rgba.len(), w * h * 4, "the buffer and its dimensions disagree");
            None
        }

        let _ = read_region_with(
            &seams(clamping_capture, recording_decode),
            rect(0, 0, 4000, 4000),
        );
        assert_eq!(SEEN_W.load(Ordering::SeqCst), 60);
        assert_eq!(SEEN_H.load(Ordering::SeqCst), 30);
    }

    /// **The production seams are the real functions**, by address. Without
    /// this every test above is about two stubs.
    #[test]
    fn production_reads_the_real_screen_and_the_real_decoder() {
        let production = RegionSeams::production();
        assert!(std::ptr::fn_addr_eq(
            production.capture,
            screen_capture::capture_rect
                as fn(ScreenRect) -> Result<Rgba, CaptureRefusal>
        ));
        assert!(std::ptr::fn_addr_eq(
            production.decode,
            qr::decode_qr as fn(&[u8], usize, usize) -> Option<Zeroizing<String>>
        ));
        // Negative control on `fn_addr_eq` itself: it can tell two functions
        // apart, so the two assertions above are claims and not tautologies.
        assert!(!std::ptr::fn_addr_eq(
            production.decode,
            no_code as fn(&[u8], usize, usize) -> Option<Zeroizing<String>>
        ));
    }

    /// **The decoded secret never reaches a formatter.** `Outcome`'s `Debug`
    /// is hand-written for exactly this; a derived one would print the seed,
    /// and `debug_leak_guard` is what stops the derive from coming back.
    #[test]
    fn the_debug_of_a_decoded_outcome_does_not_print_the_secret() {
        let secret = "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP";
        let shown = format!("{:?}", Outcome::Decoded(Zeroizing::new(secret.to_string())));
        assert!(!shown.contains("JBSWY3DPEHPK3PXP"), "{shown}");
        assert!(!shown.contains("otpauth"), "{shown}");
        assert!(shown.contains("not shown"), "{shown}");
        // The other three carry nothing secret and say what they are.
        assert_eq!(format!("{:?}", Outcome::Cancelled), "Cancelled");
        assert_eq!(format!("{:?}", Outcome::NoCode), "NoCode");
        assert_eq!(
            format!("{:?}", Outcome::Refused(CaptureRefusal::Blocked)),
            "Refused(Blocked)"
        );
    }

    // -- the frame loop, without a window ----------------------------------

    /// **A whole drag, driven by hand**: press, move, release -- and the badge
    /// locks on before the release, which is 6b's whole point.
    #[test]
    fn the_badge_locks_on_before_the_button_comes_up() {
        let overlay = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        let seams = seams(flat_capture, a_code);
        let t0 = Instant::now();

        overlay.advance(&seams, Some((100.0, 100.0)), true, t0);
        assert!(!overlay.view().found, "nothing has been decoded yet");
        assert!(overlay.is_open());

        overlay.advance(&seams, Some((400.0, 400.0)), true, t0 + DECODE_INTERVAL);
        let view = overlay.view();
        assert!(view.found, "the drag decoded and the badge did not lock on");
        assert_eq!(view.size, Some((300, 300)));
        assert_eq!(lockon_badge(view.found), Some(LOCKED_ON));
        // Still down: the overlay has not answered yet.
        assert!(overlay.is_open());
        assert!(overlay.take_outcome().is_none());

        overlay.advance(&seams, Some((400.0, 400.0)), false, t0 + DECODE_INTERVAL * 2);
        assert!(!overlay.is_open());
        assert!(matches!(overlay.take_outcome(), Some(Outcome::Decoded(_))));
        // Taken once. A second caller gets nothing rather than a second copy
        // of the seed.
        assert!(overlay.take_outcome().is_none());
    }

    /// The same drag over a region with no code: the badge never appears, and
    /// the release says so by name rather than blankly.
    #[test]
    fn a_region_with_no_code_never_locks_on_and_says_so_on_release() {
        let overlay = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        let seams = seams(flat_capture, no_code);
        let t0 = Instant::now();
        overlay.advance(&seams, Some((10.0, 10.0)), true, t0);
        overlay.advance(&seams, Some((300.0, 300.0)), true, t0 + DECODE_INTERVAL);
        assert!(!overlay.view().found);
        assert_eq!(lockon_badge(overlay.view().found), None);
        overlay.advance(&seams, Some((300.0, 300.0)), false, t0 + DECODE_INTERVAL * 2);
        assert!(matches!(overlay.take_outcome(), Some(Outcome::NoCode)));
    }

    /// **Escape captures nothing.** Not a buffer, not a partial one: the
    /// cancelled outcome is reached without the capture seam being called at
    /// all, which a seam that panics is what proves.
    #[test]
    fn cancelling_captures_nothing() {
        fn never_called(_: ScreenRect) -> Result<Rgba, CaptureRefusal> {
            panic!("a cancelled overlay captured pixels");
        }
        let overlay = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        // Mid-drag, so there is a rectangle to have captured if anything were
        // going to.
        overlay.advance(
            &seams(never_called, no_code),
            Some((10.0, 10.0)),
            true,
            Instant::now(),
        );
        overlay.finish(Outcome::Cancelled);
        assert!(!overlay.is_open());
        assert!(matches!(overlay.take_outcome(), Some(Outcome::Cancelled)));
        // Positive control on `never_called`: it really would have fired had a
        // release happened, so the test above is about the cancel path.
        let other = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        let t0 = Instant::now();
        other.advance(&seams(never_called, no_code), Some((10.0, 10.0)), true, t0);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            other.advance(
                &seams(never_called, no_code),
                Some((300.0, 300.0)),
                false,
                t0 + DECODE_INTERVAL,
            );
        }))
        .is_err());
    }

    /// **A new drag clears a stale lock-on.** Pressing again after finding a
    /// code somewhere else must not leave "Code found" on screen over a
    /// rectangle that has not been read.
    #[test]
    fn starting_a_second_drag_drops_the_previous_answer() {
        let overlay = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        let found = seams(flat_capture, a_code);
        let t0 = Instant::now();
        overlay.advance(&found, Some((10.0, 10.0)), true, t0);
        overlay.advance(&found, Some((300.0, 300.0)), true, t0 + DECODE_INTERVAL);
        assert!(overlay.view().found, "control: it locked on");
        // Back to idle without a release -- the pointer left the window --
        // then a fresh press.
        locked(&overlay.inner).drag = None;
        overlay.advance(&found, Some((600.0, 600.0)), true, t0 + DECODE_INTERVAL * 2);
        assert!(!overlay.view().found, "the stale lock-on survived a new drag");
        assert_eq!(overlay.view().size, Some((0, 0)));
    }

    /// The selection handed to the painter is in **points relative to the
    /// viewport**, with the monitor origin and the scale factor both taken
    /// out -- while the readout stays in pixels, which is what the user is
    /// framing.
    #[test]
    fn the_view_converts_back_to_points_but_reports_pixels() {
        let overlay = RegionOverlay::open(&[rect(-1280, -200, 1920, 1080)], 2.0).expect("opens");
        let t0 = Instant::now();
        let seams = seams(flat_capture, no_code);
        overlay.advance(&seams, Some((10.0, 20.0)), true, t0);
        overlay.advance(&seams, Some((60.0, 70.0)), true, t0 + DECODE_INTERVAL);
        let view = overlay.view();
        // Origin is (-1280, -200); at 2.0 the drag ran from screen pixel
        // (-1260, -160) to (-1160, -60), which is 100x100 pixels and 50x50
        // points back in the viewport's own space.
        assert_eq!(view.size, Some((100, 100)));
        let sel = view.selection.expect("a drag is in progress");
        assert_eq!(sel.min, egui::pos2(10.0, 20.0));
        assert_eq!(sel.max, egui::pos2(60.0, 70.0));
    }

    /// An overlay with no pointer over it -- the cursor on another monitor
    /// the window does not cover -- changes nothing and answers nothing.
    #[test]
    fn no_pointer_is_not_a_drag() {
        let overlay = RegionOverlay::open(&[rect(0, 0, 800, 600)], 1.0).expect("opens");
        overlay.advance(&seams(flat_capture, a_code), None, true, Instant::now());
        assert_eq!(overlay.view().selection, None);
        assert!(overlay.is_open());
        assert!(overlay.take_outcome().is_none());
    }

    /// The title is this window's identity and is distinct from the two other
    /// titles this crate names. `foreground` asserts the same thing against
    /// the constants it can reach; this is the local half, so a rename here
    /// fails next to the constant it renamed.
    #[test]
    fn the_title_is_this_window_alone() {
        assert_ne!(REGION_TITLE, crate::vault_window::WINDOW_TITLE);
        assert_ne!(REGION_TITLE, crate::vault_window::rehearsal::SCRATCH_TITLE);
        assert_ne!(REGION_TITLE, crate::preflight_card::PREFLIGHT_CARD_TITLE);
        assert!(!REGION_TITLE.is_empty());
    }
}

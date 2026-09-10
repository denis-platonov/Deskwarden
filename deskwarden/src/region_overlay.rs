//! **6b -- the dimmed full-screen surface the user drags a box on, and the
//! whole-screen scan that usually means the user never sees it.**
//!
//! The desktop dims, the selection stays lit, and the overlay **locks on
//! before the user releases**: *"Code found · release to read"*. That last
//! part is the whole character of this screen. A capture tool that only tells
//! you whether it worked after you let go teaches the user to drag, release,
//! read a refusal, and drag again; one that says "found" while the button is
//! still down turns the same drag into a single, confident gesture.
//!
//! # The scan comes first, and the drag is the fallback
//!
//! **This is a deliberate departure from the design**, asked for by the owner
//! in those words: *"would be nice if it could recognize the QR itself without
//! drawing a box - is it possible? Should be I think"*. Design 6a's row leads
//! straight to 6b, and 6b is a rectangle the user has to draw. But the reason
//! it asked for a rectangle was never that the decoder needs one --
//! [`crate::qr`] finds a code anywhere in a picture, and always could -- it
//! was that the capture is the user's screen and the design would not take one
//! they had not framed. Choosing the route **is** asking for it.
//!
//! So [`RegionOverlay::show`]'s first act is [`scan_screen_with`] over every
//! monitor, and 6b opens only when that cannot answer:
//!
//! * **exactly one code** -- the overlay never appears at all. The outcome is
//!   [`Outcome::Decoded`] before a window exists, and the caller lands on the
//!   same 6c confirmation card every other route lands on. **Nothing is
//!   saved**: 6c holds the code and its countdown, and Save is a press the
//!   user makes.
//! * **none** -- 6b opens, and its bottom bar says why, because a full-screen
//!   dim that arrives with no account of itself is a surface the user has to
//!   reverse-engineer.
//! * **more than one distinct code** -- 6b opens and says *that*. Picking one
//!   of two seeds on the user's behalf is the one thing this feature must not
//!   do, and there is no honest tie-break available: the codes are on a screen
//!   this app did not lay out.
//!
//! Everything below the scan is unchanged -- the drag, the lock-on, Escape,
//! the release that reads. The scan is a way of not needing them, not a
//! replacement for them.
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
//! * [`read_region_with`] -- every outcome, through seams;
//! * [`scan_screen_with`] -- one code, none, several, a monitor that refused
//!   and a monitor too large to decode, all through the same seams;
//! * [`RegionOverlay::prescan_step`] -- that the scan runs **once** and then
//!   never again, driven by a clock a test supplies;
//! * [`RegionOverlay::chip_gesture`] -- that a press on a bar chip belongs to
//!   that chip and does not also become a one-pixel drag.
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
//! * that the overlay itself is excluded from the blit, and that the vault
//!   window is out of the scan's way in time. See [`exclude_from_capture`]
//!   and [`PRESCAN_SETTLE`].
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
//!
//! **The whole-screen scan is a bigger version of the same object and is held
//! to the same rules, with one added.** Each monitor's pixels live in an
//! [`crate::screen_capture::Rgba`] that wipes on drop and dies inside
//! [`scan_screen_with`] before the next monitor is read; nothing is written,
//! logged, or handed out. The added rule is that **the capture is never
//! displayed**: it is not uploaded to an `egui` texture and not painted as
//! this overlay's background, however much a frozen desktop would improve the
//! dim. `egui`'s texture manager holds ordinary allocations this crate cannot
//! wipe, so painting the desktop would put the user's whole screen -- every
//! window and every secret on it -- into memory with no owner and no end. The
//! scan is capture, decode, drop, and the overlay stays transparent over the
//! live desktop exactly as it always has.

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

/// 6b's instruction. **On screen the whole time**, not only while the pointer
/// is idle: the design puts it in a bar pinned to the bottom of the surface
/// that no state removes, because the sentence beside it is the one that says
/// nothing has been saved, and a user mid-drag is exactly the user who wants
/// to read that.
pub const DRAG_TITLE: &str = "Drag a box around the QR code";

/// 6b's sub-line. **"Nothing is saved yet" has to stay true of the code**:
/// this module writes nothing anywhere, and the confirmation screen is what
/// saves.
pub const DRAG_HINT: &str = "Deskwarden reads it the moment you let go. Nothing is saved yet.";

/// 6b's lock-on badge, shown **while the button is still down**.
pub const LOCKED_ON: &str = "Code found \u{b7} release to read";

/// 6b's two shortcut affordances, at the right-hand end of the bottom bar.
/// Each is a bordered chip carrying its label and, in the design's monospace,
/// the key that does the same thing.
pub const WHOLE_SCREEN_HINT: &str = "Whole screen";
/// See [`WHOLE_SCREEN_HINT`]. The key printed inside that chip, and the key
/// the callback really matches on -- [`egui::Key::A`].
pub const WHOLE_SCREEN_KEY: &str = "A";
/// See [`WHOLE_SCREEN_HINT`].
pub const CANCEL_HINT: &str = "Cancel";
/// See [`WHOLE_SCREEN_HINT`]. Matched as [`egui::Key::Escape`].
pub const CANCEL_KEY: &str = "ESC";

/// The chips' positions in the bar, left to right, and therefore the indices
/// [`RegionOverlay::chip_gesture`] answers with.
///
/// Named rather than `0` and `1` at the call sites because the pair is drawn
/// from one array and read back by index: a chip inserted in front of these
/// two would silently make Cancel mean "scan the whole screen".
/// `the_chips_are_drawn_in_the_order_their_indices_name` pins the array
/// against them.
pub const CHIP_WHOLE_SCREEN: usize = 0;
/// See [`CHIP_WHOLE_SCREEN`].
pub const CHIP_CANCEL: usize = 1;

/// **The line 6b's bar gains, saying why this surface opened.**
///
/// Design 6b has no such line and did not need one: in the design the user
/// chose "drag a box" and got a box to drag. They now choose a route that
/// scans, so every 6b the product shows is a 6b that opened **after something
/// did not work out** -- and a full-screen dim that arrives with no account
/// of itself is a surface the user has to reverse-engineer. The rest of the
/// bar is the design's, unchanged: its instruction, its "nothing is saved
/// yet", its two chips.
/// **Every one of them ends by naming the drag**, because in every one of
/// them the drag is what the user does next and it is the thing 6b was always
/// for. They do **not** share a clause, though it would be tidier: "drag a
/// box around the one you want" is right when there are two codes to choose
/// between and wrong when there were none to find, and a single sentence
/// stretched to cover both would be the generic refusal
/// `totp_add::PickerRefusal` already refuses to write.
pub const SCAN_NO_CODE: &str = "No QR code found on your screen. Drag a box around it instead.";
/// See [`SCAN_NO_CODE`]. The case the owner was explicit about: **do not
/// guess.** Two codes on a desktop are two secrets, and there is no tie-break
/// this app is entitled to invent -- so this is the one sentence here that
/// asks the user to choose, because they are the only one who can.
pub const SCAN_SEVERAL: &str =
    "More than one QR code is on your screen. Drag a box around the one you want.";
/// See [`SCAN_NO_CODE`]. A refusal's second clause. It says "the code"
/// because a screen that could not be read tells us nothing about how many
/// codes are on it.
pub const SCAN_REFUSED_ADVICE: &str = "Drag a box around the code instead.";

/// The bar's reason line, from what the scan came back with.
///
/// A refusal keeps [`CaptureRefusal::title`]'s own words rather than being
/// flattened into a third sentence here, for the reason
/// `totp_add::PickerRefusal` gives for the same choice: the thing that knows
/// why Windows would not hand over a screen is the thing that asked it for
/// one.
pub fn scan_miss_line(miss: ScanMiss) -> String {
    match miss {
        ScanMiss::NoCode => SCAN_NO_CODE.to_string(),
        ScanMiss::Several => SCAN_SEVERAL.to_string(),
        ScanMiss::Refused(why) => format!("{}. {SCAN_REFUSED_ADVICE}", why.title()),
    }
}

// ---------------------------------------------------------------------------
// The numbers, all of them lifted out of design 6b's CSS
// ---------------------------------------------------------------------------
//
// 6b draws its mock desktop as `#201e1d` with the desktop content over it at
// `opacity: 0.32`, and everything else on the surface is positioned in the
// same CSS pixels. Those pixels are points here: the overlay is a full-screen
// viewport at the desktop's own scale factor, so a 26-point badge is the
// design's 26px badge on a 100% monitor and grows with the user's scaling the
// way every other surface in this app does.

/// How dark the desktop goes outside the selection.
///
/// **Ink at 68%, not black at 55%.** 6b composites the desktop at
/// `opacity: 0.32` over `#201e1d`, which over a real desktop is the same
/// arithmetic as painting that ink at `1 - 0.32` -- `0.68 * 255`, rounded.
/// Black was what this drew before and it read as a colder, flatter dim than
/// the design's, because the design's wash is the app's ink and carries its
/// warmth. The selection itself is left entirely unpainted, which is what
/// "stays lit" means on a transparent viewport.
pub const DIM_ALPHA: u8 = 173;

/// The solid ring around the selection: `box-shadow: 0 0 0 2px #1b3fa0`.
pub const SELECTION_RING: f32 = 2.0;
/// The soft ring outside that one: the second shadow, `0 0 0 8px`, is six
/// further points beyond the first two.
pub const SELECTION_HALO: f32 = 6.0;
/// The halo's `rgba(27, 63, 160, 0.28)`, as an alpha over [`theme::BLUE`].
pub const HALO_ALPHA: u8 = 71;

/// The arm length of 6b's four corner brackets (`width`/`height: 14px`).
pub const CORNER_ARM: f32 = 14.0;
/// Their thickness (`border-left: 3px solid #1b3fa0`).
pub const CORNER_THICK: f32 = 3.0;

/// The lock-on badge's height (`height: 26px`).
pub const BADGE_HEIGHT: f32 = 26.0;
/// Its horizontal padding (`padding: 0 10px`).
pub const BADGE_PAD_X: f32 = 10.0;
/// Its corner radius (`border-radius: 6px`).
pub const BADGE_RADIUS: u8 = 6;
/// The gap between the tick and the words (`gap: 8px`).
pub const BADGE_GAP: f32 = 8.0;
/// The tick's box (`<svg width="13" height="13">`) and the stroke width it
/// carries in that svg's own 24-unit viewBox.
pub const BADGE_TICK: f32 = 13.0;
/// See [`BADGE_TICK`].
pub const BADGE_TICK_STROKE: f32 = 2.8;
/// The words in the badge (`font-size: 12px; font-weight: 700`).
pub const BADGE_TEXT_PX: f32 = 12.0;
/// How far above the selection's **top-left** the badge sits: `left: 0; top:
/// -34px`, so its own bottom clears the selection by eight points.
pub const BADGE_OFFSET: f32 = 34.0;

/// The size readout's inset from the selection's bottom-right corner
/// (`right: 6px; bottom: 6px`).
pub const SIZE_INSET: f32 = 6.0;
/// Its padding (`padding: 3px 7px`).
pub const SIZE_PAD_X: f32 = 7.0;
/// See [`SIZE_PAD_X`].
pub const SIZE_PAD_Y: f32 = 3.0;
/// Its corner radius (`border-radius: 5px`).
pub const SIZE_RADIUS: u8 = 5;
/// Its type (`font-family: ui-monospace...; font-size: 11px`).
pub const SIZE_TEXT_PX: f32 = 11.0;
/// Its plate, `rgba(32, 30, 29, 0.86)` -- ink at `0.86 * 255`, rounded.
pub const SIZE_BG_ALPHA: u8 = 219;

/// The bottom bar's padding (`padding: 14px 18px`).
pub const BAR_PAD_X: f32 = 18.0;
/// See [`BAR_PAD_X`].
pub const BAR_PAD_Y: f32 = 14.0;
/// Its ground, `rgba(32, 30, 29, 0.92)` -- ink at `0.92 * 255`, rounded.
/// **Not opaque**, and that matters on this window: the bar sits over the
/// desktop like everything else here, and an opaque one would read as a strip
/// of chrome bolted on rather than as part of the dim.
pub const BAR_BG_ALPHA: u8 = 235;
/// Its `border-top: 1px solid #3a3736`.
pub const BAR_EDGE: egui::Color32 = egui::Color32::from_rgb(0x3a, 0x37, 0x36);
/// The bar's title (`font-size: 13px; font-weight: 700; color: #ffffff`).
pub const BAR_TITLE_PX: f32 = 13.0;
/// The line under it (`font-size: 12px; color: #bab6b6`).
pub const BAR_HINT_PX: f32 = 12.0;
/// See [`BAR_HINT_PX`]. Not one of `theme`'s inks: it is a grey chosen for a
/// dark ground and this is the only dark ground in the product.
pub const BAR_HINT_INK: egui::Color32 = egui::Color32::from_rgb(0xba, 0xb6, 0xb6);
/// The gap between those two lines (`gap: 3px`), and between either of them
/// and [`scan_miss_line`]'s line above.
pub const BAR_LINE_GAP: f32 = 3.0;
/// [`scan_miss_line`]'s type: the same 12 points as the hint under it,
/// because it is a sentence of the same weight. Setting it larger would make
/// the reason for the surface louder than the instruction on it.
pub const BAR_REASON_PX: f32 = 12.0;
/// [`scan_miss_line`]'s ink.
///
/// **[`theme::BLUE_SOFT`] rather than a new colour or the hint's grey.** This
/// surface has exactly one accent -- the blue of the selection ring, the halo
/// and the lock-on badge -- and the reason line is the one piece of type on
/// it that is neither the instruction nor the small print, so it has to be
/// distinguishable from both without introducing a second accent to a screen
/// that has one. `BLUE_SOFT` is that same blue, lightened for a dark ground,
/// which is what this bar is.
pub const BAR_REASON_INK: egui::Color32 = theme::BLUE_SOFT;

/// A shortcut chip's height (`height: 28px`).
pub const CHIP_HEIGHT: f32 = 28.0;
/// Its horizontal padding (`padding: 0 11px`).
pub const CHIP_PAD_X: f32 = 11.0;
/// Its corner radius (`border-radius: 7px`).
pub const CHIP_RADIUS: u8 = 7;
/// The gap inside a chip, and between the two of them (`gap: 8px` in both
/// places).
pub const CHIP_GAP: f32 = 8.0;
/// A chip's label (`font-size: 12px; font-weight: 600; color: #f7f6f5`).
pub const CHIP_TEXT_PX: f32 = 12.0;
/// The key beside it (`font-size: 10px; color: #9b9797`, monospace).
pub const CHIP_KEY_PX: f32 = 10.0;

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

/// The badge above the selection, or `None` for "no badge this frame".
///
/// **`None` and not a second sentence.** 6b shows the badge only once a code
/// is found; while the overlay is still searching it shows nothing extra,
/// because a "searching..." that appears on every drag would be on screen
/// almost all the time and would say nothing the user cannot already see. The
/// absence of the badge *is* the not-found state, and that is the whole
/// decision this function makes -- the instruction it used to displace is now
/// in the bottom bar, where 6b keeps it whatever the drag is doing.
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

/// **How long the whole-screen scan waits after masking Deskwarden's own
/// window before it captures anything.**
///
/// The scan is taken from behind the vault window: the user pressed the row
/// in a modal on it, so it is by construction the window most likely to be
/// sitting on top of the code. [`exclude_from_capture`] is what takes it out
/// of the blit, and the problem this constant exists for is that the flag is
/// a message to the **compositor**, not to `BitBlt`. `BitBlt` from the screen
/// DC reads what DWM last composed, so a capture taken in the same breath as
/// the call can still show the window that has just been excluded -- and what
/// the user would see is a scan that fails on a code plainly on screen, every
/// time, with no way to tell why.
///
/// So the scan is spread over two frames: mask, wait, capture. **80 ms**
/// rather than one frame's 16, because "one frame" is a claim about a
/// refresh rate this app does not know and a compositor it does not drive; at
/// 80 ms even a 30 Hz desktop has composed twice. It is short enough that the
/// card underneath does not visibly stall -- the decode that follows is
/// longer -- and it is a bound rather than a poll: the deadline is set once
/// and the next frame at or after it captures, whatever happened in between.
///
/// **Not a proof.** Nothing in this crate can assert the compositor acted;
/// see the module header's list of what a real desktop is needed for.
pub const PRESCAN_SETTLE: Duration = Duration::from_millis(80);

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
    /// [`crate::qr::codes_in`] in production, and a **third** seam rather
    /// than a widening of `decode`: the drag reads one framed rectangle and
    /// has no use for a count, while the scan reads a desktop nobody framed
    /// and has no use for anything else. One pointer answering both would
    /// make every drag pay for a question it never asks.
    pub scan: fn(&[u8], usize, usize) -> qr::Codes,
}

impl RegionSeams {
    /// The real ones. A test asserts these are the real functions by address,
    /// so a seam quietly re-pointed at a stub fails rather than passing.
    pub fn production() -> Self {
        RegionSeams {
            capture: screen_capture::capture_rect,
            decode: qr::decode_qr,
            scan: qr::codes_in,
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
// Reading the whole screen
// ---------------------------------------------------------------------------

/// Why the scan could not simply answer, and therefore why 6b opened.
///
/// Every variant is a sentence in the bar -- see [`scan_miss_line`] -- and a
/// **fallback**, never a dead end: in all three cases the drag is still there
/// and still works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMiss {
    /// No monitor held a code. Much the commonest: the code is small, or
    /// low-contrast, or half-covered by something.
    NoCode,
    /// More than one **distinct** code was on screen. See
    /// [`crate::qr::codes_in`] for what "distinct" means, and why the same
    /// code appearing twice is not this.
    Several,
    /// Nothing could be captured at all, and this is what the first monitor
    /// to refuse said. A monitor larger than [`crate::qr::MAX_PIXELS`] lands
    /// here as [`CaptureRefusal::TooLarge`].
    Refused(CaptureRefusal),
}

/// What a whole-screen scan came to.
///
/// **Hand-written `Debug`**, for [`Outcome`]'s reason exactly: `Found` holds
/// a seed.
pub enum ScreenScan {
    /// Exactly one distinct code across every monitor.
    Found(Zeroizing<String>),
    /// It could not answer. See [`ScanMiss`].
    Missed(ScanMiss),
}

impl std::fmt::Debug for ScreenScan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScreenScan::Found(text) => write!(f, "Found({} chars not shown)", text.len()),
            ScreenScan::Missed(miss) => write!(f, "Missed({miss:?})"),
        }
    }
}

/// **Looks for a QR code on every monitor, and refuses to choose between
/// two.**
///
/// # One monitor at a time, not one bounding box
///
/// [`whole_screen`] exists and is deliberately not used here.
/// [`crate::screen_capture::capture_rect`] clamps whatever it is given down
/// to *the monitor it overlaps most*, so a bounding box across a two-monitor
/// desktop reads the larger monitor and silently ignores the other -- which
/// on the commonest two-monitor arrangement is exactly the wrong one, because
/// the user pressed the row in the vault window on the monitor they are
/// looking at and the code is on the other. Feeding each monitor's own
/// rectangle in turn is the only way to cover the desktop, and it costs
/// nothing: the clamp is a no-op on a rectangle that is already a monitor.
///
/// It also keeps the memory honest. One monitor's pixels exist at a time and
/// die -- and wipe -- before the next monitor is read, rather than a single
/// buffer the size of the whole virtual desktop.
///
/// # The bound is [`crate::qr::MAX_PIXELS`], and it is checked rather than
/// # assumed
///
/// 64 megapixels. A 4K monitor is 8.3 of them and an 8K one is 33, so a
/// single monitor is comfortably inside it -- but nothing here skips the
/// check on that reasoning. `clamp_to_monitors` applies the bound per
/// monitor, and a monitor somehow past it refuses with
/// [`CaptureRefusal::TooLarge`] **without sinking the scan**: the loop keeps
/// going and another monitor can still hold the code. A refusal is only the
/// answer when *nothing anywhere* was captured.
///
/// # What it does not do
///
/// It does not save anything, and it does not pick. `Found` is a string
/// handed to the caller, which puts it on 6c's confirmation card with its
/// live code and its countdown; the write happens when the user presses Save
/// and at no other time.
pub fn scan_screen_with(seams: &RegionSeams, monitors: &[ScreenRect]) -> ScreenScan {
    // The same accumulator `codes_in` folds one picture's grids with, so "the
    // same code on two monitors is one code" and "the same code twice in one
    // window is one code" are one rule rather than two that drift apart.
    let mut tally = qr::Tally::new();
    let mut captured_any = false;
    let mut first_refusal: Option<CaptureRefusal> = None;

    for monitor in monitors {
        if monitor.width() < MIN_SIDE || monitor.height() < MIN_SIDE {
            // A degenerate entry in the enumeration. `capture_rect` would
            // refuse it as `TooSmall`, and recording that as the reason the
            // scan failed would describe a monitor the user does not have.
            continue;
        }
        let pixels = match (seams.capture)(*monitor) {
            Ok(pixels) => pixels,
            Err(why) => {
                first_refusal.get_or_insert(why);
                continue;
            }
        };
        captured_any = true;
        let (width, height) = (pixels.width() as usize, pixels.height() as usize);
        let keep_looking = tally.merge((seams.scan)(pixels.pixels(), width, height));
        // Dropped here explicitly, which is what wipes it -- including on the
        // early exit below, where it would otherwise live to the end of the
        // loop body anyway but where the intent is worth stating.
        drop(pixels);
        if !keep_looking {
            break;
        }
    }

    match tally.finish() {
        qr::Codes::One(text) => ScreenScan::Found(text),
        qr::Codes::Several => ScreenScan::Missed(ScanMiss::Several),
        // Nothing found, and which "nothing" it is depends on whether there
        // were any pixels to look at. A desktop that was read and held no
        // code is `NoCode` and the user drags; a desktop that could not be
        // read at all is the refusal in Windows' own words, because "no code
        // found" would be a claim about a screen nobody managed to look at.
        qr::Codes::None if captured_any => ScreenScan::Missed(ScanMiss::NoCode),
        qr::Codes::None => ScreenScan::Missed(ScanMiss::Refused(
            // No monitors at all is `OffScreen` -- the same answer
            // `RegionOverlay::open` gives for the same desktop.
            first_refusal.unwrap_or(CaptureRefusal::OffScreen),
        )),
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
    /// Why this surface opened, painted as the bar's first line. `None`
    /// before the scan has run -- and in production it is `Some` by the time
    /// anything is painted, because the only way to a painted 6b is a scan
    /// that could not answer.
    pub reason: Option<ScanMiss>,
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
    /// How far the whole-screen scan has got. See [`Prescan`].
    prescan: Prescan,
    /// Whether the vault window is currently masked out of screen captures.
    /// The flag rather than a second call: `SetWindowDisplayAffinity` is an
    /// OS call, and this is what makes masking and unmasking idempotent and
    /// what [`Inner::drop`] reads to guarantee the mask comes off.
    masked: bool,
    /// Why 6b opened. `None` until the scan has answered.
    reason: Option<ScanMiss>,
    /// The bar's two chips in points, as [`draw`] last painted them.
    /// `Rect::NOTHING` before the first paint, which contains no point, so a
    /// press on the frame before there are chips hits none of them.
    chips: [egui::Rect; 2],
    /// Which chip the primary button went down on, while it is still down.
    /// See [`RegionOverlay::chip_gesture`].
    chip_press: Option<usize>,
}

/// How far [`scan_screen_with`] has got on this overlay's behalf.
///
/// **A state machine and not a `bool`**, because the scan is deliberately
/// spread over two frames -- see [`PRESCAN_SETTLE`] -- and because the one
/// property that matters most here is that it **ends**. Every transition is
/// forwards: `Due` masks and sets a deadline, `Settling` waits for a clock
/// that only moves one way, and `Done` is absorbing. There is no path back to
/// `Due`, so no repaint can re-run a capture -- which is the shape of the
/// hang this module was fixed for, and the reason this is a type a test can
/// drive rather than a flag read inside a viewport callback.
#[derive(Debug, Clone, Copy)]
enum Prescan {
    /// Nothing has happened yet.
    Due,
    /// Deskwarden's own window is masked; the capture happens on the first
    /// frame at or after this instant.
    Settling { at: Instant },
    /// The scan has run, or was never going to.
    Done,
}

/// What [`RegionOverlay::prescan_step`] wants the caller to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrescanStep {
    /// Mask Deskwarden's own window and come back after [`PRESCAN_SETTLE`].
    Mask,
    /// Still settling; come back after this long.
    Settling(Duration),
    /// Capture and decode now.
    Scan,
    /// Nothing to do, now or ever again.
    Done,
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
                prescan: Prescan::Due,
                masked: false,
                reason: None,
                chips: [egui::Rect::NOTHING; 2],
                chip_press: None,
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
            reason: held.reason,
        }
    }

    /// Why 6b opened, once the scan has said. `None` before then.
    pub fn reason(&self) -> Option<ScanMiss> {
        locked(&self.inner).reason
    }

    /// **Advances the whole-screen scan by one frame**, and says what the
    /// caller should do.
    ///
    /// `now` is an argument for [`DecodeThrottle::should_attempt`]'s reason:
    /// it is what lets a test walk the settle's boundary exactly, and walk
    /// past it, without sleeping and without a window.
    ///
    /// **Every call moves forwards or stands still; none moves back.** That
    /// is the whole of the safety argument -- see [`Prescan`] -- and it is
    /// why the capture this drives cannot become the per-frame loop the
    /// viewport callback was once fixed for.
    fn prescan_step(&self, now: Instant) -> PrescanStep {
        let mut held = locked(&self.inner);
        match held.prescan {
            Prescan::Due => {
                held.prescan = Prescan::Settling {
                    at: now + PRESCAN_SETTLE,
                };
                PrescanStep::Mask
            }
            Prescan::Settling { at } => {
                let left = at.saturating_duration_since(now);
                if left > Duration::ZERO {
                    PrescanStep::Settling(left)
                } else {
                    // Marked `Done` BEFORE the scan runs, not after: the scan
                    // is the slow part, and a state that only advanced on the
                    // way out would let a re-entrant repaint start a second
                    // one.
                    held.prescan = Prescan::Done;
                    PrescanStep::Scan
                }
            }
            Prescan::Done => PrescanStep::Done,
        }
    }

    /// **Applies a scan's answer**, whether it came from the one taken before
    /// the window opened or from the *Whole screen* chip pressed on it.
    ///
    /// One code ends the overlay with [`Outcome::Decoded`] -- and therefore
    /// lands on 6c, which is where every route lands and where the only Save
    /// in this feature lives. Anything else leaves the overlay up and records
    /// why, which is what the bar's first line then says.
    ///
    /// The stale lock-on is cleared with it: a rescan that found nothing must
    /// not leave "Code found" above a rectangle from before it.
    fn apply_scan(&self, scan: ScreenScan) {
        match scan {
            ScreenScan::Found(text) => self.finish(Outcome::Decoded(text)),
            ScreenScan::Missed(miss) => {
                let mut held = locked(&self.inner);
                held.reason = Some(miss);
                held.found = false;
            }
        }
    }

    /// Takes Deskwarden's own window out of screen captures, or puts it back.
    ///
    /// Idempotent through [`Inner::masked`], so the callers can say what they
    /// want rather than track what they have already asked for -- and so the
    /// OS call happens exactly on the transitions.
    fn mask_own_window(&self, on: bool) {
        {
            let mut held = locked(&self.inner);
            if held.masked == on {
                return;
            }
            held.masked = on;
        }
        set_capture_exclusion(crate::vault_window::WINDOW_TITLE, on);
    }

    /// Records where [`draw`] painted the bar's chips, so the next frame's
    /// pointer handling can tell a press on one from a drag.
    fn remember_chips(&self, chips: [egui::Rect; 2]) {
        locked(&self.inner).chips = chips;
    }

    /// Whether the button is down on a chip, in which case this frame's
    /// pointer belongs to the chip and not to a selection.
    fn in_chip_press(&self) -> bool {
        locked(&self.inner).chip_press.is_some()
    }

    /// **One frame of the bar's chips**: `Some(index)` on the frame a press
    /// that began on a chip is released over that same chip.
    ///
    /// # Why the chips are pressed rather than merely printed
    ///
    /// 6b already names both shortcuts on screen, which is most of what a
    /// discoverable action needs -- but a bordered pill that says *Whole
    /// screen* and does nothing when it is clicked is a lie in the shape of a
    /// button, and the whole-screen scan is now the primary way this feature
    /// works rather than a corner shortcut. The key still works and is still
    /// printed beside the label; this adds the press people will try first.
    ///
    /// # Why the whole gesture is swallowed and not just the click
    ///
    /// This surface reads the raw pointer: any press is the start of a
    /// selection and any release ends one, so a press on a chip that only got
    /// *taken* on release would still have begun a drag on the way down, and
    /// the release would then read the one-pixel rectangle under the chip and
    /// end the overlay with "that region is too small". So the press is
    /// remembered, and `advance` is skipped for as long as it is held.
    ///
    /// # Why release and not press
    ///
    /// The convention every button in this app follows, and it is the one
    /// that lets a user who pressed the wrong chip slide off it and let go.
    fn chip_gesture(&self, pointer: Option<(f32, f32)>, pressed: bool, down: bool) -> Option<usize> {
        let at = pointer.map(|(x, y)| egui::pos2(x, y));
        let mut held = locked(&self.inner);
        if pressed {
            held.chip_press = at.and_then(|p| held.chips.iter().position(|chip| chip.contains(p)));
            return None;
        }
        if down {
            return None;
        }
        let started_on = held.chip_press.take()?;
        let over = at.is_some_and(|p| held.chips[started_on].contains(p));
        if over {
            Some(started_on)
        } else {
            None
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
            // **An overlay that has answered captures nothing more.** The
            // released drag is still recorded in `drag` -- deliberately, so
            // the outcome and the rectangle it came from stay consistent --
            // which means every later call would take the release arm again
            // and re-capture the same region. See the guard at the top of the
            // viewport callback for what that cost when the window kept
            // repainting after the release.
            if !held.open {
                return;
            }
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
            self.mask_own_window(false);
            return false;
        }

        // **The whole-screen scan, before this module has a window at all.**
        //
        // This is the departure the module header argues for: choosing the
        // route is asking for the scan, so the scan happens here and 6b opens
        // only if it cannot answer. Nothing below this block runs on the
        // frames it takes, so no viewport is registered and no dim appears --
        // a user whose code is found never sees this surface.
        //
        // It is bounded three ways, and the bound is the point: `prescan_step`
        // only ever moves forwards and ends at `Done`; the capture happens on
        // exactly one frame; and the repaints asked for below are asked for
        // against a deadline that a clock reaches on its own. This is not the
        // per-frame capture the viewport callback was fixed for -- the two
        // differ precisely in that this one has an exit.
        match self.prescan_step(Instant::now()) {
            PrescanStep::Mask => {
                self.mask_own_window(true);
                ctx.request_repaint_after(PRESCAN_SETTLE);
                return true;
            }
            PrescanStep::Settling(left) => {
                ctx.request_repaint_after(left);
                return true;
            }
            PrescanStep::Scan => {
                // `monitor_bounds()` enumerates the real desktop -- the one
                // production call, exactly as `RegionOverlay::open` takes its
                // monitors as an argument so the arithmetic can be tested
                // without one.
                let monitors = screen_capture::monitor_bounds();
                self.apply_scan(scan_screen_with(&RegionSeams::production(), &monitors));
                if !self.is_open() {
                    // One code, and the overlay is over before it began.
                    self.mask_own_window(false);
                    return false;
                }
                // The mask stays on for the life of the overlay. 6b is
                // about to open, the user may press *Whole screen* on it, and
                // its own release-capture is better off not seeing the vault
                // window either -- a box dragged over a window Deskwarden is
                // sitting on top of should read what the user can see behind
                // it, which is the same reason the overlay excludes itself.
                // Every way out of `show` puts it back, and `Inner`'s `Drop`
                // covers the ways that do not come through `show` at all.
            }
            PrescanStep::Done => {}
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
                // **The overlay is over the moment an outcome is recorded, and
                // from then on this callback must do nothing at all.**
                //
                // Only the *parent* viewport can take this window down: it
                // stops re-registering the deferred viewport once `show`
                // answers `false`. Until the parent runs a frame and does
                // that, egui keeps repainting this one -- and `show` asks it
                // to, once per frame, so that a lock-on cannot go stale under
                // a pointer held still. Without this guard those repaints ran
                // the whole callback again on a drag that had already been
                // released: `advance` found the same released drag every
                // frame and captured and decoded the same rectangle over and
                // over on the UI thread, with no exit. **That is the hang.** A
                // plain click was enough to reach it -- press and release with
                // no movement is a released drag like any other -- and Escape
                // could not get out of it, because `finish` is deliberately a
                // no-op once an outcome exists.
                //
                // The repaint request is the other half of the fix, and it is
                // aimed at the ROOT viewport rather than at this one. The root
                // is the only thing that can close this window, and nothing
                // else is going to wake it: an always-on-top window covering
                // every monitor means the vault window sees no input of its
                // own, so it can sit idle indefinitely with a finished overlay
                // still on screen in front of it.
                if !mine.is_open() {
                    root.request_repaint_of(egui::ViewportId::ROOT);
                    return;
                }

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

                let (pointer, pressed, down) = root.input(|i| {
                    (
                        i.pointer.latest_pos().map(|p| (p.x, p.y)),
                        i.pointer.primary_pressed(),
                        i.pointer.primary_down(),
                    )
                });
                // Taken before anything else looks at the pointer: a press
                // that landed on a chip is that chip's for the whole gesture,
                // and `advance` must not see it as a selection. See
                // `chip_gesture`.
                let chip = mine.chip_gesture(pointer, pressed, down);
                if root.input(|i| i.key_pressed(egui::Key::Escape))
                    || root.input(|i| i.viewport().close_requested())
                    || chip == Some(CHIP_CANCEL)
                {
                    mine.finish(Outcome::Cancelled);
                } else if chip == Some(CHIP_WHOLE_SCREEN)
                    || root.input(|i| i.key_pressed(egui::Key::A))
                {
                    // **The same scan the route already ran**, on demand: the
                    // user may have moved a window, closed one of two codes,
                    // or zoomed the page since. One press is one scan -- a key
                    // press and a release are single events, so this is
                    // bounded by the user rather than by the frame rate -- and
                    // when it finds one code it ends the overlay on 6c exactly
                    // as the first scan would have.
                    mine.apply_scan(scan_screen_with(
                        &RegionSeams::production(),
                        &screen_capture::monitor_bounds(),
                    ));
                } else if !mine.in_chip_press() {
                    mine.advance(&RegionSeams::production(), pointer, down, Instant::now());
                }

                // Whichever of the three above ended the overlay, this is the
                // one place that wakes the root so it can close the window.
                // Nothing is painted on the frame that ends it: the dim
                // vanishing the instant the button comes up is the honest
                // picture of what happened, and one more painted frame of a
                // surface that has already answered is a surface the user can
                // still try to drag on.
                if !mine.is_open() {
                    root.request_repaint_of(egui::ViewportId::ROOT);
                    return;
                }

                let view = mine.view();
                let chips = egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(root, |ui| draw(ui, &view))
                    .inner;
                // Where the chips landed, for the NEXT frame's pointer
                // handling. A frame late by construction -- the bar's height
                // depends on the type it carries, which is not known until it
                // is laid out -- and that costs nothing: on the first frame
                // there is nothing to have clicked, and after it the
                // rectangles only move if the reason line does.
                mine.remember_chips(chips);
            },
        );
        let still_open = self.is_open();
        if !still_open {
            // Whatever ended it, the vault window goes back into screen
            // captures here. `Inner`'s `Drop` is the backstop for the paths
            // that do not come through this line -- the form closing under a
            // live overlay, or a panic unwinding past it.
            self.mask_own_window(false);
        }
        still_open
    }
}

/// **The mask comes off however the overlay ends.**
///
/// `show` takes it off on the frame it answers `false`, which covers every
/// ordinary ending. This covers the rest: the caller drops the overlay
/// because the form closed under it, the vault locks, or a panic unwinds
/// through the frame. `WDA_EXCLUDEFROMCAPTURE` outlives whoever set it, and a
/// vault window left permanently invisible to the user's own screenshots and
/// to a support call's screen share would be a side effect of a scan they
/// took once -- exactly the kind of thing a `Drop` exists to make impossible
/// to forget.
///
/// It is on `Inner` rather than on [`RegionOverlay`] because the overlay is an
/// `Arc` handle that is cloned per frame; this runs when the last one goes.
impl Drop for Inner {
    fn drop(&mut self) {
        if self.masked {
            set_capture_exclusion(crate::vault_window::WINDOW_TITLE, false);
        }
    }
}

/// **Paints 6b.** Pure in the sense that matters here: everything it decides
/// comes out of `view`, so what it draws for a given state is the same every
/// time. It is not unit-tested beyond that -- painting is checked by looking
/// at it, and `RegionView`, the placement helpers below and the constants
/// above are what the tests pin instead.
///
/// **Painting order is the design's DOM order and is load-bearing.** 6b
/// stacks the dim, then the selection and its furniture, then the bottom bar
/// last -- so a selection dragged down over the bar is covered by it rather
/// than punching a lit hole through the one part of this surface that has to
/// stay readable.
///
/// **Returns the two chips' rectangles**, which is the one thing the painter
/// knows and the pointer handling needs. They cannot be computed ahead of the
/// paint: the bar's height follows the type it carries, and the type includes
/// a reason line that is there or not. See
/// [`RegionOverlay::remember_chips`].
pub fn draw(ui: &mut egui::Ui, view: &RegionView) -> [egui::Rect; 2] {
    let full = ui.max_rect();
    let painter = ui.painter().clone();
    let dim = egui::Color32::from_rgba_unmultiplied(0x20, 0x1e, 0x1d, DIM_ALPHA);

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
            paint_selection_edge(&painter, sel);
            if let Some((w, h)) = view.size {
                paint_size_readout(&painter, sel, w, h);
            }
            if lockon_badge(view.found).is_some() {
                paint_lockon_badge(&painter, full, sel);
            }
        }
    }

    let chips = paint_bar(&painter, full, view.reason);
    // The one hover affordance the chips get. A fill that changed under the
    // pointer would have to be painted from the PREVIOUS frame's rectangles,
    // and a pill that lights up a frame after the pointer reaches it is worse
    // than one that does not light up at all; the cursor is the browser's own
    // answer to the same problem and it needs no state.
    if let Some(at) = ui.ctx().pointer_latest_pos() {
        if chips.iter().any(|chip| chip.contains(at)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    chips
}

/// The two rings 6b draws around the selection, plus its four corner
/// brackets.
///
/// Both rings are painted **outside** the lit rectangle -- `box-shadow` in the
/// design, which is drawn beyond the border box -- so neither of them covers a
/// pixel the user framed. That is not only fidelity: the rectangle the ring
/// encloses is the rectangle that gets captured, and a ring painted over the
/// selection's own edge would hide the two rows of quiet zone that decide
/// whether the code reads.
fn paint_selection_edge(painter: &egui::Painter, sel: egui::Rect) {
    // The soft ring first, then the solid one over its inner edge:
    // `0 0 0 8px rgba(27, 63, 160, 0.28)` occupies the two to eight point
    // band, so a six-point stroke centred five points out is exactly it.
    let halo = egui::Color32::from_rgba_unmultiplied(0x1b, 0x3f, 0xa0, HALO_ALPHA);
    painter.rect_stroke(
        sel.expand(SELECTION_RING + SELECTION_HALO / 2.0),
        0.0,
        egui::Stroke::new(SELECTION_HALO, halo),
        egui::StrokeKind::Middle,
    );
    painter.rect_stroke(
        sel,
        0.0,
        egui::Stroke::new(SELECTION_RING, theme::BLUE),
        egui::StrokeKind::Outside,
    );

    // The brackets sit at `left/top: -2px` from the selection, which is the
    // outer edge of the solid ring, and each is an L of two filled arms rather
    // than two strokes: a stroke would need its own half-width offsets to land
    // on the same pixels the design's borders do.
    let outer = sel.expand(SELECTION_RING);
    let arm = egui::vec2(CORNER_ARM, CORNER_THICK);
    let post = egui::vec2(CORNER_THICK, CORNER_ARM);
    for (corner, towards) in [
        (outer.left_top(), egui::vec2(1.0, 1.0)),
        (outer.right_top(), egui::vec2(-1.0, 1.0)),
        (outer.left_bottom(), egui::vec2(1.0, -1.0)),
        (outer.right_bottom(), egui::vec2(-1.0, -1.0)),
    ] {
        for reach in [arm, post] {
            let away = egui::vec2(reach.x * towards.x, reach.y * towards.y);
            painter.rect_filled(
                egui::Rect::from_two_pos(corner, corner + away),
                0.0,
                theme::BLUE,
            );
        }
    }
}

/// Where 6b puts the lock-on badge: `left: 0; top: -34px` relative to the
/// selection, at `height: 26px`.
///
/// **Clamped into `full`, which the design has no need to say.** 6b draws one
/// selection in the middle of a mock desktop; a real drag can start two points
/// from the top of the screen, and a badge honestly placed 34 points above
/// that is a badge nobody sees -- on the one screen whose whole point is
/// telling the user, before they let go, that the code was found. So the
/// design's offset is the *preferred* position and this is what happens when
/// it does not fit.
pub fn badge_rect(full: egui::Rect, selection: egui::Rect, width: f32) -> egui::Rect {
    let wanted = egui::Rect::from_min_size(
        egui::pos2(selection.left(), selection.top() - BADGE_OFFSET),
        egui::vec2(width, BADGE_HEIGHT),
    );
    let x = wanted.left().clamp(full.left(), (full.right() - width).max(full.left()));
    let y = wanted
        .top()
        .clamp(full.top(), (full.bottom() - BADGE_HEIGHT).max(full.top()));
    egui::Rect::from_min_size(egui::pos2(x, y), wanted.size())
}

/// Where 6b puts the size readout: `right: 6px; bottom: 6px`, **inside** the
/// selection, so the number never sits on the desktop the user is reading
/// around it.
pub fn size_readout_rect(selection: egui::Rect, size: egui::Vec2) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            selection.right() - SIZE_INSET - size.x,
            selection.bottom() - SIZE_INSET - size.y,
        ),
        size,
    )
}

/// 6b's bar: `left: 0; right: 0; bottom: 0`, so it is the full width of the
/// surface and flush with its bottom edge. The height is the taller of its two
/// contents plus `padding: 14px 18px` top and bottom -- the design's own
/// `align-items: center` on a flex row, rather than a number copied off a
/// rendered screenshot.
pub fn bar_rect(full: egui::Rect, content_height: f32) -> egui::Rect {
    let height = BAR_PAD_Y * 2.0 + content_height.max(CHIP_HEIGHT);
    egui::Rect::from_min_max(
        egui::pos2(full.left(), (full.bottom() - height).max(full.top())),
        full.right_bottom(),
    )
}

/// The `M20 6 9 17l-5-5` of 6b's badge, drawn into `box` from the svg's own
/// 24-unit viewBox so the shape scales with the badge rather than with a
/// second set of hand-converted coordinates.
fn paint_tick(painter: &egui::Painter, at: egui::Rect, colour: egui::Color32, svg_stroke: f32) {
    let scale = at.width() / 24.0;
    let point = |x: f32, y: f32| at.min + egui::vec2(x * scale, y * scale);
    let stroke = egui::Stroke::new(svg_stroke * scale, colour);
    painter.line_segment([point(20.0, 6.0), point(9.0, 17.0)], stroke);
    painter.line_segment([point(9.0, 17.0), point(4.0, 12.0)], stroke);
}

/// 6b's lock-on badge: a blue pill above the selection's top-left carrying a
/// tick and [`LOCKED_ON`].
fn paint_lockon_badge(painter: &egui::Painter, full: egui::Rect, sel: egui::Rect) {
    let Some(words) = lockon_badge(true) else {
        return;
    };
    let galley = painter.layout_no_wrap(
        words.to_owned(),
        egui::FontId::new(BADGE_TEXT_PX, egui::FontFamily::Name(theme::BOLD.into())),
        theme::CARD,
    );
    let width = BADGE_PAD_X * 2.0 + BADGE_TICK + BADGE_GAP + galley.size().x;
    let rect = badge_rect(full, sel, width);
    painter.rect_filled(rect, egui::CornerRadius::same(BADGE_RADIUS), theme::BLUE);

    let tick = egui::Rect::from_min_size(
        egui::pos2(rect.left() + BADGE_PAD_X, rect.center().y - BADGE_TICK / 2.0),
        egui::Vec2::splat(BADGE_TICK),
    );
    paint_tick(painter, tick, theme::CARD, BADGE_TICK_STROKE);
    let text_height = galley.size().y;
    painter.galley(
        egui::pos2(tick.right() + BADGE_GAP, rect.center().y - text_height / 2.0),
        galley,
        theme::CARD,
    );
}

/// 6b's `250 × 250` plate, tucked into the selection's bottom-right corner.
fn paint_size_readout(painter: &egui::Painter, sel: egui::Rect, width: u32, height: u32) {
    let words = size_label(&ScreenRect {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    });
    let ink = theme::WINDOW_BG;
    let galley = painter.layout_no_wrap(words, egui::FontId::monospace(SIZE_TEXT_PX), ink);
    let plate = size_readout_rect(
        sel,
        galley.size() + egui::vec2(SIZE_PAD_X * 2.0, SIZE_PAD_Y * 2.0),
    );
    painter.rect_filled(
        plate,
        egui::CornerRadius::same(SIZE_RADIUS),
        egui::Color32::from_rgba_unmultiplied(0x20, 0x1e, 0x1d, SIZE_BG_ALPHA),
    );
    painter.galley(
        plate.min + egui::vec2(SIZE_PAD_X, SIZE_PAD_Y),
        galley,
        ink,
    );
}

/// One shortcut chip: a bordered pill carrying a label and, in the design's
/// monospace, the key that does the same thing. Returns its width, so the pair
/// can be right-aligned as a group before either is drawn.
fn chip_width(painter: &egui::Painter, label: &str, key: &str) -> f32 {
    let label_width = painter
        .layout_no_wrap(
            label.to_owned(),
            egui::FontId::new(CHIP_TEXT_PX, egui::FontFamily::Name(theme::SEMIBOLD.into())),
            theme::WINDOW_BG,
        )
        .size()
        .x;
    let key_width = painter
        .layout_no_wrap(key.to_owned(), egui::FontId::monospace(CHIP_KEY_PX), theme::TEXT_GHOST)
        .size()
        .x;
    CHIP_PAD_X * 2.0 + label_width + CHIP_GAP + key_width
}

/// Draws the chip [`chip_width`] measured, with its left edge at `left`, and
/// hands back the rectangle it drew -- which is what
/// [`RegionOverlay::chip_gesture`] later tests a press against, so the shape
/// the user aims at and the shape that answers are the same one by
/// construction rather than by two agreeing calculations.
fn paint_chip(
    painter: &egui::Painter,
    left: f32,
    middle: f32,
    label: &str,
    key: &str,
) -> egui::Rect {
    let width = chip_width(painter, label, key);
    let rect = egui::Rect::from_min_size(
        egui::pos2(left, middle - CHIP_HEIGHT / 2.0),
        egui::vec2(width, CHIP_HEIGHT),
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(CHIP_RADIUS),
        egui::Stroke::new(1.0, theme::TEXT_MUTED),
        egui::StrokeKind::Inside,
    );
    let label_galley = painter.layout_no_wrap(
        label.to_owned(),
        egui::FontId::new(CHIP_TEXT_PX, egui::FontFamily::Name(theme::SEMIBOLD.into())),
        theme::WINDOW_BG,
    );
    let label_size = label_galley.size();
    painter.galley(
        egui::pos2(rect.left() + CHIP_PAD_X, middle - label_size.y / 2.0),
        label_galley,
        theme::WINDOW_BG,
    );
    let key_galley = painter.layout_no_wrap(
        key.to_owned(),
        egui::FontId::monospace(CHIP_KEY_PX),
        theme::TEXT_GHOST,
    );
    let key_size = key_galley.size();
    painter.galley(
        egui::pos2(
            rect.left() + CHIP_PAD_X + label_size.x + CHIP_GAP,
            middle - key_size.y / 2.0,
        ),
        key_galley,
        theme::TEXT_GHOST,
    );
    rect
}

/// **6b's bottom bar**: why this surface opened, the instruction, the
/// sentence that says nothing has been saved, and the two shortcut chips.
///
/// Drawn on every frame and in every state -- see [`DRAG_TITLE`]. It is also
/// the only place on this surface that names Escape, so a user who opened the
/// overlay by accident always has the way out in front of them.
///
/// The first line is [`scan_miss_line`] and is the design's one addition here
/// -- see [`SCAN_NO_CODE`] for why 6b needs one. It is laid out first and
/// measured into the bar's height rather than overlaid, so the bar grows by a
/// line instead of the instruction moving to make room.
///
/// Returns the two chips' rectangles, in the order [`CHIP_WHOLE_SCREEN`] and
/// [`CHIP_CANCEL`] name.
fn paint_bar(
    painter: &egui::Painter,
    full: egui::Rect,
    reason: Option<ScanMiss>,
) -> [egui::Rect; 2] {
    let lead = reason.map(|miss| {
        painter.layout_no_wrap(
            scan_miss_line(miss),
            egui::FontId::new(
                BAR_REASON_PX,
                egui::FontFamily::Name(theme::SEMIBOLD.into()),
            ),
            BAR_REASON_INK,
        )
    });
    let title = painter.layout_no_wrap(
        DRAG_TITLE.to_owned(),
        egui::FontId::new(BAR_TITLE_PX, egui::FontFamily::Name(theme::BOLD.into())),
        theme::CARD,
    );
    let hint = painter.layout_no_wrap(
        DRAG_HINT.to_owned(),
        egui::FontId::proportional(BAR_HINT_PX),
        BAR_HINT_INK,
    );
    let (title_size, hint_size) = (title.size(), hint.size());
    let lead_height = lead.as_ref().map_or(0.0, |lead| lead.size().y + BAR_LINE_GAP);
    let block = lead_height + title_size.y + BAR_LINE_GAP + hint_size.y;
    let bar = bar_rect(full, block);

    painter.rect_filled(
        bar,
        0.0,
        egui::Color32::from_rgba_unmultiplied(0x20, 0x1e, 0x1d, BAR_BG_ALPHA),
    );
    painter.line_segment(
        [bar.left_top(), bar.right_top()],
        egui::Stroke::new(1.0, BAR_EDGE),
    );

    let mut top = bar.center().y - block / 2.0;
    if let Some(lead) = lead {
        let height = lead.size().y;
        painter.galley(
            egui::pos2(bar.left() + BAR_PAD_X, top),
            lead,
            BAR_REASON_INK,
        );
        top += height + BAR_LINE_GAP;
    }
    painter.galley(egui::pos2(bar.left() + BAR_PAD_X, top), title, theme::CARD);
    painter.galley(
        egui::pos2(bar.left() + BAR_PAD_X, top + title_size.y + BAR_LINE_GAP),
        hint,
        BAR_HINT_INK,
    );

    // The pair is right-aligned as a group, in the design's order: whole
    // screen first, cancel last and therefore nearest the corner the pointer
    // travels to. That order is also `CHIP_WHOLE_SCREEN` and `CHIP_CANCEL`,
    // which is what the returned rectangles are indexed by.
    let faces = [
        (WHOLE_SCREEN_HINT, WHOLE_SCREEN_KEY),
        (CANCEL_HINT, CANCEL_KEY),
    ];
    let total: f32 = faces
        .iter()
        .map(|(label, key)| chip_width(painter, label, key))
        .sum::<f32>()
        + CHIP_GAP * (faces.len() as f32 - 1.0);
    let mut left = bar.right() - BAR_PAD_X - total;
    let mut drawn = [egui::Rect::NOTHING; 2];
    for (at, (label, key)) in faces.into_iter().enumerate() {
        let rect = paint_chip(painter, left, bar.center().y, label, key);
        drawn[at] = rect;
        left = rect.right() + CHIP_GAP;
    }
    drawn
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
/// # The rejected alternative, and the one that replaced it
///
/// This note used to end by rejecting a whole-desktop capture outright: it
/// "would mean this feature takes a full-screen capture the user never asked
/// for, which is exactly the third security property of the design (capture
/// only the rectangle the user dragged)". **The premise of that has changed
/// and the conclusion with it.** The user now asks for a whole-screen capture
/// by choosing the route -- see the module header -- so [`scan_screen_with`]
/// takes one, once, per monitor, and drops it.
///
/// What was rejected then and is still rejected now is the *reason* the old
/// note gave for wanting one: **freezing that capture and painting it as this
/// overlay's background**. That is a copy of the user's entire screen living
/// in `egui`'s texture manager for as long as the overlay is up, in memory
/// this crate cannot wipe, to make a dim look slightly better. The scan
/// decodes and drops; nothing displays it.
///
/// [`set_capture_exclusion`] is what this delegates to, and the reason it
/// takes a flag is the vault window: that one is masked only for the length
/// of the scan and is put back, because a window left permanently missing
/// from the user's own screenshots is not a change this feature is entitled
/// to make. This overlay is different -- it is masked and then destroyed, so
/// there is nothing to put back.
fn exclude_from_capture(title: &str) {
    set_capture_exclusion(title, true);
}

/// [`exclude_from_capture`]'s undoable form: `WDA_EXCLUDEFROMCAPTURE` when
/// `exclude`, `WDA_NONE` when not.
///
/// **`WDA_NONE` and not "whatever it was before".** Reading the previous
/// affinity back would look more careful and would be worse: the only window
/// this is ever pointed at is the vault window, which sets no affinity of its
/// own -- deliberately, because it is a window the user is entitled to
/// screenshot and screen-share -- so `WDA_NONE` *is* what it was. Preserving
/// a value nothing sets would be machinery in place of the one fact that
/// matters.
fn set_capture_exclusion(title: &str, exclude: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };

    let Some(hwnd) = crate::foreground::own_window_titled(title) else {
        return;
    };
    let affinity = if exclude {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // Failure is ignored on purpose: see this function's note. There is
    // nothing useful to do about it and nothing secret is at risk -- a mask
    // that was refused leaves a window in the capture, and an unmask that was
    // refused leaves a window out of other people's captures, which is the
    // safe direction of the two.
    unsafe {
        let _ = SetWindowDisplayAffinity(HWND(hwnd as *mut _), affinity);
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

    /// The same in the space the painter works in: egui points inside the
    /// viewport, which is what every placement helper takes and returns.
    fn rect_pts(left: f32, top: f32, right: f32, bottom: f32) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
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

    fn no_codes(_: &[u8], _: usize, _: usize) -> qr::Codes {
        qr::Codes::None
    }

    fn several_codes(_: &[u8], _: usize, _: usize) -> qr::Codes {
        qr::Codes::Several
    }

    /// A scan whose payload **names the size of the picture it was given**.
    ///
    /// Two monitors of different sizes therefore hold two different codes,
    /// and two of the same size hold the same one -- which is exactly the
    /// distinctness rule under test, expressed without a shared counter that
    /// two tests running in parallel could race each other on.
    fn code_naming_the_size(_: &[u8], w: usize, h: usize) -> qr::Codes {
        qr::Codes::One(Zeroizing::new(format!(
            "otpauth://totp/Git%20Host:anovak{w}x{h}?secret=JBSWY3DPEHPK3PXP"
        )))
    }

    /// A capture that applies the **real** size bound without the
    /// allocation: `clamp_to_monitors` is what production's `capture_rect`
    /// runs before it blits, and `qr::MAX_PIXELS` is the number it applies.
    /// A monitor past it is refused here exactly as it would be there, and
    /// nothing tries to reserve 64 megapixels of RGBA to prove it.
    fn bounded_capture(r: ScreenRect) -> Result<Rgba, CaptureRefusal> {
        let clamped = screen_capture::clamp_to_monitors(r, &[r])?;
        flat_capture(clamped)
    }

    fn seams(
        capture: fn(ScreenRect) -> Result<Rgba, CaptureRefusal>,
        decode: fn(&[u8], usize, usize) -> Option<Zeroizing<String>>,
    ) -> RegionSeams {
        RegionSeams {
            capture,
            decode,
            scan: no_codes,
        }
    }

    /// The scanning half of the same seam. `decode` is the drag's and is
    /// never reached by [`scan_screen_with`], so it is pointed at the stub
    /// that finds nothing -- a scan that quietly went through the drag's
    /// decoder would show up as an answer of `NoCode` rather than as a pass.
    fn scan_seams(
        capture: fn(ScreenRect) -> Result<Rgba, CaptureRefusal>,
        scan: fn(&[u8], usize, usize) -> qr::Codes,
    ) -> RegionSeams {
        RegionSeams {
            capture,
            decode: no_code,
            scan,
        }
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
        assert!(std::ptr::fn_addr_eq(
            production.scan,
            qr::codes_in as fn(&[u8], usize, usize) -> qr::Codes
        ));
        // The drag's decoder and the scan's counter are DIFFERENT functions.
        // A `scan` quietly pointed at `decode_qr`'s wrapper would answer
        // "one code" for a desktop holding two, which is the one answer this
        // feature must never give.
        assert!(!std::ptr::fn_addr_eq(
            production.scan,
            no_codes as fn(&[u8], usize, usize) -> qr::Codes
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

    // -- the paint, pinned where it is a number and not a picture -----------

    /// **Every measurement `draw` uses is design 6b's, quoted here with the
    /// CSS it came from.**
    ///
    /// Painting itself is checked by looking at it; what a test can hold is
    /// that nobody has quietly rounded one of these to a nicer number. Each
    /// assertion below is the declaration in `Deskwarden.dc.html` under
    /// `id="6b"`, in the order the surface stacks.
    #[test]
    fn the_overlays_numbers_are_the_designs_own() {
        // `background: #201e1d` with the desktop over it at `opacity: 0.32`,
        // which is this ink at 68%: 0.68 * 255 = 173.4.
        assert_eq!(DIM_ALPHA, 173);

        // `box-shadow: 0 0 0 2px #1b3fa0, 0 0 0 8px rgba(27, 63, 160, 0.28)`
        // -- so two points solid and six more soft, at 0.28 * 255 = 71.4.
        assert_eq!(SELECTION_RING, 2.0);
        assert_eq!(SELECTION_HALO, 6.0);
        assert_eq!(HALO_ALPHA, 71);
        assert_eq!(SELECTION_RING + SELECTION_HALO, 8.0, "the halo no longer ends at 8px");

        // `width: 14px; height: 14px; border-left: 3px solid #1b3fa0`.
        assert_eq!((CORNER_ARM, CORNER_THICK), (14.0, 3.0));

        // `height: 26px; padding: 0 10px; border-radius: 6px; gap: 8px`, a
        // `13`-pixel tick at `stroke-width="2.8"`, `font-size: 12px`, and
        // `top: -34px`.
        assert_eq!(BADGE_HEIGHT, 26.0);
        assert_eq!(BADGE_PAD_X, 10.0);
        assert_eq!(BADGE_RADIUS, 6);
        assert_eq!(BADGE_GAP, 8.0);
        assert_eq!((BADGE_TICK, BADGE_TICK_STROKE), (13.0, 2.8));
        assert_eq!(BADGE_TEXT_PX, 12.0);
        assert_eq!(BADGE_OFFSET, 34.0);
        assert!(
            BADGE_OFFSET > BADGE_HEIGHT,
            "the badge would overlap the selection it is announcing"
        );

        // `right: 6px; bottom: 6px; padding: 3px 7px; border-radius: 5px;
        // font-size: 11px`, on `rgba(32, 30, 29, 0.86)` -- 0.86 * 255 = 219.3.
        assert_eq!(SIZE_INSET, 6.0);
        assert_eq!((SIZE_PAD_X, SIZE_PAD_Y), (7.0, 3.0));
        assert_eq!(SIZE_RADIUS, 5);
        assert_eq!(SIZE_TEXT_PX, 11.0);
        assert_eq!(SIZE_BG_ALPHA, 219);

        // `padding: 14px 18px`, `rgba(32, 30, 29, 0.92)` -- 0.92 * 255 = 234.6
        // -- `border-top: 1px solid #3a3736`, `13px/700` over `12px #bab6b6`
        // at `gap: 3px`.
        assert_eq!((BAR_PAD_X, BAR_PAD_Y), (18.0, 14.0));
        assert_eq!(BAR_BG_ALPHA, 235);
        assert_eq!(BAR_EDGE, egui::Color32::from_rgb(0x3a, 0x37, 0x36));
        assert_eq!((BAR_TITLE_PX, BAR_HINT_PX), (13.0, 12.0));
        assert_eq!(BAR_HINT_INK, egui::Color32::from_rgb(0xba, 0xb6, 0xb6));
        assert_eq!(BAR_LINE_GAP, 3.0);

        // `height: 28px; padding: 0 11px; border-radius: 7px; gap: 8px`, with
        // a `12px/600` label and a `10px` monospace key.
        assert_eq!(CHIP_HEIGHT, 28.0);
        assert_eq!(CHIP_PAD_X, 11.0);
        assert_eq!(CHIP_RADIUS, 7);
        assert_eq!(CHIP_GAP, 8.0);
        assert_eq!((CHIP_TEXT_PX, CHIP_KEY_PX), (12.0, 10.0));

        // The bar is never shorter than the chips it carries, which is what
        // `align-items: center` on the design's flex row means.
        assert_eq!(bar_rect(rect_pts(0.0, 0.0, 800.0, 600.0), 0.0).height(), 56.0);
    }

    /// **The chips say the keys the callback really matches on.** A chip
    /// promising `A` beside a handler watching some other key is the class of
    /// lie `ADD_TOTP_SHORTCUT` is pinned against elsewhere in this crate.
    #[test]
    fn the_shortcut_chips_name_the_keys_that_work() {
        assert_eq!(WHOLE_SCREEN_HINT, "Whole screen");
        assert_eq!(WHOLE_SCREEN_KEY, "A");
        assert_eq!(CANCEL_HINT, "Cancel");
        assert_eq!(CANCEL_KEY, "ESC");

        // And the callback really watches those two, which is only assertable
        // from here by source -- the closure is inside a viewport builder no
        // harness in this crate can call.
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        assert!(code.contains("i.key_pressed(egui::Key::Escape)"));
        assert!(code.contains("i.key_pressed(egui::Key::A)"));
        // And the chips are pressable, not merely printed: the callback acts
        // on both indices. A chip that looked like a button and did nothing
        // when clicked would be the same class of lie as one naming a key
        // nothing watches.
        assert!(code.contains("chip == Some(CHIP_CANCEL)"));
        assert!(code.contains("chip == Some(CHIP_WHOLE_SCREEN)"));
    }

    /// **The chips are drawn in the order their indices name.**
    ///
    /// `chip_gesture` answers with a position in the array `paint_bar` drew,
    /// and the callback turns that position into an action. A chip inserted
    /// in front of these two would make Cancel scan the screen and the scan
    /// cancel, with nothing else failing.
    #[test]
    fn the_chips_are_drawn_in_the_order_their_indices_name() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        let faces = code
            .split("let faces = [")
            .nth(1)
            .expect("`paint_bar` no longer builds its chips from one array");
        let faces = faces.split("];").next().unwrap();
        let whole = faces.find("WHOLE_SCREEN_HINT").expect("no whole-screen chip");
        let cancel = faces.find("CANCEL_HINT").expect("no cancel chip");
        assert!(whole < cancel, "the chips are no longer in the design's order");
        assert_eq!(CHIP_WHOLE_SCREEN, 0);
        assert_eq!(CHIP_CANCEL, 1);
        assert_ne!(CHIP_WHOLE_SCREEN, CHIP_CANCEL);
    }

    /// The badge sits at the selection's **top-left**, one `BADGE_OFFSET`
    /// above it -- and is pulled back onto the screen when the drag started
    /// too near an edge for the design's placement to be visible at all.
    #[test]
    fn the_badge_is_above_the_selections_top_left_and_never_off_screen() {
        let full = rect_pts(0.0, 0.0, 1920.0, 1080.0);
        let sel = rect_pts(300.0, 400.0, 550.0, 650.0);
        let placed = badge_rect(full, sel, 180.0);
        assert_eq!(placed.min, egui::pos2(300.0, 400.0 - BADGE_OFFSET));
        assert_eq!(placed.size(), egui::vec2(180.0, BADGE_HEIGHT));
        assert!(
            placed.bottom() < sel.top(),
            "the badge overlapped the selection it announces"
        );

        // A drag begun two points from the top: the design's -34 is off the
        // screen, so the badge lands on it instead.
        let high = badge_rect(full, rect_pts(10.0, 2.0, 260.0, 252.0), 180.0);
        assert_eq!(high.top(), 0.0);
        // And one begun near the right edge is pulled left far enough to fit.
        let wide = badge_rect(full, rect_pts(1900.0, 500.0, 1910.0, 560.0), 180.0);
        assert_eq!(wide.right(), 1920.0);
        assert_eq!(wide.left(), 1740.0);
    }

    /// The readout is `right: 6px; bottom: 6px` **inside** the selection, so
    /// the number sits on the region being framed rather than on the dim
    /// beside it.
    #[test]
    fn the_size_readout_is_tucked_into_the_selections_bottom_right() {
        let sel = rect_pts(100.0, 100.0, 400.0, 400.0);
        let plate = size_readout_rect(sel, egui::vec2(60.0, 18.0));
        assert_eq!(plate.max, egui::pos2(400.0 - SIZE_INSET, 400.0 - SIZE_INSET));
        assert_eq!(plate.size(), egui::vec2(60.0, 18.0));
        assert!(sel.contains_rect(plate), "the readout hung outside the selection");
    }

    /// The bar spans the whole surface and is flush with its bottom edge --
    /// `left: 0; right: 0; bottom: 0` -- and grows with the type it carries
    /// rather than being a height copied off a screenshot.
    #[test]
    fn the_bar_spans_the_bottom_of_the_surface() {
        let full = rect_pts(0.0, 0.0, 1920.0, 1080.0);
        let bar = bar_rect(full, 40.0);
        assert_eq!(bar.left(), full.left());
        assert_eq!(bar.right(), full.right());
        assert_eq!(bar.bottom(), full.bottom());
        assert_eq!(bar.height(), 40.0 + BAR_PAD_Y * 2.0);
        // Two lines of type shorter than a chip still leave the chips room:
        // the taller of the two decides, which is the flex row's own rule.
        assert_eq!(bar_rect(full, 10.0).height(), CHIP_HEIGHT + BAR_PAD_Y * 2.0);
    }

    /// **The hang, pinned.**
    ///
    /// A released drag stays recorded, so every later `advance` used to take
    /// the release arm again and capture the same rectangle -- and the
    /// viewport kept repainting after the release, so "every later" meant
    /// forever, on the UI thread, with Escape unable to break out because the
    /// outcome was already set. A plain click on the overlay was enough: press
    /// and release without moving is a released drag like any other.
    ///
    /// The capture seam here panics, so a single re-capture fails the test
    /// rather than merely being slow.
    #[test]
    fn an_overlay_that_has_answered_never_captures_again() {
        fn never_called(_: ScreenRect) -> Result<Rgba, CaptureRefusal> {
            panic!("a finished overlay captured pixels again");
        }
        let overlay = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        let t0 = Instant::now();
        // A click: down, then up in the same place. It really does reach the
        // release arm and really does try to read the degenerate rectangle it
        // made -- which `screen_capture` refuses as `TooSmall` -- and that is
        // the control that makes the panicking seam below meaningful.
        overlay.advance(&seams(flat_capture, no_code), Some((40.0, 40.0)), true, t0);
        overlay.advance(
            &seams(flat_capture, no_code),
            Some((40.0, 40.0)),
            false,
            t0 + DECODE_INTERVAL,
        );
        assert!(!overlay.is_open(), "the release did not end the overlay");

        // Every frame the window paints between the release and the parent
        // taking it down. None of them may capture anything.
        for frame in 1..=10 {
            overlay.advance(
                &seams(never_called, no_code),
                Some((40.0, 40.0)),
                false,
                t0 + DECODE_INTERVAL * (1 + frame),
            );
        }
        // Nor may a button pressed again on a surface that has answered start
        // a fresh drag on it.
        overlay.advance(
            &seams(never_called, no_code),
            Some((900.0, 900.0)),
            true,
            t0 + DECODE_INTERVAL * 20,
        );
        // The one answer it did record survived all of that, unchanged.
        assert!(matches!(overlay.take_outcome(), Some(Outcome::Refused(_))));
    }

    /// The other half of the same fix: the viewport callback returns before it
    /// does anything at all once the outcome is in, and asks the **root**
    /// viewport to repaint so that the parent can close this window. Nothing
    /// else wakes the parent -- an always-on-top window over every monitor
    /// means the vault window sees no input of its own -- so a missing request
    /// leaves a finished overlay on screen indefinitely.
    #[test]
    fn the_callback_wakes_the_root_viewport_when_it_is_finished() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        assert_eq!(
            code.matches("root.request_repaint_of(egui::ViewportId::ROOT);").count(),
            2,
            "the callback no longer wakes the root on both the guard and the finishing frame"
        );
        assert!(
            code.contains("if !mine.is_open() {"),
            "the callback no longer guards on a finished overlay"
        );
    }

    // -- the whole-screen scan ---------------------------------------------

    fn miss(scan: ScreenScan) -> ScanMiss {
        match scan {
            ScreenScan::Missed(miss) => miss,
            ScreenScan::Found(_) => panic!("the scan found a code where it should not have"),
        }
    }

    /// **One code on the desktop is answered without a window.**
    ///
    /// The whole point of the feature: the user chose the route and the route
    /// answered. What comes back is the payload, which is what the caller
    /// puts on 6c.
    #[test]
    fn one_code_anywhere_on_the_desktop_is_the_answer() {
        let monitors = [rect(0, 0, 1920, 1080)];
        match scan_screen_with(&scan_seams(flat_capture, code_naming_the_size), &monitors) {
            ScreenScan::Found(text) => assert_eq!(&*text, "otpauth://totp/Git%20Host:anovak1920x1080?secret=JBSWY3DPEHPK3PXP"),
            other => panic!("one code came back as {other:?}"),
        }
    }

    /// **Every monitor is read, not just the biggest one.**
    ///
    /// This is the reason `scan_screen_with` walks monitors instead of
    /// handing `whole_screen`'s bounding box to one capture:
    /// `capture_rect` clamps to the monitor it overlaps most, so a bounding
    /// box would read the large monitor and never look at the small one --
    /// and the small one is where the code is here, exactly as it is when the
    /// user has the vault window on their laptop screen and the setup page on
    /// the external display.
    #[test]
    fn a_code_on_the_second_monitor_is_found_as_readily_as_one_on_the_first() {
        // The code is only on the second, and the second is the smaller: a
        // scan that read a bounding box would clamp to the first and miss it.
        fn only_on_the_small_one(_: &[u8], w: usize, _: usize) -> qr::Codes {
            if w == 1280 {
                qr::Codes::One(Zeroizing::new("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP".into()))
            } else {
                qr::Codes::None
            }
        }
        let monitors = [rect(0, 0, 1920, 1080), rect(1920, 0, 3200, 720)];
        assert!(matches!(
            scan_screen_with(&scan_seams(flat_capture, only_on_the_small_one), &monitors),
            ScreenScan::Found(_)
        ));
        // The control on the same seam: with only the large monitor plugged
        // in there is nothing to find, so the pass above is about the second
        // monitor being read and not about the stub answering everything.
        assert_eq!(
            miss(scan_screen_with(
                &scan_seams(flat_capture, only_on_the_small_one),
                &monitors[..1]
            )),
            ScanMiss::NoCode
        );
    }

    /// **A desktop with no code on it falls back to the drag, and says so.**
    #[test]
    fn a_desktop_with_no_code_falls_back_to_the_drag() {
        let monitors = [rect(0, 0, 1920, 1080), rect(1920, 0, 3840, 1080)];
        assert_eq!(
            miss(scan_screen_with(&scan_seams(flat_capture, no_codes), &monitors)),
            ScanMiss::NoCode
        );
    }

    /// **Two different codes are not guessed between.**
    ///
    /// The owner's rule, and the one behaviour here that is a refusal on
    /// purpose rather than a failure: two codes on a desktop are two secrets,
    /// and the app has no basis for preferring either. Both halves are
    /// asserted -- two DIFFERENT codes are `Several`, and the same code on
    /// two monitors is one code, which is what stops a mirrored or duplicated
    /// display from making this feature refuse to work at all.
    #[test]
    fn two_different_codes_are_refused_but_the_same_code_twice_is_not() {
        // Different sizes, so `code_naming_the_size` yields different
        // payloads.
        let different = [rect(0, 0, 1920, 1080), rect(1920, 0, 3200, 1800)];
        assert_eq!(
            miss(scan_screen_with(&scan_seams(flat_capture, code_naming_the_size), &different)),
            ScanMiss::Several
        );

        // Same size, so the same payload -- one code seen twice.
        let same = [rect(0, 0, 1920, 1080), rect(1920, 0, 3840, 1080)];
        assert!(matches!(
            scan_screen_with(&scan_seams(flat_capture, code_naming_the_size), &same),
            ScreenScan::Found(_)
        ));

        // And one monitor that held two on its own settles it by itself, with
        // no second monitor needed.
        assert_eq!(
            miss(scan_screen_with(&scan_seams(flat_capture, several_codes), &same[..1])),
            ScanMiss::Several
        );
    }

    /// **A capture that refuses is reported in the capture's own words** --
    /// but only when nothing anywhere could be read.
    #[test]
    fn a_desktop_that_cannot_be_captured_at_all_reports_why() {
        let monitors = [rect(0, 0, 1920, 1080)];
        assert_eq!(
            miss(scan_screen_with(&scan_seams(refusing_capture, code_naming_the_size), &monitors)),
            ScanMiss::Refused(CaptureRefusal::Blocked)
        );
        // No monitors at all is `OffScreen`, the same answer `open` gives for
        // the same desktop.
        assert_eq!(
            miss(scan_screen_with(&scan_seams(flat_capture, code_naming_the_size), &[])),
            ScanMiss::Refused(CaptureRefusal::OffScreen)
        );
        // A degenerate monitor is skipped rather than reported: it would
        // refuse as `TooSmall`, and telling the user their screen is too
        // small would describe a monitor they do not have.
        assert_eq!(
            miss(scan_screen_with(
                &scan_seams(flat_capture, no_codes),
                &[rect(0, 0, 1, 1), rect(10, 10, 20, 20)]
            )),
            ScanMiss::NoCode
        );
    }

    /// **A monitor too large to decode does not sink the scan.**
    ///
    /// `qr::MAX_PIXELS` is a real bound and `clamp_to_monitors` applies it per
    /// monitor, so this goes through the production check rather than a
    /// number written out again here. Alone, such a monitor is the refusal;
    /// beside a readable one, the readable one still answers.
    #[test]
    fn a_monitor_too_large_to_decode_is_refused_without_sinking_the_scan() {
        // 9000 x 9000 is 81 megapixels, past the 64 the decoder will look at.
        let huge = rect(0, 0, 9_000, 9_000);
        assert!(
            huge.area() > qr::MAX_PIXELS as u64,
            "the fixture monitor is no longer past the bound it is here to cross"
        );
        assert_eq!(
            miss(scan_screen_with(&scan_seams(bounded_capture, code_naming_the_size), &[huge])),
            ScanMiss::Refused(CaptureRefusal::TooLarge)
        );
        // Beside a monitor that can be read, the code is still found.
        assert!(matches!(
            scan_screen_with(
                &scan_seams(bounded_capture, code_naming_the_size),
                &[huge, rect(9_000, 0, 10_600, 900)]
            ),
            ScreenScan::Found(_)
        ));
        // Control on `bounded_capture` itself: an ordinary monitor is not
        // refused by it, so the refusal above is about the size.
        assert_eq!(
            miss(scan_screen_with(
                &scan_seams(bounded_capture, no_codes),
                &[rect(0, 0, 1920, 1080)]
            )),
            ScanMiss::NoCode
        );
    }

    /// **The scanned secret never reaches a formatter.**
    #[test]
    fn the_debug_of_a_scan_does_not_print_the_secret() {
        let secret = "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP";
        let shown = format!("{:?}", ScreenScan::Found(Zeroizing::new(secret.to_string())));
        assert!(!shown.contains("JBSWY3DPEHPK3PXP"), "{shown}");
        assert!(!shown.contains("otpauth"), "{shown}");
        assert!(shown.contains("not shown"), "{shown}");
        assert_eq!(
            format!("{:?}", ScreenScan::Missed(ScanMiss::Several)),
            "Missed(Several)"
        );
    }

    /// **What the bar says, for each way the scan can fail to answer.**
    ///
    /// Every one of them names the drag, and no two of them name it the same
    /// way: "the one you want" is a real choice only when there is more than
    /// one code, and offering it to a user who has none would be the generic
    /// refusal this crate keeps refusing to write.
    #[test]
    fn the_bar_says_why_it_opened() {
        assert_eq!(
            scan_miss_line(ScanMiss::NoCode),
            "No QR code found on your screen. Drag a box around it instead."
        );
        assert_eq!(
            scan_miss_line(ScanMiss::Several),
            "More than one QR code is on your screen. Drag a box around the one you want."
        );
        // A refusal keeps the capture's own headline rather than a third
        // sentence invented here.
        assert_eq!(
            scan_miss_line(ScanMiss::Refused(CaptureRefusal::Blocked)),
            "Screen capture is blocked. Drag a box around the code instead."
        );
        for why in [
            CaptureRefusal::OffScreen,
            CaptureRefusal::TooSmall,
            CaptureRefusal::TooLarge,
            CaptureRefusal::GdiFailed,
            CaptureRefusal::Blocked,
        ] {
            let line = scan_miss_line(ScanMiss::Refused(why));
            assert!(line.starts_with(why.title()), "{line}");
            assert!(line.ends_with(SCAN_REFUSED_ADVICE), "{line}");
        }
        // Each of the three says the drag, and only the one with a choice in
        // it offers one.
        for miss in [
            ScanMiss::NoCode,
            ScanMiss::Several,
            ScanMiss::Refused(CaptureRefusal::GdiFailed),
        ] {
            let line = scan_miss_line(miss);
            assert!(line.contains("Drag a box"), "{miss:?} does not name the fallback: {line}");
            assert!(line.ends_with('.'), "{line}");
        }
        assert!(scan_miss_line(ScanMiss::Several).contains("the one you want"));
        assert!(!scan_miss_line(ScanMiss::NoCode).contains("the one you want"));
        assert!(!scan_miss_line(ScanMiss::Refused(CaptureRefusal::Blocked))
            .contains("the one you want"));
    }

    /// **A scan that found a code ends the overlay on 6c; one that did not
    /// leaves it up with a reason.**
    ///
    /// The two halves of `apply_scan`, and the second is the one that matters
    /// for the surface: the overlay stays open, the drag still works, and the
    /// bar now has something to say.
    #[test]
    fn a_scan_either_answers_or_explains_itself() {
        let found = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        found.apply_scan(ScreenScan::Found(Zeroizing::new("otpauth://totp/x".into())));
        assert!(!found.is_open(), "a code did not end the overlay");
        assert!(matches!(found.take_outcome(), Some(Outcome::Decoded(_))));

        for missed in [
            ScanMiss::NoCode,
            ScanMiss::Several,
            ScanMiss::Refused(CaptureRefusal::GdiFailed),
        ] {
            let overlay = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
            overlay.apply_scan(ScreenScan::Missed(missed));
            assert!(overlay.is_open(), "{missed:?} closed the overlay");
            assert!(overlay.take_outcome().is_none(), "{missed:?} recorded an outcome");
            assert_eq!(overlay.reason(), Some(missed));
            assert_eq!(overlay.view().reason, Some(missed));
        }

        // A rescan that finds nothing clears a stale lock-on: "Code found"
        // must not stay over a rectangle that has not been re-read.
        let overlay = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        let t0 = Instant::now();
        overlay.advance(&seams(flat_capture, a_code), Some((10.0, 10.0)), true, t0);
        overlay.advance(
            &seams(flat_capture, a_code),
            Some((300.0, 300.0)),
            true,
            t0 + DECODE_INTERVAL,
        );
        assert!(overlay.view().found, "control: it locked on");
        overlay.apply_scan(ScreenScan::Missed(ScanMiss::NoCode));
        assert!(!overlay.view().found, "the stale lock-on survived a rescan");
    }

    /// **The scan runs once, and then never again.**
    ///
    /// The property the module was fixed for once already: a repaint must not
    /// be able to start another capture. Every transition here is forwards,
    /// `Done` is absorbing, and a hundred further frames after it ask for
    /// nothing.
    #[test]
    fn the_screen_is_scanned_once_and_the_state_never_goes_back() {
        let overlay = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        let t0 = Instant::now();
        // The first frame masks and asks to be called back.
        assert_eq!(overlay.prescan_step(t0), PrescanStep::Mask);
        // Frames inside the settle wait, and say how long is left.
        assert_eq!(
            overlay.prescan_step(t0 + Duration::from_millis(1)),
            PrescanStep::Settling(PRESCAN_SETTLE - Duration::from_millis(1))
        );
        assert_eq!(
            overlay.prescan_step(t0 + PRESCAN_SETTLE - Duration::from_millis(1)),
            PrescanStep::Settling(Duration::from_millis(1))
        );
        // Exactly the settle is enough -- the bound is "at least", as the
        // decode throttle's is.
        assert_eq!(overlay.prescan_step(t0 + PRESCAN_SETTLE), PrescanStep::Scan);
        // And from there, nothing. Not on the next frame, not an hour later,
        // not with the clock going backwards.
        for after in [0_u64, 1, 16, 5_000, 3_600_000] {
            assert_eq!(
                overlay.prescan_step(t0 + PRESCAN_SETTLE + Duration::from_millis(after)),
                PrescanStep::Done,
                "the scan was asked for again {after} ms later"
            );
        }
        assert_eq!(overlay.prescan_step(t0), PrescanStep::Done);
    }

    /// A clock that never reaches the deadline never scans -- which is the
    /// other half of the bound: the settle is a wait, not a spin that gives
    /// up and captures anyway.
    #[test]
    fn the_settle_is_a_wait_and_not_a_countdown_of_frames() {
        let overlay = RegionOverlay::open(&[rect(0, 0, 800, 600)], 1.0).expect("opens");
        let t0 = Instant::now();
        assert_eq!(overlay.prescan_step(t0), PrescanStep::Mask);
        for _ in 0..200 {
            assert!(matches!(
                overlay.prescan_step(t0 + Duration::from_millis(1)),
                PrescanStep::Settling(_)
            ));
        }
        assert_eq!(overlay.prescan_step(t0 + PRESCAN_SETTLE), PrescanStep::Scan);
    }

    /// Production's settle is the documented one, and it is a real wait
    /// rather than zero -- which would be the same race the constant exists
    /// to avoid.
    #[test]
    fn the_production_settle_is_the_documented_one() {
        assert_eq!(PRESCAN_SETTLE, Duration::from_millis(80));
        assert!(PRESCAN_SETTLE > Duration::ZERO);
        // Short enough that a user does not sit through it: it is under the
        // same fifth of a second the lock-on interval is argued against.
        assert!(PRESCAN_SETTLE < Duration::from_millis(200));
    }

    /// **A press on a chip is that chip's, and does not also become a
    /// drag.**
    ///
    /// The defect this prevents is specific: this surface reads the raw
    /// pointer, so a press taken only on release would already have started a
    /// selection on the way down, and the release would then read the
    /// one-pixel rectangle under the chip and end the overlay with "that
    /// region is too small".
    #[test]
    fn a_press_on_a_chip_belongs_to_the_chip_for_the_whole_gesture() {
        let overlay = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        let whole = rect_pts(1500.0, 1000.0, 1620.0, 1028.0);
        let cancel = rect_pts(1630.0, 1000.0, 1720.0, 1028.0);
        locked(&overlay.inner).chips = [whole, cancel];

        // Down on the whole-screen chip: nothing yet, and the overlay knows
        // the gesture is the chip's.
        assert_eq!(overlay.chip_gesture(Some((1550.0, 1010.0)), true, true), None);
        assert!(overlay.in_chip_press());
        // Held: still nothing.
        assert_eq!(overlay.chip_gesture(Some((1552.0, 1012.0)), false, true), None);
        assert!(overlay.in_chip_press());
        // Up, over the same chip: the press lands.
        assert_eq!(
            overlay.chip_gesture(Some((1552.0, 1012.0)), false, false),
            Some(CHIP_WHOLE_SCREEN)
        );
        assert!(!overlay.in_chip_press(), "the gesture was not released");

        // The other chip, by its own index.
        assert_eq!(overlay.chip_gesture(Some((1680.0, 1010.0)), true, true), None);
        assert_eq!(
            overlay.chip_gesture(Some((1680.0, 1010.0)), false, false),
            Some(CHIP_CANCEL)
        );

        // Pressed on a chip, released off it: nothing happens, which is how
        // a user takes back a press they did not mean.
        assert_eq!(overlay.chip_gesture(Some((1550.0, 1010.0)), true, true), None);
        assert_eq!(overlay.chip_gesture(Some((400.0, 400.0)), false, false), None);
        assert!(!overlay.in_chip_press());

        // A press anywhere else is not a chip's at all, so the drag gets it.
        assert_eq!(overlay.chip_gesture(Some((400.0, 400.0)), true, true), None);
        assert!(!overlay.in_chip_press(), "an ordinary drag was swallowed as a chip press");
        assert_eq!(overlay.chip_gesture(Some((500.0, 500.0)), false, false), None);

        // And with no chips painted yet -- the first frame -- a press hits
        // none of them, because `Rect::NOTHING` contains no point.
        let fresh = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        assert_eq!(fresh.chip_gesture(Some((1550.0, 1010.0)), true, true), None);
        assert!(!fresh.in_chip_press());
    }

    /// The bar grows by the reason line rather than the instruction moving to
    /// make room for it, so 6b's own two lines sit where they always did
    /// relative to each other.
    #[test]
    fn the_reason_line_makes_the_bar_taller_rather_than_displacing_the_instruction() {
        let full = rect_pts(0.0, 0.0, 1920.0, 1080.0);
        // Three lines of type at the design's gap, against two.
        let two = bar_rect(full, 20.0 + BAR_LINE_GAP + 18.0);
        let three = bar_rect(full, 15.0 + BAR_LINE_GAP + 20.0 + BAR_LINE_GAP + 18.0);
        assert!(three.height() > two.height(), "the reason line did not make room for itself");
        assert_eq!(three.height() - two.height(), 15.0 + BAR_LINE_GAP);
        assert_eq!(three.bottom(), full.bottom(), "the bar left the bottom edge");
        assert_eq!(BAR_REASON_PX, BAR_HINT_PX);
        assert_eq!(BAR_REASON_INK, theme::BLUE_SOFT);
    }

    /// **The vault window is masked for the scan and put back afterwards.**
    ///
    /// The mask is a `SetWindowDisplayAffinity` on a real window and cannot be
    /// asserted from here -- there is no window in a test process. What can be
    /// asserted is the bookkeeping that decides when the OS call happens: it
    /// is idempotent, so the calls land on the transitions, and `Inner`'s
    /// `Drop` is what guarantees the mask comes off for the paths that do not
    /// run through `show`.
    #[test]
    fn the_mask_is_bookkept_so_that_it_always_comes_back_off() {
        let overlay = RegionOverlay::open(&[rect(0, 0, 800, 600)], 1.0).expect("opens");
        assert!(!locked(&overlay.inner).masked, "a fresh overlay has masked nothing");
        // `mask_own_window` is not called here: it would reach Win32. What is
        // asserted is that the flag is what gates it, and that `Drop` reads
        // the same flag.
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(
            code.contains("if held.masked == on {"),
            "the mask is no longer idempotent, so the OS call no longer lands on transitions"
        );
        assert!(
            code.contains("impl Drop for Inner {") && code.contains("if self.masked {"),
            "nothing puts the vault window back into screen captures when the overlay is \
             dropped rather than closed"
        );
        // `show` unmasks on all three of its ways out -- the guard that
        // finds a finished overlay, the scan that answered before a window
        // existed, and the frame the overlay ends on -- and `Drop` is the
        // backstop under all of them.
        assert_eq!(
            code.matches("self.mask_own_window(false);").count(),
            3,
            "`show` no longer puts the vault window back on every way out of it"
        );
        assert_eq!(
            code.matches("self.mask_own_window(true);").count(),
            1,
            "the vault window is masked from somewhere other than the one scan that needs it"
        );
        // And the window it masks is the vault window, which is the one the
        // modal that started this is drawn in.
        assert_eq!(crate::vault_window::WINDOW_TITLE, "Deskwarden");
        assert_ne!(crate::vault_window::WINDOW_TITLE, REGION_TITLE);
    }

    /// **The scan is decode-only: nothing paints it.**
    ///
    /// The one security property of this feature that is a *negative* about
    /// the UI rather than about the buffers. A frozen desktop would make the
    /// dim composite perfectly and would put a picture of the user's whole
    /// screen into `egui`'s texture manager, which this crate cannot wipe.
    /// The overlay stays transparent over the live desktop instead.
    #[test]
    fn nothing_in_this_module_uploads_a_capture_to_a_texture() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        for needle in ["load_texture", "TextureHandle", "ColorImage", "tex_manager"] {
            assert!(
                !code.contains(needle),
                "`{needle}` appears in this module: a capture of the user's screen must not \
                 reach `egui`'s texture manager, which holds allocations this crate cannot wipe"
            );
        }
        // Positive control: the capture really is in this file, so the
        // absences above are about textures and not about an empty haystack.
        assert!(code.contains("(seams.capture)(*monitor)"));
        // And it is dropped rather than kept.
        assert!(code.contains("drop(pixels);"));
    }
}

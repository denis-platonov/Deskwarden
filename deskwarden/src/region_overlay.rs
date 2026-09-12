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
//! * **exactly one code** -- the overlay opens for less than half a second,
//!   rings the code **where it actually is on the user's screen** with 6b's
//!   own blue ring, brackets and tick, and closes. The outcome is
//!   [`Outcome::Decoded`], and the caller lands on the same 6c confirmation
//!   card every other route lands on. **Nothing is saved**: 6c holds the code
//!   and its countdown, and Save is a press the user makes.
//!
//!   That half-second is [`REVEAL_DWELL`] and it is there because the route
//!   shipped without it and was worse: the press did something invisible and
//!   then a card appeared holding the user's secret, with no account of where
//!   it had come from. The owner asked for the missing half in these words --
//!   *"when captured - draw blue line around QR code and V sign like in
//!   design, so user sees it captured automatically"*. It is a statement and
//!   not a screen: no bar, no chips, no input read, and it ends on a clock
//!   rather than on anything the user does. See [`Reveal`].
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
//! * [`Drag::rect`] -- the geometry that is this module's rather than
//!   `screen_capture`'s;
//! * [`to_screen`] -- the one conversion this window owns, points to
//!   virtual-screen pixels;
//! * [`lockon_badge`] -- the found/not-found label decision;
//! * [`DecodeThrottle`] -- the bound on how often a decode is attempted;
//! * [`read_region_with`] -- every outcome, through seams;
//! * [`scan_screen_with`] -- one code, none, several, a monitor that refused
//!   and a monitor too large to decode, all through the same seams;
//! * [`RegionOverlay::prescan_step`] -- that the scan runs **once** and then
//!   never again, driven by a clock a test supplies;
//! * [`RegionOverlay::reveal_step`] -- that the reveal starts once, ends on
//!   the clock and never restarts, and that the code's box arrives in the
//!   right place on a monitor whose origin is not `(0, 0)`;
//! * [`RegionOverlay::chip_gesture`] -- that a press on a bar chip belongs to
//!   that chip and does not also become a one-pixel drag.
//!
//! **Not testable, and no assertion below pretends otherwise:**
//!
//! * that Windows grants this window the foreground when it opens. The raise
//!   is asked for; the OS may refuse it and flash a taskbar button instead.
//! * that what the user sees is the picture with the dim over it. The window
//!   is opaque and asks the compositor for nothing, which is the end of a
//!   history in which three compositing mechanisms were accepted by DWM and
//!   were wrong on the glass, and in which a capture-based probe could not
//!   tell -- [`RegionOverlay::take_picture`] has the whole of it. What is
//!   asserted is that the picture is taken before the window exists, that the
//!   window is out of captures before it is shown, and that it is shown only
//!   once it has been painted -- see [`Appearing`], which carries the
//!   measurement of why: the window used to be on screen for 1315 ms before
//!   anything had painted it.
//! * that the viewport covers **the display the user is on**. The rectangle
//!   handed to the builder is [`crate::screen_capture::active_display`]'s
//!   answer, and that choice is tested there as a pure function -- but a
//!   `ViewportBuilder`'s position and size are a request, and everything
//!   between here and `CreateWindowExW` may renegotiate it. That is a fact
//!   about a real desktop and a real window manager, so it is *measured*
//!   rather than asserted: [`log_window_rect`] writes the rectangle the window
//!   really got beside the one it asked for, on the frame before it is shown
//!   and on the frame it is shown, and says so loudly when they differ. On a
//!   single 5120x1440 display the two agree exactly and the window is created
//!   hidden at its final rectangle and shown once -- 0 samples at any other
//!   size, sampled every 17 ms.
//! * that the overlay itself is excluded from the blit, and that the vault
//!   window is out of the scan's way in time. See [`exclude_from_capture`]
//!   and [`PRESCAN_SETTLE`].
//!
//! Every mechanism here is a *necessary condition* for those, never a proof
//! of them.
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
//! to the same rules.** Each monitor's pixels live in an
//! [`crate::screen_capture::Rgba`] that wipes on drop and dies inside
//! [`scan_screen_with`] before the next monitor is read; nothing is written,
//! logged, or handed out.
//!
//! **One capture IS displayed, and this paragraph used to say the opposite.**
//! The rule was that no capture is ever uploaded to an `egui` texture, because
//! the texture manager holds allocations this crate cannot wipe. It was
//! reversed deliberately, after every way of showing the *live* desktop
//! through this window had been accepted by the compositor and been wrong on
//! the owner's monitor -- [`RegionOverlay::take_picture`] carries that history
//! and the log line that ended it. What the reversal permits is exactly one
//! picture, of the one display this overlay covers, taken before the window
//! exists and released on the frame the overlay closes; what it costs is
//! stated there rather than hidden: the `Rgba` still wipes, but the
//! `ColorImage` egui builds from it and the texture on the GPU are freed, not
//! wiped, when the overlay ends. The pixels in that picture are the pixels on
//! the user's monitor at that moment, and the window that shows them is
//! excluded from captures. `the_desktop_picture_lives_exactly_as_long_as_the_overlay`
//! pins the bounds.

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

/// **The reveal's badge**: what the overlay says on the half-second it spends
/// showing the user the code its scan just read.
///
/// **Not [`LOCKED_ON`], and the difference is the whole point.** That
/// sentence ends in an instruction -- *release to read* -- and on the reveal
/// there is nothing to release and nothing left to do: the code has already
/// been read, the button was never down, and the next thing the user sees is
/// 6c. A badge that told them to release would be asking for a gesture that
/// does not exist.
///
/// The words are **design 6c's own card header**, verbatim. That is
/// deliberate rather than convenient: the last thing this surface says and
/// the first thing the card says are then the same two words, so the handoff
/// between a window that vanishes and a card that appears reads as one event
/// instead of two. It is also the shortest true sentence available, which
/// matters on a badge that is on screen for less time than it takes to read a
/// long one.
pub const SCAN_FOUND: &str = "Code read";

/// 6b's two shortcut affordances, at the right-hand end of the bottom bar.
/// Each is a bordered chip carrying its label and, in the design's monospace,
/// the key that does the same thing.
///
/// # It says *All screens*, and it used to say *Whole screen*
///
/// **This is the one word the surface gains for the overlay narrowing to a
/// single display,** and it is the whole of what was needed there.
///
/// The overlay now covers the active display and nothing else -- see
/// [`RegionOverlay::open`] -- so a code sitting on the user's other monitor
/// can no longer be dragged over. That is the owner's explicit instruction and
/// is implemented as asked, but it leaves a user who *does* have a code on the
/// other screen looking at a dim they cannot extend, and a surface that offers
/// no way out of that is a surface that has stopped being the fallback it
/// exists to be.
///
/// The way out was already on the bar: [`scan_screen_with`] walks **every**
/// monitor and always has, and this chip and the `A` key are what run it on
/// demand. What was wrong was only the label. *"Whole screen"* on a surface
/// that now covers one screen reads as "the whole of this screen" -- it
/// describes the dim, and the dim is no longer the thing it does. *"All
/// screens"* describes the scan, which is what it really runs, and in doing so
/// tells the user on the surface itself that there is a control here that
/// reaches the monitor the dim does not.
///
/// No monitor picker, and none was considered for long: a chooser is a second
/// decision to put in front of someone who is trying to add a two-factor code,
/// and the scan that walks every screen answers the same question without one.
/// A code the scan cannot see on another monitor -- one it misses twice -- is
/// a residual gap, and the honest fix for it is to move the code onto the
/// screen the user is working on, which is what they would do anyway.
pub const WHOLE_SCREEN_HINT: &str = "All screens";
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

/// How dark the desktop goes outside the selection: the app's ink at 45%, so
/// the user's own screen comes through at 55%.
///
/// The colour is the design's and is not in question -- `#201e1d`, the app's
/// ink, rather than black, because a black wash reads as a colder, flatter dim
/// than this one and carries none of the product's warmth. The selection
/// itself is left entirely unwashed, which is what "stays lit" means over a
/// picture of the desktop. What changed is the **weight**.
///
/// # This is a deliberate departure from design 6b, asked for by the owner
///
/// *"don't do hard overlay - needs to be transparent enough"*. It is worth
/// being exact about what is being departed from, because the obvious reading
/// -- that the old value was a misreading of the design and this is the fix --
/// is wrong, and somebody will "fix" it back if the record does not say so.
///
/// **The design really does specify the heavy value.** `Deskwarden.dc.html`
/// under `id="6b"` draws its mock as a container at `background: #201e1d` with
/// the fake desktop inside it at `opacity: 0.32`. Over a real desktop that is
/// the same arithmetic as painting the ink at `1 - 0.32` -- `0.68 * 255`,
/// rounded, which is 173. That was checked against the file rather than
/// remembered, because an earlier pass read the two the other way round and
/// the question of which of them was wrong was worth settling. Neither: 173
/// was faithful.
///
/// **It was faithful and still wrong in the hand, and the mockup itself says
/// why.** The desktop the design fakes behind the dim is `#ffffff` chrome over
/// an `#f7f6f5` page -- a bright white screen. A real desktop is mostly darker
/// than that, so the same 68% of ink that reads as a soft scrim over white
/// reads as near-black over the screen a user actually has. The design
/// measured its wash against the one background that flatters it.
///
/// **And nobody could have noticed until the desktop first came through.**
/// Before that this viewport was opaque and cleared to near-black, so whatever
/// this number was, it was being composited over black and the result was
/// black. The first build in which the desktop was visible behind the wash was
/// the first in which the value was judgeable at all -- and the first person
/// to judge it said it was too heavy.
///
/// # Why 115
///
/// `115 / 255` is 0.451, so the desktop comes through at **55%** rather than
/// the design's 32%. Bounded on both sides, and neither bound is taste:
///
/// * Below, by what the dim is *for*. It has one job -- to make the unpainted
///   selection read as lit, so the user can see what they have framed. That
///   needs a contrast step at the selection's edge that survives whatever is
///   behind it, and a wash much under 40% stops being a step at all on a busy
///   desktop. It also has to stay dark enough to carry
///   [`BAR_HINT_INK`]'s grey and [`BAR_TITLE_PX`]'s white, which are type for
///   a dark ground.
/// * Above, by the complaint. At 68% the desktop is a rumour; at 55% it is a
///   screen someone can read a window title off, which is what "transparent
///   enough" means for a surface whose whole purpose is to point at something
///   already on it.
///
/// # Where it is applied: the framebuffer, over a picture of the desktop
///
/// **This is an alpha in the framebuffer again**, and for the last time. It
/// has been three other things: a per-pixel alpha the compositor was asked to
/// honour (blurred), a per-pixel alpha with the frame extended (blurred), and
/// the `bAlpha` of a layered window (accepted, and the window was opaque on
/// the glass -- [`RegionOverlay::take_picture`] has the log line). The window
/// is now opaque on purpose. It paints a picture of the display it covers and
/// lays [`dim_wash`] -- [`DIM_INK`] at this alpha -- over the parts of it that
/// are not the selection. 55% of the desktop comes through where the wash
/// lies, and 115 is still the owner's own number.
///
/// What the move back gives, which uniform alpha could not: the selection is
/// a **hole** in the wash again. The pixels the user has framed are the
/// picture at full strength, which is what 6b draws and what the owner asked
/// for in the words *"drag a box completely black and user cannot find where
/// to put the box"*. And the bar's plate composes over the wash and is a plate
/// again -- see [`BAR_BG_ALPHA`].
///
/// It stays **one** constant, used in exactly one place: [`dim_wash`].
/// `the_dim_is_painted_in_the_framebuffer_over_a_picture_of_the_desktop` is
/// what keeps that true.
pub const DIM_ALPHA: u8 = 115;

/// **The wash itself**: [`DIM_INK`] at [`DIM_ALPHA`], the one translucent
/// fill that makes the dim. A function rather than a constant only because
/// `Color32::from_rgba_unmultiplied` is not `const`; it is the same value on
/// every call.
pub fn dim_wash() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(DIM_INK.r(), DIM_INK.g(), DIM_INK.b(), DIM_ALPHA)
}

/// **The dim's ink**: the design's `background: #201e1d`. Opaque here, and
/// applied at [`DIM_ALPHA`] by [`dim_wash`], which is the only form in which
/// it reaches the surface -- except when there is no picture to wash, where
/// [`draw`] fills the window with it solid as the stated fallback for a
/// capture that was refused.
pub const DIM_INK: egui::Color32 = egui::Color32::from_rgb(0x20, 0x1e, 0x1d);

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
///
/// # A plate again, because the alphas stack again
///
/// This composes over the wash, on a picture, in one framebuffer, so the two
/// alphas stack: `1 - (1 - 0.451)(1 - 0.922)` is **0.957**, and the bar
/// reaches the display 96% opaque while the dim around it is 45%. That is
/// where the bar's readability comes from -- [`BAR_HINT_INK`]'s `#bab6b6` on
/// this ground is about **8.4:1**, and the white title higher -- and it is
/// the second thing the picture gave back. On the layered window that
/// preceded it the compositor applied one alpha to the finished surface, the
/// plate landed on the dim at the dim's own 45%, and the hint fell to about
/// 2.6:1 with the desktop showing through the type. Recorded so that the
/// number is not "tuned" for a problem that no longer exists.
///
/// The number is the design's 0.92 and has not moved through any of it.
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
///
/// **Re-examined twice, and left alone both times -- but the second answer is
/// weaker than the first and is written down as such.**
///
/// The first re-examination was when [`DIM_ALPHA`] was lightened from the
/// design's 68% to 45%. The worry was whether an ink picked for a near-black
/// ground survives the ground lightening, and the premise turned out to be
/// false: this ink was never on the wash. [`paint_bar`] laid the design's own
/// plate down first, and the type went on that.
///
/// The second was the move to a **layered window**, where the worry was real:
/// every colour in the framebuffer reached the display at one uniform
/// [`DIM_ALPHA`] and the contrast fell by about a factor of three. That
/// mechanism is gone -- the window is opaque and paints a picture of the
/// desktop, see [`RegionOverlay::take_picture`] -- and the plate under this
/// ink is a near-opaque plate again ([`BAR_BG_ALPHA`]), so the first answer
/// stands: this ink was picked for a dark ground, and it sits on one.
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

/// Converts a pointer position **in egui points, relative to this viewport**
/// into virtual-screen physical pixels -- the space
/// [`crate::screen_capture`] and every GDI call speak.
///
/// `origin` is the viewport's top-left in those same pixels -- the top-left
/// of the one display [`crate::screen_capture::active_display`] chose, which is
/// where [`RegionOverlay::open`] put the window. `points_per_pixel` is
/// `Context::pixels_per_point`.
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

/// **How long Deskwarden's own window takes to leave the screen after it has
/// been sent down, and therefore how long the reveal waits before starting its
/// clock.**
///
/// # What this is a settle for, which is not what it was expected to be
///
/// The obvious place to put the minimise was the `Mask` frame, beside
/// [`exclude_from_capture`] -- window out of the capture and off the screen in
/// one breath, [`PRESCAN_SETTLE`] lengthened to cover both, capture after.
/// **That design is fatal and was measured to be**, which is the whole reason
/// this is a second constant rather than a bigger first one.
///
/// Between `Mask` and `Scan` this overlay has registered **no viewport at
/// all**: [`RegionOverlay::show`] returns early on those frames, so the root
/// is an eframe application with exactly one window and that window is
/// iconic. Measured with a probe that minimises such a root and counts its
/// frames for the next five seconds: it takes **none**. Not a throttled
/// trickle -- the last frame is the one that issued the minimise, and nothing
/// follows it, however hard that frame asks for a repaint. A settle deadline
/// set on a frame that is the last frame is a deadline nothing will ever
/// reach, and a route that never reaches it is the hang this module was
/// already fixed for once.
///
/// So the window goes down on the frame that **raises** the overlay instead --
/// the second of [`Appearing`]'s two steps, one root frame after the one that
/// composites the window and asks for it to be shown -- which is the first
/// moment a live child viewport exists *and is visible*. It used to be the
/// overlay's own first painted frame, and moved with the white-box fix; the
/// guarantee got stronger rather than weaker, because "registered" became
/// "on screen". The same probe, minimising
/// there, measures the root going on at about 8 frames a second and this
/// overlay at twice that, indefinitely. That is `eframe`'s deliberate
/// `INVISIBLE_WINDOW_REPAINT_INTERVAL` throttle of a window Windows sends no
/// `WM_PAINT` to, and it is a real cost paid by the drag fallback; it is not a
/// freeze.
///
/// Which leaves one thing that genuinely has to wait, and it is not the scan's
/// capture -- that one is protected by the mask, set two frames earlier, which
/// is exactly the belt this is braces to. It is the **reveal**. The reveal
/// exists to show the user the code on their own screen, and it is issued on
/// the same frame as the minimise: without a wait, the first part of
/// [`REVEAL_DWELL`] would be spent ringing a code behind the window that is
/// still on its way down. That is the argument [`Reveal::Due`] already makes
/// about a window that does not exist yet, applied to a window that has not
/// gone yet.
///
/// # Why 120 ms
///
/// Measured, on a real desktop, by minimising a window filled with a colour
/// nothing else is and capturing its own rectangle exactly once, at one delay,
/// per process run -- one shot rather than a sampling loop, because a
/// `capture_rect` loop is a `BitBlt` off the screen DC every few milliseconds
/// and three earlier versions of this measurement measured the probe instead
/// of the minimise.
///
/// Across eight runs at requested delays from 0 to 320 ms, the window was
/// fully present in the frame before the call (`filled = 1.000` every time)
/// and **completely absent at every delay tried**, including the shortest the
/// instrument can reach -- about 60 ms after `ShowWindow` returns, which is a
/// floor set by a thread wake plus the ~30 ms a capture of that size costs,
/// not by the compositor. `ShowWindow` itself returned in 4-6 ms.
///
/// So the honest reading is "gone within 60 ms, and probably much sooner",
/// and 60 ms is an upper bound on a number this instrument cannot resolve
/// rather than the number. 120 ms is twice that: margin for a machine slower
/// than the one it was measured on, and comfortably more than
/// [`PRESCAN_SETTLE`]'s 80 ms, which buys two composes at 30 Hz on the same
/// reasoning.
///
/// It is also small enough not to matter. It is spent once, in front of a
/// 450 ms reveal, at the end of a route whose scan is a full binarisation of
/// every monitor -- so it moves the code onto the user's screen a fifth of a
/// second later in exchange for the code being the thing they can actually
/// see when it arrives.
///
/// **Not a proof**, on the same terms as [`PRESCAN_SETTLE`]: nothing in this
/// crate can assert the compositor acted. See the module header's list.
pub const MINIMISE_SETTLE: Duration = Duration::from_millis(120);

/// **How long the overlay shows the user the code it just found, before it
/// closes and 6c appears.**
///
/// # Why there is a pause at all
///
/// The scan shipped without one and the route was worse for it. The user
/// presses *Scan the code on my screen*, nothing visible happens for as long
/// as the decode takes, and then a card appears with their secret already in
/// it. Every step of that is correct and the middle of it is invisible, so
/// the only account the user has of where the code came from is that the app
/// says so. The owner asked for the missing half in these words: *"when
/// captured - draw blue line around QR code and V sign like in design, so
/// user sees it captured automatically"*. The point is not decoration; it is
/// that the app **shows its working** -- there is the code, on your screen,
/// that is the one I read.
///
/// # Why 450 ms
///
/// Bounded on both sides by things that are not taste.
///
/// Below, by what it takes to be *seen* rather than glimpsed. The mark
/// appears wherever the code happens to be, which is not where the pointer is
/// and not where the user was looking -- so the eye has to move to it. A
/// saccade to an unexpected target plus the fixation that follows is around a
/// quarter of a second before anything has been looked at at all, which is
/// also the "reads as immediate" threshold [`DECODE_INTERVAL`] is argued
/// against. A reveal near that number is a flash the user notices and cannot
/// describe. 450 ms is comfortably past it with the badge still on screen.
///
/// Above, by when a fixed wait stops reading as an answer and starts reading
/// as the app thinking. That is somewhere near a second, and 450 ms is under
/// half of it.
///
/// # Why it does not dominate the route
///
/// It must not be the longest thing between the press and the card, or the
/// feature would have been made slower to look faster. It is not: the scan in
/// front of it is [`PRESCAN_SETTLE`] plus a full binarisation and grid search
/// over **every monitor**, which is hundreds of milliseconds on one 4K screen
/// and more on two. The reveal is a fraction of what the user was already
/// waiting through, and it is the only part of that wait with anything on
/// screen.
pub const REVEAL_DWELL: Duration = Duration::from_millis(450);

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
    /// Exactly one distinct code across every monitor, and **where on the
    /// desktop it was** -- in virtual-screen physical pixels, the space every
    /// other rectangle in this module and in [`crate::screen_capture`] is in.
    ///
    /// Not the buffer-relative box [`crate::qr::Codes::One`] carries:
    /// [`scan_screen_with`] moves each monitor's answer onto the desktop
    /// before it folds them together, precisely so that this one value cannot
    /// be in "whichever monitor happened to answer first"'s coordinates. See
    /// [`crate::screen_capture::place_in_capture`].
    ///
    /// The rectangle is what the reveal rings. It is not a secret and does
    /// not wipe -- it is where on their own screen the user's code is.
    Found(Zeroizing<String>, ScreenRect),
    /// It could not answer. See [`ScanMiss`].
    Missed(ScanMiss),
}

impl std::fmt::Debug for ScreenScan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScreenScan::Found(text, at) => {
                write!(f, "Found({} chars not shown, at {at:?})", text.len())
            }
            ScreenScan::Missed(miss) => write!(f, "Missed({miss:?})"),
        }
    }
}

/// **Looks for a QR code on every monitor, and refuses to choose between
/// two.**
///
/// # One monitor at a time, not one bounding box
///
/// **This did NOT narrow when the overlay did, and that is deliberate.**
/// [`RegionOverlay::open`] now puts the window on one display, because a
/// full-desktop window is a surface the user has to be able to dismiss and a
/// geometry Windows renegotiates. None of that applies here: this decodes and
/// opens no window at all, and it is the path that usually succeeds -- it is
/// how the route answers without the user dragging anything. Narrowing it to
/// match the overlay would make the feature worse at its main job in order to
/// fix a complaint about a window. So the scan is every monitor and the
/// overlay is one, the *All screens* chip runs this rather than the overlay's
/// rectangle, and [`WHOLE_SCREEN_HINT`] is named for that.
///
/// A bounding box of the whole desktop would not work here either, which is
/// the older half of this argument and the reason there is a loop at all.
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
        let found = (seams.scan)(pixels.pixels(), width, height);
        // Dropped here explicitly, which is what wipes it -- including on the
        // early exit below, where it would otherwise live to the end of the
        // loop body anyway but where the intent is worth stating.
        drop(pixels);
        // **Placed on the desktop BEFORE it is folded in**, which is the one
        // line in this loop that has to be here and not anywhere else. The
        // box `codes_in` reports is in *this monitor's* pixels; the tally
        // keeps the first sighting and forgets which monitor it came from, so
        // a box folded in unplaced could only be interpreted against a
        // monitor nobody recorded. On the primary monitor the placement is
        // the identity, which is exactly why leaving it out would pass every
        // trial on one screen -- see
        // `crate::screen_capture::place_in_capture`, which is where that
        // argument and its assertions live.
        let found = match found {
            qr::Codes::One(text, at) => {
                qr::Codes::One(text, screen_capture::place_in_capture(*monitor, at))
            }
            other => other,
        };
        let keep_looking = tally.merge(found);
        if !keep_looking {
            break;
        }
    }

    match tally.finish() {
        qr::Codes::One(text, at) => ScreenScan::Found(text, at),
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
    /// **The code the scan found, in points relative to this viewport**, for
    /// as long as the reveal lasts. `Some` means this frame belongs to the
    /// reveal and to nothing else: [`draw`] paints the mark and no bar, and
    /// the viewport callback reads no input.
    ///
    /// Mutually exclusive with `selection` in practice -- a reveal begins
    /// from a scan, and a scan that answers ends the overlay before any drag
    /// can start -- but not by construction, so [`draw`] settles it by taking
    /// the reveal first.
    pub reveal: Option<egui::Rect>,
    /// **The picture of the display this window covers**, as the texture
    /// [`RegionOverlay::take_picture`] uploaded, or `None` when the capture
    /// was refused and the surface falls back to a solid [`DIM_INK`] ground.
    /// [`draw`] paints it first, edge to edge, and everything else over it.
    pub picture: Option<egui::TextureId>,
}

/// Everything the overlay holds, behind the `Arc<Mutex<_>>` that
/// `show_viewport_deferred` requires: the callback it stores is
/// `Fn + Send + Sync + 'static`, so it cannot borrow.
///
/// Nothing here is touched off the UI thread. The mutex is what the signature
/// demands, not a claim about concurrency.
#[derive(Debug)]
struct Inner {
    /// **The one display this overlay covers, resolved once and carried.**
    ///
    /// This used to be an `origin: (i32, i32)` alone, with the *size* read
    /// back out of `screen_capture::monitor_bounds()` on every frame of
    /// [`RegionOverlay::show`]. One rectangle rather than a corner here and an
    /// extent there is not tidiness: the two readings could disagree -- a
    /// monitor unplugged, a resolution changed or a screen rearranged between
    /// the press and the frame moved the size without moving the origin -- and
    /// a window whose position and extent come from different desktops is a
    /// window in the wrong place by construction.
    ///
    /// Resolved in [`RegionOverlay::open`] by
    /// [`crate::screen_capture::active_display`], at the moment the route
    /// starts. It must not be recomputed later: by the time the window is up,
    /// Deskwarden's own window has been minimised and the anchor that chose
    /// this rectangle no longer exists. See that function for the argument
    /// about what "active" means.
    display: ScreenRect,
    points_per_pixel: f32,
    drag: Option<Drag>,
    found: bool,
    throttle: DecodeThrottle,
    /// `None` while the overlay is still up. Set once, by the frame that ends
    /// it.
    outcome: Option<Outcome>,
    /// How far the overlay's own OS window has got towards being on screen.
    /// See [`Appearing`], which replaced a `raised: bool` and carries the
    /// measurement that made it three states rather than one flag.
    appearing: Appearing,
    open: bool,
    /// How far the whole-screen scan has got. See [`Prescan`].
    prescan: Prescan,
    /// Whether the vault window is currently masked out of screen captures.
    /// The flag rather than a second call: `SetWindowDisplayAffinity` is an
    /// OS call, and this is what makes masking and unmasking idempotent and
    /// what [`Inner::drop`] reads to guarantee the mask comes off.
    masked: bool,
    /// Whether Deskwarden's own window has been sent down for the length of
    /// this overlay. A flag for [`Inner::masked`]'s reasons exactly --
    /// `ShowWindow` is an OS call, this makes standing aside and coming back
    /// idempotent, and it is what [`Inner::drop`] reads to guarantee the
    /// window comes back. **Of the two flags this is the one with the
    /// catastrophic failure mode**: a mask left on is a window missing from
    /// screenshots, and a window left down is the user's app gone.
    aside: bool,
    /// When it went down, so the reveal can wait out [`MINIMISE_SETTLE`]
    /// before starting its clock. `None` on an overlay that never stood aside
    /// -- a test's, or one dropped before it ever had a window -- and `None`
    /// means "nothing to wait for", which is true of exactly those.
    aside_at: Option<Instant>,
    /// Why 6b opened. `None` until the scan has answered.
    reason: Option<ScanMiss>,
    /// How far the reveal has got. See [`Reveal`].
    reveal: Reveal,
    /// The bar's two chips in points, as [`draw`] last painted them.
    /// `Rect::NOTHING` before the first paint, which contains no point, so a
    /// press on the frame before there are chips hits none of them.
    chips: [egui::Rect; 2],
    /// Which chip the primary button went down on, while it is still down.
    /// See [`RegionOverlay::chip_gesture`].
    chip_press: Option<usize>,
    /// **Whether the viewport callback has painted a frame into the window
    /// yet.** Set once, by the callback, after its first `draw`; read by
    /// [`RegionOverlay::appear`], which will not show the window until this
    /// is true. See [`Appearing`]'s "shown only once painted" section: this is
    /// the one fact that separates "a frame the user can see" from "a
    /// rectangle of whatever the surface held".
    painted: bool,
    /// **The overlay window's own `HWND`, from the one lookup that found it.**
    ///
    /// Held so that the show -- [`show_painted`], a root frame or more after
    /// the lookup -- costs no second `EnumWindows` in front of the user's first
    /// sight of the surface. Never used to make the window, and only ever used
    /// while the overlay is open, which is the lifetime of the window it
    /// names.
    hwnd: Option<isize>,
    /// **The picture of the display**, uploaded by [`RegionOverlay::take_picture`]
    /// before the window exists and released on the frame the overlay closes.
    /// `None` before the capture, after the release, and when the capture was
    /// refused -- [`draw`] paints a solid [`DIM_INK`] ground in that case.
    ///
    /// A `TextureHandle` frees its texture when dropped, so `Inner`'s `Drop`
    /// covers every ending that does not run through `show`; the explicit
    /// release on the closing frame is so that the texture does not outlive
    /// the window by however long the last clone of this overlay does.
    picture: Option<Picture>,
}

/// The uploaded picture of the display: a [`egui::TextureHandle`] with a
/// `Debug` that names the texture and its size and nothing else.
///
/// The newtype exists for the `Debug`. `TextureHandle` has none, `Inner`
/// derives one, and a hand-written `Debug` is also the right shape for the
/// same reason [`Outcome`]'s is hand-written: what this wraps is a picture of
/// the user's screen, and a derived `Debug` that could ever reach the pixels
/// is one this module does not want to exist.
struct Picture(egui::TextureHandle);

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [width, height] = self.0.size();
        write!(f, "Picture({:?}, {width}x{height})", self.0.id())
    }
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

/// **How far the reveal has got**: the short, fixed stretch during which the
/// overlay rings the code its scan found before closing on 6c.
///
/// **Deliberately the same shape as [`Prescan`]**, for the same reason and
/// with the same one property that matters: every transition is forwards and
/// the last state is absorbing. `Nothing` is where an overlay that has not
/// found anything stays; a scan that finds one code moves it to `Due` **once**
/// and nothing moves it back; the first frame that paints it becomes
/// `Showing` with a deadline; the first frame at or after that deadline is
/// `Done`, which ends the overlay and is never left.
///
/// # Why `Due` exists rather than the deadline being set by the scan
///
/// Because the clock has to start when the user can *see* something, and at
/// the moment the scan answers there is no window: `show` has not registered
/// the viewport yet, so the OS has not created it and egui has not painted
/// it. A deadline set then would spend part -- on a slow first frame,
/// possibly all -- of [`REVEAL_DWELL`] on a window that is not on screen, and
/// the reveal the user got would be shorter than the one that was argued for,
/// by an amount that varies with their machine. `Due` carries the rectangle
/// and no time; the frame that first paints it is the frame that starts the
/// clock.
///
/// # Why this cannot become the hang
///
/// The defect this module was fixed for was a viewport callback that redid
/// work every repaint with no exit. This is the opposite by construction: the
/// only thing that advances it is a clock the caller reads, it advances in one
/// direction, and the state it advances into ends the overlay. A repaint that
/// arrives during `Showing` re-reads the same deadline and either waits or
/// finishes; there is no input it waits for, so a user who walks away still
/// gets 6c. See [`RegionOverlay::reveal_step`].
#[derive(Debug, Clone, Copy)]
enum Reveal {
    /// No code has been found, so there is nothing to show.
    Nothing,
    /// A code is at `at` -- **virtual-screen physical pixels** -- and the
    /// clock has not started.
    Due { at: ScreenRect },
    /// Being shown; the overlay closes on the first frame at or after `until`.
    Showing { at: ScreenRect, until: Instant },
    /// Shown, and the overlay is finished with it.
    Done,
}

/// **How far the overlay's own OS window has got towards being on screen** --
/// which is a different question from whether `egui` has a viewport for it,
/// and the difference is a second and a third of a solid full-screen
/// rectangle.
///
/// # What it replaced, and what that cost
///
/// A `bool` called `raised`, set on the first frame the window was found, with
/// four Win32 calls hanging off it. They ran late, and *late* is the whole
/// defect: `eframe` creates a window out of a frame's viewport output and
/// **shows it immediately**, so between the window appearing and the first
/// frame that paints it, what is on screen is a full-screen always-on-top
/// rectangle of whatever Windows leaves in an unpainted window's redirection
/// surface.
///
/// Measured on this machine, driving the real [`RegionOverlay::open`] and
/// [`RegionOverlay::show`] over a fixed backdrop in another process and
/// sampling a patch of screen every 17 ms:
///
/// ```text
/// t=5939 ms  backdrop     mean=(132.6, 78.3, 121.9)  sd=74.3
/// t=5956 ms  WHITE        mean=(255.0, 255.0, 255.0) sd=0.00   <- the window appears
/// ...        WHITE        80 consecutive samples, no other state between
/// t=7289 ms  (the window's first painted frame takes it out of captures)
/// t=8632 ms  see-through  model error 0.37 see-through, 60.72 opaque-flat
/// ```
///
/// **1315 ms of solid white** -- eighty consecutive samples, first to last,
/// with the next non-white sample 17.8 ms later. That is the owner's *"blinks black, then white
/// box shows up"* -- their machine starts that rectangle black and ends it
/// white, this one only ever showed white -- and in the drag-fallback case it
/// is their *"if no QR the screen is pitch black, no way I can guess where the
/// QR code is"*: a solid screen for well over a second with nothing on it to
/// point at. Their own log has the same shape two seconds wide, because every
/// window lookup on the way to the DWM call is an `EnumWindows`.
///
/// **It was never the transparency.** The steady state measures see-through in
/// every configuration tried: with the capture mask and the minimise both on
/// (mean absolute error **0.37** against a see-through model, **60.72** against
/// an opaque-flat one, correlation 1.0000) and with both off (**0.37** and
/// **59.89**). The DWM call has been landing and working the whole time. What
/// shipped was the window being on screen for a second and a third before it
/// did.
///
/// # The fix is this crate's own, borrowed from [`crate::window_host::Reveal`]
///
/// Build the viewport `with_visible(false)`, make the DWM call and set the
/// capture mask on the window while it is still hidden, and only then ask for
/// it to be shown. Re-measured the same way on the same code: **one** sample of
/// white instead of eighty -- at most one unpainted frame, which was thought
/// to be the floor for this architecture and is what `window_host::Reveal`
/// accepts for every other window in this crate.
///
/// **The show is this module's own `ShowWindow`, not a
/// `ViewportCommand::Visible(true)`.** The command went through `winit`, whose
/// `WindowFlags::apply_diff` answers every flag change by rewriting the
/// window's styles from its own flag set -- which is how, in the layered-window
/// era of this module, the owner's log came to read the ex-style back with
/// `WS_EX_LAYERED` gone on the frame after the show. The layering is history
/// ([`RegionOverlay::take_picture`]); the reason to keep `winit`'s flag
/// machinery away from this window is not, and [`show_painted`] carries it.
///
/// # Why both steps run on the ROOT's frame
///
/// The first was argued from a premise that `eframe` 0.35 does not hold, and
/// the argument is restated here on the one that does. The premise was that a
/// hidden deferred viewport's callback does not run -- `run_ui_and_paint`
/// gates on `is_visible`. It does gate on that name, but the value is
/// `viewport.info.visible().unwrap_or(true)`, and `ViewportInfo::visible()`
/// is computed from `minimized` and `occluded` alone; `egui_winit` never
/// fills `occluded` on Windows, so it is `None`, `unwrap_or` makes it `true`,
/// and **the callback runs and paints on a hidden window**. What the callback
/// still cannot do is know when its window *exists* as an `HWND` this module
/// can act on, or make the window visible to the user without the cost of
/// that being in front of a paint -- so the state machine lives on the root's
/// frame, and the show happens from there. And it is this module's own
/// `ShowWindow`, not a `ViewportCommand::Visible(true)`, for the reason
/// [`show_painted`] sets out at length: the command goes through `winit`,
/// which rewrites the window's styles on every flag change and hides a window
/// its flags believe hidden.
///
/// The second is a choice, and it was measured. The raise and the minimise are
/// an `EnumWindows` each; put them on the overlay's own first visible frame and
/// that whole cost sits between the window appearing and its first painted
/// pixel. Measured both ways, same probe, same code: **54** white samples with
/// them in the callback against **one** with them on the root's next frame.
///
/// # The window is shown only once it has been painted
///
/// A window that is shown before anything has painted into it shows whatever
/// its surface happens to hold -- measured white on this machine, described
/// as black on the owner's. `eframe` 0.35 runs and paints a viewport's
/// callback whether or not its window is visible (`ViewportInfo::visible()`
/// is minimised-or-occluded, and neither is set for a hidden window), so the
/// callback paints into the hidden window from the frame it exists, and the
/// callback says so through `Inner::painted`. The show waits for that flag:
/// that is what the `painted` argument of [`Appearing::on_screen`] gates on.
///
/// What that buys, and what it does not. A window whose surface has been
/// painted while hidden still has to be composited once it is shown, and the
/// first composite may precede the first swap after the show by a frame --
/// the "one sample of white" the measurement above found, which is the floor
/// `window_host::Reveal` accepts for every window in this crate. What it
/// removes is everything longer than that: the second and a third the
/// original arrangement had, and the two `EnumWindows` the layered
/// arrangement put between the show and the dim. The repaint asked for on
/// the frame of the show is what keeps it at one.
///
/// # Every transition is forwards, as with [`Prescan`] and [`Reveal`]
///
/// `Waiting` is where an overlay whose window does not exist yet stays, and the
/// lookup that leaves it is the only `EnumWindows` this costs -- two or three
/// per overlay, none afterwards. `Hidden` lasts until the overlay has painted
/// once, which is at most a few root frames and is guaranteed rather than
/// hoped for: every `Hidden` root frame asks for the next one, and every root
/// frame asks the overlay to paint. `Up` is absorbing, and each method below
/// is the only way out of one state, so neither can run twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Appearing {
    /// `eframe` has not created the OS window yet. **Checked rather than
    /// assumed**: the hook this replaced claimed its one chance on a frame
    /// where the lookup answered `None`, and spent it -- which is the defect
    /// that shipped through 0.15.21 and cost the compositing call, the
    /// capture mask and the raise all three, silently.
    Waiting,
    /// The window exists, is hidden, is out of screen captures, and is being
    /// painted into by the callback. It stays here until the callback has
    /// painted a frame into it.
    Hidden,
    /// It is on screen. Raised, and Deskwarden's own window has gone down
    /// behind it.
    Up,
}

impl Appearing {
    /// **Leaves `Waiting`, and nothing else.** Answers `true` on the one frame
    /// the caller should take the window out of captures and place it.
    ///
    /// `window_exists` is what the caller's lookup found, and is only consulted
    /// here -- once the window has been seen it is never looked for again.
    fn compose(&mut self, window_exists: bool) -> bool {
        if matches!(self, Self::Waiting) && window_exists {
            *self = Self::Hidden;
            true
        } else {
            false
        }
    }

    /// **Leaves `Hidden`, and nothing else, and only once the overlay has
    /// painted.** Answers `true` on the one frame the caller should show the
    /// window, raise it, and send Deskwarden's own window down.
    ///
    /// `painted` is whether the viewport callback has drawn a frame into the
    /// window yet. Until it has, the surface holds nothing the user should
    /// see, and showing it would show them exactly that.
    ///
    /// A caller that never composed stays in `Waiting` and gets `false` for
    /// ever, which is what a test overlay and an overlay whose window never
    /// appeared both are.
    fn on_screen(&mut self, painted: bool) -> bool {
        if matches!(self, Self::Hidden) && painted {
            *self = Self::Up;
            true
        } else {
            false
        }
    }
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
    /// **Opens over ONE of the given monitors -- the active display -- and no
    /// longer over the bounding box of all of them.**
    ///
    /// `points_per_pixel` is the parent context's, and `monitors` is
    /// [`crate::screen_capture::monitor_bounds`] in production -- an argument
    /// so that the placement arithmetic can be exercised without a desktop.
    ///
    /// `None` when there are no monitors to cover: there is no rectangle to
    /// put a window on, and a zero-sized always-on-top window would be a
    /// surface the user cannot dismiss.
    ///
    /// # Why one display, asked for by the owner
    ///
    /// *"just cover the whole active display with the transparent screen"*.
    /// This used to be the bounding box of every monitor -- a `whole_screen`
    /// helper that is gone, because placing this window was the only thing
    /// that ever wanted it -- which on a multi-monitor desktop is a window
    /// that is larger than any
    /// screen, can start at a negative origin, and on an L-shaped arrangement
    /// covers pixels no monitor owns. A user with two screens got both of them
    /// dimmed to select a region on one.
    ///
    /// It is also the geometry every renegotiation Windows can perform on a
    /// window has to act on. A window spanning two monitors at different
    /// scaling gets `WM_DPICHANGED` the moment it is shown, and `winit`
    /// answers that by *moving and resizing it* to the rectangle Windows
    /// suggests (`platform_impl/windows/event_loop.rs`, the `WM_DPICHANGED`
    /// arm: it computes a `new_outer_rect` from the suggested rect and calls
    /// `SetWindowPos`). That is a window that appears at one size and jumps to
    /// another, with the newly exposed part of its redirection surface
    /// unpainted until a frame covers it -- which is the shape of the "small
    /// popup, then transparent, then black" the owner reported. A window that
    /// sits inside a single monitor crosses no DPI boundary and gets no such
    /// message.
    ///
    /// # The anchor is read HERE, and here is the only place it can be
    ///
    /// [`crate::screen_capture::active_display`] carries the argument for what
    /// "active" means and why it must be resolved before anything moves. The
    /// deadline is this function: a few frames later
    /// [`RegionOverlay::stand_aside`] minimises the Deskwarden window this
    /// anchor is the centre of, and a minimised window's `GetWindowRect` is
    /// `-32000, -32000`. So the rectangle is resolved once, now, and stored in
    /// [`Inner::display`] -- `show` reads it back and never asks again.
    ///
    /// # What did NOT narrow, deliberately
    ///
    /// [`scan_screen_with`] still walks **every** monitor, on both of its
    /// paths: the prescan this route runs before any window exists, and the
    /// *All screens* chip. It is decode-only, it opens no window, and it is
    /// the path that usually succeeds -- narrowing it would make the feature
    /// worse at its main job to fix a complaint about a window. So the scan is
    /// the whole desktop and the *overlay* is one screen, and the chip is
    /// named for the scan it runs rather than for the surface it runs from.
    /// See [`WHOLE_SCREEN_HINT`].
    pub fn open(monitors: &[ScreenRect], points_per_pixel: f32) -> Option<RegionOverlay> {
        // The anchor: the centre of Deskwarden's own window, which is the
        // window the press came from and is still exactly where the user put
        // it. `None` -- no such window, which is what a test process and a
        // probe with a differently-titled root both are -- falls through to
        // the cursor inside `active_display`.
        let display = screen_capture::active_display(own_window_centre(), monitors)?;
        Self::open_on(display, points_per_pixel)
    }

    /// [`RegionOverlay::open`] with the display already chosen: everything
    /// that function does except read the desktop.
    ///
    /// This is the seam the tests drive, and it is the shape the placement
    /// arithmetic was always tested through: the arithmetic that places a
    /// drag, a mark and a reveal inside a viewport is exercised on a display
    /// that is negative, offset and scaled, and none of that needs a
    /// compositor. It is also what keeps those tests deterministic now that
    /// the production entry point reads a live cursor -- `open` on a
    /// fabricated monitor list would otherwise answer differently depending on
    /// where the mouse happened to be when the suite ran.
    pub fn open_on(display: ScreenRect, points_per_pixel: f32) -> Option<RegionOverlay> {
        if display.width() == 0 || display.height() == 0 {
            return None;
        }
        Some(RegionOverlay {
            inner: Arc::new(Mutex::new(Inner {
                display,
                points_per_pixel,
                drag: None,
                found: false,
                throttle: DecodeThrottle::new(DECODE_INTERVAL),
                outcome: None,
                appearing: Appearing::Waiting,
                open: true,
                prescan: Prescan::Due,
                masked: false,
                aside: false,
                aside_at: None,
                reason: None,
                reveal: Reveal::Nothing,
                chips: [egui::Rect::NOTHING; 2],
                chip_press: None,
                painted: false,
                hwnd: None,
                picture: None,
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
        // The one conversion this window owns, in the direction `to_screen`
        // does not go: screen pixels back into the points the painter works
        // in. Written once here and used by both the drag and the reveal, so
        // the mark round a found code and the box round a dragged one cannot
        // end up a scale factor apart.
        let to_points = |rect: ScreenRect| {
            let at = |x: i32, y: i32| {
                egui::pos2(
                    (x - held.display.left) as f32 / scale,
                    (y - held.display.top) as f32 / scale,
                )
            };
            egui::Rect::from_min_max(at(rect.left, rect.top), at(rect.right, rect.bottom))
        };
        let (selection, size) = match held.drag {
            None => (None, None),
            Some(drag) => {
                let rect = drag.rect();
                (Some(to_points(rect)), Some((rect.width(), rect.height())))
            }
        };
        RegionView {
            selection,
            size,
            found: held.found,
            reason: held.reason,
            // `Due` reports the rectangle as readily as `Showing` does: the
            // frame that paints it is the frame that starts its clock, so a
            // `Due` that painted nothing would be a frame of dim with no mark
            // on it before the mark appeared.
            reveal: match held.reveal {
                Reveal::Due { at } | Reveal::Showing { at, .. } => Some(to_points(at)),
                Reveal::Nothing | Reveal::Done => None,
            },
            picture: held.picture.as_ref().map(|picture| picture.0.id()),
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
    /// One code records [`Outcome::Decoded`] -- and therefore lands on 6c,
    /// which is where every route lands and where the only Save in this
    /// feature lives -- but **does not close the overlay yet**: it opens the
    /// reveal, which holds the answer on screen for [`REVEAL_DWELL`] and then
    /// closes. Anything else leaves the overlay up and records why, which is
    /// what the bar's first line then says.
    ///
    /// The stale lock-on is cleared with it: a rescan that found nothing must
    /// not leave "Code found" above a rectangle from before it.
    ///
    /// **It takes no clock**, which is worth noticing: the reveal's deadline
    /// is set by the first frame that paints it, not by the frame that found
    /// the code. See [`Reveal::Due`].
    fn apply_scan(&self, scan: ScreenScan) {
        match scan {
            ScreenScan::Found(text, at) => {
                let mut held = locked(&self.inner);
                // **Forwards only.** A second answer -- the *Whole screen*
                // chip pressed twice quickly, or a repaint that got past the
                // guard -- must not restart a reveal that is already running
                // or reopen one that has finished, which would be a window
                // that refuses to close. `Nothing` is the only state a reveal
                // may begin from.
                if !matches!(held.reveal, Reveal::Nothing) {
                    return;
                }
                held.reveal = Reveal::Due { at };
                // The lock-on badge's flag, cleared rather than set: the
                // reveal paints its own mark with its own words, and a
                // `found` left true would put "release to read" over a
                // selection nobody is dragging if a frame ever painted both.
                held.found = false;
                // Recorded now, taken later. `open` stays true, so
                // `take_outcome` -- which the caller only reaches once `show`
                // has answered `false` -- cannot see it until the reveal has
                // run. It is a `Zeroizing`, so an overlay dropped mid-reveal
                // wipes it rather than leaking it.
                if held.outcome.is_none() {
                    held.outcome = Some(Outcome::Decoded(text));
                }
            }
            ScreenScan::Missed(miss) => {
                let mut held = locked(&self.inner);
                held.reason = Some(miss);
                held.found = false;
            }
        }
    }

    /// **Advances the reveal by one frame**, and says what to paint.
    ///
    /// `Some(rect)` -- in virtual-screen pixels -- means this frame belongs to
    /// the reveal: paint the mark, read no input, and come back. `None` means
    /// there is no reveal, either because nothing was found or because the one
    /// there was has just ended -- and in that second case **the overlay is
    /// closed by this call**, which is what makes the reveal end on a clock
    /// rather than on anything the user does.
    ///
    /// `now` is an argument for [`RegionOverlay::prescan_step`]'s reason
    /// exactly. Every call moves forwards or stands still; see [`Reveal`].
    fn reveal_step(&self, now: Instant) -> Option<ScreenRect> {
        let mut held = locked(&self.inner);
        match held.reveal {
            Reveal::Nothing | Reveal::Done => None,
            // The first painted frame is what starts the clock, so the user
            // gets the whole dwell rather than whatever is left of it after
            // the OS has made a window.
            Reveal::Due { at } => {
                // **...and not even that frame, while Deskwarden's own window
                // is still on its way down.**
                //
                // The minimise is issued on the root frame that raises this
                // window, which is at best this frame and may be the one after
                // it -- so on a route that found a code, the window this
                // reveal exists to get out of the way can still be on screen
                // right now. Starting the clock here would spend the
                // front of `REVEAL_DWELL` ringing a code behind it, by an
                // amount that varies with the user's machine, which is exactly
                // the defect `Reveal::Due` was introduced to avoid for a
                // window that did not exist yet. `MINIMISE_SETTLE` carries the
                // measurement and the argument.
                //
                // This cannot become a wait with no end: `aside_at` is set
                // once, the comparison is against a clock that only moves
                // forwards, and the reveal branch of the callback asks for the
                // next frame itself. `None` -- an overlay that never stood
                // aside -- has nothing to wait for and does not.
                //
                // **And not while the window is still hidden, either.**
                // `Appearing::Hidden` means the callback is painting into a
                // window nobody can see yet, and the root frame that shows
                // it, raises it and minimises the vault window has not
                // happened -- it is a root frame away, and guaranteed: every
                // `Hidden` root frame asks for the next. Without this the
                // dwell could start on a hidden window, spending the front of
                // `REVEAL_DWELL` on a ring nobody can see -- the same defect
                // `MINIMISE_SETTLE` carries the measurement for, reached from
                // the other side. A test overlay is `Waiting` and never
                // `Hidden`, so this waits for nothing there; and the state
                // only moves forwards, so it cannot wait for ever.
                if matches!(held.appearing, Appearing::Hidden) {
                    return Some(at);
                }
                if let Some(down_at) = held.aside_at {
                    if now < down_at + MINIMISE_SETTLE {
                        return Some(at);
                    }
                }
                held.reveal = Reveal::Showing {
                    at,
                    until: now + REVEAL_DWELL,
                };
                Some(at)
            }
            Reveal::Showing { at, until } => {
                if now < until {
                    Some(at)
                } else {
                    // Marked `Done` and closed in the same breath, before
                    // anything else can run: `Done` is absorbing and `open`
                    // is false, so every later frame takes the callback's own
                    // finished guard and does nothing at all.
                    held.reveal = Reveal::Done;
                    held.open = false;
                    None
                }
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

    /// **Sends Deskwarden's own window down out of the user's way, or brings
    /// it back on top.**
    ///
    /// # Why this exists at all
    ///
    /// [`mask_own_window`](Self::mask_own_window) solves the machine's view
    /// and not the user's. `WDA_EXCLUDEFROMCAPTURE` takes the vault window out
    /// of the blit, so the scan reads what is behind it -- but the window is
    /// still physically on screen, on top of the code, and both of the things
    /// this overlay then does are things the user has to *look* at: the drag
    /// fallback asks them to point at a code the window is covering, and the
    /// reveal rings a code the window is covering. The owner said it in one
    /// sentence: *"when Scan code clicked - that should minimize the DW window
    /// so it doesn't obstruct the screen - obviously it doesn't have the QR
    /// code but some other window has it, once back - it should be on top
    /// again"*.
    ///
    /// # Minimise and not hide, and the difference is not aesthetic
    ///
    /// `SW_HIDE` is instant, leaves no taskbar button and looks better. It is
    /// also the one choice whose failure mode is unrecoverable: a hidden
    /// window that does not come back is a window the user cannot reach by any
    /// means at all, whereas a minimised one that does not come back is one
    /// taskbar click away. Everything below is built so that it always comes
    /// back, and choosing the option that does not *need* that to be true is
    /// how a safety argument is meant to be made.
    ///
    /// The second reason is this app's own state. `vault_window`'s
    /// `keep_ui_loaded` machinery has a hidden state of its own -- a `hidden`
    /// cell, `close_or_hide`, `spawn_show_waiter`, a named event the daemon
    /// signals -- and `ChromeAction::Hide`'s neighbour argues at length that
    /// *"Minimize is not a hide, and must not become one"*: a minimised window
    /// is still in use, keeps its taskbar button, keeps `vault_is_in_use`
    /// answering `true` and keeps its vault-service attachment, where a hidden
    /// one has none of that. Hiding from here would put the window into a
    /// state that machinery believes only it can produce, without any of the
    /// bookkeeping it does; minimising puts it in exactly the state the app's
    /// own minimise button produces, which that machinery already ignores by
    /// design.
    ///
    /// # The mask stays on as well
    ///
    /// A minimised window is not composited, so the exclusion is redundant
    /// while it is down -- and it is kept, because it is what covers the
    /// window in the frames between the press and the minimise taking effect,
    /// and because the scan's capture happens two frames *before* this is ever
    /// called. See [`MINIMISE_SETTLE`] for why it cannot be called earlier.
    ///
    /// Idempotent through [`Inner::aside`], for
    /// [`mask_own_window`](Self::mask_own_window)'s reasons.
    fn stand_aside(&self, away: bool) {
        {
            let mut held = locked(&self.inner);
            if held.aside == away {
                return;
            }
            held.aside = away;
            held.aside_at = if away { Some(Instant::now()) } else { None };
        }
        if away {
            send_window_down(crate::vault_window::WINDOW_TITLE);
        } else {
            bring_window_back(crate::vault_window::WINDOW_TITLE);
        }
    }

    /// **Takes the overlay's window from "created" to "on screen, painted,
    /// raised, with Deskwarden out of the way behind it"** -- two steps, at
    /// least one root frame apart, and never a third.
    ///
    /// Both steps run here, on the ROOT's frame. See [`Appearing`] for why
    /// neither can be in the viewport callback, and for the measurement of what
    /// the arrangement this replaced cost.
    ///
    /// # Step one, on a window nobody can see yet
    ///
    /// 1. [`exclude_from_capture`], while hidden, so there is no frame in
    ///    which this window is on screen and *in* a capture. A region dragged
    ///    on this surface is captured through where this surface is, and a
    ///    capture that included its own dim reads as "no code there" for a code
    ///    that is plainly on screen. (The picture the surface paints was taken
    ///    before this window existed -- see [`RegionOverlay::take_picture`] --
    ///    so it cannot have photographed itself either.)
    /// 2. The geometry, measured and logged while nobody has seen it.
    ///
    /// Two repaint requests follow, one for the overlay and one for the root.
    /// The overlay's is what buys the first painted frame; the root's is what
    /// buys the frame step two runs on. Without them nothing else would ask --
    /// an always-on-top window over every monitor gets no events of its own.
    ///
    /// The `return` is load-bearing: the window has not painted yet, so step
    /// two must not be on this frame.
    ///
    /// # Step two, on the first root frame after the overlay has painted
    ///
    /// * Gated on `Inner::painted`, set by the callback after its first
    ///   `draw`. Until then the surface holds whatever it held when Windows
    ///   made it, and showing it would put that on screen -- which is the
    ///   *"blinks black, then white box"* this module has already been
    ///   reported for once. Every `Hidden` root frame that finds it unpainted
    ///   asks for another root frame, so the gate cannot become a wait with no
    ///   end.
    /// * [`show_painted`] -- **this module's own `ShowWindow`, and not
    ///   `ViewportCommand::Visible(true)`**, for the reason that function sets
    ///   out. Then a repaint of the overlay, so that the first swap after the
    ///   show is a frame away and not a pointer-move away.
    /// * `foreground::pick` skips invisible windows, so this is the first frame
    ///   a raise can find anything at all. `window_host::Reveal` splits show
    ///   from raise for this same reason.
    /// * The minimise must not land while this process's only live viewport is
    ///   the one being minimised -- a minimised `eframe` root alone takes no
    ///   further frames at all, measured twice at 5.5 s of nothing. By this
    ///   frame the overlay is not merely registered but visible and painted,
    ///   which is a stronger guarantee than the one the shipped arrangement
    ///   had.
    /// * The minimise stays after the raise: `SW_SHOWMINNOACTIVE` activates
    ///   nothing, so a foreground this window has already taken is one it keeps
    ///   -- which is what leaves Escape working with the app down. See
    ///   [`send_window_down`].
    ///
    /// One thing moved with it. The reveal's dwell must not start before
    /// Deskwarden is down, and the minimise is now a frame later than the
    /// overlay's first paint, so [`RegionOverlay::reveal_step`] holds while
    /// [`Appearing::Hidden`] as well as while `MINIMISE_SETTLE` runs.
    fn appear(&self, ctx: &egui::Context) {
        // The lookup, and only while there is something to find.
        // `own_window_titled` does NOT skip invisible windows -- see its doc,
        // which is what makes a window created hidden findable at all -- and it
        // is an `EnumWindows`, measured in the hundreds of milliseconds in an
        // unoptimised build. Two or three per overlay, none after that.
        let waiting = matches!(locked(&self.inner).appearing, Appearing::Waiting);
        let found = if waiting {
            crate::foreground::own_window_titled(REGION_TITLE)
        } else {
            None
        };
        let display = locked(&self.inner).display;
        // **Every waiting frame, and before anything else touches the
        // window.** See [`hide_and_place`]: the builder's `with_visible(false)`
        // is not the only thing that decides whether this window is on screen,
        // and the owner watched a small white window appear and then move. This
        // takes it down and puts it where it belongs while nobody is looking,
        // and it runs on every frame of `Waiting` rather than once so that the
        // last thing to happen before the show is always this.
        if waiting {
            hide_and_place(REGION_TITLE, display);
        }
        if locked(&self.inner).appearing.compose(found.is_some()) {
            // `compose` only answers `true` when told the window exists, so
            // the handle is there; kept for the show, a frame or more away.
            locked(&self.inner).hwnd = found;
            exclude_from_capture(REGION_TITLE);
            // **Before the show**, which is the whole value of measuring here:
            // this is the last moment the geometry can be inspected without
            // the user having already seen whatever it is. See
            // [`log_window_rect`].
            log_window_rect(REGION_TITLE, "hidden, waiting for the first paint", display);
            ctx.request_repaint_of(region_viewport());
            ctx.request_repaint();
            return;
        }
        let painted = locked(&self.inner).painted;
        let hwnd = locked(&self.inner).hwnd;
        if locked(&self.inner).appearing.on_screen(painted) {
            if let Some(hwnd) = hwnd {
                show_painted(hwnd);
            }
            // And again on the first frame it is up, because the show itself
            // is one of the moments Windows may renegotiate the rectangle --
            // `ShowWindow` is what moves a window onto a monitor as far as
            // `WM_DPICHANGED` is concerned. Two lines, one before and one
            // after, is what tells "it was never the right size" apart from
            // "it was, and then it moved".
            log_window_rect(REGION_TITLE, "shown", display);
            // The first swap after the show, as soon as `eframe` can make it:
            // see `Appearing` for the one composite this bounds.
            ctx.request_repaint_of(region_viewport());
            // A selection surface that opens behind the window being selected
            // from is useless, so this one raises; see its row in
            // `foreground::OPENS_A_VIEWPORT_AND_RAISES_IT`.
            crate::foreground::raise_window(REGION_TITLE);
            // **And Deskwarden's own window goes down.** Every exit from `show`
            // puts it back, and `Inner`'s `Drop` covers the exits that do not
            // come through `show` at all.
            self.stand_aside(true);
        } else if matches!(locked(&self.inner).appearing, Appearing::Hidden) {
            // Hidden, being painted, and not painted yet. The overlay's own
            // paint is already asked for on every root frame by `show`; this
            // is what keeps the root frames coming until one of them finds
            // the paint has happened. Bounded by the paint, which `eframe`
            // owes a viewport that has asked for it.
            ctx.request_repaint();
        }
    }

    /// Records where [`draw`] painted the bar's chips, so the next frame's
    /// pointer handling can tell a press on one from a drag.
    fn remember_chips(&self, chips: [egui::Rect; 2]) {
        locked(&self.inner).chips = chips;
    }

    /// **Records that the callback has painted a frame into the window.**
    /// Called after every `draw`, and read by [`RegionOverlay::appear`], which
    /// keeps the window hidden until this has happened once. See
    /// [`Appearing`].
    fn note_painted(&self) {
        locked(&self.inner).painted = true;
    }

    /// **Takes the picture of the display this overlay covers, and uploads it
    /// as the surface's ground.** Called once, from `show`, on the frame the
    /// whole-screen scan runs -- which is before the viewport is registered,
    /// so before the window exists, so the picture cannot contain the window.
    /// The vault window has been out of captures since `PrescanStep::Mask`,
    /// so the picture is what is behind it, which is also what a drag on this
    /// surface captures on release: the two agree by construction.
    ///
    /// # Why a picture at all: the four mechanisms that came before it
    ///
    /// This surface was designed as a **live** view of the desktop through a
    /// translucent window, and four mechanisms for that were shipped, every
    /// one accepted by Windows and wrong on the owner's monitor:
    ///
    /// 1. `DwmEnableBlurBehindWindow` with an empty blur region -- the
    ///    twenty-year idiom for per-pixel alpha -- blurred the whole desktop
    ///    on Windows 11 26200. A phone photograph showed the bar pin-sharp and
    ///    everything above it a lavender haze.
    /// 2. `DwmExtendFrameIntoClientArea` with `-1` margins: accepted on three
    ///    overlays, "scan not fixed".
    /// 3. `WS_EX_LAYERED` + `SetLayeredWindowAttributes(LWA_ALPHA)`, the
    ///    oldest transparency Windows has. The owner's log for the run that
    ///    ended it, with the readback this module added for the purpose:
    ///
    ///    ```text
    ///    12:03:17  layered window on 0x5017c -- ex-style 0xc0118 -> 0xc0118:
    ///              WS_EX_LAYERED survived the show; SetLayeredWindowAttributes
    ///              accepted at alpha 115/255, LWA_ALPHA
    ///    12:03:26  on the frame the overlay closes -- 0x5017c has ex-style
    ///              0xc0118 (WS_EX_LAYERED on), and GetLayeredWindowAttributes
    ///              says alpha 115/255 with flags 0x2
    ///    ```
    ///
    ///    Nine seconds with the bit on and the alpha at 115, and the owner's
    ///    words for those nine seconds: *"transparent dim blinks then same
    ///    black screen with stripe at the bottom"*. The alpha was applied to
    ///    the first composite and to nothing after it, which is consistent
    ///    with the OpenGL swap being presented past the redirection surface
    ///    the layered alpha applies to. Whatever the exact path, the OS said
    ///    yes and the glass said no, and there is no fourth question to ask it.
    /// 4. Not shipped, and not to be: a capture-based probe cannot see any of
    ///    this. It measured mechanism 1 as SHARP at 1.000 on the machine that
    ///    was visibly blurring, because a `BitBlt` reads a composition DWM
    ///    does not apply its effects to.
    ///
    /// So the window asks the compositor for nothing. It is opaque, it paints
    /// this picture edge to edge, and it lays [`dim_wash`] over it in the
    /// framebuffer -- which is how the platform's own Snipping Tool does it,
    /// and which removes DWM, the driver and the swap chain from the question.
    /// What that gives back is the design: the selection is a **hole** in the
    /// wash, the framed pixels at full strength ([`DIM_ALPHA`]), and the bar
    /// is a plate again ([`BAR_BG_ALPHA`]).
    ///
    /// # What it costs, stated
    ///
    /// * **Memory.** The display's pixels, RGBA, twice over for a moment: the
    ///   `Rgba` from the capture (`Zeroizing`, wiped on drop, and dropped here
    ///   the moment the `ColorImage` exists) and the `ColorImage` egui builds
    ///   from it (`Vec<Color32>`, freed but not wiped, held by egui only until
    ///   the painter has uploaded it), then the texture on the GPU for the
    ///   life of the overlay -- 29 MB at 5120x1440. The handle is released on
    ///   the frame the overlay closes and by `Inner`'s `Drop` behind that, and
    ///   `TextureHandle`'s drop is what frees the texture.
    /// * **What is shown.** The pixels on the user's own monitor at that
    ///   moment, which they are already looking at; the window that shows them
    ///   is `WDA_EXCLUDEFROMCAPTURE`, so a recording of the desktop does not
    ///   get a second copy. The module header's rule that no capture is ever
    ///   displayed was reversed for this one picture, deliberately, and says
    ///   so.
    /// * **Scale.** The texture is the display's physical pixels; the window
    ///   is `display / scale` points, and egui paints it back at
    ///   `pixels_per_point`, so on a display whose scale is the one the vault
    ///   window's context reported it maps one to one --
    ///   [`egui::TextureOptions::NEAREST`] so that a mapping that is one to
    ///   one stays crisp rather than being resampled. A display at a different
    ///   scale from the vault window's would be resampled by the ratio, which
    ///   is soft but not misplaced: the drag box is converted through the same
    ///   `scale` by [`to_screen`], so it frames the pixels it appears to.
    /// * **Refusal.** `capture_rect` can refuse -- protected content, a GDI
    ///   failure, the same cases the drag's own capture already handles. The
    ///   overlay then opens on a solid [`DIM_INK`] ground with no picture, the
    ///   bar and the drag exactly as before, and a `warn` in the log names the
    ///   refusal. It does not panic and it does not stay closed: a surface
    ///   that cannot show the desktop can still frame a region of it, and the
    ///   release's own capture reports its own refusal in the bar.
    fn take_picture(&self, ctx: &egui::Context) {
        let display = locked(&self.inner).display;
        let picture = match screen_capture::capture_rect(display) {
            Ok(pixels) => {
                let (width, height) = (pixels.width(), pixels.height());
                if (width, height) != (display.width(), display.height()) {
                    log::warn!(
                        "region overlay: the display's picture came back {width}x{height} for \
                         a {}x{} display; it will be stretched to fit",
                        display.width(),
                        display.height()
                    );
                }
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [width as usize, height as usize],
                    pixels.pixels(),
                );
                // The wiped copy goes now; the egui copy lives until the
                // painter has uploaded it, the texture until the overlay ends.
                drop(pixels);
                let texture = ctx.load_texture(
                    "region-overlay-desktop",
                    image,
                    egui::TextureOptions::NEAREST,
                );
                log::info!(
                    "region overlay: the display's picture is {width}x{height} ({} bytes) in \
                     texture {:?}; the overlay paints it under the dim and releases it on the \
                     frame it closes",
                    u64::from(width) * u64::from(height) * 4,
                    texture.id()
                );
                Some(Picture(texture))
            }
            Err(refusal) => {
                log::warn!(
                    "region overlay: the display could not be captured for the overlay's \
                     picture ({refusal:?}); the overlay opens on a solid dim ground instead of \
                     a dimmed desktop, and a drag on it still captures on release"
                );
                None
            }
        };
        locked(&self.inner).picture = picture;
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
            let cursor = to_screen(
                (held.display.left, held.display.top),
                held.points_per_pixel,
                at,
            );
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
            // **The picture goes with the window.** This frame does not
            // re-register the viewport, so `eframe` destroys the window when
            // it ends; the texture is released here so that it does not
            // outlive the window by however long the caller keeps the last
            // clone of this overlay. Dropping the handle is what frees it.
            if locked(&self.inner).picture.take().is_some() {
                log::info!("region overlay: the display's picture is released with the window");
            }
            // **Both, and in this order.** Standing aside was the last thing
            // done on the way in, so coming back is the first thing done on
            // the way out; and the window is put back before it is put back
            // into captures, so it is never briefly on screen and invisible to
            // the user's own screenshots at the same time. Both are no-ops on
            // an overlay that never did either.
            self.stand_aside(false);
            self.mask_own_window(false);
            return false;
        }

        // **The whole-screen scan, before this module has a window at all.**
        //
        // This is the departure the module header argues for: choosing the
        // route is asking for the scan, so the scan happens here and the
        // *draggable* 6b opens only if it cannot answer. Nothing below this
        // block runs on the frames the scan itself takes, so while it is
        // running no viewport is registered and no dim appears -- what the
        // user has on screen is the picker's own "Scanning your screen" card.
        // Once it has answered a window opens either way: with the bar and
        // the box to drag if it could not answer, or with the reveal if it
        // could. See `apply_scan`.
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
                // **And the picture the surface will paint, on the same
                // frame and for the same reason**: the vault window is
                // masked, and the overlay's own window does not exist yet --
                // it is registered below and created by `eframe` at the end
                // of this frame -- so this is the one moment a capture of the
                // display can contain neither. See `take_picture`.
                self.take_picture(ctx);
                // **The overlay opens either way now, and that is the
                // change.** It used to end here when the scan found one code
                // -- no window was ever registered and the user went from the
                // modal straight to 6c, with nothing between the press and
                // the card to say where the code had come from. `apply_scan`
                // now opens the reveal instead: the window is registered
                // below like any other 6b, paints the code it found ringed
                // where it sits, and closes itself on a clock. The outcome is
                // the same `Outcome::Decoded` it always was, and `take_outcome`
                // still cannot be reached until `show` answers `false`.
                //
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

        // **One rectangle, read back from where `open` stored it, and no
        // second reading of the desktop.**
        //
        // This block used to take the origin from `Inner` and the size from a
        // fresh `screen_capture::monitor_bounds()` on every frame, which is
        // two readings of a desktop that is allowed to change between them:
        // unplug a monitor, change a resolution or drag a screen in Display
        // Settings and the position came from the old arrangement while the
        // extent came from the new one. Worse, the size was recomputed after
        // Deskwarden's own window had been minimised, which is precisely the
        // moment the anchor that chose it stopped existing. `Inner::display`
        // is resolved once in `open` and carried; see it, and
        // `screen_capture::active_display`, for why.
        let (display, scale) = {
            let held = locked(&self.inner);
            (held.display, held.points_per_pixel)
        };
        let origin = (display.left, display.top);
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
                    display.width() as f32 / scale,
                    display.height() as f32 / scale,
                ])
                .with_decorations(false)
                .with_always_on_top()
                .with_taskbar(false)
                // **`.with_transparent(true)` is deliberately NOT here, and
                // its absence is the last piece of the blur fix.**
                //
                // It was here, on the reasoning that it is the correct
                // declaration of intent and costs nothing. The first half is
                // still true and the second is not. What that flag does on
                // Windows, when it survives, is make `winit` call
                // `DwmEnableBlurBehindWindow` on this window -- the exact call
                // that blurred the owner's desktop, and the one
                // `the_overlay_makes_itself_a_layered_window_before_it_is_shown`
                // now forbids this module from making itself. Leaving a flag
                // in the builder whose only Windows effect is to ask somebody
                // else to make it would be keeping the landmine and removing
                // the sign.
                //
                // It costs nothing to drop, and that is measured rather than
                // assumed: `glutin_winit::finalize_window` strips the flag
                // whenever the GL config answers
                // `supports_transparency() == Some(false)`, which on
                // Windows/WGL it always does -- the attribute behind that
                // answer is `WGL_TRANSPARENT_ARB`, a *colour-key* attribute
                // essentially no driver advertises -- and `deskwarden.log`
                // carries twenty-six `Cannot create transparent window: the
                // GL config does not support it` lines saying so on this
                // machine. The flag has never once reached the window here.
                //
                // The surface does not need it either way: the window is
                // opaque on purpose and paints a picture of the desktop under
                // a wash in its own framebuffer -- see `take_picture` -- so
                // there is no alpha channel for anyone to composite.

                // **Created hidden, and shown by [`RegionOverlay::appear`]
                // once it is composited.**
                //
                // The same flag `app_window` and `vault_window` open their own
                // windows with, for the same reason and against a measurement
                // of the same defect: Windows shows a newly created window
                // before its GL surface, its font atlas or its first frame
                // exist, and what the user gets in the meantime is a solid
                // rectangle of whatever was in the redirection surface. On a
                // small window that is the startup white box; on this one it is
                // every pixel of every monitor, and it was measured at 1315 ms.
                // See [`Appearing`], which carries the numbers and the reason
                // the show cannot be driven from the callback below.
                .with_visible(false),
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

                // **Nothing about the window itself happens on this
                // callback's frames any more, and the move is the fix.**
                //
                // What used to be here was a hook on the first frame this
                // callback ran with an HWND to work on, and it did four things:
                // the raise, the capture mask, the DWM call that makes this
                // surface see-through, and the minimise. All four now run from
                // [`RegionOverlay::appear`], on the ROOT's frame, in two steps
                // one frame apart. Two reasons, and they point the same way:
                //
                // * This callback cannot tell when its window exists as an
                //   `HWND`, or whether the user can see it: `eframe` 0.35
                //   runs and paints it from the moment the window exists,
                //   hidden or not (`ViewportInfo::visible()` is minimised-or-
                //   occluded, and neither is set for a hidden window). So a
                //   hook here has no frame it can call "first on screen".
                // * Everything on this callback's critical path is in front of
                //   a paint. The raise and the minimise are an `EnumWindows`
                //   each, measured in the hundreds of milliseconds in an
                //   unoptimised build; doing them here, on the frame the window
                //   first became visible, put that whole cost between the
                //   window appearing and its first painted pixel. Measured: 54
                //   white samples that way against ONE with them on the root's
                //   next frame.
                //
                // See [`Appearing`] for the whole of it.

                // **The reveal owns its frames entirely.**
                //
                // While it is running the surface is a statement, not a
                // control: there is nothing to drag, nothing to choose and
                // nothing to cancel, because the code has already been read
                // and the only thing left is for the user to see it. So no
                // input is read on these frames -- not the pointer, not
                // Escape, not the chips -- which also happens to be what
                // stops a click landing in the half-second before 6c from
                // starting a drag on a surface that is about to vanish.
                //
                // The repaint request is what makes it end on its own. This
                // window is always-on-top over every monitor, so the desktop
                // behind it produces no events and nothing else would wake
                // it; without this the deadline would be reached only if the
                // user happened to move the mouse. It is bounded by the
                // deadline itself, not by a user: see `Reveal`.
                if mine.reveal_step(Instant::now()).is_some() {
                    root.request_repaint_of(region_viewport());
                    let view = mine.view();
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(root, |ui| draw(ui, &view));
                    mine.note_painted();
                    return;
                }

                // Guarded, because the call above can have closed the overlay
                // on the frame the reveal's deadline passed. Reading input
                // into a surface that has already answered is what the whole
                // finished-overlay guard at the top of this callback exists to
                // prevent, and this is the one path that reaches here with an
                // answer already recorded.
                if mine.is_open() {
                    let (pointer, pressed, down) = root.input(|i| {
                        (
                            i.pointer.latest_pos().map(|p| (p.x, p.y)),
                            i.pointer.primary_pressed(),
                            i.pointer.primary_down(),
                        )
                    });
                    // Taken before anything else looks at the pointer: a press
                    // that landed on a chip is that chip's for the whole
                    // gesture, and `advance` must not see it as a selection.
                    // See `chip_gesture`.
                    let chip = mine.chip_gesture(pointer, pressed, down);
                    if root.input(|i| i.key_pressed(egui::Key::Escape))
                        || root.input(|i| i.viewport().close_requested())
                        || chip == Some(CHIP_CANCEL)
                    {
                        mine.finish(Outcome::Cancelled);
                    } else if chip == Some(CHIP_WHOLE_SCREEN)
                        || root.input(|i| i.key_pressed(egui::Key::A))
                    {
                        // **The same scan the route already ran**, on demand:
                        // the user may have moved a window, closed one of two
                        // codes, or zoomed the page since. One press is one
                        // scan -- a key press and a release are single events,
                        // so this is bounded by the user rather than by the
                        // frame rate -- and when it finds one code it reveals
                        // it and lands on 6c exactly as the first scan does.
                        mine.apply_scan(scan_screen_with(
                            &RegionSeams::production(),
                            &screen_capture::monitor_bounds(),
                        ));
                        // **And ask for the frame that starts its clock.**
                        //
                        // This is the one place a reveal can be opened from
                        // inside the callback, and the frame that opens it is
                        // past `reveal_step` already -- so the reveal is
                        // `Due`, carrying a rectangle and no deadline, and
                        // nothing has asked to be called again. The press
                        // that got here was the last input this window will
                        // ever see: it is always-on-top over every monitor,
                        // the reveal reads no input, and the desktop behind
                        // it produces no events. Without this request the
                        // next frame never comes, the clock never starts, and
                        // a full-screen window sits there until the app is
                        // killed. That is the same shape as the hang this
                        // module was fixed for, reached from a different
                        // direction, and one request is the whole of the fix:
                        // the frame it buys takes the reveal branch, which
                        // asks for the next one itself.
                        //
                        // Harmless when the scan missed -- an extra repaint
                        // of a surface that repaints on input anyway.
                        root.request_repaint_of(region_viewport());
                    } else if !mine.in_chip_press() {
                        mine.advance(&RegionSeams::production(), pointer, down, Instant::now());
                    }
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
                // And that a frame now exists in the window, which is what
                // `appear` waits for before it lets the user see the window.
                mine.note_painted();
            },
        );
        // **After the viewport is registered, and on the ROOT's frame.**
        //
        // Both halves are load-bearing. The viewport has to be registered first
        // so that the repaint `appear` asks of the overlay has an entry in this
        // frame's viewport output to land in; and it has to be here rather than
        // in the callback above because the callback cannot tell when its
        // window exists or when the user can see it, and every `EnumWindows`
        // on its frames sits in front of a paint. See [`Appearing`].
        if self.is_open() {
            self.appear(ctx);
        }
        let still_open = self.is_open();
        if !still_open {
            // Whatever ended it -- a code found, none found, several found, a
            // refusal, Escape, the close button -- the vault window comes back
            // up and back into screen captures here. `Inner`'s `Drop` is the
            // backstop for the paths that do not come through this line: the
            // form closing under a live overlay, the vault locking, or a panic
            // unwinding past it. Order as in the early return above.
            self.stand_aside(false);
            self.mask_own_window(false);
        }
        still_open
    }
}

/// **The window comes back, and the mask comes off, however the overlay
/// ends.**
///
/// `show` does both on the frame it answers `false`, which covers every
/// ordinary ending. This covers the rest: the caller drops the overlay
/// because the form closed under it, the vault locks, or a panic unwinds
/// through the frame. `WDA_EXCLUDEFROMCAPTURE` outlives whoever set it, and a
/// vault window left permanently invisible to the user's own screenshots and
/// to a support call's screen share would be a side effect of a scan they
/// took once -- exactly the kind of thing a `Drop` exists to make impossible
/// to forget.
///
/// **The minimise is the same shape of obligation with a much worse failure
/// mode, so it is on the same hook.** A mask left on is a window the user
/// cannot screenshot. A window left down is the user's application gone: no
/// window, nothing on screen, and only a taskbar button between them and
/// concluding the app has crashed. That asymmetry is also why it is a
/// minimise and not a hide -- there *is* a taskbar button -- but the point of
/// this `Drop` is that the taskbar button should never be the thing that saves
/// it.
///
/// Order is the reverse of the way in, as in `show`: back up first, back into
/// captures second.
///
/// It is on `Inner` rather than on [`RegionOverlay`] because the overlay is an
/// `Arc` handle that is cloned per frame; this runs when the last one goes.
impl Drop for Inner {
    fn drop(&mut self) {
        if self.aside {
            bring_window_back(crate::vault_window::WINDOW_TITLE);
        }
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
/// than lifting a lit patch out of the one part of this surface that has to
/// stay readable.
///
/// **The ground is the picture, and the dim is a wash over it in the
/// framebuffer.** This window is opaque and asks the compositor for nothing
/// -- see [`RegionOverlay::take_picture`] for the four mechanisms that did
/// and were wrong on the glass. So [`paint_ground`] comes first, edge to edge,
/// and the wash ([`dim_wash`]) is laid over every part of it that is not the
/// selection: 55% of the desktop shows through the wash, and 100% of it
/// inside the box the user is dragging, which is what 6b draws.
///
/// **Returns the two chips' rectangles**, which is the one thing the painter
/// knows and the pointer handling needs. They cannot be computed ahead of the
/// paint: the bar's height follows the type it carries, and the type includes
/// a reason line that is there or not. See
/// [`RegionOverlay::remember_chips`].
pub fn draw(ui: &mut egui::Ui, view: &RegionView) -> [egui::Rect; 2] {
    let full = ui.max_rect();
    let painter = ui.painter().clone();

    paint_ground(&painter, full, view.picture);

    // **The reveal is the whole surface while it lasts**, and it takes
    // precedence over everything below because everything below is about a
    // drag that is not happening. It returns before the bar is drawn: see
    // `paint_reveal` for why this state has no bar and therefore no chips.
    if let Some(found) = view.reveal {
        paint_reveal(&painter, full, found);
        return [egui::Rect::NOTHING; 2];
    }

    match view.selection {
        // Nothing selected yet: the whole desktop dims, at the one value the
        // owner picked. See `DIM_ALPHA`.
        None => {
            painter.rect_filled(full, 0.0, dim_wash());
        }
        // The four bands around the selection are washed and the selection is
        // not: 6b's hole, back. See `paint_dim_around`.
        Some(sel) => {
            let sel = sel.intersect(full);
            paint_dim_around(&painter, full, sel);
            paint_selection_edge(&painter, sel);
            if let Some((w, h)) = view.size {
                paint_size_readout(&painter, sel, w, h);
            }
            if lockon_badge(view.found).is_some() {
                paint_badge(&painter, full, sel, LOCKED_ON);
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

/// **The ground under everything: the picture of the display, or a solid
/// [`DIM_INK`] when there is none.**
///
/// The picture is painted edge to edge with the whole texture (`uv` from
/// `(0, 0)` to `(1, 1)`) and no tint, so what reaches the framebuffer is the
/// captured pixel and nothing else; the dim is a separate wash over it. The
/// fallback is the stated one from [`RegionOverlay::take_picture`]: a capture
/// that was refused leaves the overlay a solid dark ground, on which the
/// selection ring, the badge and the bar are as legible as they ever were and
/// the desktop is simply not shown.
fn paint_ground(painter: &egui::Painter, full: egui::Rect, picture: Option<egui::TextureId>) {
    match picture {
        Some(texture) => {
            painter.image(
                texture,
                full,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        None => {
            painter.rect_filled(full, 0.0, DIM_INK);
        }
    }
}

/// **The four bands around a lit rectangle in [`dim_wash`], and the rectangle
/// itself left alone** -- which is what "stays lit" means over a picture of
/// the desktop: the framed pixels at full strength, everything around them at
/// 55%.
///
/// This is 6b's hole, back. On the layered window that preceded the picture a
/// hole could not be expressed at all, and the selection was the *least*
/// dark thing on a uniformly faded surface by fourteen levels; the owner's
/// report of that was *"drag a box completely black and user cannot find
/// where to put the box"*. Nothing is painted inside `lit` now, on purpose,
/// and `the_dim_is_painted_in_the_framebuffer_over_a_picture_of_the_desktop`
/// holds it there.
///
/// Extracted rather than written twice, because there are two lit rectangles
/// on this surface -- the box the user drags and the code the scan found --
/// and the four-band arithmetic is exactly the kind of thing that gets
/// corrected in one copy. A band with no area is skipped rather than painted
/// inverted: a selection flush with an edge produces one, and an inverted
/// `Rect` fills nothing in egui but says something wrong here.
fn paint_dim_around(painter: &egui::Painter, full: egui::Rect, lit: egui::Rect) {
    for band in [
        egui::Rect::from_min_max(full.left_top(), egui::pos2(full.right(), lit.top())),
        egui::Rect::from_min_max(egui::pos2(full.left(), lit.bottom()), full.right_bottom()),
        egui::Rect::from_min_max(egui::pos2(full.left(), lit.top()), lit.left_bottom()),
        egui::Rect::from_min_max(lit.right_top(), egui::pos2(full.right(), lit.bottom())),
    ] {
        if band.is_positive() {
            painter.rect_filled(band, 0.0, dim_wash());
        }
    }
}

/// **The reveal**: the code the whole-screen scan just read, ringed where it
/// actually sits on the user's desktop, with the design's tick beside it.
///
/// # It is 6b's own marks, not a new picture
///
/// Every stroke here is already in design 6b and is painted by the same two
/// functions the drag uses: the dim with the code left lit, the solid ring
/// and its halo, the four corner brackets, and the badge with `M20 6 9 17l-5-5`
/// in it. Only the words differ -- [`SCAN_FOUND`] rather than [`LOCKED_ON`],
/// for the reason that constant gives. The owner asked for "blue line around
/// QR code and V sign like in design"; this is that, with nothing invented to
/// go with it.
///
/// # Why there is no bottom bar
///
/// 6b's bar carries an instruction, a promise and two chips, and on this
/// stretch of frames all three would be wrong. There is nothing to drag, so
/// the instruction describes a gesture the user is not being asked for; the
/// callback reads no input while the reveal runs, so *Whole screen* and
/// *Cancel* would be bordered pills that do nothing when clicked -- the "lie
/// in the shape of a button" [`RegionOverlay::chip_gesture`] argues against
/// at length. The reveal is a statement and lasts less than half a second;
/// what it needs on screen is the mark and the tick.
///
/// The promise the bar carries is the one thing worth missing, and it is not
/// missed: *nothing is saved yet* is still true and is about to be said by
/// 6c, which is the card the user lands on and the only place a Save exists.
///
/// # The code is ringed on the picture, unwashed
///
/// What is inside the ring is the display's picture at full strength -- the
/// same pixels the scan decoded, taken on the same frame -- with the wash on
/// everything around it. The capture the scan decoded was still dropped; the
/// picture is [`RegionOverlay::take_picture`]'s, and it is the one capture
/// this module displays, for the reasons and within the bounds that function
/// sets out.
fn paint_reveal(painter: &egui::Painter, full: egui::Rect, found: egui::Rect) {
    let lit = found.intersect(full);
    paint_dim_around(painter, full, lit);
    paint_selection_edge(painter, lit);
    paint_badge(painter, full, lit, SCAN_FOUND);
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

/// **6b's badge**: a blue pill above a rectangle's top-left carrying the
/// design's tick and `words`.
///
/// The words are an argument rather than [`LOCKED_ON`] baked in, because the
/// surface now has two states that want this exact pill and they say different
/// things: the drag's lock-on, and the reveal's [`SCAN_FOUND`]. Everything
/// else about it -- the height, the padding, the radius, the gap, the tick's
/// box and its stroke, the type -- is one set of numbers from one design, and
/// a second copy of the pill would be a second set to keep in step with it.
/// The width follows the words, which is what the design's `padding: 0 10px`
/// on a flex row means.
fn paint_badge(painter: &egui::Painter, full: egui::Rect, sel: egui::Rect, words: &str) {
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
        log::warn!(
            "region overlay: no window titled {title:?} to {} screen captures",
            if exclude { "take out of" } else { "put back into" }
        );
        return;
    };
    let affinity = if exclude {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // **The outcome is logged rather than ignored**, for this module's
    // standing reason: nothing useful can be DONE about a refusal, which is
    // not the same as nothing useful being learned from one.
    // A mask that was silently skipped is a capture taken through this
    // overlay's own dim, and the user sees that as "no code in that region"
    // for a code that is plainly on screen -- a symptom with no trail at all
    // until this line existed.
    //
    // The safety argument is unchanged and still holds either way: a mask that
    // was refused leaves a window in the capture, and an unmask that was
    // refused leaves a window out of other people's captures, which is the
    // safe direction of the two.
    let result = unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut _), affinity) };
    match result {
        Ok(()) => log::info!(
            "region overlay: {title:?} is now {} screen captures",
            if exclude { "out of" } else { "back in" }
        ),
        Err(e) => log::warn!(
            "region overlay: SetWindowDisplayAffinity refused on {title:?} ({e}); a capture \
             taken now will include that window"
        ),
    }
}

/// [`RegionOverlay::stand_aside`]'s way down: `ShowWindow` with
/// `SW_SHOWMINNOACTIVE`.
///
/// # `SW_SHOWMINNOACTIVE` and not `SW_MINIMIZE`, which was measured
///
/// The two differ in one clause of the documentation and it turns out to
/// decide whether this feature works. `SW_MINIMIZE` minimises the window
/// **and activates the next top-level window in the Z order**, which is
/// somebody else's -- so the moment Deskwarden goes down, the foreground
/// leaves this process. Measured with a probe that opens the real overlay,
/// minimises the root and then asks Windows which window has the foreground:
/// with `SW_MINIMIZE` the answer was a browser, and the overlay -- which is
/// always-on-top, so still perfectly visible -- was no longer the window the
/// keyboard was going to. **Escape is how this surface is cancelled.** An
/// overlay covering every monitor that cannot be dismissed from the keyboard
/// is close to the worst thing this module could ship.
///
/// `SW_SHOWMINNOACTIVE` minimises and activates nothing. The same probe, same
/// sequence, answered "the overlay" at every check afterwards.
///
/// # What is logged, and why it is not the return value
///
/// `ShowWindow` returns the window's **previous visibility**, not success, so
/// its return value cannot answer "did this work" and is not treated as
/// though it could. `IsIconic` can, and does -- on the handle already
/// resolved, with no second `EnumWindows`, because that lookup was measured at
/// hundreds of milliseconds in an unoptimised build and this is on the frame
/// the overlay takes the screen -- a frame whose cost the user is watching.
///
/// Logged either way rather than discarded, which is this module's rule since
/// three silently-failing Win32 calls cost a day: a window that did not go
/// down is a user staring at Deskwarden sitting on top of the code they are
/// being asked to point at, and without this line there is nothing in
/// `deskwarden.log` that tells that apart from a window that went down fine.
/// **Where the user is, read at the instant the route starts**: the centre of
/// Deskwarden's own window in virtual-screen physical pixels.
///
/// This is the anchor [`crate::screen_capture::active_display`] picks a
/// monitor by, and the centre rather than the top-left because a window
/// straddling two screens belongs to the one that has most of it -- which is
/// also what `MonitorFromWindow(MONITOR_DEFAULTTONEAREST)` answers and what
/// `app::clamp_to_monitor` already relies on for the autofill card.
///
/// `None` in three cases, all of which fall through to the cursor in
/// `active_display`: there is no window with that title (a test process, or a
/// probe whose root is titled something else), `GetWindowRect` refused, or the
/// window is already minimised. **That last one is the point of the check.** A
/// minimised window's rectangle on Windows is `-32000, -32000` -- a sentinel,
/// not a position -- and handing it to a nearest-monitor rule would reliably
/// pick the top-left screen of the desktop rather than the one the user is on.
/// This function is only ever called from `RegionOverlay::open`, before
/// anything in this module has minimised anything, so an iconic window here
/// means the *user* had the app minimised when the press arrived, and the
/// cursor is then the better answer.
///
/// Logged either way, like every other Win32 call in this module: "the overlay
/// came up on the wrong screen" is a report that cannot be answered without
/// knowing what this returned.
fn own_window_centre() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic};

    let title = crate::vault_window::WINDOW_TITLE;
    let Some(hwnd) = crate::foreground::own_window_titled(title) else {
        log::info!(
            "region overlay: no window titled {title:?} to anchor the overlay's display on; \
             falling back to the mouse cursor"
        );
        return None;
    };
    let handle = HWND(hwnd as *mut _);
    if unsafe { IsIconic(handle) }.as_bool() {
        log::info!(
            "region overlay: {title:?} is already minimised, so its rectangle is Windows' \
             -32000 sentinel and not a position; falling back to the mouse cursor"
        );
        return None;
    }
    let mut rect = RECT::default();
    if let Err(e) = unsafe { GetWindowRect(handle, &mut rect) } {
        log::warn!(
            "region overlay: GetWindowRect refused on {title:?} ({hwnd:#x}): {e}; falling back \
             to the mouse cursor to choose the overlay's display"
        );
        return None;
    }
    let centre = (
        rect.left + (rect.right - rect.left) / 2,
        rect.top + (rect.bottom - rect.top) / 2,
    );
    log::info!("region overlay: {title:?} is at {rect:?}, anchoring the overlay on {centre:?}");
    Some(centre)
}

/// **What rectangle the OS actually gave the overlay's window**, against the
/// one that was asked for.
///
/// # Why this is logged and not merely trusted
///
/// A `ViewportBuilder`'s position and size are a *request*. Everything between
/// this module and `CreateWindowExW` is allowed to renegotiate it: `winit`
/// creates every window at `CW_USEDEFAULT` and applies the size and the
/// position afterwards, converting logical points to pixels with the scale
/// factor of whichever monitor the window happened to land on; Windows then
/// sends `WM_DPICHANGED` if the result straddles two monitors at different
/// scaling, and `winit`'s handler answers that by moving and resizing the
/// window to the rectangle Windows suggests. Every one of those is invisible
/// from here, and the symptom of all of them is the same: a window that is not
/// the size it asked to be, with the part of its redirection surface nobody
/// has painted showing through as a flat rectangle.
///
/// So the two rectangles are compared and the answer is written down. On the
/// hidden frame -- the one that enters [`Appearing::Hidden`] -- this is the last chance to see the
/// geometry before the user does; on the frame it is shown it is the record of
/// what they saw. A mismatch is a `warn` with both rectangles in it, which is
/// the line the next multi-monitor report will be answered from.
/// **Takes the overlay's window down and puts it exactly where it belongs,
/// before anyone has seen it** -- `ShowWindow(SW_HIDE)` and then
/// `SetWindowPos`, in that order, on a window this module has not shown yet.
///
/// # The popup this exists to remove, in the owner's words
///
/// > 1. White small popup shows up
/// > 2. Same popup moves position
///
/// That is a window created at `CW_USEDEFAULT`, shown before it has painted
/// anything, and then sized and moved -- which is exactly the order `winit`'s
/// `on_create` runs in (`platform_impl/windows/window.rs`, with its own
/// comment "Set visible before setting the size"): `set_visible`, then
/// `request_inner_size`, then `set_outer_position`. Everything after the
/// `set_visible` is a step the user watches.
///
/// # Why `with_visible(false)` was not enough
///
/// It should have been, and on the machine this was measured on it is:
/// `egui_winit::create_winit_window_attributes` really does apply
/// `.with_visible(visible.unwrap_or(true))`, `winit` really does leave
/// `WindowFlags::VISIBLE` out of the initial style, and a probe sampling the
/// real overlay every ~9 ms saw the window hidden at its final rectangle for
/// 1.8 s and then shown once, at that rectangle, with **0 samples** at any
/// other size. The owner saw the popup anyway.
///
/// Rather than keep arguing with three crates about who is allowed to show a
/// window, this module takes the window. It already resolves its own `HWND` by
/// title and already makes Win32 calls on it -- the DWM call, the capture
/// mask, the minimise -- so one more is in keeping rather than a new kind of
/// thing. `SW_HIDE` undoes any show that has happened, whoever made it, and
/// `SetWindowPos` on the hidden window makes the move step happen where nobody
/// can see it. Both are idempotent: on a window that is already hidden and
/// already placed, which is what the builder should have produced, they are
/// no-ops that cost two system calls.
///
/// # Run on every frame until the overlay shows itself, not once
///
/// The window is created by `eframe` between frames, so the earliest this can
/// run is the first root frame after that -- and anything that showed the
/// window did it before then. One call would leave whatever happened in that
/// gap on screen until the next one. Driving it from every `Appearing::Waiting`
/// frame bounds the exposure at one frame and, more importantly, guarantees
/// that the *last* thing to happen before [`show_painted`] is this: hidden, and
/// at the right rectangle.
///
/// `SWP_NOACTIVATE` and `SWP_NOZORDER` because neither is this call's business:
/// the raise is [`crate::foreground::raise_window`]'s job and happens after the
/// show, and activating a hidden window is how the overlay would lose the
/// keyboard before it ever had it.
///
/// Logged either way, like every other Win32 call here.
fn hide_and_place(title: &str, display: ScreenRect) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindowVisible, SetWindowPos, ShowWindow, HWND_TOP, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE,
    };

    let Some(hwnd) = crate::foreground::own_window_titled(title) else {
        return;
    };
    let handle = HWND(hwnd as *mut _);
    // Read BEFORE hiding, because "was it visible when we got here" is the
    // whole question the owner's report asks and the only place it can be
    // answered is this line.
    let was_visible = unsafe { IsWindowVisible(handle) }.as_bool();
    if was_visible {
        log::warn!(
            "region overlay: {title:?} was ALREADY VISIBLE before this module showed it -- \
             something other than `Appearing` put it on screen. Hiding it again; the user may \
             have seen a frame of it. This is the popup the owner reported"
        );
    }
    // The return is the PREVIOUS visibility, which was just read properly.
    let _ = unsafe { ShowWindow(handle, SW_HIDE) };
    if let Err(e) = unsafe {
        SetWindowPos(
            handle,
            HWND_TOP,
            display.left,
            display.top,
            display.width() as i32,
            display.height() as i32,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    } {
        log::warn!(
            "region overlay: SetWindowPos refused on {title:?} ({hwnd:#x}) for {display:?}: \
             {e}; the window keeps whatever rectangle it was given"
        );
    }
}

/// **Shows the overlay's window, once it has been painted: this module's own
/// `ShowWindow`, and NOT `ViewportCommand::Visible(true)`.**
///
/// # Why not the viewport command: what `winit` does with it
///
/// The show used to be `ctx.send_viewport_cmd_to(.., Visible(true))`.
/// `egui_winit::process_viewport_command` turns that into
/// `window.set_visible(true)`; `winit`'s Windows backend implements that as a
/// change to its own `WindowFlags`, applied by `WindowFlags::apply_diff`
/// (`winit` 0.30.13, `platform_impl/windows/window_state.rs`):
///
/// ```text
/// if new.contains(WindowFlags::VISIBLE) { ShowWindow(window, SW_SHOWNOACTIVATE or SW_SHOW) }
/// ...
/// if !new.contains(WindowFlags::VISIBLE) { ShowWindow(window, SW_HIDE) }
/// if diff != WindowFlags::empty() {
///     let (style, style_ex) = new.to_window_styles();
///     SetWindowLongW(window, GWL_STYLE, style as i32);
///     SetWindowLongW(window, GWL_EXSTYLE, style_ex as i32);
///     SetWindowPos(window, 0, 0, 0, 0, 0, SWP_NOZORDER | SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED ..);
/// ```
///
/// Every flag change rewrites the window's styles from `winit`'s own flags,
/// **assigning** rather than or-ing. That is how this module found out: in
/// its layered-window era it or-ed `WS_EX_LAYERED` onto the window while
/// hidden, sent the command, and the owner's log read the bit back as gone on
/// the frame after the show (`0x40118 -> 0xc0118`, twice per run). The
/// layering is gone now -- [`RegionOverlay::take_picture`] -- and the window
/// carries no style of this module's for `winit` to strip. What is still true
/// is that a window `winit` believes hidden is one it will `ShowWindow(SW_HIDE)`
/// on the next flag diff it computes, and that the command's own `ShowWindow`
/// is followed by a `SetWindowPos(SWP_FRAMECHANGED)`, a full non-client
/// recalculation, on the frame the user first sees the surface. This call is
/// the one that changes only visibility.
///
/// # `SW_SHOWNOACTIVATE`
///
/// Exactly what `winit` itself issues on a window's first show (`apply_diff`
/// picks `SW_SHOWNOACTIVATE` until its `MARKER_ACTIVATE` flag is set), so at
/// the level of the OS nothing differs from the command this replaces except
/// what follows it. Not `SW_SHOW`, for the same reason [`hide_and_place`]
/// passes `SWP_NOACTIVATE`: activating the window is
/// [`crate::foreground::raise_window`]'s job, on the same step, once the
/// window is up.
///
/// The window has been painted into before this runs -- see [`Appearing`] --
/// so what the first composite shows is the surface, and at most one frame of
/// it precedes the first swap after the show.
///
/// # What this costs, and what it forbids
///
/// `winit`'s flags go on believing the window is hidden. Two consequences,
/// both checked:
///
/// * `eframe` does not read those flags -- and, checked against `eframe`
///   0.35 rather than remembered, it does not read the window's visibility at
///   all. `glow_integration::run_ui_and_paint` gates on
///   `viewport.info.visible().unwrap_or(true)`, and `ViewportInfo::visible()`
///   is computed from `minimized` and `occluded` alone: `egui_winit` fills
///   `minimized` from `window.is_minimized()` (`IsIconic` on Windows) and
///   never fills `occluded` there, so the answer is `None` and `eframe` takes
///   `true`. The overlay's callback therefore runs and paints from the frame
///   the window exists, hidden or not; `winit`'s `VISIBLE` flag is consulted
///   nowhere on that path. (Which is what lets the window be painted BEFORE
///   this show, and is why the `painted` gate in [`Appearing`] is usually
///   already satisfied on the root frame after the window appears.)
/// * **This module must never send a `ViewportCommand` to its own viewport,
///   and the builder it passes every frame must never change.** Any flag diff
///   `winit` ever computes for this window runs `apply_diff`, whose act for a
///   window its flags call hidden is `ShowWindow(SW_HIDE)`. The builder is a
///   literal, and
///   `the_overlay_is_shown_by_this_module_and_winit_never_touches_it_again`
///   pins the absence of commands. `WM_DPICHANGED` is the one place `winit`
///   changes a flag on its own (`MAXIMIZED = false`), and a flag set to the
///   value it already has is an empty diff.
///
/// Logged either way, and the readback is `IsWindowVisible` rather than the
/// return value, which is the previous visibility and says nothing about
/// success.
fn show_painted(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, ShowWindow, SW_SHOWNOACTIVATE};

    let handle = HWND(hwnd as *mut _);
    // The return is the PREVIOUS visibility; the readback below is the check.
    let _ = unsafe { ShowWindow(handle, SW_SHOWNOACTIVATE) };
    if unsafe { IsWindowVisible(handle) }.as_bool() {
        log::info!(
            "region overlay: shown on {hwnd:#x}, already painted, by this module's own \
             ShowWindow(SW_SHOWNOACTIVATE); winit's window flags were not touched"
        );
    } else {
        log::warn!(
            "region overlay: ShowWindow(SW_SHOWNOACTIVATE) was issued on {hwnd:#x} and \
             IsWindowVisible still says no. The overlay is not on screen -- the user has a \
             scan with no window, and Escape will not reach it"
        );
    }
}

fn log_window_rect(title: &str, step: &str, asked_for: ScreenRect) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let Some(hwnd) = crate::foreground::own_window_titled(title) else {
        log::warn!("region overlay: no window titled {title:?} to measure at {step}");
        return;
    };
    let mut rect = RECT::default();
    if let Err(e) = unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut rect) } {
        log::warn!("region overlay: GetWindowRect refused on {title:?} at {step}: {e}");
        return;
    }
    let got = ScreenRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    };
    if got == asked_for {
        log::info!(
            "region overlay: {step} -- the window is at {got:?} ({}x{}), which is the display \
             that was asked for",
            got.width(),
            got.height()
        );
    } else {
        log::warn!(
            "region overlay: {step} -- the window is at {got:?} ({}x{}) and NOT at the {}x{} \
             display it asked for, {asked_for:?}. Something between this module and \
             CreateWindowExW renegotiated the geometry; the part of the surface no frame has \
             painted will show as a flat rectangle",
            got.width(),
            got.height(),
            asked_for.width(),
            asked_for.height()
        );
    }
}

fn send_window_down(title: &str) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, ShowWindow, SW_SHOWMINNOACTIVE};

    let Some(hwnd) = crate::foreground::own_window_titled(title) else {
        log::warn!(
            "region overlay: no window titled {title:?} to send down; it will stay on top of \
             whatever the user is being asked to point at"
        );
        return;
    };
    let handle = HWND(hwnd as *mut _);
    // The return is the PREVIOUS visibility. Deliberately unused: see above.
    let _ = unsafe { ShowWindow(handle, SW_SHOWMINNOACTIVE) };
    if unsafe { IsIconic(handle) }.as_bool() {
        log::info!("region overlay: {title:?} is minimised for the length of the scan");
    } else {
        log::warn!(
            "region overlay: {title:?} did not go down -- ShowWindow(SW_SHOWMINNOACTIVE) was \
             issued on {hwnd:#x} and IsIconic still says no. The overlay will be on top of it \
             either way, but the user will be dragging over a window they cannot see behind"
        );
    }
}

/// [`RegionOverlay::stand_aside`]'s way back, and **the one call in this
/// module that must not be allowed to silently not happen.**
///
/// # Restoring is not un-minimising
///
/// The owner's sentence ends *"once back - it should be on top again"*, and
/// that is two things. [`crate::foreground::raise_window`] is the crate's one
/// way to do both, and it already does them in the right order: it picks this
/// process's window by title, `SW_RESTORE`s it **because** it is iconic, then
/// asks for the foreground. A bare `ShowWindow(SW_RESTORE)` would put the
/// window back on screen behind whatever the user has since clicked on, which
/// is not what was asked for.
///
/// # A refusal is reported, not swallowed
///
/// `foreground`'s whole design is that Windows declining to hand over the
/// foreground is a **documented outcome** rather than an error: `raise_on`
/// flashes the taskbar button and answers [`crate::foreground::Raised::Flashed`]
/// rather than retrying or working around it, and that judgement is not this
/// module's to revisit. What is this module's is that the answer reaches the
/// log with *this* caller's stakes attached, because the four outcomes mean
/// very different things here:
///
/// * `Front` / `AlreadyInFront` -- what was asked for.
/// * `Flashed` -- the window is back and restored, but behind something. The
///   user has to click its flashing taskbar button. Recoverable, and worth a
///   `warn` because it is the difference between the feature working and the
///   feature appearing to have eaten the app.
/// * `NoWindow` -- nothing matched the title. At `warn`, loudly: it is the one
///   outcome in which a window this module minimised has not been brought
///   back by this call, and the only thing between the user and a lost app is
///   a taskbar button. It is also why the window is minimised rather than
///   hidden.
fn bring_window_back(title: &str) {
    use crate::foreground::Raised;

    match crate::foreground::raise_window(title) {
        outcome @ (Raised::Front | Raised::AlreadyInFront) => log::info!(
            "region overlay: {title:?} is back in front after the scan ({outcome:?})"
        ),
        Raised::Flashed => log::warn!(
            "region overlay: {title:?} was restored but Windows declined to bring it to the \
             front, so its taskbar button is flashing instead. The scan is over and the window \
             is no longer minimised"
        ),
        Raised::NoWindow => log::warn!(
            "region overlay: nothing titled {title:?} to bring back after the scan. If that \
             window was minimised by this overlay it is still minimised, and the user's only \
             way back to it is its taskbar button"
        ),
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
    ///
    /// The place it reports is [`FOUND_AT`], **buffer-relative** exactly as
    /// `codes_in`'s is, so a test can tell whether `scan_screen_with` moved it
    /// onto the desktop.
    fn code_naming_the_size(_: &[u8], w: usize, h: usize) -> qr::Codes {
        qr::Codes::One(
            Zeroizing::new(format!(
                "otpauth://totp/Git%20Host:anovak{w}x{h}?secret=JBSWY3DPEHPK3PXP"
            )),
            FOUND_AT,
        )
    }

    /// Where every scanning stub above says it found its code, in the
    /// **captured buffer's own pixels**. Deliberately not at the origin and
    /// not square, so an implementation that dropped the offset, swapped the
    /// axes, or handed back the whole monitor is a different rectangle from
    /// this one.
    const FOUND_AT: ScreenRect = ScreenRect {
        left: 300,
        top: 200,
        right: 460,
        bottom: 380,
    };

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

    /// **The overlay covers ONE display, and the window is sized from the one
    /// it was given rather than from a second reading of the desktop.**
    ///
    /// The owner's report, and the thing this whole change is: *"just cover
    /// the whole active display with the transparent screen"*. Three things
    /// have to hold together and only the first two are assertable as values;
    /// the third is source, because it is inside a `ViewportBuilder` no
    /// harness in this crate can call.
    #[test]
    fn the_overlay_covers_one_display_and_sizes_itself_from_it() {
        // The rectangle the overlay carries IS the display it was opened on,
        // corner and extent both -- not the bounding box of anything.
        let display = rect(-1280, -200, 0, 520);
        let overlay = RegionOverlay::open_on(display, 1.0).expect("opens");
        assert_eq!(locked(&overlay.inner).display, display);
        // A pointer at the far corner of the viewport is the far corner of
        // THAT display, which is the observable consequence of the extent
        // being one screen's: 1280x720 points at 1.0.
        assert_eq!(
            to_screen(
                (display.left, display.top),
                1.0,
                (display.width() as f32, display.height() as f32)
            ),
            (0, 520)
        );

        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");

        // **The display is chosen once, in `open`.** `show` runs every frame
        // and runs after the minimise; a `screen_capture::active_display` call
        // anywhere in it would be a rectangle chosen from an anchor that no
        // longer exists. See `screen_capture::active_display`.
        assert_eq!(
            code.matches("screen_capture::active_display(").count(),
            1,
            "the active display is resolved somewhere other than `open`, or not at all -- it \
             has to be read before `stand_aside` minimises the window the anchor is the centre \
             of, and carried from there"
        );
        let opener = code
            .split("pub fn open(monitors: &[ScreenRect], points_per_pixel: f32)")
            .nth(1)
            .expect("`RegionOverlay::open` was restructured");
        assert!(
            opener
                .split("fn open_on")
                .next()
                .unwrap()
                .contains("screen_capture::active_display(own_window_centre(), monitors)"),
            "`open` no longer anchors the display on Deskwarden's own window"
        );

        // **And `show` sizes the window from what was carried, not from a
        // fresh enumeration.** This is the half that used to read
        // `monitor_bounds()` every frame for the extent while taking the
        // corner from `Inner`; the two could disagree, and did so exactly when
        // the desktop changed under a live overlay.
        let builder = code
            .split("ViewportBuilder::default()")
            .nth(1)
            .expect("the viewport builder is gone")
            .split(".with_visible(false)")
            .next()
            .unwrap();
        assert!(
            builder.contains("display.width() as f32 / scale")
                && builder.contains("display.height() as f32 / scale"),
            "the overlay's window is sized from something other than the display it carries"
        );
        assert!(
            !builder.contains("monitor_bounds"),
            "the viewport builder reads the desktop again instead of using the display `open` \
             chose"
        );
    }

    /// **The scan still walks EVERY monitor, on both of its paths.**
    ///
    /// The overlay narrowed to one display and this deliberately did not. See
    /// [`scan_screen_with`]: it opens no window, it is decode-only, and it is
    /// the path that usually answers the route without the user dragging
    /// anything -- narrowing it to match the surface would make the feature
    /// worse at its main job. Both call sites are pinned because there are
    /// exactly two and one of them is inside the viewport callback.
    #[test]
    fn the_scan_still_reads_every_monitor_even_though_the_overlay_does_not() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert_eq!(
            code.matches("scan_screen_with(").count(),
            3,
            "the number of `scan_screen_with` call sites changed: one definition, the prescan \
             and the All screens chip"
        );
        // Neither of the two calls narrows what it is given, and there are
        // exactly two desktop enumerations left -- one per scan. Comment lines
        // are dropped first, because this module's docs name the function
        // constantly and a raw count would be about the prose.
        let statements: String = code
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            statements.matches("screen_capture::monitor_bounds()").count(),
            2,
            "a `monitor_bounds()` reading was added or removed; the only two left should be the \
             prescan's and the All screens chip's, both feeding `scan_screen_with`"
        );
        assert!(
            code.contains("self.apply_scan(scan_screen_with(&RegionSeams::production(), &monitors));"),
            "the prescan no longer scans the monitor list it enumerated"
        );
        assert!(
            code.contains("&screen_capture::monitor_bounds(),"),
            "the All screens chip no longer scans every monitor"
        );
        // And the chip that runs it is named for that, rather than for the
        // surface it sits on. See `WHOLE_SCREEN_HINT`.
        assert_eq!(WHOLE_SCREEN_HINT, "All screens");
    }

    /// No monitors -- and a degenerate one -- give no overlay rather than a
    /// zero-sized always-on-top window the user cannot dismiss.
    ///
    /// This used to assert the same thing of a `whole_screen` that is gone:
    /// the overlay's rectangle is now one display, chosen by
    /// `screen_capture::display_holding`, and the refusal it is asserting has
    /// moved there with it. `the_active_display_is_the_one_holding_the_anchor`
    /// and its neighbours in `screen_capture` are where the choice is tested;
    /// this is about what `open` does with "there is no display".
    #[test]
    fn no_monitors_is_no_overlay() {
        assert!(RegionOverlay::open(&[], 1.0).is_none());
        assert!(RegionOverlay::open(&[rect(10, 10, 10, 10)], 1.0).is_none());
        // The same refusal at the seam the tests drive, where nothing reads a
        // desktop at all.
        assert!(RegionOverlay::open_on(rect(10, 10, 10, 10), 1.0).is_none());
        // Positive control: with a monitor, one really does open, so the
        // `is_none`s above are about the missing rectangle.
        assert!(RegionOverlay::open(&[rect(0, 0, 800, 600)], 1.0).is_some());
        assert!(RegionOverlay::open_on(rect(0, 0, 800, 600), 1.0).is_some());
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
        // The third comparison used to be against `preflight_card`'s title.
        // That card was design 4b's send confirmation and it has been removed
        // -- see `vault_window::preflight`'s module doc -- so the daemon's
        // unlock prompt takes its place here. Still three, and still a card
        // the daemon can have on screen while this overlay is up.
        assert_ne!(REGION_TITLE, crate::unlock_prompt::UNLOCK_PROMPT_TITLE);
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
        // **The one number on this surface that is deliberately NOT the
        // design's**, so it is asserted as the property it was changed to have
        // rather than as a figure copied out of the CSS.
        //
        // The design is `background: #201e1d` with the desktop over it at
        // `opacity: 0.32` -- this ink at 68%, `0.68 * 255 = 173.4`. The owner
        // asked for a lighter wash ("don't do hard overlay - needs to be
        // transparent enough") because the design's mock fakes a bright white
        // desktop behind that 68% and a real one is darker. See `DIM_ALPHA`'s
        // own doc for the whole argument.
        //
        // What is held here is the departure and its direction, not 115: the
        // wash must let MORE of the desktop through than the design's 32%, and
        // must still be a wash rather than a tint.
        let through = 1.0 - f32::from(DIM_ALPHA) / 255.0;
        assert!(
            through > 0.32,
            "the dim is back at or below the design's own 32% desktop; it was lightened on \
             purpose -- see DIM_ALPHA"
        );
        assert!(
            (0.40..0.65).contains(&through),
            "the desktop now comes through at {through}, outside the band DIM_ALPHA argues for: \
             under 0.40 the selection stops reading as lit, over 0.65 there is no dim left"
        );

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
        // **"All screens" and not "Whole screen".** The chip is named for the
        // scan it runs, which walks every monitor, and no longer for the
        // surface it sits on, which now covers one. See `WHOLE_SCREEN_HINT`
        // for the argument, and `RegionOverlay::open` for what narrowed.
        assert_eq!(WHOLE_SCREEN_HINT, "All screens");
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
            ScreenScan::Found(..) => panic!("the scan found a code where it should not have"),
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
            ScreenScan::Found(text, at) => {
                assert_eq!(&*text, "otpauth://totp/Git%20Host:anovak1920x1080?secret=JBSWY3DPEHPK3PXP");
                // On the primary monitor the placement is the identity, which
                // is why the second-monitor test below is the one that
                // matters.
                assert_eq!(at, FOUND_AT);
            }
            other => panic!("one code came back as {other:?}"),
        }
    }

    /// **A code found on a monitor whose origin is not `(0, 0)` comes back
    /// placed on the desktop, not in that monitor's own pixels.**
    ///
    /// The arithmetic most likely to be wrong in this whole feature, and the
    /// one that no amount of use on a single screen can exercise: the box
    /// `qr::codes_in` reports starts at the captured buffer's top-left, and
    /// the overlay draws in virtual-screen coordinates. On the primary
    /// monitor the two are the same. On any other monitor a box left unplaced
    /// is drawn a monitor's width away from the code it is pointing at -- and
    /// on a monitor left of the primary it is drawn on the primary, which is
    /// where the user is looking, so it would look like a mark round nothing.
    ///
    /// Both directions are asserted, because a sign error passes one and
    /// fails the other.
    #[test]
    fn a_code_found_on_a_second_monitor_is_reported_where_it_is_on_the_desktop() {
        fn only_on_the_second(_: &[u8], w: usize, _: usize) -> qr::Codes {
            if w == 1280 {
                qr::Codes::One(
                    Zeroizing::new("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP".into()),
                    FOUND_AT,
                )
            } else {
                qr::Codes::None
            }
        }
        let placed = |monitors: &[ScreenRect]| {
            match scan_screen_with(&scan_seams(flat_capture, only_on_the_second), monitors) {
                ScreenScan::Found(_, at) => at,
                other => panic!("the code was not found at all: {other:?}"),
            }
        };

        // To the right of the primary: the commonest two-monitor desktop.
        assert_eq!(
            placed(&[rect(0, 0, 1920, 1080), rect(1920, 0, 3200, 720)]),
            rect(2220, 200, 2380, 380)
        );
        // Left of and above it, where the origin is negative -- the case a
        // `saturating_add` of an unsigned offset, or a forgotten offset, both
        // get wrong.
        assert_eq!(
            placed(&[rect(-1280, -200, 0, 520), rect(0, 0, 1920, 1080)]),
            rect(-980, 0, -820, 180)
        );
        // The size is the code's in every arrangement: a placement that
        // changed it would be a mark the wrong size in the right place.
        for monitors in [
            vec![rect(0, 0, 1920, 1080), rect(1920, 0, 3200, 720)],
            vec![rect(-1280, -200, 0, 520), rect(0, 0, 1920, 1080)],
        ] {
            let at = placed(&monitors);
            assert_eq!(
                (at.width(), at.height()),
                (FOUND_AT.width(), FOUND_AT.height()),
                "{monitors:?}"
            );
        }
    }

    /// **Every monitor is read, not just the biggest one.**
    ///
    /// This is the reason `scan_screen_with` walks monitors instead of
    /// handing the bounding box of the desktop to one capture:
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
                qr::Codes::One(
                    Zeroizing::new("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP".into()),
                    FOUND_AT,
                )
            } else {
                qr::Codes::None
            }
        }
        let monitors = [rect(0, 0, 1920, 1080), rect(1920, 0, 3200, 720)];
        assert!(matches!(
            scan_screen_with(&scan_seams(flat_capture, only_on_the_small_one), &monitors),
            ScreenScan::Found(..)
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
        match scan_screen_with(&scan_seams(flat_capture, code_naming_the_size), &same) {
            // And it comes back at the FIRST monitor's place, not the
            // second's. Both monitors reported the code at the same offset
            // into their own buffers, so the two placed boxes are a monitor's
            // width apart -- and the one that belongs to the payload being
            // held is the first. A mark at the second would ring the copy.
            ScreenScan::Found(_, at) => assert_eq!(at, FOUND_AT),
            other => panic!("one code on two monitors came back as {other:?}"),
        }

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
            ScreenScan::Found(..)
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
        let shown = format!(
            "{:?}",
            ScreenScan::Found(Zeroizing::new(secret.to_string()), FOUND_AT)
        );
        assert!(!shown.contains("JBSWY3DPEHPK3PXP"), "{shown}");
        assert!(!shown.contains("otpauth"), "{shown}");
        assert!(shown.contains("not shown"), "{shown}");
        // The place is printed: it is where on the user's own screen their
        // code is, not a secret, and it is the one useful thing in a failure
        // message about a scan that found something.
        assert!(shown.contains("left: 300"), "{shown}");
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

    /// **A scan that found a code reveals it and then answers 6c; one that
    /// did not leaves the overlay up with a reason.**
    ///
    /// The two halves of `apply_scan`. The first used to end the overlay on
    /// the spot; it now opens the reveal and the overlay stays up until the
    /// clock runs out -- which is the change, and the outcome underneath it is
    /// the same `Decoded` it always was.
    #[test]
    fn a_scan_either_answers_or_explains_itself() {
        let found = RegionOverlay::open(&[rect(0, 0, 1920, 1080)], 1.0).expect("opens");
        found.apply_scan(ScreenScan::Found(
            Zeroizing::new("otpauth://totp/x".into()),
            FOUND_AT,
        ));
        // Still up, and holding the answer back: `take_outcome` is only
        // reached by a caller once `show` has answered `false`, and it has
        // not.
        assert!(found.is_open(), "the reveal did not keep the overlay up");
        assert_eq!(found.view().reveal, Some(rect_pts(300.0, 200.0, 460.0, 380.0)));
        // The clock runs, and then it is over -- with the outcome it recorded
        // when it found the code.
        let t0 = Instant::now();
        assert!(found.reveal_step(t0).is_some());
        assert!(found.is_open());
        assert!(found.reveal_step(t0 + REVEAL_DWELL).is_none());
        assert!(!found.is_open(), "the reveal did not end the overlay");
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

    /// **The dim is a wash in the framebuffer over a picture of the desktop,
    /// with a hole where the selection is -- and the window asks the
    /// compositor for nothing.**
    ///
    /// This pin has been the opposite of itself. It used to hold that the dim
    /// was painted OPAQUE and that `DIM_ALPHA` reached the surface only as the
    /// window's layered alpha, because a framebuffer alpha would have
    /// compounded with the window's. That window was accepted by Windows with
    /// `WS_EX_LAYERED` on and `LWA_ALPHA` at 115 for nine seconds of black
    /// screen -- the log lines are in `take_picture`'s doc -- so the window is
    /// opaque on purpose now, the ground is a picture, and the alpha is back
    /// in the paint. What a test can hold is the shape of that: the ground is
    /// painted from a texture before anything else, the wash is `DIM_ALPHA`
    /// in the framebuffer and nothing else is, nothing is painted inside the
    /// lit rectangle, and no statement in the module names any of the
    /// compositing mechanisms that were accepted and wrong.
    #[test]
    fn the_dim_is_painted_in_the_framebuffer_over_a_picture_of_the_desktop() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        let statements: String = code
            .lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        // The painters that make the dim, with their prose cut away: `draw`,
        // `paint_ground`, `paint_dim_around` and `paint_reveal`, up to the
        // first function that paints something other than the ground and the
        // wash.
        let painters = statements
            .split("pub fn draw(")
            .nth(1)
            .expect("`draw` is gone")
            .split("fn paint_selection_edge(")
            .next()
            .expect("the paint section no longer ends where it did");
        let ground = painters
            .find("paint_ground(&painter, full, view.picture);")
            .expect("`draw` no longer lays the picture down");
        assert!(
            ground < painters.find("view.reveal").expect("the reveal branch is gone"),
            "the ground is painted after the reveal branch returns, so a reveal has no \
             picture under it"
        );
        assert!(
            painters.contains("painter.image("),
            "the ground is no longer painted from the picture's texture"
        );
        assert!(
            painters.contains("dim_wash()"),
            "the dim is no longer painted as `dim_wash` in the framebuffer"
        );
        assert!(
            !painters.contains("rect_filled(lit"),
            "something is painted inside the lit rectangle again. The selection is a hole \
             in the wash -- the framed pixels at full strength -- and a fill there is the \
             'drag a box completely black' the owner reported"
        );
        // **`DIM_ALPHA` reaches the surface through `dim_wash` and nowhere
        // else**: its definition, and the one use. A second use is a second
        // dim, and it would show as nothing but a screen the owner says is
        // too dark.
        assert_eq!(
            statements.matches("DIM_ALPHA").count(),
            2,
            "`DIM_ALPHA` is used somewhere other than its definition and `dim_wash`"
        );
        assert!(
            statements.contains("pub fn dim_wash() -> egui::Color32 {"),
            "`dim_wash` is gone"
        );
        // **No compositing mechanism anywhere in a statement.** Four were
        // shipped, every one accepted by the OS and wrong on the glass; the
        // prose in `take_picture` names them so that nobody tries a fifth,
        // which is why this reads statements and not the source.
        for gone in [
            "WS_EX_LAYERED",
            "SetLayeredWindowAttributes",
            "LWA_ALPHA",
            "LWA_COLORKEY",
            "DwmEnableBlurBehindWindow",
            "DwmExtendFrameIntoClientArea",
            "DWM_BLURBEHIND",
            "DwmSetWindowAttribute",
            ".with_transparent(true)",
        ] {
            assert!(
                !statements.contains(gone),
                "`{gone}` is back. Every way of asking the compositor to make this window \
                 see-through has been accepted by Windows and been wrong on the owner's \
                 monitor; the window is opaque and paints a picture instead. See \
                 `take_picture`"
            );
        }
        // The inks. The dim's ink is opaque and the design's; the wash is
        // that ink at `DIM_ALPHA`, alpha intact through premultiplication.
        assert_eq!((DIM_INK.r(), DIM_INK.g(), DIM_INK.b(), DIM_INK.a()), (0x20, 0x1e, 0x1d, 255));
        assert_eq!(dim_wash().a(), DIM_ALPHA, "the wash is not at `DIM_ALPHA`");
        // The plates are the design's own numbers, and they compose over the
        // wash in one framebuffer -- which is what makes the bar a plate
        // again. See `BAR_BG_ALPHA`.
        assert_eq!(BAR_BG_ALPHA, 235);
        assert_eq!(SIZE_BG_ALPHA, 219);
        assert_eq!(HALO_ALPHA, 71);
        assert_eq!(SELECTION_RING, 2.0);
        // The vault window's root viewport does NOT ask for transparency
        // either: on Windows that flag only reaches the GL config template,
        // where it asks for a colour-key pixel format no driver has and a
        // strict driver may answer with no formats at all -- a startup panic
        // inside `eframe`'s config picker.
        let vault = include_str!("vault_window/mod.rs").replace("\r\n", "\n");
        let vault = vault.split("#[cfg(test)]").next().unwrap();
        assert!(
            !vault.contains(".with_transparent(true)"),
            "the vault window asks for transparency again"
        );
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
        // `show` unmasks on both of its ways out -- the guard at the top that
        // finds a finished overlay, and the frame the overlay ends on -- and
        // `Drop` is the backstop under both.
        //
        // **There used to be a third**, and its removal is the reveal's doing
        // rather than an oversight. `show` had an early return for "the scan
        // found one code, so the overlay is over before it began"; a found
        // code now opens the reveal and leaves the overlay up, so that branch
        // could never be taken and a dead unmask with a comment claiming a
        // reason is worse than no unmask. Every ending still runs through one
        // of the two below.
        assert_eq!(
            code.matches("self.mask_own_window(false);").count(),
            2,
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

    /// **Deskwarden's own window goes down for the scan and comes back on
    /// every single way out.**
    ///
    /// Held to exactly the shape `the_mask_is_bookkept_so_that_it_always_comes
    /// _back_off` holds the mask to, and for a worse reason: a mask left on is
    /// a window missing from screenshots, and a window left down is the user's
    /// app apparently gone. `ShowWindow` is a call on a real window and there
    /// is none in a test process, so what is asserted is the bookkeeping that
    /// decides when it happens.
    #[test]
    fn the_window_is_bookkept_so_that_it_always_comes_back_up() {
        let overlay = RegionOverlay::open(&[rect(0, 0, 800, 600)], 1.0).expect("opens");
        let held = locked(&overlay.inner);
        assert!(!held.aside, "a fresh overlay has sent nothing down");
        assert!(held.aside_at.is_none(), "a fresh overlay has a stale settle deadline");
        drop(held);

        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(
            code.contains("if held.aside == away {"),
            "standing aside is no longer idempotent, so the OS call no longer lands on the \
             transitions"
        );
        // **The count is the whole test.** Two ways out of `show` -- the guard
        // at the top that finds a finished overlay, and the frame the overlay
        // ends on -- which between them cover a code found, none found,
        // several found, a refusal, Escape and the close button, because all
        // six end by clearing `open` and every frame after that takes one of
        // the two.
        assert_eq!(
            code.matches("stand_aside(false);").count(),
            2,
            "`show` no longer brings the vault window back on every way out of it"
        );
        assert_eq!(
            code.matches("stand_aside(true);").count(),
            1,
            "the vault window is sent down from somewhere other than the one hook that should"
        );
        // And the backstop for the ways out that are not `show` at all: the
        // form closing under a live overlay, the vault locking, a panic
        // unwinding through the frame.
        let dropped = code
            .split("impl Drop for Inner {")
            .nth(1)
            .expect("`Inner` no longer has a `Drop`");
        assert!(
            dropped.contains("if self.aside {") && dropped.contains("bring_window_back("),
            "nothing brings the vault window back when the overlay is dropped rather than \
             closed -- which is the path that loses the user their app"
        );
        // The window it sends down is the vault window, not this one. Sending
        // the overlay itself down would be a full-screen always-on-top window
        // minimising itself out of the user's reach.
        assert_eq!(crate::vault_window::WINDOW_TITLE, "Deskwarden");
        assert_ne!(crate::vault_window::WINDOW_TITLE, REGION_TITLE);
    }

    /// **The window goes down once the overlay is on screen, and not on the
    /// frame that masks it -- which is the one thing here that was measured
    /// rather than reasoned.**
    ///
    /// Between `PrescanStep::Mask` and `PrescanStep::Scan` this overlay has
    /// registered no viewport, so the root is an eframe app whose only window
    /// is the one about to be minimised -- and a minimised eframe root with no
    /// other viewport takes **no further frames at all**. A settle deadline
    /// set on that frame is never reached and the route hangs. `MINIMISE_SETTLE`'s
    /// doc carries the measurement; this keeps the code on the right side of
    /// it.
    ///
    /// It used to say "the overlay's first painted frame" and meant the
    /// viewport callback's first frame. The window is created hidden now, that
    /// callback does not run until it is shown, and both the raise and the
    /// minimise happen from [`RegionOverlay::appear`] on the root's frame --
    /// one frame after the show, when the overlay is not merely registered but
    /// visible. Same property, stronger guarantee, different place; see
    /// [`Appearing`].
    #[test]
    fn the_window_goes_down_once_the_overlay_is_on_screen() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();

        // Nothing in the prescan arms sends it down. `Mask` masks and returns.
        let prescan = code
            .split("match self.prescan_step(Instant::now()) {")
            .nth(1)
            .expect("`show` no longer drives the prescan through a match")
            .split("let (display, scale) = {")
            .next()
            .expect("the prescan match no longer ends where it did");
        assert!(
            !prescan.contains("stand_aside"),
            "the vault window is sent down from inside the prescan, where the root is its own \
             only window -- see MINIMISE_SETTLE for why that never comes back"
        );
        assert!(
            prescan.contains("self.mask_own_window(true);"),
            "the prescan no longer masks the vault window before it captures"
        );

        // It goes down in `appear`, AFTER the overlay has taken the
        // foreground. `SW_SHOWMINNOACTIVE` activates nothing, so a foreground
        // this window already holds is one it keeps -- which is what leaves
        // Escape working with the app minimised.
        let hook = code
            .split("fn appear(&self, ctx: &egui::Context) {")
            .nth(1)
            .expect("`appear` is gone")
            .split("\n    }")
            .next()
            .expect("`appear` no longer ends where it did");
        let raise = hook.find("raise_window(REGION_TITLE);").expect("the raise is gone");
        let down = hook.find("stand_aside(true);").expect("the window is never sent down");
        assert!(
            raise < down,
            "the vault window is sent down before the overlay has asked for the foreground, so \
             the overlay may never get it and Escape may never reach it"
        );
        assert!(
            hook.find("exclude_from_capture(REGION_TITLE);").expect("the mask is gone") < down,
            "the overlay stopped excluding itself from captures before standing the vault \
             window down"
        );
        // **And both of those are in the step AFTER the one that shows the
        // window.** `foreground::pick` skips invisible windows, so a raise on
        // the frame that merely asked for the show finds nothing; and a
        // minimise on that frame would be a minimise with no visible viewport
        // of this process left, which is the hang `MINIMISE_SETTLE` records.
        let shown = hook
            .find("show_painted(hwnd);")
            .expect("the overlay is never shown, so it is a window nobody can see");
        assert!(
            shown < raise && shown < down,
            "the overlay is raised or minimised into on the same step that shows it -- the \
             window has not painted on that step, so a raise there lands the foreground on a \
             surface with nothing on it"
        );
        assert!(
            hook.find("appearing.on_screen(painted)")
                .expect("the second step's gate is gone, or no longer waits for a paint")
                < raise,
            "the raise and the minimise are no longer behind `Appearing::on_screen(painted)`, \
             so they can run on the frame that only showed the window -- or twice, or before \
             anything has been painted into it"
        );
    }

    /// **It is a minimise, not a hide, and the source says so in one place.**
    ///
    /// `SW_HIDE` would look better -- instant, no taskbar button -- and is the
    /// one option whose failure mode cannot be recovered from: a hidden window
    /// that does not come back is unreachable, where a minimised one is a
    /// taskbar click away. It would also put the vault window into the state
    /// `vault_window`'s `keep_ui_loaded` machinery believes only it produces,
    /// with none of the bookkeeping (`hidden`, `close_or_hide`,
    /// `spawn_show_waiter`) that goes with it -- the same distinction that
    /// file's own `ChromeAction::Minimize` arm is built around.
    ///
    /// It used to forbid `ViewportCommand::Visible` outright, then allowed
    /// exactly one addressed at the overlay's own viewport, and now forbids it
    /// outright again -- for a new reason, which is the one worth pinning.
    /// The addressed command was how the overlay was shown, and it went
    /// through `winit`, which rewrote the window's ex-style on the way and
    /// took the layering off: see [`show_unseen`]. So the overlay is shown by
    /// `ShowWindow` and no `Visible` of any shape may exist here: not for
    /// the vault window, which `keep_ui_loaded` owns, and not for the
    /// overlay, which `winit` must never be given a reason to restyle.
    #[test]
    fn the_vault_window_is_minimised_rather_than_hidden() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        // **Comments cut off, because every needle below is a negative one.**
        // The argument for minimising rather than hiding is written out above
        // in prose, and that prose names `SW_HIDE` and `SW_MINIMIZE` -- so a
        // bare `contains` over the raw source fails on the doc that explains
        // why the code does not do those things. `foreground::tests::code`
        // exists for the same reason and is copied rather than shared:
        // it is `#[cfg(test)]` in a module this one cannot reach.
        let code: String = source
            .split("#[cfg(test)]")
            .next()
            .unwrap()
            .lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        // **The vault window is never hidden**, and the check is now scoped to
        // the function that touches it rather than to the whole file, because
        // this module *does* hide one window: its own, in `hide_and_place`,
        // before it has ever been shown. The two are opposites. Hiding the
        // user's application is how it becomes unreachable; hiding the
        // overlay's own window, which the user has not seen and which this
        // module shows itself a frame later, is how the popup stops happening.
        let down = code
            .split("fn send_window_down(title: &str) {")
            .nth(1)
            .expect("`send_window_down` is gone")
            .split("\n}")
            .next()
            .unwrap();
        assert!(
            !down.contains("SW_HIDE"),
            "this module hides the vault window; a hidden window that fails to come back is \
             unreachable, which is the one outcome the minimise exists to avoid"
        );
        // And `SW_HIDE` appears in exactly one place in the whole module: the
        // overlay's own window, before it is shown. A second use is a window
        // this module put away without a way back.
        // Twice: `hide_and_place`'s `use` line and its one call.
        assert_eq!(
            code.matches("SW_HIDE").count(),
            2,
            "`SW_HIDE` is named somewhere other than `hide_and_place`'s import and its one \
             call. The only window this module may hide is its own, and only before it has \
             ever been visible"
        );
        let placer = code
            .split("fn hide_and_place(title: &str, display: ScreenRect) {")
            .nth(1)
            .expect("`hide_and_place` is gone")
            .split("\n}")
            .next()
            .unwrap();
        assert!(
            placer.contains("SW_HIDE"),
            "the one permitted `SW_HIDE` is not the one in `hide_and_place`"
        );
        // **No `Visible` command at all, in any shape.**
        //
        // Two windows, two reasons, one needle. A `Visible` on the VAULT
        // window drives the visibility of the window `vault_window`'s
        // `keep_ui_loaded` machinery believes only it produces, with none of
        // its bookkeeping. A `Visible` on the OVERLAY's own viewport is what
        // shipped as its show, and `winit` answered it by rewriting the
        // window's ex-style from its own flags -- `WS_EX_LAYERED` gone, window
        // opaque, the owner's "pitch dark screen". See `show_unseen` for the
        // `winit` lines and the log that proved it. The overlay is shown by
        // that function's `ShowWindow` now, so there is no legitimate
        // `Visible` left in this module and the count is zero.
        assert_eq!(
            code.matches("ViewportCommand::Visible").count(),
            0,
            "this module sends a `Visible` command. On the vault window that is a hide the \
             keep_ui_loaded machinery does not know about; on the overlay it is `winit` \
             rewriting the ex-style and taking `WS_EX_LAYERED` off -- see `show_unseen`"
        );
        // Positive control, which would otherwise be true of a module whose
        // overlay is created hidden and then never shown -- a full-screen
        // window nobody can see and nobody can cancel. The show is one
        // `ShowWindow(SW_SHOWNOACTIVATE)`, in `show_unseen`, and the constant
        // is named exactly twice: its import and its one call.
        // Counted as the call and not as the constant: the constant is also
        // named by the two log lines that report the call's outcome.
        assert_eq!(
            code.matches("ShowWindow(handle, SW_SHOWNOACTIVATE)").count(),
            1,
            "`ShowWindow(.., SW_SHOWNOACTIVATE)` is made somewhere other than `show_unseen`, \
             so the overlay is shown from more than one place -- or from none"
        );
        let shower = code
            .split("fn show_painted(hwnd: isize) {")
            .nth(1)
            .expect("`show_unseen` is gone")
            .split("\n}")
            .next()
            .unwrap();
        assert!(
            shower.contains("ShowWindow(handle, SW_SHOWNOACTIVATE)"),
            "the one permitted show is not the one in `show_unseen`"
        );
        // `SW_MINIMIZE` also activates the next top-level window in Z order,
        // which is somebody else's -- measured to cost this overlay the
        // foreground, and with it Escape.
        assert!(
            code.contains("SW_SHOWMINNOACTIVE"),
            "the vault window is no longer minimised without activation"
        );
        assert!(
            !code.contains("SW_MINIMIZE"),
            "SW_MINIMIZE activates the next window in Z order and takes the foreground away \
             from this overlay; see `send_window_down`"
        );
    }

    /// **Both halves report what Windows said rather than swallowing it.**
    ///
    /// This module's rule since three silently-failing Win32 calls cost a day
    /// of debugging, and these two are the worst candidates for it yet: a
    /// window that did not go down is a window sitting on top of what the user
    /// is being asked to point at, and a raise Windows declined is a window
    /// that is back but behind, which reads to the user as the app having
    /// disappeared.
    #[test]
    fn neither_half_of_the_minimise_swallows_what_windows_said() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();

        let down = code
            .split("fn send_window_down(title: &str) {")
            .nth(1)
            .expect("`send_window_down` is gone")
            .split("\nfn ")
            .next()
            .unwrap();
        // `ShowWindow` returns the PREVIOUS visibility, not success, so the
        // check has to be a readback rather than its return value.
        assert!(
            down.contains("IsIconic("),
            "nothing checks whether the window actually went down"
        );
        assert!(
            down.contains("log::info!") && down.matches("log::warn!").count() >= 2,
            "`send_window_down` no longer reports both outcomes and the missing window"
        );

        let back = code
            .split("fn bring_window_back(title: &str) {")
            .nth(1)
            .expect("`bring_window_back` is gone")
            .split("\nfn ")
            .next()
            .unwrap();
        // `foreground` answers a refusal with a documented outcome rather than
        // an error, and all four of them mean different things to this caller.
        for outcome in ["Raised::Front", "Raised::AlreadyInFront", "Raised::Flashed", "Raised::NoWindow"] {
            assert!(
                back.contains(outcome),
                "`bring_window_back` no longer accounts for {outcome}, so that outcome leaves \
                 no trail"
            );
        }
        assert!(
            back.matches("log::warn!").count() == 2,
            "a refusal or a missing window is no longer a warning; those are the two outcomes \
             in which the user's window may not be in front of them"
        );
        // And it goes through the crate's one way to do this, which restores
        // BEFORE it activates -- "once back - it should be on top again" is
        // two things, and `ShowWindow(SW_RESTORE)` alone is only the first.
        assert!(
            back.contains("foreground::raise_window(title)"),
            "the restore no longer goes through `foreground::raise_window`, which is what makes \
             it a raise and not just an un-minimise"
        );
    }

    // -- the reveal --------------------------------------------------------

    /// A found scan, ready to be revealed, on an overlay covering `display`.
    ///
    /// Takes the one display rather than a monitor list since the overlay
    /// stopped covering every monitor: `open_on` is the seam that skips the
    /// live cursor reading, which is what keeps these assertions about the
    /// arithmetic rather than about where the mouse happened to be. See
    /// [`RegionOverlay::open_on`].
    fn found_on(display: ScreenRect, scale: f32, at: ScreenRect) -> RegionOverlay {
        let overlay = RegionOverlay::open_on(display, scale).expect("opens");
        overlay.apply_scan(ScreenScan::Found(
            Zeroizing::new("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP".into()),
            at,
        ));
        overlay
    }

    /// **The reveal starts once, ends on the clock, and never restarts.**
    ///
    /// The property that keeps it out of the defect class this module was
    /// fixed for. It is the same argument `the_screen_is_scanned_once_and_the
    /// _state_never_goes_back` makes about the prescan, and it is made
    /// separately because this is a second timed machine and the two share no
    /// code.
    #[test]
    fn the_reveal_runs_once_on_a_clock_and_then_never_again() {
        let overlay = found_on(rect(0, 0, 1920, 1080), 1.0, FOUND_AT);
        let t0 = Instant::now();
        // The first painted frame starts the clock and shows the mark.
        assert_eq!(overlay.reveal_step(t0), Some(FOUND_AT));
        // Frames inside the dwell keep showing the same rectangle -- it does
        // not drift, and the overlay stays up.
        for after in [0_u64, 1, 16, 200, 449] {
            assert_eq!(
                overlay.reveal_step(t0 + Duration::from_millis(after)),
                Some(FOUND_AT),
                "the mark moved or vanished {after} ms in"
            );
            assert!(overlay.is_open(), "the overlay closed {after} ms in");
        }
        // Exactly the dwell is enough -- "at least", as every other bound in
        // this module is -- and it is what closes the overlay.
        assert_eq!(overlay.reveal_step(t0 + REVEAL_DWELL), None);
        assert!(!overlay.is_open(), "the reveal did not end the overlay");
        // And from there, nothing: not on the next frame, not an hour later,
        // not with the clock going backwards.
        for after in [0_u64, 1, 16, 5_000, 3_600_000] {
            assert_eq!(
                overlay.reveal_step(t0 + REVEAL_DWELL + Duration::from_millis(after)),
                None,
                "the reveal came back {after} ms later"
            );
        }
        assert_eq!(overlay.reveal_step(t0), None);
        assert_eq!(overlay.view().reveal, None, "a finished reveal is still being painted");
    }

    /// **A second answer cannot restart a reveal, or reopen a finished one.**
    ///
    /// The *Whole screen* chip can be pressed while a reveal is running only
    /// if the input guard fails, and a repaint can re-enter the callback at
    /// any time -- so the state machine refuses rather than relying on the
    /// caller. A reveal that could be restarted is a window that never closes,
    /// which is the shape of the hang this module has already shipped once.
    #[test]
    fn a_second_scan_cannot_restart_or_reopen_the_reveal() {
        let overlay = found_on(rect(0, 0, 1920, 1080), 1.0, FOUND_AT);
        let t0 = Instant::now();
        assert_eq!(overlay.reveal_step(t0), Some(FOUND_AT));

        // A second `Found`, somewhere else, mid-reveal: ignored entirely.
        let elsewhere = rect(900, 900, 1000, 1000);
        overlay.apply_scan(ScreenScan::Found(
            Zeroizing::new("otpauth://totp/second".into()),
            elsewhere,
        ));
        assert_eq!(
            overlay.reveal_step(t0 + Duration::from_millis(1)),
            Some(FOUND_AT),
            "a second scan moved the mark mid-reveal"
        );
        // Including its payload: the answer is the code that was found first.
        assert_eq!(overlay.reveal_step(t0 + REVEAL_DWELL), None);
        match overlay.take_outcome() {
            Some(Outcome::Decoded(text)) => assert_eq!(&*text, "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP"),
            other => panic!("the reveal answered with {other:?}"),
        }

        // And after it is over, a `Found` cannot reopen the window.
        overlay.apply_scan(ScreenScan::Found(
            Zeroizing::new("otpauth://totp/third".into()),
            elsewhere,
        ));
        assert!(!overlay.is_open(), "a scan reopened a closed overlay");
        assert_eq!(overlay.reveal_step(t0 + REVEAL_DWELL * 2), None);
        assert!(overlay.take_outcome().is_none(), "a closed overlay recorded a second answer");
    }

    /// **The reveal ends whether or not anything else happens.**
    ///
    /// The requirement that separates it from every other state on this
    /// surface: a drag waits for the user, and this must not. A clock that
    /// never reaches the deadline never ends it, and a clock that does ends it
    /// with no pointer, no key and no press anywhere in the sequence.
    #[test]
    fn the_reveal_does_not_wait_for_the_user() {
        let overlay = found_on(rect(0, 0, 1920, 1080), 1.0, FOUND_AT);
        let t0 = Instant::now();
        // Two hundred frames inside the dwell, with no input of any kind. The
        // first of them starts the clock; the rest are the repaints an idle
        // desktop produces, and none of them may either end it early or
        // restart it.
        for _ in 0..200 {
            assert!(overlay.reveal_step(t0 + Duration::from_millis(1)).is_some());
            assert!(overlay.is_open());
        }
        // The clock alone closes it, at one dwell after the frame that
        // started it -- not after the two hundredth, which is what a deadline
        // pushed forward by each frame would give.
        assert_eq!(
            overlay.reveal_step(t0 + Duration::from_millis(1) + REVEAL_DWELL),
            None
        );
        assert!(!overlay.is_open());

        // **And a frame is always asked for**, which is the half of "does not
        // wait for the user" that lives inside the viewport callback and can
        // only be pinned from here by source.
        //
        // The reveal reads no input and this window is always-on-top over
        // every monitor, so nothing else would ever wake it. Two places have
        // to ask: the reveal branch, on every frame it paints, and the
        // on-demand scan, which is the one place a reveal can be opened by a
        // frame that has already passed `reveal_step` and would otherwise
        // leave a `Due` reveal with no frame coming to start its clock.
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        let callback = code
            .split("move |root, _class| {")
            .nth(1)
            .expect("the viewport callback was restructured");
        assert_eq!(
            callback.matches("root.request_repaint_of(region_viewport());").count(),
            2,
            "the reveal no longer asks for the frames it needs to end on its own"
        );
        // The second of them is the on-demand scan's, immediately after it.
        let after_scan = callback
            .split("&screen_capture::monitor_bounds(),")
            .nth(1)
            .expect("the on-demand scan was restructured");
        assert!(
            after_scan
                .split("} else if")
                .next()
                .unwrap()
                .contains("root.request_repaint_of(region_viewport());"),
            "a reveal opened by the Whole screen chip has no frame coming to start its clock"
        );
    }

    /// **The mark lands where the code is, in the painter's own units.**
    ///
    /// The last leg of the coordinate chain: `qr` reports a box in the
    /// captured buffer's pixels, `scan_screen_with` places it on the virtual
    /// screen, and this converts it into points inside a viewport whose origin
    /// is the covered display's top-left. Every step of that is a subtraction
    /// or a division that is the identity on a single 100% monitor at the
    /// origin, so this is driven on a display that is neither.
    #[test]
    fn the_mark_is_drawn_over_the_code_and_not_beside_it() {
        // The overlay covers ONE display, and this one is placed left of and
        // above the primary at 200% scaling -- so both the offset and the
        // scale factor have to be taken out for the answer to be right. It
        // used to be the bounding box of a two-monitor desktop, which had the
        // same top-left by construction; the arithmetic under test is
        // unchanged and the rectangle it is driven on is now the one the
        // window really gets. See `RegionOverlay::open`.
        let display = rect(-1280, -200, 0, 520);
        let at = rect(-1080, 0, -880, 200);
        let overlay = found_on(display, 2.0, at);
        // In screen pixels the code runs from (-1080, 0) to (-880, 200); the
        // viewport's origin is (-1280, -200), so that is 200..400 by 200..400
        // pixels in, and at 2.0 that is 100..200 by 100..200 points.
        assert_eq!(
            overlay.view().reveal,
            Some(rect_pts(100.0, 100.0, 200.0, 200.0))
        );
        // The same code on a plain 100% single-monitor desktop is reported at
        // its own pixels, which is the control that says the arithmetic above
        // is the offset and the scale and not a coincidence.
        let plain = found_on(rect(0, 0, 1920, 1080), 1.0, rect(300, 200, 460, 380));
        assert_eq!(
            plain.view().reveal,
            Some(rect_pts(300.0, 200.0, 460.0, 380.0))
        );
    }

    /// **Nothing else is painted while the reveal is up**, and nothing else
    /// is on screen to be clicked.
    ///
    /// `draw` answers with the two chips' rectangles, and the pointer handling
    /// tests a press against whichever it was last told about. During the
    /// reveal there is no bar and therefore no chips: the reveal returns
    /// `Rect::NOTHING` for both, which contains no point, so even a frame that
    /// somehow reached the pointer handling could not press one.
    #[test]
    fn the_reveal_paints_no_bar_and_therefore_no_chips() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        // `draw` takes the reveal first and returns before `paint_bar`.
        let drawn = code.split("pub fn draw(").nth(1).expect("`draw` was renamed");
        let reveal = drawn.find("paint_reveal(").expect("`draw` no longer paints the reveal");
        let bar = drawn.find("paint_bar(").expect("`draw` no longer paints the bar");
        assert!(reveal < bar, "the bar is painted before the reveal returns");
        assert!(
            drawn[reveal..bar].contains("return [egui::Rect::NOTHING; 2];"),
            "the reveal no longer returns before the bar, or no longer answers with no chips"
        );
        // And the callback reads no input on those frames: the reveal branch
        // returns before the pointer is ever looked at.
        let callback = code
            .split("if mine.reveal_step(Instant::now()).is_some() {")
            .nth(1)
            .expect("the callback no longer has a reveal branch");
        let ends = callback.find("return;").expect("the reveal branch does not return");
        assert!(
            !callback[..ends].contains("i.pointer") && !callback[..ends].contains("key_pressed"),
            "the reveal branch reads input before it returns"
        );
    }

    /// Production's dwell is the documented one: long enough to be seen,
    /// short enough not to be sat through, and shorter than the scan in front
    /// of it. A `REVEAL_DWELL` dropped to zero would be the reveal not
    /// happening; raised to seconds it would be the feature made slower to
    /// look faster.
    #[test]
    fn the_production_reveal_dwell_is_the_documented_one() {
        assert_eq!(REVEAL_DWELL, Duration::from_millis(450));
        // Past the quarter-second at which a mark that appeared away from the
        // pointer has only just been looked at -- and past the "reads as
        // immediate" threshold `DECODE_INTERVAL` is argued against, which is
        // the same number and is checked against the constant rather than
        // written out again.
        assert!(REVEAL_DWELL > DECODE_INTERVAL * 2);
        // And under half the second at which a fixed wait starts reading as
        // the app thinking.
        assert!(REVEAL_DWELL < Duration::from_millis(500));
        // The badge it carries is 6c's own words and is NOT the drag's, which
        // ends in an instruction for a gesture the reveal does not have.
        assert_eq!(SCAN_FOUND, "Code read");
        assert_ne!(SCAN_FOUND, LOCKED_ON);
        assert!(!SCAN_FOUND.contains("release"));
    }

    /// **The reveal's clock does not start until the vault window has had time
    /// to get off the screen.**
    ///
    /// The minimise is issued on the overlay's first painted frame, which is
    /// the same frame the reveal would otherwise start on -- so without this
    /// the front of the dwell is spent ringing a code behind the window the
    /// ring exists to see past, by an amount that varies with the machine.
    /// Exactly the argument `Reveal::Due` already makes about a window that
    /// does not exist yet.
    ///
    /// Driven by a clock a test supplies, like every other timed thing here,
    /// so no window and no sleeping is involved.
    #[test]
    fn the_reveal_waits_for_the_vault_window_to_get_out_of_the_way() {
        let overlay = found_on(rect(0, 0, 1920, 1080), 1.0, FOUND_AT);
        let t0 = Instant::now();
        // What the first-frame hook does, without the Win32 call it also
        // makes: the window went down at `t0`.
        locked(&overlay.inner).aside_at = Some(t0);

        // Every frame inside the settle paints the mark -- the ring is not
        // withheld, only the clock -- and leaves the reveal where it was.
        for at in [0_u64, 1, 60, 119] {
            assert_eq!(
                overlay.reveal_step(t0 + Duration::from_millis(at)),
                Some(FOUND_AT),
                "the mark was withheld {at} ms into the settle"
            );
            assert!(
                matches!(locked(&overlay.inner).reveal, Reveal::Due { .. }),
                "the dwell started {at} ms in, before the window was out of the way"
            );
        }

        // The first frame at or after the settle starts the clock, and the
        // user gets the WHOLE dwell from there rather than what is left of it.
        let started = t0 + MINIMISE_SETTLE;
        assert_eq!(overlay.reveal_step(started), Some(FOUND_AT));
        assert!(matches!(locked(&overlay.inner).reveal, Reveal::Showing { .. }));
        assert_eq!(
            overlay.reveal_step(started + REVEAL_DWELL - Duration::from_millis(1)),
            Some(FOUND_AT),
            "the dwell was cut short by the settle in front of it"
        );
        assert_eq!(overlay.reveal_step(started + REVEAL_DWELL), None);
        assert!(!overlay.is_open());
    }

    /// **An overlay that never stood aside does not wait for a window that
    /// never went down.**
    ///
    /// `aside_at` is `None` on exactly two overlays: one in a test process,
    /// and one dropped before it ever had a window. Neither has anything to
    /// wait for, and a settle applied to them would be a fixed delay in front
    /// of every reveal for no reason at all.
    #[test]
    fn a_reveal_with_nothing_to_wait_for_starts_at_once() {
        let overlay = found_on(rect(0, 0, 1920, 1080), 1.0, FOUND_AT);
        let t0 = Instant::now();
        assert!(locked(&overlay.inner).aside_at.is_none());
        assert_eq!(overlay.reveal_step(t0), Some(FOUND_AT));
        assert!(
            matches!(locked(&overlay.inner).reveal, Reveal::Showing { .. }),
            "the clock did not start on the first painted frame"
        );
        assert_eq!(overlay.reveal_step(t0 + REVEAL_DWELL), None);
    }

    /// Production's minimise settle is the measured one.
    ///
    /// The measurement is in [`MINIMISE_SETTLE`]'s own doc: a window filled
    /// with a colour nothing else is, minimised, and its rectangle captured
    /// exactly once per process run at one delay; absent from the capture at
    /// every delay tried, down to the ~60 ms floor of the instrument.
    #[test]
    fn the_production_minimise_settle_is_the_measured_one() {
        assert_eq!(MINIMISE_SETTLE, Duration::from_millis(120));
        assert!(MINIMISE_SETTLE > Duration::ZERO, "the settle is not a settle");
        // Twice the shortest point at which the window was measured gone, so
        // there is margin for a machine slower than the one it was measured
        // on -- and more than PRESCAN_SETTLE, which buys two composes at 30 Hz
        // for a flag that is only a message to the compositor. A minimise is
        // more work than a flag.
        assert!(
            MINIMISE_SETTLE > PRESCAN_SETTLE,
            "a minimise is now given less time to land than a display-affinity flag"
        );
        // And small enough not to dominate what it delays. It is spent once,
        // in front of the dwell, at the end of a route whose scan binarises
        // every monitor.
        assert!(
            MINIMISE_SETTLE < REVEAL_DWELL / 2,
            "the wait in front of the reveal is now a large fraction of the reveal"
        );
    }

    /// **The window is taken down and placed by hand before it is ever
    /// shown**, and the owner's popup is what this is about.
    ///
    /// > 1. White small popup shows up
    /// > 2. Same popup moves position
    ///
    /// `with_visible(false)` on the viewport builder should make that
    /// impossible and, measured on one machine, does. It did not on the
    /// owner's, so this module stopped relying on it: see [`hide_and_place`].
    /// What a test can hold is that the call is made, that it is made on every
    /// waiting frame rather than once, and that it is made **before** the show
    /// -- a hide after the show is a flicker rather than a fix.
    #[test]
    fn the_window_is_hidden_and_placed_before_anyone_can_see_it() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        let appear = code
            .split("fn appear(&self, ctx: &egui::Context) {")
            .nth(1)
            .expect("`appear` is gone")
            .split("\n    }")
            .next()
            .unwrap();
        let hide = appear
            .find("hide_and_place(REGION_TITLE, display);")
            .expect("nothing hides and places the window before it is shown");
        let shown = appear
            .find("show_painted(hwnd);")
            .expect("the overlay is never shown");
        assert!(
            hide < shown,
            "the window is hidden and placed AFTER it is shown, which is a flicker rather than \
             a fix -- the whole point is that the move happens where nobody can see it"
        );
        // Gated on `Waiting`, so it stops the moment this module shows the
        // window itself. Without the gate it would hide the overlay on every
        // frame of its life, which is a window the user never sees at all.
        assert!(
            appear.contains("if waiting {\n            hide_and_place(REGION_TITLE, display);"),
            "the hide is no longer gated on `Appearing::Waiting`, so it either runs once (and \
             leaves anything that showed the window early on screen until the next frame) or \
             runs for ever (and the overlay never appears)"
        );
        // `SW_HIDE` and not a viewport command: the point is to undo a show
        // this module did not make, which egui does not know about.
        let placer = code
            .split("fn hide_and_place(title: &str, display: ScreenRect) {")
            .nth(1)
            .expect("`hide_and_place` is gone");
        assert!(
            placer.contains("ShowWindow(handle, SW_HIDE)"),
            "`hide_and_place` no longer hides the window"
        );
        assert!(
            placer.contains("SWP_NOACTIVATE | SWP_NOZORDER"),
            "the placement activates or restacks the window; activating a hidden window is how \
             the overlay loses the keyboard before it ever has it, and the z-order is the \
             raise's business"
        );
    }

    /// **The picture is taken before the window exists, out of captures before
    /// it is shown, and released with it.**
    ///
    /// Three orderings, each of which is a photograph of the wrong thing if
    /// it slips. Taken after the viewport is registered, the picture can
    /// contain the overlay's own window; taken before the vault window is
    /// masked, it contains the card that says "Scanning your screen"; kept
    /// after the window is gone, it is a 29 MB copy of the user's screen with
    /// no window to show it on. None of these can be observed from a test
    /// process -- there is no screen to capture and no compositor to show it
    /// -- so the source is read.
    #[test]
    fn the_picture_is_taken_before_the_window_exists_and_released_with_it() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        let show = code
            .split("pub fn show(&self, ctx: &egui::Context) -> bool {")
            .nth(1)
            .expect("`show` is gone");
        // Taken on the frame the scan runs, which is after the mask
        // (`PrescanStep::Mask` is an earlier frame) and before the viewport
        // is registered on this one.
        let scan_arm = show
            .find("PrescanStep::Scan => {")
            .expect("`show` no longer drives the scan through a match");
        let taken = show
            .find("self.take_picture(ctx);")
            .expect("nothing takes the display's picture, so the overlay has no ground");
        let registered = show
            .find("ctx.show_viewport_deferred(")
            .expect("the viewport is no longer registered from `show`");
        assert!(
            scan_arm < taken && taken < registered,
            "the picture is not taken inside the scan arm, before the viewport is \
             registered: a window that exists when the display is captured can be in the \
             capture"
        );
        assert!(
            show.find("self.mask_own_window(true);").expect("the prescan no longer masks") < taken,
            "the picture is taken before the vault window is masked, so it contains the card"
        );
        // Released on the closing frame, before the vault window comes back:
        // the first thing `show` does once the overlay is over.
        let closing = show
            .split("self.stand_aside(false);")
            .next()
            .expect("`show`'s early return no longer brings the vault window back");
        assert!(
            closing.contains("locked(&self.inner).picture.take()"),
            "the picture is not released on the frame the overlay closes, so it lives for \
             as long as the caller keeps the last clone of the overlay"
        );
        // And the window is out of captures before it is shown, still.
        let appear = code
            .split("fn appear(&self, ctx: &egui::Context) {")
            .nth(1)
            .expect("`appear` is gone")
            .split("\n    }")
            .next()
            .unwrap();
        assert!(
            appear.find("exclude_from_capture(REGION_TITLE);").expect("the mask is gone")
                < appear.find("show_painted(hwnd);").expect("the overlay is never shown"),
            "the window is shown before it is taken out of screen captures, so a capture \
             taken in between reads this overlay's own picture"
        );
        // **And the step means "the window exists", not "this has not run
        // before".** That was the defect that shipped through 0.15.21 and it
        // is invisible in a diff: the calls were made, on a window that did
        // not exist yet, and every one of them resolved `None` and did nothing.
        assert!(
            appear.contains("crate::foreground::own_window_titled(REGION_TITLE)")
                && appear.contains("appearing.compose(found.is_some())"),
            "`appear` no longer waits for the window to exist, so the capture exclusion runs \
             against an HWND that is not there yet -- silently"
        );
        let gate = code
            .split("fn compose(&mut self, window_exists: bool) -> bool {")
            .nth(1)
            .expect("`Appearing::compose` is gone");
        let gate = gate.split("\n    }").next().unwrap();
        assert!(
            gate.contains("&& window_exists"),
            "`Appearing::compose` leaves `Waiting` without being told the window exists, so \
             the one chance to make these calls is spent on a frame that cannot make them"
        );
    }

    /// **The viewport is created hidden, and that is the white box.**
    ///
    /// The measurement is in [`Appearing`]'s doc and cannot be repeated from
    /// here -- there is no compositor in a test process. What can be held is
    /// the flag, and the flag is the whole fix: without it `eframe` creates the
    /// OS window and Windows shows it immediately, and every pixel of every
    /// monitor is a solid rectangle until the first frame paints. Measured at
    /// **1315 ms**, eighty consecutive 17 ms samples of `(255, 255, 255)` with
    /// `sd = 0.00` and nothing else in between; zero and one across two runs
    /// after the change.
    ///
    /// It is also the reason the layering and the capture mask left the
    /// viewport callback: that callback cannot tell when its window exists as
    /// an `HWND` or when the user can see it -- `eframe` 0.35 runs and paints
    /// it hidden or not -- and every `EnumWindows` on its frames sits in front
    /// of a paint.
    #[test]
    fn the_overlay_window_is_created_hidden_and_shown_once_it_is_composited() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        let builder = code
            .split("ViewportBuilder::default()")
            .nth(1)
            .expect("the overlay no longer builds a viewport")
            .split("move |root, _class| {")
            .next()
            .expect("the builder no longer ends at the callback");
        assert!(
            builder.contains(".with_visible(false)"),
            "the overlay's viewport is no longer created hidden, so the window is on screen \
             before it is composited and before it has painted -- which is the second and a \
             third of solid white this module was fixed for"
        );
        // Positive control on the needle: the builder really is the haystack,
        // so the presence above is about this window and not about an empty
        // string.
        assert!(builder.contains(".with_title(REGION_TITLE)"));
        // And exactly one show, so a second one cannot appear somewhere that
        // runs before the layering call.
        assert_eq!(
            code.matches("show_painted(hwnd);").count(),
            1,
            "the overlay is shown from more than one place, so the ordering the white-box fix \
             rests on is no longer decided in one spot"
        );
        // The callback is not where any of it happens any more. A hook there
        // would never run: `eframe` gates a deferred viewport's callback on
        // `is_visible`.
        let callback = code
            .split("move |root, _class| {")
            .nth(1)
            .expect("the viewport callback is gone")
            // Bounded at the line that follows the closure, or the needles
            // below would find these functions' own definitions further down
            // the file and this would be a test of nothing.
            .split("// **After the viewport is registered")
            .next()
            .expect("the callback no longer ends where it did");
        for needle in [
            "show_painted(",
            "exclude_from_capture(",
            "raise_window(",
            "stand_aside(true)",
        ] {
            assert!(
                !callback.contains(needle),
                "`{needle}` is back inside the viewport callback. That callback cannot tell \
                 when its window exists or when the user can see it, and everything it puts \
                 in front of the paint is more time before the first painted frame"
            );
        }
    }

    /// **Each step of the appearance happens once, in order, and only when it
    /// is allowed to.**
    ///
    /// The pure half of [`Appearing`], which is all a test process can reach:
    /// the effects are four Win32 calls and a viewport command and not one of
    /// them can be observed from `cargo test`. What can be observed is that
    /// `compose` waits for a window, that `on_screen` waits for `compose`, and
    /// that neither ever fires twice -- the last of which is what keeps the
    /// vault window from being re-minimised every frame for the life of the
    /// overlay.
    #[test]
    fn the_window_appears_in_two_steps_and_neither_repeats() {
        // Nothing happens while there is no window, however many frames pass
        // -- and a paint claimed before there is a window changes nothing,
        // because there is nothing it could have been painted into.
        let mut appearing = Appearing::Waiting;
        for _ in 0..50 {
            assert!(!appearing.compose(false));
            assert!(
                !appearing.on_screen(true),
                "the overlay raised and minimised into a window that does not exist yet"
            );
            assert_eq!(appearing, Appearing::Waiting);
        }
        // The frame the window appears layers it at no alpha and shows it.
        assert!(appearing.compose(true));
        assert_eq!(appearing, Appearing::Hidden);
        // And not twice, which would be a second layering, a second mask and a
        // second show every frame.
        for _ in 0..50 {
            assert!(!appearing.compose(true));
            assert_eq!(appearing, Appearing::Hidden);
        }
        // **Unseen holds for as long as nothing has been painted.** However
        // many root frames pass, the alpha is not raised over an unpainted
        // surface: that is the rectangle of "whatever the surface held" that
        // every report this module has collected begins with.
        for _ in 0..50 {
            assert!(
                !appearing.on_screen(false),
                "the window was made visible before anything had been painted into it"
            );
            assert_eq!(appearing, Appearing::Hidden);
        }
        // The first root frame after a paint is the one that raises the alpha,
        // raises the window and stands the vault window aside.
        assert!(appearing.on_screen(true));
        assert_eq!(appearing, Appearing::Up);
        // Absorbing, both ways. A `true` here is a window that re-raises itself
        // every frame and a vault window minimised again each time the user
        // clicks its taskbar button.
        for _ in 0..50 {
            assert!(!appearing.on_screen(true));
            assert!(!appearing.compose(true));
            assert_eq!(appearing, Appearing::Up);
        }
    }

    /// **The overlay is shown by this module's own `ShowWindow`, only once it
    /// has been painted, and nothing in this module ever gives `winit` a
    /// reason to touch the window.**
    ///
    /// The mechanism is in [`show_painted`]'s doc and it is `winit`'s, not
    /// this crate's: `WindowFlags::apply_diff` rewrites the window's styles
    /// from its own flags on every flag change, and hides a window its flags
    /// call hidden. So what a test can hold is the shape that keeps
    /// `apply_diff` from ever running on this window: the show is a bare
    /// `ShowWindow`, no `ViewportCommand` of any kind is sent to the viewport,
    /// and the window is not shown until the callback has painted into it --
    /// which is what keeps the first composite from being the unpainted
    /// surface every report this module has collected begins with.
    #[test]
    fn the_overlay_is_shown_by_this_module_and_winit_never_touches_it_again() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        let statements: String = code
            .lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        // **No viewport command, addressed or bare.** Every one of them is a
        // `winit` `set_*` on this window, and any that changes a flag runs
        // `apply_diff`, whose act for a window `winit` believes hidden is
        // `ShowWindow(SW_HIDE)`.
        assert_eq!(
            statements.matches("send_viewport_cmd").count(),
            0,
            "this module sends a viewport command. Whatever it is for, `winit` answers a flag \
             change on the overlay's window by hiding a window its flags call hidden and \
             rewriting its styles -- see `show_painted`"
        );
        // The show is the OS call, made on the handle from the one lookup,
        // and only inside the branch that has seen a paint.
        let appear = code
            .split("fn appear(&self, ctx: &egui::Context) {")
            .nth(1)
            .expect("`appear` is gone")
            .split("\n    }")
            .next()
            .unwrap();
        let shown = appear.find("show_painted(hwnd);").expect("the overlay is never shown");
        assert!(
            appear.find("locked(&self.inner).hwnd = found;").expect("the handle is not kept")
                < shown,
            "the handle is not kept before the show, so the show has nothing to act on \
             without a second `EnumWindows`"
        );
        let gated = appear
            .find("appearing.on_screen(painted)")
            .expect("the second step's gate is gone, or no longer waits for a paint");
        assert!(
            appear.contains("let painted = locked(&self.inner).painted;") && gated < shown,
            "the show is no longer behind `Appearing::on_screen(painted)`, so the window can \
             be shown before anything has been painted into it -- the black or white \
             rectangle"
        );
        // The callback is what says a paint happened -- once per `draw`, on
        // both of its branches.
        let callback = code
            .split("move |root, _class| {")
            .nth(1)
            .expect("the viewport callback is gone")
            .split("// **After the viewport is registered")
            .next()
            .expect("the callback no longer ends where it did");
        assert_eq!(
            callback.matches("mine.note_painted();").count(),
            callback.matches("draw(ui, &view)").count(),
            "a `draw` in the callback is not followed by `note_painted`, so a route that only \
             ever takes that branch keeps the window hidden for ever -- a scan with no window"
        );
        assert!(
            callback.matches("draw(ui, &view)").count() >= 2,
            "the callback no longer paints on both the reveal and the drag branches"
        );
        // And a `Hidden` frame that finds nothing painted asks for the next
        // root frame, so the gate is a wait with an end.
        let hidden_frame = appear
            .split("else if matches!(locked(&self.inner).appearing, Appearing::Hidden) {")
            .nth(1)
            .expect("`appear` no longer has an arm for a hidden window that has not painted")
            .split("\n        }")
            .next()
            .unwrap();
        assert!(
            hidden_frame.contains("ctx.request_repaint();"),
            "a `Hidden` root frame that finds the overlay unpainted no longer asks for \
             another root frame, so if the overlay paints after this frame it is never shown"
        );
        // The show itself changes only visibility.
        let shower = code
            .split("fn show_painted(hwnd: isize) {")
            .nth(1)
            .expect("`show_painted` is gone")
            .split("\n}")
            .next()
            .unwrap();
        assert!(
            shower.contains("ShowWindow(handle, SW_SHOWNOACTIVATE)"),
            "`show_painted` no longer shows the window with `SW_SHOWNOACTIVATE`"
        );
        for gone in ["SetWindowLongPtrW", "SetWindowPos", "SetForegroundWindow"] {
            assert!(
                !shower.contains(gone),
                "`show_painted` does more than show: `{gone}` is in it. Placement is \
                 `hide_and_place`'s, the raise is `foreground`'s, and this is the one call \
                 that changes only visibility"
            );
        }
    }

    /// **The reveal's dwell does not start until the window it is getting out
    /// of the way has been asked to go.**
    ///
    /// `MINIMISE_SETTLE` covers the stretch after the minimise is issued. This
    /// covers the stretch before it: the overlay's window is shown at the end
    /// of one root frame and the minimise happens on the next, so there is one
    /// frame in which the overlay is painting, the reveal is `Due`, and
    /// Deskwarden is still sitting on top of the code being ringed. A dwell
    /// started there is a dwell the user spends looking at a ring behind a
    /// window.
    #[test]
    fn the_reveal_waits_for_the_window_to_have_finished_appearing() {
        let overlay = found_on(rect(0, 0, 1920, 1080), 1.0, FOUND_AT);
        locked(&overlay.inner).appearing = Appearing::Hidden;
        let t0 = Instant::now();
        // Frames pass, the mark is painted, and the clock does not start: the
        // reveal is still `Due`, so a dwell later is still a whole dwell.
        for _ in 0..20 {
            assert_eq!(overlay.reveal_step(t0), Some(FOUND_AT));
            assert!(matches!(locked(&overlay.inner).reveal, Reveal::Due { .. }));
        }
        assert_eq!(
            overlay.reveal_step(t0 + REVEAL_DWELL * 4),
            Some(FOUND_AT),
            "the reveal ended while the window was still only just shown, so the user got \
             nothing at all"
        );
        // Once the window is up -- and with nothing to wait for, because a
        // test overlay never stands aside -- the clock starts on the next
        // frame and runs its full length from there.
        locked(&overlay.inner).appearing = Appearing::Up;
        let t1 = t0 + REVEAL_DWELL * 4;
        assert_eq!(overlay.reveal_step(t1), Some(FOUND_AT));
        assert!(matches!(locked(&overlay.inner).reveal, Reveal::Showing { .. }));
        assert_eq!(overlay.reveal_step(t1 + REVEAL_DWELL - Duration::from_millis(1)), Some(FOUND_AT));
        assert_eq!(overlay.reveal_step(t1 + REVEAL_DWELL), None);
        assert!(!overlay.is_open());
    }

    /// **The scan is still decode-only, and the one capture this module
    /// displays lives exactly as long as the overlay.**
    ///
    /// This test used to forbid `load_texture`, `TextureHandle` and
    /// `ColorImage` from the module outright, on the ground that egui's
    /// texture manager holds allocations this crate cannot wipe. That ground
    /// has not moved; what moved is the other side of the scale, and the
    /// module header and `take_picture` say how. So the pin is now on the
    /// BOUNDS of the exception rather than on its absence:
    ///
    /// * exactly one upload, in `take_picture`, of the one display's picture;
    /// * the wiped `Rgba` is dropped the moment the egui image exists;
    /// * the handle lives in `Inner`, whose `Drop` releases it, and is also
    ///   released on the closing frame;
    /// * the scan's own captures are still decoded and dropped, never shown.
    #[test]
    fn the_desktop_picture_lives_exactly_as_long_as_the_overlay() {
        let source = include_str!("region_overlay.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.len() < source.len(), "the test module marker was not found");
        let statements: String = code
            .lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        // One upload, of one image, with nearest filtering so that a picture
        // that maps one to one onto the display stays crisp.
        assert_eq!(
            statements.matches("load_texture(").count(),
            1,
            "a second texture is uploaded somewhere in this module; the exception to 'no \
             capture is displayed' is one picture, in `take_picture`"
        );
        let taker = statements
            .split("fn take_picture(&self, ctx: &egui::Context) {")
            .nth(1)
            .expect("`take_picture` is gone")
            .split("\n    }")
            .next()
            .unwrap();
        for needle in [
            "screen_capture::capture_rect(display)",
            "egui::ColorImage::from_rgba_unmultiplied(",
            "drop(pixels);",
            "egui::TextureOptions::NEAREST",
            "Some(Picture(texture))",
        ] {
            assert!(
                taker.contains(needle),
                "`take_picture` no longer contains `{needle}`; see its doc for what each \
                 line is for"
            );
        }
        assert!(
            taker.find("drop(pixels);").unwrap() < taker.find("load_texture(").unwrap(),
            "the wiped capture is kept alive across the upload"
        );
        // The refusal is a stated fallback: a log line and `None`, not a panic
        // and not a closed overlay.
        assert!(
            taker.contains("Err(refusal) =>") && taker.contains("log::warn!("),
            "a refused capture is no longer reported"
        );
        assert!(
            !taker.contains("unwrap()") && !taker.contains("expect(") && !taker.contains("panic!"),
            "`take_picture` can panic on a capture that was refused"
        );
        // The handle has one home, and that home is dropped with `Inner`.
        assert_eq!(
            statements.matches("egui::TextureHandle").count(),
            1,
            "a `TextureHandle` is named somewhere other than `Picture`'s one field, so a \
             picture can be held by something that does not go when the overlay does"
        );
        assert!(statements.contains("picture: Option<Picture>,"), "`Inner::picture` is gone");
        assert!(statements.contains("impl Drop for Inner {"), "`Inner`'s `Drop` is gone");
        // And the picture's `Debug` prints no pixels: the id and the size.
        let debug = statements
            .split("impl std::fmt::Debug for Picture {")
            .nth(1)
            .expect("`Picture` no longer hand-writes its `Debug`")
            .split("\n}")
            .next()
            .unwrap();
        assert!(
            debug.contains("self.0.id()") && debug.contains("self.0.size()") && !debug.contains("pixels"),
            "`Picture`'s `Debug` prints something other than the texture's id and size"
        );
        // Positive control on the scan: its captures are still in this file,
        // still decoded, and still dropped rather than kept.
        assert!(code.contains("(seams.capture)(*monitor)"));
        assert_eq!(
            statements.matches("drop(pixels);").count(),
            2,
            "a capture is dropped in a different number of places than two: the scan's, and \
             the picture's once it has been uploaded"
        );
    }
}

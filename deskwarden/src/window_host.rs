//! **The two things `eframe::run_ui_native` cannot be told**, and the smallest
//! thing that can tell them.
//!
//! This module opens no window of its own. It has no title constant, no
//! geometry, no frame closure and nothing on screen; what it has is the
//! `eframe::App` implementation that `eframe::run_ui_native` keeps private,
//! plus the little state machine that decides when a window created hidden is
//! allowed to appear. Both exist because of a defect that could not be fixed at
//! any call site.
//!
//! # 1. The clear colour, and the black region overlay
//!
//! `eframe::run_ui_native` wraps the caller's closure in a private `SimpleApp`
//! that overrides nothing else, so the app takes `epi::App::clear_color`'s
//! default -- `Color32::from_rgba_unmultiplied(12, 12, 12, 180)`. That colour
//! is what every viewport of the app is cleared to before its first pixel is
//! painted, the ROOT window and every `show_viewport_deferred` child alike.
//!
//! On an opaque window the alpha is dropped and the clear is `rgb(12, 12, 12)`
//! -- near-black. That is exactly what design 6b's region overlay was getting:
//! it opens as a deferred viewport `with_transparent(true)`, over the whole
//! desktop, and dims what is behind it by painting `DIM_ALPHA` over nothing.
//! With the window cleared to near-black instead of to the desktop, "dimmed
//! desktop" came out as a solid black screen with no way to see where to drag
//! the box.
//!
//! **Two separate refusals produced that, and both had to go.** The child
//! viewport's `with_transparent(true)` is *ignored*: `eframe`'s
//! `glow_integration` chooses the GL config's alpha capability ONCE, at app
//! startup, from the ROOT viewport's `NativeOptions`
//! (`config_template_builder.with_transparency(native_options.viewport
//! .transparent.unwrap_or(false))`), and logs `Cannot create transparent
//! window: the GL config does not support it` when a viewport later asks for
//! something the config has not got. So the host window has to ask for
//! transparency even though the host window is opaque -- see
//! `vault_window::build_frame_with_search` and
//! `app_window::the_vault_windows_viewport`, which are the two roots that can
//! host the overlay. And with the alpha channel in place the clear colour has
//! to actually be transparent, which is this module.
//!
//! # Which windows go through here, and why not all seven
//!
//! **Only the two hosts that can open the region overlay**: the vault window
//! (`vault_window::run`) and the single startup window
//! (`app_window::run_the_one_window`), which becomes that same vault UI. The
//! other five `eframe` windows in this crate -- the standalone sign-in card,
//! the spinner, the two "Add app" windows and Preferences -- keep
//! `eframe::run_ui_native` and keep the default clear.
//!
//! That is deliberate and it is the conservative side of the trade. A
//! transparent clear moves the responsibility for a window's opacity from
//! `eframe` to the window: every pixel the app does not paint this frame is
//! see-through rather than near-black. The two hosts here earn that by painting
//! `theme::paint_window_background` over their whole rect on EVERY frame, which
//! is a change made with this one -- they used to paint it on the first frame
//! only and let the clear colour cover the rest. `picker_ui`'s "Add app" window
//! does not paint a background at all on its first frame, and none of the five
//! paints one on any later frame; handing them a transparent clear would punch
//! visible holes in real windows to fix a bug none of them has. A window that
//! cannot host the overlay gains nothing from being here.
//!
//! # 2. The white box that blinked, and `Reveal`
//!
//! `theme::paint_window_background`'s own doc records half of this: the *dark*
//! flash from an unpainted first frame, fixed by painting a plain rect before
//! the fonts are live. What was left is the flash BEFORE egui's first frame at
//! all. Windows shows a newly created window with its own background while the
//! GL context, the surface and the font atlas are still being built, and the
//! user sees a white box. Startup opens the sign-in window, the spinner and the
//! vault in sequence, which is why they reported it blinking more than once.
//!
//! The fix is `ViewportBuilder::with_visible(false)` at every one of those
//! windows, and [`Reveal`] is what decides when to undo it. It cannot simply be
//! undone on the first frame: every window in this crate spends frame one
//! painting its background and registering its fonts and then RETURNS, because
//! egui only makes a new font set live at the start of the following frame. A
//! window shown on that frame is an empty coloured rectangle -- quieter than
//! the white box, but still a window appearing before it has anything in it.
//!
//! **This works because `eframe` 0.35 goes out of its way to make it work.**
//! Windows sends no `WM_PAINT` to a window that is not on screen -- `winit`'s
//! own source says so where it creates its message target, and
//! `vault_window::spawn_show_waiter` was written around the same fact -- so an
//! invisible window receives no `RedrawRequested` and, naively, would never run
//! the frame that asks to be shown. `eframe::native::run`'s
//! `check_redraw_requests` handles exactly that: a window `winit` reports as
//! invisible or minimised is painted DIRECTLY, out of the same scheduler, so
//! its viewport commands are still processed. Without that this whole approach
//! would hang with no window at all, which is why it is written down here
//! rather than discovered again later.
//!
//! ## And it was half of the fix, because painted is not presented
//!
//! The owner, months after `with_visible(false)` landed: *"when loading there
//! is some white box first and it blinks few times - could it be smoother?"*
//! The "few times" was the three windows startup used to open in sequence;
//! startup is one window now (`main`'s own tests hold that), so what is left
//! is one box per window, and `with_visible(false)` alone could never close
//! it. `region_overlay` found out why, on the same defect on its own window,
//! and the mechanism is verified there: **a hidden window has no redirection
//! surface.** `eframe` does paint and swap the hidden window's frames, but
//! those swaps land nowhere. The surface is created by the show, empty, and
//! DWM composites that empty surface -- white -- until the first swap AFTER
//! the show. So the frame [`Reveal`] painted before asking to be shown was
//! never presented, and the one composite of nothing was the box.
//!
//! `DwmSetWindowAttribute(DWMWA_CLOAK)` is what closes it. A cloaked window
//! is realised: it is visible to USER32 and to DWM, it owns a surface, it
//! receives every swap and is composited -- and is simply not drawn.
//! [`Reveal`] therefore cloaks the window on the frame it asks for the show,
//! lets one more frame swap into the real surface, and uncloaks on the frame
//! after that, which reveals a finished frame. The class background brush is
//! NOT the mechanism and could not have been scoped anyway: `winit` registers
//! its class with `hbrBackground: 0`, and one class for every window in the
//! process.
//!
//! **Every window that goes through [`Reveal::hidden`] gets this** -- the
//! vault window, the startup window, the standalone sign-in card, the
//! spinner, both "Add app" windows and Preferences -- because it is one
//! change in one state machine and none of them differ in what is being
//! fixed. What does differ is the stakes of a refusal, and they are argued
//! at [`Reveal`]: the region overlay can cancel itself if DWM will not
//! uncloak it; the vault window cannot, so it retries and is raised
//! regardless.
//!
//! # What is NOT here
//!
//! Every window still opens through a `run_ui_native(TITLE, ..)` call in its
//! own module, and still calls `foreground::raise_window(TITLE)` there. That is
//! not an accident of style: `foreground`'s window census greps each
//! window-opening module's own source for those two needles and reconciles the
//! counts, so a raise moved into a shared helper would be a raise those tests
//! could no longer see. See
//! `foreground::every_window_this_crate_opens_asks_to_be_brought_to_the_front`.

use eframe::egui;

/// The clear colour every window opened through [`run_ui_native`] gets:
/// **fully transparent**, rather than `epi::App::clear_color`'s near-black
/// default.
///
/// Named rather than inlined so the pin in this module's tests and the
/// implementation cannot drift, and because `[0.0, 0.0, 0.0, 0.0]` written at
/// the one call site says nothing about which of the four channels is the one
/// that matters. It is the alpha: the RGB is irrelevant at zero alpha, and
/// writing black there is a convention, not a colour.
pub const FULLY_TRANSPARENT: [f32; 4] = [0.0, 0.0, 0.0, 0.0];

/// The `eframe::App` that `eframe::run_ui_native` builds privately, plus the
/// one override it does not offer.
///
/// Deliberately nothing else: `ui` forwards verbatim, and every other method of
/// the trait keeps its default, so a window moved from
/// `eframe::run_ui_native` to [`run_ui_native`] differs in the clear colour and
/// in nothing at all besides.
struct TransparentlyCleared<U> {
    ui_fun: U,
}

impl<U: FnMut(&mut egui::Ui, &mut eframe::Frame) + 'static> eframe::App
    for TransparentlyCleared<U>
{
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        (self.ui_fun)(ui, frame);
    }

    /// **Transparent, so that a window's opacity is the window's own doing.**
    ///
    /// See this module's doc for the whole argument. The short of it: the
    /// region overlay is a child viewport of the vault window and shares this
    /// app's clear colour, so a near-black clear is a near-black overlay no
    /// matter what the overlay itself paints. The two hosts that reach here
    /// paint their own opaque background on every frame, which is what makes
    /// giving the clear away safe.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        FULLY_TRANSPARENT
    }
}

/// `eframe::run_ui_native`, with [`TransparentlyCleared`] in place of
/// `eframe`'s private `SimpleApp`.
///
/// **The name is load-bearing.** `foreground`'s census counts
/// `run_ui_native(TITLE,` in each window module's own source, and two more
/// tests there assert the needle is findable at all; `login_ui`,
/// `app_window` and `vault_window` each hold a pin of their own on the same
/// call. Called as `window_host::run_ui_native(WINDOW_TITLE, ..)` every one of
/// those still reads the substring it is looking for. Renaming this function --
/// or importing it and calling it bare -- silently empties several of those
/// counts.
pub fn run_ui_native(
    app_name: &str,
    native_options: eframe::NativeOptions,
    ui_fun: impl FnMut(&mut egui::Ui, &mut eframe::Frame) + 'static,
) -> eframe::Result {
    eframe::run_native(
        app_name,
        native_options,
        Box::new(|_cc| Ok(Box::new(TransparentlyCleared { ui_fun }))),
    )
}

/// What [`Reveal::advance`] wants done this frame. Split out of the method so
/// the ordering can be driven by a test: the effects are one viewport command,
/// two DWM calls and a repaint request, and none of them can be observed from
/// `cargo test`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Ask the OS to show the window -- cloaked, if DWM took the cloak this
    /// same frame -- and ask for one more frame.
    Show,
    /// The window is on screen and cloaked, and this is the first frame whose
    /// swap lands in its real surface. Ask for one more frame and touch
    /// nothing else.
    Present,
    /// The surface holds a painted frame and the window has been uncloaked --
    /// or was shown plain, if the cloak was refused. Raise it.
    Raise,
    /// DWM refused to uncloak the window this frame. Ask for another frame and
    /// try again on it.
    Retry,
    /// Nothing. Either the window is already up and raised, or this host did
    /// not create it hidden in the first place.
    Nothing,
}

/// How many frames [`Reveal::Uncloaking`] keeps asking DWM to uncloak a
/// window it has already been asked to uncloak once.
///
/// A refusal of `DWMWA_CLOAK = FALSE` on a window that accepted `TRUE` a few
/// frames earlier is, in practice, DWM restarting (`DWM_E_COMPOSITIONDISABLED`
/// while the compositor comes back), and that is transient: the frames here
/// come back to back, so sixty of them is on the order of a second, which is
/// the scale a DWM restart takes. Bounded, because a `Reveal` must end up
/// inert -- an unbounded retry is one DWM call per frame for the life of the
/// window, and a machine on which the uncloak never takes would be paying it
/// for nothing.
const UNCLOAK_RETRIES: u8 = 60;

/// **How far a window created with `with_visible(false)` has got towards being
/// on screen, in front, and painted.**
///
/// Four frames, and each of the four is needed:
///
/// 1. The window's own first frame -- the `!styled` guard every window in this
///    crate has. It paints the background, registers the fonts and returns,
///    because egui makes a new font set live only at the *start* of the next
///    frame. Nothing here runs on it.
/// 2. The first frame that paints real content. [`Step::Show`]: the window is
///    **cloaked** first -- `DWMWA_CLOAK`, see [`Compositor`] -- and then the
///    `Visible(true)` command is queued. `eframe` applies queued viewport
///    commands after the frame has been rendered and its buffers swapped, so
///    the order at the OS is cloak, then show: from the moment Windows shows
///    this window it is drawn to no monitor. This frame's own swap lands
///    nowhere -- the window is still hidden while it happens, and a hidden
///    window has no surface to swap into (this module's doc, section 2).
/// 3. [`Step::Present`]. The window is on screen, cloaked, and owns a
///    redirection surface; this frame's swap is the first that lands in it.
///    Nothing to do but ask for the next frame. It cannot be skipped: the
///    uncloak has to come AFTER a swap into the real surface, and the only
///    place after this frame's swap that this code runs is the top of the
///    next one.
/// 4. [`Step::Raise`]. The uncloak, and then the raise -- in that order, so
///    that the foreground lands on a window the user can see. What DWM draws at
///    its next composite is the surface as swapped on frame 3. Separate from
///    the show, and it has to be: `foreground::pick` skips invisible windows,
///    so a `raise_window` issued on frame 2 would be looking for a window
///    Windows has not shown yet and would answer `Raised::NoWindow`. The raise
///    is therefore moved out of the `!styled` block, where every one of these
///    windows used to do it, and down to the first frame on which there is
///    something to raise.
///
/// # The refusals, and why they are not the region overlay's
///
/// `region_overlay` has the same machine on its own window and answers both
/// refusals the safe way for a full-screen sheet that takes input: a refused
/// cloak falls back to hidden-then-shown, and a refused uncloak CANCELS the
/// scan. The second answer is wrong here. Every window through this type is
/// one the user asked for, and the vault window is the application; a window
/// that cancels itself because DWM said no is the user with no app.
///
/// * **Cloak refused** (or no handle found for the window's title): the
///   machine this replaced, exactly -- `Visible(true)` on frame 2, raise on
///   frame 3, and one composite of an empty surface. [`Reveal::Showing`] with
///   no handle goes straight to [`Step::Raise`]. Logged as the fallback, with
///   the reason.
/// * **Uncloak refused**: the window is on screen and drawn nowhere. It is
///   raised anyway -- `SetForegroundWindow` does not care about the cloak, and
///   the raise must happen exactly once, on this frame, for `foreground`'s
///   census -- and the uncloak is retried on every following frame, up to
///   [`UNCLOAK_RETRIES`], each refusal logged. If it never takes, the log says
///   so at `error` with the handle: there is no second mechanism for making a
///   cloaked window drawn, and hiding and re-showing it does not clear the
///   attribute. `Retry` is bounded so that a `Reveal` always ends inert.
/// * **Readback says still cloaked**, after an uncloak DWM accepted: only the
///   `DWM_CLOAKED_APP` bit is ours. `DWM_CLOAKED_SHELL` is the shell's -- a
///   window on another virtual desktop -- and `DWM_CLOAKED_INHERITED` its
///   owner's, and a window cloaked for either reason is exactly as visible as
///   every other window of this app, so neither is treated as a refusal.
///
/// **Its interaction with `vault_window`'s `keep_ui_loaded` hiding is that
/// there is none, by construction.** That machinery hides an established window
/// (`Visible(false)`) and shows it again (`Visible(true)` plus `Focus`) when the
/// daemon rings; this one runs exactly once, on the way up, and is
/// [`Reveal::Done`] long before any of that is reachable -- the vault window
/// starts with `hidden` unset and cannot hide until the user closes it. `Done`
/// sends no commands at all, so the two never write `Visible` in the same frame
/// and neither one has to know about the other. (That re-show is the same
/// defect on the same window -- a fresh surface, composited empty once -- and
/// is NOT covered here; it is `vault_window`'s to take up, with this cloak.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// Created hidden; nothing has asked for it yet.
    Hidden,
    /// `Visible(true)` is queued. The window is on screen from the end of the
    /// frame that queued it -- cloaked, if `cloaked` names its handle, and
    /// plain if DWM refused or the window was not found.
    Showing { cloaked: Option<isize> },
    /// On screen and cloaked. The frame that entered this state is the first
    /// whose swap lands in the window's real surface.
    Presenting { hwnd: isize },
    /// On screen, painted and raised, and DWM refused to uncloak it. Retried
    /// once per frame until it takes or `tries_left` runs out.
    Uncloaking { hwnd: isize, tries_left: u8 },
    /// Visible and raised, or never hidden to begin with. Inert.
    Done,
}

/// **The two DWM calls [`Reveal`] makes, behind a seam so the machine can be
/// driven without a window.** [`Dwm`] is the real one; the tests' fake
/// answers what they tell it to.
pub trait Compositor {
    /// Cloak the window this reveal is for. `Some(hwnd)` if DWM took it;
    /// `None` if the window could not be found or DWM refused, in which case
    /// the window is shown plain.
    fn cloak(&mut self) -> Option<isize>;
    /// Uncloak `hwnd`. `true` if the window is drawn afterwards.
    fn uncloak(&mut self, hwnd: isize) -> bool;
}

impl Reveal {
    /// For a host that opened its own window with
    /// `ViewportBuilder::with_visible(false)`.
    pub const fn hidden() -> Self {
        Self::Hidden
    }

    /// For a frame closure someone else's window is hosting -- the `pre_styled`
    /// case in `login_ui::build_login_frame` and
    /// `vault_window::build_frame_with_search`.
    ///
    /// Those closures do not own a window: `app_window` created it, `app_window`
    /// styled it, and `app_window` reveals and raises it. A second `Reveal`
    /// running inside them would send a `Visible(true)` to a window that is
    /// already visible and raise a window that is already in front, once per
    /// stage change, for the rest of the session.
    pub const fn already_visible() -> Self {
        Self::Done
    }

    /// The pure half: what this frame should do, and where that leaves us.
    /// The compositor is the only thing it reaches out to, and only on the
    /// frames that need an answer from it.
    fn step(&mut self, dwm: &mut impl Compositor) -> Step {
        match *self {
            Self::Hidden => {
                // The cloak first, then the show -- `advance` queues the
                // command after this returns, and `eframe` applies it after
                // the swap, so the window is cloaked before Windows is asked
                // to show it whatever the order inside this frame.
                *self = Self::Showing { cloaked: dwm.cloak() };
                Step::Show
            }
            Self::Showing { cloaked: Some(hwnd) } => {
                *self = Self::Presenting { hwnd };
                Step::Present
            }
            Self::Showing { cloaked: None } => {
                // The fallback is the machine this replaced, to the frame.
                *self = Self::Done;
                Step::Raise
            }
            Self::Presenting { hwnd } => {
                *self = if dwm.uncloak(hwnd) {
                    Self::Done
                } else {
                    Self::Uncloaking { hwnd, tries_left: UNCLOAK_RETRIES }
                };
                // Raised either way, and exactly once: see the type's doc.
                Step::Raise
            }
            Self::Uncloaking { hwnd, tries_left } => {
                if dwm.uncloak(hwnd) {
                    *self = Self::Done;
                    Step::Nothing
                } else if tries_left <= 1 {
                    *self = Self::Done;
                    log::error!(
                        "window host: {hwnd:#x} was uncloaked {UNCLOAK_RETRIES} times and DWM \
                         refused every one; the window is on screen, raised, and drawn to no \
                         monitor, and this module has no second way to make it drawn. Closing \
                         and reopening the window is the way out"
                    );
                    Step::Nothing
                } else {
                    *self = Self::Uncloaking { hwnd, tries_left: tries_left - 1 };
                    Step::Retry
                }
            }
            Self::Done => Step::Nothing,
        }
    }

    /// Drives one frame. **Call at the top of every frame that will paint the
    /// window's real content** -- that is, below the `!styled` guard and above
    /// every early return the closure has, so a window whose body returns early
    /// (the vault's loading and unavailable pages both do) still gets shown.
    ///
    /// Answers `true` on exactly the one frame the caller must call
    /// `foreground::raise_window` on. That call stays at the call site rather
    /// than being made here for the reason this module's doc gives: the census
    /// in `foreground` counts it in the window module's own source.
    pub fn advance(&mut self, ctx: &egui::Context) -> bool {
        // Inert for the life of the window once settled, and cheaply: the
        // title lookup below is a clone out of the input state on every frame
        // that is not this early return.
        if matches!(self, Self::Done) {
            return false;
        }
        // The window is found by the title `eframe` gave it, which is the
        // `run_ui_native(TITLE, ..)` name -- `egui_winit` copies
        // `window.title()` into the viewport info before every frame. Read
        // here rather than passed in so that no call site changes: the seven
        // windows that reach this are in six modules.
        let mut dwm = Dwm { title: ctx.input(|i| i.viewport().title.clone()) };
        match self.step(&mut dwm) {
            Step::Show => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                // Without this the next frame is only requested if something
                // else happens to want one, and a window nobody has touched yet
                // -- which is every window on its second frame -- has nothing
                // else wanting one.
                ctx.request_repaint();
                false
            }
            Step::Present | Step::Retry => {
                ctx.request_repaint();
                false
            }
            Step::Raise => true,
            Step::Nothing => false,
        }
    }
}

/// The real [`Compositor`]: `DwmSetWindowAttribute(DWMWA_CLOAK)` on the
/// window titled `title`, with every outcome logged under this module's name
/// so the owner's next look at a white box is answerable against which
/// mechanism was in force.
struct Dwm {
    title: Option<String>,
}

impl Compositor for Dwm {
    fn cloak(&mut self) -> Option<isize> {
        let Some(title) = self.title.as_deref() else {
            log::warn!(
                "window host: the viewport reports no title, so its window cannot be found to \
                 be cloaked; it is shown plain, which can cost one composite of an empty \
                 surface -- the white box"
            );
            return None;
        };
        // `own_window_titled` does NOT skip invisible windows, which is what
        // makes a window created hidden findable at all. One `EnumWindows`,
        // on this frame only -- the same one `round_window_corners` already
        // spends on the frame before.
        let Some(hwnd) = crate::foreground::own_window_titled(title) else {
            log::warn!(
                "window host: no window of this process is titled {title:?} yet, so there is \
                 nothing to cloak; it is shown plain, which can cost one composite of an \
                 empty surface -- the white box"
            );
            return None;
        };
        set_cloak(hwnd, title, true).then_some(hwnd)
    }

    fn uncloak(&mut self, hwnd: isize) -> bool {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWM_CLOAKED_APP};

        let title = self.title.as_deref().unwrap_or("?");
        if !set_cloak(hwnd, title, false) {
            return false;
        }
        // "Accepted" is read back, because it has meant nothing before --
        // `region_overlay::take_picture` lists three DWM answers that did.
        let mut still: u32 = 0;
        let read = unsafe {
            DwmGetWindowAttribute(
                HWND(hwnd as *mut _),
                DWMWA_CLOAKED,
                (&mut still as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        match read {
            Ok(()) if still & DWM_CLOAKED_APP == 0 => {
                log::info!(
                    "window host: {title:?} ({hwnd:#x}) is revealed -- uncloaked with a painted \
                     frame already in its surface, and DWMWA_CLOAKED reads back {still:#x}{}",
                    if still == 0 {
                        ""
                    } else {
                        " (not this app's bit: the shell's or an owner's, which is as visible \
                         as any window of this app is)"
                    }
                );
                true
            }
            Ok(()) => {
                log::warn!(
                    "window host: {title:?} ({hwnd:#x}) was uncloaked and DWMWA_CLOAKED still \
                     reads {still:#x} with this app's bit set; the window is on screen and \
                     drawn nowhere, so the uncloak is retried on the next frame"
                );
                false
            }
            Err(e) => {
                // It is the readback that failed, not the uncloak; retrying a
                // reveal the user can most likely see, on the strength of a
                // failed query, is the wrong direction.
                log::warn!(
                    "window host: {title:?} ({hwnd:#x}) was uncloaked but DWMWA_CLOAKED could \
                     not be read back ({e}); taking the uncloak at its word"
                );
                true
            }
        }
    }
}

/// `DwmSetWindowAttribute(DWMWA_CLOAK)`, on or off, with the outcome logged
/// and answered rather than discarded. The one `DwmSetWindowAttribute` in this
/// module, and the one attribute: whether the window is DRAWN, which is the
/// one DWM answer this module can set and read back.
fn set_cloak(hwnd: isize, title: &str, on: bool) -> bool {
    use windows::Win32::Foundation::{BOOL, HWND};
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_CLOAK};

    let value = BOOL::from(on);
    let result = unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_CLOAK,
            (&value as *const BOOL).cast(),
            std::mem::size_of::<BOOL>() as u32,
        )
    };
    match result {
        Ok(()) => {
            log::info!(
                "window host: {title:?} ({hwnd:#x}) is {} (DWMWA_CLOAK accepted); {}",
                if on { "cloaked" } else { "uncloaked" },
                if on {
                    "it will be shown, painted and composited and reach no monitor until it \
                     is uncloaked"
                } else {
                    "what DWM draws next is the surface as already swapped"
                }
            );
            true
        }
        Err(e) => {
            log::warn!(
                "window host: DWMWA_CLOAK {} refused on {title:?} ({hwnd:#x}): {e}; {}",
                if on { "on" } else { "off" },
                if on {
                    "the window is shown plain once painted, which can cost one composite of \
                     an empty surface -- the white box"
                } else {
                    "the window is on screen and drawn nowhere until an uncloak takes"
                }
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The clear colour is transparent**, which is the whole of what this
    /// module's `App` impl adds and the half of the black-overlay fix that a
    /// test can reach at all.
    ///
    /// The other half -- the root viewport asking for an alpha channel it does
    /// not use itself -- is a builder flag in two other modules and is pinned
    /// there. Nothing here can prove the two combine correctly on a real GL
    /// config; that needs a desktop.
    #[test]
    fn the_clear_colour_is_fully_transparent() {
        let app = TransparentlyCleared {
            ui_fun: |_: &mut egui::Ui, _: &mut eframe::Frame| {},
        };
        let cleared = eframe::App::clear_color(&app, &egui::Visuals::light());
        assert_eq!(
            cleared, FULLY_TRANSPARENT,
            "the clear colour is not transparent, so an unpainted pixel is a coloured pixel \
             -- and the region overlay, which is a child viewport of the vault window and \
             shares this colour, is a solid rectangle instead of a dimmed desktop"
        );
        assert_eq!(
            cleared[3], 0.0,
            "the alpha is what this constant is for; the RGB at zero alpha is a convention"
        );
        // And it is not `eframe`'s default, which is the value this override
        // exists to displace. Written out rather than read off `eframe`,
        // because reading it off `eframe` would make the assertion true by
        // construction whatever either side did.
        let eframes_default =
            egui::Color32::from_rgba_unmultiplied(12, 12, 12, 180).to_normalized_gamma_f32();
        assert_ne!(
            cleared, eframes_default,
            "control: this is `epi::App::clear_color`'s default, so the override is not \
             overriding anything"
        );
    }

    /// A [`Compositor`] that answers what it is told to and writes down what
    /// it was asked, in order. `cloak` is the handle to answer (or `None` for
    /// a refusal); `uncloaks` are the answers to successive uncloak calls,
    /// front first, with the last one repeated once they run out.
    struct FakeDwm {
        cloak: Option<isize>,
        uncloaks: Vec<bool>,
        asked: Vec<String>,
    }

    impl Compositor for FakeDwm {
        fn cloak(&mut self) -> Option<isize> {
            self.asked.push("cloak".into());
            self.cloak
        }
        fn uncloak(&mut self, hwnd: isize) -> bool {
            self.asked.push(format!("uncloak {hwnd:#x}"));
            if self.uncloaks.len() > 1 {
                self.uncloaks.remove(0)
            } else {
                self.uncloaks.first().copied().unwrap_or(true)
            }
        }
    }

    /// **Cloak and show, present, uncloak and raise, then nothing** -- and
    /// never another order.
    ///
    /// The ordering is not cosmetic. `foreground::pick` skips invisible
    /// windows, so a raise issued on the same frame as the show finds no window
    /// at all: `eframe` applies viewport commands after the frame is rendered,
    /// so at the moment the raise would run, Windows has not been told to show
    /// anything yet. And the uncloak has to come one frame later than the
    /// show, not on the same frame: the frame that shows the window swaps
    /// into nothing, and only the frame after it swaps into the surface the
    /// show created. Both failures are invisible in a diff.
    #[test]
    fn a_hidden_window_is_cloaked_shown_presented_then_uncloaked_and_raised() {
        let mut dwm = FakeDwm { cloak: Some(0x1234), uncloaks: vec![true], asked: vec![] };
        let mut reveal = Reveal::hidden();
        assert_eq!(reveal.step(&mut dwm), Step::Show);
        assert_eq!(reveal, Reveal::Showing { cloaked: Some(0x1234) });
        assert_eq!(dwm.asked, ["cloak"], "the cloak is not asked for on the show frame");
        assert_eq!(reveal.step(&mut dwm), Step::Present);
        assert_eq!(reveal, Reveal::Presenting { hwnd: 0x1234 });
        assert_eq!(
            dwm.asked,
            ["cloak"],
            "DWM was asked something on the present frame, whose only job is to swap into \
             the surface the show created -- an uncloak here reveals an empty surface"
        );
        assert_eq!(reveal.step(&mut dwm), Step::Raise);
        assert_eq!(reveal, Reveal::Done);
        assert_eq!(dwm.asked, ["cloak", "uncloak 0x1234"]);
        // And it is over: a window is shown once and raised once, however many
        // hundred frames follow, and DWM is not asked again.
        for _ in 0..100 {
            assert_eq!(
                reveal.step(&mut dwm),
                Step::Nothing,
                "a settled `Reveal` is still asking for something, so this window re-raises \
                 itself every frame -- which is a window that cannot be put behind anything"
            );
        }
        assert_eq!(dwm.asked.len(), 2, "a settled `Reveal` went back to DWM");
    }

    /// **A refused cloak is the machine this one replaced, to the frame:**
    /// show, raise, nothing -- with no present frame and no uncloak, because
    /// there is nothing cloaked to wait for or to uncloak. That path was the
    /// shipped behaviour for months and is the stated fallback; it must not
    /// have acquired an extra frame or a DWM call on the way.
    #[test]
    fn a_refused_cloak_falls_back_to_show_then_raise() {
        let mut dwm = FakeDwm { cloak: None, uncloaks: vec![true], asked: vec![] };
        let mut reveal = Reveal::hidden();
        assert_eq!(reveal.step(&mut dwm), Step::Show);
        assert_eq!(reveal, Reveal::Showing { cloaked: None });
        assert_eq!(reveal.step(&mut dwm), Step::Raise);
        assert_eq!(reveal, Reveal::Done);
        for _ in 0..10 {
            assert_eq!(reveal.step(&mut dwm), Step::Nothing);
        }
        assert_eq!(
            dwm.asked,
            ["cloak"],
            "a window that was never cloaked was asked to be uncloaked, or asked twice"
        );
    }

    /// **A refused uncloak still raises, exactly once, and then retries.**
    ///
    /// This is the refusal that must not be answered the way the region
    /// overlay answers it. The overlay cancels itself; a vault window that
    /// cancelled itself would be the user with no application. So the raise
    /// happens on its frame regardless -- `foreground`'s census counts one
    /// raise per window and the caller raises on exactly the `true` -- and
    /// the uncloak is asked for again on every frame after until it takes.
    #[test]
    fn a_refused_uncloak_raises_once_and_retries_until_it_takes() {
        let mut dwm = FakeDwm {
            cloak: Some(0x77),
            uncloaks: vec![false, false, false, true],
            asked: vec![],
        };
        let mut reveal = Reveal::hidden();
        assert_eq!(reveal.step(&mut dwm), Step::Show);
        assert_eq!(reveal.step(&mut dwm), Step::Present);
        // The first uncloak is refused. Raised anyway, and the machine is not
        // done.
        assert_eq!(reveal.step(&mut dwm), Step::Raise);
        assert_eq!(reveal, Reveal::Uncloaking { hwnd: 0x77, tries_left: UNCLOAK_RETRIES });
        // Two more refusals are two more retries...
        assert_eq!(reveal.step(&mut dwm), Step::Retry);
        assert_eq!(reveal.step(&mut dwm), Step::Retry);
        assert_eq!(reveal, Reveal::Uncloaking { hwnd: 0x77, tries_left: UNCLOAK_RETRIES - 2 });
        // ...and the one that takes ends it, with no second raise.
        assert_eq!(reveal.step(&mut dwm), Step::Nothing);
        assert_eq!(reveal, Reveal::Done);
        for _ in 0..10 {
            assert_eq!(reveal.step(&mut dwm), Step::Nothing);
        }
        let uncloaks = dwm.asked.iter().filter(|a| a.starts_with("uncloak")).count();
        assert_eq!(uncloaks, 4, "the uncloak was not retried once per frame until it took");
    }

    /// **And the retry is bounded.** A machine on which the uncloak never
    /// takes ends up `Done` -- inert, raised, logged at `error` -- rather than
    /// asking DWM once per frame for the life of the window. The bound is the
    /// constant, read off it rather than restated.
    #[test]
    fn a_never_accepted_uncloak_gives_up_after_the_bound_and_settles() {
        let mut dwm = FakeDwm { cloak: Some(0x9), uncloaks: vec![false], asked: vec![] };
        let mut reveal = Reveal::hidden();
        assert_eq!(reveal.step(&mut dwm), Step::Show);
        assert_eq!(reveal.step(&mut dwm), Step::Present);
        assert_eq!(reveal.step(&mut dwm), Step::Raise);
        let mut retries = 0;
        for _ in 0..(UNCLOAK_RETRIES as usize * 2) {
            match reveal.step(&mut dwm) {
                Step::Retry => retries += 1,
                Step::Nothing => break,
                other => panic!("{other:?} after the raise: the window would be raised twice"),
            }
        }
        assert_eq!(reveal, Reveal::Done);
        assert_eq!(
            retries,
            UNCLOAK_RETRIES as usize - 1,
            "the retry is not bounded by `UNCLOAK_RETRIES`"
        );
        for _ in 0..10 {
            assert_eq!(reveal.step(&mut dwm), Step::Nothing);
        }
        // One ask on the raise frame, then one per retry frame: the constant
        // counts the retries, not the asks.
        let uncloaks = dwm.asked.iter().filter(|a| a.starts_with("uncloak")).count();
        assert_eq!(
            uncloaks,
            UNCLOAK_RETRIES as usize + 1,
            "DWM was asked after the machine gave up"
        );
    }

    /// A frame closure hosted by somebody else's window asks for nothing at
    /// all, ever -- of DWM either. See [`Reveal::already_visible`].
    #[test]
    fn a_pre_styled_host_neither_shows_nor_raises() {
        let mut dwm = FakeDwm { cloak: Some(0x1), uncloaks: vec![true], asked: vec![] };
        let mut reveal = Reveal::already_visible();
        for _ in 0..10 {
            assert_eq!(
                reveal.step(&mut dwm),
                Step::Nothing,
                "the `pre_styled` frame closures do not own their window, and this one is \
                 sending `Visible(true)` and re-raising `app_window`'s window from inside the \
                 stage it is drawing"
            );
        }
        assert!(dwm.asked.is_empty(), "a pre-styled host cloaked a window it does not own");
    }

    /// **The one DWM attribute this module sets is the cloak, from one place,
    /// and the one viewport command it sends is `Visible(true)`, from one
    /// place.**
    ///
    /// The same pin `region_overlay` holds on its own copy, for the same
    /// reason: `DWMWA_CLOAK` is whether the window is drawn, not how, and it is
    /// the one DWM answer this module can read back. Anything else written
    /// through `DwmSetWindowAttribute` here would be a compositing request on
    /// every window in the crate at once.
    #[test]
    fn the_only_dwm_attribute_here_is_the_cloak_and_the_only_command_is_the_show() {
        let source = include_str!("window_host.rs").replace("\r\n", "\n");
        let code = source.split("#[cfg(test)]").next().unwrap();
        let statements: String = code
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            statements.matches("DwmSetWindowAttribute(").count(),
            1,
            "`DwmSetWindowAttribute` is called from more than one place; the cloak in \
             `set_cloak` is the only attribute this module may ask DWM for"
        );
        let cloaker = statements
            .split("fn set_cloak(hwnd: isize, title: &str, on: bool) -> bool {")
            .nth(1)
            .expect("`set_cloak` is gone")
            .split("\n}")
            .next()
            .unwrap();
        assert!(
            cloaker.contains("DwmSetWindowAttribute(") && cloaker.contains("DWMWA_CLOAK,"),
            "the one `DwmSetWindowAttribute` call is not the cloak"
        );
        assert_eq!(
            statements.matches("send_viewport_cmd(").count(),
            1,
            "this module sends a viewport command from more than one place; the show is the \
             only one, and `vault_window`'s `keep_ui_loaded` hiding relies on nothing else \
             here ever writing `Visible`"
        );
        assert_eq!(
            statements.matches("ViewportCommand::Visible(true)").count(),
            1,
            "the show is no longer the one `Visible(true)` in this module"
        );
        // The uncloak precedes the raise on the same frame: `step` answers
        // `Raise` from `Presenting` only after `dwm.uncloak` has been asked.
        let presenting = statements
            .split("Self::Presenting { hwnd } => {")
            .nth(1)
            .expect("the `Presenting` arm is gone")
            .split("Step::Raise")
            .next()
            .expect("the `Presenting` arm no longer answers `Raise`");
        assert!(
            presenting.contains("dwm.uncloak(hwnd)"),
            "the raise is answered before the uncloak is asked for, so the foreground lands \
             on a window the user cannot see"
        );
    }

    /// The census needle this module is named for is really in this file, in
    /// the form the census matches.
    ///
    /// A positive control on the naming constraint rather than on behaviour:
    /// `foreground`'s tables grep `run_ui_native(TITLE,` in each *window*
    /// module, so what they can never see is this function being renamed out
    /// from under them -- the call sites would stop compiling, but a rename
    /// that kept a compiling `use` alias would not.
    #[test]
    fn the_shim_is_still_spelled_the_way_the_window_census_greps_for_it() {
        let source = include_str!("window_host.rs");
        assert!(
            source.contains(concat!("pub fn run_ui_", "native(")),
            "this module's entry point is no longer named `run_ui_native`, so every call site \
             that reads `run_ui_native(WINDOW_TITLE,` has been rewritten -- and \
             `foreground`'s window census, which counts exactly that substring, now counts \
             zero windows for modules that open one"
        );
    }
}

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
/// the ordering can be driven by a test: the effects are one viewport command
/// and one repaint request, and neither can be observed from `cargo test`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Ask the OS to show the window, and ask for one more frame so the raise
    /// below actually happens.
    Show,
    /// The window is on screen now. Raise it.
    Raise,
    /// Nothing. Either the window is already up and raised, or this host did
    /// not create it hidden in the first place.
    Nothing,
}

/// **How far a window created with `with_visible(false)` has got towards being
/// on screen, in front, and painted.**
///
/// Three frames, and each of the three is needed:
///
/// 1. The window's own first frame -- the `!styled` guard every window in this
///    crate has. It paints the background, registers the fonts and returns,
///    because egui makes a new font set live only at the *start* of the next
///    frame. Nothing here runs on it.
/// 2. The first frame that paints real content. [`Step::Show`]: the
///    `Visible(true)` command is queued at the TOP of that frame, and `eframe`
///    applies queued viewport commands after the frame has been rendered and
///    its buffers swapped -- so the window appears with this frame's content
///    already in it, which is the whole point.
/// 3. [`Step::Raise`]. Separate from the show, and it has to be:
///    `foreground::pick` skips invisible windows, so a `raise_window` issued on
///    frame 2 would be looking for a window Windows has not shown yet and would
///    answer `Raised::NoWindow`. The raise is therefore moved out of the
///    `!styled` block, where every one of these windows used to do it, and down
///    to the first frame on which there is something to raise.
///
/// **Its interaction with `vault_window`'s `keep_ui_loaded` hiding is that
/// there is none, by construction.** That machinery hides an established window
/// (`Visible(false)`) and shows it again (`Visible(true)` plus `Focus`) when the
/// daemon rings; this one runs exactly once, on the way up, and is
/// [`Reveal::Done`] long before any of that is reachable -- the vault window
/// starts with `hidden` unset and cannot hide until the user closes it. `Done`
/// sends no commands at all, so the two never write `Visible` in the same frame
/// and neither one has to know about the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// Created hidden; nothing has asked for it yet.
    Hidden,
    /// `Visible(true)` is queued. The window is on screen from the end of the
    /// frame that queued it.
    Showing,
    /// Visible and raised, or never hidden to begin with. Inert.
    Done,
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
    fn step(&mut self) -> Step {
        match self {
            Self::Hidden => {
                *self = Self::Showing;
                Step::Show
            }
            Self::Showing => {
                *self = Self::Done;
                Step::Raise
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
        match self.step() {
            Step::Show => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                // Without this the raise frame is only requested if something
                // else happens to want one, and a window nobody has touched yet
                // -- which is every window on its second frame -- has nothing
                // else wanting one.
                ctx.request_repaint();
                false
            }
            Step::Raise => true,
            Step::Nothing => false,
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

    /// **Show, then raise, then nothing** -- and never the other order.
    ///
    /// The ordering is not cosmetic. `foreground::pick` skips invisible
    /// windows, so a raise issued on the same frame as the show finds no window
    /// at all: `eframe` applies viewport commands after the frame is rendered,
    /// so at the moment the raise would run, Windows has not been told to show
    /// anything yet. That is the failure this sequence exists to prevent and it
    /// is invisible in a diff.
    #[test]
    fn a_hidden_window_is_shown_first_and_raised_second() {
        let mut reveal = Reveal::hidden();
        assert_eq!(reveal.step(), Step::Show);
        assert_eq!(reveal, Reveal::Showing);
        assert_eq!(reveal.step(), Step::Raise);
        assert_eq!(reveal, Reveal::Done);
        // And it is over: a window is shown once and raised once, however many
        // hundred frames follow.
        for _ in 0..100 {
            assert_eq!(
                reveal.step(),
                Step::Nothing,
                "a settled `Reveal` is still asking for something, so this window re-raises \
                 itself every frame -- which is a window that cannot be put behind anything"
            );
        }
    }

    /// A frame closure hosted by somebody else's window asks for nothing at
    /// all, ever. See [`Reveal::already_visible`].
    #[test]
    fn a_pre_styled_host_neither_shows_nor_raises() {
        let mut reveal = Reveal::already_visible();
        for _ in 0..10 {
            assert_eq!(
                reveal.step(),
                Step::Nothing,
                "the `pre_styled` frame closures do not own their window, and this one is \
                 sending `Visible(true)` and re-raising `app_window`'s window from inside the \
                 stage it is drawing"
            );
        }
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

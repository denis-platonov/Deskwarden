//! The detail pane's slide, and the one rule that governs it.
//!
//! With nothing selected the item list runs to the right edge of the window.
//! Select something and the read pane comes in from that edge, pushing the
//! list back to [`super::LIST_WIDTH`]; close it and the list takes the room
//! back. The list genuinely SHRINKS -- it is a reflow, not a pane sliding over
//! the top of it -- which costs nothing per frame because
//! `ScrollArea::show_rows` virtualises the list: the rows laid out during the
//! animation are the dozen on screen, not the vault.
//!
//! # The arming rule
//!
//! **The animation is armed by a GESTURE, never inferred from state.** This is
//! the whole design, it is narrower than "the selection changed", and it is
//! the owner's, twice, the second time correcting exactly that reading:
//!
//! > "it only happens on close-open, if something already open and user clicks
//! > another item - no animation, just new item data populated on the same
//! > place"
//!
//! > "the window opening with an item already selected - no animation, as I
//! > said ONLY on X icon or result clicked with no details panel"
//!
//! So exactly two gestures arm it, and [`Slide::arm`] is called at exactly two
//! places in `vault_window::mod`:
//!
//! 1. the detail header's ✕ (`DetailAction::ClosePane`);
//! 2. a row opened by a primary click **while no detail pane is shown**.
//!
//! Everything else changes the selection without animating, and the list of
//! things that do is long enough that inferring from state would be actively
//! wrong rather than merely loose:
//!
//! * the window opening with an item already selected;
//! * `apply_vault_load_result`'s auto-select of `items.first()`, which fires on
//!   EVERY sync -- inferring from state would make the pane slide in at random
//!   moments while the vault refreshed in the background;
//! * a selection restored or dropped by a sync;
//! * a right-click, which selects the row on its way to a menu;
//! * `Create`'s select-what-was-just-made;
//! * switching from one item to another, where the pane stays put and only its
//!   contents change.
//!
//! `arm_sites_are_exactly_the_two_the_owner_named` pins the count at two so no
//! third path can quietly start animating later.

use eframe::egui;

/// How long the pane takes to come in or go out.
///
/// Short on purpose: this is a reflow, so every frame of it re-lays the
/// visible rows and the read pane. Long enough to read as motion rather than a
/// jump, short enough that nothing can be clicked mid-flight and be surprised.
const SLIDE_SECONDS: f32 = 0.18;

/// The egui animation this drives. One id for the window's one detail pane.
const SLIDE_ID: &str = "vault-detail-slide";

/// The detail pane's slide state: whether the NEXT width change animates, and
/// nothing else.
///
/// It deliberately does not know whether a pane is open -- that is
/// `selected_id`'s job and duplicating it here would be a second answer to the
/// same question. This type only ever answers "animate, or jump?".
#[derive(Default)]
pub struct Slide {
    /// Set by [`arm`](Self::arm), cleared when the pane finishes moving.
    ///
    /// It has to survive across frames: the gesture happens in one frame and
    /// the movement takes many, so a flag that lived for a single frame would
    /// animate the first frame and snap through the rest.
    armed: bool,
}

impl Slide {
    /// **Arm the animation.** Call at a GESTURE and nowhere else -- see the
    /// module's arming rule for the two places this is allowed to be called
    /// from, and the pin that holds it at two.
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// The detail pane's width this frame, given the width it wants when fully
    /// open (`full`) and whether a pane is being shown at all.
    ///
    /// **The unarmed path does not merely pass `0.0` as the duration**, and
    /// that is the one subtle thing in this file. egui's `animate_value` with
    /// a zero duration fixes its endpoints to the new target but still RETURNS
    /// the previous one for that frame, so a jump driven by the return value
    /// would land one frame late -- a visible flash of the old layout on every
    /// sync-driven selection change, which is exactly what this rule exists to
    /// avoid. So the call is still made (it resets egui's state, so a LATER
    /// armed transition starts from where the pane actually is) and the target
    /// is used directly.
    pub fn width(&mut self, ctx: &egui::Context, full: f32, showing: bool) -> f32 {
        let target = if showing { full.max(0.0) } else { 0.0 };
        let id = egui::Id::new(SLIDE_ID);
        let duration = if self.armed { SLIDE_SECONDS } else { 0.0 };
        let animated = ctx.animate_value_with_time(id, target, duration);
        if !self.armed {
            return target;
        }
        // Still moving: egui only advances the value when it is asked to
        // paint, so a window with nothing else going on would freeze the pane
        // half-open without this.
        if (animated - target).abs() > 0.5 {
            ctx.request_repaint();
        } else {
            self.armed = false;
        }
        animated
    }
}

/// Lays `contents` out at the width the pane has AT REST and shows whatever
/// of it the panel is currently wide enough to hold, clipped.
///
/// **This is what makes it a slide rather than a squeeze, and it is not a
/// cosmetic choice.** The panel really does narrow to nothing during the
/// animation, so laying the pane out in whatever it has been given means
/// laying it out at 200pt, at 40pt and at 1pt on the way in. The read pane
/// has no sensible layout at those widths: a sweep found it painting a header
/// control at x = -108, which is the same defect this file has already had
/// once at a real window size (a control measured at x = -34.5) -- only
/// reachable on the way into EVERY item rather than at one window size.
///
/// So the pane is never laid out narrow. It is laid out at `resting`, anchored
/// to the panel's LEFT edge, and clipped to the panel: the panel's left edge
/// travels leftwards across the window and uncovers the pane as it goes. That
/// is also the better-looking of the two, and the one the owner described --
/// the pane comes in from the right edge whole, rather than reflowing through
/// every intermediate layout on the way.
///
/// **At rest this does nothing at all.** When the panel is already at least
/// `resting` wide -- which is every frame that is not part of an animation --
/// `contents` is handed the caller's own `ui` untouched, so the shipped layout
/// is reached by exactly the code path it always was.
pub fn reveal<R>(
    ui: &mut egui::Ui,
    resting: f32,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let outer = ui.max_rect();
    if outer.width() >= resting {
        return contents(ui);
    }
    // The pane's real box, starting at the panel's left edge and running its
    // full resting width -- off the right of the panel, where the clip takes
    // over.
    let laid_out = egui::Rect::from_min_size(outer.min, egui::vec2(resting, outer.height()));
    let clip = ui.clip_rect().intersect(outer);
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(laid_out).layout(*ui.layout()));
    // Explicit, not inherited: `new_child` carries the parent's clip, and the
    // parent's is the panel's own rect only because the panel set it. Stating
    // it here is what stops a pane laid out past the panel's right edge from
    // painting over the item list.
    child.set_clip_rect(clip);
    contents(&mut child)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context that has run at least one frame, so egui's animation manager
    /// has an entry to move rather than a first-call value to seed.
    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    /// **Unarmed, the pane is simply THERE**, at the full width, on the very
    /// first frame it is shown. This is the auto-select path, the sync path
    /// and the window-opens-with-a-selection path all at once, and the value
    /// must be the target with no interpolation and no one-frame lag.
    #[test]
    fn an_unarmed_change_lands_at_the_full_width_immediately() {
        let ctx = ctx();
        let mut slide = Slide::default();
        assert_eq!(
            slide.width(&ctx, 600.0, true),
            600.0,
            "an unarmed open did not land at the full width on its own frame -- the pane \
             would flash the old layout for a frame on every sync"
        );
        assert_eq!(
            slide.width(&ctx, 600.0, false),
            0.0,
            "an unarmed close did not land closed on its own frame"
        );
    }

    /// **Armed, it starts from where it was** -- which for an open is not the
    /// full width. This is the positive control for the test above: without
    /// it, "unarmed jumps" would pass just as well against a `Slide` that
    /// never animated at all.
    #[test]
    fn an_armed_open_starts_partway_and_is_not_the_full_width() {
        let ctx = ctx();
        let mut slide = Slide::default();
        // Closed, unarmed, so the pane is genuinely at 0 to start from.
        assert_eq!(slide.width(&ctx, 600.0, false), 0.0);
        slide.arm();
        let first = slide.width(&ctx, 600.0, true);
        assert!(
            first < 600.0,
            "an armed open jumped straight to {first} instead of animating in from the edge"
        );
    }

    /// **Mid-slide, the pane is laid out at its RESTING width** and the panel
    /// clips it -- it is not squeezed into the sliver it is being revealed
    /// through.
    ///
    /// This is the assertion that stands between the read pane and the widths
    /// it has no layout for. A sweep of the read pane from 0pt up found it
    /// painting a header control at x = -108; `reveal` is why it is never
    /// asked to. Both halves matter, so both are asserted: the width it lays
    /// out at, and the clip that keeps the overhang off the item list.
    #[test]
    fn a_sliver_of_a_panel_still_lays_the_pane_out_at_its_full_width() {
        let ctx = ctx();
        let panel = egui::Rect::from_min_size(egui::pos2(900.0, 0.0), egui::vec2(40.0, 700.0));
        let mut laid_out = None;
        let mut clip = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            let mut host = ui.new_child(egui::UiBuilder::new().max_rect(panel));
            host.set_clip_rect(panel);
            reveal(&mut host, 638.0, |ui| {
                laid_out = Some(ui.max_rect());
                clip = Some(ui.clip_rect());
            });
        });
        let laid_out = laid_out.expect("reveal ran its contents");
        assert_eq!(
            laid_out.width(),
            638.0,
            "the pane was laid out at {} in a 40pt sliver instead of its 638pt resting \
             width -- it is being squeezed through the animation, which is the layout it \
             has no sensible answer for",
            laid_out.width()
        );
        assert_eq!(
            laid_out.left(),
            panel.left(),
            "the pane is not anchored to the panel's left edge, so it is not being \
             uncovered from the right"
        );
        let clip = clip.expect("reveal ran its contents");
        assert!(
            clip.right() <= panel.right() + 0.01 && clip.left() >= panel.left() - 0.01,
            "the pane's clip {clip:?} reaches outside the {panel:?} panel, so the part of \
             it hanging past the panel would paint over the item list"
        );
    }

    /// **At rest, `reveal` is a straight call through.** The shipped layout is
    /// reached by the same code path it always was, so nothing about the
    /// resting window depends on this function being right.
    #[test]
    fn a_panel_at_the_resting_width_is_passed_through_untouched() {
        let ctx = ctx();
        let panel = egui::Rect::from_min_size(egui::pos2(602.0, 0.0), egui::vec2(638.0, 700.0));
        let (mut inner, mut host_rect) = (None, None);
        let _ = ctx.run_ui(Default::default(), |ui| {
            let mut host = ui.new_child(egui::UiBuilder::new().max_rect(panel));
            host.set_clip_rect(panel);
            host_rect = Some(host.max_rect());
            reveal(&mut host, 638.0, |ui| {
                inner = Some(ui.max_rect());
            });
        });
        assert_eq!(
            inner.expect("reveal ran its contents"),
            host_rect.expect("the host was built"),
            "a panel already at the resting width did not get its own ui handed to the \
             pane, so the shipped layout is going through the animation's code path"
        );
    }

    /// Arming is not permanent: once the pane has arrived, the NEXT change is
    /// unarmed again. Otherwise one ✕ would animate every selection change for
    /// the rest of the session -- which is the owner's rule inverted.
    #[test]
    fn the_arming_is_spent_once_the_pane_has_arrived() {
        let ctx = ctx();
        let mut slide = Slide::default();
        assert_eq!(slide.width(&ctx, 600.0, false), 0.0);
        slide.arm();
        assert!(slide.armed, "arm() did not arm");
        // Drive it to the end. The width is asked for repeatedly, as frames
        // would; `animate_value_with_time` advances on egui's clock, so the
        // context is run between asks.
        for _ in 0..200 {
            let _ = ctx.run_ui(Default::default(), |_| {});
            let w = slide.width(&ctx, 600.0, true);
            if !slide.armed {
                assert!(
                    (w - 600.0).abs() < 1.0,
                    "the slide disarmed at {w}, short of its 600 target"
                );
                break;
            }
        }
        assert!(
            !slide.armed,
            "the slide never disarmed, so every later selection change would animate"
        );
    }
}

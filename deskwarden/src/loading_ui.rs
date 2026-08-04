//! A small spinner window shown while a background thread does slow,
//! non-interactive startup work (currently: waiting for `bw serve` to become
//! ready after login).
//!
//! Without this, the gap between the login window closing and the tray icon
//! appearing -- up to ~28s on a cold `bw serve` start -- showed nothing from
//! Deskwarden at all. Whatever else happened to be on screen (a terminal, in
//! more than one report) filled that silence, reading as "the app opened an
//! empty terminal" rather than "the app is still starting up".

use crate::login_ui::{
    draw_window_chrome_with_extra, round_window_corners, ChromeAction, ChromeMetrics, CloseControl,
};
use crate::theme;
use eframe::egui::{self, Margin};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

/// Named rather than inlined at the `run_ui_native` call, because
/// `foreground::raise_window` finds this window BY this title -- one
/// declaration means the two cannot drift apart.
const WINDOW_TITLE: &str = "Deskwarden";

/// Shows a "Deskwarden" window with a spinner and `message` until `rx`
/// yields a value, then closes and returns `Some(value)`.
///
/// `rx` is expected to be the receiving half of a channel whose sending half
/// was handed to a worker thread computing `T`. Both current call sites
/// (`main::wait_for_vault_ready_with_spinner` and
/// `picker_ui::pick_vault_item`) *detach* that worker with a bare
/// `std::thread::spawn`, giving it owned data (a `VaultBridge` clone, an
/// `Arc<VaultCache>` clone) rather than borrowing from the caller's stack.
/// That is deliberate: a `std::thread::scope`d worker -- which is what both
/// sites used to do -- forces the caller to block until the worker finishes
/// even after this function has already returned `None` because the user
/// closed the window, which made "the user can close this" a lie (review
/// 12). Detached, the worker simply finishes on its own and its result is
/// dropped unheard. `rx` itself is self-contained (an `mpsc::Receiver<T>`
/// borrows nothing), so it is fine to move into this window's own `'static`
/// closure regardless of where it was created.
///
/// Returns `None` if the window closed without `rx` ever yielding a value --
/// either the user closed it via the heading's ✕ or Alt+F4 (that ✕ is drawn
/// live -- `CloseControl::Active` -- because nothing about "loading" makes
/// THIS window modal or un-closable; the startup window's working stage is the
/// one that cannot be closed, and it says so by ghosting its own), or the
/// worker thread disconnected the channel without
/// sending (e.g. it panicked). Review 11's Critical: this used to
/// `.expect()` on exactly that case, which meant a user closing this spinner
/// while `pick_vault_item`'s populate ran panicked the main thread, and
/// `main.rs`'s panic hook only logs -- so the process unwound out of `main`
/// and the tray icon, hotkey, and autofill all vanished with it. Every
/// caller must now decide for itself what "the user closed this" means
/// (abandon quietly, treat as a failure, etc.) rather than that decision
/// being made for it by a crash.
/// The spinner itself: **this app's window heading**, and under it a spinner
/// and one line of prose centred on a flat [`theme::CANVAS`] panel that fills
/// whatever is left.
///
/// The heading is the same [`draw_window_chrome_with_extra`] every other window
/// in this app draws, not a second titlebar -- there was no chrome here at all
/// until the user asked for it ("keep the same window heading as the rest of
/// the windows"), which made this the one screen in the app with no title, no
/// drag zone and no window controls.
///
/// Under it, the treatment is the vault window's OWN loading body
/// (`vault_window`'s `VaultBodyState::Loading`), which is the screen this one
/// hands over to: a 28px [`theme::BLUE`] spinner over one line of 13px prose.
/// Matched rather than re-proportioned, because the two are seen seconds apart
/// in the same window and the second must not look like a different app's idea
/// of waiting. The mark that used to sit above the spinner is gone with it --
/// the heading already carries the wordmark, so drawing it a second time in the
/// middle of the screen was saying the app's name twice on a screen that has
/// one thing to say.
///
/// Returns what the heading asked for, which the host serves -- the two hosts
/// have different windows to serve it in. `close` is the host's answer to
/// whether its window may be closed AT ALL right now; see [`CloseControl`].
///
/// A function and not the inline body of [`show_while`] because there are two
/// places this look has to appear and they must not drift. `show_while` is one
/// -- a whole small window, its own `run_ui_native`, still used by the
/// cached-session launch and by `main`'s resettle. The other is the WORKING
/// STAGE of the single startup window (`app_window`), which is the same wait
/// with a window already around it: the user signs in and this replaces the
/// card in the SAME window rather than closing it and opening a second one,
/// which is the flicker the whole change exists to remove. Two hand-written
/// copies of "mark, spinner, label" would be two chances for the second window
/// the user is no longer supposed to notice to start looking like a different
/// app again.
///
/// Takes `&mut Ui` and paints only -- no channel, no viewport commands, no
/// window. That is also what makes it the one part of this feature a headless
/// `egui::Context` can actually render and read glyphs back from; everything
/// else here needs a live `eframe::Frame`, which no test can construct.
pub fn draw_spinner_body(ui: &mut egui::Ui, message: &str, close: CloseControl) -> ChromeAction {
    // `advance_cursor_after_rect` inside the chrome reads `item_spacing.y`
    // EAGERLY and bakes it into the cursor it leaves behind, so the gap between
    // the bar and the panel under it has to be zeroed before the call, not
    // after -- the same dance, for the same reason, as `vault_window`'s own
    // chrome call. Restored immediately, so this does not silently become the
    // ambient spacing of whatever the host draws next.
    let saved_item_spacing_y = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let action = draw_window_chrome_with_extra(ui, WINDOW_TITLE, HEADING, false, close, |_ui| {});
    ui.spacing_mut().item_spacing.y = saved_item_spacing_y;

    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(theme::CANVAS)
                .inner_margin(Margin::same(24)),
        )
        .show(ui, |ui| {
            // **Centred in the area BELOW THE HEADING**, rather than a fixed
            // offset from the top and no longer in the whole window either:
            // `available_height` here is what the panel was given, which the
            // chrome above has already taken its bar out of. This body is drawn
            // in two very differently sized windows -- the standalone spinner
            // window, small enough that a top offset once passed for centred,
            // and the single startup window, which is the vault window's full
            // size, where the same offset left everything huddled against the
            // top edge of an otherwise empty screen.
            let leftover = ui.available_height() - CONTENT_HEIGHT;
            ui.add_space((leftover / 2.0).max(0.0));
            ui.vertical_centered(|ui| {
                ui.add(egui::Spinner::new().size(SPINNER_SIZE).color(theme::BLUE));
                ui.add_space(SPINNER_TO_LABEL);
                ui.label(theme::semibold(message, LABEL_SIZE).color(theme::TEXT_SECONDARY));
            });
        });
    action
}

/// The heading's metrics: the VAULT window's, not the login window's.
///
/// The startup window's working stage IS the vault window a second later -- the
/// spinner is replaced by the vault in the same window -- so a 46px bar that
/// becomes a 46px bar is a heading that does not move when the vault arrives.
/// `show_while`'s own small window uses it too, for the reason this body is
/// shared at all: two hosts, one look.
const HEADING: ChromeMetrics = ChromeMetrics::VAULT;

/// Sized from the vault window's own loading body (28px spinner, 12px gap, 13px
/// label), which is the screen this one hands over to. The user asked for the
/// spinner to be bigger and the mark to go; this is the size the app already
/// uses for exactly this wait, rather than a third number invented here.
const SPINNER_SIZE: f32 = 28.0;
const SPINNER_TO_LABEL: f32 = 12.0;
const LABEL_SIZE: f32 = 13.0;

/// What [`draw_spinner_body`]'s stack occupies, used to centre it.
///
/// Summed from the pieces above rather than written as one number, so it
/// cannot drift from them -- a hand-written total is the kind of constant
/// that stays put while the thing it describes changes underneath it. The
/// label's line box is its font size times egui's default line height for
/// this face; being a pixel or two out is invisible in a centring, whereas
/// the top-anchored version this replaces was out by hundreds.
const CONTENT_HEIGHT: f32 = SPINNER_SIZE + SPINNER_TO_LABEL + LABEL_SIZE * 1.4;

/// This window GREW when the heading arrived: 320×150 had exactly enough room
/// for a mark, a 22px spinner and a line of text, and none at all for a 46px bar
/// above them -- at that size the stack overruns the panel's own bottom margin
/// and ends 19px off the window's edge, which is a cramped dialog rather than
/// the same screen the full-size window shows.
/// `the_standalone_window_has_room_for_the_heading_and_the_stack` is that
/// arithmetic asserted rather than eyeballed. Wider as well as taller, because
/// the bar has a minimum of its own: mark, wordmark and three 42px control zones
/// side by side.
const WINDOW_SIZE: [f32; 2] = [360.0, 220.0];

pub fn show_while<T: Send + 'static>(message: &str, rx: Receiver<T>) -> Option<T> {
    let result: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();
    let message = message.to_string();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(false)
            // Frameless, like every other window this app opens, because as of
            // the heading this body now draws there would otherwise be two
            // titlebars stacked on this one: the OS's and the app's.
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let mut styled = false;

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            // Frameless windows in this app ask DWM for the rounded corners and
            // shadow the OS frame would have given them. The OS window exists by
            // this first painted frame, which is the hook both this and the
            // raise below rely on.
            round_window_corners(WINDOW_TITLE);
            // This is where the window is brought to the front. See
            // `foreground`: a refusal from Windows flashes the taskbar button
            // rather than being ignored.
            crate::foreground::raise_window(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        if let Ok(value) = rx.try_recv() {
            *result_for_closure.borrow_mut() = Some(value);
        }

        // **This window's ✕ is live.** Nothing here holds a process the way the
        // startup window's working stage does; closing it returns `None` and
        // every caller already decides for itself what that means (see this
        // function's doc comment). With the frame gone, the chrome's ✕ and — are
        // the only mouse-operable way to close or minimise it.
        match draw_spinner_body(ui, &message, CloseControl::Active) {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            ChromeAction::Minimize => ui
                .ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }

        if result_for_closure.borrow().is_some() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            // No user input drives this window, so without an explicit
            // repaint request it would sit static between rx polls instead
            // of animating the spinner / noticing the channel has a value.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(80));
        }
    });

    let value = result.borrow_mut().take();
    value
}


/// The one part of this window a headless `egui::Context` can reach.
///
/// [`show_while`] itself cannot be tested: it blocks on a real winit event
/// loop and opens a real OS window. [`draw_spinner_body`] only paints into a
/// `&mut Ui`, so a `Context::run_ui` frame renders it for real -- real glyphs
/// off real galleys, real shapes -- which is what makes "the two hosts show the
/// same thing" a claim with something behind it rather than a comment.
#[cfg(test)]
mod spinner_body_tests {
    use super::*;
    use eframe::egui::{Color32, Rect};

    /// A context with this app's fonts actually installed. `theme::apply` takes
    /// effect at the START of the next frame, so the warm-up frames are not
    /// optional -- without them `theme::semibold` looks up a family that does
    /// not exist yet and the frame panics.
    fn styled_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(raw_input(), |_ui| {});
        ctx
    }

    /// The standalone spinner window's real size, so what these tests render is
    /// what that window renders.
    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(WINDOW_SIZE[0], WINDOW_SIZE[1]),
            )),
            ..Default::default()
        }
    }

    fn frame(message: &str) -> egui::FullOutput {
        let ctx = styled_ctx();
        ctx.run_ui(raw_input(), |ui| {
            draw_spinner_body(ui, message, CloseControl::Active);
        })
    }

    /// Every character actually RENDERED, glyph by glyph off the galleys --
    /// not `Galley::text()`, which answers with the source string and would
    /// therefore report a message that was laid out into zero rows.
    fn rendered(output: &egui::FullOutput) -> String {
        fn walk(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    for row in &text.galley.rows {
                        for glyph in &row.glyphs {
                            out.push(glyph.chr);
                        }
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn filled_rects(output: &egui::FullOutput) -> Vec<(Rect, Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(Rect, Color32)>) {
            match shape {
                egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn the_spinner_body_paints_the_message_it_was_given() {
        let painted = rendered(&frame("Setting up your vault..."));
        assert!(
            painted.contains("Setting up your vault..."),
            "the spinner drew no message, so the only thing on screen during a multi-second              wait is an unlabelled spinner: {painted:?}"
        );
        // Negative + positive control on the same renderer: a DIFFERENT
        // message really is absent, so the assertion above is about what was
        // passed in and not about a renderer that answers yes to anything.
        assert!(
            !painted.contains("Reconnecting"),
            "control: the glyph reader reports text that was never drawn: {painted:?}"
        );
        let other = rendered(&frame("Reconnecting"));
        assert!(
            other.contains("Reconnecting"),
            "control: the renderer cannot draw a second message at all: {other:?}"
        );
    }

    /// **The same window heading as the rest of the app.**
    ///
    /// The user's words: "keep the same window heading as the rest of the
    /// windows". This screen was the only one in the app that painted no chrome
    /// at all -- no title, no drag zone, no window controls -- and a glyph
    /// reader is exactly the instrument that says whether the heading's title is
    /// really being drawn, since the bar itself is just a filled rect that an
    /// empty titlebar would paint too.
    #[test]
    fn the_spinner_body_wears_the_window_heading() {
        let painted = rendered(&frame("Setting up your vault..."));
        assert!(
            painted.contains(WINDOW_TITLE),
            "the wait screen paints no window title, so it is the one screen in this app with \
             no heading -- and, being frameless, with nothing to drag or close it by: {painted:?}"
        );
        // Positive control: the same reader, on the same frame, finds the body's
        // own message -- so a missing title above is a missing title rather than
        // a frame that rendered no text at all.
        assert!(
            painted.contains("Setting up your vault..."),
            "control: nothing at all was rendered: {painted:?}"
        );
    }

    /// The flat fill is the half a glyph reader cannot see, and it is the half
    /// that makes the working stage read as the SAME window as the sign-in
    /// card behind it rather than as a bare grey dialog.
    #[test]
    fn the_spinner_body_fills_its_region_with_the_apps_canvas() {
        let output = frame("Setting up your vault...");
        let rects = filled_rects(&output);
        assert!(
            rects.iter().any(|(_, fill)| *fill == crate::theme::CANVAS),
            "nothing in the spinner is filled with `theme::CANVAS`, so the window behind the              spinner is whatever egui defaults to: {rects:?}"
        );
        // Positive control: the walker really found shapes, so the `any`
        // above is a search through something rather than through nothing.
        assert!(
            !rects.is_empty(),
            "control: no filled rectangles were painted at all"
        );
    }
}

#[cfg(test)]
mod spinner_centring_tests {
    use super::*;
    use eframe::egui::{Rect, pos2, vec2};

    fn styled_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(input(600.0), |_ui| {});
        crate::theme::apply(&ctx);
        let _ = ctx.run_ui(input(600.0), |_ui| {});
        ctx
    }

    fn input(height: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(840.0, height))),
            ..Default::default()
        }
    }

    /// The heading's height, written out rather than read from
    /// [`ChromeMetrics::VAULT`], so these tests state where they expect the
    /// stack to be instead of re-deriving it from the thing they are checking.
    /// If the design's bar height ever changes, the numbers below are meant to
    /// be re-decided, not to follow along quietly.
    const BAR: f32 = 46.0;

    /// The vertical span of everything painted BELOW THE HEADING -- text glyphs
    /// and the spinner's own shapes alike, so this measures the whole stack
    /// rather than whichever piece happens to be a rect.
    ///
    /// Two exclusions, and they are different:
    /// * anything starting at or above the bar's bottom edge is the heading
    ///   itself (its background, its hairline, its title, its ✕/▢/— glyphs),
    ///   which is not what "centred" is about here;
    /// * anything taller than 100px is a background fill -- the window body, the
    ///   canvas panel -- which would swamp the measurement. A height bound, not
    ///   a "smaller than the window" bound: the canvas panel in a SHORT window is
    ///   smaller than the window and still fills all of it.
    fn painted_span_below_the_heading(output: &egui::FullOutput) -> Option<(f32, f32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                other => {
                    let r = other.visual_bounding_rect();
                    if r.is_finite() && r.height() > 0.0 && r.height() < 100.0 && r.min.y >= BAR {
                        out.push(r);
                    }
                }
            }
        }
        let mut rects = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut rects);
        }
        let top = rects.iter().map(|r| r.min.y).fold(f32::INFINITY, f32::min);
        let bottom = rects.iter().map(|r| r.max.y).fold(f32::NEG_INFINITY, f32::max);
        (top.is_finite() && bottom.is_finite()).then_some((top, bottom))
    }

    fn spinner_frame(height: f32) -> egui::FullOutput {
        let ctx = styled_ctx();
        ctx.run_ui(input(height), |ui| {
            draw_spinner_body(ui, "Setting up your vault...", CloseControl::Active);
        })
    }

    /// **The stack sits in the middle of the area below the heading.**
    ///
    /// It used to start at a fixed 18px from the top, which looked centred in
    /// the small standalone spinner window and left everything huddled
    /// against the top edge of the full-size startup window -- which is what
    /// the user saw and called "bad looking". A test that only asserts the
    /// message is painted cannot tell the two apart; this asserts where.
    ///
    /// What "centred" means changed when the heading arrived: the middle of the
    /// window would sit the stack `BAR / 2` too high, with more empty space
    /// under it than over it. The expected centre is written out below rather
    /// than derived from `CONTENT_HEIGHT` or the metrics -- a test that computes
    /// its expectation from the constants under test agrees with itself no
    /// matter what they say.
    #[test]
    fn the_spinner_stack_is_centred_below_the_heading_in_a_tall_window() {
        let tall = 600.0;
        let output = spinner_frame(tall);

        let (top, bottom) = painted_span_below_the_heading(&output)
            .expect("the spinner body painted nothing below the heading at all");
        let centre = (top + bottom) / 2.0;

        // (46 + 600) / 2. Generous: this is about "middle" versus "pinned to an
        // edge", a difference of hundreds of pixels. The layout before the
        // centring fix put the stack's centre near 75px in a 600px window.
        assert!(
            (centre - 323.0).abs() < 60.0,
            "the spinner stack is centred at y={centre:.1} in a {tall:.0}px window, not near 323 \
             -- it is anchored to an edge rather than centred in the area below the heading"
        );
    }

    /// **No logo above the spinner.** The user: "no need for logo in the middle
    /// of the screen - just bigger spinner".
    ///
    /// Measured rather than asserted about the source, because "the mark is
    /// gone" is a claim about the screen. A 28px spinner over one 13px line is
    /// about 58px tall; the mark that used to sit above it was 32px with a 14px
    /// gap under it, so restoring it takes the stack past 100. The heading's own
    /// wordmark is excluded by `painted_span_below_the_heading`, which is the
    /// point -- the app's name is drawn once, up there.
    #[test]
    fn the_stack_is_a_spinner_and_a_line_of_text_and_not_a_logo_as_well() {
        let output = spinner_frame(600.0);
        let (top, bottom) = painted_span_below_the_heading(&output)
            .expect("the spinner body painted nothing below the heading at all");
        let height = bottom - top;
        assert!(
            height < 80.0,
            "the stack under the heading is {height:.1}px tall, which is more than a spinner and \
             one line of prose -- the mark is being drawn in the middle of the screen again"
        );
        // Positive control: it is not empty either, which is the other way a
        // height bound can be satisfied by accident.
        assert!(
            height > 20.0,
            "control: the stack under the heading is only {height:.1}px tall, so the bound above \
             is satisfied by nothing being drawn"
        );
    }

    /// **The standalone window is big enough for both.**
    ///
    /// The one test here that renders the REAL [`WINDOW_SIZE`] rather than a
    /// chosen height, because that constant is the decision this makes: the
    /// window it describes used to be 320×150, which fit a spinner and a line of
    /// text and nothing above them. 40px is "clearly more than the panel's own
    /// 24px margin"; at the old size the stack overruns that margin and ends
    /// 19px off the bottom edge, which is the stack wedged against it.
    #[test]
    fn the_standalone_window_has_room_for_the_heading_and_the_stack() {
        let height = WINDOW_SIZE[1];
        let output = spinner_frame(height);
        let (top, bottom) = painted_span_below_the_heading(&output)
            .expect("the spinner body painted nothing below the heading at all");

        let above = top - BAR;
        let below = height - bottom;
        assert!(
            above >= 40.0 && below >= 40.0,
            "in the {height:.0}px standalone window the stack has {above:.1}px of clearance \
             under the heading and {below:.1}px above the bottom edge -- the window is too \
             short for a heading and a spinner both, so the two are jammed together"
        );
    }

    /// The control for the centring test, and the arithmetic behind
    /// [`WINDOW_SIZE`]: in a SHORT window the same body still paints, still near
    /// the middle, and -- newly load-bearing -- still FITS between the heading
    /// and the bottom edge. The previous centring bug was invisible at small
    /// sizes and only showed at full size, so the short case stays; the fit is
    /// the opposite risk, and it is the one the heading introduced.
    #[test]
    fn the_stack_fits_below_the_heading_in_the_small_window() {
        let short = 180.0;
        let output = spinner_frame(short);

        let (top, bottom) = painted_span_below_the_heading(&output)
            .expect("the spinner body painted nothing below the heading at all");
        let centre = (top + bottom) / 2.0;
        // (46 + 180) / 2.
        assert!(
            (centre - 113.0).abs() < 30.0,
            "the spinner stack is centred at y={centre:.1} in a {short:.0}px window, not near 113"
        );
        // Deliberately not an assertion that `top >= BAR` -- the span filter
        // already drops everything above the bar, so that would be a test of the
        // filter. The bottom edge is the one nothing has already guaranteed.
        assert!(
            bottom <= short,
            "the stack ends at y={bottom:.1} in a {short:.0}px window -- the heading pushed it \
             off the bottom edge, which is what a window too short for both looks like"
        );
    }
}

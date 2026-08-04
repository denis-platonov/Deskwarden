//! A small spinner window shown while a background thread does slow,
//! non-interactive startup work (currently: waiting for `bw serve` to become
//! ready after login).
//!
//! Without this, the gap between the login window closing and the tray icon
//! appearing -- up to ~28s on a cold `bw serve` start -- showed nothing from
//! Deskwarden at all. Whatever else happened to be on screen (a terminal, in
//! more than one report) filled that silence, reading as "the app opened an
//! empty terminal" rather than "the app is still starting up".

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
/// either the user closed it via the title bar's X or Alt+F4 (this is a
/// normal decorated window; nothing about "loading" makes it modal or
/// un-closable), or the worker thread disconnected the channel without
/// sending (e.g. it panicked). Review 11's Critical: this used to
/// `.expect()` on exactly that case, which meant a user closing this spinner
/// while `pick_vault_item`'s populate ran panicked the main thread, and
/// `main.rs`'s panic hook only logs -- so the process unwound out of `main`
/// and the tray icon, hotkey, and autofill all vanished with it. Every
/// caller must now decide for itself what "the user closed this" means
/// (abandon quietly, treat as a failure, etc.) rather than that decision
/// being made for it by a crash.
/// The spinner itself: the mark, the spinner and one line of prose, centred on
/// a flat [`theme::CANVAS`] panel that fills whatever it is given.
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
pub fn draw_spinner_body(ui: &mut egui::Ui, message: &str) {
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(theme::CANVAS)
                .inner_margin(Margin::same(24)),
        )
        .show(ui, |ui| {
            // **Centred in whatever it is given**, rather than a fixed offset
            // from the top. This body is drawn in two very differently sized
            // windows: the standalone spinner window, which is small enough
            // that a top offset passed for centred, and the single startup
            // window, which is the vault window's full size -- where the same
            // offset left the mark and the message huddled against the top
            // edge of an otherwise empty screen.
            let leftover = ui.available_height() - CONTENT_HEIGHT;
            ui.add_space((leftover / 2.0).max(0.0));
            ui.vertical_centered(|ui| {
                theme::mark(ui, MARK_SIZE);
                ui.add_space(MARK_TO_SPINNER);
                ui.add(egui::Spinner::new().size(SPINNER_SIZE).color(theme::BLUE));
                ui.add_space(SPINNER_TO_LABEL);
                ui.label(theme::semibold(message, LABEL_SIZE).color(theme::TEXT_SECONDARY));
            });
        });
}

const MARK_SIZE: f32 = 32.0;
const MARK_TO_SPINNER: f32 = 14.0;
const SPINNER_SIZE: f32 = 22.0;
const SPINNER_TO_LABEL: f32 = 10.0;
const LABEL_SIZE: f32 = 13.0;

/// What [`draw_spinner_body`]'s stack occupies, used to centre it.
///
/// Summed from the pieces above rather than written as one number, so it
/// cannot drift from them -- a hand-written total is the kind of constant
/// that stays put while the thing it describes changes underneath it. The
/// label's line box is its font size times egui's default line height for
/// this face; being a pixel or two out is invisible in a centring, whereas
/// the top-anchored version this replaces was out by hundreds.
const CONTENT_HEIGHT: f32 =
    MARK_SIZE + MARK_TO_SPINNER + SPINNER_SIZE + SPINNER_TO_LABEL + LABEL_SIZE * 1.4;

pub fn show_while<T: Send + 'static>(message: &str, rx: Receiver<T>) -> Option<T> {
    let result: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();
    let message = message.to_string();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([320.0, 150.0])
            .with_resizable(false)
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
            // The OS window exists by this first painted frame (the same
            // hook `round_window_corners` uses), and this is where it is
            // brought to the front. See `foreground`: a refusal from Windows
            // flashes the taskbar button rather than being ignored.
            crate::foreground::raise_window(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        if let Ok(value) = rx.try_recv() {
            *result_for_closure.borrow_mut() = Some(value);
        }

        draw_spinner_body(ui, &message);

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

    fn raw_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(320.0, 150.0),
            )),
            ..Default::default()
        }
    }

    fn frame(message: &str) -> egui::FullOutput {
        let ctx = styled_ctx();
        ctx.run_ui(raw_input(), |ui| draw_spinner_body(ui, message))
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

    /// The vertical span of everything actually painted -- text glyphs and the
    /// spinner's own shapes alike, so this measures the whole stack rather
    /// than whichever piece happens to be a rect.
    fn painted_span(output: &egui::FullOutput) -> Option<(f32, f32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                // The panel's own full-height background fill would swamp the
                // measurement, so only marks smaller than the window count.
                other => {
                    let r = other.visual_bounding_rect();
                    if r.is_finite() && r.height() > 0.0 && r.height() < 400.0 {
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

    /// **The stack sits in the middle of whatever it is given.**
    ///
    /// It used to start at a fixed 18px from the top, which looked centred in
    /// the small standalone spinner window and left everything huddled
    /// against the top edge of the full-size startup window -- which is what
    /// the user saw and called "bad looking". A test that only asserts the
    /// message is painted cannot tell the two apart; this asserts where.
    #[test]
    fn the_spinner_stack_is_centred_in_a_tall_window() {
        let tall = 600.0;
        let ctx = styled_ctx();
        let output = ctx.run_ui(input(tall), |ui| draw_spinner_body(ui, "Setting up your vault..."));

        let (top, bottom) = painted_span(&output).expect("the spinner body painted nothing at all");
        let centre = (top + bottom) / 2.0;

        // Generous: this is about "middle of the window" versus "pinned to the
        // top", a difference of hundreds of pixels. The old layout put the
        // stack's centre near 75px in a 600px window.
        assert!(
            (centre - tall / 2.0).abs() < 60.0,
            "the spinner stack is centred at y={centre:.1} in a {tall:.0}px window, not near \
             {:.0} -- it is anchored to an edge rather than centred",
            tall / 2.0
        );
    }

    /// The control for the test above: in a SMALL window the same body still
    /// paints, and still near the middle. Without this, "centred" could be
    /// satisfied by a body that only ever draws at one fixed offset which
    /// happens to suit one height.
    #[test]
    fn the_same_body_is_still_centred_in_a_short_window() {
        let short = 180.0;
        let ctx = styled_ctx();
        let output =
            ctx.run_ui(input(short), |ui| draw_spinner_body(ui, "Setting up your vault..."));

        let (top, bottom) = painted_span(&output).expect("the spinner body painted nothing at all");
        let centre = (top + bottom) / 2.0;
        assert!(
            (centre - short / 2.0).abs() < 60.0,
            "the spinner stack is centred at y={centre:.1} in a {short:.0}px window, not near {:.0}",
            short / 2.0
        );
    }
}

//! Test harness for the kit: run one frame, flatten what it painted.

use eframe::egui::{self, epaint, Color32, Pos2, Rect, Shape, Vec2};

/// Everything one frame painted, flattened.
#[derive(Default)]
pub struct Painted {
    pub rects: Vec<epaint::RectShape>,
    pub circles: Vec<epaint::CircleShape>,
    /// (text, colour, rect on screen)
    pub texts: Vec<(String, Color32, Rect)>,
    /// The clip rect each of `texts` was painted under, in the same order:
    /// what of it a person can actually see is `rect.intersect(clip)`.
    pub text_clips: Vec<Rect>,
    pub repaint_delay: std::time::Duration,
    /// What the frame asked the window to do (drag, maximise, close...).
    pub commands: Vec<egui::ViewportCommand>,
}

impl Painted {
    pub fn text(&self, needle: &str) -> Option<&(String, Color32, Rect)> {
        self.texts.iter().find(|(t, _, _)| t.contains(needle))
    }
}

fn collect(shape: &Shape, clip: Rect, out: &mut Painted) {
    match shape {
        Shape::Rect(r) => out.rects.push(r.clone()),
        Shape::Circle(c) => out.circles.push(*c),
        Shape::Text(t) => {
            let color = t.override_text_color.unwrap_or_else(|| {
                t.galley.job.sections.first().map(|s| s.format.color).unwrap_or(t.fallback_color)
            });
            // What was DRAWN, glyph by glyph: an elided run's `text()` is
            // still the whole string, but its glyphs end in the ellipsis.
            let drawn: String = t.galley.rows.iter().flat_map(|row| row.glyphs.iter().map(|g| g.chr)).collect();
            out.texts.push((drawn, color, t.visual_bounding_rect()));
            out.text_clips.push(clip);
        }
        Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, clip, out)),
        _ => {}
    }
}

/// A context with the app's fonts live: [`crate::theme::apply`] registers
/// them, and egui binds them at the start of the next frame, so one empty
/// frame runs first.
pub fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
    ctx
}

/// Runs one frame at `time` (seconds) of a `size` screen, delivering
/// `events`, with `add` drawing into the central panel's `Ui`.
pub fn frame(
    ctx: &egui::Context,
    time: f64,
    size: Vec2,
    events: Vec<egui::Event>,
    add: impl FnMut(&mut egui::Ui),
) -> Painted {
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(time),
            events,
            ..Default::default()
        },
        add,
    );
    let mut painted = Painted {
        repaint_delay: output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
        commands: output.viewport_output[&egui::ViewportId::ROOT].commands.clone(),
        ..Default::default()
    };
    for clipped in &output.shapes {
        collect(&clipped.shape, clipped.clip_rect, &mut painted);
    }
    painted
}

/// A press and release of the primary button at `at`.
pub fn click_at(at: Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    vec![egui::Event::PointerMoved(at), button(true), button(false)]
}

/// The pointer resting at `at`.
pub fn hover_at(at: Pos2) -> Vec<egui::Event> {
    vec![egui::Event::PointerMoved(at)]
}

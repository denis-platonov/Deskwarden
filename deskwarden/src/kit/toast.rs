//! The design's toast: one short confirmation at a time, at the bottom of the
//! window, gone after two seconds.
//!
//! A screen calls [`show`]; the window calls [`paint`] once at the end of each
//! frame. A second toast replaces the first and starts its own two seconds.

use eframe::egui::{
    self, pos2, text::LayoutJob, Color32, CornerRadius, LayerId, Order, Rect, Shadow, Vec2,
};

use crate::kit::icon::{self, glyph};
use crate::kit::motion::{self, OVERSHOOT};
use crate::kit::tokens::{self, palette, Role};
use crate::theme::SEMIBOLD;

/// How long a toast stays, in seconds.
pub const LIFETIME: f64 = 2.0;
/// The widest a toast grows; longer messages end in an ellipsis.
pub const MAX_WIDTH: f32 = 560.0;

const HEIGHT: f32 = 50.0;
const FROM_BOTTOM: f32 = 28.0;
const PAD_LEFT: f32 = 16.0;
const PAD_RIGHT: f32 = 22.0;
const BADGE: f32 = 26.0;
const GAP: f32 = 10.0;
/// The entrance, in seconds.
const ENTRANCE: f64 = 0.28;

#[derive(Clone)]
struct Toast {
    message: String,
    shown_at: f64,
}

fn key() -> egui::Id {
    egui::Id::new("kit-toast")
}

/// Shows `message`, replacing any toast already up.
pub fn show(ctx: &egui::Context, message: impl Into<String>) {
    let shown_at = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(key(), Toast { message: message.into(), shown_at }));
    ctx.request_repaint();
}

/// The message showing now, if any.
pub fn current(ctx: &egui::Context) -> Option<String> {
    ctx.data(|d| d.get_temp::<Toast>(key())).map(|t| t.message)
}

/// Paints the toast, if one is up, and clears it once its time is over.
pub fn paint(ctx: &egui::Context) {
    let Some(toast) = ctx.data(|d| d.get_temp::<Toast>(key())) else {
        return;
    };
    let now = ctx.input(|i| i.time);
    let remaining = LIFETIME - (now - toast.shown_at);
    if remaining <= 0.0 {
        ctx.data_mut(|d| d.remove::<Toast>(key()));
        return;
    }
    // The entrance runs from when it was SHOWN, not from the first frame that
    // painted it: a window that paints a toast late must not replay it.
    let entered = ((now - toast.shown_at) / ENTRANCE) as f32;
    if entered < 1.0 {
        ctx.request_repaint();
    } else {
        // Settled: wake once more when it is due to leave.
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(remaining));
    }
    let pose = motion::toast(OVERSHOOT.at(entered).min(1.0));
    let opacity = pose.opacity.clamp(0.0, 1.0);

    let p = palette(ctx);
    let white = Color32::WHITE.gamma_multiply(opacity);
    let role = Role { size: 15.0, face: SEMIBOLD, tracking_em: 0.0 };
    let mut job: LayoutJob = tokens::job(role, &toast.message, white);
    job.wrap.max_width = MAX_WIDTH - PAD_LEFT - BADGE - GAP - PAD_RIGHT;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = ctx.fonts_mut(|f| f.layout_job(job));

    let width = PAD_LEFT + BADGE + GAP + galley.size().x + PAD_RIGHT;
    let screen = ctx.content_rect();
    let rect = Rect::from_min_size(
        pos2(screen.center().x - width / 2.0, screen.bottom() - FROM_BOTTOM - HEIGHT + pose.dy),
        Vec2::new(width, HEIGHT),
    );
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, key()));
    let shadow = Shadow {
        offset: [0, 14],
        blur: 34,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, 64).gamma_multiply(opacity),
    };
    painter.add(shadow.as_shape(rect, CornerRadius::same(255)));
    painter.rect_filled(rect, CornerRadius::same(255), p.ink.gamma_multiply(opacity));
    let badge = pos2(rect.left() + PAD_LEFT + BADGE / 2.0, rect.center().y);
    painter.circle_filled(badge, BADGE / 2.0, p.done_mark.gamma_multiply(opacity));
    icon::stroke(&painter, Rect::from_center_size(badge, Vec2::splat(15.0)), glyph::CHECK, 3.0, white);
    let text_at = pos2(rect.left() + PAD_LEFT + BADGE + GAP, rect.center().y - galley.size().y / 2.0);
    painter.galley(text_at, galley, white);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{ctx, frame};
    use crate::kit::tokens::Palette;
    use eframe::egui::Vec2;

    const SCREEN: Vec2 = Vec2::new(800.0, 600.0);

    #[test]
    fn a_toast_shows_then_leaves_after_two_seconds() {
        let ctx = ctx();
        frame(&ctx, 0.0, SCREEN, vec![], |ui| {
            show(ui.ctx(), "Link copied");
            paint(ui.ctx());
        });
        let during = frame(&ctx, 1.0, SCREEN, vec![], |ui| paint(ui.ctx()));
        assert!(during.text("Link copied").is_some());
        assert_eq!(current(&ctx).as_deref(), Some("Link copied"));
        let after = frame(&ctx, 2.01, SCREEN, vec![], |ui| paint(ui.ctx()));
        assert!(after.text("Link copied").is_none());
        assert_eq!(current(&ctx), None);
        let quiet = frame(&ctx, 2.5, SCREEN, vec![], |ui| paint(ui.ctx()));
        assert_ne!(quiet.repaint_delay, std::time::Duration::ZERO, "a gone toast must not keep the window drawing");
    }

    #[test]
    fn a_new_toast_replaces_the_old_and_restarts_the_clock() {
        let ctx = ctx();
        frame(&ctx, 0.0, SCREEN, vec![], |ui| show(ui.ctx(), "A"));
        frame(&ctx, 1.5, SCREEN, vec![], |ui| show(ui.ctx(), "B"));
        let at_three = frame(&ctx, 3.0, SCREEN, vec![], |ui| paint(ui.ctx()));
        assert_eq!(current(&ctx).as_deref(), Some("B"));
        assert!(at_three.texts.iter().all(|(t, _, _)| t != "A"));
        frame(&ctx, 3.6, SCREEN, vec![], |ui| paint(ui.ctx()));
        assert_eq!(current(&ctx), None);
    }

    #[test]
    fn the_toast_sits_twenty_eight_above_the_bottom_and_is_fifty_tall() {
        let ctx = ctx();
        frame(&ctx, 0.0, SCREEN, vec![], |ui| show(ui.ctx(), "Saved"));
        let painted = frame(&ctx, 1.0, SCREEN, vec![], |ui| paint(ui.ctx()));
        let pill = painted
            .rects
            .iter()
            .find(|r| r.fill == Palette::LIGHT.ink)
            .expect("the toast's pill");
        assert!((pill.rect.bottom() - 572.0).abs() < 0.5, "{:?}", pill.rect);
        assert_eq!(pill.rect.height(), 50.0);
        assert!((pill.rect.center().x - 400.0).abs() < 0.5);
        assert!(painted.circles.iter().any(|c| c.fill == Palette::LIGHT.done_mark && c.radius == 13.0));
    }

    #[test]
    fn a_long_toast_is_ellipsised_inside_the_cap() {
        let ctx = ctx();
        let long = "x".repeat(400);
        frame(&ctx, 0.0, SCREEN, vec![], |ui| show(ui.ctx(), long.clone()));
        let painted = frame(&ctx, 1.0, SCREEN, vec![], |ui| paint(ui.ctx()));
        let pill = painted.rects.iter().find(|r| r.fill == Palette::LIGHT.ink).unwrap();
        assert!(pill.rect.width() <= MAX_WIDTH, "{}", pill.rect.width());
        let (text, _, _) = painted.texts.iter().find(|(t, _, _)| t.starts_with('x')).unwrap();
        assert!(text.ends_with('…'), "{text}");
    }
}

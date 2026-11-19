//! The design's two kinds of help: the `?` dot whose bubble explains a row,
//! and the hover hint that names a control and its shortcut.
//!
//! Both paint on egui's tooltip layer so nothing in a pane can cover them,
//! and both fade in on their own short clock, restarted each time the pointer
//! arrives.

use eframe::egui::{
    self, pos2, Color32, CornerRadius, LayerId, Order, Rect, Response, Sense, Shadow, Ui, Vec2,
};

use crate::kit::motion::{self, EASE_OUT};
use crate::kit::tokens::{self, palette, Role, HINT, TOOLTIP};
use crate::theme::{BOLD, EXTRABOLD};

/// How close to the window's top a control can be before its hint drops
/// below it instead (the design's top bar).
const FLIP_BELOW: f32 = 56.0;
/// How far a hint's centre stays from the window's sides.
const SIDE_CLAMP: f32 = 90.0;
/// The gap between a control and its hint.
const HINT_GAP: f32 = 8.0;

/// What a key is called on this platform: the design's `Mod` is Ctrl on
/// Windows.
pub fn key_label(key: &str) -> &str {
    match key {
        "Mod" => "Ctrl",
        other => other,
    }
}

/// Where a `bubble`-sized hint for `target` goes in `window`: centred 8 above
/// it, or 8 below it when it sits in the top bar; never nearer a side than 90;
/// and always wholly inside the window.
pub fn hint_rect(target: Rect, window: Rect, bubble: Vec2) -> Rect {
    let below = target.top() - window.top() < FLIP_BELOW;
    let top = if below { target.bottom() + HINT_GAP } else { target.top() - HINT_GAP - bubble.y };
    let cx = target.center().x.clamp(window.left() + SIDE_CLAMP, window.right() - SIDE_CLAMP);
    inside(Rect::from_min_size(pos2(cx - bubble.x / 2.0, top), bubble), window)
}

fn tooltip_painter(ui: &Ui, salt: &str) -> egui::Painter {
    ui.ctx().layer_painter(LayerId::new(Order::Tooltip, egui::Id::new(("kit-tips", salt))))
}

/// Which side of its dot a help bubble opens on. The design opens most above;
/// the Shared header, the QR modal and the generator's look-alike dot open
/// below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Above,
    Below,
}

/// Moves `rect` the least distance that puts it inside `window`.
fn inside(rect: Rect, window: Rect) -> Rect {
    let dx = (window.left() - rect.left()).max(0.0) + (window.right() - rect.right()).min(0.0);
    let dy = (window.top() - rect.top()).max(0.0) + (window.bottom() - rect.bottom()).min(0.0);
    rect.translate(Vec2::new(dx, dy))
}

/// The 18 px `?` dot. Hovering it shows `text` in a `width`-wide bubble on
/// the `placement` side.
pub fn help_dot(ui: &mut Ui, id: egui::Id, text: &str, width: f32, placement: Placement) -> Response {
    help_dot_sized(ui, id, text, width, placement, 18.0)
}

/// [`help_dot`] at another diameter: the QR modal's is 20 with a 12 px `?`.
pub fn help_dot_sized(ui: &mut Ui, id: egui::Id, text: &str, width: f32, placement: Placement, diameter: f32) -> Response {
    let p = palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    ui.painter().circle_filled(rect.center(), diameter / 2.0, p.badge_bg);
    let glyph = ui.fonts_mut(|f| {
        f.layout_job(tokens::job(Role { size: if diameter > 18.0 { 12.0 } else { 11.0 }, face: EXTRABOLD, tracking_em: 0.0 }, "?", p.text_icon))
    });
    ui.painter().galley(rect.center() - glyph.size() / 2.0, glyph, p.text_icon);
    let clock = id.with("help-bubble");
    if response.hovered() {
        let shown = motion::progress(ui.ctx(), clock, 0.0, 0.16, EASE_OUT);
        paint_help_bubble_at(ui, rect, text, width, placement, motion::tip(shown));
    } else {
        motion::restart(ui.ctx(), clock);
    }
    response.on_hover_cursor(egui::CursorIcon::Help)
}

/// The help bubble for a dot at `anchor`, fully shown.
pub fn paint_help_bubble(ui: &Ui, anchor: Rect, text: &str, width: f32, placement: Placement) {
    paint_help_bubble_at(ui, anchor, text, width, placement, motion::Pose::REST);
}

fn paint_help_bubble_at(
    ui: &Ui,
    anchor: Rect,
    text: &str,
    width: f32,
    placement: Placement,
    pose: motion::Pose,
) {
    let white = Color32::WHITE.gamma_multiply(pose.opacity);
    let mut job = tokens::job(TOOLTIP, text, white);
    job.sections[0].format.line_height = Some(TOOLTIP.size * 1.45);
    job.wrap.max_width = width - 26.0;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let height = galley.size().y + 22.0;
    let top = match placement {
        Placement::Above => anchor.top() - 10.0 - height,
        Placement::Below => anchor.bottom() + 10.0,
    };
    // Kept inside the window whatever the dot's position: a dot in the top
    // bar or by the right edge must not lose half its explanation.
    let placed = inside(
        Rect::from_min_size(pos2(anchor.left() - 12.0, top), Vec2::new(width, height)),
        ui.ctx().content_rect(),
    );
    let rect = placed.translate(Vec2::new(pose.dx, pose.dy));
    let p = palette(ui.ctx());
    let painter = tooltip_painter(ui, "help");
    let shadow = Shadow { offset: [0, 12], blur: 28, spread: 0, color: Color32::from_rgba_unmultiplied(0, 0, 0, 56).gamma_multiply(pose.opacity) };
    painter.add(shadow.as_shape(rect, CornerRadius::same(12)));
    painter.rect_filled(rect, CornerRadius::same(12), p.ink.gamma_multiply(pose.opacity));
    painter.galley(rect.min + Vec2::new(13.0, 11.0), galley, white);
}

/// Shows the design's hover hint for `response` while it is hovered and no
/// button is down: `label`, then each of `keys` as a chip.
pub fn hint(ui: &Ui, response: &Response, label: &str, keys: &[&str]) {
    let clock = response.id.with("hover-hint");
    let pressing = ui.ctx().input(|i| i.pointer.any_down());
    if response.hovered() && !pressing {
        let shown = motion::progress(ui.ctx(), clock, 0.0, 0.14, EASE_OUT);
        paint_hint_at(ui, response.rect, label, keys, shown);
    } else {
        motion::restart(ui.ctx(), clock);
    }
}

/// The hint for a control at `target`, fully shown.
pub fn paint_hint(ui: &Ui, target: Rect, label: &str, keys: &[&str]) {
    paint_hint_at(ui, target, label, keys, 1.0);
}

fn paint_hint_at(ui: &Ui, target: Rect, label: &str, keys: &[&str], opacity: f32) {
    let p = palette(ui.ctx());
    let white = Color32::WHITE.gamma_multiply(opacity);
    let text = ui.fonts_mut(|f| f.layout_job(tokens::job(HINT, label, white)));
    let chip_role = Role { size: 11.0, face: BOLD, tracking_em: 0.0 };
    let chips: Vec<_> = keys
        .iter()
        .map(|k| ui.fonts_mut(|f| f.layout_job(tokens::job(chip_role, key_label(k), white))))
        .collect();
    let chip_w = |g: &std::sync::Arc<egui::Galley>| (g.size().x + 10.0).max(20.0);
    let inner_h = text.size().y.max(20.0);
    // Spec 2.7: 8 between the label and the chips, 3 between chips.
    let chips_w: f32 = chips.iter().map(chip_w).sum::<f32>() + 3.0 * chips.len().saturating_sub(1) as f32;
    let width = 11.0 + text.size().x + if chips.is_empty() { 0.0 } else { 8.0 + chips_w } + 8.0;
    let rect = hint_rect(target, ui.ctx().content_rect(), Vec2::new(width, inner_h + 14.0));
    let painter = tooltip_painter(ui, "hint");
    let shadow = Shadow { offset: [0, 8], blur: 20, spread: 0, color: Color32::from_rgba_unmultiplied(0, 0, 0, 64).gamma_multiply(opacity) };
    painter.add(shadow.as_shape(rect, CornerRadius::same(10)));
    painter.rect_filled(rect, CornerRadius::same(10), p.ink.gamma_multiply(opacity));
    let mid = rect.center().y;
    let mut x = rect.left() + 11.0;
    painter.galley(pos2(x, mid - text.size().y / 2.0), text.clone(), white);
    x += text.size().x;
    let chip_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 41).gamma_multiply(opacity);
    for (i, galley) in chips.into_iter().enumerate() {
        x += if i == 0 { 8.0 } else { 3.0 };
        let w = chip_w(&galley);
        let chip = Rect::from_min_size(pos2(x, mid - 10.0), Vec2::new(w, 20.0));
        painter.rect_filled(chip, CornerRadius::same(5), chip_fill);
        painter.galley(chip.center() - galley.size() / 2.0, galley, white);
        x += w;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{ctx, frame, hover_at};
    use crate::kit::tokens::Palette;
    use eframe::egui::{self, pos2, Rect, Vec2};

    const WINDOW: Rect = Rect { min: pos2(0.0, 0.0), max: pos2(1280.0, 820.0) };
    const P: Palette = Palette::LIGHT;

    fn target(x: f32, y: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), Vec2::new(40.0, 40.0))
    }

    #[test]
    fn hints_sit_above_their_control() {
        let r = hint_rect(target(600.0, 400.0), WINDOW, Vec2::new(120.0, 34.0));
        assert_eq!(r.bottom(), 392.0);
        assert_eq!(r.center().x, 620.0);
    }

    #[test]
    fn hints_in_the_top_bar_drop_below() {
        let t = target(600.0, 20.0);
        let r = hint_rect(t, WINDOW, Vec2::new(120.0, 34.0));
        assert_eq!(r.top(), t.bottom() + 8.0);
    }

    #[test]
    fn hints_near_a_side_are_pulled_in_by_ninety() {
        let t = Rect::from_center_size(pos2(10.0, 400.0), Vec2::splat(20.0));
        let r = hint_rect(t, WINDOW, Vec2::new(120.0, 34.0));
        assert_eq!(r.center().x, 90.0);
        let t = Rect::from_center_size(pos2(1275.0, 400.0), Vec2::splat(20.0));
        assert_eq!(hint_rect(t, WINDOW, Vec2::new(120.0, 34.0)).center().x, 1190.0);
    }

    #[test]
    fn a_hint_never_leaves_the_window() {
        let bubble = Vec2::new(300.0, 34.0);
        for t in [target(0.0, 0.0), target(1240.0, 0.0), target(0.0, 780.0), target(1240.0, 780.0)] {
            let r = hint_rect(t, WINDOW, bubble);
            assert!(WINDOW.contains_rect(r), "{t:?} put its hint at {r:?}");
        }
    }

    #[test]
    fn mod_reads_ctrl_on_windows() {
        assert_eq!(key_label("Mod"), "Ctrl");
        assert_eq!(key_label("Shift"), "Shift");
        assert_eq!(key_label("K"), "K");
    }

    const TIP: &str = "Autofill only works while Deskwarden is running.";

    #[test]
    fn a_help_bubble_is_hidden_until_hovered_and_shows_its_text() {
        let ctx = ctx();
        let id = egui::Id::new("tip-test");
        let size = Vec2::new(800.0, 600.0);
        let mut dot = Rect::NOTHING;
        let draw = |ui: &mut egui::Ui, dot: &mut Rect| {
            ui.add_space(200.0);
            // In from the left edge, so the bubble's -12 offset stays on screen.
            ui.horizontal(|ui| {
                ui.add_space(100.0);
                *dot = help_dot(ui, id, TIP, 260.0, Placement::Above).rect;
            });
        };
        let quiet = frame(&ctx, 0.0, size, vec![], |ui| draw(ui, &mut dot));
        assert!(quiet.text("Autofill only").is_none());
        assert_eq!(dot.size(), Vec2::splat(18.0));
        let mut shown = None;
        for time in [0.1, 0.5] {
            shown = Some(frame(&ctx, time, size, hover_at(dot.center()), |ui| draw(ui, &mut dot)));
        }
        let shown = shown.unwrap();
        let (_, color, at) = shown.text("Autofill only").expect("the bubble's text");
        assert_eq!(*color, egui::Color32::WHITE);
        let bubble = shown
            .rects
            .iter()
            .find(|r| r.fill == P.ink && r.rect.contains_rect(*at))
            .expect("an ink bubble around the text");
        assert_eq!(bubble.rect.width(), 260.0);
        assert!((bubble.rect.left() - (dot.left() - 12.0)).abs() < 0.5);
        assert!(bubble.rect.bottom() <= dot.top() - 10.0 + 0.5);
    }

    fn bubble_rect(at: Rect, placement: Placement) -> Rect {
        let ctx = ctx();
        let painted = frame(&ctx, 0.0, Vec2::new(800.0, 600.0), vec![], |ui| {
            paint_help_bubble(ui, at, TIP, 260.0, placement);
        });
        painted.rects.iter().find(|r| r.fill == P.ink).expect("a bubble").rect
    }

    #[test]
    fn a_help_bubble_below_opens_under_its_dot() {
        let dot = Rect::from_min_size(pos2(300.0, 300.0), Vec2::splat(18.0));
        let r = bubble_rect(dot, Placement::Below);
        assert_eq!(r.top(), dot.bottom() + 10.0);
        assert_eq!(r.left(), dot.left() - 12.0);
    }

    #[test]
    fn a_help_bubble_never_leaves_the_window() {
        let window = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0));
        for (dot, placement) in [
            (Rect::from_min_size(pos2(780.0, 4.0), Vec2::splat(18.0)), Placement::Above),
            (Rect::from_min_size(pos2(2.0, 580.0), Vec2::splat(18.0)), Placement::Below),
        ] {
            let r = bubble_rect(dot, placement);
            assert!(window.contains_rect(r), "{dot:?} {placement:?} put its bubble at {r:?}");
        }
    }

    /// Spec 2.7: chips sit 3 apart; 8 separates the label from the first.
    #[test]
    fn hint_chips_are_three_apart() {
        let ctx = ctx();
        let painted = frame(&ctx, 1.0, Vec2::new(800.0, 600.0), vec![], |ui| {
            paint_hint(ui, target(400.0, 300.0), "Sync now", &["Mod", "Shift", "Y"]);
        });
        let chip_fill = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 41);
        let mut chips: Vec<Rect> = painted.rects.iter().filter(|r| r.fill == chip_fill).map(|r| r.rect).collect();
        chips.sort_by(|a, b| a.left().total_cmp(&b.left()));
        assert_eq!(chips.len(), 3);
        for pair in chips.windows(2) {
            assert!((pair[1].left() - pair[0].right() - 3.0).abs() < 0.01, "{pair:?}");
        }
        let label = painted.text("Sync now").unwrap().2;
        assert!((chips[0].left() - label.right() - 8.0).abs() < 1.0, "label {label:?} chip {:?}", chips[0]);
    }

    #[test]
    fn a_painted_hint_shows_its_keys_as_chips() {
        let ctx = ctx();
        let painted = frame(&ctx, 1.0, Vec2::new(800.0, 600.0), vec![], |ui| {
            paint_hint(ui, target(400.0, 300.0), "Quick search", &["Mod", "K"]);
        });
        assert!(painted.text("Quick search").is_some());
        assert!(painted.text("Ctrl").is_some());
        assert!(painted.text("K").is_some());
        assert!(painted.rects.iter().any(|r| r.fill == P.ink));
    }
}

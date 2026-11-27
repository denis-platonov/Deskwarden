//! The launch splash (design 1a, spec part A section 2.4): the navy ground,
//! the breathing shield, "Opening your vault" and a sliding bar, fading out
//! over .45 s once the vault is ready.
//!
//! **Painted, not laid out**: it goes on a layer above the window and claims
//! no input, so the window's real caption buttons underneath still close and
//! minimise it while it opens. The splash draws the design's light-blue
//! caption glyphs over them.

use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, LayerId, Order, Rect, Stroke};

use crate::kit::motion;
use crate::kit::tokens::{self, palette, Role};
use crate::theme;

/// The line under the shield (design 1a, verbatim).
pub fn opening() -> &'static str {
    crate::i18n::t("splash.opening")
}
/// The same line when the vault is opening from the saved copy on disk.
pub fn opening_offline() -> &'static str {
    crate::i18n::t("splash.opening_offline")
}
/// How long the splash takes to fade once the vault is ready.
pub const FADE: f32 = 0.45;
/// The progress track (design 1a: 220 by 4).
const TRACK: egui::Vec2 = egui::vec2(220.0, 4.0);
/// The shield (design 1a: 84 by 98).
const SHIELD: egui::Vec2 = egui::vec2(84.0, 98.0);

/// The reversed shield (white, light blue, soft blue, bright blue): it reads
/// on the navy ground and on the dark theme's near-black one alike, so it is
/// the same four fills in both.
const MARK_REVERSED: [Color32; 4] = [
    Color32::WHITE,
    tokens::Palette::LIGHT.blue_edge,
    tokens::Palette::LIGHT.blue_soft,
    tokens::Palette::LIGHT.blue_bright,
];

/// What the splash paints with. Light: the design's navy ground with white
/// text. Dark theme: the dark window ground with the palette's own ink, so
/// the launch does not flash a blue sheet at someone in a dark room.
struct Look {
    ground: Color32,
    caption: Color32,
    text: Color32,
    track: Color32,
}

impl Look {
    fn of(p: &tokens::Palette) -> Self {
        if is_dark(p) {
            Look { ground: p.window_bg, caption: p.text_icon, text: p.ink, track: p.meter_track }
        } else {
            Look {
                ground: p.blue_deep,
                caption: p.blue_edge,
                text: Color32::WHITE,
                track: Color32::from_rgba_unmultiplied(255, 255, 255, 41),
            }
        }
    }
}

/// Whether `p` is a dark palette (its window ground is closer to black).
fn is_dark(p: &tokens::Palette) -> bool {
    let g = p.window_bg;
    u16::from(g.r()) + u16::from(g.g()) + u16::from(g.b()) < 3 * 128
}

fn layer() -> LayerId {
    LayerId::new(Order::Middle, egui::Id::new("vault-splash"))
}

/// The splash over `rect`, `fade` of the way gone (0 is fully shown).
pub fn paint(ctx: &egui::Context, rect: Rect, offline: bool, fade: f32) {
    paint_line(ctx, rect, if offline { opening_offline() } else { opening() }, fade);
}

/// [`paint`], saying `line` under the shield: the startup window's waits
/// (opening after sign-in, locking) are this same screen with their own words.
pub fn paint_line(ctx: &egui::Context, rect: Rect, line: &str, fade: f32) {
    let p = palette(ctx);
    let shown = (1.0 - fade).clamp(0.0, 1.0);
    let t = ctx.input(|i| i.time);
    let painter = ctx.layer_painter(layer());
    let look = Look::of(&p);
    painter.rect_filled(rect, CornerRadius::ZERO, look.ground.gamma_multiply(shown));

    // The design's caption glyphs, light blue on navy (the window's own
    // buttons sit underneath and still take the clicks).
    let ink = look.caption.gamma_multiply(shown);
    let cell = |i: f32| Rect::from_min_size(pos2(rect.right() - 46.0 * (3.0 - i), rect.top()), vec2(46.0, 34.0));
    let stroke = Stroke::new(1.0, ink);
    let c = cell(0.0).center();
    painter.line_segment([pos2(c.x - 5.0, c.y), pos2(c.x + 5.0, c.y)], stroke);
    painter.rect_stroke(
        Rect::from_center_size(cell(1.0).center(), egui::Vec2::splat(10.0)),
        CornerRadius::ZERO,
        Stroke::new(1.0, ink.gamma_multiply(0.4)),
        egui::StrokeKind::Inside,
    );
    let c = cell(2.0).center();
    painter.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], stroke);
    painter.line_segment([c + vec2(5.0, -5.0), c + vec2(-5.0, 5.0)], stroke);

    // The column: shield, then 28 below it the line and, 18 under that, the
    // track -- centred as a block.
    let role = Role { size: 24.0, face: theme::EXTRABOLD, tracking_em: -0.01 };
    let white = look.text.gamma_multiply(shown);
    let line = ctx.fonts_mut(|f| f.layout_job(tokens::job(role, line, white)));
    let block = SHIELD.y + 28.0 + line.size().y + 18.0 + TRACK.y;
    let top = rect.center().y - block / 2.0;
    let shield = Rect::from_center_size(
        pos2(rect.center().x, top + SHIELD.y / 2.0),
        SHIELD * motion::breathe(t),
    );
    let fills = MARK_REVERSED.map(|c| c.gamma_multiply(shown));
    theme::paint_mark_colours(&painter, shield, fills);
    let line_top = top + SHIELD.y + 28.0;
    painter.galley(pos2(rect.center().x - line.size().x / 2.0, line_top), line.clone(), white);

    let track = Rect::from_min_size(
        pos2(rect.center().x - TRACK.x / 2.0, line_top + line.size().y + 18.0),
        TRACK,
    );
    painter.rect_filled(
        track,
        CornerRadius::same(2),
        look.track.gamma_multiply(shown),
    );
    // A bar 34% of the track, sliding from -110% to 330% of its own width;
    // cut to the track so it never paints outside it.
    let bar_width = TRACK.x * 0.34;
    let bar = Rect::from_min_size(pos2(track.left() + motion::line(t) * bar_width, track.top()), vec2(bar_width, TRACK.y))
        .intersect(track);
    if bar.width() > 0.0 {
        painter.rect_filled(bar, CornerRadius::same(2), white);
    }
}

/// What the window's splash is doing between frames.
#[derive(Clone, Copy, Default)]
struct State {
    was_loading: bool,
    ended_at: Option<f64>,
}

/// The splash, once per frame: shown while `loading`, then fading over
/// [`FADE`] from the frame the vault became ready, then gone -- and asking for
/// frames only while it is on screen. A window that was never loading shows
/// none.
pub fn host(ctx: &egui::Context, rect: Rect, loading: bool, offline: bool) {
    let id = egui::Id::new("vault-splash-state");
    let now = ctx.input(|i| i.time);
    let mut state = ctx.data(|d| d.get_temp::<State>(id)).unwrap_or_default();
    if loading {
        state = State { was_loading: true, ended_at: None };
        ctx.data_mut(|d| d.insert_temp(id, state));
        paint(ctx, rect, offline, 0.0);
        // The shield breathes and the bar slides for as long as it loads.
        ctx.request_repaint();
        return;
    }
    if !state.was_loading {
        return;
    }
    let ended = *state.ended_at.get_or_insert(now);
    ctx.data_mut(|d| d.insert_temp(id, state));
    let gone = ((now - ended) as f32 / FADE).clamp(0.0, 1.0);
    if gone < 1.0 {
        paint(ctx, rect, offline, motion::EASE_OUT.at(gone));
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, Painted};
    use crate::kit::tokens::Palette;
    use eframe::egui::{self, pos2, Rect};

    const P: Palette = Palette::LIGHT;
    const SIZE: egui::Vec2 = egui::vec2(1280.0, 820.0);
    const WINDOW: Rect = Rect { min: pos2(0.0, 0.0), max: pos2(1280.0, 820.0) };

    fn painted(time: f64, offline: bool, fade: f32) -> Painted {
        let ctx = testing::ctx();
        testing::frame(&ctx, time, SIZE, vec![], |ui| paint(ui.ctx(), WINDOW, offline, fade))
    }

    #[test]
    fn the_splash_is_navy_with_the_reversed_shield_and_its_line() {
        let p = painted(0.3, false, 0.0);
        assert!(p.rects.iter().any(|r| r.fill == P.blue_deep && r.rect == WINDOW), "no navy ground");
        let (_, ink, _) = p.text(opening()).expect("the line");
        assert_eq!(*ink, egui::Color32::WHITE);
        assert!(p.text(opening_offline()).is_none());
    }

    #[test]
    fn offline_opens_the_saved_copy() {
        let p = painted(0.3, true, 0.0);
        assert!(p.text(opening_offline()).is_some());
        assert!(p.text(opening()).is_none() || opening_offline().contains(opening()));
    }

    #[test]
    fn the_bar_stays_inside_its_track() {
        for step in 0..=12 {
            let p = painted(step as f64 * 0.1, false, 0.0);
            let white = p.rects.iter().find(|r| r.fill == egui::Color32::WHITE).map(|r| r.rect);
            let track = p
                .rects
                .iter()
                .find(|r| (r.rect.width() - TRACK.x).abs() < 0.01 && (r.rect.height() - TRACK.y).abs() < 0.01)
                .expect("the track")
                .rect;
            if let Some(bar) = white {
                assert!(track.expand(0.01).contains_rect(bar), "t={step}: {bar:?} outside {track:?}");
            }
        }
    }

    #[test]
    fn a_faded_splash_paints_nothing_opaque() {
        let p = painted(0.3, false, 1.0);
        assert!(p.rects.iter().all(|r| r.fill.a() == 0), "{:?}", p.rects.iter().map(|r| r.fill).collect::<Vec<_>>());
        assert!(p.texts.iter().all(|(_, c, _)| c.a() == 0));
    }

    #[test]
    fn the_host_paints_while_loading_then_fades_and_goes_quiet() {
        let ctx = testing::ctx();
        let frame = |time: f64, loading: bool| {
            testing::frame(&ctx, time, SIZE, vec![], |ui| host(ui.ctx(), WINDOW, loading, false))
        };
        let loading = frame(0.0, true);
        assert!(loading.text(opening()).is_some());
        // The fade starts on the first frame the vault is ready...
        frame(1.0, false);
        // ...and part-way through it the splash is part-way gone.
        let fading = frame(1.2, false);
        let (_, ink, _) = fading.text(opening()).expect("still fading out");
        assert!(ink.a() > 0 && ink.a() < 255, "{ink:?}");
        let gone = frame(1.0 + FADE as f64 + 0.05, false);
        assert!(gone.text(opening()).is_none(), "the splash outlived its fade");
        let quiet = frame(2.0, false);
        assert_ne!(quiet.repaint_delay, std::time::Duration::ZERO, "a finished splash keeps the window drawing");
    }

    #[test]
    fn a_vault_that_never_loaded_shows_no_splash() {
        let ctx = testing::ctx();
        let p = testing::frame(&ctx, 0.0, SIZE, vec![], |ui| host(ui.ctx(), WINDOW, false, false));
        assert!(p.text(opening()).is_none());
    }
}

//! Slice 8b: the Security page's score card (spec C 2.6, decision D4).
//!
//! The score is 100 minus a weight for every weak, reused, old or breached
//! item the Password health report lists, floored at 0. "Fix N items" is the
//! report's own count of distinct flagged items, so the card, the rail's badge
//! and the Password health screen can never disagree about how much there is
//! to do. The window hands the report's numbers over once a frame
//! ([`offer`]); the card reads them back ([`offered`]) and never holds the
//! report, which keeps this screen free of anything that came from a password.

use eframe::egui::{self, vec2, Color32, CornerRadius, Margin, Pos2, Sense, Shape, Stroke, Ui};

use crate::kit::card as kit_card;
use crate::kit::controls::{self, ButtonKind};
use crate::kit::tokens::{self, palette, Role};
use crate::theme;
use crate::vault_window::password_health::{HealthFilter, HealthReport};
use crate::vault_window::sidebar::{request_shell, ShellRequest};

/// Points taken off for each item with that finding. A breached password is
/// the worst thing the report can say, then weak and reused, then old.
pub const WEIGHT_BREACHED: usize = 8;
pub const WEIGHT_WEAK: usize = 4;
pub const WEIGHT_REUSED: usize = 4;
pub const WEIGHT_OLD: usize = 2;

/// What the card shows, from one report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Score {
    /// 0 to 100.
    pub points: u32,
    /// Distinct items with any finding: the "Fix N items" number.
    pub to_fix: usize,
    pub weak: usize,
    pub reused: usize,
    pub old: usize,
    pub breached: usize,
}

/// The score of `report`.
pub fn score_of(report: &HealthReport) -> Score {
    let weak = report.weak.len();
    let reused: usize = report.reused.iter().map(|g| g.items.len()).sum();
    let old = report.old.len();
    let breached: usize = report.breached.iter().map(|g| g.items.len()).sum();
    let lost = weak * WEIGHT_WEAK + reused * WEIGHT_REUSED + old * WEIGHT_OLD + breached * WEIGHT_BREACHED;
    Score { points: 100u32.saturating_sub(lost.min(100) as u32), to_fix: report.flagged_items(), weak, reused, old, breached }
}

fn slot() -> egui::Id {
    egui::Id::new("settings-security-score")
}

/// The window's once-a-frame hand-over of the report's numbers.
pub fn offer(ctx: &egui::Context, report: &HealthReport) {
    let score = score_of(report);
    ctx.data_mut(|d| d.insert_temp(slot(), score));
}

/// The numbers the window last offered; `None` before the first frame.
pub fn offered(ctx: &egui::Context) -> Option<Score> {
    ctx.data(|d| d.get_temp::<Score>(slot()))
}

/// The card's headline.
pub fn headline(score: &Score) -> &'static str {
    match score.points {
        90..=100 => "Your vault is in good shape",
        60..=89 => "Your vault is mostly in good shape",
        _ => "Your vault needs attention",
    }
}

/// The line under the headline.
pub fn subline(score: &Score) -> String {
    match score.to_fix {
        0 => "Nothing to fix.".to_string(),
        1 => "Fix 1 item to reach 100.".to_string(),
        n => format!("Fix {n} items to reach 100."),
    }
}

/// The primary button's label.
pub fn fix_label(score: &Score) -> String {
    if score.to_fix == 1 {
        "Fix 1 item".to_string()
    } else {
        format!("Fix {} items", score.to_fix)
    }
}

/// What the card asks the window for: the Password health screen under a
/// filter.
fn open_health(ctx: &egui::Context, filter: HealthFilter) {
    crate::vault_window::password_health::show_filter(ctx, filter);
    request_shell(ctx, ShellRequest::OpenHealth);
}

const RING: f32 = 88.0;
const RING_RADIUS: f32 = 36.0;
const RING_STROKE: f32 = 8.0;
const TITLE: Role = Role { size: 21.0, face: theme::EXTRABOLD, tracking_em: -0.01 };
const SUB: Role = Role { size: 15.0, face: theme::REGULAR, tracking_em: 0.0 };
const NUMBER: Role = Role { size: 26.0, face: theme::EXTRABOLD, tracking_em: 0.0 };
const TILE_NUMBER: Role = Role { size: 28.0, face: theme::EXTRABOLD, tracking_em: 0.0 };
const TILE_LABEL: Role = Role { size: 15.0, face: theme::SEMIBOLD, tracking_em: 0.0 };
const TILE_LINK: Role = Role { size: 14.0, face: theme::BOLD, tracking_em: 0.0 };
const AMBER: Color32 = Color32::from_rgb(0xb7, 0x79, 0x1f);

/// Draws the card. Does nothing until the window has offered a score.
pub fn draw(ui: &mut Ui) {
    let Some(score) = offered(ui.ctx()) else { return };
    let p = palette(ui.ctx());
    kit_card::card(ui, tokens::R_CARD_SETTINGS, Margin::same(24), |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(20.0, 20.0);
        ui.horizontal(|ui| {
            ring(ui, &score);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(tokens::job(TITLE, headline(&score), p.ink));
                ui.label(tokens::job(SUB, &subline(&score), p.text_faint));
                if score.to_fix > 0 {
                    ui.add_space(6.0);
                    if controls::button(ui, ButtonKind::Primary, controls::SMALL, &fix_label(&score), None).clicked() {
                        open_health(ui.ctx(), HealthFilter::All);
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let width = ((ui.available_width() - 20.0) / 3.0).floor();
            let tiles = [
                (score.weak, "Weak passwords", theme::ERROR, HealthFilter::Weak),
                (score.reused, "Reused passwords", AMBER, HealthFilter::Reused),
                (score.old, "Older than a year", p.blue_deep, HealthFilter::Old),
            ];
            for (count, label, colour, filter) in tiles {
                if tile(ui, width, count, label, colour) {
                    open_health(ui.ctx(), filter);
                }
            }
        });
    });
}

/// One category tile; whether it was clicked.
fn tile(ui: &mut Ui, width: f32, count: usize, label: &str, colour: Color32) -> bool {
    let p = palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(width, 112.0), Sense::click());
    let fill = if response.hovered() { p.hover } else { p.list_bg };
    ui.painter().rect_filled(rect, CornerRadius::same(16), fill);
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let mut y = rect.min.y + 16.0;
    for (role, text, ink) in [
        (TILE_NUMBER, count.to_string(), colour),
        (TILE_LABEL, label.to_string(), p.ink),
        (TILE_LINK, "Review".to_string(), p.blue_deep),
    ] {
        let galley = ui.fonts_mut(|f| f.layout_job(tokens::job(role, &text, ink)));
        let h = galley.size().y;
        ui.painter().galley(Pos2::new(rect.min.x + 16.0, y), galley, ink);
        y += h + 6.0;
    }
    response.clicked()
}

/// The 88 px ring: a track, an arc for the points, the number in the middle.
fn ring(ui: &mut Ui, score: &Score) {
    let p = palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(RING, RING), Sense::hover());
    let centre = rect.center();
    let painter = ui.painter();
    painter.circle_stroke(centre, RING_RADIUS, Stroke::new(RING_STROKE, p.meter_track));
    if score.points > 0 {
        let sweep = std::f32::consts::TAU * score.points as f32 / 100.0;
        let start = -std::f32::consts::FRAC_PI_2;
        let steps = ((score.points as usize * 64) / 100).max(2);
        let points: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let a = start + sweep * i as f32 / steps as f32;
                centre + RING_RADIUS * vec2(a.cos(), a.sin())
            })
            .collect();
        painter.add(Shape::line(points, Stroke::new(RING_STROKE, p.done_mark)));
    }
    let galley = painter.layout_job(tokens::job(NUMBER, &score.points.to_string(), p.ink));
    painter.galley(centre - galley.size() / 2.0, galley, p.ink);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at};
    use crate::vault_window::password_health::{FoundItem, OldItem, ReuseGroup, WeakItem};

    fn found(id: &str) -> FoundItem {
        FoundItem { id: id.to_string(), name: id.to_string() }
    }

    fn weak(id: &str) -> WeakItem {
        WeakItem { item: found(id), chars: 4, classes: crate::password_strength::CharClasses::of("abcd") }
    }

    #[test]
    fn a_clean_report_scores_a_hundred_with_nothing_to_fix() {
        let s = score_of(&HealthReport::default());
        assert_eq!((s.points, s.to_fix), (100, 0));
        assert_eq!(subline(&s), "Nothing to fix.");
    }

    #[test]
    fn each_finding_takes_its_weight_and_the_count_is_of_distinct_items() {
        let report = HealthReport {
            weak: vec![weak("a"), weak("b")],
            reused: vec![ReuseGroup { items: vec![found("a"), found("c")] }],
            old: vec![OldItem { item: found("d"), days: 400 }],
            ..Default::default()
        };
        let s = score_of(&report);
        let lost = 2 * WEIGHT_WEAK + 2 * WEIGHT_REUSED + WEIGHT_OLD;
        assert_eq!(s.points as usize, 100 - lost);
        // a, b, c, d: "a" is both weak and reused and counts once.
        assert_eq!(s.to_fix, 4);
        assert_eq!(fix_label(&s), "Fix 4 items");
    }

    #[test]
    fn the_score_floors_at_zero() {
        let report = HealthReport { weak: (0..60).map(|i| weak(&format!("w{i}"))).collect(), ..Default::default() };
        assert_eq!(score_of(&report).points, 0);
    }

    #[test]
    fn the_card_waits_for_the_window_to_offer_a_score() {
        let ctx = testing::ctx();
        let painted = testing::frame(&ctx, 0.1, vec2(900.0, 900.0), vec![], draw);
        assert!(painted.texts.is_empty());
    }

    #[test]
    fn fix_items_asks_the_window_for_the_health_screen() {
        use crate::vault_window::sidebar::take_shell_request;
        let ctx = testing::ctx();
        let report = HealthReport { weak: vec![weak("a")], ..Default::default() };
        offer(&ctx, &report);
        for t in 0..3 {
            testing::frame(&ctx, 0.1 * (t + 1) as f64, vec2(900.0, 900.0), vec![], draw);
        }
        let painted = testing::frame(&ctx, 0.4, vec2(900.0, 900.0), vec![], draw);
        let at = painted.texts.iter().find(|(t, _, _)| t == "Fix 1 item").expect("the button").2.center();
        testing::frame(&ctx, 0.5, vec2(900.0, 900.0), click_at(at), draw);
        assert!(take_shell_request(&ctx, ShellRequest::OpenHealth));
    }

    #[test]
    fn the_window_offers_the_report_and_answers_the_request() {
        let window = include_str!("../vault_window/mod.rs").replace("\r\n", "\n");
        let offers = concat!("security_score::", "offer(ui.ctx(), &health)");
        assert_eq!(window.matches(offers).count(), 1, "the window hands over the one report it computed");
        let answers = concat!("take_shell_request(ui.ctx(), sidebar::ShellRequest::Open", "Health)");
        assert_eq!(window.matches(answers).count(), 1, "the request is answered once");
    }

    #[test]
    fn a_tile_asks_for_its_own_filter() {
        use crate::vault_window::sidebar::take_shell_request;
        let ctx = testing::ctx();
        let report = HealthReport { weak: vec![weak("a")], ..Default::default() };
        offer(&ctx, &report);
        for t in 0..3 {
            testing::frame(&ctx, 0.1 * (t + 1) as f64, vec2(900.0, 900.0), vec![], draw);
        }
        let painted = testing::frame(&ctx, 0.4, vec2(900.0, 900.0), vec![], draw);
        let at = painted.texts.iter().find(|(t, _, _)| t == "Weak passwords").expect("the tile").2.center();
        testing::frame(&ctx, 0.5, vec2(900.0, 900.0), click_at(at), draw);
        assert!(take_shell_request(&ctx, ShellRequest::OpenHealth));
    }
}

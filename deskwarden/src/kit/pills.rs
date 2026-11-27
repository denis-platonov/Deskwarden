//! The vault window's last v1 layout helpers, in the v2 palette: the one-item
//! subject a confirmation card opens with, the favicon tile it puts in front
//! of the name, and the month calendar the Send composer's expiry opens.
//!
//! These were `theme::modal_subject`, `theme::avatar_artwork_tile`,
//! `theme::avatar_image` and `theme::date_picker`, which paint with the light
//! constants and so stayed light under a dark theme. Same sizes, same layout,
//! same behaviour; only the colours come from
//! [`palette`](crate::kit::tokens::palette).

use eframe::egui::{self, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};

use crate::kit::tokens::{palette, Palette};

/// The subject tile's side: the same 28 `theme::MODAL_SUBJECT_TILE` was.
pub const SUBJECT_TILE: f32 = 28.0;
const NAME_PX: f32 = 13.0;
const USERNAME_PX: f32 = 11.0;
const GAP_Y: f32 = 2.0;

/// What a confirmation card is about: the item's tile, its name in ink and
/// its login under it in faint text. `tile` draws the 28-point tile; the
/// username is skipped when there is none.
pub fn modal_subject(ui: &mut Ui, name: &str, username: &str, tile: impl FnOnce(&mut Ui, f32)) {
    let p = palette(ui.ctx());
    ui.horizontal(|ui| {
        tile(ui, SUBJECT_TILE);
        // A vertical of its own so the one- or two-line column is centred
        // against the tile, as a list row's is.
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = GAP_Y;
            ui.add(egui::Label::new(crate::theme::semibold(name, NAME_PX).color(p.ink)).truncate());
            if !username.is_empty() {
                ui.add(egui::Label::new(RichText::new(username).size(USERNAME_PX).color(p.text_faint)).truncate());
            }
        });
    });
}

/// The edge of an avatar tile, in the palette's hairline (or the blue edge
/// when emphasised).
fn tile_edge(ui: &Ui, emphasized: bool) -> Stroke {
    let p = palette(ui.ctx());
    Stroke::new(1.0, if emphasized { p.blue_edge } else { p.hairline })
}

/// A favicon's tile: allocated, snapped to a whole physical pixel, rounded and
/// edged, with no fill (the artwork is its own ground).
pub fn artwork_tile(ui: &mut Ui, size: f32, emphasized: bool) -> Rect {
    let (allocated, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let ppp = ui.ctx().pixels_per_point();
    let min = egui::pos2((allocated.min.x * ppp).round() / ppp, (allocated.min.y * ppp).round() / ppp);
    let rect = Rect::from_min_size(min, allocated.size());
    ui.painter().rect_stroke(rect, crate::theme::avatar_corner_radius(size), tile_edge(ui, emphasized), StrokeKind::Inside);
    rect
}

/// The favicon, painted into `tile` (from [`artwork_tile`]) with the tile's
/// edge drawn again over it.
pub fn artwork_image(ui: &Ui, tile: Rect, texture: &egui::TextureHandle, emphasized: bool) {
    let (art, rounding) = crate::theme::avatar_artwork(tile, texture.size_vec2(), ui.ctx().pixels_per_point());
    egui::Image::new((texture.id(), texture.size_vec2())).corner_radius(rounding).paint_at(ui, art);
    ui.painter().rect_stroke(tile, crate::theme::avatar_corner_radius(tile.width()), tile_edge(ui, emphasized), StrokeKind::Inside);
}

// ---------------------------------------------------------------------------
// The calendar
// ---------------------------------------------------------------------------

/// A `(year, month, day)` on the proleptic-Gregorian calendar
/// [`crate::local_time`] works in. Months and days are 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Day {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

/// The seven column headings, Monday first.
pub const WEEKDAY_INITIALS: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];
/// One day cell's size: two digits plus room to be a click target.
pub const CALENDAR_CELL: Vec2 = Vec2::new(28.0, 24.0);
const CALENDAR_HEADER_H: f32 = 28.0;
const CALENDAR_WEEKDAY_H: f32 = 16.0;
const CALENDAR_WEEKDAY_PX: f32 = 10.0;
const CALENDAR_ROWS: usize = 6;
const CALENDAR_PAD: f32 = 6.0;
const CALENDAR_RADIUS: u8 = 7;
const CALENDAR_TEXT_PX: f32 = 12.0;
const CALENDAR_ICON_STROKE: f32 = 1.3;
/// The picker's outer width: seven columns and a pad each side.
pub const CALENDAR_WIDTH: f32 = CALENDAR_CELL.x * 7.0 + CALENDAR_PAD * 2.0;
/// The picker's outer height.
pub const CALENDAR_HEIGHT: f32 =
    CALENDAR_HEADER_H + CALENDAR_WEEKDAY_H + CALENDAR_CELL.y * CALENDAR_ROWS as f32 + CALENDAR_PAD;

/// One month of a calendar, with the days that are not answers drawn inert.
/// Returns the day clicked this frame.
///
/// Which month is on screen is this widget's own business: it lives in egui's
/// memory under `id_salt` (the month of what is chosen, else of `first`) and
/// is clamped into the window on the way in. The grid cannot express a date
/// that does not exist or one outside `first..=last`; days outside the window
/// are drawn inert rather than hidden. Six week rows always, so the control
/// never changes height as the month changes.
pub fn date_picker(ui: &mut Ui, id_salt: &str, selected: Option<Day>, first: Day, last: Day) -> Option<Day> {
    let memory_id = egui::Id::new(("date-picker", id_salt));
    let remembered = ui
        .ctx()
        .memory(|m| m.data.get_temp::<Day>(memory_id))
        .unwrap_or_else(|| month_of(selected.unwrap_or(first)));
    let mut shown = remembered.clamp(month_of(first), month_of(last));
    let picked = date_picker_at(ui, &mut shown, selected, first, last);
    ui.ctx().memory_mut(|m| m.data.insert_temp(memory_id, shown));
    picked
}

/// [`date_picker`]'s body with the month on screen passed in, so a test can
/// put the calendar on any month and read what it painted in one frame.
pub fn date_picker_at(ui: &mut Ui, shown: &mut Day, selected: Option<Day>, first: Day, last: Day) -> Option<Day> {
    // Read once, before any painter or font closure below.
    let p = palette(ui.ctx());
    let (frame, _) = ui.allocate_exact_size(Vec2::new(CALENDAR_WIDTH, CALENDAR_HEIGHT), Sense::hover());
    ui.painter().rect(frame, CornerRadius::same(CALENDAR_RADIUS), p.list_bg, Stroke::new(1.0, p.border), StrokeKind::Inside);

    // Header: back, "Mar 2027", forward.
    let header = Rect::from_min_size(frame.min, Vec2::new(CALENDAR_WIDTH, CALENDAR_HEADER_H));
    let arrow = Vec2::splat(CALENDAR_HEADER_H - 8.0);
    let back = Rect::from_min_size(egui::pos2(header.left() + 4.0, header.top() + 4.0), arrow);
    let forward = Rect::from_min_size(egui::pos2(header.right() - 4.0 - arrow.x, header.top() + 4.0), arrow);
    if calendar_arrow(ui, &p, back, false, *shown > month_of(first)) {
        *shown = step_month(*shown, -1);
    }
    if calendar_arrow(ui, &p, forward, true, *shown < month_of(last)) {
        *shown = step_month(*shown, 1);
    }
    let title = ui.painter().layout_no_wrap(
        format!("{} {}", crate::local_time::month_name(shown.month), shown.year),
        egui::FontId::new(CALENDAR_TEXT_PX, egui::FontFamily::Name(crate::theme::SEMIBOLD.into())),
        p.ink,
    );
    ui.painter().galley(egui::pos2(header.center().x - title.size().x / 2.0, header.center().y - title.size().y / 2.0), title, p.ink);

    // The weekday strip: Monday first, one letter each.
    for (column, letter) in WEEKDAY_INITIALS.iter().enumerate() {
        let cell = calendar_cell(frame, column, None);
        let galley = ui.painter().layout_no_wrap(
            (*letter).to_owned(),
            egui::FontId::new(CALENDAR_WEEKDAY_PX, egui::FontFamily::Name(crate::theme::SEMIBOLD.into())),
            p.text_ghost,
        );
        ui.painter().galley(
            egui::pos2(cell.center().x - galley.size().x / 2.0, cell.center().y - galley.size().y / 2.0),
            galley,
            p.text_ghost,
        );
    }

    // The days.
    let first_of_month = crate::local_time::days_from_civil(shown.year, shown.month, 1);
    let lead = monday_column(first_of_month);
    let length = crate::local_time::days_in_month(shown.year, shown.month);
    let mut picked = None;
    for day in 1..=length {
        let index = lead + (day as usize) - 1;
        let cell = calendar_cell(frame, index % 7, Some(index / 7));
        let here = Day { year: shown.year, month: shown.month, day };
        if calendar_day(ui, &p, cell, day, selected == Some(here), here >= first && here <= last) {
            picked = Some(here);
        }
    }
    picked
}

/// Where a grid cell sits; `row` is `None` for the weekday strip.
fn calendar_cell(frame: Rect, column: usize, row: Option<usize>) -> Rect {
    let (top, height) = match row {
        None => (frame.top() + CALENDAR_HEADER_H, CALENDAR_WEEKDAY_H),
        Some(row) => (frame.top() + CALENDAR_HEADER_H + CALENDAR_WEEKDAY_H + row as f32 * CALENDAR_CELL.y, CALENDAR_CELL.y),
    };
    Rect::from_min_size(
        egui::pos2(frame.left() + CALENDAR_PAD + column as f32 * CALENDAR_CELL.x, top),
        Vec2::new(CALENDAR_CELL.x, height),
    )
}

/// Which column a Unix epoch day falls in, Monday = 0 (epoch day 0 is a
/// Thursday).
fn monday_column(epoch_day: i64) -> usize {
    (epoch_day + 3).rem_euclid(7) as usize
}

/// The month a day belongs to, flattened to the 1st.
fn month_of(day: Day) -> Day {
    Day { day: 1, ..day }
}

/// `shown` moved one month in `direction`, staying on the 1st.
fn step_month(shown: Day, direction: i64) -> Day {
    let zero_based = i64::from(shown.month) - 1 + direction;
    Day { year: shown.year + zero_based.div_euclid(12), month: (zero_based.rem_euclid(12) + 1) as u32, day: 1 }
}

/// One month arrow; a disabled one is drawn and senses nothing.
fn calendar_arrow(ui: &mut Ui, p: &Palette, rect: Rect, forward: bool, enabled: bool) -> bool {
    let response = ui.interact(rect, ui.id().with(("calendar-arrow", forward)), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        ui.painter().rect_filled(rect, CornerRadius::same(4), p.hover);
    }
    let color = if enabled { p.ink } else { p.text_ghost };
    let stroke = Stroke::new(CALENDAR_ICON_STROKE, color);
    let tip_x = if forward { rect.center().x + 2.5 } else { rect.center().x - 2.5 };
    let back_x = if forward { rect.center().x - 2.0 } else { rect.center().x + 2.0 };
    let painter = ui.painter();
    painter.line_segment([egui::pos2(back_x, rect.center().y - 4.0), egui::pos2(tip_x, rect.center().y)], stroke);
    painter.line_segment([egui::pos2(tip_x, rect.center().y), egui::pos2(back_x, rect.center().y + 4.0)], stroke);
    enabled && response.clicked()
}

/// One day cell; true when chosen. Out-of-window days sense nothing.
fn calendar_day(ui: &mut Ui, p: &Palette, rect: Rect, day: u32, selected: bool, offered: bool) -> bool {
    let response = ui.interact(
        rect,
        ui.id().with(("calendar-day", rect.left() as i32, rect.top() as i32)),
        if offered { Sense::click() } else { Sense::hover() },
    );
    let hovered = offered && response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let clear = egui::Color32::TRANSPARENT;
    let (fill, ink) = if selected {
        (p.blue, egui::Color32::WHITE)
    } else if hovered {
        (p.hover, p.ink)
    } else if offered {
        (clear, p.ink)
    } else {
        (clear, p.text_ghost)
    };
    if fill != clear {
        ui.painter().rect_filled(Rect::from_center_size(rect.center(), Vec2::splat(CALENDAR_CELL.y - 2.0)), CornerRadius::same(4), fill);
    }
    let galley = ui.painter().layout_no_wrap(
        day.to_string(),
        egui::FontId::new(CALENDAR_TEXT_PX, egui::FontFamily::Proportional),
        ink,
    );
    ui.painter().galley(egui::pos2(rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0), galley, ink);
    offered && response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, Painted};
    use crate::kit::tokens::set_palette;

    fn subject(palette: Palette) -> Painted {
        let ctx = testing::ctx();
        set_palette(&ctx, palette);
        testing::frame(&ctx, 0.0, egui::vec2(400.0, 200.0), vec![], |ui| {
            modal_subject(ui, "Ledgerline", "ada@example.invalid", |ui, size| {
                let _ = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
            });
        })
    }

    #[test]
    fn the_subject_is_set_in_the_palettes_ink_and_faint_text() {
        for p in [Palette::LIGHT, Palette::DARK] {
            let painted = subject(p);
            let ink_of = |t: &str| painted.texts.iter().find(|(s, _, _)| s == t).unwrap_or_else(|| panic!("no {t}")).1;
            assert_eq!(ink_of("Ledgerline"), p.ink);
            assert_eq!(ink_of("ada@example.invalid"), p.text_faint);
        }
    }

    #[test]
    fn the_calendar_paints_under_both_palettes_with_its_ground_from_the_palette() {
        let day = |d| Day { year: 2027, month: 3, day: d };
        for p in [Palette::LIGHT, Palette::DARK] {
            let ctx = testing::ctx();
            set_palette(&ctx, p);
            let painted = testing::frame(&ctx, 0.0, egui::vec2(400.0, 300.0), vec![], |ui| {
                let mut shown = day(1);
                let _ = date_picker_at(ui, &mut shown, Some(day(14)), day(1), day(31));
            });
            assert!(painted.rects.iter().any(|r| r.fill == p.list_bg && r.rect.width() == CALENDAR_WIDTH), "the ground");
            assert!(painted.rects.iter().any(|r| r.fill == p.blue), "the chosen day");
            assert_eq!(painted.text("Mar 2027").map(|t| t.1), Some(p.ink));
        }
    }

    #[test]
    fn a_month_steps_across_a_year_and_lines_up_on_monday() {
        let dec = Day { year: 2026, month: 12, day: 1 };
        assert_eq!(step_month(dec, 1), Day { year: 2027, month: 1, day: 1 });
        assert_eq!(step_month(Day { year: 2027, month: 1, day: 1 }, -1), dec);
        // 1 January 1970 was a Thursday: column 3 of a Monday-first week.
        assert_eq!(monday_column(0), 3);
    }

    #[test]
    fn the_vault_windows_converted_views_take_their_colours_from_the_dark_palette() {
        let d = Palette::DARK;
        // Send pills.
        assert_eq!(crate::vault_window::send_ui::field_tone(&d).fill, d.blue_wash);
        assert_eq!(crate::vault_window::send_ui::secret_field_tone(&d).ink, d.danger_ink);
        assert_eq!(crate::vault_window::send_ui::state_tone(&d, crate::send::SendState::Used).fill, d.done_wash);
        // The new-item cards.
        assert_eq!(crate::vault_window::new_item_modal::choices(&d)[0].kind, crate::vault_bridge::ItemKind::Login);
        // The splash: the dark ground, not the navy one.
        let ctx = testing::ctx();
        set_palette(&ctx, d);
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
        let painted = testing::frame(&ctx, 0.0, egui::vec2(400.0, 300.0), vec![], |ui| {
            crate::vault_window::splash::paint(ui.ctx(), rect, false, 0.0);
        });
        assert!(painted.rects.iter().any(|r| r.fill == d.window_bg && r.rect == rect), "the splash is not on the dark ground");
        assert!(!painted.rects.iter().any(|r| r.fill == Palette::LIGHT.blue_deep), "the navy sheet is still there");
    }
}

//! The item editor's form furniture in the v2 look: the section card and its
//! rows, the number box, the dismiss and tune marks, the secret band, the
//! multi-line text area and the "changed" pill.
//!
//! These are the v1 `theme::section_*` family redrawn with the palette
//! ([`crate::kit::tokens::palette`]) instead of colour constants, so they follow
//! the light and the dark theme. Geometry (the label column, the row gap, the
//! field height, the card padding) is unchanged: it is the same layout, in the
//! v2 colours. Sizes and type helpers still come from `crate::theme`.

use eframe::egui::{
    self, CornerRadius, FontFamily, FontId, Margin, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};

use crate::kit::icon;
use crate::kit::tokens::{card_shadow, palette, Palette};
use crate::theme::{
    ink_drop, letterspaced, ACTION_BUTTON_MIN_WIDTH, ACTION_BUTTON_TEXT_PX, BOLD, EXTRABOLD,
    EYEBROW_GAP, SECTION_CARD_HEADER_PAD_Y, SECTION_CARD_PAD_X,
    SECTION_CARD_PAD_X_WIDE, SECTION_CARD_PAD_Y, SECTION_CARD_RADIUS, SECTION_CARD_WIDE_AT,
    SECTION_FIELD_HEIGHT, SECTION_FOOTER_BUTTON_PAD_X, SECTION_FOOTER_CHIP_GAP,
    SECTION_FOOTER_CHIP_PX, SECTION_FOOTER_TEXT_PX, SECTION_GRID_CELL_FLOOR, SECTION_GRID_GAP,
    SCROLLBAR_WIDTH, SECTION_HEADER_GAP, SECTION_LABEL_WIDTH, SECTION_ROW_CONTROL_FLOOR, SECTION_ROW_GAP, SEMIBOLD,
};

/// The title's size on an edit card (spec B 0.6: 18/800).
pub const EDIT_CARD_TITLE_PX: f32 = 18.0;

/// One cell of a segmented choice: what it says and whether it is the lit one.
#[derive(Clone, Copy)]
pub struct Segment<'a> {
    pub label: &'a str,
    pub selected: bool,
}

/// The hit box of the dismiss mark.
pub const CLOSE_HIT: f32 = 16.0;
/// Its drawn width: the two arms span this much.
pub const CLOSE_SPAN: f32 = 7.0;

const PILL_HEIGHT: f32 = 20.0;
const PILL_TEXT_PX: f32 = 10.0;
const PILL_PAD_X: f32 = 8.0;

// ---------------------------------------------------------------------------
// The changed pill
// ---------------------------------------------------------------------------

/// The word on the pill a card wears once something in it has been edited.
pub fn changed_pill() -> &'static str {
    crate::i18n::t("edit.changed_pill")
}

fn pill_galley(painter: &egui::Painter, text: &str, ink: egui::Color32) -> std::sync::Arc<egui::Galley> {
    painter.layout_no_wrap(
        text.to_string(),
        FontId::new(PILL_TEXT_PX, FontFamily::Name(BOLD.into())),
        ink,
    )
}

/// How wide [`changed_pill`] paints.
pub fn changed_pill_width(painter: &egui::Painter) -> f32 {
    pill_galley(painter, changed_pill(), egui::Color32::BLACK).size().x + PILL_PAD_X * 2.0 + 2.0
}

/// The caution-toned pill, its left edge at `left_center`.
fn paint_changed_pill(painter: &egui::Painter, left_center: Pos2, p: &Palette) -> Rect {
    let text = changed_pill();
    let galley = pill_galley(painter, text, p.caution_ink);
    let width = changed_pill_width(painter);
    let rect = Rect::from_min_size(
        Pos2::new(left_center.x, left_center.y - PILL_HEIGHT / 2.0),
        Vec2::new(width, PILL_HEIGHT),
    );
    let rounding = CornerRadius::same((PILL_HEIGHT / 2.0) as u8);
    painter.rect_filled(rect, rounding, p.caution_wash);
    painter.rect_stroke(rect, rounding, Stroke::new(1.0, p.caution_edge), StrokeKind::Inside);
    let font = FontId::new(PILL_TEXT_PX, FontFamily::Name(BOLD.into()));
    let drop = painter.ctx().fonts_mut(|f| {
        let probe = f.layout_no_wrap("X".to_string(), font.clone(), egui::Color32::BLACK);
        probe.rows.first().and_then(|row| row.glyphs.first()).map_or(0.0, |g| {
            let above = g.pos.y + g.uv_rect.offset.y;
            let below = g.font_height - g.pos.y;
            ((below - above) / 2.0).max(0.0)
        })
    });
    painter.galley(
        Pos2::new(rect.min.x + 1.0 + PILL_PAD_X, rect.center().y - galley.size().y / 2.0 + drop),
        galley,
        p.caution_ink,
    );
    rect
}

// ---------------------------------------------------------------------------
// The section card
// ---------------------------------------------------------------------------

/// The card's horizontal padding for a card this wide.
pub fn section_card_pad_x(width: f32) -> i8 {
    if width >= SECTION_CARD_WIDE_AT {
        SECTION_CARD_PAD_X_WIDE
    } else {
        SECTION_CARD_PAD_X
    }
}

/// Whether a card this wide can draw the three-column grid.
pub fn section_grid_fits_at(width: f32) -> bool {
    width >= 3.0 * SECTION_GRID_CELL_FLOOR + 2.0 * SECTION_GRID_GAP
}

/// One cell of that grid: a 12-point caption over its control.
pub fn section_grid_cell<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = palette(ui.ctx());
    ui.spacing_mut().item_spacing.y = EYEBROW_GAP;
    ui.label(RichText::new(label).size(12.0).color(p.text_faint));
    add(ui)
}

/// Whether a card this wide draws label-beside-control rows.
pub fn section_rows_fit(ui: &Ui) -> bool {
    section_rows_fit_at(ui.available_width())
}

/// [`section_rows_fit`], asked of a width.
pub fn section_rows_fit_at(width: f32) -> bool {
    width >= SECTION_LABEL_WIDTH + SECTION_ROW_GAP + SECTION_ROW_CONTROL_FLOOR
}

/// The card: palette card fill, ring, soft shadow, no padding of its own. The
/// ring is painted after the contents so an edge-to-edge rule lands under it.
pub fn section_card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> (Rect, R) {
    let p = palette(ui.ctx());
    let framed = egui::Frame::new()
        .fill(p.card)
        .shadow(card_shadow())
        .corner_radius(CornerRadius::same(SECTION_CARD_RADIUS))
        .show(ui, |ui| {
            // The bands stack flush: each carries its own margin.
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            add(ui)
        });
    let rect = framed.response.rect;
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(SECTION_CARD_RADIUS),
        Stroke::new(1.0, p.card_ring),
        StrokeKind::Inside,
    );
    (rect, framed.inner)
}

/// The card's first band: its name, an optional note and, once edited, the
/// changed pill; closed by a hairline. Answers with the title's rectangle.
pub fn section_card_header_at(ui: &mut Ui, pad_x: i8, title: &str, note: &str, changed: bool) -> Rect {
    let p = palette(ui.ctx());
    let drop = ink_drop(
        ui.ctx(),
        &FontId::new(EDIT_CARD_TITLE_PX, FontFamily::Name(EXTRABOLD.into())),
        None,
    )
    .round() as i8;
    let line = egui::Frame::new()
        .inner_margin(Margin {
            left: pad_x,
            right: pad_x,
            top: SECTION_CARD_HEADER_PAD_Y + drop,
            bottom: SECTION_CARD_HEADER_PAD_Y - drop,
        })
        .show(ui, |ui| {
            // The row is its title's height and nothing else's.
            ui.spacing_mut().interact_size.y = 0.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = SECTION_HEADER_GAP;
                let title_rect = ui
                    .label(letterspaced(title, EDIT_CARD_TITLE_PX, EXTRABOLD, -0.18, p.ink))
                    .rect;
                if !note.is_empty() {
                    ui.label(RichText::new(note).size(12.0).color(p.text_ghost));
                }
                if changed {
                    let width = changed_pill_width(ui.painter());
                    if ui.available_width() >= width {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(ui.available_width(), 0.0), Sense::hover());
                        paint_changed_pill(
                            ui.painter(),
                            Pos2::new(rect.right() - width, title_rect.center().y),
                            &p,
                        );
                    }
                }
                title_rect
            })
            .inner
        })
        .inner;
    crate::kit::card::hairline(ui);
    line
}

#[derive(Clone, Copy)]
struct SectionBody {
    pad_x: i8,
    /// No row has been drawn in this body yet, so the next one needs no rule.
    first: bool,
    /// The card's full x-range, for rules that reach both edges.
    card: egui::Rangef,
}

fn section_body_state(ui: &Ui) -> egui::Id {
    ui.id().with("form-section-body")
}

/// Marks the current row as drawn without drawing a rule: for a band that
/// carries its own.
pub fn section_row_seen(ui: &mut Ui) {
    let id = section_body_state(ui);
    if let Some(state) = ui.data(|data| data.get_temp::<SectionBody>(id)) {
        ui.data_mut(|data| data.insert_temp(id, SectionBody { first: false, ..state }));
    }
}

fn section_row_rule(ui: &mut Ui, state: SectionBody) {
    let p = palette(ui.ctx());
    let slack = (f32::from(SECTION_CARD_PAD_Y) - ui.spacing().item_spacing.y).max(0.0);
    let (band, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0 * slack + 1.0), Sense::hover());
    let line = Rect::from_x_y_ranges(
        state.card,
        egui::Rangef::new(band.center().y - 0.5, band.center().y + 0.5),
    );
    ui.painter().rect_filled(line, CornerRadius::ZERO, p.row_rule);
}

/// The card's body band, padded `pad_x` either side; rows inside rule
/// themselves off from the one above.
pub fn section_card_body_at<R>(ui: &mut Ui, pad_x: i8, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(pad_x, SECTION_CARD_PAD_Y))
        .show(ui, |ui| {
            let id = section_body_state(ui);
            let card = egui::Rangef::new(
                ui.max_rect().left() - f32::from(pad_x),
                ui.max_rect().right() + f32::from(pad_x),
            );
            ui.data_mut(|data| data.insert_temp(id, SectionBody { pad_x, first: true, card }));
            ui.spacing_mut().item_spacing.y = ui.style().spacing.item_spacing.y.max(8.0);
            add(ui)
        })
        .inner
}

/// A row: a caption in the label column, the control beside it (above it when
/// the card is too narrow).
pub fn section_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    section_row_impl(ui, label, None::<fn(&mut Ui)>, add)
}

/// [`section_row`] with `aside` under the caption in the label column.
pub fn section_row_aside<R>(
    ui: &mut Ui,
    label: &str,
    aside: impl FnOnce(&mut Ui),
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    section_row_impl(ui, label, Some(aside), add)
}

/// A row whose band reaches both edges of the card, for a row that is itself a
/// full-width control.
pub fn section_row_bleed<R>(ui: &mut Ui, band_pad_y: i8, add: impl FnOnce(&mut Ui) -> R) -> R {
    let id = section_body_state(ui);
    let state = ui.data(|data| data.get_temp::<SectionBody>(id));
    if let Some(state) = state {
        if !state.first {
            section_row_rule(ui, state);
        }
        ui.data_mut(|data| data.insert_temp(id, SectionBody { first: false, ..state }));
    }
    let Some(SectionBody { pad_x, card, .. }) = state else {
        return add(ui);
    };
    egui::Frame::new()
        .outer_margin(Margin {
            left: -pad_x,
            right: -pad_x,
            top: -band_pad_y,
            bottom: -band_pad_y,
        })
        .show(ui, |ui| {
            let lane = Rect::from_x_y_ranges(card, ui.max_rect().y_range());
            ui.scope_builder(egui::UiBuilder::new().max_rect(lane), |ui| {
                ui.set_width(lane.width());
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let clip = Rect::from_x_y_ranges(card, ui.clip_rect().y_range());
                ui.set_clip_rect(ui.clip_rect().intersect(clip));
                add(ui)
            })
            .inner
        })
        .inner
}

fn section_row_impl<R>(
    ui: &mut Ui,
    label: &str,
    aside: Option<impl FnOnce(&mut Ui)>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let p = palette(ui.ctx());
    let id = section_body_state(ui);
    if let Some(state) = ui.data(|data| data.get_temp::<SectionBody>(id)) {
        if !state.first {
            section_row_rule(ui, state);
        }
        ui.data_mut(|data| data.insert_temp(id, SectionBody { first: false, ..state }));
    }
    if !section_rows_fit(ui) {
        match aside {
            None => {
                if !label.is_empty() {
                    ui.label(RichText::new(label).size(12.0).color(p.text_muted));
                    ui.add_space(EYEBROW_GAP);
                }
            }
            Some(aside) => {
                ui.horizontal_wrapped(|ui| {
                    if !label.is_empty() {
                        ui.label(RichText::new(label).size(12.0).color(p.text_muted));
                    }
                    aside(ui);
                });
                ui.add_space(EYEBROW_GAP);
            }
        }
        return add(ui);
    }
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(SECTION_LABEL_WIDTH, SECTION_FIELD_HEIGHT),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(SECTION_LABEL_WIDTH);
                let galley = ui.painter().layout(
                    label.to_string(),
                    FontId::new(12.0, FontFamily::Proportional),
                    p.text_faint,
                    SECTION_LABEL_WIDTH,
                );
                let top = (SECTION_FIELD_HEIGHT - galley.size().y) / 2.0;
                let reserved =
                    if aside.is_some() { top + galley.size().y } else { SECTION_FIELD_HEIGHT };
                let (cell, _) = ui.allocate_exact_size(
                    Vec2::new(SECTION_LABEL_WIDTH, reserved),
                    Sense::hover(),
                );
                ui.painter().galley(Pos2::new(cell.left(), cell.top() + top), galley, p.text_faint);
                if let Some(aside) = aside {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    aside(ui);
                }
            },
        );
        ui.add_space(SECTION_ROW_GAP - ui.spacing().item_spacing.x);
        ui.vertical(|ui| add(ui)).inner
    })
    .inner
}

// ---------------------------------------------------------------------------
// Small controls and bands
// ---------------------------------------------------------------------------

/// A drag value that shows the resize cursor on hover.
pub fn number_box(ui: &mut Ui, value: egui::DragValue<'_>) -> Response {
    let response = ui.add(value);
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    response
}

/// [`number_box`], greyed and inert when `enabled` is false.
pub fn number_box_enabled(ui: &mut Ui, enabled: bool, value: egui::DragValue<'_>) -> Response {
    let response = ui.add_enabled(enabled, value);
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    response
}

fn paint_close(ui: &Ui, rect: Rect, response: &Response) {
    let p = palette(ui.ctx());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if response.hovered() { p.ink } else { p.text_ghost };
    // Two crossing strokes, not a font glyph: no bundled face carries U+2715.
    let stroke = Stroke::new(1.5, color);
    let arm = CLOSE_SPAN / 2.0;
    let c = rect.center();
    let painter = ui.painter();
    painter.line_segment([c + Vec2::new(-arm, -arm), c + Vec2::new(arm, arm)], stroke);
    painter.line_segment([c + Vec2::new(arm, -arm), c + Vec2::new(-arm, arm)], stroke);
}

/// The small x that removes a row, with a stable id so a hit test can find it.
pub fn close_glyph_with_id(ui: &mut Ui, id: egui::Id, tooltip: &str) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(CLOSE_HIT), Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    paint_close(ui, rect, &response);
    response.on_hover_text(tooltip.to_string())
}

/// The small x, titled for assistive tools and hover.
pub fn close_glyph_titled(ui: &mut Ui, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CLOSE_HIT), Sense::click());
    paint_close(ui, rect, &response);
    response.on_hover_text(tooltip.to_string())
}

/// The sliders mark that opens a row's settings; `lit` while they are open.
pub fn tune_mark(ui: &mut Ui, size: f32, lit: bool, hover: &str) -> Response {
    let p = palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = match (lit, response.hovered()) {
        (true, _) => p.blue,
        (false, true) => p.ink,
        (false, false) => p.text_nav,
    };
    let glyph = Rect::from_center_size(rect.center(), Vec2::splat(size * 0.7));
    icon::stroke(ui.painter(), glyph, icon::glyph::SETTINGS, 1.8, color);
    response.on_hover_text(hover.to_owned())
}

/// The Enter keycap on the picker's lit row.
pub fn paint_return_keycap(painter: &egui::Painter, rect: Rect, p: &Palette) {
    painter.rect_filled(rect, CornerRadius::same(5), p.blue);
    let glyph = Rect::from_center_size(rect.center(), Vec2::splat(11.0));
    icon::stroke(painter, glyph, icon::glyph::RETURN, 2.2, egui::Color32::WHITE);
}

/// The band a secret-bearing row sits in: danger wash with a danger edge.
pub fn secret_band(p: &Palette) -> egui::Frame {
    egui::Frame::new().fill(p.danger_wash).stroke(Stroke::new(1.0, p.danger_edge))
}

/// How wide a footer's standing note paints.
pub fn form_footer_note_width(ui: &Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), FontId::new(12.0, FontFamily::Proportional), egui::Color32::BLACK)
        .size()
        .x
}

/// How wide the footer's Save button is, with its key chip.
pub fn section_footer_save_width(painter: &egui::Painter, label: &str, kbd: &str) -> f32 {
    let label_w = painter
        .layout_no_wrap(
            label.to_string(),
            FontId::new(SECTION_FOOTER_TEXT_PX, FontFamily::Name(BOLD.into())),
            egui::Color32::BLACK,
        )
        .size()
        .x;
    let chip_w = SECTION_FOOTER_CHIP_GAP
        + painter
            .layout_no_wrap(
                kbd.to_string(),
                FontId::new(SECTION_FOOTER_CHIP_PX, FontFamily::Monospace),
                egui::Color32::BLACK,
            )
            .size()
            .x;
    (label_w + chip_w + SECTION_FOOTER_BUTTON_PAD_X * 2.0).max(ACTION_BUTTON_MIN_WIDTH)
}

/// How wide a footer button with `label` is.
pub fn action_button_width(painter: &egui::Painter, label: &str, pad_x: f32) -> f32 {
    let galley = painter.layout_no_wrap(
        label.to_string(),
        FontId::new(ACTION_BUTTON_TEXT_PX, FontFamily::Name(SEMIBOLD.into())),
        egui::Color32::BLACK,
    );
    (galley.size().x + pad_x * 2.0).max(ACTION_BUTTON_MIN_WIDTH)
}

/// Reserves `gutter` points at the scroll area's outer edge for the bar, so a
/// short and a long form lay out alike.
pub fn scrollbar_in_gutter(ui: &mut Ui, gutter: f32) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating_allocated_width = gutter;
    scroll.bar_width = SCROLLBAR_WIDTH;
    scroll.floating_width = SCROLLBAR_WIDTH;
    scroll.bar_outer_margin = 0.0;
}

/// Stops painting the scroll bar without moving anything it reserved.
pub fn hide_scrollbar(ui: &mut Ui) {
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.dormant_background_opacity = 0.0;
    scroll.active_background_opacity = 0.0;
    scroll.interact_background_opacity = 0.0;
    scroll.dormant_handle_opacity = 0.0;
    scroll.active_handle_opacity = 0.0;
    scroll.interact_handle_opacity = 0.0;
}

/// A multi-line box in the v2 look: a 1 pt border inside the box that turns
/// blue with the keyboard, on the card fill. At least `min_height` tall.
pub fn text_area(ui: &mut Ui, value: &mut String, hint: &str, min_height: f32, radius: u8) -> Response {
    let p = palette(ui.ctx());
    let rows = 4usize;
    let pad_y = 9.0;
    let bg = ui.painter().add(egui::Shape::Noop);
    let row_height = ui.ctx().fonts_mut(|f| f.row_height(&FontId::new(14.0, FontFamily::Proportional)));
    let top_left = ui.next_widget_position();
    let width = ui.available_width();
    let inner = Rect::from_min_size(
        Pos2::new(top_left.x + 10.0, top_left.y + pad_y),
        Vec2::new(width - 20.0, row_height * rows as f32),
    );
    let mut text_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    let response = text_ui.add(
        egui::TextEdit::multiline(value)
            .hint_text(hint)
            .frame(egui::Frame::new())
            .font(FontId::new(14.0, FontFamily::Proportional))
            .margin(Margin::ZERO)
            .desired_width(inner.width())
            .desired_rows(rows),
    );
    let outer = Rect::from_min_size(
        top_left,
        Vec2::new(width, (response.rect.height().max(inner.height()) + pad_y * 2.0).max(min_height)),
    );
    ui.advance_cursor_after_rect(outer);
    let border = Stroke::new(1.0, if response.has_focus() { p.blue } else { p.border });
    ui.painter().set(
        bg,
        egui::epaint::RectShape::new(outer, CornerRadius::same(radius), p.card, border, StrokeKind::Inside),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{ctx, frame};
    use crate::kit::tokens::set_palette;

    fn draw_all(ctx: &egui::Context, time: f64) -> crate::kit::testing::Painted {
        frame(ctx, time, Vec2::new(700.0, 600.0), vec![], |ui| {
            ui.set_width(600.0);
            section_card(ui, |ui| {
                section_card_header_at(ui, 16, "Item", "note", true);
                section_card_body_at(ui, 16, |ui| {
                    section_row(ui, "Name", |ui| {
                        ui.label("x");
                    });
                    section_row_aside(
                        ui,
                        "Other",
                        |ui| {
                            ui.label("aside");
                        },
                        |ui| {
                            close_glyph_titled(ui, "Remove");
                            tune_mark(ui, 28.0, true, "Settings");
                        },
                    );
                    let mut v = 3_u32;
                    number_box(ui, egui::DragValue::new(&mut v));
                    let mut text = String::new();
                    text_area(ui, &mut text, "hint", 96.0, 14);
                });
            });
            secret_band(&palette(ui.ctx())).show(ui, |ui| {
                ui.label("secret");
            });
        })
    }

    #[test]
    fn the_form_furniture_draws_in_light_and_dark() {
        for p in [Palette::LIGHT, Palette::DARK] {
            let ctx = ctx();
            set_palette(&ctx, p);
            draw_all(&ctx, 0.0);
            let painted = draw_all(&ctx, 0.1);
            assert!(
                painted.rects.iter().any(|r| r.fill == p.card),
                "the card is not filled with the palette's card colour"
            );
            assert!(
                painted.rects.iter().any(|r| r.fill == p.danger_wash),
                "the secret band is not the palette's danger wash"
            );
        }
    }
}

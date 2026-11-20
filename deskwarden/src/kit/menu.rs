//! The v2 design's menu: the popover, its rows, and the select that opens one.
//!
//! **Every dropdown, "more" menu, context menu and select in the app is this
//! module.** Before it each one was egui's default -- a grey-framed button per
//! row, a 7-point corner, a filled triangle on the closed box -- or a copy of
//! the design's popover hand-built where somebody noticed (the language picker,
//! the sign-in steps' add menu). `kit::menu` is the one copy, and
//! `every_menu_in_src_goes_through_the_kit` (in the tests below) fails the
//! build when anything else opens an egui menu.
//!
//! # The numbers (`docs/design/Deskwarden v2.dc.html`, the add-step menu and the
//! avatar image menu, which are the design's two popovers)
//!
//! * Surface: white, radius 16 (the avatar menu, with its two-line rows,
//!   18), padding 6 (8), ring `0 0 0 1px rgba(20,26,48,.06)` and shadow
//!   `0 18px 40px rgba(20,26,48,.2)` (`0 18px 44px .22`). Opens 8 below the
//!   thing that opened it. Enters with `dw-drop`, which egui's `Area` fade
//!   stands in for.
//! * Row: height 42, radius 11, padding `0 12`, gap 11, 15/600, icon 16.
//!   Hover is the nav hover wash. The answer in force is the blue wash with
//!   blue-deep ink and a check at the right (the language picker's rule, now
//!   everyone's). A danger row is the error red on the danger wash.
//! * The design draws no select; the closed box is the app's field (height 50,
//!   radius 14, 16pt) or its row control (42, 12, 15/600) with a drawn
//!   chevron, and its list is the same popover at the box's width.
//!
//! # Keyboard
//!
//! Escape closes (egui's popup); Down / Up with nothing focused lands on the
//! first / last row; Down / Up between rows is egui's focus movement, and Enter
//! or Space on a focused row is a click. A focused row is painted as a hovered
//! one.

use eframe::egui::{
    self, Color32, CornerRadius, InnerResponse, Key, Margin, Modifiers, Pos2, Rect, Response, Sense, Shadow, Stroke,
    StrokeKind, Ui, Vec2,
};

use super::icon::{self, glyph};
use super::tokens::{self, Palette};

/// A popover's surface: its corner, its padding and how deep its shadow is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub radius: u8,
    pub pad: i8,
    pub blur: u8,
    /// The shadow's alpha, of 255.
    pub shadow_alpha: u8,
    /// The narrowest the popover may be, outside edge to outside edge; `0.0`
    /// is "as wide as what is in it".
    pub min_width: f32,
}

/// The add-step menu: the design's ordinary popover.
pub const MENU: Look = Look { radius: 16, pad: 6, blur: 40, shadow_alpha: 51, min_width: 240.0 };
/// The avatar image menu: rows with a second line, so everything a little bigger.
pub const RICH: Look = Look { radius: 18, pad: 8, blur: 44, shadow_alpha: 56, min_width: 300.0 };

impl Look {
    /// The same surface, as wide as its content.
    pub const fn fit(self) -> Look {
        Look { min_width: 0.0, ..self }
    }

    /// The same surface at least `width` wide.
    pub const fn at_least(self, width: f32) -> Look {
        Look { min_width: width, ..self }
    }

    /// The shadow `0 18px {blur}px rgba(20,26,48,{alpha})`.
    pub fn shadow(self, palette: &Palette) -> Shadow {
        let (r, g, b) = if is_light(palette) { (20, 26, 48) } else { (0, 0, 0) };
        let alpha = if is_light(palette) { self.shadow_alpha } else { 130 };
        Shadow { offset: [0, 18], blur: self.blur, spread: 0, color: Color32::from_rgba_unmultiplied(r, g, b, alpha) }
    }

    /// The popover's frame.
    pub fn frame(self, palette: &Palette) -> egui::Frame {
        egui::Frame::new()
            .fill(palette.card)
            .corner_radius(CornerRadius::same(self.radius))
            .stroke(Stroke::new(1.0, ring(palette)))
            .shadow(self.shadow(palette))
            .inner_margin(Margin::same(self.pad))
    }

    /// What `egui::Popup::style` is handed: the frame's numbers as egui reads
    /// them (`Frame::menu`), the rows' gap, and the app's scroll bars.
    pub fn style(self, palette: Palette) -> egui::style::StyleModifier {
        egui::style::StyleModifier::new(move |style| {
            style.visuals.window_fill = palette.card;
            style.visuals.window_stroke = Stroke::new(1.0, ring(&palette));
            style.visuals.menu_corner_radius = CornerRadius::same(self.radius);
            style.visuals.popup_shadow = self.shadow(&palette);
            style.spacing.menu_margin = Margin::same(self.pad);
            style.spacing.item_spacing.y = ROW_GAP_Y;
            style.spacing.scroll = crate::theme::scroll_style();
        })
    }
}

/// The ring round the surface: `rgba(20,26,48,.06)` on white.
pub fn ring(palette: &Palette) -> Color32 {
    if is_light(palette) {
        Color32::from_rgba_unmultiplied(20, 26, 48, 15)
    } else {
        palette.hairline
    }
}

fn is_light(palette: &Palette) -> bool {
    palette.card == Palette::LIGHT.card
}

/// Rows are 2 apart in the design's avatar menu.
pub const ROW_GAP_Y: f32 = 2.0;
/// A popover opens this far below its anchor.
pub const ANCHOR_GAP: f32 = 8.0;
pub const ROW_HEIGHT: f32 = 42.0;
pub const ROW_RADIUS: u8 = 11;
pub const ROW_PAD_X: f32 = 12.0;
/// The icon column and the label, and the label and a trailing mark.
pub const ROW_GAP: f32 = 11.0;
pub const ICON_SIZE: f32 = 16.0;
/// Between a label and its right-aligned hint.
const HINT_GAP: f32 = 24.0;
const SEPARATOR_HEIGHT: f32 = 9.0;

// ---------------------------------------------------------------------------
// The popover
// ---------------------------------------------------------------------------

/// Opens a popover under `anchor` while it is open, clicking `anchor` toggles
/// it. `add` draws the rows, with [`item`], [`separator`], [`submenu`] and
/// [`note`] -- or, for a menu that holds a text box or a palette of chips, with
/// whatever the design draws, in the same surface.
pub fn popover<R>(
    anchor: &Response,
    close: egui::PopupCloseBehavior,
    look: Look,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<InnerResponse<R>> {
    let palette = tokens::palette(&anchor.ctx);
    egui::Popup::menu(anchor)
        .close_behavior(close)
        .style(look.style(palette))
        .gap(ANCHOR_GAP)
        .show(|ui| rows(ui, look, add))
}

/// [`popover`] closing on any click inside it, at the ordinary [`MENU`] look:
/// what a "more" menu is.
pub fn menu<R>(anchor: &Response, add: impl FnOnce(&mut Ui) -> R) -> Option<InnerResponse<R>> {
    popover(anchor, egui::PopupCloseBehavior::CloseOnClick, MENU, add)
}

/// A right-click menu on `response`, opening at the pointer.
pub fn context<R>(response: &Response, add: impl FnOnce(&mut Ui) -> R) -> Option<InnerResponse<R>> {
    let palette = tokens::palette(&response.ctx);
    egui::Popup::context_menu(response)
        .style(MENU.style(palette))
        .show(|ui| rows(ui, MENU, add))
}

impl Look {
    /// The width inside the surface when it is at its [`Look::min_width`]:
    /// what a caller that lays its own `Area` sets on its `Ui`.
    pub fn content_width(self) -> f32 {
        content_min_width(self)
    }
}

/// A menu held open at `at`, with the same surface and rows a right-click
/// menu has: for `ui_preview`, which has no pointer to right-click with.
pub fn show_at<R>(
    ctx: &egui::Context,
    id: egui::Id,
    at: Pos2,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<InnerResponse<R>> {
    let palette = tokens::palette(ctx);
    egui::Popup::new(id, ctx.clone(), at, egui::LayerId::new(egui::Order::Foreground, id))
        .kind(egui::PopupKind::Menu)
        .open(true)
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .style(MENU.style(palette))
        .show(|ui| rows(ui, MENU, add))
}

fn preview_key() -> egui::Id {
    egui::Id::new("kit-menu-preview-open-select")
}

/// Opens the first select this frame draws whose salt, printed with `{:?}`,
/// starts with `prefix`: for `ui_preview`, which has no pointer.
pub fn open_select_for_preview(ctx: &egui::Context, prefix: &str) {
    ctx.data_mut(|d| d.insert_temp(preview_key(), prefix.to_owned()));
}

fn content_min_width(look: Look) -> f32 {
    if look.min_width <= 0.0 {
        0.0
    } else {
        (look.min_width - 2.0 * (f32::from(look.pad) + 1.0)).max(0.0)
    }
}

fn rows<R>(ui: &mut Ui, look: Look, add: impl FnOnce(&mut Ui) -> R) -> R {
    // Not egui's justified layout: in a menu's first pass that stretches every
    // row to the 600-point default area. Rows are as wide as the widest row
    // of the frame before (`row_width`), which settles in one pass.
    let layout = egui::Layout::top_down(egui::Align::Min);
    ui.scope_builder(egui::UiBuilder::new().layout(layout), |ui| {
        ui.set_min_width(content_min_width(look));
        let key = rows_key(ui.layer_id());
        ui.data_mut(|d| d.remove::<Vec<egui::Id>>(key));
        let widest = width_key(ui.layer_id());
        let seen = ui.data_mut(|d| d.remove_temp::<f32>(widest.with("now"))).unwrap_or(0.0);
        ui.data_mut(|d| d.insert_temp(widest, seen));
        let out = add(ui);
        arrow_into_rows(ui, key);
        out
    })
    .inner
}

fn width_key(layer: egui::LayerId) -> egui::Id {
    egui::Id::new(("kit-menu-width", layer))
}

/// A row's width: its own, or the widest row's of the last frame.
fn row_width(ui: &Ui, wanted: f32) -> f32 {
    let key = width_key(ui.layer_id());
    let last = ui.data(|d| d.get_temp::<f32>(key)).unwrap_or(0.0);
    ui.data_mut(|d| {
        let now = d.get_temp_mut_or_default::<f32>(key.with("now"));
        *now = now.max(wanted);
    });
    wanted.max(last).max(ui.min_size().x)
}

fn rows_key(layer: egui::LayerId) -> egui::Id {
    egui::Id::new(("kit-menu-rows", layer))
}

/// Down or Up with nothing focused lands on the first or last row; after that
/// egui's own focus movement walks the rows.
fn arrow_into_rows(ui: &mut Ui, key: egui::Id) {
    let rows: Vec<egui::Id> = ui.data_mut(|d| d.remove_temp(key)).unwrap_or_default();
    if rows.is_empty() || ui.memory(|m| m.focused()).is_some() {
        return;
    }
    let target = if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
        rows.first()
    } else if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
        rows.last()
    } else {
        None
    };
    if let Some(&id) = target {
        ui.memory_mut(|m| m.request_focus(id));
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One row of a menu.
#[derive(Clone, Copy, Debug)]
pub struct Item<'a> {
    pub label: &'a str,
    /// A glyph from [`glyph`], drawn at [`ICON_SIZE`] in the row's ink.
    pub icon: Option<&'a str>,
    /// A chord, right-aligned and faint ("Ctrl+L").
    pub hint: Option<&'a str>,
    /// The answer in force: blue wash, blue-deep ink, a check.
    pub selected: bool,
    /// The row that deletes: the error red, on the danger wash when pointed at.
    pub danger: bool,
    /// `false` greys the row and gives it no click. Its reason goes on the
    /// returned response with `on_hover_text`.
    pub enabled: bool,
    /// A stable id for the row, for tests and callers that look its rect up.
    pub id: Option<egui::Id>,
    /// Ink for a row that is not danger but is not ordinary either (the add
    /// menu's secret steps are `#8c3c33`).
    pub ink: Option<Color32>,
}

impl<'a> Item<'a> {
    pub fn new(label: &'a str) -> Self {
        Item { label, icon: None, hint: None, selected: false, danger: false, enabled: true, id: None, ink: None }
    }
    pub fn ink(mut self, ink: Color32) -> Self {
        self.ink = Some(ink);
        self
    }
    pub fn id(mut self, id: egui::Id) -> Self {
        self.id = Some(id);
        self
    }
    pub fn icon(mut self, glyph: &'a str) -> Self {
        self.icon = Some(glyph);
        self
    }
    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = Some(hint);
        self
    }
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// The colours of a row: `(label ink, fill when pointed at or focused)`.
pub fn row_colours(palette: &Palette, item: &Item<'_>) -> (Color32, Color32) {
    if !item.enabled {
        (palette.text_ghost, Color32::TRANSPARENT)
    } else if item.danger {
        (palette.error, palette.danger_wash)
    } else if item.selected {
        (palette.blue_deep, palette.blue_wash)
    } else {
        (item.ink.unwrap_or(palette.ink), palette.hover)
    }
}

fn galley(ui: &Ui, role: tokens::Role, text: &str, color: Color32) -> std::sync::Arc<egui::Galley> {
    ui.fonts_mut(|f| f.layout_job(tokens::job(role, text, color)))
}

/// Paints one row into `rect` and answers its response. `trailing` is the
/// right-hand mark: a check, a chevron, or nothing.
fn paint_row(ui: &mut Ui, item: Item<'_>, trailing: Option<&str>) -> Response {
    let palette = tokens::palette(ui.ctx());
    let (ink, wash) = row_colours(&palette, &item);
    let label = galley(ui, tokens::NAV, item.label, ink);
    let hint = item.hint.map(|h| galley(ui, tokens::HINT, h, palette.text_faint));
    let lead = if item.icon.is_some() { ICON_SIZE + ROW_GAP } else { 0.0 };
    let tail = if trailing.is_some() { ROW_GAP + ICON_SIZE } else { 0.0 };
    let hint_width = hint.as_ref().map_or(0.0, |h| HINT_GAP + h.size().x);
    let wanted = ROW_PAD_X * 2.0 + lead + label.size().x + hint_width + tail;
    let sense = if item.enabled { Sense::click() } else { Sense::hover() };
    let size = Vec2::new(row_width(ui, wanted), ROW_HEIGHT);
    let (rect, response) = match item.id {
        Some(id) => {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            (rect, ui.interact(rect, id, sense))
        }
        None => ui.allocate_exact_size(size, sense),
    };
    if item.enabled {
        let key = rows_key(ui.layer_id());
        ui.data_mut(|d| d.get_temp_mut_or_default::<Vec<egui::Id>>(key).push(response.id));
    }
    let pointed = item.enabled && (response.hovered() || response.has_focus());
    if pointed {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // Always painted, transparent at rest: the row's hit area is then a shape
    // a test (or a reader of the frame) can find, as a framed button's was.
    let fill = if pointed || (item.selected && item.enabled) { wash } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, CornerRadius::same(ROW_RADIUS), fill);
    let painter = ui.painter();
    let mut x = rect.left() + ROW_PAD_X;
    if let Some(d) = item.icon {
        let at = Rect::from_center_size(Pos2::new(x + ICON_SIZE / 2.0, rect.center().y), Vec2::splat(ICON_SIZE));
        icon::stroke(painter, at, d, 2.0, ink);
        x += ICON_SIZE + ROW_GAP;
    }
    painter.galley(Pos2::new(x, rect.center().y - label.size().y / 2.0), label, ink);
    let mut right = rect.right() - ROW_PAD_X;
    if let Some(d) = trailing {
        let at = Rect::from_center_size(Pos2::new(right - ICON_SIZE / 2.0, rect.center().y), Vec2::splat(ICON_SIZE));
        icon::stroke(painter, at, d, 2.4, ink);
        right -= ICON_SIZE + ROW_GAP;
    }
    if let Some(hint) = hint {
        let at = Pos2::new(right - hint.size().x, rect.center().y - hint.size().y / 2.0);
        painter.galley(at, hint, palette.text_faint);
    }
    response
}

/// One row. Answers its response: the caller reads `.clicked()`.
pub fn item(ui: &mut Ui, item: Item<'_>) -> Response {
    let check = (item.selected && item.enabled).then_some(glyph::CHECK);
    paint_row(ui, item, check)
}

/// A row that opens a submenu to its right while it is pointed at.
pub fn submenu<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> Option<InnerResponse<R>> {
    let response = paint_row(ui, Item::new(label), Some(glyph::CHEVRON_RIGHT));
    egui::containers::menu::SubMenu::new().show(ui, &response, |ui| rows(ui, MENU.fit(), add))
}

/// A hairline between groups of rows.
pub fn separator(ui: &mut Ui) {
    let palette = tokens::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, SEPARATOR_HEIGHT), Sense::hover());
    let y = rect.center().y;
    ui.painter().hline(rect.x_range(), y, Stroke::new(1.0, palette.row_rule));
}

/// A line of small print under the rows (why a menu is empty, what a record
/// being read-only means): 12pt, ghost, wrapped to the menu's width.
pub fn note(ui: &mut Ui, text: &str) {
    let palette = tokens::palette(ui.ctx());
    egui::Frame::new().inner_margin(Margin::symmetric(ROW_PAD_X as i8, 8)).show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(palette.text_ghost)).wrap());
    });
}

// ---------------------------------------------------------------------------
// The select
// ---------------------------------------------------------------------------

/// One entry of a [`select`].
#[derive(Clone, Copy, Debug)]
pub struct Choice<'a> {
    pub label: &'a str,
    /// The answer in force.
    pub selected: bool,
    /// `false` shows the row greyed and unclickable.
    pub enabled: bool,
}

impl<'a> Choice<'a> {
    pub fn new(label: &'a str, selected: bool) -> Self {
        Choice { label, selected, enabled: true }
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// The closed box's size and type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectLook {
    pub height: f32,
    pub radius: u8,
    pub pad_x: f32,
    pub role: tokens::Role,
}

/// The select in a form: the app's field.
pub const FIELD: SelectLook = SelectLook {
    height: 50.0,
    radius: 14,
    pad_x: 14.0,
    role: tokens::Role { size: 16.0, face: crate::theme::REGULAR, tracking_em: 0.0 },
};
/// The select in a row of settings or a composer: the app's row control.
pub const ROW: SelectLook = SelectLook {
    height: 42.0,
    radius: 12,
    pad_x: 14.0,
    role: tokens::Role { size: 15.0, face: crate::theme::SEMIBOLD, tracking_em: 0.0 },
};

/// The chevron's box: 12 wide, 6 tall, `M6 9l6 6 6-6`.
const CHEVRON: (f32, f32) = (CHEVRON_WIDTH, 6.0);
/// The chevron's width.
pub const CHEVRON_WIDTH: f32 = 12.0;
/// The tallest a select's list grows before it scrolls.
pub const SELECT_MAX_HEIGHT: f32 = 420.0;

impl SelectLook {
    /// What is left of a box `width` wide for its text.
    pub fn text_room(&self, width: f32) -> f32 {
        (width - self.pad_x * 2.0 - CHEVRON.0 - ROW_GAP).max(0.0)
    }
}

/// The closed select box painted into `rect`: field fill, the border (blue
/// while open, the pointed-at edge on hover), the value and a drawn chevron.
pub fn paint_trigger(ui: &Ui, rect: Rect, look: SelectLook, text: &str, open: bool, hovered: bool) {
    let palette = tokens::palette(ui.ctx());
    let border = if open {
        Stroke::new(1.5, palette.blue)
    } else if hovered {
        Stroke::new(1.5, palette.blue_edge)
    } else {
        Stroke::new(1.5, palette.border)
    };
    ui.painter().rect(rect, CornerRadius::same(look.radius), palette.card, border, StrokeKind::Inside);
    let mut job = tokens::job(look.role, text, palette.ink);
    job.wrap = egui::text::TextWrapping::truncate_at_width(look.text_room(rect.width()));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter().galley(
        Pos2::new(rect.left() + look.pad_x, rect.center().y - galley.size().y / 2.0),
        galley,
        palette.ink,
    );
    let centre = Pos2::new(rect.right() - look.pad_x - CHEVRON.0 / 2.0, rect.center().y);
    let at = Rect::from_center_size(centre, Vec2::new(CHEVRON.0, CHEVRON.0));
    icon::stroke(ui.painter(), at, CHEVRON_DOWN, 2.2, palette.text_muted);
}

/// The chevron, in the icons' 24-unit box.
const CHEVRON_DOWN: &str = "M6 9l6 6 6-6";

/// A closed select box as a button: for a control that looks like a select
/// but opens something that is not a list (the bank picker's chooser).
pub fn trigger(ui: &mut Ui, look: SelectLook, width: f32, text: &str, open: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, look.height), Sense::click());
    paint_trigger(ui, rect, look, text, open, response.hovered());
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A closed select that cannot be opened: its answer at full strength on the
/// quiet fill, no chevron, no click.
pub fn select_disabled(ui: &mut Ui, look: SelectLook, width: f32, text: &str) {
    let palette = tokens::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, look.height), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(look.radius),
        palette.list_bg,
        Stroke::new(1.5, palette.hairline),
        StrokeKind::Inside,
    );
    let mut job = tokens::job(look.role, text, palette.text_muted);
    job.wrap = egui::text::TextWrapping::truncate_at_width((width - look.pad_x * 2.0).max(0.0));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter().galley(
        Pos2::new(rect.left() + look.pad_x, rect.center().y - galley.size().y / 2.0),
        galley,
        palette.text_muted,
    );
}

/// A select: the closed box showing `current`, and under it a popover of
/// `choices` at the box's width. Answers the index chosen this frame.
///
/// `id_salt` keys the box (and so its open state): give it something that
/// does not shift when a sibling goes away.
pub fn select(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    width: f32,
    look: SelectLook,
    current: &str,
    choices: &[Choice<'_>],
) -> Option<usize> {
    let salt_text = format!("{id_salt:?}");
    let id = ui.id().with(("kit-select", id_salt));
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, look.height), Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    let preview: Option<String> = ui.data(|d| d.get_temp(preview_key()));
    if preview.is_some_and(|prefix| salt_text.starts_with(&prefix)) {
        ui.data_mut(|d| d.remove::<String>(preview_key()));
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&response));
    }
    if response.has_focus() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&response));
    }
    let mut chosen = None;
    let look_of_list = MENU.at_least(width);
    let open = popover(&response, egui::PopupCloseBehavior::CloseOnClick, look_of_list, |ui| {
        ui.set_max_width(look_of_list.content_width());
        egui::ScrollArea::vertical().max_height(SELECT_MAX_HEIGHT).auto_shrink([false, true]).show(ui, |ui| {
            for (index, choice) in choices.iter().enumerate() {
                if item(ui, Item::new(choice.label).selected(choice.selected).enabled(choice.enabled)).clicked() {
                    chosen = Some(index);
                    ui.close();
                }
            }
        });
    })
    .is_some();
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    paint_trigger(ui, rect, look, current, open, response.hovered());
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{click_at, ctx, frame, hover_at};

    const SCREEN: Vec2 = Vec2::new(600.0, 700.0);

    fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(r, g, b, a)
    }

    #[test]
    fn the_popover_is_the_designs_numbers() {
        let p = Palette::LIGHT;
        assert_eq!((MENU.radius, MENU.pad, MENU.blur), (16, 6, 40));
        assert_eq!((RICH.radius, RICH.pad, RICH.blur), (18, 8, 44));
        assert_eq!(MENU.shadow(&p), Shadow { offset: [0, 18], blur: 40, spread: 0, color: rgba(20, 26, 48, 51) });
        assert_eq!(RICH.shadow(&p).color, rgba(20, 26, 48, 56));
        assert_eq!(ring(&p), rgba(20, 26, 48, 15));
        assert_eq!(ANCHOR_GAP, 8.0);
        let frame = MENU.frame(&p);
        assert_eq!(frame.fill, p.card);
        assert_eq!(frame.corner_radius, CornerRadius::same(16));
        assert_eq!(frame.inner_margin, Margin::same(6));
        assert_eq!(frame.stroke, Stroke::new(1.0, rgba(20, 26, 48, 15)));
    }

    #[test]
    fn a_row_is_the_designs_numbers() {
        assert_eq!((ROW_HEIGHT, ROW_RADIUS, ROW_PAD_X, ROW_GAP, ICON_SIZE), (42.0, 11, 12.0, 11.0, 16.0));
        assert_eq!(tokens::NAV.size, 15.0);
        assert_eq!(tokens::NAV.face, crate::theme::SEMIBOLD);
        assert_eq!(ROW_GAP_Y, 2.0);
        // 240 outside the surface, which is 228 inside its 6 of padding and
        // 1 of ring each side... the ring is the frame's stroke.
        assert_eq!(content_min_width(MENU), 240.0 - 2.0 * 7.0);
    }

    #[test]
    fn row_colours_are_the_designs() {
        let p = Palette::LIGHT;
        let plain = Item::new("Edit");
        assert_eq!(row_colours(&p, &plain), (p.ink, p.hover));
        assert_eq!(row_colours(&p, &plain.selected(true)), (p.blue_deep, p.blue_wash));
        assert_eq!(row_colours(&p, &plain.danger(true)), (p.error, p.danger_wash));
        assert_eq!(row_colours(&p, &plain.enabled(false)).0, p.text_ghost);
    }

    #[test]
    fn the_select_boxes_are_the_apps_field_and_row_control() {
        assert_eq!((FIELD.height, FIELD.radius, FIELD.role.size), (50.0, 14, 16.0));
        assert_eq!((ROW.height, ROW.radius, ROW.role.size), (42.0, 12, 15.0));
    }

    /// The painted rows of a one-item-per-line popover under a 40x30 anchor.
    fn open_menu(ctx: &egui::Context, labels: &[&str], danger: Option<&str>, at: Option<Pos2>) -> crate::kit::testing::Painted {
        let labels: Vec<String> = labels.iter().map(|s| (*s).to_owned()).collect();
        let danger = danger.map(str::to_owned);
        let run = |events: Vec<egui::Event>| {
            frame(ctx, 0.0, SCREEN, events, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(40.0, 30.0), Sense::hover());
                let anchor = ui.interact(rect, egui::Id::new("anchor"), Sense::click());
                menu(&anchor, |ui| {
                    for l in &labels {
                        item(ui, Item::new(l).danger(danger.as_deref() == Some(l.as_str())));
                    }
                });
            })
        };
        for _ in 0..2 {
            run(vec![]);
        }
        let _ = at;
        run(click_at(Pos2::new(20.0, 15.0)));
        run(vec![]);
        run(vec![])
    }

    #[test]
    fn an_open_menu_paints_the_popover_surface_and_42_point_rows() {
        let ctx = ctx();
        let painted = open_menu(&ctx, &["Edit", "Clone", "Delete"], Some("Delete"), None);
        let surface = painted
            .rects
            .iter()
            .find(|r| r.corner_radius == CornerRadius::same(16) && r.stroke.width == 1.0)
            .unwrap_or_else(|| panic!("no popover surface in {:?}", painted.rects));
        assert_eq!(surface.stroke.width, 1.0);
        let edit = painted.text("Edit").expect("Edit").2;
        let clone = painted.text("Clone").expect("Clone").2;
        assert!((clone.center().y - edit.center().y - (ROW_HEIGHT + ROW_GAP_Y)).abs() < 1.5, "{edit:?} {clone:?}");
        // The surface is at least 240 wide, and the label starts 12 in.
        assert!(surface.rect.width() >= 240.0 - 0.5, "{:?}", surface.rect);
        let delete = painted.text("Delete").expect("Delete");
        assert_eq!(delete.1, Palette::LIGHT.error, "a danger row is the error red");
        let ink = painted.text("Edit").unwrap().1;
        assert_eq!(ink, Palette::LIGHT.ink);
        // The surface hangs ANCHOR_GAP under the anchor (30 tall at the top).
        assert!((surface.rect.top() - (30.0 + ANCHOR_GAP)).abs() < 1.5, "{:?}", surface.rect);
    }

    #[test]
    fn the_row_under_the_pointer_takes_the_hover_wash() {
        let ctx = ctx();
        let _ = open_menu(&ctx, &["Edit", "Clone"], None, None);
        let painted = frame(&ctx, 0.1, SCREEN, vec![], |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(40.0, 30.0), Sense::hover());
            let anchor = ui.interact(rect, egui::Id::new("anchor"), Sense::click());
            menu(&anchor, |ui| {
                item(ui, Item::new("Edit"));
                item(ui, Item::new("Clone"));
            });
        });
        let clone = painted.text("Clone").expect("Clone").2;
        let painted = frame(&ctx, 0.2, SCREEN, hover_at(clone.center()), |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(40.0, 30.0), Sense::hover());
            let anchor = ui.interact(rect, egui::Id::new("anchor"), Sense::click());
            menu(&anchor, |ui| {
                item(ui, Item::new("Edit"));
                item(ui, Item::new("Clone"));
            });
        });
        let washes: Vec<_> = painted
            .rects
            .iter()
            .filter(|r| r.fill == Palette::LIGHT.hover && r.corner_radius == CornerRadius::same(ROW_RADIUS))
            .collect();
        assert_eq!(washes.len(), 1, "exactly the pointed-at row is washed");
        assert!((washes[0].rect.height() - ROW_HEIGHT).abs() < 0.5);
        assert!(washes[0].rect.contains(clone.center()));
    }

    #[test]
    fn a_selected_row_is_blue_wash_blue_ink_and_a_check() {
        let ctx = ctx();
        let draw = |ui: &mut Ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(40.0, 30.0), Sense::hover());
            let anchor = ui.interact(rect, egui::Id::new("anchor"), Sense::click());
            menu(&anchor, |ui| {
                item(ui, Item::new("Hyphen").selected(true));
                item(ui, Item::new("Space"));
            });
        };
        for _ in 0..2 {
            frame(&ctx, 0.0, SCREEN, vec![], draw);
        }
        frame(&ctx, 0.0, SCREEN, click_at(Pos2::new(20.0, 15.0)), draw);
        frame(&ctx, 0.1, SCREEN, vec![], draw);
        let painted = frame(&ctx, 0.2, SCREEN, vec![], draw);
        let p = Palette::LIGHT;
        assert!(painted.rects.iter().any(|r| r.fill == p.blue_wash && r.corner_radius == CornerRadius::same(ROW_RADIUS)));
        assert_eq!(painted.text("Hyphen").unwrap().1, p.blue_deep);
        assert_eq!(painted.text("Space").unwrap().1, p.ink);
    }

    #[test]
    fn a_select_shows_its_value_in_a_field_box_and_opens_a_list_the_boxes_width() {
        let ctx = ctx();
        let choices = [Choice::new("One", false), Choice::new("Two", true)];
        let mut chosen = None;
        let run = |events: Vec<egui::Event>, chosen: &mut Option<usize>| {
            frame(&ctx, 0.0, SCREEN, events, |ui| {
                ui.add_space(10.0);
                *chosen = chosen.or(select(ui, "pick", 200.0, FIELD, "Two", &choices));
            })
        };
        for _ in 0..3 {
            run(vec![], &mut chosen);
        }
        let closed = run(vec![], &mut chosen);
        let boxr = closed
            .rects
            .iter()
            .find(|r| r.corner_radius == CornerRadius::same(14))
            .expect("the closed box");
        assert!((boxr.rect.height() - 50.0).abs() < 0.5 && (boxr.rect.width() - 200.0).abs() < 0.5, "{:?}", boxr.rect);
        assert_eq!(boxr.fill, Palette::LIGHT.card);
        assert!(closed.text("Two").is_some());
        run(click_at(boxr.rect.center()), &mut chosen);
        run(vec![], &mut chosen);
        let open = run(vec![], &mut chosen);
        let one = open.text("One").expect("the list is open").2;
        let surface = open
            .rects
            .iter()
            .find(|r| r.corner_radius == CornerRadius::same(16) && r.stroke.width == 1.0)
            .expect("the list's surface")
            .rect;
        assert!((surface.width() - 200.0).abs() < 1.5, "the list is the box's width: {surface:?}");
        assert!(surface.top() > boxr.rect.bottom(), "and sits under it");
        run(click_at(one.center()), &mut chosen);
        assert_eq!(chosen, Some(0));
    }

    /// Reads `src/` once: every menu the app opens must be this module's.
    #[test]
    fn every_menu_in_src_goes_through_the_kit() {
        use std::path::Path;
        let needles = [
            concat!("Combo", "Box"),
            concat!("menu", "_button("),
            concat!("context", "_menu("),
            concat!("Popup::", "menu("),
            concat!("Popup::", "context_menu("),
            concat!("popup_", "below_widget"),
            concat!("selectable", "_value("),
        ];
        fn walk(dir: &Path, hits: &mut Vec<String>, needles: &[&str]) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, hits, needles);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let name = path.file_name().unwrap().to_string_lossy().to_string();
                    // This module is the one door; the picker's own Win32 and
                    // GDI surfaces, the tray's native menu and browser_url's UI
                    // Automation control kind are not egui menus.
                    if matches!(name.as_str(), "menu.rs" | "tray.rs" | "browser_url.rs" | "ui_automation.rs" | "main.rs") {
                        continue;
                    }
                    let text = std::fs::read_to_string(&path).unwrap();
                    // Only code: skip `//` lines, which explain what was replaced.
                    for (n, line) in text.lines().enumerate() {
                        let code = line.trim_start();
                        if code.starts_with("//") {
                            continue;
                        }
                        for needle in needles {
                            if line.contains(needle) {
                                hits.push(format!("{}:{}: {}", path.display(), n + 1, code));
                            }
                        }
                    }
                }
            }
        }
        let mut hits = Vec::new();
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits, &needles);
        assert!(hits.is_empty(), "these open an egui menu without kit::menu:\n{}", hits.join("\n"));
    }

    #[test]
    fn a_right_click_opens_the_context_menu_in_the_kit_surface() {
        let ctx = ctx();
        let draw = |ui: &mut Ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(200.0, 40.0), Sense::click());
            let row = ui.interact(rect, egui::Id::new("row"), Sense::click());
            context(&row, |ui| {
                item(ui, Item::new("Copy"));
                item(ui, Item::new("Delete").danger(true));
            });
        };
        for _ in 0..2 {
            frame(&ctx, 0.0, SCREEN, vec![], draw);
        }
        let at = Pos2::new(50.0, 20.0);
        let secondary = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&ctx, 1.0, SCREEN, vec![egui::Event::PointerMoved(at), secondary(true), secondary(false)], draw);
        frame(&ctx, 2.0, SCREEN, vec![], draw);
        let painted = frame(&ctx, 3.0, SCREEN, vec![], draw);
        assert!(painted.text("Copy").is_some(), "the context menu did not open");
        assert_eq!(painted.text("Delete").unwrap().1, Palette::LIGHT.error);
        assert!(painted.rects.iter().any(|r| r.corner_radius == CornerRadius::same(16) && r.stroke.width == 1.0));
    }

    #[test]
    fn the_scroll_bar_style_is_pinned() {
        let s = crate::theme::scroll_style();
        assert!(s.floating, "floating: it takes no room");
        assert_eq!((s.floating_width, s.bar_width), (6.0, 10.0), "6 idle, 10 under the pointer");
        assert_eq!(s.floating_allocated_width, 0.0);
        assert_eq!(s.bar_outer_margin, 3.0, "stands in from the edge");
        assert!(s.foreground_color, "the handle is ink, not the white track colour");
        assert_eq!(
            (s.dormant_background_opacity, s.active_background_opacity, s.interact_background_opacity),
            (0.0, 0.0, 0.0),
            "no track"
        );
        assert_eq!(
            (s.dormant_handle_opacity, s.active_handle_opacity, s.interact_handle_opacity),
            (0.0, 0.4, 0.7)
        );
    }

    #[test]
    fn the_theme_sets_the_scroll_bar_and_the_menu_defaults_globally() {
        let ctx = ctx();
        let style = ctx.global_style();
        assert_eq!(style.spacing.scroll, crate::theme::scroll_style());
        assert_eq!(style.visuals.menu_corner_radius, CornerRadius::same(16));
        assert_eq!(style.spacing.menu_margin, Margin::same(6));
        assert_eq!(style.visuals.popup_shadow, MENU.shadow(&Palette::LIGHT));
    }

    #[test]
    fn a_popup_style_modifier_applies_the_surface_and_scroll_bar() {
        let mut style = egui::Style::default();
        RICH.style(Palette::LIGHT).apply(&mut style);
        assert_eq!(style.visuals.menu_corner_radius, CornerRadius::same(18));
        assert_eq!(style.spacing.menu_margin, Margin::same(8));
        assert_eq!(style.spacing.scroll, crate::theme::scroll_style());
        assert_eq!(style.visuals.window_stroke, Stroke::new(1.0, rgba(20, 26, 48, 15)));
    }
}

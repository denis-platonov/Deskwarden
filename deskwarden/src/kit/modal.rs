//! Spec B §2: the v2 modal frame -- a scrim over the window that closes the
//! dialog when clicked, a radius-24 card centred on it (or set at a given
//! top), its title and subtitle, a 38pt close mark, and Esc.
//!
//! Each dialog names its scrim `"<id>-scrim"`; that name must be listed in
//! `vault_window::item_list::MODAL_SCRIM_AREAS` so the window knows a modal
//! is up.

use eframe::egui::{self, Color32, CornerRadius, Rect, Sense, Shadow, Vec2};

use crate::theme;

/// Whether the dialog is still up after this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalOutcome {
    Open,
    Closed,
}

/// `rgba(20,26,48,.38)`.
pub const SCRIM: Color32 = Color32::from_rgba_unmultiplied_const(20, 26, 48, 97);
/// The card's radius and inner padding.
pub const RADIUS: u8 = 24;
pub const PADDING: i8 = 30;
/// The close mark's square, and its inset from the card's top-right.
pub const CLOSE_SIZE: f32 = 38.0;
pub const CLOSE_INSET: f32 = 18.0;
/// The room a dialog keeps from the window's edges.
pub const GUTTER: f32 = 24.0;


/// The id of a dialog's close mark.
pub fn close_id(id: &str) -> egui::Id {
    egui::Id::new((id, "modal-close"))
}

/// The id of a dialog's card (its area).
pub fn card_id(id: &str) -> egui::Id {
    egui::Id::new((id, "modal-card"))
}

/// The name of a dialog's scrim area.
pub fn scrim_name(id: &str) -> String {
    format!("{id}-scrim")
}

/// One v2 dialog.
pub struct Dialog<'a> {
    id: &'a str,
    width: f32,
    title: Option<&'a str>,
    sub: Option<&'a str>,
    top: Option<f32>,
    closable: bool,
    padding: i8,
    radius: u8,
    tip: Option<(&'a str, f32)>,
    esc_closes: bool,
    modeless: bool,
}

impl<'a> Dialog<'a> {
    pub fn new(id: &'a str, width: f32) -> Self {
        Self { id, width, title: None, sub: None, top: None, closable: true, padding: PADDING, radius: RADIUS, tip: None, esc_closes: true, modeless: false }
    }

    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    // A builder setter for the card's sub-line, not `std::ops::Sub`.
    #[allow(clippy::should_implement_trait)]
    pub fn sub(mut self, sub: &'a str) -> Self {
        self.sub = Some(sub);
        self
    }

    /// A `?` dot after the title whose bubble opens below it (the QR modal's
    /// 20 px dot, spec B 0.4), `width` wide.
    pub fn title_tip(mut self, text: &'a str, width: f32) -> Self {
        self.tip = Some((text, width));
        self
    }

    /// Whether Esc closes the dialog (default yes). A dialog whose stage
    /// hands the keyboard to a window of its own turns it off for that stage.
    pub fn esc_closes(mut self, yes: bool) -> Self {
        self.esc_closes = yes;
        self
    }

    /// A card with no scrim and no outside click: it sits over the window and
    /// leaves it live. For a report that must not stand between the user and
    /// the next thing they press (the export report, the revoke banner); it
    /// closes only by its own button, and Esc is not taken.
    pub fn modeless(mut self) -> Self {
        self.modeless = true;
        self
    }

    /// Set the card `top` points below the window's top instead of centring
    /// it (the palette's 90).
    pub fn top(mut self, top: f32) -> Self {
        self.top = Some(top);
        self
    }

    /// No close mark (the palette); the scrim and Esc still close it.
    pub fn without_close(mut self) -> Self {
        self.closable = false;
        self
    }

    pub fn padding(mut self, padding: i8) -> Self {
        self.padding = padding;
        self
    }

    pub fn radius(mut self, radius: u8) -> Self {
        self.radius = radius;
        self
    }

    /// The card's width on this window: what was asked for, less any part
    /// that would come within [`GUTTER`] of an edge.
    pub fn width_on(&self, screen: Rect) -> f32 {
        self.width.min(screen.width() - 2.0 * GUTTER).max(0.0)
    }

    /// Draw it. `add` fills the card under the title.
    pub fn show<R>(self, ctx: &egui::Context, add: impl FnOnce(&mut egui::Ui) -> R) -> (ModalOutcome, R) {
        let screen = ctx.content_rect();
        let width = self.width_on(screen);

        let scrim_area = egui::Id::new(scrim_name(self.id));
        let scrim = if self.modeless {
            false
        } else {
        let scrim_widget = egui::Area::new(scrim_area)
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let response = ui.allocate_response(screen.size(), Sense::click());
                ui.painter().rect_filled(screen, CornerRadius::ZERO, SCRIM);
                response.clicked()
            })
            .inner;
        // Also asked of the layers directly: a click whose top layer is the
        // scrim closes the dialog, whatever the window's own passes did to the
        // widget's response.
        let (clicked, at) = ctx.input(|i| (i.pointer.any_click(), i.pointer.interact_pos()));
        let scrim_layer = egui::LayerId::new(egui::Order::Middle, scrim_area);
        // Not on the pass the dialog first appears: the click that opened it
        // is still in that pass's input, and it now lands on the new scrim.
        let pass = ctx.cumulative_pass_nr();
        let seen_id = scrim_area.with("last-pass");
        let last = ctx.data(|d| d.get_temp::<u64>(seen_id));
        ctx.data_mut(|d| d.insert_temp(seen_id, pass));
        let fresh = last.is_none_or(|last| last + 1 < pass);
        !fresh
            && (scrim_widget
                || (clicked && at.is_some_and(|at| ctx.memory(|m| m.layer_id_at(at)) == Some(scrim_layer))))
        };

        // `sense(click)`: the card takes the clicks that land on it, so they
        // never reach the scrim behind it.
        let mut area = egui::Area::new(card_id(self.id)).order(egui::Order::Foreground).sense(Sense::click());
        area = match self.top {
            Some(top) => area.anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, top)),
            None => area.anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO),
        };
        let p = crate::kit::tokens::palette(ctx);
        let (inner, closed_by_mark) = area
            .show(ctx, |ui| {
                let shown = egui::Frame::new()
                    .fill(p.card)
                    .corner_radius(CornerRadius::same(self.radius))
                    .shadow(Shadow { offset: [0, 30], blur: 80, spread: 0, color: Color32::from_rgba_unmultiplied(20, 26, 48, 89) })
                    .inner_margin(self.padding)
                    .show(ui, |ui| {
                        let inner_width = (width - 2.0 * f32::from(self.padding)).max(0.0);
                        ui.set_width(inner_width);
                        if let Some(title) = self.title {
                            // Room on the title's line for the close mark.
                            let reserve = if self.closable { 44.0 } else { 0.0 };
                            ui.allocate_ui(Vec2::new(inner_width - reserve, 0.0), |ui| {
                                let heading = egui::RichText::new(title)
                                    .font(egui::FontId::new(26.0, egui::FontFamily::Name(theme::EXTRABOLD.into())))
                                    .color(p.ink);
                                if let Some((text, bubble)) = self.tip {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        ui.label(heading);
                                        crate::kit::tips::help_dot_sized(
                                            ui,
                                            egui::Id::new((self.id, "title-tip")),
                                            text,
                                            bubble,
                                            crate::kit::tips::Placement::Below,
                                            20.0,
                                        );
                                    });
                                } else {
                                    ui.label(heading);
                                }
                                if let Some(sub) = self.sub {
                                    ui.label(egui::RichText::new(sub).size(16.0).color(p.text_faint));
                                }
                            });
                            ui.add_space(22.0);
                        }
                        add(ui)
                    });
                let mut closed = false;
                if self.closable {
                    let card = shown.response.rect;
                    let mark = Rect::from_min_size(
                        egui::pos2(card.right() - CLOSE_INSET - CLOSE_SIZE, card.top() + CLOSE_INSET),
                        Vec2::splat(CLOSE_SIZE),
                    );
                    let response = ui.interact(mark, close_id(self.id), Sense::click());
                    if response.hovered() {
                        ui.painter().rect_filled(mark, CornerRadius::same(12), p.hover);
                    }
                    crate::kit::icon::stroke(
                        ui.painter(),
                        Rect::from_center_size(mark.center(), Vec2::splat(16.0)),
                        crate::kit::icon::glyph::CLOSE,
                        2.0,
                        p.text_icon,
                    );
                    closed = response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                }
                (shown.inner, closed)
            })
            .inner;

        let esc = self.esc_closes && !self.modeless && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let outcome = if scrim || closed_by_mark || esc { ModalOutcome::Closed } else { ModalOutcome::Open };
        (outcome, inner)
    }
}

/// What a note under a dialog's body is telling the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Something went wrong or cannot be done: the danger wash.
    Danger,
    /// Something to be careful about: the amber wash.
    Caution,
}

/// The warning triangle the design draws in an amber row (spec B 7: 20 px).
pub const WARNING_GLYPH: &str = "M12 3 2 20h20zM12 10v5M12 17.5h.01";

/// A note in a dialog: a tinted box, radius 16 and a 1 px edge, the sentence
/// at 14 px in the tone's ink, and the triangle in front of a caution. Wraps
/// to the dialog's width.
pub fn note(ui: &mut egui::Ui, tone: Tone, text: &str) {
    let p = crate::kit::tokens::palette(ui.ctx());
    let (fill, edge, ink) = match tone {
        Tone::Danger => (p.danger_wash, p.danger_edge, p.error),
        Tone::Caution => (p.caution_wash, p.caution_edge, p.caution_ink),
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, edge))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                if tone == Tone::Caution {
                    let (slot, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                    crate::kit::icon::stroke(ui.painter(), slot, WARNING_GLYPH, 2.0, p.caution_mark);
                }
                ui.add(egui::Label::new(egui::RichText::new(text).size(14.0).color(ink)).wrap());
            });
        });
}

/// Which of a confirmation's two buttons was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// What a confirmation did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmed {
    Open,
    /// One of its two buttons.
    Pressed(Side),
    /// The scrim, the close mark or Esc.
    Dismissed,
}

/// The id of a confirmation's button, for tests and the window.
pub fn confirm_button_id(id: &str, side: Side) -> egui::Id {
    egui::Id::new((id, "confirm", side == Side::Left))
}

/// The v2 small confirmation: a dialog with a title, an optional line under
/// it, and two equal buttons (48 high, radius 14). The caller says which kind
/// each is; the destructive one is the red outline, the safe one the blue or
/// the grey.
pub fn confirm(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    title: &str,
    body: Option<&str>,
    left: (&str, crate::kit::controls::ButtonKind),
    right: (&str, crate::kit::controls::ButtonKind),
) -> Confirmed {
    use crate::kit::controls::{self, ButtonSize};
    let mut dialog = Dialog::new(id, width).title(title);
    if let Some(body) = body {
        dialog = dialog.sub(body);
    }
    let (outcome, pressed) = dialog.show(ctx, |ui| {
        let mut pressed = None;
        let size = ButtonSize { height: 48.0, pad_x: 18.0, radius: 14, font_px: 16.0 };
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.columns(2, |columns| {
            for (column, side, (label, kind)) in [(0, Side::Left, left), (1, Side::Right, right)] {
                let response = controls::button_filling(&mut columns[column], kind, size, label);
                let _ = columns[column].interact(response.rect, confirm_button_id(id, side), Sense::hover());
                if response.clicked() {
                    pressed = Some(side);
                }
            }
        });
        pressed
    });
    match (pressed, outcome) {
        (Some(side), _) => Confirmed::Pressed(side),
        (None, ModalOutcome::Closed) => Confirmed::Dismissed,
        (None, ModalOutcome::Open) => Confirmed::Open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::testing::{self, click_at};

    const SCREEN: Vec2 = egui::vec2(1280.0, 820.0);

    fn frame(ctx: &egui::Context, time: f64, screen: Vec2, events: Vec<egui::Event>) -> ModalOutcome {
        let mut outcome = ModalOutcome::Open;
        let _ = testing::frame(ctx, time, screen, events, |_ui| {
            let ctx = _ui.ctx().clone();
            outcome = Dialog::new("test-dialog", 560.0)
                .title("Share Ledgerline")
                .sub("Send a secure link. Revoke it any time.")
                .show(&ctx, |ui| {
                    ui.label("body");
                })
                .0;
        });
        outcome
    }

    /// Two idle frames: an anchored area is laid out unseen on its first.
    fn shown(screen: Vec2) -> egui::Context {
        let ctx = testing::ctx();
        let _ = frame(&ctx, 0.0, screen, vec![]);
        let _ = frame(&ctx, 0.1, screen, vec![]);
        // And a third: `read_response` answers from the pass before last.
        let _ = frame(&ctx, 0.15, screen, vec![]);
        ctx
    }

    #[test]
    fn the_scrim_closes_the_dialog() {
        let ctx = shown(SCREEN);
        assert_eq!(frame(&ctx, 0.2, SCREEN, click_at(egui::pos2(8.0, 8.0))), ModalOutcome::Closed);
    }

    /// A real press and release arrive in two frames.
    #[test]
    fn a_two_frame_click_on_the_scrim_closes_the_dialog() {
        let ctx = shown(SCREEN);
        let at = egui::pos2(8.0, 400.0);
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let down = frame(&ctx, 0.2, SCREEN, vec![egui::Event::PointerMoved(at), button(true)]);
        let up = frame(&ctx, 0.25, SCREEN, vec![button(false)]);
        assert!(down == ModalOutcome::Closed || up == ModalOutcome::Closed);
    }

    #[test]
    fn esc_closes_the_dialog() {
        let ctx = shown(SCREEN);
        let esc = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::default() };
        assert_eq!(frame(&ctx, 0.2, SCREEN, vec![esc]), ModalOutcome::Closed);
    }

    #[test]
    fn the_close_mark_closes_the_dialog() {
        let ctx = shown(SCREEN);
        let mark = ctx.read_response(close_id("test-dialog")).expect("the close mark").rect;
        assert_eq!(mark.size(), Vec2::splat(CLOSE_SIZE));
        assert_eq!(frame(&ctx, 0.2, SCREEN, click_at(mark.center())), ModalOutcome::Closed);
    }

    #[test]
    fn a_click_in_the_card_keeps_it_open() {
        let ctx = shown(SCREEN);
        let mark = ctx.read_response(close_id("test-dialog")).unwrap().rect;
        // Left of the close mark, on the title's line: card, not scrim.
        let inside = mark.left_center() - Vec2::new(120.0, 0.0);
        assert_eq!(frame(&ctx, 0.2, SCREEN, click_at(inside)), ModalOutcome::Open);
    }

    #[test]
    fn a_narrow_window_keeps_a_24pt_gutter() {
        let narrow = egui::vec2(500.0, 700.0);
        let dialog = Dialog::new("x", 660.0);
        assert_eq!(dialog.width_on(Rect::from_min_size(egui::Pos2::ZERO, narrow)), 452.0);
        let ctx = shown(narrow);
        let mark = ctx.read_response(close_id("test-dialog")).unwrap().rect;
        assert!(mark.right() <= narrow.x - GUTTER - CLOSE_INSET + 0.5, "{mark:?}");
    }
}

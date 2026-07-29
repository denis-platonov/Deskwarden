use crate::theme;
use eframe::egui::{self, Margin, RichText, Rounding, Sense, Stroke, Vec2};
use std::cell::RefCell;
use std::rc::Rc;

/// What the overlay shows about the matched vault item: enough for the user
/// to recognize *which* credentials are about to be filled (design 2a shows
/// the username with the item name under it), without ever putting the
/// password itself on screen.
pub struct OverlayMatch {
    pub item_name: String,
    pub username: Option<String>,
}

/// Opens the autofill overlay for `app_name`: a small, frameless,
/// always-on-top card (design 2a — "no chrome") with the Deskwarden header,
/// the matched credential row, and a keyboard-hint footer.
///
/// Returns `true` if the user chose to fill (clicked the row or pressed
/// Enter), `false` if they dismissed it (the header's ✕, Esc, or closing the
/// window).
///
/// `matched` is `None` when the item couldn't be read back from the vault at
/// prompt time; the overlay still shows, it just can't name the credentials.
pub fn show_prompt_overlay(app_name: &str, matched: Option<&OverlayMatch>) -> bool {
    let app_name = app_name.to_string();
    let (item_name, username) = match matched {
        Some(m) => (m.item_name.clone(), m.username.clone()),
        None => (String::new(), None),
    };

    // Same Rc<RefCell<_>> pattern as picker_ui::run_picker: the update
    // closure/app is 'static and must move-capture its state, so a plain
    // local bool can't be read back after the blocking call returns. A clone
    // of the Rc is moved in; the original is read here once the blocking
    // call returns (safe: same thread, no cross-thread sharing).
    let fill_clicked = Rc::new(RefCell::new(false));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([396.0, 164.0])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let app = OverlayApp {
        app_name,
        item_name,
        username,
        fill_clicked: fill_clicked.clone(),
    };

    // `run_native` rather than `run_simple_native`, because the frameless
    // card needs a transparent clear color behind its rounded corners, and
    // only a full `eframe::App` impl can override `clear_color`.
    let _ = eframe::run_native(
        "Deskwarden",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    );

    let clicked = *fill_clicked.borrow();
    clicked
}

struct OverlayApp {
    app_name: String,
    item_name: String,
    username: Option<String>,
    fill_clicked: Rc<RefCell<bool>>,
}

impl eframe::App for OverlayApp {
    // Transparent behind the card: without this the window would clear to
    // the theme's opaque panel fill and the rounded corners would sit in a
    // visible rectangle.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut done = false;

        // The overlay is keyboard-first (design 2a's footer: "↵ Fill · Esc
        // Dismiss"): Enter fills, Esc dismisses, no focus juggling needed.
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            *self.fill_clicked.borrow_mut() = true;
            done = true;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            done = true;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                match draw_overlay_card(
                    ui,
                    &self.app_name,
                    &self.item_name,
                    self.username.as_deref(),
                ) {
                    OverlayAction::Fill => {
                        *self.fill_clicked.borrow_mut() = true;
                        done = true;
                    }
                    OverlayAction::Dismiss => done = true,
                    OverlayAction::None => {}
                }
            });

        if done {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// What the user did to the overlay card on this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayAction {
    /// Nothing yet; keep the overlay up.
    #[default]
    None,
    /// Fill the matched credentials (the credential row was clicked).
    Fill,
    /// Close without filling (the header's ✕ was clicked).
    Dismiss,
}

/// Draws the overlay card itself — header (mark, wordmark, match count,
/// dismiss ✕), the matched credential row, and the keyboard-hint footer.
///
/// Public (rather than folded into `OverlayApp::update`) so the
/// `ui_preview` example can render the exact card the app ships, not a
/// re-implementation that could drift from it.
pub fn draw_overlay_card(
    ui: &mut egui::Ui,
    app_name: &str,
    item_name: &str,
    username: Option<&str>,
) -> OverlayAction {
    let mut action = OverlayAction::None;

    let card = egui::Frame::none()
        .fill(theme::CARD)
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
        .shadow(egui::epaint::Shadow {
            offset: Vec2::new(0.0, 6.0),
            blur: 18.0,
            spread: 0.0,
            color: egui::Color32::from_black_alpha(36),
        })
        .outer_margin(Margin {
            left: 4.0,
            right: 12.0,
            top: 2.0,
            bottom: 20.0,
        });

    card.show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;

        // Header: mark, wordmark, match count, and the dismiss ✕. The ✕ is
        // the only mouse-operable way out of a `with_decorations(false)`
        // window — there is no title bar to close, and the footer's "Esc
        // Dismiss" is a label, not a control. It matters more than it looks:
        // this window is raised in response to *another* app being
        // foregrounded, which is exactly the situation Windows' foreground
        // lock refuses keyboard focus for, so Esc is not guaranteed to reach
        // us at all.
        egui::Frame::none()
            .inner_margin(Margin::symmetric(12.0, 9.0))
            .show(ui, |ui| {
                if theme::card_header_with_close(ui, "1 match") {
                    action = OverlayAction::Dismiss;
                }
            });
        theme::hairline(ui);

        // The matched credential row, selected treatment.
        egui::Frame::none()
            .inner_margin(Margin::same(6.0))
            .show(ui, |ui| {
                let (primary, secondary) = row_text(app_name, item_name, username);
                if credential_row(ui, &primary, &secondary) {
                    action = OverlayAction::Fill;
                }
            });

        // Footer: keyboard hints on the tinted strip.
        theme::hairline(ui);
        egui::Frame::none()
            .fill(theme::CARD_TINT)
            .rounding(Rounding {
                sw: 9.0,
                se: 9.0,
                ..Rounding::ZERO
            })
            .inner_margin(Margin::symmetric(12.0, 8.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::footer_hints(ui, &[("Enter", "Fill"), ("Esc", "Dismiss")]);
            });
    });

    action
}

/// The two lines of the credential row: the recognizable identity on top
/// (username when known, item name otherwise) and context underneath.
fn row_text(app_name: &str, item_name: &str, username: Option<&str>) -> (String, String) {
    match (username, item_name.is_empty()) {
        (Some(u), false) => (u.to_string(), format!("{item_name} · fills {app_name}")),
        (Some(u), true) => (u.to_string(), format!("fills {app_name}")),
        (None, false) => (item_name.to_string(), format!("fills {app_name}")),
        (None, true) => ("Saved credentials".to_string(), format!("fills {app_name}")),
    }
}

/// The selected credential row (blue wash, avatar, Enter chip). Returns true
/// when clicked.
fn credential_row(ui: &mut egui::Ui, primary: &str, secondary: &str) -> bool {
    let row = egui::Frame::none()
        .fill(theme::BLUE_WASH)
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(10.0, 9.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::avatar(ui, &theme::initials(primary), 28.0, true);
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(theme::semibold(primary, 13.0).color(theme::INK));
                    ui.label(RichText::new(secondary).size(11.0).color(theme::TEXT_FAINT));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme::kbd_chip(ui, "Enter", true);
                });
            });
        });

    let response = row.response.interact(Sense::click());
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_leads_with_the_username_when_known() {
        let (primary, secondary) = row_text("ledgerline.exe", "Ledgerline", Some("a@b.com"));
        assert_eq!(primary, "a@b.com");
        assert_eq!(secondary, "Ledgerline · fills ledgerline.exe");
    }

    #[test]
    fn row_falls_back_to_the_item_name_without_a_username() {
        let (primary, secondary) = row_text("app.exe", "Postgres — Prod", None);
        assert_eq!(primary, "Postgres — Prod");
        assert_eq!(secondary, "fills app.exe");
    }

    #[test]
    fn row_still_says_something_when_the_item_could_not_be_read() {
        let (primary, secondary) = row_text("app.exe", "", None);
        assert_eq!(primary, "Saved credentials");
        assert_eq!(secondary, "fills app.exe");
    }
}

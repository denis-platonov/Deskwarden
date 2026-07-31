//! The 3e preferences window, General section.
//!
//! Design 3e specifies seven sections (General, Autofill, Native apps,
//! Security, Shortcuts, Sync & account, About); only General is built here,
//! because it's the only one with a real setting behind it today
//! (`Settings::keep_backend_running`). The others get built once there's
//! something in them to toggle.

use crate::login_ui::{draw_window_chrome, round_window_corners, ChromeAction};
use crate::settings::Settings;
use crate::theme;
use eframe::egui::{self, Margin, RichText};
use std::cell::RefCell;
use std::rc::Rc;

const WINDOW_TITLE: &str = "Deskwarden Preferences";

/// One settings row: label, description, trailing toggle. Returns the new
/// value. The whole row is the hit target, matching the design -- the pill
/// itself is a paint-only 40x22 rect (`theme::toggle_pill`), so click
/// handling lives here rather than in the pill.
fn toggle_row(ui: &mut egui::Ui, label: &str, description: &str, value: bool) -> bool {
    let mut next = value;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(theme::semibold(label, 13.0).color(theme::INK));
            ui.label(RichText::new(description).size(11.0).color(theme::TEXT_FAINT));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(40.0, 22.0), egui::Sense::click());
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                theme::toggle_pill(ui, value);
            });
            if response.clicked() {
                next = !value;
            }
        });
    });
    next
}

/// Opens the preferences window and blocks until it closes (same shape as
/// every other window in this crate -- `run_ui_native` pumps its own event
/// loop), returning the edited settings. The caller decides whether
/// anything actually changed and persists them; this function never touches
/// disk itself.
pub fn run(settings: Settings) -> Settings {
    let result = Rc::new(RefCell::new(settings.clone()));
    let result_for_closure = result.clone();
    let mut styled = false;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([520.0, 300.0])
            .with_resizable(false)
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            // Same first-frame guard every window here uses: egui only picks
            // up a new font set at the *start* of the next frame, so drawing
            // real (Archivo-styled) content this frame would either panic on
            // a font family that doesn't exist yet or, worse, flash one
            // unpainted near-black frame before the background fill lands --
            // which reads as a console window flashing open, not a
            // preferences dialog.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            round_window_corners(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        if draw_window_chrome(ui, WINDOW_TITLE) == ChromeAction::Close {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(Margin::symmetric(26, 22)))
            .show(ui, |ui| {
                let mut current = result_for_closure.borrow_mut();
                ui.label(theme::bold("General", 15.0).color(theme::INK));
                ui.add_space(16.0);

                current.keep_backend_running = toggle_row(
                    ui,
                    "Keep the Bitwarden backend running",
                    "Faster, and uses about 110 MB while idle. Off runs it only \
                     while the vault window is open; autofill is unaffected either way.",
                    current.keep_backend_running,
                );
            });
    });

    let edited = result.borrow().clone();
    edited
}

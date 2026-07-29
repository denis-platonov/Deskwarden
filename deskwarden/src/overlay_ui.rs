use eframe::egui;
use std::cell::RefCell;
use std::rc::Rc;

/// Opens a small, always-on-top blocking egui window near the screen's
/// top-right corner with "Fill" / "Dismiss" buttons for `app_name`.
///
/// Returns `true` if "Fill" was clicked, `false` if "Dismiss" was clicked (or
/// the window was closed without clicking either).
pub fn show_prompt_overlay(app_name: &str) -> bool {
    let app_name = app_name.to_string();

    // Same Rc<RefCell<_>> pattern as picker_ui::run_picker: the update
    // closure is FnMut + 'static and must move-capture its state, so a plain
    // local bool can't be read back after run_simple_native returns. A clone
    // of the Rc is moved into the closure; the original is read here once
    // the blocking call returns (safe: same thread, no cross-thread sharing).
    let fill_clicked = Rc::new(RefCell::new(false));
    let fill_clicked_for_closure = fill_clicked.clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([300.0, 100.0])
            .with_always_on_top(),
        ..Default::default()
    };

    let _ = eframe::run_simple_native("deskwarden", options, move |ctx, _frame| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut done = false;

            ui.label(format!("Fill saved credentials into {app_name}?"));
            ui.horizontal(|ui| {
                if ui.button("Fill").clicked() {
                    *fill_clicked_for_closure.borrow_mut() = true;
                    done = true;
                }
                if ui.button("Dismiss").clicked() {
                    done = true;
                }
            });
            if done {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    });

    let clicked = *fill_clicked.borrow();
    clicked
}

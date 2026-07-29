use crate::app_match::{AppMatch, TriggerMode};
use crate::process_list::{list_processes, ProcessInfo};
use crate::vault_bridge::{VaultBridge, VaultItem};
use eframe::egui;
use std::cell::RefCell;
use std::rc::Rc;

/// Opens a blocking egui window that lets the user search running processes,
/// pick one, choose a trigger mode, and save the resulting `AppMatch` onto
/// `target_item` via `vault.set_app_match`.
///
/// Returns `Some(AppMatch)` if the user clicked Save and the vault write
/// succeeded, or `None` if the user cancelled (or Save was clicked without a
/// selection, or the vault write failed).
///
/// Takes ownership of `vault` and `target_item` (rather than borrowing) because
/// `eframe::run_simple_native`'s update closure is `FnMut + 'static` and must
/// `move`-capture everything it uses; callers clone a `VaultBridge` and
/// `VaultItem` before calling this.
pub fn run_picker(vault: VaultBridge, target_item: VaultItem) -> Option<AppMatch> {
    let processes: Vec<ProcessInfo> = list_processes().unwrap_or_default();

    // The update closure must `move`-capture its state (it's FnMut + 'static
    // and runs on every repaint), so a plain local `Option<AppMatch>` can't be
    // read back by this function after `run_simple_native` returns. Instead,
    // the result lives in an `Rc<RefCell<_>>`: a clone is moved into the
    // closure, and the original is read here once the (blocking) call
    // returns. This is safe because eframe runs the closure on the same
    // thread that's blocked inside `run_simple_native` -- there's no
    // cross-thread sharing happening.
    let result: Rc<RefCell<Option<AppMatch>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();

    let mut filter = String::new();
    let mut selected_pid: Option<u32> = None;
    let mut trigger = TriggerMode::Prompt;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([420.0, 480.0]),
        ..Default::default()
    };

    let _ = eframe::run_simple_native("Add app to nodewarden", options, move |ctx, _frame| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut done = false;

            ui.heading(format!("Match a process to \"{}\"", target_item.name));
            ui.text_edit_singleline(&mut filter);

            egui::ScrollArea::vertical().show(ui, |ui| {
                for p in processes
                    .iter()
                    .filter(|p| p.exe_name.to_lowercase().contains(&filter.to_lowercase()))
                {
                    let selected = selected_pid == Some(p.pid);
                    if ui
                        .selectable_label(selected, format!("{} (pid {})", p.exe_name, p.pid))
                        .clicked()
                    {
                        selected_pid = Some(p.pid);
                    }
                }
            });

            ui.separator();
            egui::ComboBox::from_label("Trigger")
                .selected_text(format!("{trigger:?}"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut trigger, TriggerMode::Prompt, "Prompt");
                    ui.selectable_value(&mut trigger, TriggerMode::Hotkey, "Hotkey");
                    ui.selectable_value(&mut trigger, TriggerMode::Auto, "Auto");
                });

            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    if let Some(pid) = selected_pid {
                        if let Some(p) = processes.iter().find(|p| p.pid == pid) {
                            let m = AppMatch {
                                process: p.exe_name.clone(),
                                trigger,
                            };
                            if vault.set_app_match(&target_item, &m).is_ok() {
                                *result_for_closure.borrow_mut() = Some(m);
                            }
                        }
                    }
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    done = true;
                }
            });

            if done {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    });

    let saved = result.borrow_mut().take();
    saved
}

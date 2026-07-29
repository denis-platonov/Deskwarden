use crate::app_match::{AppMatch, TriggerMode};
use crate::process_list::{list_processes, ProcessInfo};
use crate::vault_bridge::{VaultBridge, VaultItem};
use eframe::egui;
use std::cell::RefCell;
use std::rc::Rc;

/// Case-insensitive substring match of a vault item's name against a search
/// box's contents. An empty filter matches everything.
///
/// Pure and separate from the UI so the search behaviour is testable without
/// opening a window.
pub fn item_matches_filter(item: &VaultItem, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    item.name.to_lowercase().contains(&filter.to_lowercase())
}

/// Opens a blocking egui window listing the user's vault items with a search
/// box, and returns the one they pick (or `None` if they cancel, or the vault
/// couldn't be read).
///
/// This is step one of the tray's "Add app..." flow: `run_picker` needs a
/// specific `VaultItem` to attach a match to, and nothing previously chose
/// one, which is why "Add app..." was an inert menu entry and `run_picker` was
/// dead code in the bin target.
///
/// Takes `&VaultBridge` rather than owning it because the vault is only used
/// *before* the (`FnMut + 'static`) update closure is built; the fetched items
/// are what gets moved in.
pub fn pick_vault_item(vault: &VaultBridge) -> Option<VaultItem> {
    let items: Vec<VaultItem> = match vault.list_items() {
        Ok(items) => items,
        Err(e) => {
            log::error!("could not list vault items for the item picker: {e:?}");
            return None;
        }
    };

    if items.is_empty() {
        log::warn!("vault has no items to attach an app match to");
        return None;
    }

    // Same Rc<RefCell<_>> pattern as `run_picker` below: the update closure is
    // FnMut + 'static and must move-capture its state, so the result is read
    // back through a shared cell once the blocking call returns.
    let result: Rc<RefCell<Option<VaultItem>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();

    let mut filter = String::new();
    let mut selected_id: Option<String> = None;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([420.0, 480.0]),
        ..Default::default()
    };

    let _ = eframe::run_simple_native("Choose a vault item", options, move |ctx, _frame| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut done = false;

            ui.heading("Which vault item should this app fill from?");
            ui.text_edit_singleline(&mut filter);

            egui::ScrollArea::vertical().show(ui, |ui| {
                for item in items.iter().filter(|i| item_matches_filter(i, &filter)) {
                    let selected = selected_id.as_deref() == Some(item.id.as_str());
                    if ui.selectable_label(selected, &item.name).clicked() {
                        selected_id = Some(item.id.clone());
                    }
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Next").clicked() {
                    if let Some(id) = &selected_id {
                        if let Some(item) = items.iter().find(|i| &i.id == id) {
                            *result_for_closure.borrow_mut() = Some(item.clone());
                            done = true;
                        }
                    }
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

    let chosen = result.borrow_mut().take();
    chosen
}

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

    let _ = eframe::run_simple_native("Add app to Deskwarden", options, move |ctx, _frame| {
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
                            match vault.set_app_match(&target_item, &m) {
                                Ok(()) => *result_for_closure.borrow_mut() = Some(m),
                                Err(e) => {
                                    log::error!(
                                        "failed to save app match onto vault item {}: {e:?}",
                                        target_item.id
                                    )
                                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> VaultItem {
        VaultItem {
            id: "1".into(),
            name: name.into(),
            fields: vec![],
            login: None,
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_filter_matches_every_item() {
        assert!(item_matches_filter(&item("Rockstar Games"), ""));
    }

    #[test]
    fn filter_is_case_insensitive_substring() {
        assert!(item_matches_filter(&item("Rockstar Games"), "ROCK"));
        assert!(item_matches_filter(&item("Rockstar Games"), "games"));
    }

    #[test]
    fn filter_excludes_non_matching_items() {
        assert!(!item_matches_filter(&item("Rockstar Games"), "mabl"));
    }
}

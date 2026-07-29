use crate::app_match::{AppMatch, TriggerMode};
use crate::process_list::{list_processes, ProcessInfo};
use crate::theme;
use crate::vault_bridge::{VaultBridge, VaultItem};
use eframe::egui::{self, Margin, RichText, Rounding, Sense, Stroke};
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

/// A design-2a list row: initials avatar, primary line, muted secondary
/// line, blue-washed when selected. Returns true when clicked.
fn list_row(ui: &mut egui::Ui, primary: &str, secondary: &str, selected: bool) -> bool {
    let frame = egui::Frame::none()
        .fill(if selected {
            theme::BLUE_WASH
        } else {
            theme::CARD
        })
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(10.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::avatar(ui, &theme::initials(primary), 28.0, selected);
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(theme::semibold(primary, 13.0).color(if selected {
                        theme::BLUE_DEEP
                    } else {
                        theme::INK
                    }));
                    if !secondary.is_empty() {
                        ui.label(RichText::new(secondary).size(11.0).color(theme::TEXT_FAINT));
                    }
                });
            });
        });
    frame.response.interact(Sense::click()).clicked()
}

/// The window's title block: a small heading with a muted one-line
/// explanation underneath, matching the design's card headers.
fn title_block(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(theme::bold(title, 16.0).color(theme::INK));
    ui.label(RichText::new(subtitle).size(12.0).color(theme::TEXT_FAINT));
}

/// A full-width search field with the design's placeholder treatment.
fn search_field(ui: &mut egui::Ui, filter: &mut String, hint: &str) {
    ui.add(
        egui::TextEdit::singleline(filter)
            .hint_text(RichText::new(hint).color(theme::TEXT_GHOST))
            .desired_width(f32::INFINITY)
            .margin(Margin::symmetric(10.0, 8.0)),
    );
}

/// The white, hairline-bordered card that scrollable lists live in.
fn list_card(ui: &mut egui::Ui, height: f32, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(theme::CARD)
        .rounding(Rounding::same(10.0))
        .stroke(Stroke::new(1.0, theme::BORDER))
        .inner_margin(Margin::same(6.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    add_contents(ui);
                });
        });
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
    let mut styled = false;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([440.0, 540.0]),
        ..Default::default()
    };

    let _ = eframe::run_simple_native("Choose a vault item", options, move |ctx, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::apply(ctx);
            styled = true;
            ctx.request_repaint();
            return;
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::CANVAS)
                    .inner_margin(Margin::symmetric(20.0, 18.0)),
            )
            .show(ctx, |ui| {
                let mut done = false;

                theme::card_header(ui, "Add app");
                ui.add_space(10.0);
                title_block(
                    ui,
                    "Which vault item should this app fill from?",
                    "Create an item that fills here from now on.",
                );
                ui.add_space(8.0);
                search_field(ui, &mut filter, "Search vault");
                ui.add_space(8.0);

                // Clamped: the subtrahend is the space reserved for the
                // buttons below, and a window resized smaller than that would
                // otherwise ask for a negative scroll-area height.
                list_card(ui, (ui.available_height() - 56.0).max(0.0), |ui| {
                    for item in items.iter().filter(|i| item_matches_filter(i, &filter)) {
                        let selected = selected_id.as_deref() == Some(item.id.as_str());
                        let username = item
                            .login
                            .as_ref()
                            .and_then(|l| l.username.clone())
                            .unwrap_or_default();
                        if list_row(ui, &item.name, &username, selected) {
                            selected_id = Some(item.id.clone());
                        }
                    }
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, "Next", None).clicked() {
                        if let Some(id) = &selected_id {
                            if let Some(item) = items.iter().find(|i| &i.id == id) {
                                *result_for_closure.borrow_mut() = Some(item.clone());
                                done = true;
                            }
                        }
                    }
                    if theme::secondary_button(ui, "Cancel").clicked() {
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

/// One entry of the trigger-mode segmented control: label plus the sentence
/// shown under the control while that mode is selected. The wording follows
/// the design's per-app "On focus" column (3e): the overlay list, hotkey
/// only, or filling straight away.
const TRIGGER_CHOICES: &[(TriggerMode, &str, &str)] = &[
    (
        TriggerMode::Prompt,
        "Prompt",
        "Show the overlay when this app is focused.",
    ),
    (
        TriggerMode::Hotkey,
        "Hotkey",
        "Fill only when the fill hotkey is pressed.",
    ),
    (
        TriggerMode::Auto,
        "Auto",
        "Fill immediately when this app is focused.",
    ),
];

/// The design's segmented pill group ("Below field | Above | At cursor"),
/// used here for the trigger mode.
fn trigger_segmented(ui: &mut egui::Ui, trigger: &mut TriggerMode) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (mode, label, _) in TRIGGER_CHOICES {
            let selected = trigger == mode;
            let button = egui::Button::new(theme::semibold(*label, 12.0).color(if selected {
                egui::Color32::WHITE
            } else {
                theme::INK
            }))
            .fill(if selected { theme::BLUE } else { theme::CARD })
            .stroke(if selected {
                Stroke::NONE
            } else {
                Stroke::new(1.0, theme::BORDER_STRONG)
            })
            .rounding(Rounding::same(7.0));
            if ui.add(button).clicked() {
                *trigger = *mode;
            }
        }
    });
    if let Some((_, _, caption)) = TRIGGER_CHOICES.iter().find(|(m, _, _)| m == trigger) {
        ui.label(RichText::new(*caption).size(11.0).color(theme::TEXT_FAINT));
    }
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
    let mut styled = false;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([440.0, 560.0]),
        ..Default::default()
    };

    let _ = eframe::run_simple_native("Add app to Deskwarden", options, move |ctx, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::apply(ctx);
            styled = true;
            ctx.request_repaint();
            return;
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::CANVAS)
                    .inner_margin(Margin::symmetric(20.0, 18.0)),
            )
            .show(ctx, |ui| {
                let mut done = false;

                theme::card_header(ui, "Add app");
                ui.add_space(10.0);
                title_block(
                    ui,
                    &format!("Match a process to \u{201c}{}\u{201d}", target_item.name),
                    "The chosen process fills from this item from now on.",
                );
                ui.add_space(8.0);
                search_field(ui, &mut filter, "Search running processes");
                ui.add_space(8.0);

                list_card(ui, (ui.available_height() - 148.0).max(0.0), |ui| {
                    for p in processes
                        .iter()
                        .filter(|p| p.exe_name.to_lowercase().contains(&filter.to_lowercase()))
                    {
                        let selected = selected_pid == Some(p.pid);
                        if list_row(ui, &p.exe_name, &format!("pid {}", p.pid), selected) {
                            selected_pid = Some(p.pid);
                        }
                    }
                });

                ui.add_space(10.0);
                theme::field_label(ui, "On focus");
                trigger_segmented(ui, &mut trigger);

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, "Save", None).clicked() {
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
                    if theme::secondary_button(ui, "Cancel").clicked() {
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

    #[test]
    fn every_trigger_mode_is_offered_in_the_segmented_control() {
        // A TriggerMode added to the enum but not to TRIGGER_CHOICES would be
        // silently un-pickable in the UI.
        for mode in [TriggerMode::Prompt, TriggerMode::Hotkey, TriggerMode::Auto] {
            assert!(
                TRIGGER_CHOICES.iter().any(|(m, _, _)| *m == mode),
                "{mode:?} is missing from TRIGGER_CHOICES"
            );
        }
    }
}

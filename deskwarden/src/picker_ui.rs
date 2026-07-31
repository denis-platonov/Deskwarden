use crate::app_match::{AppMatch, TriggerMode};
use crate::icon;
use crate::theme;
use crate::vault_bridge::{VaultBridge, VaultItem};
use crate::window_list::{self, WindowInfo};
use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SPI_GETWORKAREA,
};

/// The position for a picker window's top-left corner that centers it on the
/// primary monitor's work area (excludes the taskbar). These are plain
/// standalone dialogs with no associated target window to center against, so
/// unlike the autofill overlay (`app::overlay_position`) there's no better
/// anchor than the screen itself -- but they still need an *explicit* one:
/// left to the OS default, eframe windows on this system open pinned near
/// the top of the screen rather than anywhere near where the user is
/// looking.
fn centered_position(width: f32, height: f32) -> [f32; 2] {
    let mut work_area = RECT::default();
    let got_work_area = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work_area as *mut RECT as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();

    if !got_work_area {
        return [200.0, 150.0];
    }

    let work_w = (work_area.right - work_area.left) as f32;
    let work_h = (work_area.bottom - work_area.top) as f32;
    [
        work_area.left as f32 + (work_w - width) / 2.0,
        work_area.top as f32 + (work_h - height) / 2.0,
    ]
}

/// Case-insensitive substring match of a vault item's name against an
/// already-lowercased filter. Takes the filter pre-lowered rather than
/// lowering it internally: callers filter an entire list against one filter
/// string every repaint, and with a vault in the thousands, lowering the
/// filter once outside the scan -- instead of once per item inside this
/// function -- is the difference between one allocation per frame and one
/// per vault item per frame.
///
/// Pure and separate from the UI so the search behaviour is testable without
/// opening a window.
pub fn item_matches_filter(item: &VaultItem, filter_lower: &str) -> bool {
    if filter_lower.is_empty() {
        return true;
    }
    item.name.to_lowercase().contains(filter_lower)
}

/// A design-2a list row: icon (or, absent one, an initials avatar), primary
/// line, muted secondary line, blue-washed when selected. Returns true when
/// clicked.
fn list_row(
    ui: &mut egui::Ui,
    primary: &str,
    secondary: &str,
    selected: bool,
    icon: Option<&egui::TextureHandle>,
) -> bool {
    let frame = egui::Frame::new()
        .fill(if selected {
            theme::BLUE_WASH
        } else {
            theme::CARD
        })
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match icon {
                    Some(tex) => {
                        ui.add(
                            egui::Image::new((tex.id(), tex.size_vec2()))
                                .fit_to_exact_size(egui::Vec2::splat(28.0)),
                        );
                    }
                    None => theme::avatar(ui, &theme::initials(primary), 28.0, selected),
                }
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
    let response = frame.response.interact(Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// The window's title block: a small heading with a muted one-line
/// explanation underneath, matching the design's card headers.
fn title_block(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(theme::bold(title, 16.0).color(theme::INK));
    ui.label(RichText::new(subtitle).size(12.0).color(theme::TEXT_FAINT));
}

/// A full-width search field with the design's placeholder treatment.
fn search_field(ui: &mut egui::Ui, filter: &mut String, hint: &str) {
    // Text width, not box width: the margin sits outside `desired_width`,
    // so f32::INFINITY would overflow the parent by the margin (see
    // theme::text_field).
    let width = (ui.available_width() - 20.0).max(40.0);
    ui.add(
        egui::TextEdit::singleline(filter)
            .hint_text(RichText::new(hint).color(theme::TEXT_GHOST))
            .desired_width(width)
            .margin(Margin::symmetric(10, 8)),
    );
}

/// Estimated height (content + the 2px inter-row gap) of one [`list_row`].
/// Only needs to be close, not exact -- it drives [`egui::ScrollArea::show_rows`]'s
/// scroll-geometry estimate, not the rows' actual layout.
const LIST_ROW_HEIGHT: f32 = 48.0;

/// The white, hairline-bordered card that scrollable lists live in, showing
/// only the rows within the visible scroll range rather than laying out
/// and painting `row_count` rows on every repaint.
///
/// egui repaints on every keystroke *and* every mouse move over the window
/// (hover detection), so an unvirtualized list re-lays-out and re-paints
/// every one of its rows that often. That's fine for a few dozen rows; for a
/// vault with thousands of items it was the actual cause of the picker
/// feeling laggy while typing or moving the mouse over the list.
fn list_card(
    ui: &mut egui::Ui,
    height: f32,
    row_count: usize,
    mut add_row: impl FnMut(&mut egui::Ui, usize),
) {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::BORDER))
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .max_height(height)
                .auto_shrink([false, false])
                .show_rows(ui, LIST_ROW_HEIGHT, row_count, |ui, row_range| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for row in row_range {
                        add_row(ui, row);
                    }
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
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 540.0])
            .with_position(centered_position(440.0, 540.0))
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native("Choose a vault item", options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::CANVAS)
                    .inner_margin(Margin::symmetric(20, 18)),
            )
            .show(ui, |ui| {
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

                // Filter lowered once per frame, not once per item inside
                // item_matches_filter (see its doc comment) -- this scan is
                // still O(items), but a cheap one now.
                let filter_lower = filter.to_lowercase();
                let filtered: Vec<usize> = (0..items.len())
                    .filter(|&i| item_matches_filter(&items[i], &filter_lower))
                    .collect();

                // Clamped: the subtrahend is the space reserved for the
                // buttons below, and a window resized smaller than that would
                // otherwise ask for a negative scroll-area height.
                list_card(
                    ui,
                    (ui.available_height() - 56.0).max(0.0),
                    filtered.len(),
                    |ui, row| {
                        let item = &items[filtered[row]];
                        let selected = selected_id.as_deref() == Some(item.id.as_str());
                        let username = item
                            .login
                            .as_ref()
                            .and_then(|l| l.username.clone())
                            .unwrap_or_default();
                        if list_row(ui, &item.name, &username, selected, None) {
                            selected_id = Some(item.id.clone());
                        }
                    },
                );

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
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
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
            .corner_radius(CornerRadius::same(7));
            if ui.add(button).clicked() {
                *trigger = *mode;
            }
        }
    });
    if let Some((_, _, caption)) = TRIGGER_CHOICES.iter().find(|(m, _, _)| m == trigger) {
        ui.label(RichText::new(*caption).size(11.0).color(theme::TEXT_FAINT));
    }
}

/// Opens a blocking egui window that lets the user search open windows, pick
/// one, choose a trigger mode, and save the resulting `AppMatch` onto
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
///
/// `default_pid` is the process id of whatever window was active right
/// before "Add app..." was invoked (see `main`'s `last_active_pid`
/// tracking), if any. When it's still in the window list, the picker opens
/// with it pre-selected *and* the search box pre-filled with its name -- the
/// common case (matching the app you were just using) needs no typing at
/// all, while the search box stays live to pick something else.
pub fn run_picker(vault: VaultBridge, target_item: VaultItem, default_pid: Option<u32>) -> Option<AppMatch> {
    let windows: Vec<WindowInfo> = window_list::list_windows(std::process::id());

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

    let default_window = default_pid.and_then(|pid| windows.iter().find(|w| w.pid == pid));
    let mut filter = default_window.map(|w| w.exe_name.clone()).unwrap_or_default();
    let mut selected_pid: Option<u32> = default_window.map(|w| w.pid);
    let mut trigger = TriggerMode::Prompt;
    let mut styled = false;

    // Icon textures are loaded lazily, one GDI round-trip and one GPU upload
    // per distinct exe the *visible* rows actually need, not eagerly for
    // every window in the list -- with a couple hundred windows open,
    // extracting every icon up front would make the picker visibly slow to
    // open. A `None` cache entry means extraction was already tried and
    // failed (no icon on the file, or a GDI call errored), so a row without
    // an icon doesn't retry every single frame.
    let mut icon_cache: HashMap<String, Option<egui::TextureHandle>> = HashMap::new();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 560.0])
            .with_position(centered_position(440.0, 560.0))
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native("Add app to Deskwarden", options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::apply(ui.ctx());
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::CANVAS)
                    .inner_margin(Margin::symmetric(20, 18)),
            )
            .show(ui, |ui| {
                let mut done = false;

                theme::card_header(ui, "Add app");
                ui.add_space(10.0);
                title_block(
                    ui,
                    &format!("Match a process to \u{201c}{}\u{201d}", target_item.name),
                    "The chosen process fills from this item from now on.",
                );
                ui.add_space(8.0);
                search_field(ui, &mut filter, "Search open windows");
                ui.add_space(8.0);

                // Matches against the window title (what's shown as the
                // primary line) as well as the exe name, so searching either
                // "epic" or "epicgameslauncher" finds the same row.
                let filter_lower = filter.to_lowercase();
                let filtered: Vec<usize> = (0..windows.len())
                    .filter(|&i| {
                        let w = &windows[i];
                        w.title.to_lowercase().contains(&filter_lower)
                            || w.exe_name.to_lowercase().contains(&filter_lower)
                    })
                    .collect();

                list_card(
                    ui,
                    (ui.available_height() - 148.0).max(0.0),
                    filtered.len(),
                    |ui, row| {
                        let w = &windows[filtered[row]];
                        let selected = selected_pid == Some(w.pid);
                        let secondary = format!("({} \u{b7} {})", w.exe_name, w.pid);
                        let texture = icon_cache
                            .entry(w.exe_path.clone())
                            .or_insert_with(|| {
                                icon::extract_small_icon(&w.exe_path).map(|rgba| {
                                    let image = egui::ColorImage::from_rgba_unmultiplied(
                                        [rgba.width as usize, rgba.height as usize],
                                        &rgba.rgba,
                                    );
                                    ui.ctx().load_texture(
                                        w.exe_path.clone(),
                                        image,
                                        egui::TextureOptions::default(),
                                    )
                                })
                            })
                            .as_ref();
                        if list_row(ui, &w.title, &secondary, selected, texture) {
                            selected_pid = Some(w.pid);
                        }
                    },
                );

                ui.add_space(10.0);
                theme::field_label(ui, "On focus");
                trigger_segmented(ui, &mut trigger);

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, "Save", None).clicked() {
                        if let Some(pid) = selected_pid {
                            if let Some(w) = windows.iter().find(|w| w.pid == pid) {
                                let m = AppMatch {
                                    process: w.exe_name.clone(),
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
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
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
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_filter_matches_every_item() {
        assert!(item_matches_filter(&item("Rockstar Games"), ""));
    }

    #[test]
    fn filter_matches_a_lowercased_substring_against_the_items_name() {
        // The caller lowercases the filter once before scanning a list (see
        // this fn's doc comment); item_matches_filter itself only lowercases
        // the item's name, so this exercises it with an already-lower filter.
        assert!(item_matches_filter(&item("Rockstar Games"), "rock"));
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

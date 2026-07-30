//! The "2b" vault window: folders sidebar, item list, and detail pane. See
//! `docs/design/deskwarden-design-spec.md` section 4.8.
//!
//! Reuses `login_ui`'s frameless custom-chrome window pattern
//! (`draw_window_chrome`/`round_window_corners`) rather than duplicating
//! it -- both are already `pub fn` there for exactly this reason.

pub mod detail;
pub mod detail_edit;
pub mod item_list;
pub mod sidebar;

use crate::bw_serve;
use crate::fill_stats::FillStats;
use crate::injector::{Injector, SendInputFiller, UiAutomationFiller};
use crate::login_ui::{draw_window_chrome, round_window_corners, ChromeAction};
use crate::theme;
use crate::vault_bridge::{Folder, VaultBridge, VaultItem};
use detail::{draw_detail_read, DetailAction};
use detail_edit::{draw_detail_edit, EditAction, EditDraft};
use eframe::egui::{self, Margin, RichText, Stroke};
use item_list::{draw_item_list, IconCache, ItemListAction};
use sidebar::{draw_sidebar, SidebarAction, SidebarFilter};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const WINDOW_TITLE: &str = "Deskwarden Vault";
const WINDOW_SIZE: [f32; 2] = [1240.0, 740.0];
const SIDEBAR_WIDTH: f32 = 212.0;
const LIST_WIDTH: f32 = 390.0;

/// See Global Constraints: hardcoded until the 3e preferences window exists.
const AUTO_LOCK_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// TOTP is re-fetched from `bw serve` on this interval while an item with a
/// code is selected -- cheap enough to poll (one local HTTP call) and far
/// simpler than implementing the TOTP algorithm ourselves when `bw serve`
/// already exposes the current code directly.
const TOTP_POLL_INTERVAL: Duration = Duration::from_secs(1);

pub struct VaultWindowResult {
    pub locked: bool,
}

enum DetailMode {
    None,
    Read,
    Edit(EditDraft),
    Create(EditDraft),
}

/// One result from the background favicon loader: which item it was for,
/// and the decoded pixels (`None` if this item had no usable icon).
struct FaviconResult {
    item_id: String,
    pixels: Option<(usize, usize, Vec<u8>)>,
}

/// Opens the vault window and blocks until it's closed (the X/window-close
/// path) or locked (the `Lock` button or the auto-lock timer). Mirrors
/// `login_ui::run_login_flow`'s `Rc<RefCell<_>>` result handoff -- the
/// update closure is `FnMut + 'static` and can't return anything directly.
pub fn run<A: UiAutomationFiller, B: SendInputFiller>(
    vault: VaultBridge,
    fill_stats: FillStats,
    injector: &Injector<A, B>,
    server_url: Option<String>,
    session_token: String,
) -> VaultWindowResult {
    let locked = Rc::new(RefCell::new(false));
    let locked_for_closure = locked.clone();
    let mut sync_status: Option<Result<(), String>> = None;

    // Outcome of a click-triggered manual sync. Backgrounded for the same
    // reason `main.rs` backgrounds its update-check and update-apply flows
    // (see the `spawn_favicon_fetch` doc comment below for this file's own
    // prior art): `bw_serve::run_bw_sync` shells out and blocks on a real
    // network round-trip, and running it inline on this thread -- as it used
    // to -- froze the entire vault window (no repaint, no input) for however
    // long the sync took.
    let (sync_tx, sync_rx): (mpsc::Sender<Result<(), String>>, Receiver<Result<(), String>>) = mpsc::channel();
    // True from the moment a sync starts until its outcome arrives, so a
    // second click can't start a second concurrent sync -- same guard
    // `main.rs` uses for its update-apply flow.
    let mut sync_in_progress = false;

    let mut items: Vec<VaultItem> = vault.list_items().unwrap_or_default();
    let mut folders: Vec<Folder> = vault.list_folders().unwrap_or_default();
    // For the toolbar's avatar circle (design 4.8's `AN` initials badge).
    // `None` just omits the avatar -- an unreadable account email is not
    // worth failing the window over.
    let account_email = crate::login_ui::check_bw_status_details().user_email;
    let mut filter = SidebarFilter::All;
    let mut search = String::new();
    let mut selected_id: Option<String> = items.first().map(|i| i.id.clone());
    // Tracks the previous frame's `selected_id` so a change (from clicking a
    // different row in `draw_item_list`) can be detected and used to reset
    // the per-selection state below (`mode`, `reveal_password`, the TOTP
    // cache) -- see the reset block after the item-list panel further down.
    let mut last_selected_id: Option<String> = selected_id.clone();
    let mut mode = DetailMode::Read;
    let mut reveal_password = false;
    let mut icons = IconCache::default();

    let (favicon_tx, favicon_rx): (mpsc::Sender<FaviconResult>, Receiver<FaviconResult>) = mpsc::channel();
    let mut favicon_requested: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut totp_code: Option<String> = None;
    let mut totp_last_poll = Instant::now() - TOTP_POLL_INTERVAL;
    let mut last_activity = Instant::now();

    let mut styled = false;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(false)
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            theme::apply(ui.ctx());
            round_window_corners(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        if ui.ctx().input(|i| i.pointer.any_click() || !i.events.is_empty()) {
            last_activity = Instant::now();
        }
        if last_activity.elapsed() >= AUTO_LOCK_TIMEOUT {
            *locked_for_closure.borrow_mut() = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let remaining = AUTO_LOCK_TIMEOUT.saturating_sub(last_activity.elapsed());
        let lock_countdown = format!(
            "Locks in {}:{:02}",
            remaining.as_secs() / 60,
            remaining.as_secs() % 60
        );

        while let Ok(result) = favicon_rx.try_recv() {
            if let Some((w, h, rgba)) = result.pixels {
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                let tex = ui.ctx().load_texture(result.item_id.clone(), image, egui::TextureOptions::default());
                icons.textures.insert(result.item_id, tex);
            }
        }

        // Non-blocking, like the favicon drain above: the sync thread
        // (spawned from the Sync button below) reports its outcome here, and
        // this loop never waits on it. The fast local `bw serve` reads
        // (`list_items`/`list_folders`) stay on the main thread -- only the
        // slow remote-network `run_bw_sync` call itself was backgrounded.
        if let Ok(result) = sync_rx.try_recv() {
            sync_in_progress = false;
            if result.is_ok() {
                items = vault.list_items().unwrap_or_default();
                folders = vault.list_folders().unwrap_or_default();
            } else if let Err(e) = &result {
                log::warn!("manual vault sync failed: {e}");
            }
            sync_status = Some(result);
        }

        match draw_window_chrome(ui, "Deskwarden Vault") {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            ChromeAction::Minimize => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }

        // Spec section 5's keyboard model for this window: Ctrl+K focuses
        // search, Ctrl+L locks, Ctrl+N opens the new-item form. Ctrl+Shift+F
        // ("fill in app") is checked separately, down in the `DetailMode::Read`
        // arm below, where the selected item is already in scope.
        let (ctrl_k, ctrl_l, ctrl_n) = ui.ctx().input(|i| {
            (
                i.modifiers.ctrl && i.key_pressed(egui::Key::K),
                i.modifiers.ctrl && i.key_pressed(egui::Key::L),
                i.modifiers.ctrl && i.key_pressed(egui::Key::N),
            )
        });
        if ctrl_k {
            ui.memory_mut(|m| m.request_focus(egui::Id::new("vault-search")));
        }
        if ctrl_l {
            *locked_for_closure.borrow_mut() = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctrl_n {
            mode = DetailMode::Create(EditDraft::empty());
        }

        egui::Panel::top("vault-toolbar")
            .frame(egui::Frame::new().fill(theme::CARD).inner_margin(Margin::symmetric(20, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::mark(ui, 20.0);
                    ui.label(theme::bold("Deskwarden", 14.0).color(theme::INK));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::secondary_button(ui, "Lock").clicked() {
                            *locked_for_closure.borrow_mut() = true;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if let Some(email) = &account_email {
                            theme::avatar(ui, &theme::initials(email), 26.0, false);
                        }
                        // Manual sync: this app has nowhere that auto-syncs on
                        // a timer (see `main()`'s own single startup-time
                        // `bw sync` -- everything after that only re-reads
                        // whatever's already local). A change made on another
                        // device otherwise wouldn't show up here until the
                        // whole app restarts; this button is the escape hatch.
                        let sync_clicked = ui
                            .add_enabled_ui(!sync_in_progress, |ui| {
                                theme::secondary_button(ui, "Sync")
                            })
                            .inner
                            .clicked();
                        if sync_clicked && !sync_in_progress {
                            sync_in_progress = true;
                            let tx = sync_tx.clone();
                            let token = session_token.clone();
                            std::thread::spawn(move || {
                                let _ = tx.send(bw_serve::run_bw_sync(&token));
                            });
                        }
                        if sync_in_progress {
                            ui.label(RichText::new("Syncing…").size(11.0).color(theme::TEXT_GHOST));
                        } else if let Some(status) = &sync_status {
                            let (text, color) = match status {
                                Ok(()) => ("Synced", theme::TEXT_GHOST),
                                Err(_) => ("Sync failed", theme::ERROR),
                            };
                            ui.label(RichText::new(text).size(11.0).color(color));
                        }
                    });
                });
            });

        egui::Panel::left("vault-sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::WINDOW_BG).inner_margin(Margin::symmetric(14, 12)).stroke(Stroke::new(1.0, theme::HAIRLINE)))
            .show(ui, |ui| {
                match draw_sidebar(ui, &items, &folders, &mut filter, &lock_countdown) {
                    SidebarAction::NewFolder => {
                        if let Ok(folder) = vault.create_folder("New folder") {
                            folders.push(folder);
                        }
                    }
                    SidebarAction::DeleteFolder(id) => {
                        if vault.delete_folder(&id).is_ok() {
                            folders.retain(|f| f.id != id);
                            if filter == SidebarFilter::Folder(id) {
                                filter = SidebarFilter::All;
                            }
                        }
                    }
                    SidebarAction::None => {}
                }
            });

        egui::Panel::left("vault-item-list")
            .exact_size(LIST_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(14, 12)).stroke(Stroke::new(1.0, theme::HAIRLINE)))
            .show(ui, |ui| {
                match draw_item_list(ui, &items, &filter, &mut search, &mut selected_id, &icons) {
                    ItemListAction::NewItem => mode = DetailMode::Create(EditDraft::empty()),
                    ItemListAction::None => {}
                }
            });

        // `draw_item_list` above is the only place `selected_id` can change
        // (a row click). When it does, everything below is state that was
        // only ever meaningful for the *previous* selection, so it has to be
        // reset here -- before the detail pane (further down) reads any of
        // it to draw itself. Left stale, this is what let switching from
        // item A to item B while mid-edit silently save A's edits onto B,
        // leaked A's revealed password onto B's row, and briefly showed A's
        // TOTP code under B.
        if selected_id != last_selected_id {
            mode = DetailMode::Read;
            reveal_password = false;
            totp_code = None;
            // Force the `totp_last_poll.elapsed() >= TOTP_POLL_INTERVAL`
            // check below to be true on the very next check, matching how
            // the pre-loop initial value is already set, so the newly
            // selected item's code is fetched immediately instead of
            // waiting out the rest of the previous item's poll interval.
            totp_last_poll = Instant::now() - TOTP_POLL_INTERVAL;
            last_selected_id = selected_id.clone();
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(20, 18)))
            .show(ui, |ui| {
                let selected_item = selected_id.as_ref().and_then(|id| items.iter().find(|i| &i.id == id)).cloned();

                // Kick off a favicon fetch the first time this item is seen,
                // and only for items with a website to derive a domain from.
                if let Some(item) = &selected_item {
                    if !icons.textures.contains_key(&item.id) && !favicon_requested.contains(&item.id) {
                        if let Some(uri) = item.login.as_ref().and_then(|l| l.uris.first()).and_then(|u| u.uri.as_deref()) {
                            if let Some(domain) = crate::favicon::domain_from_uri(uri) {
                                favicon_requested.insert(item.id.clone());
                                spawn_favicon_fetch(item.id.clone(), domain, server_url.clone(), favicon_tx.clone());
                            }
                        }
                    }
                }

                match &mut mode {
                    DetailMode::None => {
                        ui.label("Select an item.");
                    }
                    DetailMode::Read => {
                        if let Some(item) = &selected_item {
                            if item.item_type != Some(1) {
                                ui.label(theme::bold(&item.name, 19.0).color(theme::INK));
                                ui.add_space(6.0);
                                ui.label("This item type isn't editable in Deskwarden yet.");
                                return;
                            }

                            if totp_last_poll.elapsed() >= TOTP_POLL_INTERVAL {
                                totp_last_poll = Instant::now();
                                totp_code = vault.get_totp(&item.id).ok().flatten();
                            }
                            let seconds_left = (30 - (std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() % 30)
                                .unwrap_or(0))) as u8;

                            let fill_count = fill_stats.count(&item.id);
                            let mut action = draw_detail_read(ui, item, fill_count, totp_code.as_deref(), seconds_left, &mut reveal_password);
                            // Ctrl+Shift+F (spec section 5) is the keyboard
                            // equivalent of clicking "Fill in app" -- checked
                            // here, not at the top level, because it needs
                            // exactly the selected `item` this arm already
                            // has and the button click above doesn't.
                            if ui.ctx().input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::F)) {
                                action = DetailAction::Fill;
                            }
                            // `item` and `totp_code` already hold everything
                            // a copy action needs -- `draw_detail_read` only
                            // needs to report *which* field was clicked, not
                            // hand the value back through its return type.
                            let login = item.login.as_ref();
                            match action {
                                DetailAction::Edit => mode = DetailMode::Edit(EditDraft::from_item(item)),
                                DetailAction::Fill => {
                                    // Fills into whatever native app targets
                                    // this item -- wired up in Task 10, which
                                    // has window-watch context this module
                                    // doesn't.
                                }
                                DetailAction::CopyUsername => {
                                    if let Some(username) = login.and_then(|l| l.username.as_deref()) {
                                        ui.ctx().copy_text(username.to_string());
                                    }
                                }
                                DetailAction::CopyPassword => {
                                    if let Some(password) = login.and_then(|l| l.password.as_deref()) {
                                        ui.ctx().copy_text(password.to_string());
                                    }
                                }
                                DetailAction::CopyTotp => {
                                    if let Some(code) = &totp_code {
                                        ui.ctx().copy_text(code.clone());
                                    }
                                }
                                DetailAction::OpenWebsite(url) => {
                                    let _ = webbrowser_open(&url);
                                }
                                DetailAction::None => {}
                            }
                        } else {
                            ui.label("Select an item.");
                        }
                    }
                    DetailMode::Edit(draft) => {
                        match draw_detail_edit(ui, draft, &folders, false) {
                            EditAction::Save => {
                                if let Some(item) = &selected_item {
                                    let updated = draft.apply_to(item);
                                    if vault.update_item(&updated).is_ok() {
                                        if let Some(pos) = items.iter().position(|i| i.id == item.id) {
                                            items[pos] = updated;
                                        }
                                        mode = DetailMode::Read;
                                    }
                                }
                            }
                            EditAction::Cancel => mode = DetailMode::Read,
                            EditAction::None => {}
                        }
                    }
                    DetailMode::Create(draft) => {
                        match draw_detail_edit(ui, draft, &folders, true) {
                            EditAction::Save => {
                                if let Ok(created) = vault.create_item(&draft.to_new_item()) {
                                    selected_id = Some(created.id.clone());
                                    items.push(created);
                                    mode = DetailMode::Read;
                                }
                            }
                            EditAction::Cancel => mode = DetailMode::Read,
                            EditAction::None => {}
                        }
                    }
                }
            });

        ui.ctx().request_repaint_after(Duration::from_millis(500));
    });

    let locked = *locked.borrow();
    VaultWindowResult { locked }
}

/// Spawns a one-shot background thread to fetch and decode `domain`'s
/// favicon, sending the result back over `tx`. A bare `thread::spawn` (not
/// `thread::scope`) because the result travels back over an owned channel
/// with no borrowed data -- there's nothing here that needs the caller's
/// stack to stay alive, unlike `loading_ui::show_while`'s worker.
fn spawn_favicon_fetch(item_id: String, domain: String, server_url: Option<String>, tx: mpsc::Sender<FaviconResult>) {
    std::thread::spawn(move || {
        let base = crate::favicon::icon_base_url(server_url.as_deref());
        let url = format!("{base}/{domain}/icon.png");
        let pixels = crate::favicon::fetch_icon_bytes(&url).and_then(|bytes| crate::favicon::decode_rgba(&bytes));
        let _ = tx.send(FaviconResult { item_id, pixels });
    });
}

fn webbrowser_open(url: &str) -> std::io::Result<()> {
    std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()?;
    Ok(())
}

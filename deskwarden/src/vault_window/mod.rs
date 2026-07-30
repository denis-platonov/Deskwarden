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
use crate::login_ui::{draw_window_chrome_with_extra, round_window_corners, ChromeAction};
use crate::theme;
use crate::vault_bridge::{Folder, VaultBridge, VaultItem};
use detail::{draw_detail_read, DetailAction};
use detail_edit::{draw_detail_edit, EditAction, EditDraft};
use eframe::egui::{self, Margin, Stroke};
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

/// How long a click-to-delete button (the sidebar's per-folder × button, or
/// the detail pane's item Delete button) stays armed waiting for a
/// confirming second click before reverting to its normal state. Chosen
/// over a native Win32 `MessageBox` (which would block the async egui event
/// loop) or a full inline "Delete X? [Yes] [No]" row (more UI than a
/// two-click pattern needs) as the simplest way to make deletion not be a
/// single accidental click away, for either the only irreversible
/// destructive action in this window (folder delete) or the newly-wired-up
/// item delete (see `confirm_click`).
const DELETE_CONFIRM_WINDOW: Duration = Duration::from_secs(3);

/// Minimum time that must pass between the arming click and the confirming
/// click before a second click on the same id is actually treated as a
/// confirmation. Without this, a habitual double-click delivers both clicks
/// to egui within the same (or an adjacent) frame, so the intermediate
/// "armed, click again" state is never actually seen on screen before the
/// delete already fires -- defeating the entire point of the two-click
/// confirmation. This is a *lower* bound on top of `DELETE_CONFIRM_WINDOW`'s
/// existing upper bound (the arm still expires after that long either way).
const MIN_CONFIRM_DWELL: Duration = Duration::from_millis(300);

pub struct VaultWindowResult {
    pub locked: bool,
}

enum DetailMode {
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
pub fn run<A: UiAutomationFiller + Clone + 'static, B: SendInputFiller + Clone + 'static>(
    vault: VaultBridge,
    fill_stats: FillStats,
    injector: &Injector<A, B>,
    server_url: Option<String>,
    // The account email for the toolbar's avatar initials. Passed in from
    // `main.rs`'s single `check_bw_status_details()` call rather than this
    // function calling it again itself -- that call spawns the `bw` CLI
    // (~1-3s on Windows), and this window used to pay that cost twice on
    // every open (once here, once in `main.rs` for `server_url`) with no UI
    // feedback before the window even appeared.
    account_email: Option<String>,
    session_token: String,
    // Directory the on-disk favicon cache lives in (`main.rs`'s
    // `project_dirs.cache_dir().join("icons")`). Not created here --
    // `favicon::write_cached_icon` creates it lazily on first write.
    icon_cache_dir: std::path::PathBuf,
) -> VaultWindowResult {
    // `eframe::run_ui_native`'s update closure must be `'static` (it's handed
    // to a real winit event loop, not run on a borrowed stack), but `injector`
    // arrives here as a plain `&Injector<A, B>` borrowed from the caller's
    // stack (see `main.rs`, which keeps its own `injector` alive across the
    // whole run loop and can only lend a reference into this call). Cloning
    // once, up front, turns it into an owned value the `move` closure below
    // can actually capture; `Injector<A, B>`'s `Clone` impl (added for this)
    // is trivial for the real fillers (`RealUiAutomation`/`RealSendInput` are
    // zero-sized), so this is not a meaningful runtime cost.
    let injector = injector.clone();
    let locked = Rc::new(RefCell::new(false));
    let locked_for_closure = locked.clone();
    let mut sync_status: Option<Result<(), String>> = None;
    // When the most recent successful sync completed, for the toolbar's
    // sync pill ("Synced N min ago" per design spec 4.8) -- set below
    // whenever `sync_status` transitions to `Ok(())`. A per-session value on
    // purpose: it resets to "just now" every time this window's own
    // auto-sync-on-open fires, so nothing here needs to survive a restart.
    let mut last_sync_at: Option<Instant> = None;

    // Outcome of a click-triggered manual sync. Backgrounded for the same
    // reason `main.rs` backgrounds its update-check and update-apply flows
    // (see the `ensure_icon_loaded` doc comment below for this file's own
    // prior art): `bw_serve::run_bw_sync` shells out and blocks on a real
    // network round-trip, and running it inline on this thread -- as it used
    // to -- froze the entire vault window (no repaint, no input) for however
    // long the sync took.
    let (sync_tx, sync_rx): (mpsc::Sender<Result<(), String>>, Receiver<Result<(), String>>) = mpsc::channel();
    // True from the moment a sync starts until its outcome arrives, so a
    // second click can't start a second concurrent sync -- same guard
    // `main.rs` uses for its update-apply flow.
    let mut sync_in_progress = false;
    // True once the auto-sync below has fired. The window's first paint
    // shows whatever's already cached locally -- exactly like the Sync
    // button, this never blocks on the sync itself -- but the vault is
    // otherwise only refreshed via that manual button (see its comment) or
    // this app's single startup-time `bw sync`, so a change made on another
    // device since then wouldn't show up here without either. Syncing once,
    // automatically, the first time this window opens closes that gap
    // without needing a click.
    let mut auto_synced = false;

    let mut items: Vec<VaultItem> = vault.list_items().unwrap_or_default();
    let mut folders: Vec<Folder> = vault.list_folders().unwrap_or_default();
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
    // Ids of the item rows `draw_item_list` actually rendered this frame --
    // populated by that call each frame, then used right after to trigger
    // favicon loads for whatever's currently scrolled into view (see
    // `ensure_icon_loaded`), not just the one selected item.
    let mut visible_ids: Vec<String> = Vec::new();

    let (favicon_tx, favicon_rx): (mpsc::Sender<FaviconResult>, Receiver<FaviconResult>) = mpsc::channel();
    let mut favicon_requested: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut totp_code: Option<String> = None;
    let mut totp_last_poll = Instant::now() - TOTP_POLL_INTERVAL;
    let mut last_activity = Instant::now();
    // The fill count shown in the detail pane's metadata line. Computed
    // once per selection change (below, and here for the initial
    // selection) rather than every frame: `fill_stats.count()` does a full
    // file read + JSON parse, which was previously happening on every
    // single repaint while an item was selected.
    let mut fill_count: u32 = selected_id.as_deref().map(|id| fill_stats.count(id)).unwrap_or(0);

    // Two-click "delete" confirmation state, shared in *pattern* (not
    // storage -- each button owns its own slot so arming one doesn't
    // disturb the other) by the sidebar's per-folder × button and the
    // detail pane's item Delete button. `(id, armed_at)`: a second click on
    // the same id, at least `MIN_CONFIRM_DWELL` but less than
    // `DELETE_CONFIRM_WINDOW` after `armed_at`, confirms the delete;
    // anything else (a different id, too fast, or the window elapsing) just
    // (re)arms it. See `confirm_click`.
    let mut folder_delete_pending: Option<(String, Instant)> = None;
    let mut item_delete_pending: Option<(String, Instant)> = None;

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

        // Fires once, on the window's first *real* frame (after the guard
        // above, so fonts are ready) -- not before the window paints:
        // `spawn_vault_sync` only starts a background thread and returns
        // immediately, so this doesn't delay the frame it runs in, and the
        // window shows whatever's already cached locally right away. The
        // result is applied silently by the same `sync_rx` drain the Sync
        // button's click already uses, below -- no special "just synced" UI
        // beyond the existing `sync_status`/`sync_in_progress` labels.
        if !auto_synced {
            auto_synced = true;
            sync_in_progress = true;
            spawn_vault_sync(sync_tx.clone(), session_token.clone());
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
                last_sync_at = Some(Instant::now());
                items = vault.list_items().unwrap_or_default();
                folders = vault.list_folders().unwrap_or_default();
                // If the item that was selected before this sync no longer
                // exists in the reloaded `items` (e.g. deleted on another
                // device), drop the stale id. The detail pane already falls
                // back to "Select an item." when `selected_id` doesn't
                // resolve, but left alone `selected_id`/`last_selected_id`
                // would keep pointing at the vanished item, leaving `mode`/
                // `reveal_password`/`totp_code` stuck in whatever they were.
                // Clearing it here makes `selected_id != last_selected_id`
                // true on the next frame, so the existing per-selection
                // reset block (below) takes care of the rest normally.
                if let Some(id) = &selected_id {
                    if !items.iter().any(|i| &i.id == id) {
                        selected_id = None;
                    }
                }
            } else if let Err(e) = &result {
                log::warn!("manual vault sync failed: {e}");
            }
            sync_status = Some(result);
        }

        // Sync, the account avatar, and Lock live in the titlebar itself
        // (spec 4.8's single toolbar row), not a separate bar underneath --
        // `draw_window_chrome_with_extra` reserves space for them between
        // the title and the ✕/▢/— controls and narrows the drag zone to
        // stop where they actually start (see its doc comment).
        match draw_window_chrome_with_extra(ui, "Deskwarden Vault", |ui| {
            // Right-to-left: the CTRL+L chip nearest the window controls,
            // then Lock immediately to its left (so the two read left-to-
            // right as "Lock CTRL+L", per spec 4.8), then the avatar, then
            // the Sync button and its status pill innermost.
            theme::kbd_chip(ui, "CTRL+L", false);
            if theme::secondary_button(ui, "Lock").clicked() {
                *locked_for_closure.borrow_mut() = true;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if let Some(email) = &account_email {
                // `true` for the dark/emphasized treatment -- spec 4.8's
                // "avatar circle AN (dark)", versus the light/muted style
                // this used before.
                theme::avatar(ui, &theme::initials(email), 26.0, true);
            }
            // Manual sync: this app has nowhere that auto-syncs on a timer
            // (see `main()`'s own single startup-time `bw sync` -- everything
            // after that only re-reads whatever's already local). A change
            // made on another device otherwise wouldn't show up here until
            // the whole app restarts; this button is the escape hatch.
            let sync_clicked = ui
                .add_enabled_ui(!sync_in_progress, |ui| theme::secondary_button(ui, "Sync"))
                .inner
                .clicked();
            if sync_clicked && !sync_in_progress {
                sync_in_progress = true;
                spawn_vault_sync(sync_tx.clone(), session_token.clone());
            }
            // Spec 4.8's "sync pill" ("● Synced 1 min ago"): a colored dot
            // plus text via `theme::status_pill`, not the bare label this
            // used before. Blue for success (there's no dedicated "success"
            // green in this app's palette -- see `theme.rs`'s module doc on
            // "one blue hue... red reserved for actual errors" -- so blue is
            // the existing color that reads as "good" here), the design's
            // error red for failure, and a neutral ghost dot while in
            // flight.
            if sync_in_progress {
                theme::status_pill(ui, theme::TEXT_GHOST, "Syncing…");
            } else if let Some(status) = &sync_status {
                match status {
                    Ok(()) => {
                        let ago = synced_ago_text(last_sync_at.map_or(Duration::ZERO, |t| t.elapsed()));
                        theme::status_pill(ui, theme::BLUE, &format!("Synced {ago}"));
                    }
                    Err(_) => theme::status_pill(ui, theme::ERROR, "Sync failed"),
                }
            }
        }) {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            ChromeAction::Minimize => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }

        // `draw_window_chrome_with_extra` advances the cursor past the 40px
        // bar via `ui.advance_cursor_after_rect`, which -- per egui's own
        // doc comment on `Ui::cursor` -- leaves the *next* widget positioned
        // `item_spacing.y` further down than that. The sidebar/list/detail
        // panels below are the next things drawn in this same outer `ui`, so
        // without this they'd start a few pixels below the bar with the
        // chrome's `WINDOW_BG` full-window fill showing through the gap --
        // a stray grey seam between the titlebar and the panels. Zeroing it
        // here (rather than passing a frame margin into the panels) keeps
        // the fix scoped to exactly the one gap it's meant to close.
        ui.spacing_mut().item_spacing.y = 0.0;

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

        // Auto-expire a stale armed folder delete before deciding what to
        // show the sidebar this frame -- `is_armed` does this too on the
        // click path, but the display needs it checked independently since
        // it runs whether or not a click happened this frame.
        if let Some((_, armed_at)) = folder_delete_pending {
            if Instant::now() >= armed_at + DELETE_CONFIRM_WINDOW {
                folder_delete_pending = None;
            }
        }
        // Owned, not borrowed: an `&str` borrow of `folder_delete_pending`
        // would still be held live inside the closure below, which also
        // needs `&mut folder_delete_pending` for `confirm_click`.
        let armed_folder_id: Option<String> = folder_delete_pending.as_ref().map(|(id, _)| id.clone());

        egui::Panel::left("vault-sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::CARD).inner_margin(Margin::symmetric(14, 12)).stroke(Stroke::new(1.0, theme::HAIRLINE)))
            .show(ui, |ui| {
                match draw_sidebar(ui, &items, &folders, &mut filter, &lock_countdown, armed_folder_id.as_deref()) {
                    SidebarAction::NewFolder => {
                        if let Ok(folder) = vault.create_folder("New folder") {
                            folders.push(folder);
                        }
                    }
                    // A click on a folder's × button: `confirm_click` is
                    // what actually decides whether this is the first
                    // (arming) click or the confirming second one -- see
                    // its doc comment. Only a confirming click reaches
                    // `delete_folder`.
                    SidebarAction::DeleteFolder(id) => {
                        if confirm_click(&mut folder_delete_pending, &id) {
                            if vault.delete_folder(&id).is_ok() {
                                folders.retain(|f| f.id != id);
                                if filter == SidebarFilter::Folder(id) {
                                    filter = SidebarFilter::All;
                                }
                            } else {
                                log::warn!("failed to delete folder {id}");
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
                match draw_item_list(ui, &items, &filter, &mut search, &mut selected_id, &icons, &mut visible_ids) {
                    ItemListAction::NewItem => mode = DetailMode::Create(EditDraft::empty()),
                    ItemListAction::None => {}
                }
            });

        // Load favicons for whatever `draw_item_list` actually drew this
        // frame, matching official Bitwarden clients ("visible items get
        // icons", not just the single selected one). `ensure_icon_loaded`'s
        // own already-resolved check makes this cheap on every frame after
        // the first for a given scroll position -- it's a HashMap/HashSet
        // lookup per id, not a fetch.
        for id in &visible_ids {
            if let Some(item) = items.iter().find(|i| &i.id == id) {
                ensure_icon_loaded(
                    ui.ctx(),
                    item,
                    &icon_cache_dir,
                    &server_url,
                    &favicon_tx,
                    &mut favicon_requested,
                    &mut icons,
                );
            }
        }

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
            // Recompute once per selection change, not every frame -- see
            // `fill_count`'s declaration above.
            fill_count = selected_id.as_deref().map(|id| fill_stats.count(id)).unwrap_or(0);
            // A delete armed on the previous item shouldn't silently carry
            // over and be confirmable against the newly selected one.
            item_delete_pending = None;
            last_selected_id = selected_id.clone();
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(20, 18)))
            .show(ui, |ui| {
                let selected_item = selected_id.as_ref().and_then(|id| items.iter().find(|i| &i.id == id)).cloned();

                // The detail pane wants its own icon regardless of whether
                // the selected item also happens to be part of this frame's
                // `visible_ids` (it's a row in `item_list`'s scrolled range) --
                // if it's selected, it's effectively "visible" too. Cheap to
                // call every frame: see `ensure_icon_loaded`'s doc comment.
                if let Some(item) = &selected_item {
                    ensure_icon_loaded(
                        ui.ctx(),
                        item,
                        &icon_cache_dir,
                        &server_url,
                        &favicon_tx,
                        &mut favicon_requested,
                        &mut icons,
                    );
                }

                match &mut mode {
                    DetailMode::Read => {
                        if let Some(item) = &selected_item {
                            if item.item_type != Some(1) {
                                ui.label(theme::bold(&item.name, 19.0).color(theme::INK));
                                ui.add_space(6.0);
                                ui.label("This item type isn't editable in Deskwarden yet.");
                                return;
                            }

                            // Only poll `bw serve` for a TOTP code if this
                            // item's own login data says one is configured.
                            // `LoginData::totp` is known locally (Task 1),
                            // so items with no TOTP secret at all no longer
                            // pay for a real HTTP round-trip to `bw serve`
                            // every ~1s just to be told "no code" again.
                            let has_totp_secret = item.login.as_ref().and_then(|l| l.totp.as_ref()).is_some();
                            if has_totp_secret && totp_last_poll.elapsed() >= TOTP_POLL_INTERVAL {
                                totp_last_poll = Instant::now();
                                totp_code = vault.get_totp(&item.id).ok().flatten();
                            }
                            let seconds_left = (30 - (std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() % 30)
                                .unwrap_or(0))) as u8;

                            // Auto-expire a stale armed item delete the same
                            // way the sidebar's folder delete does above.
                            if let Some((_, armed_at)) = item_delete_pending {
                                if Instant::now() >= armed_at + DELETE_CONFIRM_WINDOW {
                                    item_delete_pending = None;
                                }
                            }
                            let delete_pending = item_delete_pending.as_ref().map(|(id, _)| id.as_str()) == Some(item.id.as_str());

                            let mut action = draw_detail_read(
                                ui,
                                item,
                                fill_count,
                                totp_code.as_deref(),
                                seconds_left,
                                delete_pending,
                                &mut reveal_password,
                                icons.textures.get(item.id.as_str()),
                            );
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
                                    match crate::vault_bridge::extract_app_match(item) {
                                        Some(app_match) => {
                                            let windows = crate::window_list::list_windows(std::process::id());
                                            match crate::app::find_window_for_process(&windows, &app_match.process) {
                                                // fill_from_vault does its own credential lookup
                                                // and the fill in one call -- nothing else here
                                                // needs to touch `injector` directly.
                                                Some(target) => crate::app::fill_from_vault(
                                                    &vault,
                                                    &injector,
                                                    &fill_stats,
                                                    &item.id,
                                                    target.hwnd,
                                                ),
                                                None => log::info!(
                                                    "\"Fill in app\" for {}: {} isn't currently open",
                                                    item.name, app_match.process
                                                ),
                                            }
                                        }
                                        None => log::info!(
                                            "\"Fill in app\" for {}: no app is matched to this item yet",
                                            item.name
                                        ),
                                    }
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
                                    webbrowser_open(&url);
                                }
                                // As with the sidebar's folder ×,
                                // `confirm_click` gates this on a confirming
                                // second click -- see its doc comment. Only
                                // then does this actually call
                                // `vault.delete_item`.
                                DetailAction::Delete => {
                                    if confirm_click(&mut item_delete_pending, &item.id) {
                                        match vault.delete_item(&item.id) {
                                            Ok(()) => {
                                                let deleted_id = item.id.clone();
                                                items.retain(|i| i.id != deleted_id);
                                                // Select the first remaining
                                                // item, or `None` if the
                                                // vault is now empty --
                                                // either way the reset block
                                                // above clears `mode`/
                                                // `reveal_password`/
                                                // `totp_code` for us on the
                                                // next frame.
                                                selected_id = items.first().map(|i| i.id.clone());
                                            }
                                            Err(e) => log::warn!(
                                                "failed to delete item {} ({}): {e:?}",
                                                item.id, item.name
                                            ),
                                        }
                                    }
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

/// Spawns a one-shot background thread that runs `bw sync` and reports the
/// outcome over `tx`. Shared by both the Sync button's click handler and the
/// window's own auto-sync-on-open (see `run`, right after the `styled`
/// first-frame guard) so the actual thread-spawn logic exists in exactly one
/// place instead of being duplicated between them; the caller is still
/// responsible for setting `sync_in_progress` before calling this, same as
/// before this was extracted.
fn spawn_vault_sync(tx: mpsc::Sender<Result<(), String>>, session_token: String) {
    std::thread::spawn(move || {
        let _ = tx.send(bw_serve::run_bw_sync(&session_token));
    });
}

/// The toolbar sync pill's relative-time wording for a successful sync:
/// "just now" for under a minute, "N min ago" beyond that. Matches
/// `detail.rs`'s `metadata_line` relative-time pattern ("Updated N days
/// ago"), one unit down -- minutes rather than days, since `last_sync_at` is
/// a per-session value that resets to "just now" on every auto-sync-on-open
/// this window already does, so hour/day granularity would never actually
/// be reached.
fn synced_ago_text(elapsed: Duration) -> String {
    let minutes = elapsed.as_secs() / 60;
    if minutes == 0 {
        "just now".to_string()
    } else {
        format!("{minutes} min ago")
    }
}

/// Ensures `item`'s favicon is loading or loaded, doing as little work as
/// possible: skips entirely if already resolved (loaded or a fetch already
/// dispatched) this session, serves instantly from the on-disk cache if
/// present (no thread, no network), and only falls back to a background
/// network fetch on a genuine cache miss -- writing the result to the disk
/// cache on success so future opens (and every other item on the same
/// domain) never re-fetch it.
///
/// Cheap to call redundantly: this is called once per selected item and once
/// per currently-visible item, every frame, and the vast majority of those
/// calls hit the first check below and return immediately.
fn ensure_icon_loaded(
    ctx: &egui::Context,
    item: &VaultItem,
    icon_cache_dir: &std::path::Path,
    server_url: &Option<String>,
    favicon_tx: &mpsc::Sender<FaviconResult>,
    favicon_requested: &mut std::collections::HashSet<String>,
    icons: &mut IconCache,
) {
    if icons.textures.contains_key(&item.id) || favicon_requested.contains(&item.id) {
        return;
    }
    let Some(uri) = item.login.as_ref().and_then(|l| l.uris.first()).and_then(|u| u.uri.as_deref()) else {
        favicon_requested.insert(item.id.clone());
        return;
    };
    let Some(domain) = crate::favicon::domain_from_uri(uri) else {
        favicon_requested.insert(item.id.clone());
        return;
    };
    favicon_requested.insert(item.id.clone());

    if let Some(cached_bytes) = crate::favicon::read_cached_icon(icon_cache_dir, &domain) {
        if let Some((w, h, rgba)) = crate::favicon::decode_rgba(&cached_bytes) {
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            let tex = ctx.load_texture(item.id.clone(), image, egui::TextureOptions::default());
            icons.textures.insert(item.id.clone(), tex);
            return;
        }
        // Corrupt/unreadable cache entry -- fall through and re-fetch as if
        // it were a miss, rather than permanently failing this domain.
    }

    let tx = favicon_tx.clone();
    let item_id = item.id.clone();
    let server_url = server_url.clone();
    let cache_dir = icon_cache_dir.to_path_buf();
    std::thread::spawn(move || {
        let base = crate::favicon::icon_base_url(server_url.as_deref());
        let url = format!("{base}/{domain}/icon.png");
        let pixels = crate::favicon::fetch_icon_bytes(&url).and_then(|bytes| {
            let decoded = crate::favicon::decode_rgba(&bytes);
            if decoded.is_some() {
                crate::favicon::write_cached_icon(&cache_dir, &domain, &bytes);
            }
            decoded
        });
        let _ = tx.send(FaviconResult { item_id, pixels });
    });
}

/// True when `pending` is currently armed for `id` as of `now` -- i.e. a
/// delete-button click on `id` at `now` would be on the same id as the
/// arming click and within `DELETE_CONFIRM_WINDOW` of it (the dwell-time
/// floor is checked separately, by the caller). Also clears `pending` once
/// it has expired, so a stale arm from several seconds ago can never be
/// silently confirmed by an unrelated later click.
fn is_armed_at(pending: &mut Option<(String, Instant)>, id: &str, now: Instant) -> bool {
    match pending {
        Some((pending_id, armed_at)) => {
            if now >= *armed_at + DELETE_CONFIRM_WINDOW {
                *pending = None;
                false
            } else {
                pending_id == id
            }
        }
        None => false,
    }
}

/// Handles one click, at `now`, on a click-to-delete button for `id`.
///
/// The first click arms a `DELETE_CONFIRM_WINDOW`-long confirmation (storing
/// `now` as when it was armed) and returns `false` (don't delete yet). A
/// second click on the *same* `id`, within that window, only counts as the
/// *confirming* click -- clearing `pending` and returning `true` -- once at
/// least `MIN_CONFIRM_DWELL` has passed since the arming click; egui
/// delivers both clicks of a fast double-click within the same or adjacent
/// frames, so without this floor a habitual double-click would arm and
/// confirm before the user ever saw the intermediate "armed, click again"
/// state, defeating the confirmation entirely. A click on the same id that's
/// too fast, on a different id, or after the window has elapsed just
/// (re)arms `id` instead of confirming anything.
fn confirm_click_at(pending: &mut Option<(String, Instant)>, id: &str, now: Instant) -> bool {
    if is_armed_at(pending, id, now) {
        let armed_at = pending.as_ref().map(|(_, armed_at)| *armed_at).expect(
            "is_armed_at only returns true when pending is Some, so this is always populated",
        );
        if now.saturating_duration_since(armed_at) >= MIN_CONFIRM_DWELL {
            *pending = None;
            true
        } else {
            false
        }
    } else {
        *pending = Some((id.to_string(), now));
        false
    }
}

/// [`confirm_click_at`] using the real clock. Split out so tests can drive
/// `confirm_click_at` with synthetic `Instant`s instead of relying on real
/// `std::thread::sleep` calls to exercise `MIN_CONFIRM_DWELL`.
fn confirm_click(pending: &mut Option<(String, Instant)>, id: &str) -> bool {
    confirm_click_at(pending, id, Instant::now())
}

/// True when `url` is safe to hand off to the shell to open: an `http://`
/// or `https://` URL (case-insensitive scheme), nothing else. This is
/// defense in depth alongside `webbrowser_open`'s use of `ShellExecuteW`
/// (which, unlike the `cmd.exe /C start` invocation it replaced, isn't
/// vulnerable to shell-metacharacter injection in the first place) -- the
/// URL here is vault data, reachable from shared/imported collections and
/// not just what the user typed themselves, so a `javascript:`/`file:`/
/// no-scheme value is rejected outright rather than trusted to be a normal
/// web link.
fn is_safe_web_url(url: &str) -> bool {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Opens `url` in the user's default browser. Uses `ShellExecuteW` rather
/// than the previous `std::process::Command::new("cmd").args(["/C",
/// "start", "", url])`: `Command` on Windows only quotes an argument that
/// contains whitespace, it does not escape `cmd.exe` metacharacters (`&`,
/// `|`, `<`, `>`, `^`, `"`), so a vault URI like `https://x.com&calc.exe`
/// was able to run an arbitrary second command when a user clicked "Open".
/// `ShellExecuteW` takes the URL as a single string parameter with no
/// shell/argument parsing involved, so it isn't subject to that class of
/// injection. `is_safe_web_url` is still checked first, as defense in
/// depth against anything other than a normal web link (`javascript:`,
/// `file:`, ...); a rejection is logged and otherwise silently ignored --
/// this is reached from a button click, not something worth erroring the
/// whole window over.
fn webbrowser_open(url: &str) {
    if !is_safe_web_url(url) {
        log::warn!("refusing to open non-http(s) URL from vault data: {url}");
        return;
    }
    // Trimmed, not the raw `url`: `is_safe_web_url` validates `url.trim()`
    // (leading/trailing whitespace can't change a scheme check's verdict),
    // and the two should agree on which string is "the URL" -- validate and
    // use the same value rather than checking one and opening the other.
    let url = url.trim();
    let wide_url: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    let wide_verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    // Safety: `wide_url`/`wide_verb` are NUL-terminated UTF-16 buffers kept
    // alive for the duration of this call (ShellExecuteW does not retain
    // the pointers past return); the other three PCWSTR params are
    // intentionally null (no parameters/directory, default verb resolution
    // via "open"); `HWND::default()` is the documented way to pass no owner
    // window.
    unsafe {
        let _ = windows::Win32::UI::Shell::ShellExecuteW(
            windows::Win32::Foundation::HWND::default(),
            windows::core::PCWSTR(wide_verb.as_ptr()),
            windows::core::PCWSTR(wide_url.as_ptr()),
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod synced_ago_text_tests {
    use super::synced_ago_text;
    use std::time::Duration;

    #[test]
    fn under_a_minute_reads_just_now() {
        assert_eq!(synced_ago_text(Duration::from_secs(0)), "just now");
        assert_eq!(synced_ago_text(Duration::from_secs(59)), "just now");
    }

    #[test]
    fn a_minute_or_more_reads_n_min_ago() {
        assert_eq!(synced_ago_text(Duration::from_secs(60)), "1 min ago");
        assert_eq!(synced_ago_text(Duration::from_secs(125)), "2 min ago");
    }
}

#[cfg(test)]
mod url_safety_tests {
    use super::is_safe_web_url;

    #[test]
    fn accepts_http_and_https() {
        assert!(is_safe_web_url("http://example.com"));
        assert!(is_safe_web_url("https://example.com/login"));
    }

    #[test]
    fn accepts_https_scheme_case_insensitively() {
        assert!(is_safe_web_url("HTTPS://Example.com"));
        assert!(is_safe_web_url("HtTp://example.com"));
    }

    #[test]
    fn rejects_javascript_scheme() {
        assert!(!is_safe_web_url("javascript:alert(1)"));
    }

    #[test]
    fn rejects_file_scheme() {
        assert!(!is_safe_web_url("file:///C:/Windows/System32/calc.exe"));
    }

    #[test]
    fn rejects_url_with_no_scheme() {
        assert!(!is_safe_web_url("example.com"));
    }

    #[test]
    fn accepts_https_even_with_shell_metacharacters_in_it() {
        // A URI like `https://x.com&calc.exe` was the actual injection
        // vector against the old `cmd.exe /C start` invocation. It's an
        // irrelevant string to safety now that `ShellExecuteW` (not a
        // shell) does the opening -- the scheme check is what gates this,
        // and an http(s) URL passes it regardless of what shell
        // metacharacters happen to be embedded elsewhere in the string.
        assert!(is_safe_web_url("https://x.com&calc.exe"));
    }
}

#[cfg(test)]
mod delete_confirm_tests {
    use super::{confirm_click, confirm_click_at, DELETE_CONFIRM_WINDOW, MIN_CONFIRM_DWELL};
    use std::time::Instant;

    #[test]
    fn first_click_arms_but_does_not_confirm() {
        let mut pending = None;
        assert!(!confirm_click(&mut pending, "f1"));
        assert!(pending.is_some());
    }

    #[test]
    fn second_click_on_the_same_id_after_the_dwell_confirms_and_disarms() {
        let start = Instant::now();
        let mut pending = None;
        assert!(!confirm_click_at(&mut pending, "f1", start));
        // Comfortably past MIN_CONFIRM_DWELL but still well inside
        // DELETE_CONFIRM_WINDOW.
        let later = start + MIN_CONFIRM_DWELL + std::time::Duration::from_millis(1);
        assert!(confirm_click_at(&mut pending, "f1", later));
        assert!(pending.is_none());
    }

    #[test]
    fn a_click_on_a_different_id_rearms_instead_of_confirming() {
        let start = Instant::now();
        let mut pending = None;
        assert!(!confirm_click_at(&mut pending, "f1", start));
        let later = start + MIN_CONFIRM_DWELL + std::time::Duration::from_millis(1);
        assert!(!confirm_click_at(&mut pending, "f2", later));
        // f2 is now armed, not f1 -- confirming f1 again should just re-arm it.
        assert!(!confirm_click_at(
            &mut pending,
            "f1",
            later + MIN_CONFIRM_DWELL + std::time::Duration::from_millis(1)
        ));
    }

    // -- Fix: a fast double-click must not arm and confirm in one gesture --
    //
    // egui delivers both clicks of a rapid double-click within the same (or
    // an adjacent) frame, so the two `confirm_click` calls land only
    // microseconds apart in real time -- far under `MIN_CONFIRM_DWELL`. The
    // pre-fix implementation treated any second click on the same id within
    // `DELETE_CONFIRM_WINDOW` as confirming, so a habitual double-click could
    // arm and confirm a delete before the "armed, click again" state was
    // ever rendered. These tests exercise that exact timing directly, via
    // synthetic `Instant`s, without needing a real sleep or an egui context.

    #[test]
    fn a_second_click_faster_than_the_dwell_window_does_not_confirm() {
        let start = Instant::now();
        let mut pending = None;
        assert!(!confirm_click_at(&mut pending, "f1", start));
        // Simulates both clicks of a fast double-click landing in the same
        // frame: well under MIN_CONFIRM_DWELL after the arming click.
        let too_soon = start + std::time::Duration::from_millis(1);
        assert!(!confirm_click_at(&mut pending, "f1", too_soon));
        // Still armed for f1, not silently dropped -- the user gets another
        // chance to actually see the armed state and confirm it for real.
        assert!(pending.is_some());
    }

    #[test]
    fn a_click_exactly_at_the_dwell_boundary_confirms() {
        let start = Instant::now();
        let mut pending = None;
        assert!(!confirm_click_at(&mut pending, "f1", start));
        let at_boundary = start + MIN_CONFIRM_DWELL;
        assert!(confirm_click_at(&mut pending, "f1", at_boundary));
    }

    #[test]
    fn a_click_still_too_fast_after_a_rearm_still_does_not_confirm() {
        // Arm, then immediately click again (too fast -- stays armed per the
        // test above), then click again immediately once more: still too
        // fast relative to the *original* arming click, so this must still
        // not confirm.
        let start = Instant::now();
        let mut pending = None;
        assert!(!confirm_click_at(&mut pending, "f1", start));
        let too_soon = start + std::time::Duration::from_millis(1);
        assert!(!confirm_click_at(&mut pending, "f1", too_soon));
        let still_too_soon = start + std::time::Duration::from_millis(2);
        assert!(!confirm_click_at(&mut pending, "f1", still_too_soon));
        assert!(pending.is_some());
    }

    #[test]
    fn confirmation_still_expires_after_delete_confirm_window() {
        // The dwell floor is additive, not a replacement for the existing
        // upper bound: an arm still lapses after DELETE_CONFIRM_WINDOW, well
        // past MIN_CONFIRM_DWELL, and a "confirming" click that arrives only
        // after the window elapsed just re-arms instead.
        let start = Instant::now();
        let mut pending = None;
        assert!(!confirm_click_at(&mut pending, "f1", start));
        let after_window = start + DELETE_CONFIRM_WINDOW + std::time::Duration::from_millis(1);
        assert!(!confirm_click_at(&mut pending, "f1", after_window));
        // Re-armed, not confirmed or empty.
        assert!(pending.is_some());
    }
}

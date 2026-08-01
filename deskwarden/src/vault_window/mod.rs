//! The "2b" vault window: folders sidebar, item list, and detail pane. See
//! `docs/design/deskwarden-design-spec.md` section 4.8.
//!
//! Reuses `login_ui`'s frameless custom-chrome window pattern
//! (`draw_window_chrome`/`round_window_corners`) rather than duplicating
//! it -- both are already `pub fn` there for exactly this reason.

pub mod detail;
pub mod detail_edit;
pub mod folder_modal;
pub mod item_list;
pub mod sidebar;

use crate::bw_serve::{self, readiness_schedule, wait_for_vault_ready, READINESS_DEADLINE};
use crate::fill_stats::FillStats;
use crate::injector::{Injector, SendInputFiller, UiAutomationFiller};
use crate::login_ui::{draw_window_chrome_with_extra, round_window_corners, ChromeAction, ChromeMetrics};
use crate::theme;
use crate::vault_bridge::{Folder, VaultError, VaultItem};
use crate::vault_cache::VaultCache;
use detail::{draw_detail_read, DetailAction, TotpState};
use detail_edit::{draw_detail_edit, EditAction, EditDraft};
use eframe::egui::{self, Margin};
use folder_modal::{draw_folder_edit_modal, FolderEditAction, FolderEditState};
use item_list::{draw_item_list, IconCache, ItemListAction};
use sidebar::{draw_sidebar, SidebarAction, SidebarFilter};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const WINDOW_TITLE: &str = "Deskwarden";
/// The window's initial/default size -- not a fixed size. Since the
/// titlebar's maximize control was wired up (see this window's
/// `NativeOptions.with_resizable(true)` and its `maximizable: true` chrome
/// call), the window can grow past this; it just opens at this size.
const WINDOW_SIZE: [f32; 2] = [1240.0, 740.0];
const SIDEBAR_WIDTH: f32 = 212.0;
const LIST_WIDTH: f32 = 390.0;

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
    /// True if a write in this window failed with `VaultError::Unauthorized`
    /// -- the session backing `bw serve` was invalidated while the window
    /// was open (a `bw lock` run elsewhere, a server-side vault timeout, a
    /// password change on another device). `bw serve` itself stays alive and
    /// keeps answering, so nothing else notices; left unhandled, every write
    /// in this window would keep failing silently for the rest of the
    /// session, with no re-auth prompt, until the app was restarted. The
    /// caller (`open_vault_window`) treats this exactly like `locked`: close
    /// the window, then run the same recovery sequence a manual Lock already
    /// does (stop the stale backend, re-authenticate, restart with the fresh
    /// token, repopulate the cache).
    pub needs_reauth: bool,
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
    cache: std::sync::Arc<VaultCache>,
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
    // Idle timeout before this window locks itself. Was a hardcoded
    // module-level constant ("until the 3e preferences window exists"); now
    // that `Settings` exists, `main.rs` loads it once and passes it in here.
    auto_lock: Duration,
    // Whether `bw serve` was already running (`backend_is_running`, checked
    // in `main.rs`'s `open_vault_window` -- the only owner of
    // `bw_serve_child`) at the moment this window session started, before
    // this call. `main.rs` never tears the backend down or restarts it while
    // this function is running (the only paths that do -- lock/reauth
    // recovery -- close this window and return first), so the value stays
    // correct for this whole call, not just at the instant it was read.
    // Threaded through to every `spawn_vault_load` call below (review Minor
    // 3): a backend that was already up needs no readiness wait before its
    // `populate()`, exactly the exemption `spawn_sync` already makes in
    // `main.rs` for the same reason. Without it, every forced post-sync
    // reload paid for a redundant `list_items()` (`wait_for_vault_ready`'s
    // own probe) on top of `populate()`'s -- the whole vault fetched twice,
    // on every sync, hitting default mode (`keep_backend_running: true`,
    // where the backend is essentially always already running) hardest even
    // though that mode never touches the memory-saving setting at all.
    backend_already_running: bool,
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
    // See `VaultWindowResult::needs_reauth`'s doc. Set from any write's
    // error arm below via `flag_reauth_if_unauthorized`, the same
    // `Rc<RefCell<_>>` handoff `locked` already uses -- the update closure
    // is `FnMut + 'static` and can't return anything directly.
    let needs_reauth = Rc::new(RefCell::new(false));
    let needs_reauth_for_closure = needs_reauth.clone();
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

    // The vault is loaded on a background thread, not here.
    //
    // These two calls used to run *before* `run_ui_native` below, so no
    // window existed until both had finished -- and `list_items` pulls the
    // entire vault in one response (measured: ~1.1s and 1.08 MB for 1657
    // items on a cold `bw serve`, plus deserialising all of it into
    // `VaultItem`s, which is slow in an unoptimised build). Every one of
    // those seconds was spent with nothing on screen at all after the user
    // clicked the tray. Starting empty and filling in when the data lands
    // lets the window paint immediately; `vault_loading` drives a spinner
    // until then.
    // Each message is tagged with the generation of the `spawn_vault_load`
    // call that produced it (review Important 2). Both the initial load and
    // any post-sync forced reload report back over this one shared channel,
    // and a slow initial load can resolve *after* a later-spawned forced
    // reload's own result already landed -- without a way to tell which
    // spawn a given result belongs to, that stale result (Ok *or* Err) would
    // silently overwrite state the newer spawn already established. This is
    // exactly the mirror-image bug of the one `sync_status`'s doc below
    // already describes: a slow initial load failing *after* a sync had
    // already reported success used to flip the toolbar pill to "Sync
    // failed" over data the forced reload then updated to be correct and
    // fresh -- right data under a red error, the same class of lie as the
    // original stale-data-under-green-pill bug, just inverted. See
    // `load_generation`'s declaration below for how a result is matched
    // against the latest spawn before being applied.
    let (vault_tx, vault_rx): (
        mpsc::Sender<(u64, Result<(Vec<VaultItem>, Vec<Folder>), String>)>,
        Receiver<(u64, Result<(Vec<VaultItem>, Vec<Folder>), String>)>,
    ) = mpsc::channel();
    // The generation of the most recently spawned `spawn_vault_load` call.
    // Incremented immediately before every spawn (both below and at the
    // post-sync reload further down) so it always names the newest spawn;
    // a result read from `vault_rx` whose own tag doesn't match this value
    // is from a spawn that has since been superseded and is dropped outright
    // rather than applied -- see the drain below.
    let mut load_generation: u64 = 0;
    load_generation += 1;
    // Cloned because the update closure below move-captures both, and needs
    // its own pair to re-issue a load after each sync. `false`: the snapshot
    // from unlock (if any) is current, so this only actually hits the
    // backend the first time the window is opened after unlock -- see
    // `spawn_vault_load`'s doc comment. `backend_already_running`: see that
    // parameter's own doc -- skips the readiness wait when the caller
    // already knows `bw serve` is up.
    spawn_vault_load(cache.clone(), vault_tx.clone(), false, load_generation, backend_already_running);
    let mut items: Vec<VaultItem> = Vec::new();
    let mut folders: Vec<Folder> = Vec::new();
    // True until the background load above reports back.
    let mut vault_loading = true;
    let mut filter = SidebarFilter::All;
    let mut search = String::new();
    // Nothing to select yet -- set from the first item once the load lands.
    let mut selected_id: Option<String> = None;
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

    // The One-time code row's single source of truth -- see `TotpState`'s
    // doc for what this replaced and why. Updated in exactly one place, the
    // per-frame TOTP block below: unconditionally forced to `NoSecret` the
    // instant the selected item's local login data stops carrying a TOTP
    // secret (review Important 1), and otherwise only from a poll's own
    // result (`apply_totp_poll_result`), applied via the `totp_rx` drain
    // below.
    let mut totp_state = TotpState::NoSecret;
    let mut totp_last_poll = Instant::now() - TOTP_POLL_INTERVAL;
    // Tracks whether the *previous* poll for the current selection errored,
    // so the failure (and its later recovery) can be logged once on the
    // transition rather than once per second for as long as the backend
    // stays down -- see the poll site below and review Important 1 on
    // commit 1d6c5ab.
    let mut totp_poll_failing = false;
    // The TOTP poll used to run inline on this thread (a real HTTP call to
    // `bw serve` via `ureq`), which stalled the whole window -- input,
    // repaint, everything -- for however long that call took. `ureq`'s read
    // timeout bounds a single read *syscall*, not the response as a whole, so
    // a `bw serve` that accepts a connection and then trickles bytes (or
    // stalls between them) can hold a read well past the configured timeout;
    // against that, a poll every `TOTP_POLL_INTERVAL` could freeze the window
    // for a large fraction of every second it was open. Backgrounded the same
    // way `spawn_vault_load`/`spawn_vault_sync` already are: `should_start_totp_poll`
    // below gates spawning a one-shot thread that reports over `totp_tx`, and
    // the non-blocking `totp_rx` drain applies whatever it sends back.
    let (totp_tx, totp_rx): (
        mpsc::Sender<(String, Result<Option<String>, VaultError>)>,
        Receiver<(String, Result<Option<String>, VaultError>)>,
    ) = mpsc::channel();
    // True from the moment a poll thread is spawned until its result is
    // drained. Gates `should_start_totp_poll` so a `bw serve` that never
    // answers can pile up at most one outstanding poll thread rather than a
    // new one every `TOTP_POLL_INTERVAL` for as long as it stays hung.
    let mut totp_poll_in_flight = false;
    let mut last_activity = Instant::now();
    // The fill count shown in the detail pane's metadata line. Computed
    // once per selection change (below, and here for the initial
    // selection) rather than every frame: `fill_stats.count()` does a full
    // file read + JSON parse, which was previously happening on every
    // single repaint while an item was selected.
    let mut fill_count: u32 = selected_id.as_deref().map(|id| fill_stats.count(id)).unwrap_or(0);

    // Two-click "delete" confirmation state for the detail pane's item
    // Delete button. `(id, armed_at)`: a second click on the same id, at
    // least `MIN_CONFIRM_DWELL` but less than `DELETE_CONFIRM_WINDOW` after
    // `armed_at`, confirms the delete; anything else (a different id, too
    // fast, or the window elapsing) just (re)arms it. See `confirm_click`.
    // Folder delete used to have its own copy of this same pattern for the
    // sidebar's inline × button; it now lives in the "Edit folder" modal
    // (`folder_edit` below) instead, which already requires a deliberate
    // open-the-editor step before Delete is even reachable.
    let mut item_delete_pending: Option<(String, Instant)> = None;
    // The "Edit folder" modal's state, `Some` while open. Set from the
    // sidebar's `SidebarAction::EditFolder`, seeded with that folder's
    // current name; cleared on Save/Delete success or Cancel/Esc.
    let mut folder_edit: Option<FolderEditState> = None;

    let mut styled = false;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // Starting/default size only -- see WINDOW_SIZE's doc comment.
            // Unlike the login window (fixed-size by design), this window's
            // three-pane layout (fixed-width sidebar/list, flexible detail
            // pane -- see the `Panel`/`CentralPanel` setup below) degrades
            // fine at larger sizes, so it's resizable/maximizable now that
            // the titlebar's ▢ control (`draw_window_chrome_with_extra`'s
            // `maximizable: true` at this window's call site) actually does
            // something.
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(true)
            .with_decorations(false)
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            theme::paint_window_background(ui);
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
        if last_activity.elapsed() >= auto_lock {
            *locked_for_closure.borrow_mut() = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let remaining = auto_lock.saturating_sub(last_activity.elapsed());
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

        // The initial vault load, arriving from the thread spawned before
        // the window opened. Non-blocking like every other drain here, so
        // the window stays responsive (draggable, closable) throughout.
        // Also where a post-sync reload lands, so both paths share this
        // handling rather than each keeping its own copy. The actual state
        // update lives in `apply_vault_load_result` (see its doc) so it can
        // be unit tested directly.
        if let Ok((generation, load_result)) = vault_rx.try_recv() {
            apply_vault_load_result(
                generation,
                load_generation,
                load_result,
                &mut items,
                &mut folders,
                &mut vault_loading,
                &mut selected_id,
                &mut sync_status,
            );
        }

        // Non-blocking, like the favicon drain above: the sync thread
        // (spawned from the Sync button below) reports its outcome here, and
        // this loop never waits on it.
        if let Ok(result) = sync_rx.try_recv() {
            sync_in_progress = false;
            if result.is_ok() {
                last_sync_at = Some(Instant::now());
                // Re-read on the same background path the initial load uses,
                // rather than inline here. These are "fast local `bw serve`
                // reads" only in relative terms -- `list_items` still pulls
                // and parses the whole vault (~1.1s cold, 1.08 MB for 1657
                // items), and doing that here froze the window for the
                // duration every time a sync finished, which is exactly the
                // stall the background load was introduced to remove.
                //
                // `true`: `bw sync` just changed the vault underneath the
                // cache. The snapshot is still marked populated, so without
                // forcing a refresh here `spawn_vault_load` would short-
                // circuit on `is_populated` and serve the pre-sync data --
                // Sync would appear to do nothing.
                //
                // A new generation, so the drain above can tell this result
                // apart from the initial load's (review Important 2) --
                // whichever of the two was still in flight when this fires
                // is now superseded and its eventual result gets dropped
                // rather than applied. `backend_already_running`: same
                // readiness-wait exemption as the initial spawn above (review
                // Minor 3) -- carried for this whole window session, not
                // re-checked here, since nothing in this window's lifetime
                // stops or restarts the backend out from under it.
                load_generation += 1;
                spawn_vault_load(cache.clone(), vault_tx.clone(), true, load_generation, backend_already_running);
            } else if let Err(e) = &result {
                log::warn!("manual vault sync failed: {e}");
            }
            sync_status = Some(result);
        }

        // Non-blocking, like the drains above: the background TOTP poll
        // thread (spawned in the per-frame TOTP block further down) reports
        // its result here, tagged with the item id it was fetched for.
        //
        // This drain runs unconditionally every frame -- not nested inside
        // the Read-mode/selected-item block that spawns the poll -- because
        // that block does not run at all once the selection is cleared or
        // the pane switches to Edit mode, and `totp_poll_in_flight` has to be
        // cleared regardless of whether either is still true when the result
        // actually lands; leaving it gated the same way the spawn is would
        // let a poll started while an item was selected latch
        // `totp_poll_in_flight` permanently once the user switched away
        // before it returned, silently wedging every poll after it for the
        // rest of the session.
        if let Ok((item_id, poll_result)) = totp_rx.try_recv() {
            totp_poll_in_flight = false;
            // A poll only ever updates `totp_state` if it's still for the
            // selected item -- one spawned for item A can land after the
            // user has since selected item B (nothing here blocks waiting
            // for it), and applying it then would show A's code, or A's
            // failure, under B's row. Dropped silently, the same way a
            // superseded vault load is (`apply_vault_load_result`); B's own
            // poll (already in flight or about to be spawned) is what
            // determines what B's row shows.
            if totp_poll_result_is_current(&item_id, selected_id.as_deref()) {
                let seconds_left = current_totp_seconds_left();
                let error = apply_totp_poll_result(poll_result, seconds_left, &mut totp_state);
                // Logged on the failing/recovered transition only (review
                // Important 2 on commit 1d6c5ab) -- with the backend down
                // every 1s poll would otherwise fill the log file, this
                // app's only diagnostic channel, for the rest of the
                // session.
                match &error {
                    Some(e) => {
                        if !totp_poll_failing {
                            log::warn!("TOTP fetch for {item_id} started failing: {e:?}");
                            totp_poll_failing = true;
                        }
                        flag_reauth_if_unauthorized(ui.ctx(), &needs_reauth_for_closure, e);
                    }
                    None => {
                        if totp_poll_failing {
                            log::info!("TOTP fetch for {item_id} recovered");
                            totp_poll_failing = false;
                        }
                    }
                }
            }
        }

        // Sync, the account avatar, and Lock live in the titlebar itself
        // (spec 4.8's single toolbar row), not a separate bar underneath --
        // `draw_window_chrome_with_extra` reserves space for them between
        // the title and the ✕/▢/— controls and narrows the drag zone to
        // stop where they actually start (see its doc comment).
        //
        // `draw_window_chrome_with_extra` advances the cursor past the bar
        // via `ui.advance_cursor_after_rect`, which -- per egui's own doc
        // comment on `Ui::cursor` -- leaves the *next* widget positioned
        // `item_spacing.y` further down than that; `advance_cursor_after_rect`
        // itself reads `self.spacing().item_spacing` *eagerly*, at the moment
        // it runs, and bakes that value into the cursor position it computes.
        // So the gap this is meant to close has to be zeroed *before* the
        // call below, not after it -- setting it afterward (as a previous fix
        // here did) has already missed the window and does nothing. The
        // panels drawn below (sidebar/list/detail) are the next things drawn
        // in this same outer `ui`, so the value is restored immediately after
        // the call returns and before any of them are shown -- left zeroed,
        // it would silently become their ambient `item_spacing.y` too (they
        // don't set their own), collapsing the vertical gap between e.g. the
        // sidebar's VAULT/FOLDERS rows.
        let saved_item_spacing_y = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        match draw_window_chrome_with_extra(ui, WINDOW_TITLE, ChromeMetrics::VAULT, true, |ui| {
            // Right-to-left, so this reads left-to-right (nearest the title,
            // furthest from the window controls, first) as: Sync status
            // pill, "Lock CTRL+L", avatar -- design 2b's exact order. Added
            // here in the opposite order (avatar closest to the window
            // controls, sync pill furthest) since `right_to_left` packs
            // each new widget just to the left of the previous one.
            if let Some(email) = &account_email {
                // Design 2b's avatar is a 28px *circle* -- `theme::avatar`
                // draws a rounded square (used elsewhere: item-list rows,
                // the detail pane header, neither of which were asked to
                // change), so this is painted directly here rather than
                // changing that shared helper's shape for every caller.
                draw_circle_avatar(ui, &theme::initials(email));
            }
            // Design 2b's Lock control carries its own "CTRL+L" shortcut
            // nested inside the same bordered pill, not as a separate
            // floating `kbd_chip` beside it.
            if theme::toolbar_button_with_shortcut(ui, "Lock", "CTRL+L").clicked() {
                *locked_for_closure.borrow_mut() = true;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            // Manual sync: this app has nowhere that auto-syncs on a timer
            // (see `main()`'s own single startup-time `bw sync` -- everything
            // after that only re-reads whatever's already local). A change
            // made on another device otherwise wouldn't show up here until
            // the whole app restarts, so the sync status readout itself is
            // also the sync button -- clicking "● Synced 1 min ago" (design
            // 4.8's pill) starts a fresh sync rather than needing a separate
            // "Sync" button beside it. Blue for success (there's no
            // dedicated "success" green in this app's palette -- see
            // `theme.rs`'s module doc on "one blue hue... red reserved for
            // actual errors" -- so blue is the existing color that reads as
            // "good" here), the design's error red for failure, and a
            // neutral ghost dot both while in flight and before the first
            // sync has reported anything.
            let (dot, label) = if sync_in_progress {
                (theme::TEXT_GHOST, "Syncing…".to_string())
            } else {
                match &sync_status {
                    Some(Ok(())) => {
                        let ago = synced_ago_text(last_sync_at.map_or(Duration::ZERO, |t| t.elapsed()));
                        (theme::BLUE, format!("Synced {ago}"))
                    }
                    Some(Err(_)) => (theme::ERROR, "Sync failed".to_string()),
                    None => (theme::TEXT_GHOST, "Sync".to_string()),
                }
            };
            if theme::status_pill_button(ui, dot, &label).clicked() && !sync_in_progress {
                sync_in_progress = true;
                spawn_vault_sync(sync_tx.clone(), session_token.clone());
            }
        }) {
            ChromeAction::Close => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            ChromeAction::Minimize => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            ChromeAction::None => {}
        }
        ui.spacing_mut().item_spacing.y = saved_item_spacing_y;

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

        // Until the vault arrives, the whole body is one centred spinner --
        // sidebar and item list included, rather than drawing the nav around
        // an empty list. Half-drawn chrome would be showing real-looking
        // structure filled with placeholder values (every sidebar count at
        // 0, no items, nothing selectable), which reads as an empty vault
        // rather than one still loading.
        //
        // Placed after the drains and the shortcut handling above, so
        // `vault_loading` is already up to date for this frame and Ctrl+L
        // still locks while loading; the titlebar is drawn either way, so
        // the window stays draggable and closable throughout.
        if vault_loading {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(theme::CANVAS))
                .show(ui, |ui| {
                    let available = ui.available_height();
                    ui.vertical_centered(|ui| {
                        // Roughly half the spinner-plus-label block, so the
                        // pair sits centred rather than the spinner alone.
                        ui.add_space((available / 2.0 - 30.0).max(0.0));
                        ui.add(egui::Spinner::new().size(28.0).color(theme::BLUE));
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new("Loading your vault…")
                                .size(13.0)
                                .color(theme::TEXT_FAINT),
                        );
                    });
                });
            // Same fast cadence as the tail of this closure: drives the
            // spinner's animation and how promptly the load is noticed.
            ui.ctx().request_repaint_after(Duration::from_millis(16));
            return;
        }

        // No `.stroke(...)` on this frame: `egui::Panel` already paints its
        // own right-edge separator (`show_separator_line`, on by default) in
        // the same `theme::HAIRLINE` color via the ambient
        // `noninteractive.bg_stroke` style (see `theme::apply`). A full-box
        // stroke here used to duplicate that on the right edge and, worse,
        // duplicate the chrome bar's own bottom hairline on the *top* edge
        // -- the two sat flush against each other and read as one doubled
        // line right under the titlebar.
        egui::Panel::left("vault-sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .resizable(false)
            // Design 4.8: `padding: 14px 10px` -- top/bottom 14, left/right
            // 10 (`Margin::symmetric`'s args are x=left/right, y=top/bottom,
            // the opposite order CSS shorthand uses).
            .frame(egui::Frame::new().fill(theme::CARD).inner_margin(Margin::symmetric(10, 14)))
            .show(ui, |ui| {
                match draw_sidebar(ui, &items, &folders, &mut filter, &lock_countdown) {
                    SidebarAction::NewFolder => match cache.create_folder("New folder") {
                        Ok(folder) => folders.push(folder),
                        Err(e) => {
                            log::warn!("failed to create folder: {e:?}");
                            flag_reauth_if_unauthorized(ui.ctx(), &needs_reauth_for_closure, &e);
                        }
                    },
                    // Opens the "Edit folder" modal (drawn once, after all
                    // three panels, further down) seeded with this folder's
                    // current name. Rename and delete both happen there.
                    SidebarAction::EditFolder(id) => {
                        if let Some(folder) = folders.iter().find(|f| f.id == id) {
                            folder_edit = Some(FolderEditState::new(folder.id.clone(), folder.name.clone()));
                        }
                    }
                    SidebarAction::None => {}
                }
            });

        // Same reasoning as `vault-sidebar` above: no own stroke, `Panel`'s
        // built-in separator already draws the right-edge divider.
        egui::Panel::left("vault-item-list")
            .exact_size(LIST_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(14, 12)))
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
            // `NoSecret` is just this reset block's neutral placeholder for
            // "haven't looked yet" -- the per-frame TOTP block right below
            // (gated on `totp_last_poll`, forced to fire immediately by the
            // reset two lines down) overwrites it for real before this ever
            // reaches the render call, so a stale code or unavailable state
            // from the *previous* selection can never leak onto this one.
            totp_state = TotpState::NoSecret;
            // Force the `totp_last_poll.elapsed() >= TOTP_POLL_INTERVAL`
            // check below to be true on the very next check, matching how
            // the pre-loop initial value is already set, so the newly
            // selected item's code is fetched immediately instead of
            // waiting out the rest of the previous item's poll interval.
            totp_last_poll = Instant::now() - TOTP_POLL_INTERVAL;
            // A failure streak belongs to the item that was failing, not the
            // one now selected -- don't carry it over as a false "recovered"
            // log line for an item that never actually failed.
            totp_poll_failing = false;
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
                            // Unconditional, every frame -- not just on
                            // selection change. This is the fix for review
                            // Important 1 (independent review of a7b33cb):
                            // an item with TOTP selected and fetched, whose
                            // secret is then removed elsewhere, used to keep
                            // rendering the last-fetched code under a live
                            // countdown forever, because the poll that would
                            // have blanked it was gated off by the very same
                            // `has_totp_secret` that went false. Forcing
                            // `NoSecret` here every frame closes that gap
                            // regardless of how `has_totp_secret` got to be
                            // false -- selection change, a sync reload
                            // landing mid-session, or anything else.
                            totp_state = totp_state_for_secret_presence(has_totp_secret, totp_state.clone());
                            if !has_totp_secret {
                                // A failure streak belongs to an item that
                                // still has a secret to poll for; carrying it
                                // over would log a false "recovered" later if
                                // this item's secret ever comes back and the
                                // very first poll happens to succeed.
                                totp_poll_failing = false;
                            } else if should_start_totp_poll(
                                has_totp_secret,
                                totp_last_poll.elapsed() >= TOTP_POLL_INTERVAL,
                                totp_poll_in_flight,
                            ) {
                                totp_last_poll = Instant::now();
                                totp_poll_in_flight = true;
                                // The deliberate exception: TOTP codes are
                                // generated by the CLI per request and are
                                // not cacheable, so this stays on the bridge.
                                //
                                // Backgrounded on a one-shot thread rather
                                // than called inline, the same reason
                                // `spawn_vault_load`/`spawn_vault_sync` are:
                                // `get_totp` is a real HTTP round-trip to
                                // `bw serve`, and `ureq`'s read timeout bounds
                                // one read syscall, not the response as a
                                // whole, so a trickling or stalled `bw serve`
                                // could hold this call -- and this window's
                                // entire UI thread with it -- well past that
                                // timeout, once per `TOTP_POLL_INTERVAL`. The
                                // result lands on `totp_rx`, drained further
                                // up (see that drain's doc for why it isn't
                                // nested in this same block); the actual
                                // state transition still goes through
                                // `apply_totp_poll_result` there, so the fix
                                // for review Important 1 on commit 1d6c5ab
                                // (any error moves the pane to `Unavailable`
                                // rather than leaving a stale code rendering
                                // under a countdown that keeps ticking as if
                                // it were live) is unchanged.
                                let item_id = item.id.clone();
                                let bridge = cache.bridge().clone();
                                let tx = totp_tx.clone();
                                std::thread::spawn(move || {
                                    let result = bridge.get_totp(&item_id);
                                    let _ = tx.send((item_id, result));
                                });
                            }
                            // Refreshed every frame regardless of whether a
                            // poll happened this tick: the TOTP window is
                            // wall-clock-derived, not tied to the fetch, so
                            // a `Code` left over from a poll several hundred
                            // milliseconds ago still needs its countdown to
                            // read as live rather than frozen at the moment
                            // of that poll.
                            let seconds_left = current_totp_seconds_left();
                            if let TotpState::Code { seconds_left: code_seconds_left, .. } = &mut totp_state {
                                *code_seconds_left = seconds_left;
                            }

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
                                &totp_state,
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
                                                // (from the cache, not `bw serve` -- see its doc
                                                // comment) and the fill in one call -- nothing
                                                // else here needs to touch `injector` directly.
                                                Some(target) => crate::app::fill_from_vault(
                                                    &cache,
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
                                    // Only `Code` has anything to copy --
                                    // `NoSecret` and `Unavailable` both have
                                    // no valid current code (the detail pane
                                    // doesn't even offer a Copy button for
                                    // either, but this stays defensive rather
                                    // than assuming the button state and the
                                    // action handler can never drift apart).
                                    if let TotpState::Code { code, .. } = &totp_state {
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
                                // `cache.delete_item`.
                                DetailAction::Delete => {
                                    if confirm_click(&mut item_delete_pending, &item.id) {
                                        match cache.delete_item(&item.id) {
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
                                            Err(e) => {
                                                log::warn!(
                                                    "failed to delete item {} ({}): {e:?}",
                                                    item.id, item.name
                                                );
                                                flag_reauth_if_unauthorized(
                                                    ui.ctx(),
                                                    &needs_reauth_for_closure,
                                                    &e,
                                                );
                                            }
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
                                    match cache.update_item(&updated) {
                                        Ok(()) => {
                                            if let Some(pos) = items.iter().position(|i| i.id == item.id) {
                                                items[pos] = updated;
                                            }
                                            mode = DetailMode::Read;
                                        }
                                        Err(e) => {
                                            log::warn!("failed to save item {}: {e:?}", item.id);
                                            flag_reauth_if_unauthorized(
                                                ui.ctx(),
                                                &needs_reauth_for_closure,
                                                &e,
                                            );
                                        }
                                    }
                                }
                            }
                            EditAction::Cancel => mode = DetailMode::Read,
                            EditAction::None => {}
                        }
                    }
                    DetailMode::Create(draft) => {
                        match draw_detail_edit(ui, draft, &folders, true) {
                            EditAction::Save => match cache.create_item(&draft.to_new_item()) {
                                Ok(created) => {
                                    selected_id = Some(created.id.clone());
                                    items.push(created);
                                    mode = DetailMode::Read;
                                }
                                Err(e) => {
                                    log::warn!("failed to create item: {e:?}");
                                    flag_reauth_if_unauthorized(ui.ctx(), &needs_reauth_for_closure, &e);
                                }
                            },
                            EditAction::Cancel => mode = DetailMode::Read,
                            EditAction::None => {}
                        }
                    }
                }
            });

        // Drawn last so it's the newest thing on the `Foreground` layer, on
        // top of the three panels above regardless of their own draw order
        // -- `egui::Area`'s layering is independent of when in the frame
        // it's shown, but keeping this after everything else is the
        // simplest way to read "this can cover the whole window".
        if let Some(state) = &mut folder_edit {
            match draw_folder_edit_modal(ui.ctx(), state) {
                // Both arms report failure into the modal as well as the
                // log. Logging alone left the modal open and unchanged on
                // failure, which from the outside is indistinguishable from
                // the click never having registered.
                FolderEditAction::Save => match cache.update_folder(&state.folder_id, &state.name) {
                    Ok(updated) => {
                        if let Some(f) = folders.iter_mut().find(|f| f.id == updated.id) {
                            f.name = updated.name;
                        }
                        folder_edit = None;
                    }
                    Err(e) => {
                        log::warn!("failed to rename folder {}: {e:?}", state.folder_id);
                        state.error = Some("Could not rename this folder.".to_string());
                        flag_reauth_if_unauthorized(ui.ctx(), &needs_reauth_for_closure, &e);
                    }
                },
                FolderEditAction::Delete => match cache.delete_folder(&state.folder_id) {
                    Ok(()) => {
                        let deleted_id = state.folder_id.clone();
                        folders.retain(|f| f.id != deleted_id);
                        if filter == SidebarFilter::Folder(deleted_id.clone()) {
                            filter = SidebarFilter::All;
                        }
                        folder_edit = None;
                    }
                    Err(e) => {
                        log::warn!("failed to delete folder {}: {e:?}", state.folder_id);
                        state.error = Some("Could not delete this folder.".to_string());
                        flag_reauth_if_unauthorized(ui.ctx(), &needs_reauth_for_closure, &e);
                    }
                },
                FolderEditAction::Cancel => folder_edit = None,
                FolderEditAction::None => {}
            }
        }

        // Only reached once loaded -- the loading branch above returns with
        // its own, much faster, cadence.
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    });

    let locked = *locked.borrow();
    let needs_reauth = *needs_reauth.borrow();
    VaultWindowResult { locked, needs_reauth }
}

/// If `e` is `VaultError::Unauthorized`, flags the window to close and
/// re-authenticate (see `VaultWindowResult::needs_reauth`'s doc) exactly the
/// same way the Lock button does, and closes it immediately rather than
/// waiting for a future frame to notice the flag.
///
/// Called from every write's error arm in `run`'s update closure, so a
/// session invalidated while this window is open is recovered from instead
/// of leaving every subsequent write failing silently for the rest of the
/// session (review Important 2).
fn flag_reauth_if_unauthorized(ctx: &egui::Context, needs_reauth: &Rc<RefCell<bool>>, e: &VaultError) {
    if matches!(e, VaultError::Unauthorized) {
        *needs_reauth.borrow_mut() = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// How many seconds remain in the current 30-second TOTP window, derived
/// from the wall clock -- `bw serve` doesn't report this itself, and it has
/// nothing to do with when the last poll happened. Shared by both the poll
/// site (to seed a freshly-fetched `TotpState::Code`) and the per-frame
/// refresh that keeps an already-displayed code's countdown moving between
/// polls.
fn current_totp_seconds_left() -> u8 {
    (30 - (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() % 30)
        .unwrap_or(0))) as u8
}

/// Forces `previous` back to `TotpState::NoSecret` the instant
/// `has_totp_secret` is false, leaving it untouched otherwise. Called
/// unconditionally, every frame, before the poll-gated branch in `run` --
/// this is the fix for review Important 1 (independent review of a7b33cb):
/// `totp_state` used to only reset on *selection change*, so an item with
/// TOTP selected and fetched, whose secret was then removed elsewhere (a
/// sync reload landing mid-session, say), kept rendering the last-fetched
/// code under a live-looking countdown forever -- the poll that would have
/// cleared it was gated off by the very same `has_totp_secret` that had gone
/// false. Pulled out on its own, the same way `apply_totp_poll_result` is,
/// so this transition is directly unit-testable.
fn totp_state_for_secret_presence(has_totp_secret: bool, previous: TotpState) -> TotpState {
    if has_totp_secret {
        previous
    } else {
        TotpState::NoSecret
    }
}

/// Applies one `get_totp` poll result to `totp_state`, returning the error
/// (if any) for the caller to log/reauth on. Pulled out of `run`'s TOTP poll
/// site into its own function, the same way `apply_vault_load_result` was
/// (see its doc), so the fetch/assign decision is unit-testable without an
/// `eframe` context.
///
/// This is the regression fix for review Important 1 on commit 1d6c5ab,
/// re-expressed over `TotpState` (see its doc for why the bare
/// `Option<String>` this used to mutate couldn't tell "no secret" apart from
/// "unreachable"): that commit changed the poll's `Err` arm to leave the
/// displayed code untouched, on the reasoning that a transient non-401
/// failure isn't evidence the code stopped being valid. That reasoning is
/// backwards for TOTP specifically -- the code expires within 30 seconds
/// regardless of whether the connection to `bw serve` is healthy, and the
/// pane's countdown is derived from the wall clock rather than from the
/// fetch, so holding the old code renders it under a countdown that keeps
/// cycling as if the code were still live. There is no failure mode where
/// holding a TOTP code past its window is safer than dropping it, so *any*
/// error moves to `TotpState::Unavailable` -- restoring the pre-1d6c5ab
/// always-assign behaviour, just landing on an explicit "unreachable" state
/// instead of a blanked `Option` that read identically to "no TOTP here" --
/// while a genuine `Ok(None)` ("no TOTP configured") moves to `NoSecret`,
/// exactly as `Ok(None)` always has.
fn apply_totp_poll_result(
    result: Result<Option<String>, VaultError>,
    seconds_left: u8,
    totp_state: &mut TotpState,
) -> Option<VaultError> {
    match result {
        Ok(Some(code)) => {
            *totp_state = TotpState::Code { code, seconds_left };
            None
        }
        Ok(None) => {
            *totp_state = TotpState::NoSecret;
            None
        }
        Err(e) => {
            *totp_state = TotpState::Unavailable;
            Some(e)
        }
    }
}

/// Whether `run`'s per-frame TOTP block should spawn a new background poll
/// this frame. Pulled out into its own function, the same way
/// `totp_state_for_secret_presence`/`apply_totp_poll_result` are, so the
/// three conditions -- a secret worth polling for, the interval having
/// actually elapsed, and no poll already outstanding -- are unit-testable
/// together without an `eframe` context.
///
/// `poll_in_flight` is the one new condition here (see `totp_poll_in_flight`'s
/// declaration in `run`): without it, a `bw serve` that never answers would
/// still only ever have one real HTTP call blocking on it -- the call itself
/// moved to a background thread -- but `run`'s loop would spawn a *new* such
/// thread every `TOTP_POLL_INTERVAL` for as long as it stayed hung, one more
/// piling up on top of the last with nothing to bound how many accumulate.
fn should_start_totp_poll(has_totp_secret: bool, poll_due: bool, poll_in_flight: bool) -> bool {
    has_totp_secret && poll_due && !poll_in_flight
}

/// Whether a `totp_rx` message fetched for `item_id` should still be applied
/// to `totp_state`, or dropped as stale. A poll runs on a background thread
/// now (see `totp_poll_in_flight`'s declaration in `run`), so nothing blocks
/// waiting for it -- the user is free to select a different item, or none at
/// all, before it reports back. Applying a result for an item that is no
/// longer selected would show its code, or its failure, under a different
/// row than the one it was fetched for; `selected_id` being anything other
/// than `Some(item_id)` -- including `None` -- means this result is stale and
/// must be dropped rather than applied.
fn totp_poll_result_is_current(item_id: &str, selected_id: Option<&str>) -> bool {
    selected_id == Some(item_id)
}

/// Applies one result received from `vault_rx` -- the state update `run`'s
/// update closure used to do inline in its drain of that channel. Pulled out
/// into its own function so the fix for final review Important 2 (the
/// inverse of the original stale-data-under-a-green-pill bug: a slow,
/// superseded load's *failure* landing after a newer load already succeeded,
/// flipping the toolbar to "Sync failed" over data that was in fact just
/// refreshed) is unit-testable without an `eframe` window -- `run`'s closure
/// itself has no boundary a test could call into directly.
///
/// `generation` is this result's own tag (see `spawn_vault_load`'s doc);
/// `latest_generation` is `run`'s own `load_generation`, i.e. the tag of the
/// most recently *spawned* load. A mismatch means a newer load has since
/// been spawned and this result is stale -- dropped outright, `items`/
/// `folders`/`vault_loading`/`selected_id`/`sync_status` all left exactly as
/// they were, since the newer spawn's own result is what should determine
/// them instead.
fn apply_vault_load_result(
    generation: u64,
    latest_generation: u64,
    load_result: Result<(Vec<VaultItem>, Vec<Folder>), String>,
    items: &mut Vec<VaultItem>,
    folders: &mut Vec<Folder>,
    vault_loading: &mut bool,
    selected_id: &mut Option<String>,
    sync_status: &mut Option<Result<(), String>>,
) {
    if generation != latest_generation {
        log::debug!(
            "dropping a superseded vault load result (generation {generation}, latest {latest_generation})"
        );
        return;
    }
    match load_result {
        Ok((loaded_items, loaded_folders)) => {
            *items = loaded_items;
            *folders = loaded_folders;
            *vault_loading = false;
            match &*selected_id {
                // Nothing selected yet (the initial load): select the first
                // item now that there is one. This makes `selected_id !=
                // last_selected_id` true next frame, so the existing
                // per-selection reset block recomputes `fill_count` and
                // friends normally rather than needing its own copy here.
                None => *selected_id = items.first().map(|i| i.id.clone()),
                // A reload where the selected item no longer exists (deleted
                // on another device, say): drop the stale id. Left alone,
                // `selected_id` would keep pointing at the vanished item and
                // leave `mode`/`reveal_password`/`totp_code` stuck as they
                // were; clearing it routes through that same reset block.
                Some(id) => {
                    if !items.iter().any(|i| &i.id == id) {
                        *selected_id = None;
                    }
                }
            }
        }
        Err(e) => {
            // `spawn_vault_load` couldn't refresh the snapshot (`bw serve`
            // never came ready, or `populate()` itself failed) -- see that
            // function's doc for why this must not be silently swallowed.
            // Whatever was already in `items`/`folders` (the pre-refresh
            // snapshot) is left alone rather than cleared: this is the same
            // never-propagate-a-failed-populate behaviour the doc comment
            // already describes, just no longer silent.
            *vault_loading = false;
            log::warn!("vault refresh failed; showing the last known snapshot: {e}");
            // Only override `sync_status` when this refresh was following up
            // on a sync that had itself just reported success (final review
            // Important 1): that is the case where the toolbar pill would
            // otherwise say "Synced just now" over data that was never
            // actually refreshed. An initial-load failure (before any sync
            // has run) has no such claim to correct -- `sync_status` is
            // still `None` and stays that way, showing the neutral "Sync"
            // label rather than a misleading "failed". The generation check
            // above is what keeps this from also firing on a *stale*
            // failure after a newer, already-applied load already reported
            // success (this review's Important 2) -- by the time a stale
            // result reaches here, it has already been dropped.
            if matches!(sync_status, Some(Ok(()))) {
                *sync_status = Some(Err(e));
            }
        }
    }
}

/// Reads the whole vault (items + folders) from the cache on a one-shot
/// background thread and reports the outcome over `tx`.
///
/// Every vault read in this window goes through here rather than being
/// called inline. Backgrounded even though `VaultCache`'s reads themselves
/// are in-memory and effectively free, because `populate()` below is not:
/// it's the one path that still hits `bw serve`, pulling the entire vault in
/// one response -- measured at ~1.1s and 1.08 MB for 1657 items against a
/// cold backend, before the cost of deserialising all of it -- which would
/// stall the window outright on the UI thread.
///
/// A populate failure is reported as `Err`, not silently mapped to whatever
/// is already cached. Final review Important 1: this is the same bug class
/// `spawn_sync` was fixed for (fix wave 2) -- `open_vault_window` may have
/// just kicked off a background `bw serve` start (save-memory mode, backend
/// stopped) in parallel with this thread and the window's own
/// auto-sync-on-open, and `try_start_backend`/`bw sync` succeeding says
/// nothing about whether `bw serve`'s HTTP listener is actually up yet --
/// that cold start (a bundled Node process) routinely takes several seconds.
/// Without waiting for it first, `populate()` below would very often race
/// that gap, fail with a connection error, and -- if that failure were only
/// logged, as it used to be -- silently ship the *pre-sync* snapshot while
/// the caller's sync status still claims success. `run` (this function's
/// caller) uses the `Err` to correct that claim rather than just logging.
fn spawn_vault_load(
    cache: std::sync::Arc<VaultCache>,
    tx: mpsc::Sender<(u64, Result<(Vec<VaultItem>, Vec<Folder>), String>)>,
    // `true` after a sync, which changes the vault underneath us: the
    // snapshot is still marked populated but is now stale, so the
    // `is_populated` short-circuit below would serve pre-sync data and the
    // sync would appear to do nothing. `false` on window open, where the
    // snapshot from unlock is current and re-fetching would throw away the
    // whole point of the cache.
    force_refresh: bool,
    // Tags the message sent over `tx` so `run`'s drain can tell a stale,
    // superseded result apart from the one it's actually still waiting on --
    // see `run`'s `load_generation` doc (review Important 2).
    generation: u64,
    // Whether `bw serve` is already known to be up -- see `run`'s
    // `backend_already_running` parameter doc (review Minor 3). Skips the
    // `wait_for_vault_ready` probe below when true, the same exemption
    // `spawn_sync` in `main.rs` already makes for the same reason.
    skip_readiness_wait: bool,
) {
    spawn_vault_load_with_schedule(
        cache,
        tx,
        force_refresh,
        generation,
        skip_readiness_wait,
        readiness_schedule(READINESS_DEADLINE),
    );
}

/// `spawn_vault_load`'s actual body, with the readiness schedule taken as a
/// parameter rather than hardcoded to `readiness_schedule(READINESS_DEADLINE)`
/// -- same split `wait_for_vault_ready`/`readiness_schedule` already use, and
/// for the same reason: it lets a test exhaust the schedule instantly (an
/// empty one) instead of actually waiting out the real 30s deadline.
fn spawn_vault_load_with_schedule(
    cache: std::sync::Arc<VaultCache>,
    tx: mpsc::Sender<(u64, Result<(Vec<VaultItem>, Vec<Folder>), String>)>,
    force_refresh: bool,
    generation: u64,
    skip_readiness_wait: bool,
    schedule: Vec<Duration>,
) {
    std::thread::spawn(move || {
        if force_refresh || !cache.is_populated() {
            // Same wait `spawn_sync` performs before its own `populate()`
            // (see this function's doc) -- cheap when `bw serve` is already
            // answering (the very first attempt succeeds), and the only
            // thing standing between "backend mid-cold-start" and a bogus
            // connection-refused failure otherwise. Skipped when the caller
            // already knows the backend was running before this window
            // session started (`skip_readiness_wait`, review Minor 3):
            // `populate()` right below still runs, and still fails loudly if
            // the backend somehow isn't answering after all, so skipping the
            // probe costs nothing but a redundant `list_items()` call in the
            // case it's meant to skip -- not the safety net itself.
            if !skip_readiness_wait {
                if let Err(e) = wait_for_vault_ready(cache.bridge(), &schedule) {
                    log::warn!("could not populate the vault cache: bw serve never became ready: {e}");
                    let _ = tx.send((generation, Err(e)));
                    return;
                }
            }
            if let Err(e) = cache.populate() {
                log::warn!("could not populate the vault cache: {e:?}");
                let _ = tx.send((generation, Err(format!("{e:?}"))));
                return;
            }
        }
        let _ = tx.send((generation, Ok((cache.items(), cache.folders()))));
    });
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

/// Design 2b's titlebar avatar: a 28px circle, `theme::INK` background,
/// white 11px Bold initials. Paints the same way `theme::avatar` does
/// (allocate a square, paint a shape, center the text) but with a full
/// circle instead of a rounded rect -- kept local to this one call site
/// rather than added to `theme::avatar` itself, since that helper's rounded-
/// square shape is still correct for its other callers (item-list rows, the
/// detail pane header).
fn draw_circle_avatar(ui: &mut egui::Ui, text: &str) {
    const SIZE: f32 = 28.0;
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(SIZE), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), SIZE / 2.0, theme::INK);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::new(11.0, egui::FontFamily::Name(theme::BOLD.into())),
        egui::Color32::WHITE,
    );
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
mod flag_reauth_if_unauthorized_tests {
    use super::{egui, flag_reauth_if_unauthorized};
    use crate::vault_bridge::VaultError;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn an_unauthorized_error_flags_reauth() {
        let ctx = egui::Context::default();
        let needs_reauth = Rc::new(RefCell::new(false));

        flag_reauth_if_unauthorized(&ctx, &needs_reauth, &VaultError::Unauthorized);

        assert!(
            *needs_reauth.borrow(),
            "a 401 from bw serve must flag the window to re-authenticate"
        );
    }

    #[test]
    fn a_non_unauthorized_error_does_not_flag_reauth() {
        // Regression guard for the other half of the bug this fixes: an
        // ordinary transient failure (a 500, a dropped connection) must not
        // be treated the same as a stale session -- that would tear down and
        // restart a perfectly healthy backend/session over an unrelated
        // hiccup.
        let ctx = egui::Context::default();
        let needs_reauth = Rc::new(RefCell::new(false));

        flag_reauth_if_unauthorized(&ctx, &needs_reauth, &VaultError::Http("boom".to_string()));

        assert!(
            !*needs_reauth.borrow(),
            "a non-401 vault error must not trigger re-authentication"
        );
    }
}

#[cfg(test)]
mod apply_totp_poll_result_tests {
    // Regression tests for review Important 1 on commit 1d6c5ab, re-expressed
    // over `TotpState` after the independent review of a7b33cb replaced the
    // bare `Option<String>` these used to mutate (see that type's doc for
    // why): that commit's poll `Err` arm left a stale TOTP code on screen
    // under a countdown that kept ticking as though it were still valid.
    // These pin the fix -- any error moves to `Unavailable` -- while
    // confirming the genuine "no TOTP configured" case still lands on
    // `NoSecret`, with no error to report either way.
    use super::apply_totp_poll_result;
    use crate::vault_bridge::VaultError;
    use crate::vault_window::detail::TotpState;

    #[test]
    fn a_successful_fetch_becomes_a_live_code() {
        let mut totp_state = TotpState::Code { code: "111111".to_string(), seconds_left: 20 };

        let error = apply_totp_poll_result(Ok(Some("222222".to_string())), 15, &mut totp_state);

        assert_eq!(totp_state, TotpState::Code { code: "222222".to_string(), seconds_left: 15 });
        assert!(error.is_none());
    }

    #[test]
    fn a_genuine_no_totp_configured_response_becomes_no_secret_with_no_error() {
        let mut totp_state = TotpState::Code { code: "111111".to_string(), seconds_left: 20 };

        let error = apply_totp_poll_result(Ok(None), 15, &mut totp_state);

        assert_eq!(totp_state, TotpState::NoSecret);
        assert!(error.is_none());
    }

    #[test]
    fn any_error_makes_a_previously_held_code_unavailable_instead_of_freezing_it() {
        // This is the exact regression: a code fetched minutes ago must not
        // keep rendering under a live-looking countdown just because the
        // latest poll happened to fail on a dropped connection rather than
        // a 401. It also must not silently become `NoSecret` -- that would
        // read as "this item was never set up for TOTP", which is false.
        let mut totp_state = TotpState::Code { code: "111111".to_string(), seconds_left: 20 };

        let error = apply_totp_poll_result(Err(VaultError::Http("connection reset".to_string())), 15, &mut totp_state);

        assert_eq!(
            totp_state,
            TotpState::Unavailable,
            "a poll error must move the pane to Unavailable, not freeze the old code or claim NoSecret"
        );
        assert!(matches!(error, Some(VaultError::Http(_))));
    }

    #[test]
    fn an_unauthorized_error_also_becomes_unavailable_and_is_returned() {
        let mut totp_state = TotpState::Code { code: "111111".to_string(), seconds_left: 20 };

        let error = apply_totp_poll_result(Err(VaultError::Unauthorized), 15, &mut totp_state);

        assert_eq!(totp_state, TotpState::Unavailable);
        assert!(matches!(error, Some(VaultError::Unauthorized)));
    }
}

#[cfg(test)]
mod should_start_totp_poll_tests {
    // The TOTP poll moved off the UI thread onto a one-shot background
    // thread (see `totp_poll_in_flight`'s declaration in `run`) because
    // `ureq`'s read timeout bounds one read syscall, not `get_totp`'s
    // response as a whole -- a trickling or stalled `bw serve` could freeze
    // this window for well past that timeout, once per `TOTP_POLL_INTERVAL`.
    // These pin the three-way gate that replaced the old unconditional call:
    // a poll only starts when there's a secret to poll for, the interval has
    // actually elapsed, and -- the new condition -- no poll is already
    // outstanding, so a hung backend accumulates at most one background
    // thread instead of one more every second for as long as it stays hung.
    use super::should_start_totp_poll;

    #[test]
    fn starts_when_due_with_a_secret_and_nothing_in_flight() {
        assert!(should_start_totp_poll(true, true, false));
    }

    #[test]
    fn does_not_start_without_a_totp_secret() {
        assert!(!should_start_totp_poll(false, true, false));
    }

    #[test]
    fn does_not_start_before_the_interval_elapses() {
        assert!(!should_start_totp_poll(true, false, false));
    }

    #[test]
    fn does_not_start_a_second_poll_while_one_is_already_in_flight() {
        // The regression this exists to prevent: without this gate, a hung
        // `bw serve` (one read syscall away from `ureq`'s timeout, forever)
        // would still spawn a fresh background thread every
        // `TOTP_POLL_INTERVAL`, piling up indefinitely instead of the single
        // outstanding poll this is meant to bound it to.
        assert!(!should_start_totp_poll(true, true, true));
    }
}

#[cfg(test)]
mod totp_poll_result_is_current_tests {
    // Backgrounding the poll (see `should_start_totp_poll_tests` above) means
    // a result can land after the user has already selected a different item
    // than the one it was fetched for -- nothing blocks waiting for it. These
    // pin that a late result is only ever applied to the selection it was
    // actually fetched for, so a poll in flight for item A can never blank or
    // overwrite a currently-valid code showing for a since-selected item B.
    use super::totp_poll_result_is_current;

    #[test]
    fn a_result_for_the_still_selected_item_is_current() {
        assert!(totp_poll_result_is_current("item-1", Some("item-1")));
    }

    #[test]
    fn a_result_for_a_no_longer_selected_item_is_stale() {
        // The user switched from item A to item B before A's poll returned.
        assert!(!totp_poll_result_is_current("item-a", Some("item-b")));
    }

    #[test]
    fn a_result_landing_after_the_selection_was_cleared_is_stale() {
        assert!(!totp_poll_result_is_current("item-1", None));
    }
}

#[cfg(test)]
mod totp_state_removed_secret_tests {
    // Regression tests for review Important 1 on the independent review of
    // a7b33cb: an item with TOTP selected and fetched, whose secret is then
    // removed elsewhere (edited on another device, a sync reload lands),
    // must not keep showing the last-fetched code under a live-looking
    // countdown just because the poll that would have cleared it is now
    // gated off. `run`'s per-frame TOTP block calls
    // `totp_state_for_secret_presence` unconditionally, before the poll-
    // gated branch, so these tests exercise the exact function `run` uses
    // rather than a re-implementation of it.
    use super::totp_state_for_secret_presence;
    use crate::vault_window::detail::TotpState;

    #[test]
    fn a_live_code_is_cleared_the_moment_the_secret_is_gone() {
        let previous = TotpState::Code { code: "111111".to_string(), seconds_left: 12 };

        let next = totp_state_for_secret_presence(false, previous);

        assert_eq!(next, TotpState::NoSecret, "removing the secret must clear a stale code immediately");
    }

    #[test]
    fn an_unavailable_state_also_clears_when_the_secret_is_gone() {
        let next = totp_state_for_secret_presence(false, TotpState::Unavailable);

        assert_eq!(next, TotpState::NoSecret);
    }

    #[test]
    fn no_secret_stays_no_secret_when_the_secret_is_still_absent() {
        let next = totp_state_for_secret_presence(false, TotpState::NoSecret);

        assert_eq!(next, TotpState::NoSecret);
    }

    #[test]
    fn a_live_code_is_left_untouched_while_the_secret_is_still_present() {
        // The presence check must not itself clobber a code that's still
        // valid -- only the poll (a separate step) replaces it.
        let previous = TotpState::Code { code: "111111".to_string(), seconds_left: 12 };

        let next = totp_state_for_secret_presence(true, previous.clone());

        assert_eq!(next, previous);
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

#[cfg(test)]
mod spawn_vault_load_tests {
    // Regression tests for final review Important 1: a forced refresh must
    // wait for `bw serve` to be ready before `populate()`, and a populate
    // that never becomes ready must be reported as `Err`, not silently
    // swallowed into "send whatever's already cached as if it were fresh".
    use super::spawn_vault_load_with_schedule;
    use crate::vault_bridge::VaultBridge;
    use crate::vault_cache::VaultCache;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    fn items_body() -> &'static str {
        r#"{"success":true,"data":{"data":[{"id":"1","name":"A","fields":[]}]}}"#
    }

    fn folders_body() -> &'static str {
        r#"{"success":true,"data":{"data":[]}}"#
    }

    #[test]
    fn a_forced_refresh_populates_and_sends_ok_when_bw_serve_is_ready() {
        let mut server = mockito::Server::new();
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();

        let cache = Arc::new(VaultCache::new(VaultBridge::new(server.url())));
        let (tx, rx) = mpsc::channel();
        // Empty schedule: the mock answers on the very first attempt, so
        // there is nothing to retry regardless -- this only proves an empty
        // schedule doesn't itself block a successful readiness check.
        spawn_vault_load_with_schedule(cache, tx, true, 1, false, vec![]);

        let (generation, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        assert_eq!(generation, 1, "the result must be tagged with the generation it was spawned with");
        let (items, folders) = result.expect("bw serve was ready; load must succeed");
        assert_eq!(items.len(), 1);
        assert!(folders.is_empty());
    }

    #[test]
    fn a_forced_refresh_reports_err_instead_of_stale_data_when_bw_serve_never_answers() {
        // Nothing is listening at this URL at all, so every readiness attempt
        // fails immediately (connection refused) -- an empty schedule means
        // that single failure is also the last one, so this resolves fast
        // rather than waiting out the real READINESS_DEADLINE.
        let cache = Arc::new(VaultCache::new(VaultBridge::new("http://127.0.0.1:1")));
        let (tx, rx) = mpsc::channel();
        spawn_vault_load_with_schedule(cache, tx, true, 7, false, vec![]);

        let (generation, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        assert_eq!(generation, 7);
        assert!(
            result.is_err(),
            "a populate that never becomes ready must be reported as a failure, not silently \
             mapped to whatever (if anything) was already cached"
        );
    }

    #[test]
    fn skip_readiness_wait_avoids_the_redundant_list_items_probe() {
        // Regression test for final review Minor 3: when the caller already
        // knows `bw serve` is up (`skip_readiness_wait: true`), the
        // `wait_for_vault_ready` probe -- itself a `list_items()` call --
        // must be skipped entirely, leaving only the one `list_items()`
        // `populate()` itself makes. `.expect(1)` fails the mock (and this
        // test) if the endpoint is hit more than once.
        let mut server = mockito::Server::new();
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .expect(1)
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();

        let cache = Arc::new(VaultCache::new(VaultBridge::new(server.url())));
        let (tx, rx) = mpsc::channel();
        spawn_vault_load_with_schedule(cache, tx, true, 1, true, vec![]);

        let (_, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        assert!(result.is_ok(), "populate() must still run and succeed even with the wait skipped");
        items.assert();
    }

    #[test]
    fn without_the_skip_the_readiness_probe_hits_list_items_before_populate_does() {
        // The other half of the regression guard above: with
        // `skip_readiness_wait: false` (the default for an unknown backend
        // state), `list_items()` is hit twice -- once by
        // `wait_for_vault_ready`, once by `populate()` -- so this is the
        // behaviour Minor 3's exemption must NOT apply when the caller does
        // not already know the backend is up.
        let mut server = mockito::Server::new();
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .expect(2)
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();

        let cache = Arc::new(VaultCache::new(VaultBridge::new(server.url())));
        let (tx, rx) = mpsc::channel();
        spawn_vault_load_with_schedule(cache, tx, true, 1, false, vec![]);

        let (_, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        assert!(result.is_ok());
        items.assert();
    }
}

#[cfg(test)]
mod apply_vault_load_result_tests {
    // Regression tests for final review Important 2: a load result must
    // never contradict what's actually displayed, in either direction --
    // neither the original bug (stale data shown under a claimed-fresh
    // "Synced" pill) nor its inverse, newly introduced by the same fix
    // (fresh, correct data shown under a "Sync failed" pill because a
    // slower, superseded load's failure arrived after a faster, newer
    // load's success already landed).
    use super::apply_vault_load_result;
    use crate::vault_bridge::VaultItem;

    fn item(id: &str) -> VaultItem {
        serde_json::from_str(&format!(r#"{{"id":"{id}","name":"{id}","fields":[]}}"#)).unwrap()
    }

    // `VaultItem` has no `PartialEq` (its `other` field is arbitrary JSON,
    // and nothing else needs to compare items for equality) -- these tests
    // only care that the right items ended up in the list, so they compare
    // ids instead of asserting on the items themselves.
    fn ids(items: &[VaultItem]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn a_stale_failure_does_not_override_a_newer_success() {
        // This is the exact scenario the review flagged: generation 1 (the
        // initial load) is still in flight when generation 2 (the post-sync
        // forced reload) is spawned and succeeds first, setting
        // `sync_status` to `Ok(())` and the list to fresh data. Generation
        // 1's failure then arrives late. Without the generation check, that
        // failure would flip `sync_status` to `Err` over data that was, in
        // fact, just correctly refreshed -- correct data under a lying "Sync
        // failed" pill.
        let mut items = vec![item("2")];
        let mut folders = Vec::new();
        let mut vault_loading = false;
        let mut selected_id = Some("2".to_string());
        let mut sync_status = Some(Ok(()));

        apply_vault_load_result(
            1, // this result's generation
            2, // the latest generation actually spawned
            Err("connection refused".to_string()),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut selected_id,
            &mut sync_status,
        );

        assert_eq!(
            sync_status,
            Some(Ok(())),
            "a superseded (stale) load failure must not overwrite a newer load's success"
        );
        assert_eq!(ids(&items), vec!["2"], "the fresh data from the newer load must be left untouched");
    }

    #[test]
    fn a_stale_success_does_not_override_a_newer_failure() {
        // The mirror case: a slow generation-1 success must not silently
        // erase a newer generation-2 failure's warning state, nor
        // resurrect a "Synced" pill the newer load already invalidated.
        let mut items = vec![item("stale")];
        let mut folders = Vec::new();
        let mut vault_loading = false;
        let mut selected_id = Some("stale".to_string());
        let mut sync_status = Some(Err("bw serve never became ready".to_string()));

        apply_vault_load_result(
            1,
            2,
            Ok((vec![item("late")], Vec::new())),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut selected_id,
            &mut sync_status,
        );

        assert_eq!(ids(&items), vec!["stale"], "a superseded (stale) success must not be applied");
        assert_eq!(
            sync_status,
            Some(Err("bw serve never became ready".to_string())),
            "a superseded success must not clear a newer, still-standing failure"
        );
    }

    #[test]
    fn the_current_generation_failure_after_a_reported_sync_success_does_flip_the_pill() {
        // Not every Err is stale: when the *latest* spawn is the one that
        // failed -- e.g. the post-sync forced reload itself never got
        // through -- the pill must still correct itself off of the sync
        // handler's optimistic `Ok(())` (final review Important 1). This is
        // the behaviour Important 2's generation check must not break.
        let mut items = vec![item("pre-sync")];
        let mut folders = Vec::new();
        let mut vault_loading = false;
        let mut selected_id = Some("pre-sync".to_string());
        let mut sync_status = Some(Ok(()));

        apply_vault_load_result(
            2,
            2,
            Err("bw serve never became ready".to_string()),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut selected_id,
            &mut sync_status,
        );

        assert_eq!(sync_status, Some(Err("bw serve never became ready".to_string())));
    }

    #[test]
    fn the_current_generation_success_updates_items_and_folders() {
        let mut items = Vec::new();
        let mut folders = Vec::new();
        let mut vault_loading = true;
        let mut selected_id = None;
        let mut sync_status = None;

        apply_vault_load_result(
            1,
            1,
            Ok((vec![item("a"), item("b")], Vec::new())),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut selected_id,
            &mut sync_status,
        );

        assert_eq!(ids(&items), vec!["a", "b"]);
        assert!(!vault_loading);
        assert_eq!(selected_id, Some("a".to_string()), "the first item is selected once nothing was selected yet");
        assert_eq!(sync_status, None, "a load with no preceding sync claim must not invent one");
    }
}

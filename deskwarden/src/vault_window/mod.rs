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
use crate::vault_cache::{PopulateOutcome, VaultCache, VaultEra, VaultSnapshot, VaultUnavailable};
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
/// The size this window opens at the very first time, before it has ever been
/// closed and had its geometry recorded. Design 2b's own 1240x740.
///
/// Every later launch uses `Settings::vault_window` instead, run through
/// `settings::clamp_window_geometry` -- see `initial_placement`.
const WINDOW_SIZE: [f32; 2] = [1240.0, 740.0];
const SIDEBAR_WIDTH: f32 = 212.0;
const LIST_WIDTH: f32 = 390.0;

/// TOTP is re-fetched from `bw serve` on this interval while an item with a
/// code is selected -- cheap enough to poll (one local HTTP call) and far
/// simpler than implementing the TOTP algorithm ourselves when `bw serve`
/// already exposes the current code directly.
const TOTP_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How often the vault window wakes itself when nothing is animating.
///
/// This is NOT a frame rate: egui repaints on input regardless. It is the
/// deadline by which a window that nobody is touching must run a frame anyway,
/// and everything in `run`'s closure that is time-driven rather than
/// input-driven depends on it -- the AUTO-LOCK check, the "Synced N min ago"
/// pill, and the drains of the four background channels (`sync_rx`,
/// `vault_rx`, `favicon_rx`, `totp_rx`), none of which can be noticed on a
/// frame that never runs. Requested UNCONDITIONALLY at the top of the frame
/// closure, above every early return; see the call site.
const FRAME_INTERVAL: Duration = Duration::from_millis(500);

/// The tighter cadence the loading body asks for on top of [`FRAME_INTERVAL`],
/// so the spinner animates smoothly and a landed load is painted promptly.
/// Roughly one 60Hz refresh.
const LOADING_FRAME_INTERVAL: Duration = Duration::from_millis(16);

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

/// The size and position to open this window at, given whatever the last
/// session recorded and the monitors that exist now.
///
/// The `None` arm is the only thing separating "never been closed yet" from
/// "closed at design 2b's own size": on a first run there is nothing to
/// clamp, so the window opens at [`WINDOW_SIZE`] and the OS places it. Every
/// other case is [`settings::clamp_window_geometry`]'s, which is where all
/// the actual rules live.
fn initial_placement(
    saved: Option<crate::settings::WindowGeometry>,
    work_areas: &[crate::settings::WorkArea],
) -> crate::settings::WindowPlacement {
    match saved {
        Some(geometry) => crate::settings::clamp_window_geometry(geometry, work_areas),
        None => crate::settings::WindowPlacement {
            width: WINDOW_SIZE[0] as i32,
            height: WINDOW_SIZE[1] as i32,
            position: None,
        },
    }
}

/// The geometry worth writing back to `settings.json` this frame, or `None`
/// if this frame's window state does not describe one.
///
/// Three states are deliberately excluded, and each would otherwise be
/// persisted as the *restored* size:
///
///  * **Maximized.** Its rect is the whole work area. Recorded, a user who
///    maximizes once gets a window that opens filling the screen forever, and
///    un-maximizing restores it to... the whole screen.
///  * **Minimized.** winit reports no position at all for a minimized window
///    (`update_viewport_info` sets both rects to `None` for it), so there is
///    nothing here to record in the first place -- but saying so is what stops
///    a future egui reporting some placeholder rect from being believed.
///  * **A non-finite rect**, which no comparison in `clamp_window_geometry`
///    could then reject, since that function's `i32`s cannot represent it.
fn geometry_to_record(
    inner_rect: Option<egui::Rect>,
    maximized: bool,
    minimized: bool,
) -> Option<crate::settings::WindowGeometry> {
    if maximized || minimized {
        return None;
    }
    let rect = inner_rect?;
    if !rect.min.x.is_finite()
        || !rect.min.y.is_finite()
        || !rect.width().is_finite()
        || !rect.height().is_finite()
    {
        return None;
    }
    Some(crate::settings::WindowGeometry {
        x: rect.min.x.round() as i32,
        y: rect.min.y.round() as i32,
        width: rect.width().round() as i32,
        height: rect.height().round() as i32,
    })
}

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
    //
    // The payload is ONE `VaultSnapshot`, not a `(Vec<VaultItem>,
    // Vec<Folder>)` pair, and that is where the "items and folders cannot
    // come from two eras" guarantee actually lives (review 29's Important
    // 1). It used to be a pair, so both send sites hand-assembled
    // `Ok((snapshot.items, snapshot.folders))` -- and `Ok((snapshot.items,
    // cache.folders()))` compiled at exactly that spot, which is the
    // spelling this crate has now written by accident fourteen times. The
    // guarantee was being claimed one layer too early, inside
    // `vault_load_step`, which is the one place nobody was going to write
    // it. A snapshot arrives here as the single value
    // `snapshot_unless_superseded` built under one lock acquisition.
    let (vault_tx, vault_rx): (
        mpsc::Sender<(u64, Result<VaultSnapshot, VaultLoadFailure>)>,
        Receiver<(u64, Result<VaultSnapshot, VaultLoadFailure>)>,
    ) = mpsc::channel();
    // The generation of the most recently spawned `spawn_vault_load` call.
    // Incremented immediately before every spawn (both below and at the
    // post-sync reload further down) so it always names the newest spawn;
    // a result read from `vault_rx` whose own tag doesn't match this value
    // is from a spawn that has since been superseded and is dropped outright
    // rather than applied -- see the drain below.
    let mut load_generation: u64 = 0;
    load_generation += 1;
    // THE VAULT SESSION THIS WINDOW IS SHOWING. Captured ONCE, here, on the
    // main thread, before anything is painted -- and it is the era every
    // load this window ever spawns is checked against, including the
    // post-sync reload further down (review 29's Important 2).
    //
    // It used to be re-read at each spawn, which made it answer a strictly
    // weaker question: "is this still the session that existed when I
    // spawned?". Re-read at spawn time it equals the current era BY
    // CONSTRUCTION, so it could only ever detect a `clear` landing after its
    // own capture -- and the call site below claimed it answered "is this
    // still the vault session the window is showing?", which it did not. The
    // gap was real, not rhetorical: window opens on account A and paints it,
    // a `clear` plus a re-populate under account B lands, the user clicks
    // Sync, the reload re-captures B's era, compares equal, and paints
    // ACCOUNT B'S VAULT INTO A WINDOW SHOWING ACCOUNT A -- no `Superseded`,
    // no `Err`, nothing on screen. It is unreachable today only because
    // every production `cache.clear()` runs on the main thread this function
    // occupies, and thread affinity is precisely the argument this project
    // keeps having to un-write (see `spawn_vault_load_with_schedule`'s
    // `DiscardedStale` arm, which deliberately no longer rests on it).
    //
    // Captured once, the era binds this window to ONE vault session for its
    // whole lifetime, which is what the window's contents already claim. A
    // window whose session ended has nothing true left to show, and the
    // paths that end one (lock, re-auth) close it and return anyway -- so
    // this costs nothing in the reachable world and refuses in the one the
    // reviewer described. `window_era_placement_tests` pins the single
    // capture in the source, because nothing in the type system stops a
    // future edit from re-reading the cache at a spawn site.
    let window_era = cache.epoch().era();
    // Cloned because the update closure below move-captures both, and needs
    // its own pair to re-issue a load after each sync. `force_refresh:
    // false`: the snapshot from unlock (if any) is current, so this only
    // actually hits the backend the first time the window is opened after
    // unlock -- see `spawn_vault_load`'s doc comment. `skip_readiness_wait`:
    // see `backend_already_running`'s own doc -- skips the readiness wait
    // when the caller already knows `bw serve` is up.
    spawn_vault_load(
        cache.clone(),
        vault_tx.clone(),
        VaultLoadRequest {
            force_refresh: false,
            era: window_era,
            generation: load_generation,
            skip_readiness_wait: backend_already_running,
        },
    );
    let mut items: Vec<VaultItem> = Vec::new();
    let mut folders: Vec<Folder> = Vec::new();
    // True until the background load above reports back.
    let mut vault_loading = true;
    // Why the most recent applied load produced nothing to paint, or `None`
    // if it produced something (review 29's Minor 3). Two things read it,
    // and neither could be answered from `sync_status`:
    //
    //  * `vault_body_state` -- a give-up at the INITIAL load used to leave a
    //    blank window under a neutral "Sync" pill, because the `Err` arm's
    //    "keep whatever is on screen" is vacuous when nothing is on screen
    //    yet.
    //  * `sync_pill` -- a give-up after a SUCCESSFUL sync used to paint a red
    //    "Sync failed", which is a different untruth about the same event.
    //
    // Cleared by any load that does paint, so it never outlives the state it
    // describes.
    let mut vault_load_error: Option<String> = None;
    let mut filter = SidebarFilter::All;
    let mut search = String::new();
    // Nothing to select yet -- set from the first item once the load lands.
    let mut selected_id: Option<String> = None;
    // Tracks the previous frame's `selected_id` so a change (from clicking a
    // different row in `draw_item_list`) can be detected and used to reset
    // the per-selection state below (`mode`, `reveal`, the TOTP
    // cache) -- see the reset block after the item-list panel further down.
    let mut last_selected_id: Option<String> = selected_id.clone();
    let mut mode = DetailMode::Read;
    // Every masked value the read pane can reveal -- a login's password, a
    // card's number and its security code. It lives HERE, in `run`'s
    // per-selection state, and not inside the pane's closure: a `let mut
    // revealed = false` declared per frame is dropped at the end of that
    // frame, so the toggle flips and the next frame draws masked again. See
    // `RevealState`'s doc; that bug has been shipped once already, in
    // `detail_edit.rs`. Cleared by the selection-change reset block below,
    // so a revealed card number cannot follow the user onto the next item.
    let mut reveal = detail::RevealState::default();
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
    //
    // Tagged `(load_generation, item_id, result)`: the id alone said which
    // ITEM a result belongs to but not which vault state it was fetched
    // against, so a poll issued before a reload could be applied after it
    // and undo the reload's re-arm (review 15's Minor 5) -- see
    // `totp_poll_result_is_current`.
    let (totp_tx, totp_rx): (
        mpsc::Sender<(u64, String, Result<Option<String>, VaultError>)>,
        Receiver<(u64, String, Result<Option<String>, VaultError>)>,
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
    // Where this window was when it was last closed, re-homed onto the
    // monitors that exist right now. Read here, on the main thread, before
    // the window exists -- `Settings::load` is a small file read and every
    // failure in it (missing, partial, corrupt) already falls back to
    // defaults, so this cannot be a reason the window does not open.
    let settings_path = crate::settings::default_path();
    let placement = initial_placement(
        settings_path
            .as_deref()
            .and_then(|path| crate::settings::Settings::load(path).vault_window),
        &crate::login_ui::monitor_work_areas(),
    );
    // The geometry to write back on close, updated every frame the window is
    // in an ordinary (neither maximized nor minimized) state. An
    // `Rc<RefCell<_>>` for the same reason `locked` and `needs_reauth` are:
    // the update closure is `FnMut + 'static` and cannot hand anything back
    // directly. Written to disk ONCE, after the event loop returns -- a
    // per-frame write would be a file write per pixel of a resize drag.
    let last_geometry: Rc<RefCell<Option<crate::settings::WindowGeometry>>> =
        Rc::new(RefCell::new(None));
    let last_geometry_for_closure = last_geometry.clone();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([placement.width as f32, placement.height as f32])
        // `with_resizable(true)` alone is INERT here: `with_decorations(false)`
        // means there is no OS frame to grab, which is exactly the bug this
        // window shipped with. It is still set because it is what makes the
        // OS treat the window as sizable at all (maximize, snap, and the
        // `BeginResize` drag `login_ui::draw_resize_handles` starts all go
        // through it); the actual grabbable edges are drawn by that function,
        // called at the top of the frame closure below.
        .with_resizable(true)
        // Enforced by the OS during the resize drag, so the user cannot pull
        // an edge past it -- the other half of the floor
        // `settings::clamp_window_geometry` applies to a *stored* size. Both
        // are needed: this one cannot stop a bad value on disk, and that one
        // cannot stop a drag.
        .with_min_inner_size([
            crate::settings::MIN_VAULT_WINDOW_SIZE.0 as f32,
            crate::settings::MIN_VAULT_WINDOW_SIZE.1 as f32,
        ])
        .with_decorations(false)
        .with_icon(theme::window_icon());
    if let Some((x, y)) = placement.position {
        viewport = viewport.with_position([x as f32, y as f32]);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, _frame| {
        if !styled {
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            round_window_corners(WINDOW_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        // The eight grabbable edges/corners. FIRST, before anything else this
        // frame, and deliberately above every early return below (auto-lock,
        // the loading spinner, the unavailable body): a window that is still
        // loading, or that failed to load, is still a window the user is
        // allowed to resize. It costs nothing to be here rather than at the
        // end -- the zones live in their own `Order::Foreground` layer, so
        // egui hit-tests them ahead of the panels regardless of call order
        // (see `draw_resize_handles`).
        //
        // The login and preferences windows do not call this, which is the
        // whole of how they stay fixed-size.
        crate::login_ui::draw_resize_handles(ui.ctx());

        // EVERY frame schedules the next one, and it is done HERE -- above the
        // body match, next to `draw_resize_handles`, for the same structural
        // reason that call is here (review 31's Important 1).
        //
        // This used to sit at the TAIL of the closure, which was true only
        // while the loading branch was the sole early return: `Loading` asked
        // for its own faster cadence before returning, and everything else
        // fell through to the tail. Then `Unavailable` was added as a second
        // early return and got NEITHER -- so a window whose initial load had
        // failed rendered its error page and then stopped repainting
        // altogether. Nothing below this line is what needs the frame:
        // `last_activity`/AUTO-LOCK, the `sync_rx` drain that spawns the
        // post-sync reload, the `vault_rx` drain, `favicon_rx` and `totp_rx`
        // all sit above the body match, so a Sync click from the error page
        // spawned a thread whose result no frame ever drained, the pill sat on
        // "Syncing..." until the pointer moved, and the vault stayed unlocked
        // indefinitely with `bw serve` up -- while `run` held the main thread,
        // blocking the tray, the global hotkey and the window watcher too.
        //
        // Hoisted rather than repeated in the `Unavailable` arm: three call
        // sites agreeing is what failed once already. Here, "every frame
        // schedules its successor" is true by construction, and a branch that
        // wants a tighter cadence (the spinner's 16ms) simply asks for one --
        // egui keeps the smallest request of the frame, which
        // `frame_schedule_placement_tests::the_tightest_request_in_a_frame_wins`
        // asserts against the real `egui::Context` rather than assuming.
        ui.ctx().request_repaint_after(FRAME_INTERVAL);

        // Where the window is right now, kept for the write that happens
        // after the event loop returns. `None` on the frames that describe
        // no restorable geometry (maximized/minimized) LEAVES THE PREVIOUS
        // VALUE STANDING rather than clearing it: a window that is maximized
        // when the user closes it should reopen at whatever size it had
        // before being maximized, not at the default.
        if let Some(geometry) = geometry_to_record(
            ui.ctx().input(|i| i.viewport().inner_rect),
            ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false)),
            ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false)),
        ) {
            *last_geometry_for_closure.borrow_mut() = Some(geometry);
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
                &mut vault_load_error,
                &mut selected_id,
                &mut sync_status,
                &mut totp_state,
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
                //
                // `window_era` -- the era captured ONCE before this loop, not
                // re-read here (review 29's Important 2). That is what makes
                // the sentence this comment used to end on actually true: it
                // asks "is this still the vault session the window is
                // SHOWING?", which is a genuinely different question from
                // `load_generation` one line up ("has this window spawned a
                // newer load?"). A lock or a re-auth moves the era and no
                // generation at all. Re-read here it would instead ask "is
                // this still the session that existed one instruction ago",
                // which is answered "yes" by construction. See `window_era`'s
                // declaration for the account-A/account-B scenario that
                // distinguishes them.
                load_generation += 1;
                spawn_vault_load(
                    cache.clone(),
                    vault_tx.clone(),
                    VaultLoadRequest {
                        force_refresh: true,
                        era: window_era,
                        generation: load_generation,
                        skip_readiness_wait: backend_already_running,
                    },
                );
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
        if let Ok((generation, item_id, poll_result)) = totp_rx.try_recv() {
            // Cleared before any staleness check, deliberately: a dropped
            // result still means the thread that held this flag has
            // finished, and gating the clear on currency is exactly how it
            // would latch and wedge every later poll.
            totp_poll_in_flight = false;
            // A poll only ever updates `totp_state` if it's still for the
            // selected item -- one spawned for item A can land after the
            // user has since selected item B (nothing here blocks waiting
            // for it), and applying it then would show A's code, or A's
            // failure, under B's row. Dropped silently, the same way a
            // superseded vault load is (`apply_vault_load_result`); B's own
            // poll (already in flight or about to be spawned) is what
            // determines what B's row shows.
            if totp_poll_result_is_current(&item_id, selected_id.as_deref(), generation, load_generation) {
                let seconds_left = current_totp_seconds_left();
                let before = totp_state.clone();
                let error = apply_totp_poll_result(poll_result, seconds_left, &mut totp_state);
                // `Ok(None)` is not a quiet success: at this call site it can
                // only mean the item we hold says it has a TOTP seed and
                // `bw serve` will not produce a code for it (review 14's
                // Important). Logged on the transition only -- see
                // `entered_no_code_reported`'s doc.
                if entered_no_code_reported(&before, &totp_state) {
                    log::warn!(
                        "bw serve reports no current one-time code for {item_id}, but this \
                         item's login data carries a TOTP seed -- showing the row as \
                         \"no code available\"; polling for it stops until the selection \
                         changes or a vault reload lands"
                    );
                }
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
                        // Not every non-error is a recovery (review 15's
                        // nit): an outage that flips to a `400` yields
                        // `Ok(None)`, so the log read "no current one-time
                        // code ..." immediately followed by "TOTP fetch
                        // recovered", two lines that contradict each other
                        // about the same poll.
                        if totp_poll_failing && poll_success_is_a_recovery(&totp_state) {
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
            //
            // The wording decision itself lives in `sync_pill` so it can be
            // asserted (review 29's Minor 3) -- this pill is the only place
            // any of these states is spelled for the user.
            let (dot, label) = sync_pill(
                sync_in_progress,
                sync_status.as_ref(),
                vault_load_error.as_deref(),
                last_sync_at.map_or(Duration::ZERO, |t| t.elapsed()),
            );
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
        //
        // The same slot also carries the case where the load gave up with
        // nothing on screen to keep (review 29's Minor 3) -- see
        // `vault_body_state`, which decides between the two.
        match vault_body_state(vault_loading, items.is_empty(), vault_load_error.as_deref()) {
            VaultBodyState::Loading => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme::CANVAS))
                    .show(ui, |ui| {
                        let available = ui.available_height();
                        ui.vertical_centered(|ui| {
                            // Roughly half the spinner-plus-label block, so
                            // the pair sits centred rather than the spinner
                            // alone.
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
                // A REFINEMENT of the unconditional request at the top of this
                // closure, not a replacement for it: egui keeps the smallest
                // request made during a frame, so this tightens `FRAME_INTERVAL`
                // to roughly one display refresh for as long as the load is in
                // flight. It drives the spinner's animation and how promptly
                // the landed load is noticed. If this line is ever deleted, the
                // window still repaints -- just at the slower idle cadence.
                ui.ctx().request_repaint_after(LOADING_FRAME_INTERVAL);
                return;
            }
            // No spinner: nothing is in flight, and the way out is the
            // toolbar's Sync button, which is drawn above this and stays
            // clickable. It does NOT follow that this state needs no frames --
            // the older comment here said exactly that, and it was wrong
            // (review 31's Important 1): the Sync button spawns a background
            // thread whose result needs a frame to be drained, and the
            // auto-lock deadline needs one to be evaluated at all. The
            // unconditional `request_repaint_after` at the top of this closure
            // covers this arm; this arm deliberately adds nothing of its own,
            // because there is no animation here to drive.
            //
            // The reason shown is the loader's own words
            // (`VAULT_SUPERSEDED_BEFORE_LOAD` and friends), not a rewrite of
            // them here -- a second wording would be a second thing to keep
            // true.
            VaultBodyState::Unavailable(reason) => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme::CANVAS))
                    .show(ui, |ui| {
                        let available = ui.available_height();
                        ui.vertical_centered(|ui| {
                            ui.add_space((available / 2.0 - 30.0).max(0.0));
                            ui.label(
                                egui::RichText::new("Your vault could not be loaded")
                                    .size(15.0)
                                    .color(theme::INK),
                            );
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(reason).size(13.0).color(theme::TEXT_FAINT));
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("Click Sync to try again.")
                                    .size(13.0)
                                    .color(theme::TEXT_FAINT),
                            );
                        });
                    });
                return;
            }
            VaultBodyState::Vault => {}
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
        // NO INNER MARGIN, unlike the sidebar's. Design 2b gives this pane a
        // white toolbar strip that spans its full width and a list area with
        // its own, different padding beneath -- one panel margin cannot be
        // both, and a margin here would inset the strip so it read as a card
        // floating on grey rather than the tile the design draws. Both
        // paddings live in `draw_item_list` instead; see its header comment.
        egui::Panel::left("vault-item-list")
            .exact_size(LIST_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::CANVAS))
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
            reveal = detail::RevealState::default();
            // `NoSecret` is this reset block's neutral "haven't looked yet"
            // value, and it is deliberately the one value the per-frame TOTP
            // block's `totp_state_for_secret_presence` promotes: on the very
            // next line of that block, still in this same frame, an item
            // that does have a seed becomes `Fetching` (so the row is
            // present and honest while the background poll is out) and one
            // that doesn't stays `NoSecret`. What this line actually
            // guarantees is only that nothing from the *previous* selection
            // -- a code, an `Unavailable`, or a `NoCodeReported` -- survives
            // into this one; it does not, and since the poll moved onto a
            // background thread cannot, guarantee a fetched code is in place
            // by render time. (The comment that used to sit here claimed
            // exactly that -- review 12 flagged it as false.)
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
                            // There is deliberately no `item_type != Some(1)`
                            // early return here any more. There used to be
                            // one, drawing the name over "This item type
                            // isn't editable in Deskwarden yet." and
                            // returning -- which meant `draw_detail_read` was
                            // *never called* for a card, a note, an identity
                            // or an SSH key. Every kind-aware decision in
                            // `detail.rs` would have been correct and inert,
                            // which is this repository's most-repeated defect
                            // shape. The pane is now kind-aware itself
                            // (`detail::detail_body_for`), so this bails out
                            // of nothing.
                            //
                            // What that guard was protecting is still
                            // protected, one layer down and more precisely:
                            // the *edit* form is still login-shaped, so
                            // `detail::kind_offers_edit` draws no Edit button
                            // for a kind `EditDraft` would corrupt (see its
                            // doc). Read and Delete are safe for every kind.
                            //
                            // That removal is now pinned by a test rather
                            // than by this comment:
                            // `draw_read_arm_tests::the_read_arm_paints_a_
                            // real_pane_for_every_kind` fails for all five
                            // non-login kinds if any early return reappears
                            // inside `draw_read_arm`.

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
                                totp_last_poll.elapsed() >= TOTP_POLL_INTERVAL,
                                totp_poll_in_flight,
                                totp_state_wants_poll(&totp_state),
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
                                // Tagged with the vault state this poll is
                                // being fetched against, so a reload
                                // spawned while it is in flight can drop it
                                // rather than let it overwrite the state
                                // that reload re-armed (review 15's Minor
                                // 5) -- see `totp_poll_result_is_current`.
                                let generation = load_generation;
                                std::thread::spawn(move || {
                                    let result = bridge.get_totp(&item_id);
                                    let _ = tx.send((generation, item_id, result));
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

                            // Everything this arm *draws* -- the pane and the
                            // Ctrl+Shift+F gate over it -- lives in
                            // `draw_read_arm` rather than here, so it can be
                            // driven headlessly by a test. See that
                            // function's doc for why that is not a stylistic
                            // preference on this particular arm.
                            let action = draw_read_arm(
                                ui,
                                item,
                                fill_count,
                                &totp_state,
                                delete_pending,
                                &mut reveal,
                                icons.textures.get(item.id.as_str()),
                            );
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
                                // The two card secrets are looked up from the
                                // item here rather than carried on the action,
                                // exactly as the password above is: that keeps
                                // the plaintext inside the `Zeroizing` it
                                // arrived in for as long as this app controls
                                // it. (The clipboard is one of the escape
                                // routes `deskwarden/README.md` already
                                // documents; that is unchanged here.)
                                //
                                // Both go through `detail::card_fields`, the
                                // pane's own formatter, rather than reading
                                // the raw field: `card_fields` trims, the
                                // rendered row is trimmed, and a number stored
                                // as `" 4242… "` used to display trimmed and
                                // copy with the whitespace some payment forms
                                // reject. There is now exactly one producer of
                                // a card's displayed text, so what is copied
                                // is what is on screen.
                                DetailAction::CopyCardNumber => {
                                    if let Some(number) =
                                        item.card.as_ref().and_then(|c| detail::card_fields(c).number)
                                    {
                                        ui.ctx().copy_text(number);
                                    }
                                }
                                DetailAction::CopyCardCode => {
                                    if let Some(code) = item.card.as_ref().and_then(|c| detail::card_fields(c).code) {
                                        ui.ctx().copy_text(code);
                                    }
                                }
                                // A non-secret row (the card's cardholder
                                // name, brand and expiry, and every identity
                                // field) hands its own already-rendered value
                                // back -- see `DetailAction::CopyValue`.
                                DetailAction::CopyValue(value) => {
                                    ui.ctx().copy_text(value);
                                }
                                DetailAction::CopyTotp => {
                                    // Only `Code` has anything to copy --
                                    // `NoSecret`, `Fetching`,
                                    // `NoCodeReported` and `Unavailable` all
                                    // have no valid current code (the detail pane
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
                                                // `reveal`/
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

        // NOTHING SCHEDULES THE NEXT FRAME HERE ANY MORE. The
        // `request_repaint_after` that used to close this closure was reachable
        // only in the `Vault` state, because two branches of the body match
        // return before it -- see the hoisted call at the TOP of the closure,
        // next to `draw_resize_handles`, and review 31's Important 1.
    });

    // One write, here, after the window is gone -- not per frame, which
    // during a resize drag would be a file write per repaint. A failure is
    // logged and otherwise ignored: losing the remembered size is a smaller
    // problem than anything worth failing a lock/close over, and
    // `Settings::load` treats whatever is (or is not) on disk as advisory
    // anyway. Read-modify-write, so a preference changed in the preferences
    // window while this one was open is not reverted -- see
    // `persist_vault_window_geometry`.
    if let (Some(path), Some(geometry)) = (settings_path.as_deref(), *last_geometry.borrow()) {
        if let Err(e) = crate::settings::Settings::persist_vault_window_geometry(path, geometry) {
            log::warn!("could not save the vault window's geometry: {e}");
        }
    }

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
/// `has_totp_secret` is false, leaving it untouched otherwise -- except for
/// promoting a bare `NoSecret` to `Fetching` when a secret *is* present
/// (review 12's Important 3). Called unconditionally, every frame, before
/// the poll-gated branch in `run`.
///
/// The false-branch behaviour is the fix for review Important 1 (independent
/// review of a7b33cb): `totp_state` used to only reset on *selection
/// change*, so an item with TOTP selected and fetched, whose secret was then
/// removed elsewhere (a sync reload landing mid-session, say), kept
/// rendering the last-fetched code under a live-looking countdown forever --
/// the poll that would have cleared it was gated off by the very same
/// `has_totp_secret` that had gone false.
///
/// The `NoSecret` -> `Fetching` promotion closes a gap the TOTP poll's move
/// to a background thread opened: `run`'s selection-change reset
/// unconditionally sets `totp_state` to `NoSecret` (a neutral "haven't
/// looked yet" placeholder its own comment says this function overwrites
/// "before render"), which was true only while the poll itself ran inline,
/// synchronously, within the same per-frame block that comment refers to.
/// Now the poll is a one-shot background thread whose result lands later,
/// over `totp_rx` -- so without this promotion, a freshly selected item with
/// a real secret rendered no row at all (`NoSecret` draws nothing, see
/// `detail::draw_detail_read`) for as long as the fetch took, up to the
/// ~10s `ureq` read timeout if a *different*, since-deselected item's poll
/// was still outstanding and holding `totp_poll_in_flight`. A `Code` or
/// `Unavailable` previous state is left untouched either way -- there is
/// already something honest to show, and a fresh poll (once one starts) is
/// what should replace it, not this presence check.
///
/// `NoCodeReported` is likewise left untouched, and that is the whole point
/// of it existing as its own variant (review 13's Important). It used to be
/// spelled `NoSecret` -- the same value this function promotes -- so a poll
/// that had just answered "no current code for this item" was promoted
/// straight back to `Fetching` on the very next frame, `should_start_totp_poll`
/// fired another poll a second later, and an item whose stored seed
/// `bw serve` rejects with a `400` (removed on another device before a sync
/// landed, or malformed) rendered "One-time code / Fetching..." forever
/// while hammering the backend once a second for as long as it stayed
/// selected. Because the two situations are now two variants, that promotion
/// is not merely guarded against here, it is unrepresentable: this function
/// cannot see the polled answer at all.
///
/// Pulled out on its own, the same way `apply_totp_poll_result` is, so this
/// transition is directly unit-testable.
fn totp_state_for_secret_presence(has_totp_secret: bool, previous: TotpState) -> TotpState {
    match (has_totp_secret, previous) {
        (false, _) => TotpState::NoSecret,
        (true, TotpState::NoSecret) => TotpState::Fetching,
        (true, TotpState::NoCodeReported) => TotpState::NoCodeReported,
        (true, other) => other,
    }
}

/// Whether the current `TotpState` is one a fresh poll could still improve
/// on -- the second half of review 13's Important, and the reason
/// `NoCodeReported` had to stop polling as well as stop being promoted.
///
/// Every state except `NoCodeReported` wants polling: `Fetching` is waiting
/// for the first answer, `Code` needs refreshing before its 30s window
/// closes, `Unavailable` is a *transient* failure that a later poll should
/// recover from, and `NoSecret` never reaches this function's call site
/// anyway (the call site's `if !has_totp_secret` arm returns first, and the
/// derivation above has already promoted any `NoSecret` to `Fetching` by
/// then). `NoCodeReported` is the one state where the backend has already
/// given a definitive, successful answer for this item, so re-asking every
/// second buys nothing and is exactly the per-second HTTP flood the review
/// found. Selecting the item again re-derives from `NoSecret` and polls
/// normally.
///
/// Exhaustive, no catch-all, for the same reason the render site is: a new
/// variant must be a compile error here, not a silent default.
fn totp_state_wants_poll(state: &TotpState) -> bool {
    match state {
        TotpState::NoSecret | TotpState::Fetching | TotpState::Code { .. } | TotpState::Unavailable => true,
        TotpState::NoCodeReported => false,
    }
}

/// The `TotpState` a landed vault reload should leave behind.
///
/// The re-arm itself is review 14's Important part b and stays: a reload is
/// the only other event besides a selection change that can un-latch
/// `NoCodeReported`, which deliberately stops polling, so a user who fixed
/// the item's seed in the web vault and clicked Sync needs it to fire.
/// `NoSecret` is the neutral "haven't looked yet" value the selection-change
/// reset also uses -- the per-frame presence derivation promotes it back to
/// `Fetching` in the same frame if the (possibly just-updated) item still
/// carries a seed.
///
/// `Code` is the exception, and it is the only one. Two reasons, and the
/// second is why this is a skip rather than a `totp_last_poll` reset:
///  * There is nothing to un-latch. `totp_state_wants_poll` returns true for
///    `Code`, so that state is already polling once per `TOTP_POLL_INTERVAL`
///    and will pick up a changed seed on its own within a second. The
///    re-arm exists for the states that STOPPED polling.
///  * Blanking it is visible. A displayed code becomes "Fetching...",
///    losing the Copy button and shifting the row's layout under a user
///    reaching for it, and the window's own first-frame auto-sync makes that
///    the ordinary case rather than a corner. Resetting `totp_last_poll`
///    alongside the blank would shorten the flicker to one HTTP round-trip
///    but not remove it; skipping removes it entirely.
///
/// A `Code` for an item whose seed has since been removed is not this
/// function's problem either: `totp_state_for_secret_presence` clears it to
/// `NoSecret` on the very next frame, from the freshly loaded item, which is
/// review 9's property and is unaffected by anything here.
///
/// Exhaustive, no catch-all, like every other decision over `TotpState`.
fn totp_state_after_reload(previous: TotpState) -> TotpState {
    match previous {
        TotpState::Code { code, seconds_left } => TotpState::Code { code, seconds_left },
        TotpState::NoSecret | TotpState::Fetching | TotpState::NoCodeReported | TotpState::Unavailable => {
            TotpState::NoSecret
        }
    }
}

/// Whether a poll that returned no *error* actually ended a failure streak.
///
/// `apply_totp_poll_result` reports `None` for both `Ok(Some(code))` and
/// `Ok(None)`, and the drain treated either as a recovery -- so a backend
/// outage that turns into a `400` (`bw serve` answering, but refusing this
/// item's seed) logged `warn: bw serve reports no current one-time code ...`
/// and `info: TOTP fetch ... recovered` back to back about the same poll
/// (review 15's nit). Only a poll that produced a code is a recovery; the
/// streak is left standing for `NoCodeReported`, so if the seed is later
/// fixed and a reload re-arms the poll, the eventual code still logs the
/// recovery the streak was waiting for.
///
/// Exhaustive, no catch-all. The three states that cannot be reached from a
/// successful poll are named rather than defaulted, so a future variant is a
/// compile error here like everywhere else `TotpState` is decided over.
fn poll_success_is_a_recovery(after: &TotpState) -> bool {
    match after {
        TotpState::Code { .. } => true,
        TotpState::NoCodeReported => false,
        // Not reachable from `apply_totp_poll_result`'s `Ok` arms (they
        // write only the two above), and not a claim of recovery either.
        TotpState::NoSecret | TotpState::Fetching | TotpState::Unavailable => false,
    }
}

/// Whether applying a poll result just *entered* `NoCodeReported`, i.e. this
/// is the transition into it rather than another frame already sitting in it.
///
/// The `Ok(None)` arm was the one poll outcome that left no trace anywhere
/// (review 14's Important): `apply_totp_poll_result` returns `None` for the
/// error, so the drain's `None` arm logged nothing and cleared
/// `totp_poll_failing` -- the app's only diagnostic channel said a poll had
/// simply succeeded. Logged on the transition only, exactly the way
/// `totp_poll_failing` already gates the error path, so this cannot become a
/// per-poll flood; `NoCodeReported` stops polling anyway, but the guard is
/// what makes that a property of the logging rather than a coincidence of
/// the poll gate.
///
/// Pulled out as its own predicate for the same reason every other decision
/// in this block is: it is directly testable, and `run`'s closure is not.
fn entered_no_code_reported(before: &TotpState, after: &TotpState) -> bool {
    matches!(after, TotpState::NoCodeReported) && !matches!(before, TotpState::NoCodeReported)
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
/// while a genuine `Ok(None)` ("no code for this item") moves to
/// `NoCodeReported`.
///
/// `Ok(None)` used to land on `NoSecret`, which rendered identically but was
/// also the value the per-frame presence derivation
/// (`totp_state_for_secret_presence`) promotes to `Fetching` -- so this arm
/// was invisible in the live composition and the pane looped
/// `Fetching` -> poll -> `Ok(None)` -> `Fetching` forever, once a second
/// (review 13's Important). `NoCodeReported` exists so that this arm's
/// answer is a state neither the derivation nor the poll gate can undo. See
/// its doc.
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
            *totp_state = TotpState::NoCodeReported;
            None
        }
        Err(e) => {
            *totp_state = TotpState::Unavailable;
            Some(e)
        }
    }
}

/// Everything `run`'s `DetailMode::Read` arm *draws*: the detail pane itself
/// and the Ctrl+Shift+F gate layered over it. Returns the single
/// [`DetailAction`] the arm then acts on; it performs no side effects of its
/// own, so a test can call it with nothing but an `egui::Context`.
///
/// **This is a function, and not a block inside `run`'s closure, because the
/// single most load-bearing line of commit b758f5e had no test.** That commit
/// deleted an `if item.item_type != Some(1) { ...; return; }` early return
/// from the top of this arm. Without that deletion `draw_detail_read` is
/// *never called* for a card, a note, an identity or an SSH key, and every
/// kind-aware decision in `detail.rs` is correct and inert -- this
/// repository's most-repeated defect shape. Reinstating the guard left all
/// 392 tests green. The same hole covered the [`fill_hotkey_applies`] call
/// site: reverting it to a bare `ui.ctx().input(..)` check kept
/// `fill_hotkey_applies_tests` green while the hotkey filled a card again.
///
/// Neither is observable by running the app: the vault this was built against
/// holds 1656 items and every one of them is a login. `draw_read_arm_tests`
/// is the only evidence that exists, and Task 6 will be editing this exact
/// region to make Edit kind-aware.
///
/// The per-frame TOTP block deliberately stays in `run`: it spawns threads,
/// reads `run`'s poll bookkeeping, and its rendering is already pinned from
/// both directions in `detail.rs`. Moving it would restructure the one part
/// of this pane with five findings and a redesign behind it.
fn draw_read_arm(
    ui: &mut egui::Ui,
    item: &VaultItem,
    fill_count: u32,
    totp_state: &TotpState,
    delete_pending: bool,
    reveal: &mut detail::RevealState,
    icon: Option<&egui::TextureHandle>,
) -> DetailAction {
    let mut action = draw_detail_read(
        ui,
        item,
        fill_count,
        totp_state,
        delete_pending,
        reveal,
        icon,
    );
    // Ctrl+Shift+F (spec section 5) is the keyboard equivalent of clicking
    // "Fill in app" -- checked here, not at the top level, because it needs
    // exactly the selected `item` and the button click above doesn't. Gated
    // on the item's kind by the same predicate the button is; see
    // `fill_hotkey_applies`.
    if fill_hotkey_applies(
        crate::vault_bridge::ItemKind::of(item),
        ui.ctx()
            .input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::F)),
    ) {
        action = DetailAction::Fill;
    }
    action
}

/// Whether Ctrl+Shift+F should fill the currently selected item.
///
/// Gated on exactly the predicate the "Fill in app" button is
/// (`detail::kind_offers_fill`), not on a second copy of the rule: the
/// shortcut is that button's keyboard equivalent, so hiding the button for a
/// card while leaving the shortcut live would keep the very door open that
/// hiding it was meant to close -- two empty strings typed into whatever
/// window is focused. Pulled out of the `egui` closure so that pairing is
/// something a test can assert rather than something a reader has to notice.
///
/// Its *call site* is `draw_read_arm`, which is itself a function rather than
/// a block inside `run`'s closure, so `draw_read_arm_tests` can prove the
/// wiring as well as the rule -- reverting the call site to a bare
/// `ui.ctx().input(..)` check used to leave every test here green.
fn fill_hotkey_applies(kind: crate::vault_bridge::ItemKind, pressed: bool) -> bool {
    pressed && detail::kind_offers_fill(kind)
}

/// Whether `run`'s per-frame TOTP block should spawn a new background poll
/// this frame. Pulled out into its own function, the same way
/// `totp_state_for_secret_presence`/`apply_totp_poll_result` are, so the two
/// conditions -- the interval having actually elapsed, and no poll already
/// outstanding -- are unit-testable together without an `eframe` context.
///
/// Takes no `has_totp_secret` parameter (review 12's Minor 4): its one call
/// site only ever reaches this function inside the `else` of `if
/// !has_totp_secret`, so the argument was always `true` there and checking
/// it again here was dead weight.
///
/// `poll_in_flight` is the one condition here that isn't just "is it time
/// yet" (see `totp_poll_in_flight`'s declaration in `run`): without it, a
/// `bw serve` that never answers would still only ever have one real HTTP
/// call blocking on it -- the call itself moved to a background thread --
/// but `run`'s loop would spawn a *new* such thread every
/// `TOTP_POLL_INTERVAL` for as long as it stayed hung, one more piling up on
/// top of the last with nothing to bound how many accumulate.
///
/// `state_wants_poll` (from `totp_state_wants_poll`) is review 13's other
/// half: a `NoCodeReported` state is a definitive answer already received
/// for this item, so continuing to poll it every second is pure load on
/// `bw serve` with no possible new information. Passed in as a plain `bool`
/// rather than taking the `TotpState` itself so this stays the "is it time
/// yet" decision and the "does this state still want an answer" decision
/// stays in its own testable function.
fn should_start_totp_poll(poll_due: bool, poll_in_flight: bool, state_wants_poll: bool) -> bool {
    poll_due && !poll_in_flight && state_wants_poll
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
///
/// The same result must also be dropped if it was fetched against a vault
/// state that has since been superseded (review 15's Minor 5). A poll
/// carries the `load_generation` current when it was spawned; `run`'s
/// `vault_rx` drain runs before its `totp_rx` drain, so a reload can land
/// and re-arm `totp_state` and then, in the same frame, a poll issued before
/// that reload can be applied on top -- landing back on `NoCodeReported`,
/// which stops polling, and silently undoing the re-arm the user asked for
/// by clicking Sync. This is the same generation-tag pattern `vault_rx`
/// results already use (`apply_vault_load_result`), chosen over reordering
/// the two drains: that ordering is load-bearing and enforced by nothing but
/// source order inside one long `eframe` closure, and adding a second such
/// dependency would trade one implicit constraint for two.
///
/// It is deliberately slightly eager: `load_generation` is incremented when
/// a reload is *spawned*, not when it lands, so a poll spawned before that
/// increment is dropped even if the reload has not landed yet. The cost is
/// one skipped poll -- the next one fires within `TOTP_POLL_INTERVAL` -- and
/// the alternative (tagging with "the generation last applied") would need a
/// second counter that means almost the same thing.
fn totp_poll_result_is_current(
    item_id: &str,
    selected_id: Option<&str>,
    generation: u64,
    latest_generation: u64,
) -> bool {
    selected_id == Some(item_id) && generation == latest_generation
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
    // ONE `VaultSnapshot`, destructured here rather than assembled by the
    // worker (review 29's Important 1) -- see `run`'s `vault_tx` declaration
    // for why the pair this used to be put the guarantee in the wrong place.
    load_result: Result<VaultSnapshot, VaultLoadFailure>,
    items: &mut Vec<VaultItem>,
    folders: &mut Vec<Folder>,
    vault_loading: &mut bool,
    // See `run`'s declaration: what the body and the pill say when a load
    // produced nothing to paint (review 29's Minor 3).
    vault_load_error: &mut Option<String>,
    selected_id: &mut Option<String>,
    sync_status: &mut Option<Result<(), String>>,
    totp_state: &mut TotpState,
) {
    if generation != latest_generation {
        log::debug!(
            "dropping a superseded vault load result (generation {generation}, latest {latest_generation})"
        );
        return;
    }
    match load_result {
        Ok(snapshot) => {
            *items = snapshot.items;
            *folders = snapshot.folders;
            *vault_loading = false;
            // A load that painted answers the question the notice existed to
            // answer, so the notice goes with it -- otherwise a stale reason
            // would sit under a pill describing a vault that did load.
            *vault_load_error = None;
            // Fresh item data deserves a fresh poll (review 14's Important,
            // part b). `totp_state` was otherwise reset *only* on a selection
            // change, and `NoCodeReported` deliberately stops polling -- so a
            // user who noticed the "no code available" row, fixed the item's
            // authenticator key in the web vault and clicked Sync got the
            // reload landing here, replacing `items` underneath a latched
            // `NoCodeReported`, and still no code, with nothing on screen
            // saying "click a different item and back". `NoSecret` is the
            // same neutral "haven't looked yet" value the selection-change
            // reset uses: the per-frame derivation promotes it to `Fetching`
            // in the same frame if the (possibly just-updated) item still
            // carries a seed, and leaves it alone if it doesn't.
            //
            // Deliberately keyed on "a reload landed", not on comparing the
            // seed's VALUE: holding a copy of the 2FA seed in this loop to
            // diff against is exactly what 8b1e441 stopped doing, and a
            // reload is a sufficient trigger on its own -- they are rare
            // (window open, sync, forced refresh), so re-polling on each
            // costs one request.
            //
            // `totp_state_after_reload`, not a bare assignment: the
            // unconditional version blanked a code that was on screen and
            // live (review 15's Minor 3). See that function for why a `Code`
            // is the one state a reload has nothing to fix.
            *totp_state = totp_state_after_reload(totp_state.clone());
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
                // leave `mode`/`reveal`/`totp_code` stuck as they
                // were; clearing it routes through that same reset block.
                Some(id) => {
                    if !items.iter().any(|i| &i.id == id) {
                        *selected_id = None;
                    }
                }
            }
        }
        Err(failure) => {
            // `spawn_vault_load` couldn't refresh the snapshot (`bw serve`
            // never came ready, or `populate()` itself failed) -- see that
            // function's doc for why this must not be silently swallowed.
            // Whatever was already in `items`/`folders` (the pre-refresh
            // snapshot) is left alone rather than cleared: this is the same
            // never-propagate-a-failed-populate behaviour the doc comment
            // already describes, just no longer silent.
            *vault_loading = false;
            let reason = failure.reason().to_string();
            log::warn!("vault refresh failed; showing the last known snapshot: {reason}");
            // "Keep whatever is on screen" is VACUOUS at the initial load,
            // where nothing is on screen yet (review 29's Minor 3): that used
            // to leave an empty window under a neutral "Sync" pill and no
            // statement anywhere that a load had failed at all. The reason is
            // recorded unconditionally, and `vault_body_state`/`sync_pill`
            // decide what the user actually sees from it.
            *vault_load_error = Some(reason.clone());
            match failure {
                // The vault session this load was spawned for is gone. Any
                // sync that preceded it SUCCEEDED -- `bw sync` really did
                // run, and the cache really did refill; it refilled for a
                // different session. Reporting that as "Sync failed" is the
                // mirror image of the bug the override below exists to fix:
                // a red pill over an operation that worked (review 29's Minor
                // 3). `sync_status` is left exactly as it was and the pill
                // says the true thing instead -- the vault was not
                // refreshed -- via `vault_load_error` above.
                VaultLoadFailure::Superseded(_) => {}
                // A refresh that genuinely did not happen. Override
                // `sync_status` only when this refresh was following up on a
                // sync that had itself just reported success (review 12's
                // Important 1): that is the case where the toolbar pill would
                // otherwise say "Synced just now" over data that was never
                // actually refreshed. An initial-load failure (before any
                // sync has run) has no such claim to correct -- `sync_status`
                // is still `None` and stays that way; the pill's "vault not
                // refreshed" state, not a fabricated sync failure, is what
                // reports it. The generation check above is what keeps this
                // from also firing on a *stale* failure after a newer,
                // already-applied load already reported success -- by the
                // time a stale result reaches here, it has already been
                // dropped.
                VaultLoadFailure::Refresh(_) => {
                    if matches!(sync_status, Some(Ok(()))) {
                        *sync_status = Some(Err(reason));
                    }
                }
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
    tx: mpsc::Sender<(u64, Result<VaultSnapshot, VaultLoadFailure>)>,
    request: VaultLoadRequest,
) {
    spawn_vault_load_with_schedule(cache, tx, request, readiness_schedule(READINESS_DEADLINE));
}

/// What one vault load is being asked for.
///
/// A STRUCT AND NOT A POSITIONAL ARGUMENT LIST, for one reason (review 29's
/// Minor 4): `force_refresh` and `skip_readiness_wait` are both `bool` and sat
/// two apart in a seven-argument call, so swapping them compiled silently and
/// turned a forced post-sync reload into an unforced one that also skips the
/// readiness wait -- i.e. Sync appearing to do nothing, which is the exact
/// failure `force_refresh` exists to prevent. Named fields make that a
/// compile error at every call site, and the fields cannot be mis-ordered
/// because field-init order does not matter.
///
/// `era` and `generation` were never the hazard here -- [`VaultEra`] is a
/// newtype, so swapping those two is already a type error -- but they are
/// carried together because they are the same kind of answer about the same
/// load, and their difference is the thing every reader gets wrong (see
/// `era`'s doc below).
struct VaultLoadRequest {
    /// `true` after a sync, which changes the vault underneath us: the
    /// snapshot is still marked populated but is now stale, so the
    /// short-circuit in the worker would serve pre-sync data and the sync
    /// would appear to do nothing. `false` on window open, where the snapshot
    /// from unlock is current and re-fetching would throw away the whole
    /// point of the cache.
    force_refresh: bool,
    /// Which vault SESSION this load is for, captured by the caller on the
    /// MAIN THREAD before the spawn (review 26's recorded producer).
    ///
    /// It is a parameter and not something the worker reads for itself, and
    /// that is the entire guard: an era captured inside the worker would be
    /// compared against itself, so a `clear` landing between the user's
    /// action and the worker's first instruction would be invisible. This is
    /// the same shape `picker_ui::pick_vault_item` already uses. `run`
    /// captures it ONCE per window session rather than once per spawn -- see
    /// `window_era`'s declaration for why per-spawn was a weaker question
    /// than the call site claimed (review 29's Important 2).
    ///
    /// Distinct from `generation`, which answers a different question:
    /// `generation` is WINDOW-REFRESH staleness (has this window spawned a
    /// newer load?), `era` is VAULT staleness (is the vault session this load
    /// was spawned for still the one the cache holds?). A lock or a re-auth
    /// advances the era and touches no generation at all.
    era: VaultEra,
    /// Tags the message sent over `tx` so `run`'s drain can tell a stale,
    /// superseded result apart from the one it's actually still waiting on --
    /// see `run`'s `load_generation` doc.
    generation: u64,
    /// Whether `bw serve` is already known to be up -- see `run`'s
    /// `backend_already_running` parameter doc. Skips the
    /// `wait_for_vault_ready` probe in the worker when true, the same
    /// exemption `spawn_sync` in `main.rs` already makes for the same reason.
    skip_readiness_wait: bool,
}

/// Why a vault load produced nothing to paint.
///
/// The two variants exist because they demand DIFFERENT things of the toolbar
/// pill, which is the only place either is spelled for the user (review 29's
/// Minor 3):
///
///  * `Superseded` -- the vault session this load was spawned for is gone. If
///    a sync preceded it, that sync SUCCEEDED: `bw sync` ran and the cache
///    refilled, for a different session. Painting "Sync failed" over that is
///    a red pill on an operation that worked.
///  * `Refresh` -- the refresh itself did not produce a readable vault (the
///    backend never came ready, `populate()` failed, or it reported success
///    and left nothing behind). A sync claim standing over that IS wrong and
///    is corrected.
///
/// Both carry the reason the loader logged, so nothing downstream re-words a
/// failure it did not observe.
#[derive(Debug)]
enum VaultLoadFailure {
    Superseded(&'static str),
    Refresh(String),
}

impl VaultLoadFailure {
    /// The loader's own words for this failure, for the log and for the
    /// on-screen notice.
    fn reason(&self) -> &str {
        match self {
            VaultLoadFailure::Superseded(reason) => reason,
            VaultLoadFailure::Refresh(reason) => reason,
        }
    }
}

/// What the load worker should do next, decided from ONE era-checked
/// observation of the cache.
///
/// Hoisted out of the detached `std::thread::spawn` below so each arm is
/// pinned by a test directly instead of being inferred from a thread's
/// observable behaviour -- `vault_load_step_tests`.
#[derive(Debug)]
enum VaultLoadStep {
    /// The cache already holds a vault for the caller's era: send it. The
    /// payload is a whole [`VaultSnapshot`], never a pair the worker
    /// assembled, so the items and folders it paints provably come from one
    /// lock acquisition in one era.
    Paint(VaultSnapshot),
    /// Go and fetch. Either nothing has ever been fetched into this era, or
    /// the caller forced a refresh because a `bw sync` just changed the vault
    /// underneath the snapshot.
    Populate,
    /// Neither fetch nor paint; report this failure as an `Err`.
    GiveUp(VaultLoadFailure),
}

/// A `clear` -- a lock, or a re-auth into a possibly different account --
/// began a new vault session before this load could read anything.
const VAULT_SUPERSEDED_BEFORE_LOAD: &str = "the vault was locked before this load could read it";
/// The same thing, but the `clear` landed after the fetch had started.
const VAULT_CLEARED_WHILE_REFRESHING: &str = "the vault was locked while refreshing";
/// A populate reported success and yet left nothing readable for this era.
const VAULT_EMPTY_AFTER_REFRESH: &str = "the vault refresh left nothing to show";
/// The populate itself failed: `bw serve` refused, errored, or could not be
/// reached. PROSE, like its three neighbours, and for a concrete reason
/// (review 31's Minor 2) -- every one of these strings is painted verbatim
/// under "Your vault could not be loaded", and this one used to be
/// `format!("{e:?}")`, i.e. a `VaultError` Debug rendering shown to a user as
/// if it were an explanation. The Debug detail is not lost: `spawn_vault_load_
/// with_schedule` logs it immediately before sending this.
const VAULT_REFRESH_FAILED: &str = "the vault could not be read from the local backend";

/// The load worker's first decision, from one call to
/// [`VaultCache::snapshot_unless_superseded`].
///
/// **Why the two [`VaultUnavailable`] variants are handled apart.** They are
/// opposite situations wearing one word ("no vault"), which is what this
/// crate's defect history is made of:
///
///  * `Unpopulated` -- same vault session, nothing fetched into it yet. A
///    populate is exactly the cure, and this is what every window open on a
///    fresh process sees (era 0 compares EQUAL to an era captured before the
///    first populate), so refusing here would leave the window permanently
///    empty.
///  * `Superseded` -- a different vault session. A populate CANNOT cure it:
///    `populate` takes its own, newer epoch and fills the cache for the
///    session that exists now, so the fetch would spend a full vault
///    round-trip and hand this window another account's data. The window's
///    result is meaningless, so it neither fetches nor paints, and reports an
///    `Err` -- which `apply_vault_load_result` turns into "keep whatever
///    snapshot is already on screen", the only honest thing to draw.
///
/// A FORCED refresh gives up on `Superseded` too. "Forced" means "a sync
/// changed the vault under me", not "fetch me some other account's vault";
/// the era it was spawned for is gone either way. It costs one snapshot clone
/// on the ordinary forced path (the `Ok` arm below discards it), which is the
/// price of being able to see a `clear` at all before spending a round-trip.
fn vault_load_step(force_refresh: bool, read: Result<VaultSnapshot, VaultUnavailable>) -> VaultLoadStep {
    match read {
        Err(VaultUnavailable::Superseded) => {
            VaultLoadStep::GiveUp(VaultLoadFailure::Superseded(VAULT_SUPERSEDED_BEFORE_LOAD))
        }
        Err(VaultUnavailable::Unpopulated) => VaultLoadStep::Populate,
        Ok(_) if force_refresh => VaultLoadStep::Populate,
        Ok(snapshot) => VaultLoadStep::Paint(snapshot),
    }
}

/// The load worker's second decision: the read that follows a successful
/// `populate()`, also era-checked and also one lock acquisition.
///
/// This is the window the old code could not see at all. `populate()` would
/// report `Populated`, a `clear` would land, and the separate `cache.items()`
/// / `cache.folders()` reads underneath it then handed back the empty vault
/// that clear left -- an empty list painted as data, under an `Ok`, with the
/// `DiscardedStale` arm above (which exists precisely to stop that) never
/// consulted because the populate had already succeeded.
///
/// `Unpopulated` is unreachable after a populate that returned `Populated` in
/// this era, and is still not folded into the arm above: it would mean the
/// snapshot is empty because nothing filled it, not because a different vault
/// session began. Different situations, different reasons in the log; neither
/// may be reported as `Ok`.
///
/// They are also classified differently for the toolbar (review 29's Minor
/// 3): a `clear` mid-refresh is `Superseded` -- whatever sync triggered this
/// still succeeded -- while a populate that reported success and left nothing
/// readable is a `Refresh` failure, and a standing "Synced just now" claim
/// over that one is wrong and gets corrected.
fn vault_read_after_populate(
    read: Result<VaultSnapshot, VaultUnavailable>,
) -> Result<VaultSnapshot, VaultLoadFailure> {
    match read {
        Ok(snapshot) => Ok(snapshot),
        Err(VaultUnavailable::Superseded) => Err(VaultLoadFailure::Superseded(VAULT_CLEARED_WHILE_REFRESHING)),
        Err(VaultUnavailable::Unpopulated) => Err(VaultLoadFailure::Refresh(VAULT_EMPTY_AFTER_REFRESH.to_string())),
    }
}

/// `spawn_vault_load`'s actual body, with the readiness schedule taken as a
/// parameter rather than hardcoded to `readiness_schedule(READINESS_DEADLINE)`
/// -- same split `wait_for_vault_ready`/`readiness_schedule` already use, and
/// for the same reason: it lets a test exhaust the schedule instantly (an
/// empty one) instead of actually waiting out the real 30s deadline.
fn spawn_vault_load_with_schedule(
    cache: std::sync::Arc<VaultCache>,
    tx: mpsc::Sender<(u64, Result<VaultSnapshot, VaultLoadFailure>)>,
    request: VaultLoadRequest,
    schedule: Vec<Duration>,
) {
    let VaultLoadRequest {
        force_refresh,
        era,
        generation,
        skip_readiness_wait,
    } = request;
    std::thread::spawn(move || {
        // ONE lock acquisition, and the era check is inside it. This used to
        // be `force_refresh || !cache.is_populated()` here and
        // `cache.items(), cache.folders()` at the bottom -- three separate
        // acquisitions, so items and folders could come from different
        // populates (account A's items filed under account B's folders) and
        // the common path skipped the populate entirely, which took the
        // `DiscardedStale` arm below out of the picture and let a `clear`
        // landing between the gate and the reads paint an EMPTY VAULT as
        // data. See `vault_load_step`.
        match vault_load_step(force_refresh, cache.snapshot_unless_superseded(era)) {
            // Sent AS THE SNAPSHOT, not as `(snapshot.items,
            // snapshot.folders)` -- see `run`'s `vault_tx` declaration
            // (review 29's Important 1). The pair spelling is what let
            // `Ok((snapshot.items, cache.folders()))` compile right here.
            VaultLoadStep::Paint(snapshot) => {
                let _ = tx.send((generation, Ok(snapshot)));
                return;
            }
            VaultLoadStep::GiveUp(failure) => {
                log::warn!("not loading the vault for era {era}: {}", failure.reason());
                let _ = tx.send((generation, Err(failure)));
                return;
            }
            VaultLoadStep::Populate => {}
        }
        // Same wait `spawn_sync` performs before its own `populate()` (see
        // this function's doc) -- cheap when `bw serve` is already answering
        // (the very first attempt succeeds), and the only thing standing
        // between "backend mid-cold-start" and a bogus connection-refused
        // failure otherwise. Skipped when the caller already knows the
        // backend was running before this window session started
        // (`skip_readiness_wait`, review Minor 3): `populate()` right below
        // still runs, and still fails loudly if the backend somehow isn't
        // answering after all, so skipping the probe costs nothing but a
        // redundant `list_items()` call in the case it's meant to skip --
        // not the safety net itself.
        if !skip_readiness_wait {
            if let Err(e) = wait_for_vault_ready(cache.bridge(), &schedule) {
                log::warn!("could not populate the vault cache: bw serve never became ready: {e}");
                let _ = tx.send((generation, Err(VaultLoadFailure::Refresh(e))));
                return;
            }
        }
        match cache.populate() {
            Ok(PopulateOutcome::Populated) => {}
            // The cache was cleared underneath this populate (a lock, or a
            // re-auth into a possibly different account), so it is
            // deliberately empty. Reporting `Ok((empty, empty))` here would
            // paint an empty vault list -- data, drawn from the absence of
            // data (review 14's Minor). The `Err` arm in
            // `apply_vault_load_result` instead keeps whatever snapshot was
            // already on screen and says so in the log.
            //
            // This arm is no longer the only thing standing between a
            // mid-flight `clear` and a painted empty vault, and it is no
            // longer bypassable: the gate above is an era-checked read rather
            // than an `is_populated()` short-circuit, and the read below is
            // era-checked too, so the same `clear` is refused at whichever of
            // the three points it lands. It is kept because it is the more
            // specific report -- the populate itself knows it was discarded,
            // where the read below can only observe that the era moved.
            //
            // The old note here said this was unreachable because every
            // `VaultCache::clear` runs on the main thread and
            // `vault_window::run` owns that thread. That is an argument about
            // thread affinity, not about this code, and it is the argument
            // this project has had to un-write repeatedly; it is deliberately
            // not what keeps any of these three refusals correct.
            Ok(PopulateOutcome::DiscardedStale) => {
                log::warn!("the vault was cleared while this refresh was in flight");
                let _ = tx.send((
                    generation,
                    Err(VaultLoadFailure::Superseded(VAULT_CLEARED_WHILE_REFRESHING)),
                ));
                return;
            }
            Err(e) => {
                // The Debug rendering goes to the LOG, where a developer reads
                // it; the channel carries prose, because whatever it carries is
                // painted verbatim to the user (review 31's Minor 2). See
                // `VAULT_REFRESH_FAILED`.
                log::warn!("could not populate the vault cache: {e:?}");
                let _ = tx.send((
                    generation,
                    Err(VaultLoadFailure::Refresh(VAULT_REFRESH_FAILED.to_string())),
                ));
                return;
            }
        }
        // The second era-checked read, and the second thing the old code got
        // wrong: `cache.items(), cache.folders()` here was two more lock
        // acquisitions with no era check at all, so a `clear` landing after
        // the populate wrote back was invisible and its empty snapshot went
        // out under an `Ok`. See `vault_read_after_populate`.
        match vault_read_after_populate(cache.snapshot_unless_superseded(era)) {
            Ok(snapshot) => {
                let _ = tx.send((generation, Ok(snapshot)));
            }
            Err(failure) => {
                log::warn!(
                    "the refreshed vault is not readable for era {era}: {}",
                    failure.reason()
                );
                let _ = tx.send((generation, Err(failure)));
            }
        }
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

/// What the toolbar pill says when the most recent load produced nothing to
/// paint but the sync that triggered it worked. Deliberately NOT "Sync
/// failed": the sync is not what failed.
const VAULT_NOT_REFRESHED_PILL: &str = "Vault not refreshed";

/// The toolbar sync pill's dot colour and label -- the ONLY place any of
/// these states is spelled for the user, which is why it is a function with
/// tests rather than an `if` chain inside the closure (review 29's Minor 3).
///
/// Precedence, and why each step outranks the one below it:
///
///  1. `sync_in_progress` -- something is happening right now; anything else
///     would be describing a finished operation while a newer one runs.
///  2. A failed sync -- the operation the user started did not work, which is
///     the strongest claim available and the only one that should be able to
///     say "Sync failed".
///  3. A load that produced nothing (`vault_load_error`) -- this OUTRANKS a
///     successful sync, and that is the point: a `Superseded` give-up after a
///     sync used to leave "Synced just now" standing over a window that was
///     never refreshed, and the old code's alternative was to forge a
///     `sync_status` failure for a sync that succeeded. Both are untrue; this
///     says the true thing about the vault instead of a false thing about the
///     sync.
///  4. A successful sync, then nothing at all.
///
/// **What step 3 deliberately does NOT distinguish** (review 31's Minor 4).
/// Both `VaultLoadFailure` variants land on the same
/// `(ERROR, VAULT_NOT_REFRESHED_PILL)`: at pill size there is no useful copy
/// that separates "a different vault session began" from "the refresh
/// failed", and the two are told apart on screen only by the reason string the
/// `Unavailable` body paints beneath. That body is gated on `items.is_empty()`
/// -- so if that condition is ever relaxed, the pill becomes the only thing
/// the user sees and the distinction collapses SILENTLY. Anyone loosening that
/// gate owes this function a second label.
fn sync_pill(
    sync_in_progress: bool,
    sync_status: Option<&Result<(), String>>,
    vault_load_error: Option<&str>,
    since_last_sync: Duration,
) -> (egui::Color32, String) {
    if sync_in_progress {
        return (theme::TEXT_GHOST, "Syncing…".to_string());
    }
    // Blue for success (there's no dedicated "success" green in this app's
    // palette -- see `theme.rs`'s module doc on "one blue hue... red reserved
    // for actual errors"), the design's error red for failure, and a neutral
    // ghost dot before the first sync has reported anything.
    match (sync_status, vault_load_error) {
        (Some(Err(_)), _) => (theme::ERROR, "Sync failed".to_string()),
        (_, Some(_)) => (theme::ERROR, VAULT_NOT_REFRESHED_PILL.to_string()),
        (Some(Ok(())), None) => (theme::BLUE, format!("Synced {}", synced_ago_text(since_last_sync))),
        (None, None) => (theme::TEXT_GHOST, "Sync".to_string()),
    }
}

/// What the window body shows this frame.
#[derive(Debug, PartialEq, Eq)]
enum VaultBodyState<'a> {
    /// A load is in flight and nothing has landed yet: one centred spinner.
    Loading,
    /// A load finished, produced nothing, and there was nothing already on
    /// screen for the `Err` arm's "keep what you have" to keep -- so say what
    /// happened instead of drawing an empty vault (review 29's Minor 3). The
    /// payload is the loader's own reason string.
    Unavailable(&'a str),
    /// Draw the vault: sidebar, list, detail pane.
    Vault,
}

/// Decides between the three (see [`VaultBodyState`]).
///
/// The two conditions on `Unavailable` are both load-bearing:
///
///  * `items.is_empty()` -- a failed refresh over a vault that IS on screen
///    must keep showing it. That is the whole "keep the last known snapshot"
///    behaviour, and replacing a populated window with an error page because
///    a background refresh failed would be a much worse regression than the
///    blank window this fixes. The pill still reports it.
///  * `vault_load_error.is_some()` -- an empty vault with no failure is a
///    genuinely empty vault, and it gets the normal chrome (sidebar counts at
///    zero, the list's own empty state), not an error page.
fn vault_body_state(vault_loading: bool, items_empty: bool, vault_load_error: Option<&str>) -> VaultBodyState<'_> {
    if vault_loading {
        return VaultBodyState::Loading;
    }
    match vault_load_error {
        Some(reason) if items_empty => VaultBodyState::Unavailable(reason),
        Some(_) | None => VaultBodyState::Vault,
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
    fn a_genuine_no_code_response_becomes_no_code_reported_with_no_error() {
        // Deliberately NOT `NoSecret`, even though the two render the same:
        // `NoSecret` is the value the per-frame presence derivation promotes
        // back to `Fetching`, which is what made this arm invisible in the
        // live composition (review 13's Important). See
        // `a_backend_reported_absence_is_never_promoted_back_to_fetching`.
        let mut totp_state = TotpState::Code { code: "111111".to_string(), seconds_left: 20 };

        let error = apply_totp_poll_result(Ok(None), 15, &mut totp_state);

        assert_eq!(totp_state, TotpState::NoCodeReported);
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

    /// **The composed assertion** (review 14's Important). Each pure function
    /// below is correct on its own; what a user actually sees is the result
    /// of running them in sequence at the live call site and then rendering
    /// the outcome, and *that* is where an item with a seed used to end up
    /// drawing nothing at all.
    ///
    /// The live call site (`run`'s per-frame TOTP block) reaches `get_totp`
    /// only through `has_totp_secret == true`, so this walks every poll
    /// result that can occur there, applies it, runs the same per-frame
    /// presence derivation the next frame runs, and asserts the render layer
    /// still draws a One-time code row. No poll result may make an item that
    /// carries a seed look like an item that never had 2FA.
    #[test]
    fn an_item_with_a_seed_never_renders_as_a_no_totp_item_whatever_the_poll_returned() {
        use crate::vault_window::detail::totp_row_for;
        use crate::vault_window::{totp_state_for_secret_presence, totp_state_wants_poll};

        let poll_results: Vec<Result<Option<String>, VaultError>> = vec![
            Ok(Some("123456".to_string())),
            Ok(None),
            Err(VaultError::Http("connection reset".to_string())),
            Err(VaultError::Unauthorized),
        ];

        for poll_result in poll_results {
            let label = format!("{poll_result:?}");
            // Frame 0: the item was just selected, so `run`'s reset block set
            // `NoSecret` and the derivation promoted it to `Fetching`.
            let mut totp_state = totp_state_for_secret_presence(true, TotpState::NoSecret);
            assert_eq!(totp_state, TotpState::Fetching, "{label}");
            assert!(totp_row_for(&totp_state).is_some(), "{label}: no row while fetching");

            // The poll lands.
            let _ = apply_totp_poll_result(poll_result, 15, &mut totp_state);
            assert!(
                totp_row_for(&totp_state).is_some(),
                "{label}: the One-time code row vanished for an item whose own login data \
                 carries a seed -- pixel-identical to an item that never had 2FA"
            );

            // Every subsequent frame re-runs the derivation, with the seed
            // still present. The row must stay put for as long as the item
            // stays selected, whether or not the state still wants polling.
            for _ in 0..3 {
                totp_state = totp_state_for_secret_presence(true, totp_state.clone());
                assert!(
                    totp_row_for(&totp_state).is_some(),
                    "{label}: the row vanished on a later frame's presence derivation"
                );
            }
            let _ = totp_state_wants_poll(&totp_state);
        }
    }

    /// The other half of the composition: once the item's *own* login data no
    /// longer carries a seed, the row does go away -- that is review 9's
    /// property and it must survive `NoCodeReported` gaining a row.
    #[test]
    fn an_item_whose_seed_is_gone_does_render_as_a_no_totp_item() {
        use crate::vault_window::detail::totp_row_for;
        use crate::vault_window::totp_state_for_secret_presence;

        for previous in [
            TotpState::Fetching,
            TotpState::Code { code: "123456".to_string(), seconds_left: 9 },
            TotpState::Unavailable,
            TotpState::NoCodeReported,
        ] {
            let state = totp_state_for_secret_presence(false, previous);
            assert_eq!(state, TotpState::NoSecret);
            assert_eq!(totp_row_for(&state), None);
        }
    }
}

#[cfg(test)]
mod should_start_totp_poll_tests {
    // The TOTP poll moved off the UI thread onto a one-shot background
    // thread (see `totp_poll_in_flight`'s declaration in `run`) because
    // `ureq`'s read timeout bounds one read syscall, not `get_totp`'s
    // response as a whole -- a trickling or stalled `bw serve` could freeze
    // this window for well past that timeout, once per `TOTP_POLL_INTERVAL`.
    // These pin the two-way gate that replaced the old unconditional call:
    // a poll only starts when the interval has actually elapsed, and --
    // the new condition -- no poll is already outstanding, so a hung backend
    // accumulates at most one background thread instead of one more every
    // second for as long as it stays hung. Whether there's a secret to poll
    // for at all is checked by this function's one call site, not here
    // (review 12's Minor 4) -- see `should_start_totp_poll`'s own doc.
    use super::should_start_totp_poll;

    #[test]
    fn starts_when_due_with_nothing_in_flight() {
        assert!(should_start_totp_poll(true, false, true));
    }

    #[test]
    fn does_not_start_before_the_interval_elapses() {
        assert!(!should_start_totp_poll(false, false, true));
    }

    #[test]
    fn does_not_start_when_the_state_already_has_a_definitive_answer() {
        // Review 13's Important, second half: a `NoCodeReported` state means
        // the backend has already answered "no current code for this item".
        // Without this the pane re-asked once a second, forever, for as long
        // as the item stayed selected.
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
mod totp_state_wants_poll_tests {
    // Review 13's Important, second half. `NoCodeReported` is the only state
    // where the backend has already given a definitive, *successful* answer
    // for this item, so it is the only one that must stop the per-second
    // poll. Everything else -- including `Unavailable`, which is transient
    // by definition and must be able to recover -- keeps asking.
    use super::totp_state_wants_poll;
    use crate::vault_window::detail::TotpState;

    #[test]
    fn a_backend_reported_absence_stops_polling() {
        assert!(!totp_state_wants_poll(&TotpState::NoCodeReported));
    }

    #[test]
    fn a_pending_fetch_keeps_polling() {
        assert!(totp_state_wants_poll(&TotpState::Fetching));
    }

    #[test]
    fn a_live_code_keeps_polling_so_it_can_be_refreshed_before_its_window_closes() {
        assert!(totp_state_wants_poll(&TotpState::Code {
            code: "111111".to_string(),
            seconds_left: 4
        }));
    }

    #[test]
    fn an_unavailable_state_keeps_polling_so_a_transient_outage_can_recover() {
        // The distinction that makes `NoCodeReported` worth its own variant:
        // "the backend could not be reached" must keep retrying, "the
        // backend answered, there is no code" must not.
        assert!(totp_state_wants_poll(&TotpState::Unavailable));
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
        assert!(totp_poll_result_is_current("item-1", Some("item-1"), 3, 3));
    }

    #[test]
    fn a_result_for_a_no_longer_selected_item_is_stale() {
        // The user switched from item A to item B before A's poll returned.
        assert!(!totp_poll_result_is_current("item-a", Some("item-b"), 3, 3));
    }

    #[test]
    fn a_result_landing_after_the_selection_was_cleared_is_stale() {
        assert!(!totp_poll_result_is_current("item-1", None, 3, 3));
    }

    #[test]
    fn a_result_from_before_a_newer_load_was_spawned_is_stale() {
        // Review 15's Minor 5. The `vault_rx` drain runs BEFORE the
        // `totp_rx` drain in `run`'s closure, so within a single frame a
        // reload can land (re-arming `totp_state`) and then a poll issued
        // against the PRE-sync backend state can be applied on top of it --
        // re-latching `NoCodeReported` and stopping polling again, defeating
        // the re-arm the user just triggered by clicking Sync. Fixed by
        // tagging, the way vault loads already are, rather than by swapping
        // the two drains: the ordering in that closure is load-bearing and
        // enforced by nothing but source order, and a second such dependency
        // is not an improvement on one.
        assert!(!totp_poll_result_is_current("item-1", Some("item-1"), 1, 2));
    }
}

#[cfg(test)]
mod totp_state_for_secret_presence_tests {
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
    fn a_backend_reported_absence_also_clears_when_the_secret_is_gone() {
        // Review 9's property must survive review 13's redesign: the
        // derivation runs unconditionally, every frame, *before* the poll
        // gate, so an item whose seed was removed on another device lands on
        // `NoSecret` in the same frame the reload carrying that removal
        // lands -- from any previous state, `NoCodeReported` included. The
        // new variant is exempt from the *promotion*, not from this.
        let next = totp_state_for_secret_presence(false, TotpState::NoCodeReported);

        assert_eq!(next, TotpState::NoSecret);
    }

    #[test]
    fn a_backend_reported_absence_survives_while_the_secret_is_still_present() {
        // The other side of the same coin: while the item still carries a
        // seed, the polled answer stands. Left as `NoSecret` it would be
        // promoted to `Fetching` on this very frame and re-polled on the
        // next.
        let next = totp_state_for_secret_presence(true, TotpState::NoCodeReported);

        assert_eq!(next, TotpState::NoCodeReported);
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

    #[test]
    fn an_unavailable_state_is_left_untouched_while_the_secret_is_still_present() {
        // Same reasoning as the live-code case: a failed poll's honest
        // "unavailable" state must stand until a fresh poll replaces it, not
        // be silently reset back to `Fetching` (which would misleadingly
        // suggest a poll is about to arrive when none has been started).
        let next = totp_state_for_secret_presence(true, TotpState::Unavailable);

        assert_eq!(next, TotpState::Unavailable);
    }

    #[test]
    fn a_backend_reported_absence_is_never_promoted_back_to_fetching() {
        // The composed assertion review 13 found missing: both halves were
        // individually correct, and together they looped forever.
        use super::apply_totp_poll_result;
        let mut state = TotpState::Fetching;
        apply_totp_poll_result(Ok(None), 15, &mut state);

        let next = totp_state_for_secret_presence(true, state);

        assert_ne!(
            next,
            TotpState::Fetching,
            "a poll that authoritatively answered \"no code for this item\" must not be \
             promoted straight back into Fetching, or the pane says \"Fetching...\" forever \
             and polls bw serve once a second for as long as the item stays selected"
        );
    }

    #[test]
    fn no_secret_is_promoted_to_fetching_once_a_secret_is_present() {
        // Regression test for review 12's Important 3: the TOTP poll now
        // runs on a background thread and reports back later over
        // `totp_rx`, so a freshly selected item (or one whose secret just
        // reappeared) must not render as `NoSecret` -- which draws no row at
        // all (`totp_row_for` maps it, and only it, to `None`) and reads as
        // "this item has no TOTP" -- for however long the fetch takes.
        // `NoSecret` is the value DERIVED FROM THE ITEM: it is only correct
        // while the item we hold carries no seed. It is deliberately not
        // what a poll concludes -- `apply_totp_poll_result`'s `Ok(None)` arm
        // has written `NoCodeReported` since 48cff27, precisely so this
        // promotion cannot undo it -- and it is also the neutral "haven't
        // looked yet" value that a selection change and a landed reload
        // (`totp_state_after_reload`) reset to. In all of those cases, with
        // a seed present, it must read as "fetching", not "not set up here".
        let next = totp_state_for_secret_presence(true, TotpState::NoSecret);

        assert_eq!(next, TotpState::Fetching);
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

/// Where `run`'s [`detail::RevealState`] is *declared*.
///
/// **A source-position assertion, deliberately, and the only kind that can
/// reach this.** `detail.rs`'s
/// `a_reveal_click_in_one_frame_is_still_revealed_in_the_next` proves the
/// whole pane path -- the Reveal button writes through to the caller's struct
/// and the next frame honours it -- but it supplies its own long-lived
/// `RevealState`, exactly as `run` does. Moving `run`'s own `let mut reveal`
/// from its per-selection state block *into* the `run_ui_native` frame
/// closure re-creates it every frame, makes Reveal a visible no-op in the
/// shipped app, and is invisible to every behavioural test in this crate:
/// `run` opens a native window and cannot be called from a test at all, and
/// no function below it can observe where its caller's binding lives.
///
/// That is the bug `detail_edit.rs` shipped once already, and it is the only
/// route back to it left open. So this asserts the two observable facts that
/// distinguish the placements: the declaration exists exactly once and comes
/// *before* the frame closure, and the reset assignment still sits inside the
/// selection-change block. If any needle below stops matching, the test fails
/// loudly rather than silently passing -- read the message, then update the
/// needle.
///
/// **What these do NOT guarantee, stated so the doc does not imply coverage it
/// does not have.** They pin source *positions* and the *spelling* of one
/// condition. They cannot see behaviour: nothing here would notice the reset
/// being moved out of the selection-change block into some other block whose
/// condition happens to be spelled the same way, nor a `RevealState` whose
/// `Default` stopped meaning "masked". Both of those are visible in a diff that
/// touches these lines; what the guards exist for is the edit that touches
/// neither and still re-creates the state per frame.
#[cfg(test)]
mod reveal_state_placement_tests {
    // EVERY NEEDLE IS SPLIT ACROSS TWO LITERALS, AND THAT IS LOAD-BEARING.
    // `include_str!("mod.rs")` pulls in this test module too, so a needle
    // written as one literal is always present in the source -- inside the very
    // const that defines it. That made both `unwrap_or_else` panics below
    // unreachable dead code, and it let the whole test pass with the regression
    // live: rename the frame closure's second parameter and a long
    // `FRAME_CLOSURE` matches nothing but its own definition down here at the
    // bottom of the file, after which *any* declaration position at all
    // compares "before" it. `concat!` joins the halves at compile time, so each
    // needle exists in the binary but appears nowhere in this file's source
    // except where the real code is. Do not re-join these into one literal --
    // and note that the occurrence-count assertions below are what ENFORCE
    // that, not this comment: re-joining any needle makes it appear one extra
    // time, in its own const, and fails that needle's count.
    const DECLARATION: &str = concat!("let mut reveal = detail::", "RevealState::default();");
    // Deliberately stops before the closure's parameter list: a rename of
    // `_frame` (that parameter will be used eventually) is an unrelated
    // refactor and must not be able to invalidate this needle.
    const FRAME_CLOSURE: &str = concat!("run_ui_native(", "WINDOW_TITLE, options,");
    const RESET_GUARD: &str = concat!("if selected_id != ", "last_selected_id {");
    // The bare assignment, which is also the tail of `DECLARATION` -- hence the
    // "exactly two occurrences" shape in the reset test below.
    const RESET: &str = concat!("reveal = detail::", "RevealState::default();");

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    #[test]
    fn the_reveal_state_is_declared_outside_the_per_frame_closure() {
        let source = source();
        let declaration = source.find(DECLARATION).unwrap_or_else(|| {
            panic!("no {DECLARATION:?} in this file -- if `run`'s RevealState was renamed, \
                    update this needle; if it was DELETED, the reveal toggle no longer has \
                    caller-owned state and that is the regression this guards")
        });
        assert_eq!(
            source.matches(DECLARATION).count(),
            1,
            "{DECLARATION:?} appears more than once in this file. A second declaration \
             shadows the first, and the position check below would be satisfied by \
             whichever came first while the pane read the other -- so this is a failure \
             even though both may be outside the closure."
        );
        let closure = source.find(FRAME_CLOSURE).unwrap_or_else(|| {
            panic!("no {FRAME_CLOSURE:?} in this file -- `run`'s call to the frame runner was \
                    renamed or its first two arguments changed; update this needle")
        });
        assert_eq!(
            source.matches(FRAME_CLOSURE).count(),
            1,
            "{FRAME_CLOSURE:?} appears more than once in this file, so 'before the closure' \
             no longer names one position; update this needle to something unique."
        );
        assert!(
            declaration < closure,
            "`run`'s RevealState is declared INSIDE the per-frame closure, so it is \
             re-created every frame and Reveal is a no-op in the shipped app -- see this \
             module's doc. Declare it in `run`'s per-selection state block instead."
        );
    }

    /// The declaration being in the right place is only half of it: the reset
    /// is what keeps a revealed value from following the user onto the next
    /// item, and *widening or deleting its condition* re-creates the state on
    /// every frame just as effectively as moving the declaration would --
    /// identical user-visible regression, and the position test above stays
    /// green through it.
    #[test]
    fn the_reveal_state_is_reset_only_when_the_selection_changes() {
        let source = source();
        let guard = source.find(RESET_GUARD).unwrap_or_else(|| {
            panic!("no {RESET_GUARD:?} in this file -- the selection-change condition was \
                    deleted or WIDENED. If it now holds on more than a selection change, the \
                    reset below it runs on frames it should not: `reveal` is cleared while \
                    the user is looking at it and Reveal becomes a no-op in the shipped app. \
                    If the condition was merely reworded, update this needle.")
        });
        assert_eq!(
            source.matches(RESET_GUARD).count(),
            1,
            "{RESET_GUARD:?} appears more than once in this file, so it no longer names one \
             block; update this needle to something unique."
        );
        let occurrences: Vec<usize> = source.match_indices(RESET).map(|(i, _)| i).collect();
        assert_eq!(
            occurrences.len(),
            2,
            "expected exactly two {RESET:?} in this file -- `run`'s declaration and the \
             single reset in the selection-change block -- found {}. FEWER means one of \
             them was renamed or DELETED (a deleted reset lets a revealed card number \
             follow the user onto the next item); MORE means something other than the \
             selection-change block also clears `reveal`.",
            occurrences.len()
        );
        assert_eq!(
            occurrences[0],
            source.find(DECLARATION).expect("checked by the placement test above") + "let mut ".len(),
            "the first {RESET:?} is not the tail of `run`'s declaration, so these two \
             occurrences are not the pair this test believes it is looking at"
        );
        assert!(
            guard < occurrences[1],
            "the reset assignment comes BEFORE the selection-change condition, so it is not \
             inside that block -- it runs unconditionally, once per frame, and Reveal is a \
             no-op in the shipped app."
        );
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
    use super::{spawn_vault_load_with_schedule, VaultLoadRequest};
    use crate::vault_bridge::VaultBridge;
    use crate::vault_cache::{PopulateOutcome, VaultCache};
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
        // The era is captured before the spawn, exactly as `run` does it.
        let era = cache.epoch().era();
        // Empty schedule: the mock answers on the very first attempt, so
        // there is nothing to retry regardless -- this only proves an empty
        // schedule doesn't itself block a successful readiness check.
        spawn_vault_load_with_schedule(
            cache,
            tx,
            VaultLoadRequest {
                force_refresh: true,
                era,
                generation: 1,
                skip_readiness_wait: false,
            },
            vec![],
        );

        let (generation, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        assert_eq!(generation, 1, "the result must be tagged with the generation it was spawned with");
        let snapshot = result.expect("bw serve was ready; load must succeed");
        assert_eq!(snapshot.items.len(), 1);
        assert!(snapshot.folders.is_empty());
    }

    /// Review 31's Minor 2. The reason a `VaultLoadFailure` carries is PAINTED,
    /// verbatim, under "Your vault could not be loaded" by the `Unavailable`
    /// body. This arm used to send `format!("{e:?}")`, so a user whose backend
    /// answered 500 read a Rust `VaultError` Debug rendering -- `Http("...")`
    /// -- where the other three reasons are hand-written prose. The detail is
    /// not lost: it is logged one line above the send.
    #[test]
    fn a_failed_populate_reports_prose_rather_than_a_debug_rendering() {
        let mut server = mockito::Server::new();
        // Answers, so the readiness probe passes, but fails the actual list --
        // which is what reaches `cache.populate()`'s `Err` arm rather than any
        // of the era-checked refusals around it.
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(500)
            .with_body("boom")
            .create();

        let cache = Arc::new(VaultCache::new(VaultBridge::new(server.url())));
        let (tx, rx) = mpsc::channel();
        let era = cache.epoch().era();
        spawn_vault_load_with_schedule(
            cache,
            tx,
            VaultLoadRequest {
                force_refresh: true,
                era,
                generation: 3,
                // The readiness probe would also fail against a 500, and its
                // own message is already prose; skipping it is what puts this
                // test on the populate arm specifically.
                skip_readiness_wait: true,
            },
            vec![],
        );

        let (_, result) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("load thread must report back");
        let reason = result.expect_err("a 500 from bw serve is not a vault").reason().to_string();
        assert!(
            !reason.contains('"') && !reason.contains('('),
            "this string is painted to the user under \"Your vault could not be loaded\" -- a \
             Rust Debug rendering is not an explanation: {reason:?}"
        );
        assert_eq!(reason, super::VAULT_REFRESH_FAILED);
    }

    #[test]
    fn a_forced_refresh_reports_err_instead_of_stale_data_when_bw_serve_never_answers() {
        // Nothing is listening at this URL at all, so every readiness attempt
        // fails immediately (connection refused) -- an empty schedule means
        // that single failure is also the last one, so this resolves fast
        // rather than waiting out the real READINESS_DEADLINE.
        let cache = Arc::new(VaultCache::new(VaultBridge::new("http://127.0.0.1:1")));
        let (tx, rx) = mpsc::channel();
        let era = cache.epoch().era();
        spawn_vault_load_with_schedule(
            cache,
            tx,
            VaultLoadRequest {
                force_refresh: true,
                era,
                generation: 7,
                skip_readiness_wait: false,
            },
            vec![],
        );

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
        let era = cache.epoch().era();
        spawn_vault_load_with_schedule(
            cache,
            tx,
            VaultLoadRequest {
                force_refresh: true,
                era,
                generation: 1,
                skip_readiness_wait: true,
            },
            vec![],
        );

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
        let era = cache.epoch().era();
        spawn_vault_load_with_schedule(
            cache,
            tx,
            VaultLoadRequest {
                force_refresh: true,
                era,
                generation: 1,
                skip_readiness_wait: false,
            },
            vec![],
        );

        let (_, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        assert!(result.is_ok());
        items.assert();
    }

    #[test]
    fn a_superseded_era_neither_fetches_nor_paints() {
        // REVIEW 26'S RECORDED PRODUCER, the whole point of the `era`
        // parameter. The era is captured on the main thread; a `clear` lands
        // before the worker gets to read. The window's result is meaningless
        // for the session that asked, so nothing is fetched (the mocks are
        // `.expect(1)`, satisfied by the manual populate below and nothing
        // else) and nothing is painted -- `Err`, so
        // `apply_vault_load_result` keeps whatever is already on screen.
        let mut server = mockito::Server::new();
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .expect(1)
            .create();
        let folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .expect(1)
            .create();

        let cache = Arc::new(VaultCache::new(VaultBridge::new(server.url())));
        assert_eq!(
            cache.populate().expect("the mocks answer"),
            PopulateOutcome::Populated
        );
        let era = cache.epoch().era();
        cache.clear();

        // `true` -- a FORCED refresh must give up too. Forcing says "the
        // vault changed under me", not "fetch me some other account's
        // vault"; the era it was spawned for is gone either way.
        let (tx, rx) = mpsc::channel();
        spawn_vault_load_with_schedule(
            cache.clone(),
            tx,
            VaultLoadRequest {
                force_refresh: true,
                era,
                generation: 4,
                skip_readiness_wait: true,
            },
            vec![],
        );
        let (generation, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        assert_eq!(generation, 4);
        assert!(
            result.is_err(),
            "a load whose era was superseded must not paint -- an Ok here is another account's \
             vault, or an empty one presented as data"
        );

        let (tx, rx) = mpsc::channel();
        spawn_vault_load_with_schedule(
            cache,
            tx,
            VaultLoadRequest {
                force_refresh: false,
                era,
                generation: 5,
                skip_readiness_wait: true,
            },
            vec![],
        );
        let (_, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        assert!(result.is_err(), "and neither may the unforced path");

        items.assert();
        folders.assert();
    }

    #[test]
    fn the_common_path_paints_both_halves_from_one_checked_read_without_fetching() {
        // The `!force_refresh && already populated` path: no HTTP at all
        // (both mocks are `.expect(1)`, spent by the manual populate), and
        // the items and folders it sends come from ONE
        // `snapshot_unless_superseded` call, so they cannot be from two eras.
        let mut server = mockito::Server::new();
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .expect(1)
            .create();
        let folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[{"id":"f1","name":"F"}]}}"#)
            .expect(1)
            .create();

        let cache = Arc::new(VaultCache::new(VaultBridge::new(server.url())));
        assert_eq!(
            cache.populate().expect("the mocks answer"),
            PopulateOutcome::Populated
        );
        let era = cache.epoch().era();

        let (tx, rx) = mpsc::channel();
        spawn_vault_load_with_schedule(
            cache,
            tx,
            VaultLoadRequest {
                force_refresh: false,
                era,
                generation: 1,
                skip_readiness_wait: true,
            },
            vec![],
        );
        let (_, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        // ONE value off the channel, destructured HERE and nowhere in the
        // worker (review 29's Important 1).
        let snapshot = result.expect("the snapshot is readable for this era");
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(
            snapshot.folders.len(),
            1,
            "the folders half must arrive with the items half"
        );

        items.assert();
        folders.assert();
    }

    #[test]
    fn a_clear_from_inside_a_response_handler_paints_neither_half() {
        // `vault_cache.rs`'s deterministic-interleaving technique rather than
        // a sleep: the `clear()` fires from inside the mocked folders
        // response handler, so it lands strictly after the populate began
        // fetching and strictly before it writes back. The forced refresh
        // must report a failure, not the pre-clear items, and not the empty
        // vault the clear left behind.
        let mut server = mockito::Server::new();
        let cache = Arc::new(VaultCache::new(VaultBridge::new(server.url())));
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let cache_for_handler = cache.clone();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |_| {
                cache_for_handler.clear();
                folders_body().as_bytes().to_vec()
            })
            .create();

        let era = cache.epoch().era();
        let (tx, rx) = mpsc::channel();
        spawn_vault_load_with_schedule(
            cache,
            tx,
            VaultLoadRequest {
                force_refresh: true,
                era,
                generation: 1,
                skip_readiness_wait: true,
            },
            vec![],
        );
        let (_, result) = rx.recv_timeout(Duration::from_secs(5)).expect("load thread must report back");
        assert!(
            result.is_err(),
            "a refresh cleared mid-flight must report a failure -- painting its Ok would draw \
             either a stale account's items or an empty vault as data"
        );
    }
}

#[cfg(test)]
mod vault_load_step_tests {
    // The decision `spawn_vault_load_with_schedule`'s detached worker used to
    // make inline as `force_refresh || !cache.is_populated()` across three
    // separate lock acquisitions (review 26's recorded producer). Hoisted out
    // of the closure so each arm is pinned directly instead of being inferred
    // from a thread's observable behaviour.
    use super::{vault_load_step, vault_read_after_populate, VaultLoadFailure, VaultLoadStep};
    use crate::vault_cache::{VaultSnapshot, VaultUnavailable};

    fn a_snapshot() -> VaultSnapshot {
        VaultSnapshot {
            items: vec![serde_json::from_str(r#"{"id":"1","name":"A","fields":[]}"#).expect("valid item")],
            folders: vec![serde_json::from_str(r#"{"id":"f1","name":"F"}"#).expect("valid folder")],
        }
    }

    #[test]
    fn a_superseded_era_gives_up_rather_than_fetching_a_vault_it_cannot_use() {
        // `Superseded` and `Unpopulated` are NOT the same situation, which is
        // this codebase's whole defect history in one sentence. A populate
        // cannot cure `Superseded`: it takes its own, newer epoch and fills
        // the cache for the session that exists NOW, which is not the one
        // this window session captured its era in. Fetching would spend a
        // full vault round-trip to produce another account's data.
        for force_refresh in [false, true] {
            assert!(
                matches!(
                    vault_load_step(force_refresh, Err(VaultUnavailable::Superseded)),
                    VaultLoadStep::GiveUp(_)
                ),
                "force_refresh = {force_refresh}"
            );
        }
    }

    #[test]
    fn an_unpopulated_snapshot_in_the_same_era_is_exactly_what_a_populate_cures() {
        // The everyday first-open path: a fresh process is era 0 and
        // unpopulated, so an era captured before the first populate compares
        // EQUAL. Refusing here would leave the window permanently empty.
        for force_refresh in [false, true] {
            assert!(
                matches!(
                    vault_load_step(force_refresh, Err(VaultUnavailable::Unpopulated)),
                    VaultLoadStep::Populate
                ),
                "force_refresh = {force_refresh}"
            );
        }
    }

    #[test]
    fn a_readable_snapshot_is_painted_unforced_and_refetched_when_forced() {
        let VaultLoadStep::Paint(snapshot) = vault_load_step(false, Ok(a_snapshot())) else {
            panic!("an unforced load over a readable snapshot must paint it, not re-fetch");
        };
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.folders.len(), 1);

        assert!(
            matches!(vault_load_step(true, Ok(a_snapshot())), VaultLoadStep::Populate),
            "a forced refresh follows a sync that changed the vault underneath the snapshot"
        );
    }

    // DELETED, NOT MOVED: `the_only_way_to_paint_is_a_snapshot_that_arrived_
    // as_one_value` used to sit here, asserting
    // `matches!(vault_load_step(false, Ok(a_snapshot())),
    // VaultLoadStep::Paint(VaultSnapshot { .. }))`. That cannot fail for any
    // implementation that COMPILES -- `Paint`'s payload type IS
    // `VaultSnapshot` -- so it was a tautology labelled as structural
    // coverage, and its other half duplicated
    // `a_readable_snapshot_is_painted_unforced_and_refetched_when_forced`
    // above (review 29's Minor 2). The claim it was standing in for now lives
    // where the data actually crosses the thread boundary: the channel
    // carries `Result<VaultSnapshot, VaultLoadFailure>`, so the tearing
    // spelling is not writable at either send site (review 29's Important 1).
    // That is a TYPE fact, and re-asserting a type fact from a test would be
    // the same tautology one layer out. The behavioural half is
    // `the_common_path_paints_both_halves_from_one_checked_read_without_fetching`,
    // which asserts a non-empty folders half end to end.

    #[test]
    fn a_superseded_give_up_is_not_classified_as_a_refresh_failure() {
        // The classification the toolbar rests on (review 29's Minor 3), and
        // the reason `VaultLoadFailure` has two variants at all: a sync that
        // was followed by a `clear` SUCCEEDED, so nothing downstream may
        // report it as a failed sync. Anything the populate itself got wrong
        // is a `Refresh` failure, and a standing "Synced just now" claim over
        // one of those IS wrong and gets corrected.
        assert!(matches!(
            vault_load_step(false, Err(VaultUnavailable::Superseded)),
            VaultLoadStep::GiveUp(VaultLoadFailure::Superseded(_))
        ));
        assert!(matches!(
            vault_read_after_populate(Err(VaultUnavailable::Superseded)),
            Err(VaultLoadFailure::Superseded(_))
        ));
        assert!(
            matches!(
                vault_read_after_populate(Err(VaultUnavailable::Unpopulated)),
                Err(VaultLoadFailure::Refresh(_))
            ),
            "a populate that reported success and left nothing readable is the refresh's own \
             failure, not a different vault session"
        );
    }

    #[test]
    fn a_clear_landing_after_the_populate_wrote_back_is_still_refused() {
        // The read AFTER the populate is era-checked too, and this is the
        // window the old code could not see at all: `populate()` reports
        // `Populated`, a `clear` lands, and the separate `cache.items()` /
        // `cache.folders()` reads then hand back the empty vault the clear
        // left -- data drawn from the absence of data, with a green result.
        assert!(vault_read_after_populate(Err(VaultUnavailable::Superseded)).is_err());
        // Unreachable after a successful populate in the same era, but a
        // distinct situation and not folded into the one above: it means the
        // snapshot is empty because nothing filled it, not because a
        // different vault session began. Either way there is nothing to
        // paint, so neither may be reported as `Ok`.
        assert!(vault_read_after_populate(Err(VaultUnavailable::Unpopulated)).is_err());
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
    use super::{
        apply_vault_load_result, vault_body_state, VaultBodyState, VaultLoadFailure,
        VAULT_SUPERSEDED_BEFORE_LOAD,
    };
    use crate::vault_bridge::VaultItem;
    use crate::vault_cache::VaultSnapshot;
    use crate::vault_window::detail::TotpState;

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
        let mut vault_load_error = None;
        let mut selected_id = Some("2".to_string());
        let mut sync_status = Some(Ok(()));
        let mut totp_state = TotpState::Code { code: "123456".to_string(), seconds_left: 9 };

        apply_vault_load_result(
            1, // this result's generation
            2, // the latest generation actually spawned
            Err(VaultLoadFailure::Refresh("connection refused".to_string())),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
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
        let mut vault_load_error = None;
        let mut selected_id = Some("stale".to_string());
        let mut sync_status = Some(Err("bw serve never became ready".to_string()));
        let mut totp_state = TotpState::Code { code: "123456".to_string(), seconds_left: 9 };

        apply_vault_load_result(
            1,
            2,
            Ok(VaultSnapshot { items: vec![item("late")], folders: Vec::new() }),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
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
        let mut vault_load_error = None;
        let mut selected_id = Some("pre-sync".to_string());
        let mut sync_status = Some(Ok(()));
        let mut totp_state = TotpState::Code { code: "123456".to_string(), seconds_left: 9 };

        apply_vault_load_result(
            2,
            2,
            Err(VaultLoadFailure::Refresh("bw serve never became ready".to_string())),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
        );

        assert_eq!(sync_status, Some(Err("bw serve never became ready".to_string())));
    }

    #[test]
    fn the_current_generation_success_updates_items_and_folders() {
        let mut items = Vec::new();
        let mut folders = Vec::new();
        let mut vault_loading = true;
        let mut vault_load_error = None;
        let mut selected_id = None;
        let mut sync_status = None;
        let mut totp_state = TotpState::Code { code: "123456".to_string(), seconds_left: 9 };

        apply_vault_load_result(
            1,
            1,
            Ok(VaultSnapshot { items: vec![item("a"), item("b")], folders: Vec::new() }),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
        );

        assert_eq!(ids(&items), vec!["a", "b"]);
        assert!(!vault_loading);
        assert_eq!(selected_id, Some("a".to_string()), "the first item is selected once nothing was selected yet");
        assert_eq!(sync_status, None, "a load with no preceding sync claim must not invent one");
    }

    /// Review 14's Important, part b. `NoCodeReported` deliberately stops
    /// polling, and `totp_state` was reset only on a *selection change*, so
    /// the obvious user response to the "no code available" row -- fix the
    /// item's authenticator key elsewhere, then click Sync -- replaced
    /// `items` while leaving the latched state behind, and the row stayed
    /// exactly as it was. A landed reload has to un-latch it.
    #[test]
    fn a_landed_reload_re_arms_the_totp_poll() {
        let mut items = vec![item("a")];
        let mut folders = Vec::new();
        let mut vault_loading = true;
        let mut vault_load_error = None;
        let mut selected_id = Some("a".to_string());
        let mut sync_status = Some(Ok(()));
        let mut totp_state = TotpState::NoCodeReported;

        apply_vault_load_result(
            1,
            1,
            Ok(VaultSnapshot { items: vec![item("a")], folders: Vec::new() }),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
        );

        assert_eq!(
            totp_state,
            TotpState::NoSecret,
            "a reload must re-arm the TOTP poll: NoCodeReported stops polling, so a user who \
             fixed the seed and clicked Sync would otherwise still see no code with nothing \
             on screen telling them to reselect the item"
        );
        // And the composed consequence: the very next frame's presence
        // derivation turns that back into a row for an item that has a seed.
        assert_eq!(
            super::totp_state_for_secret_presence(true, totp_state),
            TotpState::Fetching
        );
    }

    /// Review 15's Minor 3: the re-arm was unconditional inside the
    /// applied-`Ok` arm, so it also blanked a code that was on screen and
    /// live. The auto-sync that fires on the window's first frame makes this
    /// the ordinary case, not a corner: open the window on a TOTP item, the
    /// code appears, and a second later it flickers to "Fetching..." and
    /// back while the row's layout shifts under a user reaching for Copy.
    #[test]
    fn an_applied_reload_does_not_blank_a_live_code() {
        let mut items = vec![item("a")];
        let mut folders = Vec::new();
        let mut vault_loading = true;
        let mut vault_load_error = None;
        let mut selected_id = Some("a".to_string());
        let mut sync_status = None;
        let mut totp_state = TotpState::Code { code: "123456".to_string(), seconds_left: 9 };

        apply_vault_load_result(
            1,
            1,
            Ok(VaultSnapshot { items: vec![item("a")], folders: Vec::new() }),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
        );

        assert_eq!(
            totp_state,
            TotpState::Code { code: "123456".to_string(), seconds_left: 9 },
            "a reload must not replace a displayed code with \"Fetching...\" -- a live Code is \
             already polling, so there is no latch here for the re-arm to break"
        );
    }

    /// The reset belongs to a load that was actually *applied*. A superseded
    /// one is dropped whole -- re-arming a poll off it would be state the
    /// newer load is about to determine for itself.
    #[test]
    fn a_superseded_reload_does_not_re_arm_the_totp_poll() {
        let mut items = vec![item("a")];
        let mut folders = Vec::new();
        let mut vault_loading = false;
        let mut vault_load_error = None;
        let mut selected_id = Some("a".to_string());
        let mut sync_status = None;
        let mut totp_state = TotpState::Code { code: "123456".to_string(), seconds_left: 9 };

        apply_vault_load_result(
            1,
            2,
            Ok(VaultSnapshot { items: vec![item("late")], folders: Vec::new() }),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
        );

        assert_eq!(totp_state, TotpState::Code { code: "123456".to_string(), seconds_left: 9 });
    }

    /// A *failed* reload leaves the last known snapshot on screen (see the
    /// `Err` arm's own comment), so the state that goes with it stays too --
    /// re-arming here would drop a live code for data that did not change.
    #[test]
    fn a_failed_reload_does_not_re_arm_the_totp_poll() {
        let mut items = vec![item("a")];
        let mut folders = Vec::new();
        let mut vault_loading = true;
        let mut vault_load_error = None;
        let mut selected_id = Some("a".to_string());
        let mut sync_status = None;
        let mut totp_state = TotpState::Code { code: "123456".to_string(), seconds_left: 9 };

        apply_vault_load_result(
            1,
            1,
            Err(VaultLoadFailure::Refresh("connection refused".to_string())),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
        );

        assert_eq!(totp_state, TotpState::Code { code: "123456".to_string(), seconds_left: 9 });
    }

    /// Review 29's Minor 3, first half. A `Superseded` give-up at the INITIAL
    /// load left an EMPTY window under a neutral "Sync" pill: the `Err` arm's
    /// "keep whatever is on screen" is vacuous when nothing is on screen yet,
    /// and `sync_status` is `None` at window open so the override never
    /// fired. The failure now leaves its reason behind, and the body renders
    /// it instead of a blank vault.
    #[test]
    fn an_initial_load_that_gives_up_leaves_a_reason_for_the_empty_window() {
        let mut items: Vec<VaultItem> = Vec::new();
        let mut folders = Vec::new();
        let mut vault_loading = true;
        let mut vault_load_error = None;
        let mut selected_id = None;
        let mut sync_status = None;
        let mut totp_state = TotpState::NoSecret;

        apply_vault_load_result(
            1,
            1,
            Err(VaultLoadFailure::Superseded(VAULT_SUPERSEDED_BEFORE_LOAD)),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
        );

        assert!(!vault_loading, "the spinner must stop -- nothing else is coming");
        assert_eq!(
            vault_load_error.as_deref(),
            Some(VAULT_SUPERSEDED_BEFORE_LOAD),
            "the loader's own reason must survive the hand-off, or the window has nothing true \
             to say about why it is empty"
        );
        assert_eq!(
            vault_body_state(vault_loading, items.is_empty(), vault_load_error.as_deref()),
            VaultBodyState::Unavailable(VAULT_SUPERSEDED_BEFORE_LOAD),
            "an empty window after a give-up must SAY so rather than drawing an empty vault"
        );
    }

    /// Review 29's Minor 3, second half. On the ordinary post-sync path a
    /// `Superseded` give-up used to render a red "Sync failed" pill for a
    /// sync that actually SUCCEEDED -- `bw sync` ran, the cache refilled, for
    /// a different vault session. The sync claim is left alone and the true
    /// statement is made about the vault instead.
    #[test]
    fn a_superseded_give_up_does_not_report_a_successful_sync_as_failed() {
        let mut items = vec![item("a")];
        let mut folders = Vec::new();
        let mut vault_loading = false;
        let mut vault_load_error = None;
        let mut selected_id = Some("a".to_string());
        let mut sync_status = Some(Ok(()));
        let mut totp_state = TotpState::NoSecret;

        apply_vault_load_result(
            2,
            2,
            Err(VaultLoadFailure::Superseded(VAULT_SUPERSEDED_BEFORE_LOAD)),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
        );

        assert_eq!(
            sync_status,
            Some(Ok(())),
            "the sync succeeded; a vault session that ended afterwards is not a sync failure"
        );
        assert_eq!(vault_load_error.as_deref(), Some(VAULT_SUPERSEDED_BEFORE_LOAD));
        assert_eq!(
            ids(&items),
            vec!["a"],
            "and the last known snapshot stays on screen, as the Err arm has always promised"
        );
    }

    /// The notice describes the LAST load. A load that painted has nothing
    /// left to explain, so leaving the reason behind would put a red pill
    /// over a vault that is on screen and current.
    #[test]
    fn a_load_that_paints_clears_the_previous_failure_notice() {
        let mut items = Vec::new();
        let mut folders = Vec::new();
        let mut vault_loading = false;
        let mut vault_load_error = Some(VAULT_SUPERSEDED_BEFORE_LOAD.to_string());
        let mut selected_id = None;
        let mut sync_status = None;
        let mut totp_state = TotpState::NoSecret;

        apply_vault_load_result(
            1,
            1,
            Ok(VaultSnapshot { items: vec![item("a")], folders: Vec::new() }),
            &mut items,
            &mut folders,
            &mut vault_loading,
            &mut vault_load_error,
            &mut selected_id,
            &mut sync_status,
            &mut totp_state,
        );

        assert_eq!(vault_load_error, None);
        assert_eq!(ids(&items), vec!["a"]);
    }
}

#[cfg(test)]
mod sync_pill_tests {
    // Review 29's Minor 3. The pill is the ONLY place any of these states is
    // spelled for the user, and until this function existed the wording was
    // an `if` chain inside a 3000-line `eframe` closure that nothing could
    // call.
    use super::{sync_pill, theme, VAULT_NOT_REFRESHED_PILL};
    use std::time::Duration;

    #[test]
    fn a_give_up_after_a_successful_sync_reads_as_neither_success_nor_a_failed_sync() {
        let (dot, label) = sync_pill(
            false,
            Some(&Ok(())),
            Some("the vault was locked before this load could read it"),
            Duration::ZERO,
        );
        assert_eq!(
            label, VAULT_NOT_REFRESHED_PILL,
            "\"Sync failed\" would be a red pill on a sync that worked; \"Synced just now\" \
             would claim a refresh that never landed"
        );
        assert_eq!(dot, theme::ERROR, "it is still something the user has to act on");
    }

    #[test]
    fn a_real_sync_failure_still_says_so() {
        let (dot, label) = sync_pill(false, Some(&Err("boom".to_string())), None, Duration::ZERO);
        assert_eq!(label, "Sync failed");
        assert_eq!(dot, theme::ERROR);
    }

    #[test]
    fn a_sync_failure_outranks_a_load_failure_that_followed_it() {
        // The sync is the thing the user started, and its failure is the
        // stronger and more actionable statement.
        let (_, label) = sync_pill(false, Some(&Err("boom".to_string())), Some("also this"), Duration::ZERO);
        assert_eq!(label, "Sync failed");
    }

    #[test]
    fn an_in_flight_sync_outranks_every_finished_one() {
        let (dot, label) = sync_pill(true, Some(&Err("boom".to_string())), Some("also this"), Duration::ZERO);
        assert_eq!(label, "Syncing…");
        assert_eq!(dot, theme::TEXT_GHOST);
    }

    #[test]
    fn a_clean_success_still_reads_as_one() {
        let (dot, label) = sync_pill(false, Some(&Ok(())), None, Duration::from_secs(120));
        assert_eq!(label, "Synced 2 min ago");
        assert_eq!(dot, theme::BLUE);
    }

    #[test]
    fn nothing_reported_yet_is_neutral() {
        let (dot, label) = sync_pill(false, None, None, Duration::ZERO);
        assert_eq!(label, "Sync");
        assert_eq!(dot, theme::TEXT_GHOST);
    }
}

#[cfg(test)]
mod vault_body_state_tests {
    use super::{vault_body_state, VaultBodyState};

    #[test]
    fn a_load_in_flight_is_the_spinner_whatever_else_is_true() {
        assert_eq!(vault_body_state(true, true, Some("stale reason")), VaultBodyState::Loading);
    }

    #[test]
    fn an_empty_window_after_a_failure_says_what_happened() {
        assert_eq!(
            vault_body_state(false, true, Some("the vault was locked before this load could read it")),
            VaultBodyState::Unavailable("the vault was locked before this load could read it")
        );
    }

    #[test]
    fn an_empty_vault_that_simply_has_no_items_is_not_an_error() {
        // A real empty vault gets the normal chrome -- sidebar, list, empty
        // state -- not an error page.
        assert_eq!(vault_body_state(false, true, None), VaultBodyState::Vault);
    }

    #[test]
    fn a_failed_refresh_over_a_populated_window_keeps_showing_the_vault() {
        // The whole "keep the last known snapshot" behaviour. Replacing a
        // populated window with an error page because a background refresh
        // failed would be a worse regression than the blank window this
        // fixes; the pill reports it instead.
        assert_eq!(vault_body_state(false, false, Some("connection refused")), VaultBodyState::Vault);
    }
}

#[cfg(test)]
mod item_pane_frame_placement_tests {
    //! The item pane's panel frame must carry NO inner margin.
    //!
    //! Design 2b's white toolbar strip spans that pane edge to edge, and
    //! `item_list::draw_item_list` applies the design's two different
    //! paddings (12 for the strip, 10 for the list) itself. A margin here
    //! insets the strip, so it reads as a card floating on grey -- which is
    //! exactly the "search field should be on white tile" report, reopened.
    //!
    //! A SOURCE-TEXT GUARD, and stated plainly, because
    //! `item_list::toolbar_strip_tests` CANNOT catch this: those tests call
    //! `draw_item_list` directly with the full pane width, so a margin
    //! applied by this caller is invisible to them. Restoring the margin was
    //! probed and left all six of them green. Same idiom, and same
    //! split-literal rule, as `reveal_state_placement_tests` -- do not
    //! re-join these.
    const PANE: &str = concat!("egui::Panel::left(\"vault-item", "-list\")");
    const FRAME: &str = concat!("egui::Frame::new().fill(theme::CANVAS", "))");

    #[test]
    fn the_item_pane_panel_has_no_inner_margin() {
        let source = include_str!("mod.rs");
        let start = source
            .find(PANE)
            .unwrap_or_else(|| panic!("no {PANE:?} in this file -- the panel was renamed"));
        // Just the panel's own builder chain, not the whole file.
        let chain = &source[start..start + 400];
        assert!(
            chain.contains(FRAME),
            "the item pane's frame is no longer exactly {FRAME:?}. If an `inner_margin` was \
             added back, design 2b's white toolbar strip stops reaching the pane's edges and \
             the search field is on a floating card again; `item_list`'s own tests cannot see \
             this, because they hand `draw_item_list` the full pane width themselves"
        );
    }
}

#[cfg(test)]
mod window_geometry_tests {
    //! The two decisions either side of `settings::clamp_window_geometry`:
    //! what to open with, and what is worth writing back.
    use super::{geometry_to_record, initial_placement, WINDOW_SIZE};
    use crate::settings::{WindowGeometry, WindowPlacement, WorkArea};
    use eframe::egui;

    const SCREEN: WorkArea = WorkArea { x: 0, y: 0, width: 1920, height: 1040 };

    #[test]
    fn a_first_ever_launch_opens_at_the_design_size_and_lets_the_os_place_it() {
        assert_eq!(
            initial_placement(None, &[SCREEN]),
            WindowPlacement {
                width: WINDOW_SIZE[0] as i32,
                height: WINDOW_SIZE[1] as i32,
                position: None,
            },
            "with nothing stored there is nothing to clamp, and inventing a position would \
             put the window somewhere the OS did not choose for no reason at all"
        );
    }

    #[test]
    fn a_stored_geometry_goes_through_the_clamp_rather_than_straight_to_the_window() {
        // The regression this guards is the obvious shortcut: taking the
        // stored value as-is because it "was fine when we wrote it". It was
        // fine on the monitor that wrote it.
        let placement =
            initial_placement(Some(WindowGeometry { x: 9000, y: 9000, width: 10, height: 10 }), &[SCREEN]);
        assert_eq!(placement.width, 900, "the floor was applied");
        assert_eq!(placement.height, 600);
        let (x, y) = placement.position.expect("a known monitor yields a position");
        assert!(x + 900 <= 1920 && y + 600 <= 1040, "{placement:?} is off-screen");
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Option<egui::Rect> {
        Some(egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h)))
    }

    #[test]
    fn an_ordinary_window_records_its_rect_rounded_to_whole_points() {
        assert_eq!(
            geometry_to_record(rect(120.4, 80.6, 1240.2, 739.5), false, false),
            Some(WindowGeometry { x: 120, y: 81, width: 1240, height: 740 })
        );
    }

    #[test]
    fn a_maximized_window_records_nothing() {
        // Its rect is the whole work area. Recorded, one click of ▢ makes
        // every future launch full-screen and un-maximizing restores to the
        // same full screen -- the size the user actually chose is gone.
        assert_eq!(geometry_to_record(rect(0.0, 0.0, 1920.0, 1040.0), true, false), None);
    }

    #[test]
    fn a_minimized_window_records_nothing() {
        assert_eq!(geometry_to_record(rect(0.0, 0.0, 1240.0, 740.0), false, true), None);
    }

    #[test]
    fn a_window_with_no_reported_rect_records_nothing() {
        // What winit actually reports for a minimized window
        // (`update_viewport_info` sets both rects to `None`), and what egui
        // reports on a frame before the window has been measured.
        assert_eq!(geometry_to_record(None, false, false), None);
    }

    #[test]
    fn a_non_finite_rect_records_nothing() {
        // `WindowGeometry` is `i32`, so a NaN would land as some arbitrary
        // integer that every later comparison would treat as a real position.
        // Rejecting it here is the only place it can be rejected.
        assert_eq!(geometry_to_record(rect(f32::NAN, 0.0, 1240.0, 740.0), false, false), None);
        assert_eq!(geometry_to_record(rect(0.0, 0.0, f32::INFINITY, 740.0), false, false), None);
    }
}

#[cfg(test)]
mod window_era_placement_tests {
    // Review 29's Important 2. `run` must capture `cache.epoch().era()` ONCE,
    // before its loop, and hand that ONE value to every load it spawns.
    // Re-read at each spawn it answers "is this still the session that
    // existed one instruction ago?" -- yes, by construction -- while the call
    // site claims it answers "is this still the vault session the window is
    // SHOWING?". Nothing in the type system distinguishes the two: both are
    // a `VaultEra`, and a future edit that writes `era: cache.epoch().era()`
    // at a spawn site compiles, passes every other test in this file, and
    // silently restores the account-A/account-B hole. This is the same
    // source-text guard `reveal_state_placement_tests` uses, for the same
    // reason, and the same split-literal rule applies -- see that module's
    // comment. Do not re-join these.
    const ERA_READ: &str = concat!("cache.epoch()", ".era()");
    const CAPTURE: &str = concat!("let window_era = cache.epoch()", ".era();");
    const SPAWN_USE: &str = concat!("era: window", "_era,");
    // The first `#[cfg(test)]` in the file: everything after it is test code,
    // where reading the era per call is normal and expected.
    const TESTS_BEGIN: &str = concat!("#[cfg(", "test)]");

    fn production() -> &'static str {
        let source = include_str!("mod.rs");
        let end = source.find(TESTS_BEGIN).unwrap_or_else(|| {
            panic!("no {TESTS_BEGIN:?} in this file -- this guard needs a marker for where \
                    production code ends; if the test modules moved, update it")
        });
        &source[..end]
    }

    #[test]
    fn the_era_is_read_exactly_once_in_production_code() {
        let production = production();
        assert_eq!(
            production.matches(ERA_READ).count(),
            1,
            "{ERA_READ:?} appears more than once outside the tests. A second read is a \
             per-spawn capture: it equals the current era BY CONSTRUCTION, so the load it \
             tags can only detect a `clear` that lands after its own capture -- which is \
             exactly the guarantee `window_era`'s doc says this window does NOT rest on. \
             Capture once, before the loop, and hand that value to every spawn."
        );
        assert_eq!(
            production.matches(CAPTURE).count(),
            1,
            "{CAPTURE:?} is not the single read above -- `window_era` was renamed or the \
             capture was moved; update this needle if so, and check it is still before the \
             frame closure"
        );
    }

    #[test]
    fn both_spawns_are_checked_against_that_one_era() {
        // The initial load and the post-sync forced reload. If a third spawn
        // is ever added it must join them, and this count is what says so.
        assert_eq!(
            production().matches(SPAWN_USE).count(),
            2,
            "{SPAWN_USE:?} must appear once per `spawn_vault_load` call in `run` (the initial \
             load and the post-sync reload). Fewer means a spawn is checked against something \
             other than the window's own vault session."
        );
    }

    #[test]
    fn the_production_slice_is_not_quietly_shrinking() {
        // Review 31's Minor 5. `production()` slices at the FIRST `#[cfg(test)]`
        // and nothing said that no production item follows it -- true today,
        // but this file is ~4900 lines and a new `impl` appended below the test
        // modules would be invisible to every guard above. Two cheap checks:
        //
        //  * the WHOLE file holds exactly the two spawn uses the slice found,
        //    so a third spawn added after the marker cannot hide from
        //    `both_spawns_are_checked_against_that_one_era`;
        //  * the slice still covers most of the file, so if the marker ever
        //    moves to the top (a `#[cfg(test)] use` at the head, say) the
        //    guards fail loudly instead of passing over an empty string.
        let source = include_str!("mod.rs");
        assert_eq!(
            source.matches(SPAWN_USE).count(),
            2,
            "the whole file holds {} of {SPAWN_USE:?} but the production slice holds {}. If the \
             file has MORE, a spawn was added below the first `#[cfg(test)]` where the count \
             above cannot see it; if it has FEWER, a spawn stopped being checked against the \
             window's own era entirely",
            source.matches(SPAWN_USE).count(),
            production().matches(SPAWN_USE).count()
        );
        let covered = production().len() as f64 / source.len() as f64;
        assert!(
            covered > 0.5,
            "the production slice is only {:.0}% of the file: the first {TESTS_BEGIN:?} has \
             moved up, so every guard in this module is now inspecting a fraction of the \
             production code and passing for the wrong reason",
            covered * 100.0
        );
    }
}

#[cfg(test)]
mod frame_schedule_placement_tests {
    //! Review 31's Important 1. `run`'s frame closure must schedule the NEXT
    //! frame on EVERY path, including the ones that return early.
    //!
    //! The regression this pins: the `VaultBodyState::Unavailable` arm drew its
    //! error page and returned, skipping the `request_repaint_after` that used
    //! to sit only at the tail. Everything that needs a frame to happen at all
    //! -- the auto-lock deadline, the `sync_rx`/`vault_rx`/`favicon_rx`/
    //! `totp_rx` drains -- sits ABOVE the body match, so a window parked in
    //! that state stopped draining its channels and, far worse, STOPPED
    //! AUTO-LOCKING: `last_activity.elapsed()` is never evaluated on a frame
    //! that never runs, and `run` holds the main thread, so the tray, the
    //! global hotkey and the window watcher stay blocked the whole time.
    //!
    //! WHAT THIS MODULE CAN AND CANNOT SEE, PLAINLY. The frame closure lives
    //! inside `eframe::run_ui_native` and opens a real OS window; no test in
    //! this crate can call it, so no test can watch a real `Unavailable` frame
    //! and read its `repaint_delay` back. What IS observable is (a) the egui
    //! semantics the single hoisted call rests on -- see
    //! `the_tightest_request_in_a_frame_wins` below, which runs a real
    //! `egui::Context` -- and (b) the source-level fact that the call precedes
    //! every early return, which is what makes "every frame schedules the next
    //! one" true by construction rather than by three call sites agreeing.
    //! (b) is the same source-text guard idiom as `window_era_placement_tests`
    //! and `reveal_state_placement_tests`, and the same split-literal rule
    //! applies: do not re-join these needles, or they match themselves.
    use eframe::egui;
    use std::time::Duration;

    const SCHEDULE: &str = concat!("ui.ctx().request_repaint", "_after(FRAME_INTERVAL);");
    const BODY_MATCH: &str = concat!("match vault_body", "_state(");
    const ANY_SCHEDULE: &str = concat!("request_repaint", "_after(");
    const TESTS_BEGIN: &str = concat!("#[cfg(", "test)]");

    fn production() -> &'static str {
        let source = include_str!("mod.rs");
        let end = source
            .find(TESTS_BEGIN)
            .expect("no test marker in this file -- see `window_era_placement_tests`");
        &source[..end]
    }

    #[test]
    fn the_frame_schedules_its_successor_before_any_early_return() {
        let production = production();
        let schedule = production.find(SCHEDULE).unwrap_or_else(|| {
            panic!(
                "{SCHEDULE:?} is not in the production code. The frame closure must schedule the \
                 next frame in exactly one place, above the body match -- if it was renamed, \
                 update this needle; if it was moved back to the tail, the `Unavailable` body's \
                 early return silently stops the auto-lock timer again"
            )
        });
        let body = production.find(BODY_MATCH).unwrap_or_else(|| {
            panic!("{BODY_MATCH:?} is not in the production code -- update this needle")
        });
        assert!(
            schedule < body,
            "the repaint request is at byte {schedule}, below the body match at {body}. Both the \
             loading branch and the unavailable branch return from inside that match, so anything \
             below it runs only in the `Vault` state"
        );
        // Only the font-setup guard at the very top of the closure returns
        // before the schedule, and that one calls `request_repaint()` itself.
        assert_eq!(
            production[..schedule].matches("\n            return;").count(),
            1,
            "a second early return was added above the repaint request. Every path out of the \
             frame closure must have scheduled the next frame first"
        );
    }

    #[test]
    fn nothing_else_schedules_a_frame_behind_its_back() {
        // Exactly two: the one hoisted call, and the loading branch's tighter
        // refinement of it. A third would mean the cadence is back to being a
        // property of which branch you happen to be in.
        assert_eq!(
            production().matches(ANY_SCHEDULE).count(),
            2,
            "{ANY_SCHEDULE:?} must appear exactly twice in production: the unconditional \
             per-frame schedule and the loading branch's faster one"
        );
    }

    /// Runs `frame` on a SETTLED `egui::Context` and returns the delay egui
    /// scheduled for the next frame.
    ///
    /// MEASURED, NOT ASSUMED, and the reason this helper exists: a fresh
    /// `Context` reports `0ns` no matter what the frame asks for, because egui
    /// zeroes `repaint_delay` while any repaint is still outstanding (fonts,
    /// first layout). The first draft of this test read `0ns` where it expected
    /// `16ms` and would have "passed" against any production code at all had it
    /// been written the other way round. So: idle frames first, until egui
    /// reports `Duration::MAX` ("nothing pending, sleep until input") -- only
    /// then does the delay measure the frame rather than the warm-up.
    fn scheduled_delay(frame: impl FnMut(&mut egui::Ui)) -> Duration {
        fn delay_of(output: eframe::egui::FullOutput) -> Duration {
            output
                .viewport_output
                .values()
                .map(|v| v.repaint_delay)
                .min()
                .expect("a frame always produces one viewport")
        }
        let ctx = egui::Context::default();
        let mut settled = false;
        for _ in 0..16 {
            if delay_of(ctx.run_ui(Default::default(), |_| {})) == Duration::MAX {
                settled = true;
                break;
            }
        }
        assert!(settled, "the context never went idle -- this measurement would be of the warm-up");
        delay_of(ctx.run_ui(Default::default(), frame))
    }

    #[test]
    fn the_tightest_request_in_a_frame_wins() {
        // The fact the hoist rests on, asserted against the real egui rather
        // than assumed from its docs: calling the slow schedule first and the
        // spinner's fast one afterwards must leave the FAST one standing, or
        // hoisting would have slowed the loading spinner from 16ms to 500ms.
        //
        // Compared RELATIVELY, not against the literals passed in: egui
        // subtracts its predicted frame time from every request (so a 16ms ask
        // comes back as 0ns at the default 60fps). Pinning the arithmetic would
        // be pinning egui's internals; the property the production code needs
        // is only that the second, tighter call is the one that survives.
        let slow = scheduled_delay(|ui| {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        });
        let fast = scheduled_delay(|ui| {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        });
        let both = scheduled_delay(|ui| {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        });
        assert!(fast < slow, "the fixture is wrong if 16ms is not tighter than 500ms");
        assert_eq!(
            both, fast,
            "egui must keep the tightest request of the frame ({both:?} vs {fast:?}), or hoisting \
             the 500ms schedule above the loading branch would have slowed the spinner to 2fps"
        );
    }

    #[test]
    fn a_frame_that_only_makes_the_hoisted_request_is_still_scheduled() {
        // The `Unavailable` shape: no branch of its own adds anything, so the
        // hoisted call is the only one that runs, and it must still leave a
        // finite deadline behind.
        let hoisted_only = scheduled_delay(|ui| {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        });
        assert!(
            hoisted_only < Duration::from_millis(600),
            "a frame that makes only the unconditional request must be scheduled roughly at that \
             cadence, not {hoisted_only:?}"
        );
        // And the regression itself: a frame that requests nothing sleeps until
        // an input event wakes it, which is exactly how the auto-lock deadline
        // stopped being evaluated and the `sync_rx` drain stopped running.
        assert_eq!(
            scheduled_delay(|_| {}),
            Duration::MAX,
            "if this ever stops being MAX, egui has started polling on its own and this whole \
             finding changes shape -- do not weaken the production fix on that basis without \
             re-reading it"
        );
    }
}

#[cfg(test)]
mod poll_success_is_a_recovery_tests {
    use super::{apply_totp_poll_result, poll_success_is_a_recovery};
    use crate::vault_window::detail::TotpState;

    #[test]
    fn a_fetched_code_ends_the_failure_streak() {
        let mut state = TotpState::Unavailable;
        let error = apply_totp_poll_result(Ok(Some("123456".to_string())), 12, &mut state);
        assert!(error.is_none());
        assert!(poll_success_is_a_recovery(&state));
    }

    #[test]
    fn a_backend_reported_absence_is_not_a_recovery() {
        // Review 15's nit: the outage turned into a 400 rather than going
        // away, so "TOTP fetch recovered" directly contradicts the warning
        // logged one line above it about the same poll.
        let mut state = TotpState::Unavailable;
        let error = apply_totp_poll_result(Ok(None), 12, &mut state);
        assert!(error.is_none(), "Ok(None) carries no error -- which is how it reached the log");
        assert!(!poll_success_is_a_recovery(&state));
    }
}

#[cfg(test)]
mod totp_state_after_reload_tests {
    use super::totp_state_after_reload;
    use crate::vault_window::detail::TotpState;

    #[test]
    fn every_non_code_state_is_re_armed() {
        // Review 14's Important part b: a landed reload is what un-latches
        // NoCodeReported (which stops polling) and gives Unavailable a fresh
        // start; NoSecret/Fetching are already the neutral values.
        for state in [
            TotpState::NoSecret,
            TotpState::Fetching,
            TotpState::NoCodeReported,
            TotpState::Unavailable,
        ] {
            assert_eq!(totp_state_after_reload(state.clone()), TotpState::NoSecret, "{state:?}");
        }
    }

    #[test]
    fn a_live_code_survives_a_reload() {
        // Review 15's Minor 3. `Code` already polls (`totp_state_wants_poll`
        // is true for it), so there is no latch here for the re-arm to
        // break -- and blanking it is a visible flicker plus a layout shift
        // on the window's own first-frame auto-sync.
        let code = TotpState::Code { code: "123456".to_string(), seconds_left: 9 };
        assert_eq!(totp_state_after_reload(code.clone()), code);
    }

    #[test]
    fn a_surviving_code_is_still_cleared_when_the_seed_is_gone() {
        // The composed assertion (the lesson from review 13): keeping the
        // Code across a reload must not resurrect review 9's bug, where a
        // seed removed on another device left a stale code rendering under a
        // live countdown. The next frame's presence derivation, run against
        // the item the reload just delivered, is what clears it.
        let kept = totp_state_after_reload(TotpState::Code {
            code: "123456".to_string(),
            seconds_left: 9,
        });
        assert_eq!(super::totp_state_for_secret_presence(false, kept), TotpState::NoSecret);
    }
}

#[cfg(test)]
mod entered_no_code_reported_tests {
    // Review 14's Important: `Ok(None)` used to leave no trace anywhere --
    // no error to log, and the drain's success arm actively cleared the
    // failing flag. This is the predicate that gives it one, once.
    use super::entered_no_code_reported;
    use crate::vault_window::detail::TotpState;

    #[test]
    fn entering_the_state_is_logged() {
        assert!(entered_no_code_reported(&TotpState::Fetching, &TotpState::NoCodeReported));
        assert!(entered_no_code_reported(
            &TotpState::Code { code: "123456".to_string(), seconds_left: 9 },
            &TotpState::NoCodeReported
        ));
        assert!(entered_no_code_reported(&TotpState::Unavailable, &TotpState::NoCodeReported));
    }

    #[test]
    fn staying_in_the_state_is_not_logged_again() {
        assert!(!entered_no_code_reported(
            &TotpState::NoCodeReported,
            &TotpState::NoCodeReported
        ));
    }

    #[test]
    fn no_other_outcome_logs_this() {
        for after in [
            TotpState::NoSecret,
            TotpState::Fetching,
            TotpState::Code { code: "123456".to_string(), seconds_left: 9 },
            TotpState::Unavailable,
        ] {
            assert!(!entered_no_code_reported(&TotpState::Fetching, &after), "{after:?}");
        }
    }
}

#[cfg(test)]
mod fill_hotkey_applies_tests {
    use super::fill_hotkey_applies;
    use crate::vault_bridge::ItemKind;

    /// Ctrl+Shift+F is the keyboard equivalent of the "Fill in app" button,
    /// so it has to be gated by the same predicate. Gating only the button
    /// would leave the hotkey typing two empty strings into the focused
    /// window for exactly the kinds the button was hidden from -- a fix
    /// correct at one layer and inert at the door next to it, which is the
    /// shape this repository's ledger keeps recording.
    #[test]
    fn the_fill_hotkey_is_gated_by_the_same_rule_as_the_fill_button() {
        for kind in [
            ItemKind::Login,
            ItemKind::SecureNote,
            ItemKind::Card,
            ItemKind::Identity,
            ItemKind::SshKey,
            ItemKind::Unknown(9),
        ] {
            assert_eq!(
                fill_hotkey_applies(kind, true),
                crate::vault_window::detail::kind_offers_fill(kind),
                "{kind:?}: the hotkey and the button disagree"
            );
        }
    }

    #[test]
    fn an_unpressed_hotkey_fills_nothing_even_on_a_login() {
        assert!(!fill_hotkey_applies(ItemKind::Login, false));
    }
}

#[cfg(test)]
mod draw_read_arm_tests {
    //! The wiring tests for the detail pane's Read arm.
    //!
    //! `detail.rs`'s own tests prove `draw_detail_read` obeys the per-kind
    //! decisions. They cannot prove that `run` ever *calls* it, nor that the
    //! Ctrl+Shift+F gate is the one at the call site. Both of those were
    //! untested until this module existed, and both are load-bearing: commit
    //! b758f5e's entire contribution was deleting an `item_type != Some(1)`
    //! early return from this arm, and reinstating it left all 392 tests
    //! green while every kind-aware behaviour silently disappeared.
    //!
    //! These drive `draw_read_arm` -- the function that arm's body was
    //! hoisted into for exactly this reason -- headlessly, and read back what
    //! the frame actually painted.
    use super::{draw_read_arm, DetailAction, TotpState};
    use crate::theme;
    use crate::vault_bridge::{ItemKind, VaultItem};
    use eframe::egui;

    fn an_item(item_type: Option<i64>) -> VaultItem {
        VaultItem {
            id: "id-1".to_string(),
            name: "Sample".to_string(),
            fields: Vec::new(),
            login: None,
            card: None,
            identity: None,
            notes: None,
            item_type,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    fn item_type_for(kind: ItemKind) -> Option<i64> {
        match kind {
            ItemKind::Login => Some(1),
            ItemKind::SecureNote => Some(2),
            ItemKind::Card => Some(3),
            ItemKind::Identity => Some(4),
            ItemKind::SshKey => Some(5),
            ItemKind::Unknown(t) => Some(t),
        }
    }

    const EVERY_KIND: [ItemKind; 6] = [
        ItemKind::Login,
        ItemKind::SecureNote,
        ItemKind::Card,
        ItemKind::Identity,
        ItemKind::SshKey,
        ItemKind::Unknown(9),
    ];

    /// Runs one headless frame of `draw_read_arm` and returns both what it
    /// returned and every string it painted.
    ///
    /// `theme::apply`'s font set only takes effect at the start of the *next*
    /// frame, so two throwaway frames run first -- same reason `detail.rs`'s
    /// `painted_text` harness does.
    fn run_read_arm(item: &VaultItem, hotkey_pressed: bool) -> (DetailAction, Vec<String>) {
        let ctx = egui::Context::default();
        let hotkey_modifiers = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let input = |hotkey: bool| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 900.0),
            )),
            modifiers: if hotkey {
                hotkey_modifiers
            } else {
                egui::Modifiers::default()
            },
            events: if hotkey {
                vec![egui::Event::Key {
                    key: egui::Key::F,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: hotkey_modifiers,
                }]
            } else {
                Vec::new()
            },
            ..Default::default()
        };
        let _ = ctx.run_ui(input(false), |_ui| {});
        theme::apply(&ctx);
        let _ = ctx.run_ui(input(false), |_ui| {});

        let mut reveal = crate::vault_window::detail::RevealState::default();
        let mut action = DetailAction::None;
        let output = ctx.run_ui(input(hotkey_pressed), |ui| {
            action = draw_read_arm(
                ui,
                item,
                3,
                &TotpState::NoSecret,
                false,
                &mut reveal,
                None,
            );
        });

        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut texts);
        }
        (action, texts)
    }

    fn collect_text(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    fn contains(texts: &[String], needle: &str) -> bool {
        texts.iter().any(|t| t.contains(needle))
    }

    /// **The regression guard for b758f5e.** Reinstating the
    /// `item_type != Some(1)` early return -- which is what this arm did
    /// before that commit, and what Task 6 will be editing around -- makes
    /// every non-login kind fail here, because the pane it drew instead was
    /// the item's name over "This item type isn't editable in Deskwarden
    /// yet." and nothing else.
    ///
    /// The user's real vault is 1656 items, every one type 1, so nothing on
    /// this path is observable by running the app. These assertions are the
    /// only evidence that exists.
    #[test]
    fn the_read_arm_paints_a_real_pane_for_every_kind() {
        for kind in EVERY_KIND {
            let item = an_item(item_type_for(kind));
            let (_, texts) = run_read_arm(&item, false);
            assert!(
                texts.contains(&kind.label()),
                "{kind:?}: the read arm painted no {:?} subtitle, so `draw_detail_read` \
                 was not reached for it; painted: {texts:?}",
                kind.label()
            );
            assert!(
                contains(&texts, "Delete"),
                "{kind:?}: the read arm painted no Delete button; painted: {texts:?}"
            );
            assert!(
                !contains(&texts, "isn't editable in Deskwarden yet"),
                "{kind:?}: the read arm is short-circuiting to a placeholder again; \
                 painted: {texts:?}"
            );
        }
    }

    /// **The regression guard for the hotkey call site.** `fill_hotkey_applies`
    /// being correct proves nothing about whether the arm calls it: reverting
    /// this call site to a bare `ui.ctx().input(..)` check leaves
    /// `fill_hotkey_applies_tests` entirely green while Ctrl+Shift+F fills a
    /// card with two empty strings again.
    #[test]
    fn the_fill_hotkey_gate_is_wired_at_the_call_site_for_every_kind() {
        for kind in EVERY_KIND {
            let item = an_item(item_type_for(kind));
            let (action, _) = run_read_arm(&item, true);
            let expected = if crate::vault_window::detail::kind_offers_fill(kind) {
                DetailAction::Fill
            } else {
                DetailAction::None
            };
            assert_eq!(
                action, expected,
                "{kind:?}: Ctrl+Shift+F at the call site disagrees with kind_offers_fill"
            );
        }
    }

    /// The other half: the gate must not manufacture a fill out of nothing.
    #[test]
    fn an_unpressed_hotkey_returns_no_action_on_a_login() {
        let (action, _) = run_read_arm(&an_item(Some(1)), false);
        assert_eq!(action, DetailAction::None);
    }
}

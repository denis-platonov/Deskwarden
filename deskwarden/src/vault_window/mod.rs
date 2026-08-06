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
use crate::login_ui::{
    draw_window_chrome_with_extra, round_window_corners, ChromeAction, ChromeMetrics, CloseControl,
};
use crate::settings::AutoLock;
use crate::theme;
use crate::vault_bridge::{Folder, VaultError, VaultItem};
use crate::vault_cache::{PopulateOutcome, VaultCache, VaultEra, VaultSnapshot, VaultUnavailable};
use detail::{draw_detail_read, DetailAction, LaunchPlan, TotpState};
use detail_edit::{draw_detail_edit, EditAction, EditDraft};
use eframe::egui::{self, Margin};
use folder_modal::{draw_folder_edit_modal, FolderEditAction, FolderEditState};
use item_list::{draw_item_list, IconCache, ItemListAction};
use sidebar::{draw_sidebar, OutOfVault, SidebarAction, SidebarFilter};
use std::cell::RefCell;
use std::os::windows::process::CommandExt;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// What the sidebar shows where the "Locks in m:ss" countdown normally goes,
/// when auto-lock has been turned off in Preferences.
///
/// Not an empty string: the countdown occupying that corner is how the user
/// knows the idle timer is running at all, so a blank space there is
/// indistinguishable from a timer that has stopped working. Saying so out
/// loud also means the one place this window can tell you the vault will not
/// lock itself is the same place it otherwise tells you when it will.
const AUTO_LOCK_OFF_LABEL: &str = "Auto-lock is off";

/// Visible to the crate for `WINDOW_SIZE`'s reason just below: the login
/// window paints this window's titlebar, wordmark and all.
pub(crate) const WINDOW_TITLE: &str = "Deskwarden";
/// The size this window opens at the very first time, before it has ever been
/// closed and had its geometry recorded. Design 2b's own 1240x740.
///
/// Every later launch uses `Settings::vault_window` instead, run through
/// `settings::clamp_window_geometry` -- see `initial_placement`.
/// Visible to the crate because the LOGIN window opens at this window's
/// geometry and paints this window's empty panes behind its card, so the
/// user's sign-in does not end in a small card vanishing and a large window
/// appearing somewhere else (see `login_ui::vault_skeleton`). Copies of these
/// three numbers over there would be three numbers that drift.
pub(crate) const WINDOW_SIZE: [f32; 2] = [1240.0, 740.0];
pub(crate) const SIDEBAR_WIDTH: f32 = 212.0;
pub(crate) const LIST_WIDTH: f32 = 390.0;

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
///
/// Visible to the crate so the LOGIN window opens at the same placement this
/// one will restore to, by calling this rather than by reimplementing it --
/// see `login_ui::run_login_flow_for`.
pub(crate) fn initial_placement(
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

/// How this window gets the account email and server URL its toolbar shows:
/// already in hand, or on its way.
///
/// **The window does not wait for them.** They come from `bw status`, which
/// spawns the Bitwarden CLI and regularly takes 1-3 seconds on Windows (2.39s
/// measured on the user's machine), and `open_vault_window` used to call it
/// synchronously before eframe was even asked for a window -- so on a prefetch
/// miss the user clicked the tray and looked at nothing at all for that long.
/// What that call fetches is an avatar's initials and the host a favicon is
/// fetched from; neither is worth a second of blank screen.
///
/// [`Pending`](Self::Pending) is therefore the miss: the fetch runs on its own
/// thread, the window opens immediately, and the details are drained frame by
/// frame exactly the way [`spawn_vault_load`]'s item list already is -- the
/// same mechanism, not a second one. `Ready` is the hit (`main`'s prefetch, or
/// the startup window's own off-thread `bw status`), where the toolbar has
/// them on its very first frame and nothing appears late.
pub enum AccountDetails {
    /// Known before the window opens. The common case: `main` prefetches on a
    /// thread at startup and re-warms the cache from every window's result.
    Ready(crate::login_ui::BwStatusDetails),
    /// Not known yet, arriving over this channel. Never waited on -- see
    /// `details_rx`'s drain in [`build_frame`]'s closure.
    Pending(Receiver<crate::login_ui::BwStatusDetails>),
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
    /// True if the titlebar's gear was clicked: the user asked for the
    /// Preferences window, and this window closed only because eframe cannot
    /// nest one native window's event loop inside another's. The caller
    /// (`open_vault_window`) runs `prefs_ui::run`, applies whatever came
    /// back, and reopens this window.
    ///
    /// **A third field, not a reuse of either flag above, and the
    /// distinction is not cosmetic.** `locked` and `needs_reauth` are
    /// handled alike by the caller precisely because they mean the same
    /// thing about the session -- it is gone -- and both therefore run the
    /// full recovery: clear the cache, stop `bw serve`, re-authenticate,
    /// restart, repopulate. Opening Preferences means nothing whatsoever
    /// about the session. Folded into either flag, every visit to the gear
    /// would make the user re-enter their master password to get back to a
    /// vault that was never locked, and would tear down and restart a
    /// perfectly healthy backend to do it. Distinct situations get distinct
    /// fields here.
    pub open_preferences: bool,
    /// The account the user picked in the titlebar switcher. The window closed
    /// only because `main` has to tear one backend down and bring another one
    /// up, and that cannot happen while this window owns the event loop --
    /// exactly the reason [`open_preferences`](Self::open_preferences) exists.
    ///
    /// **A fourth field, and distinct from all three above.** `locked` and
    /// `needs_reauth` mean the session is gone; this session was never lost.
    /// Folded into either, asking to switch would run the lock recovery, and
    /// that recovery re-authenticates against **the account this process is
    /// already on** -- so the user would be asked for the master password of
    /// the account they were leaving, and would then be left on it. Folded into
    /// `open_preferences` it would open the preferences window instead.
    pub switch_to: Option<crate::accounts::AccountId>,
    /// True if the account menu's "Add account..." was clicked. The caller runs
    /// `add_account`, which opens a sign-in window and then settles onto the
    /// new account with the same switch every other account change goes
    /// through, and reopens this window afterwards.
    ///
    /// **A fifth field, distinct from all four above**, for the reason
    /// [`switch_to`](Self::switch_to) is distinct from
    /// [`locked`](Self::locked): the session was never lost, so the lock
    /// recovery would prompt for the master password of the account the user is
    /// on and leave them there. Distinct from `switch_to` too, which names an
    /// account that already exists — there is no id to send back for one that
    /// does not exist yet.
    pub add_account: bool,
    /// True if the account menu's "Remove this account..." was clicked. The
    /// caller confirms (`confirm_account_removal`, the one dialog, which
    /// defaults to No) and then runs `remove_account` on the **active**
    /// account.
    ///
    /// A sixth field for the same reason, and a bool rather than an id because
    /// the row removes the account this window is showing — the one the
    /// identity block at the top of that menu names. An id would be a second
    /// answer to "which account?" that could disagree with it.
    ///
    /// Only ever set when
    /// [`can_remove_active`](crate::accounts::AccountsState::can_remove_active)
    /// allowed the row to be drawn at all, so this is not a request the caller
    /// is expected to refuse — but the caller re-asks anyway, the way the switch
    /// re-asks `switchable()`.
    pub remove_account: bool,
    /// The account details this session ENDED UP WITH -- handed in
    /// [`AccountDetails::Ready`], or drained from the channel while the window
    /// was up. `None` only when the window was closed before a
    /// [`Pending`](AccountDetails::Pending) fetch reported back.
    ///
    /// **Not an outcome; the warm cache.** `open_vault_window` keeps one
    /// `cached_status_details` so the *next* open pays no `bw status` spawn,
    /// and it used to be refilled with the value that open had already
    /// blocked on. Nothing blocks on one any more, so the value the window
    /// actually used has to come back out the way every other thing the
    /// window learns does -- through here. Read by the caller immediately, so
    /// the account switch and the lock recovery still invalidate it
    /// afterwards: a stale email under a new account is worse than a blank
    /// one.
    pub account_details: Option<crate::login_ui::BwStatusDetails>,
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

/// Builds the vault window's per-frame closure, its viewport options, and the
/// handles its outcome is read back through -- WITHOUT opening a window.
///
/// The split exists because this UI now has two hosts. [`run`] is the one it
/// has always had: its own `eframe::run_ui_native`, which is what a tray
/// click still opens. The other is `app_window`, the single window that
/// carries sign-in, the spinner and then this vault inside ONE event loop, so
/// that signing in no longer closes one window and opens two more. eframe
/// cannot nest event loops, so the second host cannot call `run`; it needs
/// the closure without the loop around it, which is exactly what this returns.
///
/// `pre_styled` is the one behavioural knob. The closure's first frame
/// normally installs the fonts, rounds the window's corners and raises it --
/// work that belongs to whoever OWNS the window. `run` owns its window and
/// passes `false`; `app_window` did all three on its own first frame, long
/// before this vault frame existed, and passes `true` so the vault does not
/// re-raise a window the user may have deliberately sent behind something.
///
/// Mirrors `login_ui::run_login_flow`'s `Rc<RefCell<_>>` result handoff -- the
/// update closure is `FnMut + 'static` and can't return anything directly.
#[allow(clippy::too_many_arguments)]
pub fn build_frame<A: UiAutomationFiller + Clone + 'static, B: SendInputFiller + Clone + 'static>(
    cache: std::sync::Arc<VaultCache>,
    fill_stats: FillStats,
    injector: &Injector<A, B>,
    // The account email (the toolbar avatar's initials) and the server URL
    // (the host a favicon is fetched from), either in hand or on their way --
    // see [`AccountDetails`]. Passed in from `main.rs`'s single
    // `check_bw_status_details()` call rather than this function calling it
    // again itself -- that call spawns the `bw` CLI (~1-3s on Windows), and
    // this window used to pay that cost twice on every open (once here, once
    // in `main.rs`) with no UI feedback before the window even appeared.
    //
    // An enum rather than the two `Option<String>`s it used to be, because
    // "not known yet" and "known to be absent" are different states and only
    // the second may be painted as an answer: the toolbar draws an EMPTY
    // avatar circle while a fetch is in flight and favicons wait for the
    // server URL rather than guessing it (see the drain in the closure).
    details: AccountDetails,
    session_token: String,
    // Directory the on-disk favicon cache lives in (`main.rs`'s
    // `project_dirs.cache_dir().join("icons")`). Not created here --
    // `favicon::write_cached_icon` creates it lazily on first write.
    icon_cache_dir: std::path::PathBuf,
    // When -- if ever -- this window locks itself. Was a hardcoded
    // module-level constant ("until the 3e preferences window exists"); now
    // that `Settings` exists, `main.rs` reads it on every pass and passes it
    // in here. `AutoLock::Never` rather than a very large `Duration` because
    // the two are not the same thing: a duration is compared against
    // `last_activity.elapsed()` every frame, and any arithmetic on it can
    // produce a short one, whereas the variant simply has no comparison to
    // make. See `settings::AutoLock`.
    auto_lock: AutoLock,
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
    // What the titlebar's account switcher offers, and the one door to it (see
    // `account_switcher`). By value rather than by reference because the
    // update closure below is `'static`; `None` in exactly one state --
    // `StartupAccounts::NoAccountList`, where this app has no `Account` at all
    // -- and the titlebar then carries no switcher.
    accounts: Option<crate::accounts::AccountsState>,
    pre_styled: bool,
) -> (eframe::NativeOptions, VaultFrameFn, VaultFrameHandles) {
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

    // **The account details, and the fact that this window never waits for
    // them.** On a hit they are here already and every frame including the
    // first paints them. On a miss `details_rx` is `Some` and these two stay
    // `None` until the drain in the closure below fills them in -- which is
    // the whole change: the 2.39s `bw status` spawn that used to happen
    // before eframe was asked for a window now happens beside a window that
    // is already up.
    let (ready, mut details_rx) = match details {
        AccountDetails::Ready(details) => (Some(details), None),
        AccountDetails::Pending(rx) => (None, Some(rx)),
    };
    let mut server_url = ready.as_ref().and_then(|d| d.server_url.clone());
    let mut account_email = ready.as_ref().and_then(|d| d.user_email.clone());
    // What the caller re-warms its cache from -- see
    // `VaultWindowResult::account_details`. Seeded whole on the `Ready` path
    // (the window used those, so the next open should too) and set by the
    // drain below on the `Pending` one. The whole struct, never one rebuilt
    // from the two fields this window happens to read: `status` is the third
    // field and this window has no business inventing it.
    let account_details: Rc<RefCell<Option<crate::login_ui::BwStatusDetails>>> =
        Rc::new(RefCell::new(ready));
    let account_details_for_closure = account_details.clone();
    // When this frame was built, for the arrival log below. Not `opened_at`
    // in `main`: that clock now stops at the eframe handoff, because there is
    // nothing left between the click and the window for it to measure.
    let details_requested_at = Instant::now();

    let locked = Rc::new(RefCell::new(false));
    let locked_for_closure = locked.clone();
    // See `VaultWindowResult::needs_reauth`'s doc. Set from any write's
    // error arm below via `flag_reauth_if_unauthorized`, the same
    // `Rc<RefCell<_>>` handoff `locked` already uses -- the update closure
    // is `FnMut + 'static` and can't return anything directly.
    let needs_reauth = Rc::new(RefCell::new(false));
    let needs_reauth_for_closure = needs_reauth.clone();
    // See `VaultWindowResult::open_preferences`. Same `Rc<RefCell<_>>`
    // handoff as `locked` above and for the same mechanical reason (the
    // update closure is `FnMut + 'static` and cannot return anything), but
    // a separate cell rather than a share of either: see that field's doc
    // for why the three outcomes must not collapse into one.
    let open_preferences = Rc::new(RefCell::new(false));
    let open_preferences_for_closure = open_preferences.clone();
    // See `VaultWindowResult::switch_to`. A fourth cell rather than a share of
    // any of the three above, for the reason that field's doc gives.
    let switch_to: Rc<RefCell<Option<crate::accounts::AccountId>>> =
        Rc::new(RefCell::new(None));
    let switch_to_for_closure = switch_to.clone();
    // See `VaultWindowResult::add_account` and `::remove_account`. Two more
    // cells rather than a share of any above, for the reason those fields give.
    let add_account = Rc::new(RefCell::new(false));
    let add_account_for_closure = add_account.clone();
    let remove_account = Rc::new(RefCell::new(false));
    let remove_account_for_closure = remove_account.clone();
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
    // **The number nothing in this app reported, and the one the user was
    // actually timing.** `main.rs` logs how long it takes to hand off to
    // eframe (microseconds) and this module logs how long eframe takes to
    // paint a first frame (tens of milliseconds); between them they account
    // for a tenth of a second of the ten the report describes. The rest of it
    // is this: the window is up, painting its spinner, waiting for the load
    // below. Measured against the live backend, `/list/object/items` is 3.46s
    // cold and 0.065s warm, and a window opened occasionally is always the
    // cold case -- plus a readiness wait when `bw serve` was only just
    // started.
    //
    // Taken (not merely read) by the first result the drain actually APPLIES,
    // so it reports the initial load and never a later refresh, and so a
    // result dropped as superseded does not stop the clock on a load still in
    // flight. `log::info!` and one `Instant`, i.e. free: this app's slowest
    // visible action should be able to say what it spent the time on.
    let mut initial_load_started = Some(Instant::now());
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
    // The two lists that are NOT the live snapshot -- the trash and the
    // archive. Each is fetched off-thread the first time its row is selected
    // and dropped whenever the live vault is reloaded; see `AuxList`.
    let mut trash_list = AuxList::default();
    let mut archive_list = AuxList::default();
    // `Option` inside the `Ok`: `None` is a fetch that completed against a
    // vault session this window has since left. See `spawn_aux_load`.
    let (aux_tx, aux_rx): (
        mpsc::Sender<(u64, OutOfVault, Result<Option<Vec<VaultItem>>, AuxLoadError>)>,
        Receiver<(u64, OutOfVault, Result<Option<Vec<VaultItem>>, AuxLoadError>)>,
    ) = mpsc::channel();
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
    // The edit form's app block resolves a matched executable's real name (and
    // icon) off the UI thread and caches it per path -- see `app_identity`. It
    // lives HERE, beside `icons`, for the same two reasons: it must survive
    // across frames (a per-frame cache is a lookup per frame, which is what
    // this module exists to avoid), and it holds a GPU texture and a channel,
    // neither of which belongs on a `Clone` draft.
    let mut app_identities = crate::app_identity::AppIdentityCache::default();
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
    // repaint, everything -- for however long that call took. Since `4058e1c`
    // and `665a645` that is a bounded stall rather than an open-ended one:
    // the bridge's read agent is `http_agent::bounded_total`, so
    // `READ_DEADLINE` (10s) covers the whole request, body included, and a
    // `bw serve` that accepts a connection and then trickles bytes is cut off
    // at it. Ten seconds of frozen window is still ten seconds, and this poll
    // fires once per `TOTP_POLL_INTERVAL` against a backend nothing else
    // would notice had gone slow, so the bound is not a substitute for the
    // thread. Backgrounded the same
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
    // The app launch that has been asked for and has not happened yet. See
    // [`PendingLaunch`]: the Open arm records the request and NOTHING there
    // starts a program, so every launch in this window goes through the one
    // block that drains this -- and a plan carrying vault-supplied arguments
    // only gets there once the user has been shown the command line.
    let mut pending_launch: Option<PendingLaunch> = None;
    // The inline "that write did not happen" message, shown under the item
    // list's toolbar. Set by a drag-to-folder that the sidebar refused (the
    // virtual "No Folder" bucket, or the folder the item is already in) or
    // that the backend rejected, and by any of the four row commands that
    // move an item between this window's three lists -- see
    // `list_command_failure_message`. Cleared when the user clicks it away,
    // and whenever a new drag begins -- an explanation of the last gesture
    // must not still be sitting there describing the next one.
    //
    // Still named `move_error` while carrying more than a folder move: every
    // one of its sources is a write that was supposed to move an item
    // somewhere and did not, they all take the same precedence
    // (`NoticeSource::Move`, below Generate), and they all want the same
    // dismissal -- a plain clear with no side effect, unlike the Aux band's,
    // whose dismissal is also its retry.
    let mut move_error: Option<String> = None;
    // The inline "that Generate did not happen" message, shown in the same
    // band as `move_error` (see `inline_notice`, which decides between them).
    // Set by a failed generate in either detail-pane draft, cleared when the
    // user clicks it away and at the start of every fresh attempt -- an
    // explanation of the last click must not still be sitting there
    // describing the next one, exactly as for `move_error`.
    //
    // It lives out here, per window rather than per draft, because the draft
    // is `detail_edit`'s and the failure is not: `draw_detail_edit` has no
    // backend handle and cannot know a request failed. See
    // `EditAction::GeneratePassword`'s doc, which states that reporting is
    // the caller's job.
    let mut generate_error: Option<String> = None;

    // `pre_styled` when someone else already owns this window's first frame
    // -- see this function's doc. `false` is `run`'s own value and the
    // behaviour this window has always had.
    let mut styled = pre_styled;
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

    // Measured from just before `run_ui_native` to the first frame the
    // closure is handed, which is the span the user experiences as "nothing
    // is happening yet" -- eframe creating the OS window, choosing a
    // graphics backend and building the font atlas. None of it is
    // observable from inside the closure, so it has to be taken from out
    // here, and it is reported once rather than every frame.
    let eframe_handoff = std::time::Instant::now();

    let vault_frame_fn = move |ui: &mut egui::Ui, _frame: &mut eframe::Frame| {
        if !styled {
            log::info!("vault window: first frame {:?} after eframe was asked", eframe_handoff.elapsed());
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            round_window_corners(WINDOW_TITLE);
            // The OS window exists by this first painted frame (the same
            // hook `round_window_corners` uses), and this is where it is
            // brought to the front. See `foreground`: a refusal from Windows
            // flashes the taskbar button rather than being ignored.
            crate::foreground::raise_window(WINDOW_TITLE);
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
        // `Never` is handled by there being no timeout to compare against at
        // all -- not by a comparison against a large number. The sidebar
        // still gets a line, because a countdown that simply vanished would
        // read as the timer having broken rather than as a setting the user
        // themselves turned off.
        let lock_countdown = match auto_lock {
            AutoLock::Never => AUTO_LOCK_OFF_LABEL.to_owned(),
            AutoLock::After(timeout) => {
                if last_activity.elapsed() >= timeout {
                    *locked_for_closure.borrow_mut() = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                let remaining = timeout.saturating_sub(last_activity.elapsed());
                format!(
                    "Locks in {}:{:02}",
                    remaining.as_secs() / 60,
                    remaining.as_secs() % 60
                )
            }
        };

        while let Ok(result) = favicon_rx.try_recv() {
            if let Some((w, h, rgba)) = result.pixels {
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                let tex = ui.ctx().load_texture(result.item_id.clone(), image, egui::TextureOptions::default());
                icons.textures.insert(result.item_id, tex);
            }
        }

        // The account details, arriving from the thread `open_vault_window`
        // spawned instead of waiting for. Non-blocking exactly like the vault
        // load below and for the same reason: this window opened without
        // them, and a blocking receive here would be the freeze this whole
        // change removes, just moved inside the window where it would look
        // like a hang instead of a slow launch.
        //
        // One shot -- `details_rx` is dropped once a value lands, so this
        // costs one `try_recv` per frame until then and nothing afterwards.
        let arrived = details_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(details) = arrived {
            log::info!(
                "vault window: account details arrived {:?} after the window was handed to \
                 eframe (email {}, server {})",
                details_requested_at.elapsed(),
                if details.user_email.is_some() { "present" } else { "absent" },
                if details.server_url.is_some() { "present" } else { "absent" },
            );
            server_url = details.server_url.clone();
            account_email = details.user_email.clone();
            *account_details_for_closure.borrow_mut() = Some(details);
            details_rx = None;
        }
        // Whether the favicon base URL is KNOWN, not merely present:
        // `favicon::icon_base_url(None)` is Bitwarden's own cloud, which is
        // the right answer for an account that has no `serverUrl` and the
        // wrong one for a self-hosted account whose URL simply has not
        // arrived yet. `ensure_icon_loaded` marks every id it touches as
        // requested and the on-disk icon cache is keyed by DOMAIN ALONE, so
        // guessing here would not just draw the wrong icons in this window --
        // it would write them to disk under the right domain and serve them
        // to every later window that does know the server. Icons wait; the
        // rows they sit on do not.
        let icon_base_known = details_rx.is_none();

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
            // `vault_loading` is cleared by `apply_vault_load_result` only for
            // a result it actually applied -- a superseded one is dropped and
            // leaves it set -- so this is the moment the spinner comes off the
            // screen, whether that is because items landed or because the load
            // gave up. Both are reported: a ten-second wait that ends in a
            // failure is the worse of the two and the one most worth timing.
            if !vault_loading {
                if let Some(started) = initial_load_started.take() {
                    log::info!(
                        "vault window: vault load settled in {:?} -- {} items on screen{}",
                        started.elapsed(),
                        items.len(),
                        match vault_load_error.as_deref() {
                            Some(reason) => format!(", gave up: {reason}"),
                            None => String::new(),
                        }
                    );
                }
            }
            // A vault that has just been re-read makes both on-demand lists
            // suspect: the same `bw sync` that changed `items` can have
            // trashed, restored, archived or unarchived something elsewhere.
            // Dropped rather than refetched -- the fetch happens when a row
            // that needs one is actually selected, which for these two rows
            // is rare.
            trash_list.invalidate();
            archive_list.invalidate();
        }

        // The on-demand trash/archive fetch reporting back. Non-blocking like
        // every other drain here.
        //
        // `in_flight` is cleared BEFORE the generation check, deliberately
        // and for the reason the TOTP drain gives: a dropped result still
        // means the thread that set the flag has finished, and gating the
        // clear on currency is how a flag like this latches forever.
        if let Ok((generation, which, result)) = aux_rx.try_recv() {
            let list = match which {
                OutOfVault::Trash => &mut trash_list,
                OutOfVault::Archive => &mut archive_list,
            };
            list.in_flight = false;
            let label = which.label();
            if generation != load_generation {
                // Fetched against a vault this window has since reloaded.
                // Dropped, and the list stays unfetched, so selecting the row
                // asks again against the vault that is now on screen.
                log::info!("dropped a stale {label} list fetched against generation {generation}");
            } else {
                match result {
                    Ok(Some(fetched)) => {
                        list.items = Some(fetched);
                        list.error = None;
                    }
                    // Fetched against a vault SESSION this window has since
                    // left -- a `clear` began a new era while the request was
                    // in flight. Dropped exactly like a stale generation, and
                    // for a stronger reason: the generation check above says
                    // this window asked for a reload, this says the vault
                    // underneath was replaced, possibly by another account's.
                    // No error is shown, because nothing failed. See
                    // `spawn_aux_load`.
                    Ok(None) => {
                        log::info!(
                            "dropped a {label} list fetched against a superseded vault session"
                        );
                    }
                    // Straight onto the same re-auth path every other backend
                    // call in this window takes -- through the SAME helper,
                    // so a `401` from here cannot behave differently from a
                    // `401` anywhere else.
                    Err(AuxLoadError::Unauthorized) => {
                        flag_reauth_if_unauthorized(
                            ui.ctx(),
                            &needs_reauth_for_closure,
                            &VaultError::Unauthorized,
                        );
                        list.error = Some(format!("{label} could not be read: the vault session expired."));
                    }
                    Err(AuxLoadError::Other(message)) => {
                        log::warn!("could not read the {label} list: {message}");
                        // STATED, not swallowed. The alternative is a row
                        // that sits at an en dash forever with no reason,
                        // which is indistinguishable from a slow fetch.
                        list.error = Some(format!(
                            "{label} could not be read from the vault. Click here to try again."
                        ));
                    }
                }
            }
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
        match draw_window_chrome_with_extra(
            ui,
            WINDOW_TITLE,
            ChromeMetrics::VAULT,
            true,
            // The vault's ✕ is always live: this window owns nothing that a
            // close would strand. (The startup window's spinner stage does,
            // and disables it -- see `loading_ui::draw_spinner_body`.)
            CloseControl::Active,
            |ui| {
                // Right-to-left, so this reads left-to-right (nearest the title,
                // furthest from the window controls, first) as: Sync status
                // pill, avatar, account-menu chevron, settings gear.
                // Added here in the
                // opposite order (the gear closest to the window controls, sync
                // pill furthest) since `right_to_left` packs each new widget
                // just to the left of the previous one.
                //
                // The gear is therefore added FIRST precisely because it is
                // asked to sit to the RIGHT of the avatar -- in this layout,
                // earlier is further right. `the_settings_gear_sits_to_the_right_
                // of_the_avatar` asserts the painted rects rather than this
                // order, because reasoning about it is exactly how it would end
                // up on the wrong side.
                //
                // Design 2b specifies no settings affordance at all (there is no
                // gear, cog or "Settings" anywhere in `Deskwarden.dc.html`), so
                // this is the user's direction rather than the design's, the
                // same way the detail pane's star, kebab and eye were. What it
                // does take from 2b is its metrics: `theme::gear_button` is 28px
                // square, matching the Lock pill's height and the avatar's
                // diameter beside it.
                if theme::gear_button(ui).clicked() {
                    // The same two-step dance Lock does immediately below -- set
                    // the flag, then ask the window to close -- and for a reason
                    // specific to this control: `prefs_ui::run` is its own
                    // `eframe` window on this same thread, and eframe cannot
                    // nest one native event loop inside another. Calling it from
                    // inside this frame closure is not an option, so the request
                    // has to leave this window entirely and be served by the
                    // caller once this loop has ended.
                    *open_preferences_for_closure.borrow_mut() = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                // **The avatar, the chevron beside it, and the menu they open
                // together**, all inside one call -- see `account_menu`, which
                // owns both marks so that "an app with no account list draws no
                // menu control" is a decision a test can reach rather than an
                // `if` in this closure, which nothing can.
                //
                // Design 2b's avatar is a 28px *circle* -- `theme::avatar`
                // draws a rounded square (used elsewhere: item-list rows,
                // the detail pane header, neither of which were asked to
                // change), so it is painted directly rather than by changing
                // that shared helper's shape for every caller. 2b has no
                // switcher and no menu (it has one account), so those are the
                // user's direction the way the gear is; what they take from 2b
                // is its metrics, 28px square to match the gear and the avatar,
                // and the kebab menu in the detail pane for the rows.
                //
                // **Design 2b's "Lock CTRL+L" pill used to sit here and is
                // gone**, at the user's direction, now that Lock is a row in
                // this menu. `CTRL+L` still works -- it is handled by the key
                // check a few lines below, which never went through the pill --
                // and its hint rides the menu row so it stays discoverable.
                match account_menu(
                    ui,
                    accounts.as_ref(),
                    // Empty until the details land, and empty rather than
                    // `theme::initials("")`'s "?" placeholder: a question mark
                    // reads as an answer ("this account has no name"), and the
                    // truthful state here is that nothing has been said yet.
                    // The circle itself is drawn either way, so nothing left of
                    // it shifts sideways when the email arrives.
                    &avatar_initials(account_email.as_deref()),
                    account_email.as_deref(),
                    server_url.as_deref(),
                    // **The same fact the favicons wait on**, asked once. "No
                    // server URL" means Bitwarden's own cloud once `bw status`
                    // has answered and means nothing at all before it has, and
                    // this window already has exactly one name for that
                    // distinction.
                    icon_base_known,
                ) {
                    // The gear's two-step dance, for the same reason and one more:
                    // `main` cannot tear this account's backend down and bring the
                    // other one up while this window owns the event loop, and the
                    // master-password prompt the switch may raise is itself
                    // another eframe window on this same thread.
                    Some(AccountAction::Switch(picked)) => {
                        *switch_to_for_closure.borrow_mut() = Some(picked);
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    // **The `locked` flag the pill used to set**, not a path of
                    // its own: locking from the menu has to end exactly where
                    // locking from the pill and from `CTRL+L` end, which is the
                    // caller's one lock recovery -- sign-in for the account that
                    // was locked, and no switch anywhere else.
                    Some(AccountAction::Lock) => {
                        *locked_for_closure.borrow_mut() = true;
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    // Both open a window of their own on this same thread -- a
                    // sign-in for the add, a confirmation for the remove -- and
                    // both then run the switch sequence, so both leave this
                    // window exactly the way Preferences and a switch do.
                    Some(AccountAction::Add) => {
                        *add_account_for_closure.borrow_mut() = true;
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Some(AccountAction::Remove) => {
                        *remove_account_for_closure.borrow_mut() = true;
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    None => {}
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
            },
        ) {
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
        // A drag begun this frame supersedes whatever the LAST one had to
        // say, so the stale explanation goes before the sidebar can post a
        // new one. Read before the panels draw: the payload is on egui's
        // clipboard from the frame the drag starts until the frame it is
        // released on, and on that release frame this clear runs first and
        // the refusal below runs second, so a refusal is never wiped by the
        // gesture that produced it.
        if egui::DragAndDrop::has_payload_of_type::<item_list::DraggedItem>(ui.ctx()) {
            move_error = None;
        }
        // Set by a drop on a folder row below: `Ok` is a move to attempt,
        // `Err` a refusal the sidebar already decided. Drained after the
        // panel, for the same reason `row_command` is.
        let mut folder_drop: Option<Result<(String, String), &'static str>> = None;

        // The on-demand lists, started here -- BEFORE the panels draw, and
        // off-thread. The selected row is what asks for one, so a user who
        // never opens Trash or Archive never pays for either query, and a
        // user who does pays once per vault load rather than once per frame
        // (see `AuxList::wants_fetch`).
        let selected_source = filter.source().out_of_vault();
        for (which, list) in [
            (OutOfVault::Trash, &mut trash_list),
            (OutOfVault::Archive, &mut archive_list),
        ] {
            if list.wants_fetch(selected_source == Some(which)) {
                list.in_flight = true;
                spawn_aux_load(cache.clone(), which, load_generation, window_era, aux_tx.clone());
            }
        }
        egui::Panel::left("vault-sidebar")
            .exact_size(SIDEBAR_WIDTH)
            .resizable(false)
            // Design 4.8: `padding: 14px 10px` -- top/bottom 14, left/right
            // 10 (`Margin::symmetric`'s args are x=left/right, y=top/bottom,
            // the opposite order CSS shorthand uses).
            .frame(egui::Frame::new().fill(theme::CARD).inner_margin(Margin::symmetric(10, 14)))
            .show(ui, |ui| {
                // Every list this window holds, in the one shape the sidebar
                // and the item pane both read -- so a row's badge and its
                // contents cannot come from different places. Built at each
                // of the two call sites rather than once above them: it
                // borrows `items`, and the folder-drop handler between the
                // two panels has to be able to write to it.
                let lists = sidebar::VaultLists {
                    live: &items,
                    trash: trash_list.items.as_deref(),
                    archive: archive_list.items.as_deref(),
                };
                match draw_sidebar(ui, lists, &folders, &mut filter, &lock_countdown) {
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
                    // An item row was dragged onto a folder row. Not acted on
                    // here: this closure holds `items` borrowed for the
                    // sidebar's own counts, and the write has to be able to
                    // rewrite the entry it moves.
                    SidebarAction::MoveItemToFolder { item_id, folder_id } => {
                        folder_drop = Some(Ok((item_id, folder_id)))
                    }
                    // A drop the sidebar refused -- the virtual "No Folder"
                    // bucket, or the folder the item is already in. Carried
                    // out rather than swallowed: see `sidebar::CANNOT_UNFILE`.
                    SidebarAction::RefusedMove(reason) => folder_drop = Some(Err(reason)),
                    SidebarAction::None => {}
                }
            });

        // The drop the sidebar reported, acted on now that `items` is free.
        match folder_drop.take() {
            Some(Ok((item_id, folder_id))) => {
                move_error = move_item_into_folder(
                    ui.ctx(),
                    &cache,
                    &needs_reauth_for_closure,
                    &mut items,
                    &item_id,
                    &folder_id,
                );
            }
            Some(Err(reason)) => move_error = Some(reason.to_string()),
            None => {}
        }

        // Same reasoning as `vault-sidebar` above: no own stroke, `Panel`'s
        // built-in separator already draws the right-edge divider.
        // NO INNER MARGIN, unlike the sidebar's. Design 2b gives this pane a
        // white toolbar strip that spans its full width and a list area with
        // its own, different padding beneath -- one panel margin cannot be
        // both, and a margin here would inset the strip so it read as a card
        // floating on grey rather than the tile the design draws. Both
        // paddings live in `draw_item_list` instead; see its header comment.
        // Set by an item row's right-click menu below, and drained further
        // down once the selection that right-click made has been reacted to.
        let mut row_command: Option<(String, item_list::RowCommand)> = None;
        // A failed Trash/Archive fetch, for the row that is selected right
        // now. Cloned out of the `AuxList` so the panel closure below does
        // not hold it borrowed while the dismissal writes to it.
        let aux_error: Option<String> = match filter.source().out_of_vault() {
            Some(OutOfVault::Trash) => trash_list.error.clone(),
            Some(OutOfVault::Archive) => archive_list.error.clone(),
            None => None,
        };
        // The one message the inline band shows this frame, and which of the
        // three sources it came from -- see `inline_notice` for the order and
        // why Generate is first. Computed out here so the dismissal below can
        // clear exactly the source that was on screen.
        let notice = inline_notice(
            generate_error.as_deref(),
            aux_error.as_deref(),
            move_error.as_deref(),
        );
        let notice_source = notice.map(|(source, _)| source);
        // The inline band was clicked away. Applied after the closure for the
        // same reason its message is computed before it: the three `Option`s
        // the band reads are borrowed for the length of the call.
        let mut dismiss_move_error = false;
        egui::Panel::left("vault-item-list")
            .exact_size(LIST_WIDTH)
            .resizable(false)
            .frame(egui::Frame::new().fill(theme::CANVAS))
            .show(ui, |ui| {
                // THE LIST THE SELECTED ROW ACTUALLY READS, which for Trash
                // and Archive is not `items` at all -- those two rows are
                // separate queries whose results are disjoint from the live
                // vault (see `sidebar::FilterSource`). Passing `items` here
                // regardless is what made the Trash row list nothing: the
                // pane filtered a list that by construction holds none of
                // its members.
                //
                // `None` is carried through to `draw_item_list` rather than
                // flattened to `&[]` here, because the search placeholder
                // beneath the toolbar has to be able to tell "no items" from
                // "no answer yet" -- see `list_unless_unfetched`.
                let shown: Option<&[VaultItem]> = list_unless_unfetched(
                    filter.source(),
                    &items,
                    trash_list.items.as_deref(),
                    archive_list.items.as_deref(),
                );
                match draw_item_list(
                    ui,
                    shown,
                    &folders,
                    &filter,
                    &mut search,
                    &mut selected_id,
                    item_delete_pending.as_ref().map(|(id, _)| id.as_str()),
                    &icons,
                    &mut visible_ids,
                    notice.map(|(_, message)| message),
                    // NOT `notice.is_some()`: the band can be carrying a
                    // Generate or a Move failure, neither of which says
                    // anything about whether this list's fetch is still
                    // running. `aux_error` is this row's own `AuxList::error`
                    // and is the only one of the three that does.
                    aux_error.is_some(),
                ) {
                    // The kind the `+ New` menu was clicked on -- `empty_of`,
                    // not `empty`, which would open a login form whatever row
                    // the user picked. Always one of `CREATABLE_KINDS`; the
                    // menu has no other rows.
                    ItemListAction::NewItem(kind) => {
                        mode = DetailMode::Create(EditDraft::empty_of(kind))
                    }
                    // Not acted on here: this closure holds `items` borrowed
                    // (a Delete has to drain it) and, more importantly, the
                    // selection this right-click just made has not been
                    // reacted to yet -- see where `row_command` is handled,
                    // below the reset block.
                    ItemListAction::Row { id, command } => row_command = Some((id, command)),
                    ItemListAction::DismissMoveError => dismiss_move_error = true,
                    ItemListAction::None => {}
                }
            });

        // Only the source that was actually on screen is cleared. Clearing
        // all three would mean waving away one message also fired the
        // Trash/Archive refetch below, on a frame where the user had not seen
        // that failure at all.
        //
        // **That is a hazard going forward, not a bug that was here.** An
        // earlier version of this comment said clearing all three "meant"
        // exactly that, and it did not: while the band was
        // `aux_error.or(move_error)`, a move message reached the screen only
        // when `aux_error` was `None`, so `trash_list.error = None` was a
        // no-op and no refetch could fire. What makes the scenario reachable
        // for the first time is `inline_notice`'s precedence -- Generate
        // outranks Aux, so a generate failure can now be the message on
        // screen while a real `AuxList::error` sits behind it, and a
        // clear-all dismissal would fire that row's refetch on a click that
        // was about something else entirely.
        if dismiss_move_error {
            match notice_source {
                Some(NoticeSource::Generate) => generate_error = None,
                Some(NoticeSource::Move) => move_error = None,
                // Dismissing a Trash/Archive failure is also the retry:
                // clearing `error` is exactly what makes `AuxList::wants_fetch`
                // true again, so the next frame asks the server. A band that
                // could only be waved away, leaving the row at an en dash with
                // no way to try again short of a full Sync, would be a dead
                // affordance.
                Some(NoticeSource::Aux) => match filter.source().out_of_vault() {
                    Some(OutOfVault::Trash) => trash_list.error = None,
                    Some(OutOfVault::Archive) => archive_list.error = None,
                    None => {}
                },
                // The band reported a dismissal in a frame that drew no band.
                // Not reachable through `draw_item_list`, which only returns
                // `DismissMoveError` from inside the `if let Some(message)`
                // that draws it.
                None => {}
            }
        }

        // Load favicons for whatever `draw_item_list` actually drew this
        // frame, matching official Bitwarden clients ("visible items get
        // icons", not just the single selected one). `ensure_icon_loaded`'s
        // own already-resolved check makes this cheap on every frame after
        // the first for a given scroll position -- it's a HashMap/HashSet
        // lookup per id, not a fetch.
        //
        // Skipped entirely until the server URL is known -- see
        // `icon_base_known`. Nothing is lost by waiting: `favicon_requested`
        // is only marked by a call that actually runs, so the frame after the
        // details land asks for every icon this one skipped.
        for id in &visible_ids {
            if !icon_base_known {
                break;
            }
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

        // An entry of some item row's right-click menu, chosen this frame.
        //
        // Handled here, and deliberately AFTER the reset block above: a
        // right-click both opens the menu and selects the row, and that
        // reset clears `item_delete_pending`. Acting first would let the
        // reset wipe the arm a Delete entry had just set, so the two-click
        // confirmation could never reach its confirming click.
        //
        // Handled in its OWN block rather than folded into the read arm's
        // `DetailAction` match, because the item list is drawn in EVERY
        // mode: a command chosen while the edit form is open would otherwise
        // be silently dropped, and forcing the pane back to Read to receive
        // it would discard the draft. `Fill` and `Delete` go through the
        // same two helpers the read arm calls, so the two paths cannot
        // drift apart.
        //
        // The item is resolved from the id the menu carried rather than from
        // `selected_id`. They agree -- the right-click selected this row --
        // and that is exactly why neither has to be trusted to.
        if let Some((id, command)) = row_command.take() {
            // Resolved from the list the row was DRAWN from, not from `items`
            // -- a trashed or archived item is not in the live snapshot at
            // all, so looking it up there would find nothing and every entry
            // on those two menus would be a click that did nothing.
            let from_list = list_for(
                filter.source(),
                &items,
                trash_list.items.as_deref(),
                archive_list.items.as_deref(),
            )
            .iter()
            .find(|i| i.id == id);
            if let Some(item) = from_list.cloned() {
                let login = item.login.as_ref();
                match command {
                    // No reveal and no confirmation on either copy, matching
                    // the detail pane's own Copy buttons.
                    item_list::RowCommand::CopyUsername => {
                        if let Some(username) = login.and_then(|l| l.username.as_deref()) {
                            ui.ctx().copy_text(username.to_string());
                        }
                    }
                    item_list::RowCommand::CopyPassword => {
                        if let Some(password) = login.and_then(|l| l.password.as_deref()) {
                            ui.ctx().copy_text(password.to_string());
                        }
                    }
                    // Read out of the SAME `TotpState` the detail pane
                    // renders -- this does not start a fetch of its own, and
                    // deliberately does not touch the TOTP state machine.
                    // The right-click that opened this menu selected the
                    // row, which the reset block above turns into an
                    // immediate poll, so by the time an entry can be clicked
                    // the code has normally landed. When it has not (the
                    // backend is unreachable, or the click was faster than
                    // the round trip) there is no current code to copy and
                    // this says so in the log rather than putting a stale or
                    // empty one on the clipboard.
                    item_list::RowCommand::CopyTotp => match &totp_state {
                        TotpState::Code { code, .. } => ui.ctx().copy_text(code.clone()),
                        TotpState::NoSecret
                        | TotpState::Fetching
                        | TotpState::NoCodeReported
                        | TotpState::Unavailable => log::info!(
                            "\"Copy TOTP\" for {}: no current code yet ({:?})",
                            item.name,
                            totp_state
                        ),
                    },
                    item_list::RowCommand::Fill => {
                        fill_item_into_app(&item, &cache, &injector, &fill_stats);
                    }
                    item_list::RowCommand::OpenWebsite(url) => webbrowser_open(&url),
                    // Only from Read. A draft already open on this item IS
                    // what "Edit" asks for, so re-seeding it would do nothing
                    // but discard whatever the user had typed; the same goes
                    // for a half-filled "+ New" draft. Editing a DIFFERENT
                    // item is unaffected -- the right-click changed the
                    // selection, and the reset block above has already put
                    // the pane back into Read for it.
                    item_list::RowCommand::Edit => {
                        if matches!(mode, DetailMode::Read) {
                            mode = DetailMode::Edit(EditDraft::from_item(&item));
                        }
                    }
                    // Through `VaultCache`, never the bridge: the cache's
                    // snapshot is what the rest of the app reads, and its
                    // replay log is what stops an in-flight populate filing
                    // the item back where it was.
                    item_list::RowCommand::MoveToFolder(folder_id) => {
                        match cache.move_item_to_folder(&item, Some(folder_id.as_str())) {
                            Ok(moved) => {
                                if let Some(pos) = items.iter().position(|i| i.id == item.id) {
                                    // The value the cache wrote into its own
                                    // snapshot -- which is the SERVER's copy,
                                    // not a local rebuild of it. A rebuild
                                    // would carry the `revisionDate` this
                                    // write just superseded and the next
                                    // write of the row would be refused; see
                                    // `vault_bridge`'s `REVISION_DATE_KEY`.
                                    items[pos] = moved;
                                }
                            }
                            Err(e) => {
                                log::warn!(
                                    "failed to move item {} ({}) into folder {folder_id}: {e:?}",
                                    item.id,
                                    item.name
                                );
                                flag_reauth_if_unauthorized(
                                    ui.ctx(),
                                    &needs_reauth_for_closure,
                                    &e,
                                );
                            }
                        }
                    }
                    // The existing two-click confirmation, unchanged: the
                    // first choice arms `item_delete_pending` (which makes
                    // the entry read "Delete? Click to confirm" the next time
                    // the menu is opened), a second within the window
                    // confirms.
                    item_list::RowCommand::Delete => {
                        if confirm_click(&mut item_delete_pending, &item.id) {
                            if let Some(message) = delete_vault_item(
                                ui.ctx(),
                                &cache,
                                &needs_reauth_for_closure,
                                &mut items,
                                &mut selected_id,
                                &mut trash_list,
                                &item,
                            ) {
                                move_error = Some(message);
                            }
                        }
                    }
                    // Delete, just above, is the FIRST of the five commands
                    // that move an item between this window's three lists,
                    // and read this arm's comment as covering it too. It is a
                    // SOFT delete -- `delete_item` sends no `permanent=true`,
                    // which is what `purge_item` is for -- so it moves the
                    // item out of the live vault and into the Trash exactly
                    // as these four move items between the other pairs. Its
                    // body is in `delete_vault_item` rather than inline
                    // because the detail pane's kebab reaches the same
                    // command, and an invalidation written into this arm
                    // alone would have covered one of the two doors. (It
                    // owed both of the things below and did neither, for as
                    // long as this comment said "the four commands".)
                    //
                    // The four below go through
                    // `VaultCache`, never the bridge, so the snapshot and its
                    // pending-write log move with the server -- and all of
                    // them drop the on-demand list they touched rather than
                    // editing it in place, because that list is not cached
                    // anywhere and refetching it is the cheap, always-correct
                    // answer (`VaultCache::list_trash_unless_superseded`'s
                    // recorded decision).
                    //
                    // NOTHING HERE READS A LIST BACK TO CONFIRM THE WRITE.
                    // A 200 from `/archive/item/{id}` does not prove the
                    // state changed -- an item archived immediately after
                    // creation answered 200 and stayed in the default list
                    // until a ~1.5s settle -- so a read taken here would race
                    // that settle and report a failure that did not happen,
                    // and the caller would undo a correct archive. The
                    // refetch that does happen is the next time the row is
                    // opened, which is far past it.
                    //
                    // WHAT THAT COSTS, stated rather than glossed. If an
                    // archive genuinely did not take -- the measured
                    // 200-without-effect, as opposed to the settle -- the
                    // item is removed from `items` and from the cache
                    // snapshot and logged `deleted:true`, so it is in NEITHER
                    // the live rows nor the Archive row, and it stays
                    // invisible **until the user clicks Sync or reopens the
                    // window**. Nothing reconciles it before that: this app
                    // has no timed auto-sync (see the Sync pill's own comment
                    // further up, which says so outright), and the window's
                    // single first-frame `auto_synced` sync has long since
                    // fired by the time a row can be right-clicked. An
                    // earlier version of this reasoning leaned on "the vault
                    // window's 30s auto-sync", which does not exist.
                    //
                    // The item is not lost -- the next populate's
                    // `seq > since` retirement restores it correctly -- and a
                    // reload is deliberately NOT triggered from this arm: one
                    // fired here would land inside the very ~1.5s settle the
                    // no-read-back decision exists to avoid, fetch the item as
                    // still-live, and put it straight back into the list the
                    // user just archived it out of.
                    item_list::RowCommand::Archive => {
                        match cache.archive_item(&item) {
                            Ok(()) => {
                                items.retain(|i| i.id != item.id);
                                if selected_id.as_deref() == Some(item.id.as_str()) {
                                    selected_id = items.first().map(|i| i.id.clone());
                                }
                                archive_list.invalidate();
                            }
                            Err(e) => {
                                log::warn!("failed to archive item {}: {e:?}", item.id);
                                move_error = Some(list_command_failure_message(
                                    ListCommand::Archive,
                                    &item.name,
                                    &e,
                                ));
                                flag_reauth_if_unauthorized(ui.ctx(), &needs_reauth_for_closure, &e);
                            }
                        }
                    }
                    item_list::RowCommand::Unarchive => {
                        match cache.unarchive_item(&item) {
                            // THE CACHE'S COPY, not `item`. The cache read the
                            // item's current revision token back after the
                            // write (`VaultCache::current_revision_of`);
                            // pushing the caller's `item` here would put the
                            // pre-unarchive token straight back into the list
                            // the next edit of this item is built from, which
                            // is `fba91ff`'s defect one door along.
                            Ok(unarchived) => {
                                // The item is live again, so the window's own
                                // copy of the live list gains it -- exactly
                                // what the cache just did to its snapshot.
                                if !items.iter().any(|i| i.id == item.id) {
                                    items.push(unarchived);
                                }
                                archive_list.invalidate();
                            }
                            Err(e) => {
                                log::warn!("failed to unarchive item {}: {e:?}", item.id);
                                move_error = Some(list_command_failure_message(
                                    ListCommand::Unarchive,
                                    &item.name,
                                    &e,
                                ));
                                flag_reauth_if_unauthorized(ui.ctx(), &needs_reauth_for_closure, &e);
                            }
                        }
                    }
                    item_list::RowCommand::Restore => {
                        match cache.restore_item(&item) {
                            // THE CACHE'S COPY, for the reason the unarchive
                            // arm above states. It has already had
                            // `without_deleted_date` applied -- which is
                            // load-bearing: an item put back into a live list
                            // still carrying `deletedDate` would PUT that key
                            // on its next ordinary edit, at a backend whose
                            // handling of it is unverified -- and it carries
                            // the revision token the server reports now. This
                            // arm used to rebuild both of those locally and
                            // got the second one wrong.
                            Ok(restored) => {
                                if !items.iter().any(|i| i.id == item.id) {
                                    items.push(restored);
                                }
                                trash_list.invalidate();
                            }
                            Err(e) => {
                                log::warn!("failed to restore item {}: {e:?}", item.id);
                                move_error = Some(list_command_failure_message(
                                    ListCommand::Restore,
                                    &item.name,
                                    &e,
                                ));
                                flag_reauth_if_unauthorized(ui.ctx(), &needs_reauth_for_closure, &e);
                            }
                        }
                    }
                    // The only irreversible command in this window, on the
                    // same two-click confirmation as Delete.
                    item_list::RowCommand::PurgeForever => {
                        if confirm_click(&mut item_delete_pending, &item.id) {
                            match cache.purge_item(&item.id) {
                                Ok(()) => {
                                    if selected_id.as_deref() == Some(item.id.as_str()) {
                                        selected_id = None;
                                    }
                                    trash_list.invalidate();
                                }
                                Err(e) => {
                                    log::warn!("failed to purge item {}: {e:?}", item.id);
                                    move_error = Some(list_command_failure_message(
                                        ListCommand::Purge,
                                        &item.name,
                                        &e,
                                    ));
                                    flag_reauth_if_unauthorized(
                                        ui.ctx(),
                                        &needs_reauth_for_closure,
                                        &e,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::CANVAS).inner_margin(Margin::symmetric(20, 18)))
            .show(ui, |ui| {
                // Resolved from the list the selection was made in, for the
                // reason the row menu is: a row clicked under Trash or
                // Archive is not in `items`, so looking it up there would
                // leave the pane on "Select an item." for a row the user just
                // clicked.
                let selected_item = selected_id
                    .as_ref()
                    .and_then(|id| {
                        list_for(
                            filter.source(),
                            &items,
                            trash_list.items.as_deref(),
                            archive_list.items.as_deref(),
                        )
                        .iter()
                        .find(|i| &i.id == id)
                    })
                    .cloned();
                // `Some` only for Trash and Archive -- see `OutOfVault`,
                // which exists so this branch cannot be taken for a live
                // item.
                let out_of_vault = filter.source().out_of_vault();

                // The detail pane wants its own icon regardless of whether
                // the selected item also happens to be part of this frame's
                // `visible_ids` (it's a row in `item_list`'s scrolled range) --
                // if it's selected, it's effectively "visible" too. Cheap to
                // call every frame: see `ensure_icon_loaded`'s doc comment.
                //
                // And, like that loop, not before the server URL is known:
                // see `icon_base_known`.
                if let Some(item) = selected_item.as_ref().filter(|_| icon_base_known) {
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

                // -- the one TOTP poll, and the one countdown, for BOTH panes --
                //
                // HOISTED out of the `Read` arm rather than run in both of
                // them. The trigger and the `seconds_left` refresh used to
                // live inside that arm, and the `Edit` arm issued neither --
                // so opening the editor on an item whose code had already
                // been fetched showed THAT code, unrefreshed and with no
                // countdown, for the whole edit session. The keystroke
                // preview's `{TOTP}` was frozen: the user's own case
                // (`2134{TOTP}` -> `2134776699`) rendered a number that had
                // long since expired, and it never moved to
                // `NoCodeReported` either.
                //
                // Hoisted rather than copied because a second copy of the
                // trigger and the countdown is a second thing to keep in
                // step, and one-poll-one-answer is the very decision the
                // editor's `&totp_state` argument exists to express: the
                // form reads the vault window's one `TotpState` instead of
                // opening a poll of its own. Running the trigger in both
                // arms would keep one poll but grow two schedulers for it.
                // There is still exactly one of each, and now its scope is
                // the two panes that can show a code rather than one of
                // them.
                //
                // Gated to those two, and not simply run unconditionally:
                //
                //  * `out_of_vault` -- that pane deliberately drives NONE of
                //    this (see its arm: every action it could offer reads
                //    through the live list, which by definition does not
                //    hold the item). Polling for it would be the round trip
                //    that arm exists to refuse.
                //  * `Create` -- there is no item, which is exactly why the
                //    create form is handed `None` at the seam below.
                //
                // `item_delete_pending`'s expiry stays in the `Read` arm: it
                // is that pane's own armed confirmation, not a vault fact.
                if out_of_vault.is_none() && !matches!(mode, DetailMode::Create(_)) {
                    if let Some(item) = &selected_item {
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
                            // `bw serve`, and a stalled backend holds it
                            // -- and this window's entire UI thread with
                            // it -- for as long as the bridge's whole-
                            // request `READ_DEADLINE` allows, up to 10s,
                            // once per `TOTP_POLL_INTERVAL`. Bounded is
                            // not the same as short. The
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
                    }
                }

                match &mut mode {
                    // AN ITEM OUTSIDE THE LIVE VAULT GETS ITS OWN PANE, not
                    // the ordinary read pane with some buttons hidden. Every
                    // action that pane offers -- Edit, Fill, Delete, the copy
                    // rows, the favourite star, the TOTP poll -- reads or
                    // writes through the LIVE list, which by definition does
                    // not hold this item, so each of them would be a control
                    // that quietly did nothing. The state is stated instead,
                    // with the one action that works named.
                    //
                    // Checked before `mode` rather than inside the Read arm:
                    // an Edit draft cannot be open on an item that was never
                    // editable, and the selection-change reset has already
                    // put `mode` back to Read for the row that was clicked.
                    _ if out_of_vault.is_some() && selected_item.is_some() => {
                        detail::draw_out_of_vault_read(
                            ui,
                            selected_item.as_ref().expect("guarded above"),
                            out_of_vault.expect("guarded above"),
                        );
                    }
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
                                sidebar::folder_name(&folders, item.folder_id.as_deref()),
                                fill_count,
                                &totp_state,
                                delete_pending,
                                &mut reveal,
                                icons.textures.get(item.id.as_str()),
                                &mut app_identities,
                            );
                            // `item` and `totp_code` already hold everything
                            // a copy action needs -- `draw_detail_read` only
                            // needs to report *which* field was clicked, not
                            // hand the value back through its return type.
                            let login = item.login.as_ref();
                            match action {
                                DetailAction::Edit => mode = DetailMode::Edit(EditDraft::from_item(item)),
                                DetailAction::Fill => {
                                    fill_item_into_app(item, &cache, &injector, &fill_stats);
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
                                // The SSH private key, read back off the item
                                // through its one producer for the same
                                // reason the two card secrets are: what is
                                // copied is what was painted, trimming
                                // included.
                                DetailAction::CopySshPrivateKey => {
                                    if let Some(key) = item
                                        .ssh_key
                                        .as_ref()
                                        .and_then(|s| detail::ssh_key_fields(s).private_key)
                                    {
                                        ui.ctx().copy_text(key);
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
                                // **The band, not just the log.** A program
                                // file that has been uninstalled or moved
                                // since it was matched is the ordinary case
                                // here, and a click that starts nothing and
                                // says nothing is indistinguishable from a
                                // click that missed the button.
                                //
                                // **This arm no longer starts anything.** It
                                // records the request; the block after the
                                // panels is the one caller of `launch_app` in
                                // the program, and it is reachable only
                                // through a `LaunchConfirmAction::Launch`.
                                // For a plan with vault-supplied arguments
                                // that action can come from nowhere but the
                                // dialog, which is what makes "no launch
                                // without the command line on screen" a
                                // property of the shape rather than of a
                                // remembered `if`. See `PendingLaunch`.
                                DetailAction::OpenApp(plan) => {
                                    pending_launch =
                                        Some(PendingLaunch::new(item.name.clone(), plan));
                                }
                                // `set_favorite` returns the written item
                                // rather than `Ok(())` precisely so this
                                // cannot paint a favourited row from a write
                                // the server refused: the local copy is
                                // replaced with what came back, or nothing
                                // changes and the failure is reported.
                                DetailAction::ToggleFavorite(to) => {
                                    match cache.set_favorite(item, to) {
                                        // Named for the arm it belongs to so
                                        // the source pin in
                                        // `write_arms_adopt_the_backends_copy_tests`
                                        // can tell the three write arms
                                        // apart. This one already adopted the
                                        // cache's answer; the other two did
                                        // not, and that was the defect.
                                        Ok(starred) => {
                                            if let Some(pos) =
                                                items.iter().position(|i| i.id == starred.id)
                                            {
                                                items[pos] = starred;
                                            }
                                        }
                                        // The band, not just the log: see
                                        // `ItemWrite`. `to` is the direction
                                        // that was ASKED for, so a refused
                                        // favourite says "it still isn't one"
                                        // and a refused un-favourite says the
                                        // opposite -- both true of the item as
                                        // it now stands.
                                        Err(e) => {
                                            log::warn!("could not change the favourite flag: {e:?}");
                                            move_error = Some(item_write_failure_message(
                                                if to { ItemWrite::Favorite } else { ItemWrite::Unfavorite },
                                                &item.name,
                                                &e,
                                            ));
                                            flag_reauth_if_unauthorized(
                                                ui.ctx(),
                                                &needs_reauth_for_closure,
                                                &e,
                                            );
                                        }
                                    }
                                }
                                // The action carries the row INDEX, not the
                                // password: a previous password is a secret,
                                // and `DetailAction` is built by the render
                                // pass, so a value in it would be a plaintext
                                // copy living for the frame. Resolve it here.
                                DetailAction::CopyPasswordHistory(index) => {
                                    match crate::vault_bridge::password_history(item).get(index) {
                                        Some(entry) => ui.ctx().copy_text(entry.password.to_string()),
                                        None => log::warn!(
                                            "copy requested for history row {index}, which no \
                                             longer exists on this item"
                                        ),
                                    }
                                }
                                // As with the sidebar's folder ×,
                                // `confirm_click` gates this on a confirming
                                // second click -- see its doc comment. Only
                                // then does this actually call
                                // `cache.delete_item`.
                                DetailAction::Delete => {
                                    if confirm_click(&mut item_delete_pending, &item.id) {
                                        // The SECOND door onto the soft
                                        // delete. It gets the invalidation
                                        // and the band message for free
                                        // because both live in
                                        // `delete_vault_item`; all this arm
                                        // owes is routing the sentence to
                                        // the band, exactly as the row menu's
                                        // arm does.
                                        if let Some(message) = delete_vault_item(
                                            ui.ctx(),
                                            &cache,
                                            &needs_reauth_for_closure,
                                            &mut items,
                                            &mut selected_id,
                                            &mut trash_list,
                                            item,
                                        ) {
                                            move_error = Some(message);
                                        }
                                    }
                                }
                                // **The MATCHED APP card's two writes.** Both
                                // go through `cache.update_item`, which
                                // returns the SERVER's copy, and both adopt
                                // it -- see
                                // `write_arms_adopt_the_backends_copy_tests`
                                // and `vault_bridge`'s `REVISION_DATE_KEY`:
                                // keeping the value that was sent carries a
                                // superseded `revisionDate` and the item's
                                // next write is refused with a 400.
                                //
                                // NOT `cache.set_app_match`, deliberately.
                                // That method answers an `AppMatchWrite` --
                                // the diagnosis `main`'s "Add app..." handler
                                // needs before it rebuilds the match engine
                                // from the snapshot moments later -- and does
                                // not hand the written item back, so an arm
                                // built on it could not adopt anything. Both
                                // write through the same snapshot; this
                                // window's edits are already case 2 of
                                // `apply_sync_outcome`'s doc ("a WRITE -- an
                                // \"Add app...\" save, or any vault-window
                                // edit").
                                DetailAction::SetAppTrigger(to) => {
                                    match crate::vault_bridge::extract_app_match(item) {
                                        Some(current) => {
                                            // ONE field changes, and
                                            // `app_match_with_trigger` is
                                            // where that is decided -- this
                                            // arm never rebuilds the four
                                            // fields the picker captured off
                                            // a live window.
                                            let rebound = crate::vault_bridge::with_app_match(
                                                item,
                                                &detail::app_match_with_trigger(&current, to),
                                            );
                                            match cache.update_item(&rebound) {
                                                Ok(triggered) => {
                                                    if let Some(pos) =
                                                        items.iter().position(|i| i.id == item.id)
                                                    {
                                                        items[pos] = triggered;
                                                    }
                                                }
                                                Err(e) => {
                                                    log::warn!("could not change the autofill trigger: {e:?}");
                                                    move_error = Some(item_write_failure_message(
                                                        ItemWrite::AppTrigger,
                                                        &item.name,
                                                        &e,
                                                    ));
                                                    flag_reauth_if_unauthorized(
                                                        ui.ctx(),
                                                        &needs_reauth_for_closure,
                                                        &e,
                                                    );
                                                }
                                            }
                                        }
                                        // Only reachable if the field went
                                        // away between the frame that painted
                                        // the pills and this one. Nothing to
                                        // change, and nothing to write.
                                        None => log::warn!(
                                            "a trigger change was reported for {}, which no \
                                             longer carries an app match",
                                            item.id
                                        ),
                                    }
                                }
                                DetailAction::RemoveAppMatch => {
                                    let unbound = crate::vault_bridge::without_app_match(item);
                                    match cache.update_item(&unbound) {
                                        Ok(cleared) => {
                                            if let Some(pos) =
                                                items.iter().position(|i| i.id == item.id)
                                            {
                                                items[pos] = cleared;
                                            }
                                        }
                                        Err(e) => {
                                            log::warn!("could not remove the app match: {e:?}");
                                            move_error = Some(item_write_failure_message(
                                                ItemWrite::AppUnmatch,
                                                &item.name,
                                                &e,
                                            ));
                                            flag_reauth_if_unauthorized(
                                                ui.ctx(),
                                                &needs_reauth_for_closure,
                                                &e,
                                            );
                                        }
                                    }
                                }
                                DetailAction::None => {}
                            }
                        } else {
                            // Nothing is selected, so there is nothing a copy
                            // confirmation could belong to. Dropped rather
                            // than left ticking: without this, deselecting an
                            // item and reselecting it inside the five seconds
                            // brought the confirmation back, because
                            // `forget_copy_toast_on_item_change` sees the same
                            // id it recorded and leaves the toast alone. See
                            // `detail::forget_copy_toast`.
                            detail::forget_copy_toast(ui.ctx());
                            ui.label("Select an item.");
                        }
                    }
                    // The editors clear it for the same reason, by the same
                    // one call: copy a password, click Edit, cancel back
                    // inside the five seconds, and the read pane used to
                    // resume with the confirmation still up for a copy the
                    // user had looked away from.
                    DetailMode::Edit(draft) => {
                        detail::forget_copy_toast(ui.ctx());
                        match draw_detail_edit(ui, draft, &folders, false, &mut app_identities, selected_item.as_ref(), &totp_state) {
                            EditAction::Save => {
                                if let Some(item) = &selected_item {
                                    let updated = draft.apply_to(item);
                                    match cache.update_item(&updated) {
                                        // The SERVER's copy, not `updated`:
                                        // see the move arm above and
                                        // `vault_bridge`'s
                                        // `REVISION_DATE_KEY`. Reinstating
                                        // `updated` here is what made a
                                        // second save of one item fail.
                                        Ok(saved) => {
                                            if let Some(pos) = items.iter().position(|i| i.id == item.id) {
                                                items[pos] = saved;
                                            }
                                            mode = DetailMode::Read;
                                        }
                                        // See `ItemWrite`. `generate_error`
                                        // is dropped first because it
                                        // OUTRANKS this band (see
                                        // `inline_notice`) and the editor is
                                        // still open, so the clear at the
                                        // bottom of this closure cannot reach
                                        // it: a generate that failed earlier
                                        // in this same draft would otherwise
                                        // hide the refused save behind a
                                        // message about a box the user has
                                        // since moved on from.
                                        Err(e) => {
                                            log::warn!("failed to save item {}: {e:?}", item.id);
                                            generate_error = None;
                                            move_error = Some(item_write_failure_message(
                                                ItemWrite::Save,
                                                &item.name,
                                                &e,
                                            ));
                                            flag_reauth_if_unauthorized(
                                                ui.ctx(),
                                                &needs_reauth_for_closure,
                                                &e,
                                            );
                                        }
                                    }
                                }
                            }
                            // The box is left unchanged on failure, so a
                            // swallowed error here reads as a dead button --
                            // which is what `log::warn!` alone made of it,
                            // against `EditAction::GeneratePassword`'s own
                            // doc putting the reporting here.
                            // `generator_request` carries the form's own kind
                            // and size choices.
                            //
                            // The sentence goes to the item list's inline
                            // band via `generate_error`; `generate_failure`
                            // decides both it and, for a 401, a re-auth that
                            // deliberately does NOT close the window out from
                            // under a half-typed form the way every other
                            // write's `flag_reauth_if_unauthorized` does. The
                            // band is painted by a panel drawn earlier in
                            // this same frame, so it appears on the next one
                            // -- which always comes: the closure schedules a
                            // repaint unconditionally at its top.
                            EditAction::GeneratePassword => {
                                generate_error = None;
                                match cache.bridge().generate(&draft.generator_request()) {
                                    Ok(generated) => draft.set_generated_password(&generated),
                                    Err(e) => {
                                        log::warn!("could not generate a password: {e:?}");
                                        let failure = generate_failure(&e);
                                        if failure.needs_reauth {
                                            *needs_reauth_for_closure.borrow_mut() = true;
                                        }
                                        generate_error = Some(failure.message);
                                    }
                                }
                            }
                            // Opened HERE rather than inside the form's own
                            // closure, exactly as the generate above is: the
                            // shell's file dialog is modal and pumps its own
                            // message loop, so calling it mid-frame would
                            // re-enter egui's. A cancelled dialog answers
                            // `None` and nothing changes -- there is no failure
                            // to report, so this arm has no error band.
                            EditAction::PickAppFile => {
                                if let Some(path) = crate::file_picker::pick_executable() {
                                    draft.set_app_path(&path);
                                }
                            }
                            EditAction::Cancel => mode = DetailMode::Read,
                            EditAction::None => {}
                        }
                    }
                    DetailMode::Create(draft) => {
                        detail::forget_copy_toast(ui.ctx());
                        match draw_detail_edit(ui, draft, &folders, true, &mut app_identities, None, &totp_state) {
                            // `to_new_item` is fallible because `NewItem` has no
                            // variant for `ItemKind::Unknown(_)`: a future Bitwarden
                            // type has no create payload, and every total
                            // alternative lies -- returning a login would POST an
                            // item of the wrong type from a form filled in for
                            // something else. `detail_edit` already withholds Save
                            // for an uncreatable kind, so this `None` is the third
                            // door on a path that should be unreachable, not a case
                            // the user can provoke.
                            EditAction::Save => {
                                if let Some(new_item) = draft.to_new_item() {
                                    match cache.create_item(&new_item) {
                                        Ok(created) => {
                                            selected_id = Some(created.id.clone());
                                            items.push(created);
                                            mode = DetailMode::Read;
                                        }
                                        // See `ItemWrite`, and the Save arm
                                        // above for why `generate_error` goes
                                        // first. The name comes from the
                                        // DRAFT: there is no vault item to
                                        // name yet, which is the whole of
                                        // what a refused create means.
                                        Err(e) => {
                                            log::warn!("failed to create item: {e:?}");
                                            generate_error = None;
                                            move_error = Some(item_write_failure_message(
                                                ItemWrite::Create,
                                                new_item.name(),
                                                &e,
                                            ));
                                            flag_reauth_if_unauthorized(ui.ctx(), &needs_reauth_for_closure, &e);
                                        }
                                    }
                                } else {
                                    log::warn!(
                                        "Save reached an item kind with no create payload; \
                                         the form should not have offered it"
                                    );
                                }
                            }
                            // The box is left unchanged on failure, so a
                            // swallowed error here reads as a dead button --
                            // which is what `log::warn!` alone made of it,
                            // against `EditAction::GeneratePassword`'s own
                            // doc putting the reporting here.
                            // `generator_request` carries the form's own kind
                            // and size choices.
                            //
                            // The sentence goes to the item list's inline
                            // band via `generate_error`; `generate_failure`
                            // decides both it and, for a 401, a re-auth that
                            // deliberately does NOT close the window out from
                            // under a half-typed form the way every other
                            // write's `flag_reauth_if_unauthorized` does. The
                            // band is painted by a panel drawn earlier in
                            // this same frame, so it appears on the next one
                            // -- which always comes: the closure schedules a
                            // repaint unconditionally at its top.
                            EditAction::GeneratePassword => {
                                generate_error = None;
                                match cache.bridge().generate(&draft.generator_request()) {
                                    Ok(generated) => draft.set_generated_password(&generated),
                                    Err(e) => {
                                        log::warn!("could not generate a password: {e:?}");
                                        let failure = generate_failure(&e);
                                        if failure.needs_reauth {
                                            *needs_reauth_for_closure.borrow_mut() = true;
                                        }
                                        generate_error = Some(failure.message);
                                    }
                                }
                            }
                            // Unreachable, and spelled out rather than caught
                            // by a `_ =>`: the create form draws no app block
                            // (a draft with no item behind it carries no
                            // binding -- see `EditDraft::app`), so there is no
                            // Browse button here to click. `EditAction` is
                            // matched exhaustively so that a control added to
                            // one of the two forms cannot be silently ignored
                            // by the other.
                            EditAction::PickAppFile => log::warn!(
                                "the create form asked for a file picker it does not draw"
                            ),
                            EditAction::Cancel => mode = DetailMode::Read,
                            EditAction::None => {}
                        }
                    }
                }
            });

        // A GENERATE FAILURE BELONGS TO AN OPEN DRAFT, AND DIES WITH IT.
        //
        // `generate_error` used to be cleared only by the band's own
        // dismissal and at the start of the next generate. Neither
        // `EditAction::Cancel` nor the selection-change reset touched it, and
        // `inline_notice` ranks Generate above Move -- so a generate failure
        // the user had walked away from (cancel the form, click another item)
        // kept the band and outranked every later "Couldn't archive ..." they
        // provoked. One click surfaced the archive message on the next frame,
        // so it was never a strand; what it cost was that the refused archive
        // was invisible for that click, which is the whole of what the band
        // exists to prevent.
        //
        // One condition rather than a clear at each of the five exits
        // (Cancel and a successful Save in each of the two editors, plus the
        // selection reset), because it states the actual rule instead of
        // enumerating today's doors: the sentence explains a click made
        // INSIDE an editor, and `DetailMode::Read` is precisely "there is no
        // editor". A failing Generate leaves `mode` on `Edit`/`Create`, so
        // this cannot clear the message on the frame it was set, and the
        // band is computed at the TOP of the next frame -- after this ran,
        // which is what makes the message disappear in one frame rather than
        // lingering for one more.
        //
        // `move_error` has the analogous gap and does not get the analogous
        // line: it is already bounded by the drag-begin clear further up, and
        // it is the band's LOWEST-ranked source, so a stale one hides nothing.
        let editor_is_closed = matches!(mode, DetailMode::Read);
        if editor_is_closed {
            generate_error = None;
        }

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

        // **The only place in this program that starts an app match.** Drawn
        // here for `folder_edit`'s reason -- last, so the dialog is over the
        // three panels -- and drained with `take`, so a launch happens once
        // and the request is gone whichever way it ended.
        //
        // A plan with no vault-supplied arguments is approved on arrival and
        // never draws a dialog: it launches on this same frame, exactly as
        // every launch did before, with no extra click.
        if let Some(pending) = pending_launch.take() {
            let decision = if pending.needs_confirmation {
                draw_launch_confirm_modal(ui.ctx(), &pending)
            } else {
                LaunchConfirmAction::Launch
            };
            match decision {
                // **The band, not just the log.** A program file that has
                // been uninstalled or moved since it was matched is the
                // ordinary case here, and a click that starts nothing and
                // says nothing is indistinguishable from a click that missed
                // the button.
                LaunchConfirmAction::Launch => {
                    if let Err(e) = launch_app(&pending.plan) {
                        log::warn!("could not start {}: {e}", pending.plan.program);
                        move_error = Some(app_launch_failure_message(
                            &pending.item_name,
                            &pending.plan.program,
                            &e,
                        ));
                    }
                }
                // Dropped: `take` already removed it, so the dialog is gone
                // and nothing ran.
                LaunchConfirmAction::Cancel => {}
                // Still up. Put back exactly as it was -- in particular
                // `needs_confirmation` is not cleared here, because a
                // confirmation that disarmed itself while still on screen
                // would launch on whatever the next frame decided.
                LaunchConfirmAction::None => pending_launch = Some(pending),
            }
        }

        // NOTHING SCHEDULES THE NEXT FRAME HERE ANY MORE. The
        // `request_repaint_after` that used to close this closure was reachable
        // only in the `Vault` state, because two branches of the body match
        // return before it -- see the hoisted call at the TOP of the closure,
        // next to `draw_resize_handles`, and review 31's Important 1.
    };

    (
        options,
        Box::new(vault_frame_fn),
        VaultFrameHandles { locked, needs_reauth, open_preferences, switch_to, add_account, remove_account, account_details, last_geometry, settings_path },
    )
}

/// The vault UI's per-frame closure, boxed so it can be stored in a struct
/// and handed to either host.
pub type VaultFrameFn = Box<dyn FnMut(&mut egui::Ui, &mut eframe::Frame)>;

/// The cells [`build_frame`]'s closure reports its outcome through, and the
/// one file write that outcome implies.
///
/// A struct rather than four loose `Rc`s so that BOTH hosts end a vault
/// session the same way: by calling [`VaultFrameHandles::finish`]. The
/// geometry write in particular used to sit inline in `run` after the event
/// loop returned, where a second host would simply not have had it -- a vault
/// session that forgot the window's size, silently, on exactly the path this
/// split was made for.
pub struct VaultFrameHandles {
    locked: Rc<RefCell<bool>>,
    needs_reauth: Rc<RefCell<bool>>,
    open_preferences: Rc<RefCell<bool>>,
    switch_to: Rc<RefCell<Option<crate::accounts::AccountId>>>,
    /// See [`VaultWindowResult::add_account`] and
    /// [`VaultWindowResult::remove_account`].
    add_account: Rc<RefCell<bool>>,
    remove_account: Rc<RefCell<bool>>,
    /// See [`VaultWindowResult::account_details`]. Written either before the
    /// window opened (a `Ready` hand-in) or by the frame that drained the
    /// fetch, and read back out by [`finish`](Self::finish) so the caller's
    /// cache is warm for the next open.
    account_details: Rc<RefCell<Option<crate::login_ui::BwStatusDetails>>>,
    last_geometry: Rc<RefCell<Option<crate::settings::WindowGeometry>>>,
    settings_path: Option<std::path::PathBuf>,
}

impl VaultFrameHandles {
    /// Ends a vault session: persists the geometry and reads the four
    /// outcome cells. Call once, after the frame closure has stopped running.
    pub fn finish(&self) -> VaultWindowResult {
        // One write, here, after the window is gone -- not per frame, which
        // during a resize drag would be a file write per repaint. A failure is
        // logged and otherwise ignored: losing the remembered size is a smaller
        // problem than anything worth failing a lock/close over, and
        // `Settings::load` treats whatever is (or is not) on disk as advisory
        // anyway. Read-modify-write, so a preference changed in the preferences
        // window while this one was open is not reverted -- see
        // `persist_vault_window_geometry`.
        if let (Some(path), Some(geometry)) =
            (self.settings_path.as_deref(), *self.last_geometry.borrow())
        {
            if let Err(e) = crate::settings::Settings::persist_vault_window_geometry(path, geometry)
            {
                log::warn!("could not save the vault window's geometry: {e}");
            }
        }

        let locked = *self.locked.borrow();
        let needs_reauth = *self.needs_reauth.borrow();
        // Read out of its own cell and reported as its own field. The geometry
        // write just above has already happened by this point, which is what
        // makes the caller's `persist_preferences` safe to run next: this
        // window's `settings.json` write is done, so the preferences save cannot
        // race it, and `persist_preferences` is a read-modify-write of the two
        // preference fields only, so it cannot clobber the geometry either.
        let open_preferences = *self.open_preferences.borrow();
        let switch_to = self.switch_to.borrow_mut().take();
        let add_account = *self.add_account.borrow();
        let remove_account = *self.remove_account.borrow();
        // Cloned rather than taken: `finish` is documented "call once", but a
        // second call returning a result with no details in it would silently
        // cost the caller its warm cache rather than failing, and this is the
        // one field whose absence means "spawn the CLI again".
        let account_details = self.account_details.borrow().clone();
        VaultWindowResult { locked, needs_reauth, open_preferences, switch_to, add_account, remove_account, account_details }
    }
}

/// Opens the vault window in its OWN event loop and blocks until it's closed
/// (the X/window-close path) or locked (the `Lock` button or the auto-lock
/// timer).
///
/// This is the tray-click host. The startup host is `app_window`, which calls
/// [`build_frame`] directly -- see that function's doc.
#[allow(clippy::too_many_arguments)]
pub fn run<A: UiAutomationFiller + Clone + 'static, B: SendInputFiller + Clone + 'static>(
    cache: std::sync::Arc<VaultCache>,
    fill_stats: FillStats,
    injector: &Injector<A, B>,
    details: AccountDetails,
    session_token: String,
    icon_cache_dir: std::path::PathBuf,
    auto_lock: AutoLock,
    backend_already_running: bool,
    accounts: Option<crate::accounts::AccountsState>,
) -> VaultWindowResult {
    let (options, mut frame_fn, handles) = build_frame(
        cache,
        fill_stats,
        injector,
        details,
        session_token,
        icon_cache_dir,
        auto_lock,
        backend_already_running,
        accounts,
        // This host owns its window, so its first frame is the one that
        // installs the fonts, rounds the corners and raises it.
        false,
    );

    let _ = eframe::run_ui_native(WINDOW_TITLE, options, move |ui, frame| frame_fn(ui, frame));

    handles.finish()
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
/// "Fill in app" for `item`: resolve its matched process to an open window
/// and hand the fill to `app::fill_from_vault`.
///
/// Shared by the detail pane's Fill button and an item row's context-menu
/// entry. Extracted rather than copied because both non-fatal outcomes
/// (nothing matched yet, matched but not running) are only ever reported in
/// the log, and a second copy of that reporting is a second place for it to
/// go quietly missing.
/// Why "Fill in app" did nothing, when it did nothing.
///
/// Three refusals rather than two, because the third used to be reported as
/// the second: a dead binding fell through to "isn't currently open", which
/// is false and reads as "try again once it is running" about a binding that
/// will never fire however many apps are running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FillRefusal {
    /// The item carries no readable `deskwarden:app-match` field.
    NotBound,
    /// It carries one that [`detail::app_match_is_dead`] calls dead -- see
    /// [`fill_target`].
    BindingIsDead,
    /// A live binding whose app is not among the open windows.
    NotOpen,
}

/// Which open window "Fill in app" should fill `item` into, or why none.
///
/// **The whole decision, as a pure function over a window list.**
/// [`fill_item_into_app`] is untestable -- it enumerates the real desktop and
/// drives a real injector -- and a decision that only exists inside it is a
/// decision nothing can pin. That is not a hypothetical here: the check that
/// a dead binding must not be filled through existed, was correct, was
/// covered, and was consulted at exactly two sites that render TEXT.
///
/// **`find_window_for_process` is still the gate that protects the
/// credential**, and it refuses a host process itself; this function cannot
/// be routed around either, because it is the only place `fill_item_into_app`
/// gets an hwnd from. The dead arm is here so the refusal has a NAME to
/// report, and so `a_dead_binding_resolves_no_window_to_fill` can assert it
/// without a desktop.
fn fill_target<'a>(
    item: &VaultItem,
    windows: &'a [crate::window_list::WindowInfo],
) -> Result<&'a crate::window_list::WindowInfo, FillRefusal> {
    if detail::item_binding_is_dead(item) {
        return Err(FillRefusal::BindingIsDead);
    }
    let app_match =
        crate::vault_bridge::extract_app_match(item).ok_or(FillRefusal::NotBound)?;
    crate::app::find_window_for_process(windows, &app_match.process)
        .ok_or(FillRefusal::NotOpen)
}

/// What the log says about each refusal, off the item it is about.
///
/// Separate from [`fill_target`] so the decision can be asserted without the
/// wording and the wording without the decision -- and in one place, because
/// both of `fill_item_into_app`'s callers reach it and a second copy is a
/// second place for a sentence to go quietly missing.
fn fill_refusal_note(item: &VaultItem, why: FillRefusal) -> String {
    let name = &item.name;
    match why {
        FillRefusal::NotBound => {
            format!("\"Fill in app\" for {name}: no app is matched to this item yet")
        }
        FillRefusal::BindingIsDead => format!(
            "\"Fill in app\" for {name}: the app match on this item names the process that \
             owns the window for every Microsoft Store app, and recorded no window title to \
             tell those apps apart, so Deskwarden ignores it -- there is no one app to fill. \
             Use \u{201c}Add app\u{2026}\u{201d} in the tray menu to pick the app again, or \
             Remove on the item's MATCHED APP card to clear it"
        ),
        FillRefusal::NotOpen => {
            let process = crate::vault_bridge::extract_app_match(item)
                .map(|m| m.process)
                .unwrap_or_default();
            format!("\"Fill in app\" for {name}: {process} isn't currently open")
        }
    }
}

fn fill_item_into_app<A: UiAutomationFiller + Clone + 'static, B: SendInputFiller + Clone + 'static>(
    item: &VaultItem,
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &FillStats,
) {
    // Enumerated once, whatever the outcome: `fill_target` owns the decision
    // and this function owns the two effects it can have.
    let windows = crate::window_list::list_windows(std::process::id());
    match fill_target(item, &windows) {
        // fill_from_vault does its own credential lookup (from the cache, not
        // `bw serve` -- see its doc comment) and the fill in one call --
        // nothing else here needs to touch `injector` directly.
        Ok(target) => {
            crate::app::fill_from_vault(cache, injector, fill_stats, &item.id, target.hwnd)
        }
        Err(why) => log::info!("{}", fill_refusal_note(item, why)),
    }
}

/// Move `item` to the trash and drop it out of this window's copy of the
/// vault, returning the inline-band sentence to show (`None` on success).
///
/// Shared by the detail pane's Delete button and an item row's context-menu
/// entry, both of which reach it only through `confirm_click`'s two-click
/// confirmation -- this function does no confirming of its own and must not
/// be called without it.
///
/// **The two things this body does beyond the write are here, and not at
/// either call site, because there are two call sites.** `delete_item` is a
/// SOFT delete (no `permanent=true` -- see `VaultCache::purge_item` for the
/// other one), so it is the fifth command that moves an item between this
/// window's three lists, and it owes the same two things the other four owe:
///
///  * `trash_list.invalidate()`, because the item is now IN the trash. The
///    on-demand list is not cached and is never pruned in place, so a Trash
///    row already fetched keeps listing -- and the badge keeps counting --
///    the vault as it was before the delete, for the life of the window:
///    `AuxList::wants_fetch` sees a list already in hand and never asks
///    again.
///  * a user-visible failure. This used to be `log::warn!` plus the re-auth
///    flag and nothing else, which is the same "looked like a success that
///    hadn't refreshed yet" the other four were fixed out of: the row simply
///    stayed where it was.
///
/// Written in one place rather than twice, so a fix cannot cover the row menu
/// and miss the kebab -- which is precisely the shape the other four's
/// per-arm guard would have let through.
#[must_use = "a refused delete has to reach the inline band -- assign it to \
              `move_error`, or the failure is silent again"]
fn delete_vault_item(
    ctx: &egui::Context,
    cache: &VaultCache,
    needs_reauth: &Rc<RefCell<bool>>,
    items: &mut Vec<VaultItem>,
    selected_id: &mut Option<String>,
    trash_list: &mut AuxList,
    item: &VaultItem,
) -> Option<String> {
    match cache.delete_item(&item.id) {
        Ok(()) => {
            items.retain(|i| i.id != item.id);
            // Select the first remaining item, or `None` if the vault is now
            // empty -- either way the selection-change reset block clears
            // `mode`/`reveal`/`totp_state` on the next frame.
            *selected_id = items.first().map(|i| i.id.clone());
            trash_list.invalidate();
            None
        }
        Err(e) => {
            log::warn!("failed to delete item {} ({}): {e:?}", item.id, item.name);
            flag_reauth_if_unauthorized(ctx, needs_reauth, &e);
            Some(list_command_failure_message(ListCommand::Delete, &item.name, &e))
        }
    }
}

/// Moves `item_id` into `folder_id`, **reverting the window's own list if the
/// write fails**, and returns the inline message to show (`None` on success).
///
/// The optimistic write and the revert are not theatre, and they are not an
/// animation: `VaultCache::move_item_to_folder` is synchronous, so the whole
/// sequence happens inside one frame and nothing half-moved is ever painted.
/// What they buy is that the row's folder is written in exactly one place and
/// unwritten in exactly one place, so "a failed move leaves the item where it
/// was" is a property of this function rather than of a `match` arm that
/// happens not to have touched anything yet. That was the user's explicit
/// choice over leaving the row looking moved.
///
/// Through `VaultCache`, never `cache.bridge()`: the cache's snapshot is what
/// the rest of the app reads, and its replay log is what stops an in-flight
/// populate from filing the item back where it was.
fn move_item_into_folder(
    ctx: &egui::Context,
    cache: &VaultCache,
    needs_reauth: &Rc<RefCell<bool>>,
    items: &mut [VaultItem],
    item_id: &str,
    folder_id: &str,
) -> Option<String> {
    let Some(at) = items.iter().position(|i| i.id == item_id) else {
        // The vault reloaded out from under the gesture. Nothing to move and
        // nothing to say -- the row the user dragged is not on screen either.
        log::warn!("dropped item {item_id} onto folder {folder_id}, but it is no longer listed");
        return None;
    };
    let before = items[at].clone();
    // Optimistic, and locally rebuilt ON PURPOSE: this paints the row in its
    // new folder for the duration of the call. It is REPLACED by the server's
    // copy on success below -- keeping this value would leave the row holding
    // a `revisionDate` the write has superseded, and the next write of it
    // would be refused (see `vault_bridge`'s `REVISION_DATE_KEY`).
    items[at] = crate::vault_bridge::with_folder(&before, Some(folder_id));
    match cache.move_item_to_folder(&before, Some(folder_id)) {
        Ok(moved) => {
            items[at] = moved;
            None
        }
        Err(e) => {
            items[at] = before;
            log::warn!("failed to move item {item_id} into folder {folder_id}: {e:?}");
            flag_reauth_if_unauthorized(ctx, needs_reauth, &e);
            Some(move_failure_message(&items[at].name, &e))
        }
    }
}

/// The inline message a refused write shows, per failure.
///
/// Exhaustively matched with no catch-all: a new `VaultError` variant must be
/// given its own wording rather than silently inheriting someone else's.
///
/// The error's own payload -- a `ureq` transport string, a serde message -- is
/// deliberately NOT interpolated. It is a developer's sentence in the middle
/// of a user's, and the log line at the call site already carries the whole
/// thing for anyone who needs it. What the band has to say is what happened
/// and what state the item is in, and both halves are here.
fn move_failure_message(name: &str, e: &VaultError) -> String {
    let because = match e {
        VaultError::Unauthorized => "the vault backend no longer accepts this session",
        VaultError::Http(_) => "the vault backend refused the write",
        VaultError::Parse(_) => "the vault backend's answer couldn't be read",
    };
    format!("Couldn't move \"{name}\" -- {because}. It's still in its old folder.")
}

/// One of the **five** commands that move an item between this window's three
/// lists.
///
/// Its own type rather than `item_list::RowCommand`, which also covers the
/// copies, the fill and the edit -- none of which this message shape fits, and
/// three of which cannot fail this way at all. A closed enum is what makes
/// [`list_command_failure_message`] exhaustive, so a sixth command added later
/// is a compile error here instead of a silent fall-through to somebody else's
/// wording.
///
/// **It was four until a reviewer counted.** [`Self::Delete`] is a SOFT
/// delete -- `VaultCache::delete_item` sends no `permanent=true`, which is
/// what `purge_item` is for -- so it takes an item out of the live vault and
/// puts it into the Trash, which is the same kind of move the other four make
/// and wants the same invalidation and the same message. It was left out of
/// both for exactly as long as the comment above the archive arm said "the
/// four commands".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListCommand {
    Archive,
    Unarchive,
    Restore,
    Purge,
    Delete,
}

/// What a failed row command shows in the inline band.
///
/// **These four used to be `log::warn!` and nothing else.** The re-auth flag
/// was set and the failure was otherwise silent -- on the same screen whose
/// branch already routes aux-FETCH failures through the band -- so a rejected
/// write looked exactly like a successful one that had not refreshed yet, and
/// the user's only signal was the item still sitting where they had just told
/// it to leave.
///
/// Archive is the one where rejection is genuinely likely rather than
/// theoretical: re-archiving an already-archived item returns 400, which is
/// what `vault_cache`'s own
/// `a_rejected_archive_is_an_error_not_a_silent_success` exists for.
///
/// This reuses the existing band (`NoticeSource::Move`) rather than inventing
/// a fourth source. Every one of these is a write that was supposed to move an
/// item between lists and did not, which is the same thing a refused
/// drag-to-folder is; they want the same precedence and the same dismissal --
/// a plain clear, NOT the Aux band's dismissal-is-also-the-retry. A fourth
/// `NoticeSource` would need a fourth precedence rule with nothing to derive
/// it from.
///
/// Every sentence names the state the item is actually in, for
/// [`generate_failure`]'s reason: the question a user has after clicking
/// Restore and seeing the row unchanged is "did it half-work?", and the answer
/// is always no.
fn list_command_failure_message(command: ListCommand, name: &str, e: &VaultError) -> String {
    // The same three, worded the same way as `move_failure_message`'s: one
    // vocabulary for "the backend said no" across this window, not two.
    let because = match e {
        VaultError::Unauthorized => "the vault backend no longer accepts this session",
        VaultError::Http(_) => "the vault backend refused the write",
        VaultError::Parse(_) => "the vault backend's answer couldn't be read",
    };
    let (verb, unchanged) = match command {
        ListCommand::Archive => ("archive", "It's still in your vault."),
        ListCommand::Unarchive => ("unarchive", "It's still in the archive."),
        ListCommand::Restore => ("restore", "It's still in the trash."),
        // "permanently delete", not "delete": this is the one irreversible
        // command in the window, and a message reading "Couldn't delete"
        // would leave a user unsure which of the two deletes was refused.
        // The two really are both reachable -- `ListCommand::Delete` below is
        // the soft one -- so the distinction is not hypothetical.
        ListCommand::Purge => ("permanently delete", "It's still in the trash."),
        // The soft delete: a refused one leaves the item exactly where it
        // was, in the live vault, NOT in the trash it was headed for.
        ListCommand::Delete => ("delete", "It's still in your vault."),
    };
    format!("Couldn't {verb} \"{name}\" -- {because}. {unchanged}")
}

/// The three writes in the detail pane that a refusal used to leave silent.
///
/// **This is the case the user actually reported.** "Tried to Fav one item --
/// it shows as faved in folder but not in original client even after syncing":
/// a `PUT` refused with the 400 in `vault_bridge`'s `REVISION_DATE_KEY`, and a
/// star that looked flipped because the row was painted from the local copy.
/// `fba91ff` removed the cause the implementer found -- a stale revision token
/// this app kept -- but a 400 has other causes that are all still reachable
/// (the official client edited the item, a concurrent write, a restore whose
/// token this app had not read back), and every one of them reproduces that
/// report exactly. The arms did `log::warn!` and nothing else, so the user was
/// told nothing on any of them.
///
/// [`ListCommand`]'s own doc is the precedent and the argument is the same one
/// `03c36ea` made for Generate: a write whose failure leaves **no** trace on
/// screen has to make its own. A favourite is that shape -- the star comes
/// from the local copy, so a refused toggle and an accepted one look
/// identical -- and so is a Save, which leaves the form exactly as it was
/// whether the write landed or not.
///
/// **These go to the same band ([`NoticeSource::Move`]) rather than to a
/// fourth source**, for the reason [`ListCommand`] does not have one either:
/// they want the same precedence and the same plain-clear dismissal, and a
/// fourth `NoticeSource` would need a fourth precedence rule with nothing to
/// derive it from.
///
/// **On [`VaultError::Unauthorized`] the band is not what the user sees**, and
/// that is not a gap. `flag_reauth_if_unauthorized` closes the window on that
/// one variant, so the sentence is set and the window goes; the re-auth the
/// flag triggers is the trace, and it is a louder one. The 400 -- the variant
/// the report was actually about -- leaves the window open and the band up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemWrite {
    /// Two variants rather than one carrying a `bool`, so the wording cannot
    /// be got backwards at the call site: "It's still not a favourite" after a
    /// failed UN-favourite would be a false statement about the user's vault.
    Favorite,
    Unfavorite,
    Save,
    Create,
    /// The MATCHED APP card's trigger pills.
    AppTrigger,
    /// The MATCHED APP card's Remove. Two variants rather than one, for the
    /// favourite pair's reason: what the user is told the item still is
    /// differs, and getting it backwards is a false statement about their
    /// vault.
    AppUnmatch,
}

/// What a refused detail-pane write shows in the inline band.
///
/// Same three `because` clauses, worded the same way as
/// [`list_command_failure_message`]'s and [`move_failure_message`]'s: one
/// vocabulary for "the backend said no" across this window, not three. And
/// like both of those, every sentence ends by naming the state the item or the
/// form is actually in, because the question a user has after clicking and
/// seeing nothing move is "did it half-work?" -- and the answer is always no.
fn item_write_failure_message(write: ItemWrite, name: &str, e: &VaultError) -> String {
    let because = match e {
        VaultError::Unauthorized => "the vault backend no longer accepts this session",
        VaultError::Http(_) => "the vault backend refused the write",
        VaultError::Parse(_) => "the vault backend's answer couldn't be read",
    };
    // Exhaustive with no catch-all, for [`move_failure_message`]'s reason: a
    // fifth write must be given its own wording rather than silently
    // inheriting a neighbour's.
    match write {
        // The star in the detail pane is painted from this window's own copy
        // of the item, which a refused write does not change -- so "it is not
        // a favourite" is the whole of what the user cannot otherwise see.
        ItemWrite::Favorite => {
            format!("Couldn't add \"{name}\" to your favourites -- {because}. It still isn't one.")
        }
        ItemWrite::Unfavorite => {
            format!(
                "Couldn't remove \"{name}\" from your favourites -- {because}. It still is one."
            )
        }
        // The editor stays open on failure (`mode` is left on `Edit`), so the
        // user's typing is not lost and the sentence says so -- otherwise the
        // safe reaction to "couldn't save" is to assume it is gone.
        ItemWrite::Save => format!(
            "Couldn't save your changes to \"{name}\" -- {because}. Nothing has been written, and \
             your edits are still in the form."
        ),
        // `name` is the draft's name, not a vault item's: there is no vault
        // item yet, which is exactly what this says.
        ItemWrite::Create => format!(
            "Couldn't create \"{name}\" -- {because}. Nothing has been added to your vault, and \
             what you typed is still in the form."
        ),
        // The card repaints from this window's own copy of the item, which a
        // refused write does not change -- so the pills snap back to where
        // they were, and this says so rather than leaving the user to wonder
        // whether the one they clicked took.
        ItemWrite::AppTrigger => format!(
            "Couldn't change when \"{name}\" fills -- {because}. It still fills the way it did."
        ),
        ItemWrite::AppUnmatch => format!(
            "Couldn't unmatch \"{name}\" from its app -- {because}. It is still matched to it."
        ),
    }
}

/// What a failed **Generate** does: the sentence the inline band shows, and
/// whether the session it failed against should be re-authenticated.
///
/// A decision rather than a `match` arm because the arm it replaces is inside
/// `run`'s update closure, which no test in this crate can call -- a reviewer
/// proved the three action arms wired by `6c40075` are invisible to the suite
/// by replacing all three bodies with `{}` and watching it stay green. The
/// wording and the re-auth policy are the whole of what this fix decides, so
/// they live where `generate_failure_tests` can reach them; the arm keeps
/// only the plumbing, pinned by `generate_failure_wiring_tests`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GenerateFailure {
    /// The band's sentence. Never empty -- see [`generate_failure`].
    message: String,
    /// True only for [`VaultError::Unauthorized`]: the session really is gone
    /// and the window records it, so `open_vault_window` runs the same
    /// recovery a Lock does **when this window closes**.
    ///
    /// Deliberately NOT `flag_reauth_if_unauthorized`, which closes the window
    /// immediately. That is right for Save -- a terminal gesture whose whole
    /// point was to write -- and wrong here: Generate is a mid-composition
    /// convenience click that changes nothing on failure, so closing the
    /// window on it discards every field the user has typed into a half-filled
    /// new-item form in exchange for nothing. The flag alone keeps the
    /// recovery and drops the eviction: the draft survives, the user is told
    /// they need to sign in again, and the re-auth happens the moment they are
    /// done with the window either way.
    needs_reauth: bool,
}

/// The band sentence and re-auth policy for a Generate that failed.
///
/// Exhaustive with no catch-all, for the reason [`move_failure_message`] is: a
/// new `VaultError` variant must be given its own wording and its own policy
/// rather than silently inheriting someone else's. The error's own payload
/// stays in the log for the same reason too.
///
/// Every sentence says the box is unchanged. That is the actual question a
/// user has after clicking Generate and seeing the password box look exactly
/// as it did -- "did it half-work?" -- and the answer is always no.
fn generate_failure(e: &VaultError) -> GenerateFailure {
    let because = match e {
        VaultError::Unauthorized => "the vault backend no longer accepts this session",
        VaultError::Http(_) => "the vault backend refused the request",
        VaultError::Parse(_) => "the vault backend's answer couldn't be read",
    };
    let tail = match e {
        // The only variant that has something further to ask of the user: the
        // draft is intact but nothing will save until the session is renewed.
        VaultError::Unauthorized => " You'll need to sign in again before saving.",
        VaultError::Http(_) | VaultError::Parse(_) => "",
    };
    GenerateFailure {
        message: format!(
            "Couldn't generate a password -- {because}. Nothing in this form has changed.{tail}"
        ),
        needs_reauth: matches!(e, VaultError::Unauthorized),
    }
}

/// Which of the window's three unrelated failures the one inline band is
/// showing this frame.
///
/// Three, because the band under the item list's toolbar is the only inline
/// user-visible channel this window has, and it already carried two before
/// this fix (a refused drag-to-folder, and a Trash/Archive fetch that failed).
/// Naming the source is what lets the dismissal clear *that* one rather than
/// all of them -- dismissing an Archive failure is also its retry, and firing
/// that retry because someone waved away a generate message would be a
/// surprise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeSource {
    Generate,
    Aux,
    Move,
}

/// Which message the single inline band shows, given the three that might be
/// waiting, and where it came from.
///
/// **Generate outranks the other two**, and that is a deliberate reversal of
/// the order the aux band arrived with. The other two describe standing
/// conditions -- a list that is still empty, an item that is still where it
/// was -- and both are still true, and still shown, a frame after this one is
/// dismissed. A generate failure answers a click the user made this instant,
/// and if it loses the band it is not deferred but *lost*: the box is
/// unchanged and there is nothing else on screen that a click happened at all.
/// That is precisely the dead-button failure this fix exists to close, so it
/// must not be reintroduced by a Trash fetch that failed ten minutes ago.
fn inline_notice<'a>(
    generate: Option<&'a str>,
    aux: Option<&'a str>,
    moved: Option<&'a str>,
) -> Option<(NoticeSource, &'a str)> {
    generate
        .map(|m| (NoticeSource::Generate, m))
        .or_else(|| aux.map(|m| (NoticeSource::Aux, m)))
        .or_else(|| moved.map(|m| (NoticeSource::Move, m)))
}

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
/// ~10s `READ_DEADLINE` if a *different*, since-deselected item's poll
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
/// One of the vault window's two on-demand item lists -- the trash and the
/// archive.
///
/// Neither lives in `VaultCache`'s snapshot, and that is a recorded decision
/// with reasons (see `VaultCache::list_trash_unless_superseded`), so the
/// window holds them itself. Both are fetched off the UI thread the first
/// time their row is selected.
///
/// **`items: None` is "never fetched", which is NOT "empty".** That
/// distinction is the whole point of the `Option`: a badge that printed `0`
/// for a list nobody had asked for yet is exactly the untruth the Trash row
/// shipped with for months, and `sidebar::badge_text` draws an en dash for
/// `None` rather than a number this app does not have.
///
/// `error` exists so a failed fetch is reported ONCE rather than retried
/// every frame. Without it, `wants_fetch` would be true again on the very
/// next frame and a `bw serve` that is down would be hammered at the frame
/// rate for as long as the row stays selected.
/// **The list a given row actually reads**, or `None` when that list exists
/// but has not been fetched yet.
///
/// This is the whole of the Trash/Archive plumbing's central decision, and it
/// is a function rather than three copies of one `match` because it WAS three
/// copies. `run` has to answer this question at three unrelated points -- the
/// list the item pane draws, the item a selection resolves to, and the item a
/// right-click command acts on -- all inside a closure no test in this crate
/// can call. A reviewer replaced the Trash and Archive arms of the FIRST of
/// them with `&items` and the whole suite stayed green while both rows listed
/// nothing, which is precisely the defect the feature was written to fix,
/// restored and invisible.
///
/// One function cannot make those three sites agree by itself -- a caller can
/// still hand it the wrong arguments -- but it turns three untested decisions
/// into one directly tested one, and `out_of_vault_wiring_tests` counts the
/// call sites so a re-inlined `match` cannot creep back in beside it.
///
/// `None` is NOT `&[]`, for [`sidebar::badge_text`]'s reason: "not fetched
/// yet" and "fetched, and empty" are different facts, and the search
/// placeholder one control to the right of the badge was still printing `0`
/// for the first of them. [`list_for`] is the flattening, for the callers
/// that genuinely only want something to iterate.
fn list_unless_unfetched<'a>(
    source: sidebar::FilterSource,
    live: &'a [VaultItem],
    trash: Option<&'a [VaultItem]>,
    archive: Option<&'a [VaultItem]>,
) -> Option<&'a [VaultItem]> {
    match source {
        // The live snapshot is always "fetched": before the first load lands
        // it is legitimately empty, and the window has its own loading state
        // for that.
        sidebar::FilterSource::LiveVault => Some(live),
        sidebar::FilterSource::Trash => trash,
        sidebar::FilterSource::Archive => archive,
    }
}

/// [`list_unless_unfetched`] for the callers that only need something to
/// search. An unfetched list reads as empty for the one or two frames before
/// its fetch lands -- the sidebar badge says "not known yet" throughout,
/// which is the honest half of that pair.
fn list_for<'a>(
    source: sidebar::FilterSource,
    live: &'a [VaultItem],
    trash: Option<&'a [VaultItem]>,
    archive: Option<&'a [VaultItem]>,
) -> &'a [VaultItem] {
    list_unless_unfetched(source, live, trash, archive).unwrap_or(&[])
}

#[derive(Default)]
struct AuxList {
    items: Option<Vec<VaultItem>>,
    in_flight: bool,
    error: Option<String>,
}

impl AuxList {
    /// Whether the window should start a fetch for this list this frame.
    ///
    /// A pure predicate rather than three conditions spelled out inside the
    /// render closure: the whole decision is four booleans, and inside that
    /// closure no test in this crate could reach it. Its failure modes are
    /// all "reachable but wrong" rather than "does not compile" -- a missing
    /// `in_flight` check spawns a thread per frame, a missing `error` check
    /// retries a dead backend at 60Hz, and a missing `items` check refetches
    /// a list it already has on every frame the row is open.
    fn wants_fetch(&self, selected: bool) -> bool {
        selected && self.items.is_none() && !self.in_flight && self.error.is_none()
    }

    /// Forget everything: the list, any failure, and the right to be
    /// considered fetched. Called whenever the live vault is reloaded or
    /// written to, so the next visit to the row asks the server again.
    ///
    /// `in_flight` is deliberately NOT cleared -- a thread is still running
    /// and clearing it would let a second one start. The result of the
    /// in-flight fetch is dropped by the generation check instead.
    fn invalidate(&mut self) {
        self.items = None;
        self.error = None;
    }
}

/// Fetches one of the two on-demand lists on a background thread.
///
/// Backgrounded for the reason every other backend call this window makes
/// off-thread is: a real HTTP round-trip to `bw serve`, which a stalled
/// backend can hold for the whole of the bridge's `READ_DEADLINE` -- 10s of
/// frozen window, bounded but not short, and this pays it on a click. This
/// window
/// already runs nine calls synchronously in the render closure -- a known,
/// recorded debt -- and this deliberately does not become the tenth.
///
/// The result is tagged with the `load_generation` it was started against,
/// so a vault reload that lands first can drop it rather than have a list
/// fetched against the old vault overwrite one fetched against the new.
///
/// **`era` is the second, independent question, and the generation cannot
/// answer it.** `load_generation` is a SPAWN TAG -- `run` increments it when
/// it spawns a vault load, and `VaultCache::clear` does not touch it -- so it
/// answers "has this window asked for a reload since?" and nothing else. A
/// `clear` and a re-populate under a different account leave it untouched, so
/// a fetch outstanding across one comes back matching and is applied: account
/// B's trashed item names under account A's chrome. That is the hole
/// `window_era` was introduced (review 29) to close for the live load, and
/// this path is why it was still open beside it -- `window_era`'s own doc
/// explicitly refuses to rest on "every production `cache.clear()` runs on
/// the main thread", which is precisely what an unguarded fetch here
/// reinstates.
///
/// No reachable path produces it today: both the lock and the re-auth that
/// would `clear` the cache also close the window. **Defence in depth**, and
/// six lines of it -- see `VaultCache::list_trash_unless_superseded`, which
/// performs the check where the fetch happens, and returns a superseded
/// result as `Ok(None)`.
fn spawn_aux_load(
    cache: std::sync::Arc<VaultCache>,
    which: OutOfVault,
    generation: u64,
    era: crate::vault_cache::VaultEra,
    tx: mpsc::Sender<(u64, OutOfVault, Result<Option<Vec<VaultItem>>, AuxLoadError>)>,
) {
    std::thread::spawn(move || {
        let result = match which {
            OutOfVault::Trash => cache.list_trash_unless_superseded(era),
            OutOfVault::Archive => cache.list_archive_unless_superseded(era),
        };
        let _ = tx.send((generation, which, result.map_err(AuxLoadError::of)));
    });
}

/// Why an on-demand list could not be fetched.
///
/// `Unauthorized` keeps its own variant all the way to the UI thread rather
/// than being flattened into the message, because the window's
/// re-authentication path keys off it -- and re-detecting it by matching on a
/// `{:?}`-formatted string is exactly the re-parsing this crate keeps having
/// to un-write. `VaultError` itself is not sent because it is not `Send`-
/// friendly to keep around and only these two facts are wanted here: is it a
/// dead session, and what does the user get told.
enum AuxLoadError {
    Unauthorized,
    Other(String),
}

impl AuxLoadError {
    fn of(e: VaultError) -> Self {
        match e {
            VaultError::Unauthorized => AuxLoadError::Unauthorized,
            e => AuxLoadError::Other(format!("{e:?}")),
        }
    }
}

fn draw_read_arm(
    ui: &mut egui::Ui,
    item: &VaultItem,
    // The item's folder, already resolved to a name by `sidebar::folder_name`
    // out of the folder list this window is already holding -- the pane paints
    // it in the header subtitle and never sees an id.
    folder: Option<&str>,
    fill_count: u32,
    totp_state: &TotpState,
    delete_pending: bool,
    reveal: &mut detail::RevealState,
    icon: Option<&egui::TextureHandle>,
    // The window's one `AppIdentityCache`, threaded through to the read pane
    // for the same reason the edit form gets it: the `MATCHED APP` card now
    // shows what the bound app is really CALLED, and that answer comes off a
    // worker thread and is cached per path for the life of the window.
    apps: &mut crate::app_identity::AppIdentityCache,
) -> DetailAction {
    let mut action = draw_detail_read(
        ui,
        item,
        folder,
        fill_count,
        totp_state,
        delete_pending,
        reveal,
        icon,
        apps,
    );
    // Ctrl+Shift+F (spec section 5) is the keyboard equivalent of clicking
    // "Fill in app" -- checked here, not at the top level, because it needs
    // exactly the selected `item` and the button click above doesn't. Gated
    // on the item's kind by the same predicate the button is; see
    // `fill_hotkey_applies`.
    if fill_hotkey_applies(
        item,
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
/// (`detail::item_offers_fill`), not on a second copy of the rule: the
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
/// Takes the ITEM, not just its kind, because the button's predicate does:
/// `detail::item_offers_fill` is the kind AND the item's binding not being
/// dead, and a chord that stayed on the kind alone would be the door left
/// open beside a hidden button -- the exact failure this doc's own second
/// paragraph describes, one field further down.
fn fill_hotkey_applies(item: &VaultItem, pressed: bool) -> bool {
    pressed && detail::item_offers_fill(item, crate::vault_bridge::ItemKind::of(item))
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
/// What the titlebar avatar shows for `email`, including when there is no
/// email yet.
///
/// `None` is EMPTY, not [`theme::initials`]'s `"?"`. The two cases this
/// function separates are "the account details have not arrived" and "they
/// arrived and named nobody", and only the first is reachable now that the
/// window opens without waiting for `bw status` -- but `initials` answers
/// both with a glyph that reads as a value the app is asserting about the
/// account. An empty circle asserts nothing, holds the toolbar's layout
/// still, and is replaced by the real initials in one step rather than two.
fn avatar_initials(email: Option<&str>) -> String {
    match email {
        Some(email) => theme::initials(email),
        None => String::new(),
    }
}

/// The titlebar avatar, and **a button**: the user asked for the account menu
/// to open on a click of the avatar itself, not only of the chevron beside it.
///
/// Returns its `Response` so [`account_menu`] can hand `egui::Popup::menu` the
/// union of this and the chevron — one popup with two hit areas, rather than
/// two popups that could both be open.
fn draw_circle_avatar(ui: &mut egui::Ui, text: &str) -> egui::Response {
    const SIZE: f32 = 28.0;
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(SIZE), egui::Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().circle_filled(rect.center(), SIZE / 2.0, theme::INK);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::new(11.0, egui::FontFamily::Name(theme::BOLD.into())),
        egui::Color32::WHITE,
    );
    response
}

/// What the account menu says when there is nothing to switch to and
/// nothing is stopping one. One account is the overwhelmingly common state, and
/// an empty menu is indistinguishable from a menu that failed to build.
const NO_OTHER_ACCOUNTS: &str = "No other accounts yet";

/// The menu's Lock row, and the chord it also answers to.
///
/// **The titlebar's "Lock CTRL+L" pill is gone** — design 2b has one, and this
/// is the user's direction over it ("we can also remove Lock button in that
/// case if it lives in the dropdown menu"). `CTRL+L` itself is untouched: it is
/// handled by `run`'s own key check, not by the pill, so removing the pill
/// removes an affordance and not a shortcut. The hint moves onto this row so
/// the chord stays discoverable, which is the rule every copy chord in this app
/// already follows.
const LOCK_LABEL: &str = "Lock";
const LOCK_SHORTCUT: &str = "CTRL+L";

/// The menu's two account-management rows. Spelled with `...` rather than `…`,
/// matching `tray::ADD_ACCOUNT` and `tray::remove_account_label` — both menus
/// say the same thing about the same two actions.
const ADD_ACCOUNT: &str = "Add account...";
const REMOVE_ACCOUNT: &str = "Remove this account...";

/// How wide the account menu is allowed to get.
///
/// A blocked state paints [`AccountsState::blocked_reason`] in here, and those
/// sentences name a directory path — unwrapped, one of them is a menu wider
/// than the window it hangs off. A self-hosted server URL is long enough to
/// want it too.
const SWITCHER_MENU_WIDTH: f32 = 300.0;

/// What a click in the titlebar's account menu asked for.
///
/// One enum rather than four flags out of [`account_menu`], because exactly one
/// of these can happen per click and three of them close the window.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AccountAction {
    /// Switch to another configured account.
    Switch(crate::accounts::AccountId),
    /// Lock this account's vault. **Not "log out"** — this app does not use
    /// that word, because it has two different meanings here and the menu
    /// offers both separately: Lock clears the session and a master password
    /// brings it back, [`Remove`](Self::Remove) deletes the profile.
    ///
    /// It lands on the sign-in state **for the account that was locked**, and
    /// deliberately does not switch anywhere: a security action must not leave
    /// the user somewhere unlocked they did not ask to be. That is not a
    /// decision made here — it is what the caller's existing `locked` recovery
    /// already does, which is why this reuses that flag rather than adding a
    /// path of its own.
    Lock,
    /// Sign in to a further account and settle onto it.
    Add,
    /// Delete this account's local profile and drop it from the list.
    Remove,
}

/// What the titlebar's account menu shows, decided apart from the drawing.
///
/// A plan, the way `tray::AccountsMenuPlan` is one, and for the same reason:
/// the drawing needs a live `egui::Context` and the *decisions* are what go
/// wrong. The two menus are deliberately not one type — they have different
/// rows and different wording — but every decision either of them makes is
/// asked of [`AccountsState`](crate::accounts::AccountsState) and of nothing
/// else.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AccountMenuPlan {
    /// The identity block, top to bottom, strongest line first: **the address,
    /// then the host**. Two lines whenever the host is known at all, which is
    /// every session past the first second of one.
    ///
    /// **There is no display name and none is invented.** `bw status` — the
    /// only thing that knows anything about the signed-in identity — reports a
    /// status, a `userEmail` and a `serverUrl` and nothing else; Bitwarden's
    /// own name field is a vault-level profile value this app never fetches. So
    /// the address is the primary line rather than a subtitle under a name, and
    /// nothing here derives one from the local part of the address: "Ana Novak"
    /// guessed from `ana.novak@` is an assertion about a person, and the one it
    /// would get wrong is the one that matters.
    ///
    /// The first line is [`account_label`](crate::accounts::account_label) when
    /// nothing better has arrived, which always has something to say. The
    /// second is [`account_host_line`] — see it for the three cases a user can
    /// be in and the one case that has no line at all.
    identity: Vec<String>,
    /// One row per account the user may switch **to**, in
    /// [`switchable`](crate::accounts::AccountsState::switchable) order. Never
    /// [`all`](crate::accounts::AccountsState::all), which still reports the
    /// active account and still reports duplicate ids — a menu built from that
    /// offers a row that switches to where the user already is, and two rows
    /// onto one directory.
    switch_to: Vec<(crate::accounts::AccountId, String)>,
    /// Why there is nothing to switch to, when there is nothing to switch to.
    /// `None` whenever [`switch_to`](Self::switch_to) is non-empty.
    notice: Option<String>,
    /// Whether "Add account..." is offered, per
    /// [`can_add`](crate::accounts::AccountsState::can_add).
    can_add: bool,
    /// Whether "Remove this account..." is offered, per
    /// [`can_remove_active`](crate::accounts::AccountsState::can_remove_active)
    /// — which is where both of `remove_account`'s refusals are answered. A row
    /// that can only fail is worse than one that is not there.
    can_remove: bool,
}

/// The account menu's second identity line: **which of the three servers a user
/// can be on this account lives on**, or `None` when nothing has said yet.
///
/// > "it's either whatever self-host URL or bitwarden.com or bitwarden.eu"
///
/// Which region an official-cloud account is in is real information, not noise:
/// with a self-hosted server beside them, this line is what tells three
/// accounts apart at a glance. So the four answers are:
///
/// * **A self-hosted URL** → its bare host, through
///   [`login_ui::server_host`](crate::login_ui::server_host) — the login
///   window's footer already answers "which server?" for this user, and a
///   second normalisation here would be a second answer. It strips the scheme
///   and the path, so `https://nw37.example.net/api` reads as
///   `nw37.example.net`.
/// * **`bitwarden.com` or any subdomain of it** → `bitwarden.com`.
/// * **`bitwarden.eu` or any subdomain of it** → `bitwarden.eu`.
///
///   Both through [`favicon::bitwarden_cloud`](crate::favicon::bitwarden_cloud),
///   which is the crate's one host test and matches by exact host or
///   host-suffix rather than by substring: `vault.bitwarden.community` is a
///   self-hosted domain that *contains* `bitwarden.com`, and a naive check
///   tells that user their vault is on Bitwarden's cloud. The REGION is
///   painted rather than the host that was matched, because `vault.bitwarden.eu`
///   and `bitwarden.eu` are one answer to the question this line asks.
/// * **The CLI answered and named no server** → `bitwarden.com`. That is the
///   CLI's own default rather than a guess, and it is the identical reading
///   `login_ui::server_host(None)` and `favicon::icon_base_url(None)` already
///   make.
///
/// And the one case with **no line at all**: nothing has answered yet and
/// nothing is stored — an account `resolve_startup` minted on a first install
/// has `server_url: None` for the same reason it has no email, and `bw status`
/// has not come back. Printing `bitwarden.com` there states something *false*
/// about a self-hosted account that simply has not been asked about, which is
/// the mistake the empty avatar and the waiting favicons both already avoid.
/// `server_known` is that distinction, and it is the window's own
/// `icon_base_known` — the same fact, asked once.
fn account_host_line(
    server_url: Option<&str>,
    stored: Option<&str>,
    server_known: bool,
) -> Option<String> {
    let url = server_url
        .or(stored)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match url {
        Some(url) => Some(
            crate::favicon::bitwarden_cloud(&crate::favicon::host_from_url(url))
                .map(str::to_string)
                .unwrap_or_else(|| crate::login_ui::server_host(Some(url))),
        ),
        None if server_known => Some(crate::login_ui::server_host(None)),
        None => None,
    }
}

/// Decides the account menu's contents. See [`AccountMenuPlan`].
///
/// `email` and `server_url` are what `bw status` said about the profile this
/// window is pointed at — the same two values the avatar's initials and the
/// favicon host come from, and the freshest thing the app has. They fall back
/// to what is stored against the account, which is what a window whose
/// `AccountDetails` have not arrived yet is left with.
fn account_menu_plan(
    state: &crate::accounts::AccountsState,
    email: Option<&str>,
    server_url: Option<&str>,
    server_known: bool,
) -> AccountMenuPlan {
    let active = state.active();
    // `account_label` rather than the email directly: an account minted on a
    // first install has none until a sign-in fills it in, and a blank first
    // line would be a strip of menu with nothing on it. Preferring the live
    // answer over the stored one is what makes the very first session -- the
    // one before anything has been persisted -- show an address rather than the
    // directory name.
    let primary = email
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::accounts::account_label(active).to_string());
    let mut identity = vec![primary];
    identity.extend(account_host_line(
        server_url.filter(|s| !s.is_empty()),
        active.server_url.as_deref(),
        server_known,
    ));

    let switch_to: Vec<(crate::accounts::AccountId, String)> = state
        .switchable()
        .iter()
        .map(|a| (a.id.clone(), crate::accounts::account_label(a).to_string()))
        .collect();
    // The blocked reason outranks "no other accounts yet", and it has to: a
    // blocked state may well hold several accounts, and telling the user they
    // have one would be false as well as unactionable. It is also the only
    // thing this window does with `blocked_reason` -- silently offering no rows
    // would read as "you have one account", and this refusal is one the user
    // can go and act on.
    let notice = switch_to.is_empty().then(|| {
        state
            .blocked_reason()
            .map(str::to_string)
            .unwrap_or_else(|| NO_OTHER_ACCOUNTS.to_string())
    });

    AccountMenuPlan {
        identity,
        switch_to,
        notice,
        can_add: state.can_add(),
        can_remove: state.can_remove_active(),
    }
}

/// The titlebar's account control: the avatar circle, the chevron beside it,
/// and the menu the two of them open together.
///
/// **Both marks open one menu.** The user asked for it on the avatar; the
/// chevron is the conventional disclosure mark and was already there. Hanging
/// two `egui::Popup`s off them would be two menus that can both be open, so the
/// popup is hung off `chevron | avatar` — egui's `Response::union`, whose
/// `clicked()` is either one's and whose rect covers both, so the menu also
/// drops under the pair rather than under half of it.
///
/// **The avatar is drawn unconditionally and the chevron is not.** `None`
/// accounts — `StartupAccounts::NoAccountList`, where this app has no `Account`
/// at all — is an app with nothing to say about accounts, so there is no menu
/// and no mark that opens one. The circle stays: it is the account's place in
/// the toolbar, and everything left of it shifts sideways if it comes and goes.
fn account_menu(
    ui: &mut egui::Ui,
    accounts: Option<&crate::accounts::AccountsState>,
    initials: &str,
    email: Option<&str>,
    server_url: Option<&str>,
    server_known: bool,
) -> Option<AccountAction> {
    // The chevron FIRST and the avatar second, because this strip packs
    // right-to-left: earlier is further right. The chevron belongs on the
    // avatar's right-hand side, which is where a disclosure mark goes -- and
    // `the_switcher_paints_between_the_avatar_and_the_gear` measures the
    // painted rects rather than trusting this ordering, which is inverted and
    // is exactly the kind of thing reasoning gets backwards.
    let chevron = accounts.map(|_| theme::account_switcher_button(ui));
    let avatar = draw_circle_avatar(ui, initials);
    let (state, chevron) = (accounts?, chevron?);
    let plan = account_menu_plan(state, email, server_url, server_known);

    let mut action = None;
    egui::Popup::menu(&(chevron | avatar)).show(|ui| {
        ui.set_max_width(SWITCHER_MENU_WIDTH);
        // Whose vault am I looking at? -- the menu's first job, and the one the
        // avatar's two initials can only hint at. Labels, not buttons: there is
        // nowhere to go from the account you are already on.
        for (row, line) in plan.identity.iter().enumerate() {
            // The address reads as the heading it is; everything under it (the
            // server) is a subtitle, exactly the weight relationship the detail
            // pane's own header/subtitle pair uses.
            let text = egui::RichText::new(line).size(12.0);
            let text = if row == 0 {
                text.color(theme::INK)
                    .family(egui::FontFamily::Name(theme::SEMIBOLD.into()))
            } else {
                text.color(theme::TEXT_FAINT)
            };
            ui.add(egui::Label::new(text).wrap());
        }

        ui.separator();
        for (id, label) in &plan.switch_to {
            if ui.button(label).clicked() {
                action = Some(AccountAction::Switch(id.clone()));
                ui.close();
            }
        }
        if let Some(notice) = &plan.notice {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(notice)
                        .size(12.0)
                        .color(theme::TEXT_SECONDARY),
                )
                .wrap(),
            );
        }

        ui.separator();
        // The chord rides the row, the way design 2b's pill carried it.
        if ui
            .add(egui::Button::new(LOCK_LABEL).shortcut_text(LOCK_SHORTCUT))
            .clicked()
        {
            action = Some(AccountAction::Lock);
            ui.close();
        }
        if plan.can_add && ui.button(ADD_ACCOUNT).clicked() {
            action = Some(AccountAction::Add);
            ui.close();
        }
        if plan.can_remove && ui.button(REMOVE_ACCOUNT).clicked() {
            action = Some(AccountAction::Remove);
            ui.close();
        }
    });
    action
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
/// Starts the program a [`LaunchPlan`] names.
///
/// **It re-checks nothing, and that is deliberate.** The plan is the
/// permission: `detail::app_launch_plan` is the only thing that builds one, it
/// is the only caller of `AppMatch::launchable_path`, and its `program` field
/// is that function's return value rather than the vault's `path` string. A
/// second check here would be a second copy of a rule (and the copy that
/// drifts); a *fallback* here -- "launch it anyway if the check failed" --
/// would be the whole gate undone, and there is no branch below that could
/// become one, because this function is never handed anything but a plan.
///
/// **In particular there is no `GetDriveType` here**, which
/// `AppMatch::launchable_path` once said "belongs to the launcher" while this
/// doc said it re-checks nothing -- two docs pointing at each other with
/// nobody implementing the check. The gap is accepted rather than delegated;
/// `launchable_path`'s doc now states the decision and the three reasons for
/// it (a blocking SMB timeout on the UI thread, refusing legitimate installs
/// on mapped drives, and a residual bounded by the file-name check). This
/// sentence exists so the two docs agree.
///
/// Four Windows details, each of which was a bug waiting:
///
///  * **`raw_arg`, not `args`.** The tail is a command line, already in the
///    form the target program will read; `args` would re-quote it. See
///    [`LaunchPlan`], which argues this at length. The call is skipped
///    entirely for an empty tail, so a bare launch has a bare command line
///    rather than one with a trailing space.
///  * **`CREATE_BREAKAWAY_FROM_JOB`.** `job_object::KillOnCloseJob` is created
///    with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and everything in that job
///    dies when Deskwarden's handle to it closes -- i.e. when Deskwarden
///    exits, for any reason. Deskwarden assigns only `bw serve` to that job
///    and never assigns *itself*, so a child spawned here does not inherit it
///    and a plain spawn would already be safe. This flag is for the case that
///    is not under this crate's control: Deskwarden started from inside
///    somebody else's job (a debugger, a task-scheduler wrapper, an installer,
///    a terminal that uses one), where a child inherits job membership by
///    default and closing the user's browser when Deskwarden quits would be a
///    spectacular bug. `CreateProcess` fails with `ACCESS_DENIED` when the
///    containing job forbids breakaway, so the flag is not merely set and
///    hoped for -- see the retry below.
///  * **`CREATE_NO_WINDOW`**, so a console-subsystem program does not flash a
///    console. It is the same flag `bw_path` sets for the same reason; a GUI
///    program like a browser is unaffected by it.
///  * **Null standard handles.** Deskwarden is a GUI process, but inheriting
///    whatever it happens to hold would tie the child's lifetime to pipes it
///    knows nothing about. The `Child` itself is dropped immediately, which on
///    Windows closes Deskwarden's handle and does not touch the process.
///
/// The working directory is the program's own folder, not Deskwarden's: an app
/// started by hand is started from where it lives, and a program that loads a
/// DLL beside itself would otherwise search Deskwarden's directory first.
fn launch_app(plan: &LaunchPlan) -> std::io::Result<()> {
    launch_with(&plan.program, |breakaway| {
        let mut command = std::process::Command::new(&plan.program);
        if !plan.raw_tail.is_empty() {
            command.raw_arg(&plan.raw_tail);
        }
        if let Some(dir) = std::path::Path::new(&plan.program).parent() {
            command.current_dir(dir);
        }
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(creation_flags_for(breakaway))
            .spawn()
            .map(|_child| ())
    })
}

/// The creation flags for one spawn attempt, as a value rather than as a
/// spelling inside a closure.
///
/// **This exists because the flag used to be unprovable.** It was written as
/// an `if breakaway { A | B } else { A }` expression inside `launch_app`'s
/// spawn closure, and the only thing guarding it was a source-text count of
/// `CREATE_BREAKAWAY_FROM_JOB`. That needle counts the *`if` arm's spelling*,
/// not the call that selects it: changing `spawn(true)` to `spawn(false)`
/// removed the flag from every launch in the program while the guard went on
/// reporting it present, with the whole suite green. A `bool -> u32` function
/// can be called by a test with both inputs and compared against the
/// constants, which is a fact about the value that reaches `CreateProcess`
/// rather than a fact about the characters near it.
///
/// See [`launch_app`]'s doc for why each flag is here.
fn creation_flags_for(breakaway: bool) -> u32 {
    if breakaway {
        crate::bw_path::CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB
    } else {
        crate::bw_path::CREATE_NO_WINDOW
    }
}

/// The breakaway attempt-and-fallback policy, over any spawner.
///
/// **Generic over the spawn so that a test can be the spawn.** The three
/// decisions here -- that the *first* attempt asks for breakaway, that
/// `ACCESS_DENIED` and only `ACCESS_DENIED` earns a retry, and that the retry
/// asks for it *without* -- are the entire value of the flag, and none of them
/// is visible in a source-text needle. A fake spawner records the `bool` it
/// was handed, so the test asserts the sequence `[true, false]` reached the
/// caller, which no amount of moving the flag's spelling around can fake.
/// Nothing in the test path constructs a `Command`, let alone runs one.
///
/// `program` is used only for the log line.
fn launch_with(
    program: &str,
    mut spawn: impl FnMut(bool) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match spawn(true) {
        Ok(()) => Ok(()),
        // The containing job forbids breakaway. Falling back is right and not
        // a hole: without the flag the child joins whatever job Deskwarden is
        // already in, which is the same fate Deskwarden itself has -- the
        // alternative is refusing to open the app at all in an environment
        // this crate did not create.
        Err(e) if e.raw_os_error() == Some(ACCESS_DENIED_ERRNO) => {
            log::warn!("breakaway from job refused, starting {program} inside it");
            spawn(false)
        }
        Err(e) => Err(e),
    }
}

/// `ERROR_ACCESS_DENIED`, which is what `CreateProcess` returns when
/// `CREATE_BREAKAWAY_FROM_JOB` is set and the containing job does not permit
/// it. Named rather than spelled `5` at the match arm.
const ACCESS_DENIED_ERRNO: i32 = 5;

/// `CREATE_BREAKAWAY_FROM_JOB`, from `windows::Win32::System::Threading`.
/// Taken as the constant's `.0` at the point of use, the same way
/// `job_object::spawn_in_job` takes `CREATE_SUSPENDED.0`.
const CREATE_BREAKAWAY_FROM_JOB: u32 =
    windows::Win32::System::Threading::CREATE_BREAKAWAY_FROM_JOB.0;

/// What a refused app launch shows in the inline band.
///
/// Same shape as [`item_write_failure_message`]: a `because` clause, then the
/// state the world is actually in -- because the question after a click that
/// did nothing is "did it half-work?", and the answer is no. It names the
/// program file, because "couldn't open the app" without saying *which file it
/// tried* leaves nothing to fix; the file is already on screen in the Program
/// file row, and this says that is the one.
///
/// It does **not** say anything about the vault: unlike every other message in
/// this family, no write was attempted, so "your vault is unchanged" would be
/// answering a question nobody asked.
fn app_launch_failure_message(name: &str, program: &str, e: &std::io::Error) -> String {
    let because = match e.kind() {
        std::io::ErrorKind::NotFound => {
            "Windows couldn\u{2019}t find that program \u{2014} it may have been moved, renamed \
             or uninstalled since the app was matched"
        }
        std::io::ErrorKind::PermissionDenied => "Windows refused to start it",
        _ => "Windows couldn\u{2019}t start it",
    };
    format!(
        "Couldn\u{2019}t open the app for \u{201c}{name}\u{201d} \u{2014} {because}. Nothing was \
         started. ({program})"
    )
}

/// A launch the user has asked for, waiting to happen.
///
/// **Every Open on an app match becomes one of these, and the launcher is
/// reached from exactly one place**: the `DetailAction::OpenApp` arm no longer
/// starts anything, it records the request, and the block that drains this is
/// the only caller of [`launch_app`] in the program. That shape is the gate --
/// a launch that skipped the confirmation would have to be a second call to
/// `launch_app`, and `open_app_wiring_tests` counts them.
///
/// It outlives the frame it was created on, because a confirmed launch takes
/// at least two: the frame the Open was clicked on puts the dialog up, and the
/// frame the dialog's Open is clicked on starts the program.
pub(crate) struct PendingLaunch {
    /// The item the Open was clicked on. Carried because the failure band
    /// names it, and by the time the launch happens the selection may be a
    /// frame or several old.
    item_name: String,
    /// **The plan that is shown and the plan that runs.** One value: the
    /// dialog renders [`detail::command_line`] of this field and the launcher
    /// spawns this field, so there is no pair to drift.
    plan: LaunchPlan,
    /// Whether the user still has to approve it. Set once, from
    /// [`launch_needs_confirmation`], when the request is recorded.
    needs_confirmation: bool,
}

impl PendingLaunch {
    fn new(item_name: String, plan: LaunchPlan) -> Self {
        Self {
            needs_confirmation: launch_needs_confirmation(&plan),
            item_name,
            plan,
        }
    }
}

/// Whether a plan may go straight to the launcher, or must be shown to the
/// user first.
///
/// **`has_raw_args`, and nothing else.** `AppMatch::args` reaches the program
/// verbatim -- that is what makes `--profile-directory="Profile 2"` survive
/// intact, and it is equally what makes `--gpu-launcher="cmd /c calc.exe"` run
/// `calc.exe`: `--gpu-launcher`, `--renderer-cmd-prefix`,
/// `--utility-cmd-prefix` and `--browser-subprocess-path` all make Chrome
/// execute the string they are given. `args` is unvalidated vault data, and a
/// vault item can be shared in by another member of an organisation, so the
/// string is not necessarily one the user ever typed.
///
/// Shell metacharacters are inert here (no shell is involved; `&`, `|`, `^`
/// and `%` arrive as ordinary argv tokens and no second program can be
/// injected), so the surface is exactly "arbitrary flags to the one pinned
/// program" -- which for a browser is the whole machine. Consent on the exact
/// command line is the mitigation; see [`LaunchPlan`]'s doc for the two
/// stronger options and why neither is here.
///
/// A plan with no stored arguments is **not** confirmed. Its whole command
/// line is a program the launcher already vetted plus, at most, the item's own
/// URL through `quote_arg`; prompting for that would put a click in front of
/// the ordinary case and teach the user to dismiss the dialog without reading
/// it, which is the failure mode a confirmation has.
fn launch_needs_confirmation(plan: &LaunchPlan) -> bool {
    plan.has_raw_args
}

/// What the launch confirmation reports back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchConfirmAction {
    /// Still up. **The default**, so a frame in which nothing was clicked
    /// cannot be mistaken for approval.
    None,
    Launch,
    Cancel,
}

/// The heading the confirmation shows.
const LAUNCH_CONFIRM_TITLE: &str = "Open this app?";

/// The sentence above the command line.
///
/// It says where the arguments came from, because that is the fact the user
/// needs in order to judge the string underneath: they may never have typed
/// it, and on an item shared into their vault they certainly did not.
const LAUNCH_CONFIRM_BODY: &str =
    "This app match stores extra command-line arguments, which are passed to the program \
     exactly as they are written in the vault. Deskwarden will run:";

/// The button that starts the program. Not "OK": the label says what happens.
const LAUNCH_CONFIRM_GO: &str = "Open";

/// Draws the launch confirmation as a centered card over a dimmed scrim,
/// exactly as [`draw_folder_edit_modal`] does, and reports what was clicked.
///
/// **A dialog, not `confirm_click`.** The two-click confirm is deliberately
/// reserved for the item Delete (see its doc), and it is the wrong instrument
/// here for a reason that is not taste: `confirm_click` confirms *that* the
/// user meant it, by making them click twice. What has to be confirmed here is
/// *what will run* -- an arbitrary string out of the vault, possibly hundreds
/// of characters of it -- and a button that relabels itself cannot show that.
/// The tooltip already tried the "put it somewhere it can be read" approach
/// and requires hovering.
///
/// **It shows the command line in full.** `ui.set_width` plus a wrapping
/// monospace label, never a truncation or an ellipsis: a control that elides
/// the dangerous half of the string is worse than no control, because it looks
/// like one. The card is also allowed to grow and scroll rather than clip.
///
/// **It defaults to not launching.** `None` unless something was clicked, Esc
/// cancels, and the scrim eats clicks aimed at whatever is behind it.
///
/// Pure view: it starts nothing. The caller launches, which is what lets this
/// be driven from a test at all.
fn draw_launch_confirm_modal(ctx: &egui::Context, pending: &PendingLaunch) -> LaunchConfirmAction {
    use eframe::egui::{CornerRadius, Margin, RichText, Stroke};

    let mut action = LaunchConfirmAction::None;

    egui::Area::new(egui::Id::new("launch-confirm-scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            let screen = ctx.content_rect();
            ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter()
                .rect_filled(screen, CornerRadius::ZERO, egui::Color32::from_black_alpha(90));
        });

    egui::Area::new(egui::Id::new("launch-confirm-modal"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::CARD)
                .corner_radius(CornerRadius::same(10))
                .stroke(Stroke::new(1.0, theme::BORDER))
                .inner_margin(Margin::same(20))
                .show(ui, |ui| {
                    ui.set_width(420.0);
                    ui.label(theme::bold(LAUNCH_CONFIRM_TITLE, 15.0).color(theme::INK));
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(&pending.item_name)
                            .size(12.0)
                            .color(theme::TEXT_GHOST),
                    );
                    ui.add_space(10.0);
                    ui.label(RichText::new(LAUNCH_CONFIRM_BODY).size(12.0).color(theme::INK));
                    ui.add_space(10.0);
                    // The string itself. Monospace because it is a command
                    // line, and `wrap()` because the alternative at this width
                    // is an ellipsis over the part that matters.
                    egui::Frame::new()
                        .fill(theme::CARD_TINT)
                        .corner_radius(CornerRadius::same(7))
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(detail::command_line(&pending.plan))
                                    .size(12.0)
                                    .monospace()
                                    .color(theme::INK),
                            );
                        });
                    ui.add_space(18.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::primary_button(ui, LAUNCH_CONFIRM_GO, None).clicked() {
                            action = LaunchConfirmAction::Launch;
                        }
                        ui.add_space(8.0);
                        if theme::secondary_button(ui, "Cancel").clicked() {
                            action = LaunchConfirmAction::Cancel;
                        }
                    });
                });
        });

    // Esc cancels, same as every other transient overlay in this app.
    if action == LaunchConfirmAction::None && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = LaunchConfirmAction::Cancel;
    }

    action
}

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
    // thread (see `totp_poll_in_flight`'s declaration in `run`) because a
    // trickling or stalled `bw serve` freezes this window for as long as
    // `get_totp` takes -- up to the bridge's whole-request `READ_DEADLINE`
    // of 10s -- once per `TOTP_POLL_INTERVAL`.
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
        // `bw serve` (every poll running the full `READ_DEADLINE` before it
        // fails) would still spawn a fresh background thread every
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
/// The two write arms that live INSIDE the per-frame closure, where no test
/// can call them, pinned as source text.
///
/// `move_item_into_folder` is a free function and is tested for real
/// (`a_move_the_backend_accepts_files_the_item_and_says_nothing` asserts the
/// row adopts the backend's copy). These two are not reachable that way, and
/// they are the same decision: on success, the window's local `Vec` must take
/// the item the CACHE handed back -- which is the backend's copy -- rather
/// than the value the window built and sent. Keeping the sent value leaves the
/// row holding a `revisionDate` the write has already superseded, and the next
/// write of that row is refused with a 400 (see `vault_bridge`'s
/// `REVISION_DATE_KEY`). That is the user-reported favourite defect.
#[cfg(test)]
mod write_arms_adopt_the_backends_copy_tests {
    // EVERY NEEDLE IS SPLIT ACROSS TWO LITERALS, AND THAT IS LOAD-BEARING --
    // see `reveal_state_placement_tests` for the full reasoning: `include_str!`
    // pulls this module in too, so an unsplit needle always matches its own
    // definition and the test can never fail. The occurrence counts below are
    // what enforce the splitting.
    const MOVE_ARM: &str = concat!("items[pos] = ", "moved;");
    const SAVE_ARM: &str = concat!("items[pos] = ", "saved;");
    const FAVOURITE_ARM: &str = concat!("items[pos] = ", "starred;");
    /// What the move arm used to do: rebuild the row locally from the item it
    /// sent. Present nowhere in this file once the arm is right.
    const MOVE_ARM_REBUILD: &str = concat!("items[pos] = crate::vault_bridge::", "with_folder(");
    /// What the save arm used to do: reinstate the value it PUT.
    const SAVE_ARM_REINSTATE: &str = concat!("items[pos] = ", "updated;");
    /// The MATCHED APP card's two arms, and the two values each of them must
    /// NOT reinstate -- `rebound`/`unbound` are what those arms SEND.
    const TRIGGER_ARM: &str = concat!("items[pos] = ", "triggered;");
    const UNMATCH_ARM: &str = concat!("items[pos] = ", "cleared;");
    const TRIGGER_ARM_REINSTATE: &str = concat!("items[pos] = ", "rebound;");
    const UNMATCH_ARM_REINSTATE: &str = concat!("items[pos] = ", "unbound;");
    /// The calls that make each arm a write at all.
    const UNMATCH_CALL: &str = concat!("crate::vault_bridge::without_app", "_match(item)");
    const TRIGGER_CALL: &str = concat!("detail::app_match_with", "_trigger(&current, to)");
    /// **Which row the returned item is written ONTO** -- the half of "adopt
    /// the backend's copy" this suite did not pin (review 20's Minor 3).
    /// Replacing this selector with `position(|_i| true)` writes the server's
    /// copy of the edited item over row 0 of the list, and the whole suite
    /// stayed green: every `items[pos] = ...;` needle above still matched, and
    /// nothing else reads the list's ordering.
    const ROW_SELECTOR: &str = concat!("items.iter().position(|i| i.id == ", "item.id)");
    /// The favourite arm names its own returned item rather than `item`, so
    /// it has its own spelling of the same rule.
    const STAR_ROW_SELECTOR: &str = concat!("items.iter().position(|i| i.id == ", "starred.id)");
    /// The head of every one of these adoptions, and the boundary between one
    /// arm and the next -- see [`arm_pins_its_row`]. Split for this module's
    /// standing reason, and note that it is also the tail of every `*_ARM`
    /// const above, so nothing counts occurrences of it.
    const ASSIGNMENT_HEAD: &str = concat!("items[pos]", " = ");
    /// The bare call, whatever predicate it was given. The anchor
    /// [`arm_pins_its_row`] searches back to, so a MUTATED predicate is what
    /// the assertion reads rather than a correct one further up the file.
    const ANY_ROW_SELECTOR: &str = concat!("items.iter().", "position(");

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    /// Asserts that the arm whose adoption reads `assignment` picks its row
    /// with `selector`.
    ///
    /// **Bounded by the code itself, not by a byte count.** The slice runs
    /// from the LAST [`ANY_ROW_SELECTOR`] before the assignment -- so a
    /// mutated predicate is the one this reads, rather than a correct one
    /// further up the file -- and it is rejected outright if another arm's
    /// [`ASSIGNMENT_HEAD`] sits in between, which is what it would mean for
    /// the selector found to belong to a different arm. A fixed byte window
    /// was tried first and is what those two facts replace: the widest of
    /// these arms carries 600 bytes of comment between its selector and its
    /// assignment, and a window wide enough for that reaches its neighbour.
    ///
    /// A slice rather than a whole-file count for the same reason: a count
    /// says "the right selector appears N times somewhere", which a mutation
    /// in ONE arm leaves true of the others.
    fn arm_pins_its_row(assignment: &str, selector: &str, what: &str) {
        let source = source();
        let at = source.find(assignment).unwrap_or_else(|| {
            panic!(
                "no {assignment:?} in this file -- {what} was renamed or deleted, and this                  guard is now aimed at nothing"
            )
        });
        let selector_at = source[..at].rfind(ANY_ROW_SELECTOR).unwrap_or_else(|| {
            panic!("{what} reaches {assignment:?} without resolving a row at all")
        });
        assert!(
            !source[selector_at..at].contains(ASSIGNMENT_HEAD),
            "{what} has no row lookup of its own -- the nearest one above it belongs to              another arm, which this guard must not accept as proof. Between them: {:?}",
            &source[selector_at..at]
        );
        assert!(
            source[selector_at..].starts_with(selector),
            "{what} adopts the backend's copy but does not pin WHICH ROW it lands on: it              resolves the row with {:?} rather than {selector:?}, so the server's copy of              the edited item is written over some other item's row",
            &source[selector_at..at]
        );
    }

    #[test]
    fn the_row_menus_move_arm_takes_the_item_the_cache_returned() {
        assert_eq!(
            source().matches(MOVE_ARM).count(),
            1,
            "the row menu's move arm does not adopt the cache's returned item exactly once \
             (needle {MOVE_ARM:?})"
        );
        assert_eq!(
            source().matches(MOVE_ARM_REBUILD).count(),
            0,
            "a move arm is rebuilding the row locally again ({MOVE_ARM_REBUILD:?}); that copy \
             carries a superseded revisionDate and the row's next write is refused"
        );
    }

    #[test]
    fn the_edit_panes_save_arm_takes_the_item_the_cache_returned() {
        assert_eq!(
            source().matches(SAVE_ARM).count(),
            1,
            "the edit pane's save arm does not adopt the cache's returned item exactly once \
             (needle {SAVE_ARM:?})"
        );
        assert_eq!(
            source().matches(SAVE_ARM_REINSTATE).count(),
            0,
            "the save arm is reinstating the value it sent again ({SAVE_ARM_REINSTATE:?}); a \
             second save of one item is then refused with a 400"
        );
    }

    /// POSITIVE CONTROL for the two `count() == 0` assertions above. A needle
    /// that never matched anything would make them pass forever, including
    /// against a spelling change that left the defect live. This proves the
    /// search itself finds this file's real text.
    /// The arm the user's report is about. It has adopted the cache's answer
    /// all along -- what had not was `VaultCache::set_favorite` itself, which
    /// handed back the value it sent. Pinned here so the two halves of that
    /// fix cannot drift apart.
    #[test]
    fn the_favourite_arm_takes_the_item_the_cache_returned() {
        assert_eq!(
            source().matches(FAVOURITE_ARM).count(),
            1,
            "the star's arm does not adopt the cache's returned item exactly once (needle              {FAVOURITE_ARM:?})"
        );
    }

    /// The MATCHED APP card's two write arms, which live in the same closure
    /// as the three above and are unreachable from this suite for the same
    /// reason. Both PUT the item, so both carry the identical `revisionDate`
    /// hazard: a trigger change that reinstated its own copy would make the
    /// user's NEXT edit of that item fail with a 400.
    #[test]
    fn the_app_match_arms_take_the_item_the_cache_returned() {
        for (needle, what) in [
            (TRIGGER_ARM, "the MATCHED APP card's trigger arm"),
            (UNMATCH_ARM, "the MATCHED APP card's Remove arm"),
        ] {
            assert_eq!(
                source().matches(needle).count(),
                1,
                "{what} does not adopt the cache's returned item exactly once (needle \
                 {needle:?})"
            );
        }
        // Neither arm may rebuild the row from what it SENT -- the same
        // defect the move and save arms were fixed out of, one card further
        // on.
        for needle in [TRIGGER_ARM_REINSTATE, UNMATCH_ARM_REINSTATE] {
            assert_eq!(
                source().matches(needle).count(),
                0,
                "a MATCHED APP arm is reinstating the value it sent ({needle:?}); the item's \
                 next write is then refused with a 400"
            );
        }
        // **The write really is a write.** An arm that adopted a returned
        // item without ever calling the cache would satisfy everything above.
        assert_eq!(
            source().matches(UNMATCH_CALL).count(),
            1,
            "the Remove arm does not clear the app-match field through \
             `vault_bridge::without_app_match` exactly once (needle {UNMATCH_CALL:?}); \
             without it, Remove reports and writes nothing"
        );
        // **And each arm SENDS the value it built.** Passing the bare
        // `item` to the cache instead of the rebuilt/cleared copy writes the
        // UNCHANGED item -- the trigger change never reaches the vault -- and
        // the whole suite stays green; only `unused variable` catches it,
        // i.e. the compiler and not a test.
        for (needle, what) in [
            (concat!("cache.update_item(&", "rebound)"), "the trigger arm"),
            (concat!("cache.update_item(&", "unbound)"), "the Remove arm"),
        ] {
            assert_eq!(
                source().matches(needle).count(),
                1,
                "{what} does not PUT the item it built exactly once (needle {needle:?}); \
                 sending `item` instead writes the unchanged item and the edit is lost"
            );
        }
        assert_eq!(
            source().matches(TRIGGER_CALL).count(),
            1,
            "the trigger arm does not go through `detail::app_match_with_trigger` exactly \
             once (needle {TRIGGER_CALL:?}); rebuilding the match here would make this a \
             second producer of the four fields the picker captured off a live window"
        );
    }

    /// **Onto the RIGHT ROW.** "Adopt the returned item" was pinned above and
    /// "put it back where it came from" was not, so `position(|i| i.id ==
    /// item.id)` could become `position(|_i| true)` in either MATCHED APP arm
    /// with 1309/1309 green and no warning -- the edited item's server copy
    /// landing on row 0 of the list, which is a different item every time the
    /// user's sort or filter changes.
    ///
    /// The two pre-existing arms are here as well: they share the shape, the
    /// mutation survives in them identically, and covering them costs one
    /// line each.
    #[test]
    fn every_write_arm_puts_the_returned_item_back_on_its_own_row() {
        for (assignment, what) in [
            (TRIGGER_ARM, "the MATCHED APP card's trigger arm"),
            (UNMATCH_ARM, "the MATCHED APP card's Remove arm"),
            (MOVE_ARM, "the row menu's move arm"),
            (SAVE_ARM, "the edit pane's save arm"),
        ] {
            arm_pins_its_row(assignment, ROW_SELECTOR, what);
        }
        // The star's arm resolves its row off the item the cache handed back
        // rather than off `item`, so it has its own selector.
        arm_pins_its_row(FAVOURITE_ARM, STAR_ROW_SELECTOR, "the star's arm");
        // POSITIVE CONTROL, in two parts: the helper must be capable of
        // failing, or none of the assertions above can.
        //
        // 1. The mutation this test exists to catch appears nowhere.
        assert_eq!(
            source().matches(concat!("items.iter().position(|_i| ", "true)")).count(),
            0,
            "the mutation this test exists to catch is live somewhere in this file"
        );
        // 2. The star's arm really does use a DIFFERENT selector, so the
        //    per-arm slicing is doing work: asked for the ordinary one, the
        //    helper must reject it.
        let mismatched = std::panic::catch_unwind(|| {
            arm_pins_its_row(FAVOURITE_ARM, ROW_SELECTOR, "a deliberately wrong pairing")
        });
        assert!(
            mismatched.is_err(),
            "arm_pins_its_row accepts a selector the arm does not use, so an arm that              resolved the wrong row would pass"
        );
    }

    #[test]
    fn the_needles_are_searched_against_this_files_real_source() {
        assert!(
            source().contains(concat!("fn move_item_into_", "folder(")),
            "include_str! is not reading this module's own source"
        );
    }
}

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
    // The head of the PER-FRAME CLOSURE ITSELF, not the `run_ui_native` call
    // that used to wrap it. Those were the same position until `build_frame`
    // split the closure out from the event loop; they are not any more, and
    // `run_ui_native` is now BELOW every line of the closure, so a needle
    // aimed at it would put "before the closure" past the end of the closure
    // and this guard would pass against a `RevealState` declared anywhere at
    // all inside it. Aimed at the closure's own head, "before the closure"
    // still means what it says.
    const FRAME_CLOSURE: &str = concat!("let vault_frame_fn = ", "move |ui: &mut egui::Ui");
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
mod folder_drop_tests {
    //! What a drag-to-folder does to the window's own item list.
    //!
    //! The user's explicit choice was that a FAILED move **reverts in the UI
    //! and shows an inline error** rather than leaving the row looking moved.
    //! Both halves are here: the list's entry and the returned message.
    //!
    //! These drive `move_item_into_folder` against a real `VaultCache` over a
    //! real HTTP server (mockito), so the success path is a write that
    //! actually reached a backend and the failure path is a backend that
    //! actually refused -- not a stubbed `Result`.
    use super::*;
    use crate::vault_bridge::{VaultBridge, VaultItem};
    use crate::vault_cache::VaultCache;

    fn item(id: &str, folder_id: Option<&str>) -> VaultItem {
        VaultItem {
            id: id.into(),
            name: "Ledgerline".into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: Some(1),
            folder_id: folder_id.map(str::to_string),
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    /// The pieces `move_item_into_folder` needs beyond the list.
    fn ctx_and_reauth() -> (egui::Context, Rc<RefCell<bool>>) {
        (egui::Context::default(), Rc::new(RefCell::new(false)))
    }

    #[test]
    fn a_move_the_backend_accepts_files_the_item_and_says_nothing() {
        let mut server = mockito::Server::new();
        // Answers the way the backend does -- with the item, carrying the
        // `revisionDate` this write minted. `move_item_into_folder` must adopt
        // THAT, not the optimistic local rebuild it painted first; see
        // `vault_bridge`'s `REVISION_DATE_KEY`.
        let put = crate::vault_bridge::echoing_item_put(
            &mut server,
            "/object/item/i1",
            "2026-08-03T02:33:03.427Z",
        )
        .create();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let (ctx, reauth) = ctx_and_reauth();
        let mut items = vec![item("i1", None)];

        let message = move_item_into_folder(&ctx, &cache, &reauth, &mut items, "i1", "f2");

        put.assert();
        assert_eq!(message, None, "a successful move should have nothing to say");
        assert_eq!(items[0].folder_id.as_deref(), Some("f2"));
        // THE ROW MUST HOLD THE SERVER'S COPY, not the optimistic rebuild
        // that was painted first. They agree on the folder -- which is why
        // the assertion above passes either way -- and differ on
        // `revisionDate`, the token the item's NEXT write has to carry or be
        // refused with a 400. See `vault_bridge`'s `REVISION_DATE_KEY`.
        assert_eq!(
            items[0].other.get("revisionDate").and_then(|v| v.as_str()),
            Some("2026-08-03T02:33:03.427Z"),
            "the moved row kept this window's own copy, not the backend's answer"
        );
    }

    #[test]
    fn a_move_the_backend_refuses_leaves_the_item_exactly_where_it_was() {
        // THE REVERT. The optimistic write happens first and unconditionally,
        // so this is not "the failure arm forgot to touch anything" -- the
        // entry really is rewritten and really is put back. Asserted on the
        // FOLDER, which is the only thing a move may change.
        let mut server = mockito::Server::new();
        let _put = server.mock("PUT", "/object/item/i1").with_status(500).create();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let (ctx, reauth) = ctx_and_reauth();
        let mut items = vec![item("i1", Some("f1"))];

        let message = move_item_into_folder(&ctx, &cache, &reauth, &mut items, "i1", "f2");

        assert_eq!(
            items[0].folder_id.as_deref(),
            Some("f1"),
            "a refused move left the row looking moved"
        );
        let message = message.expect("a refused move said nothing at all");
        assert!(
            message.contains("Ledgerline"),
            "the message does not name the item: {message:?}"
        );
    }

    #[test]
    fn a_refused_move_leaves_the_rest_of_the_item_untouched_too() {
        // The revert restores the whole entry, not a patched folder: anything
        // `with_folder` normalises on the way through must come back as well.
        let mut server = mockito::Server::new();
        let _put = server.mock("PUT", "/object/item/i1").with_status(500).create();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let (ctx, reauth) = ctx_and_reauth();
        let before = item("i1", Some("f1"));
        let mut items = vec![before.clone()];

        let _ = move_item_into_folder(&ctx, &cache, &reauth, &mut items, "i1", "f2");

        // `VaultItem` is not `PartialEq` (it carries a `Zeroizing` password),
        // so the whole entry is compared through its own serialization --
        // which also covers everything riding the `#[serde(flatten)] other`
        // catch-alls, the fields a field-by-field comparison would miss.
        assert_eq!(
            serde_json::to_value(&items[0]).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }

    #[test]
    fn a_drop_on_an_item_that_is_no_longer_listed_changes_nothing_and_says_nothing() {
        // The vault reloaded out from under the gesture. There is no row on
        // screen to explain anything about, so an inline error here would be
        // about an item the user can no longer see.
        let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
        let (ctx, reauth) = ctx_and_reauth();
        let mut items = vec![item("i1", None)];

        let message = move_item_into_folder(&ctx, &cache, &reauth, &mut items, "gone", "f2");

        assert_eq!(message, None);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "i1");
        assert_eq!(items[0].folder_id, None);
    }

    #[test]
    fn a_401_during_a_move_still_reverts_and_flags_reauth() {
        // `Unauthorized` is the one failure that closes the window and
        // re-authenticates. The revert must not be skipped on the way out --
        // the snapshot survives a re-auth, and a row left looking moved would
        // still be wrong when the window came back.
        let mut server = mockito::Server::new();
        let _put = server.mock("PUT", "/object/item/i1").with_status(401).create();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let (ctx, reauth) = ctx_and_reauth();
        let mut items = vec![item("i1", Some("f1"))];

        let message = move_item_into_folder(&ctx, &cache, &reauth, &mut items, "i1", "f2");

        assert_eq!(items[0].folder_id.as_deref(), Some("f1"));
        assert!(*reauth.borrow(), "a 401 did not flag re-authentication");
        assert!(message.is_some());
    }

    #[test]
    fn every_failure_gets_its_own_wording_and_all_of_them_say_where_the_item_is() {
        // Exhaustive over `VaultError`, spelled out: a new variant must be
        // given its own sentence rather than inheriting one, and every
        // sentence has to answer the question the user actually has.
        let messages = [
            move_failure_message("Ledgerline", &VaultError::Unauthorized),
            move_failure_message("Ledgerline", &VaultError::Http("500".into())),
            move_failure_message("Ledgerline", &VaultError::Parse("bad json".into())),
        ];
        for message in &messages {
            assert!(message.contains("Ledgerline"), "{message:?} does not name the item");
            assert!(
                message.contains("old folder"),
                "{message:?} does not say the item did not move"
            );
        }
        let unique: std::collections::BTreeSet<&String> = messages.iter().collect();
        assert_eq!(unique.len(), messages.len(), "two failures share one wording: {messages:?}");
        // The developer-facing payload stays in the log, not in the band.
        assert!(!messages[1].contains("500"));
        assert!(!messages[2].contains("bad json"));
    }
}

/// What a refused favourite, save or create says.
///
/// The behaviour these pin is the whole of the fix for "a refused write is
/// still silent on the two arms the bug was reported through": before it, all
/// three arms were `log::warn!` plus a re-auth flag, so a 400 -- the official
/// client edited the item, a concurrent write, a restore whose token this app
/// had not read back -- left the star looking flipped and the form looking
/// saved with nothing on screen to say otherwise. That is the user's report
/// verbatim.
#[cfg(test)]
mod item_write_failure_tests {
    use super::{item_write_failure_message, ItemWrite, VaultError};

    /// Every variant against every error, spelled out rather than iterated:
    /// neither enum has a catch-all, so a new one has to be brought here
    /// deliberately.
    fn all() -> Vec<String> {
        let mut out = Vec::new();
        for write in [
            ItemWrite::Favorite,
            ItemWrite::Unfavorite,
            ItemWrite::Save,
            ItemWrite::Create,
            ItemWrite::AppTrigger,
            ItemWrite::AppUnmatch,
        ] {
            for e in [
                VaultError::Unauthorized,
                VaultError::Http("400 Bad Request".into()),
                VaultError::Parse("expected value at line 1".into()),
            ] {
                out.push(item_write_failure_message(write, "Ledgerline", &e));
            }
        }
        out
    }

    #[test]
    fn every_refusal_names_the_item_and_says_what_failed() {
        // A band that said only "the vault backend refused the write" is a
        // band the user cannot act on: this window shows one at a time and
        // they may have clicked several things. Dropping `{name}` from any arm
        // fails here with that arm's own sentence quoted.
        for message in all() {
            assert!(message.contains("Ledgerline"), "which item? {message}");
            assert!(message.starts_with("Couldn't "), "what failed? {message}");
        }
    }

    #[test]
    fn every_refusal_says_the_thing_the_user_cannot_see_is_unchanged() {
        // The question a user has after clicking and seeing nothing move is
        // "did it half-work?", and the answer is always no. The star and the
        // form are both painted from local state a refused write does not
        // touch, so nothing on screen answers it.
        //
        // Deleting the tail of any one arm fails here with that arm quoted.
        for message in all() {
            assert!(
                message.contains("still")
                    || message.contains("Nothing has been written")
                    || message.contains("Nothing has been added"),
                "does not say what state the item or the form is in: {message}"
            );
        }
    }

    #[test]
    fn the_two_favourite_directions_are_not_worded_the_same_way() {
        // THE ONE THING THAT CAN BE GOT BACKWARDS. "It still isn't one" after
        // a failed UN-favourite is a false statement about the user's vault --
        // it tells them the star is off when it is on -- and a single variant
        // carrying a `bool` is exactly how that mistake gets made. Swapping
        // the two arms' bodies fails here.
        let e = VaultError::Http("400 Bad Request".into());
        let on = item_write_failure_message(ItemWrite::Favorite, "Ledgerline", &e);
        let off = item_write_failure_message(ItemWrite::Unfavorite, "Ledgerline", &e);

        assert!(on.contains("add") && on.contains("still isn't one"), "{on}");
        assert!(off.contains("remove") && off.contains("still is one"), "{off}");
    }

    #[test]
    fn a_refused_save_or_create_promises_the_typing_survives() {
        // Both editors are LEFT OPEN on failure (`mode` stays on
        // `Edit`/`Create`), so this is true -- and saying it is what stops a
        // user from assuming their work is gone and retyping it. Deleting
        // either clause fails here.
        let e = VaultError::Http("400 Bad Request".into());
        assert!(
            item_write_failure_message(ItemWrite::Save, "Ledgerline", &e)
                .contains("still in the form"),
            "a refused save must say the edits survived"
        );
        assert!(
            item_write_failure_message(ItemWrite::Create, "Ledgerline", &e)
                .contains("still in the form"),
            "a refused create must say the entries survived"
        );
    }

    #[test]
    fn no_two_of_the_twelve_share_a_wording() {
        // The same property `move_failure_message` and
        // `list_command_failure_message` are held to, and for the same reason:
        // a band that reads identically for four different clicks cannot tell
        // the user which one it is about. Collapsing any two arms into one
        // fails here with the duplicate count.
        let messages = all();
        let unique: std::collections::BTreeSet<&String> = messages.iter().collect();
        assert_eq!(
            unique.len(),
            messages.len(),
            "two refusals share one wording: {messages:?}"
        );
    }

    #[test]
    fn the_backends_own_words_stay_in_the_log() {
        // Same rule as every other band in this window: the payload is a
        // developer's, not a user's.
        for message in all() {
            assert!(!message.contains("400 Bad Request"), "{message}");
            assert!(!message.contains("expected value"), "{message}");
        }
    }
}

/// What a failed Generate says, and what it does about the session.
///
/// The behaviour these pin is the whole of the fix for "Generate swallows
/// every non-401 failure": before it, the only report was a `log::warn!` the
/// user never sees, so a 500, a parse failure, a dead `bw serve` or the
/// request deadline all left the password box unchanged with nothing on
/// screen -- indistinguishable from a button that does nothing.
#[cfg(test)]
mod generate_failure_tests {
    use super::{generate_failure, VaultError};

    /// Every variant, spelled out rather than iterated: `generate_failure`
    /// has no catch-all, so a new `VaultError` must be brought here
    /// deliberately.
    fn all() -> [super::GenerateFailure; 3] {
        [
            generate_failure(&VaultError::Unauthorized),
            generate_failure(&VaultError::Http("500 Internal Server Error".into())),
            generate_failure(&VaultError::Parse("expected value at line 1".into())),
        ]
    }

    #[test]
    fn every_failure_has_a_message_of_its_own_that_says_the_form_is_untouched() {
        let failures = all();
        for failure in &failures {
            assert!(
                failure.message.contains("Couldn't generate"),
                "{:?} does not say what failed",
                failure.message
            );
            // The question a user has when the box looks exactly as it did.
            assert!(
                failure.message.contains("Nothing in this form has changed"),
                "{:?} does not say the draft is intact",
                failure.message
            );
        }
        let unique: std::collections::BTreeSet<&String> =
            failures.iter().map(|f| &f.message).collect();
        assert_eq!(
            unique.len(),
            failures.len(),
            "two failures share one wording: {failures:?}"
        );
    }

    #[test]
    fn the_backend_s_own_error_payload_stays_in_the_log() {
        let failures = all();
        assert!(
            !failures[1].message.contains("500"),
            "{:?} puts a transport string in a user's sentence",
            failures[1].message
        );
        assert!(
            !failures[2].message.contains("line 1"),
            "{:?} puts a serde message in a user's sentence",
            failures[2].message
        );
    }

    #[test]
    fn only_an_unauthorized_generate_asks_for_re_authentication() {
        let failures = all();
        // Positive control first: without it the two negatives below pass
        // against a function that returns `false` unconditionally.
        assert!(
            failures[0].needs_reauth,
            "a 401 on Generate must still flag the session as needing re-auth -- the \
             backend really has stopped accepting it, and the window's caller is what \
             recovers"
        );
        assert!(!failures[1].needs_reauth, "a 500 is not an expired session");
        assert!(
            !failures[2].needs_reauth,
            "an unreadable answer is not an expired session"
        );
    }

    #[test]
    fn the_unauthorized_message_is_the_only_one_that_asks_the_user_to_sign_in() {
        let failures = all();
        assert!(
            failures[0].message.contains("sign in again"),
            "{:?} flags re-auth without telling the user why saving will fail",
            failures[0].message
        );
        assert!(!failures[1].message.contains("sign in"));
        assert!(!failures[2].message.contains("sign in"));
    }
}

/// What a failed Archive / Unarchive / Restore / Delete-forever tells the
/// user.
///
/// Before this, nothing: the four arms called `log::warn!`, set the re-auth
/// flag on a 401, and were otherwise silent -- on the same screen whose
/// branch already routes aux-FETCH failures through the inline band. A
/// rejected write was indistinguishable from a successful one that had not
/// refreshed yet.
#[cfg(test)]
mod list_command_failure_message_tests {
    use super::{list_command_failure_message, ListCommand, VaultError};

    /// Every command, spelled out rather than derived: the point is that a
    /// fifth variant has to be brought here deliberately, and
    /// `list_command_failure_message`'s own `match` has no catch-all for the
    /// same reason.
    const EVERY_COMMAND: [ListCommand; 5] = [
        ListCommand::Archive,
        ListCommand::Unarchive,
        ListCommand::Restore,
        ListCommand::Purge,
        ListCommand::Delete,
    ];

    fn every_error() -> [VaultError; 3] {
        [
            VaultError::Unauthorized,
            VaultError::Http("400 Bad Request".into()),
            VaultError::Parse("expected value at line 1".into()),
        ]
    }

    #[test]
    fn every_message_names_the_item_and_says_it_did_not_move() {
        for command in EVERY_COMMAND {
            for e in every_error() {
                let message = list_command_failure_message(command, "Ledgerline", &e);
                assert!(
                    message.contains("\"Ledgerline\""),
                    "{command:?}/{e:?} does not name the item: {message:?}"
                );
                // The actual question a user has after clicking Restore and
                // seeing the row unchanged is "did it half-work?". Every
                // sentence has to answer it, and the answer is always no.
                assert!(
                    message.contains("It's still in"),
                    "{command:?}/{e:?} does not say where the item still is: {message:?}"
                );
                assert!(message.starts_with("Couldn't "), "{command:?}/{e:?}: {message:?}");
            }
        }
    }

    #[test]
    fn each_command_says_which_one_it_was() {
        // A shared "Couldn't complete that" would satisfy the test above for
        // every command at once. These are five different actions with five
        // different consequences, and the band is the only place the user
        // learns which one was refused.
        let e = VaultError::Http("400 Bad Request".into());
        let messages: Vec<String> = EVERY_COMMAND
            .iter()
            .map(|c| list_command_failure_message(*c, "Ledgerline", &e))
            .collect();
        let mut distinct = messages.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            EVERY_COMMAND.len(),
            "two commands share a sentence: {messages:?}"
        );
        assert!(messages[0].contains("archive \""), "{:?}", messages[0]);
        assert!(messages[1].contains("unarchive \""), "{:?}", messages[1]);
        assert!(messages[2].contains("restore \""), "{:?}", messages[2]);
        // NOT bare "delete": this is the one irreversible command in the
        // window, and the ordinary Delete sits two entries away on the live
        // menu. "Couldn't delete" would leave a user unsure which refused.
        assert!(
            messages[3].contains("permanently delete \""),
            "the purge failure does not say it was the permanent one: {:?}",
            messages[3]
        );
        // And its opposite number: the soft delete must NOT claim to be the
        // permanent one. These two are the pair the wording exists to keep
        // apart, so each is asserted against the other's word.
        assert!(
            messages[4].contains("delete \"") && !messages[4].contains("permanently"),
            "the soft delete's failure is not distinguishable from the purge's: {:?}",
            messages[4]
        );
        // It also has to name where the item still is, and that is the live
        // vault -- NOT the trash it was on its way to.
        assert!(
            messages[4].contains("still in your vault"),
            "the soft delete's failure does not say the item is still in the vault: {:?}",
            messages[4]
        );
    }

    #[test]
    fn each_error_says_why_in_the_windows_one_vocabulary() {
        // The same three reasons `move_failure_message` gives, worded
        // identically: one vocabulary for "the backend said no" across this
        // window, not two that drift.
        for command in EVERY_COMMAND {
            let named = |e: &VaultError| list_command_failure_message(command, "L", e);
            assert!(named(&VaultError::Unauthorized).contains("no longer accepts this session"));
            assert!(named(&VaultError::Http("400".into())).contains("refused the write"));
            assert!(named(&VaultError::Parse("x".into())).contains("answer couldn't be read"));
        }
    }

    #[test]
    fn the_backends_own_payload_stays_out_of_the_users_sentence() {
        // `move_failure_message`'s rule, restated because the same mistake is
        // one interpolation away: a `ureq` transport string or a serde
        // message is a developer's sentence in the middle of a user's, and
        // the log line at the call site already carries the whole thing.
        let message = list_command_failure_message(
            ListCommand::Archive,
            "Ledgerline",
            &VaultError::Http("400 Bad Request: cipher already archived".into()),
        );
        assert!(
            !message.contains("400") && !message.contains("cipher already archived"),
            "the backend's payload leaked into the band: {message:?}"
        );
    }
}

/// Which of the three failures the window's one inline band shows.
#[cfg(test)]
mod inline_notice_tests {
    use super::{inline_notice, NoticeSource};

    #[test]
    fn nothing_waiting_shows_no_band() {
        assert_eq!(inline_notice(None, None, None), None);
    }

    #[test]
    fn each_source_reaches_the_band_on_its_own() {
        // The positive control for the precedence tests below: each of the
        // three really is shown when it is the only one waiting, so a
        // `inline_notice` that returned `None` for two of them could not pass
        // the precedence tests by default.
        assert_eq!(
            inline_notice(Some("g"), None, None),
            Some((NoticeSource::Generate, "g"))
        );
        assert_eq!(inline_notice(None, Some("a"), None), Some((NoticeSource::Aux, "a")));
        assert_eq!(inline_notice(None, None, Some("m")), Some((NoticeSource::Move, "m")));
    }

    #[test]
    fn a_generate_failure_outranks_both_standing_ones() {
        // The regression this exists to prevent: a Trash fetch that failed ten
        // minutes ago holding the band means the Generate click the user made
        // one second ago produces nothing at all on screen -- the dead button,
        // back again, in the one case where the band was already occupied.
        assert_eq!(
            inline_notice(Some("g"), Some("a"), Some("m")),
            Some((NoticeSource::Generate, "g"))
        );
        assert_eq!(
            inline_notice(Some("g"), Some("a"), None),
            Some((NoticeSource::Generate, "g"))
        );
        assert_eq!(
            inline_notice(Some("g"), None, Some("m")),
            Some((NoticeSource::Generate, "g"))
        );
    }

    #[test]
    fn a_failed_aux_fetch_still_outranks_a_refused_move() {
        // Unchanged from before Generate existed as a source: the pane the
        // aux error explains is the one on screen.
        assert_eq!(
            inline_notice(None, Some("a"), Some("m")),
            Some((NoticeSource::Aux, "a"))
        );
    }
}

/// Source-text guards on the two `EditAction::GeneratePassword` arms.
///
/// They exist because those arms live inside `run`'s update closure, which
/// needs a real event loop and so is unreachable from this suite -- a
/// reviewer demonstrated that by replacing three of its action arms with `{}`
/// and watching all 784 tests stay green. `generate_failure` and
/// `inline_notice` carry the decisions precisely so they can be tested
/// directly; what is left in the arms is plumbing, and this is what pins the
/// plumbing.
///
/// **What they do not guarantee**, stated so the doc does not imply coverage
/// it lacks: they see spellings and counts, not behaviour. A `generate_error`
/// that is set here and then never read would pass. What they catch is the
/// edit that quietly puts a swallowed error back -- which is exactly the
/// regression that happened, twice, with a commit message claiming otherwise.
#[cfg(test)]
mod generate_failure_wiring_tests {
    // EVERY NEEDLE IS SPLIT WITH `concat!`, AND THAT IS LOAD-BEARING:
    // `include_str!("mod.rs")` pulls this module in too, so a needle written
    // as one literal always matches -- inside the const that defines it --
    // which is how the equivalent guards elsewhere in this file once passed
    // with their regression live. The count assertions ENFORCE the split, not
    // this comment: re-joining a needle makes it appear one extra time and
    // fails.
    const ARM: &str = concat!("EditAction::GeneratePassword", " => {");
    const NEXT_ARM: &str = concat!("EditAction::Cancel", " =>");
    const DECIDES: &str = concat!("generate_failure", "(&e)");
    const REPORTS: &str = concat!("generate_error = Some(", "failure.message)");
    const CLOSES_THE_WINDOW: &str = concat!("flag_reauth_if_", "unauthorized(");
    const FLAGS_THE_SESSION: &str = concat!("*needs_reauth_for_closure.borrow_mut()", " = true;");
    const BAND_FEED: &str = concat!("let notice = ", "inline_notice(");
    const DISMISS_BLOCK: &str = concat!("if dismiss_move_", "error {");
    /// The favicon loop that follows the dismissal, which is the last thing
    /// in its own block.
    const DISMISS_BLOCK_END: &str = concat!("for id in &visible", "_ids {");
    const DISMISS_MATCHES_SOURCE: &str = concat!("match notice", "_source {");
    const DISMISS_GENERATE: &str = concat!("NoticeSource::Generate) => generate", "_error = None,");
    const DISMISS_MOVE: &str = concat!("NoticeSource::Move) => move", "_error = None,");
    const DISMISS_AUX: &str =
        concat!("NoticeSource::Aux) => match filter.source()", ".out_of_vault()");
    /// The clear that ends a Generate failure's life with the draft it
    /// explains. Two needles rather than one spanning a newline, because
    /// this file is checked out with CRLF on Windows and a needle carrying
    /// `\n` would pass here and fail there.
    ///
    /// The binding is named rather than tested inline because
    /// `matches!(mode, DetailMode::Read)` is also the guard on
    /// `RowCommand::Edit`, and a needle that matched both would count two and
    /// say nothing about either.
    const DIES_WITH_THE_DRAFT: &str =
        concat!("let editor_is_closed = matches!(mode, DetailMode::", "Read);");
    const CLEARS_GENERATE: &str = concat!("generate_error = ", "None;");
    /// The condition that SPENDS the binding, and the exact inverse of it.
    ///
    /// Naming the binding pins that the frame knows whether an editor is
    /// open; it says nothing about which way the clear is hung off it. One
    /// `!` here left all 854 tests green while clearing `generate_error` on
    /// exactly the frames where the editor is OPEN -- so the failure is
    /// wiped on the frame it is set, before the band that renders it next
    /// frame ever sees it: the Generate band becomes permanently invisible
    /// AND the stale band this clear exists to end comes straight back.
    const GUARDED_BY: &str = concat!("if editor_is_", "closed {");
    const INVERTED: &str = concat!("if !editor_is_", "closed");

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    /// The text between a `{` that has just been consumed and its matching
    /// `}`.
    ///
    /// Depth-counted rather than "up to the next `}`" for one reason: an
    /// EMPTY block must come back as an empty slice. The guarded block left
    /// empty with the clear moved out below it is the other half of the
    /// polarity dodge -- it clears unconditionally, every frame -- and a
    /// proximity window, or a slice that ran past the closing brace, cannot
    /// tell it from the fix.
    ///
    /// (Neither this doc nor the ones below may SPELL the condition: the
    /// needles are counted over `include_str!("mod.rs")`, which is this
    /// module too, so a literal here would be an extra match. That is the
    /// same rule the `concat!` splits obey, in comment form.)
    fn braced_body(after_open: &str) -> &str {
        let mut depth = 1usize;
        for (at, ch) in after_open.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &after_open[..at];
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces after {GUARDED_BY:?} -- this guard slices the block it opens")
    }

    /// The body of each Generate arm: from the arm's own `=> {` up to the
    /// next arm of the same `match`. Slicing rather than searching the whole
    /// file is the point -- `flag_reauth_if_unauthorized` is still correct
    /// everywhere else in this closure, and a file-wide count could not tell
    /// a Generate arm apart from a Save one.
    fn arm_bodies() -> Vec<&'static str> {
        let source = source();
        source
            .match_indices(ARM)
            .map(|(at, _)| {
                let rest = &source[at..];
                let end = rest.find(NEXT_ARM).unwrap_or_else(|| {
                    panic!(
                        "no {NEXT_ARM:?} after a Generate arm -- these guards slice the arm \
                         body between the two and cannot without it"
                    )
                });
                &rest[..end]
            })
            .collect()
    }

    #[test]
    fn there_are_exactly_two_generate_arms() {
        // The Edit draft's and the Create draft's. This is what makes the
        // "exactly twice" counts below meaningful rather than arbitrary: a
        // third draft form added later fails here first, with the reason.
        assert_eq!(
            source().matches(ARM).count(),
            2,
            "expected {ARM:?} exactly twice -- once in the Edit draft's match and once in \
             the Create draft's. A new one must be wired to report its failures too"
        );
    }

    #[test]
    fn both_arms_route_their_failure_through_the_tested_decision() {
        for body in arm_bodies() {
            assert_eq!(
                body.matches(DECIDES).count(),
                1,
                "a Generate arm does not call {DECIDES:?}. Without it the failure is \
                 log-only again: the password box is unchanged, so nothing on screen \
                 distinguishes a failed generate from a dead button.\n{body}"
            );
            assert_eq!(
                body.matches(REPORTS).count(),
                1,
                "a Generate arm computes a message and never shows it -- {REPORTS:?} is \
                 what puts it in the inline band.\n{body}"
            );
        }
    }

    #[test]
    fn neither_arm_closes_the_window_on_an_expired_session() {
        for body in arm_bodies() {
            assert_eq!(
                body.matches(CLOSES_THE_WINDOW).count(),
                0,
                "a Generate arm calls {CLOSES_THE_WINDOW:?}, which sends \
                 `ViewportCommand::Close`. On a half-filled new-item form that discards \
                 every field the user typed, in exchange for a request that changed \
                 nothing. `GenerateFailure::needs_reauth` is the divergence -- flag the \
                 session, keep the window.\n{body}"
            );
            // Positive control: an arm body that failed to slice (empty, or
            // the wrong region) would satisfy the count above for free.
            assert!(
                body.contains(DECIDES),
                "sliced a Generate arm body that does not call {DECIDES:?} -- the slice \
                 is wrong, so the assertion above proved nothing.\n{body}"
            );
        }
    }

    #[test]
    fn both_arms_record_an_expired_session_for_the_close_to_recover_from() {
        // The other half of `neither_arm_closes_the_window_on_an_expired_
        // session`, and the half that commit's safety argument actually rests
        // on. Dropping `ViewportCommand::Close` is only safe BECAUSE the flag
        // still routes through `open_vault_window`, which reads it
        // unconditionally when the window closes and runs the same recovery a
        // Lock does. Deleting this line from BOTH arms left the whole suite
        // green -- and it is strictly worse than the bug it replaced: an
        // expired session shows its message, keeps the draft, and then closes
        // with no path back to a sign-in at all.
        for body in arm_bodies() {
            assert_eq!(
                body.matches(FLAGS_THE_SESSION).count(),
                1,
                "a Generate arm never sets {FLAGS_THE_SESSION:?}. `generate_failure` \
                 decided `needs_reauth` and the arm dropped it on the floor, so an \
                 expired session is reported and then forgotten: the window closes \
                 without `open_vault_window` ever learning it must re-authenticate.\n{body}"
            );
            // Positive control, as above: a mis-sliced (or empty) body would
            // fail the count rather than pass it, but this says which of the
            // two possible causes it is.
            assert!(
                body.contains(DECIDES),
                "sliced a Generate arm body that does not call {DECIDES:?} -- the slice \
                 is wrong, so the assertion above proved nothing.\n{body}"
            );
        }
    }

    #[test]
    fn the_band_is_fed_by_the_tested_precedence_function() {
        assert_eq!(
            source().matches(BAND_FEED).count(),
            1,
            "expected the item list's band message to come from `inline_notice` exactly \
             once. Zero means the precedence `inline_notice_tests` pins is not the one \
             the window uses"
        );
    }

    /// The band's message is only half the wiring. `inline_notice` also
    /// reports WHICH of the three sources it came from, and the dismissal has
    /// to clear exactly that one -- a fact
    /// `the_band_is_fed_by_the_tested_precedence_function` says nothing
    /// about, because it counts a call and stops there.
    ///
    /// Two mutations were demonstrated against it, both green:
    ///
    ///  * making the Generate arm's dismissal `=> {}` renders the generate
    ///    band **permanently undismissable** -- it is recomputed from the
    ///    same `generate_error` on every subsequent frame, so the click does
    ///    nothing and the band never leaves;
    ///  * restoring the old clear-all dismissal reintroduces the silent
    ///    refetch: waving away a Generate or Move message also clears the
    ///    selected row's `AuxList::error`, which is exactly what makes
    ///    `wants_fetch` true again, so the next frame asks the server on
    ///    behalf of a failure the user never saw.
    #[test]
    fn the_dismissal_clears_exactly_the_source_that_was_on_screen() {
        let source = source();
        let at = source.find(DISMISS_BLOCK).unwrap_or_else(|| {
            panic!("no {DISMISS_BLOCK:?} in this file -- this guard slices the dismissal from it")
        });
        let rest = &source[at..];
        let end = rest
            .find(DISMISS_BLOCK_END)
            .unwrap_or_else(|| panic!("no {DISMISS_BLOCK_END:?} after the dismissal"));
        let block = &rest[..end];
        for (needle, why) in [
            (
                DISMISS_GENERATE,
                "a dismissed Generate band is recomputed from the same `generate_error` \
                 next frame, so the band cannot be waved away at all",
            ),
            (
                DISMISS_MOVE,
                "a dismissed Move band is recomputed from the same `move_error` next \
                 frame, so the band cannot be waved away at all",
            ),
            (
                DISMISS_AUX,
                "the Aux dismissal must be keyed on the SELECTED row's list. Clearing \
                 both -- or clearing all three sources -- is the silent-refetch \
                 regression: clearing `AuxList::error` is what re-arms `wants_fetch`",
            ),
        ] {
            assert_eq!(
                block.matches(needle).count(),
                1,
                "the band's dismissal does not do {needle:?} exactly once -- {why}.\n{block}"
            );
        }
        // Positive control: the block must be keyed on the SOURCE at all. A
        // dismissal that cleared the three unconditionally satisfies none of
        // the needles above, but a future one that matched on something else
        // entirely could.
        assert!(
            block.contains(DISMISS_MATCHES_SOURCE),
            "the dismissal no longer matches on {DISMISS_MATCHES_SOURCE:?}, so it is not \
             clearing the source that was on screen.\n{block}"
        );
    }

    #[test]
    fn a_generate_failure_does_not_outlive_the_draft_it_explains() {
        // `generate_error` was cleared only by the dismissal above and at the
        // start of the next generate. `EditAction::Cancel` and the
        // selection-change reset both left it set, and `inline_notice` ranks
        // Generate ABOVE Move -- so a generate failure the user had walked
        // away from (cancel the form, click another item) kept the band and
        // outranked every later "Couldn't archive ..." until it was clicked
        // away. Never a strand -- one click surfaced the archive message on
        // the next frame -- but the refused archive was invisible for that
        // click, which is the whole of what the band exists to prevent.
        //
        // The fix is one condition on `mode` rather than a clear at each of
        // the five exits, so this guard is three needles: the binding, the
        // condition that spends it, and the clear INSIDE that condition's
        // block. A future edit that deletes any of them, or moves the clear
        // back to enumerating doors, fails here.
        //
        // An earlier form counted the binding and looked for the clear
        // within the next 160 bytes, and so could not tell the fix from its
        // exact inverse. Both of these passed it (neither is spelled out
        // here -- see `braced_body`'s note on why a literal in this module
        // would be an extra match):
        //
        //  * the condition NEGATED -- one character. Clears on exactly the
        //    frames where the editor is OPEN, which is the frame a Generate
        //    failure is set, so the band is never rendered at all and the
        //    stale-band bug comes back with it.
        //  * the condition's block left EMPTY with the clear moved below it
        //    -- same two spellings, same distance apart, clearing
        //    unconditionally every frame.
        let source = source();
        assert_eq!(
            source.matches(DIES_WITH_THE_DRAFT).count(),
            1,
            "nothing clears `generate_error` when the editor closes. A generate failure \
             outranks the Move band, so a stale one hides every later refused \
             Archive/Restore/Purge/Delete for as long as it takes the user to click it \
             away -- on a screen where the row they acted on has not moved, which is the \
             same ambiguity `list_command_failure_message` was written to end."
        );
        // Polarity, stated twice over: the un-negated condition is present
        // exactly once, and the negated one is nowhere. Either count also
        // enforces the `concat!` split -- re-joined, each needle would find
        // itself in the const that defines it.
        assert_eq!(
            source.matches(GUARDED_BY).count(),
            1,
            "the clear is no longer hung off {GUARDED_BY:?}. That condition, un-negated, \
             is the whole of the rule: a Generate failure belongs to an open draft, so it \
             dies on the first frame there is no editor -- and never on the frame it was \
             set, which is what would make the band invisible instead of transient."
        );
        assert_eq!(
            source.matches(INVERTED).count(),
            0,
            "the clear is guarded by {INVERTED:?} -- the exact inverse of the fix. That \
             clears `generate_error` on the frames where the editor is OPEN, i.e. on the \
             frame the failure is set, so the band never renders; and it clears nothing \
             once the draft is gone, so the stale band that outranks every later \
             \"Couldn't archive ...\" is back."
        );
        let at = source.find(DIES_WITH_THE_DRAFT).expect("counted above");
        let opens = at + source[at..].find(GUARDED_BY).expect("counted above");
        let body = braced_body(&source[opens + GUARDED_BY.len()..]);
        assert!(
            body.contains(CLEARS_GENERATE),
            "the {GUARDED_BY:?} block no longer clears the generate error. It is the only \
             thing that condition is for, and the clear must be INSIDE the block: moved \
             out below it, it runs every frame.\n{body}"
        );
    }
}

/// Source-text guards on the edit form's app block, for the one piece of it
/// no test in this crate can drive: the Browse button's other half.
///
/// `EditAction::PickAppFile` is reported by the form and asserted directly
/// (`detail_edit::generator_row_tests::clicking_browse_asks_the_caller_to_open_
/// the_file_dialog`). What happens next lives in `run`'s update closure, which
/// needs a real event loop, and it ends in `IFileOpenDialog::Show` -- a modal
/// dialog that would sit there waiting for a person. So the arm is pinned by
/// source position, exactly as `refused_write_wiring_tests` pins the arms it
/// cannot run.
///
/// **What this cannot see:** that the dialog's answer is any good, or that the
/// arm runs at the right moment. What it catches is the arm being deleted or
/// gutted -- which would leave a Browse button that reports an action nobody
/// acts on, i.e. a button that does nothing, with the whole suite green.
#[cfg(test)]
mod app_block_wiring_tests {
    // EVERY NEEDLE IS SPLIT WITH `concat!` and is a single line with no line
    // ending, for `refused_write_wiring_tests`' reasons: `include_str!("mod.rs")`
    // pulls this module in too, so an unsplit needle matches its own
    // declaration, and a needle carrying a newline passes on one checkout's
    // line endings and fails on the other's.
    const PICK_ARM: &str = concat!("EditAction::PickApp", "File => {");
    const OPENS_THE_DIALOG: &str = concat!("file_picker::pick_", "executable()");
    const HANDS_IT_BACK: &str = concat!("draft.set_app_", "path(&path)");
    /// The cache that makes the name-and-icon lookup happen once rather than
    /// once a frame -- it has to be the SAME one across frames, so it is
    /// passed in from `run`'s own state. Twice: the edit form and the create
    /// form both take it.
    const PASSES_THE_CACHE: &str = concat!("&mut app_", "identities,");

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn the_counter_finds_needles_that_are_really_there() {
        // The positive control, driving the same counting the assertions use
        // rather than a re-implementation of it.
        let planted = concat!("match x { EditAction::PickApp", "File => { pick(); } }");
        assert_eq!(occurrences(planted, PICK_ARM), 1, "planted: {planted}");
        assert_eq!(occurrences("nothing here", PICK_ARM), 0);
        assert_eq!(occurrences("nothing here", OPENS_THE_DIALOG), 0);
        assert_eq!(occurrences("nothing here", HANDS_IT_BACK), 0);
    }

    #[test]
    fn browse_opens_the_file_dialog_and_the_answer_reaches_the_draft() {
        assert_eq!(
            occurrences(source(), PICK_ARM),
            1,
            "expected {PICK_ARM:?} exactly once -- the edit form's Browse arm. Zero means the \
             form still reports the action and nobody acts on it: the button opens nothing, \
             and every test of the button keeps passing because the button really does report"
        );
        assert_eq!(
            occurrences(source(), OPENS_THE_DIALOG),
            1,
            "expected {OPENS_THE_DIALOG:?} exactly once -- nothing else in this app opens a \
             file dialog, and an arm that does not open one is a dead Browse button"
        );
        assert_eq!(
            occurrences(source(), HANDS_IT_BACK),
            1,
            "expected {HANDS_IT_BACK:?} exactly once -- the chosen path must reach the draft \
             through `set_app_path`, which is what derives `process` from it and so keeps \
             `AppMatch::launchable_path`'s file-name tie-back satisfiable. An arm that \
             assigned `draft.app.path` directly would store a path the launcher refuses"
        );
    }

    #[test]
    fn every_pane_that_names_an_app_is_handed_the_one_identity_cache() {
        assert_eq!(
            occurrences(source(), PASSES_THE_CACHE),
            3,
            "expected {PASSES_THE_CACHE:?} exactly three times -- once per draft editor, and \
             once for the READ pane, whose MATCHED APP card now shows what the bound app is \
             really called. Constructing \
             a cache inside the frame closure instead would resolve the matched app's name and \
             re-upload its icon on EVERY repaint, which is the per-frame I/O `app_identity` \
             exists to prevent"
        );
    }
}

/// Source-text guards on the three arms a refused write used to leave silent:
/// `DetailAction::ToggleFavorite`, and `EditAction::Save` in each of the two
/// draft editors.
///
/// Same reason as [`generate_failure_wiring_tests`], which is the precedent
/// these follow line for line: the arms live inside `run`'s update closure,
/// which needs a real event loop and is unreachable from this suite. The
/// decision they carry lives in `item_write_failure_message`, where
/// `item_write_failure_tests` drives it directly; what is left in the arms is
/// plumbing, and this pins the plumbing.
///
/// **What they do not guarantee**, stated so the doc claims no coverage it
/// lacks: they see spellings and counts, not behaviour. A `move_error` set
/// here and never rendered would pass -- `BAND_FEED` in the module above is
/// what pins that the band is fed at all. What they catch is the edit that
/// puts a swallowed error back, which is the regression this whole finding is.
#[cfg(test)]
mod refused_write_wiring_tests {
    // EVERY NEEDLE IS SPLIT WITH `concat!`, for the reason spelled out in
    // `generate_failure_wiring_tests`: `include_str!("mod.rs")` pulls this
    // module in too, so a needle written as one literal always matches itself.
    // The count assertions ENFORCE the split -- re-joining one makes it appear
    // an extra time and fails.
    const FAV_ARM: &str = concat!("DetailAction::ToggleFavorite(to)", " => {");
    const FAV_NEXT: &str = concat!("DetailAction::CopyPassword", "History(index) =>");
    const SAVE_ARM: &str = concat!("EditAction::Save", " => {");
    const SAVE_NEXT: &str = concat!("EditAction::Generate", "Password =>");
    /// One needle for both halves of the fix: it is the assignment that puts a
    /// sentence in the band AND the call that decides the sentence. An arm
    /// that computed a message and dropped it, or set the band from a literal,
    /// fails on this.
    const REPORTS: &str = concat!("move_error = Some(item_write_failure", "_message(");
    const FAVOURITE_ON: &str = concat!("ItemWrite::", "Favorite");
    const FAVOURITE_OFF: &str = concat!("ItemWrite::", "Unfavorite");
    const SAVE_KIND: &str = concat!("ItemWrite::", "Save");
    const CREATE_KIND: &str = concat!("ItemWrite::", "Create");
    /// The drop that stops a Generate failure earlier in the SAME draft from
    /// outranking the refused save (`inline_notice` ranks Generate above
    /// Move), which the `editor_is_closed` clear cannot do because a failed
    /// save leaves the editor open.
    const DROPS_THE_OUTRANKING_ONE: &str = concat!("generate_error = ", "None;");

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    /// The text of each arm opened by `arm`, up to the next arm of the same
    /// `match`. Slicing rather than searching the whole file is the point:
    /// `move_error = Some(...)` is correct in a dozen other places in this
    /// closure, and a file-wide count could not tell those from these.
    fn bodies(arm: &str, next: &str) -> Vec<&'static str> {
        let source = source();
        source
            .match_indices(arm)
            .map(|(at, _)| {
                let rest = &source[at..];
                let end = rest.find(next).unwrap_or_else(|| {
                    panic!(
                        "no {next:?} after {arm:?} -- these guards slice the arm body between \
                         the two and cannot without it"
                    )
                });
                &rest[..end]
            })
            .collect()
    }

    #[test]
    fn there_is_one_favourite_arm_and_two_save_arms() {
        // What makes the counts below meaningful rather than arbitrary: a
        // third draft form, or a second favourite door, fails here first and
        // says it must be wired to report its failures too.
        assert_eq!(source().matches(FAV_ARM).count(), 1, "expected {FAV_ARM:?} exactly once");
        assert_eq!(
            source().matches(SAVE_ARM).count(),
            2,
            "expected {SAVE_ARM:?} exactly twice -- the Edit draft's and the Create draft's"
        );
    }

    #[test]
    fn the_favourite_arm_reports_a_refusal_in_the_direction_it_was_asked() {
        let body = bodies(FAV_ARM, FAV_NEXT).remove(0);
        assert_eq!(
            body.matches(REPORTS).count(),
            1,
            "the favourite arm does not call {REPORTS:?}. Without it the failure is log-only \
             again -- the star is painted from this window's own copy of the item, which a \
             refused write does not change, so a refused toggle and an accepted one look \
             identical. That is the user's report.\n{body}"
        );
        // Both directions, because one variant serving both is exactly how
        // "It still isn't one" ends up on a failed un-favourite.
        assert_eq!(
            body.matches(FAVOURITE_ON).count(),
            1,
            "the favourite arm does not name {FAVOURITE_ON:?}\n{body}"
        );
        assert_eq!(
            body.matches(FAVOURITE_OFF).count(),
            1,
            "the favourite arm does not name {FAVOURITE_OFF:?}, so one direction of the \
             toggle reports the other direction's sentence\n{body}"
        );
    }

    #[test]
    fn both_save_arms_report_a_refusal_under_their_own_kind() {
        let bodies = bodies(SAVE_ARM, SAVE_NEXT);
        assert_eq!(bodies.len(), 2, "counted above");
        for body in &bodies {
            assert_eq!(
                body.matches(REPORTS).count(),
                1,
                "a Save arm does not call {REPORTS:?}. The editor is left exactly as it was \
                 on failure, so nothing else on screen says the write did not land.\n{body}"
            );
            assert_eq!(
                body.matches(DROPS_THE_OUTRANKING_ONE).count(),
                1,
                "a Save arm does not drop {DROPS_THE_OUTRANKING_ONE:?} first. A generate \
                 failure earlier in this same draft outranks this band and the editor is \
                 still open, so the `editor_is_closed` clear cannot reach it -- the refused \
                 save is invisible behind a message about a box the user has moved on \
                 from.\n{body}"
            );
        }
        // EACH ARM UNDER ITS OWN KIND, identified by the write it makes and
        // not by its position. "One of each between them" was the first
        // version of this, and swapping the two kinds survived it -- which
        // would tell a user editing an existing item that nothing was added to
        // their vault, and a user creating one that their unsaved edits are
        // safe in a form that is about to be discarded.
        for body in &bodies {
            let (makes, kind, other) = if body.contains(concat!("cache.update_", "item(")) {
                (concat!("cache.update_", "item("), SAVE_KIND, CREATE_KIND)
            } else {
                (concat!("cache.create_", "item("), CREATE_KIND, SAVE_KIND)
            };
            assert!(
                body.contains(kind),
                "the arm that calls {makes:?} does not report under {kind:?}\n{body}"
            );
            assert!(
                !body.contains(other),
                "the arm that calls {makes:?} reports under {other:?}, which is the other \
                 editor's sentence\n{body}"
            );
        }
    }

    #[test]
    fn the_slices_are_the_arms_they_claim_to_be() {
        // POSITIVE CONTROL for all three tests above: an arm body that failed
        // to slice -- empty, or the wrong region -- would satisfy a `count()`
        // assertion for free and satisfy `(1, 1)` by accident. Each body must
        // contain the call that is the reason the arm exists at all.
        assert!(
            bodies(FAV_ARM, FAV_NEXT)[0].contains(concat!("cache.set_", "favorite(")),
            "sliced something that is not the favourite arm"
        );
        for body in bodies(SAVE_ARM, SAVE_NEXT) {
            assert!(
                body.contains(concat!("cache.update_", "item("))
                    || body.contains(concat!("cache.create_", "item(")),
                "sliced something that is not a Save arm\n{body}"
            );
        }
    }
}

/// The plumbing that reaches the Trash/Archive feature's pure decisions.
///
/// Those decisions -- `sidebar::menu_entries`, `items_for`, `badge_for`,
/// `badge_text`, `AuxList::wants_fetch`, `SidebarFilter::source` -- are
/// genuinely well tested. **The wiring that carries a value from the sidebar
/// filter to each of them was not**, and all of it lives inside `run`'s
/// update closure, which no test in this crate can call. A reviewer applied
/// five separate mutations to it, each alone, and the entire suite stayed
/// green through every one:
///
///  * the item pane read `&items` under Trash and Archive -- both rows list
///    nothing, which is the original defect the feature was written to fix;
///  * the detail pane's `out_of_vault` became a literal `None` -- a trashed
///    item opens the ordinary Read pane offering Edit, Fill, Delete and the
///    copy rows, every one of them a no-op;
///  * the aux fetch's `selected` argument became `false` -- neither list is
///    ever fetched, so both rows are a permanent en dash over an empty pane
///    and the feature ships 100% dead;
///  * the four per-command `invalidate()` calls were deleted -- a restored
///    item keeps sitting in Trash, and keeps being counted there.
///
/// (The fifth, the row menu's `filter.source()`, is closed behaviourally by
/// `item_list::row_tile_tests`, which paints the real menu under a real
/// Trash filter and clicks a real entry in it.)
///
/// These are source-text guards for the same reason
/// `generate_failure_wiring_tests` and `window_era_placement_tests` are: the
/// code they cover is unreachable from a test, so the alternative is not a
/// better test but no test. **Every needle is split with `concat!`**, because
/// `include_str!("mod.rs")` pulls this module in too -- a needle written as
/// one literal always matches, inside the const that defines it, which is how
/// several guards in this file once passed with their regression live. The
/// count assertions enforce the split rather than this comment.
#[cfg(test)]
mod out_of_vault_wiring_tests {
    /// The `Option`-preserving selection: `run`'s `shown`, plus `list_for`'s
    /// own delegation to it.
    const SELECTS_A_LIST: &str = concat!("list_unless_", "unfetched(");
    /// The flattening, called by the two sites that only need something to
    /// search. Neither needle matches its own `fn` item, which carries a
    /// lifetime parameter between the name and the paren.
    const FLATTENED: &str = concat!("list_", "for(");
    const DEFINES_SELECTS: &str = concat!("fn list_unless_unfetched", "<'a>(");
    const DEFINES_FLATTENED: &str = concat!("fn list_", "for<'a>(");
    /// The one place a `FilterSource` may still be taken apart by hand.
    const INLINE_MATCH: &str = concat!("sidebar::FilterSource::", "Trash =>");
    /// Every argument a selection site passes, **in the order it must pass
    /// them**. See `every_selection_site_passes_the_arguments_it_was_given`
    /// for why pinning the call alone was not enough.
    const SELECTION_ARGUMENTS: [&str; 4] = [
        concat!("filter.sou", "rce(),"),
        concat!("&it", "ems,"),
        concat!("trash_list.items.as_", "deref(),"),
        concat!("archive_list.items.as_", "deref(),"),
    ];
    /// The item pane's own call, pinned by the BINDING it produces and not
    /// merely by the function's name.
    const SHOWN_BINDING: &str =
        concat!("let shown: Option<&[VaultItem]> = list_unless_", "unfetched(");
    /// Where an argument list stops: the call's own closing paren for the
    /// `shown` binding, and the `.iter()` both `list_for` sites chain onto
    /// theirs.
    const ARGUMENTS_END: &str = ");";
    const CHAINED_ARGUMENTS_END: &str = concat!(".it", "er()");
    /// The two selection helpers as BARE names, for the shadowing guard.
    /// Every needle above pins one of these names; none of them pins the
    /// function it resolves to.
    const SELECTION_HELPERS: [&str; 2] = [
        concat!("list_unless_", "unfetched"),
        concat!("list_", "for"),
    ];
    /// The only three things that may follow one of those names in
    /// production: a call's own paren, the definition's lifetime parameter,
    /// and the closing backtick of a doc link. Every way of BINDING the
    /// name instead -- `use ...::name;`, `use {name}`, `use x as name;`,
    /// `let name = ...`, a local `fn name(` -- leaves something else there,
    /// or is caught by the call counts above.
    const HELPER_MAY_BE_FOLLOWED_BY: [char; 3] = ['(', '<', '`'];
    /// The one shadowing form that never spells the name at all.
    const GLOB_IMPORT: &str = concat!("::", "*;");
    /// The detail pane's out-of-vault branch: the derivation, and the
    /// condition that reads it.
    const DERIVES_OUT_OF_VAULT: &str =
        concat!("let out_of_vault = filter.source()", ".out_of_vault();");
    const PANE_BRANCH: &str =
        concat!("_ if out_of_vault.is_some() && ", "selected_item.is_some() =>");
    const PANE_CALL: &str = concat!("detail::draw_out_of_", "vault_read(");
    /// The aux fetch's own "is this row the one on screen?".
    const SELECTED_SOURCE: &str =
        concat!("let selected_source = filter.source()", ".out_of_vault();");
    const WANTS_FETCH: &str = concat!("list.wants_fetch(selected_source == ", "Some(which))");

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    /// Production code only. Every needle here describes a decision `run`
    /// makes; the test modules below legitimately spell the same things while
    /// exercising them. Same marker and same reasoning as
    /// `window_era_placement_tests::production`, which also states why
    /// slicing at the FIRST `#[cfg(test)]` is sound.
    fn production() -> &'static str {
        let source = source();
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file -- see `window_era_placement_tests`");
        &source[..end]
    }

    #[test]
    fn every_list_selection_goes_through_the_one_tested_function() {
        // Three call sites, and they are the whole of the feature's reach
        // into `run`: the list the item pane draws, the item a selection
        // resolves to, and the item a right-click command acts on. Each was
        // its own hand-written `match`, and mutating the first alone left
        // the suite green.
        assert_eq!(
            production().matches(SELECTS_A_LIST).count(),
            2,
            "expected {SELECTS_A_LIST:?} twice in production code: `run`'s `shown`, \
             which must keep the unfetched/empty distinction so the search placeholder \
             can tell them apart, and `list_for`'s own delegation to it. Fewer means \
             one of them decides for itself again -- which is how the item pane came to \
             filter `items` under Trash and list nothing."
        );
        assert_eq!(
            production().matches(FLATTENED).count(),
            2,
            "expected {FLATTENED:?} twice in production code: the selection lookup that \
             feeds the detail pane, and the lookup that resolves a right-clicked row to \
             the item its command acts on. Both must read the list the row was DRAWN \
             from -- resolved against `items`, a trashed row finds nothing and every \
             entry on its menu is a click that does nothing."
        );
        // Positive controls for the two counts above: each is satisfied for
        // free if the function it names has been deleted and its call sites
        // now reach something else spelled the same way.
        for definition in [DEFINES_SELECTS, DEFINES_FLATTENED] {
            assert_eq!(
                production().matches(definition).count(),
                1,
                "{definition:?} is not defined exactly once in production code -- it was \
                 renamed or removed, so the counts above are measuring something else"
            );
        }
    }

    #[test]
    fn no_call_site_takes_a_filter_source_apart_by_hand_any_more() {
        // The mutation this catches is the re-inlining: a future edit that
        // spells the three-way match out at a call site again compiles, is
        // invisible to the counts above (which only notice a MISSING call),
        // and reintroduces exactly the divergence between call sites that
        // `list_unless_unfetched` exists to make impossible.
        assert_eq!(
            production().matches(INLINE_MATCH).count(),
            1,
            "a `FilterSource` is matched on outside `list_unless_unfetched`. That \
             decision belongs in one place: three copies of it is what the item pane, \
             the selection lookup and the row-command lookup each had, and only one of \
             them had to be wrong."
        );
    }

    /// One selection site's argument list, checked to pass
    /// [`SELECTION_ARGUMENTS`] in order.
    ///
    /// `site` names it in the panic; `call` is the slice from the call's own
    /// opening needle to its terminator.
    fn arguments_in_order(site: &str, call: &str) {
        let mut at = 0;
        for argument in SELECTION_ARGUMENTS {
            let found = call[at..].find(argument).unwrap_or_else(|| {
                panic!(
                    "{site} does not pass {argument:?}, or passes it out of order. \
                     `list_unless_unfetched` takes the source first and then the three \
                     lists positionally, so a wrong or transposed argument compiles, \
                     counts, and takes nothing apart by hand -- which is why the two \
                     guards above see none of it.\n{call}"
                )
            });
            at += found + argument.len();
        }
    }

    /// Slices `count` argument lists out of production code: from each
    /// occurrence of `opens` to the next `ends`.
    fn argument_lists(opens: &str, ends: &str, count: usize) -> Vec<&'static str> {
        let source = production();
        let calls: Vec<&'static str> = source
            .match_indices(opens)
            .map(|(at, _)| {
                let rest = &source[at..];
                let end = rest.find(ends).unwrap_or_else(|| {
                    panic!("no {ends:?} after {opens:?} -- this guard slices to it")
                });
                &rest[..end]
            })
            .collect();
        assert_eq!(
            calls.len(),
            count,
            "expected {count} occurrence(s) of {opens:?} in production code, found {}. \
             The counts above measure the same sites; if they still pass, this needle \
             is stale rather than the code being wrong",
            calls.len()
        );
        calls
    }

    /// The index of the `)` that closes a `(` which has just been consumed.
    ///
    /// Depth-counted, because the argument lists here nest three levels
    /// (`trash_list.items.as_deref()`), and the point is to land on the
    /// call's OWN closing paren rather than on any `);` after it.
    fn matching_paren(site: &str, after_open: &str) -> usize {
        let mut depth = 1usize;
        for (at, ch) in after_open.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return at;
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced parens after {site} -- this guard slices the call it opens")
    }

    #[test]
    fn neither_selection_helper_can_be_shadowed_out_from_under_its_callers() {
        // EVERY NEEDLE IN THIS MODULE PINS A NAME, NOT A FUNCTION. Adding a
        // `pub fn list_unless_unfetched` to `sidebar.rs` that always answers
        // `Some(live)`, plus a scope-local `use sidebar::list_unless_
        // unfetched;` immediately above the `shown` binding, redirects the
        // item pane's call for the whole of `run` -- the inner import
        // shadows the module-level fn, `DEFINES_SELECTS` still finds the
        // real definition, every count and every argument above still holds,
        // and the suite stays green. The two `list_for` sites take the same
        // edit.
        //
        // A shadow has to BIND the bare name, and no binding form puts a
        // call's `(` or the definition's `<'a>` immediately after it. So the
        // check is the complement: in production, every occurrence of either
        // name is a call, the definition, or a doc link. A local `fn name(`
        // slips past that one and is caught instead by the call counts in
        // `every_list_selection_goes_through_the_one_tested_function`, which
        // would see three occurrences where they require two.
        for name in SELECTION_HELPERS {
            for (at, _) in production().match_indices(name) {
                let next = production()[at + name.len()..].chars().next();
                assert!(
                    next.is_some_and(|c| HELPER_MAY_BE_FOLLOWED_BY.contains(&c)),
                    "{name:?} appears in production followed by {next:?}, which is neither \
                     a call, the definition, nor a doc link -- so it is a BINDING of that \
                     name, and every guard in this module that pins the name now pins the \
                     binding instead of the function. Here: {:?}",
                    production()[at..].chars().take(48).collect::<String>()
                );
            }
        }
        // The one shadowing form the loop above cannot see, because it never
        // spells either name: a glob import in an inner scope shadows the
        // module's own items for the rest of that scope. Production has none
        // and does not need one.
        assert_eq!(
            production().matches(GLOB_IMPORT).count(),
            0,
            "production code has a glob import. A `use ...::*;` inside `run` shadows this \
             module's own `list_unless_unfetched`/`list_for` for the rest of that scope, \
             which is the one redirection the name check above cannot see."
        );
    }

    #[test]
    fn every_selection_site_passes_the_arguments_it_was_given() {
        // THE TWO GUARDS ABOVE PIN THAT A CALL HAPPENS, NOT WHAT IT SAYS.
        // Three mutations were demonstrated against them, each alone, each
        // leaving the whole suite green:
        //
        //  * the item pane's first argument replaced with a literal
        //    `sidebar::FilterSource::LiveVault`. The call still counts and
        //    nothing is taken apart by hand, but the pane draws the LIVE
        //    VAULT under Trash and Archive: the Trash row lists the user's
        //    entire vault, and clicking a row leaves the pane on "Select an
        //    item." because the two `list_for` sites still resolve against
        //    the real trash list. That is the original defect, restored.
        //  * the trash and archive arguments transposed in that same call.
        //    Trash lists the archived items, Archive the trashed ones, and
        //    every menu entry on those rows is a silent no-op.
        //  * the real call kept as a `let _ =` so the count still sees two,
        //    with a hand-rolled `match` behind a local `use ... as FS` alias
        //    feeding `shown` instead -- invisible to a needle spelled
        //    `sidebar::FilterSource::Trash =>`.
        //
        // Pinning the ARGUMENT LIST kills all three: the first two because a
        // wrong or transposed argument is not the text that must appear, the
        // third because the item pane's site is pinned by the BINDING it
        // produces, so a `let _ =` in its place is not that site at all.
        //
        // WHAT THIS STILL DOES NOT COVER, stated rather than glossed --
        // this file has twice carried a comment claiming more than the code
        // did:
        //
        //  * a rebind. `let shown = ...` a second time, after the pinned
        //    call, shadows it and satisfies every needle here.
        //  * `production()` reads `include_str!("mod.rs")` only, so a
        //    hand-rolled decision moved into `sidebar.rs` or `item_list.rs`
        //    and called from here is out of reach of `INLINE_MATCH`. All
        //    three sites here are pinned by NAME -- the item pane's by the
        //    binding it produces, the other two by `list_for` -- and a name
        //    is not a function: a same-named helper in another module,
        //    imported into scope, satisfies all of them.
        //    `neither_selection_helper_can_be_shadowed_out_from_under_its_callers`
        //    closes that second half by forbidding any BINDING of either
        //    name in production; the hand-rolled-decision half stays open,
        //    because a helper `sidebar.rs` exports under a DIFFERENT name
        //    and this file merely calls is out of reach of every needle in
        //    this module.
        //
        // Closing either needs a real behavioural test, and there is none to
        // be had: all three sites are inside `run`'s update closure, which
        // needs an event loop and cannot be called from this suite. That is
        // the same reason `generate_failure_wiring_tests` exists. The paint
        // harness `item_list::row_tile_tests` uses reaches `draw_item_list`
        // and the row menu -- it is what closes the row menu's own
        // `filter.source()` behaviourally -- but it cannot reach the caller
        // that decides which list `draw_item_list` is handed, which is
        // precisely the decision mutated above.
        arguments_in_order(
            "the item pane's `shown`",
            argument_lists(SHOWN_BINDING, ARGUMENTS_END, 1)[0],
        );
        // AND THAT THE CALL IS THE WHOLE OF THE BINDING. `ARGUMENTS_END`
        // stops at the call's own `);`, so anything CHAINED onto the result
        // sits outside every needle above while all four still match, in
        // order -- a fourth dodge past the pin:
        //
        //     )
        //     .or(Some(items.as_slice()));
        //
        // which is the obvious wrong "fix" for a momentarily blank pane and
        // makes Trash and Archive list the user's ENTIRE LIVE VAULT for
        // every frame before the on-demand fetch lands: the original defect,
        // restored for exactly the window `list_unless_unfetched` returns
        // `None` to describe.
        let at = production()
            .find(SHOWN_BINDING)
            .expect("`argument_lists` counted this exactly once just above");
        let after = &production()[at + SHOWN_BINDING.len()..];
        let close = matching_paren("the item pane's `shown`", after);
        let tail = after[close + 1..].trim_start();
        assert!(
            tail.starts_with(';'),
            "the item pane's `shown` chains onto `list_unless_unfetched`'s answer instead \
             of being it. Everything past the call's closing paren is outside every needle \
             above, and the `None` it may return is the search placeholder's only way to \
             tell \"nothing in this list\" from \"no answer yet\" -- so a `.or(...)` here \
             lists the live vault under Trash and Archive until the fetch lands. Found: \
             {:?}",
            tail.chars().take(60).collect::<String>()
        );
        // The selection lookup that feeds the detail pane, and the lookup
        // that resolves a right-clicked row to the item its command acts on.
        // Both take the same four arguments in the same order, and both are
        // as transposable as the one above: swapped here, every entry on a
        // trashed row's menu acts on an archived item.
        for call in argument_lists(FLATTENED, CHAINED_ARGUMENTS_END, 2) {
            arguments_in_order("a `list_for` lookup", call);
        }
    }

    #[test]
    fn the_detail_panes_out_of_vault_branch_is_derived_from_the_live_filter() {
        // Replacing the derivation with a literal `None` -- one line -- left
        // the suite green while a trashed item opened the ordinary Read pane
        // with Edit, Fill, Delete, the copy rows and the favourite star, all
        // of which read or write through the live list and so do nothing.
        //
        // The pre-existing guard only proved `draw_out_of_vault_read(`
        // appeared SOMEWHERE in the file, and said nothing about the
        // condition that reaches it, which is why that mutation passed.
        for (needle, why) in [
            (
                DERIVES_OUT_OF_VAULT,
                "the branch condition is no longer derived from the live filter. A \
                 constant here compiles and is invisible: the pane simply stops being \
                 reached, and a trashed item gets the live Read pane instead",
            ),
            (
                PANE_BRANCH,
                "the out-of-vault arm's condition changed. It must be reached for \
                 exactly a row outside the live vault that has a selection",
            ),
            (
                PANE_CALL,
                "nothing calls the out-of-vault pane -- the branch is there and inert",
            ),
        ] {
            assert_eq!(
                production().matches(needle).count(),
                1,
                "expected {needle:?} exactly once in production code -- {why}"
            );
        }
    }

    #[test]
    fn the_aux_fetch_asks_only_for_the_row_that_is_selected() {
        // `wants_fetch(false)` is a one-word mutation that ships the feature
        // 100% dead: neither list is ever fetched, so both rows sit at a
        // permanent en dash over an empty pane. The suite stayed green
        // because `wants_fetch` itself is pure and perfectly tested -- it is
        // the ARGUMENT that was never pinned.
        for (needle, why) in [
            (
                SELECTED_SOURCE,
                "the selected row is no longer resolved to an `OutOfVault` before the \
                 fetch loop, so nothing can tell the loop which list is on screen",
            ),
            (
                WANTS_FETCH,
                "the fetch loop does not ask `wants_fetch` about the SELECTED row. A \
                 constant `false` never fetches either list; a constant `true` fetches \
                 both on the first frame, which is the per-visit cost this design \
                 exists to avoid",
            ),
        ] {
            assert_eq!(
                production().matches(needle).count(),
                1,
                "expected {needle:?} exactly once in production code -- {why}"
            );
        }
    }

    /// The aux spawn's era argument -- `window_era`, the one value captured
    /// before the loop, not a fresh read.
    const AUX_SPAWN_ERA: &str = concat!("load_generation, window", "_era, aux_tx");

    #[test]
    fn the_aux_fetch_is_checked_against_the_windows_one_era() {
        // `load_generation` alone cannot answer this. It is a SPAWN TAG --
        // incremented when a vault load is spawned, untouched by
        // `VaultCache::clear` -- so a fetch outstanding across a clear and a
        // re-populate under a DIFFERENT ACCOUNT comes back carrying the tag
        // it left with, matches, and is applied.
        //
        // `window_era` is the value that answers it, and it must be the one
        // captured before the loop rather than a fresh `cache.epoch().era()`
        // here: a per-spawn read equals the current era by construction and
        // proves nothing, which is the whole subject of
        // `window_era_placement_tests` (whose "read exactly once" count is
        // also what stops this spawn from acquiring its own).
        assert_eq!(
            production().matches(AUX_SPAWN_ERA).count(),
            1,
            "the on-demand fetch is not tagged with `window_era`. Without it the only \
             guard on the result is `load_generation`, which a `clear` does not touch -- \
             so a list fetched under one account can be applied under another's chrome. \
             The check itself lives in `VaultCache::list_trash_unless_superseded`."
        );
    }

    /// The **five** commands that move an item between this window's three
    /// lists: the arm marker, the needle proving it drops the on-demand list
    /// it moved an item into or out of, and the needle proving a refusal
    /// reaches the user.
    ///
    /// The list is not cached anywhere, so refetching it is the cheap,
    /// always-correct answer -- but only if the command actually asks. After
    /// a Restore that does not, the restored item keeps sitting in Trash and
    /// keeps being counted there for the life of the window, because
    /// `wants_fetch` sees a list already in hand.
    ///
    /// **It listed four until a reviewer added the fifth and watched both
    /// guards fail.** Delete is a SOFT delete -- `VaultCache::delete_item`
    /// sends no `permanent=true` -- so it moves an item out of the live vault
    /// and into the Trash, and it honoured neither rule. THIS ARRAY IS AN
    /// ASSERTION INPUT, not a summary: an entry missing from it silently
    /// narrows every test that reads it, which is the same way
    /// `MENU_VOCABULARY` once filtered a real menu entry out of both sides of
    /// an "exactly these entries" comparison.
    ///
    /// Delete's two needles are the delegation rather than the deed, because
    /// its deed is in `delete_vault_item`: it has TWO doors (this menu and
    /// the detail pane's kebab), so writing the invalidation and the message
    /// inline here would have covered one of them. What this array pins for
    /// Delete is that the arm hands the helper the list to drop and routes
    /// the sentence it returns to the band;
    /// `the_soft_delete_is_wired_at_both_of_its_doors` pins the helper itself
    /// and the other door.
    const COMMAND_ARMS: [(&str, &str, &str); 5] = [
        (
            concat!("RowCommand::Del", "ete => {"),
            concat!("&mut trash_", "list,"),
            concat!("move_error = Some(mes", "sage);"),
        ),
        (
            concat!("RowCommand::Arch", "ive => {"),
            concat!("archive_list.inval", "idate();"),
            REPORTS_THE_FAILURE,
        ),
        (
            concat!("RowCommand::Unarch", "ive => {"),
            concat!("archive_list.inval", "idate();"),
            REPORTS_THE_FAILURE,
        ),
        (
            concat!("RowCommand::Rest", "ore => {"),
            concat!("trash_list.inval", "idate();"),
            REPORTS_THE_FAILURE,
        ),
        (
            concat!("RowCommand::PurgeFor", "ever => {"),
            concat!("trash_list.inval", "idate();"),
            REPORTS_THE_FAILURE,
        ),
    ];

    /// Where an arm body stops. The four arms are consecutive, so each ends
    /// at the next `RowCommand::` of the same `match`; the last ends at the
    /// panel that follows the whole block.
    const NEXT_ARM: &str = concat!("item_list::Row", "Command::");
    const AFTER_LAST_ARM: &str = concat!("egui::CentralPanel", "::default()");

    /// The body of one command arm, from its own `=> {` to the start of the
    /// next arm.
    ///
    /// **Slicing is the entire point of this guard.** The pre-existing
    /// `both_on_demand_lists_are_dropped_when_the_vault_reloads` searches the
    /// WHOLE file for its two needles, and the two calls at the vault-reload
    /// site satisfy it on their own -- so all four per-command invalidations
    /// were removable with the suite still green. A needle that must appear
    /// within each arm's own slice is the shape that works. Same idiom as
    /// `generate_failure_wiring_tests::arm_bodies`.
    fn arm_body(marker: &str) -> &'static str {
        let source = production();
        let at = source
            .find(marker)
            .unwrap_or_else(|| panic!("no {marker:?} arm in this file"));
        let rest = &source[at + marker.len()..];
        let end = rest
            .find(NEXT_ARM)
            .or_else(|| rest.find(AFTER_LAST_ARM))
            .unwrap_or_else(|| {
                panic!(
                    "no {NEXT_ARM:?} or {AFTER_LAST_ARM:?} after the {marker:?} arm -- \
                     this guard slices the arm body up to one of them and cannot without it"
                )
            });
        &rest[..end]
    }

    #[test]
    fn each_list_moving_command_drops_the_list_it_moved_an_item_out_of() {
        for (marker, invalidates, _) in COMMAND_ARMS {
            let body = arm_body(marker);
            assert_eq!(
                body.matches(invalidates).count(),
                1,
                "the {marker:?} arm does not call {invalidates:?}. That on-demand list is \
                 not cached anywhere and is never pruned in place, so without this the row \
                 keeps showing -- and counting -- an item that is no longer in it, for the \
                 life of the window: `wants_fetch` sees a list already in hand and never \
                 asks again.\n{body}"
            );
        }
    }

    /// What each of the four inline arms must put in front of the user when
    /// its write is refused. Same slicing as the invalidation guard above,
    /// and for the same reason -- a file-wide search is satisfied by any ONE
    /// arm having it. (Delete's own needle is in `COMMAND_ARMS`: it builds
    /// its sentence in `delete_vault_item` and the arm only forwards it.)
    const REPORTS_THE_FAILURE: &str =
        concat!("move_error = Some(list_command_", "failure_message(");

    #[test]
    fn each_list_moving_command_says_so_when_it_is_refused() {
        // All of them were `log::warn!` plus the re-auth flag and nothing
        // else, on the same screen whose branch already routes aux-FETCH
        // failures through the inline band. A refused write looked exactly
        // like a successful one that had not refreshed yet, and the user's
        // only signal was the item still sitting where they had just told it
        // to leave. Archive is the one where rejection is genuinely likely
        // rather than theoretical: re-archiving returns 400. Delete was left
        // in that state for a whole commit after the other four were fixed,
        // because it was not counted as a list-moving command at all.
        for (marker, _, reports) in COMMAND_ARMS {
            let body = arm_body(marker);
            assert_eq!(
                body.matches(reports).count(),
                1,
                "the {marker:?} arm does not report its failure with {reports:?}. \
                 `flag_reauth_if_unauthorized` covers only an expired session, and a \
                 `log::warn!` is not a user-visible channel -- the band under the \
                 toolbar is.\n{body}"
            );
        }
    }

    /// `delete_vault_item`'s body: where the soft delete's invalidation and
    /// its failure sentence actually live.
    const DEFINES_DELETE: &str = concat!("fn delete_vault", "_item(");
    /// The `fn` that follows it, which is where its body stops.
    const AFTER_DELETE: &str = concat!("fn move_item_into", "_folder(");
    /// Both doors' shared spelling of "route the sentence to the band".
    const DELETE_REPORTS: &str = concat!("move_error = Some(mes", "sage);");
    /// The helper's name at a call site or a definition alike.
    const CALLS_DELETE: &str = concat!("delete_vault_", "item(");
    /// The SECOND door: the detail pane's kebab, and the arm that follows it.
    /// The row menu's door is `COMMAND_ARMS[0]`, sliced by `arm_body`.
    const KEBAB_DOOR: &str = concat!("DetailAction::Del", "ete => {");
    const AFTER_KEBAB_DOOR: &str = concat!("DetailAction::No", "ne => {}");

    /// The detail pane's kebab arm, from its own `=> {` to the arm after it.
    ///
    /// Same idiom, and the same reason, as [`arm_body`]: a file-wide count
    /// cannot tell which door supplied a match. `DELETE_REPORTS` counted 2
    /// across the whole file while the kebab door was `let _ =
    /// delete_vault_item(...)` and one unrelated `move_error =
    /// Some(message);` elsewhere in `run` made up the difference -- which is
    /// a plausible spelling for any future arm that gets a sentence back
    /// from a helper.
    fn kebab_delete_body() -> &'static str {
        let source = production();
        let at = source
            .find(KEBAB_DOOR)
            .unwrap_or_else(|| panic!("no {KEBAB_DOOR:?} arm in production code"));
        let rest = &source[at + KEBAB_DOOR.len()..];
        let end = rest.find(AFTER_KEBAB_DOOR).unwrap_or_else(|| {
            panic!(
                "no {AFTER_KEBAB_DOOR:?} after the {KEBAB_DOOR:?} arm -- this guard slices \
                 the arm body up to it and cannot without it"
            )
        });
        &rest[..end]
    }

    #[test]
    fn the_soft_delete_is_wired_at_both_of_its_doors() {
        // The finding this closes: Delete is a SOFT delete, so it moves the
        // item into the Trash, and it did neither of the two things the other
        // four do. Reproduce the invalidation half by hand -- open Trash (the
        // list is fetched and the badge reads N), switch to All items, delete
        // an item, open Trash again: `wants_fetch` sees a list already in
        // hand, nothing refetches, and the just-deleted item is absent from
        // Trash with the badge still reading N for the life of the window.
        //
        // It has TWO doors -- the row menu and the detail pane's kebab -- and
        // the per-arm guard above can only ever see the first. That is why
        // the deed is in `delete_vault_item` and this checks the helper's own
        // body: one body, so both doors are covered by construction rather
        // than by a second needle that could be satisfied by the first door
        // alone.
        let source = production();
        let at = source
            .find(DEFINES_DELETE)
            .unwrap_or_else(|| panic!("no {DEFINES_DELETE:?} in production code"));
        let rest = &source[at + DEFINES_DELETE.len()..];
        let end = rest.find(AFTER_DELETE).unwrap_or_else(|| {
            panic!(
                "no {AFTER_DELETE:?} after {DEFINES_DELETE:?} -- this guard slices the \
                 helper's body up to it and cannot without it"
            )
        });
        let body = &rest[..end];
        for (needle, why) in [
            (
                concat!("trash_list.inval", "idate();"),
                "the soft delete does not drop the Trash list it just moved an item \
                 INTO. The list is not cached and is never pruned in place, so an \
                 already-open Trash row keeps listing -- and its badge keeps counting \
                 -- the vault as it was before the delete, for the life of the window",
            ),
            (
                concat!("ListCommand::Del", "ete,"),
                "the soft delete does not build its own failure sentence. `Purge`'s \
                 wording says \"permanently delete\" precisely so this one can say \
                 \"delete\"; borrowing another command's is worse than the \
                 `log::warn!` this replaced, and a `log::warn!` is not a user-visible \
                 channel at all",
            ),
        ] {
            assert_eq!(
                body.matches(needle).count(),
                1,
                "`delete_vault_item` does not do {needle:?} exactly once -- {why}.\n{body}"
            );
        }
        // Positive control for the slice, and for the "one body" claim: the
        // helper must be reached from exactly two places. Three occurrences
        // = the definition plus two call sites; a third caller, or a call
        // site deleted, changes the count and this guard stops being true.
        assert_eq!(
            production().matches(CALLS_DELETE).count(),
            3,
            "expected {CALLS_DELETE:?} three times in production code: the definition, \
             the row menu's arm, and the detail pane's kebab. A fourth door would not be \
             covered by anything here; a missing one means a Delete that no longer goes \
             through the body checked above"
        );
        // And that BOTH doors forward the sentence. `#[must_use]` on the
        // helper makes ignoring the return a warning rather than nothing, but
        // `let _ =` silences that, and this does not.
        //
        // ONCE IN EACH DOOR'S OWN SLICE, not twice in the file. The file-wide
        // count let one door supply the other's needle: the kebab as `let _ =
        // delete_vault_item(...)` plus one unrelated `if let Some(message) =
        // ... { move_error = Some(message); }` anywhere else in `run` counts
        // 2, keeps `CALLS_DELETE` at 3, and passes with the kebab door
        // silent. Same shape as `COMMAND_ARMS`, which has always sliced the
        // row menu's door for exactly this reason.
        for (door, body) in [
            ("the row menu's arm", arm_body(COMMAND_ARMS[0].0)),
            ("the detail pane's kebab", kebab_delete_body()),
        ] {
            assert_eq!(
                body.matches(DELETE_REPORTS).count(),
                1,
                "{door} does not do {DELETE_REPORTS:?} exactly once. `delete_vault_item` \
                 returning a sentence that this door drops is exactly as silent as the \
                 `log::warn!` it replaced -- and the other door having it proves nothing \
                 about this one.\n{body}"
            );
        }
        // And that there is no THIRD reporting site: two doors, two forwards.
        assert_eq!(
            production().matches(DELETE_REPORTS).count(),
            2,
            "expected {DELETE_REPORTS:?} twice in production code -- once per door, and \
             the two slices above account for both. A third is either an undiscovered \
             door onto the soft delete or a second spelling of the same forward"
        );
    }

    #[test]
    fn the_command_arm_slices_are_the_arms_they_claim_to_be() {
        // Positive control for the test above. Every needle it checks is an
        // "appears once" count, which a slice that came out empty or landed
        // on the wrong region would fail -- but for the wrong reason, and
        // with a message pointing at production code rather than at this
        // guard. These say which it is.
        for (marker, _, _) in COMMAND_ARMS {
            let body = arm_body(marker);
            assert!(
                !body.trim().is_empty(),
                "the {marker:?} slice is empty -- the arm markers or the terminators are \
                 stale, and the guard above proved nothing"
            );
            // `needs_reauth_for_closure` rather than `flag_reauth_if_\
            // unauthorized(`, which four of the five call directly and the
            // fifth reaches through `delete_vault_item`. All five name the
            // flag itself, so this stays one needle for all of them rather
            // than a per-arm exception that could go stale unnoticed.
            assert!(
                body.contains(concat!("needs_reauth_for", "_closure")),
                "the {marker:?} slice does not contain that arm's own re-auth handling, \
                 so it is not the arm body.\n{body}"
            );
        }
        // And that the five are really five distinct regions: a terminator
        // that stopped matching would make each slice run to the end of the
        // file, and every needle would then be found in someone else's arm.
        let mut bodies: Vec<&str> = COMMAND_ARMS.iter().map(|(m, _, _)| arm_body(m)).collect();
        bodies.sort_unstable();
        bodies.dedup();
        assert_eq!(bodies.len(), 5, "two command arms sliced to the same text");
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
        //  * the slice still reaches the LAST item defined above the first
        //    marker, so if that marker ever moves up (a `#[cfg(test)] use` at
        //    the head, say) the guards fail loudly instead of passing over a
        //    fraction of the file.
        //
        // That second check used to be "the slice is more than half the file",
        // and it was measuring the wrong thing: this file's test modules grow
        // every task, so the ratio decays on its own and says nothing about
        // where the marker is. Task 14 took it under 50% by adding tests
        // ONLY -- production got 80 lines and the tests got 900 -- which is
        // the healthy direction and would have been reported as the marker
        // moving to the top of the file.
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
        // The last thing defined above the first test marker. Named rather
        // than measured, so this cannot drift with how much test code the
        // file carries.
        const LAST_PRODUCTION_ITEM: &str = concat!("fn webbrowser", "_open(url: &str)");
        assert_eq!(
            source.matches(LAST_PRODUCTION_ITEM).count(),
            1,
            "{LAST_PRODUCTION_ITEM:?} is no longer in this file exactly once -- pick the new \
             last production item above the first {TESTS_BEGIN:?} and name it here"
        );
        assert!(
            production().contains(LAST_PRODUCTION_ITEM),
            "the production slice stops before {LAST_PRODUCTION_ITEM:?}, so the first \
             {TESTS_BEGIN:?} has moved up and every guard in this module is now inspecting a \
             fraction of the production code and passing for the wrong reason"
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
mod aux_list_tests {
    //! The on-demand trash/archive fetch decision.
    use super::AuxList;

    /// Every combination of the four inputs, because each of the three
    /// guards fails in its own visible-but-wrong way: without the `selected`
    /// check the window fetches both lists on every vault it ever opens,
    /// without `items` it refetches a list it already has on every frame,
    /// without `in_flight` it starts a thread per frame while the first is
    /// running, and without `error` it retries a dead backend at the frame
    /// rate.
    #[test]
    fn a_fetch_starts_only_for_the_selected_row_that_has_no_list_no_fetch_and_no_failure() {
        let fresh = AuxList::default();
        assert!(fresh.wants_fetch(true), "the selected row never asked for its list");
        assert!(!fresh.wants_fetch(false), "an unselected row fetched anyway");

        let fetched = AuxList { items: Some(Vec::new()), ..AuxList::default() };
        assert!(
            !fetched.wants_fetch(true),
            "a list that has already answered was fetched again -- note the answer was EMPTY, \
             which is the case a `Vec::is_empty` check instead of an `Option` would get wrong"
        );

        let running = AuxList { in_flight: true, ..AuxList::default() };
        assert!(!running.wants_fetch(true), "a second fetch started while one was in flight");

        let failed = AuxList { error: Some("boom".into()), ..AuxList::default() };
        assert!(!failed.wants_fetch(true), "a failed fetch was retried immediately");
    }

    /// Invalidating is what makes a failed row recoverable and a stale row
    /// re-read -- and it must NOT clear `in_flight`, or a vault reload that
    /// lands while a fetch is running starts a second one.
    #[test]
    fn invalidating_forgets_the_list_and_the_failure_but_not_the_thread() {
        let mut list = AuxList {
            items: Some(vec![]),
            error: Some("boom".into()),
            in_flight: true,
        };
        list.invalidate();
        assert!(list.items.is_none());
        assert!(list.error.is_none());
        assert!(list.in_flight, "invalidating let a second fetch start over a running one");

        // ...and with the thread finished, the row asks again. Without this
        // the two assertions above are compatible with a list that can never
        // be fetched at all.
        list.in_flight = false;
        assert!(list.wants_fetch(true));
    }
}

#[cfg(test)]
mod out_of_vault_pane_placement_tests {
    //! The detail pane's out-of-vault branch, and the four row commands that
    //! move an item between this window's three lists.
    //!
    //! SOURCE-TEXT GUARDS, and stated plainly. Everything below lives inside
    //! `run`'s eframe closure, which opens an OS window and which no test in
    //! this crate can call -- the same reason
    //! `item_pane_frame_placement_tests` and `reveal_state_placement_tests`
    //! are written this way. What they pin is the wiring: the decisions
    //! themselves (`menu_entries`, `detail::out_of_vault_text`,
    //! `AuxList::wants_fetch`, `sidebar::items_for`) are all pure and tested
    //! directly, and every one of them can be correct while nothing calls
    //! it -- which is this repository's most-repeated defect and the reason
    //! these exist at all.
    //!
    //! Needles are split with `concat!` so they cannot match their own
    //! declaration here. Do not re-join them.

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    #[test]
    fn the_read_pane_branches_to_the_out_of_vault_pane() {
        let needle = concat!("detail::draw_out_of_vault", "_read(");
        assert!(
            source().contains(needle),
            "nothing calls the out-of-vault detail pane. A trashed or archived row would open \
             the ordinary read pane, whose Edit/Fill/Delete/copy controls all act through the \
             LIVE item list -- which does not hold that item -- so every one of them would be \
             a control that quietly did nothing"
        );
    }

    #[test]
    fn each_of_the_four_list_commands_calls_the_cache() {
        // One needle per command, each naming the cache method it must
        // reach. A missing arm is a compile error (the match is exhaustive);
        // an arm that logs and does nothing is not, and that is what this
        // catches.
        for (command, call) in [
            (
                concat!("RowCommand::Arch", "ive =>"),
                concat!("cache.archive", "_item(&item)"),
            ),
            (
                concat!("RowCommand::Unarch", "ive =>"),
                concat!("cache.unarchive", "_item(&item)"),
            ),
            (
                concat!("RowCommand::Rest", "ore =>"),
                concat!("cache.restore", "_item(&item)"),
            ),
            (
                concat!("RowCommand::PurgeFor", "ever =>"),
                concat!("cache.purge", "_item(&item.id)"),
            ),
        ] {
            let at = source()
                .find(command)
                .unwrap_or_else(|| panic!("no {command:?} arm in this file"));
            let arm = &source()[at..at + 1400];
            assert!(
                arm.contains(call),
                "the {command:?} arm no longer calls {call:?} -- the menu entry is inert"
            );
        }
    }

    #[test]
    fn the_permanent_delete_is_still_behind_the_two_click_confirmation() {
        // The one irreversible action in this window. A `purge_item` reached
        // without `confirm_click` deletes the user's item on the first click
        // of a menu entry.
        let at = source().find(concat!("RowCommand::PurgeFor", "ever =>")).expect("the arm");
        let arm = &source()[at..at + 1400];
        assert!(
            arm.contains(concat!("confirm_", "click(&mut item_delete_pending")),
            "\"Delete forever\" is no longer two-click confirmed"
        );
    }

    #[test]
    fn both_on_demand_lists_are_dropped_when_the_vault_reloads() {
        // Without this a Trash list fetched before a sync stays on screen
        // afterwards, showing items the user has since restored elsewhere --
        // and, because `wants_fetch` sees a list already in hand, it is never
        // asked for again for the life of the window.
        for needle in [
            concat!("trash_list.inval", "idate()"),
            concat!("archive_list.inval", "idate()"),
        ] {
            assert!(
                source().contains(needle),
                "nothing calls {needle:?} -- an on-demand list is never refreshed"
            );
        }
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
#[cfg(test)]
mod fill_target_tests {
    //! The whole of "Fill in app"'s decision, over a window list a test
    //! writes -- no desktop, no process, no injector.

    use super::{fill_refusal_note, fill_target, FillRefusal};
    use crate::app_match::{AppMatch, TriggerMode};
    use crate::vault_bridge::VaultItem;
    use crate::window_list::WindowInfo;

    const HOST: &str = "ApplicationFrameHost.exe";

    fn login(id: &str) -> VaultItem {
        serde_json::from_str(&format!(
            r#"{{"id":"{id}","name":"Ledgerline","fields":[],"type":1,               "login":{{"username":"a.novak","password":"hunter2"}}}}"#
        ))
        .unwrap()
    }

    fn bound(process: &str, hosted: bool, title: &str) -> VaultItem {
        crate::vault_bridge::with_app_match(
            &login("id-1"),
            &AppMatch {
                process: process.to_string(),
                title: title.to_string(),
                hosted,
                path: String::new(),
                args: String::new(),
                sequence: String::new(),
                trigger: TriggerMode::Auto,
            },
        )
    }

    fn window(hwnd: isize, exe_name: &str, title: &str) -> WindowInfo {
        WindowInfo {
            hwnd,
            pid: 12472,
            exe_path: format!(r"C:\\Windows\\{exe_name}"),
            exe_name: exe_name.to_string(),
            title: title.to_string(),
            hosted: false,
        }
    }

    /// **The reviewer's exhibit, as an assertion.** Two Microsoft Store apps
    /// are open, so the unattributable host frame is in the window list; the
    /// item is bound to that host frame. Before the fix `find_window_for_
    /// process` handed back the FIRST of them and `fill_from_vault` typed the
    /// user's user name and password into whichever Store app happened to own
    /// it -- in the same frame in which the MATCHED APP card was saying
    /// Deskwarden ignores that binding.
    #[test]
    fn a_dead_binding_resolves_no_window_to_fill_however_many_store_apps_are_open() {
        let windows = [
            window(7, HOST, "Some Other Store App"),
            window(8, HOST, "Speedtest"),
        ];
        let item = bound(HOST, false, "");
        // The premise: a plain name compare really would have found one.
        assert!(
            windows.iter().any(|w| w.exe_name.eq_ignore_ascii_case(HOST)),
            "the fixture holds no host frame, so nothing here is being refused"
        );
        assert_eq!(
            fill_target(&item, &windows).map(|w| w.hwnd),
            Err(FillRefusal::BindingIsDead),
            "the fill resolved a window for a binding the pane says never fires"
        );
        // And it is reported as what it is, not as "isn't currently open" --
        // which would tell the user to launch something that is running.
        let note = fill_refusal_note(&item, FillRefusal::BindingIsDead);
        assert!(note.contains("ignores it"), "{note}");
        assert!(!note.contains("currently open"), "{note}");
    }

    /// The positive control on the test above: the same two open windows, and
    /// a binding to a real executable that is among them, still fills.
    /// Without this a `fill_target` that had simply stopped resolving
    /// anything would pass.
    #[test]
    fn a_live_binding_still_resolves_the_window_it_names() {
        let windows = [
            window(7, HOST, "Some Other Store App"),
            window(11, "Ledgerline.exe", "Ledgerline -- Invoices"),
        ];
        assert_eq!(
            fill_target(&bound("Ledgerline.exe", false, ""), &windows).map(|w| w.hwnd),
            Ok(11)
        );
        // Case-insensitively, the way every exe-name compare in this crate is.
        assert_eq!(
            fill_target(&bound("ledgerline.EXE", false, ""), &windows).map(|w| w.hwnd),
            Ok(11)
        );
    }

    /// The other two refusals, so the three are told apart and the messages
    /// cannot collapse into one.
    #[test]
    fn an_unbound_item_and_a_closed_app_are_refused_as_themselves() {
        let windows = [window(11, "Ledgerline.exe", "Ledgerline")];
        let unbound = login("id-2");
        assert_eq!(
            fill_target(&unbound, &windows).map(|w| w.hwnd),
            Err(FillRefusal::NotBound)
        );
        let elsewhere = bound("Mabl.exe", false, "");
        assert_eq!(
            fill_target(&elsewhere, &windows).map(|w| w.hwnd),
            Err(FillRefusal::NotOpen)
        );

        let notes = [
            fill_refusal_note(&unbound, FillRefusal::NotBound),
            fill_refusal_note(&elsewhere, FillRefusal::NotOpen),
            fill_refusal_note(&bound(HOST, false, ""), FillRefusal::BindingIsDead),
        ];
        for (i, a) in notes.iter().enumerate() {
            for b in &notes[i + 1..] {
                assert_ne!(a, b, "two refusals report the same sentence");
            }
        }
        // The closed-app note names the app, which is the whole reason that
        // arm reads the match again.
        assert!(notes[1].contains("Mabl.exe"), "{}", notes[1]);
    }

    /// A hosted match the picker really can save -- an attributed Store app,
    /// listed under its OWN executable name -- is not dead and still fills.
    /// The refusal is about the HOST's name, not about Store apps, and a fix
    /// that had refused every `hosted` match would fail here.
    #[test]
    fn an_attributed_store_app_is_not_collateral_damage() {
        let windows = [window(21, "Ledgerline.exe", "Ledgerline")];
        let item = bound("Ledgerline.exe", true, "Ledgerline");
        assert!(
            !crate::vault_window::detail::item_binding_is_dead(&item),
            "the premise: a match with a real exe name is not dead"
        );
        assert_eq!(fill_target(&item, &windows).map(|w| w.hwnd), Ok(21));
    }
}

#[cfg(test)]
mod fill_hotkey_applies_tests {
    use super::fill_hotkey_applies;
    use crate::app_match::{AppMatch, TriggerMode};
    use crate::vault_bridge::{ItemKind, VaultItem};

    /// An item of `kind`, carrying no `deskwarden:app-match` field at all.
    fn item_of_kind(kind: ItemKind) -> VaultItem {
        let item_type = match kind {
            ItemKind::Login => 1,
            ItemKind::SecureNote => 2,
            ItemKind::Card => 3,
            ItemKind::Identity => 4,
            ItemKind::SshKey => 5,
            ItemKind::Unknown(other) => other,
        };
        let item: VaultItem = serde_json::from_str(&format!(
            r#"{{"id":"id-1","name":"Ledgerline","fields":[],"type":{item_type}}}"#
        ))
        .unwrap();
        // The premise of every case below: the fixture really is the kind it
        // is named for, so a builder that silently produced a Login for
        // everything could not make these assertions vacuous.
        assert_eq!(ItemKind::of(&item), kind, "the fixture is not the kind it claims");
        item
    }

    /// A login bound to a match `MatchEngine` can never look up: the host
    /// process that owns the top-level window for every Microsoft Store app,
    /// with no title recorded to tell those apps apart.
    fn login_with_a_dead_binding() -> VaultItem {
        let item = crate::vault_bridge::with_app_match(
            &item_of_kind(ItemKind::Login),
            &AppMatch {
                process: "ApplicationFrameHost.exe".to_string(),
                title: String::new(),
                hosted: false,
                path: String::new(),
                args: String::new(),
                sequence: String::new(),
                trigger: TriggerMode::Auto,
            },
        );
        assert!(
            crate::vault_window::detail::item_binding_is_dead(&item),
            "the premise: this binding really is dead"
        );
        item
    }

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
            let item = item_of_kind(kind);
            assert_eq!(
                fill_hotkey_applies(&item, true),
                crate::vault_window::detail::item_offers_fill(&item, kind),
                "{kind:?}: the hotkey and the button disagree"
            );
        }
    }

    /// **The door beside the hidden button.** The header stops drawing Fill
    /// for a dead binding; a chord still gated on the KIND alone would reach
    /// `fill_item_into_app` for the very item the pane just said Deskwarden
    /// ignores. The pair below is the whole assertion: the same kind, the
    /// same press, differing only in the binding.
    #[test]
    fn the_fill_hotkey_is_refused_on_a_login_whose_binding_is_dead() {
        assert!(
            fill_hotkey_applies(&item_of_kind(ItemKind::Login), true),
            "the control: an unbound login is exactly what the hotkey is for"
        );
        assert!(
            !fill_hotkey_applies(&login_with_a_dead_binding(), true),
            "Ctrl+Shift+F fired on a binding the pane says never fires"
        );
    }

    #[test]
    fn an_unpressed_hotkey_fills_nothing_even_on_a_login() {
        assert!(!fill_hotkey_applies(&item_of_kind(ItemKind::Login), false));
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
            ssh_key: None,
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
    fn run_read_arm(
        item: &VaultItem,
        folder: Option<&str>,
        hotkey_pressed: bool,
    ) -> (DetailAction, Vec<String>) {
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
                folder,
                3,
                &TotpState::NoSecret,
                false,
                &mut reveal,
                None,
                &mut crate::app_identity::AppIdentityCache::default(),
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
            let (_, texts) = run_read_arm(&item, None, false);
            assert!(
                texts.contains(&kind.label()),
                "{kind:?}: the read arm painted no {:?} subtitle, so `draw_detail_read` \
                 was not reached for it; painted: {texts:?}",
                kind.label()
            );
            // NO "Delete" ASSERTION HERE ANY MORE. `2616427` moved Edit and
            // Delete behind the header's kebab menu, so neither paints until
            // that menu is opened, and this arm draws one closed frame.
            //
            // Dropping it costs this test nothing, which was checked rather
            // than assumed: with the `item_type != Some(1)` early return
            // reinstated, the placeholder pane paints the item's NAME and the
            // sentence below, so the subtitle assertion above and the
            // placeholder assertion below each fail on their own for every
            // non-login kind. Both were re-run against that mutation. What
            // the kebab now contains is `detail.rs`'s to prove, and it has
            // its own tests for it.
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
            let (action, _) = run_read_arm(&item, None, true);
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
        let (action, _) = run_read_arm(&an_item(Some(1)), None, false);
        assert_eq!(action, DetailAction::None);
    }

    /// **The folder this arm is handed reaches the header.**
    ///
    /// `detail.rs` proves the subtitle is built and painted; this proves the
    /// arm between `run` and it does not drop the name on the floor -- a
    /// `draw_read_arm` that took the parameter and passed `None` on would
    /// leave every one of those tests green and every real header folderless.
    ///
    /// Paired with its control: the same item with no folder still names its
    /// kind, so "the folder is absent" cannot be satisfied by a subtitle that
    /// stopped painting.
    #[test]
    fn the_read_arm_hands_the_folder_it_is_given_to_the_header() {
        let item = an_item(Some(1));

        let (_, texts) = run_read_arm(&item, Some("Engineering"), false);
        assert!(
            texts.contains(&"Login · Engineering".to_string()),
            "the read arm painted no `<kind> · <folder>` subtitle for an item in a \
             folder; painted: {texts:?}"
        );

        let (_, texts) = run_read_arm(&item, None, false);
        assert!(
            texts.contains(&"Login".to_string()),
            "the read arm painted no subtitle at all for an item in no folder; \
             painted: {texts:?}"
        );
    }
}

/// Where the header's folder NAME comes from, guarded at source level.
///
/// The pane is handed a name, not an id, and `sidebar::folder_name` is what
/// turns one into the other -- including the three cases where the answer is
/// "nothing": no folder, an id the list does not have, and `bw serve`'s
/// virtual bucket. The call that does it is inside `run`'s eframe closure, so
/// no harness in this crate can reach it (see `draw_read_arm_tests`' own
/// header for why that is a wall and not an omission). What it can have wrong
/// is precise: a second lookup written inline here, agreeing with the sidebar
/// today and drifting from it later, or a raw `item.folder_id` handed
/// straight to the pane -- which paints a uuid under the item's name.
///
/// Split-literal needles, single line, for this file's standing reasons: a
/// whole-literal needle matches its own declaration, and one carrying a line
/// ending passes on LF and fails on CRLF.
#[cfg(test)]
mod header_folder_placement_tests {
    const RESOLVED: &str = concat!(
        "sidebar::folder_",
        "name(&folders, item.folder_id.as_deref())"
    );
    const CALL: &str = concat!("let action = draw_read_", "arm(");
    const TESTS_BEGIN: &str = concat!("#[cfg(", "test)]");

    fn production() -> &'static str {
        let source = include_str!("mod.rs");
        let end = source
            .find(TESTS_BEGIN)
            .expect("no test marker in this file -- see `window_era_placement_tests`");
        &source[..end]
    }

    #[test]
    fn the_read_arms_folder_is_resolved_through_the_sidebars_lookup() {
        let production = production();
        let call = production.find(CALL).unwrap_or_else(|| {
            panic!(
                "{CALL:?} is not in the production code -- the read arm's call site was \
                 reshaped, so this guard no longer knows where to look"
            )
        });
        let resolved = production.find(RESOLVED).unwrap_or_else(|| {
            panic!(
                "{RESOLVED:?} is not in the production code. The header's folder is no \
                 longer resolved through `sidebar::folder_name`, which is where the \
                 virtual bucket and the missing-folder case are decided"
            )
        });
        // Bounded FORWARD from the call and short: the argument is the third
        // one, a few lines in. A window wide enough to reach the next
        // statement would pass against a resolution that had drifted out of
        // this call entirely.
        assert!(
            call < resolved && resolved - call < 200,
            "the folder-name resolution is not inside the read arm's call (call at \
             {call}, resolution at {resolved})"
        );
    }
}

/// The `+ New` menu's wiring, guarded at source level.
///
/// WHAT THIS MODULE CAN AND CANNOT SEE, PLAINLY. `item_list`'s own painted
/// tests drive the real button through real pointer frames and pin that
/// picking "Card" reports `ItemListAction::NewItem(ItemKind::Card)`. What no
/// test in this crate can reach is the other half -- the arm in `run` that
/// turns that action into a draft -- because `run` opens a real OS window
/// inside `eframe::run_ui_native`. The failure that half can have is precise
/// and silent: `EditDraft::empty()` where `empty_of(kind)` belongs opens a
/// LOGIN form no matter which row was picked, and every painted test upstream
/// still passes. So it is guarded the same way `window_era_placement_tests`
/// guards its own unreachable slice, with the same split-literal rule -- do
/// not re-join these needles, or they match themselves inside this module.
#[cfg(test)]
mod new_item_kind_placement_tests {
    const ARM: &str = concat!("ItemListAction::NewItem(kind)", " => {");
    const SEEDED: &str = concat!("DetailMode::Create(EditDraft::", "empty_of(kind))");
    /// The kindless constructor. Exactly one production use is legitimate:
    /// Ctrl+N, which has no kind to carry and deliberately opens the default
    /// (a login). A second occurrence means the menu's arm lost its kind.
    const KINDLESS: &str = concat!("EditDraft::", "empty()");
    const TESTS_BEGIN: &str = concat!("#[cfg(", "test)]");

    fn production() -> &'static str {
        let source = include_str!("mod.rs");
        let end = source
            .find(TESTS_BEGIN)
            .expect("no test marker in this file -- see `window_era_placement_tests`");
        &source[..end]
    }

    #[test]
    fn the_new_item_arm_seeds_the_draft_with_the_kind_that_was_picked() {
        let production = production();
        let arm = production.find(ARM).unwrap_or_else(|| {
            panic!(
                "{ARM:?} is not in the production code. `+ New` reports the kind that was \
                 picked; if the arm was reshaped, update this needle -- but if it stopped \
                 binding the kind at all, the menu's five rows all open the same form"
            )
        });
        let seeded = production.find(SEEDED).unwrap_or_else(|| {
            panic!(
                "{SEEDED:?} is not in the production code -- the picked kind is no longer what \
                 the draft is created from"
            )
        });
        assert!(
            arm < seeded && seeded - arm < 400,
            "the seeded-draft construction is not inside the NewItem arm (arm at {arm}, \
             construction at {seeded})"
        );
    }

    #[test]
    fn the_kindless_constructor_survives_only_on_the_ctrl_n_path() {
        let count = production().matches(KINDLESS).count();
        assert_eq!(
            count, 1,
            "expected exactly one production use of {KINDLESS:?} (Ctrl+N, which has no kind to \
             carry), found {count}. A second one is how the type menu quietly goes back to \
             opening a login whatever row was clicked"
        );
    }
}

/// The titlebar's settings gear: where it sits, and what its click does.
///
/// Source guards rather than click tests, and the reason is structural: the
/// gear lives in a closure passed to `draw_window_chrome_with_extra` from
/// inside `run`, which is the eframe application itself. No harness in this
/// crate can call `run`, so the closure's contents cannot be pressed the way
/// `detail.rs`'s star and kebab can be. Rebuilding the strip in a test would
/// assert the replica, not the code -- the shape of dead test this project
/// has shipped repeatedly.
///
/// The needles are `concat!`-split and single-line: a needle written as one
/// literal can match its own declaration, and a needle containing a newline
/// passes on LF and fails on CRLF (this repo has files in both states).
#[cfg(test)]
mod settings_gear_placement_tests {
    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    /// Production code only -- sliced at the first test marker, the same way
    /// `window_era_placement_tests::production` is, and sound for the same
    /// reason: every `fn` and `impl` in this file sits above it.
    fn production() -> &'static str {
        let source = source();
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file -- see `window_era_placement_tests`");
        &source[..end]
    }

    fn gear_needle() -> String {
        concat!("theme::gear_", "button(ui).clicked()").to_string()
    }

    /// **The strip's call, not the avatar's own definition.** The avatar used
    /// to be drawn by `run` itself, so `draw_circle_avatar(ui,` located it in
    /// the strip; it is drawn by `account_menu` now, and that call sits several
    /// thousand lines BELOW the strip in this file -- so the old needle would
    /// have gone on comparing `gear_at < avatar_at` and passing however the
    /// strip was reordered. What is in the strip is the `account_menu` call,
    /// which draws the chevron and then the avatar.
    fn account_menu_needle() -> String {
        concat!("match account_", "menu(").to_string()
    }

    /// **The gear must be added BEFORE the account menu**, because the strip
    /// packs right-to-left: `right_to_left` puts each new widget just to the
    /// LEFT of the previous one, so the widget added earlier ends up further
    /// right. The user asked for the gear to the right of the avatar, and
    /// reasoning about that inversion is precisely how it lands on the wrong
    /// side -- so it is pinned rather than argued.
    #[test]
    fn the_settings_gear_sits_to_the_right_of_the_avatar() {
        let production = production();
        let gear = gear_needle();
        let menu = account_menu_needle();

        // Positive controls. Without these, a rename of either call would
        // leave both `find`s returning `None` and the ordering assertion
        // below comparing nothing at all.
        let gear_at = production
            .find(&gear)
            .unwrap_or_else(|| panic!("no {gear:?} in production code -- the gear is gone"));
        let menu_at = production.find(&menu).unwrap_or_else(|| {
            panic!("no {menu:?} in production code -- the titlebar account menu is gone")
        });
        assert_eq!(
            production.matches(&gear).count(),
            1,
            "expected exactly one titlebar gear; more than one and this ordering says nothing"
        );

        assert!(
            gear_at < menu_at,
            "the settings gear is added AFTER the account menu, so the right-to-left strip \
             paints it to the LEFT of the avatar -- the opposite of what was asked for"
        );
    }

    /// The click has to do both halves. Setting the flag without closing
    /// leaves the request sitting in a cell nobody reads until the user
    /// closes the window by hand; closing without setting it loses the
    /// request entirely and reads as a window that shut for no reason.
    #[test]
    fn the_gear_asks_for_preferences_and_then_closes_the_window() {
        let body = gear_click_body();
        let sets_flag = concat!("*open_preferences_for_", "closure.borrow_mut() = true;");
        let closes = concat!("ViewportCommand::", "Close");

        assert!(
            body.contains(sets_flag),
            "the gear's click does not record the request; `main` has nothing to act on: {body:?}"
        );
        assert!(
            body.contains(closes),
            "the gear's click does not close the window, so `prefs_ui::run` -- which is its own \
             eframe window on this thread -- can never be reached: {body:?}"
        );
    }

    /// **A Settings request is not a lock and not an expired session.** Both
    /// of those run the full recovery sequence: stop the backend,
    /// re-authenticate, restart, repopulate. Folding the gear into either
    /// would make every visit to Preferences demand the master password.
    /// That is why `open_preferences` is its own field, and this is what
    /// stops a later tidy-up from collapsing the three.
    #[test]
    fn asking_for_preferences_is_neither_a_lock_nor_an_expired_session() {
        let body = gear_click_body();
        let locks = concat!("*locked_for_", "closure.borrow_mut()");
        let reauths = concat!("needs_reauth_for_", "closure");

        assert!(
            !body.contains(locks),
            "the gear's click also sets the LOCK flag, so opening Preferences tears down the \
             backend and demands the master password on the way back: {body:?}"
        );
        assert!(
            !body.contains(reauths),
            "the gear's click also flags an expired session, so opening Preferences runs the \
             re-authentication path: {body:?}"
        );
    }

    /// The body of the gear's `if ... clicked()` block, depth-counted to its
    /// matching brace. Slicing matters: a file-wide search would find the
    /// Lock arm's flag a few lines below and report it as the gear's.
    fn gear_click_body() -> &'static str {
        let production = production();
        let gear = gear_needle();
        let at = production
            .find(&gear)
            .unwrap_or_else(|| panic!("no {gear:?} in production code -- the gear is gone"));
        let after_open = &production[at..];
        let open = after_open
            .find('{')
            .expect("the gear's click has no block to slice");
        let after_open = &after_open[open + 1..];

        let mut depth = 1usize;
        for (offset, ch) in after_open.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let body = &after_open[..offset];
                        assert!(
                            !body.trim().is_empty(),
                            "the gear's click block is empty, so every assertion over it would \
                             pass against nothing"
                        );
                        return body;
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces after the gear's click -- this guard slices the block it opens")
    }
}

/// That "auto-lock is off" really is off, in the one place it has to be.
///
/// A source-text guard, for the same reason the modules above are: the idle
/// check lives inside `run`'s eframe closure, and no harness in this crate can
/// call `run`. `AutoLock` being an enum already makes the *type* mistakes
/// impossible -- `last_activity.elapsed() >= auto_lock` no longer compiles,
/// and neither does forgetting the `Never` arm -- but nothing in the compiler
/// stops someone writing a lock, or an elapsed-time comparison, into the
/// `Never` arm itself, which is exactly the change that would make "never"
/// mean "after a while" again.
///
/// Every needle is split with `concat!` and is single-line: `include_str!`
/// pulls this module in too, so a one-piece literal would match its own
/// declaration, and a needle containing a newline passes on LF and fails on
/// CRLF -- this repo has both.
#[cfg(test)]
mod auto_lock_never_wiring_tests {
    const MATCH_HEAD: &str = concat!("let lock_countdown = match auto_", "lock {");
    const NEVER_ARM: &str = concat!("AutoLock::Never => AUTO_LOCK_OFF_", "LABEL.to_owned(),");
    const AFTER_ARM: &str = concat!("AutoLock::After(timeout) ", "=> {");
    const ELAPSED_CHECK: &str = concat!("if last_activity.elapsed() >= ", "timeout {");
    const LOCKS: &str = concat!("*locked_for_closure.borrow_mut() = ", "true;");

    fn production() -> &'static str {
        let source = include_str!("mod.rs");
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file -- see `window_era_placement_tests`");
        &source[..end]
    }

    /// Everything the off case runs: the head of the match plus the `Never`
    /// arm, up to where the timed arm begins. Sliced from the head rather
    /// than from the `Never` arm itself so that a lock added *above* the arms
    /// -- which would fire in both states -- is inside the slice too.
    fn off_branch() -> &'static str {
        let production = production();
        let head = production.find(MATCH_HEAD).unwrap_or_else(|| {
            panic!(
                "{MATCH_HEAD:?} is not in production code -- the idle timer no longer \
                 branches on `AutoLock` at all"
            )
        });
        let after = production.find(AFTER_ARM).unwrap_or_else(|| {
            panic!(
                "{AFTER_ARM:?} is not in production code -- the timed arm is what the \
                 `Never` arm is being contrasted against here"
            )
        });
        assert!(after > head, "the match's arms are not in the order this guard slices them in");
        // Non-empty by construction (the two indices differ), so neither
        // assertion below can pass vacuously.
        &production[head..after]
    }

    #[test]
    fn the_never_arm_neither_locks_nor_measures_elapsed_time() {
        let arm = off_branch();
        assert!(
            !arm.contains(LOCKS),
            "the off branch locks the vault -- auto-lock being turned off must mean the idle \
             timer never closes the window with `locked = true` at all: {arm:?}"
        );
        assert!(
            !arm.contains("elapsed()"),
            "the off branch measures elapsed time -- `Never` is not a very long timeout, and \
             anything comparing against it is a timeout by another name: {arm:?}"
        );
        assert!(
            arm.contains(NEVER_ARM),
            "{NEVER_ARM:?} is not in the off branch, so the two assertions above are no longer \
             looking at the case they name: {arm:?}"
        );
    }

    #[test]
    fn the_timed_arm_still_locks_after_the_timeout_elapses() {
        // The positive control for the test above, which would otherwise be
        // satisfied by a window that never auto-locks in either state.
        let production = production();
        let after = production.find(AFTER_ARM).expect("no timed arm in production code");
        let arm = &production[after..];
        let elapsed = arm
            .find(ELAPSED_CHECK)
            .expect("the timed arm no longer compares the idle time against its timeout");
        let locks = arm[elapsed..]
            .find(LOCKS)
            .expect("the timed arm no longer locks once its timeout has elapsed");
        assert!(locks > 0, "the lock must be inside the elapsed check, not before it");
    }
}

/// **The titlebar's account menu**, driven through real frames.
///
/// The gear beside it could only ever be source-guarded (see
/// `settings_gear_placement_tests`) because it *is* a click in a closure `run`
/// owns. The menu is not: its whole decision -- what it says about the account
/// this window is on, which other accounts are offered, what a blocked state
/// says, which of Lock/Add/Remove are offered at all, and what a click on any
/// of them reports -- lives in `account_menu`, which is an ordinary function a
/// headless `egui::Context` can press. So these are click tests, and only the
/// two things that genuinely cannot be reached (that `run`'s strip calls it,
/// and what it does with each answer) are guarded by source below.
///
/// **Every row is clicked, not merely read.** This feature set has repeatedly
/// shipped a menu whose contents were right and whose clicks reached nothing;
/// a test that only reads the painted strings passes against exactly that.
///
/// Modelled on `detail.rs`'s `Pane`, including its two hard-won details: a
/// press *and* a release is what egui counts as a click, and **a popup only
/// paints on the frame after the click that opened it**, so the frame that
/// finds a menu row and the frame that opened the menu can never be the same
/// one.
#[cfg(test)]
mod account_switcher_tests {
    use super::*;
    use crate::accounts::{Account, AccountId, AccountsState};

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";
    /// A third id, for the account with no email.
    const C: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn account(id: &str, email: &str) -> Account {
        Account {
            id: AccountId::parse(id).expect("a 32-char lowercase hex id"),
            email: email.to_string(),
            server_url: None,
        }
    }

    fn a() -> Account {
        account(A, "ana@example.com")
    }

    fn b() -> Account {
        account(B, "bruno@example.com")
    }

    /// An account as `resolve_startup` mints one on a first install: no email
    /// at all until a sign-in fills it in.
    fn blank() -> Account {
        account(C, "")
    }

    /// A sentence with the same shape as the real `relativeDataDir` refusal --
    /// long, and naming a directory.
    const BLOCKED_REASON: &str =
        "a bitwarden-cli directory sits beside bw.exe (C:\\tools\\bin\\bitwarden-cli), so the \
         CLI ignores the profile Deskwarden points it at";

    /// Built through `AccountsState::from_blocked_reason`, which is the only
    /// constructor this file may use: `no_window_answers_may_i_switch_for_
    /// itself` bans it from naming `AccountsState::new`'s two inputs at all,
    /// tests included. `the_test_constructor_agrees_with_the_real_one` (in
    /// `accounts.rs`, which may name them) pins the two together.
    fn available_state() -> AccountsState {
        AccountsState::from_blocked_reason(vec![a(), b()], a().id, None)
            .expect("these accounts are not empty")
    }

    fn blocked_state() -> AccountsState {
        AccountsState::from_blocked_reason(
            vec![a(), b()],
            a().id,
            Some(BLOCKED_REASON.to_string()),
        )
        .expect("these accounts are not empty")
    }

    fn lone_state() -> AccountsState {
        AccountsState::from_blocked_reason(vec![a()], a().id, None)
            .expect("this account is not empty")
    }

    /// The switcher with an account that has never signed in.
    fn blank_email_state() -> AccountsState {
        AccountsState::from_blocked_reason(vec![a(), blank()], a().id, None)
            .expect("these accounts are not empty")
    }

    fn click_at(pos: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ]
    }

    /// The pane these frames are laid out at. An absolute number, never
    /// re-derived from anything under test.
    const PANE: f32 = 420.0;

    struct Frame {
        picked: Option<AccountAction>,
        texts: Vec<(String, egui::Rect)>,
        /// The characters really laid out, which `Galley::text()` is blind to
        /// -- see `detail.rs`'s `Frame::rendered`. A menu row elided down to
        /// one "..." still reports the whole email it was handed.
        rendered: Vec<(String, String)>,
        chevrons: Vec<egui::Rect>,
    }

    impl Frame {
        fn strings(&self) -> Vec<&str> {
            self.texts.iter().map(|(t, _)| t.as_str()).collect()
        }

        fn painted(&self, label: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == label)
        }

        /// What was actually DRAWN for the run laid out from `label`.
        fn glyphs(&self, label: &str) -> String {
            let found: Vec<&String> = self
                .rendered
                .iter()
                .filter(|(source, _)| source == label)
                .map(|(_, drawn)| drawn)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one run laid out from {label:?}, found {}; painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0].clone()
        }

        fn rect_of(&self, label: &str) -> egui::Rect {
            let found: Vec<egui::Rect> = self
                .texts
                .iter()
                .filter(|(t, _)| t == label)
                .map(|(_, r)| *r)
                .collect();
            assert_eq!(
                found.len(),
                1,
                "expected exactly one {label:?}, found {}; painted: {:?}",
                found.len(),
                self.strings()
            );
            found[0]
        }

        fn chevron(&self) -> egui::Rect {
            assert_eq!(
                self.chevrons.len(),
                1,
                "expected exactly one switcher chevron, found {}; painted: {:?}",
                self.chevrons.len(),
                self.strings()
            );
            self.chevrons[0]
        }

        /// The avatar circle, located by the initials painted in the middle of
        /// it -- the real drawn position, not the rect the harness asked for.
        fn avatar(&self) -> egui::Rect {
            self.rect_of("AN")
        }
    }

    /// The harness. Draws the real `account_menu` and nothing else, so
    /// every string and every stroke a frame reports is the menu's.
    ///
    /// `email`/`server_url` are the `bw status` answers the window was handed.
    /// `None` for both is the state before they arrive -- which is the state
    /// every pre-existing test here ran in, and is left as the default so that
    /// the identity block's fallback to `account_label` is what they exercise.
    struct Switcher {
        ctx: egui::Context,
        email: Option<String>,
        server_url: Option<String>,
        /// Whether `bw status` has answered at all -- the window's own
        /// `icon_base_known`. `with_details` sets it, because a harness that
        /// hands over an answer has by definition had one.
        server_known: bool,
    }

    impl Switcher {
        fn new() -> Self {
            let ctx = egui::Context::default();
            // The same two throwaway frames every harness in this crate runs:
            // a font set registered during a frame is only usable from the
            // start of the next one.
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            theme::apply(&ctx);
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            Self {
                ctx,
                email: None,
                server_url: None,
                server_known: false,
            }
        }

        /// The same harness with `bw status`'s answers in hand.
        fn with_details(email: Option<&str>, server_url: Option<&str>) -> Self {
            Self {
                email: email.map(str::to_string),
                server_url: server_url.map(str::to_string),
                server_known: true,
                ..Self::new()
            }
        }

        fn input(events: Vec<egui::Event>) -> egui::RawInput {
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::splat(PANE),
                )),
                events,
                ..Default::default()
            }
        }

        fn frame(&mut self, accounts: Option<&AccountsState>, events: Vec<egui::Event>) -> Frame {
            let mut action = None;
            let (email, server_url, server_known) =
                (self.email.clone(), self.server_url.clone(), self.server_known);
            let output = self.ctx.run_ui(Self::input(events), |ui| {
                action = account_menu(
                    ui,
                    accounts,
                    // A fixed pair of initials: what the circle carries is
                    // `avatar_initials`' decision and has its own tests, and a
                    // harness that recomputed it here would be a second copy of
                    // that rule. "AN" is also two characters no menu row
                    // contains, so `rect_of` cannot confuse the two.
                    "AN",
                    email.as_deref(),
                    server_url.as_deref(),
                    server_known,
                );
            });
            frame_from(action, &output)
        }

        fn idle(&mut self, accounts: Option<&AccountsState>) -> Frame {
            self.frame(accounts, Vec::new())
        }

        fn click(&mut self, accounts: Option<&AccountsState>, pos: egui::Pos2) -> Frame {
            self.frame(accounts, click_at(pos))
        }

        /// Click the chevron, then let the popup paint. Returns the frame the
        /// open menu is on -- the click's own frame never has it.
        fn open(&mut self, accounts: Option<&AccountsState>) -> Frame {
            let chevron = self.idle(accounts).chevron();
            let _ = self.click(accounts, chevron.center());
            self.idle(accounts)
        }

        /// The same, opened by clicking the AVATAR rather than the chevron --
        /// which is what the user actually asked for ("on avatar icon click").
        fn open_from_avatar(&mut self, accounts: Option<&AccountsState>) -> Frame {
            let avatar = self.idle(accounts).avatar();
            let _ = self.click(accounts, avatar.center());
            self.idle(accounts)
        }

    }

    /// Opens a menu on a **fresh** harness and clicks the row painted from
    /// `label`, reporting what that click asked for.
    ///
    /// Fresh every time, and not a method, because opening is a toggle: a
    /// second `open` on a harness whose menu is already up closes it, and every
    /// assertion afterwards then runs against a bare avatar.
    fn pick(accounts: Option<&AccountsState>, label: &str) -> Option<AccountAction> {
        let mut switcher = Switcher::new();
        let open = switcher.open(accounts);
        assert!(
            open.painted(label),
            "the open menu has no {label:?} row to click; it painted: {:?}",
            open.strings()
        );
        switcher.click(accounts, open.rect_of(label).center()).picked
    }

    fn frame_from(picked: Option<AccountAction>, output: &egui::FullOutput) -> Frame {
        // ONE tree, not one call per clipped shape: a probe run per clipped
        // shape could never see a chevron whose two arms landed in different
        // ones.
        let all = egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());
        let mut texts = Vec::new();
        let mut rendered = Vec::new();
        collect_switcher_text(&all, &mut texts, &mut rendered);
        Frame {
            picked,
            texts,
            rendered,
            chevrons: theme::icon_probe::chevrons(&all),
        }
    }

    fn collect_switcher_text(
        shape: &egui::Shape,
        texts: &mut Vec<(String, egui::Rect)>,
        rendered: &mut Vec<(String, String)>,
    ) {
        match shape {
            egui::Shape::Text(text) => {
                texts.push((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                ));
                rendered.push((
                    text.galley.text().to_string(),
                    text.galley
                        .rows
                        .iter()
                        .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                        .collect(),
                ));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_switcher_text(shape, texts, rendered);
                }
            }
            _ => {}
        }
    }

    /// **The pick has to REACH the caller.** A switcher that paints its rows
    /// correctly and answers `None` is the "decision correct, renderer inert"
    /// shape this codebase keeps producing -- five mutations to this feature's
    /// wiring have each left the whole suite green.
    #[test]
    fn the_switcher_lists_only_the_switchable_accounts_and_reports_the_pick() {
        let mut switcher = Switcher::new();
        let state = available_state();
        let open = switcher.open(Some(&state));

        assert!(
            open.painted(&b().email),
            "the open menu offers no row for the other account; it painted: {:?}",
            open.strings()
        );
        let row = open.rect_of(&b().email);

        let picked = switcher.click(Some(&state), row.center());
        assert_eq!(
            picked.picked,
            Some(AccountAction::Switch(b().id)),
            "the click on {:?} reported {:?}",
            b().email,
            picked.picked
        );
    }

    /// The other half of the same claim: the row the user is ALREADY on is not
    /// a switch target. `all()` still holds it (and still holds duplicate ids);
    /// `switchable()` is what this menu is built from.
    #[test]
    fn the_account_already_active_is_named_but_is_not_something_to_click() {
        let mut switcher = Switcher::new();
        let state = available_state();
        let open = switcher.open(Some(&state));

        assert!(
            open.painted(&a().email),
            "the menu never says which account this window is showing: {:?}",
            open.strings()
        );
        // Positive control on the same frame: the row that IS clickable is
        // there, so "clicking a does nothing" below is not a menu that drew
        // nothing at all.
        assert!(open.painted(&b().email), "control: {:?}", open.strings());

        let clicked = switcher.click(Some(&state), open.rect_of(&a().email).center());
        assert_eq!(
            clicked.picked, None,
            "clicking the account this window is already on reported a switch to it, which \
             would tear the backend down and put up a master-password prompt to arrive \
             exactly where the user already was"
        );
    }

    /// **A refusal is said out loud.** Painting nothing is indistinguishable
    /// from "you have one account", and the refusal this gate exists for -- a
    /// `bitwarden-cli` directory beside `bw.exe` -- is something the user can
    /// go and act on. Nothing else in this window mentions it.
    #[test]
    fn a_blocked_state_paints_the_reason_instead_of_a_switcher() {
        let mut switcher = Switcher::new();
        let state = blocked_state();
        let open = switcher.open(Some(&state));

        assert!(
            !open.painted(&b().email),
            "a switch to {:?} was offered while it cannot work: the CLI would ignore the \
             profile and both accounts would share one. Painted: {:?}",
            b().email,
            open.strings()
        );
        assert!(
            open.painted(BLOCKED_REASON),
            "the user is not told why the switcher is empty; painted: {:?}",
            open.strings()
        );
        // `Galley::text()` is blind to truncation: the reason names a
        // directory, and a menu that elided it down to "a bitwarden-cli..."
        // would pass the assertion above while telling the user nothing.
        let drawn = open.glyphs(BLOCKED_REASON);
        assert!(
            drawn.contains("bitwarden-cli"),
            "the reason was laid out but drawn as {drawn:?}"
        );
        assert!(
            !drawn.contains('\u{2026}'),
            "the reason was elided rather than wrapped; drawn as {drawn:?}"
        );
    }

    /// The positive control for the test above. "The switcher offers no
    /// blocked account" passes trivially against a switcher that draws
    /// nothing at all, so the same helper has to be watched offering one.
    #[test]
    fn the_same_switcher_offers_the_account_once_nothing_is_blocking_it() {
        let mut switcher = Switcher::new();
        let open = switcher.open(Some(&available_state()));
        assert!(
            open.painted(&b().email),
            "the harness offers nothing even unblocked, so the blocked assertions say \
             nothing: {:?}",
            open.strings()
        );
        assert!(
            !open.painted(BLOCKED_REASON),
            "an unblocked state paints a refusal: {:?}",
            open.strings()
        );
    }

    /// **A blank row is not a row.** An account minted by `resolve_startup` on
    /// a first install, or by `prepare_new_account`, carries an empty email
    /// until a sign-in fills it in -- so without `accounts::account_label` the
    /// switcher offers a strip of menu with nothing written on it, and the
    /// user has no way to tell what they would be switching to.
    #[test]
    fn an_account_with_no_email_is_offered_by_its_id_rather_than_as_a_blank_row() {
        let mut switcher = Switcher::new();
        let state = blank_email_state();
        let open = switcher.open(Some(&state));

        assert!(
            open.painted(C),
            "the account with no email is not named by its id; painted: {:?}",
            open.strings()
        );
        assert!(
            !open.painted(""),
            "the switcher painted an empty string, which is the blank row this exists to \
             stop: {:?}",
            open.strings()
        );
        // And it is really clickable, not just readable.
        let picked = switcher.click(Some(&state), open.rect_of(C).center());
        assert_eq!(picked.picked, Some(AccountAction::Switch(blank().id)));
    }

    /// One account and nothing blocking is the overwhelmingly common state.
    /// An empty menu there is indistinguishable from a menu that failed to
    /// build, and from the blocked state above -- which is a refusal the user
    /// could act on and this is not.
    #[test]
    fn a_single_account_gets_a_menu_that_says_so_rather_than_an_empty_one() {
        let mut switcher = Switcher::new();
        let open = switcher.open(Some(&lone_state()));
        // The literal, not `NO_OTHER_ACCOUNTS`: a test that reads the constant
        // it is checking passes against any value that constant is given,
        // including an empty string -- which is precisely the failure this
        // test exists to catch. The constant is named only in the message.
        assert!(
            open.painted("No other accounts yet"),
            "the lone-account menu says nothing at all (`NO_OTHER_ACCOUNTS` is {:?}); it \
             painted: {:?}",
            NO_OTHER_ACCOUNTS,
            open.strings()
        );
        assert!(
            !open.painted(BLOCKED_REASON),
            "having one account is reported as a refusal: {:?}",
            open.strings()
        );
    }

    /// `StartupAccounts::NoAccountList`: this app has no `Account` at all,
    /// because `settings.json` could not be read. There is nothing to say
    /// about accounts, so there is no menu and no mark that opens one -- but
    /// the circle stays, because everything left of it in the titlebar shifts
    /// sideways if it comes and goes.
    #[test]
    fn an_app_with_no_account_list_still_wears_its_avatar_but_opens_no_menu() {
        let mut switcher = Switcher::new();
        let idle = switcher.idle(None);
        assert!(
            idle.chevrons.is_empty(),
            "a menu control was drawn for an app that has no accounts"
        );
        assert!(
            idle.painted("AN"),
            "the avatar vanished along with the menu, so the whole toolbar shifts sideways \
             for an app whose account list could not be read; painted: {:?}",
            idle.strings()
        );
        // Clicking it opens nothing and asks for nothing -- there is no
        // `AccountsState` to build a menu from.
        let clicked = switcher.click(None, idle.avatar().center());
        assert_eq!(clicked.picked, None);
        assert!(
            !switcher.idle(None).painted(LOCK_LABEL),
            "a menu opened for an app that has no accounts: {:?}",
            switcher.idle(None).strings()
        );
        // The positive control: the same harness, the same frame, one
        // difference.
        assert_eq!(
            switcher.idle(Some(&available_state())).chevrons.len(),
            1,
            "the harness draws no chevron even WITH accounts, so the assertions above are \
             about the harness rather than about the menu"
        );
    }

    // ------------------------------------------------------------------
    // The menu the user asked for: an identity block, the other accounts,
    // then Lock / Add / Remove.
    // ------------------------------------------------------------------

    /// **The avatar is the trigger.** "on avatar icon click there should be a
    /// list with something meaningful" — the chevron beside it was the only way
    /// in before, and it is a 28px mark next to a 28px circle that reads as the
    /// obvious thing to press.
    #[test]
    fn clicking_the_avatar_opens_the_same_menu_the_chevron_does() {
        let state = available_state();

        let mut from_avatar = Switcher::new();
        let opened = from_avatar.open_from_avatar(Some(&state));
        assert!(
            opened.painted(&b().email),
            "clicking the avatar opened nothing; painted: {:?}",
            opened.strings()
        );
        assert!(opened.painted(LOCK_LABEL), "painted: {:?}", opened.strings());

        // The same menu, not merely *a* menu: every row the chevron produces.
        let mut from_chevron = Switcher::new();
        let by_chevron = from_chevron.open(Some(&state));
        assert_eq!(
            opened.strings(),
            by_chevron.strings(),
            "the avatar and the chevron open two different menus"
        );

        // And a click on the avatar really reaches a row: `Popup::menu` is hung
        // off the union of the two responses, so an implementation that hung it
        // off the chevron alone would open on the avatar's click only by
        // accident of the rects overlapping.
        let row = opened.rect_of(&b().email);
        assert_eq!(
            from_avatar.click(Some(&state), row.center()).picked,
            Some(AccountAction::Switch(b().id))
        );
    }

    /// The identity block: **two lines, the address and the host**. There is no
    /// display name — `bw status` reports a status, an email and a server URL
    /// and nothing else — so the address is the heading, and nothing is
    /// invented from it.
    #[test]
    fn the_menu_names_the_account_and_its_host_rather_than_a_hash() {
        let mut switcher =
            Switcher::with_details(Some("ana@example.com"), Some("https://nw37.example.net/api"));
        let open = switcher.open(Some(&available_state()));

        assert!(
            open.painted("ana@example.com"),
            "the menu does not say whose vault this is: {:?}",
            open.strings()
        );
        // The HOST, scheme and path stripped, which is what the login window's
        // footer already shows this user. A literal, not a call to
        // `server_host` -- a test that re-derives its expectation from the
        // function under test passes against whatever that function does.
        assert!(
            open.painted("nw37.example.net"),
            "the menu does not say which host this account is on -- the one fact a \
             self-hosted user needs from it -- or says it as a raw URL: {:?}",
            open.strings()
        );
        assert!(
            !open.painted("https://nw37.example.net/api"),
            "the menu paints the raw `bw status` URL rather than the host treatment the \
             login footer uses: {:?}",
            open.strings()
        );
        // Drawn, not merely laid out: a host elided to "nw37.exa..." would
        // satisfy the assertion above and tell the user nothing.
        let drawn = open.glyphs("nw37.example.net");
        assert!(
            !drawn.contains('\u{2026}'),
            "the host was elided rather than wrapped; drawn as {drawn:?}"
        );
        // And no name was conjured out of the local part of the address.
        for invented in ["Ana", "ana", "Ana Novak"] {
            assert!(
                !open.painted(invented),
                "the menu painted {invented:?} as though it knew a display name; nothing this \
                 app can ask reports one: {:?}",
                open.strings()
            );
        }
    }

    /// The bug behind "not some random hash": an account minted on a first
    /// install has no stored email, so `account_label` names it by its
    /// directory. The live `bw status` answer outranks it the moment it lands.
    #[test]
    fn the_menu_prefers_the_live_address_to_the_id_it_would_otherwise_show() {
        // The active account is the blank one, so `account_label` would say C.
        let state = AccountsState::from_blocked_reason(vec![blank(), a()], blank().id, None)
            .expect("these accounts are not empty");

        let mut cold = Switcher::new();
        let before = cold.open(Some(&state));
        assert!(
            before.painted(C),
            "control: with no details in hand the menu really does fall back to the id, which \
             is what makes the assertion below about the details rather than about nothing: \
             {:?}",
            before.strings()
        );

        let mut warm = Switcher::with_details(Some("ana@example.com"), None);
        let after = warm.open(Some(&state));
        assert!(
            after.painted("ana@example.com"),
            "the menu shows the directory name even though `bw status` has answered: {:?}",
            after.strings()
        );
        assert!(
            !after.painted(C),
            "the menu shows the account id beside the address it now knows: {:?}",
            after.strings()
        );
    }

    /// **The three servers a user can be on, and the fourth state that is not
    /// one of them.**
    ///
    /// > "it's either whatever self-host URL or bitwarden.com or bitwarden.eu"
    ///
    /// The whole table, so no case can be special-cased into working while
    /// another silently is not — and including `vault.bitwarden.community`,
    /// which is a self-hosted domain that *contains* `bitwarden.com` and which
    /// a substring test tells the user is Bitwarden's cloud.
    #[test]
    fn the_host_line_tells_self_hosted_from_bitwarden_com_from_bitwarden_eu() {
        // Literals, never `server_host`/`bitwarden_cloud` re-run here: a test
        // that re-derives its expectation from the code under test passes
        // against whatever that code does.
        for (reported, expected) in [
            ("https://nw37.example.net/api", "nw37.example.net"),
            ("https://vault.bitwarden.com", "bitwarden.com"),
            ("https://bitwarden.com", "bitwarden.com"),
            ("https://vault.bitwarden.eu", "bitwarden.eu"),
            ("https://bitwarden.eu", "bitwarden.eu"),
            // The trap: a self-hosted domain that contains "bitwarden.com".
            ("https://vault.bitwarden.community", "vault.bitwarden.community"),
        ] {
            let mut switcher = Switcher::with_details(Some("ana@example.com"), Some(reported));
            let open = switcher.open(Some(&available_state()));
            assert!(
                open.painted(expected),
                "an account on {reported:?} is not reported as {expected:?}; the menu \
                 painted: {:?}",
                open.strings()
            );
            assert!(
                !open.painted(reported),
                "the menu paints the raw `bw status` URL rather than the host treatment the \
                 login footer uses: {:?}",
                open.strings()
            );
        }

        // The CLI answered and named no server: that is its own default, which
        // is Bitwarden's cloud, and the same reading the login footer makes.
        let mut cloud = Switcher::with_details(Some("ana@example.com"), None);
        let cloud = cloud.open(Some(&available_state()));
        assert!(
            cloud.painted("bitwarden.com"),
            "an account the CLI reported no server for says nothing about where it lives: {:?}",
            cloud.strings()
        );
    }

    /// **The fourth state: nothing has answered yet.** An account
    /// `resolve_startup` minted on a first install has `server_url: None` for
    /// the same reason it has no email, and `bw status` has not come back —
    /// so printing `bitwarden.com` states something FALSE about a self-hosted
    /// account that has simply not been asked about. No line, which is what
    /// the empty avatar and the waiting favicons already do with this exact
    /// fact.
    #[test]
    fn a_server_nothing_has_reported_yet_gets_no_line_rather_than_a_default() {
        // `Switcher::new` is the un-answered harness: `server_known` is false.
        let mut switcher = Switcher::new();
        let open = switcher.open(Some(&available_state()));
        for premature in ["bitwarden.com", "bitwarden.eu"] {
            assert!(
                !open.painted(premature),
                "the menu claims this account is on {premature:?} before anything has said \
                 so: {:?}",
                open.strings()
            );
        }
        // The positive control: the identity block IS there, so "no host line"
        // is not "no menu at all".
        assert!(open.painted(&a().email), "{:?}", open.strings());
        assert!(open.painted(LOCK_LABEL), "{:?}", open.strings());

        // And what IS stored still counts as knowing, even before `bw status`
        // answers -- `settings.json` remembering a server is an answer.
        let hosted = Account {
            server_url: Some("https://nw37.example.net".to_string()),
            ..a()
        };
        let state = AccountsState::from_blocked_reason(vec![hosted.clone(), b()], hosted.id, None)
            .expect("these accounts are not empty");
        let mut stored = Switcher::new();
        let open = stored.open(Some(&state));
        assert!(
            open.painted("nw37.example.net"),
            "a server this app already wrote down is not shown until `bw status` repeats it: \
             {:?}",
            open.strings()
        );
    }

    /// Lock, and its chord, and what the click reports. **Not "log out"** —
    /// this app has two different things that could mean, and this menu offers
    /// both under their own names.
    #[test]
    fn the_menu_offers_lock_with_its_shortcut_and_reports_the_click() {
        let mut switcher = Switcher::new();
        let open = switcher.open(Some(&available_state()));
        assert!(
            open.painted("Lock"),
            "the menu has no Lock row, and the titlebar pill it replaced is gone: {:?}",
            open.strings()
        );
        assert!(
            open.painted("CTRL+L"),
            "the Lock row carries no shortcut hint; the chord still works but nothing on \
             screen says so any more: {:?}",
            open.strings()
        );
        // The word this app does not use, in either menu.
        for banned in ["Log out", "Log Out", "Logout", "Sign out"] {
            assert!(
                !open.painted(banned),
                "the menu offers {banned:?}, which means either Lock or Remove and says which \
                 of the two it is not: {:?}",
                open.strings()
            );
        }
        assert_eq!(
            pick(Some(&available_state()), "Lock"),
            Some(AccountAction::Lock)
        );
    }

    /// Add and Remove, and the gates on them. Removing is offered only when
    /// `can_remove_active` says the app has somewhere to land — a row that can
    /// only fail is worse than one that is not there.
    #[test]
    fn add_and_remove_are_offered_when_they_can_work_and_report_their_clicks() {
        let mut switcher = Switcher::new();
        let state = available_state();
        let open = switcher.open(Some(&state));
        assert!(
            open.painted("Add account..."),
            "the menu offers no way to add an account, which the user asked to sit under the \
             list: {:?}",
            open.strings()
        );
        assert!(
            open.painted("Remove this account..."),
            "the menu offers no removal for an account that has a survivor to settle onto: \
             {:?}",
            open.strings()
        );
        assert_eq!(
            pick(Some(&state), "Add account..."),
            Some(AccountAction::Add)
        );
        assert_eq!(
            pick(Some(&state), "Remove this account..."),
            Some(AccountAction::Remove)
        );
    }

    /// The two refusals, each paired with the control above: a lone account has
    /// nowhere to land after a removal, and a blocked state cannot reach the
    /// survivor it would have to settle onto first. `add` survives the first
    /// and not the second, which is what makes these two states rather than
    /// one.
    #[test]
    fn a_removal_that_could_only_fail_is_not_offered_at_all() {
        let mut lone = Switcher::new();
        let open = lone.open(Some(&lone_state()));
        assert!(
            !open.painted("Remove this account..."),
            "the only account was offered for removal, which `remove_account` refuses -- there \
             is no profile left to point the CLI at: {:?}",
            open.strings()
        );
        assert!(
            open.painted("Add account..."),
            "control: a lone account can still ADD one, so the menu is not simply empty below \
             the separator: {:?}",
            open.strings()
        );
        assert!(open.painted("Lock"), "control: {:?}", open.strings());

        let mut blocked = Switcher::new();
        let open = blocked.open(Some(&blocked_state()));
        assert!(
            !open.painted("Remove this account..."),
            "a removal was offered while the app cannot reach the survivor to settle onto it: \
             {:?}",
            open.strings()
        );
        assert!(
            !open.painted("Add account..."),
            "an account was addable into one shared profile: {:?}",
            open.strings()
        );
        assert!(
            open.painted("Lock"),
            "control: Lock is refused by nothing and is still there, so the two assertions \
             above are about the gates rather than about a menu that stopped drawing rows: \
             {:?}",
            open.strings()
        );
    }
}

/// **Where the switcher sits in the titlebar, and whether its clicks survive
/// the drag zone.**
///
/// `draw_window_chrome_with_extra` narrows the window's drag zone to stop
/// where the extra controls start, and registers that zone AFTER them -- so a
/// control whose rect the zone still covers is a control whose clicks the drag
/// swallows, which presents as a button that does nothing. This runs the real
/// chrome function with the real switcher inside it and presses it.
///
/// The three controls are drawn here in the order
/// `the_switcher_is_added_between_the_gear_and_the_avatar` pins `run`'s own
/// strip to. That guard and this measurement are two halves: no harness in
/// this crate can call `run` (it is the eframe application itself), so the
/// ordering is pinned by source and its CONSEQUENCE is measured here rather
/// than argued.
#[cfg(test)]
mod titlebar_switcher_placement_tests {
    use super::*;
    use crate::accounts::{Account, AccountId, AccountsState};
    use crate::login_ui::ChromeMetrics;

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";
    /// Design 2b's own window size, which is what the titlebar's controls pack
    /// against the right-hand end of.
    const WINDOW_W: f32 = 1240.0;
    const WINDOW_H: f32 = 740.0;
    /// The email the switcher's one row is drawn from, and the string every
    /// "did the menu open?" assertion below looks for.
    const OTHER: &str = "bruno@example.com";

    fn account(id: &str, email: &str) -> Account {
        Account {
            id: AccountId::parse(id).expect("a 32-char lowercase hex id"),
            email: email.to_string(),
            server_url: None,
        }
    }

    fn state() -> AccountsState {
        let active = account(A, "ana@example.com");
        AccountsState::from_blocked_reason(
            vec![active.clone(), account(B, OTHER)],
            active.id,
            None,
        )
        .expect("these accounts are not empty")
    }

    struct Strip {
        ctx: egui::Context,
    }

    /// One frame of the real titlebar chrome: what the gear allocated, where
    /// the chevron landed, where the avatar's initials landed, what strings
    /// were painted, and what the frame asked the window to do.
    struct Bar {
        gear: egui::Rect,
        chevron: egui::Rect,
        initials: egui::Rect,
        texts: Vec<String>,
        started_drag: bool,
    }

    impl Bar {
        /// Whether the switcher's menu is on this frame.
        fn menu_is_open(&self) -> bool {
            self.texts.iter().any(|t| t == OTHER)
        }
    }

    impl Strip {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            theme::apply(&ctx);
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            Self { ctx }
        }

        fn input(events: Vec<egui::Event>) -> egui::RawInput {
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(WINDOW_W, WINDOW_H),
                )),
                events,
                ..Default::default()
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Bar {
            let state = state();
            let mut gear = egui::Rect::NOTHING;
            let output = self.ctx.run_ui(Self::input(events), |ui| {
                let _ = draw_window_chrome_with_extra(
                    ui,
                    WINDOW_TITLE,
                    ChromeMetrics::VAULT,
                    true,
                    CloseControl::Active,
                    |ui| {
                        // The gear FIRST, then `account_menu`, which draws the
                        // chevron and then the avatar itself: right-to-left
                        // packing means earlier is further right, so this lands
                        // as avatar, chevron, gear from the left.
                        gear = theme::gear_button(ui).rect;
                        let _ = account_menu(ui, Some(&state), "AN", None, None, false);
                    },
                );
            });

            let all = egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());
            let chevrons = theme::icon_probe::chevrons(&all);
            assert_eq!(
                chevrons.len(),
                1,
                "expected exactly one chevron in the titlebar, found {}",
                chevrons.len()
            );
            let mut texts = Vec::new();
            let mut initials = None;
            collect_titlebar_text(&all, &mut texts, &mut initials);
            let started_drag = output.viewport_output.values().any(|viewport| {
                viewport
                    .commands
                    .iter()
                    .any(|c| matches!(c, egui::ViewportCommand::StartDrag))
            });
            Bar {
                gear,
                chevron: chevrons[0],
                initials: initials.expect("the titlebar painted no avatar initials"),
                texts,
                started_drag,
            }
        }

        fn idle(&mut self) -> Bar {
            self.frame(Vec::new())
        }

        fn click(&mut self, pos: egui::Pos2) -> Bar {
            self.frame(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ])
        }

        /// A press held down, then one frame of dragging -- which is what
        /// actually starts a window drag. A press and release inside one frame
        /// never reports `drag_started`.
        fn drag_from(&mut self, pos: egui::Pos2) -> Bar {
            let _ = self.frame(vec![egui::Event::PointerMoved(pos)]);
            let _ = self.frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            }]);
            self.frame(vec![egui::Event::PointerMoved(pos + egui::vec2(12.0, 4.0))])
        }
    }

    fn collect_titlebar_text(
        shape: &egui::Shape,
        texts: &mut Vec<String>,
        initials: &mut Option<egui::Rect>,
    ) {
        match shape {
            egui::Shape::Text(text) => {
                let source = text.galley.text().to_string();
                if source == "AN" {
                    *initials = Some(egui::Rect::from_min_size(text.pos, text.galley.size()));
                }
                texts.push(source);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_titlebar_text(shape, texts, initials);
                }
            }
            _ => {}
        }
    }

    /// **Measured rects, not source order.** The user asked for the gear to
    /// the right of the avatar; the switcher goes between them, which is where
    /// a disclosure chevron belongs -- against the thing it discloses.
    #[test]
    fn the_switcher_paints_between_the_avatar_and_the_gear() {
        let bar = Strip::new().idle();
        assert!(
            bar.initials.center().x < bar.chevron.center().x,
            "the switcher paints to the LEFT of the avatar (avatar at {:?}, chevron at \
             {:?}) -- the right-to-left strip inverts the order it is written in, which is \
             exactly how this lands on the wrong side",
            bar.initials.center(),
            bar.chevron.center()
        );
        assert!(
            bar.chevron.center().x < bar.gear.center().x,
            "the switcher paints to the RIGHT of the gear (chevron at {:?}, gear at {:?})",
            bar.chevron.center(),
            bar.gear.center()
        );
        // The controls really are packed against the window controls rather
        // than floating in the middle of a 1240px bar, which is what makes
        // "between" a statement about the group the user sees.
        assert!(
            bar.chevron.center().x > WINDOW_W - 300.0,
            "the titlebar controls are not against the right-hand end of the bar at all; \
             the chevron is at {:?}",
            bar.chevron.center()
        );
    }

    /// **The drag zone stops where the switcher starts.**
    /// `draw_window_chrome_with_extra` registers the drag interaction AFTER
    /// the extra controls and over everything left of them, so a zone that
    /// still covered this control would take its clicks -- and the switcher
    /// would be a chevron that does nothing, with no error anywhere.
    #[test]
    fn the_drag_zone_does_not_swallow_the_switchers_clicks() {
        let mut strip = Strip::new();
        let chevron = strip.idle().chevron;

        let clicked = strip.click(chevron.center());
        assert!(
            !clicked.menu_is_open(),
            "control: a popup paints on the frame AFTER the click that opened it, so this \
             one must not have it yet"
        );
        assert!(
            strip.idle().menu_is_open(),
            "clicking the chevron opened no menu, so the click never reached it -- the drag \
             zone took it"
        );
    }

    /// The positive control for the test above, and the other half of the same
    /// property: a press on the titlebar BESIDE the switcher still drags the
    /// window. Without this, a drag zone shrunk to nothing would pass the test
    /// above while making the window unmovable.
    #[test]
    fn the_titlebar_left_of_the_switcher_still_drags_the_window() {
        let mut strip = Strip::new();
        let chevron = strip.idle().chevron;
        let beside = egui::pos2(chevron.left() - 60.0, chevron.center().y);

        let dragged = strip.drag_from(beside);
        assert!(
            dragged.started_drag,
            "a press at {beside:?} -- on the titlebar, 60px left of the switcher -- started \
             no window drag, so the drag zone has been shrunk to nothing"
        );
        // And the same press does NOT open the switcher's menu, which is what
        // makes the two zones distinct rather than overlapping.
        assert!(
            !strip.idle().menu_is_open(),
            "a press in the drag zone opened the account menu"
        );
    }
}

/// The two halves of the switcher no harness can reach: that `run`'s titlebar
/// strip calls it at all, and where in that strip. Source guards for
/// `settings_gear_placement_tests`' reason -- the strip is a closure inside
/// `run`, which is the eframe application, and rebuilding it in a test would
/// assert the replica rather than the code.
///
/// The needles are `concat!`-split and single-line: a needle written as one
/// literal can match its own declaration, and a needle containing a newline
/// passes on LF and fails on CRLF (this repo has files in both states).
#[cfg(test)]
mod switcher_wiring_tests {
    fn production() -> &'static str {
        let source = include_str!("mod.rs");
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file -- see `window_era_placement_tests`");
        &source[..end]
    }

    fn switcher_needle() -> String {
        concat!("match account_", "menu(").to_string()
    }

    fn gear_needle() -> String {
        concat!("theme::gear_", "button(ui).clicked()").to_string()
    }

    /// The strip packs right-to-left, so the widget added EARLIER ends up
    /// further right: the gear first, then `account_menu`, which draws the
    /// chevron and then the avatar and so puts the chevron between the gear
    /// and the avatar.
    /// `the_switcher_paints_between_the_avatar_and_the_gear` measures the
    /// consequence; this is what ties that measurement to `run`.
    #[test]
    fn the_account_menu_is_added_after_the_gear_and_exactly_once() {
        let production = production();
        let (gear, switcher) = (gear_needle(), switcher_needle());

        // Positive controls. Without these, a rename of either would leave the
        // `find`s returning `None` and the ordering below comparing nothing.
        let gear_at = production
            .find(&gear)
            .unwrap_or_else(|| panic!("no {gear:?} in production code -- the gear is gone"));
        let switcher_at = production.find(&switcher).unwrap_or_else(|| {
            panic!(
                "no {switcher:?} in production code -- the titlebar account menu is gone, so \
                 every click test over it is testing a function nothing calls"
            )
        });
        assert_eq!(
            production.matches(&switcher).count(),
            1,
            "expected exactly one titlebar account menu; more than one and this ordering says \
             nothing"
        );

        assert!(
            gear_at < switcher_at,
            "the account menu is added BEFORE the gear, so the right-to-left strip paints the \
             avatar to the RIGHT of the gear rather than to its left"
        );
    }

    /// **Every row of the menu has to REACH `main`.** This feature set has
    /// repeatedly shipped code complete, correct and unreachable: `account_menu`
    /// can decide all four actions perfectly and `run` can drop three of them
    /// on the floor, with every click test in `account_switcher_tests` still
    /// green. So each arm is sliced out by itself and watched doing its own two
    /// halves -- record the request, then close the window -- because `main`
    /// cannot serve any of them while this window owns the event loop.
    ///
    /// Sliced per arm rather than over one fixed window of bytes: a window that
    /// overran into the next arm would let one arm satisfy another's assertion,
    /// which is precisely the mistake being guarded against.
    #[test]
    fn every_row_of_the_account_menu_records_its_request_and_closes_the_window() {
        let closes = concat!("ViewportCommand::", "Close");
        for (arm, records) in [
            (
                "Switch(picked)",
                concat!("*switch_to_for_", "closure.borrow_mut() = Some(picked);"),
            ),
            (
                "Lock",
                concat!("*locked_for_", "closure.borrow_mut() = true;"),
            ),
            (
                "Add",
                concat!("*add_account_for_", "closure.borrow_mut() = true;"),
            ),
            (
                "Remove",
                concat!("*remove_account_for_", "closure.borrow_mut() = true;"),
            ),
        ] {
            let body = menu_arm_body(arm);
            assert!(
                body.contains(records),
                "the account menu's {arm} row does not record its request, so `main` has \
                 nothing to act on: {body:?}"
            );
            assert!(
                body.contains(closes),
                "the account menu's {arm} row does not close the window, so `main` -- which \
                 cannot run it while this window owns the event loop -- never gets to: {body:?}"
            );
        }
    }

    /// **Four distinct outcomes, and none of them is another.** `locked` and
    /// `needs_reauth` mean the session is gone and both run the full recovery,
    /// which re-authenticates against the account this process is ALREADY on:
    /// folded into either, asking to switch would prompt for the master
    /// password of the account being left and then leave the user on it, and
    /// asking to add would do the same for an account that does not exist yet.
    /// `open_preferences` opens a different window entirely. Lock is the one
    /// row that SHOULD set `locked`, and must set nothing else — it is a
    /// security action, and it lands on sign-in for the account that was
    /// locked rather than switching anywhere.
    #[test]
    fn each_row_of_the_account_menu_sets_its_own_flag_and_no_others() {
        let flags = [
            ("Switch(picked)", concat!("switch_to_for_", "closure")),
            ("Lock", concat!("locked_for_", "closure")),
            ("Add", concat!("add_account_for_", "closure")),
            ("Remove", concat!("remove_account_for_", "closure")),
        ];
        // Never set by any row: Preferences is a different window, and an
        // expired session is something a click cannot mean.
        let never = [
            (
                concat!("needs_reauth_for_", "closure"),
                "flags an expired session, so it runs the re-authentication path",
            ),
            (
                concat!("open_preferences_for_", "closure"),
                "asks for the Preferences window instead",
            ),
        ];

        for (arm, own) in flags {
            let body = menu_arm_body(arm);
            // The positive control, per arm: the slice really is this row's
            // block, which every `!contains` below would otherwise pass against.
            assert!(
                body.contains(own),
                "the sliced body is not the {arm} row at all: {body:?}"
            );
            for (other, _) in flags.iter().filter(|(label, _)| *label != arm) {
                // `locked_for_closure` is a substring of nothing else here, but
                // `add_account_for_closure` is NOT a substring of
                // `remove_account_for_closure` either way round, so a plain
                // `contains` is sound for every pair.
                assert!(
                    !body.contains(other),
                    "the account menu's {arm} row also writes {other:?}, so two outcomes \
                     leave this window as one: {body:?}"
                );
            }
            for (needle, consequence) in never {
                assert!(
                    !body.contains(needle),
                    "the account menu's {arm} row also {consequence}: {body:?}"
                );
            }
        }
    }

    /// The last hop, and the one with nothing else watching it: the cell the
    /// click writes into has to be read back out into `VaultWindowResult`
    /// after the window closes. `run` is the eframe application, so no test
    /// can watch a real window hand its result back -- and a `switch_to` left
    /// hard-`None` there would make every click test above pass against a
    /// switcher whose answer never leaves the window.
    #[test]
    fn the_recorded_pick_is_read_back_out_into_the_result() {
        let production = production();
        let result = concat!("VaultWindowResult { locked, needs_reauth,", " open_preferences,");

        for (read_back, what) in [
            (
                concat!("let switch_to = self.switch_to.borrow_mut()", ".take();"),
                "the switcher's pick",
            ),
            (
                concat!("let add_account = *self.add_", "account.borrow();"),
                "the \"Add account...\" request",
            ),
            (
                concat!("let remove_account = *self.remove_", "account.borrow();"),
                "the \"Remove this account...\" request",
            ),
        ] {
            assert!(
                production.contains(read_back),
                "the cell {what} is written into is never read back, so it dies with the window"
            );
        }
        let at = production
            .find(result)
            .expect("no `VaultWindowResult` construction in production code");
        let rest = &production[at..];
        let construction = &rest[..rest.len().min(200)];
        // `switch_to,` and not `switch_to }`: this field is no longer last in
        // the construction (`account_details` follows it), and a needle that
        // spelled the closing brace would fail for a reason that has nothing
        // to do with the switcher the moment another field is appended.
        for field in [
            concat!("switch", "_to, "),
            concat!("add_", "account, "),
            concat!("remove_", "account, "),
        ] {
            assert!(
                construction.contains(field),
                "the result is built without {field:?}, so that row's answer never leaves the \
                 window: {construction:?}"
            );
        }
        // Positive control: the construction really was isolated, rather than
        // being the whole rest of the file.
        assert!(
            construction.len() < production.len(),
            "control: the slice isolated a region"
        );
    }

    /// The body of one arm of the strip's `match account_menu(...)`, depth-
    /// counted from that arm's own `{` to its matching `}`.
    ///
    /// Depth-counted rather than taken as a fixed number of bytes: a byte
    /// window long enough to hold one arm is long enough to overrun into the
    /// next, and the failure that produces is one arm quietly satisfying
    /// another's assertion.
    ///
    /// **Searched from the `match` rather than from the top of the file**, so a
    /// second `AccountAction::Lock` somewhere else in production cannot be what
    /// gets sliced.
    fn menu_arm_body(arm: &str) -> &'static str {
        let production = production();
        let switcher = switcher_needle();
        let at = production.find(&switcher).unwrap_or_else(|| {
            panic!("no {switcher:?} in production code -- the titlebar account menu is gone")
        });
        let region = &production[at..];
        // Single-line needle, and split so it cannot match this line itself.
        let head = format!("{}{arm}) => {{", concat!("Some(AccountAction", "::"));
        let arm_at = region.find(&head).unwrap_or_else(|| {
            panic!(
                "the strip's `match` has no {head:?} arm -- the {arm} row decides something \
                 nothing acts on"
            )
        });
        let after_open = &region[arm_at + head.len()..];

        let mut depth = 1usize;
        for (offset, ch) in after_open.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let body = &after_open[..offset];
                        assert!(
                            !body.trim().is_empty(),
                            "the {arm} arm's block is empty, so every assertion over it would \
                             pass against nothing"
                        );
                        assert!(
                            body.len() < region.len() / 4,
                            "control: the slice isolated one arm rather than running on into \
                             the rest of the strip"
                        );
                        return body;
                    }
                }
                _ => {}
            }
        }
        panic!("the {arm} arm's block is never closed");
    }
}

/// The seam [`build_frame`] opened, held from the source.
///
/// None of this can be observed by running anything: [`run`] blocks on a real
/// winit event loop and opens a real OS window, so no test in this crate calls
/// it. What the split newly makes *possible to get wrong* is the tray-click
/// host quietly losing a step that used to be inline -- the geometry write and
/// the outcome read both moved into [`VaultFrameHandles::finish`], and a `run`
/// that dropped the handles and returned a fresh `VaultWindowResult` compiles,
/// passes every other test in this file, and ships a vault window that forgets
/// its size and can never lock, switch account or open Preferences again.
#[cfg(test)]
mod frame_host_tests {
    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    /// Everything before the first `#[cfg(test)]`. Split with `concat!` so the
    /// marker exists in the binary but appears in this file only where the
    /// real attributes are -- otherwise this needle would find ITSELF, at a
    /// position above all the production code, and every slice below would be
    /// empty (and every `!contains` assertion vacuously true).
    fn production() -> &'static str {
        let source = source();
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file");
        &source[..end]
    }

    /// `run`'s body: from its signature to the end of production code.
    fn run_body() -> &'static str {
        let production = production();
        let at = production
            .find(concat!("pub fn run<A: UiAutomationFiller", " + Clone + 'static,"))
            .expect(
                "no `pub fn run` in this file -- the tray-click host was renamed or deleted; \
                 if renamed, update this needle",
            );
        &production[at..]
    }

    #[test]
    fn the_tray_click_host_runs_the_shared_frame_rather_than_a_second_copy_of_it() {
        let body = run_body();
        assert!(
            body.contains(concat!("build_", "frame(")),
            "`run` no longer calls `build_frame`, so the vault UI the tray opens is not the \
             one the single-window host draws: {body:?}"
        );
        // The whole point of the split: the closure body lives in ONE place.
        // A second `move |ui: &mut egui::Ui` inside `run` would be a copy of
        // it, which is the shape this refactor exists to prevent.
        assert_eq!(
            production().matches(concat!("move |ui: ", "&mut egui::Ui")).count(),
            1,
            "there is more than one vault frame closure in this file; `build_frame`'s is \
             meant to be the only one"
        );
    }

    #[test]
    fn the_tray_click_host_still_ends_its_session_through_finish() {
        let body = run_body();
        assert!(
            body.contains(concat!("handles.", "finish()")),
            "`run` never calls `finish`, so closing the vault window neither saves its \
             geometry nor reports a lock, a re-auth, a Preferences request or an account \
             switch -- all four outcomes die with the window: {body:?}"
        );
        // Positive control on the slice: `run_body` really isolated `run` and
        // did not hand back something that trivially contains anything.
        assert!(
            !body.contains(concat!("let mut styled = ", "pre_styled;")),
            "control: `run_body` reaches back into `build_frame`, so the assertions above \
             may be satisfied by `build_frame`'s own text rather than `run`'s"
        );
    }

    #[test]
    fn the_shared_frame_is_built_without_opening_a_window() {
        let production = production();
        let build_at = production
            .find(concat!("pub fn build_", "frame<A: UiAutomationFiller"))
            .expect("no `build_frame` in this file");
        let run_at = production
            .find(concat!("pub fn run<A: UiAutomationFiller", " + Clone + 'static,"))
            .expect("no `run` in this file");
        assert!(
            build_at < run_at,
            "control: `build_frame` is expected above `run`, and the slice below assumes it"
        );
        let build_body = &production[build_at..run_at];
        // The CALL, not the words: this file's prose says "run_ui_native" in
        // several doc comments, and a needle that matched those would fail
        // here for the wrong reason and, worse, would pass the count below
        // only by accident of how much prose happened to be written.
        const CALL: &str = concat!("eframe::run_ui_", "native(");
        assert!(
            !build_body.contains(CALL),
            "`build_frame` opens its own event loop, so the single-window host calling it \
             would nest one native event loop inside another -- which eframe cannot do. \
             The loop belongs to the host, not to the frame."
        );
        // Positive control: `run_ui_native` really is findable in this file,
        // so the negative above is about where it is and not about a needle
        // that never matches anything.
        assert_eq!(
            production.matches(CALL).count(),
            1,
            "expected exactly one event loop in this file, in `run`"
        );
    }
}

/// **The window opens without the account details, and says nothing untrue
/// while it waits for them.**
///
/// `open_vault_window` used to begin with a synchronous `bw status` on a
/// prefetch miss -- 2.39 seconds measured on the user's machine, spent before
/// eframe was asked for a window, so the click produced nothing on screen at
/// all. What it fetched is the avatar's initials and the host favicons come
/// from. The window now opens first and drains them, the way the item list is
/// already drained.
///
/// Half of that is drawing, which no test can run: `eframe::Frame` has no
/// public constructor, so the frame closure cannot be called. What CAN be run
/// is what the toolbar shows for an absent email and what `finish` reports;
/// the rest is held from the source, the way this file already holds its
/// other closure-only decisions.
#[cfg(test)]
mod account_details_tests {
    use super::*;

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    /// Everything before the first `#[cfg(test)]`. `concat!`-split so this
    /// needle does not find its own declaration and leave every slice empty.
    fn production() -> &'static str {
        let source = source();
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file");
        &source[..end]
    }

    /// `source` with every `//` line comment removed, so an assertion about
    /// the CODE cannot be satisfied by prose that merely names the thing.
    ///
    /// A `//` inside a string literal over-strips the rest of that line. That
    /// only ever removes text a check is looking for, so it can turn a pass
    /// into a failure and never the other way round -- the safe direction for
    /// a guard.
    fn code_only(source: &str) -> String {
        source
            .lines()
            .map(|line| match line.find("//") {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn an_account_whose_details_have_not_arrived_gets_an_empty_circle() {
        assert_eq!(
            avatar_initials(None),
            "",
            "the avatar paints something for an account nothing is known about yet"
        );
        // The value this must NOT be, read off the helper it would come from
        // rather than asserted as a literal: routing `None` through
        // `theme::initials` is the one-character edit that breaks this, and
        // "?" is what it produces.
        assert_ne!(
            avatar_initials(None),
            theme::initials(""),
            "the avatar shows `theme::initials`' placeholder for an account whose details are \
             merely still in flight -- a glyph that reads as an answer about the account \
             (\"this one has no name\") when nothing has been said about it yet"
        );
    }

    /// The positive control for the test above, and the case that is not a
    /// placeholder at all: details that HAVE arrived are drawn.
    #[test]
    fn an_account_that_is_known_gets_its_initials() {
        assert_eq!(
            avatar_initials(Some("a.novak@ledgerline.com")),
            "AN",
            "a known account's avatar is not its initials, so the empty case above is not a \
             transition to anything"
        );
    }

    /// The details the window ended up with have to leave it: `main` refills
    /// its one-entry cache from here, and that cache is what keeps the NEXT
    /// open from spawning the CLI at all.
    #[test]
    fn the_details_the_window_used_are_read_back_out_into_the_result() {
        let details = crate::login_ui::BwStatusDetails {
            status: crate::login_ui::BwStatus::Unlocked,
            user_email: Some("held@example.eu".to_string()),
            server_url: Some("https://vault.example.eu".to_string()),
        };
        let handles = VaultFrameHandles {
            locked: Rc::new(RefCell::new(false)),
            needs_reauth: Rc::new(RefCell::new(false)),
            open_preferences: Rc::new(RefCell::new(false)),
            switch_to: Rc::new(RefCell::new(None)),
            add_account: Rc::new(RefCell::new(false)),
            remove_account: Rc::new(RefCell::new(false)),
            account_details: Rc::new(RefCell::new(Some(details.clone()))),
            last_geometry: Rc::new(RefCell::new(None)),
            // No path, so `finish` writes no file: this test is about the
            // outcome read, and geometry has its own tests.
            settings_path: None,
        };

        assert_eq!(
            handles.finish().account_details.as_ref(),
            Some(&details),
            "`finish` drops the account details, so every open re-spawns `bw status` -- the \
             cache `main` keeps can never be warm again"
        );

        // The other half: a window closed before its fetch reported back has
        // nothing to hand back, and must not invent something.
        let unheld = VaultFrameHandles {
            locked: Rc::new(RefCell::new(false)),
            needs_reauth: Rc::new(RefCell::new(false)),
            open_preferences: Rc::new(RefCell::new(false)),
            switch_to: Rc::new(RefCell::new(None)),
            add_account: Rc::new(RefCell::new(false)),
            remove_account: Rc::new(RefCell::new(false)),
            account_details: Rc::new(RefCell::new(None)),
            last_geometry: Rc::new(RefCell::new(None)),
            settings_path: None,
        };
        assert!(
            unheld.finish().account_details.is_none(),
            "control: `finish` reports details even when the cell is empty, so the assertion \
             above is satisfied by a constant"
        );
    }

    /// The drain is non-blocking, like every other drain in this closure.
    ///
    /// `recv()` here would be the freeze this change removes, moved inside the
    /// window where it looks like a hang instead of a slow launch: the frame
    /// closure would sit on the channel and the window would stop repainting,
    /// stop dragging and stop closing for the whole `bw status` spawn.
    #[test]
    fn the_frame_never_blocks_on_the_account_details() {
        let production = production();
        assert!(
            production
                .contains(concat!("details_rx.as_ref().and_then(|rx| rx.try_", "recv().ok())")),
            "the frame no longer drains the account-details channel with `try_recv`"
        );
        assert!(
            !production.contains(concat!("details_rx.", "recv()")),
            "the frame closure BLOCKS on the account details, which freezes the window for as \
             long as the `bw` CLI takes -- exactly the wait this change moved off the open, \
             now paid with a window on screen that does not respond"
        );
        // Positive control: `recv` is findable in this file at all, so the
        // negative above is about `details_rx` and not about a needle that
        // could never match anything.
        assert!(
            production.contains(concat!("try_", "recv()")),
            "control: this file has no channel drains at all, so the assertion above says \
             nothing"
        );
    }

    /// The avatar is drawn unconditionally, and shows `avatar_initials`.
    ///
    /// It used to be inside `if let Some(email) = &account_email`, which is
    /// correct-looking and, once the email can arrive a second into the
    /// session, means the toolbar RELAYOUTS while the user is looking at it:
    /// the sync pill and the title slide sideways when a 28px circle appears
    /// between them.
    ///
    /// Source-level for the strip's half only. `account_menu` now owns the
    /// circle, and that it draws one even for an app with no account list is
    /// pressed rather than read -- see
    /// `an_app_with_no_account_list_still_wears_its_avatar_but_opens_no_menu`.
    #[test]
    fn the_toolbar_reserves_the_avatar_rather_than_growing_one_later() {
        let production = production();
        assert!(
            production.contains(concat!("&avatar_initials(account_email", ".as_deref()),")),
            "the titlebar avatar is no longer painted through `avatar_initials`, so nothing \
             says what it shows before the account details arrive"
        );
        assert!(
            !production.contains(concat!("if let Some(email) = &account", "_email")),
            "the avatar is drawn only once the email has arrived, so the toolbar shifts \
             sideways mid-session when it does"
        );
    }

    /// Favicons wait for the server URL instead of guessing it.
    ///
    /// `favicon::icon_base_url(None)` is Bitwarden's own cloud, and the
    /// on-disk icon cache is keyed by DOMAIN ALONE -- so a self-hosted
    /// account whose URL simply had not arrived yet would not just draw the
    /// wrong icons in this window, it would write them to disk for every
    /// later window that does know the server.
    #[test]
    fn no_icon_is_fetched_before_the_server_url_is_known() {
        let production = production();
        assert!(
            production.contains(concat!("let icon_base_known = details_rx.is_", "none();")),
            "nothing in the frame records whether the favicon base URL is known yet"
        );

        let call = concat!("ensure_icon_", "loaded(");
        let sites: Vec<usize> = production
            .match_indices(call)
            // The definition, not a call.
            .filter(|(at, _)| !production[..*at].ends_with("fn "))
            .map(|(at, _)| at)
            .collect();
        assert_eq!(
            sites.len(),
            2,
            "expected the two icon call sites (the visible rows and the detail pane); found \
             {} -- if one was added, it needs the same guard, and if one was removed this \
             test is checking less than it says",
            sites.len()
        );
        for at in sites {
            let before = &production[..at];
            // 400 bytes back: each call site's guard sits within ~200 of it
            // (a `for`/`if let` head). Bounded rather than "anywhere
            // earlier", which would be satisfied by the declaration itself
            // thousands of lines up and would assert nothing.
            //
            // COMMENTS STRIPPED FIRST, and that is not a detail: both call
            // sites are preceded by a comment that explains the guard by
            // name, so the first version of this test passed against a call
            // site whose guard had been deleted and whose comment had not.
            let window = code_only(&before[before.len().saturating_sub(400)..]);
            assert!(
                window.contains("icon_base_known"),
                "an `ensure_icon_loaded` call at byte {at} is not guarded by \
                 `icon_base_known`: with no server URL yet it fetches from Bitwarden's cloud \
                 and writes the result into a domain-keyed disk cache that every later \
                 window reads"
            );
        }
    }

    /// **The account menu is handed the same fact, not a fresh reading of it.**
    /// Its host line has the identical problem the favicons have: "no server
    /// URL" means Bitwarden's own cloud once `bw status` has answered and means
    /// nothing at all before it has. `account_host_line`'s three cases are
    /// tested by clicking, but which value the strip passes for `server_known`
    /// is inside `run`'s closure, which no harness here can call -- and `true`
    /// hard-coded there compiles and makes every one of those tests go on
    /// passing while the real window claims a first-run account is on
    /// Bitwarden's cloud.
    #[test]
    fn the_account_menu_waits_for_the_server_url_the_way_the_favicons_do() {
        let production = production();
        let call = concat!("match account_", "menu(");
        let at = production
            .find(call)
            .expect("the titlebar account menu is gone");
        let rest = &production[at..];
        // Forward from the call to its closing paren, comments stripped: the
        // comment above the argument names `icon_base_known` out loud, and a
        // window that included it would pass against an argument that had been
        // changed to `true`.
        let end = rest.find(") {").expect("the account menu call is never closed");
        let args = code_only(&rest[..end]);
        assert!(
            args.contains("icon_base_known"),
            "the account menu is not handed `icon_base_known`, so its host line answers \
             \"which server?\" from a second reading of a fact this window already has one \
             name for: {args:?}"
        );
        assert!(
            !args.contains("true"),
            "the account menu is told the server is known unconditionally, so a first-run \
             account -- which has no server URL and has not been asked about -- is reported \
             as being on Bitwarden's cloud: {args:?}"
        );
        assert!(
            args.len() < 1200,
            "control: the slice isolated the call's arguments rather than running on into the \
             rest of the strip"
        );
    }
}


/// The Open-the-app path: the sentence a refusal shows, driven directly, and a
/// source-text guard on the arm that runs the program.
///
/// **Why a source guard for half of it.** The arm lives inside `run`'s update
/// closure, which needs a real event loop and is unreachable from this suite --
/// the same reason [`refused_write_wiring_tests`] exists, and these follow that
/// module line for line. It is doubly true here: the thing the arm does is
/// `CreateProcess`, and a test that exercised it would start a program on the
/// machine running the suite.
///
/// **What the source guards do not guarantee**, so the doc claims no coverage
/// it lacks: they see spellings and counts, not behaviour. What they catch is
/// the edit that deletes the launch, or the launch that stops reporting its
/// failure -- and, deliberately, the Windows details a refactor would quietly
/// drop: `raw_arg` (without which `--profile-directory="Profile 2"` is
/// re-quoted into something else), the working directory, and the emptiness
/// test that keeps a bare launch off a command line that is nothing but a
/// space.
///
/// **`CREATE_BREAKAWAY_FROM_JOB` is no longer among them, and that is the
/// point.** It used to be guarded by counting its spelling, which counted the
/// `if` arm rather than the call that selects it -- so `spawn(true)` could
/// become `spawn(false)`, removing the flag from every launch in the program,
/// with the guard still reporting it present and the whole suite green. Two
/// of the tests below are ordinary unit tests instead:
/// [`creation_flags_for`] is called with both inputs, and [`launch_with`] is
/// driven with a fake spawner that records the sequence of attempts. Neither
/// constructs a `Command`; the source guard's remaining job is only to pin
/// that the real launcher goes through them.
///
/// The DECISION about whether a launch may happen at all is not here and not
/// in this file: it is `detail::app_launch_plan`, which `detail`'s own suite
/// drives directly.
#[cfg(test)]
mod open_app_wiring_tests {
    use super::*;

    // EVERY NEEDLE IS SPLIT WITH `concat!`, for `refused_write_wiring_tests`'
    // reason: `include_str!("mod.rs")` pulls this module in too, so a needle
    // written as one literal always matches itself. The count assertions
    // ENFORCE the split -- re-joining one makes it appear an extra time and
    // fails.
    const ARM: &str = concat!("DetailAction::OpenApp", "(plan) => {");
    /// What the arm does instead of launching: it records the request.
    const RECORDS: &str = concat!("PendingLaunch::", "new(item.name.clone(), plan)");
    /// The drain block, which is where every launch in this program happens.
    const DRAIN: &str = concat!("if let Some(pending) = pending_launch.", "take() {");
    /// The gate. A plan that has not been approved does not reach the
    /// launcher; making this `false` launches everything on the click.
    const GATE: &str = concat!("if pending.", "needs_confirmation {");
    /// The dialog, handed **the pending launch itself** -- the same value the
    /// launcher below reads its plan out of, so the string on screen and the
    /// string that runs cannot be two different plans.
    const CONFIRMS: &str = concat!("draw_launch_confirm_modal(ui.ctx(), ", "&pending)");
    /// The only outcome that reaches the launcher.
    const APPROVED: &str = concat!("LaunchConfirmAction::", "Launch => {");
    /// The call that actually starts the program. Delete it and this fails.
    const LAUNCHES: &str = concat!("launch_app", "(&pending.plan)");
    /// One needle for both halves of the failure report: the assignment that
    /// puts a sentence in the band AND the call that decides the sentence.
    const REPORTS: &str = concat!("move_error = Some(app_launch_failure", "_message(");
    /// What the band is told to name. The failure sentence quotes a file so
    /// the user has something to fix; handing it the *tail* instead would name
    /// the arguments, which the unit test cannot catch because it calls
    /// `app_launch_failure_message` with its own literals.
    const NAMES_THE_PROGRAM: &str = concat!("&pending.plan.", "program,");
    /// The other field of the plan, which must NOT appear in that call.
    const NAMES_THE_TAIL: &str = concat!("&pending.plan.", "raw_tail,");
    /// The spawn itself, inside `launch_app`.
    const SPAWNS: &str = concat!(".spawn", "()");
    /// The verbatim command-line tail. `\u{2e}args(` here instead would
    /// re-quote it.
    const RAW: &str = concat!("command.raw_arg", "(&plan.raw_tail)");
    /// The working directory, which is the program's own folder and not
    /// Deskwarden's -- see [`launch_app`]'s doc for the DLL-search-order
    /// reason. Deleting it used to change nothing any test could see.
    const CHDIR: &str = concat!("command.current_dir", "(dir)");
    /// The emptiness test that keeps a bare launch from getting a command line
    /// that is nothing but a trailing space. Deleting the `if` used to change
    /// nothing any test could see.
    const TAIL_GUARD: &str = concat!("if !plan.raw_tail.is_", "empty() {");
    /// The one place the flags are computed. A test calls
    /// [`creation_flags_for`] with both inputs; this pins that the spawn
    /// closure asks it about the attempt it is actually making, rather than
    /// hard-coding one answer.
    const FLAGS_FROM_ATTEMPT: &str = concat!(".creation_flags(creation_flags_", "for(breakaway))");
    /// The real launcher goes through the policy that
    /// [`the_first_attempt_asks_for_breakaway_and_only_access_denied_falls_back`]
    /// drives, rather than open-coding a second copy of it.
    const USES_POLICY: &str = concat!("launch_with(&plan.program,", " |breakaway| {");

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.match_indices(needle).count()
    }

    /// The drain block: from the `take` to the comment that has always
    /// followed the frame closure's tail.
    fn drain_body(source: &'static str) -> &'static str {
        let at = source.find(DRAIN).expect("the drain block is there");
        let end = source[at..]
            .find(concat!("// NOTHING SCHEDULES THE NEXT", " FRAME HERE ANY MORE."))
            .expect("the drain block is followed by the repaint comment it always was");
        &source[at..at + end]
    }

    /// The `DetailAction::OpenApp` arm, up to the comment that has always
    /// followed it.
    fn arm_body(source: &'static str) -> &'static str {
        let at = source.find(ARM).expect("the arm is there");
        let end = source[at..]
            .find(concat!("// `set_favorite` returns the written", " item"))
            .expect("the arm is followed by the ToggleFavorite comment it always was");
        &source[at..at + end]
    }

    /// **The arm does not launch, it asks.**
    ///
    /// This used to be `the_open_app_arm_launches_and_reports_what_it_could_
    /// not_launch`, and it pinned the launch and the failure report *inside*
    /// the arm. They have moved, together, into the one block that drains
    /// `pending_launch` -- and that move is the security control, not a
    /// tidy-up: with exactly one `launch_app` in the program, reachable only
    /// from a `LaunchConfirmAction::Launch`, a launch that skipped the
    /// confirmation would have to be a *second* call, which the count below
    /// refuses. The assertions this test used to make are all still here, on
    /// the drain block; the ones about the arm are new and are the strict
    /// half: the arm must contain no launch and no report at all.
    #[test]
    fn the_open_app_arm_records_the_request_and_starts_nothing_itself() {
        let source = source();
        assert_eq!(occurrences(source, ARM), 1, "expected exactly one {ARM:?}");
        let arm = arm_body(source);
        assert_eq!(
            occurrences(arm, RECORDS),
            1,
            "the Open arm does not record the request through {RECORDS:?}, so whatever it \
             does instead has not been through `launch_needs_confirmation`: {arm}"
        );
        assert_eq!(
            occurrences(arm, LAUNCHES),
            0,
            "the Open arm starts a program itself, which is a launch that never passes the \
             confirmation: {arm}"
        );
        assert_eq!(
            occurrences(arm, concat!("launch_", "app(")),
            0,
            "the Open arm calls the launcher under some other spelling: {arm}"
        );
        assert!(
            arm.len() < 1400,
            "control: the slice isolated the arm rather than running on into the rest of \
             the closure"
        );
    }

    /// **The one launch, and what it is gated on.**
    #[test]
    fn the_only_launch_in_the_program_is_the_one_the_user_approved() {
        let source = source();
        assert_eq!(
            occurrences(source, LAUNCHES),
            1,
            "expected {LAUNCHES:?} exactly once -- a second call to the launcher anywhere in \
             this file is a launch that does not go past the confirmation"
        );
        assert_eq!(
            occurrences(source, REPORTS),
            1,
            "expected {REPORTS:?} exactly once -- a launch that fails is silent, and a click \
             that starts nothing and says nothing is indistinguishable from a missed button"
        );
        let body = drain_body(source);
        assert_eq!(
            occurrences(body, GATE),
            1,
            "the drain block does not ask whether this plan still needs confirming, so it \
             either confirms every launch or none: {body}"
        );
        assert_eq!(
            occurrences(body, CONFIRMS),
            1,
            "the dialog is not drawn from the pending launch itself, so the command line on \
             screen and the command line that runs can be two different plans: {body}"
        );
        assert_eq!(
            occurrences(body, APPROVED),
            1,
            "expected exactly one {APPROVED:?} -- the launcher must hang off the approving \
             arm and nothing else: {body}"
        );
        // The launch is inside the approving arm, not merely inside the block.
        let approved_at = body.find(APPROVED).expect("the approving arm is there");
        let approved = &body[approved_at..];
        assert_eq!(
            occurrences(approved, LAUNCHES),
            1,
            "the launch is not under `LaunchConfirmAction::Launch`, so it runs whatever the \
             dialog answered: {body}"
        );
        assert_eq!(occurrences(body, LAUNCHES), 1, "the launch is not in the drain block: {body}");
        assert_eq!(occurrences(body, REPORTS), 1, "the report is not in the drain block: {body}");
        // And it names the PROGRAM FILE, not the arguments. The unit test on
        // `app_launch_failure_message` passes its own literals, so it is blind
        // to which field the call site hands over.
        assert_eq!(
            occurrences(body, NAMES_THE_PROGRAM),
            1,
            "the failure band is not told the program file: it quotes a path so the user has \
             something to fix, and any other field leaves \u{201c}couldn\u{2019}t open the \
             app\u{201d} with nothing actionable in it: {body}"
        );
        assert_eq!(
            occurrences(body, NAMES_THE_TAIL),
            0,
            "the failure band is being handed the command-line tail, so it names the \
             ARGUMENTS where the program file belongs: {body}"
        );
        assert!(
            body.len() < 2600,
            "control: the slice isolated the drain block rather than running on into the \
             rest of the closure"
        );
    }

    #[test]
    fn the_launcher_spawns_the_plan_verbatim_and_out_of_any_job() {
        let source = source();
        assert_eq!(
            occurrences(source, SPAWNS),
            1,
            "expected {SPAWNS:?} exactly once -- `launch_app` is the only thing in this file \
             that starts a process"
        );
        assert_eq!(
            occurrences(source, RAW),
            1,
            "expected {RAW:?} exactly once -- passing the tail through `Command::args` would \
             re-quote --profile-directory=\"Profile 2\" into a different flag"
        );
        assert_eq!(
            occurrences(source, TAIL_GUARD),
            1,
            "expected {TAIL_GUARD:?} exactly once -- without it a plan with nothing to add \
             still calls `raw_arg`, and the program is started with a command line whose \
             whole tail is a space"
        );
        assert_eq!(
            occurrences(source, CHDIR),
            1,
            "expected {CHDIR:?} exactly once -- without it the child inherits Deskwarden's \
             working directory, and a program that loads a DLL beside itself searches \
             Deskwarden's folder first"
        );
        assert_eq!(
            occurrences(source, FLAGS_FROM_ATTEMPT),
            1,
            "expected {FLAGS_FROM_ATTEMPT:?} exactly once -- the spawn must ask \
             `creation_flags_for` about the attempt it is making; hard-coding either answer \
             makes the retry a no-op or removes the flag from every launch"
        );
        assert_eq!(
            occurrences(source, USES_POLICY),
            1,
            "expected {USES_POLICY:?} exactly once -- the real launcher must go through the \
             same `launch_with` the sequence test drives, not open-code a second copy"
        );
    }

    /// The flag, proven as a value.
    ///
    /// **This is the test the old source-text needle was not.** The needle
    /// counted `CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB` as it appeared
    /// in an `if` arm; it could not tell whether anything ever selected that
    /// arm. Here the mapping is called.
    #[test]
    fn breakaway_is_a_flag_that_is_actually_set_and_dropped_by_the_bool() {
        let with = creation_flags_for(true);
        let without = creation_flags_for(false);

        assert_ne!(
            with, without,
            "both attempts ask for the same flags, so the fallback retries with exactly what \
             was just refused and can only fail again"
        );
        assert_eq!(
            with,
            crate::bw_path::CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB,
            "the breakaway attempt does not carry CREATE_BREAKAWAY_FROM_JOB"
        );
        assert_eq!(
            without,
            crate::bw_path::CREATE_NO_WINDOW,
            "the fallback attempt carries something other than CREATE_NO_WINDOW alone"
        );
        // Both keep the console suppressed: a console-subsystem program must
        // not flash a window on either path.
        assert_eq!(with & crate::bw_path::CREATE_NO_WINDOW, crate::bw_path::CREATE_NO_WINDOW);
        assert_eq!(without & crate::bw_path::CREATE_NO_WINDOW, crate::bw_path::CREATE_NO_WINDOW);
        // And the fallback really is the one WITHOUT breakaway, stated as the
        // bit rather than as inequality with the whole value.
        assert_eq!(with & CREATE_BREAKAWAY_FROM_JOB, CREATE_BREAKAWAY_FROM_JOB);
        assert_eq!(without & CREATE_BREAKAWAY_FROM_JOB, 0);
    }

    /// The sequence of attempts, proven by being the spawner.
    ///
    /// **Nothing here constructs or runs a `Command`.** The fake records the
    /// `bool` it was handed and returns whatever the case under test needs,
    /// so the assertions are about the argument that reaches the flag
    /// computation -- the exact thing three separate mutations used to be able
    /// to change with the whole suite green.
    #[test]
    fn the_first_attempt_asks_for_breakaway_and_only_access_denied_falls_back() {
        fn attempts(mut outcome: impl FnMut(usize, bool) -> std::io::Result<()>) -> Vec<bool> {
            let mut seen = Vec::new();
            let _ = launch_with(r"C:\Apps\Ledgerline\Ledgerline.exe", |breakaway| {
                seen.push(breakaway);
                outcome(seen.len() - 1, breakaway)
            });
            seen
        }
        let access_denied = || std::io::Error::from_raw_os_error(ACCESS_DENIED_ERRNO);

        // A launch that works asks for breakaway, once, and stops.
        assert_eq!(
            attempts(|_, _| Ok(())),
            vec![true],
            "the successful launch does not ask for breakaway (or asks twice)"
        );

        // ACCESS_DENIED -- the containing job forbids breakaway -- retries
        // WITHOUT it, and the retry is the last attempt.
        assert_eq!(
            attempts(|n, _| if n == 0 { Err(access_denied()) } else { Ok(()) }),
            vec![true, false],
            "a job that forbids breakaway does not produce a second attempt without the flag: \
             either Open fails forever inside such a job, or the retry asks for the very flag \
             that was just refused"
        );

        // Any OTHER error is the program's problem, not the job's, and must
        // not be retried -- a missing .exe is not going to appear on a second
        // CreateProcess, and a retry would double every failure's latency.
        let missing = attempts(|_, _| Err(std::io::Error::from(std::io::ErrorKind::NotFound)));
        assert_eq!(missing, vec![true], "a missing program file was retried: {missing:?}");

        // Control: the fake is really being consulted -- if `launch_with`
        // ignored the closure's result the two cases above would be identical.
        assert_ne!(attempts(|_, _| Err(access_denied())), attempts(|_, _| Ok(())));

        // And the error the caller sees is the FIRST one when there is no
        // retry, and the RETRY's when there is.
        let refused = launch_with("x", |b| {
            if b {
                Err(access_denied())
            } else {
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            }
        });
        assert_eq!(
            refused.unwrap_err().kind(),
            std::io::ErrorKind::NotFound,
            "the fallback's own failure is swallowed and the job's ACCESS_DENIED is reported \
             instead, which points the band at the wrong cause"
        );
    }

    /// The one thing in this path that is a decision rather than plumbing:
    /// what the band says.
    #[test]
    fn a_missing_program_file_is_reported_differently_from_a_refused_one() {
        let missing = app_launch_failure_message(
            "Ledgerline",
            r"C:\Apps\Ledgerline\Ledgerline.exe",
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        let refused = app_launch_failure_message(
            "Ledgerline",
            r"C:\Apps\Ledgerline\Ledgerline.exe",
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert_ne!(
            missing, refused,
            "two different failures produce one sentence, so the band cannot tell a moved \
             program from a refused one"
        );
        assert!(missing.contains("moved, renamed or uninstalled"), "{missing}");
        assert!(refused.contains("refused to start it"), "{refused}");

        for message in [&missing, &refused] {
            // The item, so a band that outlives the selection still names
            // what it is about; and the file, because "couldn't open the app"
            // without saying which file it tried leaves nothing to fix.
            assert!(message.contains("Ledgerline"), "{message}");
            assert!(message.contains(r"C:\Apps\Ledgerline\Ledgerline.exe"), "{message}");
            assert!(message.contains("Nothing was started"), "{message}");
            // No write was attempted, so it must not talk about the vault.
            assert!(!message.contains("vault"), "{message}");
        }

        // A third kind gets its own generic clause, and not one of the two
        // specific ones -- the fallback is not a copy of a neighbour.
        let other = app_launch_failure_message(
            "Ledgerline",
            r"C:\Apps\Ledgerline\Ledgerline.exe",
            &std::io::Error::from(std::io::ErrorKind::Other),
        );
        assert_ne!(other, missing);
        assert_ne!(other, refused);
    }
}

/// **The confirmation is a security control, so it is tested as one.**
///
/// The whole feature is one claim: a launch whose command line carries vault
/// data does not happen until the user has seen that command line, in full,
/// and clicked through it. [`open_app_wiring_tests`] holds the half that lives
/// inside `run`'s update closure -- there is exactly one `launch_app`, and it
/// hangs off `LaunchConfirmAction::Launch`. This module holds the half that
/// can be run: which plans are gated, and what the dialog does.
///
/// Placed at the END of the file, after every other test module: the
/// `production()` slices in this file cut at the FIRST `#[cfg(test)]`, so a
/// module inserted higher up would silently empty every one of them and leave
/// the suite green with the source guards vacated.
#[cfg(test)]
mod launch_confirm_tests {
    use super::*;

    /// The payload the security review found, spelled as it was spelled. It
    /// has a space and a pair of quotes in it, which is what makes it worth
    /// testing that it survives to the screen intact.
    const HOSTILE: &str = r#"--gpu-launcher="cmd /c calc.exe""#;
    const PROGRAM: &str = r"C:\Apps\Chrome\chrome.exe";
    const URL: &str = "https://app.ledgerline.com/";

    fn hostile_plan() -> LaunchPlan {
        LaunchPlan {
            program: PROGRAM.to_string(),
            raw_tail: detail::launch_tail(HOSTILE, URL),
            has_raw_args: true,
        }
    }

    fn bare_plan() -> LaunchPlan {
        LaunchPlan {
            program: PROGRAM.to_string(),
            raw_tail: detail::launch_tail("", URL),
            has_raw_args: false,
        }
    }

    // --------------------------------------------------------------
    // The gate.
    // --------------------------------------------------------------

    #[test]
    fn a_plan_carrying_vault_arguments_is_confirmed_and_a_bare_one_is_not() {
        assert!(
            launch_needs_confirmation(&hostile_plan()),
            "a command line carrying --gpu-launcher out of the vault launches on the click"
        );
        assert!(
            !launch_needs_confirmation(&bare_plan()),
            "a launch with no stored arguments asks for a click it never used to need, and \
             the ordinary case is exactly where a confirmation becomes something to dismiss \
             unread"
        );
        // The tail alone cannot decide this: both of these have one.
        assert!(!bare_plan().raw_tail.is_empty());
        assert!(!hostile_plan().raw_tail.is_empty());
    }

    #[test]
    fn a_pending_launch_takes_its_verdict_from_the_gate_rather_than_a_second_rule() {
        for plan in [hostile_plan(), bare_plan()] {
            let expected = launch_needs_confirmation(&plan);
            let pending = PendingLaunch::new("Ledgerline".to_string(), plan.clone());
            assert_eq!(
                pending.needs_confirmation, expected,
                "the recorded request disagrees with `launch_needs_confirmation` about \
                 {plan:?}, so the gate the drain block reads is a second copy of the rule"
            );
            assert_eq!(
                pending.plan, plan,
                "the recorded request is not the plan the Open control reported"
            );
        }
    }

    // --------------------------------------------------------------
    // The dialog.
    // --------------------------------------------------------------

    const SCREEN: f32 = 900.0;

    struct Shot {
        action: LaunchConfirmAction,
        /// Source string, the characters really laid out, and the rect.
        ///
        /// The middle one is the point: `Galley::text()` returns the layout
        /// job's SOURCE, so a command line elided to `--gpu-lau\u{2026}` still
        /// reports itself in full. A security control that shows half the
        /// string is worse than none, so the glyphs are what is asserted on.
        texts: Vec<(String, String, egui::Rect)>,
    }

    impl Shot {
        fn sources(&self) -> Vec<&str> {
            self.texts.iter().map(|(s, _, _)| s.as_str()).collect()
        }

        fn find(&self, source: &str) -> Option<&(String, String, egui::Rect)> {
            self.texts.iter().find(|(s, _, _)| s == source)
        }

        fn painted(&self, source: &str) -> bool {
            self.find(source).is_some()
        }

        fn rect_of(&self, source: &str) -> egui::Rect {
            self.find(source)
                .unwrap_or_else(|| {
                    panic!("{source:?} was not painted; painted: {:?}", self.sources())
                })
                .2
        }
    }

    struct Dialog {
        ctx: egui::Context,
    }

    impl Dialog {
        fn new() -> Self {
            let ctx = egui::Context::default();
            // The same two throwaway frames every harness in this crate runs:
            // a font set registered during a frame is only usable from the
            // start of the next one.
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            theme::apply(&ctx);
            let _ = ctx.run_ui(Self::input(Vec::new()), |_ui| {});
            Self { ctx }
        }

        fn input(events: Vec<egui::Event>) -> egui::RawInput {
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::splat(SCREEN),
                )),
                events,
                ..Default::default()
            }
        }

        fn frame(&mut self, pending: &PendingLaunch, events: Vec<egui::Event>) -> Shot {
            let mut action = LaunchConfirmAction::None;
            let ctx = self.ctx.clone();
            let output = self.ctx.run_ui(Self::input(events), |_ui| {
                action = draw_launch_confirm_modal(&ctx, pending);
            });
            let all = egui::Shape::Vec(output.shapes.iter().map(|c| c.shape.clone()).collect());
            let mut texts = Vec::new();
            collect(&all, &mut texts);
            Shot { action, texts }
        }

        fn idle(&mut self, pending: &PendingLaunch) -> Shot {
            self.frame(pending, Vec::new())
        }

        /// The dialog, up and painted.
        ///
        /// **Two frames, and the first one is not optional.** An anchored
        /// `egui::Area` does not know its own size until it has been laid out
        /// once, so its very first frame paints nothing at all -- `shapes` is
        /// three `Noop`s. The same reason `Switcher::open` above lets the
        /// popup paint on a frame after the click. A test that read the first
        /// frame would find no command line and no buttons and would be
        /// asserting about an empty screen.
        fn up(&mut self, pending: &PendingLaunch) -> Shot {
            let blank = self.idle(pending);
            assert!(
                blank.texts.is_empty(),
                "the first frame already painted, so this warm-up is hiding something: {:?}",
                blank.sources()
            );
            self.idle(pending)
        }

        fn click(&mut self, pending: &PendingLaunch, pos: egui::Pos2) -> Shot {
            self.frame(
                pending,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
            )
        }
    }

    fn collect(shape: &egui::Shape, out: &mut Vec<(String, String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.text().to_string(),
                text.galley
                    .rows
                    .iter()
                    .flat_map(|row| row.glyphs.iter().map(|glyph| glyph.chr))
                    .collect(),
                egui::Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }

    /// The screen the dialog is drawn on.
    fn screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::splat(SCREEN))
    }

    fn a_pending(plan: LaunchPlan) -> PendingLaunch {
        PendingLaunch::new("Ledgerline".to_string(), plan)
    }

    /// **The control the whole feature is.**
    ///
    /// Not "a command line is painted somewhere": the *exact* string that will
    /// be handed to `raw_arg`, every character of it, inside the window, and
    /// laid out rather than elided. This crate has shipped controls that
    /// silently elided the only part that mattered, and `Galley::text()` is
    /// blind to exactly that -- so the assertion is on the glyphs.
    #[test]
    fn the_dialog_shows_the_whole_command_line_that_will_run() {
        let pending = a_pending(hostile_plan());
        let expected = detail::command_line(&pending.plan);
        let shot = Dialog::new().up(&pending);

        let (_, glyphs, rect) = shot.find(&expected).unwrap_or_else(|| {
            panic!(
                "the dialog does not show the command line that will run.\nexpected: \
                 {expected}\npainted: {:?}",
                shot.sources()
            )
        });
        assert!(
            screen().contains_rect(*rect),
            "the command line is painted at {rect:?} on a {SCREEN}x{SCREEN}pt screen -- it is \
             off the edge, so the control is up and the string is not"
        );
        // Wrapping is allowed and elision is not, so the comparison drops
        // whitespace: every non-space character of the command line must have
        // been laid out.
        let laid: String = glyphs.chars().filter(|c| !c.is_whitespace()).collect();
        let want: String = expected.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(
            laid, want,
            "the command line was elided: {glyphs:?} was drawn for {expected:?}, and a \
             control that hides the dangerous half of the string is worse than no control"
        );
        assert!(
            !glyphs.contains('\u{2026}'),
            "the command line was drawn with an ellipsis in it: {glyphs:?}"
        );
        // And the payload itself, named, so a future change that reformats the
        // command line cannot quietly drop it.
        assert!(
            expected.contains(HOSTILE),
            "the string the dialog shows has lost the arguments that make this dangerous: \
             {expected}"
        );
        // The item is named too -- the user has to know which Open this is.
        assert!(shot.painted("Ledgerline"), "painted: {:?}", shot.sources());
        assert!(shot.painted(LAUNCH_CONFIRM_TITLE), "painted: {:?}", shot.sources());
    }

    /// **Default deny.** A frame in which nothing happened is not approval.
    #[test]
    fn a_dialog_nobody_touched_does_not_approve_the_launch() {
        let pending = a_pending(hostile_plan());
        let mut dialog = Dialog::new();
        for _ in 0..3 {
            assert_eq!(
                dialog.idle(&pending).action,
                LaunchConfirmAction::None,
                "the dialog approved a launch on a frame in which nothing was clicked"
            );
        }
    }

    #[test]
    fn the_dialog_is_dismissible_without_launching() {
        let pending = a_pending(hostile_plan());

        let mut by_button = Dialog::new();
        let up = by_button.up(&pending);
        let cancel = up.rect_of("Cancel").center();
        assert_eq!(
            by_button.click(&pending, cancel).action,
            LaunchConfirmAction::Cancel,
            "Cancel did not dismiss the dialog"
        );

        let mut by_esc = Dialog::new();
        let _ = by_esc.up(&pending);
        let escaped = by_esc.frame(
            &pending,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert_eq!(
            escaped.action,
            LaunchConfirmAction::Cancel,
            "Escape did not dismiss the dialog, so the only way out is a button"
        );
    }

    /// The other direction: the dialog can be obeyed. A control that could
    /// never say yes would pass every test above and ship an Open that does
    /// nothing.
    #[test]
    fn the_confirming_button_approves_the_launch() {
        let pending = a_pending(hostile_plan());
        let mut dialog = Dialog::new();
        let up = dialog.up(&pending);
        let go = up.rect_of(LAUNCH_CONFIRM_GO).center();
        assert_eq!(
            dialog.click(&pending, go).action,
            LaunchConfirmAction::Launch,
            "the dialog's {LAUNCH_CONFIRM_GO:?} button does not approve the launch"
        );
        // And the two buttons are not the same button.
        assert_ne!(
            up.rect_of(LAUNCH_CONFIRM_GO),
            up.rect_of("Cancel"),
            "the confirm and the cancel are one control"
        );
    }
}

/// **Every door out of the read pane drops the copy confirmation.**
///
/// `detail::forget_copy_toast_on_item_change` is called from
/// `draw_detail_read` and fires on a DIFFERENT item, so it covers exactly one
/// door. The editors and the empty selection are two more, and neither
/// changes the item: copy a password, click Edit, cancel back inside the five
/// seconds, and the confirmation was still there for a copy the user had
/// looked away from. Same for deselecting and reselecting.
///
/// `detail`'s own suite drives the primitive on a real pane
/// (`a_confirmation_does_not_survive_the_pane_being_looked_away_from`); what
/// cannot be run is the wiring, because these three branches live inside
/// `run`'s update closure -- see [`open_app_wiring_tests`] for that
/// limitation and why a source guard is the answer to it here.
///
/// At the END of the file for the reason [`launch_confirm_tests`] states.
#[cfg(test)]
mod copy_toast_wiring_tests {
    // No `use super::*`: this module reads the file's TEXT and calls nothing.

    // `concat!`-split for `open_app_wiring_tests`' reason: `include_str!`
    // pulls this module in too.
    const CLEARS: &str = concat!("detail::forget_copy_toast", "(ui.ctx());");
    const EDIT: &str = concat!("DetailMode::Edit", "(draft) => {");
    const CREATE: &str = concat!("DetailMode::Create", "(draft) => {");
    const NOTHING_SELECTED: &str = concat!("ui.label(\"Select an", " item.\");");

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.match_indices(needle).count()
    }

    /// `before` bytes on one side of `marker` and `after` on the other, cut
    /// back to char boundaries. Wide enough to hold the branch's own comment
    /// and its statements, nothing like wide enough to reach the next branch
    /// -- the length control on each assertion below says so.
    fn window(source: &'static str, marker: &str, before: usize, after: usize) -> &'static str {
        let at = source.find(marker).unwrap_or_else(|| panic!("no {marker:?} in this file"));
        let mut start = at.saturating_sub(before);
        while !source.is_char_boundary(start) {
            start += 1;
        }
        let mut end = (at + marker.len() + after).min(source.len());
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        &source[start..end]
    }

    #[test]
    fn the_editors_and_the_empty_selection_drop_a_live_copy_confirmation() {
        let source = source();
        assert_eq!(
            occurrences(source, CLEARS),
            3,
            "expected {CLEARS:?} exactly three times -- the edit pane, the create pane and \
             the no-selection branch. Fewer means one of the three routes back into the read \
             pane still resurrects a confirmation the user has looked away from"
        );
        // The clear is the statement immediately after the branch opens, for
        // the two editors; for the empty selection it is immediately before
        // the label, which is what opens that branch's body. Hence the two
        // directions -- the window is around the marker, not after it.
        for (marker, what, before, after) in [
            (EDIT, "the edit pane", 0usize, 120usize),
            (CREATE, "the create pane", 0, 120),
            (NOTHING_SELECTED, "the no-selection branch", 120, 0),
        ] {
            assert_eq!(occurrences(source, marker), 1, "expected exactly one {marker:?}");
            let branch = window(source, marker, before, after);
            assert_eq!(
                occurrences(branch, CLEARS),
                1,
                "{what} does not clear the copy confirmation -- a round trip through it \
                 brings back a confirmation for a copy the user has looked away from: \
                 {branch}"
            );
        }
    }
}
/// The two arguments the EDIT form is handed at [`build_frame`]'s seam, and
/// the scope of the one TOTP poll behind them -- held from the source.
///
/// Same mechanism, and the same reason, as [`app_block_wiring_tests`]: the
/// seam lives inside `build_frame`'s update closure, which needs a real
/// `VaultCache` over a live `bw serve`, so no test in this crate runs it.
///
/// `app_block_wiring_tests::PASSES_THE_CACHE` is not a substitute. It matches
/// BOTH editor calls, so what it pins is that *a* cache is passed at all --
/// it says nothing about which item or which `TotpState` either arm gets, and
/// both of those survived the whole suite:
///
///  * `selected_item.as_ref()` -> `None` (what the Create arm legitimately
///    passes) kills `{TOTP}` and every `{S:...}` in the keystroke palette and
///    makes every custom-field reference unresolvable in the preview. The
///    `{S:PIN}` discoverability the feature exists for, gone silently.
///  * `&totp_state` -> `&TotpState::NoSecret` makes the eye report "no
///    one-time code on this item" for every item forever.
///
/// Each needle carries all four correlated arguments of its own arm, so
/// either arm taking the other's is a failure here, in both directions.
#[cfg(test)]
mod edit_seam_argument_tests {
    // EVERY NEEDLE IS SPLIT WITH `concat!` and is a single line with no line
    // ending, for `app_block_wiring_tests`' reasons: `include_str!("mod.rs")`
    // pulls this module in too, so an unsplit needle matches its own
    // declaration, and a needle carrying a newline passes on one checkout's
    // line endings and fails on the other's.
    //
    // The split points are also chosen so that no literal here contains
    // `app_block_wiring_tests::PASSES_THE_CACHE` -- one that did would break
    // that module's count of exactly two.
    const EDIT_ARM: &str =
        concat!("&folders, false, &mut app_ident", "ities, selected_item.as_ref(), &totp_state) {");
    const CREATE_ARM: &str =
        concat!("&folders, true, &mut app_ident", "ities, None, &totp_state) {");
    const BOTH_EDITORS: &str = concat!("draw_detail_", "edit(ui,");

    fn source() -> &'static str {
        include_str!("mod.rs")
    }

    /// Everything before the first `#[cfg(test)]`. Same split, and the same
    /// self-matching hazard, as `frame_host_tests::production`.
    fn production() -> &'static str {
        let source = source();
        let end = source
            .find(concat!("#[cfg(", "test)]"))
            .expect("no test marker in this file -- see `frame_host_tests`");
        &source[..end]
    }

    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn the_counter_finds_needles_that_are_really_there() {
        // The positive control, driving the same counting the assertions use.
        let planted = concat!("x(ui, draft, &folders, true, &mut app_ident", "ities, None, &totp_state) {");
        assert_eq!(occurrences(planted, CREATE_ARM), 1, "planted: {planted}");
        assert_eq!(occurrences(planted, EDIT_ARM), 0);
        assert_eq!(occurrences("nothing here", BOTH_EDITORS), 0);
    }

    #[test]
    fn the_edit_form_is_handed_the_selected_item_and_the_windows_one_totp_state() {
        let production = production();
        assert_eq!(
            occurrences(production, BOTH_EDITORS),
            2,
            "expected two draft editors -- the control on the two assertions below, which \
             each expect one arm to exist"
        );
        assert_eq!(
            occurrences(production, EDIT_ARM),
            1,
            "expected {EDIT_ARM:?} exactly once. `None` in place of the item leaves the \
             keystroke palette with nothing but the two boxes on the form: no `{{TOTP}}`, no \
             `{{S:...}}`, and every custom-field reference in the preview unresolvable. \
             `&TotpState::NoSecret` in place of the state makes the eye say this item has no \
             one-time code, for every item, forever. Both compile, and both used to pass \
             every test in this crate"
        );
        assert_eq!(
            occurrences(production, CREATE_ARM),
            1,
            "expected {CREATE_ARM:?} exactly once -- the create form, which has no item yet \
             and so is the ONE arm entitled to `None`. Two of these means the edit arm has \
             quietly taken the create arm's arguments"
        );
    }

    /// **The scope of the one poll.**
    ///
    /// The trigger and the countdown must sit ABOVE the pane `match`, not
    /// inside its `Read` arm. They used to be inside it, and the `Edit` arm
    /// ran neither -- so entering the editor on an item whose code had
    /// already been fetched showed that code, frozen, for the whole edit
    /// session: no refresh, no countdown, and no transition to
    /// `NoCodeReported`. The keystroke preview's `{TOTP}` was a number that
    /// had long since expired.
    ///
    /// This is a POSITION assertion rather than a count, because the defect
    /// was never a missing call -- there was exactly one of each, in the
    /// wrong scope.
    #[test]
    fn the_one_totp_poll_and_countdown_are_above_the_pane_match_not_inside_the_read_arm() {
        let production = production();
        const PANE_MATCH: &str = concat!("match &mut mo", "de {");
        const TRIGGER: &str = concat!("} else if should_start_totp_", "poll(");
        const COUNTDOWN: &str = concat!("let seconds_left = current_totp_seconds_", "left();");

        assert_eq!(
            occurrences(production, PANE_MATCH),
            1,
            "control: this test locates the poll relative to the ONE pane match"
        );
        assert_eq!(
            occurrences(production, TRIGGER),
            1,
            "there is more than one place a TOTP poll is started. The editor reads the vault \
             window's one `TotpState` precisely so there is one poll behind it; a second \
             trigger is a second schedule for it"
        );
        assert_eq!(
            occurrences(production, COUNTDOWN),
            2,
            "control: the countdown is read in exactly two places -- where a poll result \
             lands, and where the pane is about to be drawn"
        );

        let pane_match = production.find(PANE_MATCH).expect("checked above");
        assert!(
            production.find(TRIGGER).expect("checked above") < pane_match,
            "the TOTP poll is triggered inside the pane match, so it belongs to whichever \
             arm it landed in. The Edit pane draws a `{{TOTP}}` preview and would issue no \
             poll at all"
        );
        assert!(
            production.rfind(COUNTDOWN).expect("checked above") < pane_match,
            "the last `seconds_left` refresh is inside the pane match, so the countdown \
             stops advancing in every arm but the one holding it -- which is what froze the \
             editor's previewed code for the whole edit session"
        );
    }
}

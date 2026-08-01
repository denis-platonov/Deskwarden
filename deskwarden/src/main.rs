// Builds this binary against the Windows GUI subsystem instead of the console
// subsystem (the default). Without it, every launch -- including the
// autostart-on-login the installer registers -- pops a black console window
// next to the tray icon, and closing that window kills the whole app.
//
// This attribute applies to the crate it appears in, and a Cargo binary
// target is its own crate rooted at this file: it belongs here, in main.rs,
// and specifically NOT in lib.rs (where it would do nothing for this binary
// and would apply to `cargo test`'s harness and the examples, which *do* want
// a console). Verified on the built artifact by reading the PE optional
// header's Subsystem field (2 = GUI, 3 = console).
//
// Consequence: `println!`/`eprintln!` from this binary go nowhere. There is
// exactly one such call left (the logging-init failure below, which by
// definition can't use the log file), and it is a fallback for a case the
// user cannot act on anyway. Everything user-facing goes through the tray,
// the log file, or -- for the startup failures that happen before the tray
// exists -- a native message box; see `message_box`/`fatal_startup_error`.
#![windows_subsystem = "windows"]

//! Binary entry point.
//!
//! Declares no modules of its own: every module lives in `lib.rs` (see the
//! note there). This file is only `fn main()` and the startup sequence.

use deskwarden::app::{fill_from_vault, handle_match, match_entries, pump_windows_messages};
use deskwarden::backend_policy;
// `BACKEND_OP_TIMEOUT`: the upper bound on how long a legitimate backend
// start or sync may take before something is treated as having gone wrong.
// Used both by this file's own backend-op bookkeeping
// (`backend_task_in_progress`'s wedge deadline, `open_vault_window`'s
// lock-recovery wait) and by `picker_ui::run_picker`'s readiness probe (see
// its own doc for review 11's Important 2) -- defined in `bw_serve`, not
// here, so both sides share the exact same number rather than disagreeing.
use deskwarden::bw_serve::{
    self, readiness_schedule, wait_for_vault_ready, BACKEND_OP_TIMEOUT, BW_SERVE_URL,
    READINESS_DEADLINE,
};
use deskwarden::dispatch;
use deskwarden::injector::{
    Injector, RealSendInput, RealUiAutomation, SendInputFiller, UiAutomationFiller,
};
use deskwarden::match_engine::MatchEngine;
use deskwarden::updater::{self, ReleaseInfo};
use deskwarden::vault_bridge::VaultBridge;
use deskwarden::vault_cache::{PopulateOutcome, VaultCache, VaultEpoch, VaultEra};
use deskwarden::{
    fill_stats, hotkey, job_object, loading_ui, logging, login_ui, picker_ui, prefs_ui,
    session_store, settings, tray, vault_window, window_watch,
};
use semver::Version;
use std::process::Child;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use windows::core::HSTRING;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONWARNING, MB_OK,
    MB_SETFOREGROUND, MB_SYSTEMMODAL, MB_YESNO, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE,
};

/// How often to poll GitHub for a newer release. Checked on startup and then
/// on this cadence from the main loop, same pattern as `REFRESH_INTERVAL`.
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Real GitHub REST API base, passed to `updater::check_for_update`. Not
/// `github.com` itself -- that's the web UI host; `api.github.com` is the
/// API host the releases endpoint actually lives on.
const GITHUB_API_BASE: &str = "https://api.github.com";

/// Authenticode thumbprint an update's signature must match before it's
/// trusted and applied.
///
/// TODO: set once SignPath cert is issued (Task 5's manual prerequisite).
/// The real certificate deskwarden's release builds will be signed with does
/// not exist yet at this point in the project, so there is no genuine value
/// to put here. This placeholder is intentionally not a plausible-looking
/// thumbprint: it can never match a real signature, so `is_trusted_signer`
/// (and therefore `download_and_verify`) fails closed -- refusing every
/// update -- until this constant is replaced with the real one.
const EXPECTED_SIGNER_THUMBPRINT: &str = "PLACEHOLDER_SET_ONCE_SIGNPATH_CERT_ISSUED";

/// Organization (`O=`) values accepted as proof that the resolved `bw.exe`
/// (see `bw_path::resolve_bw_exe`) really is Bitwarden's own CLI.
///
/// Mirrors `$BitwardenSignerOrganizations` in `installer/bootstrap-bw.ps1` --
/// kept in sync there by hand, not shared code, since one runs at install
/// time (PowerShell) and this runs at every app startup (Rust). Pinning the
/// *path* `bw.exe` is resolved from isn't enough on its own: the installer's
/// `bin` directory is itself inside deskwarden's user-writable install tree,
/// so anything able to plant a file beside `deskwarden.exe` can just as
/// easily overwrite `bin\bw.exe`. This is the check that actually matters --
/// whatever ends up at that path must be signed by Bitwarden before it's
/// handed the user's master password (`login_ui::run_bw_with_password`) or
/// session token.
///
/// TODO (verify before shipping): this list has not yet been confirmed
/// against a real Bitwarden-signed `bw.exe` -- see the identical TODO on
/// `$BitwardenSignerOrganizations` in `installer/bootstrap-bw.ps1` for the
/// verification step.
///
/// Because the list is *known to be unverified*, a mismatch is deliberately
/// **not** treated the same way `EXPECTED_SIGNER_THUMBPRINT` treats a bad
/// update signature. See `check_bw_signature` below for the graded response
/// and the reasoning behind it: an unsigned or tamper-detected binary is
/// still refused outright, but "validly signed by an organization this
/// unverified list doesn't happen to name" asks the user instead of killing a
/// tray app with no console and no explanation.
const TRUSTED_BW_SIGNER_ORGANIZATIONS: &[&str] = &[
    "Bitwarden Inc.",
    "Bitwarden, Inc.",
    "Bitwarden Inc",
    "Bitwarden",
    "8bit Solutions LLC",
];

fn main() {
    let project_dirs = directories::ProjectDirs::from("dev", "Deskwarden", "Deskwarden")
        .expect("could not resolve config directory");
    let config_dir = project_dirs.config_dir().to_path_buf();
    std::fs::create_dir_all(&config_dir).expect("failed to create config directory");

    // Downloaded installers go under the *cache* directory, not next to
    // `session.bin` and the log in the config directory: they are large,
    // disposable, and regenerable, which is exactly what a cache directory is
    // for -- and keeping multi-megabyte attacker-supplied-until-verified
    // downloads out of the directory holding the encrypted session token is
    // worth the one extra path. Created lazily by `download_and_verify`.
    let update_download_dir = project_dirs.cache_dir().join("updates");

    // On-disk favicon cache, keyed by domain (see `favicon::write_cached_icon`).
    // Also a cache directory, for the same reason `update_download_dir` is:
    // disposable and regenerable, so it belongs alongside it rather than in
    // `config_dir`. Not created here -- `favicon::write_cached_icon` creates
    // it lazily on first write, same as `update_download_dir`'s directory is
    // created lazily by `download_and_verify`.
    let icon_cache_dir = project_dirs.cache_dir().join("icons");

    // Logging first: a background tray app has no console, so without a log
    // file every failure below is invisible to whoever has to diagnose it.
    match logging::init(&config_dir) {
        Ok(path) => log::info!("deskwarden starting; logging to {}", path.display()),
        Err(e) => eprintln!("warning: {e}"),
    }

    // Building with windows_subsystem = "windows" (no console) means a
    // panic's default stderr backtrace goes nowhere -- the process just
    // vanishes with zero trace, which is exactly the invisibility logging
    // was added to eliminate. Route panics into the same log file instead.
    std::panic::set_hook(Box::new(|info| {
        log::error!("panicked: {info}");
    }));

    // Verified once, up front, before anything below spawns the CLI or shows
    // the login window: `bw_serve`/`login_ui` hand this binary the user's
    // master password and, afterwards, their live session token. Refusing to
    // proceed with an unsigned or wrongly-signed `bw.exe` is the whole point
    // of resolving it to a specific path in the first place -- see
    // `bw_path::resolve_bw_exe` and `TRUSTED_BW_SIGNER_ORGANIZATIONS` above.
    let Some(bw_exe) = deskwarden::bw_path::resolve_bw_exe() else {
        fatal_startup_error(
            "Deskwarden could not work out its own install directory, so it cannot tell which \
             bw.exe is the real Bitwarden CLI.\n\nRather than guess -- and risk handing your \
             master password to the wrong program -- it is stopping here.\n\nReinstalling \
             Deskwarden should fix this.",
        );
    };
    if !bw_exe.exists() {
        fatal_startup_error(&format!(
            "Deskwarden needs the Bitwarden CLI (bw.exe) and could not find it.\n\nExpected it \
             at:\n{}\n\nInstall the Bitwarden CLI, or reinstall Deskwarden (its installer \
             downloads a signed copy for you).",
            bw_exe.display()
        ));
    }
    check_bw_signature(&bw_exe);

    // Resolved and verified exactly once, here. Everything that later spawns
    // the CLI -- `bw_serve`, `login_ui`, including the one call that hands
    // over the master password -- reads this single recorded result instead of
    // re-resolving, so a `bw.exe` appearing on disk *after* this point can
    // never be the one that gets the secrets.
    deskwarden::bw_path::remember_verified_bw_exe(bw_exe);

    // Any installer still sitting in the download directory is spent by now:
    // either it was applied (and this process is the result) or its attempt
    // failed. Deleting them here rather than after applying one is not a
    // stylistic choice -- `apply_update` launches the installer and this
    // process exits immediately after, so at that moment the file is a
    // running process image and cannot be deleted.
    match updater::cleanup_stale_downloads(&update_download_dir) {
        Ok(0) => {}
        Ok(n) => log::info!("cleaned up {n} stale update download(s)"),
        Err(e) => log::warn!("could not clean up stale update downloads: {e}"),
    }

    let session_path = config_dir.join("session.bin");
    let store = session_store::SessionStore::new(session_path);

    let fill_stats_path = config_dir.join("fill-stats.json");
    let fill_stats = fill_stats::FillStats::new(fill_stats_path);

    // User preferences (backend lifecycle, auto-lock timeout). A missing or
    // corrupt file falls back to defaults -- see `Settings::load` -- so this
    // is never a reason startup fails.
    //
    // `mut`, and the path kept around as `settings_path`: the preferences
    // window (`prefs_ui::run`) can change and save these mid-session, and
    // this binding -- not the file on disk -- is what every later read in
    // this loop (`settings.auto_lock_timeout()`, `settings.keep_backend_running`
    // in the idle reconciliation below) actually consults. Reassigning it in
    // the tray handler is what makes a change take effect immediately rather
    // than only on next launch.
    let settings_path = config_dir.join("settings.json");
    let mut settings = settings::Settings::load(&settings_path);

    // Every child process we spawn joins this job object, which is configured
    // to kill its members when the last handle closes. Our handles close when
    // this process dies for *any* reason -- clean quit, panic, Ctrl+C, Task
    // Manager -- so `bw serve` can no longer be orphaned holding an unlocked
    // vault open on localhost. This must outlive the whole run, hence the
    // binding here rather than inside the spawn helper.
    // `Arc`-wrapped (rather than a plain `Option<KillOnCloseJob>` borrowed by
    // reference, as before) so a clone can be handed to the background
    // threads that now start `bw serve` off the main thread -- see
    // `spawn_backend_start` -- without them needing a `'static` borrow of a
    // stack local. `KillOnCloseJob` itself is `Send + Sync` with no `unsafe`
    // now that it's backed by `OwnedHandle` (see `job_object`), so this needs
    // no unsafe either.
    let job: Arc<Option<job_object::KillOnCloseJob>> = Arc::new(match job_object::KillOnCloseJob::new()
    {
        Ok(job) => Some(job),
        Err(e) => {
            log::error!(
                "could not create a kill-on-close job object ({e}); `bw serve` will only be \
                 cleaned up on a clean quit"
            );
            None
        }
    });

    // A cached session token is worthless if it has since been invalidated
    // (manual `bw lock`, password change, reboot). Trusting it unconditionally
    // is how the app used to proceed "unlocked" with no recovery path.
    let mut session_token = match store.load() {
        Some(token) => match login_ui::check_bw_status_with_session(Some(&token)) {
            login_ui::BwStatus::Unlocked => {
                log::info!("cached session token verified as unlocked");
                token
            }
            other => {
                log::warn!("cached session token reports {other:?}; re-authenticating");
                reauthenticate(&store)
            }
        },
        None => {
            log::info!("no cached session token; showing login flow");
            reauthenticate(&store)
        }
    };

    let vault = VaultBridge::new(BW_SERVE_URL);
    // The vault window's reads and writes, and now autofill's own reads (see
    // `app::fill_from_vault`), go through this in-memory snapshot rather than
    // straight to `bw serve` -- see `vault_cache`'s module doc. Built once,
    // here, wrapping the same bridge everything else in `main` still uses
    // directly: startup's readiness check, the picker's item list, and the
    // periodic match-engine refresh all still want the live server rather
    // than a snapshot that's deliberately not re-fetched on every read.
    let cache = Arc::new(VaultCache::new(vault.clone()));
    // Captured here, *before* the readiness probe below, because that probe's
    // own `list_items()` is the fetch whose result seeds the cache further
    // down via `populate_with` -- and the epoch guard can only cover the
    // window it is handed (review 14's Minor 3). Nothing between here and
    // there calls `cache.clear()` today, so this is inert; it is written this
    // way so it stays correct if any of that moves onto a background thread.
    let startup_epoch = cache.epoch();
    let mut engine = MatchEngine::new();

    // `Option` rather than a plain `Child`: with `keep_backend_running`
    // turned off, the backend is only up while the vault window is open (see
    // `backend_policy::should_run`), so "not currently running" has to be
    // representable. Always `Some` here at startup -- `start_backend` starts
    // it unconditionally, since something has to answer the very first
    // `wait_for_vault_ready_with_spinner` call below regardless of the
    // setting.
    let mut bw_serve_child: Option<Child> = Some(start_backend(&session_token, job_ref(&job)));

    // `bw serve` is a bundled Node binary: its cold start regularly takes
    // several seconds, far longer than the fixed 500ms sleep this replaces.
    // Losing that race used to leave the match engine permanently empty with
    // no diagnostic, so the app silently did nothing forever.
    let schedule = readiness_schedule(READINESS_DEADLINE);
    let items = match wait_for_vault_ready_with_spinner(&vault, &schedule, SETUP_MESSAGE) {
        VaultReadyOutcome::Ready(items) => items,
        VaultReadyOutcome::Dismissed => {
            // Closing the "setting up" window is not, on its own, evidence
            // that the backend or session is broken -- unlike a genuine
            // timeout (the `Failed` arm below), there's no "maybe the
            // session was rejected" signal to act on here (review 12's
            // Important 2). Give the same, still-running backend one more
            // honest readiness probe -- no kill, no reauth -- before falling
            // back to the heavier recovery a real failure gets.
            log::info!(
                "setup window closed before the vault backend was confirmed ready; trying the \
                 readiness probe again before treating anything as actually broken"
            );
            match wait_for_vault_ready_with_spinner(&vault, &schedule, SETUP_RETRY_MESSAGE) {
                VaultReadyOutcome::Ready(items) => items,
                VaultReadyOutcome::Dismissed => recover_from_failed_vault_wait(
                    "setup window closed a second time without the vault backend becoming ready",
                    &vault,
                    &schedule,
                    &mut bw_serve_child,
                    &mut session_token,
                    &job,
                    &store,
                    &config_dir,
                ),
                VaultReadyOutcome::Failed(e) => recover_from_failed_vault_wait(
                    &e,
                    &vault,
                    &schedule,
                    &mut bw_serve_child,
                    &mut session_token,
                    &job,
                    &store,
                    &config_dir,
                ),
            }
        }
        VaultReadyOutcome::Failed(e) => recover_from_failed_vault_wait(
            &e,
            &vault,
            &schedule,
            &mut bw_serve_child,
            &mut session_token,
            &job,
            &store,
            &config_dir,
        ),
    };

    let entries = match_entries(&items);
    log::info!("match engine loaded with {} app match(es)", entries.len());
    engine.rebuild(&entries);

    // Seeds the cache with the `items` the readiness probe just fetched
    // (`VaultCache::populate_with`), rather than a plain `populate()`
    // listing them all over again right after -- the same request, for data
    // that cannot have changed in the instant between the two calls. Still
    // fetches folders, since nothing above needed those. This also means
    // `items` doesn't need a separate `drop()`: it becomes the cache's own
    // storage instead of a throwaway local that would otherwise keep the
    // entire deserialized vault (potentially thousands of items, each
    // carrying a serde_json::Map "other" catch-all) resident for the rest of
    // the process's life doing nothing -- this app spends nearly all its
    // runtime idle in the tray with no window open.
    match cache.populate_with(items, startup_epoch) {
        Ok(PopulateOutcome::Populated) => {}
        Ok(PopulateOutcome::DiscardedStale) => {
            log::warn!("the vault cache was cleared during startup's populate; it stays empty")
        }
        Err(e) => log::warn!("could not populate the vault cache at startup: {e:?}"),
    }

    // The lifecycle this app promises: unlock -> start the backend -> fill
    // the cache once -> *then* obey the policy. The backend has had to be up
    // unconditionally until now, because nothing above this point could have
    // populated the cache without it -- but with the cache now filled, a
    // `keep_backend_running = false` setting means it should already be
    // torn back down again before the tray even appears, not "eventually,
    // the next time something happens to notice". Everything downstream (the
    // vault window opening, the tray's Sync item, another lock) restarts it
    // only for as long as it is actually needed and reconciles again
    // afterwards -- see `stop_backend_if_idle` and the main loop below.
    stop_backend_if_idle(&mut bw_serve_child, settings.keep_backend_running);

    let injector = Injector {
        ui: RealUiAutomation,
        fallback: RealSendInput,
    };

    // The tray icon and the global hotkey manager each create a hidden
    // Win32 window on the thread that builds them (here, the main thread)
    // and rely on that thread pumping its message queue: tray clicks arrive
    // as WM_COMMAND/WM_MENUCOMMAND and the hotkey arrives as WM_HOTKEY, both
    // delivered only via GetMessage/PeekMessage + DispatchMessage on the
    // owning thread. That's why both are built here rather than on the
    // window-watch thread below (which runs its own, unrelated message
    // loop), and why the main loop calls `pump_windows_messages()` every
    // iteration -- without it, tray clicks and hotkey presses would sit
    // undelivered in the queue forever and `tray::next_menu_event()` /
    // `hotkey::fill_hotkey_pressed()` would never see anything.
    let fill_hotkey = hotkey::register_fill_hotkey();
    let tray = tray::build_tray();

    let current_version =
        Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is not valid semver");

    // Bounded connect/read timeouts, same "don't trust an external dependency
    // to answer promptly" reasoning as `bw_serve::READINESS_DEADLINE` -- just
    // applied to `api.github.com` instead of localhost. `ureq::Agent` is a
    // cheap `Arc`-backed handle, so it's fine to clone into the background
    // threads below.
    let http_agent = updater::build_agent();

    // The update check talks to an external host and, prior to this fix, ran
    // synchronously here -- before the tray, hotkey, and window-watch thread
    // even existed -- so a stalled `api.github.com` connection hung the
    // *entire app* on every launch before it became interactive at all. It's
    // now kicked off on its own background thread and reported back over
    // `update_rx`, polled non-blockingly from the main loop below, so a slow
    // or hung check can never delay startup. Same shape as the
    // `window_watch` thread just below.
    let mut available_update: Option<ReleaseInfo> = None;
    let (update_tx, update_rx) = mpsc::channel::<ReleaseInfo>();
    {
        let agent = http_agent.clone();
        let version = current_version.clone();
        let tx = update_tx.clone();
        std::thread::spawn(move || {
            if let Some(release) = check_for_update_logged(&version, &agent) {
                let _ = tx.send(release);
            }
        });
    }
    let mut last_update_check = Instant::now();

    // Prefetches the account email + server URL the vault window's toolbar
    // needs (see `open_vault_window`), on its own thread: `bw status`
    // regularly takes 1-3s to spawn on Windows, and `open_vault_window`
    // used to call it inline in the tray-click handler, so every "Open
    // Vault" -- including the very first one -- waited that long before the
    // window even appeared. Polled non-blockingly below, same shape as
    // `update_rx`; `open_vault_window` still falls back to a synchronous
    // call itself if a click lands before this has reported back.
    let mut cached_status_details: Option<login_ui::BwStatusDetails> = None;
    let (status_details_tx, status_details_rx) = mpsc::channel::<login_ui::BwStatusDetails>();
    {
        let tx = status_details_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(login_ui::check_bw_status_details());
        });
    }

    // Outcome of a click-triggered update attempt. `Ok(())` means the
    // installer was downloaded, signature-verified, and launched, and this
    // process should now shut down for it; `Err` carries a message for the
    // log and the tray.
    //
    // The work behind this channel used to run inline in the tray-click
    // handler below, which streams a multi-megabyte download and then blocks
    // on a `powershell.exe` spawn for signature verification -- all while
    // `pump_windows_messages()` isn't running, so the tray, the global
    // hotkey, and window-watching were dead for the whole duration and
    // Windows would flag the app as not responding. It now runs on a
    // background thread and reports back here, polled non-blockingly from the
    // main loop, exactly like `update_rx` above.
    //
    // The *shutdown* deliberately stays on the main thread: `bw_serve_child`
    // is owned here, and the whole point of the shutdown path is that the
    // backend is killed before this process goes away.
    let (apply_tx, apply_rx) = mpsc::channel::<Result<(), String>>();

    // True from the moment a download starts until its outcome arrives, so a
    // second click can't start a second concurrent download of the same
    // installer into the same destination path.
    let mut update_in_progress = false;

    let (tx, rx) = mpsc::channel::<window_watch::ForegroundEvent>();
    std::thread::spawn(move || {
        if let Err(e) = window_watch::watch_foreground_windows(move |event| {
            let _ = tx.send(event);
        }) {
            log::error!("foreground window watcher stopped: {e}");
        }
    });

    // Set when a `Hotkey`-trigger match is seen: the item/window that's
    // eligible to be filled once the user presses the fill hotkey, rather
    // than being filled immediately from the window-match path.
    let mut pending_hotkey_fill: Option<(String, isize)> = None;

    // The hwnd of the last foreground window we acted on. See
    // `dispatch::should_dispatch` for why re-dispatching the same hwnd must be
    // suppressed (short version: closing our own overlay hands foreground back
    // to the target, which would otherwise re-match and re-show the overlay
    // forever, so "Dismiss" never dismissed).
    let mut last_dispatched_hwnd: Option<isize> = None;

    // The process id of the last real (not our own) foreground window, kept
    // up to date alongside every event below. "Add app..." defaults its
    // process picker to this -- the app the user was just in -- rather than
    // making them search for it every time.
    let mut last_active_pid: Option<u32> = None;
    let own_pid = std::process::id();

    // Seed with whatever is already focused: `SetWinEventHook` only reports
    // foreground *changes*, so an app that was matched and already in front
    // when deskwarden started would otherwise be ignored until the next window
    // switch.
    if let Some(event) = window_watch::current_foreground_event() {
        log::info!(
            "seeding with current foreground window: {} (hwnd {})",
            event.exe_name,
            event.hwnd
        );
        if event.pid != own_pid {
            last_active_pid = Some(event.pid);
        }
        process_foreground_event(
            &event,
            &cache,
            &injector,
            &fill_stats,
            &engine,
            &mut pending_hotkey_fill,
            &mut last_dispatched_hwnd,
        );
    }

    // Outcome of a background operation that starts or restarts `bw serve`:
    // either `open_vault_window` making sure it's up before showing the
    // window, or the tray's "Sync" item forcing a resync. Reported back here
    // rather than joined inline on whichever thread kicked it off --
    // `try_start_backend` can take up to 30s (a port-release wait plus a
    // synchronous `bw sync`), and blocking on that before returning control
    // to the main loop used to freeze the tray, hotkey, and window-watching
    // for the whole wait -- see the fix note on `open_vault_window`.
    //
    // Both operations funnel through this one channel (rather than one each)
    // so `backend_task_in_progress` below can guarantee at most one is ever
    // in flight: two `try_start_backend` calls racing to bind the same port
    // would make one fail for a reason that has nothing to do with a real
    // problem, and it also means there is exactly one place -- not two -- a
    // lock event has to drain before it can safely stop/restart the backend
    // itself (see `open_vault_window`'s `locked` branch).
    let (backend_op_tx, backend_op_rx) = mpsc::channel::<BackendOp>();
    // `Some((started, kind))` while a background backend operation is in
    // flight, the instant it was set recording when -- rather than a plain
    // `bool` -- so the main loop below can tell a merely-slow operation
    // apart from one that has been outstanding so long it must be treated as
    // wedged (see the `BACKEND_OP_TIMEOUT` check right after the
    // non-blocking drain). `run_bw_sync` (`Command::output()`, no timeout of
    // its own) and `try_start_backend` (which calls it) have no bound on how
    // long they can take, so without this a stalled `bw sync` would leave
    // this flag `Some` forever: `stop_backend_if_idle` refuses to run while
    // it's set (save-memory mode never reclaims the backend's memory),
    // `open_vault_window` refuses to start a fresh attempt while it's set
    // (writes and TOTP stay dead), and the tray item is stuck disabled on
    // "Syncing...". `kind` records which of the two operations
    // (`BackendOpKind`) this is, so the wedge-deadline check can report a
    // stall in terms of what was actually requested (review Minor 4) instead
    // of always assuming a sync.
    let mut backend_task_in_progress: Option<(Instant, BackendOpKind)> = None;

    loop {
        pump_windows_messages();

        if let Some(event) = tray::next_menu_event() {
            if event.id == tray.quit_id {
                // `bw serve` doesn't get killed on its own: `Child` doesn't
                // kill its process on `Drop`, and `process::exit` below
                // skips destructors anyway. Kill it explicitly, before
                // exiting -- nothing after `process::exit` ever runs -- so
                // it doesn't keep serving the unlocked vault over
                // `BW_SERVE_URL` after the user believes they've quit. The
                // process may already be gone (e.g. crashed, or killed
                // externally), so a `kill()` error is expected and ignored
                // rather than treated as fatal.
                //
                // The cache is cleared for the same reason, one level up:
                // decrypted vault contents shouldn't outlive the moment the
                // user asked to quit, even for the instant between here and
                // `process::exit` actually tearing the process down.
                log::info!("quit requested from tray; killing bw serve");
                cache.clear();
                if let Some(child) = bw_serve_child.as_mut() {
                    bw_serve::stop_bw_serve(child);
                }
                std::process::exit(0);
            }

            if event.id == tray.open_vault_id {
                open_vault_window(
                    &cache,
                    &fill_stats,
                    &injector,
                    &mut session_token,
                    &mut bw_serve_child,
                    &job,
                    &store,
                    &schedule,
                    &mut engine,
                    &icon_cache_dir,
                    &mut cached_status_details,
                    settings.auto_lock_timeout(),
                    &tray,
                    &backend_op_tx,
                    &backend_op_rx,
                    &mut backend_task_in_progress,
                );
                last_dispatched_hwnd = None;
            }

            if event.id == tray.preferences_id {
                // Blocks the main loop for as long as the window is open --
                // same as every other window here (`open_vault_window`,
                // `picker_ui::run_picker`). The idle backend reconciliation
                // a bit further down only runs once this returns, so a
                // changed `keep_backend_running` takes effect on the very
                // next iteration rather than waiting for the next launch.
                let edited = prefs_ui::run(settings.clone());
                if edited != settings {
                    settings = edited;
                    if let Err(e) = settings.save(&settings_path) {
                        log::warn!("could not save settings: {e}");
                    }
                }
                last_dispatched_hwnd = None;
            }

            if event.id == tray.add_app_id {
                // Two-step flow: choose the vault item the credentials come
                // from, then choose the process to attach to it. Both
                // `pick_vault_item` and `run_picker`'s window/trigger choice
                // work purely from the cache and `window_list` -- but the
                // Save at the very end of `run_picker` calls
                // `cache.set_app_match`, a write, which needs `bw serve` up
                // *and answering*, not merely started (review 10's Important
                // 2 -- see `run_picker`'s own doc for how it now waits for
                // that itself rather than assuming a kicked-off start is
                // enough). Read once, before it might change, and reused for
                // `run_picker`'s own readiness wait below: whether `bw serve`
                // was already up at this exact moment is also exactly what
                // decides whether `run_picker` needs to wait for it at all
                // (same `backend_already_running` exemption `open_vault_window`
                // and `vault_window::run` already make).
                let backend_already_running = backend_is_running(&mut bw_serve_child);

                // Review 9's Important: in save-memory mode nothing here used
                // to start `bw serve` at all, so a save always failed after
                // two windows of user effort with nothing visible on screen.
                // Kick a start off now, the same non-blocking way
                // `open_vault_window` does. `run_picker` itself waits for it
                // to actually answer before letting Save fire.
                if needs_backend_start(&backend_task_in_progress, backend_already_running) {
                    backend_task_in_progress = Some((Instant::now(), BackendOpKind::EnsureRunning));
                    // A Sync click landing while this start is in flight
                    // would otherwise be silently dropped by the
                    // `backend_task_in_progress` guard below with nothing to
                    // show for it (review 10's Minor 6). Disabling the item
                    // here (not `set_sync_in_progress` -- this isn't a sync)
                    // means the click can't be issued in the first place;
                    // `apply_backend_op`'s `EnsureRunning` arms re-enable it
                    // once this completes.
                    tray::set_sync_busy_with_backend_op(&tray);
                    spawn_backend_start(session_token.clone(), job.clone(), backend_op_tx.clone());
                }

                // The vault session this "Add app..." belongs to, captured
                // before the first of its two windows opens -- review 25's
                // Minor 3. The rebuild below is the only consumer; see there
                // for why an era rather than a bare "is it populated?".
                let add_app_era = cache.epoch().era();
                if let Some(item) = picker_ui::pick_vault_item(&cache) {
                    log::info!("adding an app match to vault item {}", item.id);
                    match picker_ui::run_picker(cache.clone(), item, last_active_pid, backend_already_running) {
                        Some(m) => {
                            log::info!("saved app match for {} ({:?})", m.process, m.trigger);
                            // Make the new match live immediately rather than
                            // waiting for the user to trigger a sync -- from
                            // the CACHE, which already holds the save
                            // (`run_picker`'s Save goes through
                            // `cache.set_app_match`, which updates the
                            // snapshot on success precisely so nothing has to
                            // re-fetch it).
                            //
                            // This used to be `app::refresh_match_engine`
                            // (deleted in review 23, once this was its last
                            // caller and it had none left), a THIRD
                            // live `list_items` against `bw serve` after the
                            // picker's own populate and the save's PUT
                            // (review 21's Minor). A transient 500 or a reset
                            // connection on that one request logged a warn and
                            // left the engine unarmed, so the match the user
                            // had just spent two windows creating did not go
                            // live until some later sync -- the exact failure
                            // mode review 16 removed from the unlock path, and
                            // the reason nothing in this app arms the engine
                            // from a request that has not already succeeded.
                            //
                            // ONE LOCK, not `is_populated()` then `items()`
                            // (review 23's fifth Minor): the two-lock spelling
                            // was sound only by an argument about which thread
                            // every `clear` runs on, and this is a place where
                            // "populated" and "the items" must be the same
                            // observation.
                            //
                            // AND ONE DOOR, not two (review 25's Minor 3).
                            // This was `items_if_populated()`, which is
                            // `items_unless_superseded` minus the era check;
                            // the era `add_app_era` supplies is not ceremony
                            // here, because the two windows this flow opens
                            // are a long user interaction and a `clear` --
                            // the vault locking, or a re-auth into a possibly
                            // different ACCOUNT -- can land inside it. Arming
                            // the engine from whatever snapshot happens to
                            // exist afterwards would be arming it from a
                            // vault the user did not just edit.
                            if let Some(items) = cache.items_unless_superseded(add_app_era) {
                                let entries = match_entries(&items);
                                log::info!(
                                    "match engine refreshed: {} app match(es)",
                                    entries.len()
                                );
                                engine.rebuild(&entries);
                            } else {
                                // Not reachable from here today -- every
                                // `clear` runs on this thread, which is
                                // blocked inside the two picker windows for
                                // the whole flow, and `pick_vault_item`
                                // populates the cache and returns nothing to
                                // pick if that fails -- and checked rather
                                // than assumed, because rebuilding from an
                                // empty or another account's snapshot would
                                // DISARM autofill instead of merely failing to
                                // arm the new match.
                                log::warn!(
                                    "an app match was saved against a vault cache that is no \
                                     longer the session it was saved in (unpopulated, or cleared \
                                     and refilled meanwhile); leaving the match engine as it is \
                                     rather than rebuilding it from the wrong snapshot. The match \
                                     goes live at the next sync or unlock"
                                );
                            }
                        }
                        None => log::info!("app-match picker cancelled (or save failed)"),
                    }
                }

                // Our own picker windows just stole and released foreground.
                // Forget the last-dispatched hwnd so the window the user
                // returns to is treated as a fresh switch rather than being
                // suppressed as a repeat.
                last_dispatched_hwnd = None;
            }

            if event.id == tray.sync_id {
                // Defensive re-check, same reasoning as the update item just
                // below: the item is disabled while a sync (or a
                // window-open's own backend start) is in flight, but the
                // click event is handled the same way regardless of whether
                // tray-icon's disabled state actually suppressed the click.
                if backend_task_in_progress.is_some() {
                    log::info!("sync requested from tray but a backend operation is already in \
                                 progress; ignoring");
                } else {
                    log::info!("sync requested from tray");
                    tray::set_sync_in_progress(&tray);
                    backend_task_in_progress = Some((Instant::now(), BackendOpKind::Sync));

                    // Whether `bw serve` needs to be started first is decided
                    // here, on the main thread (the only place that owns
                    // `bw_serve_child`), and handed to the background thread
                    // as a plain bool -- see `backend_is_running`'s doc for
                    // why a `Some` child isn't automatically "running".
                    let currently_running = backend_is_running(&mut bw_serve_child);
                    spawn_sync(
                        session_token.clone(),
                        job.clone(),
                        cache.clone(),
                        currently_running,
                        backend_op_tx.clone(),
                    );
                }
            }

            if event.id == tray.update_id {
                // The item is disabled (and so shouldn't be clickable) until
                // `available_update` is `Some`, but the check is repeated
                // here defensively rather than trusting tray-icon's disabled
                // state to suppress the click event. Same reasoning for
                // re-checking `update_in_progress`.
                match (&available_update, update_in_progress) {
                    (Some(release), false) => {
                        log::info!(
                            "update requested from tray; downloading v{} in the background",
                            release.version
                        );
                        tray::set_update_in_progress(&tray, &release.version);
                        update_in_progress = true;

                        // Everything the thread needs is cloned in: the
                        // release (hence `ReleaseInfo: Clone`), the agent (a
                        // cheap `Arc` handle), the destination directory, and
                        // a sender. Nothing here is joined or waited on --
                        // the main loop keeps pumping messages and picks the
                        // outcome up from `apply_rx` whenever it lands.
                        let release = release.clone();
                        let agent = http_agent.clone();
                        let dest_dir = update_download_dir.clone();
                        let tx = apply_tx.clone();
                        std::thread::spawn(move || {
                            let outcome = updater::download_and_verify(
                                &release,
                                EXPECTED_SIGNER_THUMBPRINT,
                                &dest_dir,
                                &agent,
                            )
                            .and_then(|installer_path| updater::apply_update(&installer_path));
                            let _ = tx.send(outcome);
                        });
                    }
                    (Some(release), true) => log::info!(
                        "update to v{} is already being downloaded; ignoring repeat click",
                        release.version
                    ),
                    (None, _) => log::debug!("update item clicked with no update available"),
                }
            }
        }

        // Left click opens the vault directly; right click still shows the
        // menu (built with `with_menu_on_left_click(false)` specifically so
        // the two aren't the same action). Same event, same recovery path as
        // the menu's "Open Vault" item above -- just a different trigger.
        if let Some(event) = tray::next_tray_icon_event() {
            if tray::is_left_click(&event) {
                open_vault_window(
                    &cache,
                    &fill_stats,
                    &injector,
                    &mut session_token,
                    &mut bw_serve_child,
                    &job,
                    &store,
                    &schedule,
                    &mut engine,
                    &icon_cache_dir,
                    &mut cached_status_details,
                    settings.auto_lock_timeout(),
                    &tray,
                    &backend_op_tx,
                    &backend_op_rx,
                    &mut backend_task_in_progress,
                );
                last_dispatched_hwnd = None;
            }
        }

        if hotkey::fill_hotkey_pressed(&fill_hotkey) {
            if let Some((item_id, hwnd)) = pending_hotkey_fill.take() {
                // Revalidate against the *actual* current foreground window
                // rather than trusting the stored value alone: even with
                // the invalidation below, there's a window between the
                // event that armed this and the hotkey press where focus
                // could have moved without us having processed a
                // `ForegroundEvent` for it yet.
                let current_fg = unsafe { GetForegroundWindow() }.0 as isize;
                if current_fg == hwnd {
                    fill_from_vault(&cache, &injector, &fill_stats, &item_id, hwnd);
                } else {
                    log::info!("fill hotkey ignored: foreground window is no longer the match");
                }
            }
        }

        // Non-blocking: whenever a background backend operation
        // (`open_vault_window` making sure `bw serve` is up, or a
        // tray-triggered Sync) reports back, apply its outcome. This is also
        // where `backend_task_in_progress` is cleared, so the reconciliation
        // step right after it is never fighting a still-in-flight operation.
        if let Ok(op) = backend_op_rx.try_recv() {
            backend_task_in_progress = None;
            apply_backend_op(op, &mut bw_serve_child, &cache, &mut engine, &tray);
        }

        // A deadline on the FLAG itself, not just on any one `recv` -- see
        // `backend_task_in_progress`'s own doc for why a stalled `bw sync`
        // (or backend start) can otherwise wedge it `Some` forever with
        // nothing here ever noticing on its own. Reusing `BACKEND_OP_TIMEOUT`
        // rather than a second constant: it already means "how long a
        // legitimate backend operation can take before something is
        // genuinely wrong" for `open_vault_window`'s own bounded wait on this
        // same flag, and that reasoning applies here unchanged.
        if let Some((started, kind)) = backend_task_in_progress {
            if backend_task_is_wedged(started, BACKEND_OP_TIMEOUT) {
                backend_task_in_progress = None;
                // Report -- and, on the tray, only claim to have synced --
                // what actually stalled (review Minor 4). But *both* kinds
                // must still re-enable the tray's "Sync" item here (review
                // 11's Important 3): the comment this used to carry --
                // "an `EnsureRunning` wedge never touched the tray's Sync
                // item in the first place" -- stopped being true as soon as
                // the `tray.add_app_id` handler started disabling it before
                // kicking off its own `EnsureRunning` (see that call site).
                // Leaving it disabled here means a stalled "Add app..."
                // backend start (a hung `bw sync`, exactly the case this
                // wedge-deadline check exists for) permanently kills Sync
                // for the rest of the session: nothing else can re-enable
                // it, since `set_sync_idle`/`set_sync_failed` are only
                // reachable through paths that themselves require the item
                // to already be clickable.
                match kind {
                    BackendOpKind::Sync => {
                        log::error!(
                            "a background sync has been outstanding for over \
                             {BACKEND_OP_TIMEOUT:?} with no result; treating it as failed so the \
                             tray doesn't stay stuck on \"Syncing...\" forever"
                        );
                        tray::set_sync_failed(&tray);
                    }
                    BackendOpKind::EnsureRunning => {
                        log::error!(
                            "a background bw serve start (no sync requested) has been \
                             outstanding for over {BACKEND_OP_TIMEOUT:?} with no result; \
                             treating it as failed so the backend lifecycle doesn't stay wedged \
                             on it forever"
                        );
                        // Nothing is in flight any more, so the item goes all
                        // the way back to idle -- label, enabled state and
                        // tooltip together (review 18's Minor). Re-enabling
                        // alone used to be enough here only because the
                        // `tray.add_app_id` handler never changes the label;
                        // a wedged *sync* reaching a release like this one is
                        // what left the menu reading "Syncing..." on an idle
                        // item. Not `set_sync_failed`: no sync was requested,
                        // and claiming one failed would be untrue.
                        tray::set_sync_idle(&tray);
                    }
                }
            }
        }

        // The policy, reconciled here -- at idle, in the main loop -- rather
        // than only as a side effect of the vault window opening or closing.
        // This is what makes `keep_backend_running = false` actually save
        // memory in the common case (autofill-only, vault window never
        // opened this session): without it, `bw serve` -- started
        // unconditionally at startup so the cache could be populated --
        // would simply stay up forever, since nothing else was ever in a
        // position to notice and stop it. Only the "stop" half is evaluated
        // here; "start" is never something idle should initiate on its own
        // now that the periodic refresh that used to do that is gone (a
        // failed start would otherwise retry every ~200ms with nothing
        // throttling it) -- the three places that *do* need the backend
        // (startup, `open_vault_window`, the tray's Sync item) each ask for
        // it explicitly and this only ever tears it back down afterwards.
        if backend_task_in_progress.is_none() {
            stop_backend_if_idle(&mut bw_serve_child, settings.keep_backend_running);
        }

        if last_update_check.elapsed() >= UPDATE_CHECK_INTERVAL {
            // Same off-thread treatment as the startup check above: this now
            // runs once a day from a live, interactive app, but it still
            // talks to an external host, so it's still kicked off on a
            // background thread rather than blocking the main loop (and
            // therefore tray/hotkey/window-watch responsiveness) for however
            // long `api.github.com` takes to answer.
            let agent = http_agent.clone();
            let version = current_version.clone();
            let tx = update_tx.clone();
            std::thread::spawn(move || {
                if let Some(release) = check_for_update_logged(&version, &agent) {
                    let _ = tx.send(release);
                }
            });
            last_update_check = Instant::now();
        }

        // Non-blocking: whenever the prefetch thread (or a fallback
        // synchronous call inside `open_vault_window`) reports back, keep
        // the cache warm so the next "Open Vault" doesn't pay the `bw
        // status` spawn again.
        if let Ok(details) = status_details_rx.try_recv() {
            cached_status_details = Some(details);
        }

        if let Ok(release) = update_rx.try_recv() {
            // Not while a download is in flight: relabelling the item back to
            // "Update available" mid-download would contradict what is
            // actually happening (the click would be rejected anyway, see the
            // handler above). Rare -- checks are 24h apart -- but the tray is
            // the only status this app shows, so it shouldn't lie.
            if !update_in_progress {
                tray::set_update_available(&tray, &release.version);
            }
            available_update = Some(release);
        }

        // Non-blocking, like the check above: the download thread reports here
        // when it's finished (or failed), and the main loop never waits on it.
        if let Ok(outcome) = apply_rx.try_recv() {
            update_in_progress = false;
            match outcome {
                Ok(()) => {
                    // Same shutdown path as the Quit handler above: kill
                    // `bw serve` and clear the cache explicitly before
                    // exiting so the installer (which replaces and
                    // relaunches this binary) doesn't leave an orphaned
                    // backend serving the unlocked vault, or decrypted vault
                    // contents sitting in this process's memory a moment
                    // longer than it takes to tear down.
                    log::info!("update installer launched; shutting down for update");
                    cache.clear();
                    if let Some(child) = bw_serve_child.as_mut() {
                        bw_serve::stop_bw_serve(child);
                    }
                    std::process::exit(0);
                }
                Err(e) => {
                    // Surfaced, not just logged: a tray app with no window
                    // and no console has nowhere else to say this, and the
                    // user just asked for an update and is entitled to know
                    // it didn't happen.
                    log::error!("update failed: {e}");
                    if let Some(release) = &available_update {
                        tray::set_update_failed(&tray, &release.version);
                    }
                }
            }
        }

        if let Ok(event) = rx.recv_timeout(Duration::from_millis(200)) {
            if event.pid != own_pid {
                last_active_pid = Some(event.pid);
            }
            process_foreground_event(
                &event,
                &cache,
                &injector,
                &fill_stats,
                &engine,
                &mut pending_hotkey_fill,
                &mut last_dispatched_hwnd,
            );
        }
    }
}

/// Shows a native message box.
///
/// The only user-visible channel that exists this early. Everything in this
/// process is either a tray icon (not yet built at startup-check time) or an
/// egui window (which needs an event loop this code hasn't reached), and the
/// GUI subsystem means `eprintln!` goes nowhere at all -- so a plain
/// `MessageBoxW` is the one mechanism that can actually put words in front of
/// the user before anything else exists. `MB_SETFOREGROUND | MB_SYSTEMMODAL`
/// because this fires during login-time autostart, when whatever the shell is
/// doing would otherwise bury it.
fn message_box(title: &str, text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(text),
            &HSTRING::from(title),
            style | MB_SETFOREGROUND | MB_SYSTEMMODAL,
        )
    }
}

/// Logs `message`, shows it to the user, and exits.
///
/// The failure paths this replaces logged a line to a file nobody has open and
/// then called `exit(1)`: from the user's side, a double-clicked app that
/// simply never appeared, with no clue as to why.
///
/// **RESERVED FOR THE GENUINELY PRE-TRAY STARTUP PATH -- do not call it from
/// anywhere the tray already exists.** Every caller left is above `main`'s
/// loop, where there is no running app to preserve and no affordance the user
/// could recover through, so refusing to start is both true and the only
/// option. Past the tray it is neither: the text says "Deskwarden cannot
/// start" about an app that has been running for hours, and the exit takes
/// the tray, the global hotkey, autofill and window-watching down with it
/// over conditions that are usually transient. Three consecutive reviews
/// (12, 17, 18) each removed one such call from `open_vault_window`'s lock
/// recovery; the answer there is [`stand_down_after_unlock`], which leaves
/// the app running and locked and names a recovery that works.
fn fatal_startup_error(message: &str) -> ! {
    log::error!("refusing to start: {}", message.replace('\n', " "));
    message_box("Deskwarden cannot start", message, MB_ICONERROR | MB_OK);
    std::process::exit(1);
}

/// Checks that the resolved `bw.exe` is Bitwarden's, and decides what to do
/// when it can't be shown to be.
///
/// The response is graded rather than uniform, because the two failures are
/// not equally conclusive:
///
/// * **The signature itself is invalid** (unsigned, tampered with, expired,
///   or not chaining to a trusted root). That is a fact about the binary, not
///   an opinion of ours, and it is exactly the case this check exists to stop.
///   Refused outright -- with an explanation the user can actually see.
/// * **The signature is valid but the signer's `O=` isn't in
///   [`TRUSTED_BW_SIGNER_ORGANIZATIONS`]**, or the check couldn't be run at
///   all. Here the evidence points at *our* list as much as at the binary:
///   that list carries a standing "not yet confirmed against a real
///   Bitwarden-signed bw.exe" TODO, and `installer/bootstrap-bw.ps1` will
///   happily leave a Scoop- or Chocolatey-installed `bw` in place, whose
///   signer is legitimately somebody else. Hard-exiting on our own unverified
///   data would brick those installs with no recovery path -- the updater
///   can't help, this runs before it. So the user is told precisely what was
///   found and asked, with "no, quit" as the default button.
///
/// The judgment call, stated plainly: a *known-unverified* allowlist should
/// not be able to silently kill the app, but it also shouldn't be quietly
/// ignored, because the next thing to happen is the master password being
/// typed. Asking is the only option that is honest about both.
fn check_bw_signature(bw_exe: &std::path::Path) {
    let (headline, detail) = match deskwarden::signature::verify_authenticode(bw_exe) {
        Ok(info)
            if deskwarden::signature::is_trusted_organization(
                &info,
                TRUSTED_BW_SIGNER_ORGANIZATIONS,
            ) =>
        {
            log::info!(
                "bw CLI at {} verified as Bitwarden-signed",
                bw_exe.display()
            );
            return;
        }
        Ok(info) if !info.valid => {
            log::error!(
                "refusing to start: {} does not carry a valid Authenticode signature \
                 (subject: {:?})",
                bw_exe.display(),
                info.subject_dn
            );
            fatal_startup_error(&format!(
                "The Bitwarden CLI that Deskwarden found is not validly signed, so Deskwarden \
                 will not run it.\n\nFile:\n{}\n\nWindows could not confirm the file's \
                 signature. It may have been modified or replaced. Deskwarden hands this \
                 program your master password, so it is stopping instead.\n\nReinstall the \
                 Bitwarden CLI from bitwarden.com, or reinstall Deskwarden.",
                bw_exe.display()
            ));
        }
        Ok(info) => {
            log::warn!(
                "{} is validly signed, but by an organization not in the (still unverified) \
                 trusted list; subject: {:?}",
                bw_exe.display(),
                info.subject_dn
            );
            (
                "signed by an organization Deskwarden does not recognize",
                describe_signer(info.subject_dn.as_deref()),
            )
        }
        Err(e) => {
            log::warn!(
                "could not verify the signature of {}: {e}",
                bw_exe.display()
            );
            (
                "could not be signature-checked at all",
                format!("The check failed with: {e}"),
            )
        }
    };

    let answer = message_box(
        "Deskwarden: unrecognized Bitwarden CLI",
        &format!(
            "The Bitwarden CLI Deskwarden is about to use {headline}.\n\nFile:\n{}\n\n{detail}\n\n\
             Deskwarden gives this program your master password and vault session, so it should \
             only be Bitwarden's own CLI. This can also happen with a `bw` installed through \
             Scoop or Chocolatey, which are signed differently (or not at all).\n\nContinue \
             anyway?\n\nChoose No unless you know where this bw.exe came from.",
            bw_exe.display()
        ),
        MB_ICONWARNING | MB_YESNO | MB_DEFBUTTON2,
    );

    if answer == IDYES {
        log::warn!(
            "user chose to continue with an unrecognized bw.exe at {}",
            bw_exe.display()
        );
    } else {
        log::error!(
            "user declined to continue with an unrecognized bw.exe at {}; exiting",
            bw_exe.display()
        );
        std::process::exit(1);
    }
}

/// Turns a signer's subject DN into a sentence for the message box, since a
/// raw multi-line DN in a dialog is noise to everyone who isn't debugging.
fn describe_signer(subject_dn: Option<&str>) -> String {
    let Some(dn) = subject_dn else {
        return "It has no signer certificate.".to_string();
    };
    let orgs = deskwarden::signature::dn_component(dn, "O");
    match orgs.first() {
        Some(org) => format!("It is signed by: {org}"),
        None => "Its signer certificate names no organization.".to_string(),
    }
}

/// Applies the dispatch rules to one foreground event and, if it survives
/// them, matches and dispatches it.
///
/// Takes the cache, not the bridge -- `handle_match` (and the `fill_from_vault`
/// it may call) reads the vault from `VaultCache`'s snapshot rather than
/// hitting `bw serve` directly, which is what lets autofill keep working with
/// the backend stopped (see `backend_policy`).
fn process_foreground_event(
    event: &window_watch::ForegroundEvent,
    cache: &VaultCache,
    injector: &Injector<RealUiAutomation, RealSendInput>,
    fill_stats: &fill_stats::FillStats,
    engine: &MatchEngine,
    pending_hotkey_fill: &mut Option<(String, isize)>,
    last_dispatched_hwnd: &mut Option<isize>,
) {
    // Our own windows (prompt overlay, process picker, login) are focused,
    // always-on-top windows, so showing one fires EVENT_SYSTEM_FOREGROUND for
    // this process. Those are not app switches: ignore them entirely, without
    // even invalidating a pending hotkey fill (the target hasn't changed --
    // we just temporarily covered it).
    if dispatch::is_own_process(event.pid) {
        return;
    }

    // Any foreground-window change invalidates a pending hotkey
    // fill unless it's the very window that armed it re-foregrounding
    // (same hwnd). Without this, arming the fill and then switching
    // away to an unrelated window -- without ever pressing the fill
    // hotkey -- would leave `pending_hotkey_fill` stale: a later,
    // unrelated Ctrl+Alt+B press would fire it against a `hwnd` that
    // may since have been recycled by the OS for a different window,
    // contradicting the guarantee that the hotkey does nothing when
    // no matching window is foregrounded.
    if let Some((_, armed_hwnd)) = pending_hotkey_fill.as_ref() {
        if *armed_hwnd != event.hwnd {
            *pending_hotkey_fill = None;
        }
    }

    if !dispatch::should_dispatch(event, *last_dispatched_hwnd) {
        log::debug!(
            "suppressing repeat foreground event for hwnd {} ({})",
            event.hwnd,
            event.exe_name
        );
        return;
    }
    *last_dispatched_hwnd = Some(event.hwnd);

    if let Some((item_id, m)) = engine.lookup(&event.exe_name) {
        log::info!(
            "matched {} to vault item {item_id} (trigger {:?})",
            event.exe_name,
            m.trigger
        );
        if let Some(armed) =
            handle_match(cache, injector, fill_stats, item_id, m, event.hwnd, &event.exe_name)
        {
            *pending_hotkey_fill = Some(armed);
        }
    }
}

/// Calls `updater::check_for_update` against the real GitHub API and logs the
/// outcome. Network failures, a malformed release, and "no update" are all
/// deliberately non-fatal here -- this runs on a background thread (see call
/// sites), so the worst case is that a check is skipped until the next
/// cycle, not that the app goes down (or hangs) over a transient GitHub API
/// problem.
fn check_for_update_logged(current_version: &Version, agent: &ureq::Agent) -> Option<ReleaseInfo> {
    match updater::check_for_update(GITHUB_API_BASE, current_version, agent) {
        Ok(Some(release)) => {
            log::info!(
                "update available: v{} (current: v{current_version})",
                release.version
            );
            Some(release)
        }
        Ok(None) => {
            log::debug!("no update available (current: v{current_version})");
            None
        }
        Err(e) => {
            log::warn!("update check failed: {e}");
            None
        }
    }
}

/// Rebuilds both halves of the post-unlock state -- the vault cache and the
/// match engine -- once the readiness wait has confirmed `bw serve` is
/// answering with the *new* session.
///
/// Takes `cache`, not a separate `vault: &VaultBridge`: `cache.bridge()` is
/// that same bridge, so a second parameter would just be another name for it,
/// and since review 16 nothing in here needs the bridge directly anyway.
///
/// Both halves are rebuilt from `items` -- the vault the readiness probe
/// ITSELF listed, a fetch already known to have succeeded -- exactly as
/// startup does (`match_entries` + `engine.rebuild`, then `populate_with`).
/// Nothing here re-fetches the item list, and that is the whole point.
///
/// The history is worth keeping, because the same defect was fixed twice at
/// the wrong depth. Between 128000c and review 15's Important, the engine's
/// refresh was tied to `cache.populate()`'s outcome and the engine cleared
/// otherwise; `populate()` is two requests (`list_items` then `list_folders`)
/// and atomic over both, so a 500 on the folders half -- a failure
/// `picker_ui::load_items_for_picker`'s doc records as something that
/// actually happens -- cleared the engine even though the vault read fine.
/// e83ef03 made the two independent, which fixed the folders case and left
/// the engine depending on the since-deleted `app::refresh_match_engine`'s
/// own, THIRD `list_items`
/// (review 16's Important): a transient 500 or a connection reset on that one
/// request cleared the engine just the same. With no periodic match-engine
/// refresh left in this app, either version disarmed autofill silently for
/// the whole session -- nothing matches, nothing prompts, nothing arms the
/// hotkey, and the app looks perfectly alive.
///
/// Building from `items` removes the failure mode rather than moving it: the
/// engine's arming now depends on a fetch that has ALREADY SUCCEEDED, so
/// there is no request left whose failure could disarm it. It also drops two
/// full-vault round-trips (~1.1s / 1.08 MB each on a 1657-item vault, measured
/// in this repo) from a recovery that blocks the main thread.
///
/// There is consequently no `engine.clear()` here at all, and there must not
/// be one. The invariant that motivated the old coupling -- "an empty cache
/// beside a populated engine is inconsistent" -- is only true when the
/// ENGINE'S CONTENTS might belong to a different account. That is the
/// `Dismissed` arm's situation (no usable backend, nothing re-fetched, the
/// entries are whatever the pre-lock account left behind), and clearing there
/// is right and stays. Here the engine is rebuilt outright from the CURRENT
/// account's items, so pre-lock entries cannot survive even when the new
/// account has no app matches at all (`rebuild` replaces, it does not merge);
/// and an empty cache paired with those entries is a pairing this codebase
/// deliberately supports -- `app::fill_from_vault` falls back to the bridge on
/// a cache miss precisely so a fill still works in it.
fn repopulate_and_refresh_after_unlock(
    cache: &VaultCache,
    engine: &mut MatchEngine,
    items: Vec<deskwarden::vault_bridge::VaultItem>,
    // Captured by the caller BEFORE the readiness probe that produced
    // `items`, for the reason `VaultCache::epoch`'s doc gives: the guard can
    // only cover the window it is handed, and a `clear` landing between that
    // probe's fetch and this write is invisible to an epoch captured any
    // later. Same contract, and the same reason, as startup's
    // `startup_epoch`.
    epoch: VaultEpoch,
) {
    // Engine first, and unconditionally: it is pure, it cannot fail, and
    // doing it before the move into `populate_with` is what lets `items` be
    // handed to the cache rather than cloned.
    let entries = match_entries(&items);
    engine.rebuild(&entries);
    log::info!(
        "match engine rebuilt after unlock: {} app match(es)",
        entries.len()
    );

    // Seeds the cache with the same already-fetched items instead of listing
    // them again; still fetches folders, since nothing has. A failure here
    // leaves the engine armed on purpose (see this function's doc).
    match cache.populate_with(items, epoch) {
        Ok(PopulateOutcome::Populated) => {}
        Ok(PopulateOutcome::DiscardedStale) => log::warn!(
            "the vault cache was cleared again while repopulating after unlock; it stays empty"
        ),
        Err(e) => log::warn!(
            "could not repopulate the vault cache after unlock ({e:?}); autofill will fall back \
             to bw serve per fill until the next successful populate"
        ),
    }
}

/// Restarts `bw serve` for the lock recovery, standing autofill down instead
/// of exiting when it cannot be started. `None` means the recovery is over:
/// the caller has no child to track and nothing left to probe.
///
/// **Why it does not exit** (review 18's Important). This was the last
/// `fatal_startup_error` left in `open_vault_window`, and every sibling arm
/// around it had already been made survivable -- `Ready` survives an
/// all-500 backend, `Dismissed` survives by review 12's design, and review
/// 17 made the readiness TIMEOUT stand down for the reason that applies here
/// with a higher base rate still: there is an already-running app -- tray,
/// hotkey, autofill, window-watching -- to preserve, and killing it costs
/// the user far more than the transient it is reacting to.
///
/// Transient is the operative word. The dominant failure here is
/// [`BackendStartError::PortHeld`], and this call site killed *its own*
/// `bw serve` a few lines earlier, so a socket that has not been released
/// yet is the EXPECTED case rather than an exceptional one -- which is what
/// [`bw_serve::PORT_RELEASE_GRACE_RESTART`] exists for, and what
/// `try_start_backend`'s own doc already said in as many words ("returns the
/// failure instead of exiting, because on the restart paths ... killing the
/// whole app over a socket that needs another second to close is far
/// worse"). Only the caller disagreed, and it fired immediately after the
/// user had retyped their master password.
///
/// Standing down reuses [`stand_down_after_unlock`] rather than inventing a
/// second mechanism, so ONE place decides what "we could not get the vault
/// back" looks like and says so to the user. The state the caller is left in
/// is the one the `Dismissed` path already produces and has been shipping:
/// cache cleared, `bw_serve_child` `None` (the old child was stopped and the
/// new one never existed, so nothing is orphaned that this process was
/// tracking), a freshly re-authenticated `session_token`,
/// `cached_status_details` `None` so the next open re-fetches,
/// `backend_task_in_progress` `None`, the engine cleared, and the tray's
/// "Sync" item idle and clickable -- the recovery `stand_down_after_unlock`'s
/// message names. The one difference from the readiness-timeout stand-down is
/// that no backend is left running to come up on its own; a tray Sync starts
/// one itself (`spawn_sync` takes `currently_running: false`), so the named
/// recovery still works.
fn restart_backend_after_unlock(
    engine: &mut MatchEngine,
    start: impl FnOnce() -> Result<Child, BackendStartError>,
) -> Option<Child> {
    match start() {
        Ok(child) => Some(child),
        Err(e) => {
            stand_down_after_unlock(
                engine,
                &format!("the Bitwarden backend could not be restarted after unlocking ({e})"),
            );
            None
        }
    }
}

/// Decides what the lock recovery does with the readiness probe's outcome:
/// repopulate, retry once, or stand autofill down.
///
/// Split out of `open_vault_window` -- which takes seventeen parameters and
/// blocks the main thread on real windows -- so the composition itself (first
/// probe -> optional retry -> repopulate or stand down) is what the tests
/// drive, rather than a reimplementation of it beside the live one. `probe`
/// is the readiness wait, taking the message its spinner should show; the
/// only caller passes `wait_for_vault_ready_with_spinner`.
///
/// **Review 17's Critical.** Before this, a `Dismissed` here went straight to
/// `engine.clear()`, and the warn it logged advised the user to "open it
/// again from the tray to retry" -- advice that is false. The engine is only
/// ever rebuilt at four places (startup, this function's `Ready` path, a
/// completed tray `Sync`, and the "Add app..." save's rebuild from the cache),
/// and `open_vault_window` reaches the recovery ONLY when the window
/// reports `locked || needs_reauth`. A normal open/close never touches the
/// engine at all, so reopening the vault window repopulates the CACHE and
/// leaves the ENGINE empty: the user sees all their items and autofill is
/// still dead. The scenario is one impatient click -- the vault auto-locks,
/// the master password is accepted, `bw serve` restarts fine, the spinner
/// appears and the user closes it -- and review 12 already ruled that
/// gesture must not be destructive.
///
/// So a dismissal now buys ONE free readiness probe before anything
/// destructive happens, exactly as startup's own dismissal does
/// (`SETUP_RETRY_MESSAGE`). That matters beyond politeness:
/// `wait_for_vault_ready_with_spinner`'s worker is DETACHED and still
/// running at that moment, so the vault is very likely ready a second later
/// and the retry simply takes the ordinary `Ready` path -- engine armed from
/// the probe's own items, cache seeded from the same ones. It is bounded the
/// same way startup's is, and structurally rather than by a counter: two
/// `probe` calls appear in this function and there is no loop.
///
/// A `Failed` -- the ~30s readiness deadline expiring -- does not retry. It
/// has already spent that deadline, and startup's `Failed` arm does not
/// retry either. It does now STAND DOWN rather than exit: see
/// `stand_down_after_unlock`.
fn settle_vault_after_unlock(
    cache: &VaultCache,
    engine: &mut MatchEngine,
    epoch: VaultEpoch,
    mut probe: impl FnMut(&'static str) -> VaultReadyOutcome,
) {
    match probe(SETUP_MESSAGE) {
        VaultReadyOutcome::Ready(items) => {
            repopulate_and_refresh_after_unlock(cache, engine, items, epoch)
        }
        VaultReadyOutcome::Dismissed => {
            // Closing this window is not, on its own, evidence that anything
            // is broken -- the same reasoning startup's dismissal retry is
            // built on. Nothing is killed, nothing is re-authenticated: the
            // still-running backend gets one more honest look.
            log::info!(
                "setup window closed before the vault backend was confirmed ready after \
                 unlocking; probing readiness once more before standing autofill down"
            );
            match probe(SETUP_RETRY_MESSAGE) {
                VaultReadyOutcome::Ready(items) => {
                    repopulate_and_refresh_after_unlock(cache, engine, items, epoch)
                }
                VaultReadyOutcome::Dismissed => stand_down_after_unlock(
                    engine,
                    "the setup window was closed a second time without the vault backend \
                     becoming ready after unlocking",
                ),
                VaultReadyOutcome::Failed(e) => stand_down_after_unlock(
                    engine,
                    &format!("the vault backend did not become ready after unlocking ({e})"),
                ),
            }
        }
        VaultReadyOutcome::Failed(e) => stand_down_after_unlock(
            engine,
            &format!("the vault backend did not become ready after unlocking ({e})"),
        ),
    }
}

/// Leaves the app running with the vault effectively still locked: cache
/// empty (the recovery's own `cache.clear()` emptied it), engine empty, tray
/// and hotkey and window-watching all still alive.
///
/// **Why the engine is cleared** (review 13's Minor 3, unchanged): nothing
/// on this path confirmed that `bw serve` is answering under the new
/// session, so nothing re-fetched, so the engine can only be holding the
/// PRE-lock account's matches. Left armed beside an empty cache, a matched
/// process still raises the autofill prompt and the fill then misses in the
/// cache and falls through to a `get_item` with an id from an account this
/// session is no longer signed into -- a prompt that can only ever end in an
/// error log. This is deliberately NOT what the `Ready` path does, and the
/// difference is the backend: there the probe itself listed the vault, so
/// the engine is rebuilt from THOSE items and an empty cache beside them is
/// a supported pairing (see `repopulate_and_refresh_after_unlock`).
///
/// **Why the message names Sync** (review 17's Critical): the warn this
/// replaces told the user to "open it again from the tray to retry", and
/// reopening the vault window provably does not rebuild the engine -- see
/// `settle_vault_after_unlock`'s doc. The recoveries that actually do are
/// the tray's "Sync", an "Add app..." save, and another lock/unlock cycle
/// whose readiness probe is allowed to finish. A message that names a
/// recovery which does not work is worse than no message: it costs the user
/// the one chance they had of finding a working one.
///
/// **Why it does not exit** (review 17's Minor): this used to be two
/// different answers to two transient conditions -- a dismissal survived,
/// while a readiness TIMEOUT called `fatal_startup_error` and took the whole
/// process down with it. Review 12's justification for making the dismissal
/// survivable ("there is an already-running app to preserve") applies
/// identically to a timeout, the error text came from a function named for
/// STARTUP at a call site that is not startup, and a probe that timed out is
/// weaker evidence of unrecoverable breakage than the all-requests-500 state
/// the `Ready` path now deliberately survives. `fatal_startup_error` is
/// reserved for the genuinely pre-tray path.
///
/// The freshly (re)started `bw serve` from just above is left running rather
/// than killed: it may still come up on its own, in which case a tray Sync
/// works immediately. `main`'s idle reconciliation tears it back down if
/// `keep_backend_running` says to, exactly as it does after startup. The
/// third caller ([`restart_backend_after_unlock`], review 18) is the one case
/// where no backend is running at all, because starting it is what failed --
/// `bw_serve_child` is `None` there and a tray Sync starts one itself, so the
/// recovery this message names still works.
fn stand_down_after_unlock(engine: &mut MatchEngine, reason: &str) {
    engine.clear();
    log::warn!(
        "{reason}; leaving Deskwarden running with the vault effectively still locked. The app \
         matches are cleared too, so nothing can prompt to autofill until they are rebuilt: use \
         \"Sync\" in the tray menu to rebuild them. Reopening the vault window refills the item \
         cache but does NOT rebuild the app matches."
    );
}

/// Opens the vault window and handles it locking itself before returning.
/// Shared by both ways of asking for it -- the tray menu's "Open Vault" item
/// and a left click on the tray icon -- so the recovery sequence (mirroring
/// the startup retry path: `stop_bw_serve` on the old child ->
/// `reauthenticate` -> `try_start_backend` -> `wait_for_vault_ready_with_spinner`
/// -> rebuild the match engine) exists in exactly one place.
///
/// Does **not** decide whether `bw serve` should keep running once the
/// window closes. That decision used to live here, as an `else if
/// !backend_policy::should_run(..)` right after this function's old body --
/// which is exactly what review Critical 2 flagged: the *only* place the
/// policy was ever reconciled was a side effect of calling this function, so
/// a session that never opens the vault window (the normal autofill-only
/// case) held `bw serve` up forever under `keep_backend_running = false`.
/// The policy is now reconciled every idle iteration of `main`'s own loop
/// instead (see `stop_backend_if_idle`), which runs whether or not this
/// function was ever called. That also fixes review Important 4 as a direct
/// consequence: the old `locked` branch below never rechecked the policy (it
/// was the `if` half of an `if`/`else`, and the policy check was only in the
/// `else`), so locking the vault window in save-memory mode used to leave
/// the backend up indefinitely. Now both branches just return, and the
/// caller's next loop iteration reconciles either way.
///
/// Starting the backend for the window is also no longer awaited inline
/// (review Important 5): that used to be a scoped background thread joined
/// right after `vault_window::run` returned, which blocked the tray, the
/// global hotkey, and window-watching for up to ~30s (a port-release wait
/// plus a synchronous `bw sync`) on a window that may have been open for all
/// of two seconds -- and then immediately killed the child it just waited
/// for, if the policy said to. It's now a detached background operation
/// reported back through `backend_op_tx`/`backend_op_rx` and applied by
/// `main`'s own loop, same non-blocking shape as the update-download flow.
#[allow(clippy::too_many_arguments)]
fn open_vault_window<A: UiAutomationFiller + Clone + 'static, B: SendInputFiller + Clone + 'static>(
    cache: &Arc<VaultCache>,
    fill_stats: &deskwarden::fill_stats::FillStats,
    injector: &Injector<A, B>,
    session_token: &mut String,
    bw_serve_child: &mut Option<Child>,
    job: &Arc<Option<job_object::KillOnCloseJob>>,
    store: &session_store::SessionStore,
    schedule: &[Duration],
    engine: &mut MatchEngine,
    icon_cache_dir: &std::path::Path,
    // Warmed by a background thread at startup (see `main`'s
    // `status_details_rx`) and reused across opens, so the common case pays
    // no `bw status` spawn at all here. `None` only on a genuine cache miss
    // (a click landing before the prefetch reports back, or right after the
    // invalidation below) -- that path still falls back to the same
    // synchronous call this function always made, just no longer on every
    // single open.
    cached_status_details: &mut Option<login_ui::BwStatusDetails>,
    auto_lock: Duration,
    tray: &tray::AppTray,
    backend_op_tx: &mpsc::Sender<BackendOp>,
    backend_op_rx: &mpsc::Receiver<BackendOp>,
    backend_task_in_progress: &mut Option<(Instant, BackendOpKind)>,
) {
    let status_details = match cached_status_details.take() {
        Some(details) => details,
        None => login_ui::check_bw_status_details(),
    };
    // Refill the cache with what this open just used -- a cheap clone in the
    // common (already-cached) case, and what lets the *next* open skip the
    // spawn too when this call itself was the one that had to fall back.
    *cached_status_details = Some(status_details.clone());

    // Read once, before the `if` below might short-circuit past it, and
    // reused for `vault_window::run`'s own `backend_already_running`
    // (review Minor 3): whether `bw serve` was already up at this exact
    // moment -- before this function might kick off a start of its own --
    // is also exactly the fact `spawn_vault_load` needs to know it can skip
    // its readiness wait. Nothing between here and `vault_window::run`
    // returning stops or restarts the backend out from under this snapshot
    // (the only paths that do -- lock/reauth recovery -- close the window
    // and return first), so it stays valid for the window's whole session.
    let backend_already_running = backend_is_running(bw_serve_child);

    // Reads don't need `bw serve` at all (`vault_window::run` paints
    // entirely from `cache`); writes and TOTP do. If save-memory mode tore
    // the backend down after the last close (or it crashed -- review Minor
    // 8: `backend_is_running` catches a `Some(dead child)` that a plain
    // `.is_none()` check would miss), kick a start off in the background and
    // move straight on to opening the window rather than waiting for it --
    // see this function's doc for why waiting here used to be a real freeze.
    if needs_backend_start(backend_task_in_progress, backend_already_running) {
        *backend_task_in_progress = Some((Instant::now(), BackendOpKind::EnsureRunning));
        spawn_backend_start(session_token.clone(), job.clone(), backend_op_tx.clone());
    }

    let result = vault_window::run(
        cache.clone(),
        fill_stats.clone(),
        injector,
        status_details.server_url,
        status_details.user_email,
        session_token.clone(),
        icon_cache_dir.to_path_buf(),
        auto_lock,
        backend_already_running,
    );

    if result.locked || result.needs_reauth {
        // Two different triggers land here, both needing the exact same
        // recovery: the vault window locked itself (manual Lock button or
        // its own auto-lock timer), or a write inside it hit `bw serve`
        // returning 401 -- the session was invalidated out from under a
        // still-running backend (`bw lock` elsewhere, a server-side vault
        // timeout, a password change on another device). `backend_is_running`
        // only checks whether the *process* is alive, so that case would
        // otherwise go unnoticed forever: `bw serve` keeps answering, just
        // with 401s, and nothing before this fix ever re-authenticated (see
        // review Important 2). Both invalidate `bw serve`'s session exactly
        // the same way a rejected cached session does at startup, so both
        // get the same fix.
        if result.needs_reauth {
            log::warn!(
                "vault window write failed with an unauthorized session; re-authenticating"
            );
        } else {
            log::info!("vault window locked itself; re-authenticating");
        }

        // A backend operation kicked off above (or a tray Sync click that
        // landed while the window was open) may still be in flight. Unlike
        // `main`'s own non-blocking drain, this path is about to tear the
        // backend down and start a fresh one right now, so it has to wait
        // for that operation to actually finish first -- otherwise the two
        // attempts race to bind the same port. The user is already looking
        // at a blocking re-authentication flow at this point, so a few more
        // seconds here is not a new freeze, just a longer instance of one
        // that was already happening.
        //
        // Bounded, not a plain `recv()`: `backend_op_tx` lives in `main` for
        // the whole process, so the channel never disconnects on its own --
        // if the worker thread that owns the other end ever panicked before
        // sending, an unbounded `recv()` here would block this thread
        // forever, with no message pump running to keep the tray, hotkey, or
        // window-watching alive (review Minor). Giving up after
        // `BACKEND_OP_TIMEOUT` and proceeding anyway is strictly safer: the
        // worst case is racing a start/sync that eventually does land (see
        // `apply_backend_op`'s callers), not an unkillable app.
        if backend_task_in_progress.is_some() {
            log::info!("waiting for an in-flight backend operation before handling the lock");
            match backend_op_rx.recv_timeout(BACKEND_OP_TIMEOUT) {
                Ok(op) => apply_backend_op(op, bw_serve_child, cache, engine, tray),
                Err(_) => {
                    log::warn!(
                        "in-flight backend operation did not report back within \
                         {BACKEND_OP_TIMEOUT:?}; proceeding with lock recovery anyway. If it \
                         later reports back late (see `apply_backend_op`'s child-adoption \
                         guard), its child is stopped rather than allowed to overwrite the one \
                         this recovery is about to start."
                    );
                    // Review 11's Important 3: whatever operation was in
                    // flight may have disabled the tray's "Sync" item before
                    // stalling (the `tray.add_app_id` handler does, for its
                    // own `EnsureRunning`), and `apply_backend_op` -- the
                    // only other place that re-enables it -- is never going
                    // to run for an operation that gave up waiting on
                    // instead of receiving. Left disabled, a hung `bw sync`
                    // right here permanently kills Sync for the rest of the
                    // session. A no-op if nothing had disabled it.
                    //
                    // Review 18's Minor: all the way back to idle, not just
                    // re-enabled. THIS is the site where the two disagreed --
                    // the operation being abandoned here is very often the
                    // tray `Sync` that set the label to "Syncing...", and its
                    // thread is by definition never going to report back and
                    // relabel it. The stand-down message a few lines below
                    // names "Sync"; leaving the item saying "Syncing..."
                    // means the menu contains no item by that name, and the
                    // one that is there reads as busy.
                    tray::set_sync_idle(tray);
                }
            }
            *backend_task_in_progress = None;
        }

        // The account the *next* unlock lands on may not be this one (a
        // "Log out" followed by a different sign-in), so the snapshot built
        // from this account must not survive into that one. Left populated,
        // the next window open -- or the next autofill, straight from
        // `cache.items()` -- would silently serve this account's items and
        // passwords under the new session, indefinitely if `bw sync` then
        // fails offline.
        cache.clear();
        if backend_is_running(bw_serve_child) {
            if let Some(child) = bw_serve_child.as_mut() {
                bw_serve::stop_bw_serve(child);
            }
        }
        *bw_serve_child = None;
        *session_token = reauthenticate(store);
        // Drop the cached email/server too, for the same reason: the *next*
        // open must re-fetch rather than show a stale account in the
        // toolbar.
        *cached_status_details = None;

        // Same lifecycle as startup: the backend has to come up -- blocking,
        // with a spinner, since there is nothing useful to show without it
        // -- to re-populate the cache. `main`'s idle reconciliation tears it
        // back down afterwards if the policy says to, exactly as it does
        // after startup's own unconditional start; this function no longer
        // needs to know or care what the policy says.
        //
        // A failure to start it is survivable here, and standing down is the
        // whole of the recovery: see `restart_backend_after_unlock`. There is
        // nothing left to probe once no backend came up, so this returns
        // rather than spending the ~30s readiness deadline on a port nothing
        // is listening on.
        let Some(child) = restart_backend_after_unlock(engine, || {
            try_start_backend(
                session_token,
                job_ref(job),
                bw_serve::PORT_RELEASE_GRACE_RESTART,
            )
        }) else {
            return;
        };
        *bw_serve_child = Some(child);
        // Captured here, *before* the readiness probe below, for the same
        // reason startup captures `startup_epoch` before its own probe: that
        // probe's `list_items()` is the fetch whose result seeds the cache
        // via `populate_with`, and the epoch guard can only cover the window
        // it is handed (review 14's Minor 3). It has to be taken after the
        // `cache.clear()` further up -- that clear is the one this recovery
        // is repopulating from, not one to discard against. Nothing between
        // here and there clears the cache today (every `clear` site in the
        // crate runs on this same, currently blocked, main thread), so this
        // is inert; it is written this way so it stays correct if any of it
        // moves onto a background thread.
        let unlock_epoch = cache.epoch();
        settle_vault_after_unlock(cache, engine, unlock_epoch, |message| {
            wait_for_vault_ready_with_spinner(cache.bridge(), schedule, message)
        });
    }
}

/// Borrows the job object out of its `Arc` wrapper for a synchronous call.
///
/// The `Arc` only exists so a clone can be handed off to a background
/// thread (see `spawn_backend_start`/`spawn_sync`); every other call site
/// still just wants a plain `Option<&KillOnCloseJob>`, same as before that
/// wrapper existed.
fn job_ref(job: &Arc<Option<job_object::KillOnCloseJob>>) -> Option<&job_object::KillOnCloseJob> {
    job.as_ref().as_ref()
}

/// Whether `bw serve` is currently running, treating an already-exited child
/// the same as `None` rather than trusting `Option::is_some` alone.
///
/// `Child` has no way to notice its own process exiting on its own --
/// `bw_serve_child` stays `Some` even long after the process is gone unless
/// something calls `try_wait`. Review Minor 8: code that only checked
/// `.is_none()` to decide whether `bw serve` needed (re)starting would never
/// notice a `Some(dead child)` and so never restart it. Clears `*child` to
/// `None` on a detected exit, so callers can go back to the simpler
/// `is_none()` check afterwards.
fn backend_is_running(child: &mut Option<Child>) -> bool {
    let Some(c) = child.as_mut() else {
        return false;
    };
    match c.try_wait() {
        Ok(None) => true,
        Ok(Some(status)) => {
            log::warn!("bw serve exited on its own (status: {status}); treating it as stopped");
            *child = None;
            false
        }
        Err(e) => {
            // Can't tell either way. Assuming it's still running is the
            // safer failure mode: the alternative risks a second
            // `try_start_backend` racing the still-alive first one to bind
            // the same port.
            log::warn!("could not check whether bw serve is still running ({e}); assuming it is");
            true
        }
    }
}

/// Stops `bw serve` if it's running but [`backend_policy::should_run`] says
/// it shouldn't be, with no vault window open.
///
/// The other half of the policy -- starting the backend when it should be
/// running but isn't -- is deliberately not handled here as a symmetric
/// "else start it": with the periodic refresh removed (review Critical 1),
/// nothing throttles a repeated failure, and calling this every idle loop
/// iteration (as `main` does) would turn a backend that keeps failing to
/// start into a retry storm. The three places that genuinely need the
/// backend -- startup, `open_vault_window`, and the tray's Sync item -- each
/// ask for it explicitly instead; this function only ever tears it back
/// down again afterwards once the policy says it's no longer needed.
fn stop_backend_if_idle(bw_serve_child: &mut Option<Child>, keep_backend_running: bool) {
    if backend_policy::should_run(keep_backend_running) {
        return;
    }
    if backend_is_running(bw_serve_child) {
        log::info!("save-memory mode: nothing needs bw serve right now; stopping it");
        if let Some(child) = bw_serve_child.as_mut() {
            bw_serve::stop_bw_serve(child);
        }
        *bw_serve_child = None;
    }
}

/// Whether a backend operation marked in-flight since `started` has been
/// outstanding long enough to treat as wedged rather than merely slow.
///
/// A standalone predicate (rather than the `Duration` comparison inlined at
/// its one call site in `main`'s loop) purely so it can be unit tested --
/// `main` itself never returns, so nothing inside its loop is otherwise
/// reachable from a test. See `backend_task_in_progress`'s doc in `main` for
/// what not catching this leads to.
fn backend_task_is_wedged(started: Instant, deadline: Duration) -> bool {
    started.elapsed() >= deadline
}

/// Whether a background "ensure `bw serve` is running" kick should be
/// started right now.
///
/// Shared by `open_vault_window` and the tray's "Add app..." handler
/// (review 9's Important finding): both reach a step -- the vault window's
/// writes/TOTP, the picker's Save -- that needs `bw serve` up, and both
/// start it the same non-blocking way rather than waiting. Never start a
/// second attempt on top of one already in flight (`backend_task_in_progress`
/// racing itself to bind the same port), and never restart something that's
/// already running.
///
/// A standalone predicate for the same reason as `backend_task_is_wedged`:
/// testable without opening a window.
fn needs_backend_start(
    backend_task_in_progress: &Option<(Instant, BackendOpKind)>,
    backend_already_running: bool,
) -> bool {
    backend_task_in_progress.is_none() && !backend_already_running
}

/// Which kind of background backend operation `backend_task_in_progress` is
/// currently tracking. Recorded alongside the `Instant` so the wedge-deadline
/// check in `main`'s loop can say what actually stalled (review Minor 4):
/// `open_vault_window`'s `EnsureRunning` -- just making sure `bw serve` is up
/// before showing the window, no sync requested at all -- used to always be
/// reported (and shown on the tray) as a failed *sync* if it wedged, which
/// was simply untrue whenever no sync was involved.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BackendOpKind {
    EnsureRunning,
    Sync,
}

/// Outcome of a background operation that starts or restarts `bw serve`.
///
/// Both kinds -- `open_vault_window` making sure the backend is up, and the
/// tray's "Sync" item -- funnel through this one enum/channel rather than
/// one each, so `main`'s `backend_task_in_progress` flag can guarantee at
/// most one is ever in flight. Two concurrent `try_start_backend` calls
/// would race to bind the same port and make one fail for a reason that has
/// nothing to do with a real problem; sharing one channel also means there
/// is exactly one place -- not two -- a lock event has to drain before it
/// can safely stop and restart the backend itself (see `open_vault_window`'s
/// `locked` branch).
enum BackendOp {
    /// `open_vault_window` made sure the backend was up before showing the
    /// window. No sync/populate/rebuild attached -- reads already come from
    /// `cache` regardless of whether this succeeded.
    EnsureRunning(Result<Child, BackendStartError>),
    /// The tray's "Sync" item: ensure the backend is running (`child` is
    /// `Some` only if this operation itself had to start it), then run
    /// `bw sync` and repopulate the cache.
    Sync {
        child: Option<Result<Child, BackendStartError>>,
        outcome: SyncOutcome,
    },
}

/// What a tray-triggered sync actually achieved.
///
/// Three outcomes, not two: a `Result<(), String>` could not tell "the vault
/// was refreshed" apart from "the sync ran but its result was discarded
/// because the vault locked underneath it", so the latter took the success
/// path -- logging "sync complete", rebuilding the match engine from an
/// empty cache and returning the tray to its idle "Sync" label as though the
/// vault were freshly in sync (review 14's Minor).
///
/// This is what the WORKER observed. What the main thread should do about it
/// is [`SettledSync`], decided by `settle_sync_outcome`; both are matched
/// exhaustively, with no catch-all anywhere between here and the tray.
#[derive(Debug)]
enum SyncOutcome {
    /// `bw sync` succeeded and the refreshed vault landed in the cache.
    Refreshed {
        /// The vault era this sync was written in, captured by the worker
        /// BEFORE its own fetch. Re-checked by `settle_sync_outcome` when the
        /// outcome is finally applied on the main thread -- see that function
        /// and [`deskwarden::vault_cache::VaultEra`].
        ///
        /// The ERA, not the whole [`deskwarden::vault_cache::VaultEpoch`] the
        /// worker captured: the write half of that epoch is a writer's
        /// concern and was consumed by `populate_with` on the worker thread.
        /// What is left to decide here is only "is this still the same vault
        /// session?", and carrying a write position into it would invite
        /// exactly the "something changed, give up" reading review 18 removed.
        ///
        /// Deliberately the ONLY thing this variant carries. It used to carry
        /// the match entries this sync's own `list_items` produced as well,
        /// which is review 18's third finding: see `settle_sync_outcome`.
        era: VaultEra,
    },
    /// `bw sync` succeeded, but the cache was cleared while the repopulate
    /// was in flight, so nothing local was refreshed. Not a failure of the
    /// sync and not a success for the user's purposes either.
    DiscardedStale,
    /// Starting the backend, syncing, or repopulating failed.
    Failed(String),
}

/// What the main thread should DO about a completed sync, decided by
/// `settle_sync_outcome` the moment before it acts.
///
/// A separate type from [`SyncOutcome`] on purpose. `SyncOutcome` says what
/// happened on the worker thread, minutes ago and possibly for a vault
/// session that no longer exists; this says what is to be done here and now.
/// Collapsing the two -- `settle_sync_outcome` used to take a `SyncOutcome`
/// and return a `SyncOutcome` -- is what made it possible to write a
/// re-checked outcome that still carried the worker's own stale payload, and
/// for `apply_backend_op` to act on that payload believing the re-check had
/// blessed it. Here the only variant that means "go ahead" carries the data
/// to go ahead WITH, taken from the cache under the same check, so there is
/// nothing else in scope for the apply site to reach for.
enum SettledSync {
    /// The sync is still applicable -- same vault session, cache still
    /// populated -- and `items` is the snapshot AS IT STANDS NOW: this sync's
    /// refresh plus anything written since, which is newer truth than the
    /// sync and must survive it. Rebuild the match engine from these.
    Applicable {
        items: Vec<deskwarden::vault_bridge::VaultItem>,
    },
    /// The sync refreshed nothing that is still around to act on: either its
    /// own populate was discarded on the worker thread, or a `clear` (lock,
    /// re-auth, quit) started a new epoch before the main thread got here.
    /// Touch neither the engine nor the cache.
    NothingToApply,
    /// Starting the backend, syncing, or repopulating failed.
    Failed(String),
}

/// Applies a completed [`BackendOp`]: updates `bw_serve_child` and, for a
/// `Sync`, rebuilds the match engine and reflects the outcome on the tray.
///
/// Whether a sync's outcome is still applicable at all, and what to build the
/// engine from if it is, are both decided by `settle_sync_outcome` -- see
/// there for the contract. This function never reaches into `cache` itself
/// for that data, so there is no second, unchecked route to it.
fn apply_backend_op(
    op: BackendOp,
    bw_serve_child: &mut Option<Child>,
    cache: &VaultCache,
    engine: &mut MatchEngine,
    tray: &tray::AppTray,
) {
    match op {
        BackendOp::EnsureRunning(Ok(child)) => {
            if adopt_started_child(bw_serve_child, child) {
                log::info!("bw serve started for the vault window");
            }
            // Back to idle rather than merely re-enabled (review 18's
            // Minor): the "Add app..." handler that disabled it leaves the
            // label alone, but this arm also runs for an `EnsureRunning`
            // that a lock recovery abandoned, by which point the label may
            // be a stale "Syncing..." from an earlier wedged sync.
            tray::set_sync_idle(tray);
        }
        BackendOp::EnsureRunning(Err(e)) => {
            log::error!(
                "could not start bw serve for the vault window (writes and TOTP will fail until \
                 the next attempt; reads still work from the cache): {e}"
            );
            tray::set_sync_idle(tray);
        }
        BackendOp::Sync { child, outcome } => {
            match child {
                Some(Ok(c)) => {
                    adopt_started_child(bw_serve_child, c);
                }
                Some(Err(e)) => log::error!("sync could not start bw serve: {e}"),
                None => {}
            }
            match settle_sync_outcome(outcome, cache) {
                SettledSync::Applicable { items } => {
                    // From the snapshot `settle_sync_outcome` just checked and
                    // handed over, NOT from anything this sync froze earlier:
                    // a write that landed while the sync was in flight is
                    // newer truth and has to survive it (review 18's third
                    // finding). See `settle_sync_outcome`.
                    let entries = match_entries(&items);
                    log::info!(
                        "sync complete; match engine refreshed: {} app match(es)",
                        entries.len()
                    );
                    engine.rebuild(&entries);
                    tray::set_sync_idle(tray);
                }
                SettledSync::NothingToApply => {
                    // Deliberately touches neither the engine nor the cache:
                    // whatever cleared the cache (lock, re-auth) owns both,
                    // and by now it may already have repopulated them for a
                    // *different* account -- writing this sync's result here
                    // could just as easily wipe a freshly correct engine
                    // as clear a stale one. What must not happen is the tray
                    // reporting a completed sync for a sync that refreshed
                    // nothing locally; "click to retry" is the honest label,
                    // and retrying is exactly right once the vault is
                    // unlocked again.
                    log::warn!(
                        "sync ran, but the vault was locked while its result was being applied; \
                         nothing local was refreshed"
                    );
                    tray::set_sync_failed(tray);
                }
                SettledSync::Failed(e) => {
                    log::error!("sync failed: {e}");
                    tray::set_sync_failed(tray);
                }
            }
        }
    }
}

/// Adopts a freshly started `bw serve` child into `*bw_serve_child`, unless
/// one is already tracked there and still alive -- in which case the
/// incoming `child` is stopped instead. Returns whether it was adopted.
///
/// Exists for the race the final review's lock-recovery Minor flagged:
/// `open_vault_window`'s lock-recovery path gives up waiting on an in-flight
/// backend operation after `BACKEND_OP_TIMEOUT` and starts a fresh backend of
/// its own, synchronously, right there -- but giving up does not stop the
/// background thread it was waiting on. That thread can still complete
/// afterwards and send its own `Ok(child)` through the same channel, which
/// `main`'s ordinary non-blocking drain then hands to `apply_backend_op` like
/// any other result. Applying it unconditionally -- as this used to -- would
/// silently replace `*bw_serve_child` with that late, stale handle, orphaning
/// the newer process lock recovery is actually using: `Child`'s `Drop` does
/// not kill its process, so the replaced handle would simply be gone, with
/// nothing left able to stop or restart the process it pointed to on
/// purpose. Since at most one process can hold `BW_SERVE_PORT` at a time, a
/// late arrival landing while a live child is already tracked is by
/// definition redundant (or never got as far as actually binding the port),
/// so it's stopped outright rather than risking the swap.
fn adopt_started_child(bw_serve_child: &mut Option<Child>, mut child: Child) -> bool {
    if backend_is_running(bw_serve_child) {
        log::warn!(
            "a bw serve start reported back after a backend was already running (most likely \
             abandoned during lock recovery); stopping the redundant instance instead of \
             losing track of the one already in use"
        );
        bw_serve::stop_bw_serve(&mut child);
        return false;
    }
    *bw_serve_child = Some(child);
    true
}

/// Kicks off a background attempt to make sure `bw serve` is running,
/// reporting the outcome through `tx` rather than being joined -- see
/// `BackendOp`'s doc for why this can't just be awaited inline.
fn spawn_backend_start(
    session_token: String,
    job: Arc<Option<job_object::KillOnCloseJob>>,
    tx: mpsc::Sender<BackendOp>,
) {
    std::thread::spawn(move || {
        let result = try_start_backend(
            &session_token,
            job_ref(&job),
            bw_serve::PORT_RELEASE_GRACE_RESTART,
        );
        let _ = tx.send(BackendOp::EnsureRunning(result));
    });
}

/// Kicks off the tray's "Sync" item in the background: ensure the backend is
/// running, `bw sync`, then repopulate the cache.
///
/// `currently_running` is decided by the caller -- on the main thread, the
/// only place that owns `bw_serve_child` -- before this thread starts, so
/// there is no race between this thread's own start attempt and the main
/// loop's idle `stop_backend_if_idle`.
///
/// `try_start_backend` already runs `bw sync` itself as part of coming up
/// (see its doc), so this only issues a separate, explicit sync when the
/// backend was already running and therefore never got that free one.
fn spawn_sync(
    session_token: String,
    job: Arc<Option<job_object::KillOnCloseJob>>,
    cache: Arc<VaultCache>,
    currently_running: bool,
    tx: mpsc::Sender<BackendOp>,
) {
    std::thread::spawn(move || {
        // Captured before ANY fetch below, for the reason
        // `VaultCache::epoch`'s doc gives: the guard only covers the window
        // it is handed. This is the one genuinely live `DiscardedStale`
        // producer in the crate -- it runs on a background thread while the
        // main thread can call `cache.clear()` -- so unlike the other epoch
        // captures in this file it is not inert.
        let sync_epoch = cache.epoch();
        let child = if currently_running {
            None
        } else {
            Some(try_start_backend(
                &session_token,
                job_ref(&job),
                bw_serve::PORT_RELEASE_GRACE_RESTART,
            ))
        };

        // `Ok(Some(items))` when the readiness wait below already listed the
        // vault: `sync_outcome_from` reuses those rather than paying for a
        // second full-vault `list_items` (~1.1s / 1.08 MB on a 1657-item
        // vault, measured in this repo), the same reuse review 16 made on
        // the unlock path. `Ok(None)` when nothing has listed it yet.
        let start_failed = matches!(&child, Some(Err(_)));
        let ready = if start_failed {
            Err("bw serve could not be started".to_string())
        } else if currently_running {
            bw_serve::run_bw_sync(&session_token).map(|()| None)
        } else {
            // We just started `bw serve` ourselves. `try_start_backend`
            // returns as soon as the child process is resumed -- it does
            // *not* wait for `bw serve` (a bundled Node binary whose cold
            // start regularly takes several seconds) to actually be
            // listening. That gap is exactly why `wait_for_vault_ready`
            // exists and why the startup path always calls it before its
            // first `populate()`. Without the same wait here, `populate()`
            // below would very often race a backend that isn't answering
            // requests yet, fail with a connection error, and report "sync
            // failed" even though `try_start_backend`'s own `bw sync` had
            // completed successfully and the cache was never actually
            // refreshed -- precisely the mode this tray item exists for
            // (`keep_backend_running = false`, backend stopped at idle).
            // The `currently_running` branch above needs no such wait: a
            // backend that was already running before this click is, by
            // definition, already past this race.
            let schedule = readiness_schedule(READINESS_DEADLINE);
            wait_for_vault_ready(cache.bridge(), &schedule).map(Some)
        };

        let outcome = match ready {
            Err(e) => SyncOutcome::Failed(e),
            Ok(probe_items) => sync_outcome_from(&cache, sync_epoch, probe_items),
        };

        let _ = tx.send(BackendOp::Sync { child, outcome });
    });
}

/// **The freshness contract for a completed backend operation, in one
/// place.** Re-checks a sync's result against the cache as it stands NOW, on
/// the main thread, and answers with what to do about it.
///
/// There are two windows between a sync's `list_items` and the engine rebuild
/// it eventually causes, and they need OPPOSITE answers. That is the thing
/// five consecutive reviews of this seam have each rediscovered from a
/// different door, so it is written out rather than left to be re-derived:
///
///  1. **A `clear` -- lock, re-auth into a possibly different account, quit.**
///    The sync's result must be thrown away. `VaultCache::populate_with`'s own
///    epoch guard covers the worker's fetch-to-write window; this covers the
///    one after it, between the worker sending its outcome and `main` draining
///    it, which is reachable because `open_vault_window` abandons an in-flight
///    backend operation after `BACKEND_OP_TIMEOUT` and carries on into a lock
///    recovery that calls `cache.clear()`. Applied unguarded, such a late
///    arrival writes the PREVIOUS account's app matches over an engine the
///    recovery has just armed correctly for the new one -- the same
///    cross-account pairing `stand_down_after_unlock` exists to prevent, from
///    the other direction. It also covers review 17's finding, where the
///    recovery's own populate failed and left the cache legitimately empty:
///    that recovery clears first, so the epoch has moved and nothing is
///    applied to the empty cache.
///  2. **A WRITE -- an "Add app..." save, or any vault-window edit.** The
///    sync's result must NOT be thrown away, and must not overwrite the write
///    either. A write is newer truth than a fetch that predates it, and the
///    "Add app..." handler does not block on `backend_task_in_progress`
///    while its two picker windows block the main thread, so a save landing
///    inside a queued sync's window is ordinary, not exotic.
///
/// Review 18's third finding is what happens when one mechanism is asked to
/// answer both: `Refreshed` carried the entries from the sync's own
/// `list_items`, and an unchanged epoch was read as proving they were still
/// the vault. It never proved that -- writes mutate the snapshot without
/// touching the epoch, deliberately (see [`VaultEpoch`]) -- so the app match
/// the user had just spent two windows creating was rebuilt away, with two
/// success log lines and no warning.
///
/// The fix is not a second counter to detect case 2. A counter would only let
/// this discard a sync that is perfectly applicable, trading a silently lost
/// match for a silently skipped refresh. What case 2 wants is the snapshot
/// itself, at apply time -- which is also, by construction, exactly what case
/// 1 wants when it is safe: `Refreshed` is only produced when the sync's own
/// populate SUCCEEDED, so at an unchanged epoch the cache holds that sync's
/// items plus any newer writes. One question ("is this vault session still
/// the one I saw, and if so what is in it?"), asked of the one type that can
/// answer it under a single lock: [`VaultCache::items_unless_superseded`].
fn settle_sync_outcome(outcome: SyncOutcome, cache: &VaultCache) -> SettledSync {
    match outcome {
        SyncOutcome::Refreshed { era } => match cache.items_unless_superseded(era) {
            Some(items) => SettledSync::Applicable { items },
            None => {
                log::info!(
                    "a completed sync's result reached the main thread after the vault was \
                     cleared (era {era} -> {}); discarding it rather than writing it over \
                     whatever cleared it",
                    cache.epoch().era()
                );
                SettledSync::NothingToApply
            }
        },
        SyncOutcome::DiscardedStale => SettledSync::NothingToApply,
        SyncOutcome::Failed(e) => SettledSync::Failed(e),
    }
}

/// Refreshes the cache from `bw serve` after a completed `bw sync` and says
/// what that achieved.
///
/// `probe_items` lets a caller that has already listed the vault (the
/// readiness wait `spawn_sync` runs when it had to start the backend itself)
/// hand those items over instead of paying for a second full-vault
/// `list_items`.
///
/// `epoch` must be captured by the caller BEFORE the sync it is reporting on
/// -- see [`VaultCache::epoch`]. This is the one genuinely live
/// `DiscardedStale` producer in the crate.
///
/// **What this deliberately does NOT return** (review 18's third finding).
/// Between review 17 and review 18 it also returned `match_entries(&items)`,
/// so that the engine was rebuilt from the fetch that produced the outcome
/// rather than from a re-read of the cache on the main thread. That closed
/// review 17's case -- a late sync rebuilding the engine from a cache a lock
/// recovery had emptied -- but by freezing data on a background thread and
/// applying it minutes later, which is how it then erased writes that landed
/// in between. Review 17's case is closed by the epoch instead (that recovery
/// calls `cache.clear()`, which starts a new epoch, so the outcome is
/// discarded before it can be applied to anything), and the engine is once
/// again built from the cache at apply time -- see `settle_sync_outcome`,
/// which owns that decision and the reasoning behind it.
fn sync_outcome_from(
    cache: &VaultCache,
    epoch: VaultEpoch,
    probe_items: Option<Vec<deskwarden::vault_bridge::VaultItem>>,
) -> SyncOutcome {
    let items = match probe_items {
        Some(items) => Ok(items),
        None => cache.bridge().list_items(),
    };
    let items = match items {
        Ok(items) => items,
        Err(e) => return SyncOutcome::Failed(format!("{e:?}")),
    };

    match cache.populate_with(items, epoch) {
        Ok(PopulateOutcome::Populated) => SyncOutcome::Refreshed { era: epoch.era() },
        Ok(PopulateOutcome::DiscardedStale) => SyncOutcome::DiscardedStale,
        Err(e) => SyncOutcome::Failed(format!("{e:?}")),
    }
}

/// What the readiness spinner says the first time it is shown for a given
/// attempt at getting the vault ready.
const SETUP_MESSAGE: &str = "Setting up your vault...";

/// What it says when it comes back after the user closed it (review 13's
/// Minor 4). Closing the window used to bring an apparently identical one
/// straight back with nothing to distinguish it, so the retry read as the
/// app ignoring the click rather than as a deliberate second attempt. Kept
/// short: this is a 320px-wide window with one line of text.
const SETUP_RETRY_MESSAGE: &str = "Still not ready -- trying once more...";

/// What it says on the wait that follows a fresh master-password sign-in
/// (`recover_from_failed_vault_wait`).
///
/// Its own message rather than `SETUP_RETRY_MESSAGE` (review 14's nit):
/// "Still not ready -- trying once more..." describes a retry of something
/// the user watched fail, but from *this* window's point of view the user
/// has just typed their master password into a fresh login and nothing has
/// been tried since. What is actually happening is a backend that was just
/// restarted under a new session coming up.
const SETUP_AFTER_SIGN_IN_MESSAGE: &str = "Signed in -- starting your vault...";

/// Outcome of [`wait_for_vault_ready_with_spinner`].
///
/// Review 12's Critical: a user closing the "setting up" window and the
/// readiness probe itself genuinely failing used to both collapse into the
/// same `Err`, even though they call for very different responses --
/// dismissal is not evidence that anything is actually broken, while a
/// failure is. Kept as its own enum, not a sentinel string stuffed inside
/// `Err`, so that distinction is enforced by the compiler at every call site
/// (an exhaustive `match`, same discipline `TotpState` uses) rather than by
/// whoever remembers to check the message text.
enum VaultReadyOutcome {
    /// The vault became ready in time.
    Ready(Vec<deskwarden::vault_bridge::VaultItem>),
    /// The user closed the spinner (title-bar X / Alt+F4) before the probe
    /// reported back.
    Dismissed,
    /// The readiness probe itself failed or timed out.
    Failed(String),
}

/// Same as `wait_for_vault_ready`, but shows a spinner window for the
/// duration instead of blocking with nothing on screen.
///
/// The worker runs fully detached (`std::thread::spawn`, not a
/// `thread::scope`d one this function has to join before returning) --
/// review 12's Important 2. With a `thread::scope`d worker, closing the
/// spinner early (`show_while` returning `None`) still left this function's
/// caller blocked -- with no window on screen at all, the exact silence this
/// module's spinner exists to prevent -- until the probe finished on its
/// own, up to the rest of `schedule`'s ~30s deadline; the probe's own
/// eventual result then had nowhere to go (the receiver had already been
/// dropped) and was thrown away regardless of whether it was actually `Ok`.
/// `vault`/`schedule` are cloned into the worker rather than borrowed for
/// the same reason: a detached thread can't borrow the caller's stack.
/// `message` is what the spinner says. It is a parameter rather than a
/// constant because the retry after a dismissal has to look *different* from
/// the window the user just closed (review 13's Minor 4): re-running this
/// with the identical wording made closing the window pop an apparently
/// identical one straight back, with nothing to explain why, and closing
/// that one jumped to a master-password prompt with no explanation either.
/// The bounded retry itself is correct and stays; only its wording changes.
fn wait_for_vault_ready_with_spinner(
    vault: &VaultBridge,
    schedule: &[Duration],
    message: &str,
) -> VaultReadyOutcome {
    let (tx, rx) = mpsc::channel();
    let worker_vault = vault.clone();
    let worker_schedule = schedule.to_vec();
    std::thread::spawn(move || {
        let _ = tx.send(wait_for_vault_ready(&worker_vault, &worker_schedule));
    });
    match loading_ui::show_while(message, rx) {
        Some(Ok(items)) => VaultReadyOutcome::Ready(items),
        Some(Err(e)) => VaultReadyOutcome::Failed(e),
        None => VaultReadyOutcome::Dismissed,
    }
}

/// Recovers from a vault-readiness wait that didn't produce a ready vault at
/// startup -- either the probe genuinely failed, or the user dismissed the
/// spinner a second time in a row (see `main`'s own call sites for the free
/// first retry a mere dismissal gets before landing here). Kills the current
/// `bw serve`, sends the user through the login flow again (a rejected
/// session is indistinguishable from a slow start at this level, so this is
/// a reasonable guess even when the real cause turns out to be something
/// else), restarts the backend, and waits for it once more -- exiting
/// fatally if that second wait also doesn't produce a ready vault. There is
/// nothing left to fall back to at this point: the tray, hotkey, and
/// window-watch thread don't exist yet, so unlike the lock-recovery path in
/// `open_vault_window` (review 12's Critical), there is no already-running
/// app for a further dismissal here to preserve.
#[allow(clippy::too_many_arguments)]
fn recover_from_failed_vault_wait(
    reason: &str,
    vault: &VaultBridge,
    schedule: &[Duration],
    bw_serve_child: &mut Option<Child>,
    session_token: &mut String,
    job: &Arc<Option<job_object::KillOnCloseJob>>,
    store: &session_store::SessionStore,
    config_dir: &std::path::Path,
) -> Vec<deskwarden::vault_bridge::VaultItem> {
    log::error!("{reason}");
    log::warn!("retrying once after a fresh login, in case the session was rejected");
    if let Some(child) = bw_serve_child.as_mut() {
        bw_serve::stop_bw_serve(child);
    }
    *session_token = reauthenticate(store);
    // The longer grace: we just killed our own `bw serve`, and the user just
    // retyped their master password. Give the socket real time to come free
    // rather than aborting on them.
    *bw_serve_child = match try_start_backend(
        session_token.as_str(),
        job_ref(job),
        bw_serve::PORT_RELEASE_GRACE_RESTART,
    ) {
        Ok(child) => Some(child),
        Err(e) => {
            log::error!("{e}");
            fatal_startup_error(&format!(
                "Deskwarden could not start its Bitwarden backend after you signed \
                 in.\n\n{e}\n\nFull details are in:\n{}",
                logging::log_file_path(config_dir).display()
            ));
        }
    };

    match wait_for_vault_ready_with_spinner(vault, schedule, SETUP_AFTER_SIGN_IN_MESSAGE) {
        VaultReadyOutcome::Ready(items) => items,
        VaultReadyOutcome::Dismissed => {
            if let Some(child) = bw_serve_child.as_mut() {
                bw_serve::stop_bw_serve(child);
            }
            fatal_startup_error(
                "Deskwarden's Bitwarden backend restarted after you signed back in, but the \
                 setup window was closed again before it was confirmed ready.\n\nRelaunch \
                 Deskwarden and give the setup window a little longer to finish.",
            );
        }
        VaultReadyOutcome::Failed(e) => {
            log::error!("{e}");
            if let Some(child) = bw_serve_child.as_mut() {
                bw_serve::stop_bw_serve(child);
            }
            fatal_startup_error(&format!(
                "Deskwarden's Bitwarden backend started but never became usable, so \
                 there is nothing to match your apps against.\n\n{e}\n\nFull details \
                 are in:\n{}",
                logging::log_file_path(config_dir).display()
            ));
        }
    }
}

/// Runs the login/unlock UI and persists the resulting session token.
fn reauthenticate(store: &session_store::SessionStore) -> String {
    let token = login_ui::run_login_flow();
    if let Err(e) = store.save(&token) {
        log::error!("failed to persist session token: {e}");
    }
    token
}

/// Why `bw serve` could not be brought up.
///
/// Distinguished rather than collapsed into a string because the two cases
/// have very different prognoses: a held port frequently frees itself a moment
/// later, whereas a missing CLI never will.
enum BackendStartError {
    /// Something is still listening on the port after the grace period.
    PortHeld(Duration),
    /// No verified `bw.exe` is on record, so there is nothing safe to spawn.
    NoVerifiedCli(String),
    /// The `bw` process could not be spawned at all.
    Spawn(std::io::Error),
}

impl std::fmt::Display for BackendStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PortHeld(waited) => write!(
                f,
                "something is still listening on localhost:{} after waiting {waited:?} -- most \
                 likely an orphaned `bw serve` (or a grandchild of one we killed) that has not \
                 released the socket yet. Refusing to start rather than talking to an unknown \
                 process holding an unknown session.",
                bw_serve::BW_SERVE_PORT
            ),
            Self::NoVerifiedCli(e) => write!(f, "cannot start `bw serve`: {e}"),
            Self::Spawn(e) => write!(
                f,
                "failed to spawn `bw serve` from the verified Bitwarden CLI path (see \
                 bw_path::resolve_bw_exe for where that path comes from): {e}"
            ),
        }
    }
}

/// Syncs the vault, then spawns `bw serve` and attaches it to `job`.
///
/// Refuses to start if something is already listening on the `bw serve` port:
/// that is almost always an orphaned `bw serve` from a previous unclean exit,
/// and our newly spawned one would silently fail to bind while `VaultBridge`
/// happily talked to the *other* process -- a different, unknown session
/// serving an unknown vault.
///
/// Returns the failure instead of exiting, because on the restart paths (and
/// especially the one right after the user retyped their master password)
/// killing the whole app over a socket that needs another second to close is
/// far worse than logging and trying again on the next cycle. `port_grace` is
/// how long to give the port: short at startup, [`bw_serve::
/// PORT_RELEASE_GRACE_RESTART`] when we've just killed our own child.
fn try_start_backend(
    session_token: &str,
    job: Option<&job_object::KillOnCloseJob>,
    port_grace: Duration,
) -> Result<Child, BackendStartError> {
    if !bw_serve::wait_for_port_free(bw_serve::BW_SERVE_PORT, port_grace) {
        return Err(BackendStartError::PortHeld(port_grace));
    }

    // Pull the latest vault state down before the match engine is built, so a
    // match added on another device is live on first run rather than after the
    // next incidental sync.
    match bw_serve::run_bw_sync(session_token) {
        Ok(()) => log::info!("bw sync completed"),
        Err(e) => log::warn!("bw sync failed (continuing with cached vault): {e}"),
    }

    // Spawned suspended and assigned to the job before it runs a single
    // instruction, so there is no window in which a crash of *this* process
    // could orphan an unlocked-vault server. See `job_object::spawn_in_job`.
    let command =
        bw_serve::bw_serve_command(session_token).map_err(BackendStartError::NoVerifiedCli)?;
    job_object::spawn_in_job(job, command).map_err(BackendStartError::Spawn)
}

/// Startup variant of [`try_start_backend`]: there is nothing to fall back to
/// before the main loop exists, so a failure here is fatal.
fn start_backend(session_token: &str, job: Option<&job_object::KillOnCloseJob>) -> Child {
    match try_start_backend(session_token, job, bw_serve::PORT_RELEASE_GRACE) {
        Ok(child) => child,
        Err(e) => {
            log::error!("{e}");
            fatal_startup_error(&format!(
                "Deskwarden could not start its Bitwarden backend.\n\n{e}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real, short-lived child process, for exercising `backend_is_running`
    /// and `stop_backend_if_idle` against an actual `Child` without needing a
    /// real `bw serve` -- neither function cares what the process is, only
    /// whether it's alive.
    fn long_lived_command() -> std::process::Command {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/c", "ping", "-n", "20", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd
    }

    fn quick_exit_command() -> std::process::Command {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/c", "exit", "0"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd
    }

    fn kill_and_reap(child: &mut Option<Child>) {
        if let Some(c) = child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    #[test]
    fn backend_is_running_is_true_for_a_live_child() {
        let mut child = Some(long_lived_command().spawn().unwrap());
        assert!(backend_is_running(&mut child));
        assert!(child.is_some(), "a live child must not be cleared");
        kill_and_reap(&mut child);
    }

    #[test]
    fn backend_is_running_detects_an_already_exited_child_and_clears_it() {
        // Regression test for review Minor 8: code that only checked
        // `bw_serve_child.is_none()` never noticed a `Some(dead child)` and so
        // never restarted it. `wait()` blocks until the process has actually
        // exited (not just been asked to), so the `try_wait()` inside
        // `backend_is_running` is guaranteed to see it as gone rather than
        // racing a process that hasn't finished exiting yet.
        let mut c = quick_exit_command().spawn().unwrap();
        let _ = c.wait();
        let mut child = Some(c);

        assert!(!backend_is_running(&mut child));
        assert!(
            child.is_none(),
            "a dead child must be cleared to None, not left dangling as a stale Some"
        );
    }

    #[test]
    fn backend_is_running_is_false_with_nothing_running() {
        let mut child: Option<Child> = None;
        assert!(!backend_is_running(&mut child));
    }

    #[test]
    fn stop_backend_if_idle_leaves_a_running_backend_alone_when_keeping_it() {
        let mut child = Some(long_lived_command().spawn().unwrap());
        stop_backend_if_idle(&mut child, true);
        assert!(
            backend_is_running(&mut child),
            "keep_backend_running = true must never stop the backend"
        );
        kill_and_reap(&mut child);
    }

    #[test]
    fn stop_backend_if_idle_stops_a_running_backend_in_save_memory_mode() {
        // The core of review Critical 2's fix: with no vault window open and
        // `keep_backend_running = false`, idle reconciliation must actually
        // tear the backend down rather than leaving it running forever.
        let mut child = Some(long_lived_command().spawn().unwrap());
        stop_backend_if_idle(&mut child, false);
        assert!(
            child.is_none(),
            "save-memory mode must stop bw serve once nothing needs it"
        );
    }

    #[test]
    fn stop_backend_if_idle_is_a_no_op_with_nothing_running() {
        let mut child: Option<Child> = None;
        stop_backend_if_idle(&mut child, false);
        assert!(child.is_none());
    }

    #[test]
    fn stop_backend_if_idle_clears_an_already_dead_child_too() {
        // The `backend_is_running` fix applies here too: a dead child left in
        // `Some` must not be treated as "still needs stopping" (harmless) but
        // must at least end up cleared to `None` either way.
        let mut c = quick_exit_command().spawn().unwrap();
        let _ = c.wait();
        let mut child = Some(c);

        stop_backend_if_idle(&mut child, false);
        assert!(child.is_none());
    }

    #[test]
    fn backend_task_is_wedged_is_false_while_within_the_deadline() {
        let started = Instant::now();
        assert!(!backend_task_is_wedged(started, Duration::from_secs(60)));
    }

    #[test]
    fn backend_task_is_wedged_is_true_once_the_deadline_has_passed() {
        // Regression test for final review Important 2: `run_bw_sync` has no
        // timeout of its own, so nothing else ever notices a stalled
        // operation on its own -- this predicate is what `main`'s loop uses
        // to catch it. Backdating `started` rather than sleeping keeps the
        // test instant.
        let started = Instant::now() - Duration::from_secs(120);
        assert!(backend_task_is_wedged(started, Duration::from_secs(90)));
    }

    #[test]
    fn needs_backend_start_is_true_with_nothing_running_and_no_task_in_flight() {
        assert!(needs_backend_start(&None, false));
    }

    #[test]
    fn needs_backend_start_is_false_while_a_task_is_already_in_flight() {
        // Guards against two attempts racing to bind the same port -- see
        // this fn's doc.
        let in_progress = Some((Instant::now(), BackendOpKind::EnsureRunning));
        assert!(!needs_backend_start(&in_progress, false));
    }

    #[test]
    fn needs_backend_start_is_false_when_the_backend_is_already_running() {
        assert!(!needs_backend_start(&None, true));
    }

    #[test]
    fn adopt_started_child_adopts_into_an_empty_slot() {
        let mut bw_serve_child: Option<Child> = None;
        let child = long_lived_command().spawn().unwrap();

        assert!(adopt_started_child(&mut bw_serve_child, child));
        assert!(backend_is_running(&mut bw_serve_child));
        kill_and_reap(&mut bw_serve_child);
    }

    #[test]
    fn adopt_started_child_stops_a_late_arrival_instead_of_replacing_a_live_one() {
        // Regression test for the final review's lock-recovery Minor:
        // `open_vault_window`'s lock-recovery path can give up waiting on a
        // backend operation
        // (`BACKEND_OP_TIMEOUT` expiry) and start its own fresh backend
        // before the abandoned operation's own `Ok(child)` eventually arrives
        // through `apply_backend_op`. That late arrival must not overwrite
        // the handle to the backend actually in use -- doing so would orphan
        // it (`Child::drop` does not kill its process) with nothing left able
        // to stop or restart it on purpose.
        let mut bw_serve_child = Some(long_lived_command().spawn().unwrap());
        let current_pid = bw_serve_child.as_ref().unwrap().id();

        let late_arrival = long_lived_command().spawn().unwrap();
        let late_pid = late_arrival.id();
        assert!(!adopt_started_child(&mut bw_serve_child, late_arrival));

        // The originally tracked child must still be the one in place...
        assert_eq!(
            bw_serve_child.as_ref().unwrap().id(),
            current_pid,
            "the live, already-tracked child must not be replaced"
        );
        // ...and the redundant late arrival must actually have been stopped,
        // not merely dropped (which would leave it running, untracked).
        // `adopt_started_child` routes it through `stop_bw_serve`, which
        // calls `wait()` after `kill()`, so the process is already reaped by
        // the time this assertion runs.
        assert!(
            !is_pid_running(late_pid),
            "the redundant late-arriving child must be stopped, not orphaned"
        );

        kill_and_reap(&mut bw_serve_child);
    }

    fn vault_item_with_match(id: &str, process: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"{id}","type":1,"fields":[{{"name":"deskwarden:app-match","value":"{{\"process\":\"{process}\",\"trigger\":\"auto\"}}"}}]}}"#
        )
    }

    /// The items a readiness probe would have handed back.
    fn probe_items(specs: &[(&str, &str)]) -> Vec<deskwarden::vault_bridge::VaultItem> {
        specs
            .iter()
            .map(|(id, process)| {
                serde_json::from_str(&vault_item_with_match(id, process))
                    .expect("the test fixture must deserialize as a vault item")
            })
            .collect()
    }

    /// Review 15's Important: a transient `list_folders` failure on the
    /// post-unlock repopulate must NOT disarm autofill for the rest of the
    /// session. `populate_with` still fetches folders, so a 500 on that
    /// request fails the whole populate -- but the match engine is built
    /// from the readiness probe's OWN items, a fetch already known to have
    /// succeeded against a backend just restarted with the new session, so
    /// its entries are the current account's by construction and
    /// `fill_from_vault`'s documented bridge fallback serves the fill from
    /// an empty cache.
    #[test]
    fn a_folders_failure_after_unlock_leaves_the_match_engine_armed() {
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(500)
            .with_body("nope")
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let epoch = cache.epoch();

        repopulate_and_refresh_after_unlock(
            &cache,
            &mut engine,
            probe_items(&[("1", "notepad.exe")]),
            epoch,
        );

        assert!(
            !cache.is_populated(),
            "the populate genuinely failed; this test is about what happens *despite* that"
        );
        assert!(
            engine.lookup("notepad.exe").is_some(),
            "a transient list_folders failure must not disarm autofill for the whole session -- \
             there is no periodic match-engine refresh left, so nothing would ever re-arm it"
        );
    }

    /// Review 16's Important: the engine must be armed from the readiness
    /// probe's own items even if the backend answers 500 to absolutely
    /// everything afterwards. Before this fix the engine was rebuilt by
    /// `app::refresh_match_engine` (since deleted), i.e. by a THIRD
    /// `list_items` after the
    /// probe's and the populate's, and a transient failure of that one
    /// request cleared the engine and silently disarmed autofill for the
    /// whole session -- the exact blast radius of review 15's finding, one
    /// request over.
    #[test]
    fn the_engine_is_armed_from_the_probes_items_even_if_every_later_request_fails() {
        let mut server = mockito::Server::new();
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(500)
            .with_body("nope")
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(500)
            .with_body("nope")
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let epoch = cache.epoch();

        repopulate_and_refresh_after_unlock(
            &cache,
            &mut engine,
            probe_items(&[("1", "notepad.exe")]),
            epoch,
        );

        assert!(
            engine.lookup("notepad.exe").is_some(),
            "the engine's arming must depend only on the fetch already known to have succeeded, \
             so no later backend failure can disarm autofill for the rest of the session"
        );
    }

    /// The other half of the same invariant: entries the engine is holding
    /// from the account this app was signed into BEFORE the unlock must not
    /// survive into a session that may be a different account. The probe's
    /// items are the new account's, so rebuilding from them replaces the old
    /// ones outright -- including when the new account has no app matches at
    /// all, which is the case that would otherwise leave stale ones armed.
    #[test]
    fn matches_from_the_pre_lock_account_do_not_survive_the_unlock() {
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(500)
            .with_body("nope")
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch {
                process: "notepad.exe".into(),
                trigger: deskwarden::app_match::TriggerMode::Auto,
            },
        )]);
        let epoch = cache.epoch();

        repopulate_and_refresh_after_unlock(&cache, &mut engine, probe_items(&[]), epoch);

        assert!(
            engine.lookup("notepad.exe").is_none(),
            "matches from the account this app was signed into before the unlock must not \
             survive an unlock whose own vault does not have them"
        );
    }

    /// The cache seeding reuses the probe's items too, so a successful
    /// populate needs only the folders request -- no second `list_items`.
    #[test]
    fn the_cache_is_seeded_from_the_probes_items_without_listing_them_again() {
        let mut server = mockito::Server::new();
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .expect(0)
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let epoch = cache.epoch();

        repopulate_and_refresh_after_unlock(
            &cache,
            &mut engine,
            probe_items(&[("1", "notepad.exe")]),
            epoch,
        );

        assert!(cache.is_populated(), "the populate must have succeeded");
        assert_eq!(cache.items().len(), 1, "seeded from the probe's own items");
        items.assert();
    }

    /// A scripted stand-in for `wait_for_vault_ready_with_spinner`, recording
    /// the message each probe was asked to show so a test can assert both
    /// HOW MANY probes ran and that the retry looked different from the
    /// window the user just closed.
    fn scripted_probe<'a>(
        script: Vec<VaultReadyOutcome>,
        seen: &'a std::cell::RefCell<Vec<&'static str>>,
    ) -> impl FnMut(&'static str) -> VaultReadyOutcome + 'a {
        let mut remaining = script.into_iter();
        move |message| {
            seen.borrow_mut().push(message);
            remaining
                .next()
                .expect("the lock recovery must not probe more times than the script allows")
        }
    }

    /// Review 17's Critical: closing the post-unlock spinner is ONE CLICK,
    /// the gesture review 12 already ruled must not be destructive, and it
    /// used to disarm autofill for the rest of the session -- the detached
    /// readiness worker was very likely about to answer `Ok(items)`, and
    /// that answer was thrown away. Startup gives a dismissal one free probe
    /// (`SETUP_RETRY_MESSAGE`); this site now gives the same one, and a
    /// probe that then succeeds takes the ordinary `Ready` path.
    #[test]
    fn a_dismissed_spinner_after_unlock_gets_one_free_readiness_retry() {
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let mut engine = MatchEngine::new();
        let epoch = cache.epoch();
        let seen = std::cell::RefCell::new(Vec::new());

        settle_vault_after_unlock(
            &cache,
            &mut engine,
            epoch,
            scripted_probe(
                vec![
                    VaultReadyOutcome::Dismissed,
                    VaultReadyOutcome::Ready(probe_items(&[("1", "notepad.exe")])),
                ],
                &seen,
            ),
        );

        assert!(
            engine.lookup("notepad.exe").is_some(),
            "a dismissal followed by a successful retry must arm the engine exactly as a \
             first-probe success does -- otherwise one impatient click kills autofill for the \
             whole session with no recovery the user would ever think to try"
        );
        assert!(
            cache.is_populated(),
            "the retry's items must seed the cache too, i.e. the ordinary Ready path"
        );
        assert_eq!(
            *seen.borrow(),
            vec![SETUP_MESSAGE, SETUP_RETRY_MESSAGE],
            "the retry has to look different from the window the user just closed \
             (review 13's Minor 4)"
        );
    }

    /// The retry is bounded exactly as startup's is: a dismissal buys one
    /// more probe and nothing more. Two calls, structurally -- not a loop.
    #[test]
    fn a_second_dismissal_after_unlock_stands_autofill_down_without_looping() {
        let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch {
                process: "notepad.exe".into(),
                trigger: deskwarden::app_match::TriggerMode::Auto,
            },
        )]);
        let epoch = cache.epoch();
        let seen = std::cell::RefCell::new(Vec::new());

        settle_vault_after_unlock(
            &cache,
            &mut engine,
            epoch,
            scripted_probe(
                vec![VaultReadyOutcome::Dismissed, VaultReadyOutcome::Dismissed],
                &seen,
            ),
        );

        assert_eq!(
            seen.borrow().len(),
            2,
            "exactly one retry -- the scripted probe panics if a third is asked for"
        );
        assert!(
            engine.lookup("notepad.exe").is_none(),
            "nothing confirmed the backend, so the engine can only be holding the PRE-lock \
             account's matches and a locked app must be inert (review 13's Minor 3)"
        );
    }

    /// Review 17's Minor: a readiness TIMEOUT is a transient condition, and
    /// it used to call `fatal_startup_error` -- killing the tray, the
    /// hotkey, autofill and window-watching over a ~30s probe that did not
    /// answer, at a call site that is not startup and has an already-running
    /// app to preserve. It now stands down exactly as a dismissal does. That
    /// this test can run at all is the assertion: the old arm called
    /// `std::process::exit(1)`.
    #[test]
    fn a_readiness_timeout_after_unlock_leaves_the_app_running() {
        let cache = VaultCache::new(VaultBridge::new("http://127.0.0.1:1".to_string()));
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch {
                process: "notepad.exe".into(),
                trigger: deskwarden::app_match::TriggerMode::Auto,
            },
        )]);
        let epoch = cache.epoch();
        let seen = std::cell::RefCell::new(Vec::new());

        settle_vault_after_unlock(
            &cache,
            &mut engine,
            epoch,
            scripted_probe(
                vec![VaultReadyOutcome::Failed("timed out".to_string())],
                &seen,
            ),
        );

        assert_eq!(
            seen.borrow().len(),
            1,
            "a genuine timeout has already spent the whole readiness deadline; it does not buy \
             another one (startup's Failed arm does not retry either)"
        );
        assert!(
            engine.lookup("notepad.exe").is_none(),
            "same stand-down as a dismissal: empty cache, empty engine, app alive and locked"
        );
    }

    /// Review 18's Important, and the twin of the test above. Commit 7041360
    /// made the readiness TIMEOUT survivable on the argument that a transient
    /// failure must not kill tray, hotkey, autofill and window-watching when
    /// there is a running app to preserve -- and then left the
    /// `try_start_backend` failure twenty lines earlier calling
    /// `fatal_startup_error`. That failure is *more* likely to be transient,
    /// not less: its dominant shape is `PortHeld`, and this very call site
    /// killed its own `bw serve` moments before, so a port that has not been
    /// released yet is the EXPECTED case -- which is exactly why
    /// `PORT_RELEASE_GRACE_RESTART` exists and why `try_start_backend`'s own
    /// doc says it returns the failure "instead of exiting, because on the
    /// restart paths (and especially the one right after the user retyped
    /// their master password) killing the whole app over a socket that needs
    /// another second to close is far worse". Only the caller disagreed.
    ///
    /// It now stands down through the same `stand_down_after_unlock` the
    /// readiness arms use, so there is one place that decides what "we could
    /// not get the vault back" looks like. That this test runs at all is
    /// again part of the assertion: the old arm called `std::process::exit(1)`
    /// and would have taken the test runner with it.
    #[test]
    fn a_backend_that_cannot_be_restarted_after_unlock_leaves_the_app_running() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch {
                process: "notepad.exe".into(),
                trigger: deskwarden::app_match::TriggerMode::Auto,
            },
        )]);

        let child = restart_backend_after_unlock(&mut engine, || {
            Err(BackendStartError::PortHeld(Duration::from_secs(1)))
        });

        assert!(
            child.is_none(),
            "there is no child to track -- and `bw_serve_child` must stay None so the next open \
             starts one rather than talking to a process nothing owns"
        );
        assert!(
            engine.lookup("notepad.exe").is_none(),
            "nothing confirmed the backend under the NEW session, so the engine can only hold \
             the pre-lock account's matches: same stand-down the readiness arms produce, not an \
             exit and not a silently armed engine"
        );
    }

    /// The other half, so the fix above cannot pass by standing down
    /// unconditionally: a start that succeeds hands the child straight back
    /// and touches nothing. The engine it leaves alone is about to be rebuilt
    /// by the readiness probe on the ordinary path.
    #[test]
    fn a_backend_that_does_restart_after_unlock_is_handed_back_untouched() {
        let mut engine = MatchEngine::new();
        engine.rebuild(&[(
            "old".to_string(),
            deskwarden::app_match::AppMatch {
                process: "notepad.exe".into(),
                trigger: deskwarden::app_match::TriggerMode::Auto,
            },
        )]);

        let started = restart_backend_after_unlock(&mut engine, || {
            std::process::Command::new("cmd")
                .args(["/C", "exit"])
                .spawn()
                .map_err(BackendStartError::Spawn)
        });

        let mut child = started.expect("a successful start must hand its child back");
        let _ = child.wait();
        assert!(
            engine.lookup("notepad.exe").is_some(),
            "a successful restart must not stand autofill down"
        );
    }

    /// A `bw serve` that answers one item carrying an app match, plus the
    /// folders every populate also fetches.
    fn sync_server() -> mockito::ServerGuard {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"success":true,"data":{{"data":[{}]}}}}"#,
                vault_item_with_match("1", "notepad.exe")
            ))
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        server
    }

    /// The ordinary case, so the guard below cannot pass by simply
    /// discarding everything: nothing cleared the vault underneath this
    /// sync, so its own entries arm the engine.
    #[test]
    fn a_completed_sync_arms_the_engine_from_the_entries_it_fetched() {
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let outcome = sync_outcome_from(&cache, cache.epoch(), None);

        let mut engine = MatchEngine::new();
        // No `{other:?}` here, and `SettledSync` deliberately does not derive
        // `Debug`: its applicable variant carries vault items, and this is a
        // type whose whole job is to be matched on rather than printed.
        match settle_sync_outcome(outcome, &cache) {
            SettledSync::Applicable { items } => engine.rebuild(&match_entries(&items)),
            SettledSync::NothingToApply => panic!("expected a refreshed sync, got NothingToApply"),
            SettledSync::Failed(e) => panic!("expected a refreshed sync, got Failed({e})"),
        }
        assert!(
            engine.lookup("notepad.exe").is_some(),
            "a sync nothing interfered with must still refresh the match engine"
        );
    }

    /// Review 17's third finding. `SyncOutcome::Refreshed` used to be a unit
    /// variant, and `apply_backend_op` answered it by rebuilding the engine
    /// from a RE-READ of `cache.items()` on the main thread. That read
    /// happens after the worker sent the outcome, and a lock recovery can
    /// land in between -- reachable because `open_vault_window` abandons an
    /// in-flight backend operation at `BACKEND_OP_TIMEOUT` and carries on
    /// into a recovery that clears the cache. If that recovery then went
    /// `Ready` with a failing `list_folders` (engine armed from the probe's
    /// items, cache empty by design), the late sync rebuilt the engine from
    /// nothing and disarmed autofill for the whole session -- review 15's
    /// finding through a different door.
    ///
    /// Review 18 removed the entries again and left the epoch to close this,
    /// which is why the test is worth reading twice: the recovery's own
    /// `cache.clear()` starts a new epoch, so the outcome is discarded before
    /// anything is read from the empty cache. The engine the recovery armed
    /// is left exactly as it is -- the same assertion as before, now resting
    /// on the epoch rather than on frozen entries.
    #[test]
    fn a_late_sync_result_cannot_disarm_an_engine_a_lock_recovery_just_armed() {
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let outcome = sync_outcome_from(&cache, cache.epoch(), None);

        // The lock recovery: clears the cache, then arms the engine from its
        // own readiness probe while `populate_with`'s folders request fails.
        cache.clear();
        let mut engine = MatchEngine::new();
        engine.rebuild(&match_entries(&probe_items(&[("2", "code.exe")])));

        assert!(
            matches!(
                settle_sync_outcome(outcome, &cache),
                SettledSync::NothingToApply
            ),
            "a sync whose snapshot was cleared before its result reached the main thread must \
             not be applied to anything"
        );
        assert!(
            engine.lookup("code.exe").is_some(),
            "the engine the recovery armed must survive the late arrival"
        );
        assert!(
            engine.lookup("notepad.exe").is_none(),
            "the pre-clear vault's matches must not be armed by the late arrival"
        );
    }

    /// The same guard in the shape with the worst consequence, and the one
    /// review 18's redesign had to be checked against hardest: the recovery
    /// SUCCEEDS and repopulates for a DIFFERENT account, so at settle time
    /// the cache is populated and non-empty. Since the engine is now built
    /// from the cache at apply time rather than from the sync's own frozen
    /// entries, getting this wrong would not merely lose a refresh -- it
    /// would arm account B's engine on a click that belongs to account A, and
    /// (the mirror of it) an unguarded apply would write A's matches over B.
    /// The epoch is what closes it: `clear` starts a new one, and nothing
    /// afterwards can make the old one current again.
    #[test]
    fn a_late_sync_from_a_previous_account_is_discarded_even_though_the_cache_refilled() {
        let server = sync_server();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        // Account A's sync: fetched, populated, outcome queued.
        let outcome = sync_outcome_from(&cache, cache.epoch(), None);

        // Lock, re-authentication into account B, and a recovery that works:
        // the cache is cleared and repopulated from B's own readiness probe.
        cache.clear();
        let epoch_b = cache.epoch();
        let refilled = cache.populate_with(probe_items(&[("2", "code.exe")]), epoch_b);
        assert_eq!(refilled.unwrap(), PopulateOutcome::Populated);
        let mut engine = MatchEngine::new();
        engine.rebuild(&match_entries(&probe_items(&[("2", "code.exe")])));

        assert!(
            matches!(
                settle_sync_outcome(outcome, &cache),
                SettledSync::NothingToApply
            ),
            "a sync from the previous account must be discarded even when the cache has since \
             been refilled for the new one"
        );
        assert!(
            engine.lookup("code.exe").is_some(),
            "the new account's matches must stay armed"
        );
        assert!(
            engine.lookup("notepad.exe").is_none(),
            "the previous account's matches must not be armed after an account switch"
        );
    }

    /// Review 18's third finding, and the composed case that matters: a sync
    /// in flight, a WRITE landing while it is in flight, the sync's outcome
    /// applying afterwards. The write must survive.
    #[test]
    fn an_app_match_saved_while_a_sync_was_in_flight_survives_that_sync() {
        const ONE_ITEM_NO_MATCH: &str =
            r#"{"success":true,"data":{"data":[{"id":"1","name":"1","type":1,"fields":[]}]}}"#;
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(ONE_ITEM_NO_MATCH)
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        server
            .mock("PUT", "/object/item/1")
            .with_status(200)
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        // The tray Sync worker: its `list_items` returns the vault as it was
        // BEFORE the save below, and its outcome sits in the channel while
        // the picker windows block the main thread.
        let outcome = sync_outcome_from(&cache, cache.epoch(), None);

        // "Add app...": the user saves a match through the cache, and
        // the save path's rebuild from the cache arms the engine from it.
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache
            .set_app_match(
                &item,
                &deskwarden::app_match::AppMatch {
                    process: "notepad.exe".into(),
                    trigger: deskwarden::app_match::TriggerMode::Auto,
                },
            )
            .unwrap();
        let mut engine = MatchEngine::new();
        engine.rebuild(&match_entries(&cache.items()));
        assert!(
            engine.lookup("notepad.exe").is_some(),
            "the save itself must arm the engine"
        );

        // Only now does the main thread drain the sync outcome. Nothing
        // cleared the cache, so it is applied -- and applying it must not
        // reinstate the pre-save vault the worker happened to have fetched.
        match settle_sync_outcome(outcome, &cache) {
            SettledSync::Applicable { items } => engine.rebuild(&match_entries(&items)),
            SettledSync::NothingToApply => panic!(
                "a write is not a supersession: the vault session is the same one, so this sync \
                 is still applicable"
            ),
            SettledSync::Failed(e) => panic!("expected a refreshed sync, got Failed({e})"),
        }
        assert!(
            engine.lookup("notepad.exe").is_some(),
            "the app match the user saved while the sync was in flight was silently dropped by \
             the sync's own, older item list"
        );
    }

    /// REVIEW 21'S CRITICAL, composed, and the ORDERING THE SUITE ABOVE DOES
    /// NOT COVER. `an_app_match_saved_while_a_sync_was_in_flight_survives_that
    /// _sync` runs `sync_outcome_from` to completion *before* the save, so the
    /// save always lands on top of a populate that has already finished -- the
    /// one ordering that worked. Here the save lands INSIDE the sync's fetch
    /// window, which is `spawn_sync`'s `!currently_running` branch exactly:
    /// mark captured before anything, the readiness probe's `list_items`
    /// handed to `sync_outcome_from` as `probe_items`, and `populate_with`
    /// writing that fetch back afterwards. That fetch predates the save, and
    /// before the fix it was assigned to the snapshot wholesale.
    ///
    /// It asserts the survival TWICE, because the two consequences are
    /// different in kind: in what reaches the match engine (autofill dead
    /// until the next sync -- session-scoped) and in the CACHE (a later
    /// vault-window edit PUTs the stale item back, and the item's `fields`
    /// array is always present in that body, so `bw serve`'s
    /// merge-on-omitted-keys behaviour cannot save it -- permanent).
    #[test]
    fn an_app_match_saved_while_a_syncs_fetch_was_in_flight_survives_that_sync() {
        const ONE_ITEM_NO_MATCH: &str =
            r#"{"success":true,"data":{"data":[{"id":"1","name":"1","type":1,"fields":[]}]}}"#;
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(ONE_ITEM_NO_MATCH)
            .create();
        server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        server
            .mock("PUT", "/object/item/1")
            .with_status(200)
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        // Startup, so the cache is populated exactly as it is when the tray
        // Sync item can be clicked at all.
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        // The tray Sync worker, up to the point its fetch has happened and
        // nothing has been written back yet.
        let sync_epoch = cache.epoch();
        let probe = cache.bridge().list_items().unwrap();

        // "Add app...": two picker windows, then a save through the cache --
        // all of it inside the window above, because the handler does not
        // block on `backend_task_in_progress`.
        let item = cache.items().into_iter().find(|i| i.id == "1").unwrap();
        cache
            .set_app_match(
                &item,
                &deskwarden::app_match::AppMatch {
                    process: "notepad.exe".into(),
                    trigger: deskwarden::app_match::TriggerMode::Auto,
                },
            )
            .unwrap();
        let mut engine = MatchEngine::new();
        engine.rebuild(&match_entries(&cache.items()));
        assert!(
            engine.lookup("notepad.exe").is_some(),
            "the save itself must arm the engine"
        );

        // Only now does the worker write its (older) fetch back and report.
        let outcome = sync_outcome_from(&cache, sync_epoch, Some(probe));
        match settle_sync_outcome(outcome, &cache) {
            SettledSync::Applicable { items } => engine.rebuild(&match_entries(&items)),
            SettledSync::NothingToApply => panic!(
                "a write is not a supersession: the vault session is the same one, so this sync \
                 is still applicable"
            ),
            SettledSync::Failed(e) => panic!("expected a refreshed sync, got Failed({e})"),
        }

        assert!(
            engine.lookup("notepad.exe").is_some(),
            "the app match the user saved while the sync was FETCHING was reverted by the \
             sync's populate, so the engine was rebuilt without it"
        );
        assert!(
            cache
                .items()
                .iter()
                .find(|i| i.id == "1")
                .unwrap()
                .fields
                .iter()
                .any(|f| f.name.as_deref() == Some(deskwarden::app_match::APP_MATCH_FIELD_NAME)),
            "the CACHE lost the app match, which is the worse half: the next edit of this item \
             PUTs the stale copy back and the loss stops being session-scoped"
        );
    }

    /// Whether a process with the given id still exists, via `tasklist` --
    /// used only by the `adopt_started_child` regression test above to prove
    /// the discarded child was actually killed rather than merely dropped
    /// (dropping a `Child` does not kill its process, which is exactly the
    /// failure mode this test guards against).
    fn is_pid_running(pid: u32) -> bool {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .expect("tasklist must run");
        String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
    }
}

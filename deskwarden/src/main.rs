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

use deskwarden::app::{
    fill_from_vault, handle_match, match_entries, pump_windows_messages, refresh_match_engine,
};
use deskwarden::backend_policy;
use deskwarden::bw_serve::{
    self, readiness_schedule, wait_for_vault_ready, BW_SERVE_URL, READINESS_DEADLINE,
};
use deskwarden::dispatch;
use deskwarden::injector::{
    Injector, RealSendInput, RealUiAutomation, SendInputFiller, UiAutomationFiller,
};
use deskwarden::match_engine::MatchEngine;
use deskwarden::updater::{self, ReleaseInfo};
use deskwarden::vault_bridge::VaultBridge;
use deskwarden::vault_cache::VaultCache;
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

/// Upper bound on how long a background backend operation may legitimately
/// stay outstanding before something is treated as having gone wrong. Used
/// two ways: `open_vault_window`'s lock-recovery path waits this long for an
/// in-flight operation to report back over `backend_op_rx` before giving up
/// on it and proceeding anyway; `main`'s own loop uses the same bound as a
/// deadline on `backend_task_in_progress` itself (see that variable's doc),
/// clearing it and surfacing a tray failure if nothing has reported back in
/// this long.
///
/// `backend_op_tx` lives in `main` for the lifetime of the process, so the
/// channel itself never disconnects -- an unbounded `recv()` here would block
/// forever if the worker thread panicked (or otherwise returned) before
/// sending, with no Windows message pump running on this thread to keep the
/// tray, hotkey, or window-watching alive. Generous rather than tight: the
/// operation being waited on can legitimately take up to
/// `PORT_RELEASE_GRACE_RESTART` (30s) plus a real `bw sync` round-trip, and
/// this timeout exists to catch "the worker will never answer", not to cut
/// short a slow-but-healthy one.
const BACKEND_OP_TIMEOUT: Duration = Duration::from_secs(90);

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
    let items = match wait_for_vault_ready_with_spinner(&vault, &schedule) {
        Ok(items) => items,
        Err(e) => {
            // A rejected session is indistinguishable from a slow start at
            // this level, so give the user one chance to re-authenticate
            // before giving up rather than exiting on a recoverable problem.
            log::error!("{e}");
            log::warn!("retrying once after a fresh login, in case the session was rejected");
            if let Some(child) = bw_serve_child.as_mut() {
                bw_serve::stop_bw_serve(child);
            }
            session_token = reauthenticate(&store);
            // The longer grace: we just killed our own `bw serve`, and the
            // user just retyped their master password. Give the socket real
            // time to come free rather than aborting on them.
            bw_serve_child = match try_start_backend(
                &session_token,
                job_ref(&job),
                bw_serve::PORT_RELEASE_GRACE_RESTART,
            ) {
                Ok(child) => Some(child),
                Err(e) => {
                    log::error!("{e}");
                    fatal_startup_error(&format!(
                        "Deskwarden could not start its Bitwarden backend after you signed \
                         in.\n\n{e}\n\nFull details are in:\n{}",
                        logging::log_file_path(&config_dir).display()
                    ));
                }
            };

            match wait_for_vault_ready_with_spinner(&vault, &schedule) {
                Ok(items) => items,
                Err(e) => {
                    log::error!("{e}");
                    if let Some(child) = bw_serve_child.as_mut() {
                        bw_serve::stop_bw_serve(child);
                    }
                    fatal_startup_error(&format!(
                        "Deskwarden's Bitwarden backend started but never became usable, so \
                         there is nothing to match your apps against.\n\n{e}\n\nFull details \
                         are in:\n{}",
                        logging::log_file_path(&config_dir).display()
                    ));
                }
            }
        }
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
    if let Err(e) = cache.populate_with(items) {
        log::warn!("could not populate the vault cache at startup: {e:?}");
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
                    &config_dir,
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
                // from, then choose the process to attach to it.
                if let Some(item) = picker_ui::pick_vault_item(&cache) {
                    log::info!("adding an app match to vault item {}", item.id);
                    match picker_ui::run_picker(cache.clone(), item, last_active_pid) {
                        Some(m) => {
                            log::info!("saved app match for {} ({:?})", m.process, m.trigger);
                            // Make the new match live immediately rather than
                            // waiting for the user to trigger a sync.
                            match refresh_match_engine(&vault, &mut engine) {
                                Ok(count) => {
                                    log::info!("match engine refreshed: {count} app match(es)")
                                }
                                Err(e) => {
                                    log::warn!("refresh after saving app match failed: {e:?}")
                                }
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
                    &config_dir,
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
                // Report -- and, on the tray, only claim -- what actually
                // stalled (review Minor 4). An `EnsureRunning` wedge never
                // touched the tray's "Sync" item in the first place (only
                // the click handler above calls `set_sync_in_progress`), so
                // there's nothing on it to revert; unconditionally calling
                // `set_sync_failed` here used to show "Sync failed - click
                // to retry" for a stall that was never a sync at all.
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

/// Same as `wait_for_vault_ready`, but shows a spinner window for the
/// duration instead of blocking with nothing on screen.
///
/// `wait_for_vault_ready` itself never touches a window -- it's a plain
/// network retry loop -- so it runs on a scoped background thread while the
/// main thread shows `loading_ui::show_while`'s spinner. Scoped rather than
/// a bare `std::thread::spawn`: the worker only needs `&vault`/`&schedule`
/// for the length of this call, not `'static` ownership of them.
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
///
/// Takes `cache`, not a separate `vault: &VaultBridge` -- `cache.bridge()` is
/// that same bridge, so a second parameter would just be another name for it.
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
    config_dir: &std::path::Path,
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
    if backend_task_in_progress.is_none() && !backend_already_running {
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
                Err(_) => log::warn!(
                    "in-flight backend operation did not report back within \
                     {BACKEND_OP_TIMEOUT:?}; proceeding with lock recovery anyway. If it later \
                     reports back late (see `apply_backend_op`'s child-adoption guard), its \
                     child is stopped rather than allowed to overwrite the one this recovery is \
                     about to start."
                ),
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
        *bw_serve_child = match try_start_backend(
            session_token,
            job_ref(job),
            bw_serve::PORT_RELEASE_GRACE_RESTART,
        ) {
            Ok(child) => Some(child),
            Err(e) => {
                log::error!("{e}");
                fatal_startup_error(&format!(
                    "Deskwarden could not restart its Bitwarden backend after the vault \
                     window locked.\n\n{e}\n\nFull details are in:\n{}",
                    logging::log_file_path(config_dir).display()
                ));
            }
        };
        match wait_for_vault_ready_with_spinner(cache.bridge(), schedule) {
            Ok(_items) => {
                if let Err(e) = cache.populate() {
                    log::warn!("could not repopulate the vault cache after unlock: {e:?}");
                }
                match refresh_match_engine(cache.bridge(), engine) {
                    Ok(count) => {
                        log::info!("match engine refreshed after unlock: {count} app match(es)")
                    }
                    Err(e) => log::warn!("match engine refresh after unlock failed: {e:?}"),
                }
            }
            Err(e) => {
                log::error!("{e}");
                if let Some(child) = bw_serve_child.as_mut() {
                    bw_serve::stop_bw_serve(child);
                }
                fatal_startup_error(&format!(
                    "Deskwarden's Bitwarden backend did not come back up after the vault \
                     window locked.\n\n{e}\n\nFull details are in:\n{}",
                    logging::log_file_path(config_dir).display()
                ));
            }
        }
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
    /// `bw sync` and repopulate the cache. `outcome` is `Err` if starting,
    /// syncing, or repopulating failed.
    Sync {
        child: Option<Result<Child, BackendStartError>>,
        outcome: Result<(), String>,
    },
}

/// Applies a completed [`BackendOp`]: updates `bw_serve_child` and, for a
/// `Sync`, rebuilds the match engine from the freshly repopulated cache and
/// reflects the outcome on the tray.
fn apply_backend_op(
    op: BackendOp,
    bw_serve_child: &mut Option<Child>,
    cache: &Arc<VaultCache>,
    engine: &mut MatchEngine,
    tray: &tray::AppTray,
) {
    match op {
        BackendOp::EnsureRunning(Ok(child)) => {
            if adopt_started_child(bw_serve_child, child) {
                log::info!("bw serve started for the vault window");
            }
        }
        BackendOp::EnsureRunning(Err(e)) => log::error!(
            "could not start bw serve for the vault window (writes and TOTP will fail until \
             the next attempt; reads still work from the cache): {e}"
        ),
        BackendOp::Sync { child, outcome } => {
            match child {
                Some(Ok(c)) => {
                    adopt_started_child(bw_serve_child, c);
                }
                Some(Err(e)) => log::error!("sync could not start bw serve: {e}"),
                None => {}
            }
            match outcome {
                Ok(()) => {
                    let entries = match_entries(&cache.items());
                    log::info!(
                        "sync complete; match engine refreshed: {} app match(es)",
                        entries.len()
                    );
                    engine.rebuild(&entries);
                    tray::set_sync_idle(tray);
                }
                Err(e) => {
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
        let child = if currently_running {
            None
        } else {
            Some(try_start_backend(
                &session_token,
                job_ref(&job),
                bw_serve::PORT_RELEASE_GRACE_RESTART,
            ))
        };

        let start_failed = matches!(&child, Some(Err(_)));
        let outcome = if start_failed {
            Err("bw serve could not be started".to_string())
        } else if currently_running {
            bw_serve::run_bw_sync(&session_token)
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
            wait_for_vault_ready(cache.bridge(), &schedule).map(|_items| ())
        }
        .and_then(|()| cache.populate().map_err(|e| format!("{e:?}")));

        let _ = tx.send(BackendOp::Sync { child, outcome });
    });
}

fn wait_for_vault_ready_with_spinner(
    vault: &VaultBridge,
    schedule: &[Duration],
) -> Result<Vec<deskwarden::vault_bridge::VaultItem>, String> {
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        scope.spawn(move || {
            let _ = tx.send(wait_for_vault_ready(vault, schedule));
        });
        loading_ui::show_while("Setting up your vault...", rx)
    })
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

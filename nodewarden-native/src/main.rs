//! Binary entry point.
//!
//! Declares no modules of its own: every module lives in `lib.rs` (see the
//! note there). This file is only `fn main()` and the startup sequence.

use nodewarden_native::app::{
    fill_from_vault, handle_match, match_entries, pump_windows_messages, refresh_match_engine,
};
use nodewarden_native::bw_serve::{
    self, readiness_schedule, wait_for_vault_ready, BW_SERVE_URL, READINESS_DEADLINE,
};
use nodewarden_native::dispatch;
use nodewarden_native::injector::{Injector, RealSendInput, RealUiAutomation};
use nodewarden_native::match_engine::MatchEngine;
use nodewarden_native::vault_bridge::VaultBridge;
use nodewarden_native::{
    hotkey, job_object, logging, login_ui, picker_ui, session_store, tray, window_watch,
};
use std::process::Child;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// How often the match engine is rebuilt from the vault while running, so
/// matches added via the picker (or synced from another device) take effect
/// without restarting the app.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

fn main() {
    let config_dir = directories::ProjectDirs::from("dev", "nodewarden", "nodewarden-native")
        .expect("could not resolve config directory")
        .config_dir()
        .to_path_buf();
    std::fs::create_dir_all(&config_dir).expect("failed to create config directory");

    // Logging first: a background tray app has no console, so without a log
    // file every failure below is invisible to whoever has to diagnose it.
    match logging::init(&config_dir) {
        Ok(path) => log::info!("nodewarden-native starting; logging to {}", path.display()),
        Err(e) => eprintln!("warning: {e}"),
    }

    let session_path = config_dir.join("session.bin");
    let store = session_store::SessionStore::new(session_path);

    // Every child process we spawn joins this job object, which is configured
    // to kill its members when the last handle closes. Our handles close when
    // this process dies for *any* reason -- clean quit, panic, Ctrl+C, Task
    // Manager -- so `bw serve` can no longer be orphaned holding an unlocked
    // vault open on localhost. This must outlive the whole run, hence the
    // binding here rather than inside the spawn helper.
    let job = match job_object::KillOnCloseJob::new() {
        Ok(job) => Some(job),
        Err(e) => {
            log::error!(
                "could not create a kill-on-close job object ({e}); `bw serve` will only be \
                 cleaned up on a clean quit"
            );
            None
        }
    };

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
    let mut engine = MatchEngine::new();

    let mut bw_serve_child = start_backend(&session_token, job.as_ref());

    // `bw serve` is a bundled Node binary: its cold start regularly takes
    // several seconds, far longer than the fixed 500ms sleep this replaces.
    // Losing that race used to leave the match engine permanently empty with
    // no diagnostic, so the app silently did nothing forever.
    let schedule = readiness_schedule(READINESS_DEADLINE);
    let items = match wait_for_vault_ready(&vault, &schedule) {
        Ok(items) => items,
        Err(e) => {
            // A rejected session is indistinguishable from a slow start at
            // this level, so give the user one chance to re-authenticate
            // before giving up rather than exiting on a recoverable problem.
            log::error!("{e}");
            log::warn!("retrying once after a fresh login, in case the session was rejected");
            bw_serve::stop_bw_serve(&mut bw_serve_child);
            session_token = reauthenticate(&store);
            // The longer grace: we just killed our own `bw serve`, and the
            // user just retyped their master password. Give the socket real
            // time to come free rather than aborting on them.
            bw_serve_child = match try_start_backend(
                &session_token,
                job.as_ref(),
                bw_serve::PORT_RELEASE_GRACE_RESTART,
            ) {
                Ok(child) => child,
                Err(e) => {
                    log::error!("{e}");
                    log::error!(
                        "giving up: `bw serve` could not be restarted after re-authenticating. \
                         See {}",
                        logging::log_file_path(&config_dir).display()
                    );
                    std::process::exit(1);
                }
            };

            match wait_for_vault_ready(&vault, &schedule) {
                Ok(items) => items,
                Err(e) => {
                    log::error!("{e}");
                    log::error!(
                        "giving up: without a reachable, unlocked `bw serve` there is nothing \
                         to match against. See {}",
                        logging::log_file_path(&config_dir).display()
                    );
                    bw_serve::stop_bw_serve(&mut bw_serve_child);
                    std::process::exit(1);
                }
            }
        }
    };

    let entries = match_entries(&items);
    log::info!("match engine loaded with {} app match(es)", entries.len());
    engine.rebuild(&entries);

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

    // Seed with whatever is already focused: `SetWinEventHook` only reports
    // foreground *changes*, so an app that was matched and already in front
    // when nodewarden started would otherwise be ignored until the next window
    // switch.
    if let Some(event) = window_watch::current_foreground_event() {
        log::info!(
            "seeding with current foreground window: {} (hwnd {})",
            event.exe_name,
            event.hwnd
        );
        process_foreground_event(
            &event,
            &vault,
            &injector,
            &engine,
            &mut pending_hotkey_fill,
            &mut last_dispatched_hwnd,
        );
    }

    let mut last_refresh = Instant::now();

    // How many periodic refreshes have failed in a row. A backend that still
    // holds its port but keeps failing is assumed busy at first and wedged
    // after a few tries; see `bw_serve::recovery_action`.
    let mut consecutive_refresh_failures: u32 = 0;

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
                log::info!("quit requested from tray; killing bw serve");
                bw_serve::stop_bw_serve(&mut bw_serve_child);
                std::process::exit(0);
            }

            if event.id == tray.add_app_id {
                // Two-step flow: choose the vault item the credentials come
                // from, then choose the process to attach to it.
                if let Some(item) = picker_ui::pick_vault_item(&vault) {
                    log::info!("adding an app match to vault item {}", item.id);
                    match picker_ui::run_picker(vault.clone(), item) {
                        Some(m) => {
                            log::info!("saved app match for {} ({:?})", m.process, m.trigger);
                            // Make the new match live immediately rather than
                            // waiting for the next periodic refresh.
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
                    fill_from_vault(&vault, &injector, &item_id, hwnd);
                } else {
                    log::info!("fill hotkey ignored: foreground window is no longer the match");
                }
            }
        }

        if last_refresh.elapsed() >= REFRESH_INTERVAL {
            match refresh_match_engine(&vault, &mut engine) {
                Ok(count) => {
                    consecutive_refresh_failures = 0;
                    log::debug!("match engine refreshed: {count} app match(es)");
                }
                Err(e) => {
                    // A failing refresh is the signal that something broke
                    // *while running*. Two independent things can cause it and
                    // they need different fixes, so both are probed:
                    //
                    // * the session went stale (`bw lock`, a server-side vault
                    //   timeout, a password change elsewhere) -- `bw status`
                    //   sees this, because it shells out to the CLI and does
                    //   not go through `bw serve` at all;
                    // * `bw serve` itself died or wedged while the session
                    //   stayed valid -- `bw status` is *blind* to this and
                    //   cheerfully reports `Unlocked`, so the port is probed
                    //   separately. Relying on the status check alone (as this
                    //   used to) meant a crashed backend logged this warning
                    //   every 60s forever and never restarted.
                    consecutive_refresh_failures += 1;
                    log::warn!(
                        "periodic match engine refresh failed \
                         (consecutive failures: {consecutive_refresh_failures}): {e:?}"
                    );

                    let status = login_ui::check_bw_status_with_session(Some(&session_token));
                    let port_listening = bw_serve::port_in_use(bw_serve::BW_SERVE_PORT);
                    let action = bw_serve::recovery_action(
                        status == login_ui::BwStatus::Unlocked,
                        port_listening,
                        consecutive_refresh_failures,
                    );

                    match action {
                        bw_serve::RecoveryAction::Wait => log::info!(
                            "session is {status:?} and `bw serve` is still listening on port \
                             {}; treating this as a transient failure and retrying next cycle",
                            bw_serve::BW_SERVE_PORT
                        ),
                        _ => {
                            if action == bw_serve::RecoveryAction::Reauthenticate {
                                log::warn!("session is now {status:?}; re-authenticating");
                                bw_serve::stop_bw_serve(&mut bw_serve_child);
                                session_token = reauthenticate(&store);
                                // The login window we just showed stole and
                                // released foreground, exactly like the tray
                                // picker above: forget the last-dispatched
                                // hwnd so the window the user returns to is
                                // treated as a fresh switch instead of being
                                // suppressed as a repeat.
                                last_dispatched_hwnd = None;
                            } else {
                                log::warn!(
                                    "session is still unlocked but `bw serve` is unusable \
                                     (port {} listening: {port_listening}); restarting the \
                                     backend",
                                    bw_serve::BW_SERVE_PORT
                                );
                                bw_serve::stop_bw_serve(&mut bw_serve_child);
                            }

                            // Deliberately *not* fatal, and deliberately more
                            // patient than the startup path. Exiting here --
                            // possibly seconds after the user retyped their
                            // master password -- over a socket that a `bw`
                            // grandchild has not released yet would punish
                            // them for a timing problem that usually resolves
                            // itself. Log it and try again next cycle.
                            match try_start_backend(
                                &session_token,
                                job.as_ref(),
                                bw_serve::PORT_RELEASE_GRACE_RESTART,
                            ) {
                                Ok(child) => {
                                    bw_serve_child = child;
                                    match wait_for_vault_ready(&vault, &schedule) {
                                        Ok(items) => {
                                            let entries = match_entries(&items);
                                            log::info!(
                                                "backend restarted; match engine reloaded with \
                                                 {} app match(es)",
                                                entries.len()
                                            );
                                            engine.rebuild(&entries);
                                            consecutive_refresh_failures = 0;
                                        }
                                        Err(e) => {
                                            log::error!("backend still unusable after restart: {e}")
                                        }
                                    }
                                }
                                Err(e) => log::error!(
                                    "could not restart `bw serve`: {e} -- staying up and \
                                     retrying on the next refresh cycle rather than exiting"
                                ),
                            }
                        }
                    }
                }
            }

            // Re-stamped *after* the work, not just before it. Recovery can
            // block for a long time -- a 30s port wait, a 30s readiness wait,
            // or a login window the user leaves open -- and measuring the next
            // interval from before all that would make the following cycle
            // fire immediately, turning a persistent failure into a tight
            // retry loop instead of one attempt per minute.
            last_refresh = Instant::now();
        }

        if let Ok(event) = rx.recv_timeout(Duration::from_millis(200)) {
            process_foreground_event(
                &event,
                &vault,
                &injector,
                &engine,
                &mut pending_hotkey_fill,
                &mut last_dispatched_hwnd,
            );
        }
    }
}

/// Applies the dispatch rules to one foreground event and, if it survives
/// them, matches and dispatches it.
fn process_foreground_event(
    event: &window_watch::ForegroundEvent,
    vault: &VaultBridge,
    injector: &Injector<RealUiAutomation, RealSendInput>,
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
        if let Some(armed) = handle_match(vault, injector, item_id, m, event.hwnd, &event.exe_name)
        {
            *pending_hotkey_fill = Some(armed);
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
            Self::Spawn(e) => write!(
                f,
                "failed to spawn `bw serve` (is the Bitwarden CLI installed and on PATH?): {e}"
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
    job_object::spawn_in_job(job, bw_serve::bw_serve_command(session_token))
        .map_err(BackendStartError::Spawn)
}

/// Startup variant of [`try_start_backend`]: there is nothing to fall back to
/// before the main loop exists, so a failure here is fatal.
fn start_backend(session_token: &str, job: Option<&job_object::KillOnCloseJob>) -> Child {
    match try_start_backend(session_token, job, bw_serve::PORT_RELEASE_GRACE) {
        Ok(child) => child,
        Err(e) => {
            log::error!("{e}");
            std::process::exit(1);
        }
    }
}

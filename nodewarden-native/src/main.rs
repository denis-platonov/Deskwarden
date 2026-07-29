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
use nodewarden_native::{hotkey, logging, login_ui, session_store, tray, window_watch};
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

    let session_token = match store.load() {
        Some(token) => {
            log::info!("loaded cached session token");
            token
        }
        None => {
            log::info!("no cached session token; showing login flow");
            let token = login_ui::run_login_flow();
            if let Err(e) = store.save(&token) {
                log::error!("failed to persist session token: {e}");
            }
            token
        }
    };

    // Pull the latest vault state down before building the match engine, so a
    // match added on another device is live on first run rather than after the
    // next incidental sync.
    match bw_serve::run_bw_sync(&session_token) {
        Ok(()) => log::info!("bw sync completed"),
        Err(e) => log::warn!("bw sync failed (continuing with cached vault): {e}"),
    }

    let mut bw_serve_child = spawn_bw_serve(&session_token);

    let vault = VaultBridge::new(BW_SERVE_URL);
    let mut engine = MatchEngine::new();

    // `bw serve` is a bundled Node binary: its cold start regularly takes
    // several seconds, far longer than the fixed 500ms sleep this replaces.
    // Losing that race used to leave the match engine permanently empty with
    // no diagnostic, so the app silently did nothing forever.
    match wait_for_vault_ready(&vault, &readiness_schedule(READINESS_DEADLINE)) {
        Ok(items) => {
            let entries = match_entries(&items);
            log::info!("match engine loaded with {} app match(es)", entries.len());
            engine.rebuild(&entries);
        }
        Err(e) => {
            log::error!("{e}");
            log::error!(
                "giving up: without a reachable, unlocked `bw serve` there is nothing to match \
                 against. Check {}\\nodewarden.log and that the Bitwarden CLI is installed.",
                config_dir.display()
            );
            let _ = bw_serve_child.kill();
            std::process::exit(1);
        }
    }

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
                if let Err(e) = bw_serve_child.kill() {
                    log::warn!("bw serve kill on quit failed (already gone?): {e}");
                }
                std::process::exit(0);
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
            last_refresh = Instant::now();
            match refresh_match_engine(&vault, &mut engine) {
                Ok(count) => log::debug!("match engine refreshed: {count} app match(es)"),
                Err(e) => log::warn!("periodic match engine refresh failed: {e:?}"),
            }
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

fn spawn_bw_serve(session_token: &str) -> Child {
    match bw_serve::spawn_bw_serve(session_token) {
        Ok(child) => child,
        Err(e) => {
            log::error!(
                "failed to spawn `bw serve` (is the Bitwarden CLI installed and on PATH?): {e}"
            );
            std::process::exit(1);
        }
    }
}

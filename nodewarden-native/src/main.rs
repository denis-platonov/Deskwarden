//! Binary entry point.
//!
//! Declares no modules of its own: every module lives in `lib.rs` (see the
//! note there). This file is only `fn main()` and the startup sequence.

use nodewarden_native::app::{
    handle_match, pump_windows_messages, refresh_match_engine,
};
use nodewarden_native::injector::{Injector, RealSendInput, RealUiAutomation};
use nodewarden_native::match_engine::MatchEngine;
use nodewarden_native::vault_bridge::VaultBridge;
use nodewarden_native::{hotkey, login_ui, session_store, tray, window_watch};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

const BW_SERVE_URL: &str = "http://localhost:8087";

fn main() {
    let config_dir = directories::ProjectDirs::from("dev", "nodewarden", "nodewarden-native")
        .expect("could not resolve config directory")
        .config_dir()
        .to_path_buf();
    std::fs::create_dir_all(&config_dir).expect("failed to create config directory");

    let session_path = config_dir.join("session.bin");
    let store = session_store::SessionStore::new(session_path);

    let session_token = match store.load() {
        Some(token) => token,
        None => {
            let token = login_ui::run_login_flow();
            store.save(&token).expect("failed to persist session token");
            token
        }
    };

    let mut bw_serve = spawn_bw_serve(&session_token);
    std::thread::sleep(Duration::from_millis(500));

    let vault = VaultBridge::new(BW_SERVE_URL);
    let mut engine = MatchEngine::new();
    let _ = refresh_match_engine(&vault, &mut engine);

    let injector = Injector { ui: RealUiAutomation, fallback: RealSendInput };

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
        let _ = window_watch::watch_foreground_windows(move |event| {
            let _ = tx.send(event);
        });
    });

    // Set when a `Hotkey`-trigger match is seen: the item/window that's
    // eligible to be filled once the user presses the fill hotkey, rather
    // than being filled immediately from the window-match path.
    let mut pending_hotkey_fill: Option<(String, isize)> = None;

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
                let _ = bw_serve.kill();
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
                    nodewarden_native::app::fill_from_vault(&vault, &injector, &item_id, hwnd);
                }
            }
        }

        if let Ok(event) = rx.recv_timeout(Duration::from_millis(200)) {
            // Any foreground-window change invalidates a pending hotkey
            // fill unless it's the very window that armed it re-foregrounding
            // (same hwnd). Without this, arming the fill and then switching
            // away to an unrelated window -- without ever pressing the fill
            // hotkey -- would leave `pending_hotkey_fill` stale: a later,
            // unrelated Ctrl+Alt+B press would fire it against a `hwnd` that
            // may since have been recycled by the OS for a different window,
            // contradicting the guarantee that the hotkey does nothing when
            // no matching window is foregrounded.
            if let Some((_, armed_hwnd)) = pending_hotkey_fill {
                if armed_hwnd != event.hwnd {
                    pending_hotkey_fill = None;
                }
            }

            if let Some((item_id, m)) = engine.lookup(&event.exe_name) {
                if let Some(armed) =
                    handle_match(&vault, &injector, item_id, m, event.hwnd, &event.exe_name)
                {
                    pending_hotkey_fill = Some(armed);
                }
            }
        }
    }
}

fn spawn_bw_serve(session_token: &str) -> Child {
    Command::new("bw")
        .args(["serve", "--port", "8087"])
        .env("BW_SESSION", session_token)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn `bw serve` (is the Bitwarden CLI installed and on PATH?)")
}

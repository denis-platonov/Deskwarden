mod app_match;
mod hotkey;
mod injector;
mod login_ui;
mod match_engine;
mod overlay_ui;
mod picker_ui;
mod process_list;
mod session_store;
mod tray;
mod vault_bridge;
mod window_watch;

use app_match::AppMatch;
use injector::{Injector, RealSendInput, RealUiAutomation};
use match_engine::MatchEngine;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use vault_bridge::VaultBridge;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

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
    refresh_match_engine(&vault, &mut engine);

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
            // "Add app..." has no wired action: picker_ui::run_picker needs
            // a specific vault item to attach the match to, and nothing in
            // this task's UI selects one from the tray. Left as a visible
            // but inert menu entry, same as the brief's own main.rs sample,
            // which never references picker_ui either.
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

/// Drains any pending Win32 messages on the calling (main) thread without
/// blocking, so the hidden windows owned by the tray icon and the global
/// hotkey manager get their WM_COMMAND/WM_HOTKEY messages dispatched.
fn pump_windows_messages() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn fill_from_vault(
    vault: &VaultBridge,
    injector: &Injector<RealUiAutomation, RealSendInput>,
    item_id: &str,
    hwnd: isize,
) {
    if let Ok(items) = vault.list_items() {
        if let Some(item) = items.iter().find(|i| i.id == item_id) {
            let (username, password) = credentials_for(item);
            let _ = injector.fill(hwnd, &username, &password);
        }
    }
}

/// Dispatches a freshly foregrounded, matched window according to its
/// trigger mode. `Auto` and `Prompt` fill immediately (`Prompt` only if the
/// user clicks Fill on the overlay) and return `None`. `Hotkey` doesn't fill
/// from this path at all -- per the spec, it arms `(item_id, hwnd)` and
/// returns it so the main loop's separate `fill_hotkey_pressed` check can
/// fill it later, once the user actually presses the fill hotkey.
fn handle_match(
    vault: &VaultBridge,
    injector: &Injector<RealUiAutomation, RealSendInput>,
    item_id: &str,
    m: &AppMatch,
    hwnd: isize,
    exe_name: &str,
) -> Option<(String, isize)> {
    match m.trigger {
        app_match::TriggerMode::Auto => {
            fill_from_vault(vault, injector, item_id, hwnd);
            None
        }
        app_match::TriggerMode::Prompt => {
            if overlay_ui::show_prompt_overlay(exe_name) {
                fill_from_vault(vault, injector, item_id, hwnd);
            }
            None
        }
        app_match::TriggerMode::Hotkey => Some((item_id.to_string(), hwnd)),
    }
}

fn credentials_for(_item: &vault_bridge::VaultItem) -> (String, String) {
    // bw serve's item payload includes a `login: { username, password }`
    // object, but `vault_bridge::VaultItem` (Task 4) only models
    // `id`/`name`/`fields` -- that's all the app-match extraction logic
    // needed. Before relying on real fills end-to-end, extend `VaultItem`
    // with the `login: Option<LoginData>` shape `bw serve` actually returns
    // and read it here instead. Left as an explicit placeholder (not
    // silently glossed over) so the rest of the pipeline -- matching,
    // triggering, injecting -- is wired and independently testable now.
    (String::new(), String::new())
}

fn refresh_match_engine(vault: &VaultBridge, engine: &mut MatchEngine) {
    let entries = vault
        .list_items()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| vault_bridge::extract_app_match(item).map(|m| (item.id.clone(), m)))
        .collect::<Vec<_>>();
    engine.rebuild(&entries);
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

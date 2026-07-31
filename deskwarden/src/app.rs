//! Application-level glue: the pieces `main` orchestrates, kept in the library
//! so they're reachable from examples and integration tests rather than being
//! locked inside the binary target.

use crate::app_match::{AppMatch, TriggerMode};
use crate::injector::ui_automation;
use crate::injector::{Injector, SendInputFiller, UiAutomationFiller};
use crate::match_engine::MatchEngine;
use crate::overlay_ui;
use crate::vault_bridge::{extract_app_match, VaultBridge, VaultError, VaultItem};
use crate::vault_cache::VaultCache;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetWindowRect, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

/// The overlay's fixed size (must match `overlay_ui::show_prompt_overlay`'s
/// `with_inner_size`) -- needed here to clamp its position on-screen before
/// the window exists to measure.
const OVERLAY_WIDTH: f32 = 396.0;
const OVERLAY_HEIGHT: f32 = 164.0;
/// Gap between the field/window edge and the overlay, so it doesn't sit
/// flush against the thing it's about to fill.
const OVERLAY_GAP: f32 = 10.0;

/// Where to place the autofill overlay so it reads as "next to the field"
/// rather than wherever the OS happens to put a new window: just below the
/// focused/matched field if UI Automation can find one, else just outside
/// the matched window's own top-right corner. Clamped to the nearest
/// monitor's work area so it can't land off-screen or under the taskbar.
fn overlay_position(hwnd: isize) -> Option<(f32, f32)> {
    let (x, y) = match ui_automation::field_anchor_rect(hwnd) {
        Ok(Some(rect)) => (rect.left as f32, rect.bottom as f32 + OVERLAY_GAP),
        _ => {
            let window = window_rect(hwnd)?;
            (
                window.right as f32 - OVERLAY_WIDTH,
                window.top as f32 + OVERLAY_GAP,
            )
        }
    };
    Some(clamp_to_monitor(hwnd, x, y))
}

fn window_rect(hwnd: isize) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(HWND(hwnd as *mut core::ffi::c_void), &mut rect).ok()? };
    Some(rect)
}

fn clamp_to_monitor(hwnd: isize, x: f32, y: f32) -> (f32, f32) {
    unsafe {
        let monitor =
            MonitorFromWindow(HWND(hwnd as *mut core::ffi::c_void), MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let work = info.rcWork;
            let clamped_x = x
                .min(work.right as f32 - OVERLAY_WIDTH)
                .max(work.left as f32);
            let clamped_y = y
                .min(work.bottom as f32 - OVERLAY_HEIGHT)
                .max(work.top as f32);
            return (clamped_x, clamped_y);
        }
    }
    (x, y)
}

/// Drains any pending Win32 messages on the calling (main) thread without
/// blocking, so the hidden windows owned by the tray icon and the global
/// hotkey manager get their WM_COMMAND/WM_HOTKEY messages dispatched.
///
/// The `hwnd` argument to `PeekMessageW` is deliberately `None` (all windows
/// on the thread) rather than a specific window: `tray-icon` and
/// `global-hotkey` each create their *own* hidden message-only window
/// internally and never expose the handle, so there is no hwnd we could
/// narrow to that would still service both. Narrowing here would silently
/// re-break the exact thing this pump was added to fix -- tray clicks and
/// hotkey presses sitting undelivered in the queue forever. The cost of the
/// broad scope is that any other window owned by this thread also gets its
/// messages dispatched here, which is harmless: we own the thread and create
/// no other long-lived windows on it (the egui windows run their own nested
/// loops and block this one while they're up).
pub fn pump_windows_messages() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Extracts the credentials to type for a vault item, from the `login` object
/// `bw serve` returns. Items without a login object (secure notes, cards)
/// yield empty strings.
pub fn credentials_for(item: &VaultItem) -> (String, String) {
    match &item.login {
        Some(login) => (
            login.username.clone().unwrap_or_default(),
            login.password.clone().unwrap_or_default(),
        ),
        None => (String::new(), String::new()),
    }
}

/// Fetches the item's credentials and injects them into `hwnd`.
///
/// Reads from `VaultCache`'s in-memory snapshot rather than `bw serve`
/// directly -- this is the path that makes autofill work with the backend
/// fully stopped (see `backend_policy` and `vault_cache`'s module docs): a
/// keystroke-triggered fill used to be an HTTP round-trip to a process this
/// app might no longer even be running. An empty cache while the vault is
/// genuinely unlocked should not happen (`main` populates it once per
/// unlock), so a miss falls back to the bridge -- serving the fill rather
/// than failing it outright -- and logs a warning, since a miss here is a
/// bug signal worth noticing rather than silently swallowing.
pub fn fill_from_vault<A: UiAutomationFiller, B: SendInputFiller>(
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
    hwnd: isize,
) {
    let item = cache
        .items()
        .into_iter()
        .find(|i| i.id == item_id)
        .map(Ok)
        .unwrap_or_else(|| {
            log::warn!("cache miss for item {item_id} during a fill; falling back to bw serve");
            cache.bridge().get_item(item_id)
        });
    match item {
        Ok(item) => {
            let (username, password) = credentials_for(&item);
            if username.is_empty() && password.is_empty() {
                log::warn!("vault item {item_id} has no login credentials; nothing to fill");
                return;
            }
            match injector.fill(hwnd, &username, &password) {
                Ok(()) => fill_stats.record_fill(item_id),
                Err(e) => log::error!("fill failed for item {item_id} into hwnd {hwnd}: {e}"),
            }
        }
        Err(e) => log::error!("could not read vault item {item_id} to fill it: {e:?}"),
    }
}

/// Dispatches a freshly foregrounded, matched window according to its
/// trigger mode. `Auto` and `Prompt` fill immediately (`Prompt` only if the
/// user clicks Fill on the overlay) and return `None`. `Hotkey` doesn't fill
/// from this path at all -- per the spec, it arms `(item_id, hwnd)` and
/// returns it so the main loop's separate `fill_hotkey_pressed` check can
/// fill it later, once the user actually presses the fill hotkey.
pub fn handle_match<A: UiAutomationFiller, B: SendInputFiller>(
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
    m: &AppMatch,
    hwnd: isize,
    exe_name: &str,
) -> Option<(String, isize)> {
    match m.trigger {
        TriggerMode::Auto => {
            fill_from_vault(cache, injector, fill_stats, item_id, hwnd);
            None
        }
        TriggerMode::Prompt => {
            // Read the item back first so the overlay can say *which*
            // credentials it is offering (design 2a shows the username and
            // item name, never a bare "fill something?"). A failed read is
            // not fatal to the prompt -- the overlay just can't name the
            // credentials -- and the fill path re-fetches on its own anyway.
            //
            // This one deliberately stays on `cache.bridge()` rather than
            // `cache.items()`: it's cosmetic (the overlay's label), not the
            // fill itself, so it isn't worth the same cache-miss fallback
            // dance `fill_from_vault` does -- a failed read here just means a
            // slightly less informative overlay, handled by the existing
            // `.ok()`.
            //
            // The username is read straight off the login object rather than
            // through `credentials_for`: that helper also clones the
            // plaintext password into a `String` this path has no use for,
            // and which would then be dropped without being zeroized. The
            // overlay never shows a password, so it should never hold one.
            let matched = cache.bridge().get_item(item_id).ok().map(|item| {
                let username = item.login.as_ref().and_then(|l| l.username.clone());
                overlay_ui::OverlayMatch {
                    item_name: item.name.clone(),
                    username: username.filter(|u| !u.is_empty()),
                }
            });
            if overlay_ui::show_prompt_overlay(exe_name, matched.as_ref(), overlay_position(hwnd))
            {
                fill_from_vault(cache, injector, fill_stats, item_id, hwnd);
            }
            None
        }
        TriggerMode::Hotkey => Some((item_id.to_string(), hwnd)),
    }
}

/// Finds a currently-open window whose exe name matches `process` -- for
/// "Fill in app" (the vault window's detail pane), which has no
/// window-watch context of its own and needs to resolve a target hwnd from
/// just an item's `deskwarden:app-match` process name.
pub fn find_window_for_process<'a>(
    windows: &'a [crate::window_list::WindowInfo],
    process: &str,
) -> Option<&'a crate::window_list::WindowInfo> {
    windows.iter().find(|w| w.exe_name.eq_ignore_ascii_case(process))
}

/// Pure helper: turns a list of vault items into the `(item_id, AppMatch)`
/// entries the match engine is rebuilt from, dropping items with no
/// `deskwarden:app-match` field.
pub fn match_entries(items: &[VaultItem]) -> Vec<(String, AppMatch)> {
    items
        .iter()
        .filter_map(|item| extract_app_match(item).map(|m| (item.id.clone(), m)))
        .collect()
}

/// Re-reads the vault and rebuilds `engine` from it.
///
/// Returns the number of matches loaded, or the underlying vault error --
/// notably *not* swallowing failure into an empty engine, which is how the app
/// used to end up silently matching nothing forever.
pub fn refresh_match_engine(
    vault: &VaultBridge,
    engine: &mut MatchEngine,
) -> Result<usize, VaultError> {
    let items = vault.list_items()?;
    let entries = match_entries(&items);
    engine.rebuild(&entries);
    Ok(entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_match::APP_MATCH_FIELD_NAME;
    use crate::vault_bridge::VaultField;

    fn item(id: &str, match_json: Option<&str>) -> VaultItem {
        VaultItem {
            id: id.into(),
            name: format!("item {id}"),
            fields: match_json
                .map(|v| {
                    vec![VaultField {
                        name: Some(APP_MATCH_FIELD_NAME.into()),
                        value: Some(v.into()),
                    }]
                })
                .unwrap_or_default(),
            login: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    #[test]
    fn match_entries_keeps_only_items_with_an_app_match() {
        let items = vec![
            item("1", Some(r#"{"process":"a.exe","trigger":"auto"}"#)),
            item("2", None),
            item("3", Some("not json")),
        ];
        let entries = match_entries(&items);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "1");
        assert_eq!(entries[0].1.process, "a.exe");
    }

    #[test]
    fn match_entries_is_empty_for_an_empty_vault() {
        assert!(match_entries(&[]).is_empty());
    }

    #[test]
    fn credentials_come_from_the_login_object() {
        let item: VaultItem = serde_json::from_str(
            r#"{"id":"1","name":"A","fields":[],"login":{"username":"u","password":"p"}}"#,
        )
        .unwrap();
        assert_eq!(credentials_for(&item), ("u".to_string(), "p".to_string()));
    }

    #[test]
    fn credentials_are_empty_for_items_without_a_login_object() {
        assert_eq!(
            credentials_for(&item("1", None)),
            (String::new(), String::new())
        );
    }

    #[test]
    fn credentials_tolerate_a_partial_login_object() {
        let item: VaultItem =
            serde_json::from_str(r#"{"id":"1","name":"A","fields":[],"login":{"username":"u"}}"#)
                .unwrap();
        assert_eq!(credentials_for(&item), ("u".to_string(), String::new()));
    }

    #[test]
    fn find_window_for_process_matches_case_insensitively() {
        let windows = vec![
            crate::window_list::WindowInfo {
                hwnd: 1,
                pid: 100,
                exe_path: r"C:\Games\EpicGamesLauncher.exe".into(),
                exe_name: "EpicGamesLauncher.exe".into(),
                title: "Epic Games Launcher".into(),
            },
            crate::window_list::WindowInfo {
                hwnd: 2,
                pid: 200,
                exe_path: r"C:\Windows\notepad.exe".into(),
                exe_name: "notepad.exe".into(),
                title: "Untitled - Notepad".into(),
            },
        ];
        let found = find_window_for_process(&windows, "epicgameslauncher.exe").unwrap();
        assert_eq!(found.hwnd, 1);
        assert!(find_window_for_process(&windows, "steam.exe").is_none());
    }
}

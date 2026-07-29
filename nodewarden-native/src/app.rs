//! Application-level glue: the pieces `main` orchestrates, kept in the library
//! so they're reachable from examples and integration tests rather than being
//! locked inside the binary target.

use crate::app_match::{AppMatch, TriggerMode};
use crate::injector::{Injector, SendInputFiller, UiAutomationFiller};
use crate::match_engine::MatchEngine;
use crate::overlay_ui;
use crate::vault_bridge::{extract_app_match, VaultBridge, VaultError, VaultItem};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

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
/// Fetches the *single* item by id rather than pulling and scanning the whole
/// vault, which is what a fill used to do on every keystroke-worth of work.
pub fn fill_from_vault<A: UiAutomationFiller, B: SendInputFiller>(
    vault: &VaultBridge,
    injector: &Injector<A, B>,
    item_id: &str,
    hwnd: isize,
) {
    match vault.get_item(item_id) {
        Ok(item) => {
            let (username, password) = credentials_for(&item);
            if username.is_empty() && password.is_empty() {
                log::warn!("vault item {item_id} has no login credentials; nothing to fill");
                return;
            }
            if let Err(e) = injector.fill(hwnd, &username, &password) {
                log::error!("fill failed for item {item_id} into hwnd {hwnd}: {e}");
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
    vault: &VaultBridge,
    injector: &Injector<A, B>,
    item_id: &str,
    m: &AppMatch,
    hwnd: isize,
    exe_name: &str,
) -> Option<(String, isize)> {
    match m.trigger {
        TriggerMode::Auto => {
            fill_from_vault(vault, injector, item_id, hwnd);
            None
        }
        TriggerMode::Prompt => {
            if overlay_ui::show_prompt_overlay(exe_name) {
                fill_from_vault(vault, injector, item_id, hwnd);
            }
            None
        }
        TriggerMode::Hotkey => Some((item_id.to_string(), hwnd)),
    }
}

/// Pure helper: turns a list of vault items into the `(item_id, AppMatch)`
/// entries the match engine is rebuilt from, dropping items with no
/// `nodewarden:app-match` field.
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
}

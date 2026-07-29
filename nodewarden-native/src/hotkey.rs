use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

pub struct FillHotkey {
    _manager: GlobalHotKeyManager,
    hotkey_id: u32,
}

pub fn register_fill_hotkey() -> FillHotkey {
    let manager = GlobalHotKeyManager::new().expect("failed to init hotkey manager");
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyB);
    manager.register(hotkey).expect("failed to register Ctrl+Alt+B");

    FillHotkey { _manager: manager, hotkey_id: hotkey.id() }
}

/// Drains the global hotkey event channel and reports whether the fill
/// hotkey was pressed. Only `HotKeyState::Pressed` counts -- `global-hotkey`
/// emits a separate `Released` event for every key-up, and without this
/// filter a single Ctrl+Alt+B press would be observed twice (once on the
/// way down, once on the way up), double-firing the fill.
pub fn fill_hotkey_pressed(fh: &FillHotkey) -> bool {
    if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
        return event.id == fh.hotkey_id && event.state == HotKeyState::Pressed;
    }
    false
}

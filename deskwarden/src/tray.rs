use semver::Version;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

pub struct AppTray {
    _icon: TrayIcon,
    pub add_app_id: MenuId,
    pub quit_id: MenuId,
    /// Id of the "Update available" item, for comparing against
    /// `MenuEvent::id` in the main loop -- same pattern as `add_app_id` and
    /// `quit_id`.
    pub update_id: MenuId,
    /// The item itself, kept (not just its id) because making its label and
    /// enabled state reflect a discovered update requires the `MenuItem`
    /// handle: `set_text`/`set_enabled` mutate it in place. Private so
    /// callers go through `set_update_available` rather than poking at menu
    /// internals directly.
    update_item: MenuItem,
}

pub fn build_tray() -> AppTray {
    let menu = Menu::new();
    let add_app = MenuItem::new("Add app...", true, None);
    let quit = MenuItem::new("Quit", true, None);
    // Present in the menu from startup, but disabled until an update is
    // actually found. Muda's `MenuItem` supports updating text and enabled
    // state in place after the menu is built (`set_text`/`set_enabled`),
    // which is simpler and less error-prone than inserting/removing menu
    // entries at runtime, so that's the mechanism `set_update_available`
    // uses rather than rebuilding the menu.
    let update_item = MenuItem::new("Update available", false, None);
    menu.append(&add_app).unwrap();
    menu.append(&update_item).unwrap();
    menu.append(&quit).unwrap();

    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("deskwarden")
        .build()
        .expect("failed to build tray icon");

    AppTray {
        _icon: icon,
        add_app_id: add_app.id().clone(),
        quit_id: quit.id().clone(),
        update_id: update_item.id().clone(),
        update_item,
    }
}

pub fn next_menu_event() -> Option<MenuEvent> {
    MenuEvent::receiver().try_recv().ok()
}

/// Enables the "Update available" tray item and labels it with the version
/// that was found, once `updater::check_for_update` has found one. Called
/// from the main loop's periodic update check; a no-op on repeated calls with
/// the same version beyond redundantly re-setting the same text.
pub fn set_update_available(tray: &AppTray, version: &Version) {
    tray.update_item.set_text(format!("Update available (v{version})"));
    tray.update_item.set_enabled(true);
}

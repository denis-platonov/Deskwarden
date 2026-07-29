use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

pub struct AppTray {
    _icon: TrayIcon,
    pub add_app_id: tray_icon::menu::MenuId,
    pub quit_id: tray_icon::menu::MenuId,
}

pub fn build_tray() -> AppTray {
    let menu = Menu::new();
    let add_app = MenuItem::new("Add app...", true, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&add_app).unwrap();
    menu.append(&quit).unwrap();

    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("nodewarden-native")
        .build()
        .expect("failed to build tray icon");

    AppTray { _icon: icon, add_app_id: add_app.id().clone(), quit_id: quit.id().clone() }
}

pub fn next_menu_event() -> Option<MenuEvent> {
    MenuEvent::receiver().try_recv().ok()
}

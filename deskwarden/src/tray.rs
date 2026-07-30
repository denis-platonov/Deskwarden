use semver::Version;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Ordinal of the icon resource `build.rs` embeds into the executable. Must
/// stay in step with the id passed to `set_icon_with_id` there -- there is no
/// compile-time link between the two, so this constant and that call are the
/// contract.
const APP_ICON_RESOURCE_ID: u16 = 1;

/// Tooltip shown when nothing update-related is going on. Also the string the
/// tray reverts to conceptually -- the update states below replace it while
/// they apply.
const IDLE_TOOLTIP: &str = "Deskwarden";

pub struct AppTray {
    /// Kept (not just dropped-on-the-floor) because the tooltip is this app's
    /// only user-visible channel for update progress/failure: a tray app has
    /// no window and no console, so without this a failed update would be
    /// visible only to whoever goes looking in the log file.
    icon: TrayIcon,
    pub open_vault_id: MenuId,
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
    let open_vault = MenuItem::new("Open Vault", true, None);
    let add_app = MenuItem::new("Add app...", true, None);
    let quit = MenuItem::new("Quit", true, None);
    // Present in the menu from startup, but disabled until an update is
    // actually found. Muda's `MenuItem` supports updating text and enabled
    // state in place after the menu is built (`set_text`/`set_enabled`),
    // which is simpler and less error-prone than inserting/removing menu
    // entries at runtime, so that's the mechanism `set_update_available`
    // uses rather than rebuilding the menu.
    let update_item = MenuItem::new("Update available", false, None);
    menu.append(&open_vault).unwrap();
    menu.append(&add_app).unwrap();
    menu.append(&update_item).unwrap();
    menu.append(&quit).unwrap();

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(IDLE_TOOLTIP);
    if let Some(app_icon) = app_icon() {
        builder = builder.with_icon(app_icon);
    }

    let icon = builder.build().expect("failed to build tray icon");

    AppTray {
        icon,
        open_vault_id: open_vault.id().clone(),
        add_app_id: add_app.id().clone(),
        quit_id: quit.id().clone(),
        update_id: update_item.id().clone(),
        update_item,
    }
}

/// Loads the icon `build.rs` embedded into this executable.
///
/// `None` (rather than a panic) if it isn't there: the resource step is
/// best-effort by design -- a machine without `rc.exe`/`windres` builds a
/// working binary with no icon resource -- and an iconless tray is a cosmetic
/// problem, not a reason to refuse to start a password filler. That was the
/// behaviour before an icon existed at all.
fn app_icon() -> Option<Icon> {
    match Icon::from_resource(APP_ICON_RESOURCE_ID, None) {
        Ok(icon) => Some(icon),
        Err(e) => {
            log::warn!("could not load the embedded application icon ({e}); tray will be iconless");
            None
        }
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
    set_tooltip(tray, format!("Deskwarden - update available (v{version})"));
}

/// Reflects an in-flight download/verify/apply attempt (see `main.rs`'s
/// background update thread).
///
/// The item is disabled for the duration so a second click can't start a
/// second concurrent download of the same installer, and the label says what
/// is happening: the work now runs off the main thread, so without this the
/// only feedback for a multi-megabyte download would be a menu item that
/// appears to do nothing.
pub fn set_update_in_progress(tray: &AppTray, version: &Version) {
    tray.update_item.set_text(format!("Downloading update (v{version})..."));
    tray.update_item.set_enabled(false);
    set_tooltip(tray, format!("Deskwarden - downloading update v{version}"));
}

/// Reports a failed update attempt.
///
/// A failure used to be logged and otherwise invisible: the app has no window
/// and (deliberately) no console, so the user clicked "Update available" and
/// simply nothing ever happened. The item is re-enabled -- the failure is
/// frequently transient (network, a GitHub hiccup) and retrying is the right
/// affordance -- and both the label and the tooltip say so.
pub fn set_update_failed(tray: &AppTray, version: &Version) {
    tray.update_item
        .set_text(format!("Update to v{version} failed - click to retry"));
    tray.update_item.set_enabled(true);
    set_tooltip(
        tray,
        format!("Deskwarden - update to v{version} failed; see the log file"),
    );
}

/// Best-effort tooltip update: a tooltip that won't set is a cosmetic
/// problem, never a reason to fail the operation it was describing.
fn set_tooltip(tray: &AppTray, text: String) {
    if let Err(e) = tray.icon.set_tooltip(Some(&text)) {
        log::debug!("could not set tray tooltip: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_application_icon_loads_by_ordinal() {
        // The only thing tying `APP_ICON_RESOURCE_ID` to the id build.rs
        // passes to `set_icon_with_id` is that both say 1. Nothing checks
        // that at compile time, and a mismatch fails silently -- the tray
        // just goes back to being iconless, which is exactly the state this
        // was added to fix. `build_tray()` itself can't be tested here (it
        // creates real Win32 windows and needs a message pump), so the icon
        // load is tested on its own.
        assert!(
            app_icon().is_some(),
            "the icon resource embedded by build.rs could not be loaded by ordinal \
             {APP_ICON_RESOURCE_ID}"
        );
    }
}

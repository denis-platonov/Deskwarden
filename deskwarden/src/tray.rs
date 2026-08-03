use crate::accounts::{account_label, AccountId, AccountsState};
use semver::Version;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Ordinal of the icon resource `build.rs` embeds into the executable. Must
/// stay in step with the id passed to `set_icon_with_id` there -- there is no
/// compile-time link between the two, so this constant and that call are the
/// contract.
const APP_ICON_RESOURCE_ID: u16 = 1;

/// Tooltip shown when nothing update-related is going on. Also the string the
/// tray reverts to conceptually -- the update states below replace it while
/// they apply.
const IDLE_TOOLTIP: &str = "Deskwarden";

/// Label of the submenu every account action lives under.
const ACCOUNTS_SUBMENU: &str = "Accounts";

/// Label of the item that mints and signs in to another account.
const ADD_ACCOUNT: &str = "Add account...";

/// Shown, disabled, when there is exactly one account and nothing is refused.
/// An empty submenu reads as a broken menu; this reads as "you have one".
const NO_OTHER_ACCOUNTS: &str = "No other accounts yet";

/// Shown, disabled, when this process has no `AccountsState` at all --
/// `StartupAccounts::Unmigrated`, where there is no `Account` in existence and
/// the app is running against the CLI's own default profile. Not a blocked
/// state and not a one-account state: there is nothing here to switch *from*.
const ACCOUNTS_NOT_SET_UP: &str =
    "Accounts are not set up on this machine yet - restart Deskwarden";

/// What the "Accounts" submenu should contain, decided from
/// [`AccountsState`](crate::accounts::AccountsState) alone.
///
/// **Every field is an answer this type asked the one door for.** Nothing here
/// re-derives "may I switch" from the CLI's availability or the migration's
/// outcome -- `tray.rs` is on the ban list
/// `no_window_answers_may_i_switch_for_itself` enforces, and it is on it for
/// the same reason `vault_window/mod.rs` is: a second reading of those two
/// facts is a second answer, and the two would disagree exactly where the trap
/// is.
///
/// Separated from the `muda` construction below because that construction
/// needs a real Windows menu and a live message loop. This is the half every
/// test drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountsMenuPlan {
    /// The account this process is on, as a disabled header. Always present
    /// when there is an `AccountsState` at all: a menu of switch targets with
    /// no indication of where you currently are is a menu you cannot read.
    pub active: Option<String>,
    /// One row per account the user may switch **to**, in
    /// [`AccountsState::switchable`](crate::accounts::AccountsState::switchable)
    /// order. Never [`all`](crate::accounts::AccountsState::all), which still
    /// reports the active account, still reports duplicate ids, and is not
    /// emptied when switching is refused.
    pub switch_to: Vec<(AccountId, String)>,
    /// A disabled row explaining why there is nothing to switch to, when there
    /// is nothing to switch to. `None` whenever [`Self::switch_to`] is
    /// non-empty.
    pub notice: Option<String>,
    /// Whether "Add account..." is offered at all.
    ///
    /// **Gated on
    /// [`can_add`](crate::accounts::AccountsState::can_add), which is what
    /// keeps `SwitchOutcome::Declined` unambiguous.** `add_account` answers
    /// `Declined` both for "the user closed the sign-in window" and for "the
    /// gate refused", and a tray that reported "cancelled" for a
    /// `relativeDataDir` block would be telling the user something that never
    /// happened. An item that is not there cannot be clicked, so the only
    /// `Declined` this wiring can ever see is the cancelled sign-in.
    pub add: bool,
    /// The label of the account "Remove..." would delete, when a removal could
    /// possibly succeed -- `None` otherwise, because a menu item that can only
    /// fail is worse than one that is not there.
    ///
    /// Two refusals, both `remove_account`'s own and both asked of the same
    /// door: **the last account cannot be removed** (there is nowhere coherent
    /// for the app to land), and a **blocked** state cannot remove anything
    /// (the app cannot reach the survivor it would have to settle onto). Both
    /// collapse to "there is at least one switchable account", because that is
    /// exactly the survivor `next_active_after_removal` picks.
    pub remove: Option<String>,
}

/// The label the "Remove..." item carries for `active`.
pub fn remove_account_label(active: &str) -> String {
    format!("Remove {active}...")
}

/// Decides the submenu's contents. See [`AccountsMenuPlan`].
///
/// `None` is `StartupAccounts::Unmigrated`: `main` builds no `AccountsState`
/// there because there is no `Account` to build one around, and
/// `vault_window::run` already takes the same `Option`. The submenu is not
/// hidden in that case -- a menu item that vanishes is a menu item the user
/// concludes they imagined -- it says so and offers nothing.
pub fn accounts_menu_plan(state: Option<&AccountsState>) -> AccountsMenuPlan {
    let Some(state) = state else {
        return AccountsMenuPlan {
            active: None,
            switch_to: Vec::new(),
            notice: Some(ACCOUNTS_NOT_SET_UP.to_string()),
            add: false,
            remove: None,
        };
    };

    let active = account_label(state.active()).to_string();
    let switch_to: Vec<(AccountId, String)> = state
        .switchable()
        .iter()
        .map(|a| (a.id.clone(), account_label(a).to_string()))
        .collect();

    let notice = if !switch_to.is_empty() {
        None
    } else {
        // The blocked reason outranks "no other accounts yet", and it has to:
        // a blocked state may well hold several accounts, and telling the user
        // they have one would be false as well as unactionable.
        Some(
            state
                .blocked_reason()
                .map(str::to_string)
                .unwrap_or_else(|| NO_OTHER_ACCOUNTS.to_string()),
        )
    };

    AccountsMenuPlan {
        remove: (!switch_to.is_empty()).then(|| remove_account_label(&active)),
        active: Some(active),
        switch_to,
        notice,
        add: state.can_add(),
    }
}

/// The built submenu's answer to "which account was clicked?".
///
/// A `MenuId` → `AccountId` map rather than a match on labels: two accounts
/// can carry the same email (the same address on two servers), and a label
/// match would then switch to whichever came first.
#[derive(Debug, Clone, Default)]
pub struct AccountsMenu {
    entries: Vec<(MenuId, AccountId)>,
    /// `None` when the item was not built, which is how a refused add becomes
    /// unclickable rather than merely refused after the fact.
    add_id: Option<MenuId>,
    remove_id: Option<MenuId>,
}

impl AccountsMenu {
    pub fn from_entries(
        entries: Vec<(MenuId, AccountId)>,
        add_id: Option<MenuId>,
        remove_id: Option<MenuId>,
    ) -> Self {
        Self {
            entries,
            add_id,
            remove_id,
        }
    }

    /// The account `id` switches to, or `None` -- including for
    /// "Add account..." and "Remove ...", which must never be mistaken for
    /// accounts.
    pub fn account_for_menu_id(&self, id: &MenuId) -> Option<&AccountId> {
        self.entries
            .iter()
            .find(|(menu_id, _)| menu_id == id)
            .map(|(_, account)| account)
    }

    pub fn is_add(&self, id: &MenuId) -> bool {
        self.add_id.as_ref() == Some(id)
    }

    pub fn is_remove(&self, id: &MenuId) -> bool {
        self.remove_id.as_ref() == Some(id)
    }

    /// Whether `id` belongs to this submenu at all, so the main loop can skip
    /// the whole account block for the clicks that are not one.
    pub fn owns(&self, id: &MenuId) -> bool {
        self.account_for_menu_id(id).is_some() || self.is_add(id) || self.is_remove(id)
    }
}

pub struct AppTray {
    /// Kept (not just dropped-on-the-floor) because the tooltip is this app's
    /// only user-visible channel for update progress/failure: a tray app has
    /// no window and no console, so without this a failed update would be
    /// visible only to whoever goes looking in the log file.
    icon: TrayIcon,
    pub open_vault_id: MenuId,
    pub add_app_id: MenuId,
    /// Id of the "Sync" item. With the periodic match-engine refresh gone
    /// (see `main.rs`), this is the only way to pull in a match added on
    /// another device, or a change made there, without opening the vault
    /// window (whose own toolbar already has a sync pill) or restarting the
    /// app.
    pub sync_id: MenuId,
    /// Id of the "Preferences..." item -- opens `prefs_ui::run` (see
    /// `main.rs`'s menu-event handling).
    pub preferences_id: MenuId,
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
    /// Same reasoning as `update_item`, for `set_sync_in_progress`/
    /// `set_sync_idle`/`set_sync_failed`.
    sync_item: MenuItem,
    /// The "Accounts" submenu, kept for the same reason `update_item` is: its
    /// contents change whenever an account is added, removed or switched to,
    /// and rebuilding it needs the handle.
    accounts_submenu: Submenu,
    /// What the submenu's ids currently mean. Replaced wholesale by
    /// [`AppTray::rebuild_accounts_menu`] -- the ids are minted with the items,
    /// so a stale map is a click that switches to the wrong account.
    accounts: AccountsMenu,
}

pub fn build_tray() -> AppTray {
    let menu = Menu::new();
    let open_vault = MenuItem::new("Open Vault", true, None);
    let add_app = MenuItem::new("Add app...", true, None);
    let sync_item = MenuItem::new("Sync", true, None);
    let quit = MenuItem::new("Quit", true, None);
    // Present in the menu from startup, but disabled until an update is
    // actually found. Muda's `MenuItem` supports updating text and enabled
    // state in place after the menu is built (`set_text`/`set_enabled`),
    // which is simpler and less error-prone than inserting/removing menu
    // entries at runtime, so that's the mechanism `set_update_available`
    // uses rather than rebuilding the menu.
    let update_item = MenuItem::new("Update available", false, None);
    let preferences = MenuItem::new("Preferences...", true, None);
    // Empty here and filled by `rebuild_accounts_menu`, which `main` calls
    // once the account list exists and again after every change to it. Built
    // empty rather than not at all so there is exactly one place that decides
    // what is in it.
    let accounts_submenu = Submenu::new(ACCOUNTS_SUBMENU, true);
    menu.append(&open_vault).unwrap();
    menu.append(&add_app).unwrap();
    menu.append(&sync_item).unwrap();
    menu.append(&update_item).unwrap();
    menu.append(&accounts_submenu).unwrap();
    menu.append(&preferences).unwrap();
    menu.append(&quit).unwrap();

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(IDLE_TOOLTIP)
        // Left click opens the vault directly (see `next_tray_icon_event`'s
        // caller in `main.rs`) instead of showing the same menu right-click
        // already shows -- this crate's default is to show the menu on
        // either button.
        .with_menu_on_left_click(false);
    if let Some(app_icon) = app_icon() {
        builder = builder.with_icon(app_icon);
    }

    let icon = builder.build().expect("failed to build tray icon");

    AppTray {
        icon,
        open_vault_id: open_vault.id().clone(),
        add_app_id: add_app.id().clone(),
        sync_id: sync_item.id().clone(),
        preferences_id: preferences.id().clone(),
        quit_id: quit.id().clone(),
        update_id: update_item.id().clone(),
        update_item,
        sync_item,
        accounts_submenu,
        accounts: AccountsMenu::default(),
    }
}

impl AppTray {
    /// What the "Accounts" submenu currently means.
    pub fn accounts(&self) -> &AccountsMenu {
        &self.accounts
    }

    /// Rebuilds the submenu from [`accounts_menu_plan`], and replaces the id
    /// map with the ids of the items it just built.
    ///
    /// Rebuilt rather than mutated in place: the number of rows changes with
    /// every add and every removal, and an item's `MenuId` is minted when the
    /// item is. The two therefore have to move together, which is why this
    /// function owns both halves and `self.accounts` is private.
    ///
    /// **This is the untestable half**, and deliberately the thinnest one:
    /// every decision above it is `accounts_menu_plan`'s, and every lookup
    /// after it is [`AccountsMenu`]'s. What is left here is `muda` calls on a
    /// menu owned by a real tray icon, which no test in this crate can build
    /// (see `build_tray`).
    pub fn rebuild_accounts_menu(&mut self, state: Option<&AccountsState>) {
        let plan = accounts_menu_plan(state);
        self.accounts = build_accounts_submenu(&self.accounts_submenu, &plan);
    }
}

/// Fills `submenu` with `plan` and hands back the id map for it.
///
/// Split out of [`AppTray::rebuild_accounts_menu`] for the reason the
/// `sync_item_to_*` helpers below are split out of the `set_sync_*` functions:
/// an `AppTray` owns a real `TrayIcon` and cannot be built in a test, but a
/// bare `Submenu` can -- so the mapping between a plan and the items it
/// produces is driven directly.
fn build_accounts_submenu(submenu: &Submenu, plan: &AccountsMenuPlan) -> AccountsMenu {
    // Emptied first, and from the front: `remove_at` is the only removal that
    // does not need a handle on the item being removed, and these handles were
    // dropped as soon as the previous rebuild finished with their ids.
    while submenu.remove_at(0).is_some() {}

    if let Some(active) = &plan.active {
        // Disabled: this is the account you are on, and "switch to where you
        // already are" still tears the backend down and demands a master
        // password.
        let header = MenuItem::new(format!("Signed in: {active}"), false, None);
        let _ = submenu.append(&header);
        let _ = submenu.append(&PredefinedMenuItem::separator());
    }

    let mut entries = Vec::new();
    for (id, label) in &plan.switch_to {
        let item = MenuItem::new(label, true, None);
        entries.push((item.id().clone(), id.clone()));
        let _ = submenu.append(&item);
    }
    if let Some(notice) = &plan.notice {
        let _ = submenu.append(&MenuItem::new(notice, false, None));
    }

    let add_id = plan.add.then(|| {
        let item = MenuItem::new(ADD_ACCOUNT, true, None);
        let id = item.id().clone();
        let _ = submenu.append(&item);
        id
    });
    let remove_id = plan.remove.as_ref().map(|label| {
        let item = MenuItem::new(label, true, None);
        let id = item.id().clone();
        let _ = submenu.append(&item);
        id
    });

    AccountsMenu::from_entries(entries, add_id, remove_id)
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

/// True when `event` is a completed left click on the tray icon (the button
/// release, not the press -- Windows reports both as separate `Click`
/// events with a `button_state`, and reacting on `Down` as well would fire
/// this twice per physical click).
pub fn is_left_click(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    )
}

pub fn next_tray_icon_event() -> Option<TrayIconEvent> {
    TrayIconEvent::receiver().try_recv().ok()
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

/// Reflects an in-flight tray-triggered sync (see `main.rs`'s background sync
/// thread, spawned from the "Sync" menu item).
///
/// Same shape as `set_update_in_progress`: the item is disabled for the
/// duration so a second click can't start a second concurrent sync, and the
/// label says what's happening -- the work runs off the main thread, so
/// without this the item would just appear to do nothing while it's running.
pub fn set_sync_in_progress(tray: &AppTray) {
    sync_item_to_in_progress(&tray.sync_item);
    set_tooltip(tray, "Deskwarden - syncing...".to_string());
}

/// Returns the "Sync" item to its resting state: label, enabled state and
/// tooltip together.
///
/// This is the **only** way anything re-enables the item (review 18's Minor).
/// It used to share that job with a `set_sync_item_enabled(tray, true)` that
/// restored the enabled state and nothing else, and the two disagreed in a
/// reachable state: `open_vault_window` abandons a wedged backend operation
/// at `BACKEND_OP_TIMEOUT` and re-enabled the item there, so a wedged *sync*
/// left the menu reading "Syncing..." -- with the "Deskwarden - syncing..."
/// tooltip -- on an item that was idle and clickable. That label says "busy,
/// do not click" at exactly the moment `stand_down_after_unlock`'s message
/// tells the user to click "Sync", which is then not a name anything in the
/// menu has. It could persist for the whole session, because the thread that
/// would have relabelled it is the one that never reported back.
pub fn set_sync_idle(tray: &AppTray) {
    sync_item_to_idle(&tray.sync_item);
    set_tooltip(tray, IDLE_TOOLTIP.to_string());
}

/// Reports a failed tray-triggered sync.
///
/// Re-enabled immediately (unlike the update item, which stays disabled while
/// its "click to retry" label shows): a sync failure is frequently transient
/// (network, `bw serve` still coming up) and there's no download in flight
/// that a second click could collide with.
///
/// The label itself, though, follows `set_update_failed`'s pattern rather
/// than the tooltip-only shape this used to have: a tray tooltip is only
/// visible on hover, so a failure that landed while the user wasn't looking
/// was otherwise invisible until they happened to hover the icon or went
/// looking in the log file. The label is seen the moment the menu is opened,
/// which is also the same click that retries.
pub fn set_sync_failed(tray: &AppTray) {
    sync_item_to_failed(&tray.sync_item);
    set_tooltip(tray, "Deskwarden - sync failed; see the log file".to_string());
}

/// Disables the "Sync" item without touching its label, for an in-flight
/// backend operation that needs `bw serve` up but isn't itself a sync -- the
/// tray's "Add app..." handler starting the backend so the picker can save.
///
/// Without this, a Sync click landing while that start is still in flight
/// used to be silently dropped by `main`'s `backend_task_in_progress` guard
/// (review 10's Minor 6): the item still looked normal and clickable, so the
/// click appeared to do nothing. Kept separate from `set_sync_in_progress`
/// deliberately: that label says "Syncing...", which would be untrue for a
/// plain backend start with no `bw sync` attached -- this only changes
/// whether the item can be clicked, not what it claims is happening.
///
/// **One direction only** (review 18's Minor): this used to take a bool, and
/// its `true` half was how four call sites re-enabled the item, leaving
/// whatever busy label was showing in place. Undoing this is
/// [`set_sync_idle`], which restores label, enabled state and tooltip
/// together, so "enabled" and "idle" cannot disagree. Preserving the label
/// while DISABLING is safe in a way preserving it while enabling is not: a
/// disabled item cannot invite a click it will then refuse, and the label it
/// preserves ("Sync failed - click to retry", say) is still true.
pub fn set_sync_busy_with_backend_op(tray: &AppTray) {
    tray.sync_item.set_enabled(false);
}

// The four appearances of the "Sync" item, applied to the `MenuItem` alone.
//
// Split out from the `set_sync_*` functions above purely so they can be
// tested: an `AppTray` cannot be constructed in a test (it owns a real
// `TrayIcon`, which needs a window and a message pump), but a bare
// `MenuItem` can. The tooltip stays with the public functions, since that
// genuinely does need the icon.

fn sync_item_to_in_progress(item: &MenuItem) {
    item.set_text("Syncing...");
    item.set_enabled(false);
}

fn sync_item_to_idle(item: &MenuItem) {
    item.set_text("Sync");
    item.set_enabled(true);
}

fn sync_item_to_failed(item: &MenuItem) {
    item.set_text("Sync failed - click to retry");
    item.set_enabled(true);
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

    /// Review 18's Minor. Every path that lets the user click "Sync" again
    /// has to leave it *saying* "Sync" -- there is no state in which enabled
    /// and busy-labelled is what anyone wanted.
    ///
    /// The reachable instance: a tray Sync wedges, `open_vault_window` gives
    /// up on it after `BACKEND_OP_TIMEOUT` and releases the item, and before
    /// this fix that release restored only the enabled state. The menu then
    /// read "Syncing..." on an idle item, for as long as the wedged thread
    /// never reported -- i.e. possibly the whole session, since review 17
    /// made that state outlive the process. `stand_down_after_unlock`, which
    /// runs moments later on that same path, tells the user to click "Sync";
    /// there is then no such item, and the one that is there says "busy".
    ///
    /// HONEST NOTE ON THE RED: this test cannot fail in its final form,
    /// because the defect was the CALL SITE picking an enable-only API and
    /// no test can build an `AppTray` to drive that call site. It was
    /// watched failing (`"Syncing..." != "Sync"`) against a
    /// `set_enabled(true)`-only release helper standing in for the deleted
    /// `set_sync_item_enabled(tray, true)`; the fix deletes that helper, so
    /// no call site can choose it again and the compiler enforces it.
    #[test]
    fn releasing_the_sync_item_leaves_it_labelled_idle_not_busy() {
        let item = MenuItem::new("Sync", true, None);

        sync_item_to_in_progress(&item);
        assert_eq!(item.text(), "Syncing...");
        assert!(!item.is_enabled());

        sync_item_to_idle(&item);
        assert!(
            item.is_enabled(),
            "the whole point of the release is that the item is clickable again"
        );
        assert_eq!(
            item.text(),
            "Sync",
            "an enabled item still reading \"Syncing...\" tells the user not to click the one \
             affordance the stand-down message names"
        );
    }

    /// The other releasing state has to hold the same invariant: it is
    /// enabled, so its label must describe something the user can act on
    /// rather than something in flight.
    #[test]
    fn the_failed_sync_label_is_enabled_and_says_what_to_do() {
        let item = MenuItem::new("Sync", true, None);
        sync_item_to_in_progress(&item);
        sync_item_to_failed(&item);

        assert!(item.is_enabled());
        assert_eq!(item.text(), "Sync failed - click to retry");
    }

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

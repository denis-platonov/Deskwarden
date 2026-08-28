use crate::accounts::{account_label, AccountId, AccountsState};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Ordinal of the icon resource `build.rs` embeds into the executable. Must
/// stay in step with the id passed to `set_icon_with_id` there -- there is no
/// compile-time link between the two, so this constant and that call are the
/// contract.
const APP_ICON_RESOURCE_ID: u16 = 1;

/// The tooltip, and now very nearly the only one.
///
/// **It no longer announces updates, and that is the point of this constant's
/// new doc.** It used to be one of four strings: the other three said an
/// update was available, was downloading, or had failed. All three are gone
/// with the menu item they described, because a tooltip is visible only while
/// the pointer is resting on a 16px icon -- so it reported a download to
/// whoever happened to be hovering and to nobody else, and reported a failure
/// the same way. That is not a channel for something the user asked for and is
/// waiting on.
///
/// The update flow reports into Preferences → About instead: the page the user
/// clicked in is the page that answers. What is left here is the app's name,
/// plus `set_sync_in_progress`/`set_sync_failed`'s two sync lines -- which
/// stay, because a sync is something the user starts *from this menu* and the
/// menu item beside the tooltip carries the same words.
const IDLE_TOOLTIP: &str = "Deskwarden";

/// Label of the submenu every account action lives under.
const ACCOUNTS_SUBMENU: &str = "Accounts";

/// Label of the item that mints and signs in to another account.
const ADD_ACCOUNT: &str = "Add account...";

/// Shown, disabled, when there is exactly one account and nothing is refused.
/// An empty submenu reads as a broken menu; this reads as "you have one".
const NO_OTHER_ACCOUNTS: &str = "No other accounts yet";

/// Shown, disabled, when this process has no `AccountsState` at all --
/// `StartupAccounts::NoAccountList`, where `settings.json` could not be read,
/// there is no `Account` in existence, and the app is running against the
/// CLI's own default profile. Not a blocked state and not a one-account state:
/// there is nothing here to switch *from*.
const ACCOUNTS_NOT_SET_UP: &str =
    "Accounts are not set up on this machine yet - restart Deskwarden";

/// What the "Accounts" submenu should contain, decided from
/// [`AccountsState`](crate::accounts::AccountsState) alone.
///
/// **Every field is an answer this type asked the one door for.** Nothing here
/// re-derives "may I switch" from the CLI's availability -- `tray.rs` is on the
/// ban list `no_window_answers_may_i_switch_for_itself` enforces, and it is on
/// it for the same reason `vault_window/mod.rs` is: a second reading of that
/// fact is a second answer, and the two would disagree exactly where the trap
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
    /// Both refusals are `remove_account`'s own and both are asked of the one
    /// door, [`AccountsState::can_remove_active`](crate::accounts::AccountsState::can_remove_active)
    /// — see it for why they collapse into a single question. The vault
    /// window's account menu offers the same removal and asks the same door;
    /// this used to spell the rule out here instead, which is one of the two
    /// menus keeping an item that can only fail.
    pub remove: Option<String>,
}

/// The label the "Remove..." item carries for `active`.
pub fn remove_account_label(active: &str) -> String {
    format!("Remove {active}...")
}

/// Decides the submenu's contents. See [`AccountsMenuPlan`].
///
/// `None` is `StartupAccounts::NoAccountList`: `main` builds no `AccountsState`
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
        remove: state
            .can_remove_active()
            .then(|| remove_account_label(&active)),
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
    /// Kept (not just dropped-on-the-floor) because the tooltip is set through
    /// it: `set_sync_in_progress`/`set_sync_failed` say there what the Sync
    /// item says in the menu beside it.
    ///
    /// It used to carry the update flow's only user-visible reporting too, on
    /// the grounds that a tray app has no window. That grounds was wrong in
    /// one direction -- the app does have a window when Preferences is open,
    /// which is exactly when an update is being asked for -- and the tooltip
    /// was a poor channel in the other, being visible only while the pointer
    /// rests on the icon. See [`IDLE_TOOLTIP`].
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
    /// Kept (not just its id) because `set_sync_in_progress`/`set_sync_idle`/
    /// `set_sync_failed` make its label and enabled state reflect what the
    /// backend is doing, and `set_text`/`set_enabled` mutate it in place.
    /// Private so callers go through those functions rather than poking at
    /// menu internals directly.
    sync_item: MenuItem,
    /// The "Accounts" submenu, kept for the same reason `sync_item` is: its
    /// contents change whenever an account is added, removed or switched to,
    /// and rebuilding it needs the handle.
    accounts_submenu: Submenu,
    /// What the submenu's ids currently mean. Replaced wholesale by
    /// [`AppTray::rebuild_accounts_menu`] -- the ids are minted with the items,
    /// so a stale map is a click that switches to the wrong account.
    accounts: AccountsMenu,
}

/// The chord this app registers for filling. Spelled here as the tray
/// shows it; `prefs_ui` has its own copy for the Preferences row, and
/// `the_tray_and_preferences_agree_on_the_chord` holds the two together.
pub const FILL_HOTKEY: &str = "CTRL+ALT+B";

/// What the tray says about filling by keyboard.
///
/// **Informational, and deliberately not clickable.** Filling targets the
/// FOREGROUND window, and opening this menu makes the tray the foreground —
/// so a *Fill* the user could click would either do nothing or fill the
/// wrong window. The chord works because the target still has focus when it
/// is pressed, which is exactly what clicking a menu gives up.
///
/// It tells the truth about registration rather than always printing the
/// chord: if another program holds it, a menu promising CTRL+ALT+B would be
/// a menu lying to somebody whose fill is silently doing nothing — which is
/// the report that made `hotkey::availability` exist.
#[must_use]
pub fn fill_hint(status: &crate::hotkey::HotkeyStatus) -> String {
    match status {
        crate::hotkey::HotkeyStatus::Armed => format!("Fill:  {FILL_HOTKEY}"),
        crate::hotkey::HotkeyStatus::Unavailable(_) => {
            format!("Fill shortcut ({FILL_HOTKEY}) unavailable")
        }
    }
}

pub fn build_tray() -> AppTray {
    let menu = Menu::new();
    let open_vault = MenuItem::new("Open Vault", true, None);
    let add_app = MenuItem::new("Add app...", true, None);
    // **Disabled: a label, not a command.** See `fill_hint` for why a
    // clickable Fill would fill the wrong window. Placed under Add app so
    // the two things about filling sit together.
    let fill_hint_item = MenuItem::new(fill_hint(&crate::hotkey::availability()), false, None);
    let sync_item = MenuItem::new("Sync", true, None);
    let quit = MenuItem::new("Quit", true, None);
    // **There is no update item here, and its absence is deliberate.** There
    // was one, created as `MenuItem::new("Update available", false, None)`:
    // the words were baked in at build time and the check's only effect was to
    // *enable* it. So on every session with no update -- nearly all of them --
    // this menu asserted that an update was available and then refused the
    // click it was inviting. The whole flow now lives on Preferences → About
    // (`prefs_ui::draw_update_card`), where it can say "no update" as easily
    // as "update", where the release notes fit, and where the download it
    // starts can report back to the page that started it.
    let preferences = MenuItem::new("Preferences...", true, None);
    // Empty here and filled by `rebuild_accounts_menu`, which `main` calls
    // once the account list exists and again after every change to it. Built
    // empty rather than not at all so there is exactly one place that decides
    // what is in it.
    let accounts_submenu = Submenu::new(ACCOUNTS_SUBMENU, true);
    menu.append(&open_vault).unwrap();
    menu.append(&add_app).unwrap();
    menu.append(&fill_hint_item).unwrap();
    menu.append(&sync_item).unwrap();
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
/// PRIVATE, and that is load-bearing rather than tidiness. `TrayIconEvent`
/// does not leave this module: [`next_left_click`] is the only way anything
/// outside it reads that channel, so no caller can be acting on a variant that
/// [`discard_queued_icon_events`] throws away. See that function.
fn is_left_click(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    )
}

/// Private for the reason [`is_left_click`] is: this channel is read from
/// outside this module through [`next_left_click`] and nothing else.
fn next_tray_icon_event() -> Option<TrayIconEvent> {
    TrayIconEvent::receiver().try_recv().ok()
}

/// The next queued tray-icon event, reduced to the only question this app asks
/// of that channel: was it a completed left click?
///
/// `None` when the channel is empty. `Some(false)` is an event that is not a
/// left click -- a button-down, a pointer move, the right click that opens the
/// menu (which `muda` delivers on its own separate channel anyway) -- and the
/// caller ignores it, exactly as it did back when it was handed the event
/// itself and tested it with `is_left_click`.
///
/// **The reduction is the point, not a convenience.** It is what makes
/// [`discard_queued_icon_events`] provably lossless: that function throws away
/// every queued tray-icon event, and no caller can have been acting on the
/// variants it discards, because outside this module no caller can see them.
/// Restoring `pub` to either function below reopens exactly that gap.
pub fn next_left_click() -> Option<bool> {
    next_tray_icon_event().as_ref().map(is_left_click)
}

/// Throws away every tray-icon event sitting in the channel RIGHT NOW, and
/// reports how many of them were left clicks (i.e. how many "open the vault"
/// requests are being dropped as already answered).
///
/// Called immediately after the vault window returns; see `main`'s
/// `requests_outliving_a_window` for the rule, and why "queued at this instant"
/// is a boundary rather than a race.
///
/// Draining the whole channel rather than only the left clicks is safe here
/// because of the module boundary [`next_left_click`] draws -- see its doc.
pub fn discard_queued_icon_events() -> usize {
    let mut left_clicks = 0;
    while let Some(was_left_click) = next_left_click() {
        if was_left_click {
            left_clicks += 1;
        }
    }
    left_clicks
}

/// Reflects an in-flight tray-triggered sync (see `main.rs`'s background sync
/// thread, spawned from the "Sync" menu item).
///
/// The item is disabled for the duration so a second click can't start a
/// second concurrent sync, and the
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
/// Re-enabled immediately: a sync failure is frequently transient (network,
/// `bw serve` still coming up) and there's no work in flight that a second
/// click could collide with.
///
/// The label itself, though, does not follow the tooltip-only shape this used
/// to have -- for the reason [`IDLE_TOOLTIP`] now records at length: a tray
/// tooltip is only
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
    use crate::hotkey::{HotkeyStatus, Unavailable};

    /// **The chord is worth showing where the user already is.** The owner
    /// asked for it in the tray -- "so it is always handy" -- because a
    /// shortcut you have to open Preferences to remember is a shortcut you do
    /// not use.
    #[test]
    fn an_armed_hotkey_is_shown_by_its_chord() {
        let hint = fill_hint(&HotkeyStatus::Armed);
        assert!(hint.contains(FILL_HOTKEY), "the tray does not name the chord: {hint}");
    }

    /// **And an unregistered one says so rather than printing a chord that
    /// does nothing.** Somebody whose fill silently stops working is the
    /// report that made `hotkey::availability` exist; a menu that kept
    /// advertising the chord would hide exactly that.
    #[test]
    fn an_unavailable_hotkey_is_not_advertised_as_working() {
        let hint = fill_hint(&HotkeyStatus::Unavailable(Unavailable::TakenByAnotherProgram));
        assert!(
            hint.to_lowercase().contains("unavailable"),
            "the tray promises a shortcut that is not registered: {hint}"
        );
        // The control: the two states really do differ, so the assertion
        // above is not passing on a function that returns one string.
        assert_ne!(hint, fill_hint(&HotkeyStatus::Armed));
    }

    /// One chord, two places that print it. The tray and Preferences must not
    /// drift, and a user reading different chords in one app would not know
    /// which to believe.
    #[test]
    fn the_tray_and_preferences_agree_on_the_chord() {
        let prefs = include_str!("prefs_ui.rs");
        let declared = prefs
            .lines()
            .find(|l| l.trim_start().starts_with("const FILL_HOTKEY"))
            .expect("control: prefs_ui no longer declares the chord");
        assert!(
            declared.contains(FILL_HOTKEY),
            "the tray and Preferences print different chords: {declared}"
        );
    }

    use super::*;
    use crate::accounts::Account;
    use tray_icon::menu::ContextMenu;
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuStringW, HMENU, MF_BYPOSITION};

    /// The label Windows itself holds for the item at `position` of `menu`.
    ///
    /// **`MenuItem::text()` must never be called from this file, and this
    /// exists so that it does not have to be.** muda 0.15.3 reads a menu
    /// item's label like this (`platform_impl/windows/mod.rs`):
    ///
    /// ```text
    /// info.cch += 1;
    /// info.dwTypeData = Vec::with_capacity(info.cch as usize).as_mut_ptr();
    /// GetMenuItemInfoW(*hmenu, self.internal_id(), false.into(), &mut info);
    /// let text = decode_wide(info.dwTypeData);
    /// ```
    ///
    /// That `Vec` is a temporary. It is dropped -- and its buffer handed back
    /// to the allocator -- at the semicolon, *before* `GetMenuItemInfoW`
    /// writes the label into it and before `decode_wide` reads it back. It is
    /// a use-after-free whose window is one Win32 call wide. On a quiet thread
    /// the freed block is still intact and the label comes back right, which
    /// is why this looks like a working API; under `cargo test`, with a couple
    /// of thousand tests allocating on other threads, another thread
    /// occasionally takes that block first and `text()` answers with something
    /// that is not the label.
    ///
    /// Measured, not supposed: 6 failures in 200 runs of `tray::` at
    /// `--test-threads=16` on `1d0f98d`, every one of them a row that the line
    /// immediately above had just found by the same call -- i.e. two reads of
    /// one unchanged item disagreeing. That is the whole of the "these two
    /// tests are flaky" report.
    ///
    /// So labels are read by POSITION, out of the menu handle, into a buffer
    /// that is alive for the call. muda's `add_menu_item` appends to `hmenu`
    /// and `hpopupmenu` in the same order it pushes into `children`, so
    /// position `i` here is `menu.items()[i]`.
    fn label_at(menu: &Submenu, position: usize) -> String {
        let hmenu = HMENU(menu.hpopupmenu() as *mut std::ffi::c_void);
        let position = u32::try_from(position).expect("a menu position");
        // `None` asks for the length; a separator answers 0 and has no label.
        let len = unsafe { GetMenuStringW(hmenu, position, None, MF_BYPOSITION) };
        assert!(len >= 0, "there is no item at position {position} of the menu");
        if len == 0 {
            return String::new();
        }
        // `len + 1`, because the copy is NUL-terminated and `cchmax` counts
        // the terminator. The buffer is a named binding, so it outlives the
        // call that fills it -- which is the entire difference from `text()`.
        let mut buf = vec![0u16; len as usize + 1];
        let copied = unsafe { GetMenuStringW(hmenu, position, Some(&mut buf), MF_BYPOSITION) };
        assert_eq!(copied, len, "the label at position {position} was copied short");
        String::from_utf16_lossy(&buf[..copied as usize])
    }

    /// Every row of `menu`, in order, with separators spelled out.
    fn labels(menu: &Submenu) -> Vec<String> {
        menu.items()
            .iter()
            .enumerate()
            .map(|(i, item)| match item.as_menuitem() {
                Some(_) => label_at(menu, i),
                None => "<separator>".to_string(),
            })
            .collect()
    }

    /// The position of the one row labelled `text`, or a panic naming what was
    /// there instead -- which is the report a `find` returning `None` owes.
    fn row(menu: &Submenu, text: &str) -> usize {
        let labels = labels(menu);
        labels
            .iter()
            .position(|l| l == text)
            .unwrap_or_else(|| panic!("no item labelled {text:?}; the menu holds {labels:?}"))
    }

    /// `text`'s row, as a `MenuItem`. `is_enabled` reads `MIIM_STATE` and takes
    /// no buffer, so it is sound to call; only `text()` is not.
    fn item_at(menu: &Submenu, position: usize) -> MenuItem {
        menu.items()
            .into_iter()
            .nth(position)
            .and_then(|item| item.as_menuitem().cloned())
            .unwrap_or_else(|| panic!("position {position} is not a menu item"))
    }

    // Two ids, spelled out rather than generated, so the assertions below name
    // a specific account and not "whichever one came back".
    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";
    const C: &str = "00112233445566778899aabbccddeeff";

    fn id(raw: &str) -> AccountId {
        AccountId::parse(raw).expect("a 32-char hex id")
    }

    fn account(raw: &str, email: &str) -> Account {
        Account {
            id: id(raw),
            email: email.to_string(),
            server_url: None,
        }
    }

    /// Built through `AccountsState`'s own test constructor, which takes the
    /// one `Option<String>` `new` distils the CLI's availability into and
    /// computes `switchable` through the same
    /// `switch_targets`. A hand-built state here would be a second idea of
    /// what the tray is allowed to offer, which is the entire thing the one
    /// door exists to prevent.
    fn state(accounts: Vec<Account>, active: &str, blocked: Option<&str>) -> AccountsState {
        AccountsState::from_blocked_reason(accounts, id(active), blocked.map(str::to_string))
            .expect("a non-empty account list")
    }

    fn one_account() -> AccountsState {
        state(vec![account(A, "solo@example.com")], A, None)
    }

    fn two_accounts() -> AccountsState {
        state(
            vec![account(A, "one@example.com"), account(B, "two@example.com")],
            A,
            None,
        )
    }

    fn blocked_two_accounts() -> AccountsState {
        state(
            vec![account(A, "one@example.com"), account(B, "two@example.com")],
            A,
            Some("this bw.exe stores its data beside itself (relativeDataDir)"),
        )
    }

    /// The plan's own test. A submenu whose ids map to the wrong account
    /// switches the user to somewhere they did not ask for, and "Add
    /// account..." mistaken for an account switches them to nothing at all.
    #[test]
    fn a_menu_id_maps_back_to_the_account_it_was_built_for() {
        let menu = AccountsMenu::from_entries(
            vec![
                (MenuId::new("m1"), id(A)),
                (MenuId::new("m2"), id(B)),
            ],
            Some(MenuId::new("add")),
            Some(MenuId::new("remove")),
        );

        assert_eq!(menu.account_for_menu_id(&MenuId::new("m2")), Some(&id(B)));
        assert_eq!(menu.account_for_menu_id(&MenuId::new("m1")), Some(&id(A)));
        assert_eq!(
            menu.account_for_menu_id(&MenuId::new("add")),
            None,
            "\"Add account...\" was mistaken for an account to switch to"
        );
        assert_eq!(
            menu.account_for_menu_id(&MenuId::new("remove")),
            None,
            "the removal item was mistaken for an account to switch to"
        );
        assert_eq!(menu.account_for_menu_id(&MenuId::new("nope")), None);

        assert!(menu.is_add(&MenuId::new("add")));
        assert!(!menu.is_add(&MenuId::new("remove")));
        assert!(menu.is_remove(&MenuId::new("remove")));
        assert!(!menu.is_remove(&MenuId::new("m1")));
        assert!(menu.owns(&MenuId::new("m1")) && menu.owns(&MenuId::new("add")));
        assert!(
            !menu.owns(&MenuId::new("nope")),
            "the submenu claimed a click that belongs to another menu item"
        );
    }

    /// An item that was never built has no id, so a menu that is not offering
    /// an add cannot be talked into one by a stale or forged `MenuId`.
    #[test]
    fn a_menu_with_no_add_item_answers_no_to_every_id() {
        let absent = AccountsMenu::from_entries(vec![(MenuId::new("m1"), id(A))], None, None);
        assert!(!absent.is_add(&MenuId::new("add")));
        assert!(!absent.is_remove(&MenuId::new("remove")));
        // Positive control on the same two predicates, so the negatives above
        // are about the absent item and not about a predicate that never says
        // yes to anything.
        let present = AccountsMenu::from_entries(
            vec![(MenuId::new("m1"), id(A))],
            Some(MenuId::new("add")),
            Some(MenuId::new("remove")),
        );
        assert!(present.is_add(&MenuId::new("add")));
        assert!(present.is_remove(&MenuId::new("remove")));
    }

    /// **The `relativeDataDir` refusal.** A tray that offered a switch under it
    /// would point the CLI at a directory it ignores -- every account sharing
    /// one profile -- and the user would watch the switch "work" and change
    /// nothing.
    #[test]
    fn a_blocked_state_offers_no_switch_and_no_add() {
        let blocked = accounts_menu_plan(Some(&blocked_two_accounts()));
        assert!(
            blocked.switch_to.is_empty(),
            "the tray offered a switch the CLI would ignore"
        );
        assert!(
            !blocked.add,
            "the tray offered to add an account that would share the one profile"
        );
        assert_eq!(
            blocked.remove, None,
            "the tray offered to delete a profile it cannot reach"
        );

        // Positive control on the same helper and the same two accounts: it is
        // the block that empties the menu, not a plan that draws nothing.
        let available = accounts_menu_plan(Some(&two_accounts()));
        assert_eq!(
            available.switch_to.len(),
            1,
            "only the non-active account is a switch target"
        );
        assert_eq!(available.switch_to[0].0, id(B));
        assert!(available.add);
        assert!(available.remove.is_some());
    }

    /// The blocked reason outranks "you have one account": a blocked state can
    /// hold several accounts, and telling the user they have one would be
    /// false as well as unactionable. The reason names the directory they can
    /// go and act on.
    #[test]
    fn a_blocked_state_says_why_rather_than_claiming_there_is_one_account() {
        let plan = accounts_menu_plan(Some(&blocked_two_accounts()));
        let notice = plan.notice.expect("a blocked submenu must say something");
        assert!(
            notice.contains("relativeDataDir"),
            "the submenu does not name what is wrong: {notice:?}"
        );
        assert_ne!(
            notice, "No other accounts yet",
            "a blocked state with two accounts was reported as a single-account one, which is \
             both false and unactionable"
        );
    }

    /// The literal is read here and the constant is named only in the failure
    /// message: a test that re-derived its expectation from `NO_OTHER_ACCOUNTS`
    /// would pass against `NO_OTHER_ACCOUNTS = ""`, which is the exact defect
    /// it exists to catch -- an empty submenu that reads as broken.
    #[test]
    fn a_lone_account_is_told_so_rather_than_shown_an_empty_submenu() {
        let plan = accounts_menu_plan(Some(&one_account()));
        assert!(plan.switch_to.is_empty(), "there is nowhere to switch to");
        assert_eq!(
            plan.notice.as_deref(),
            Some("No other accounts yet"),
            "an empty submenu reads as a broken menu; NO_OTHER_ACCOUNTS is what fills it"
        );
        assert!(
            plan.add,
            "the one thing a single-account user can do here is add a second"
        );
    }

    /// **The menu must not offer an action that can only fail.**
    /// `remove_account` refuses the last account outright -- there is no
    /// profile to point the CLI at afterwards -- so the item is not built.
    #[test]
    fn the_last_account_is_not_offered_for_removal() {
        assert_eq!(
            accounts_menu_plan(Some(&one_account())).remove,
            None,
            "the tray offered a removal `remove_account` refuses outright"
        );
        // Positive control: the same field, non-`None` the moment there is a
        // survivor to settle onto, and naming the account that would go.
        let removable = accounts_menu_plan(Some(&two_accounts()))
            .remove
            .expect("with two accounts there is a survivor to land on");
        assert!(
            removable.contains("one@example.com"),
            "the removal item does not say which account it would delete: {removable:?}"
        );
    }

    /// An account with no email yet -- minted by `resolve_startup` on a first
    /// install, or by `prepare_new_account` before its sign-in lands -- is a
    /// blank strip of menu the user is invited to click.
    #[test]
    fn an_account_with_no_address_is_still_named_in_the_menu() {
        let plan = accounts_menu_plan(Some(&state(
            vec![account(A, "one@example.com"), account(B, "")],
            A,
            None,
        )));
        assert_eq!(plan.switch_to.len(), 1);
        assert_eq!(
            plan.switch_to[0].1, B,
            "an account with no address is offered as an empty row"
        );
    }

    /// `StartupAccounts::NoAccountList`: there is no `Account` in existence, so
    /// `main` has no `AccountsState` to hand over. The submenu still says
    /// something -- a control that vanishes is one the user concludes they
    /// imagined -- and offers nothing that would act on an account list that
    /// is not there.
    #[test]
    fn with_no_accounts_state_the_menu_explains_itself_and_offers_nothing() {
        let plan = accounts_menu_plan(None);
        assert!(plan.switch_to.is_empty());
        assert!(!plan.add);
        assert_eq!(plan.remove, None);
        assert_eq!(plan.active, None);
        let notice = plan.notice.expect("an empty submenu reads as a broken one");
        assert!(
            notice.contains("not set up"),
            "the submenu says nothing a user could act on: {notice:?}"
        );
    }

    /// **The construction, not just the decision.** Every test above drives a
    /// plan, and all of them would go on passing against a
    /// `build_accounts_submenu` that appended nothing -- which is precisely
    /// the "decision correct, renderer inert" shape this feature has shipped
    /// before. A bare `Submenu` needs no tray icon, so the real items are
    /// built and read back.
    #[test]
    fn the_built_submenu_carries_the_plans_rows_and_the_ids_they_were_built_for() {
        let submenu = Submenu::new(ACCOUNTS_SUBMENU, true);
        let plan = accounts_menu_plan(Some(&two_accounts()));
        let menu = build_accounts_submenu(&submenu, &plan);

        let labels = labels(&submenu);
        assert!(
            labels.iter().any(|l| l.contains("one@example.com")),
            "the submenu never says which account you are on: {labels:?}"
        );
        assert!(
            labels.iter().any(|l| l == "two@example.com"),
            "the switch target the plan chose was never built into a menu item: {labels:?}"
        );
        assert!(
            labels.iter().any(|l| l == ADD_ACCOUNT),
            "\"Add account...\" was planned and never built: {labels:?}"
        );
        assert!(
            labels.iter().any(|l| l.starts_with("Remove ")),
            "the removal was planned and never built: {labels:?}"
        );

        // ...and the ids the caller will get back name those same items.
        let switch_row = item_at(&submenu, row(&submenu, "two@example.com"));
        assert_eq!(
            menu.account_for_menu_id(switch_row.id()),
            Some(&id(B)),
            "clicking the row labelled `two@example.com` would not switch to that account"
        );
        let add_row = item_at(&submenu, row(&submenu, ADD_ACCOUNT));
        assert!(menu.is_add(add_row.id()));
        assert_eq!(
            menu.account_for_menu_id(add_row.id()),
            None,
            "the add row would be read back as an account to switch to"
        );
    }

    /// The account you are on is shown, and shown as unclickable: "switch to
    /// where you already are" still tears the backend down and demands a
    /// master password.
    #[test]
    fn the_header_and_the_notice_are_disabled_and_the_switch_rows_are_not() {
        let submenu = Submenu::new(ACCOUNTS_SUBMENU, true);
        build_accounts_submenu(&submenu, &accounts_menu_plan(Some(&two_accounts())));
        let enabled = |text: &str| item_at(&submenu, row(&submenu, text)).is_enabled();
        assert!(
            !enabled("Signed in: one@example.com"),
            "the account you are already on is offered as a switch target"
        );
        assert!(
            enabled("two@example.com"),
            "positive control: the switch row really is clickable"
        );

        let lone = Submenu::new(ACCOUNTS_SUBMENU, true);
        build_accounts_submenu(&lone, &accounts_menu_plan(Some(&one_account())));
        let notice = item_at(&lone, row(&lone, "No other accounts yet")).is_enabled();
        assert!(!notice, "the explanatory notice invites a click it cannot serve");
    }

    /// **A rebuild replaces, it does not append.** `MenuId`s are minted with
    /// their items, so a submenu that grew on every account change would leave
    /// the previous rebuild's rows in place -- rows whose ids still map, and
    /// which would switch the user to an account that may since have been
    /// deleted.
    #[test]
    fn rebuilding_the_submenu_replaces_the_previous_rows_and_their_ids() {
        let submenu = Submenu::new(ACCOUNTS_SUBMENU, true);
        let first = build_accounts_submenu(&submenu, &accounts_menu_plan(Some(&two_accounts())));
        let before = submenu.items().len();
        assert!(before > 2, "control: the first build really put rows in");
        let stale = first
            .account_for_menu_id(item_at(&submenu, row(&submenu, "two@example.com")).id())
            .cloned()
            .expect("the first build mapped that row to an account");
        assert_eq!(stale, id(B));

        // The user removed B and the app rebuilt.
        let second = build_accounts_submenu(&submenu, &accounts_menu_plan(Some(&one_account())));
        assert!(
            !labels(&submenu).iter().any(|l| l == "two@example.com"),
            "the removed account is still a clickable row in the submenu"
        );
        assert!(
            submenu.items().len() < before,
            "the submenu grew instead of being replaced: {} rows, was {before}",
            submenu.items().len()
        );
        assert_eq!(
            second.account_for_menu_id(&MenuId::new("whatever")),
            None,
            "control: the fresh map does not answer for ids it never minted"
        );
    }

    /// A third account is a third row: the loop is a loop, not a hard-coded
    /// pair. Pinned because every other test here uses one or two accounts,
    /// and a `switch_to` built from `switchable().first()` would pass all of
    /// them.
    #[test]
    fn every_switchable_account_gets_a_row() {
        let plan = accounts_menu_plan(Some(&state(
            vec![
                account(A, "one@example.com"),
                account(B, "two@example.com"),
                account(C, "three@example.com"),
            ],
            A,
            None,
        )));
        let labels: Vec<&str> = plan.switch_to.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(labels, vec!["two@example.com", "three@example.com"]);
    }

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
        // In a menu, as the real one is: an item that is in no menu answers
        // `text()` out of its own field, which would make this a test of a
        // `String` rather than of what the user's menu says.
        let (menu, item) = sync_item_in_a_menu();

        sync_item_to_in_progress(&item);
        assert_eq!(label_at(&menu, 0), "Syncing...");
        assert!(!item.is_enabled());

        sync_item_to_idle(&item);
        assert!(
            item.is_enabled(),
            "the whole point of the release is that the item is clickable again"
        );
        assert_eq!(
            label_at(&menu, 0),
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
        let (menu, item) = sync_item_in_a_menu();
        sync_item_to_in_progress(&item);
        sync_item_to_failed(&item);

        assert!(item.is_enabled());
        assert_eq!(label_at(&menu, 0), "Sync failed - click to retry");
    }

    /// A "Sync" item that really is a row of a menu, so its label can be read
    /// from the menu rather than from the item's own copy of it.
    fn sync_item_in_a_menu() -> (Submenu, MenuItem) {
        let menu = Submenu::new("Deskwarden", true);
        let item = MenuItem::new("Sync", true, None);
        menu.append(&item).expect("a fresh submenu accepts an item");
        (menu, item)
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

    /// **No code in this file may read a menu label through muda.**
    ///
    /// The fix for the intermittent failures documented on [`label_at`] was
    /// not to rewrite one assertion: it was to take the unsound call out of
    /// this file entirely. That only stays true if the next test written here
    /// cannot reach for it again -- and it is the obvious thing to reach for,
    /// because it reads like an accessor and it works nine hundred and
    /// ninety-odd times out of a thousand. So the ban is checked rather than
    /// remembered.
    ///
    /// The needles are split so that this test is not itself an instance of
    /// what it forbids.
    #[test]
    fn no_label_in_this_file_is_read_through_mudas_unsound_text() {
        let source = include_str!("tray.rs");
        for needle in [concat!(".text", "()"), concat!("MenuItem", "::text)")] {
            let offenders: Vec<(usize, &str)> = source
                .lines()
                .enumerate()
                .filter(|(_, line)| line.contains(needle))
                .map(|(n, line)| (n + 1, line.trim()))
                .collect();
            assert!(
                offenders.is_empty(),
                "{needle:?} is a use-after-free in muda 0.15.3 and returns a corrupted label \
                 under a parallel test run -- see `label_at`, which reads the same label \
                 soundly and by position. Offending lines: {offenders:?}"
            );
        }
    }

    /// **Nothing in this menu talks about updates, and nothing here may put it
    /// back.**
    ///
    /// The item that was removed was not removed for being redundant. It was
    /// removed because of what it *was*: `MenuItem::new("Update available",
    /// false, None)` -- the words baked in at build time, the check's only
    /// effect being to enable it -- so a session with no update (nearly every
    /// session) showed a permanent claim that there was one, on a control that
    /// then refused the click it invited. A tray menu cannot fix that, because
    /// it has nowhere to say "no update" and nowhere to put the release notes
    /// or a progress bar; the About page can, and does.
    ///
    /// A source-text guard rather than a menu walk, deliberately. `build_tray`
    /// creates a real `TrayIcon`, which needs a window and a message pump and
    /// so cannot be built in a test at all -- which is exactly how the defect
    /// survived: nothing could look at that menu. This can, and it also
    /// catches the near-miss the menu walk would not, which is a
    /// `set_update_*` helper reintroduced with no item appended yet.
    #[test]
    fn the_tray_menu_makes_no_claim_about_updates() {
        // Comments stripped, because the ones explaining WHY the item is gone
        // necessarily quote it -- including this test's own doc. What is being
        // pinned is the code.
        let source: String = include_str!("tray.rs")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for needle in [
            concat!("Update", " available"),
            concat!("update_", "item"),
            concat!("set_update_", "available"),
            concat!("Downloading ", "update"),
        ] {
            assert!(
                !source.contains(needle),
                "{needle:?} is back in tray.rs. The update flow lives on Preferences > About \
                 (`prefs_ui::draw_update_card`), which can say \"no update\" as easily as \
                 \"update\", can show the release notes, and can report a download to the page \
                 that started it. A tray menu item can do none of those, and the one that was \
                 here spent nearly every session asserting an update nobody had."
            );
        }
    }
}

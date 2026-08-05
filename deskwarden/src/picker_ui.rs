use crate::app_match::{AppMatch, TriggerMode};
use crate::bw_serve::{readiness_schedule, wait_for_vault_ready, BACKEND_OP_TIMEOUT};
use crate::icon;
use crate::loading_ui;
use crate::theme;
use crate::vault_bridge::{ItemKind, VaultError, VaultItem};
use crate::vault_cache::{AppMatchWrite, PopulateOutcome, VaultCache, VaultEra, VaultUnavailable};
use crate::window_list::{self, WindowInfo};
use crate::window_watch;
use eframe::egui::{self, CornerRadius, Margin, RichText, Sense, Stroke};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use windows::core::HSTRING;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, SystemParametersInfoW, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SPI_GETWORKAREA,
};

/// The position for a picker window's top-left corner that centers it on the
/// primary monitor's work area (excludes the taskbar). These are plain
/// standalone dialogs with no associated target window to center against, so
/// unlike the autofill overlay (`app::overlay_position`) there's no better
/// anchor than the screen itself -- but they still need an *explicit* one:
/// left to the OS default, eframe windows on this system open pinned near
/// the top of the screen rather than anywhere near where the user is
/// looking.
fn centered_position(width: f32, height: f32) -> [f32; 2] {
    let mut work_area = RECT::default();
    let got_work_area = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work_area as *mut RECT as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();

    if !got_work_area {
        return [200.0, 150.0];
    }

    let work_w = (work_area.right - work_area.left) as f32;
    let work_h = (work_area.bottom - work_area.top) as f32;
    [
        work_area.left as f32 + (work_w - width) / 2.0,
        work_area.top as f32 + (work_h - height) / 2.0,
    ]
}

/// Case-insensitive substring match of a vault item's name against an
/// already-lowercased filter. Takes the filter pre-lowered rather than
/// lowering it internally: callers filter an entire list against one filter
/// string every repaint, and with a vault in the thousands, lowering the
/// filter once outside the scan -- instead of once per item inside this
/// function -- is the difference between one allocation per frame and one
/// per vault item per frame.
///
/// Pure and separate from the UI so the search behaviour is testable without
/// opening a window.
pub fn item_matches_filter(item: &VaultItem, filter_lower: &str) -> bool {
    if filter_lower.is_empty() {
        return true;
    }
    item.name.to_lowercase().contains(filter_lower)
}

/// A design-2a list row: icon (or, absent one, an initials avatar), primary
/// line, muted secondary line, blue-washed when selected. Returns true when
/// clicked.
fn list_row(
    ui: &mut egui::Ui,
    primary: &str,
    secondary: &str,
    selected: bool,
    icon: Option<&egui::TextureHandle>,
) -> bool {
    let frame = egui::Frame::new()
        .fill(if selected {
            theme::BLUE_WASH
        } else {
            theme::CARD
        })
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match icon {
                    Some(tex) => {
                        ui.add(
                            egui::Image::new((tex.id(), tex.size_vec2()))
                                .fit_to_exact_size(egui::Vec2::splat(28.0)),
                        );
                    }
                    None => theme::avatar(ui, &theme::initials(primary), 28.0, selected),
                }
                ui.add_space(2.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(theme::semibold(primary, 13.0).color(if selected {
                        theme::BLUE_DEEP
                    } else {
                        theme::INK
                    }));
                    if !secondary.is_empty() {
                        ui.label(RichText::new(secondary).size(11.0).color(theme::TEXT_FAINT));
                    }
                });
            });
        });
    let response = frame.response.interact(Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// The window's title block: a small heading with a muted one-line
/// explanation underneath, matching the design's card headers.
fn title_block(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(theme::bold(title, 16.0).color(theme::INK));
    ui.label(RichText::new(subtitle).size(12.0).color(theme::TEXT_FAINT));
}

/// A full-width search field with the design's placeholder treatment.
fn search_field(ui: &mut egui::Ui, filter: &mut String, hint: &str) {
    // Text width, not box width: the margin sits outside `desired_width`,
    // so f32::INFINITY would overflow the parent by the margin (see
    // theme::text_field).
    let width = (ui.available_width() - 20.0).max(40.0);
    ui.add(
        egui::TextEdit::singleline(filter)
            .hint_text(RichText::new(hint).color(theme::TEXT_GHOST))
            .desired_width(width)
            .margin(Margin::symmetric(10, 8)),
    );
}

/// Estimated height (content + the 2px inter-row gap) of one [`list_row`].
/// Only needs to be close, not exact -- it drives [`egui::ScrollArea::show_rows`]'s
/// scroll-geometry estimate, not the rows' actual layout.
const LIST_ROW_HEIGHT: f32 = 48.0;

/// Named rather than inlined at the `run_ui_native` calls, because
/// `foreground::raise_window` finds each window BY its title -- one
/// declaration apiece means the two cannot drift apart.
const PICK_ITEM_TITLE: &str = "Choose a vault item";
const ADD_APP_TITLE: &str = "Add app to Deskwarden";

/// The white, hairline-bordered card that scrollable lists live in, showing
/// only the rows within the visible scroll range rather than laying out
/// and painting `row_count` rows on every repaint.
///
/// egui repaints on every keystroke *and* every mouse move over the window
/// (hover detection), so an unvirtualized list re-lays-out and re-paints
/// every one of its rows that often. That's fine for a few dozen rows; for a
/// vault with thousands of items it was the actual cause of the picker
/// feeling laggy while typing or moving the mouse over the list.
fn list_card(
    ui: &mut egui::Ui,
    height: f32,
    row_count: usize,
    mut add_row: impl FnMut(&mut egui::Ui, usize),
) {
    egui::Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .stroke(Stroke::new(1.0, theme::BORDER))
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .max_height(height)
                .auto_shrink([false, false])
                .show_rows(ui, LIST_ROW_HEIGHT, row_count, |ui, row_range| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for row in row_range {
                        add_row(ui, row);
                    }
                });
        });
}

/// The outcome of loading the picker's item list, kept as three distinct
/// terminal states rather than collapsing "nothing to show" into one shape
/// two different ways (review 10's Minor 5): a vault that genuinely has no
/// items and a vault that couldn't be reached both used to produce the same
/// empty `Vec`, so `pick_vault_item` had no way to tell a new user's empty
/// vault apart from a dead backend and told both of them to go check the
/// backend.
enum PickerItemsResult {
    /// At least one item to choose from.
    Items(Vec<VaultItem>),
    /// The cache (freshly populated, if it needed to be) is genuinely empty.
    EmptyVault,
    /// The vault holds items, but none of them are logins -- so the picker,
    /// which offers logins only (see `logins_only`), has nothing to list.
    ///
    /// Its own variant rather than a second use of `EmptyVault`, for exactly
    /// the reason `EmptyVault` and `BackendUnreachable` were split apart in
    /// the first place (review 10's Minor 5) and `VaultLocked` after them
    /// (review 14's Minor): the vault plainly *does* have items, so telling
    /// this user it "doesn't have any items yet" would be a fresh instance of
    /// the misdiagnosis those two changes removed from this very function.
    /// The remedy differs too -- "add an item" versus "add a *login*" -- and
    /// a message that names the wrong one sends the user somewhere that will
    /// not fix it.
    NoLogins,
    /// The cache was never populated and a fresh populate also failed --
    /// `bw serve` is unreachable, not merely idle.
    BackendUnreachable(String),
    /// The populate succeeded but was discarded: the vault was locked (or
    /// re-authenticated into a possibly different account) while it was in
    /// flight, so the cache is deliberately empty.
    ///
    /// Its own variant for the same reason `EmptyVault` and
    /// `BackendUnreachable` are each their own (review 14's Minor): with
    /// only `Ok`/`Err` to go on, this landed on `EmptyVault` and told a user
    /// whose vault had just locked that their vault "doesn't have any items
    /// yet" and to go add one.
    ///
    /// **Not reachable today, and recorded as such rather than claimed as a
    /// fix to live misbehaviour** (review 15's Minor 1). Every
    /// `VaultCache::clear` in the crate runs on the main thread, and both
    /// this picker's spinner and `vault_window::run` block that thread for
    /// their whole duration, so no `clear` can interleave with the picker's
    /// worker at all. Even if a leftover detached worker from an abandoned
    /// picker did produce this, its `rx` is already dropped, so the value
    /// never reaches the `MessageBoxW` in `pick_vault_item`. The typing is
    /// still the right call: the planned encrypted disk cache adds exactly
    /// the background-thread callers that make this reachable, and a future
    /// session should not credit it as verified-live before then.
    ///
    /// Since review 25's Minor 2 it is reported for THREE distinct ways the
    /// era can move, not one, and `load_items_for_picker` names each at its
    /// site: the populate itself coming back `DiscardedStale`; a `clear`
    /// after the click that makes the pre-populate read miss; and a `clear`
    /// after the click that a *successful* populate then refills for a later
    /// session. The last two are the ones the old two-lock spelling reported
    /// as `EmptyVault`.
    VaultLocked,
}

/// Whether `pick_vault_item`'s "Next" button should be clickable: only once
/// an item is selected.
///
/// Extracted as its own pure, directly-tested predicate for the same reason
/// `can_save_app_match` (below, `run_picker`'s equivalent gate) is: without
/// it, clicking "Next" with nothing selected was a silent no-op rather than
/// unreachable (review 11's Minor) -- the exact "enabled with nothing to do"
/// shape `can_save_app_match`'s own doc says was eliminated one window
/// later in the same "Add app..." flow.
fn can_pick_next(selected_id: &Option<String>) -> bool {
    selected_id.is_some()
}

/// The items the picker should list: the cache's own snapshot if it has one,
/// otherwise a live populate as a fallback.
///
/// `main`'s startup fills the cache once via `populate_with` and warns (but
/// otherwise continues) if that fails -- e.g. `list_folders` answering 500.
/// Before this, that left the cache permanently empty for the rest of the
/// session with no retry, so "Add app..." stayed inert even once `bw serve`
/// was healthy again: `pick_vault_item` read an empty `cache.items()` and
/// gave up. `fill_from_vault` has a bridge fallback for exactly this kind of
/// cache miss (`app.rs`'s `credentials_for` caller); this is the picker's
/// equivalent -- try to populate the cache (which needs `bw serve` up, same
/// as any other write/TOTP path) rather than trusting a snapshot that was
/// never actually taken.
///
/// Pure with respect to its one side effect (`cache.populate`) being the
/// thing under test, not UI -- exercised directly in this module's tests
/// against a mock `bw serve` via `VaultCache::new`, the same pattern
/// `vault_cache`'s own tests use. Called from a background thread (see
/// `pick_vault_item`), not the calling thread, so a slow or hung populate no
/// longer blocks the app before any window has even appeared (review 10's
/// Minor 4).
///
/// **`era` is the vault session the user's click belongs to**, captured by
/// `pick_vault_item` on the main thread before this is handed to that
/// detached worker, and it is what every read below is checked against --
/// review 25's Minor 2. This used to be `if !cache.is_populated() { .. }`
/// followed by `let items = cache.items();`: TWO locks, on a thread that is
/// explicitly not the main one, so a `clear` landing between them yielded an
/// empty `Vec` and the answer "your vault doesn't have any items yet" for a
/// vault that had just LOCKED -- precisely the misdiagnosis
/// [`PickerItemsResult::VaultLocked`] exists to prevent. It was sound only
/// while the main thread stayed parked in `loading_ui::show_while`, which is
/// an argument about thread affinity rather than about this code, and this
/// crate has had to un-write that argument more than once.
///
/// The era does strictly more than close the two-lock window, and that is
/// why it is a parameter rather than a capture taken here: a `clear` that
/// lands after the click but before this function even starts is invisible to
/// anything captured on entry, and the populate below would then succeed
/// against the NEW session (it takes its own, newer epoch) and hand this
/// click a list belonging to a vault session the user never asked about.
/// **The two refusals are handled differently, and that distinction lives in
/// the return type rather than being re-derived here** -- review 26's Minor 3.
/// While the checked read answered a bare `Option` (the `items_unless_superseded`
/// projection review 28's Important 1 deleted), a cleared cache
/// and a never-filled one were the same answer, so this function ran a full
/// vault populate -- seconds of HTTP under the spinner -- for a `VaultLocked`
/// that was already knowable, only to fail the re-check below. Correct answer,
/// pointless latency.
fn load_items_for_picker(cache: &VaultCache, era: VaultEra) -> PickerItemsResult {
    let items = match cache.snapshot_unless_superseded(era) {
        // Folders are read here and dropped: this door takes both halves
        // under one lock deliberately (see its own doc), and the picker
        // paying for a `Vec<Folder>` clone -- an id and a name apiece -- is
        // the price of not having a second, items-only era-checked read that
        // a folder-needing caller could later compose with `folders()`.
        Ok(snapshot) => snapshot.items,
        Err(VaultUnavailable::Superseded) => {
            // No fetch: a populate takes its OWN, newer epoch, so it would
            // refill the cache for the session that exists now and the
            // re-check below would still refuse. There is no vault for this
            // click's era and no request can produce one.
            log::warn!(
                "the vault was cleared between the picker's click and this read; there is no \
                 vault for that era and populating one cannot produce it"
            );
            return PickerItemsResult::VaultLocked;
        }
        Err(VaultUnavailable::Unpopulated) => {
            // NOT HANDLED HERE, AND SAYING SO BECAUSE THE ERA ABOVE MAKES
            // THIS SITE READ AS THOUGH IT WERE (review 26's Minor 4,
            // pre-existing). The read above proves the era was still this
            // click's AT THAT MOMENT; a `clear` landing between it and the
            // `populate()` below is invisible to it, and the populate then
            // captures its own, newer epoch and fills the cache with
            // `populated = true` under the NEW era -- work done for a session
            // this click does not serve. It is not reachable today only
            // because every `clear` site runs on the main thread, which is
            // parked in `loading_ui::show_while` while this detached worker
            // runs; that is an argument about thread affinity, not about this
            // code, and this crate has had to un-write that argument more
            // than once. What keeps it CORRECT rather than merely unreachable
            // is the re-check below, which refuses the refilled snapshot.
            log::warn!("no vault snapshot for the picker's era; populating it now");
            match cache.populate() {
                Ok(PopulateOutcome::Populated) => {}
                Ok(PopulateOutcome::DiscardedStale) => {
                    log::warn!(
                        "the vault was cleared while the picker's populate was in flight; the \
                         cache is deliberately empty"
                    );
                    return PickerItemsResult::VaultLocked;
                }
                Err(e) => {
                    log::error!("could not populate the vault cache for the picker: {e:?}");
                    return PickerItemsResult::BackendUnreachable(format!("{e:?}"));
                }
            }
            // A populate that *succeeded* still proves nothing about this
            // click's era: it captured its own epoch, so it happily refills
            // the cache for whatever session is current now. Asking again
            // under the same era is what distinguishes "the vault is here"
            // from "a vault is here, but not yours".
            //
            // BOTH REFUSALS COLLAPSE HERE, ON PURPOSE, AND THIS IS THE ONE
            // READ IN THE CRATE WHERE THAT IS SOUND -- recorded because it
            // reads like the collapse review 28 removed from
            // `settle_sync_outcome`, and it is not the same thing. The
            // populate above returned `Populated`, and the only thing that
            // can un-set `populated` afterwards is `clear`, which ALSO bumps
            // the era; so an `Unpopulated` refusal in this era is not
            // reachable from this point, and a `Superseded` one is exactly
            // what the log line below describes. They are also handled
            // identically -- `VaultLocked` -- so nothing downstream could act
            // on the difference even if it existed.
            //
            // It goes through `snapshot_unless_superseded` regardless (the
            // file's ONE checked door since review 28's Important 1 deleted
            // the items-only projection), and the discarded `folders` clone
            // is the same price this function already pays at its first read.
            match cache.snapshot_unless_superseded(era) {
                Ok(snapshot) => snapshot.items,
                Err(VaultUnavailable::Superseded | VaultUnavailable::Unpopulated) => {
                    log::warn!(
                        "the vault was cleared between the picker's click and its populate; the \
                         snapshot that now exists belongs to a later vault session"
                    );
                    return PickerItemsResult::VaultLocked;
                }
            }
        }
    };

    // Emptiness is judged BEFORE the login filter and non-emptiness after
    // it, so the three outcomes stay distinct: nothing in the vault at all,
    // things in the vault but nothing attachable, and something to list.
    // "There is no vault for this era at all" is a FOURTH state and was
    // handled above, as `VaultLocked` -- it must never arrive here as an
    // empty `Vec`.
    if items.is_empty() {
        return PickerItemsResult::EmptyVault;
    }
    let logins = logins_only(items);
    if logins.is_empty() {
        PickerItemsResult::NoLogins
    } else {
        PickerItemsResult::Items(logins)
    }
}

/// The picker offers only logins.
///
/// An app match on a secure note or a card is meaningless: `credentials_for`
/// would resolve an empty username and password, and the injector would type
/// two empty strings into the matched application. Filtering here rather
/// than at fill time means the user is never offered the choice.
///
/// Goes through `ItemKind` rather than testing `item_type == Some(1)`, so
/// this and the sidebar's `Logins` filter cannot drift apart about what
/// counts as a login -- including for an item whose `type` the server
/// omitted, which both treat as one.
fn logins_only(items: Vec<VaultItem>) -> Vec<VaultItem> {
    items
        .into_iter()
        .filter(|i| ItemKind::of(i) == ItemKind::Login)
        .collect()
}

/// Opens a blocking egui window listing the user's vault **logins** with a
/// search box, and returns the one they pick (or `None` if they cancel, or
/// there is nothing to show).
///
/// Logins only (`load_items_for_picker` -> `logins_only`): autofill resolves
/// exactly a username and a password, so an app matched to a card or a
/// secure note would have the injector type two empty strings into it. A
/// vault with items but no logins gets its own `NoLogins` message rather
/// than the empty-vault one.
///
/// This is step one of the tray's "Add app..." flow: `run_picker` needs a
/// specific `VaultItem` to attach a match to, and nothing previously chose
/// one, which is why "Add app..." was an inert menu entry and `run_picker` was
/// dead code in the bin target.
///
/// Reads [`VaultCache::items`] (via `load_items_for_picker`) rather than
/// listing via `VaultBridge` directly: with `keep_backend_running` off,
/// `bw serve` is normally stopped at idle, so a bridge call here failed with
/// a connection error every time -- logged, but with nothing on screen,
/// "Add app..." just appeared to do nothing. The cache holds the same data
/// in memory regardless of whether the backend is running (see
/// `vault_cache`'s module doc), so the common case of listing items for the
/// picker never needs to start it; `load_items_for_picker` only reaches for
/// the backend in the fallback case where the cache was never successfully
/// filled in the first place.
///
/// `load_items_for_picker` runs on a background thread while a loading
/// spinner is shown, rather than being called inline before any window
/// exists (review 10's Minor 4): a hung or slow-to-respond backend used to
/// leave "Add app..." looking completely inert -- no window, no feedback --
/// for as long as the populate attempt took.
///
/// Takes `&Arc<VaultCache>` rather than `&VaultCache` so the populate can run
/// on a fully detached thread (see below) rather than one `thread::scope`
/// has to join before this function can return.
pub fn pick_vault_item(cache: &Arc<VaultCache>) -> Option<VaultItem> {
    let (tx, rx) = mpsc::channel();
    // Detached (`std::thread::spawn`, not `thread::scope`'d), and handed its
    // own clone of the `Arc` rather than borrowing `cache` -- review 12's
    // Minor 5. A `thread::scope`d worker forces this function to block until
    // that worker actually finishes, even after `show_while` has already
    // returned `None` because the user closed the spinner: on the bridge
    // fallback (`load_items_for_picker`'s live `populate()`, up to ~13s) that
    // meant "quietly abandon" from the comment below was a lie -- the click
    // still froze the app, invisibly, for however long the populate had left
    // to run. Detaching means this function returns as soon as `show_while`
    // does; the worker simply finishes on its own and its result (nobody is
    // listening any more) is dropped.
    // Captured HERE, on the main thread, before the worker starts: it names
    // the vault session this click belongs to. Everything the worker goes on
    // to read is checked against it in one lock
    // (`VaultCache::snapshot_unless_superseded`), so a `clear` landing while the
    // worker runs -- the vault locking, or a re-auth into a possibly
    // different account -- cannot come back as this click's item list. See
    // `load_items_for_picker`.
    let era = cache.epoch().era();
    let cache_for_thread = cache.clone();
    std::thread::spawn(move || {
        let _ = tx.send(load_items_for_picker(&cache_for_thread, era));
    });
    let result = loading_ui::show_while("Loading your vault...", rx);

    // The user closed the spinner (title-bar X or Alt+F4) before the
    // populate finished, rather than waiting it out -- review 11's Critical.
    // Treated exactly like Cancel at the very next step: quietly abandon
    // "Add app..." for this click. There is nothing to fall back to (no
    // items were ever read), and re-panicking here would be the bug this
    // fix exists to remove.
    let Some(result) = result else {
        log::info!("\"Add app...\" cancelled: the vault-loading window was closed early");
        return None;
    };

    let items = match result {
        PickerItemsResult::Items(items) => items,
        PickerItemsResult::EmptyVault => {
            log::warn!("vault has no items to attach an app match to");
            unsafe {
                MessageBoxW(
                    None,
                    &HSTRING::from(
                        "Your Bitwarden vault doesn\u{2019}t have any items yet.\n\nAdd an item \
                         from the vault window, then use \u{201c}Add app\u{2026}\u{201d} again.",
                    ),
                    &HSTRING::from("Deskwarden: vault is empty"),
                    MB_ICONWARNING | MB_OK | MB_SETFOREGROUND,
                );
            }
            return None;
        }
        // Deliberately NOT folded into `EmptyVault` above -- see `NoLogins`'s
        // own doc. This user's vault is full; it just has nothing that can
        // usefully fill an application, and the message says which.
        PickerItemsResult::NoLogins => {
            log::warn!("vault has items but no logins to attach an app match to");
            unsafe {
                MessageBoxW(
                    None,
                    &HSTRING::from(
                        "\u{201c}Add app\u{2026}\u{201d} can only fill from a login, and your \
                         Bitwarden vault doesn\u{2019}t have any login items yet \u{2014} only \
                         other kinds, like secure notes or cards.\n\nAdd a login from the vault \
                         window, then use \u{201c}Add app\u{2026}\u{201d} again.",
                    ),
                    &HSTRING::from("Deskwarden: no logins to choose from"),
                    MB_ICONWARNING | MB_OK | MB_SETFOREGROUND,
                );
            }
            return None;
        }
        PickerItemsResult::BackendUnreachable(reason) => {
            log::warn!("could not load vault items for the picker: {reason}");
            unsafe {
                MessageBoxW(
                    None,
                    &HSTRING::from(format!(
                        "Deskwarden could not load your vault items because its Bitwarden \
                         backend isn\u{2019}t reachable right now.\n\nOpen the vault (which \
                         starts it) or try Sync from the tray menu, then use \u{201c}Add \
                         app\u{2026}\u{201d} again.\n\n{reason}"
                    )),
                    &HSTRING::from("Deskwarden: could not load vault"),
                    MB_ICONWARNING | MB_OK | MB_SETFOREGROUND,
                );
            }
            return None;
        }
        // Unreachable as the app is wired today -- see `VaultLocked`'s own
        // doc for why, and for why the arm exists anyway.
        PickerItemsResult::VaultLocked => {
            log::warn!("the vault locked while loading items for the picker");
            unsafe {
                MessageBoxW(
                    None,
                    &HSTRING::from(
                        "Deskwarden\u{2019}s vault was locked while it was loading your \
                         items.\n\nOpen the vault window and unlock it, then use \u{201c}Add \
                         app\u{2026}\u{201d} again.",
                    ),
                    &HSTRING::from("Deskwarden: vault is locked"),
                    MB_ICONWARNING | MB_OK | MB_SETFOREGROUND,
                );
            }
            return None;
        }
    };

    // Same Rc<RefCell<_>> pattern as `run_picker` below: the update closure is
    // FnMut + 'static and must move-capture its state, so the result is read
    // back through a shared cell once the blocking call returns.
    let result: Rc<RefCell<Option<VaultItem>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();

    let mut filter = String::new();
    let mut selected_id: Option<String> = None;
    let mut styled = false;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 540.0])
            .with_position(centered_position(440.0, 540.0))
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native(PICK_ITEM_TITLE, options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
            // The OS window exists by this first painted frame, and this is
            // where it is brought to the front. See `foreground`: a refusal
            // from Windows flashes the taskbar button rather than being
            // ignored.
            crate::foreground::raise_window(PICK_ITEM_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::CANVAS)
                    .inner_margin(Margin::symmetric(20, 18)),
            )
            .show(ui, |ui| {
                let mut done = false;

                theme::card_header(ui, "Add app");
                ui.add_space(10.0);
                title_block(
                    ui,
                    "Which vault item should this app fill from?",
                    "Create an item that fills here from now on.",
                );
                ui.add_space(8.0);
                search_field(ui, &mut filter, "Search vault");
                ui.add_space(8.0);

                // Filter lowered once per frame, not once per item inside
                // item_matches_filter (see its doc comment) -- this scan is
                // still O(items), but a cheap one now.
                let filter_lower = filter.to_lowercase();
                let filtered: Vec<usize> = (0..items.len())
                    .filter(|&i| item_matches_filter(&items[i], &filter_lower))
                    .collect();

                // Clamped: the subtrahend is the space reserved for the
                // buttons below, and a window resized smaller than that would
                // otherwise ask for a negative scroll-area height.
                list_card(
                    ui,
                    (ui.available_height() - 56.0).max(0.0),
                    filtered.len(),
                    |ui, row| {
                        let item = &items[filtered[row]];
                        let selected = selected_id.as_deref() == Some(item.id.as_str());
                        let username = item
                            .login
                            .as_ref()
                            .and_then(|l| l.username.clone())
                            .unwrap_or_default();
                        if list_row(ui, &item.name, &username, selected, None) {
                            selected_id = Some(item.id.clone());
                        }
                    },
                );

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    // Review 11's Minor: unclickable with nothing selected,
                    // rather than a silent no-op -- the same "enabled with
                    // nothing to do" shape `run_picker`'s own Save gate
                    // (`can_save_app_match`) exists to avoid, one window
                    // earlier in the same "Add app..." flow.
                    let next_clicked = ui
                        .add_enabled_ui(can_pick_next(&selected_id), |ui| {
                            theme::primary_button(ui, "Next", None)
                        })
                        .inner
                        .clicked();
                    if next_clicked {
                        if let Some(id) = &selected_id {
                            if let Some(item) = items.iter().find(|i| &i.id == id) {
                                *result_for_closure.borrow_mut() = Some(item.clone());
                                done = true;
                            }
                        }
                    }
                    if theme::secondary_button(ui, "Cancel").clicked() {
                        done = true;
                    }
                });

                if done {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
    });

    let chosen = result.borrow_mut().take();
    chosen
}

/// One entry of the trigger-mode segmented control: label plus the sentence
/// shown under the control while that mode is selected. The wording follows
/// the design's per-app "On focus" column (3e): the overlay list, hotkey
/// only, or filling straight away.
const TRIGGER_CHOICES: &[(TriggerMode, &str, &str)] = &[
    (
        TriggerMode::Prompt,
        "Prompt",
        "Show the overlay when this app is focused.",
    ),
    (
        TriggerMode::Hotkey,
        "Hotkey",
        "Fill only when the fill hotkey is pressed.",
    ),
    (
        TriggerMode::Auto,
        "Auto",
        "Fill immediately when this app is focused.",
    ),
];

/// The design's segmented pill group ("Below field | Above | At cursor"),
/// used here for the trigger mode.
fn trigger_segmented(ui: &mut egui::Ui, trigger: &mut TriggerMode) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (mode, label, _) in TRIGGER_CHOICES {
            let selected = trigger == mode;
            let button = egui::Button::new(theme::semibold(*label, 12.0).color(if selected {
                egui::Color32::WHITE
            } else {
                theme::INK
            }))
            .fill(if selected { theme::BLUE } else { theme::CARD })
            .stroke(if selected {
                Stroke::NONE
            } else {
                Stroke::new(1.0, theme::BORDER_STRONG)
            })
            .corner_radius(CornerRadius::same(7));
            if ui.add(button).clicked() {
                *trigger = *mode;
            }
        }
    });
    if let Some((_, _, caption)) = TRIGGER_CHOICES.iter().find(|(m, _, _)| m == trigger) {
        ui.label(RichText::new(*caption).size(11.0).color(theme::TEXT_FAINT));
    }
}

/// The picker's own view of whether `bw serve` can safely be written to
/// right now -- computed in exactly one place (the readiness thread spawned
/// at the top of `run_picker`, drained once per frame) and matched
/// exhaustively wherever it's rendered, the same discipline `TotpState` (see
/// `vault_window::detail`) established for the TOTP pane after *its* three
/// consecutive patches. Before this, "started" and "ready to answer" were
/// conflated: `try_start_backend` returning was treated as good enough, when
/// it only means the child process was resumed, not that its HTTP listener
/// (a Node cold start, routinely several seconds) has bound the port yet
/// (review 10's Important 2).
#[derive(Debug, Clone, PartialEq)]
enum BackendReadiness {
    /// `bw serve` is being started (or was already up and is being
    /// confirmed) and hasn't yet answered a real vault query.
    Preparing,
    /// Confirmed reachable: a `wait_for_vault_ready` probe succeeded.
    Ready,
    /// The readiness probe didn't succeed within its deadline.
    ///
    /// Not terminal (review 11's Important 2): a save-memory start can
    /// legitimately still be landing when the probe gives up (see
    /// `BACKEND_OP_TIMEOUT`'s doc for why), so this is rendered with a
    /// "Retry" button that spawns a fresh probe and returns to `Preparing`,
    /// rather than permanently disabling Save for the rest of this window's
    /// life over a start that may finish moments later.
    Unavailable(String),
}

/// Whether the picker's Save button should be clickable at all: a process
/// must be selected *and* the backend confirmed reachable.
///
/// Extracted into its own pure, directly-tested predicate (rather than an
/// `if let Some(pid) = selected_pid { ... }` guard with the actual save
/// logic nested inside it) because that nesting is exactly what let review
/// 10's Important 1 through: the save attempt was gated correctly, but the
/// window-closing `done = true` sat *outside* both the `if let` and the
/// match on the save result, so clicking Save with nothing selected closed
/// the window exactly like Cancel -- nothing saved, nothing logged. Gating
/// the button's own enabled state on this function makes that click
/// unreachable in the first place, rather than reachable-but-silently-inert.
///
/// Takes the selected window's **process name**, not its pid, because of the
/// third condition: a match whose process is a window host
/// ([`window_watch::is_host_process`]) is wrong by construction and must never
/// reach the vault. `ApplicationFrameHost.exe` owns the top-level window of
/// every Microsoft Store app, so an entry naming it does not mean "this app",
/// it means "every Store app" -- the reported bug, where a match saved for
/// KeepSolid popped the overlay on Speedtest. There is no trigger mode, no
/// narrowing and no later fix that makes such an entry correct, so the only
/// honest thing this window can do is decline to write it and say why (see
/// [`host_process_refusal`]).
/// The row the user has selected, found by **window handle**.
///
/// Not by pid, which is what this window used to carry the selection as. Every
/// `UnresolvedHost` row is listed under the host's pid (`window_list::enum_proc`
/// hands `owner_pid` to rows it could not attribute), so on the reporting
/// machine both Store frames -- titled "Speedtest" and "Settings" -- were pid
/// 12472, and a `find(|w| w.pid == pid)` could not tell them apart: it
/// answered with whichever came first while the highlight sat on the other.
/// Harmless *today*, because both are refused and refused for the same reason,
/// so the sentence beside the button is the same either way. It stops being
/// harmless the moment those two rows can differ -- which is one attribution
/// improvement away, since a frame whose CoreWindow becomes readable resolves
/// to a real app while its neighbour does not.
///
/// An `HWND` is unique among live windows, which is precisely the property
/// "the row the user clicked" needs and a pid does not have. The `default_pid`
/// this window opens on is still a pid -- it comes from a foreground event,
/// which has no row of its own -- so that one lookup stays by pid and its
/// answer's `hwnd` becomes the selection.
fn selected_window(windows: &[WindowInfo], selected: Option<isize>) -> Option<&WindowInfo> {
    let hwnd = selected?;
    windows.iter().find(|w| w.hwnd == hwnd)
}

/// The match that gets written for the row the user selected.
///
/// **Everything is copied off the live window, here, at the moment Save is
/// clicked -- nothing is resolved later.** That is not tidiness, it is the
/// only order that works: the title is what identifies a Microsoft Store app
/// once Windows suspends it, and by then the app has no `CoreWindow`, no
/// nameable process and no reachable image path (see
/// [`window_watch::attribute_window`]). A design that stored the process name
/// and looked the rest up on demand would be able to answer only for apps that
/// never needed it.
///
/// All three come from the SAME `WindowInfo`, which is what makes them
/// consistent with each other: `window_list::enum_proc` builds the row's
/// `exe_name` and `exe_path` from the one attributed pid, so the path really
/// is the image of the process being named -- the invariant
/// [`AppMatch::launchable_path`] re-checks before anything runs it.
///
/// **The title is recorded only for a HOSTED row** -- one whose top-level
/// window an `ApplicationFrameHost.exe` frame owned while a real app resolved
/// inside it (`WindowInfo::hosted`). Review 31's Important 1: it used to be
/// recorded for every row, on the reasoning that `MatchEngine::lookup` only
/// consults titles for host-owned windows anyway. That reasoning was about who
/// may READ the table and said nothing about what is IN it -- and what was in
/// it was every saved app's title, so a Store app whose title follows its
/// content could wear an ordinary desktop app's title and claim its match. The
/// row already knows which kind it is, so the fix is to record the truth rather
/// than to filter for it on every lookup.
///
/// The flag is stored beside the title rather than being re-derived later,
/// because it cannot be re-derived: `process` alone cannot say whether the
/// window it came off was inside a frame.
fn app_match_for(w: &WindowInfo, trigger: TriggerMode) -> AppMatch {
    AppMatch {
        process: w.exe_name.clone(),
        title: if w.hosted { w.title.clone() } else { String::new() },
        hosted: w.hosted,
        path: w.exe_path.clone(),
        // **Empty, and deliberately not offered here.** There is nothing to
        // copy off a live window: `AppMatch::args` is what the user wants the
        // app started WITH, not what this one happened to be started with, and
        // the two are different questions (a browser's running command line
        // carries session and crash-recovery switches that would be nonsense to
        // replay). The edit form is where arguments are typed -- see
        // `vault_window::detail_edit`.
        args: String::new(),
        sequence: String::new(),
        trigger,
    }
}

fn can_save_app_match(selected_process: Option<&str>, backend_ready: &BackendReadiness) -> bool {
    let Some(process) = selected_process else {
        return false;
    };
    host_process_refusal(process).is_none() && *backend_ready == BackendReadiness::Ready
}

/// Why a match against `process` must not be saved, or `None` when it may be.
///
/// `Option<String>` rather than a `bool` so the reason and the refusal cannot
/// drift apart: the same call that disables Save produces the sentence shown
/// beside it, so "refused" can never be rendered without an explanation. The
/// silent no-op is the failure mode this window has already been patched for
/// twice (see [`can_save_app_match`]'s own doc).
///
/// **The remedy it names was measured -- but it is not the only cause, and
/// this no longer claims it is.** A row can still be carrying a host's name in
/// exactly the two cases [`window_watch::attribute_window`] falls through to
/// `UnresolvedHost`:
///
///  * The frame had **no** `Windows.UI.Core.CoreWindow` child at all. That is
///    what the reporting machine had: both open Store windows were minimised,
///    and a minimised UWP app is suspended with its CoreWindow gone. Restoring
///    the app brings the child back, and the row then lists under the app's
///    own name.
///  * The child was there but **could not be opened or named** --
///    `attribute_window`'s `child.exe_name` was `None`. A Store app running at
///    higher integrity than Deskwarden does that, and restoring it changes
///    nothing whatever.
///
/// The first wording stated the first cause as certain, which made "restore
/// the app and open Add app... again" advice that can never work for anyone in
/// the second -- stated as fact, so a user in that case is told to keep
/// retrying something that cannot succeed. This window cannot tell the two
/// apart from a process name, so it names the likely one as likely and says
/// what the other looks like: a row that still shows the host after the app is
/// on screen.
fn host_process_refusal(process: &str) -> Option<String> {
    if !window_watch::is_host_process(process) {
        return None;
    }
    Some(format!(
        "{process} isn\u{2019}t an app -- it\u{2019}s the Windows process that owns the window for \
         every Microsoft Store app, so matching it would fill this item into all of them. \
         Deskwarden won\u{2019}t save that. This row is showing the host because Deskwarden \
         couldn\u{2019}t see which app is inside it. Usually that\u{2019}s because the app is \
         minimised (Windows suspends it): restore the app so it\u{2019}s on screen, then open \
         \u{201c}Add app\u{2026}\u{201d} again and it should be listed under its own name. If it \
         still shows as {process} with the app on screen, Windows is keeping that app out of \
         Deskwarden\u{2019}s reach and this item can\u{2019}t be matched to it here."
    ))
}

/// Shown when the item this picker is targeting **already** carries a match
/// against a window host -- one saved before the foreground watcher learned to
/// look through `ApplicationFrameHost.exe`.
///
/// This is the chosen surface for the bad data already in the user's vault.
/// `MatchEngine::rebuild` has stopped acting on such an entry, so the wrong
/// pop-ups have stopped, but nothing had told the user *why* their match went
/// quiet -- and this window is the one place where saying so is also the place
/// they fix it, since Saving here overwrites the field. **Nothing is rewritten
/// on their behalf:** the custom field stays exactly as they saved it until
/// they choose a real app and click Save.
fn existing_host_match_notice(item_name: &str, process: &str) -> String {
    format!(
        "\u{201c}{item_name}\u{201d} is currently matched to {process}, which Deskwarden is \
         ignoring: that process owns the window for every Microsoft Store app, so the match fired \
         on all of them rather than on the one you meant. Nothing in your vault has been changed. \
         Pick the app below and Save to replace it."
    )
}

/// The gate the button actually uses: [`can_save_app_match`] plus "this
/// window has not already saved something".
///
/// Added by review 28's Important 2, which gave [`AppMatchWrite::ServerOnly`]
/// a user-visible surface. That surface keeps the window OPEN with a notice
/// instead of closing it, which is the first state in this window's life
/// where a save has succeeded and the window still exists -- and a second
/// Save click from there would re-PUT a match the server already has.
///
/// A separate function rather than an `&& !already_saved` at the call site so
/// it is testable at all: everything else in that closure is unreachable
/// outside a real event loop.
fn can_save_app_match_now(
    selected_process: Option<&str>,
    backend_ready: &BackendReadiness,
    already_saved: bool,
) -> bool {
    !already_saved && can_save_app_match(selected_process, backend_ready)
}

/// What the user is told, inline and on screen, when the save reached the
/// server but not the snapshot ([`AppMatchWrite::ServerOnly`]).
///
/// Review 28's Important 2: this state had no surface at all. `run_picker`
/// returned `Some(m)`, `main` logged a warn, and the user was shown a window
/// that simply closed -- the same thing a fully live save looks like -- while
/// the match was invisible to everything reading the cache.
///
/// **The wording deliberately does not promise the next sync will fix it**,
/// which is what every copy of this message used to say. That promise holds
/// only for the unpopulated miss. The reachable path for the other one --
/// the id being absent from a populated snapshot -- needs the item to have
/// stopped existing after the PUT was accepted (see
/// [`crate::vault_cache::AppMatchWrite::ServerOnly`]'s own doc), and no sync
/// can bring a deleted item's match back. So this says what is TRUE for both
/// -- the vault has it, this app's autofill may not yet -- and names the one
/// action that is always right.
fn server_only_notice(item_name: &str) -> String {
    format!(
        "Your match was saved to \u{201c}{item_name}\u{201d} in your vault, but Deskwarden \
         couldn\u{2019}t make it live in this session -- so autofill may not use it yet. Reopen \
         your vault (or restart Deskwarden) and check the item; if the match isn\u{2019}t there, \
         the item was changed or removed elsewhere while you were saving."
    )
}

/// The inline message shown for a failed save attempt (review 10's
/// Important 1's sibling bug, and the redesign's item 2): stays inline,
/// under the Save/Cancel row, rather than a `MessageBoxW` -- this app has its
/// own design language throughout (see `vault_window/folder_modal.rs`'s
/// identical `state.error` pattern) and a native modal is a foreign element
/// in it. Distinguishes `Unauthorized` (needs a fresh sign-in, not a retry)
/// from every other failure the same way the vault window's own write paths
/// already do.
fn save_error_message(e: &VaultError) -> String {
    match e {
        VaultError::Unauthorized => "Your Bitwarden session has expired, so this wasn\u{2019}t \
             saved. Open the vault to sign in again, then try Save once more."
            .to_string(),
        other => format!(
            "This app match couldn\u{2019}t be saved ({other:?}). Check that Deskwarden\u{2019}s \
             Bitwarden backend is reachable, then try Save again."
        ),
    }
}

/// Opens a blocking egui window that lets the user search open windows, pick
/// one, choose a trigger mode, and save the resulting `AppMatch` onto
/// `target_item` via `cache.set_app_match`.
///
/// Returns `Some(AppMatch)` if the user clicked Save and the vault write
/// succeeded, or `None` if the user cancelled. A failed save no longer
/// returns `None` on its own (see the redesign note below) -- the window
/// stays open with the selection intact and an inline error, and the user
/// either retries Save or explicitly cancels.
///
/// Takes `Arc<VaultCache>` rather than a bare `VaultBridge` reference so the
/// save updates the shared snapshot, not just the server: reads (the vault
/// window's item list, and now autofill's `fill_from_vault`) are served from
/// `VaultCache`, so writing straight to the bridge here would leave the
/// snapshot holding the pre-match copy of `target_item`. The next edit made
/// to that item from the vault window -- before any refresh -- would then
/// read the stale cached copy, apply its change on top of it, and PUT that
/// whole object back, silently deleting the app-match field this function
/// just told the server to save. See `VaultCache::set_app_match`'s own doc
/// comment for the general form of this hazard.
///
/// Takes ownership of `cache` and `target_item` (rather than borrowing)
/// because `eframe::run_simple_native`'s update closure is `FnMut + 'static`
/// and must `move`-capture everything it uses; callers clone the `Arc` and
/// the `VaultItem` before calling this.
///
/// `default_pid` is the process id of whatever window was active right
/// before "Add app..." was invoked (see `main`'s `last_active_pid`
/// tracking), if any. When it's still in the window list, the picker opens
/// with it pre-selected *and* the search box pre-filled with its name -- the
/// common case (matching the app you were just using) needs no typing at
/// all, while the search box stays live to pick something else.
///
/// `backend_already_running` is whatever `main` observed *before* it (maybe)
/// kicked off a start for this flow -- same `backend_already_running`
/// exemption `open_vault_window`/`vault_window::run` already make for their
/// own readiness waits. When true, `Save` is immediately clickable (default
/// mode, where `bw serve` is normally already up, is unaffected down to the
/// frame -- no wait is even started). When false, a background thread polls
/// `wait_for_vault_ready` and the window renders `BackendReadiness::Preparing`
/// until it reports back, without blocking this thread or freezing the
/// window: this is the fix for review 10's Important 2/3 -- "Add app..."
/// needed a backend that was actually answering, not merely mid-start, and
/// nothing previously waited for that distinction before letting Save fire.
/// This probe doesn't care who is starting `bw serve` (this flow's own kick,
/// a concurrent tray Sync, or it was simply already up) -- it only waits for
/// the port to answer, so it self-heals regardless of which of those it was.
///
/// Uses `BACKEND_OP_TIMEOUT` (90s) as the probe's deadline, not the shorter
/// `READINESS_DEADLINE` (30s) `wait_for_vault_ready_with_spinner` uses at
/// startup -- review 11's Important 2. A save-memory start goes through
/// `wait_for_port_free` (up to 30s) *then* an unbounded `bw sync` *then*
/// node's own cold start (10-20s more) before it can answer at all, which is
/// exactly why `BACKEND_OP_TIMEOUT` is 90s in the first place; a 30s probe
/// here used to report `Unavailable` for a start that was still healthy and
/// ~20s from landing.
fn spawn_readiness_probe(cache: &VaultCache) -> mpsc::Receiver<Result<(), String>> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let vault = cache.bridge().clone();
    std::thread::spawn(move || {
        let schedule = readiness_schedule(BACKEND_OP_TIMEOUT);
        let result = wait_for_vault_ready(&vault, &schedule).map(|_items| ());
        let _ = ready_tx.send(result);
    });
    ready_rx
}

/// What [`run_picker`] hands back on a successful Save: the match the user
/// built **and** how far it actually got.
///
/// A struct rather than a bare [`AppMatch`] because of review 28's Important
/// 2. The caller's next act is to rebuild the match engine, and for
/// [`AppMatchWrite::ServerOnly`] the snapshot it would rebuild from does not
/// hold this match -- so `Some(AppMatch)` alone was an answer the caller
/// could not act on correctly, and it silently armed an engine without the
/// thing the user had just spent two windows creating.
///
/// `None` still means, and only means, "the user cancelled". A save that
/// reached the server is `Some` whichever variant it carries: reporting it as
/// a cancellation would be a different and false statement.
pub struct SavedAppMatch {
    pub app_match: AppMatch,
    pub write: AppMatchWrite,
}

pub fn run_picker(
    cache: Arc<VaultCache>,
    target_item: VaultItem,
    default_pid: Option<u32>,
    backend_already_running: bool,
) -> Option<SavedAppMatch> {
    let windows: Vec<WindowInfo> = window_list::list_windows(std::process::id());

    // The update closure must `move`-capture its state (it's FnMut + 'static
    // and runs on every repaint), so a plain local `Option<AppMatch>` can't be
    // read back by this function after `run_simple_native` returns. Instead,
    // the result lives in an `Rc<RefCell<_>>`: a clone is moved into the
    // closure, and the original is read here once the (blocking) call
    // returns. This is safe because eframe runs the closure on the same
    // thread that's blocked inside `run_simple_native` -- there's no
    // cross-thread sharing happening.
    let result: Rc<RefCell<Option<SavedAppMatch>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();

    let default_window = default_pid.and_then(|pid| windows.iter().find(|w| w.pid == pid));
    let mut filter = default_window.map(|w| w.exe_name.clone()).unwrap_or_default();
    // The WINDOW, not its pid -- see `selected_window`. `default_pid` is a
    // foreground event's pid and has no row of its own, so it is resolved to a
    // row once, here, and it is that row's handle the window carries.
    let mut selected_hwnd: Option<isize> = default_window.map(|w| w.hwnd);
    let mut trigger = TriggerMode::Prompt;
    let mut styled = false;

    // Read once, before the loop: the match this item already carries, if it
    // names a window host. See `existing_host_match_notice` for why this is
    // surfaced here and why nothing is rewritten. `None` for the overwhelming
    // majority of items, including every item with a perfectly good match.
    let existing_host_match: Option<String> = crate::vault_bridge::extract_app_match(&target_item)
        .map(|m| m.process)
        .filter(|process| window_watch::is_host_process(process));

    // Icon textures are loaded lazily, one GDI round-trip and one GPU upload
    // per distinct exe the *visible* rows actually need, not eagerly for
    // every window in the list -- with a couple hundred windows open,
    // extracting every icon up front would make the picker visibly slow to
    // open. A `None` cache entry means extraction was already tried and
    // failed (no icon on the file, or a GDI call errored), so a row without
    // an icon doesn't retry every single frame.
    let mut icon_cache: HashMap<String, Option<egui::TextureHandle>> = HashMap::new();

    // See `BackendReadiness`'s doc and this function's own doc for why this
    // exists: `Save` must not fire against a `bw serve` that was merely
    // kicked off, only one confirmed to be answering. Skipped entirely (no
    // thread spawned, immediately `Ready`) when the backend was already
    // running before this flow started -- default mode is unaffected down
    // to the frame, matching `spawn_sync`/`spawn_vault_load`'s identical
    // `skip_readiness_wait` exemption elsewhere in this app.
    //
    // `ready_rx` is `None` whenever no probe is currently in flight -- either
    // this flow never needed one (`backend_already_running`), or the last
    // one already resolved to `Ready`/`Unavailable`. Re-set to `Some` by the
    // "Retry" button in the `Unavailable` render arm below (review 11's
    // Important 2): a fresh `Unavailable` isn't a dead end, just another
    // `Preparing`.
    let mut ready_rx = if backend_already_running {
        None
    } else {
        Some(spawn_readiness_probe(&cache))
    };
    let mut backend_ready = if backend_already_running {
        BackendReadiness::Ready
    } else {
        BackendReadiness::Preparing
    };

    // Set on a failed `cache.set_app_match` and rendered inline, under the
    // Save/Cancel row (see `save_error_message`'s doc for why inline rather
    // than a `MessageBoxW`). Never cleared automatically -- it's replaced by
    // a fresh attempt's own outcome the next time Save is clicked, or the
    // window simply closes on Cancel with the failed attempt's item and
    // process choice both still intact for the user to try "Add app..."
    // again from scratch if they want to.
    let mut save_error: Option<String> = None;

    // The other inline message, added by review 28's Important 2 and rendered
    // in the same slot: the save SUCCEEDED against the server but the cache's
    // snapshot did not take it (`AppMatchWrite::ServerOnly`). Distinct from
    // `save_error` in three ways that all matter -- it is not an error, it is
    // terminal (there is nothing useful to retry), and the window's result is
    // already set when it appears, so closing from here still returns the
    // saved match rather than a cancellation. See `server_only_notice`.
    let mut save_notice: Option<String> = None;

    // Whether this window has already put a match on the server, and therefore
    // whether Save may fire again -- review 30's Minor 4.
    //
    // Its own flag rather than `save_notice.is_some()`, which is what the gate
    // used to be handed: the notice is set on the `ServerOnly` arm ONLY, so on
    // a `WroteThrough` save the gate stayed OPEN. `done = true` merely queues a
    // `ViewportCommand::Close`; frames still render before the window is
    // actually destroyed, and in those frames Save was enabled and a second
    // click re-PUT a match the server already has. That is the precise hazard
    // `can_save_app_match_now` exists to prevent, and it was being applied to
    // one of the two success arms. Set in BOTH.
    let mut already_saved = false;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 560.0])
            .with_position(centered_position(440.0, 560.0))
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native(ADD_APP_TITLE, options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::apply(ui.ctx());
            // The OS window exists by this first painted frame, and this is
            // where it is brought to the front. See `foreground`: a refusal
            // from Windows flashes the taskbar button rather than being
            // ignored.
            crate::foreground::raise_window(ADD_APP_TITLE);
            styled = true;
            ui.ctx().request_repaint();
            return;
        }

        // Non-blocking, like every other background-thread drain in this
        // app (favicon loads, vault loads, the TOTP poll): whenever the
        // readiness thread spawned above reports back, apply it and drop the
        // receiver -- the probe that sent it is done either way. A fresh one
        // only exists again if the user clicks "Retry" from the
        // `Unavailable` arm below (review 11's Important 2), so this is a
        // no-op between that click and the new probe's own result.
        if let Some(rx) = &ready_rx {
            if let Ok(result) = rx.try_recv() {
                backend_ready = match result {
                    Ok(()) => BackendReadiness::Ready,
                    Err(e) => BackendReadiness::Unavailable(e),
                };
                ready_rx = None;
            }
        }
        // Keep polling the channel above at a steady cadence while still
        // waiting -- without an explicit repaint request, a window with no
        // other animation and no user input sits static between frames and
        // wouldn't notice the channel until the next mouse move.
        if backend_ready == BackendReadiness::Preparing {
            ui.ctx().request_repaint_after(Duration::from_millis(150));
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::CANVAS)
                    .inner_margin(Margin::symmetric(20, 18)),
            )
            .show(ui, |ui| {
                let mut done = false;

                theme::card_header(ui, "Add app");
                ui.add_space(10.0);
                title_block(
                    ui,
                    &format!("Match a process to \u{201c}{}\u{201d}", target_item.name),
                    "The chosen process fills from this item from now on.",
                );
                ui.add_space(8.0);
                search_field(ui, &mut filter, "Search open windows");
                ui.add_space(8.0);

                // Matches against the window title (what's shown as the
                // primary line) as well as the exe name, so searching either
                // "epic" or "epicgameslauncher" finds the same row.
                let filter_lower = filter.to_lowercase();
                let filtered: Vec<usize> = (0..windows.len())
                    .filter(|&i| {
                        let w = &windows[i];
                        w.title.to_lowercase().contains(&filter_lower)
                            || w.exe_name.to_lowercase().contains(&filter_lower)
                    })
                    .collect();

                list_card(
                    ui,
                    (ui.available_height() - 148.0).max(0.0),
                    filtered.len(),
                    |ui, row| {
                        let w = &windows[filtered[row]];
                        let selected = selected_hwnd == Some(w.hwnd);
                        let secondary = format!("({} \u{b7} {})", w.exe_name, w.pid);
                        let texture = icon_cache
                            .entry(w.exe_path.clone())
                            .or_insert_with(|| {
                                icon::extract_small_icon(&w.exe_path).map(|rgba| {
                                    let image = egui::ColorImage::from_rgba_unmultiplied(
                                        [rgba.width as usize, rgba.height as usize],
                                        &rgba.rgba,
                                    );
                                    ui.ctx().load_texture(
                                        w.exe_path.clone(),
                                        image,
                                        egui::TextureOptions::default(),
                                    )
                                })
                            })
                            .as_ref();
                        if list_row(ui, &w.title, &secondary, selected, texture) {
                            selected_hwnd = Some(w.hwnd);
                            // Review 11's Minor: a previous attempt's failure
                            // message otherwise persisted even after the
                            // user picked a different process, reading like
                            // it was still about *this* choice.
                            save_error = None;
                        }
                    },
                );

                ui.add_space(10.0);
                theme::field_label(ui, "On focus");
                trigger_segmented(ui, &mut trigger);

                ui.add_space(10.0);
                // Exhaustively matched (no catch-all), same discipline as
                // `TotpState`'s render: `Ready` needs no caption of its own
                // (the button's enabled state already says everything), so
                // that arm is deliberately empty rather than missing.
                match &backend_ready {
                    BackendReadiness::Preparing => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(12.0).color(theme::TEXT_FAINT));
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Preparing your Bitwarden backend\u{2026}")
                                    .size(11.0)
                                    .color(theme::TEXT_FAINT),
                            );
                        });
                        ui.add_space(6.0);
                    }
                    BackendReadiness::Unavailable(reason) => {
                        // Review 11's Minor: the old wording pointed at
                        // "Sync from the tray menu", which this very flow
                        // has disabled (see `main`'s `tray.add_app_id`
                        // handler) and which the user couldn't reach anyway
                        // -- this window blocks the main loop like every
                        // other blocking window in this app. "Retry" below
                        // is the actual way forward.
                        ui.label(
                            RichText::new(format!(
                                "Deskwarden\u{2019}s Bitwarden backend hasn\u{2019}t answered yet, \
                                 so Save is unavailable right now. It may still be starting up \
                                 -- click Retry to keep waiting, or Cancel and use \u{201c}Add \
                                 app\u{2026}\u{201d} again in a moment.\n\n{reason}"
                            ))
                            .size(11.0)
                            .color(theme::ERROR),
                        );
                        ui.add_space(4.0);
                        if theme::secondary_button(ui, "Retry").clicked() {
                            // Not terminal (review 11's Important 2): a
                            // save-memory start can still land after this
                            // probe's own deadline, so give the user a way
                            // to wait again rather than making this window
                            // permanently unusable until Cancel.
                            ready_rx = Some(spawn_readiness_probe(&cache));
                            backend_ready = BackendReadiness::Preparing;
                        }
                        ui.add_space(6.0);
                    }
                    BackendReadiness::Ready => {}
                }
                // The selection is carried as a window handle, but every gate
                // below is about the process NAME (see `can_save_app_match`),
                // so it is resolved once, here, through the same list -- and
                // through the same `selected_window` the save itself uses, so
                // the row this answers for and the row that gets written can
                // never be two different rows.
                let selected_process =
                    selected_window(&windows, selected_hwnd).map(|w| w.exe_name.as_str());

                // The refusal, said out loud. `can_save_app_match` disables
                // Save for a host process; without this the button would
                // simply be grey for no stated reason, which is the "silent
                // no-op" shape reviews 10 and 11 both removed from this
                // window.
                if let Some(refusal) = selected_process.and_then(host_process_refusal) {
                    ui.label(RichText::new(refusal).size(11.0).color(theme::ERROR));
                    ui.add_space(6.0);
                }

                // The item's PRE-EXISTING bad match, if it has one. Shown
                // until something else is saved over it -- it is not about
                // this window's own attempt, so it is not cleared by a
                // selection or an error like `save_error` is.
                if let Some(process) = &existing_host_match {
                    ui.label(
                        RichText::new(existing_host_match_notice(&target_item.name, process))
                            .size(11.0)
                            .color(theme::TEXT_FAINT),
                    );
                    ui.add_space(6.0);
                }

                if let Some(error) = &save_error {
                    ui.label(RichText::new(error).size(11.0).color(theme::ERROR));
                    ui.add_space(6.0);
                }
                // Same slot, same idiom, deliberately NOT `theme::ERROR`: the
                // save worked, and colouring it as a failure would send the
                // user to re-do something that is already in their vault.
                if let Some(notice) = &save_notice {
                    ui.label(RichText::new(notice).size(11.0).color(theme::TEXT_FAINT));
                    ui.add_space(6.0);
                }

                ui.horizontal(|ui| {
                    // Item 1 (review 10's Important 1) and item 3 (Important
                    // 2/3) both collapse into this one gate: unclickable
                    // with nothing selected, and unclickable until the
                    // backend is confirmed to actually be answering --
                    // never both true is what let either bug happen before.
                    let can_save =
                        can_save_app_match_now(selected_process, &backend_ready, already_saved);
                    let save_clicked = ui
                        .add_enabled_ui(can_save, |ui| theme::primary_button(ui, "Save", None))
                        .inner
                        .clicked();
                    if save_clicked {
                        // `can_save` already guarantees this, but matching
                        // defensively rather than `.unwrap()`ing costs nothing
                        // and means a future change to the gate can't turn
                        // this into a panic.
                        if let Some(w) = selected_window(&windows, selected_hwnd) {
                            let m = app_match_for(w, trigger);
                            match cache.set_app_match(&target_item, &m) {
                                Ok(written) => {
                                    // Matched exhaustively rather than
                                    // ignored: review 26's Minor 2 is
                                    // that this used to be `Ok(())` for
                                    // BOTH of these, and the second one
                                    // means the match is saved in the
                                    // vault but absent from the snapshot
                                    // `main` immediately rebuilds the
                                    // engine from -- so the match the
                                    // user just spent two windows on
                                    // does nothing until the next full
                                    // sync. Save still SUCCEEDED (the
                                    // server has it), so this window
                                    // still closes with `Some(m)`:
                                    // returning `None` would tell `main`
                                    // the user cancelled, which is a
                                    // different and false statement. See
                                    // the ledger for the `main.rs` half
                                    // this cannot reach from here.
                                    // Set BEFORE the arms below, because
                                    // the `ServerOnly` arm deliberately
                                    // does not close the window: whatever
                                    // closes it afterwards (Cancel, the
                                    // title-bar X) must still hand this
                                    // match back, since the save really
                                    // did happen.
                                    *result_for_closure.borrow_mut() = Some(SavedAppMatch {
                                        app_match: m,
                                        write: written,
                                    });
                                    match written {
                                        AppMatchWrite::WroteThrough => {
                                            already_saved = true;
                                            done = true;
                                        }
                                        AppMatchWrite::ServerOnly => {
                                            already_saved = true;
                                            log::warn!(
                                                "saved the app match for vault item {} to the \
                                                 server, but the cache's snapshot does not \
                                                 hold that item, so a match engine rebuilt \
                                                 from the cache alone would NOT have it",
                                                target_item.id
                                            );
                                            // Review 28's Important 2:
                                            // this used to close exactly
                                            // like a fully live save, so
                                            // the ONE state where the
                                            // user has to do something
                                            // looked identical to the
                                            // state where they do not.
                                            // Staying open is what makes
                                            // the notice visible at all;
                                            // `can_save` above goes false
                                            // the moment it is set, so
                                            // Cancel (relabelled below)
                                            // is the only way on.
                                            save_error = None;
                                            save_notice =
                                                Some(server_only_notice(&target_item.name));
                                        }
                                    }
                                }
                                // Item 2 (review 10's Important 1): stay
                                // open with the item and process choice
                                // both still intact -- `selected_hwnd`,
                                // `filter`, and `trigger` are untouched
                                // above -- instead of discarding two
                                // windows of user effort the way an
                                // unconditional `done = true` used to.
                                // `save_error` renders inline above; the
                                // user retries by clicking Save again
                                // (still enabled, since the backend
                                // itself is fine -- this one write just
                                // failed) or gives up via Cancel.
                                Err(e) => {
                                    log::error!(
                                        "failed to save app match onto vault item {}: {e:?}",
                                        target_item.id
                                    );
                                    save_error = Some(save_error_message(&e));
                                }
                            }
                        }
                    }
                    // "Close", not "Cancel", once a save has landed: the
                    // window's result is already set, so this button no
                    // longer cancels anything and labelling it as though it
                    // did would suggest clicking it undoes the save.
                    let dismiss = if save_notice.is_some() {
                        "Close"
                    } else {
                        "Cancel"
                    };
                    if theme::secondary_button(ui, dismiss).clicked() {
                        done = true;
                    }
                });

                if done {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
    });

    let saved = result.borrow_mut().take();
    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> VaultItem {
        VaultItem {
            id: "1".into(),
            name: name.into(),
            fields: vec![],
            login: None,
            card: None,
            identity: None,
            ssh_key: None,
            notes: None,
            item_type: None,
            folder_id: None,
            favorite: false,
            other: serde_json::Map::new(),
        }
    }

    fn item_of_type(name: &str, item_type: Option<i64>) -> VaultItem {
        VaultItem {
            item_type,
            ..item(name)
        }
    }

    #[test]
    fn the_picker_lists_only_logins() {
        // Attaching an app match to a secure note is meaningless: the fill
        // would type two empty strings into the matched application.
        let items = vec![
            item_of_type("Site", Some(1)),
            item_of_type("Wifi", Some(2)),
            item_of_type("Visa", Some(3)),
            item_of_type("Legacy", None),
        ];
        let listed: Vec<String> = logins_only(items).into_iter().map(|i| i.name).collect();
        assert_eq!(listed, vec!["Site".to_string(), "Legacy".to_string()]);
    }

    #[test]
    fn empty_filter_matches_every_item() {
        assert!(item_matches_filter(&item("Rockstar Games"), ""));
    }

    #[test]
    fn filter_matches_a_lowercased_substring_against_the_items_name() {
        // The caller lowercases the filter once before scanning a list (see
        // this fn's doc comment); item_matches_filter itself only lowercases
        // the item's name, so this exercises it with an already-lower filter.
        assert!(item_matches_filter(&item("Rockstar Games"), "rock"));
        assert!(item_matches_filter(&item("Rockstar Games"), "games"));
    }

    #[test]
    fn filter_excludes_non_matching_items() {
        assert!(!item_matches_filter(&item("Rockstar Games"), "mabl"));
    }

    #[test]
    fn every_trigger_mode_is_offered_in_the_segmented_control() {
        // A TriggerMode added to the enum but not to TRIGGER_CHOICES would be
        // silently un-pickable in the UI.
        for mode in [TriggerMode::Prompt, TriggerMode::Hotkey, TriggerMode::Auto] {
            assert!(
                TRIGGER_CHOICES.iter().any(|(m, _, _)| *m == mode),
                "{mode:?} is missing from TRIGGER_CHOICES"
            );
        }
    }

    use crate::vault_bridge::VaultBridge;

    fn cache_for(url: String) -> VaultCache {
        VaultCache::new(VaultBridge::new(url))
    }

    fn items_body() -> &'static str {
        r#"{"success":true,"data":{"data":[
            {"id":"1","name":"Alpha","fields":[],"type":1}
        ]}}"#
    }

    fn folders_body() -> &'static str {
        r#"{"success":true,"data":{"data":[]}}"#
    }

    #[test]
    fn load_items_for_picker_reads_a_populated_cache_without_touching_the_backend() {
        // Only `/list/object/folders` is mocked, for `populate_with`'s own
        // setup call below -- `/list/object/items` deliberately is not: if
        // `load_items_for_picker` fell back to a live populate instead of
        // trusting the already-populated cache, that unmocked request would
        // come back as an error and the assertion below would fail.
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        assert_eq!(
            cache.populate_with(vec![item("Alpha")], cache.epoch()).unwrap(),
            PopulateOutcome::Populated
        );

        let result = load_items_for_picker(&cache, cache.epoch().era());

        match result {
            PickerItemsResult::Items(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].name, "Alpha");
            }
            _ => panic!("expected Items, a populated cache must not be reported as empty or unreachable"),
        }
    }

    #[test]
    fn load_items_for_picker_falls_back_to_a_live_populate_when_the_cache_is_empty() {
        // Regression test for review 9's Minor: a failed startup
        // `populate_with` (see main.rs) used to leave `pick_vault_item`
        // stuck reading an empty cache for the rest of the session, even
        // with a healthy `bw serve`. `load_items_for_picker` must notice the
        // cache was never populated and go fetch instead of trusting the
        // empty snapshot.
        let mut server = mockito::Server::new();
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .expect(1)
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .expect(1)
            .create();
        let cache = cache_for(server.url());
        assert!(!cache.is_populated());

        let result = load_items_for_picker(&cache, cache.epoch().era());

        match result {
            PickerItemsResult::Items(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].name, "Alpha");
            }
            _ => panic!("expected Items after a successful fallback populate"),
        }
        assert!(cache.is_populated(), "the fallback populate should fill the cache");
    }

    #[test]
    fn load_items_for_picker_reports_backend_unreachable_distinctly_from_an_empty_vault() {
        // Review 10's Minor 5: a genuinely empty vault and an unreachable
        // backend used to both come back as an empty `Vec`, so the message
        // shown was always "make sure the backend is reachable" -- a
        // misdiagnosis for a brand new user with a healthy backend and
        // nothing in their vault yet. The two must be distinguishable.
        let cache = cache_for("http://127.0.0.1:1".to_string());
        assert!(!cache.is_populated());

        let result = load_items_for_picker(&cache, cache.epoch().era());

        assert!(
            matches!(result, PickerItemsResult::BackendUnreachable(_)),
            "an unreachable backend must not be reported the same way as a genuinely empty vault"
        );
        assert!(!cache.is_populated());
    }

    #[test]
    fn load_items_for_picker_reports_a_genuinely_empty_vault_distinctly() {
        // The other half of the Minor 5 fix: a populated cache with zero
        // items (new user, healthy backend) must not be conflated with the
        // backend-unreachable case above.
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        assert_eq!(
            cache.populate_with(vec![], cache.epoch()).unwrap(),
            PopulateOutcome::Populated
        );

        let result = load_items_for_picker(&cache, cache.epoch().era());

        assert!(
            matches!(result, PickerItemsResult::EmptyVault),
            "a populated-but-empty cache must be reported as EmptyVault, not BackendUnreachable"
        );
    }

    #[test]
    fn load_items_for_picker_reports_a_vault_with_items_but_no_logins_distinctly() {
        // The picker filters to logins, so a vault holding only notes and
        // cards now lists nothing -- but it is emphatically NOT an empty
        // vault, and saying "your vault doesn't have any items yet" about a
        // vault full of items would be a fresh instance of the exact
        // misdiagnosis reviews 10 and 14 removed from this very function.
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        assert_eq!(
            cache
                .populate_with(
                    vec![item_of_type("Wifi", Some(2)), item_of_type("Visa", Some(3))],
                    cache.epoch()
                )
                .unwrap(),
            PopulateOutcome::Populated
        );

        let result = load_items_for_picker(&cache, cache.epoch().era());

        assert!(
            matches!(result, PickerItemsResult::NoLogins),
            "a vault with items but no logins must not be reported as an empty vault"
        );
    }

    #[test]
    fn load_items_for_picker_only_offers_logins() {
        // The filter must be applied where the picker actually reads its
        // list, not merely available as a helper nothing calls -- this
        // plan's recurring failure shape is a change correct in isolation
        // that never reaches the behaviour it claims.
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        assert_eq!(
            cache
                .populate_with(
                    vec![item_of_type("Wifi", Some(2)), item_of_type("Site", Some(1))],
                    cache.epoch()
                )
                .unwrap(),
            PopulateOutcome::Populated
        );

        match load_items_for_picker(&cache, cache.epoch().era()) {
            PickerItemsResult::Items(items) => {
                let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
                assert_eq!(names, vec!["Site"], "a non-login was offered to the picker");
            }
            _ => panic!("expected Items: the vault holds a login"),
        }
    }

    #[test]
    fn load_items_for_picker_reports_a_vault_cleared_after_the_click_as_locked() {
        // REVIEW 25'S MINOR 2, and the half that no `DiscardedStale` check can
        // reach. `pick_vault_item` captures the era on the main thread at the
        // moment of the click and hands it to this function on a DETACHED
        // thread; a `clear` -- lock, or re-auth into a possibly different
        // account -- can land after that. The populate below then succeeds
        // perfectly well (it captures its own, newer epoch), so "did the
        // populate land?" answers yes, and the old spelling
        // (`is_populated()` then `items()`) went on to hand the user a list.
        // For the era this click belongs to there is no vault: the right
        // answer is `VaultLocked`, not a list and not "your vault doesn't have
        // any items yet".
        let mut server = mockito::Server::new();
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        assert_eq!(cache.populate().unwrap(), PopulateOutcome::Populated);

        // The click: the era is captured here, before the worker starts.
        let era = cache.epoch().era();
        // ...and the vault locks while the worker is on its way.
        cache.clear();

        let result = load_items_for_picker(&cache, era);

        assert!(
            matches!(result, PickerItemsResult::VaultLocked),
            "a vault cleared after the click was reported as data (or as an empty vault) \
             rather than as locked"
        );
    }

    #[test]
    fn load_items_for_picker_reports_a_vault_that_locked_mid_populate_distinctly() {
        // Review 14's Minor. `pick_vault_item` runs this on a detached
        // thread, and `main` clears the cache when the vault locks or the
        // user re-authenticates. A populate that started before that clear
        // and finished after it is correctly discarded -- but it used to be
        // indistinguishable from a real populate, so the picker read the
        // (deliberately) empty cache as data and told the user their vault
        // "doesn't have any items yet" and to go add one, when what actually
        // happened is that it locked.
        //
        // Deterministic, no sleeping: the `clear()` fires from inside the
        // mocked folders response handler, so it lands strictly after the
        // populate began fetching and strictly before it tries to write --
        // the same interleaving `vault_cache`'s own guard test uses.
        let mut server = mockito::Server::new();
        let cache = std::sync::Arc::new(cache_for(server.url()));
        let _items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .create();
        let cache_for_handler = cache.clone();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |_| {
                cache_for_handler.clear();
                folders_body().as_bytes().to_vec()
            })
            .create();

        let result = load_items_for_picker(&cache, cache.epoch().era());

        assert!(
            matches!(result, PickerItemsResult::VaultLocked),
            "a vault that locked mid-populate must not be reported as an empty vault"
        );
        assert!(!cache.is_populated());
    }

    #[test]
    fn load_items_for_picker_does_not_fetch_a_vault_it_has_already_been_told_is_gone() {
        // REVIEW 26'S MINOR 3. Correct answer, wrong cost. Once the era has
        // moved, no populate can produce a vault for THIS click -- the
        // populate takes its own, newer epoch, refills the cache for the new
        // session, and the re-check below it then fails exactly as it must.
        // The user paid seconds of HTTP under the spinner for a `VaultLocked`
        // that was already knowable. `expect(0)` is the assertion: the fix is
        // a return type that separates "superseded" from "never fetched", not
        // a faster populate.
        let mut server = mockito::Server::new();
        let items = server
            .mock("GET", "/list/object/items")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(items_body())
            .expect(0)
            .create();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(folders_body())
            .create();
        let cache = cache_for(server.url());
        assert_eq!(
            cache.populate_with(vec![item("Alpha")], cache.epoch()).unwrap(),
            PopulateOutcome::Populated
        );
        let era = cache.epoch().era();
        cache.clear();

        let result = load_items_for_picker(&cache, era);

        assert!(
            matches!(result, PickerItemsResult::VaultLocked),
            "a cleared vault must still be reported as locked"
        );
        items.assert();
    }

    #[test]
    fn next_is_disabled_with_nothing_selected() {
        // Review 11's Minor: `selected_id` is `None` on a fresh open of the
        // vault-item picker, and "Next" must be unclickable until something
        // is chosen -- the same shape `can_save_app_match` already
        // guarantees one window later.
        assert!(!can_pick_next(&None));
    }

    #[test]
    fn next_is_enabled_once_an_item_is_selected() {
        assert!(can_pick_next(&Some("1".to_string())));
    }

    #[test]
    fn save_is_disabled_with_nothing_selected_even_once_the_backend_is_ready() {
        // Review 10's Important 1: `selected_pid` is `None` on a fresh
        // launch, and Save must be unclickable until something is chosen --
        // regardless of backend readiness.
        assert!(!can_save_app_match(None, &BackendReadiness::Ready));
    }

    #[test]
    fn save_is_disabled_while_the_backend_is_still_preparing() {
        // Review 10's Important 2/3: a process being selected is not enough
        // on its own -- firing into a port that isn't confirmed bound yet is
        // exactly the bug being closed here.
        assert!(!can_save_app_match(Some("notepad.exe"), &BackendReadiness::Preparing));
    }

    #[test]
    fn save_is_disabled_when_the_backend_never_became_reachable() {
        assert!(!can_save_app_match(
            Some("notepad.exe"),
            &BackendReadiness::Unavailable("connection refused".to_string())
        ));
    }

    #[test]
    fn save_is_enabled_only_once_something_is_selected_and_the_backend_is_ready() {
        assert!(can_save_app_match(Some("notepad.exe"), &BackendReadiness::Ready));
    }

    const HOST: &str = "ApplicationFrameHost.exe";

    /// A match against the frame host is wrong by construction -- it names
    /// the process that owns the window for EVERY Microsoft Store app -- so
    /// this window must not be able to write one, no matter how ready the
    /// backend is.
    #[test]
    fn save_is_disabled_for_a_window_host_even_with_everything_else_ready() {
        // Deleting the `host_process_refusal(process).is_none() &&` from
        // `can_save_app_match` gives
        //     "picking the Store-app frame host would save a match that fires on every Store app"
        assert!(
            !can_save_app_match(Some(HOST), &BackendReadiness::Ready),
            "picking the Store-app frame host would save a match that fires on every Store app"
        );
        // Positive control on the same call with the same readiness: only the
        // process name differs, so this cannot pass against a build where
        // Save is simply never enabled.
        assert!(can_save_app_match(Some("KeepSolid.exe"), &BackendReadiness::Ready));
    }

    #[test]
    fn the_host_refusal_survives_the_outer_gate_too() {
        assert!(!can_save_app_match_now(Some(HOST), &BackendReadiness::Ready, false));
        assert!(can_save_app_match_now(
            Some("KeepSolid.exe"),
            &BackendReadiness::Ready,
            false
        ));
    }

    /// "Say clearly in the UI why, rather than silently doing nothing" -- the
    /// disabled button and the sentence beside it come from the same call, so
    /// they cannot drift apart.
    #[test]
    fn the_refusal_names_the_process_and_says_why_without_blaming_the_user() {
        let refusal = host_process_refusal(HOST).expect("the frame host must be refused");
        assert!(refusal.contains(HOST), "the user needs to know WHICH process: {refusal}");
        assert!(
            refusal.contains("Store"),
            "the reason is that this process fronts every Microsoft Store app: {refusal}"
        );
        assert!(
            refusal.contains("Add app"),
            "and it must say what to do instead: {refusal}"
        );
        // The likely cause and its remedy: a host row exists only when
        // `attribute_window` fell through, which on the reporting machine was
        // because both Store apps were minimised. "Try again" without
        // restoring the app would fail identically.
        assert!(
            refusal.contains("minimised") && refusal.contains("restore"),
            "the remedy for the likely cause is restoring the app so its own window exists: \
             {refusal}"
        );
    }

    #[test]
    fn the_refusal_does_not_state_the_likely_cause_as_the_only_one() {
        // `window_watch::attribute_window` yields `UnresolvedHost` in TWO
        // cases: no `CoreWindow` child (the minimised app), and a child whose
        // process could not be opened or named -- a Store app running at
        // higher integrity than Deskwarden. The first wording asserted the
        // first as fact, which made "restore the app and reopen Add app..."
        // advice that can never work for anyone in the second, stated as
        // certainty.
        //
        // Restoring the "because the app is minimised" phrasing fails here on
        // the hedge; deleting the closing sentence fails on the second
        // assertion.
        let refusal = host_process_refusal(HOST).expect("the frame host must be refused");
        assert!(
            refusal.contains("Usually"),
            "the likely cause is stated as the certain one, so a user it does not apply to is \
             told to keep retrying something that cannot succeed: {refusal}"
        );
        assert!(
            refusal.contains("still shows"),
            "nothing tells a user in the other case how to recognise it -- a row that still \
             names the host with the app on screen: {refusal}"
        );
    }

    /// The picker keys its selection by window handle, which is the reason
    /// `selected_window` exists at all rather than a `find` at each site.
    #[test]
    fn two_rows_that_share_a_pid_are_still_two_selections() {
        // `window_list::enum_proc` gives every `UnresolvedHost` row the HOST's
        // pid, because there is no other one to give. On the reporting machine
        // both Store frames -- "Speedtest" and "Settings" -- were pid 12472,
        // so a `find(|w| w.pid == pid)` answered with whichever came first
        // while the highlight sat on the other.
        //
        // Keying on `w.pid == hwnd as u32`, or returning `windows.first()`,
        // gives
        //     the row the gate answers for is not the row the user clicked
        //     left: "Speedtest"  right: "Settings"
        let windows = vec![frame("Speedtest", 101), frame("Settings", 102)];

        let picked = selected_window(&windows, Some(102)).expect("the second frame");

        assert_eq!(
            picked.title, "Settings",
            "the row the gate answers for is not the row the user clicked"
        );
        // POSITIVE CONTROL: the other handle really does resolve to the other
        // row, so this cannot pass against a function that always answers
        // last.
        assert_eq!(selected_window(&windows, Some(101)).unwrap().title, "Speedtest");
    }

    #[test]
    fn nothing_selected_and_a_handle_with_no_row_are_both_no_selection() {
        // The second is not hypothetical: the list is read once when the
        // picker opens, but the window it names can be closed while the picker
        // is up. `can_save_app_match` refuses a `None` process, so the button
        // greys out rather than saving against a row that is gone.
        let windows = vec![frame("Speedtest", 101)];
        assert!(selected_window(&windows, None).is_none());
        assert!(selected_window(&windows, Some(999)).is_none());
        assert!(selected_window(&[], Some(101)).is_none());
    }

    /// Two unattributable Store frames as `window_list` builds them: distinct
    /// handles, distinct titles, and **one** pid between them.
    fn frame(title: &str, hwnd: isize) -> WindowInfo {
        WindowInfo {
            hwnd,
            pid: 12472,
            exe_path: "C:\\Windows\\System32\\ApplicationFrameHost.exe".into(),
            exe_name: HOST.into(),
            title: title.into(),
            // An UNRESOLVED frame: nothing was found inside it, so it is listed
            // under the host's own name and its title names nothing.
            hosted: false,
        }
    }

    /// A row for a Microsoft Store app that WAS resolved inside its frame: the
    /// app's own executable, and the one kind of row whose title is an identity
    /// worth saving.
    fn hosted_row(exe_name: &str, title: &str) -> WindowInfo {
        WindowInfo { hosted: true, ..app_row(exe_name, title) }
    }

    /// A row for an ordinary, attributed application -- the only kind the save
    /// gate lets through.
    fn app_row(exe_name: &str, title: &str) -> WindowInfo {
        WindowInfo {
            hwnd: 501,
            pid: 4242,
            exe_path: format!("C:\\Program Files\\Vendor\\{exe_name}"),
            exe_name: exe_name.into(),
            title: title.into(),
            hosted: false,
        }
    }

    #[test]
    fn saving_captures_the_title_and_the_path_off_the_row_as_well_as_the_process() {
        // The whole point of capturing at add-time: a Store app that is later
        // suspended has no process to name and no image path to resolve, so
        // anything not copied here can never be recovered. Reverting
        // `app_match_for` to the `{ process, trigger }` it used to build gives
        //     left: ""  right: "KeepSolid"
        let m = app_match_for(&hosted_row("KeepSolid.exe", "KeepSolid"), TriggerMode::Auto);

        assert_eq!(m.process, "KeepSolid.exe");
        assert_eq!(m.title, "KeepSolid");
        assert!(m.hosted, "the flag that makes the title matchable at all");
        assert_eq!(m.path, "C:\\Program Files\\Vendor\\KeepSolid.exe");
        assert_eq!(m.trigger, TriggerMode::Auto);
    }

    /// **Review 31's Important 1.** An ordinary desktop app's title is not an
    /// identity anything is allowed to match on, so it is not recorded --
    /// otherwise it sits in the vault indistinguishable from a Store app's, and
    /// any frame that can be made to wear it claims this item's credentials.
    #[test]
    fn an_ordinary_row_records_no_title_to_be_matched_by() {
        // Changing the title arm back to `w.title.clone()` gives
        //     left: "Ledgerline - Invoices"  right: ""
        let m = app_match_for(&app_row("Ledgerline.exe", "Ledgerline - Invoices"), TriggerMode::Auto);

        assert_eq!(m.title, "", "a desktop app's title must not become a needle");
        assert!(!m.hosted);
        // Positive controls, so "records nothing" cannot be how this passes:
        // everything else about the row is still captured, and the ONLY
        // difference from the test above is which kind of row it was.
        assert_eq!(m.process, "Ledgerline.exe");
        assert_eq!(m.path, "C:\\Program Files\\Vendor\\Ledgerline.exe");
        assert_eq!(m.trigger, TriggerMode::Auto);
    }

    #[test]
    fn a_captured_match_carries_a_path_that_names_its_own_process() {
        // The invariant anything that later OPENS the app re-checks
        // (`AppMatch::launchable_path`): the row's `exe_name` and `exe_path`
        // both come from the one attributed pid, so what this window writes
        // satisfies the check by construction. Copying the path from a
        // different row -- or storing the window title in `path` -- gives
        //     "the picker wrote a path its own launch check rejects"
        let m = app_match_for(&app_row("Ledgerline.exe", "Ledgerline"), TriggerMode::Prompt);

        assert_eq!(
            m.launchable_path(),
            Some("C:\\Program Files\\Vendor\\Ledgerline.exe"),
            "the picker wrote a path its own launch check rejects"
        );
    }

    #[test]
    fn an_ordinary_process_is_not_refused() {
        // The positive control for the test above: a `host_process_refusal`
        // that refused everything would satisfy it while making the picker
        // unable to save anything at all.
        assert_eq!(host_process_refusal("KeepSolid.exe"), None);
        assert_eq!(host_process_refusal("Speedtest.exe"), None);
    }

    /// The chosen surface for a bad match ALREADY in the user's vault. It is
    /// detected and ignored at match time (`MatchEngine::rebuild`), reported
    /// here, and never rewritten on their behalf -- so the notice has to say
    /// all three of those things.
    #[test]
    fn the_existing_match_notice_names_the_item_and_promises_no_rewrite() {
        let notice = existing_host_match_notice("KeepSolid", HOST);
        assert!(notice.contains("KeepSolid"), "which item: {notice}");
        assert!(notice.contains(HOST), "which process: {notice}");
        assert!(
            notice.contains("ignoring"),
            "the user's real question is why their match went quiet: {notice}"
        );
        assert!(
            notice.contains("Nothing in your vault has been changed"),
            "this app does not rewrite the user's vault, and must not imply it did: {notice}"
        );
    }

    #[test]
    fn save_error_message_distinguishes_an_expired_session_from_every_other_failure() {
        let unauthorized = save_error_message(&VaultError::Unauthorized);
        assert!(
            unauthorized.contains("session") && unauthorized.contains("expired"),
            "got: {unauthorized}"
        );

        let other = save_error_message(&VaultError::Http("connection reset".to_string()));
        assert!(
            !other.contains("expired"),
            "a non-auth failure must not claim the session expired: {other}"
        );
    }

    /// Review 28's Important 2. A `ServerOnly` save is not a failure -- the
    /// vault has the match -- so Save must not stay armed for a retry that
    /// would PUT the same match again; the window is now a report, and the
    /// only thing left to do in it is close it.
    #[test]
    fn save_is_disabled_once_a_save_has_already_succeeded() {
        assert!(
            !can_save_app_match_now(Some("notepad.exe"), &BackendReadiness::Ready, true),
            "the save already landed on the server; clicking Save again repeats the PUT"
        );
        assert!(
            can_save_app_match_now(Some("notepad.exe"), &BackendReadiness::Ready, false),
            "and it must not disable Save for a window that has saved nothing yet"
        );
        assert!(
            !can_save_app_match_now(None, &BackendReadiness::Ready, false),
            "the original gate still applies underneath"
        );
    }

    /// Review 28's Important 2 again: `ServerOnly` had NO user-visible
    /// surface -- `run_picker` returned `Some(m)` and only logged, so the
    /// user was told nothing while the match sat live on the server and
    /// invisible to everything reading the cache.
    ///
    /// The wording is asserted rather than left to taste because the old copy
    /// promised a remedy that does not apply: "it goes live at the next full
    /// sync" is true for the unpopulated miss and FALSE for the reachable
    /// `position` miss, where the item was deleted out from under the write
    /// and no sync can bring the match back.
    #[test]
    fn the_server_only_notice_does_not_promise_a_sync_will_fix_it() {
        let notice = server_only_notice("GitHub");
        assert!(
            notice.contains("GitHub"),
            "the user needs to know WHICH item this is about: {notice}"
        );
        assert!(
            notice.contains("saved"),
            "the save succeeded and saying otherwise would send the user to re-do it: {notice}"
        );
        assert!(
            !notice.contains("next full sync") && !notice.contains("goes live at"),
            "a promise the crate cannot keep -- see this test's doc: {notice}"
        );
    }
}

/// Source-position guards for the Save gate's "this window has already saved
/// something" input (review 30's Minor 4).
///
/// `can_save_app_match_now` is pure and directly tested above, but WHAT IS
/// PASSED TO IT lives inside `run_picker`'s per-frame closure, which nothing
/// can drive outside a real event loop. The defect was exactly there: the
/// third argument was `save_notice.is_some()`, and the notice is set on the
/// `ServerOnly` arm ONLY. On `WroteThrough` the arm sets `done`, which merely
/// queues a `ViewportCommand::Close` -- frames still render before the window
/// is destroyed, and in those frames Save was ENABLED and a second click
/// re-PUT a match the server already has. That is the precise hazard
/// `can_save_app_match_now` exists to prevent, applied to one of two success
/// arms.
///
/// **What these guards can and cannot see**, in the idiom
/// `vault_window::reveal_state_placement_tests` established: they pin the
/// spelling and the COUNT of the flag's assignments and the gate's argument.
/// They cannot see a flag that is set in both arms and then never read, nor
/// one reset to false somewhere silly. Both of those are visible in a diff
/// that touches these lines; what this guards is the third success arm added
/// later that forgets one of them.
#[cfg(test)]
mod save_gate_placement_tests {
    // SPLIT ACROSS TWO LITERALS, DELIBERATELY. `include_str!` pulls this module
    // in too, so a needle written as one literal always matches -- inside the
    // const that defines it -- which is what made the equivalent guards in
    // `vault_window` pass with their regression live. `concat!` joins at
    // compile time. The count assertions are what ENFORCE this, not the
    // comment: re-joining a needle makes it appear one extra time and fails.
    const FLAG_SET: &str = concat!("already_saved", " = true;");
    const GATE_ARGUMENT: &str = concat!("already_saved", ")");
    /// The capture. `app_match_for` is pure and directly tested above, but the
    /// only call to it is inside the same unreachable closure, and the
    /// mutation is a one-liner: `AppMatch::for_process(w.exe_name.clone(),
    /// trigger)` compiles, saves, closes the window, and quietly writes a
    /// match that records neither the title a suspended Store app is
    /// recognised by nor the path the detail pane shows.
    /// **The ARGUMENT is part of the needle** (review 31's Minor 4). The
    /// previous spelling was `app_match_for(w,` and stopped at the comma, so
    /// `app_match_for(w, TriggerMode::Prompt)` -- which silently discards the
    /// trigger the user chose in this very window, on every save -- left the
    /// whole suite green. Same shape as `window_title(` in the ledger and as
    /// `MUT-6` in `app_window.rs`: pinning a call without its arguments pins
    /// the wrong half.
    const CAPTURE_CALL: &str = concat!("app_match_for", "(w, trigger)");

    fn source() -> &'static str {
        include_str!("picker_ui.rs")
    }

    #[test]
    fn both_successful_save_arms_mark_the_window_as_having_saved() {
        assert_eq!(
            source().matches(FLAG_SET).count(),
            2,
            "expected {FLAG_SET:?} exactly twice -- once on each arm of the `AppMatchWrite` \
             match. One occurrence means a success arm leaves Save clickable for the frames \
             between `done` and the window actually closing, and a second click re-PUTs a \
             match the server already has"
        );
    }

    #[test]
    fn the_counter_finds_a_capture_call_that_is_really_there() {
        // Positive control for the count below, in this module's own idiom:
        // without it a needle that never matched anything would satisfy a
        // `== 0` assertion, and one that matched everything would satisfy any
        // other.
        let planted = concat!("let m = app_match_for", "(w, trigger);");
        assert_eq!(planted.matches(CAPTURE_CALL).count(), 1, "planted: {planted}");
        assert_eq!("nothing here".matches(CAPTURE_CALL).count(), 0);
        // And the mutation the old needle could not see: the call is still
        // there, the trigger is not.
        let discarded = concat!("let m = app_match_for", "(w, TriggerMode::Prompt);");
        assert_eq!(discarded.matches(CAPTURE_CALL).count(), 0, "planted: {discarded}");
    }

    #[test]
    fn save_writes_the_match_built_from_the_whole_selected_row() {
        assert_eq!(
            source().matches(CAPTURE_CALL).count(),
            1,
            "expected {CAPTURE_CALL:?} exactly once -- the value Save writes, built from the \
             whole selected row AND the trigger the user chose. Zero means either the saved \
             match went back to being built from the row's process name alone (so a Microsoft \
             Store app records no title and can never be matched once Windows suspends it, and \
             no item has a path to open), or the trigger argument stopped being `trigger` and \
             every save now writes one fixed mode"
        );
    }

    #[test]
    fn the_save_gate_is_fed_that_flag_and_not_the_server_only_notice() {
        assert_eq!(
            source().matches(GATE_ARGUMENT).count(),
            1,
            "expected {GATE_ARGUMENT:?} exactly once, as `can_save_app_match_now`'s third \
             argument. Zero means the gate went back to deriving \"already saved\" from \
             something else -- `save_notice.is_some()` is the spelling that was wrong, because \
             the notice exists on only one of the two success arms"
        );
    }
}

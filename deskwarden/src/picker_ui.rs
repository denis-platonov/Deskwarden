use crate::app_match::{AppMatch, TriggerMode};
use crate::bw_serve::{readiness_schedule, wait_for_vault_ready, BACKEND_OP_TIMEOUT};
use crate::icon;
use crate::loading_ui;
use crate::theme;
use crate::vault_bridge::{ItemKind, VaultError, VaultItem};
use crate::vault_cache::{PopulateOutcome, VaultCache};
use crate::window_list::{self, WindowInfo};
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
    /// their whole duration, so no `clear` can interleave with the populate
    /// below. Even if a leftover detached worker from an abandoned picker
    /// did produce this, its `rx` is already dropped, so the value never
    /// reaches the `MessageBoxW` in `pick_vault_item`. The typing is still
    /// the right call: the planned encrypted disk cache adds exactly the
    /// background-thread callers that make this reachable, and a future
    /// session should not credit it as verified-live before then.
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
fn load_items_for_picker(cache: &VaultCache) -> PickerItemsResult {
    if !cache.is_populated() {
        log::warn!("vault cache is not populated; populating it now for the picker");
        match cache.populate() {
            Ok(PopulateOutcome::Populated) => {}
            // Not reachable from this call site today (every `clear` is on
            // the blocked main thread) -- see `PickerItemsResult::VaultLocked`.
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
    }

    let items = cache.items();
    // Emptiness is judged BEFORE the login filter and non-emptiness after
    // it, so the three outcomes stay distinct: nothing in the vault at all,
    // things in the vault but nothing attachable, and something to list.
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
    let cache_for_thread = cache.clone();
    std::thread::spawn(move || {
        let _ = tx.send(load_items_for_picker(&cache_for_thread));
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

    let _ = eframe::run_ui_native("Choose a vault item", options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::paint_window_background(ui);
            theme::apply(ui.ctx());
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
fn can_save_app_match(selected_pid: Option<u32>, backend_ready: &BackendReadiness) -> bool {
    selected_pid.is_some() && *backend_ready == BackendReadiness::Ready
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

pub fn run_picker(
    cache: Arc<VaultCache>,
    target_item: VaultItem,
    default_pid: Option<u32>,
    backend_already_running: bool,
) -> Option<AppMatch> {
    let windows: Vec<WindowInfo> = window_list::list_windows(std::process::id());

    // The update closure must `move`-capture its state (it's FnMut + 'static
    // and runs on every repaint), so a plain local `Option<AppMatch>` can't be
    // read back by this function after `run_simple_native` returns. Instead,
    // the result lives in an `Rc<RefCell<_>>`: a clone is moved into the
    // closure, and the original is read here once the (blocking) call
    // returns. This is safe because eframe runs the closure on the same
    // thread that's blocked inside `run_simple_native` -- there's no
    // cross-thread sharing happening.
    let result: Rc<RefCell<Option<AppMatch>>> = Rc::new(RefCell::new(None));
    let result_for_closure = result.clone();

    let default_window = default_pid.and_then(|pid| windows.iter().find(|w| w.pid == pid));
    let mut filter = default_window.map(|w| w.exe_name.clone()).unwrap_or_default();
    let mut selected_pid: Option<u32> = default_window.map(|w| w.pid);
    let mut trigger = TriggerMode::Prompt;
    let mut styled = false;

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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 560.0])
            .with_position(centered_position(440.0, 560.0))
            .with_icon(theme::window_icon()),
        ..Default::default()
    };

    let _ = eframe::run_ui_native("Add app to Deskwarden", options, move |ui, _frame| {
        if !styled {
            // egui applies a new font set at the *start* of the next frame,
            // not the one that calls set_fonts -- drawing Archivo-styled
            // text in this same frame would look up a family that doesn't
            // exist yet and panic. Skip drawing this frame; the real UI
            // starts on the next one, once the fonts are actually live.
            theme::apply(ui.ctx());
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
                        let selected = selected_pid == Some(w.pid);
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
                            selected_pid = Some(w.pid);
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
                if let Some(error) = &save_error {
                    ui.label(RichText::new(error).size(11.0).color(theme::ERROR));
                    ui.add_space(6.0);
                }

                ui.horizontal(|ui| {
                    // Item 1 (review 10's Important 1) and item 3 (Important
                    // 2/3) both collapse into this one gate: unclickable
                    // with nothing selected, and unclickable until the
                    // backend is confirmed to actually be answering --
                    // never both true is what let either bug happen before.
                    let can_save = can_save_app_match(selected_pid, &backend_ready);
                    let save_clicked = ui
                        .add_enabled_ui(can_save, |ui| theme::primary_button(ui, "Save", None))
                        .inner
                        .clicked();
                    if save_clicked {
                        // `can_save` already guarantees both of these, but
                        // matching defensively rather than `.unwrap()`ing
                        // costs nothing and means a future change to the
                        // gate can't turn this into a panic.
                        if let Some(pid) = selected_pid {
                            if let Some(w) = windows.iter().find(|w| w.pid == pid) {
                                let m = AppMatch {
                                    process: w.exe_name.clone(),
                                    trigger,
                                };
                                match cache.set_app_match(&target_item, &m) {
                                    Ok(()) => {
                                        *result_for_closure.borrow_mut() = Some(m);
                                        done = true;
                                    }
                                    // Item 2 (review 10's Important 1): stay
                                    // open with the item and process choice
                                    // both still intact -- `selected_pid`,
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

        let result = load_items_for_picker(&cache);

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

        let result = load_items_for_picker(&cache);

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

        let result = load_items_for_picker(&cache);

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

        let result = load_items_for_picker(&cache);

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

        let result = load_items_for_picker(&cache);

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

        match load_items_for_picker(&cache) {
            PickerItemsResult::Items(items) => {
                let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
                assert_eq!(names, vec!["Site"], "a non-login was offered to the picker");
            }
            _ => panic!("expected Items: the vault holds a login"),
        }
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

        let result = load_items_for_picker(&cache);

        assert!(
            matches!(result, PickerItemsResult::VaultLocked),
            "a vault that locked mid-populate must not be reported as an empty vault"
        );
        assert!(!cache.is_populated());
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
        assert!(!can_save_app_match(Some(1234), &BackendReadiness::Preparing));
    }

    #[test]
    fn save_is_disabled_when_the_backend_never_became_reachable() {
        assert!(!can_save_app_match(
            Some(1234),
            &BackendReadiness::Unavailable("connection refused".to_string())
        ));
    }

    #[test]
    fn save_is_enabled_only_once_something_is_selected_and_the_backend_is_ready() {
        assert!(can_save_app_match(Some(1234), &BackendReadiness::Ready));
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
}

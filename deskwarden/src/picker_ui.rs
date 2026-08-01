use crate::app_match::{AppMatch, TriggerMode};
use crate::bw_serve::{readiness_schedule, wait_for_vault_ready, BACKEND_OP_TIMEOUT};
use crate::icon;
use crate::loading_ui;
use crate::theme;
use crate::vault_bridge::{VaultError, VaultItem};
use crate::vault_cache::VaultCache;
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
    /// The cache was never populated and a fresh populate also failed --
    /// `bw serve` is unreachable, not merely idle.
    BackendUnreachable(String),
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
        if let Err(e) = cache.populate() {
            log::error!("could not populate the vault cache for the picker: {e:?}");
            return PickerItemsResult::BackendUnreachable(format!("{e:?}"));
        }
    }

    let items = cache.items();
    if items.is_empty() {
        PickerItemsResult::EmptyVault
    } else {
        PickerItemsResult::Items(items)
    }
}

/// Opens a blocking egui window listing the user's vault items with a search
/// box, and returns the one they pick (or `None` if they cancel, or the vault
/// has nothing to show).
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
/// Takes `&VaultCache` rather than owning it because it's only used *before*
/// the (`FnMut + 'static`) update closure is built; the items already read
/// out of it are what gets moved in.
pub fn pick_vault_item(cache: &VaultCache) -> Option<VaultItem> {
    let result = std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        scope.spawn(move || {
            let _ = tx.send(load_items_for_picker(cache));
        });
        loading_ui::show_while("Loading your vault...", rx)
    });

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
        cache.populate_with(vec![item("Alpha")]).unwrap();

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
        cache.populate_with(vec![]).unwrap();

        let result = load_items_for_picker(&cache);

        assert!(
            matches!(result, PickerItemsResult::EmptyVault),
            "a populated-but-empty cache must be reported as EmptyVault, not BackendUnreachable"
        );
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

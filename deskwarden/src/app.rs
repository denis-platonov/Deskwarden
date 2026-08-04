//! Application-level glue: the pieces `main` orchestrates, kept in the library
//! so they're reachable from examples and integration tests rather than being
//! locked inside the binary target.

use crate::app_match::{AppMatch, TriggerMode};
use crate::injector::ui_automation;
use crate::injector::{Injector, SendInputFiller, UiAutomationFiller};
use crate::overlay_ui;
use crate::vault_bridge::{extract_app_match, VaultItem};
use crate::vault_cache::VaultCache;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetWindowRect, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

/// The overlay's fixed size (must match `overlay_ui::show_prompt_overlay`'s
/// `with_inner_size`) -- needed here to clamp its position on-screen before
/// the window exists to measure.
const OVERLAY_WIDTH: f32 = 396.0;
const OVERLAY_HEIGHT: f32 = 164.0;
/// Gap between the field/window edge and the overlay, so it doesn't sit
/// flush against the thing it's about to fill.
const OVERLAY_GAP: f32 = 10.0;

/// Where to place the autofill overlay so it reads as "next to the field"
/// rather than wherever the OS happens to put a new window: just below the
/// focused/matched field if UI Automation can find one, else just outside
/// the matched window's own top-right corner. Clamped to the nearest
/// monitor's work area so it can't land off-screen or under the taskbar.
fn overlay_position(hwnd: isize) -> Option<(f32, f32)> {
    let (x, y) = match ui_automation::field_anchor_rect(hwnd) {
        Ok(Some(rect)) => (rect.left as f32, rect.bottom as f32 + OVERLAY_GAP),
        _ => {
            let window = window_rect(hwnd)?;
            (
                window.right as f32 - OVERLAY_WIDTH,
                window.top as f32 + OVERLAY_GAP,
            )
        }
    };
    Some(clamp_to_monitor(hwnd, x, y))
}

fn window_rect(hwnd: isize) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(HWND(hwnd as *mut core::ffi::c_void), &mut rect).ok()? };
    Some(rect)
}

fn clamp_to_monitor(hwnd: isize, x: f32, y: f32) -> (f32, f32) {
    unsafe {
        let monitor =
            MonitorFromWindow(HWND(hwnd as *mut core::ffi::c_void), MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let work = info.rcWork;
            let clamped_x = x
                .min(work.right as f32 - OVERLAY_WIDTH)
                .max(work.left as f32);
            let clamped_y = y
                .min(work.bottom as f32 - OVERLAY_HEIGHT)
                .max(work.top as f32);
            return (clamped_x, clamped_y);
        }
    }
    (x, y)
}

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
            login.password.as_deref().map(|p| p.to_owned()).unwrap_or_default(),
        ),
        None => (String::new(), String::new()),
    }
}

/// Fetches the item's credentials and injects them into `hwnd`.
///
/// Reads from `VaultCache`'s in-memory snapshot rather than `bw serve`
/// directly -- this is the path that makes autofill work with the backend
/// fully stopped (see `backend_policy` and `vault_cache`'s module docs): a
/// keystroke-triggered fill used to be an HTTP round-trip to a process this
/// app might no longer even be running. An empty cache while the vault is
/// genuinely unlocked should not happen (`main` populates it once per
/// unlock), so a miss falls back to the bridge -- serving the fill rather
/// than failing it outright -- and logs a warning, since a miss here is a
/// bug signal worth noticing rather than silently swallowing.
pub fn fill_from_vault<A: UiAutomationFiller, B: SendInputFiller>(
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
    hwnd: isize,
) {
    let item = cache
        .items()
        .into_iter()
        .find(|i| i.id == item_id)
        .map(Ok)
        .unwrap_or_else(|| {
            log::warn!("cache miss for item {item_id} during a fill; falling back to bw serve");
            cache.bridge().get_item(item_id)
        });
    match item {
        Ok(item) => {
            let (username, password) = credentials_for(&item);
            if username.is_empty() && password.is_empty() {
                log::warn!("vault item {item_id} has no login credentials; nothing to fill");
                return;
            }
            match injector.fill(hwnd, &username, &password) {
                Ok(()) => fill_stats.record_fill(item_id),
                Err(e) => log::error!("fill failed for item {item_id} into hwnd {hwnd}: {e}"),
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
///
/// **Takes the window as one `ForegroundEvent`**, not as a handle plus a name
/// plus a title. Those three describe one window and are only correct
/// together; handed over separately, the one call site -- inside `main`'s
/// event loop, where no test reaches -- can pass a title that belongs to no
/// window, and the overlay then names the wrong thing while every test here
/// stays green.
pub fn handle_match<A: UiAutomationFiller, B: SendInputFiller>(
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
    m: &AppMatch,
    window: &crate::window_watch::ForegroundEvent,
) -> Option<(String, isize)> {
    let hwnd = window.hwnd;
    match m.trigger {
        TriggerMode::Auto => {
            fill_from_vault(cache, injector, fill_stats, item_id, hwnd);
            None
        }
        TriggerMode::Prompt => {
            // Read the item back first so the overlay can say *which*
            // credentials it is offering (design 2a shows the username and
            // item name, never a bare "fill something?"). A miss here is not
            // fatal to the prompt -- the overlay just can't name the
            // credentials -- and the fill path re-resolves the item on its
            // own anyway.
            //
            // Reads `cache.items()`, not `cache.bridge()`: the fill itself
            // (`fill_from_vault`, two lines below) already resolves the item
            // from the cache, so it is provably in memory here too -- going
            // to the bridge instead meant that with `keep_backend_running`
            // off and the backend stopped at idle, this always missed,
            // degrading every Prompt-mode overlay to the bare
            // "fill something?" this comment used to call unacceptable.
            //
            // (`prompt_request` is where the password is deliberately never
            // touched -- see its doc.)
            let item = cache.items().into_iter().find(|i| i.id == item_id);
            // Every decision about the overlay -- what it says, and where it
            // opens -- is made in `prompt_arm`, which a test drives with a
            // recording presenter; this function only reads the cache and
            // supplies the real presenter. **No value the overlay is given is
            // computed on this line**, which is the point: this line is the
            // one no test can reach, and an argument written here is an
            // argument nothing can check (review 32's Important 1).
            if prompt_arm(&REAL_OVERLAY, window, item.as_ref()) {
                fill_from_vault(cache, injector, fill_stats, item_id, hwnd);
            }
            None
        }
        TriggerMode::Hotkey => Some((item_id.to_string(), hwnd)),
    }
}

/// Everything the autofill overlay is told about a Prompt-mode match: what to
/// call the window, which credentials are being offered, and where to put the
/// window.
///
/// A struct rather than three returned values, so a caller cannot pass them to
/// `show_prompt_overlay` in the wrong order or quietly substitute one.
pub struct PromptRequest<'a> {
    pub label: &'a str,
    pub matched: Option<overlay_ui::OverlayMatch>,
    pub position: Option<(f32, f32)>,
}

/// **The whole of the Prompt decision, as a pure function** (review 31's
/// Important 2).
///
/// This used to be three statements inside [`handle_match`], which nothing can
/// call: it needs a `VaultCache`, an `Injector` and a `FillStats`, and it opens
/// a real window. So `let label = &window.exe_name;` -- reinstating the exact
/// bug the user reported, an overlay reading "ApplicationFrameHost.exe wants
/// your password" for a title-matched Store app -- compiled and left all 1300
/// tests green. [`window_label`] was tested, but only as a pure function that
/// nothing was pinned to call. Moving the computation into `handle_match` (as
/// the previous commit did) moved it from one untested call site to another.
///
/// The label is `window_label`, not `exe_name`: a match found through the title
/// table belongs to a window whose `exe_name` is the frame host's, and that
/// name means nothing to the user.
///
/// `item` is passed in rather than looked up here, because the lookup is the
/// one part that needs the cache; `position` likewise, because computing it
/// needs Win32. What is left is the part that can be got wrong silently.
///
/// The username is read straight off the login object rather than through
/// [`credentials_for`]: that helper also clones the plaintext password into a
/// `String` this path has no use for, and which would then be dropped without
/// being zeroized. The overlay never shows a password, so it should never hold
/// one.
pub fn prompt_request<'a>(
    window: &'a crate::window_watch::ForegroundEvent,
    item: Option<&VaultItem>,
    position: Option<(f32, f32)>,
) -> PromptRequest<'a> {
    PromptRequest {
        label: window_label(&window.exe_name, &window.title),
        matched: item.map(|item| {
            let username = item.login.as_ref().and_then(|l| l.username.clone());
            overlay_ui::OverlayMatch {
                item_name: item.name.clone(),
                username: username.filter(|u| !u.is_empty()),
            }
        }),
        position,
    }
}

/// The two Win32-shaped things the Prompt arm does: work out where the overlay
/// goes, and put it on screen.
///
/// **A trait so that the wiring between them is testable** (review 32's
/// Important 1). [`prompt_request`] was pure and directly tested, but the
/// value it was *handed* for `position` was computed on `handle_match`'s one
/// unreachable line, as an argument -- and passing that argument through a
/// `.map` that replaced its `y` with `0.0` (spelled out in
/// `prompt_wiring_tests` below, where it cannot collide with the needles)
/// pinned the overlay to the top of the screen on every Prompt match with the
/// whole suite green and no warnings. A test can only see what a test can
/// reach, and it could not reach that argument.
///
/// With the placement asked for *through* the presenter, the argument no
/// longer exists at an unreachable call site: [`prompt_arm`] computes nothing
/// and passes on what the presenter answered, and
/// `the_overlay_opens_where_the_placement_answered_for_that_window` fails on
/// any alteration of it, including ones nobody has thought of.
///
/// What is left unreachable is naming the two real functions, and that is
/// [`REAL_OVERLAY`] -- a struct literal of two function *references*, with no
/// arguments and no expression anywhere in it for a mutation to hide in, held
/// by source position in `prompt_wiring_tests` below. Wrapping them in
/// hand-written method bodies instead would have re-created the very hole this
/// closes one level down: `overlay_position(hwnd)` inside an unreachable
/// `fn position` can be given a `.map` exactly as easily as it could as an
/// argument.
pub trait PromptPresenter {
    /// Where to open the overlay for the window `hwnd`, or `None` to let the
    /// OS choose.
    fn position(&self, hwnd: isize) -> Option<(f32, f32)>;
    /// Shows the overlay and returns whether the user chose to fill.
    fn show(
        &self,
        label: &str,
        matched: Option<&overlay_ui::OverlayMatch>,
        position: Option<(f32, f32)>,
    ) -> bool;
}

/// A [`PromptPresenter`] that is nothing but the two functions it forwards to.
///
/// **Function pointers rather than a hand-written `impl` per presenter**, so
/// that the production presenter can be a struct literal naming two functions
/// -- see [`REAL_OVERLAY`]. The forwarding below is the only code in the way,
/// and unlike a Win32 body it is directly driven by
/// `an_fn_presenter_forwards_to_the_two_functions_it_was_built_from`.
pub struct FnPresenter {
    /// Asked where the overlay for this window goes.
    pub position: fn(isize) -> Option<(f32, f32)>,
    /// Asked to put it on screen; answers whether the user chose to fill.
    pub show: fn(&str, Option<&overlay_ui::OverlayMatch>, Option<(f32, f32)>) -> bool,
}

impl PromptPresenter for FnPresenter {
    fn position(&self, hwnd: isize) -> Option<(f32, f32)> {
        (self.position)(hwnd)
    }

    fn show(
        &self,
        label: &str,
        matched: Option<&overlay_ui::OverlayMatch>,
        position: Option<(f32, f32)>,
    ) -> bool {
        (self.show)(label, matched, position)
    }
}

/// The production presenter: the real placement calculation and the real
/// window, named and not called.
///
/// This is the whole of the Prompt arm that no test can execute, and it is
/// data: two function references, no arguments, nothing computed. The two
/// needles in `prompt_wiring_tests` cover each field from its name to its
/// comma, so a wrapper closure around either -- the shape review 32's
/// Important 1 was demonstrated with -- cannot be written here without
/// failing.
const REAL_OVERLAY: FnPresenter = FnPresenter {
    position: overlay_position,
    show: overlay_ui::show_prompt_overlay,
};

/// The whole of the Prompt arm except the vault lookup and the fill: ask
/// [`prompt_request`] what to show, ask the presenter where to show it, show
/// it, and answer whether the user clicked Fill.
///
/// Generic over the presenter so a test can drive this with a recorder and
/// assert that **the overlay is opened at the placement that was answered for
/// this window** -- the thing `the_position_it_is_handed_is_the_position_it_asks_for`
/// could not assert, because it only proved `prompt_request` does not drop
/// what it is given and nothing pinned what it was given.
///
/// The presenter is asked about `window.hwnd` rather than about a handle
/// passed in beside the event, for the reason [`handle_match`]'s doc gives for
/// taking one `ForegroundEvent`: a handle and a title that describe different
/// windows place the overlay beside one and name the other.
pub fn prompt_arm<P: PromptPresenter>(
    presenter: &P,
    window: &crate::window_watch::ForegroundEvent,
    item: Option<&VaultItem>,
) -> bool {
    let PromptRequest { label, matched, position } =
        prompt_request(window, item, presenter.position(window.hwnd));
    presenter.show(label, matched.as_ref(), position)
}

/// What to call the app in a window a foreground event describes.
///
/// Normally its executable's file name, which is what every overlay and log
/// line in this app has always said. The exception is the one window that has
/// no executable to name: an unattributable host frame, whose `exe_name` is
/// `ApplicationFrameHost.exe` -- a Windows implementation detail, and the very
/// string the user was shown in the bug they reported. Such a window is
/// matched by its title (see [`crate::match_engine::MatchEngine::lookup`]), so
/// the title is also the only name it has.
///
/// A host frame with no title falls back to the host's name rather than to an
/// empty string: no such window can be matched, so this is unreachable from
/// the fill path, but "" beside "wants to fill" would be worse than an ugly
/// name if any other caller ever arrives.
pub fn window_label<'a>(exe_name: &'a str, title: &'a str) -> &'a str {
    if crate::window_watch::is_host_process(exe_name) && !title.is_empty() {
        return title;
    }
    exe_name
}

/// Finds a currently-open window whose exe name matches `process` -- for
/// "Fill in app" (the vault window's detail pane), which has no
/// window-watch context of its own and needs to resolve a target hwnd from
/// just an item's `deskwarden:app-match` process name.
pub fn find_window_for_process<'a>(
    windows: &'a [crate::window_list::WindowInfo],
    process: &str,
) -> Option<&'a crate::window_list::WindowInfo> {
    windows.iter().find(|w| w.exe_name.eq_ignore_ascii_case(process))
}

/// Pure helper: turns a list of vault items into the `(item_id, AppMatch)`
/// entries the match engine is rebuilt from, dropping items with no
/// `deskwarden:app-match` field.
pub fn match_entries(items: &[VaultItem]) -> Vec<(String, AppMatch)> {
    items
        .iter()
        .filter_map(|item| extract_app_match(item).map(|m| (item.id.clone(), m)))
        .collect()
}

// DELIBERATELY ABSENT: a `refresh_match_engine(vault, engine)` that did its own
// `list_items` and rebuilt from the result. Reviews 15, 16 and 21 each removed
// one of its call sites -- every time, the defect was the same shape: an extra
// live request on a path that already had the data, so a transient backend
// failure left the engine unarmed and the user's just-saved match dead until
// the next sync. It survived as dead `pub` code (nothing warns) until review
// 23. Rebuild from `match_entries(&cache.items())` instead: the cache is the
// app's one source of vault truth and every write has already updated it.

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
                        other: serde_json::Map::new(),
                    }]
                })
                .unwrap_or_default(),
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

    #[test]
    fn find_window_for_process_matches_case_insensitively() {
        let windows = vec![
            crate::window_list::WindowInfo {
                hwnd: 1,
                pid: 100,
                exe_path: r"C:\Games\EpicGamesLauncher.exe".into(),
                exe_name: "EpicGamesLauncher.exe".into(),
                title: "Epic Games Launcher".into(),
                hosted: false,
            },
            crate::window_list::WindowInfo {
                hwnd: 2,
                pid: 200,
                exe_path: r"C:\Windows\notepad.exe".into(),
                exe_name: "notepad.exe".into(),
                title: "Untitled - Notepad".into(),
                hosted: false,
            },
        ];
        let found = find_window_for_process(&windows, "epicgameslauncher.exe").unwrap();
        assert_eq!(found.hwnd, 1);
        assert!(find_window_for_process(&windows, "steam.exe").is_none());
    }

    const HOST: &str = "ApplicationFrameHost.exe";

    #[test]
    fn an_ordinary_window_is_named_by_its_executable() {
        // The positive control for the two below: a `window_label` that always
        // answered with the title would fail here, and one that always
        // answered with the exe name would fail the next test.
        assert_eq!(window_label("Ledgerline.exe", "Ledgerline -- Invoices"), "Ledgerline.exe");
    }

    #[test]
    fn an_unattributable_store_frame_is_named_by_its_title() {
        // Deleting the `is_host_process` branch gives
        //     left: "ApplicationFrameHost.exe"  right: "Speedtest"
        // -- which is the exact string the user reported being shown.
        assert_eq!(window_label(HOST, "Speedtest"), "Speedtest");
    }

    #[test]
    fn an_untitled_host_frame_falls_back_to_the_host_name_rather_than_to_nothing() {
        assert_eq!(window_label(HOST, ""), HOST);
    }

    // ---- The Prompt decision itself (review 31's Important 2) ----

    fn window(exe_name: &str, title: &str) -> crate::window_watch::ForegroundEvent {
        crate::window_watch::ForegroundEvent {
            hwnd: 0x1234,
            pid: 4242,
            exe_name: exe_name.to_string(),
            title: title.to_string(),
        }
    }

    fn login_item(name: &str, username: &str) -> VaultItem {
        VaultItem {
            name: name.to_string(),
            login: Some(serde_json::from_str(&format!(r#"{{"username":"{username}"}}"#)).unwrap()),
            ..item("1", None)
        }
    }

    /// **The user's bug, at the place the overlay's words are actually
    /// decided.** A Store app matched through the title table arrives with the
    /// frame host's `exe_name`, and the overlay must not say that name.
    /// Replacing the `window_label` call with `&window.exe_name` gives
    ///     left: "ApplicationFrameHost.exe"  right: "Speedtest"
    #[test]
    fn a_title_matched_store_frame_is_prompted_for_under_its_own_title() {
        let w = window(HOST, "Speedtest");
        let request = prompt_request(&w, None, None);
        assert_eq!(request.label, "Speedtest");
    }

    #[test]
    fn an_ordinary_matched_window_is_prompted_for_under_its_executable() {
        // The positive control: a `prompt_request` that always answered with
        // the title would pass the test above and fail this one.
        let w = window("Ledgerline.exe", "Ledgerline -- Invoices");
        let request = prompt_request(&w, None, None);
        assert_eq!(request.label, "Ledgerline.exe");
    }

    #[test]
    fn the_prompt_names_the_credentials_it_is_offering() {
        let item = login_item("Ledgerline", "denis@example.com");
        let w = window("Ledgerline.exe", "Ledgerline");
        let request = prompt_request(&w, Some(&item), None);

        let matched = request.matched.expect("design 2a: never a bare \"fill something?\"");
        assert_eq!(matched.item_name, "Ledgerline");
        assert_eq!(matched.username.as_deref(), Some("denis@example.com"));
    }

    #[test]
    fn an_item_with_no_usable_username_still_prompts_by_name() {
        // An empty username is "no username", not a blank line in the overlay.
        let item = login_item("Ledgerline", "");
        let w = window("Ledgerline.exe", "Ledgerline");
        let request = prompt_request(&w, Some(&item), None);
        let matched = request.matched.expect("the item is still named");
        assert_eq!(matched.item_name, "Ledgerline");
        assert_eq!(matched.username, None);

        // And a cache miss is not fatal to the prompt at all.
        let request = prompt_request(&w, None, None);
        assert!(request.matched.is_none());
    }

    #[test]
    fn the_position_it_is_handed_is_the_position_it_asks_for() {
        // The overlay's placement is computed from Win32 and cannot be
        // computed here; what CAN be got wrong is dropping it on the way
        // through, which lands the overlay wherever the OS likes.
        let w = window("a.exe", "A");
        let request = prompt_request(&w, None, Some((120.0, 340.0)));
        assert_eq!(request.position, Some((120.0, 340.0)));
        let request = prompt_request(&w, None, None);
        assert_eq!(request.position, None);
    }

    // ---- The Prompt arm's wiring (review 32's Important 1) ----

    /// What the overlay was told, in a form a test can compare:
    /// `overlay_ui::OverlayMatch` is a plain data carrier with no `PartialEq`,
    /// and this is deliberately built from its fields rather than by cloning
    /// the type, so a field added to it and dropped on the way here is a
    /// compile error rather than a silently unchecked value.
    type Shown = (String, Option<(String, Option<String>)>, Option<(f32, f32)>);

    /// A [`PromptPresenter`] that answers with a fixed placement and records
    /// what it was asked and what it was shown.
    #[derive(Default)]
    struct RecordingPresenter {
        /// What `show` returns -- the user clicking Fill, or not.
        answer: bool,
        /// What `position` answers, standing in for the Win32 calculation.
        placement: Option<(f32, f32)>,
        asked_about: std::cell::Cell<Option<isize>>,
        shown: std::cell::RefCell<Vec<Shown>>,
    }

    impl PromptPresenter for RecordingPresenter {
        fn position(&self, hwnd: isize) -> Option<(f32, f32)> {
            self.asked_about.set(Some(hwnd));
            self.placement
        }

        fn show(
            &self,
            label: &str,
            matched: Option<&overlay_ui::OverlayMatch>,
            position: Option<(f32, f32)>,
        ) -> bool {
            self.shown.borrow_mut().push((
                label.to_string(),
                matched.map(|m| (m.item_name.clone(), m.username.clone())),
                position,
            ));
            self.answer
        }
    }

    /// **Review 32's Important 1, driven rather than pinned.** The overlay's
    /// placement used to be computed as an argument on `handle_match`'s one
    /// unreachable line, where mapping the placement's `y` to `0.0` -- the
    /// overlay pinned to the top of the screen on every Prompt match -- left
    /// 1211 lib and 111 bin tests green with no warnings.
    #[test]
    fn the_overlay_opens_where_the_placement_answered_for_that_window() {
        let item = login_item("Ledgerline", "denis@example.com");
        let w = window("Ledgerline.exe", "Ledgerline");
        let presenter = RecordingPresenter {
            placement: Some((120.0, 340.0)),
            ..Default::default()
        };

        prompt_arm(&presenter, &w, Some(&item));

        // Asked about the window that matched, not about some other handle.
        assert_eq!(presenter.asked_about.get(), Some(w.hwnd));
        let shown = presenter.shown.borrow();
        assert_eq!(shown.len(), 1, "the overlay is shown exactly once");
        assert_eq!(
            shown[0].2,
            Some((120.0, 340.0)),
            "the overlay must open at the placement that was computed for this window -- \
             anything else is the overlay landing somewhere the user's field is not"
        );
        // And the other two values, so this test also fails on a substituted
        // label or a substituted item rather than only on the placement.
        assert_eq!(shown[0].0, "Ledgerline.exe");
        assert_eq!(
            shown[0].1,
            Some(("Ledgerline".to_string(), Some("denis@example.com".to_string())))
        );
    }

    #[test]
    fn a_placement_that_could_not_be_computed_is_passed_on_as_none() {
        // The positive control for the test above: a `prompt_arm` that always
        // handed the overlay a fixed pair would pass that one and fail this.
        let w = window(HOST, "Speedtest");
        let presenter = RecordingPresenter::default();

        prompt_arm(&presenter, &w, None);

        let shown = presenter.shown.borrow();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].2, None, "no placement is not a placement of (0, 0)");
        // The title-matched Store frame is still named by its title here, on
        // the path that actually reaches the overlay.
        assert_eq!(shown[0].0, "Speedtest");
        assert_eq!(shown[0].1, None);
    }

    #[test]
    fn the_arm_answers_with_the_users_answer() {
        // What the caller does with this is fill the user's password into the
        // window, so an inverted or constant answer fills on Dismiss.
        let w = window("Ledgerline.exe", "Ledgerline");
        let filled = RecordingPresenter {
            answer: true,
            ..Default::default()
        };
        assert!(prompt_arm(&filled, &w, None));
        let dismissed = RecordingPresenter::default();
        assert!(!prompt_arm(&dismissed, &w, None));
    }

    // ---- and the adapter the production presenter is built out of ----

    /// Statics rather than captured state, because [`FnPresenter`] holds plain
    /// `fn` pointers: that is the property that makes [`REAL_OVERLAY`] a
    /// struct literal with no expression in it. Only the one test below uses
    /// these.
    static ASKED_HWND: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());
    static FORWARDED: std::sync::Mutex<Vec<Shown>> = std::sync::Mutex::new(Vec::new());

    fn recording_position(hwnd: isize) -> Option<(f32, f32)> {
        ASKED_HWND.lock().unwrap().push(hwnd);
        Some((11.0, 22.0))
    }

    fn recording_show(
        label: &str,
        matched: Option<&overlay_ui::OverlayMatch>,
        position: Option<(f32, f32)>,
    ) -> bool {
        FORWARDED.lock().unwrap().push((
            label.to_string(),
            matched.map(|m| (m.item_name.clone(), m.username.clone())),
            position,
        ));
        true
    }

    /// The forwarding is the only code between [`REAL_OVERLAY`]'s two named
    /// functions and the screen, so it is driven here -- swapping the two
    /// fields, or dropping an argument, fails.
    #[test]
    fn an_fn_presenter_forwards_to_the_two_functions_it_was_built_from() {
        let presenter = FnPresenter {
            position: recording_position,
            show: recording_show,
        };

        assert_eq!(presenter.position(4242), Some((11.0, 22.0)));
        assert_eq!(*ASKED_HWND.lock().unwrap(), vec![4242]);

        let matched = overlay_ui::OverlayMatch {
            item_name: "Ledgerline".to_string(),
            username: Some("denis@example.com".to_string()),
        };
        assert!(presenter.show("Ledgerline.exe", Some(&matched), Some((3.0, 4.0))));

        let forwarded = FORWARDED.lock().unwrap();
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0].0, "Ledgerline.exe");
        assert_eq!(
            forwarded[0].1,
            Some(("Ledgerline".to_string(), Some("denis@example.com".to_string())))
        );
        assert_eq!(forwarded[0].2, Some((3.0, 4.0)));
    }
}

/// Source-position guard for the two places in the Prompt arm that no test can
/// execute: `handle_match`'s one line, and [`REAL_OVERLAY`]'s two fields.
///
/// [`prompt_arm`] and [`prompt_request`] are both driven directly by the tests
/// above, with a recording presenter -- so *what* the overlay is told, and
/// *where* it is opened, are behavioural questions now, not textual ones. What
/// is left over is naming: `handle_match` needs a `VaultCache`, an `Injector`
/// and a `FillStats` and then opens a real window, and `REAL_OVERLAY` names the
/// two Win32 functions. Neither can be run.
///
/// That leftover is worth pinning because it is precisely where this crate's
/// recurring defect lives. Review 31's Important 2 was `let label =
/// &window.exe_name;` on `handle_match`'s line, leaving 1300 tests green while
/// restoring the reported bug; review 32's Important 1 was the `position`
/// argument on that same line, wrapped in a `.map` that zeroed its `y` --
/// green, and with no warnings either. A decision nothing is pinned to *call*
/// is a decision that can be dropped from the path it exists for.
///
/// **What this can and cannot see.** The `REAL_OVERLAY` needles run from each
/// field's name to its comma, so neither can grow a wrapper closure or a `.map`
/// without failing -- that is the shape review 32 was demonstrated with, and
/// the reason `REAL_OVERLAY` is a struct literal of two function *references*
/// with no expression anywhere in it. What remains invisible is a
/// `prompt_arm` whose result is computed and then ignored; that is legible in
/// any diff touching these lines. What this guards is the silent revert.
#[cfg(test)]
mod prompt_wiring_tests {
    // SPLIT ACROSS TWO LITERALS, on ONE line, in this crate's established idiom:
    // `include_str!` pulls this module in too, so a whole needle would match its
    // own declaration, and a needle containing a newline would pass on an LF
    // checkout and fail on a CRLF one (this repo has both).
    const ARM_CALL: &str = concat!("prompt_arm", "(&REAL_OVERLAY, window, item.as_ref())");
    const REAL_POSITION: &str = concat!("position: ", "overlay_position,");
    const REAL_SHOW: &str = concat!("show: ", "overlay_ui::show_prompt_overlay,");

    fn source() -> &'static str {
        include_str!("app.rs")
    }

    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn the_counter_finds_calls_that_are_really_there() {
        let planted = concat!("if prompt_arm", "(&REAL_OVERLAY, window, item.as_ref()) {");
        assert_eq!(occurrences(planted, ARM_CALL), 1, "planted: {planted}");
        assert_eq!(occurrences("nothing here", ARM_CALL), 0);

        // The mutations each needle exists for: the call survives, but what it
        // is handed does not. Review 31's was the label, review 32's the
        // placement -- and the placement one is now unwritable at the call site
        // at all, so its modern form is a wrapper around the named function.
        let mutated = concat!("if prompt_arm", "(&REAL_OVERLAY, window, None) {");
        assert_eq!(occurrences(mutated, ARM_CALL), 0, "planted: {mutated}");

        let planted = concat!("    position: ", "overlay_position,");
        assert_eq!(occurrences(planted, REAL_POSITION), 1, "planted: {planted}");
        let mutated = concat!("    position: ", "|hwnd| overlay_position(hwnd).map(|(x, _y)| (x, 0.0)),");
        assert_eq!(occurrences(mutated, REAL_POSITION), 0, "planted: {mutated}");

        let planted = concat!("    show: ", "overlay_ui::show_prompt_overlay,");
        assert_eq!(occurrences(planted, REAL_SHOW), 1, "planted: {planted}");
        let mutated = concat!("    show: ", "|_label, m, p| overlay_ui::show_prompt_overlay(\"\", m, p),");
        assert_eq!(occurrences(mutated, REAL_SHOW), 0, "planted: {mutated}");
    }

    #[test]
    fn handle_match_asks_the_prompt_arm_rather_than_deciding_anything_itself() {
        assert_eq!(
            occurrences(source(), ARM_CALL),
            1,
            "expected {ARM_CALL:?} exactly once in app.rs -- `handle_match`'s Prompt arm. \
             Zero means the overlay's label, credentials or placement are being assembled on \
             that line again, where no test can reach them: the first thing to go wrong there \
             was naming a title-matched Microsoft Store app ApplicationFrameHost.exe, and the \
             second was a `.map` on the placement that pinned every overlay to the top of the \
             screen with the whole suite green"
        );
    }

    #[test]
    fn the_real_presenter_names_the_two_functions_and_computes_nothing() {
        assert_eq!(
            occurrences(source(), REAL_POSITION),
            1,
            "expected {REAL_POSITION:?} exactly once in app.rs -- `REAL_OVERLAY`'s placement \
             field. Zero means it is no longer the real calculation *named*, and the only \
             reason to write anything else there is to alter what it answers -- which is \
             review 32's Important 1, one level down from the call site that no longer \
             accepts it"
        );

        assert_eq!(
            occurrences(source(), REAL_SHOW),
            1,
            "expected {REAL_SHOW:?} exactly once in app.rs -- `REAL_OVERLAY`'s show field. \
             Zero means the overlay is reached through something other than the real function \
             named plainly, and a wrapper here can substitute any of the three values \
             `prompt_arm` was careful to pass through unaltered"
        );
    }
}

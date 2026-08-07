//! Application-level glue: the pieces `main` orchestrates, kept in the library
//! so they're reachable from examples and integration tests rather than being
//! locked inside the binary target.

use crate::app_match::AppMatch;
use crate::injector::ui_automation;
use crate::injector::sequence;
use crate::injector::{Injector, SendInputFiller, UiAutomationFiller};
use crate::key_sequence;
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

/// The overlay's fixed width -- needed here to clamp its position on-screen
/// before the window exists to measure.
///
/// **Imported, not re-declared.** This was a second `const OVERLAY_WIDTH: f32
/// = 396.0` carrying a "must match `overlay_ui`" comment, and a comment is
/// not an enforcement: `overlay_position`'s own tests assert against *this*
/// file's copy, so the pair were self-consistent and blind to each other
/// drifting apart. It is precisely the duplication `adcb346` deleted
/// `OVERLAY_HEIGHT` to be rid of, left standing on the width. There is now
/// one definition, in the module that hands it to `with_inner_size`, and this
/// name is an alias for it.
///
/// **The height is deliberately still not a constant here.** It was `164.0`,
/// the one-row card's height, and the card grows by `ROW_HEIGHT` per choice
/// row: a four-row card is 314pt, so clamping it as if it were 164pt leaves
/// 150pt of a frameless, always-on-top, unscrollable window below the work
/// area -- rows the user cannot see or click. The height comes from
/// [`overlay_ui::overlay_height`] and the row count, from the one place that
/// knows both.
use crate::overlay_ui::OVERLAY_WIDTH;
/// Gap between the field/window edge and the overlay, so it doesn't sit
/// flush against the thing it's about to fill.
const OVERLAY_GAP: f32 = 10.0;

/// Where to place the autofill overlay so it reads as "next to the field"
/// rather than wherever the OS happens to put a new window: just below the
/// focused/matched field if UI Automation can find one, else just outside
/// the matched window's own top-right corner. Clamped to the nearest
/// monitor's work area so it can't land off-screen or under the taskbar.
///
/// `rows` is how many choice rows the card will show; it is what the clamp's
/// idea of "how tall is this window" is computed from.
fn overlay_position(hwnd: isize, rows: usize) -> Option<(f32, f32)> {
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
    Some(clamp_to_monitor(hwnd, x, y, rows))
}

fn window_rect(hwnd: isize) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(HWND(hwnd as *mut core::ffi::c_void), &mut rect).ok()? };
    Some(rect)
}

/// The clamp itself, with the Win32 taken out: given a work area and a
/// proposed top-left corner, where does a card of `rows` rows actually go?
///
/// **Pure, and a function of the row count** -- both deliberately. The old
/// clamp was `work.bottom - 164.0`, a literal that was the whole card's
/// height when the card had one row and is now the height of its top half. A
/// four-row card anchored near the bottom of the work area was clamped to a
/// position leaving 150pt of it below the taskbar; the window has no
/// decorations, no scrollbar and cannot be moved, so those rows were simply
/// unreachable. Nothing could catch that, because the only caller needs a
/// monitor handle.
///
/// The work area is `(left, top, right, bottom)` in pixels, matching
/// `MONITORINFO::rcWork`.
///
/// `.max(left)` / `.max(top)` after the `min` on purpose: on a work area
/// narrower or shorter than the card, the top-left corner is what survives,
/// because the card's header and its first row are worth more than its
/// footer.
pub fn clamp_into_work_area(
    work: (f32, f32, f32, f32),
    x: f32,
    y: f32,
    rows: usize,
) -> (f32, f32) {
    let (left, top, right, bottom) = work;
    let clamped_x = x.min(right - OVERLAY_WIDTH).max(left);
    let clamped_y = y.min(bottom - overlay_ui::overlay_height(rows)).max(top);
    (clamped_x, clamped_y)
}

fn clamp_to_monitor(hwnd: isize, x: f32, y: f32, rows: usize) -> (f32, f32) {
    unsafe {
        let monitor =
            MonitorFromWindow(HWND(hwnd as *mut core::ffi::c_void), MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let work = info.rcWork;
            // Nothing is decided on this line: the arithmetic is
            // `clamp_into_work_area`'s, which is directly tested. This reads
            // the monitor and hands over four numbers.
            return clamp_into_work_area(
                (
                    work.left as f32,
                    work.top as f32,
                    work.right as f32,
                    work.bottom as f32,
                ),
                x,
                y,
                rows,
            );
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
///
/// **Takes the notifier rather than reaching for a global.** The three
/// refusals below reach the user through whatever was handed in; production
/// hands in `sequence::REAL_NOTIFIER` and a test hands in a
/// [`sequence::RecordingNotifier`], so no test in *either* crate can put a
/// task-modal window on a desktop. See [`sequence::Notifier`] for what this
/// replaced and why a `#[cfg(test)]` gate could not do it.
///
/// `&dyn` and not a third type parameter: the call is once per fill, and the
/// alternative would have added a turbofish-shaped burden to every one of this
/// function's and [`handle_match`]'s call sites for nothing.
pub fn fill_from_vault<A: UiAutomationFiller, B: SendInputFiller>(
    cache: &VaultCache,
    injector: &Injector<A, B>,
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
    hwnd: isize,
    choice: FillChoice,
    notifier: &dyn sequence::Notifier,
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
            // The one-time code is fetched **only** when the sequence asks
            // for one. `get_totp` is an HTTP round trip to `bw serve`, and
            // making every fill pay for it -- including the overwhelming
            // majority that store no sequence at all -- would put a network
            // request on the path this app deliberately serves from the
            // in-memory cache so that autofill works with the backend
            // stopped.
            // Asked of the **choice**, not of the stored sequence.
            // `sequence_needs_a_one_time_code` inspects only the sequence, so
            // on `Just(Totp)` for an item that stores no `{TOTP}` it answers
            // `false`, no code is fetched, and the one-time-code row refuses
            // with `Unresolved` one hundred percent of the time.
            let totp = if needs_a_one_time_code(&item, &choice) {
                match cache.bridge().get_totp(item_id) {
                    Ok(code) => code,
                    Err(e) => {
                        log::warn!("could not fetch a one-time code for {item_id}: {e:?}");
                        None
                    }
                }
            } else {
                None
            };

            match fill_action(&item, totp.as_deref(), &choice) {
                Ok(FillAction::Default) => {
                    let (username, password) = credentials_for(&item);
                    // **The one plaintext password on this arm's stack, and
                    // now the one that gets wiped.** `credentials_for` hands
                    // back an owned `String` clone that lives for the whole
                    // arm -- across a UIA attempt, a SendInput fallback and a
                    // refusal -- and dropped it back to the allocator in the
                    // clear. Every other holder of a resolved secret in this
                    // crate zeroizes (`Plan`, `TextRun`, `LoginForm`,
                    // `hello::unlock_password_for`); wrapping at the call site
                    // wipes on every exit including the early return and an
                    // unwind, without changing `credentials_for`'s signature
                    // or its callers.
                    let password = zeroize::Zeroizing::new(password);
                    if username.is_empty() && password.is_empty() {
                        log::warn!(
                            "vault item {item_id} has no login credentials; nothing to fill"
                        );
                        return;
                    }
                    // `Injector::fill` is synchronous and already knows its
                    // own outcome, so this path's semantics are exactly what
                    // they always were. It is routed through the same sink so
                    // that "what counts as a fill" is answered in one place
                    // for both paths rather than by which arm a call sits in.
                    let outcome = match injector.fill(hwnd, &username, &password) {
                        Ok(()) => crate::fill_stats::FillOutcome::Typed,
                        Err(e) => {
                            log::error!("fill failed for item {item_id} into hwnd {hwnd}: {e}");
                            // **A refusal reaches the user here too**, for the
                            // reason spelled out on the `Err(refusal)` arm
                            // below: a fill that quietly does nothing is
                            // indistinguishable from a hotkey that never
                            // registered. `d6ff857` gave this path the
                            // one-fill-at-a-time guard, so it can now refuse
                            // exactly as the Sequence arm can -- and the user
                            // who pressed the hotkey has no idea which of the
                            // two paths their item was on.
                            //
                            // **Only the refusal, and not every `Err`.** The
                            // other ways this returns `Err` are a foreground
                            // that moved between the match and the keystroke
                            // and a failed `SendInput` call: Win32 diagnostics
                            // with no action behind them, and a modal box is
                            // an expensive way to tell a user something they
                            // cannot act on. A UIA failure does not reach here
                            // at all -- `Injector::fill` logs it and falls
                            // back, and a fallback that then succeeds returns
                            // `Ok`. [`ALREADY_TYPING`] is the one error whose
                            // text is written for a person and names the way
                            // out of the state it reports, which is what makes
                            // it worth interrupting them with.
                            if e == crate::injector::ALREADY_TYPING {
                                notifier.refused(&e);
                            }
                            crate::fill_stats::FillOutcome::NotTyped
                        }
                    };
                    fill_outcome_sink(fill_stats, item_id)(outcome);
                }
                Ok(FillAction::Sequence(plan)) => {
                    // **No `record_fill` on the `Ok` arm.** `fill_sequence`
                    // returns as soon as the typing thread has been *started*,
                    // so `Ok(())` cannot tell a sequence that typed a password
                    // from one that refused before the first keystroke or was
                    // abandoned when the user alt-tabbed -- and counting the
                    // latter two floats an item that never filled to the top
                    // of the picker. The sink is what records, from the thread
                    // that knows.
                    let sink = fill_outcome_sink(fill_stats, item_id);
                    if let Err(e) = injector.fill_sequence(hwnd, plan, sink) {
                        log::error!(
                            "auto-type sequence failed for item {item_id} into hwnd {hwnd}: {e}"
                        );
                        // **The same rule the Default arm applies, and for
                        // the same reason.** This used to call
                        // `notifier.refused(&e)` on every `Err`, which
                        // contradicted the argument made twelve lines above:
                        // that a Win32 foreground/`SendInput` diagnostic is
                        // an expensive way to tell a user something they
                        // cannot act on.
                        //
                        // The argument that an abandoned sequence is
                        // different -- the user has watched half a login
                        // typed, so they are owed a word -- is a good one,
                        // and it is **not about this `Err`**.
                        // `Injector::fill_sequence` returns as soon as the
                        // typing thread has been *started*, so the only two
                        // things that can arrive here both happen before a
                        // single keystroke: [`ALREADY_TYPING`], from the
                        // one-at-a-time guard, and
                        // `send_input::ensure_foreground`'s "target window
                        // {hwnd} is not foreground after N attempts" -- an
                        // HWND number and a retry count, raised as an
                        // `MB_TOPMOST` task-modal box in exactly the
                        // situation `RealSendInput::fill_sequence`'s own doc
                        // calls common right after our overlay closes.
                        //
                        // The genuinely abandoned run -- the foreground
                        // re-check refusing between steps, with the username
                        // already typed -- never comes back through this
                        // return value at all. It is reported from inside the
                        // typing thread by `injector::perform`, which calls
                        // `Notifier::refused` on every `Err` from
                        // `sequence::run` and is untouched by this. So the
                        // half-typed login is still reported; what stopped
                        // being reported is a diagnostic about a fill that
                        // typed nothing.
                        if e == crate::injector::ALREADY_TYPING {
                            notifier.refused(&e);
                        }
                    }
                }
                Err(refusal) => {
                    // Reaches the **user**, not only the log. A fill that
                    // quietly does nothing is indistinguishable from a hotkey
                    // that never registered, and the user's next move differs
                    // completely between the two.
                    let message = refusal.message();
                    log::error!("refusing to fill item {item_id}: {message}");
                    notifier.refused(&message);
                }
            }
        }
        Err(e) => log::error!("could not read vault item {item_id} to fill it: {e:?}"),
    }
}

/// **The wire between a fill's outcome and the count**, built once per fill.
///
/// The sequence path performs its typing on another thread, so the answer to
/// "did that fill?" is not available when dispatch returns. This hands that
/// thread a closure that owns everything it needs to answer later: `FillStats`
/// is a `PathBuf` and the item id is copied, so the typing thread holds no
/// borrow of anything the UI owns, and the UI never waits on it. The decision
/// itself is [`crate::fill_stats::counts_as_a_fill`], a pure function with its
/// own tests; this is only the wiring.
///
/// A free function rather than a closure written inline at each call site, so
/// that both fill paths demonstrably share one policy and a test can exercise
/// the wiring without a window, an injector or a vault.
pub fn fill_outcome_sink(
    fill_stats: &crate::fill_stats::FillStats,
    item_id: &str,
) -> crate::injector::OutcomeSink {
    let fill_stats = fill_stats.clone();
    let item_id = item_id.to_string();
    Box::new(move |outcome| {
        if crate::fill_stats::counts_as_a_fill(outcome) {
            fill_stats.record_fill(&item_id);
        }
    })
}

/// The stored auto-type sequence for `item`, or `""` if it has no app match
/// or the match records none.
///
/// The sequence lives on the item's own `deskwarden:app-match` field, so this
/// needs nothing but the item -- which is why [`fill_from_vault`] keeps its
/// signature and why the hotkey path in `main` gets sequences without knowing
/// they exist.
pub fn sequence_for(item: &VaultItem) -> String {
    extract_app_match(item).map(|m| m.sequence).unwrap_or_default()
}

/// What the user asked to be typed. Never persisted; never leaves a fill.
///
/// One screen may want an email only (an SSO stop), the next a username and a
/// password together, a third nothing but a one-time code. The overlay asks
/// *what* to type rather than guessing, and this is the answer it carries.
///
/// **No `Email` row exists, deliberately.** Bitwarden's login object has no
/// email field; the SSO case wants `login.username`, which *is* the address.
/// Inventing a row for a field that is not on the wire would be a row that can
/// never resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillChoice {
    /// The existing default fill: UI Automation's named-field fill first,
    /// `SendInput` username-Tab-password second.
    ///
    /// **Not `parse(DEFAULT_SEQUENCE)`**, for exactly the reason
    /// [`FillAction::Default`] is not: UI Automation fills named fields and a
    /// sequence types at focus, and collapsing the two would delete the UI
    /// Automation path for every item in every existing vault.
    UserTabPass,
    /// One field, alone, through the sequence runner.
    Just(key_sequence::FieldRef),
    /// The item's own stored auto-type sequence.
    Saved,
}

impl FillChoice {
    /// The overlay's row label.
    ///
    /// [`Self::Just`] defers to [`key_sequence::FieldRef::label`] rather than
    /// restating it, so a field renamed there cannot end up named two
    /// different things in two different parts of this UI.
    pub fn label(&self) -> String {
        match self {
            Self::UserTabPass => "Username + Tab + Password".to_string(),
            Self::Just(field) => field.label(),
            Self::Saved => "Saved sequence".to_string(),
        }
    }
}

/// The rows to offer for **this item**, in the order they are shown.
///
/// Pure and presence-only: it asks whether a value is there, never what it is,
/// so building the overlay's rows reads no secret. The presence question
/// itself is [`key_sequence::field_palette`]'s, reused rather than restated --
/// it already answers "what does this item have" and is already tested.
///
/// An item with a stored sequence offers that sequence and nothing else: the
/// user wrote it precisely because the generic rows were not what that app
/// wanted, so offering them back alongside it would be offering the thing they
/// already rejected.
///
/// **Custom `{S:Field}` rows are deliberately absent.** An item may carry any
/// number of custom fields, and an unbounded row count is a geometry hazard
/// for a fixed-size overlay; the sequence builder already covers them. That
/// cap is what makes this list at most four rows long, by construction.
pub fn fill_choices(item: &VaultItem) -> Vec<FillChoice> {
    if !sequence_for(item).is_empty() {
        return vec![FillChoice::Saved];
    }
    let palette = key_sequence::field_palette(item);
    let has = |field: key_sequence::FieldRef| palette.contains(&field);
    let username = has(key_sequence::FieldRef::Username);
    let password = has(key_sequence::FieldRef::Password);

    let mut out = Vec::new();
    if username && password {
        // First, because it is what the overwhelming majority of screens
        // want; the single-field rows below exist for the ones that do not.
        out.push(FillChoice::UserTabPass);
    }
    if username {
        out.push(FillChoice::Just(key_sequence::FieldRef::Username));
    }
    if password {
        out.push(FillChoice::Just(key_sequence::FieldRef::Password));
    }
    if has(key_sequence::FieldRef::Totp) {
        out.push(FillChoice::Just(key_sequence::FieldRef::Totp));
    }
    out
}

/// Whether **this choice** needs an HTTP round-trip for a one-time code.
///
/// Separate from [`fill_action`] because it is the one question that cannot be
/// answered purely: the answer decides whether a request happens.
///
/// This takes the *choice*, not just the item, and that is the whole point of
/// it. [`sequence_needs_a_one_time_code`] inspects only the **stored
/// sequence**, so it answers `false` for [`FillChoice::Just`] of
/// [`key_sequence::FieldRef::Totp`] on an item whose sequence has no
/// `{TOTP}` -- and the one-time-code row would then be shown, be clickable,
/// and refuse with `Refusal::Unresolved` every single time.
pub fn needs_a_one_time_code(item: &VaultItem, choice: &FillChoice) -> bool {
    match choice {
        FillChoice::Just(key_sequence::FieldRef::Totp) => true,
        FillChoice::Just(_) | FillChoice::UserTabPass => false,
        FillChoice::Saved => key_sequence::parse(&sequence_for(item))
            .iter()
            .any(|t| matches!(t, key_sequence::Token::Field(key_sequence::FieldRef::Totp))),
    }
}

/// Whether this item's sequence contains a `{TOTP}`, and so whether a fill of
/// that stored sequence has to go and fetch a code before it can plan.
///
/// A thin wrapper over [`needs_a_one_time_code`] for the stored-sequence
/// choice, kept so the existing stored-sequence callers do not move in this
/// step. New code should ask [`needs_a_one_time_code`] about the choice the
/// user actually made.
pub fn sequence_needs_a_one_time_code(item: &VaultItem) -> bool {
    needs_a_one_time_code(item, &FillChoice::Saved)
}

/// What a fill will actually do.
#[derive(Debug)]
pub enum FillAction {
    /// This item stores no sequence, so the fill is **exactly** what it has
    /// always been: UI Automation's named-field fill, falling back to
    /// username-Tab-password through `SendInput`.
    ///
    /// Not `Sequence(plan_of(DEFAULT_SEQUENCE))`, even though
    /// [`key_sequence::DEFAULT_SEQUENCE`] says that is what the fallback
    /// types. See [`crate::injector::Injector::fill_sequence`] for why the two
    /// are different acts and collapsing them would delete the UI Automation
    /// path for every item in every existing vault.
    Default,
    /// This item stores a sequence, and it planned.
    Sequence(sequence::Plan),
}

/// **The whole of the sequence-versus-default decision, as a pure function.**
///
/// Takes the item and the one value that cannot be read off it (the current
/// one-time code) and answers what to do, with no cache, no injector and no
/// window -- so every branch is reachable from a unit test. The alternative,
/// which this crate has been bitten by ten commits running, is a decision made
/// inside [`fill_from_vault`], which nothing can call.
/// **`choice` is what the user asked for, and it is not a hint.**
/// [`FillChoice::UserTabPass`] answers [`FillAction::Default`], and answers it
/// *before* anything is parsed or planned. It is emphatically **not**
/// `plan(parse(DEFAULT_SEQUENCE))`, even though [`key_sequence::DEFAULT_SEQUENCE`]
/// spells out what the fallback types: `FillAction::Default` tries UI
/// Automation's named-field fill first and only then falls back to `SendInput`,
/// while a plan types at whatever happens to have focus. Collapsing the two
/// reads as a simplification and would silently delete the UI Automation path
/// for every item in every existing vault. See [`FillAction::Default`]'s own
/// doc and [`crate::injector::Injector::fill_sequence`].
///
/// [`FillChoice::Just`] goes through the *same* runner as a stored sequence --
/// [`key_sequence::render`] of that one field, then [`key_sequence::parse`],
/// then [`sequence::plan`] -- rather than constructing a [`sequence::Plan`] by
/// hand. A hand-built plan would not get the runner's rate, its literal
/// escaping or its `Unresolved` refusal, so a field that cannot resolve would
/// type nothing where it should have refused.
///
/// [`FillChoice::Saved`] is exactly what this function did before it took a
/// choice at all, the empty-sequence fallback to `Default` included. That is
/// why it is the choice `main.rs`'s hotkey path names: it has no overlay
/// answer of its own to forward, so `Saved` is the only spelling there that
/// preserves what the hotkey has always done. `handle_match` forwards the
/// user's answer instead, which as of step 5 is deliberately not
/// behaviour-preserving. `app::fill_call_site_tests` holds each file to its
/// own rule.
///
/// **What is NOT pinned, stated plainly.** The `UserTabPass` arm returns
/// before the parse, and that ordering is deliberate for the reason above --
/// but nothing here can catch it being lost. Rewriting the arm as
/// `FillChoice::UserTabPass => String::new(),` and letting it fall through to
/// the `stored.is_empty()` fallback was measured to leave the whole suite
/// green, because it is genuinely behaviour-identical: both routes answer
/// `FillAction::Default`. Nothing ships broken by that spelling; it simply is
/// not a distinction any test in this crate can see, so do not read the
/// guards named above as pinning it. What they pin is that `UserTabPass`
/// answers `Default` **and not a planned `DEFAULT_SEQUENCE`**, which is the
/// difference that costs users the UI Automation path.
pub fn fill_action(
    item: &VaultItem,
    totp: Option<&str>,
    choice: &FillChoice,
) -> Result<FillAction, sequence::Refusal> {
    let stored = match choice {
        // Returns before the parse, deliberately. See the doc above.
        FillChoice::UserTabPass => return Ok(FillAction::Default),
        FillChoice::Just(field) => {
            key_sequence::render(&[key_sequence::Token::Field(field.clone())])
        }
        FillChoice::Saved => sequence_for(item),
    };
    if stored.is_empty() {
        return Ok(FillAction::Default);
    }
    let login = item.login.as_ref();
    let username = login.and_then(|l| l.username.as_deref()).unwrap_or("");
    let password = login.and_then(|l| l.password.as_deref()).map(|p| p.as_str()).unwrap_or("");
    let values = sequence::Resolved {
        username,
        password,
        totp,
        custom: key_sequence::custom_pairs(item),
    };
    sequence::plan(&key_sequence::parse(&stored), &values).map(FillAction::Sequence)
}

/// What focusing a matched window does, beyond arming the hotkey.
///
/// Two variants rather than a bare `bool` at the call site so the arm that
/// *does nothing extra* is a named thing a test can assert on, and so adding a
/// third behaviour later is a compile error at every match rather than a
/// silently-taken `else`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchDisposition {
    /// Raise the overlay. Nothing is typed unless the user clicks Fill on it.
    Prompt,
    /// Do nothing on focus. The armed hotkey is the only way in.
    Nothing,
}

/// **The whole of the match-dispatch decision, as a pure function.**
///
/// Takes the one global preference ([`crate::settings::Settings::prompt_on_match`])
/// and answers what focusing a matched window should do -- with no cache, no
/// injector and no window, so both branches are reachable from a unit test.
/// The alternative, which this crate has been bitten by repeatedly, is a
/// decision made inside [`handle_match`], which nothing can call.
///
/// **The item's own `AppMatch::trigger` is deliberately not an input.** The
/// user asked for one global switch, not a choice per item; see
/// [`crate::app_match::AppMatch::trigger`] for why the field is still read and
/// written but no longer consulted.
///
/// **Neither answer includes filling by itself.** There is no silent-fill
/// disposition: `Prompt` types nothing until the user clicks Fill, and
/// `Nothing` types nothing at all. That is the retirement of the old `Auto`
/// mode, not a rename of it.
pub fn match_disposition(prompt_on_match: bool) -> MatchDisposition {
    if prompt_on_match {
        MatchDisposition::Prompt
    } else {
        MatchDisposition::Nothing
    }
}

/// **Whether a match arms the fill hotkey. Always.**
///
/// A function rather than a literal `Some(...)` inside [`handle_match`] so
/// that "every match arms, in both settings" is a claim a test can make
/// directly, and so substituting `prompt_on_match` for this fails that test.
/// If arming ever became conditional on the preference, turning the prompt off
/// would turn autofill off entirely rather than falling back to the hotkey --
/// which is precisely the fallback the whole design rests on.
pub fn match_arms_hotkey(_prompt_on_match: bool) -> bool {
    true
}

/// **The decision, directly.** Both inputs are driven, and the two answers are
/// asserted to *differ* -- a pair of fixtures that agreed would pass against a
/// `match_disposition` that ignored its argument entirely, which is precisely
/// the mutation that matters here.
#[cfg(test)]
mod match_disposition_tests {
    use super::*;

    #[test]
    fn the_setting_on_prompts_and_the_setting_off_does_nothing() {
        assert_eq!(match_disposition(true), MatchDisposition::Prompt);
        assert_eq!(match_disposition(false), MatchDisposition::Nothing);
        assert_ne!(
            match_disposition(true),
            match_disposition(false),
            "the premise: the preference actually decides something. Equal answers mean \
             `match_disposition` is ignoring its argument, and the switch in preferences does \
             nothing at all"
        );
    }

    /// **The one that would silently switch autofill off.** `false` means
    /// "no prompt", never "no autofill": the hotkey is the fallback the user
    /// is relying on, so it arms for a match in *either* setting. A
    /// `match_arms_hotkey` that returned `prompt_on_match` would leave the
    /// first assertion green and this one red.
    #[test]
    fn every_match_arms_the_hotkey_whatever_the_setting_says() {
        assert!(match_arms_hotkey(true), "a prompted match still arms the hotkey");
        assert!(
            match_arms_hotkey(false),
            "with the prompt off the hotkey is the ONLY way anything is typed. If a match \
             stops arming here, Ctrl+Alt+B fills nothing and turning the prompt off has \
             turned autofill off entirely"
        );
        assert_eq!(
            match_arms_hotkey(true),
            match_arms_hotkey(false),
            "arming is deliberately NOT a function of the preference"
        );
    }

    /// Neither disposition is a fill. Stated as a test because the retired
    /// `Auto` mode was exactly a third variant here, and a future one added
    /// without thinking is how it would come back.
    #[test]
    fn no_disposition_fills_by_itself() {
        for on in [true, false] {
            match match_disposition(on) {
                // Raises the overlay; `handle_match` fills only if
                // `prompt_arm` answers `true`, which is the user's click.
                MatchDisposition::Prompt => {}
                MatchDisposition::Nothing => {}
            }
        }
    }
}

/// Dispatches a freshly foregrounded, matched window.
///
/// **Always arms `(item_id, hwnd)`** and returns it, so the main loop's
/// separate `fill_hotkey_pressed` check can fill it later once the user
/// actually presses the fill hotkey -- see [`match_arms_hotkey`]. On top of
/// that, and only when [`match_disposition`] says so, it raises the overlay
/// and fills if the user clicks Fill.
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
    prompt_on_match: bool,
    window: &crate::window_watch::ForegroundEvent,
    notifier: &dyn sequence::Notifier,
) -> Option<(String, isize)> {
    let hwnd = window.hwnd;
    match match_disposition(prompt_on_match) {
        MatchDisposition::Nothing => {}
        MatchDisposition::Prompt => {
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
            // (`prompt_subject` is where the password is deliberately never
            // touched -- see its doc.)
            //
            // **The item is not bound here any more.** It is the lookup
            // closure's answer, reduced to a `PromptSubject` and dropped
            // inside `prompt_arm_for` before the overlay opens -- a `let item`
            // on this line would be alive for the whole time the modal card is
            // on screen, which is however long the user takes to decide.
            //
            // Every decision about the overlay -- what it says, and where it
            // opens -- is made in `prompt_arm_for`/`prompt_arm`, which a test
            // drives with a recording presenter and a recording lookup; this
            // function only names the cache read and supplies the real
            // presenter. **No value the overlay is given is computed on this
            // line**, which is the point: this line is the one no test can
            // reach, and an argument written here is an argument nothing can
            // check (review 32's Important 1).
            let lookup = || cache.items().into_iter().find(|i| i.id == item_id);
            if let Some(choice) = prompt_arm_for(&REAL_OVERLAY, window, lookup) {
                // `choice` is the user's answer, and the fill is OF it. The
                // `debug_assert_eq!(choice, FillChoice::Saved)` that stood
                // here is gone because the thing it was standing in for has
                // landed: `fill_from_vault` can now be told which choice to
                // run, so the answer is forwarded instead of being asserted
                // away. As of step 5 this is no longer a distinction without
                // a difference: `prompt_choices` offers the rows the item
                // really supports, so `UserTabPass` and `Just(field)` are
                // both reachable here and naming a literal `Saved` would type
                // something other than the row the user clicked.
                fill_from_vault(cache, injector, fill_stats, item_id, hwnd, choice, notifier);
            }
        }
    }
    // Unconditional, and outside the match: the hotkey is the fallback the
    // whole design rests on, so it arms for a match the user was prompted
    // about just as much as for one they were not.
    match_arms_hotkey(prompt_on_match).then(|| (item_id.to_string(), hwnd))
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
    /// The rows the overlay offers, in order; the first is the primary, and
    /// the one Enter takes.
    pub choices: Vec<FillChoice>,
}

/// The rows to offer for a Prompt-mode match: **the rows this item actually
/// supports**, from [`fill_choices`].
///
/// This is the line the whole feature was built towards. Every step before it
/// widened the machinery -- the overlay draws N rows, sizes itself for N,
/// answers *which* row, and [`fill_from_vault`] runs the row it is told to --
/// while this function kept production at the single [`FillChoice::Saved`] row
/// the app has always shown, so that each step was provably invisible. It is
/// no longer invisible: an item with a username and a password now offers
/// `Username + Tab + Password` first and each field on its own beneath it, and
/// an SSO screen's email-only item offers exactly the one row it can fill.
///
/// **A cache miss stays fillable.** `None` (the item could not be read back)
/// answers the empty list, and an empty list is not "no rows": both
/// [`overlay_ui::draw_overlay_card_rows`] and [`overlay_ui::overlay_height`]
/// treat it as the single matched-credential row the overlay has always
/// painted, which answers [`FillChoice::Saved`] -- exactly the old behaviour,
/// for exactly the case that used to be all of them. An item with no
/// username, no password, no one-time code and no stored sequence takes the
/// same path.
///
/// It stays a named function rather than being inlined into
/// [`prompt_subject`] so that "what production offers" is a question a test
/// can ask directly, without a presenter.
pub fn prompt_choices(item: Option<&VaultItem>) -> Vec<FillChoice> {
    item.map(fill_choices).unwrap_or_default()
}

/// Everything the overlay needs about the matched item, and **nothing else**.
///
/// The point of this type is what it does *not* contain. `handle_match` used
/// to hold a SECOND copy of the matched item -- a whole cloned [`VaultItem`],
/// login object, plaintext password and TOTP seed included -- for the entire
/// lifetime of a modal overlay the user may leave on screen for minutes, and
/// only drop it afterwards. The secrets are [`zeroize::Zeroizing`], so that
/// copy was wiped on that drop rather than released in the clear, but the
/// window in which a duplicate existed was as long as the user was undecided,
/// which is the part worth removing.
///
/// **This is one copy fewer, not zero copies.** The [`VaultCache`] itself
/// holds the whole `Vec<VaultItem>` -- plaintext `Zeroizing`
/// passwords included -- for the entire unlocked session, and the fill's
/// lookup below clones out of it. So the claim is narrow and deliberate: the
/// prompt path no longer *adds* a copy, and the copy the fill makes lives for
/// the length of a fill rather than the length of a decision. It is not "the
/// password is not resident while a window is open"; while the vault is
/// unlocked it is resident regardless.
///
/// So the item is now reduced to this the moment it is read and dropped
/// before the overlay opens. Both fields are presence-or-label only: the
/// display name and username are what the card already showed, and
/// [`fill_choices`] asks only *whether* a value is there. The fill re-resolves
/// the item from the cache after the choice, as it already did.
pub struct PromptSubject {
    /// What the card says it is offering; `None` on a cache miss.
    pub matched: Option<overlay_ui::OverlayMatch>,
    /// The rows to offer, in order; the first is the primary.
    pub choices: Vec<FillChoice>,
}

/// Reduces a matched item to [`PromptSubject`] -- the only thing that crosses
/// into the prompt.
///
/// The username is read straight off the login object rather than through
/// [`credentials_for`]: that helper also clones the plaintext password into a
/// `String` this path has no use for. The overlay never shows a password, so
/// it should never hold one.
pub fn prompt_subject(item: Option<&VaultItem>) -> PromptSubject {
    PromptSubject {
        matched: item.map(|item| {
            let username = item.login.as_ref().and_then(|l| l.username.clone());
            overlay_ui::OverlayMatch {
                item_name: item.name.clone(),
                username: username.filter(|u| !u.is_empty()),
            }
        }),
        choices: prompt_choices(item),
    }
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
/// The `subject` is passed in rather than looked up here, because the lookup
/// is the one part that needs the cache; `position` likewise, because
/// computing it needs Win32. What is left is the part that can be got wrong
/// silently.
///
/// **It takes a [`PromptSubject`], not a `&VaultItem`.** That is the type-level
/// half of the drop-early guarantee: this function, and everything downstream
/// of it, cannot hold the item across the overlay because it is never handed
/// one. [`prompt_arm_for`] is the behavioural half.
pub fn prompt_request<'a>(
    window: &'a crate::window_watch::ForegroundEvent,
    subject: PromptSubject,
    position: Option<(f32, f32)>,
) -> PromptRequest<'a> {
    PromptRequest {
        label: window_label(&window.exe_name, &window.title),
        matched: subject.matched,
        position,
        choices: subject.choices,
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
    /// OS choose. `rows` is the card's row count, which is what its height --
    /// and therefore the bottom clamp -- is computed from.
    fn position(&self, hwnd: isize, rows: usize) -> Option<(f32, f32)>;
    /// Shows the overlay and returns **which** row the user picked, or `None`
    /// if they dismissed it.
    fn show(
        &self,
        label: &str,
        matched: Option<&overlay_ui::OverlayMatch>,
        position: Option<(f32, f32)>,
        choices: &[FillChoice],
    ) -> Option<FillChoice>;
}

/// A [`PromptPresenter`] that is nothing but the two functions it forwards to.
///
/// **Function pointers rather than a hand-written `impl` per presenter**, so
/// that the production presenter can be a struct literal naming two functions
/// -- see [`REAL_OVERLAY`]. The forwarding below is the only code in the way,
/// and unlike a Win32 body it is directly driven by
/// `an_fn_presenter_forwards_to_the_two_functions_it_was_built_from`.
pub struct FnPresenter {
    /// Asked where the overlay for this window, at this row count, goes.
    pub position: fn(isize, usize) -> Option<(f32, f32)>,
    /// Asked to put it on screen; answers which row the user picked.
    pub show: fn(
        &str,
        Option<&overlay_ui::OverlayMatch>,
        Option<(f32, f32)>,
        &[FillChoice],
    ) -> Option<FillChoice>,
}

impl PromptPresenter for FnPresenter {
    fn position(&self, hwnd: isize, rows: usize) -> Option<(f32, f32)> {
        (self.position)(hwnd, rows)
    }

    fn show(
        &self,
        label: &str,
        matched: Option<&overlay_ui::OverlayMatch>,
        position: Option<(f32, f32)>,
        choices: &[FillChoice],
    ) -> Option<FillChoice> {
        (self.show)(label, matched, position, choices)
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
/// it, and answer **which row** the user picked -- `None` if they dismissed it.
///
/// The choice, not a bare `true`: "a fill was authorized" and "this is what to
/// type" are different facts, and the second is the one the caller needs once
/// the card offers more than one row.
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
    subject: PromptSubject,
) -> Option<FillChoice> {
    // The placement is asked for BEFORE the request is built, because how
    // tall the window is -- and therefore how far down the work area its top
    // may be -- is a function of the row count. The count asked about is the
    // subject's OWN list, the same one that is shown a line later -- not a
    // second call to `prompt_choices`, which would be two answers about one
    // card and could disagree.
    // `the_placement_is_asked_about_the_number_of_rows_that_are_shown` holds
    // the two together.
    let position = presenter.position(window.hwnd, subject.choices.len());
    let PromptRequest { label, matched, position, choices } =
        prompt_request(window, subject, position);
    presenter.show(label, matched.as_ref(), position, &choices)
}

/// [`prompt_arm`] with the vault lookup in front of it, so that **the item is
/// dropped before the overlay opens**.
///
/// This exists to make an ordering claim executable. `handle_match` cannot be
/// called by a test -- it needs a `VaultCache`, an `Injector` and a
/// `FillStats`, and it opens a real window -- so "the item is reduced and let
/// go before the modal card goes up" written inline there would be a comment,
/// and the exact shape of defect this crate keeps finding is a correct
/// statement at a place nothing can observe. Here, the lookup is a closure and
/// the presenter is a parameter, so a test can hand in a value whose `Drop`
/// records itself and a presenter that records itself, and read the order.
///
/// Generic over what the lookup answers (`I: Borrow<VaultItem>`) purely so
/// that test value can exist; production instantiates it at `VaultItem` and
/// the monomorphised body is the same three lines.
///
/// The item lives for exactly one statement. It is not bound to a `let` and
/// then `drop`ped, because a binding is something a later edit can quietly
/// move past the `prompt_arm` call; a temporary that never escapes its
/// statement cannot outlive it however the lines beneath are rearranged.
pub fn prompt_arm_for<P: PromptPresenter, I: std::borrow::Borrow<VaultItem>>(
    presenter: &P,
    window: &crate::window_watch::ForegroundEvent,
    lookup: impl FnOnce() -> Option<I>,
) -> Option<FillChoice> {
    // The item is a temporary of THIS statement: it is dropped -- and its
    // `Zeroizing` password and TOTP seed wiped -- at the semicolon, which is
    // before the presenter is touched at all, let alone before the card is on
    // screen. `the_item_is_dropped_before_the_overlay_opens` reads that order.
    let subject = prompt_subject(lookup().as_ref().map(|item| item.borrow()));
    prompt_arm(presenter, window, subject)
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
        let request = prompt_request(&w, prompt_subject(None), None);
        assert_eq!(request.label, "Speedtest");
    }

    #[test]
    fn an_ordinary_matched_window_is_prompted_for_under_its_executable() {
        // The positive control: a `prompt_request` that always answered with
        // the title would pass the test above and fail this one.
        let w = window("Ledgerline.exe", "Ledgerline -- Invoices");
        let request = prompt_request(&w, prompt_subject(None), None);
        assert_eq!(request.label, "Ledgerline.exe");
    }

    #[test]
    fn the_prompt_names_the_credentials_it_is_offering() {
        let item = login_item("Ledgerline", "denis@example.com");
        let w = window("Ledgerline.exe", "Ledgerline");
        let request = prompt_request(&w, prompt_subject(Some(&item)), None);

        let matched = request.matched.expect("design 2a: never a bare \"fill something?\"");
        assert_eq!(matched.item_name, "Ledgerline");
        assert_eq!(matched.username.as_deref(), Some("denis@example.com"));
    }

    #[test]
    fn an_item_with_no_usable_username_still_prompts_by_name() {
        // An empty username is "no username", not a blank line in the overlay.
        let item = login_item("Ledgerline", "");
        let w = window("Ledgerline.exe", "Ledgerline");
        let request = prompt_request(&w, prompt_subject(Some(&item)), None);
        let matched = request.matched.expect("the item is still named");
        assert_eq!(matched.item_name, "Ledgerline");
        assert_eq!(matched.username, None);

        // And a cache miss is not fatal to the prompt at all.
        let request = prompt_request(&w, prompt_subject(None), None);
        assert!(request.matched.is_none());
    }

    #[test]
    fn the_position_it_is_handed_is_the_position_it_asks_for() {
        // The overlay's placement is computed from Win32 and cannot be
        // computed here; what CAN be got wrong is dropping it on the way
        // through, which lands the overlay wherever the OS likes.
        let w = window("a.exe", "A");
        let request = prompt_request(&w, prompt_subject(None), Some((120.0, 340.0)));
        assert_eq!(request.position, Some((120.0, 340.0)));
        let request = prompt_request(&w, prompt_subject(None), None);
        assert_eq!(request.position, None);
    }

    // ---- Where a card of N rows is allowed to open ----

    /// A 1920x1040 work area with the taskbar at the bottom, at the origin.
    const WORK: (f32, f32, f32, f32) = (0.0, 0.0, 1920.0, 1040.0);

    /// **The bug the row count exists to prevent.** With the clamp still
    /// reading a literal `164.0`, a four-row card anchored near the bottom is
    /// pinned at y = 876 and is 314pt tall, so it ends at 1190 -- 150pt below
    /// the work area, on a frameless always-on-top window with no title bar,
    /// no scrollbar and no way to move it. The bottom rows cannot be seen or
    /// clicked.
    #[test]
    fn a_four_row_card_anchored_at_the_bottom_stays_inside_the_work_area() {
        let height = overlay_ui::overlay_height(4);
        assert_eq!(height, 314.0, "the fixture is a card that is really taller");

        // Anchored well below where a card this tall can start.
        let (_x, y) = clamp_into_work_area(WORK, 200.0, 1000.0, 4);
        assert!(
            y + height <= WORK.3,
            "a 4-row card opened at y = {y} ends at {} , past the work area's {}",
            y + height,
            WORK.3
        );
        assert_eq!(y, WORK.3 - height);

        // The positive control: the assertion above can fail. This is what
        // the old, row-blind clamp answered for the same input.
        let stale = 1000.0_f32.min(WORK.3 - 164.0).max(WORK.1);
        assert!(
            stale + height > WORK.3,
            "the control is not a failing case, so the test above proves nothing"
        );
        assert_ne!(y, stale);
    }

    #[test]
    fn a_one_row_card_is_clamped_exactly_where_it_always_was() {
        // The other half: the historical geometry must not move. 164.0 is
        // still the right number -- for a card that really has one row.
        let (x, y) = clamp_into_work_area(WORK, 5000.0, 1000.0, 1);
        assert_eq!(y, WORK.3 - 164.0);
        // Fully qualified, deliberately: the point of the width being imported
        // is that there is ONE of it, and a re-declared local `const
        // OVERLAY_WIDTH` in this file would be picked up by a bare
        // `OVERLAY_WIDTH` here just as it would by the clamp -- leaving the
        // two agreeing with each other and blind to `overlay_ui`. Naming the
        // module makes this an assertion about the width the WINDOW is built
        // at, which is the only width that matters.
        assert_eq!(x, WORK.2 - crate::overlay_ui::OVERLAY_WIDTH);
    }

    #[test]
    fn a_card_that_already_fits_is_not_moved() {
        // Otherwise "clamped on screen" could be implemented as "always at the
        // bottom-right", which passes the two tests above.
        assert_eq!(clamp_into_work_area(WORK, 300.0, 400.0, 4), (300.0, 400.0));
    }

    #[test]
    fn a_card_is_never_pushed_off_the_top_or_left_by_the_clamp() {
        // A work area smaller than the card: the top-left corner wins, so the
        // header and the FIRST row are what survive.
        let tiny = (100.0, 200.0, 300.0, 260.0);
        assert_eq!(clamp_into_work_area(tiny, 0.0, 0.0, 4), (100.0, 200.0));
    }

    /// Every row count the card can have (`fill_choices` caps at four) fits,
    /// with the loop's own visit count asserted -- a loop that ran zero times
    /// would otherwise pass green.
    #[test]
    fn no_card_the_overlay_can_show_is_clamped_off_the_bottom() {
        let mut checked = 0;
        for rows in 1..=4 {
            let height = overlay_ui::overlay_height(rows);
            let (_x, y) = clamp_into_work_area(WORK, 200.0, 9999.0, rows);
            assert!(y + height <= WORK.3, "{rows} rows: {y} + {height}");
            assert!(y >= WORK.1);
            checked += 1;
        }
        assert_eq!(checked, 4, "the loop must have visited all four row counts");
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
        /// What `show` returns -- WHICH row the user picked, or `None` for a
        /// dismissal. Not a bool: the caller types what this says.
        answer: Option<FillChoice>,
        /// What `position` answers, standing in for the Win32 calculation.
        placement: Option<(f32, f32)>,
        asked_about: std::cell::Cell<Option<isize>>,
        /// The row count `position` was asked about, so a placement computed
        /// for a card of the wrong height is visible to a test.
        asked_rows: std::cell::Cell<Option<usize>>,
        shown: std::cell::RefCell<Vec<Shown>>,
        /// The choice list each `show` was handed.
        offered: std::cell::RefCell<Vec<Vec<FillChoice>>>,
    }

    impl PromptPresenter for RecordingPresenter {
        fn position(&self, hwnd: isize, rows: usize) -> Option<(f32, f32)> {
            self.asked_about.set(Some(hwnd));
            self.asked_rows.set(Some(rows));
            self.placement
        }

        fn show(
            &self,
            label: &str,
            matched: Option<&overlay_ui::OverlayMatch>,
            position: Option<(f32, f32)>,
            choices: &[FillChoice],
        ) -> Option<FillChoice> {
            self.shown.borrow_mut().push((
                label.to_string(),
                matched.map(|m| (m.item_name.clone(), m.username.clone())),
                position,
            ));
            self.offered.borrow_mut().push(choices.to_vec());
            self.answer.clone()
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

        prompt_arm(&presenter, &w, prompt_subject(Some(&item)));

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

        prompt_arm(&presenter, &w, prompt_subject(None));

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
            answer: Some(FillChoice::Saved),
            ..Default::default()
        };
        assert!(prompt_arm(&filled, &w, prompt_subject(None)).is_some());
        let dismissed = RecordingPresenter::default();
        assert!(prompt_arm(&dismissed, &w, prompt_subject(None)).is_none());
    }

    /// **The choice the overlay answered is the choice the caller receives.**
    ///
    /// Asserted on the choice, not on "a fill happened": a `prompt_arm` that
    /// collapsed its answer to `Some(FillChoice::Saved)` -- the historical
    /// behaviour, and therefore the mutation that looks harmless -- passes
    /// every `is_some()` test in this file and types the user's password into
    /// a field they asked for their username in.
    #[test]
    fn the_choice_the_overlay_answered_is_the_choice_the_caller_receives() {
        let w = window("Ledgerline.exe", "Ledgerline");
        let answered = FillChoice::Just(key_sequence::FieldRef::Password);
        let presenter = RecordingPresenter {
            answer: Some(answered.clone()),
            ..Default::default()
        };

        assert_eq!(prompt_arm(&presenter, &w, prompt_subject(None)), Some(answered.clone()));
        // The control: the answer is not the one a collapse would produce, so
        // the assertion above cannot pass by accident.
        assert_ne!(answered, FillChoice::Saved);
        assert_ne!(answered, FillChoice::UserTabPass);
    }

    #[test]
    fn dismissing_the_overlay_answers_none() {
        // The positive control for the test above: an arm that fabricated a
        // choice when the user dismissed the card would pass that one and
        // fill on Esc.
        let w = window("Ledgerline.exe", "Ledgerline");
        let presenter = RecordingPresenter {
            answer: None,
            ..Default::default()
        };
        assert_eq!(prompt_arm(&presenter, &w, prompt_subject(None)), None);
        // ... and it really did open the overlay, so `None` is the user's
        // answer and not a card that was never shown.
        assert_eq!(presenter.shown.borrow().len(), 1);
    }

    /// The overlay is offered the rows [`prompt_choices`] decides on, and is
    /// *placed* for that same number of rows. Two counts that must agree: a
    /// card placed as if it had one row and drawn with four hangs off the
    /// bottom of the work area.
    #[test]
    fn the_placement_is_asked_about_the_number_of_rows_that_are_shown() {
        let item = login_item("Ledgerline", "denis@example.com");
        let w = window("Ledgerline.exe", "Ledgerline");
        let presenter = RecordingPresenter::default();

        prompt_arm(&presenter, &w, prompt_subject(Some(&item)));

        let offered = presenter.offered.borrow();
        assert_eq!(offered.len(), 1, "the overlay is offered rows exactly once");
        assert!(!offered[0].is_empty(), "a card with no rows is not a card");
        assert_eq!(
            presenter.asked_rows.get(),
            Some(offered[0].len()),
            "the placement was computed for a card of a different height than the one \
             that is drawn"
        );
    }

    // ---- Step 5: production offers the rows the item really supports ----

    /// A password that appears nowhere else in this crate, so finding it
    /// anywhere the overlay can see is unambiguous.
    const PROMPT_SECRET: &str = "correct-horse-STAPLE-battery-42";

    /// An item with both credentials and **no stored sequence**, so
    /// [`fill_choices`] takes its presence branch rather than answering
    /// `Saved`. `login_item`'s sibling: that one has a username and nothing
    /// else, which is the SSO shape.
    fn both_credentials_item(name: &str, username: &str) -> VaultItem {
        VaultItem {
            name: name.to_string(),
            login: Some(crate::vault_bridge::LoginData {
                username: Some(username.to_string()),
                password: Some(PROMPT_SECRET.to_string().into()),
                ..crate::vault_bridge::LoginData::default()
            }),
            ..item("1", None)
        }
    }

    /// **The SSO screen the user described**: "mabl has two options -- user\\pass
    /// and SSO where only email". An item that carries an email and no
    /// password can fill exactly one thing, and the card must offer exactly
    /// that -- not a `Username + Tab + Password` row that would type a Tab and
    /// then nothing, and not a password row for a password that is not there.
    ///
    /// `assert_eq!` on the whole vector, not `contains`: the defect this
    /// forbids is an EXTRA row, and `contains` cannot see one.
    #[test]
    fn an_sso_item_offers_exactly_one_row_in_production() {
        let sso = login_item("Mabl SSO", "denis@example.com");
        assert_eq!(
            prompt_choices(Some(&sso)),
            vec![FillChoice::Just(key_sequence::FieldRef::Username)]
        );

        // The fixture really is the SSO shape and not just an item nothing
        // can be said about: the same function answers three rows for an item
        // that does have a password, so the one row above is a fact about
        // this item rather than a constant.
        assert_eq!(
            prompt_choices(Some(&both_credentials_item("Ledgerline", "denis@example.com"))).len(),
            3
        );
    }

    /// **The common case leads with the row Enter takes.** The overlay's
    /// primary row is `choices[0]`, and Enter fills it without the user
    /// looking; on a user+password screen that must be
    /// `Username + Tab + Password`. Reordering the list is behaviour-identical
    /// for every mouse click and wrong for every keyboard user.
    ///
    /// Driven through the **production** path -- `prompt_arm` with a recording
    /// presenter -- rather than by calling `fill_choices` directly, so it also
    /// fails if `prompt_choices` stops being what the overlay is handed.
    #[test]
    fn an_item_with_both_credentials_leads_with_username_tab_password() {
        let item = both_credentials_item("Ledgerline", "denis@example.com");
        let w = window("Ledgerline.exe", "Ledgerline");
        let presenter = RecordingPresenter::default();

        prompt_arm(&presenter, &w, prompt_subject(Some(&item)));

        let offered = presenter.offered.borrow();
        assert_eq!(offered.len(), 1, "the overlay is offered rows exactly once");
        assert_eq!(
            offered[0],
            vec![
                FillChoice::UserTabPass,
                FillChoice::Just(key_sequence::FieldRef::Username),
                FillChoice::Just(key_sequence::FieldRef::Password),
            ]
        );
        // Said again as the property, not as the list: this is the assertion
        // that a reordering has to break, and it is the one the doc names.
        assert_eq!(offered[0][0], FillChoice::UserTabPass);
        // ... and it is not the only row, or "first" would be vacuous.
        assert!(offered[0].len() > 1);
    }

    /// **The card is placed and built for the rows production really gives
    /// it.** `prompt_choices` widening from one row to three moves the window
    /// the OS is asked for by 150pt; a placement or a viewport still computed
    /// for one row is a frameless, unresizable, unscrollable card with its
    /// bottom rows off the work area -- and every row-content assertion above
    /// stays green, because the rows are all correctly *drawn*, just not all
    /// on screen.
    #[test]
    fn the_overlay_is_sized_for_the_rows_production_actually_gives_it() {
        let w = window("Ledgerline.exe", "Ledgerline");
        let sso = login_item("Mabl SSO", "denis@example.com");
        let both = both_credentials_item("Ledgerline", "denis@example.com");

        let mut heights = Vec::new();
        let mut checked = 0;
        for item in [&sso, &both] {
            let presenter = RecordingPresenter::default();
            prompt_arm(&presenter, &w, prompt_subject(Some(item)));

            let offered = presenter.offered.borrow();
            assert_eq!(offered.len(), 1);
            let rows = offered[0].len();
            assert_eq!(
                presenter.asked_rows.get(),
                Some(rows),
                "the placement was computed for a card of a different height than the one \
                 that is drawn"
            );
            // Observed out of the viewport production really asks the OS for,
            // not recomputed from `overlay_height` here.
            let requested = overlay_ui::overlay_options(&offered[0], None)
                .viewport
                .inner_size
                .expect("the overlay viewport must request an inner size at all");
            assert_eq!(requested.y, overlay_ui::overlay_height(rows));
            heights.push(requested.y);
            checked += 1;
        }
        assert_eq!(checked, 2, "the loop must have visited both fixtures");
        // The control: the two fixtures disagree, so a viewport pinned to one
        // row -- the historical size, and the mutation that looks like a
        // simplification -- cannot pass this test.
        assert_ne!(heights[0], heights[1]);
        assert_eq!(heights[0], overlay_ui::overlay_height(1));
    }

    /// A [`PromptPresenter`] that keeps **every `&str` it is handed**, from
    /// every argument that carries one.
    #[derive(Default)]
    struct StringSpy {
        seen: std::cell::RefCell<Vec<String>>,
    }

    impl PromptPresenter for StringSpy {
        fn position(&self, _hwnd: isize, _rows: usize) -> Option<(f32, f32)> {
            None
        }

        fn show(
            &self,
            label: &str,
            matched: Option<&overlay_ui::OverlayMatch>,
            _position: Option<(f32, f32)>,
            choices: &[FillChoice],
        ) -> Option<FillChoice> {
            let mut seen = self.seen.borrow_mut();
            seen.push(label.to_string());
            if let Some(m) = matched {
                seen.push(m.item_name.clone());
                seen.extend(m.username.clone());
            }
            // The labels are what the rows SAY, which is the other string
            // surface the overlay renders.
            seen.extend(choices.iter().map(|c| c.label()));
            None
        }
    }

    /// **Nothing plaintext-secret crosses the prompt.** The overlay is a
    /// window on another app's screen, and the strings it is given end up in
    /// egui's galley cache and in this process's memory for as long as the
    /// card is up. `fill_choices` is presence-only and the card is handed
    /// labels; this asserts that end-to-end rather than by reading the code.
    #[test]
    fn no_secret_value_is_handed_to_the_presenter() {
        let item = both_credentials_item("Ledgerline", "denis@example.com");
        let w = window("Ledgerline.exe", "Ledgerline");
        let spy = StringSpy::default();

        prompt_arm(&spy, &w, prompt_subject(Some(&item)));

        let seen = spy.seen.borrow();
        // Without this, a presenter that was handed nothing at all -- or a
        // `show` that was never called -- would pass the loop below trivially.
        // Deliberately "not empty" and not a row count: this test's claim is
        // about what the strings CONTAIN, and coupling it to how many rows
        // production offers would make it fail for reasons that are not
        // leaks, which is how a security guard gets read as noise.
        assert!(
            !seen.is_empty(),
            "the spy was handed no strings at all, so the loop below proves nothing"
        );
        // And it is the right strings: the fixture's own username is there,
        // so the spy is looking at the surface the overlay draws.
        assert!(seen.iter().any(|s| s == "denis@example.com"));
        assert!(seen.iter().any(|s| s == "Ledgerline"));

        // The fixture's password is a value nothing on this path has any use
        // for. Substring, not equality: a label that embedded it, or a
        // `format!` that pasted it into the secondary line, must fail too.
        for s in seen.iter() {
            assert!(
                !s.contains(PROMPT_SECRET),
                "the overlay was handed {s:?}, which contains the item's plaintext password"
            );
        }
        // The control for the check itself: it can fail.
        assert!(format!("fills {PROMPT_SECRET}").contains(PROMPT_SECRET));
    }

    // ---- The item is let go before the card goes up ----

    /// A `VaultItem` wrapper whose `Drop` writes into a shared log, so that
    /// "the item was released" is an event with a position in a sequence
    /// rather than an assertion about source text.
    struct DropRecorder {
        item: VaultItem,
        log: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
    }

    impl std::borrow::Borrow<VaultItem> for DropRecorder {
        fn borrow(&self) -> &VaultItem {
            &self.item
        }
    }

    impl Drop for DropRecorder {
        fn drop(&mut self) {
            self.log.borrow_mut().push("item released");
        }
    }

    /// A presenter that writes its two calls into the same log.
    struct OrderingPresenter {
        log: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
    }

    impl PromptPresenter for OrderingPresenter {
        fn position(&self, _hwnd: isize, _rows: usize) -> Option<(f32, f32)> {
            self.log.borrow_mut().push("placement asked");
            None
        }

        fn show(
            &self,
            _label: &str,
            _matched: Option<&overlay_ui::OverlayMatch>,
            _position: Option<(f32, f32)>,
            _choices: &[FillChoice],
        ) -> Option<FillChoice> {
            self.log.borrow_mut().push("overlay shown");
            None
        }
    }

    /// **The security claim of this step, as behaviour.** `handle_match` used
    /// to bind the whole cloned item -- login object, plaintext password and
    /// TOTP seed -- and hold it for as long as the modal overlay was on
    /// screen, which is as long as the user took to decide. It is now reduced
    /// to a `PromptSubject` of labels and dropped before the card exists.
    ///
    /// Behavioural, not pinned: the item's `Drop` and the presenter's two
    /// calls write into one log and the order is read off it.
    #[test]
    fn the_item_is_dropped_before_the_overlay_opens() {
        let w = window("Ledgerline.exe", "Ledgerline");
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let presenter = OrderingPresenter { log: log.clone() };

        let answer = prompt_arm_for(&presenter, &w, || {
            Some(DropRecorder {
                item: both_credentials_item("Ledgerline", "denis@example.com"),
                log: log.clone(),
            })
        });
        assert_eq!(answer, None, "the ordering presenter dismisses");

        assert_eq!(
            *log.borrow(),
            vec!["item released", "placement asked", "overlay shown"],
            "the matched item must be released BEFORE the overlay is placed or shown -- \
             anything else leaves the plaintext password resident for the whole life of a \
             modal window"
        );
    }

    /// The positive control for the test above, and the regression it exists
    /// for: an item held across the call logs its release last. Without this,
    /// an assertion that merely found the three events in some order -- or a
    /// `DropRecorder` that recorded at the wrong moment -- would look like
    /// proof.
    #[test]
    fn an_item_held_across_the_prompt_is_visibly_released_last() {
        let w = window("Ledgerline.exe", "Ledgerline");
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let presenter = OrderingPresenter { log: log.clone() };

        let held = DropRecorder {
            item: both_credentials_item("Ledgerline", "denis@example.com"),
            log: log.clone(),
        };
        // `&VaultItem` also satisfies the lookup's bound, which is exactly how
        // a caller keeps the item alive: the arm can only drop what it owns.
        prompt_arm_for(&presenter, &w, || Some(&held.item));
        assert_eq!(*log.borrow(), vec!["placement asked", "overlay shown"]);
        drop(held);
        assert_eq!(
            *log.borrow(),
            vec!["placement asked", "overlay shown", "item released"]
        );
    }

    /// A cache miss must still raise a fillable card. `prompt_choices(None)`
    /// is the empty list, and the empty list is not "no rows": the overlay
    /// paints the single matched-credential row it always painted and answers
    /// [`FillChoice::Saved`] for it, which is the pre-step-5 behaviour for the
    /// case that used to be all of them.
    #[test]
    fn an_item_that_could_not_be_read_back_still_offers_the_row_it_always_did() {
        assert_eq!(prompt_choices(None), Vec::new());
        let w = window("Ledgerline.exe", "Ledgerline");
        let presenter = RecordingPresenter::default();
        prompt_arm(&presenter, &w, prompt_subject(None));

        let offered = presenter.offered.borrow();
        assert_eq!(offered.len(), 1, "the overlay is still shown");
        assert!(offered[0].is_empty());
        // The empty list is one painted row, and one row's worth of window --
        // read out of `overlay_ui`, whose own tests own that promise.
        assert_eq!(
            presenter.asked_rows.get(),
            Some(0),
            "the row count asked about is the list's length, not a fabricated 1"
        );
        assert_eq!(
            overlay_ui::overlay_height(0),
            overlay_ui::overlay_height(1),
            "an empty list must still be sized for the one row it paints"
        );
    }

    // ---- and the adapter the production presenter is built out of ----

    /// Statics rather than captured state, because [`FnPresenter`] holds plain
    /// `fn` pointers: that is the property that makes [`REAL_OVERLAY`] a
    /// struct literal with no expression in it. Only the one test below uses
    /// these.
    static ASKED_HWND: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());
    static FORWARDED: std::sync::Mutex<Vec<Shown>> = std::sync::Mutex::new(Vec::new());

    static ASKED_ROWS: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
    static OFFERED: std::sync::Mutex<Vec<Vec<FillChoice>>> = std::sync::Mutex::new(Vec::new());

    fn recording_position(hwnd: isize, rows: usize) -> Option<(f32, f32)> {
        ASKED_HWND.lock().unwrap().push(hwnd);
        ASKED_ROWS.lock().unwrap().push(rows);
        Some((11.0, 22.0))
    }

    fn recording_show(
        label: &str,
        matched: Option<&overlay_ui::OverlayMatch>,
        position: Option<(f32, f32)>,
        choices: &[FillChoice],
    ) -> Option<FillChoice> {
        FORWARDED.lock().unwrap().push((
            label.to_string(),
            matched.map(|m| (m.item_name.clone(), m.username.clone())),
            position,
        ));
        OFFERED.lock().unwrap().push(choices.to_vec());
        Some(FillChoice::Just(key_sequence::FieldRef::Totp))
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

        assert_eq!(presenter.position(4242, 3), Some((11.0, 22.0)));
        assert_eq!(*ASKED_HWND.lock().unwrap(), vec![4242]);
        assert_eq!(
            *ASKED_ROWS.lock().unwrap(),
            vec![3],
            "the row count is forwarded, not replaced with a default"
        );

        let matched = overlay_ui::OverlayMatch {
            item_name: "Ledgerline".to_string(),
            username: Some("denis@example.com".to_string()),
        };
        let offered = vec![
            FillChoice::UserTabPass,
            FillChoice::Just(key_sequence::FieldRef::Username),
        ];
        assert_eq!(
            presenter.show("Ledgerline.exe", Some(&matched), Some((3.0, 4.0)), &offered),
            Some(FillChoice::Just(key_sequence::FieldRef::Totp)),
            "the answer is forwarded back unaltered -- not collapsed to the first row"
        );
        assert_eq!(*OFFERED.lock().unwrap(), vec![offered]);

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
    const ARM_CALL: &str = concat!("prompt_arm_for", "(&REAL_OVERLAY, window, lookup)");
    /// The cache read `handle_match` hands the arm, as a closure it does NOT
    /// call itself. `|| cache.items()...` bound to a `let item` and passed as
    /// `item.as_ref()` -- the shape this line had until step 5 -- keeps the
    /// plaintext item alive for the whole time the overlay is on screen, and
    /// every behavioural test in this file still passes, because the overlay
    /// is told the same things either way. The difference is residency, and
    /// residency has no observable effect at all from outside this line.
    const LOOKUP: &str =
        concat!("let lookup = || cache.items()", ".into_iter().find(|i| i.id == item_id);");
    const REAL_POSITION: &str = concat!("position: ", "overlay_position,");
    const REAL_SHOW: &str = concat!("show: ", "overlay_ui::show_prompt_overlay,");
    /// `handle_match` asks [`super::match_disposition`] the question, and asks
    /// it about the value it was HANDED. `match_disposition(true)` -- the
    /// preference read out of the path, the prompt back on for everyone who
    /// turned it off -- compiles, and every behavioural test of
    /// `match_disposition` stays green, because that function is still
    /// perfectly correct and is simply no longer being asked the right thing.
    const DISPOSITION_CALL: &str = concat!("match_disposition", "(prompt_on_match)");
    /// The same hazard for the arming half: `match_arms_hotkey(false)` (or a
    /// literal `None`) switches the hotkey fallback off for every match.
    const ARMS_CALL: &str = concat!("match_arms_hotkey", "(prompt_on_match).then");
    /// **`prompt_arm`'s answer is the CONDITION, not a statement.**
    ///
    /// [`ARM_CALL`] is the bare call, so it still matches inside
    /// `if { prompt_arm(..); true } {` -- the overlay opens, its answer is
    /// thrown away, and the fill happens whether the user clicked Fill or
    /// Dismiss. That mutant was run against this suite and survived it: 1646
    /// lib and 133 bin tests green, with "nothing is typed without a user
    /// action" broken. The module doc above called that shape "legible in any
    /// diff", which is true and is not the same as caught.
    ///
    /// So the needle is the whole `if ... {`, and the mutant no longer
    /// contains it.
    ///
    /// **Retargeted when `prompt_arm` began answering `Option<FillChoice>`.**
    /// The arm is now `if let Some(choice) = prompt_arm(...) {`, and the old
    /// needle -- `if prompt_arm(...) {` -- stopped matching anything. A stale
    /// source pin does not fail: it just quietly stops pinning, and the
    /// guarantee it exists for ("nothing is typed without a user action")
    /// would have gone back to being unheld the moment the signature changed.
    /// The needle binds the answer to a NAME, so the two mutants it must
    /// catch -- discarding the answer, and substituting a hardcoded choice for
    /// it -- both stop containing it.
    const GUARDED_ARM: &str =
        concat!("if let Some(choice) = prompt_arm_for", "(&REAL_OVERLAY, window, lookup) {");

    fn source() -> &'static str {
        include_str!("app.rs")
    }

    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.matches(needle).count()
    }

    #[test]
    fn the_counter_finds_calls_that_are_really_there() {
        let planted = concat!("if prompt_arm_for", "(&REAL_OVERLAY, window, lookup) {");
        assert_eq!(occurrences(planted, ARM_CALL), 1, "planted: {planted}");
        assert_eq!(occurrences("nothing here", ARM_CALL), 0);

        // The mutations each needle exists for: the call survives, but what it
        // is handed does not. Review 31's was the label, review 32's the
        // placement -- and the placement one is now unwritable at the call site
        // at all, so its modern form is a wrapper around the named function.
        let mutated = concat!("if prompt_arm_for", "(&REAL_OVERLAY, window, || None) {");
        assert_eq!(occurrences(mutated, ARM_CALL), 0, "planted: {mutated}");

        // And the same for the retargeted `GUARDED_ARM`, whose whole job is to
        // stop matching when the user's answer stops gating the fill. Both
        // mutants below were run for real (see the ledger): the first opens
        // the overlay and fills regardless, the second keeps the `if let` but
        // types a choice the user did not pick.
        let planted =
            concat!("            if let Some(choice) = prompt_arm_for", "(&REAL_OVERLAY, window, lookup) {");
        assert_eq!(occurrences(planted, GUARDED_ARM), 1, "planted: {planted}");
        let discarded = concat!("            prompt_arm_for", "(&REAL_OVERLAY, window, lookup);");
        assert_eq!(occurrences(discarded, GUARDED_ARM), 0, "planted: {discarded}");
        let hardcoded = concat!(
            "            if prompt_arm_for",
            "(&REAL_OVERLAY, window, lookup).is_some() { let choice = FillChoice::UserTabPass;"
        );
        assert_eq!(occurrences(hardcoded, GUARDED_ARM), 0, "planted: {hardcoded}");
        // The pre-step-5 arm shape, so a revert to it -- which is a revert to
        // holding the item across the overlay -- is not silently equivalent.
        assert_eq!(
            occurrences(source(), concat!("prompt_arm", "(&REAL_OVERLAY, window, item.as_ref())")),
            0,
            "the pre-step-5 arm shape is back in app.rs: the item is bound at `handle_match`'s \
             unreachable line and held for the whole life of the overlay again, and \
             `GUARDED_ARM` no longer describes the code it guards"
        );

        // `LOOKUP`'s own controls. The regression it exists for keeps the
        // arm call intact and only changes what is passed, so `ARM_CALL`
        // above cannot see it -- but a `let item =` on that line can only be
        // there in order to outlive the statement.
        let planted =
            concat!("            let lookup = || cache.items()", ".into_iter().find(|i| i.id == item_id);");
        assert_eq!(occurrences(planted, LOOKUP), 1, "planted: {planted}");
        let held = concat!("            let item = cache.items()", ".into_iter().find(|i| i.id == item_id);");
        assert_eq!(occurrences(held, LOOKUP), 0, "planted: {held}");

        let planted = concat!("    position: ", "overlay_position,");
        assert_eq!(occurrences(planted, REAL_POSITION), 1, "planted: {planted}");
        let mutated = concat!("    position: ", "|hwnd| overlay_position(hwnd).map(|(x, _y)| (x, 0.0)),");
        assert_eq!(occurrences(mutated, REAL_POSITION), 0, "planted: {mutated}");

        let planted = concat!("    show: ", "overlay_ui::show_prompt_overlay,");
        assert_eq!(occurrences(planted, REAL_SHOW), 1, "planted: {planted}");
        let mutated = concat!("    show: ", "|_label, m, p| overlay_ui::show_prompt_overlay(\"\", m, p),");
        assert_eq!(occurrences(mutated, REAL_SHOW), 0, "planted: {mutated}");

        let planted = concat!("match match_disposition", "(prompt_on_match) {");
        assert_eq!(occurrences(planted, DISPOSITION_CALL), 1, "planted: {planted}");
        let mutated = concat!("match match_disposition", "(true) {");
        assert_eq!(occurrences(mutated, DISPOSITION_CALL), 0, "planted: {mutated}");

        let planted = concat!("match_arms_hotkey", "(prompt_on_match).then(|| (item_id.to_string(), hwnd))");
        assert_eq!(occurrences(planted, ARMS_CALL), 1, "planted: {planted}");
        let mutated = concat!("match_arms_hotkey", "(false).then(|| (item_id.to_string(), hwnd))");
        assert_eq!(occurrences(mutated, ARMS_CALL), 0, "planted: {mutated}");
    }

    /// **The preference reaches the decision, and the decision is the one
    /// made.** `handle_match` needs a `VaultCache`, an `Injector` and a
    /// `FillStats` and then opens a real overlay window, so nothing can call
    /// it; these two lines are where a correct pure function gets asked the
    /// wrong question, which is this crate's signature defect.
    #[test]
    fn handle_match_asks_the_disposition_about_the_value_it_was_handed() {
        assert_eq!(
            occurrences(source(), DISPOSITION_CALL),
            1,
            "expected {DISPOSITION_CALL:?} exactly once in app.rs -- `handle_match`'s one \
             decision. Zero means the global prompt preference is no longer what decides \
             whether a matched window raises the overlay: a literal in its place turns the \
             prompt on for every user who switched it off, or off for every user who did not, \
             with the whole suite green"
        );
    }

    /// **The fill waits for the user's click.** See [`GUARDED_ARM`] for the
    /// mutant this exists for and for why [`ARM_CALL`] alone did not catch it.
    #[test]
    fn the_prompt_arms_answer_is_what_gates_the_fill() {
        assert_eq!(
            occurrences(source(), GUARDED_ARM),
            1,
            "expected {GUARDED_ARM:?} exactly once in app.rs. Zero means the overlay's answer \
             is no longer the condition on the fill below it, so a matched window would type \
             the user's password whether they clicked Fill or Dismiss -- and, because the \
             fill falls back to blind SendInput, into whatever holds focus"
        );
    }

    /// **The item is handed over as a lookup, not held.** The behavioural half
    /// of this claim is
    /// `prompt_wiring_tests`' sibling `the_item_is_dropped_before_the_overlay_opens`,
    /// which drives `prompt_arm_for` with a recording lookup; what it cannot
    /// reach is `handle_match` choosing to bind the item on its own line and
    /// pass a closure that ignores it, which reinstates the residency this
    /// step exists to close while leaving the arm's behaviour identical.
    #[test]
    fn handle_match_hands_the_prompt_a_lookup_rather_than_a_held_item() {
        assert_eq!(
            occurrences(source(), LOOKUP),
            1,
            "expected {LOOKUP:?} exactly once in app.rs -- `handle_match`'s cache read. Zero \
             means the matched item is bound at that line again and therefore alive, \
             plaintext password and TOTP seed included, for the whole time the modal overlay \
             is on screen: as long as the user takes to decide"
        );
    }

    #[test]
    fn handle_match_arms_the_hotkey_through_the_function_that_says_it_always_does() {
        assert_eq!(
            occurrences(source(), ARMS_CALL),
            1,
            "expected {ARMS_CALL:?} exactly once in app.rs -- `handle_match`'s return. Zero \
             means the arming is conditional again, and with the prompt switched off that is \
             an app that fills nothing at all: Ctrl+Alt+B is the only remaining way in"
        );
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

/// **Every call site of the fill passes the choice ITS OWN FILE is entitled
/// to pass, and the two files are not entitled to the same one.**
///
/// The claim here is about the *call sites*, which no behavioural test in this
/// crate can reach: `handle_match`'s call needs a real overlay and `main`'s
/// needs a real hotkey. A call site switched to `FillChoice::UserTabPass`
/// compiles, and it retires the stored auto-type sequence of every item in
/// every existing vault -- while still typing a username and a password, so
/// every "was something typed" assertion here stays green.
///
/// **What is guaranteed is different in each file, so the rule is per file.**
///
/// * `main.rs`'s hotkey fill has no overlay and therefore no answer of its own
///   to forward, so the only choice that preserves what the hotkey has always
///   done is `FillChoice::Saved` *named there*: that is exactly what
///   `fill_action`'s body was before it took a choice, empty-sequence fallback
///   included. A binding is not accepted there, whatever it is called.
/// * `app.rs`'s `handle_match` is the opposite: as of step 5 it is
///   deliberately **not** behaviour-preserving. `prompt_choices` offers the
///   rows the item really supports, so the guarantee is that the user's OWN
///   answer is forwarded -- the binding `choice` -- and a literal
///   `FillChoice::Saved` there is the step-5 bug (click the one-time-code row,
///   get your password typed). A named literal is not accepted there.
///
/// This used to be one two-element disjunction applied to both files, and the
/// `, choice,` half of it -- which exists only for `handle_match` -- was
/// nothing but a spelling away from being accepted in `main.rs` too. A
/// `let choice = FillChoice::UserTabPass;` on the hotkey path passed the guard
/// with the whole suite green. `the_two_files_rules_are_not_interchangeable`
/// is what stops the two rules being merged back together, and the per-file
/// call COUNT is what stops a *new* call site in either file being quietly
/// waved through by the other file's rule: a file's count is stated in the
/// same table as its accepted form, and a file with no entry is a hard error
/// rather than a skip.
///
/// Scanned rather than pinned as a whole-call needle, so that reformatting,
/// renaming a local or adding a call cannot silently stop it pinning: it finds
/// EVERY production call and checks EVERY one, and asserts it found the number
/// it expects.
///
/// Scanned rather than pinned as a whole-call needle, so that reformatting,
/// renaming a local or adding a call cannot silently stop it pinning: it finds
/// EVERY production call and checks EVERY one, and asserts it found the number
/// it expects.
///
/// **Gated test modules are cut out first, and that is not tidiness.** The
/// tests in this file deliberately drive `FillChoice::UserTabPass` and
/// `FillChoice::Just(..)` through `fill_from_vault` -- that is what proves the
/// new choices reach the two arms at all -- so a scan of the whole file would
/// find them and either fail forever or have to be weakened until it accepted
/// them, and a scan weakened to accept a test's `UserTabPass` would accept
/// production's too. What is being claimed is about what SHIPS.
#[cfg(test)]
mod fill_call_site_tests {
    // Split across two literals so `include_str!` of this very file does not
    // match this declaration, in this crate's established idiom. No needle
    // here contains a newline: a `\r\n` needle is vacuous on an LF checkout
    // and vice versa, and this repo has no `.gitattributes`.
    const CALL: &str = concat!("fill_from_vault", "(");

    /// The choice each file's production calls must pass, and how many calls
    /// that file has -- **one row per file, and the two rows differ**.
    ///
    /// The form and the count live in the same row deliberately. A rule that
    /// was global and a count that was per file is what let `main.rs` inherit
    /// `app.rs`'s `, choice,`; keeping them together means a call site that
    /// appears in either file has to be given a row's worth of thought, and
    /// the count assertion below fires before anything is accepted.
    ///
    /// The count is also what stops a call site DELETED (rather than changed)
    /// passing this by leaving nothing to check.
    const RULES: [(&str, &str, usize); 2] = [
        // Forwards the overlay's own answer; see the module doc.
        ("app.rs", ", choice,", 1),
        // Names the preserving choice; there is no answer to forward.
        ("main.rs", concat!("FillChoice", "::Saved"), 1),
    ];

    /// The row for `name`, or a hard failure.
    ///
    /// **Not a skip and not a fallback.** A file scanned with no rule of its
    /// own would otherwise be checked against nothing, or -- worse, and this
    /// is the defect being closed -- against whatever rule happened to be
    /// lying around for another file.
    fn rule(name: &str) -> (&'static str, usize) {
        RULES
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, form, count)| (*form, *count))
            .unwrap_or_else(|| {
                panic!(
                    "{name} is scanned but has no rule of its own. Add a row to RULES saying \
                     what THAT file's call sites are entitled to pass -- do not reach for \
                     another file's row, which is exactly how the hotkey path came to accept \
                     a binding it has no business having"
                )
            })
    }

    fn sources() -> [(&'static str, &'static str); 2] {
        [("app.rs", include_str!("app.rs")), ("main.rs", include_str!("main.rs"))]
    }

    /// `source` with every top-level `#[cfg(test)]` module removed.
    ///
    /// Line-based and anchored at column zero: a `#[cfg(test)]` on its own
    /// line with no indentation, up to and including the next `}` on its own
    /// line with no indentation. Every gated module in these two files has
    /// that shape, and `the_cut_really_removes_the_tests` is what checks that
    /// claim rather than assuming it.
    ///
    /// **`trim_end`, not a bare comparison.** `str::lines` strips the `\n` and
    /// leaves the `\r`, so on this repo's CRLF working tree every line would
    /// end in a carriage return, nothing would ever equal `"#[cfg(test)]"`,
    /// the cut would silently do nothing and this whole module would go back
    /// to scanning the tests it exists to exclude -- passing, because the
    /// counts would then be whatever they are. Writing `"#[cfg(test)]\r\n"`
    /// into a needle is the opposite trap: vacuous on an LF checkout.
    fn production_only(source: &str) -> String {
        let mut out = String::new();
        let mut skipping = false;
        for line in source.lines() {
            let flat = line.trim_end();
            if !skipping && flat == "#[cfg(test)]" {
                skipping = true;
                continue;
            }
            if skipping {
                if flat == "}" {
                    skipping = false;
                }
                continue;
            }
            out.push_str(flat);
            out.push('\n');
        }
        assert!(!skipping, "a gated module never closed at column zero; the cut is unreliable");
        out
    }

    /// The argument list of every call to the fill in `source`, as the
    /// text between the opening paren and the first `);` after it.
    fn argument_lists(source: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = source;
        while let Some(at) = rest.find(CALL) {
            let after = &rest[at + CALL.len()..];
            let end = after.find(");").expect("a call that never closes");
            out.push(after[..end].to_string());
            rest = &after[end..];
        }
        out
    }

    #[test]
    fn the_scanner_finds_calls_and_can_tell_the_two_forms_apart() {
        let planted = concat!(
            "fill_from_vault", "(cache, injector, fill_stats, item_id, hwnd, choice, notifier);\n",
            "fill_from_vault", "(&cache, &stats, 4242, FillChoice", "::UserTabPass, &notifier);"
        );
        let args = argument_lists(planted);
        assert_eq!(args.len(), 2, "the scanner did not find both planted calls: {args:?}");
        let (app_form, _) = rule("app.rs");
        assert!(args[0].contains(app_form), "{}", args[0]);
        assert!(
            !args[1].contains(app_form),
            "the scanner accepts a call site that forwards nothing: {}",
            args[1]
        );
        assert_eq!(argument_lists("nothing here").len(), 0);
    }

    /// **The two files' rules are not interchangeable, in both directions.**
    ///
    /// This is the guard on the guard. The defect being closed was one rule
    /// accepting either spelling everywhere, so the cheapest way to "fix" a
    /// future failure of the test below is to widen a row until it accepts the
    /// other file's form again -- and that would be silent. So the forms are
    /// pinned as mutually rejecting: `main.rs`'s form must reject the shape
    /// `app.rs` really passes, and `app.rs`'s form must reject the shape
    /// `main.rs` really passes. Merge them and this fails.
    ///
    /// The two shapes are written the way the production call sites are
    /// written, whitespace included, so this is a positive control for each
    /// row as well as a negative one for the other.
    #[test]
    fn the_two_files_rules_are_not_interchangeable() {
        let forwards = "item_id, hwnd, choice, notifier";
        let names_it = concat!("hwnd,\ndeskwarden::app::FillChoice", "::Saved,\n&notifier,");
        let (app_form, app_count) = rule("app.rs");
        let (main_form, main_count) = rule("main.rs");

        assert!(app_form != main_form, "the two rows have collapsed into one rule again");
        assert!(app_count > 0 && main_count > 0, "a file is scanned but has nothing to check");

        assert!(app_form.contains("choice"), "app.rs's rule stopped being about forwarding");
        assert!(forwards.contains(app_form), "app.rs's rule rejects what app.rs really passes");
        assert!(
            !names_it.contains(app_form),
            "app.rs's rule accepts a call that names a literal instead of forwarding"
        );

        assert!(names_it.contains(main_form), "main.rs's rule rejects what main.rs really passes");
        assert!(
            !forwards.contains(main_form),
            "main.rs's rule accepts a forwarded binding. The hotkey has no answer to forward, \
             so a binding there can hold ANY choice -- `let choice = FillChoice::UserTabPass;` \
             is the whole regression, and it is one line"
        );

        assert_eq!(RULES.len(), sources().len(), "a file is scanned with no rule, or vice versa");
    }

    /// **The cut is a real cut**, in both directions: it removes the gated
    /// modules and keeps the production body. A `production_only` that
    /// returned its input unchanged -- which is exactly what a `\r`-blind
    /// comparison produces -- passes nothing here.
    #[test]
    fn the_cut_really_removes_the_tests() {
        let mut kept_something = 0;
        for (name, source) in sources() {
            let cut = production_only(source);
            assert!(cut.len() < source.len(), "{name}: the cut removed nothing at all");
            // A LINE equal to the attribute, for the reason given below: both
            // files name it in production prose.
            let attrs = |s: &str| {
                s.lines().filter(|l| l.trim() == concat!("#[", "test]")).count()
            };
            assert_eq!(
                attrs(&cut),
                0,
                "{name}: a test survived the cut, so this module is scanning tests"
            );
            // A LINE equal to the gate, not the string anywhere: production
            // doc comments in this file name the attribute in prose, and a
            // guard that fires on prose is a guard that gets deleted.
            assert_eq!(
                cut.lines().filter(|l| l.trim_end() == concat!("#[cfg", "(test)]")).count(),
                0,
                "{name}: a gated module survived the cut"
            );
            assert!(attrs(source) > 0, "{name}: has no tests, so removing them proves nothing");
            kept_something += 1;
        }
        assert_eq!(kept_something, 2, "a source was skipped");
        // And the production body really is still there: the two call sites
        // this module exists to check survive the cut.
        assert!(production_only(include_str!("app.rs")).contains(concat!("fn handle", "_match")));
        assert!(production_only(include_str!("main.rs"))
            .contains(concat!("fill_hotkey", "_pressed")));
    }

    #[test]
    fn every_fill_call_site_passes_the_choice_its_own_file_is_entitled_to() {
        let mut checked = 0;
        for (name, source) in sources() {
            let args = argument_lists(&production_only(source));
            let (form, expected) = rule(name);
            assert_eq!(
                args.len(),
                expected,
                "{name} has {} production calls to the fill, not {expected}. A call site was \
                 added or deleted: give it a row's worth of thought and update RULES \
                 deliberately -- a new call site must not inherit the other file's rule",
                args.len()
            );
            assert!(expected > 0, "{name} is scanned but has no call to check");
            for arg in &args {
                assert!(
                    arg.contains(form),
                    "a fill in {name} does not pass {form:?}, which is the ONLY choice that \
                     file's call sites are entitled to pass. In main.rs that means the hotkey \
                     no longer names the choice that preserves what it has always done, and \
                     every stored auto-type sequence in every existing vault is retired on \
                     that path; in app.rs it means `handle_match` no longer forwards the \
                     user's own answer, so the row they clicked is not the row that is typed. \
                     Its arguments are ({arg})"
                );
                checked += 1;
            }
        }
        assert_eq!(
            checked,
            RULES.iter().map(|(_, _, c)| c).sum::<usize>(),
            "the loop did not check every call it found"
        );
        assert!(checked > 0, "the loop visited nothing at all");
    }

    /// The production call in `handle_match` forwards the overlay's OWN
    /// answer, in the order it forwards it.
    ///
    /// The scan above now rejects a named literal in `app.rs` on its own, so
    /// this is no longer the only thing standing between `handle_match` and
    /// `FillChoice::Saved`; it is the stronger claim, and worth keeping as
    /// one. The scan asks only that the text `, choice,` appear somewhere in
    /// the argument list -- which a call that forwarded `choice` into the
    /// *wrong* position would also satisfy -- and this pins the whole list.
    /// Naming a literal there is the step-5 bug: the user clicks the
    /// one-time-code row and their password is typed.
    #[test]
    fn the_prompt_path_forwards_the_answer_it_was_given_rather_than_naming_one() {
        let needle =
            concat!("fill_from_vault", "(cache, injector, fill_stats, item_id, hwnd, choice, ");
        assert_eq!(
            production_only(include_str!("app.rs")).matches(needle).count(),
            1,
            "handle_match no longer hands the overlay's own answer to the fill"
        );
    }
}

/// **The wiring**: that a stored sequence actually reaches the typing path,
/// and that an item without one still takes the fill it always took.
///
/// The decision itself is [`fill_action`], a pure function tested above it.
/// These tests exist because this crate's signature defect is a correct
/// decision that nothing consults: reviews of ten consecutive commits each
/// found one, and one of those was a credential leak reached exactly that way.
/// So the two calls in [`fill_from_vault`] are pinned separately -- delete
/// either and a test here fails while every test of `fill_action` stays green.
#[cfg(test)]
mod fill_dispatch_tests {
    use super::*;
    use crate::app_match::{TriggerMode, APP_MATCH_FIELD_NAME};
    use crate::injector::sequence::{Plan, Step};
    use crate::vault_bridge::{LoginData, VaultBridge, VaultField};
    use std::sync::{Arc, Mutex};

    /// **The username and password disagree, and neither contains the other.**
    /// A fixture whose two values agree cannot tell which one was typed.
    const USER: &str = "work.account@contoso.com";
    const PASS: &str = "Zq7-tremulous-BADGER";

    fn item_with(sequence: &str) -> VaultItem {
        let m = AppMatch {
            sequence: sequence.to_string(),
            ..AppMatch::for_process("msedge.exe", TriggerMode::Auto)
        };
        VaultItem {
            id: "item-1".into(),
            name: "Work Microsoft 365".into(),
            fields: vec![
                VaultField {
                    name: Some(APP_MATCH_FIELD_NAME.into()),
                    value: Some(m.to_field_value()),
                    other: serde_json::Map::new(),
                },
                VaultField {
                    name: Some("PIN".into()),
                    value: Some("4821".into()),
                    other: serde_json::Map::new(),
                },
            ],
            login: Some(LoginData {
                username: Some(USER.into()),
                password: Some(PASS.to_string().into()),
                ..LoginData::default()
            }),
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

    // -- fill_action, the pure decision -------------------------------------

    #[test]
    fn an_item_with_no_sequence_takes_the_default_fill() {
        // Not a plan of DEFAULT_SEQUENCE: UI Automation fills named fields
        // and a sequence types at focus, and those are different acts.
        assert!(matches!(
            fill_action(&item_with(""), None, &FillChoice::Saved),
            Ok(FillAction::Default)
        ));
    }

    #[test]
    fn an_item_with_no_app_match_at_all_takes_the_default_fill() {
        let mut item = item_with("");
        item.fields.clear();
        assert!(matches!(fill_action(&item, None, &FillChoice::Saved), Ok(FillAction::Default)));
    }

    #[test]
    fn a_stored_sequence_is_planned_from_the_items_own_values() {
        let item = item_with("{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}{S:PIN}");
        let Ok(FillAction::Sequence(plan)) = fill_action(&item, None, &FillChoice::Saved) else {
            panic!("a stored sequence must plan");
        };
        let typed: Vec<String> = plan
            .steps()
            .iter()
            .filter_map(|s| match s {
                Step::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        // Username first, then password and the custom field on the second
        // screen: reading the password where the username belongs would give
        // the same *shape* and a broken login.
        assert_eq!(typed, vec![USER.to_string(), format!("{PASS}4821")]);
    }

    #[test]
    fn a_sequence_whose_field_is_missing_refuses_rather_than_falling_back_to_the_default() {
        // Silently degrading to username-Tab-password would type a password
        // into a screen that is asking for an email address.
        let item = item_with("{S:NOPE}");
        assert!(matches!(
            fill_action(&item, None, &FillChoice::Saved),
            Err(sequence::Refusal::Unresolved(_))
        ));
    }

    #[test]
    fn the_one_time_code_is_handed_to_the_plan() {
        let item = item_with("{TOTP}");
        let Ok(FillAction::Sequence(plan)) =
            fill_action(&item, Some("246813"), &FillChoice::Saved)
        else {
            panic!("plans with a code");
        };
        assert_eq!(
            plan.steps(),
            [Step::Text { text: "246813".into(), rate: sequence::DEFAULT_RATE }]
        );
        // Positive control: without the code, the same sequence refuses.
        assert!(fill_action(&item_with("{TOTP}"), None, &FillChoice::Saved).is_err());
    }

    #[test]
    fn only_a_sequence_that_uses_a_one_time_code_asks_for_one() {
        // This is what keeps an HTTP round-trip off the path that this app
        // deliberately serves from the in-memory cache.
        assert!(sequence_needs_a_one_time_code(&item_with("{USERNAME}{TAB}{TOTP}")));
        assert!(!sequence_needs_a_one_time_code(&item_with("{USERNAME}{TAB}{PASSWORD}")));
        assert!(!sequence_needs_a_one_time_code(&item_with("")));
    }

    // -- fill_action, the choice ------------------------------------------

    /// **The guard against the one damaging edit available in this step.**
    ///
    /// `FillChoice::UserTabPass` must answer `FillAction::Default` -- UI
    /// Automation's named-field fill first, `SendInput` second -- and NOT
    /// `plan(parse(DEFAULT_SEQUENCE))`. The two look interchangeable because
    /// `DEFAULT_SEQUENCE` spells out what the fallback types, and collapsing
    /// them reads as a simplification. It would delete the UI Automation path
    /// for every item in every existing vault, silently: every behavioural
    /// test that only asks "was something typed" would stay green.
    #[test]
    fn the_default_choice_is_still_the_default_fill_and_not_a_planned_sequence() {
        assert!(matches!(
            fill_action(&item_with(""), None, &FillChoice::UserTabPass),
            Ok(FillAction::Default)
        ));
        // **And on an item that DOES store a sequence.** Without this the
        // collapse is only tested where `Saved` also answers `Default`, so a
        // `UserTabPass` arm that fell through to the stored-sequence body
        // would pass. The fixture is asserted to disagree first, so the claim
        // above is not being made against an item that plans nothing.
        let stored = item_with("{USERNAME}{TAB}{PASSWORD}");
        assert!(
            matches!(fill_action(&stored, None, &FillChoice::Saved), Ok(FillAction::Sequence(_))),
            "the fixture stores no sequence, so it cannot tell the two choices apart"
        );
        assert!(matches!(
            fill_action(&stored, None, &FillChoice::UserTabPass),
            Ok(FillAction::Default)
        ));
    }

    /// **Each narrow row types its own field and nothing else.**
    ///
    /// Asserted on the typed TEXT, not on the step count: `Just(Username)` and
    /// `Just(Password)` produce the same *shape* -- one `Step::Text` -- so a
    /// mapping with the two swapped types a password into a username box and
    /// every shape-only assertion stays green. The four expected values are
    /// pairwise distinct by construction (see `USER`/`PASS`), which is what
    /// makes the text assertion able to fail.
    #[test]
    fn each_narrow_choice_plans_only_the_field_it_names() {
        use key_sequence::FieldRef;
        // No stored sequence: so a plan here can only have come from the
        // choice. `Saved` on this same item answers `Default`, asserted below.
        let item = item_with("");
        let code = "135790";
        let cases = [
            (FieldRef::Username, USER.to_string()),
            (FieldRef::Password, PASS.to_string()),
            (FieldRef::Totp, code.to_string()),
            (FieldRef::Custom("PIN".to_string()), "4821".to_string()),
        ];

        // The choices under test are pairwise distinct, and so are the values
        // they must type. Either collapsing would make the loop below assert
        // one thing four times.
        for (i, (a, _)) in cases.iter().enumerate() {
            for (b, _) in cases.iter().skip(i + 1) {
                assert_ne!(a, b, "two cases name the same field");
            }
        }
        for (i, (_, a)) in cases.iter().enumerate() {
            for (_, b) in cases.iter().skip(i + 1) {
                assert_ne!(a, b, "two cases expect the same text: {a}");
            }
        }
        assert!(
            matches!(fill_action(&item, Some(code), &FillChoice::Saved), Ok(FillAction::Default)),
            "the fixture stores a sequence, so a plan below need not have come from the choice"
        );

        let mut checked = 0;
        for (field, expected) in &cases {
            let choice = FillChoice::Just(field.clone());
            let Ok(FillAction::Sequence(plan)) = fill_action(&item, Some(code), &choice) else {
                panic!("{choice:?} must plan through the sequence runner");
            };
            assert_eq!(
                plan.steps(),
                [Step::Text { text: expected.clone(), rate: sequence::DEFAULT_RATE }],
                "{choice:?} typed the wrong thing"
            );
            checked += 1;
        }
        assert_eq!(checked, cases.len(), "the loop skipped a choice");
    }

    /// A narrow row whose field is not on the item **refuses**, exactly as a
    /// stored sequence naming a missing field does -- it does not silently
    /// degrade to the default fill, which would type a password into a screen
    /// asking for something else.
    #[test]
    fn a_narrow_choice_for_a_field_the_item_lacks_refuses() {
        let choice = FillChoice::Just(key_sequence::FieldRef::Custom("NOPE".to_string()));
        assert!(matches!(
            fill_action(&item_with(""), None, &choice),
            Err(sequence::Refusal::Unresolved(_))
        ));
    }

    /// `Saved` is what `fill_action` did before it took a choice at all --
    /// stored sequence, empty-sequence fallback to `Default` included. This is
    /// the claim the whole step's behaviour-preservation rests on.
    #[test]
    fn the_saved_choice_still_plans_the_items_stored_sequence() {
        let item = item_with("{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}{S:PIN}");
        let Ok(FillAction::Sequence(plan)) = fill_action(&item, None, &FillChoice::Saved) else {
            panic!("a stored sequence must plan");
        };
        let typed: Vec<String> = plan
            .steps()
            .iter()
            .filter_map(|s| match s {
                Step::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(typed, vec![USER.to_string(), format!("{PASS}4821")]);
        // And the other half of the old body: no stored sequence is still the
        // default fill, not a refusal and not an empty plan.
        assert!(matches!(
            fill_action(&item_with(""), None, &FillChoice::Saved),
            Ok(FillAction::Default)
        ));
    }

    /// The choice, not the stored sequence, is what is asked about the code.
    #[test]
    fn only_a_choice_that_uses_a_one_time_code_asks_for_one() {
        let plain = item_with("");
        assert!(needs_a_one_time_code(&plain, &FillChoice::Just(key_sequence::FieldRef::Totp)));
        let password = FillChoice::Just(key_sequence::FieldRef::Password);
        assert!(!needs_a_one_time_code(&plain, &password));
        assert!(!needs_a_one_time_code(&plain, &FillChoice::UserTabPass));
        assert!(!needs_a_one_time_code(&plain, &FillChoice::Saved));
        // The stored-sequence answer is unchanged, and disagrees with the
        // line above on the SAME choice for a different item -- so the
        // predicate is reading the item too.
        assert!(needs_a_one_time_code(&item_with("{TOTP}"), &FillChoice::Saved));
    }

    #[test]
    fn the_sequence_is_read_off_the_items_own_app_match_field() {
        assert_eq!(sequence_for(&item_with("{TAB}")), "{TAB}");
        assert_eq!(sequence_for(&item_with("")), "");
    }

    // -- fill_choices, what the overlay offers -------------------------------

    /// A one-time code seed, distinct from every other fixture value so a test
    /// that reads the wrong field cannot accidentally pass.
    const SEED: &str = "JBSWY3DPEHPK3PXP";

    /// `item_with`, stripped down to exactly the credentials named. The
    /// `deskwarden:app-match` field is kept (with `sequence`) so the
    /// stored-sequence branch is reachable, and the `PIN` custom field is kept
    /// so a test can prove custom fields do *not* become rows.
    fn item_having(
        sequence: &str,
        username: Option<&str>,
        password: Option<&str>,
        totp: Option<&str>,
    ) -> VaultItem {
        let mut item = item_with(sequence);
        item.login = Some(LoginData {
            username: username.map(|u| u.to_string()),
            password: password.map(|p| p.to_string().into()),
            totp: totp.map(|t| t.to_string().into()),
            ..LoginData::default()
        });
        item
    }

    #[test]
    fn an_item_with_both_credentials_leads_with_username_tab_password() {
        let item = item_having("", Some(USER), Some(PASS), None);
        // The fixture must actually disagree with itself: a "both" item whose
        // password is absent (or equal to the username) proves nothing.
        assert_ne!(USER, PASS);
        assert_eq!(item.login.as_ref().unwrap().username.as_deref(), Some(USER));
        assert_eq!(
            item.login.as_ref().unwrap().password.as_deref().map(|p| p.as_str()),
            Some(PASS)
        );

        let choices = fill_choices(&item);
        // FIRST, not merely present: the primary row is the one the user's
        // eye lands on, and `contains` would pass with it last.
        assert_eq!(choices.first(), Some(&FillChoice::UserTabPass));
        assert_eq!(
            choices,
            vec![
                FillChoice::UserTabPass,
                FillChoice::Just(key_sequence::FieldRef::Username),
                FillChoice::Just(key_sequence::FieldRef::Password),
            ]
        );
    }

    #[test]
    fn an_sso_item_with_only_a_username_offers_exactly_one_row() {
        // mabl's SSO screen wants the email address alone. There is no
        // password on this item at all.
        let item = item_having("", Some(USER), None, None);
        assert!(item.login.as_ref().unwrap().password.is_none());

        // `assert_eq!` on the whole vector, never `contains`: a `contains`
        // assertion passes against a set that also offers a password row this
        // item could only ever fail to fill.
        assert_eq!(
            fill_choices(&item),
            vec![FillChoice::Just(key_sequence::FieldRef::Username)]
        );
    }

    #[test]
    fn an_item_with_only_a_password_offers_exactly_the_password_row() {
        let item = item_having("", None, Some(PASS), None);
        assert!(item.login.as_ref().unwrap().username.is_none());
        assert_eq!(
            fill_choices(&item),
            vec![FillChoice::Just(key_sequence::FieldRef::Password)]
        );
    }

    #[test]
    fn an_item_with_a_totp_secret_is_offered_a_one_time_code_row() {
        let item = item_having("", Some(USER), Some(PASS), Some(SEED));
        assert_eq!(
            item.login.as_ref().unwrap().totp.as_deref().map(|t| t.as_str()),
            Some(SEED)
        );
        assert_eq!(
            fill_choices(&item),
            vec![
                FillChoice::UserTabPass,
                FillChoice::Just(key_sequence::FieldRef::Username),
                FillChoice::Just(key_sequence::FieldRef::Password),
                FillChoice::Just(key_sequence::FieldRef::Totp),
            ]
        );
    }

    #[test]
    fn an_item_with_no_totp_secret_is_never_offered_a_one_time_code() {
        // Positive control first: with a seed, the row is there. Without one
        // this assertion would be vacuous -- a `fill_choices` that never
        // offered TOTP at all would pass the negative half.
        assert!(fill_choices(&item_having("", Some(USER), Some(PASS), Some(SEED)))
            .contains(&FillChoice::Just(key_sequence::FieldRef::Totp)));

        for item in [
            item_having("", Some(USER), Some(PASS), None),
            item_having("", Some(USER), Some(PASS), Some("")),
        ] {
            assert!(!fill_choices(&item)
                .contains(&FillChoice::Just(key_sequence::FieldRef::Totp)));
        }
    }

    #[test]
    fn an_item_with_a_stored_sequence_offers_only_the_saved_sequence() {
        // The user wrote a sequence precisely because the generic rows were
        // not what this app wanted; offering them back is offering the thing
        // they already rejected.
        let item = item_having("{USERNAME}{TAB}{PASSWORD}", Some(USER), Some(PASS), Some(SEED));
        // The item has every credential, so the generic rows are all
        // *available* -- which is what makes their absence meaningful.
        assert_eq!(
            fill_choices(&item_having("", Some(USER), Some(PASS), Some(SEED))).len(),
            4
        );
        assert_eq!(fill_choices(&item), vec![FillChoice::Saved]);
    }

    #[test]
    fn no_item_ever_offers_more_than_four_rows() {
        let items = [
            item_having("", None, None, None),
            item_having("", Some(USER), None, None),
            item_having("", None, Some(PASS), None),
            item_having("", None, None, Some(SEED)),
            item_having("", Some(USER), Some(PASS), Some(SEED)),
            item_having("{USERNAME}", Some(USER), Some(PASS), Some(SEED)),
        ];
        let mut visited = 0;
        for item in &items {
            assert!(fill_choices(item).len() <= 4, "{:?}", fill_choices(item));
            visited += 1;
        }
        // The loop above is worthless if it ran zero times.
        assert_eq!(visited, items.len());
        assert!(visited > 0);
    }

    #[test]
    fn every_offered_row_has_a_label_no_other_row_shares() {
        // Two rows reading the same thing is a UI the user cannot use.
        let item = item_having("", Some(USER), Some(PASS), Some(SEED));
        let choices = fill_choices(&item);
        assert_eq!(choices.len(), 4, "the fixture must offer every generic row");
        let mut labels: Vec<String> = choices.iter().map(|c| c.label()).collect();
        assert!(labels.iter().all(|l| !l.is_empty()));
        labels.sort();
        let count = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), count, "duplicate label among {labels:?}");
        // And the saved row, which is never shown beside them, is distinct too.
        assert!(!labels.contains(&FillChoice::Saved.label()));
        // `Just` defers to the field's own label rather than restating it.
        assert_eq!(
            FillChoice::Just(key_sequence::FieldRef::Totp).label(),
            key_sequence::FieldRef::Totp.label()
        );
    }

    // -- needs_a_one_time_code, the HTTP decision ---------------------------

    #[test]
    fn choosing_a_one_time_code_is_what_makes_the_fill_go_and_fetch_one() {
        // A stored sequence that does NOT mention {TOTP}. This is the exact
        // case the split exists for.
        let item = item_having("{USERNAME}{TAB}{PASSWORD}", Some(USER), Some(PASS), Some(SEED));

        // The new function looks at the choice: the user asked for a code.
        assert!(needs_a_one_time_code(
            &item,
            &FillChoice::Just(key_sequence::FieldRef::Totp)
        ));
        // The old function looks only at the stored sequence and says no --
        // asserted explicitly, so this test proves the new function is not
        // the old one renamed. Were it a rename, the row would be shown,
        // clicked, and refused as Unresolved 100% of the time.
        assert!(!sequence_needs_a_one_time_code(&item));

        // And the choices that plainly need no code still need none.
        assert!(!needs_a_one_time_code(&item, &FillChoice::UserTabPass));
        assert!(!needs_a_one_time_code(
            &item,
            &FillChoice::Just(key_sequence::FieldRef::Username)
        ));
        assert!(!needs_a_one_time_code(
            &item,
            &FillChoice::Just(key_sequence::FieldRef::Password)
        ));
    }

    #[test]
    fn the_saved_choice_asks_for_a_code_exactly_when_its_sequence_uses_one() {
        let with = item_having("{USERNAME}{TAB}{TOTP}", Some(USER), Some(PASS), Some(SEED));
        let without = item_having("{USERNAME}{TAB}{PASSWORD}", Some(USER), Some(PASS), Some(SEED));
        assert!(needs_a_one_time_code(&with, &FillChoice::Saved));
        assert!(!needs_a_one_time_code(&without, &FillChoice::Saved));
        // The wrapper is exactly this question, so the two must agree.
        assert_eq!(
            needs_a_one_time_code(&with, &FillChoice::Saved),
            sequence_needs_a_one_time_code(&with)
        );
        assert_eq!(
            needs_a_one_time_code(&without, &FillChoice::Saved),
            sequence_needs_a_one_time_code(&without)
        );
    }

    #[test]
    fn a_custom_field_never_becomes_a_row() {
        // The fixture really does carry a custom field -- otherwise the
        // absence below proves nothing.
        let item = item_having("", Some(USER), Some(PASS), None);
        assert!(key_sequence::field_palette(&item)
            .contains(&key_sequence::FieldRef::Custom("PIN".into())));
        assert!(!fill_choices(&item)
            .iter()
            .any(|c| matches!(c, FillChoice::Just(key_sequence::FieldRef::Custom(_)))));
    }

    // -- fill_from_vault, the two calls -------------------------------------

    #[derive(Default)]
    struct Recorder {
        default_fills: Mutex<Vec<(isize, String, String)>>,
        sequences: Mutex<Vec<(isize, Vec<Step>)>>,
    }

    #[derive(Clone)]
    struct NoUiAutomation;
    impl UiAutomationFiller for NoUiAutomation {
        fn fill(&self, _: isize, _: &str, _: &str) -> Result<bool, String> {
            // Never succeeds, so the default path is observable at the
            // fallback -- and a sequence that wrongly went down the default
            // path would show up in `default_fills`.
            Ok(false)
        }
    }

    /// Records instead of typing. **There is no path from this test module to
    /// `SendInput`** -- the same discipline as `main.rs`'s `NeverTypes`.
    ///
    /// `reports` is what its "typing" claims to have done. The real filler
    /// says this from its typing thread once the plan has run or stopped;
    /// here it is said synchronously, so an assertion can follow the call.
    /// `default_result` is what the non-sequence path returns, so the two
    /// paths' counting can be tested apart.
    #[derive(Clone)]
    struct RecordingFiller {
        rec: Arc<Recorder>,
        reports: crate::fill_stats::FillOutcome,
        default_result: Result<(), String>,
    }
    impl SendInputFiller for RecordingFiller {
        fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String> {
            self.rec.default_fills.lock().unwrap().push((hwnd, user.into(), pass.into()));
            self.default_result.clone()
        }
        fn fill_sequence(
            &self,
            hwnd: isize,
            plan: Plan,
            guard: crate::injector::SequenceGuard,
        ) -> Result<(), String> {
            let mut guard = guard;
            guard.report(self.reports);
            // Released here, synchronously, rather than moved onto a thread:
            // these tests want the next fill to be allowed to start, and they
            // want the outcome to have reached the sink before they assert.
            drop(guard);
            self.rec.sequences.lock().unwrap().push((hwnd, plan.steps().to_vec()));
            Ok(())
        }
    }

    /// A cache holding exactly `item`.
    ///
    /// The mock server exists only to satisfy `populate_with`'s folder fetch
    /// and is **dropped before the fill runs**, so a fill that reached for
    /// the network instead of the in-memory snapshot would fail visibly
    /// rather than quietly succeed -- which is the property
    /// `fill_from_vault`'s doc claims and nothing else here checks.
    fn cache_with(item: VaultItem) -> VaultCache {
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let _ = cache.populate_with(vec![item], cache.epoch()).expect("seeds");
        assert_eq!(cache.items().len(), 1, "the cache must actually hold the item");
        cache
    }

    /// A `FillStats` on a path unique to this call, **with its parent
    /// directory created**.
    ///
    /// Both halves matter. `FillStats::new` reads whatever is already at the
    /// path, so a fixed name would let one test's recorded fill be counted by
    /// the next test to run. And `FillStats::save` writes the file but does
    /// *not* create its parent, and swallows the error by design -- so
    /// without the `create_dir_all` the count silently stays 0 and every
    /// assertion about it passes for entirely the wrong reason, the negative
    /// controls most of all. Every negative control below is paired with a
    /// positive one on the same helper for exactly that reason.
    fn scratch_stats(label: &str) -> crate::fill_stats::FillStats {
        let dir = std::env::temp_dir().join(format!(
            "deskwarden-fill-dispatch-{label}-{}-{:?}-{:?}",
            std::process::id(),
            std::thread::current().id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        std::fs::create_dir_all(&dir).expect("the stats directory is creatable");
        crate::fill_stats::FillStats::new(dir.join("stats.json"))
    }

    /// Runs one fill and hands back **all three** things it can be judged by:
    /// what the filler was asked to type, the `FillStats` it was given, and
    /// what the user was told.
    ///
    /// `reports` is what the filler's "typing" will claim to have done, and
    /// `default_result` is what the non-sequence path will return.
    ///
    /// The notices come back from a [`sequence::RecordingNotifier`] this
    /// fixture *owns and passes in*, not from a thread-local that a production
    /// notifier writes to when it happens to be compiled under `cfg(test)`.
    /// That is the whole change: the recorder is the only notifier this fill
    /// has, so there is no configuration in which it opens a window instead.
    fn fill_reporting(
        item: VaultItem,
        reports: crate::fill_stats::FillOutcome,
        default_result: Result<(), String>,
    ) -> (Arc<Recorder>, crate::fill_stats::FillStats, Vec<String>) {
        // `fill_from_vault` reaches `Injector::fill_sequence`, which contends
        // for a process-global "already typing" flag. See
        // `injector::sequence_test_lock`.
        let _serialised = crate::injector::sequence_test_lock();

        let rec = Arc::new(Recorder::default());
        let injector = Injector {
            ui: NoUiAutomation,
            fallback: RecordingFiller { rec: rec.clone(), reports, default_result },
        };
        let stats = scratch_stats("dispatch");
        let notifier = sequence::RecordingNotifier::default();
        fill_from_vault(
            &cache_with(item),
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Saved,
            &notifier,
        );
        (rec, stats, notifier.take())
    }

    fn fill_recording_stats(item: VaultItem) -> (Arc<Recorder>, crate::fill_stats::FillStats) {
        let (rec, stats, _) = fill_reporting(item, crate::fill_stats::FillOutcome::Typed, Ok(()));
        (rec, stats)
    }

    /// A fill judged by what it typed **and** what the user was told.
    fn fill_with_notices(item: VaultItem) -> (Arc<Recorder>, Vec<String>) {
        let (rec, _stats, notices) =
            fill_reporting(item, crate::fill_stats::FillOutcome::Typed, Ok(()));
        (rec, notices)
    }

    /// A filler whose typing succeeds and says so -- for the tests that are
    /// about which path a fill took rather than about what it counted.
    fn recording_filler(rec: &Arc<Recorder>) -> RecordingFiller {
        RecordingFiller {
            rec: rec.clone(),
            reports: crate::fill_stats::FillOutcome::Typed,
            default_result: Ok(()),
        }
    }

    fn fill(item: VaultItem) -> Arc<Recorder> {
        fill_recording_stats(item).0
    }

    // -- the sink: outcome in, count out ------------------------------------

    /// **The wiring between the pure decision and the file.**
    ///
    /// The positive case runs first and on the *same helper*, so the two
    /// negatives below cannot be passing because the harness records nothing
    /// at all -- the failure mode a missing parent directory produces, and the
    /// one that made an earlier "nothing was recorded" assertion meaningless.
    #[test]
    fn the_outcome_sink_records_only_an_outcome_that_counts() {
        use crate::fill_stats::FillOutcome;

        let typed = scratch_stats("sink-typed");
        fill_outcome_sink(&typed, "item-1")(FillOutcome::Typed);
        assert_eq!(typed.count("item-1"), 1, "the harness cannot record at all");

        let partial = scratch_stats("sink-partial");
        fill_outcome_sink(&partial, "item-1")(FillOutcome::Partial);
        assert_eq!(partial.count("item-1"), 0, "a half-typed sequence was counted as a fill");

        let untyped = scratch_stats("sink-untyped");
        fill_outcome_sink(&untyped, "item-1")(FillOutcome::NotTyped);
        assert_eq!(untyped.count("item-1"), 0, "a fill that typed nothing was counted");
    }

    /// **The sink credits the item it was handed**, and only that one.
    ///
    /// The id here is deliberately not the one every other fixture in this
    /// module uses, so a call site that passed the wrong string -- the item's
    /// *name*, a hard-coded id, an empty one -- fails here rather than
    /// blending in.
    #[test]
    fn the_outcome_sink_credits_the_item_it_was_given() {
        let stats = scratch_stats("sink-id");
        fill_outcome_sink(&stats, "item-7")(crate::fill_stats::FillOutcome::Typed);

        assert_eq!(stats.count("item-7"), 1, "the item the sink was built for was not credited");
        assert_eq!(stats.count("item-1"), 0, "some other item was credited instead");
    }

    /// The sink owns its inputs outright, so the typing thread that runs it
    /// borrows nothing the UI owns. If this ever stops compiling, the sink has
    /// grown a lifetime and the typing thread has grown a way to outlive what
    /// it points at.
    #[test]
    fn the_outcome_sink_can_be_moved_onto_another_thread() {
        let stats = scratch_stats("sink-thread");
        let sink = fill_outcome_sink(&stats, "item-1");
        std::thread::spawn(move || sink(crate::fill_stats::FillOutcome::Typed))
            .join()
            .expect("the reporting thread finished");

        assert_eq!(stats.count("item-1"), 1);
    }

    /// **A sequence that typed counts as a fill.**
    ///
    /// `record_fill` is what the picker orders its suggestions by, so an item
    /// filled only ever through the sequence path would stay at the bottom of
    /// the list forever. Passing `Box::new(|_| {})` to `fill_sequence` instead
    /// of `fill_outcome_sink(fill_stats, item_id)` fails here.
    ///
    /// What "typed" means is the filler's word, reported through the guard --
    /// which is the whole change. The `Ok(())` this call returns means the
    /// typing *started*, and nothing counts a fill off it any more.
    #[test]
    fn a_sequence_that_typed_is_recorded_against_the_item() {
        let (rec, stats) = fill_recording_stats(item_with("{USERNAME}{TAB}{PASSWORD}"));
        assert_eq!(
            rec.sequences.lock().unwrap().len(),
            1,
            "this test is not exercising the sequence path"
        );
        assert_eq!(stats.count("item-1"), 1, "a sequence fill was not recorded");
        // The count is against *this* item and is not a blanket increment.
        assert_eq!(stats.count("item-2"), 0, "an unrelated item was credited");
    }

    /// **A sequence abandoned when the user alt-tabbed is not a fill.**
    ///
    /// The case the threaded design exists to handle, and the one the old
    /// `Ok(()) => record_fill(item_id)` arm got wrong: the typing thread
    /// started, so dispatch returned `Ok(())`, so the item was credited with a
    /// password it never typed and climbed the picker for it. Everything about
    /// this run is identical to the test above except the outcome reported.
    #[test]
    fn a_sequence_abandoned_part_way_is_not_recorded_as_a_fill() {
        let (rec, stats, _notices) = fill_reporting(
            item_with("{USERNAME}{TAB}{PASSWORD}"),
            crate::fill_stats::FillOutcome::Partial,
            Ok(()),
        );
        assert_eq!(
            rec.sequences.lock().unwrap().len(),
            1,
            "this test is not exercising the sequence path"
        );
        assert_eq!(stats.count("item-1"), 0, "a half-typed sequence was counted as a fill");
    }

    /// **A sequence that reached the filler and typed nothing is not a fill.**
    /// The runner's foreground check refusing on the very first step, or a
    /// filler that could not restore foreground at all.
    #[test]
    fn a_sequence_that_typed_nothing_is_not_recorded_as_a_fill() {
        let (rec, stats, _notices) = fill_reporting(
            item_with("{USERNAME}{TAB}{PASSWORD}"),
            crate::fill_stats::FillOutcome::NotTyped,
            Ok(()),
        );
        assert_eq!(rec.sequences.lock().unwrap().len(), 1, "the sequence path was not reached");
        assert_eq!(stats.count("item-1"), 0, "a sequence that typed nothing was counted");
    }

    /// The other negative control: a sequence refused at *plan* time never
    /// reaches the filler at all, so nothing is there to report an outcome and
    /// nothing may be counted.
    #[test]
    fn a_refused_sequence_is_not_recorded_as_a_fill() {
        // `{PICKCHARS}` is unimplemented, so `fill_action` refuses at plan
        // time and the filler is never reached.
        let (rec, stats) = fill_recording_stats(item_with("{USERNAME}{PICKCHARS}"));
        assert!(
            rec.sequences.lock().unwrap().is_empty(),
            "a refused sequence still reached the filler"
        );
        assert_eq!(stats.count("item-1"), 0, "a refused sequence was counted as a fill");
    }

    /// **The default path's counting is exactly what it was.** `Injector::fill`
    /// is synchronous and its return value really does mean "typed", so a
    /// successful default fill is still recorded -- routing it through the
    /// shared sink must not have quietly changed that.
    #[test]
    fn a_default_fill_is_still_recorded_against_the_item() {
        let (rec, stats) = fill_recording_stats(item_with(""));
        assert_eq!(
            rec.default_fills.lock().unwrap().len(),
            1,
            "this test is not exercising the default path"
        );
        assert_eq!(stats.count("item-1"), 1, "a default fill was not recorded");
    }

    /// And its negative control, which the default path always had implicitly:
    /// a fill that returned an error is not a fill. Reporting `Typed`
    /// regardless -- the shape of the sequence bug, transplanted -- fails here.
    #[test]
    fn a_default_fill_that_failed_is_not_recorded_as_a_fill() {
        let (rec, stats, _notices) = fill_reporting(
            item_with(""),
            crate::fill_stats::FillOutcome::Typed,
            Err("target window is not foreground".into()),
        );
        assert_eq!(rec.default_fills.lock().unwrap().len(), 1, "the default path was not reached");
        assert_eq!(stats.count("item-1"), 0, "a failed default fill was counted as a fill");
    }

    /// **Delete `injector.fill_sequence(hwnd, plan)` from `fill_from_vault`
    /// and this is the test that fails.**
    #[test]
    fn an_item_with_a_sequence_is_typed_through_the_sequence_path() {
        let rec = fill(item_with("{USERNAME}{ENTER}{DELAY 2000}{PASSWORD}"));

        let sequences = rec.sequences.lock().unwrap();
        assert_eq!(sequences.len(), 1, "the sequence path was not reached");
        assert_eq!(sequences[0].0, 4242, "the plan went to the wrong window");
        // The plan is the item's own, not some other sequence: substituting
        // `DEFAULT_SEQUENCE` here fails on both the shape and the values.
        assert_eq!(
            sequences[0].1.iter().filter(|s| matches!(s, Step::Wait(_))).count(),
            1,
            "the delay did not survive into the plan"
        );
        assert!(
            sequences[0]
                .1
                .contains(&Step::Text { text: PASS.into(), rate: sequence::DEFAULT_RATE }),
            "the item's own password is not in the plan: {:?}",
            sequences[0].1
        );
        assert!(
            rec.default_fills.lock().unwrap().is_empty(),
            "a sequenced item also took the default fill"
        );
    }

    /// **Delete `injector.fill(hwnd, &username, &password)` and this fails.**
    /// The existing behaviour, unchanged, for every item in every existing
    /// vault.
    #[test]
    fn an_item_without_a_sequence_still_takes_the_original_fill() {
        let rec = fill(item_with(""));

        assert_eq!(
            *rec.default_fills.lock().unwrap(),
            vec![(4242, USER.to_string(), PASS.to_string())]
        );
        assert!(
            rec.sequences.lock().unwrap().is_empty(),
            "an item with no sequence went down the sequence path"
        );
    }

    /// **`fill_from_vault` really does fetch a one-time code and hand it to
    /// the plan.** Stubbing the fetch out (`let totp = if false`) leaves
    /// `the_one_time_code_is_handed_to_the_plan` and
    /// `only_a_sequence_that_uses_a_one_time_code_asks_for_one` green, because
    /// one tests `fill_action` with a code it was handed and the other tests
    /// the predicate -- neither is the *act*. This one is: it needs the code
    /// to come off the wire and end up in the plan, so it fails.
    ///
    /// The mock server is alive for the whole fill here (unlike `fill`'s),
    /// which is the point -- the fetch is the one part of a fill that is
    /// deliberately allowed to touch the network.
    #[test]
    fn a_sequence_that_uses_a_one_time_code_fetches_it_and_types_it() {
        // Contends for the same process-global "already typing" flag as the
        // fills above (see `injector::sequence_test_lock`). Without this the
        // sequence assertion below fails at random when an unrelated test
        // happens to be holding a `SequenceGuard`.
        let _serialised = crate::injector::sequence_test_lock();
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        let totp = server
            .mock("GET", "/object/totp/item-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":"482913"}}"#)
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let _ = cache
            .populate_with(vec![item_with("{USERNAME}{TAB}{TOTP}")], cache.epoch())
            .expect("seeds");

        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = crate::fill_stats::FillStats::new(
            std::env::temp_dir().join("deskwarden-fill-totp").join("stats.json"),
        );
        fill_from_vault(
            &cache,
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Saved,
            &sequence::RecordingNotifier::default(),
        );

        totp.assert();
        let sequences = rec.sequences.lock().unwrap();
        assert_eq!(sequences.len(), 1, "the sequence path was not reached");
        assert!(
            sequences[0].1.contains(&Step::Text {
                text: "482913".to_string(),
                rate: sequence::DEFAULT_RATE
            }),
            "the fetched code is not in the plan: {:?}",
            sequences[0].1
        );
    }

    /// Positive control for the test above: an item whose sequence does not
    /// mention `{TOTP}` must not pay for the round trip at all.
    #[test]
    fn a_sequence_without_a_one_time_code_makes_no_totp_request() {
        // Contends for the same process-global "already typing" flag as the
        // fills above (see `injector::sequence_test_lock`). Without this the
        // sequence assertion below fails at random when an unrelated test
        // happens to be holding a `SequenceGuard`.
        let _serialised = crate::injector::sequence_test_lock();
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        let totp = server.mock("GET", "/object/totp/item-1").with_status(200).create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let _ = cache
            .populate_with(vec![item_with("{USERNAME}{TAB}{PASSWORD}")], cache.epoch())
            .expect("seeds");

        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = crate::fill_stats::FillStats::new(
            std::env::temp_dir().join("deskwarden-fill-nototp").join("stats.json"),
        );
        fill_from_vault(
            &cache,
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Saved,
            &sequence::RecordingNotifier::default(),
        );

        assert_eq!(rec.sequences.lock().unwrap().len(), 1);
        totp.expect(0).assert();
    }

    /// **The CHOICE is what makes the fill fetch a code.**
    ///
    /// The item here stores no sequence at all, so
    /// `sequence_needs_a_one_time_code` answers `false` about it. Gating the
    /// fetch on that older, sequence-only question -- which is what
    /// `fill_from_vault` did before this step -- leaves `Just(Totp)` with
    /// `totp: None`, and the one-time-code row then refuses with `Unresolved`
    /// one hundred percent of the time while every test of `fill_action` and
    /// of the predicate stays green. Only the act catches it.
    #[test]
    fn a_choice_that_needs_a_code_is_what_makes_the_fill_fetch_one() {
        let _serialised = crate::injector::sequence_test_lock();
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        let totp = server
            .mock("GET", "/object/totp/item-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":"907142"}}"#)
            .create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        // Deliberately no stored sequence -- see the doc above.
        let _ = cache.populate_with(vec![item_with("")], cache.epoch()).expect("seeds");
        assert!(
            !sequence_needs_a_one_time_code(&item_with("")),
            "the fixture stores a {{TOTP}}, so the old gate would have fetched anyway and this \
             test would prove nothing"
        );

        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("choice-totp");
        fill_from_vault(
            &cache,
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Just(key_sequence::FieldRef::Totp),
            &sequence::RecordingNotifier::default(),
        );

        totp.assert();
        let sequences = rec.sequences.lock().unwrap();
        assert_eq!(sequences.len(), 1, "the sequence path was not reached");
        assert_eq!(
            sequences[0].1,
            [Step::Text { text: "907142".to_string(), rate: sequence::DEFAULT_RATE }],
            "the fetched code is not what was typed"
        );
    }

    /// The negative control for the test above, on the **same item**: a choice
    /// that needs no code pays for no round trip. Without this, "the fetch
    /// happens" is also what a `fill_from_vault` that fetches unconditionally
    /// reports -- and that version would put an HTTP request on the path this
    /// app deliberately serves from the in-memory cache.
    #[test]
    fn a_choice_that_needs_no_code_makes_no_totp_request() {
        let _serialised = crate::injector::sequence_test_lock();
        let mut server = mockito::Server::new();
        let _folders = server
            .mock("GET", "/list/object/folders")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"data":{"data":[]}}"#)
            .create();
        let totp = server.mock("GET", "/object/totp/item-1").with_status(200).create();

        let cache = VaultCache::new(VaultBridge::new(server.url()));
        let _ = cache.populate_with(vec![item_with("")], cache.epoch()).expect("seeds");

        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("choice-nototp");
        fill_from_vault(
            &cache,
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Just(key_sequence::FieldRef::Password),
            &sequence::RecordingNotifier::default(),
        );

        // Positive control: the fill really ran and really typed, so
        // "no request" cannot mean "nothing happened".
        let sequences = rec.sequences.lock().unwrap();
        assert_eq!(sequences.len(), 1, "the fill never reached the sequence path");
        assert_eq!(
            sequences[0].1,
            [Step::Text { text: PASS.to_string(), rate: sequence::DEFAULT_RATE }]
        );
        totp.expect(0).assert();
    }

    /// **`UserTabPass` reaches the UI-Automation-first path, not the runner.**
    ///
    /// `fill_action`'s unit test says `Ok(FillAction::Default)`; this says the
    /// fill ACTED on it. A `fill_from_vault` that mapped the default choice
    /// through the sequence runner would still type a username and a password
    /// into the window, so every "was something typed" assertion in this
    /// module stays green -- what changes is that it arrives as keystrokes at
    /// whatever has focus instead of through named UIA fields.
    #[test]
    fn the_default_choice_reaches_the_default_fill_and_not_the_runner() {
        let _serialised = crate::injector::sequence_test_lock();
        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("choice-default");
        // A stored sequence, so `Saved` here would take the OTHER arm: the
        // fixture disagrees with itself between the two choices.
        let item = item_with("{USERNAME}{TAB}{PASSWORD}");
        fill_from_vault(
            &cache_with(item),
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::UserTabPass,
            &sequence::RecordingNotifier::default(),
        );
        assert_eq!(
            *rec.default_fills.lock().unwrap(),
            vec![(4242, USER.to_string(), PASS.to_string())],
            "the default choice did not go through the default fill"
        );
        assert!(
            rec.sequences.lock().unwrap().is_empty(),
            "the default choice was planned as a sequence, which deletes the UI Automation path"
        );
    }

    /// A refused sequence types **nothing at all** -- not the sequence, and
    /// not the default fill it might have degraded to.
    #[test]
    fn a_refused_sequence_types_nothing_by_either_path() {
        let rec = fill(item_with("{USERNAME}{PICKCHARS}{PASSWORD}"));
        assert!(rec.sequences.lock().unwrap().is_empty());
        assert!(rec.default_fills.lock().unwrap().is_empty());
    }

    /// **The refusal reaches the user, not only the log.** Deleting the
    /// `notifier.refused(&message)` line leaves every other test in this
    /// module green and fails only here -- which is the whole point of it
    /// being a separate assertion.
    #[test]
    fn a_refused_sequence_tells_the_user_which_construct_stopped_it() {
        let (_rec, notices) = fill_with_notices(item_with("{USERNAME}{PICKCHARS}{PASSWORD}"));
        assert_eq!(notices.len(), 1, "the user was not told: {notices:?}");
        assert!(
            notices[0].contains("{PICKCHARS}"),
            "the notice must name the construct: {}",
            notices[0]
        );
    }

    /// Positive control for the test above: a fill that succeeds says nothing.
    /// Without this, a notifier that fired on every fill would pass.
    #[test]
    fn a_fill_that_works_does_not_interrupt_the_user() {
        assert!(fill_with_notices(item_with("{USERNAME}{TAB}{PASSWORD}")).1.is_empty());
        assert!(fill_with_notices(item_with("")).1.is_empty());
    }

    /// Runs one **default** fill while something else genuinely holds the
    /// one-fill-at-a-time flag, and hands back everything that fill can be
    /// judged by.
    ///
    /// The flag is taken for real rather than faked by a filler that returns
    /// the sentence: what is under test is the arm's response to
    /// `Injector::fill`'s *own* refusal, and a fixture that manufactures the
    /// string would keep passing if the guard were removed from the default
    /// path altogether -- which is the regression `d6ff857` exists to prevent.
    ///
    /// It does its own locking rather than going through `fill_reporting`,
    /// because the guard has to be held *across* the fill and
    /// `sequence_test_lock` is not reentrant. Both are taken here in the same
    /// order every other test in the crate takes them: the test lock first,
    /// so a test that acquires the flag without it cannot race this one.
    fn fill_while_something_else_is_typing(
        item: VaultItem,
    ) -> (Arc<Recorder>, crate::fill_stats::FillStats, Vec<String>) {
        let _serialised = crate::injector::sequence_test_lock();
        let held =
            crate::injector::SequenceGuard::acquire().expect("nothing else holds the flag");
        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("refused-default");
        let notifier = sequence::RecordingNotifier::default();
        fill_from_vault(
            &cache_with(item),
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Saved,
            &notifier,
        );
        drop(held);
        (rec, stats, notifier.take())
    }

    /// **A refused default fill reaches the user, exactly as a refused
    /// sequence does.**
    ///
    /// This is the whole asymmetry: `d6ff857` gave both fill paths the guard,
    /// so both can refuse -- but only the Sequence arm told anyone. A default
    /// fill that quietly does nothing is indistinguishable from a hotkey that
    /// never registered, and the user's next move differs completely between
    /// the two.
    #[test]
    fn a_refused_default_fill_tells_the_user_something_is_already_typing() {
        let (rec, _stats, notices) = fill_while_something_else_is_typing(item_with(""));

        // Nothing was typed by either path: the fixture really did refuse.
        assert!(rec.default_fills.lock().unwrap().is_empty());
        assert!(rec.sequences.lock().unwrap().is_empty());

        assert_eq!(notices.len(), 1, "the user was not told: {notices:?}");
        assert_eq!(notices[0], crate::injector::ALREADY_TYPING);
        // The sentence must be actionable, for the same reason the sequence
        // path's is: a bare "no" is the thing being ruled out.
        assert!(notices[0].contains("press the hotkey again"), "got: {}", notices[0]);
    }

    /// **The third refusal call site, which nothing used to reach.**
    ///
    /// `fill_from_vault` tells the user in three places -- the default arm's
    /// `ALREADY_TYPING`, the planning refusal, and the `Err` of
    /// `fill_sequence` -- and only the first two were pinned. Deleting
    /// `notifier.refused(&e)` from the Sequence arm left the whole suite
    /// green, so a sequence fill that was refused because something else was
    /// already typing said nothing at all to the user while the default fill
    /// for the same item said the same sentence the same way.
    ///
    /// It goes through the **real** refusal rather than a fixture that returns
    /// an error: `Injector::fill_sequence` acquires the one-at-a-time flag
    /// itself, so holding it is what production holding it looks like, and a
    /// manufactured `Err` would keep passing if that guard were removed.
    #[test]
    fn a_sequence_refused_because_something_else_is_typing_tells_the_user_too() {
        let (rec, _stats, notices) =
            fill_while_something_else_is_typing(item_with("{USERNAME}{TAB}{PASSWORD}"));

        // Positive control: the fill really did take the sequence path and
        // really was stopped before it typed anything by either route.
        assert!(rec.sequences.lock().unwrap().is_empty());
        assert!(rec.default_fills.lock().unwrap().is_empty());

        assert_eq!(notices.len(), 1, "the user was not told: {notices:?}");
        assert_eq!(notices[0], crate::injector::ALREADY_TYPING);
    }

    /// A filler whose sequence path fails the way the real one's single
    /// pre-keystroke failure fails: `ensure_foreground` gave up, the guard is
    /// told nothing was typed, and the message is the Win32 diagnostic.
    ///
    /// Written out rather than added as a field on `RecordingFiller`, so that
    /// the shape under test is visibly `RealSendInput::fill_sequence`'s early
    /// return and not a manufactured error in a fixture that also does five
    /// other things.
    struct ForegroundLostFiller {
        rec: Arc<Recorder>,
    }
    impl SendInputFiller for ForegroundLostFiller {
        fn fill(&self, hwnd: isize, user: &str, pass: &str) -> Result<(), String> {
            self.rec.default_fills.lock().unwrap().push((hwnd, user.into(), pass.into()));
            Ok(())
        }
        fn fill_sequence(
            &self,
            _hwnd: isize,
            plan: Plan,
            guard: crate::injector::SequenceGuard,
        ) -> Result<(), String> {
            let mut guard = guard;
            guard.report(crate::fill_stats::FillOutcome::NotTyped);
            drop(guard);
            drop(plan);
            Err("refusing to type: target window 4242 is not foreground \
                 (foreground is 99) after 5 attempts"
                .to_string())
        }
    }

    /// **The two arms apply the same rule to the same error class.**
    ///
    /// `a_default_fill_that_failed_for_another_reason_opens_no_dialog` is this
    /// test's twin on the other arm, and for one release the two arms
    /// disagreed: the Default arm filtered to [`ALREADY_TYPING`] on the stated
    /// grounds that a Win32 foreground diagnostic is "an expensive way to tell
    /// a user something they cannot act on", while the Sequence arm called
    /// `notifier.refused(&e)` on every `Err` -- including that exact
    /// diagnostic, as an `MB_TOPMOST` task-modal box, in the moments right
    /// after our own overlay closes, which the sequence path's own doc calls
    /// the common case.
    ///
    /// What is **not** given up is the report the user actually needs. A run
    /// abandoned mid-sequence, with the username already on screen, does not
    /// return through `fill_sequence` at all -- `Ok(())` there means "the
    /// thread started" -- and is reported by `injector::perform`, which is
    /// untouched. `injector::orchestration_tests` pins that half.
    #[test]
    fn a_sequence_that_failed_for_another_reason_opens_no_dialog() {
        let _serialised = crate::injector::sequence_test_lock();

        let rec = Arc::new(Recorder::default());
        let injector =
            Injector { ui: NoUiAutomation, fallback: ForegroundLostFiller { rec: rec.clone() } };
        let stats = scratch_stats("sequence-diagnostic");
        let notifier = sequence::RecordingNotifier::default();
        fill_from_vault(
            &cache_with(item_with("{USERNAME}{TAB}{PASSWORD}")),
            &injector,
            &stats,
            "item-1",
            4242,
            FillChoice::Saved,
            &notifier,
        );

        // Positive control: the fill really did take the sequence path and
        // really did fail there. Without this, "no dialog" is also what a
        // fill that never ran reports.
        assert!(
            rec.default_fills.lock().unwrap().is_empty(),
            "the fill took the default arm, so this says nothing about the sequence arm"
        );
        assert_eq!(stats.count("item-1"), 0, "a failed sequence was counted");

        assert!(
            notifier.take().is_empty(),
            "a Win32 foreground diagnostic was raised as a task-modal box on the sequence \
             arm, which is exactly what the default arm refuses to do with the same error"
        );
    }

    /// And the accounting half of it: a sequence refused before it started is
    /// not a fill, exactly as a refused default fill is not.
    #[test]
    fn a_sequence_refused_before_it_started_counts_no_fill() {
        let (_rec, stats, _notices) =
            fill_while_something_else_is_typing(item_with("{USERNAME}{TAB}{PASSWORD}"));
        assert_eq!(stats.count("item-1"), 0, "a refused sequence was counted");
    }

    /// **A refused default fill still records nothing**, which is the half of
    /// this that must not have moved. The notifier is a side channel; the
    /// count is the accounting, and a refusal is not a fill.
    #[test]
    fn a_refused_default_fill_counts_no_fill() {
        let (_rec, stats, _notices) = fill_while_something_else_is_typing(item_with(""));
        assert_eq!(stats.count("item-1"), 0, "a refused fill was counted");
    }

    /// **Negative control, and the reason the arm tests the error rather than
    /// notifying on every one.**
    ///
    /// A default fill fails for reasons that are not refusals -- the
    /// foreground moved between the match and the keystroke, a `SendInput`
    /// call returned 0. Those are logged, they count nothing, and they are
    /// diagnostics a modal box could tell the user nothing useful about. Only
    /// the refusal has a sentence written for a person and a way out of the
    /// state it names.
    #[test]
    fn a_default_fill_that_failed_for_another_reason_opens_no_dialog() {
        let (rec, stats, notices) = fill_reporting(
            item_with(""),
            crate::fill_stats::FillOutcome::Typed,
            Err("SendInput delivered 0 of 2 events".to_string()),
        );

        // Positive control: the fill really was attempted and really did fail.
        assert_eq!(rec.default_fills.lock().unwrap().len(), 1);
        assert_eq!(stats.count("item-1"), 0, "a failed fill was counted");
        assert!(notices.is_empty(), "a Win32 diagnostic was shown to the user");
    }

    /// **The default arm's own copy of the password does not reach the
    /// allocator in the clear.**
    ///
    /// `LoginData::password` is already a `Zeroizing<String>`, so every clone
    /// of the cached item wipes itself -- which left exactly one plaintext
    /// copy on this path: the `String` `credentials_for` builds out of it,
    /// live for the whole default arm. That copy is what this watches, and it
    /// is why the assertion below is meaningful rather than being satisfied by
    /// the wipes that were already there: replace the `Zeroizing::new` in
    /// `fill_from_vault`'s Default arm with the bare `password` and this
    /// fails, with the rest of the module green.
    ///
    /// The item is built here rather than by `item_with` so the password is
    /// the probe string; the cache and the fixture are built **before** the
    /// watch is armed, so what the watch sees is only what the fill itself
    /// released.
    #[test]
    fn a_default_fill_does_not_release_the_password_in_the_clear() {
        use crate::login_ui::password_lifetime_tests::{plaintext_reached_the_allocator, PROBE};

        // **The instrument is awake, in this thread and in this direction.**
        // `refusal_lifetime_tests` opens with exactly this line and says why:
        // without it, a probe that had gone deaf makes the assertion below
        // pass by saying nothing. This test relied instead on "the fill
        // really ran", which is a different fact -- a fill can run in full
        // while the watch reports on nothing at all. A deaf instrument
        // reporting clean is the exact failure this suite exists to catch,
        // and the one shape it cannot catch in itself.
        let bare = String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8");
        assert!(
            plaintext_reached_the_allocator(move || drop(bare)),
            "the probe cannot see an unwiped password, so this test proves nothing"
        );

        let _serialised = crate::injector::sequence_test_lock();

        let mut item = item_with("");
        item.login.as_mut().expect("the fixture has a login").password =
            Some(String::from_utf8(PROBE.as_bytes().to_vec()).expect("PROBE is UTF-8").into());

        let cache = cache_with(item);
        let rec = Arc::new(Recorder::default());
        let injector = Injector { ui: NoUiAutomation, fallback: recording_filler(&rec) };
        let stats = scratch_stats("default-lifetime");

        let notifier = sequence::RecordingNotifier::default();
        let leaked = plaintext_reached_the_allocator(|| {
            fill_from_vault(
                &cache,
                &injector,
                &stats,
                "item-1",
                4242,
                FillChoice::Saved,
                &notifier,
            );
        });

        // Positive control: the fill really did take the default path and
        // really did handle the probe, so a `false` above cannot mean "nothing
        // happened".
        assert_eq!(rec.default_fills.lock().unwrap().len(), 1);
        assert!(!leaked, "the default fill freed the password in the clear");
    }
}
